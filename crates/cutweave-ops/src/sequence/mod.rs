// SPDX-License-Identifier: GPL-3.0-or-later
//! The `sequence` namespace (WP-2B.01; arch §4.2, §5.1): a project holds several sequences,
//! each with its own aspect, resolution, frame rate and background.
//!
//! * `sequence.create`: a new sequence with `V1` and `A1`, from an aspect preset
//!   ([`cutweave_model::ASPECT_PRESETS`]) or an explicit format.
//! * `sequence.create_from_media`: a new sequence holding the given assets in order.
//! * `sequence.duplicate`: a deep copy with new IDs and shared media, for another platform.
//! * `sequence.rename`, `sequence.delete` (never the last one, never while a compound clip
//!   refers to it).
//! * `sequence.set_format`: canvas size, frame rate, sample rate and background. A new canvas
//!   size or shape reframes the clips' transforms by a [`ReframePolicy`] ([`reframe`]); a new
//!   frame rate re-snaps every item to the new grid ([`regrid`]). Both refuse with `Locked`
//!   when a locked track holds items they would change.
//! * `view.set_active_sequence` (in [`crate::view`]): which sequence the editor shows.
//! * `sequence.list`: the sequences, and on request the preset tables.
//!
//! The background is the format's `background` color here; the blur and image backgrounds are
//! WP-2B.02.

mod copy;
pub mod reframe;
pub mod regrid;

use std::collections::BTreeMap;

use cutweave_commands::{
    CmdError, Command, Query, QueryCtx, Registry, RegistryError, Tx, command, query, wire,
};
use cutweave_model::{
    ASPECT_PRESETS, Clip, Color, Duration, EntityKind, FRAME_RATE_PRESETS, FrameRate, GroupId,
    Invariant, Item, ItemId, MediaId, MediaKind, Project, Rounding, SampleRate, Sequence,
    SequenceFormat, SequenceId, Time, Track, TrackId, TrackKind, aspect_preset,
};

pub use reframe::ReframePolicy;

use crate::{
    media::has_audio_stream,
    project::SequenceBrief,
    util::{MAX_NAME_CHARS, check_name},
};

/// Most assets `sequence.create_from_media` takes.
pub const MAX_FROM_MEDIA: usize = 1_000;

/// Registers the `sequence.*` commands and queries.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<SequenceCreate>()?
        .register::<SequenceCreateFromMedia>()?
        .register::<SequenceDuplicate>()?
        .register::<SequenceRename>()?
        .register::<SequenceDelete>()?
        .register::<SequenceSetFormat>()?
        .register::<SequenceList>()?;
    Ok(())
}

// ----- formats -------------------------------------------------------------------------------

/// A format, field by field. Absent fields keep the base value: the project's default format
/// for a new sequence, the sequence's own format for `sequence.set_format`.
#[wire]
#[derive(Default)]
pub struct FormatPatch {
    /// Canvas width in pixels, `1..=16384`.
    pub width: Option<u32>,
    /// Canvas height in pixels, `1..=16384`.
    pub height: Option<u32>,
    /// The edit grid, `1..=1000` fps and a whole number of flicks per frame.
    pub frame_rate: Option<FrameRate>,
    /// The mix rate in Hz, `8000..=384000`.
    pub sample_rate: Option<SampleRate>,
    /// The canvas color behind all tracks, `#rrggbb`.
    pub background: Option<Color>,
}

impl FormatPatch {
    fn apply_to(&self, mut f: SequenceFormat) -> SequenceFormat {
        f.width = self.width.unwrap_or(f.width);
        f.height = self.height.unwrap_or(f.height);
        f.frame_rate = self.frame_rate.unwrap_or(f.frame_rate);
        f.sample_rate = self.sample_rate.unwrap_or(f.sample_rate);
        f.background = self.background.unwrap_or(f.background);
        f
    }
}

/// `Invalid` at `path` unless `format` is a valid sequence format.
fn check_format(path: &str, format: &SequenceFormat) -> Result<(), CmdError> {
    let mut probe = Project::new("format check");
    probe.settings.default_format = format.clone();
    for v in probe.violations() {
        if let Invariant::InvalidFormat { reason, .. } = v {
            return Err(CmdError::invalid(path, reason));
        }
    }
    Ok(())
}

/// The format of a new sequence: the project default, then the preset's size, then `format`.
fn new_format(
    doc: &Project,
    preset: Option<&str>,
    format: Option<&FormatPatch>,
) -> Result<SequenceFormat, CmdError> {
    let mut f = doc.settings.default_format.clone();
    if let Some(id) = preset {
        let p = aspect_preset(id).ok_or_else(|| {
            let known: Vec<&str> = ASPECT_PRESETS.iter().map(|p| p.id).collect();
            CmdError::invalid(
                "/preset",
                format!("unknown preset `{id}`; use one of {}", known.join(", ")),
            )
        })?;
        f.width = p.width;
        f.height = p.height;
    }
    if let Some(patch) = format {
        f = patch.apply_to(f);
    }
    check_format(
        if format.is_some() {
            "/format"
        } else {
            "/preset"
        },
        &f,
    )?;
    Ok(f)
}

/// A new sequence with `V1` over `A1`.
fn new_sequence(tx: &mut Tx<'_>, name: String, format: SequenceFormat) -> NewSequence {
    let mut sequence = Sequence::new(tx.new_id(), name, format);
    let video = Track::new(tx.new_id(), TrackKind::Video, "V1");
    let audio = Track::new(tx.new_id(), TrackKind::Audio, "A1");
    let (video_id, audio_id) = (video.id, audio.id);
    sequence.insert_track(0, video);
    sequence.insert_track(1, audio);
    NewSequence {
        sequence,
        video: video_id,
        audio: audio_id,
    }
}

struct NewSequence {
    sequence: Sequence,
    video: TrackId,
    audio: TrackId,
}

// ----- sequence.create -----------------------------------------------------------------------

/// Result of `sequence.create` and `sequence.create_from_media`.
#[wire]
pub struct SequenceCreateOutput {
    /// The new sequence.
    pub sequence: SequenceId,
    /// Its video track `V1`.
    pub video_track: TrackId,
    /// Its audio track `A1`.
    pub audio_track: TrackId,
    /// Its format.
    pub format: SequenceFormat,
}

/// Adds a sequence with an empty `V1` over an empty `A1`. The format starts from the project's
/// default; `preset` (an aspect preset ID such as `9:16`) sets the canvas size and `format`
/// overrides single fields.
#[command(
    name = "sequence.create",
    summary = "Add a sequence from an aspect preset or a format",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = false)
)]
pub struct SequenceCreate {
    /// Name (1..=200 characters).
    pub name: String,
    /// Aspect preset ID: `16:9`, `16:9-4k`, `9:16`, `1:1`, `4:5` or `21:9`.
    pub preset: Option<String>,
    /// Format fields; they win over the preset.
    pub format: Option<FormatPatch>,
}

impl Command for SequenceCreate {
    type Output = SequenceCreateOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        check_name("/name", &self.name)?;
        new_format(ctx.doc(), self.preset.as_deref(), self.format.as_ref()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<SequenceCreateOutput, CmdError> {
        let name = check_name("/name", &self.name)?;
        let format = new_format(tx.doc(), self.preset.as_deref(), self.format.as_ref())?;
        let new = new_sequence(tx, name, format);
        tx.insert_sequence(&new.sequence)?;
        Ok(new.output())
    }
}

impl NewSequence {
    fn output(&self) -> SequenceCreateOutput {
        SequenceCreateOutput {
            sequence: self.sequence.id,
            video_track: self.video,
            audio_track: self.audio,
            format: self.sequence.format.clone(),
        }
    }
}

// ----- sequence.create_from_media ------------------------------------------------------------

/// Adds a sequence holding the given assets one after another from 0: pictures and stills on
/// `V1`, a video's sound linked on `A1`, audio files on `A1`. Each clip takes the whole asset,
/// floored to whole frames (a still gets 5 s). The format is the project's default.
#[command(
    name = "sequence.create_from_media",
    summary = "Add a sequence holding the given assets in order",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct SequenceCreateFromMedia {
    /// The assets, in order (1..=1000).
    pub media: Vec<MediaId>,
    /// Name (default `Sequence <n>`).
    pub name: Option<String>,
}

/// One clip to place: its asset, its length and whether it has sound to link.
struct Placed {
    media: MediaId,
    kind: MediaKind,
    duration: Duration,
    sound: bool,
}

fn planned(doc: &Project, media: &[MediaId], rate: FrameRate) -> Result<Vec<Placed>, CmdError> {
    if media.is_empty() || media.len() > MAX_FROM_MEDIA {
        return Err(CmdError::invalid(
            "/media",
            format!("give between 1 and {MAX_FROM_MEDIA} assets"),
        ));
    }
    let to_duration = |frames: i64, path: &str| -> Result<Duration, CmdError> {
        rate.frame_to_time(frames)
            .ok()
            .and_then(|t| Duration::try_from(t).ok())
            .ok_or_else(|| CmdError::invalid(path, "the clip would be too long"))
    };
    media
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let path = format!("/media/{i}");
            let asset = doc
                .media
                .get(id)
                .ok_or_else(|| CmdError::not_found(EntityKind::Media, id))?;
            if asset.kind == MediaKind::SequenceRef {
                return Err(CmdError::unsupported(
                    "compound clips (sequence media) cannot be placed yet",
                ));
            }
            let frames = match (asset.kind, asset.probe.duration) {
                (MediaKind::Image, _) => crate::timeline::default_still_frames(doc, rate)?,
                (_, Some(total)) => {
                    let t = Time::from_flicks(total.flicks())
                        .map_err(|e| CmdError::invalid(&path, e.to_string()))?;
                    rate.time_to_frame(t, Rounding::Floor)
                }
                (_, None) => {
                    return Err(CmdError::invalid(
                        &path,
                        "the media length is unknown, so it cannot be placed",
                    ));
                }
            };
            if frames < 1 {
                return Err(CmdError::invalid(
                    &path,
                    "the media is shorter than one frame",
                ));
            }
            Ok(Placed {
                media: *id,
                kind: asset.kind,
                duration: to_duration(frames, &path)?,
                sound: has_audio_stream(asset),
            })
        })
        .collect()
}

impl Command for SequenceCreateFromMedia {
    type Output = SequenceCreateOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        if let Some(name) = &self.name {
            check_name("/name", name)?;
        }
        planned(
            ctx.doc(),
            &self.media,
            ctx.doc().settings.default_format.frame_rate,
        )
        .map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<SequenceCreateOutput, CmdError> {
        let format = tx.doc().settings.default_format.clone();
        let placed = planned(tx.doc(), &self.media, format.frame_rate)?;
        let name = match &self.name {
            Some(n) => check_name("/name", n)?,
            None => format!("Sequence {}", tx.doc().sequences.len() + 1),
        };
        let mut new = new_sequence(tx, name, format);
        let (video, audio) = (new.video, new.audio);
        let mut cursor = Time::ZERO;
        for p in placed {
            let group: Option<GroupId> =
                (p.kind == MediaKind::Video && p.sound).then(|| tx.new_id());
            let mut clips = Vec::new();
            if p.kind != MediaKind::Audio {
                clips.push((video, p.media));
            }
            if p.kind == MediaKind::Audio || group.is_some() {
                clips.push((audio, p.media));
            }
            for (track, media) in clips {
                let mut clip = Clip::new(tx.new_id::<ItemId>(), media, cursor, p.duration);
                clip.linked_group = group;
                if let Some(t) = new.sequence.track_mut(&track) {
                    t.items.insert(Item::Clip(clip));
                }
            }
            cursor = cursor
                .checked_add(p.duration)
                .map_err(|e| CmdError::invalid("/media", e.to_string()))?;
        }
        tx.insert_sequence(&new.sequence)?;
        Ok(new.output())
    }
}

// ----- sequence.duplicate --------------------------------------------------------------------

/// Result of `sequence.duplicate`.
#[wire]
pub struct SequenceDuplicateOutput {
    /// The copy.
    pub sequence: SequenceId,
    /// Its name.
    pub name: String,
    /// How many items it holds.
    pub item_count: u32,
}

/// Copies a sequence: the copy has new IDs for the sequence, its tracks, items, markers and
/// linked groups, and uses the same media. Editing the copy never changes the original.
#[command(
    name = "sequence.duplicate",
    summary = "Copy a sequence (new IDs, shared media), for another platform",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct SequenceDuplicate {
    /// The sequence to copy.
    pub sequence: SequenceId,
    /// Name of the copy (default: the name plus ` copy`).
    pub name: Option<String>,
}

impl Command for SequenceDuplicate {
    type Output = SequenceDuplicateOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        ctx.sequence(self.sequence)?;
        if let Some(name) = &self.name {
            check_name("/name", name)?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<SequenceDuplicateOutput, CmdError> {
        self.validate(&tx.query_ctx())?;
        let source = tx.sequence(self.sequence)?.clone();
        let name = if let Some(n) = &self.name {
            check_name("/name", n)?
        } else {
            format!("{} copy", source.name)
                .chars()
                .take(MAX_NAME_CHARS)
                .collect()
        };
        let copy = copy::deep_copy(tx, &source, name.clone())?;
        tx.insert_sequence(&copy)?;
        Ok(SequenceDuplicateOutput {
            sequence: copy.id,
            name,
            item_count: count(copy.tracks.values().map(|t| t.items.len()).sum()),
        })
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

// ----- sequence.rename -----------------------------------------------------------------------

/// Result of `sequence.rename`.
#[wire]
pub struct SequenceRenameOutput {
    /// The sequence.
    pub sequence: SequenceId,
    /// The name now in effect (trimmed).
    pub name: String,
}

/// Renames a sequence.
#[command(
    name = "sequence.rename",
    summary = "Rename a sequence",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct SequenceRename {
    /// The sequence.
    pub sequence: SequenceId,
    /// New name (1..=200 characters).
    pub name: String,
}

impl Command for SequenceRename {
    type Output = SequenceRenameOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        ctx.sequence(self.sequence)?;
        check_name("/name", &self.name).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<SequenceRenameOutput, CmdError> {
        self.validate(&tx.query_ctx())?;
        let name = check_name("/name", &self.name)?;
        tx.update_sequence(self.sequence, |s| {
            s.name.clone_from(&name);
            Ok(())
        })?;
        Ok(SequenceRenameOutput {
            sequence: self.sequence,
            name,
        })
    }
}

// ----- sequence.delete -----------------------------------------------------------------------

/// Result of `sequence.delete`.
#[wire]
pub struct SequenceDeleteOutput {
    /// The removed sequence.
    pub sequence: SequenceId,
    /// How many items it held (removed with it).
    pub removed_items: u32,
    /// How many sequences the project has now.
    pub sequence_count: u32,
}

/// Removes a sequence with its tracks and items; the media stays in the bin. Fails with
/// `Invalid` for the last sequence and for a sequence that compound clips refer to.
#[command(
    name = "sequence.delete",
    summary = "Delete a sequence (not the last one, not while a compound clip uses it)",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct SequenceDelete {
    /// The sequence.
    pub sequence: SequenceId,
}

impl Command for SequenceDelete {
    type Output = SequenceDeleteOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        ctx.sequence(self.sequence)?;
        if ctx.doc().sequences.len() <= 1 {
            return Err(CmdError::invalid(
                "/sequence",
                "a project keeps at least one sequence; this is the last one",
            ));
        }
        let users: Vec<String> = ctx
            .doc()
            .media
            .values()
            .filter(|m| m.sequence == Some(self.sequence))
            .map(|m| m.id.to_string())
            .collect();
        if !users.is_empty() {
            return Err(CmdError::invalid(
                "/sequence",
                format!(
                    "compound clips use this sequence (media {}); remove them first",
                    users.join(", ")
                ),
            ));
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<SequenceDeleteOutput, CmdError> {
        self.validate(&tx.query_ctx())?;
        let removed = tx.remove_sequence(self.sequence)?;
        Ok(SequenceDeleteOutput {
            sequence: removed.id,
            removed_items: count(removed.tracks.values().map(|t| t.items.len()).sum()),
            sequence_count: count(tx.doc().sequences.len()),
        })
    }
}

// ----- sequence.set_format -------------------------------------------------------------------

/// Result of `sequence.set_format`.
#[wire]
pub struct SequenceSetFormatOutput {
    /// The sequence.
    pub sequence: SequenceId,
    /// Its format now.
    pub format: SequenceFormat,
    /// Clips whose position or scale changed to follow the new canvas.
    pub reframed: u32,
    /// Items whose start or length moved to the new frame grid.
    pub retimed: u32,
}

/// Changes a sequence's canvas size, frame rate, sample rate or background (absent fields stay).
/// A new canvas size reframes every video clip by `reframe` (default `fit`: the whole composition
/// shrinks to sit inside the new canvas; `fill` covers it; `keep_scale` keeps pixel sizes;
/// `none` leaves transforms). A new frame rate moves every item edge to the nearest frame of
/// the new grid without creating overlaps or gaps. Fails with `Locked` if a locked track holds
/// items this would change.
#[command(
    name = "sequence.set_format",
    summary = "Change a sequence's size, frame rate, sample rate or background",
    undoable,
    mcp = "core",
    annotations(destructive = true, idempotent = true)
)]
pub struct SequenceSetFormat {
    /// The sequence.
    pub sequence: SequenceId,
    /// Canvas width in pixels, `1..=16384`.
    pub width: Option<u32>,
    /// Canvas height in pixels, `1..=16384`.
    pub height: Option<u32>,
    /// The edit grid.
    pub frame_rate: Option<FrameRate>,
    /// The mix rate in Hz.
    pub sample_rate: Option<SampleRate>,
    /// Canvas color, `#rrggbb`.
    pub background: Option<Color>,
    /// How clips follow a new canvas size (default `fit`).
    pub reframe: Option<ReframePolicy>,
}

impl SequenceSetFormat {
    fn patch(&self) -> FormatPatch {
        FormatPatch {
            width: self.width,
            height: self.height,
            frame_rate: self.frame_rate,
            sample_rate: self.sample_rate,
            background: self.background,
        }
    }

    /// The new format and what it changes, checked against `doc`.
    fn plan(&self, doc: &Project) -> Result<FormatChange, CmdError> {
        let sequence = doc
            .sequences
            .get(&self.sequence)
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, self.sequence))?;
        let old = sequence.format.clone();
        let new = self.patch().apply_to(old.clone());
        check_format("/", &new)?;
        let policy = self.reframe.unwrap_or_default();
        let resized = (new.width, new.height) != (old.width, old.height);
        let reframe = resized && policy != ReframePolicy::None;
        let regrid = new.frame_rate != old.frame_rate;
        // Edits that would change an item of a locked track are refused (INV-28).
        let locked = sequence.tracks_in_order().find(|t| {
            t.locked && !t.items.is_empty() && (regrid || (reframe && t.kind == TrackKind::Video))
        });
        if let Some(t) = locked {
            return Err(CmdError::locked(EntityKind::Track, t.id));
        }
        Ok(FormatChange {
            old,
            new,
            policy,
            reframe,
            regrid,
        })
    }
}

struct FormatChange {
    old: SequenceFormat,
    new: SequenceFormat,
    policy: ReframePolicy,
    reframe: bool,
    regrid: bool,
}

impl Command for SequenceSetFormat {
    type Output = SequenceSetFormatOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        let change = self.plan(ctx.doc())?;
        if change.regrid {
            let sequence = ctx.sequence(self.sequence)?;
            regrid::plan(ctx.doc(), sequence, change.new.frame_rate)?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<SequenceSetFormatOutput, CmdError> {
        let change = self.plan(tx.doc())?;
        let timing = if change.regrid {
            Some(regrid::plan(
                tx.doc(),
                tx.sequence(self.sequence)?,
                change.new.frame_rate,
            )?)
        } else {
            None
        };
        // The picture aspect of each asset, for the reframe factors.
        let aspects: BTreeMap<MediaId, f64> = tx
            .doc()
            .media
            .values()
            .filter_map(|m| reframe::picture_aspect(m).map(|a| (m.id, a)))
            .collect();
        let old = reframe::Canvas::new(change.old.width, change.old.height);
        let new = reframe::Canvas::new(change.new.width, change.new.height);
        let k = change
            .reframe
            .then(|| reframe::factor(change.policy, old, new))
            .flatten();
        let (mut reframed, mut retimed) = (0_u32, 0_u32);
        let format = change.new.clone();
        tx.update_sequence(self.sequence, |s| {
            s.format = format;
            let track_ids: Vec<TrackId> = s.tracks.keys().copied().collect();
            for track_id in track_ids {
                let Some(track) = s.tracks.get_mut(&track_id) else {
                    continue;
                };
                let ids: Vec<ItemId> = track.items.iter().map(Item::id).collect();
                for id in ids {
                    let timing = timing.as_ref().and_then(|t| t.items.get(&id));
                    let visual = k.is_some() && track.kind == TrackKind::Video;
                    track.items.update(&id, |item| {
                        if let Some((start, duration, source)) = timing
                            && (item.start() != *start || item.duration() != *duration)
                        {
                            item.set_start(*start);
                            item.set_duration(*duration);
                            if let (Some(source), Some(clip)) = (source, item.as_clip_mut()) {
                                clip.source_in = *source;
                            }
                            retimed += 1;
                        }
                        if let (true, Some(k), Some(clip)) = (visual, k, item.as_clip_mut()) {
                            let a = aspects.get(&clip.media).copied().unwrap_or(old.w / old.h);
                            if reframe::reframe_clip(clip, reframe::multipliers(k, a, old, new)) {
                                reframed += 1;
                            }
                        }
                    });
                }
            }
            Ok(())
        })?;
        let sequence = tx.sequence(self.sequence)?;
        Ok(SequenceSetFormatOutput {
            sequence: self.sequence,
            format: sequence.format.clone(),
            reframed,
            retimed,
        })
    }
}

// ----- sequence.list -------------------------------------------------------------------------

/// One aspect preset.
#[wire]
pub struct PresetInfo {
    /// The ID `sequence.create` takes as `preset`.
    pub id: String,
    /// Aspect width (the `16` of 16:9).
    pub aspect_w: u32,
    /// Aspect height (the `9` of 16:9).
    pub aspect_h: u32,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// What it is used for.
    pub use_for: String,
}

/// Result of `sequence.list`.
#[wire]
pub struct SequenceListOutput {
    /// The sequences, ordered by ID.
    pub sequences: Vec<SequenceBrief>,
    /// The aspect presets, when asked for.
    pub presets: Option<Vec<PresetInfo>>,
    /// The offered frame rates (`[num, den]` pairs), when presets are asked for.
    pub frame_rates: Option<Vec<FrameRate>>,
}

/// Lists the sequences with their format, length and track counts; `presets: true` adds the
/// aspect presets and frame rates `sequence.create` and `sequence.set_format` offer.
#[query(
    name = "sequence.list",
    summary = "List the sequences (and the aspect presets on request)",
    mcp = "catalog"
)]
pub struct SequenceList {
    /// Also return the aspect and frame-rate presets.
    pub presets: Option<bool>,
}

impl Query for SequenceList {
    type Output = SequenceListOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<SequenceListOutput, CmdError> {
        let sequences = ctx
            .doc()
            .sequences
            .values()
            .map(crate::project::sequence_brief)
            .collect();
        let want = self.presets == Some(true);
        Ok(SequenceListOutput {
            sequences,
            presets: want.then(|| {
                ASPECT_PRESETS
                    .iter()
                    .map(|p| PresetInfo {
                        id: p.id.to_owned(),
                        aspect_w: p.aspect_w,
                        aspect_h: p.aspect_h,
                        width: p.width,
                        height: p.height,
                        use_for: p.use_for.to_owned(),
                    })
                    .collect()
            }),
            frame_rates: want.then(|| FRAME_RATE_PRESETS.to_vec()),
        })
    }
}
