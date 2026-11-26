// SPDX-License-Identifier: GPL-3.0-or-later
//! `media.import_images_as_slideshow` (WP-2F.05d, ADR-0035 item 3): pictures imported as stills
//! and placed one after the other on a track, in one transaction (one undo step).
//!
//! - **Import.** Every file is probed like `media.import` does (the caller's allowlist, the
//!   timeout, in parallel). A picture whose content is already in the project reuses that asset
//!   (the content hash decides). A file that cannot be read, or that is not a still picture, is
//!   listed in `failed` and left out; when no file is a picture the call is `Invalid`.
//! - **Order.** `given` keeps the order of `files`; `name` sorts by file name without regard to
//!   case; `name_numeric` also compares the numbers in the names by value (`img2` before
//!   `img10`).
//! - **Placement.** Each picture is one `timeline.insert` of `per_image` (default: the
//!   project's still duration, a whole number of frames), the first at `at` (default: the end of
//!   the track) and each next one where the one before ends. `timeline.insert`'s own default
//!   mode applies: on the magnetic main track what is after `at` moves later, on any other track
//!   the range must be free.
//! - **One transaction.** The registrations and the inserts go through
//!   [`MediaSink::slideshow`](super::MediaSink::slideshow); a sink runs [`place_slideshow`]
//!   inside its transaction, so a refused insert leaves no asset behind.

use std::cmp::Ordering;

use cutweave_commands::{CmdError, Command, EngineCommand, Tx, command, wire};
use cutweave_jobs::JobId;
use cutweave_model::{
    Duration, EntityKind, ItemId, MediaId, MediaKind, Project, SequenceId, Time, TimeInput,
    TrackId, TrackKind,
};

use super::{
    AnalysisInput, ImportCtx, ImportFailure, ImportFailureCode, ImportPolicy, ImportProxy,
    ImportedMedia, MAX_IMPORT_PATHS, MediaRegister, MediaRegistered, OUTSIDE_ROOTS_HINT, failure,
    policy,
};
use crate::timeline::{TimelineInsert, TimelineInsertOutput};
use crate::util::check_abs_path;

/// The order the pictures of a slideshow are placed in.
#[wire]
#[derive(Copy, Default, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SlideshowOrder {
    /// As listed in `files`.
    #[default]
    Given,
    /// By file name, ignoring case.
    Name,
    /// By file name, ignoring case, numbers compared by value (`img2` before `img10`).
    NameNumeric,
}

/// Result of `media.import_images_as_slideshow`.
#[wire]
pub struct MediaSlideshowOutput {
    /// The sequence the pictures went into.
    pub sequence: SequenceId,
    /// The track they are on.
    pub track: TrackId,
    /// Where the first picture starts (snapped to the frame grid).
    pub at: Time,
    /// How long each picture stays (a whole number of frames).
    pub per_image: Duration,
    /// Where the last picture ends.
    pub end: Time,
    /// The new clips, in slideshow order.
    pub clips: Vec<ItemId>,
    /// One entry per placed picture, in slideshow order.
    pub media: Vec<ImportedMedia>,
    /// The assets of `media`, in the same order (a duplicate names the existing asset).
    pub media_ids: Vec<MediaId>,
    /// The files that were left out: unreadable, or not a still picture.
    pub failed: Vec<ImportFailure>,
    /// Background analysis jobs queued for the new assets (wait with `job.wait`).
    pub job_ids: Vec<JobId>,
}

/// Imports pictures as stills and places them one after the other on a video track, each
/// `per_image` long (default: the project's still duration), as one undo step. Pictures whose
/// content is already in the project reuse that asset. Files that cannot be read or are not
/// still pictures are listed in `failed` and left out; with no picture at all the call fails
/// with `Invalid`. `order` is `given` (default), `name` or `name_numeric`. The pictures go to
/// `track` (default: the main video track, else the first unlocked video track) of `sequence`
/// (optional when the project has one), starting at `at` (default: the end of that track). On
/// the magnetic main track what is after `at` moves later; on another track the range must be
/// free. Agents and headless callers may only import from the project folder and user-approved
/// folders.
#[command(
    name = "media.import_images_as_slideshow",
    summary = "Import pictures as stills and place them one after the other on a track",
    engine,
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct MediaImportImagesAsSlideshow {
    /// Absolute paths of the pictures (1..=256, no repeats).
    pub files: Vec<String>,
    /// How long each picture stays, `> 0` (default: the project's still duration).
    pub per_image: Option<TimeInput>,
    /// `given` (default), `name` or `name_numeric`.
    pub order: Option<SlideshowOrder>,
    /// The sequence; optional when the project has one.
    pub sequence: Option<SequenceId>,
    /// A video track (default: the main video track, else the first unlocked one).
    pub track: Option<TrackId>,
    /// Where the first picture starts (default: the end of the track).
    pub at: Option<TimeInput>,
}

/// What a slideshow puts into the document in one transaction ([`place_slideshow`]).
#[derive(Clone, Debug)]
pub struct SlideshowBatch {
    /// The probed pictures, in slideshow order.
    pub pictures: Vec<MediaRegister>,
    /// The sequence; `None`: the project's only one.
    pub sequence: Option<SequenceId>,
    /// The track; `None`: the main video track, else the first unlocked video track.
    pub track: Option<TrackId>,
    /// Where the first picture starts; `None`: the end of the track.
    pub at: Option<TimeInput>,
    /// The length of each picture; `None`: the project's still duration.
    pub per_image: Option<TimeInput>,
}

/// What [`place_slideshow`] did.
#[derive(Clone, Debug)]
pub struct SlideshowPlaced {
    /// One result per picture, in order.
    pub registered: Vec<MediaRegistered>,
    /// The sequence.
    pub sequence: SequenceId,
    /// The track.
    pub track: TrackId,
    /// Where the first picture starts.
    pub at: Time,
    /// How long each picture stays.
    pub per_image: Duration,
    /// Where the last picture ends.
    pub end: Time,
    /// The clips, in order.
    pub clips: Vec<ItemId>,
}

/// The document commands a slideshow is made of, as the transaction of a
/// [`MediaSink`](super::MediaSink) runs them: [`Tx`] implements it, and the engine implements
/// it by submitting `media.register` and `timeline.insert` into one batch transaction.
pub trait SlideshowSteps {
    /// The document as the transaction sees it now.
    fn doc(&self) -> Project;

    /// `media.register`.
    fn register(&mut self, register: &MediaRegister) -> Result<MediaRegistered, CmdError>;

    /// `timeline.insert`.
    fn insert(&mut self, insert: &TimelineInsert) -> Result<TimelineInsertOutput, CmdError>;
}

impl SlideshowSteps for Tx<'_> {
    fn doc(&self) -> Project {
        Tx::doc(self).clone()
    }

    fn register(&mut self, register: &MediaRegister) -> Result<MediaRegistered, CmdError> {
        register.validate(&self.query_ctx())?;
        register.apply(self)
    }

    fn insert(&mut self, insert: &TimelineInsert) -> Result<TimelineInsertOutput, CmdError> {
        insert.validate(&self.query_ctx())?;
        insert.apply(self)
    }
}

/// The sequence, the track and the start of a slideshow in `doc`.
fn resolve(
    doc: &Project,
    batch: &SlideshowBatch,
) -> Result<(SequenceId, TrackId, TimeInput), CmdError> {
    let (sequence_id, sequence) = match batch.sequence {
        Some(id) => doc
            .sequences
            .get(&id)
            .map(|s| (id, s))
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, id))?,
        None => {
            let mut all = doc.sequences.iter();
            match (all.next(), all.next()) {
                (Some((id, s)), None) => (*id, s),
                (None, _) => {
                    return Err(CmdError::invalid(
                        "/sequence",
                        "the project has no sequence",
                    ));
                }
                (Some(_), Some(_)) => {
                    return Err(CmdError::invalid(
                        "/sequence",
                        format!(
                            "the project has {} sequences; name one",
                            doc.sequences.len()
                        ),
                    ));
                }
            }
        }
    };
    let track = match batch.track {
        Some(id) => match sequence.track(&id) {
            Some(track) => track,
            None if doc.sequences.values().any(|s| s.track(&id).is_some()) => {
                return Err(CmdError::invalid(
                    "/track",
                    format!("track `{id}` is not in sequence `{sequence_id}`"),
                ));
            }
            None => return Err(CmdError::not_found(EntityKind::Track, id)),
        },
        None => {
            let usable = |magnetic: bool| {
                sequence
                    .tracks_in_order()
                    .find(|t| t.kind == TrackKind::Video && !t.locked && t.magnetic == magnetic)
            };
            usable(true).or_else(|| usable(false)).ok_or_else(|| {
                CmdError::invalid(
                    "/track",
                    "the sequence has no unlocked video track for the pictures; add or unlock one",
                )
            })?
        }
    };
    let at = batch
        .at
        .unwrap_or_else(|| TimeInput::from(track.items.end()));
    Ok((sequence_id, track.id, at))
}

/// Registers the pictures of `batch` and inserts them one after the other. For
/// [`MediaSink::slideshow`](super::MediaSink::slideshow) implementations: run it inside one
/// transaction and cancel that transaction when it fails. Error pointers name the fields of
/// `media.import_images_as_slideshow`; a picture is `/pictures/<index in the batch>`.
pub fn place_slideshow(
    steps: &mut dyn SlideshowSteps,
    batch: &SlideshowBatch,
) -> Result<SlideshowPlaced, CmdError> {
    let (sequence, track, first_at) = resolve(&steps.doc(), batch)?;
    let mut registered = Vec::with_capacity(batch.pictures.len());
    for (i, picture) in batch.pictures.iter().enumerate() {
        let result = steps.register(picture).map_err(|e| match e {
            CmdError::Invalid { path, msg } => {
                CmdError::invalid(format!("/pictures/{i}{path}"), msg)
            }
            other => other,
        })?;
        registered.push(result);
    }
    let mut clips = Vec::with_capacity(registered.len());
    let mut next = first_at;
    let mut span: Option<(Time, Duration)> = None;
    let mut end = Time::ZERO;
    for picture in &registered {
        let placed = steps
            .insert(&TimelineInsert {
                sequence,
                track,
                media: picture.media_id,
                at: next,
                source_in: None,
                duration: batch.per_image,
                overlap: None,
                link: None,
                mode: None,
                audio_track: None,
                ripple_all_tracks: None,
            })
            .map_err(|e| match e {
                CmdError::Invalid { path, msg } if path == "/duration" => {
                    CmdError::invalid("/per_image", msg)
                }
                other => other,
            })?;
        end = placed
            .at
            .checked_add(placed.duration)
            .map_err(|e| CmdError::invalid("/at", e.to_string()))?;
        next = TimeInput::from(end);
        span.get_or_insert((placed.at, placed.duration));
        clips.extend(placed.clips);
    }
    let (at, per_image) =
        span.ok_or_else(|| CmdError::invalid("/files", "a slideshow needs a picture"))?;
    Ok(SlideshowPlaced {
        registered,
        sequence,
        track,
        at,
        per_image,
        end,
        clips,
    })
}

/// The file name of `path`, lower-cased (the sort key of `name` and `name_numeric`).
fn name_key(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_lowercase()
}

/// `a` against `b` with every run of digits compared by its value.
fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    let digits = |s: &[u8]| s.iter().take_while(|c| c.is_ascii_digit()).count();
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (run_a, run_b) = (digits(a), digits(b));
                let strip = |s: &[u8]| s.iter().take_while(|c| **c == b'0').count();
                let (value_a, value_b) = (&a[strip(&a[..run_a])..run_a], &b[strip(&b[..run_b])..run_b]);
                let by_value = value_a
                    .len()
                    .cmp(&value_b.len())
                    .then_with(|| value_a.cmp(value_b));
                if by_value != Ordering::Equal {
                    return by_value;
                }
                (a, b) = (&a[run_a..], &b[run_b..]);
            }
            (Some(x), Some(y)) if x != y => return x.cmp(y),
            _ => (a, b) = (&a[1..], &b[1..]),
        }
    }
}

/// Queues the analysis and the proxy of a registered file, as `media.import` does for each of
/// its files, and describes it for the result.
pub(super) fn announce(
    ctx: &ImportCtx,
    register: MediaRegister,
    registered: MediaRegistered,
    job_ids: &mut Vec<JobId>,
) -> ImportedMedia {
    let mut index_job = Vec::new();
    let path = register.path.clone();
    if registered.created
        && let Some(analyzer) = &ctx.analysis
    {
        let stages = analyzer.spawn_stages(AnalysisInput {
            media_id: registered.media_id,
            kind: registered.kind,
            path: register.path,
            content_hash: register.content_hash,
            probe: register.probe,
        });
        index_job.extend(
            stages
                .iter()
                .filter(|(stage, _)| *stage == "index")
                .map(|(_, id)| *id),
        );
        job_ids.extend(stages.into_iter().map(|(_, id)| id));
    }
    let proxy = ctx.proxies.as_ref().map_or(ImportProxy::None, |p| {
        p.imported(registered.media_id, &index_job)
    });
    ImportedMedia {
        path,
        media_id: registered.media_id,
        kind: registered.kind,
        duplicate: !registered.created,
        proxy,
    }
}

impl MediaImportImagesAsSlideshow {
    fn check(&self, policy: &ImportPolicy) -> Result<(), CmdError> {
        if self.files.is_empty() || self.files.len() > MAX_IMPORT_PATHS {
            return Err(CmdError::invalid(
                "/files",
                format!("give 1..={MAX_IMPORT_PATHS} pictures"),
            ));
        }
        for (i, path) in self.files.iter().enumerate() {
            let pointer = format!("/files/{i}");
            check_abs_path(&pointer, path)?;
            if self.files[..i].contains(path) {
                return Err(CmdError::invalid(
                    pointer,
                    format!("`{path}` is listed twice"),
                ));
            }
            if let Some(roots) = &policy.allowed_roots
                && !roots.allows(path)
            {
                return Err(CmdError::invalid(
                    pointer,
                    format!(
                        "`{path}` is outside the folders you may import from (the project folder \
                         and the folders the user allowed); {OUTSIDE_ROOTS_HINT}"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Probes the files: the pictures (with their index in `files`) and the files left out.
    fn pictures(&self, ctx: &ImportCtx) -> (Vec<(usize, MediaRegister)>, Vec<ImportFailure>) {
        let probed = policy::probe_all(&ctx.prober, &self.files, &ctx.policy, &mut || {});
        let mut pictures = Vec::with_capacity(self.files.len());
        let mut failed = Vec::new();
        for (i, (path, result)) in self.files.iter().zip(probed).enumerate() {
            let probed = match result {
                Ok(probed) => probed,
                Err(e) => {
                    failed.push(failure(path, &e));
                    continue;
                }
            };
            let register = MediaRegister {
                media_id: None,
                path: path.clone(),
                probe: probed.probe,
                content_hash: probed.content_hash,
                folder: None,
            };
            match register.kind() {
                Ok(MediaKind::Image) => pictures.push((i, register)),
                Ok(kind) => failed.push(ImportFailure {
                    path: path.clone(),
                    code: ImportFailureCode::Unsupported,
                    error: format!(
                        "`{path}` is {kind} media, not a still picture, so it is not part of \
                         the slideshow (import it with media.import)"
                    ),
                }),
                Err(e) => failed.push(ImportFailure {
                    path: path.clone(),
                    code: ImportFailureCode::Invalid,
                    error: format!("`{path}` cannot be imported: {}", e.message()),
                }),
            }
        }
        (pictures, failed)
    }
}

impl EngineCommand for MediaImportImagesAsSlideshow {
    type Context = ImportCtx;
    type Output = MediaSlideshowOutput;

    fn run(&self, ctx: &mut ImportCtx) -> Result<MediaSlideshowOutput, CmdError> {
        self.check(&ctx.policy)?;
        let (mut pictures, failed) = self.pictures(ctx);
        if pictures.is_empty() {
            let why = failed
                .first()
                .map_or_else(String::new, |f| format!(": {}", f.error));
            return Err(CmdError::invalid(
                "/files",
                format!("none of the files is a picture that can be imported{why}"),
            ));
        }
        match self.order.unwrap_or_default() {
            SlideshowOrder::Given => {}
            SlideshowOrder::Name => pictures.sort_by_cached_key(|(_, r)| name_key(&r.path)),
            SlideshowOrder::NameNumeric => {
                let mut keyed: Vec<(String, (usize, MediaRegister))> = pictures
                    .into_iter()
                    .map(|p| (name_key(&p.1.path), p))
                    .collect();
                keyed.sort_by(|a, b| natural(&a.0, &b.0));
                pictures = keyed.into_iter().map(|(_, p)| p).collect();
            }
        }
        let (origin, batch): (Vec<usize>, Vec<MediaRegister>) = pictures.into_iter().unzip();
        let placed = ctx
            .sink
            .slideshow(SlideshowBatch {
                pictures: batch.clone(),
                sequence: self.sequence,
                track: self.track,
                at: self.at,
                per_image: self.per_image,
            })
            .map_err(|e| files_pointer(e, &origin))?;
        if placed.registered.len() != batch.len() {
            return Err(CmdError::internal(
                "the media sink returned a different number of results than requested",
            ));
        }
        let mut out = MediaSlideshowOutput {
            sequence: placed.sequence,
            track: placed.track,
            at: placed.at,
            per_image: placed.per_image,
            end: placed.end,
            clips: placed.clips,
            media: Vec::with_capacity(batch.len()),
            media_ids: Vec::with_capacity(batch.len()),
            failed,
            job_ids: Vec::new(),
        };
        let mut announced = std::collections::HashSet::new();
        for (register, registered) in batch.into_iter().zip(placed.registered) {
            // A picture listed under two names is one asset: its analysis is queued once.
            let entry = if announced.insert(registered.media_id) {
                announce(ctx, register, registered, &mut out.job_ids)
            } else {
                ImportedMedia {
                    path: register.path,
                    media_id: registered.media_id,
                    kind: registered.kind,
                    duplicate: true,
                    proxy: ImportProxy::None,
                }
            };
            out.media_ids.push(entry.media_id);
            out.media.push(entry);
        }
        Ok(out)
    }
}

/// Rewrites the `/pictures/<batch index>` pointer of a sink error ([`place_slideshow`]) to the
/// file's index in the request (`/files/<i>`).
fn files_pointer(error: CmdError, origin: &[usize]) -> CmdError {
    let CmdError::Invalid { path, msg } = error else {
        return error;
    };
    let rewritten = path.strip_prefix("/pictures/").and_then(|rest| {
        let (index, tail) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let original = origin.get(index.parse::<usize>().ok()?)?;
        Some(format!("/files/{original}{tail}"))
    });
    CmdError::Invalid {
        path: rewritten.unwrap_or(path),
        msg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_compares_numbers_by_value() {
        let mut names = vec!["img10.png", "img2.png", "img1.png", "a.png", "img02b.png"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(
            names,
            ["a.png", "img1.png", "img2.png", "img02b.png", "img10.png"]
        );
        assert_eq!(natural("shot_007", "shot_7"), Ordering::Equal);
        assert_eq!(natural("a", "a1"), Ordering::Less);
    }

    #[test]
    fn the_sort_key_is_the_lower_cased_file_name() {
        assert_eq!(name_key("/a/b/Beach.PNG"), "beach.png");
        assert_eq!(name_key("C:\\pics\\Zoo.jpg"), "zoo.jpg");
        assert_eq!(name_key("plain"), "plain");
    }
}
