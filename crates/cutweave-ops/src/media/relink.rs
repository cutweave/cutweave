// SPDX-License-Identifier: GPL-3.0-or-later
//! Relinking missing media (WP-2F.03; arch §7): `media.find_missing` says which assets' files
//! are gone, `media.relink` points one asset at a file, and `media.relink_folder` searches a
//! folder for the files of every missing asset. All three are in the MCP catalog.
//!
//! **Health is not document state.** Whether a file is there is a fact about this computer, so
//! it is never saved: `media.find_missing` checks the disk each time it is called. Only the
//! new location is written to the project, through the hidden, undoable `media.set_location`,
//! which the relink commands submit in one transaction through a [`RelinkSink`] (one undo step
//! puts every old path back).
//!
//! **Matching (`media.relink_folder`).** A file is an asset's when its content hash equals the
//! asset's (the fast hash import computed: size, first and last MiB), whatever its name or
//! folder. Files are probed lazily: the ones named like a missing asset first, the rest only
//! while assets are still unmatched, at most [`MAX_RELINK_FILES`] per call. An asset no hash
//! matched falls back to the one file with its name (ignoring case), its kind and its exact
//! duration (a re-wrapped copy); the asset then takes that file's hash and probe. The document
//! stores no file size and only the fast hash, so the size is covered by the hash alone and a
//! full hash cannot settle a tie: of two files with an asset's hash the first in walk order is
//! used.
//!
//! **Image sequences (ADR-0035).** A sequence is one asset made of many files, and its path is
//! its first picture. It is `missing` when that picture is gone and `incomplete` when the first
//! is there but other pictures of the run are not (the numbers that had no file at import are
//! never counted). Relinking moves the whole run: `media.relink` takes any picture of the run
//! at its new place, or the folder, and `media.relink_folder` looks for the run's first file
//! name. Either way every picture of the run must be there. The run is recognized by its
//! content hash, which is the first picture's hash mixed with the run (what import computed:
//! the prober's answer for the run's locator), so the same pictures in a new folder match by
//! hash; other pictures with the run's names are a name match.
//!
//! **Who may read what.** Agents and headless callers may only name paths inside the project
//! folder and the folders the user approved ([`ImportPolicy::allowed_roots`]), as for
//! `media.import`.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    path::Path,
    sync::Arc,
};

use cutweave_commands::{
    Annotations, CmdError, Command, CommandSpec, Described, EngineCommand, EntryKind, McpExposure,
    QueryCtx, Registrable, Registry, RegistryError, Scope, Tx, command, wire,
};
use cutweave_model::{
    EntityKind, ImageSequence, MediaAsset, MediaId, MediaKind, ProbeInfo, Project, StreamKind,
};

use super::{
    ImportPolicy, MAX_HASH_CHARS, MediaProbe, OUTSIDE_ROOTS_HINT, Probed, asset_name,
    kind_of_probe, policy,
    sequence::{join, parent_and_name},
    walk,
};
use crate::util::check_abs_path;

/// Most assets one relink command names.
pub const MAX_RELINK_MEDIA: usize = 256;
/// Most files one `media.relink_folder` call probes.
pub const MAX_RELINK_FILES: usize = walk::MAX_IMPORT_FILES;

// ----- the disk ------------------------------------------------------------------------------

/// The files of a folder, as [`RelinkFiles::list`] found them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderFiles {
    /// Absolute paths, in walk order.
    pub files: Vec<String>,
    /// The folder holds more files than were listed.
    pub truncated: bool,
}

/// What the relink commands ask the disk. The engine uses [`DiskFiles`]; tests use
/// [`MemoryFiles`].
pub trait RelinkFiles: Send + Sync {
    /// Whether there is a file at `path`.
    fn exists(&self, path: &str) -> bool;

    /// Whether `path` is a folder.
    fn is_folder(&self, path: &str) -> bool;

    /// The files of `folder`, and of the folders below it when `recursive` (no links, no
    /// hidden files).
    fn list(&self, folder: &str, recursive: bool) -> FolderFiles;
}

/// The real disk.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiskFiles;

impl RelinkFiles for DiskFiles {
    fn exists(&self, path: &str) -> bool {
        Path::new(path).is_file()
    }

    fn is_folder(&self, path: &str) -> bool {
        walk::is_folder(path)
    }

    fn list(&self, folder: &str, recursive: bool) -> FolderFiles {
        if recursive {
            let walked = walk::walk(Path::new(folder));
            return FolderFiles {
                files: walked.files.into_iter().map(|f| f.path).collect(),
                truncated: walked.truncated,
            };
        }
        let mut files: Vec<String> = fs::read_dir(folder)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.path().to_str().map(str::to_owned))
            .collect();
        files.sort_by_cached_key(|p| (p.to_lowercase(), p.clone()));
        FolderFiles {
            files,
            truncated: false,
        }
    }
}

/// A [`RelinkFiles`] over a list of paths (with `/` separators), for tests and headless runs.
#[derive(Clone, Debug, Default)]
pub struct MemoryFiles {
    files: BTreeSet<String>,
    folders: BTreeSet<String>,
}

impl MemoryFiles {
    /// No files.
    #[must_use]
    pub fn new() -> MemoryFiles {
        MemoryFiles::default()
    }

    /// The same disk with a file at `path` (its parent folders exist too).
    #[must_use]
    pub fn with_file(mut self, path: &str) -> MemoryFiles {
        let mut dir = path;
        while let Some((parent, _)) = dir.rsplit_once('/') {
            if parent.is_empty() {
                break;
            }
            self.folders.insert(parent.to_owned());
            dir = parent;
        }
        self.files.insert(path.to_owned());
        self
    }

    /// The same disk with an (empty) folder at `path`.
    #[must_use]
    pub fn with_folder(mut self, path: &str) -> MemoryFiles {
        self.folders.insert(path.to_owned());
        self
    }
}

impl RelinkFiles for MemoryFiles {
    fn exists(&self, path: &str) -> bool {
        self.files.contains(path)
    }

    fn is_folder(&self, path: &str) -> bool {
        self.folders.contains(path)
    }

    fn list(&self, folder: &str, recursive: bool) -> FolderFiles {
        let prefix = format!("{}/", folder.trim_end_matches('/'));
        FolderFiles {
            files: self
                .files
                .iter()
                .filter(|f| {
                    f.strip_prefix(&prefix)
                        .is_some_and(|rest| recursive || !rest.contains('/'))
                })
                .cloned()
                .collect(),
            truncated: false,
        }
    }
}

// ----- media.set_location --------------------------------------------------------------------

/// Result of `media.set_location`.
#[wire]
#[derive(Copy)]
pub struct MediaLocationSet {
    /// The asset.
    pub media_id: MediaId,
}

/// Records where a media file is now. Internal plumbing of `media.relink` and
/// `media.relink_folder`, which check the file first: this command only writes the path, and
/// the content hash and probe results when the file's content is not what was imported. The
/// project-relative path is dropped (it named the old place) and recomputed when the project is
/// saved. For an image sequence the path is the first picture of the run at its new place.
#[command(
    name = "media.set_location",
    summary = "Record where a media file is now (used by relink)",
    undoable,
    mcp = "hidden",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaSetLocation {
    /// The asset (a file, not a compound clip).
    pub media: MediaId,
    /// Absolute path of the file.
    pub path: String,
    /// The file's content hash, when it differs from the asset's.
    pub content_hash: Option<String>,
    /// What probing the file found, when its content differs from the asset's.
    pub probe: Option<ProbeInfo>,
}

fn file_asset<'a>(
    doc: &'a Project,
    pointer: &str,
    id: MediaId,
) -> Result<&'a MediaAsset, CmdError> {
    let asset = doc
        .media
        .get(&id)
        .ok_or_else(|| CmdError::not_found(EntityKind::Media, id))?;
    if asset.kind == MediaKind::SequenceRef || asset.path_abs.is_none() {
        return Err(CmdError::invalid(
            pointer,
            format!("`{id}` is not a media file, so it has no location"),
        ));
    }
    Ok(asset)
}

impl Command for MediaSetLocation {
    type Output = MediaLocationSet;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        check_abs_path("/path", &self.path)?;
        if let Some(hash) = &self.content_hash
            && (hash.is_empty()
                || hash.len() > MAX_HASH_CHARS
                || hash.chars().any(char::is_whitespace))
        {
            return Err(CmdError::invalid(
                "/content_hash",
                format!("the hash must be 1..={MAX_HASH_CHARS} characters without whitespace"),
            ));
        }
        let asset = file_asset(ctx.doc(), "/media", self.media)?;
        if let Some(probe) = &self.probe {
            let fits = match run_of(asset) {
                // The probe of a run is its first picture with the run's length.
                Some(run) => {
                    probe.streams.iter().any(|s| s.kind == StreamKind::Video)
                        && probe.duration == run.duration()
                }
                None => kind_of_probe(probe) == Some(asset.kind),
            };
            if !fits {
                return Err(CmdError::invalid(
                    "/probe/streams",
                    format!("the file is not {} media like the asset", asset.kind),
                ));
            }
            if let (Some(old), Some(new)) = (asset.probe.duration, probe.duration)
                && new < old
            {
                return Err(CmdError::invalid(
                    "/probe/duration",
                    "the file is shorter than the asset, so clips that use it could run past \
                     its end",
                ));
            }
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaLocationSet, CmdError> {
        self.validate(&tx.query_ctx())?;
        tx.update_media(self.media, |m| {
            m.path_abs = Some(self.path.clone());
            // The old relative path names the old place; the next save computes the new one.
            m.path_rel = None;
            if let Some(hash) = &self.content_hash {
                m.content_hash = Some(hash.clone());
            }
            if let Some(probe) = &self.probe {
                m.probe = probe.clone();
            }
            Ok(())
        })?;
        Ok(MediaLocationSet {
            media_id: self.media,
        })
    }
}

/// Applies `batch` in order inside `tx`. For [`RelinkSink`] implementations: the engine runs
/// it in one transaction, so a relink is one undo step.
pub fn relocate_batch(tx: &mut Tx<'_>, batch: &[MediaSetLocation]) -> Result<(), CmdError> {
    for set in batch {
        set.apply(tx)?;
    }
    Ok(())
}

// ----- the context ---------------------------------------------------------------------------

/// Applies the locations a relink found: the engine implements it by submitting
/// `media.set_location` for each in one document transaction on the doc actor.
pub trait RelinkSink: Send {
    /// Records `batch` atomically (all or none).
    fn relocate(&mut self, batch: Vec<MediaSetLocation>) -> Result<(), CmdError>;
}

/// What the relink commands run against (their engine-command context). The engine builds one
/// per call.
pub struct RelinkCtx {
    /// The current document.
    pub doc: Project,
    /// Reads the candidate files.
    pub prober: Arc<dyn MediaProbe>,
    /// The disk.
    pub files: Arc<dyn RelinkFiles>,
    /// Commits the new locations; `None` for a caller that may only look
    /// (`media.find_missing`).
    pub sink: Option<Box<dyn RelinkSink>>,
    /// The caller's allowlist, the probe timeout and parallelism.
    pub policy: ImportPolicy,
}

impl std::fmt::Debug for RelinkCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelinkCtx")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl RelinkCtx {
    fn check_path(&self, pointer: &str, path: &str) -> Result<(), CmdError> {
        check_abs_path(pointer, path)?;
        if let Some(roots) = &self.policy.allowed_roots
            && !roots.allows(path)
        {
            return Err(CmdError::invalid(
                pointer,
                format!(
                    "`{path}` is outside the folders you may read (the project folder and the \
                     folders the user allowed); {OUTSIDE_ROOTS_HINT}"
                ),
            ));
        }
        Ok(())
    }

    fn probe(&self, paths: &[String]) -> Vec<Option<Probed>> {
        policy::probe_all(&self.prober, paths, &self.policy, &mut || {})
            .into_iter()
            .map(Result::ok)
            .collect()
    }

    fn relocate(&mut self, batch: Vec<MediaSetLocation>) -> Result<(), CmdError> {
        match &mut self.sink {
            Some(sink) => sink.relocate(batch),
            None => Err(CmdError::unsupported(
                "this engine cannot change the project from a relink",
            )),
        }
    }

    /// Whether `asset`'s file is not where the document says.
    fn is_missing(&self, asset: &MediaAsset) -> bool {
        asset
            .path_abs
            .as_deref()
            .is_some_and(|p| !self.files.exists(p))
    }

    /// How many pictures of `run` are not in the folder `dir` (the numbers that had no file at
    /// import are not looked for). One listing of the folder answers for the pictures that are
    /// there; only the others are asked for one by one.
    fn frames_gone(&self, dir: &str, run: &ImageSequence) -> u32 {
        let listed: HashSet<String> = self
            .files
            .list(dir, false)
            .files
            .iter()
            .map(|path| parent_and_name(path).1.to_owned())
            .collect();
        let gone = frame_numbers(run)
            .filter_map(|number| run.file_name(number))
            .filter(|name| !listed.contains(name) && !self.files.exists(&join(dir, name)))
            .count();
        u32::try_from(gone).unwrap_or(u32::MAX)
    }

    /// How many pictures of an image-sequence asset are gone, its first one included; 0 for
    /// any other asset.
    fn asset_frames_gone(&self, asset: &MediaAsset) -> u32 {
        match (run_of(asset), asset.path_abs.as_deref()) {
            (Some(run), Some(first)) => self
                .frames_gone(parent_and_name(first).0, run)
                .max(u32::from(self.is_missing(asset))),
            _ => 0,
        }
    }

    /// Whether a relink has something to find for `asset`: its file is gone, or it is an image
    /// sequence with pictures gone.
    fn needs_relink(&self, asset: &MediaAsset) -> bool {
        self.is_missing(asset) || self.asset_frames_gone(asset) > 0
    }
}

/// The run of an image-sequence asset (ADR-0035); `None` for any other asset.
fn run_of(asset: &MediaAsset) -> Option<&ImageSequence> {
    asset
        .image_sequence
        .as_ref()
        .filter(|_| asset.kind == MediaKind::ImageSequence)
}

/// The numbers of `run` that had a file at import, ascending.
fn frame_numbers(run: &ImageSequence) -> impl Iterator<Item = u32> + '_ {
    run.segments()
        .into_iter()
        .flat_map(|(start, len)| start..start.saturating_add(len))
}

/// Whether `name` is the file name of one of the numbers `run` covers.
fn is_frame_name(run: &ImageSequence, name: &str) -> bool {
    let Some((before, _, after)) = ImageSequence::split_pattern(&run.pattern) else {
        return false;
    };
    name.strip_prefix(before.as_str())
        .and_then(|rest| rest.strip_suffix(after.as_str()))
        .and_then(|digits| digits.parse::<u32>().ok())
        .is_some_and(|number| {
            number >= run.first
                && number - run.first < run.count
                && run.file_name(number).as_deref() == Some(name)
        })
}

/// `path` with `/` separators and in lower case, for telling whether two paths name one file.
fn path_key(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

fn is_file_asset(asset: &MediaAsset) -> bool {
    asset.kind != MediaKind::SequenceRef && asset.path_abs.is_some()
}

/// The file name of a path with either separator, lower-cased for comparing.
fn file_name_key(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_lowercase()
}

/// How a file was recognized as an asset's.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RelinkMatch {
    /// Its content hash is the asset's: the same file, moved or renamed.
    Hash,
    /// Other bytes with the asset's file name, kind and duration (`media.relink_folder`).
    Name,
    /// The caller named it (`media.relink`) and its content differs from what was imported.
    Chosen,
}

/// One asset that was pointed at a file.
#[wire]
pub struct Relinked {
    /// The asset.
    pub media_id: MediaId,
    /// Where the document said its file was.
    pub from: Option<String>,
    /// Where it is now.
    pub to: String,
    /// How the file was recognized.
    pub matched_by: RelinkMatch,
}

// ----- media.relink --------------------------------------------------------------------------

/// Points one media asset at a file: the same file after it was moved or renamed, or another
/// file to use in its place. A replacement must be the same kind of media (video, audio or
/// image) and at least as long as the original, so every clip that uses the asset stays valid;
/// the asset then takes the new file's content hash and probe results. Fails with `Invalid` at
/// `/path` when the file cannot be read or cannot stand in. One undo step puts the old path
/// back. Agents and headless callers may only name files inside the project folder and the
/// folders the user approved.
///
/// For an image sequence, `path` is any picture of the run at its new place, or the folder
/// that holds it: the asset is pointed at the run there, which must have every picture (same
/// file names, same numbers; `Invalid` at `/path` says how many are not there). `to` is the
/// first picture.
#[command(
    name = "media.relink",
    summary = "Point a media asset at a file (the moved original, or a replacement)",
    engine,
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaRelink {
    /// The asset (a file, not a compound clip).
    pub media: MediaId,
    /// Absolute path of the file to use (for an image sequence: one of its pictures at the
    /// new place, or the folder they are in).
    pub path: String,
}

impl MediaRelink {
    /// Points an image sequence at its run in the folder `self.path` names.
    fn relink_run(
        &self,
        ctx: &mut RelinkCtx,
        asset: &MediaAsset,
        run: &ImageSequence,
    ) -> Result<Relinked, CmdError> {
        let invalid = |msg: String| CmdError::invalid("/path", msg);
        let last = run.first.saturating_add(run.count.saturating_sub(1));
        let dir = if ctx.files.is_folder(&self.path) {
            self.path.as_str()
        } else {
            let (dir, name) = parent_and_name(&self.path);
            if !is_frame_name(run, name) {
                return Err(invalid(format!(
                    "`{name}` is not a picture of this image sequence (`{}`, {} to {last}); name \
                     one of its pictures at the new place, or the folder they are in",
                    run.pattern, run.first
                )));
            }
            dir
        };
        let first_name = run
            .file_name(run.first)
            .ok_or_else(|| CmdError::internal("the image sequence has no readable pattern"))?;
        let first = join(dir, &first_name);
        let gone = ctx.frames_gone(dir, run);
        if gone > 0 {
            return Err(invalid(format!(
                "the run is not complete there: {gone} of the pictures `{}` ({} to {last}) are \
                 not in `{dir}`",
                run.pattern, run.first
            )));
        }
        // The run's own hash: the prober's answer for its locator, as at import.
        let probed = policy::probe_all(
            &ctx.prober,
            std::slice::from_ref(&run.locator(&first)),
            &ctx.policy,
            &mut || {},
        )
        .pop()
        .unwrap_or_else(|| Err(super::ProbeError::NotFound(first.clone())))
        .map_err(|e| invalid(e.to_string()))?;
        let same = asset.content_hash.as_deref() == Some(probed.content_hash.as_str());
        ctx.relocate(vec![MediaSetLocation {
            media: self.media,
            path: first.clone(),
            content_hash: (!same).then(|| probed.content_hash.clone()),
            probe: (!same).then(|| probed.probe.clone()),
        }])?;
        Ok(Relinked {
            media_id: self.media,
            from: asset.path_abs.clone(),
            to: first,
            matched_by: if same {
                RelinkMatch::Hash
            } else {
                RelinkMatch::Chosen
            },
        })
    }
}

/// Why the file `probed` cannot stand in for `asset`, if it cannot.
fn stand_in_problem(asset: &MediaAsset, probed: &Probed) -> Option<String> {
    let kind = kind_of_probe(&probed.probe);
    if kind != Some(asset.kind) {
        return Some(format!(
            "the asset is {} media and this file is {}",
            asset.kind,
            kind.map_or_else(|| "not media".to_owned(), |k| format!("{k} media")),
        ));
    }
    match (asset.probe.duration, probed.probe.duration) {
        (Some(old), Some(new)) if new < old => Some(
            "this file is shorter than the original, so clips that use the asset could run past \
             its end"
                .to_owned(),
        ),
        _ => None,
    }
}

impl EngineCommand for MediaRelink {
    type Context = RelinkCtx;
    type Output = Relinked;

    fn run(&self, ctx: &mut RelinkCtx) -> Result<Relinked, CmdError> {
        ctx.check_path("/path", &self.path)?;
        let asset = file_asset(&ctx.doc, "/media", self.media)?.clone();
        if let Some(run) = run_of(&asset) {
            return self.relink_run(ctx, &asset, run);
        }
        let probed = policy::probe_all(
            &ctx.prober,
            std::slice::from_ref(&self.path),
            &ctx.policy,
            &mut || {},
        )
        .pop()
        .unwrap_or_else(|| Err(super::ProbeError::NotFound(self.path.clone())))
        .map_err(|e| CmdError::invalid("/path", e.to_string()))?;
        let same = asset.content_hash.as_deref() == Some(probed.content_hash.as_str());
        if !same && let Some(problem) = stand_in_problem(&asset, &probed) {
            return Err(CmdError::invalid(
                "/path",
                format!("`{}` cannot replace this asset: {problem}", self.path),
            ));
        }
        ctx.relocate(vec![MediaSetLocation {
            media: self.media,
            path: self.path.clone(),
            content_hash: (!same).then(|| probed.content_hash.clone()),
            probe: (!same).then(|| probed.probe.clone()),
        }])?;
        Ok(Relinked {
            media_id: self.media,
            from: asset.path_abs,
            to: self.path.clone(),
            matched_by: if same {
                RelinkMatch::Hash
            } else {
                RelinkMatch::Chosen
            },
        })
    }
}

// ----- media.relink_folder -------------------------------------------------------------------

/// Result of `media.relink_folder`.
#[wire]
pub struct MediaRelinkFolderOutput {
    /// The assets whose file was found, by asset id.
    pub relinked: Vec<Relinked>,
    /// The assets (of `media`, or of the project) that are still missing or incomplete.
    pub still_missing: Vec<MediaId>,
    /// The number of files that were read (an image sequence's run counts once).
    pub scanned: u32,
    /// The folder holds more files than one call reads (2,000): run it on its subfolders.
    pub truncated: bool,
}

/// Searches a folder for the files of missing media and points each asset at its file, all in
/// one undo step. A file is recognized by its content, so moved and renamed files are found;
/// an asset whose content is nowhere in the folder takes the one file with its name, kind and
/// duration, if there is exactly one. Assets whose file is where the project says are left
/// alone. An image sequence is found as a whole: by the file name of its first picture, in a
/// folder that holds every picture of the run (an incomplete run is passed over), by content
/// when the pictures are the same and else when it is the only such run. Reads at most 2,000
/// files per call (16 levels down, no links, no hidden files).
/// Agents and headless callers may only search the project folder and the folders the user
/// approved (`Invalid` at `/folder` otherwise).
#[command(
    name = "media.relink_folder",
    summary = "Find the files of missing media in a folder (by content, then name) and relink them",
    engine,
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaRelinkFolder {
    /// Absolute path of the folder on disk to search.
    pub folder: String,
    /// Also search the folders inside it (default true).
    pub recursive: Option<bool>,
    /// Only relink these assets (1..=256, no repeats); absent: every missing asset.
    #[serde(default)]
    pub media: Option<Vec<MediaId>>,
}

impl MediaRelinkFolder {
    /// The missing assets this call is about, by id.
    fn targets(&self, ctx: &RelinkCtx) -> Result<Vec<MediaAsset>, CmdError> {
        let Some(media) = &self.media else {
            return Ok(ctx
                .doc
                .media
                .values()
                .filter(|m| is_file_asset(m) && ctx.needs_relink(m))
                .cloned()
                .collect());
        };
        if media.is_empty() || media.len() > MAX_RELINK_MEDIA {
            return Err(CmdError::invalid(
                "/media",
                format!("name 1..={MAX_RELINK_MEDIA} assets"),
            ));
        }
        let mut targets = BTreeMap::new();
        for (i, id) in media.iter().enumerate() {
            let pointer = format!("/media/{i}");
            if media[..i].contains(id) {
                return Err(CmdError::invalid(
                    pointer,
                    format!("`{id}` is listed twice"),
                ));
            }
            let asset = file_asset(&ctx.doc, &pointer, *id)?;
            if ctx.needs_relink(asset) {
                targets.insert(*id, asset.clone());
            }
        }
        Ok(targets.into_values().collect())
    }
}

/// A probed file of the searched folder.
struct Candidate {
    path: String,
    probed: Probed,
    taken: bool,
}

/// For each asset of `targets` no hash matched: the one untaken candidate with its file name,
/// kind and duration, if there is exactly one.
fn match_by_name(
    targets: &[MediaAsset],
    candidates: &mut [Candidate],
    found: &mut BTreeMap<MediaId, (usize, RelinkMatch)>,
) {
    for asset in targets {
        if found.contains_key(&asset.id) {
            continue;
        }
        let Some(name) = asset.path_abs.as_deref().map(file_name_key) else {
            continue;
        };
        let mut alike = candidates.iter().enumerate().filter(|(_, c)| {
            !c.taken
                && file_name_key(&c.path) == name
                && kind_of_probe(&c.probed.probe) == Some(asset.kind)
                && c.probed.probe.duration == asset.probe.duration
        });
        if let (Some((index, _)), None) = (alike.next(), alike.next()) {
            candidates[index].taken = true;
            found.insert(asset.id, (index, RelinkMatch::Name));
        }
    }
}

/// Finds the runs of the image sequences among `targets` in the searched `folder`: for each,
/// the folders that hold a file with the name of its first picture (the searched folder
/// itself, then the listed `paths`) and every other picture of the run. A run whose content
/// hash is the asset's wins; an asset without one takes the only complete run nothing else
/// took. Returns the first picture and the probe of each run found, and the number of runs
/// that were read.
fn match_runs(
    ctx: &RelinkCtx,
    folder: &str,
    paths: &[String],
    targets: &[MediaAsset],
) -> (BTreeMap<MediaId, (String, Probed, RelinkMatch)>, u32) {
    let mut found = BTreeMap::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut by_name: Vec<(MediaId, Vec<(String, Probed)>)> = Vec::new();
    let mut scanned = 0_u32;
    for asset in targets {
        let Some(run) = run_of(asset) else { continue };
        let Some(first_name) = run.file_name(run.first) else {
            continue;
        };
        let here = asset.path_abs.as_deref().map(path_key);
        let direct = join(folder, &first_name);
        let mut firsts: Vec<String> = Vec::new();
        let listed = paths
            .iter()
            .filter(|p| file_name_key(p) == first_name.to_lowercase());
        for path in ctx
            .files
            .exists(&direct)
            .then_some(&direct)
            .into_iter()
            .chain(listed)
        {
            let key = path_key(path);
            let known = firsts.iter().any(|f| path_key(f) == key);
            if !known
                && Some(&key) != here.as_ref()
                && ctx.frames_gone(parent_and_name(path).0, run) == 0
            {
                firsts.push(path.clone());
            }
        }
        let locators: Vec<String> = firsts.iter().map(|first| run.locator(first)).collect();
        scanned = scanned.saturating_add(u32::try_from(locators.len()).unwrap_or(u32::MAX));
        let mut complete: Vec<(String, Probed)> = firsts
            .into_iter()
            .zip(ctx.probe(&locators))
            .filter_map(|(first, probed)| Some((first, probed?)))
            .collect();
        let same = complete.iter().position(|(first, probed)| {
            asset.content_hash.as_deref() == Some(probed.content_hash.as_str())
                && !taken.contains(&path_key(first))
        });
        if let Some(at) = same {
            let (first, probed) = complete.swap_remove(at);
            taken.insert(path_key(&first));
            found.insert(asset.id, (first, probed, RelinkMatch::Hash));
        } else {
            by_name.push((asset.id, complete));
        }
    }
    for (id, complete) in by_name {
        let mut free = complete
            .into_iter()
            .filter(|(first, _)| !taken.contains(&path_key(first)));
        if let (Some((first, probed)), None) = (free.next(), free.next()) {
            taken.insert(path_key(&first));
            found.insert(id, (first, probed, RelinkMatch::Name));
        }
    }
    (found, scanned)
}

impl EngineCommand for MediaRelinkFolder {
    type Context = RelinkCtx;
    type Output = MediaRelinkFolderOutput;

    fn run(&self, ctx: &mut RelinkCtx) -> Result<MediaRelinkFolderOutput, CmdError> {
        ctx.check_path("/folder", &self.folder)?;
        if !ctx.files.is_folder(&self.folder) {
            return Err(CmdError::invalid(
                "/folder",
                format!("`{}` is not a folder that can be read", self.folder),
            ));
        }
        let targets = self.targets(ctx)?;
        let mut out = MediaRelinkFolderOutput {
            relinked: Vec::new(),
            still_missing: Vec::new(),
            scanned: 0,
            truncated: false,
        };
        if targets.is_empty() {
            return Ok(out);
        }
        let listing = ctx.files.list(&self.folder, self.recursive.unwrap_or(true));
        out.truncated = listing.truncated || listing.files.len() > MAX_RELINK_FILES;
        let mut paths = listing.files;
        paths.truncate(MAX_RELINK_FILES);

        // Image sequences are found as whole runs; the files below are for the other assets.
        let (runs, run_scans) = match_runs(ctx, &self.folder, &paths, &targets);
        out.scanned = run_scans;
        let all = targets;
        let targets: Vec<MediaAsset> = all
            .iter()
            .filter(|m| run_of(m).is_none())
            .cloned()
            .collect();

        // Files named like a missing asset are read first: usually that is all it takes.
        let wanted_names: HashSet<String> = targets
            .iter()
            .filter_map(|m| m.path_abs.as_deref().map(file_name_key))
            .collect();
        let (named, others): (Vec<String>, Vec<String>) = paths
            .into_iter()
            .partition(|p| wanted_names.contains(&file_name_key(p)));
        let by_hash: HashMap<&str, MediaId> = targets
            .iter()
            .filter_map(|m| Some((m.content_hash.as_deref()?, m.id)))
            .collect();

        let mut candidates: Vec<Candidate> = Vec::new();
        let mut found: BTreeMap<MediaId, (usize, RelinkMatch)> = BTreeMap::new();
        for batch in [named, others] {
            if found.len() == targets.len() || batch.is_empty() {
                continue;
            }
            let probed = ctx.probe(&batch);
            out.scanned = out
                .scanned
                .saturating_add(u32::try_from(batch.len()).unwrap_or(u32::MAX));
            for (path, probed) in batch.into_iter().zip(probed) {
                let Some(probed) = probed else { continue };
                let index = candidates.len();
                let hit = by_hash
                    .get(probed.content_hash.as_str())
                    .copied()
                    .filter(|id| !found.contains_key(id));
                if let Some(id) = hit {
                    found.insert(id, (index, RelinkMatch::Hash));
                }
                candidates.push(Candidate {
                    path,
                    probed,
                    taken: hit.is_some(),
                });
            }
        }

        match_by_name(&targets, &mut candidates, &mut found);

        let mut batch = Vec::with_capacity(found.len() + runs.len());
        for asset in &all {
            let (path, probed, matched_by) = if let Some((first, probed, by)) = runs.get(&asset.id)
            {
                (first, probed, *by)
            } else if let Some(&(index, by)) = found.get(&asset.id) {
                (&candidates[index].path, &candidates[index].probed, by)
            } else {
                out.still_missing.push(asset.id);
                continue;
            };
            let new_content = matched_by != RelinkMatch::Hash;
            batch.push(MediaSetLocation {
                media: asset.id,
                path: path.clone(),
                content_hash: new_content.then(|| probed.content_hash.clone()),
                probe: new_content.then(|| probed.probe.clone()),
            });
            out.relinked.push(Relinked {
                media_id: asset.id,
                from: asset.path_abs.clone(),
                to: path.clone(),
                matched_by,
            });
        }
        if !batch.is_empty() {
            ctx.relocate(batch)?;
        }
        Ok(out)
    }
}

// ----- media.find_missing --------------------------------------------------------------------

/// Whether an asset's file is usable (`MediaAsset.status` of WP-2F.03; never saved).
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MediaStatus {
    /// The file is where the project says.
    Ok,
    /// There is no readable file at the asset's path.
    Missing,
    /// There is a file, but its content is not what was imported (`verify` only).
    Changed,
    /// An image sequence whose first picture is there while other pictures of the run are
    /// gone (`missing_frames` says how many).
    Incomplete,
}

/// An asset whose file is missing or changed, or an image sequence with pictures gone.
#[wire]
pub struct MissingMedia {
    /// The asset.
    pub media_id: MediaId,
    /// The name the bin shows.
    pub name: String,
    /// Where the project says its file is.
    pub path: Option<String>,
    /// What is wrong: `missing`, `changed` or `incomplete`.
    pub status: MediaStatus,
    /// For an image sequence: how many of its pictures are gone, the first one included (the
    /// numbers that had no file at import are not counted). Absent for other media.
    pub missing_frames: Option<u32>,
}

/// Result of `media.find_missing`.
#[wire]
pub struct MediaFindMissingOutput {
    /// The assets whose file is missing or changed and the incomplete image sequences, by id.
    pub missing: Vec<MissingMedia>,
    /// The number of media files that were checked.
    pub checked: u32,
}

/// Lists the media whose file is not where the project says (moved, renamed, deleted or on a
/// drive that is not connected). With `verify` it also reads every file that is there and
/// reports the ones whose content is no longer what was imported as `changed`. An image
/// sequence is `missing` when its first picture is gone and `incomplete` when the first is
/// there but other pictures of the run are not; `missing_frames` counts them. Relink them
/// with `media.relink_folder` or `media.relink`.
#[wire]
pub struct MediaFindMissing {
    /// Also read the files that exist and report changed content (slower; default false).
    pub verify: Option<bool>,
}

impl Described for MediaFindMissing {
    const SPEC: CommandSpec = CommandSpec {
        name: "media.find_missing",
        summary: "List the media whose file is missing (or, with verify, changed) on this computer",
        kind: EntryKind::Engine,
        undoable: false,
        view: false,
        scope: Scope::Any,
        mcp: McpExposure::Catalog,
        annotations: Annotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        secret_fields: &[],
        rust_type: "MediaFindMissing",
    };
    const JOURNALED: bool = false;
}

impl Registrable for MediaFindMissing {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<MediaFindMissing>().map(drop)
    }
}

impl EngineCommand for MediaFindMissing {
    type Context = RelinkCtx;
    type Output = MediaFindMissingOutput;

    fn run(&self, ctx: &mut RelinkCtx) -> Result<MediaFindMissingOutput, CmdError> {
        let assets: Vec<&MediaAsset> = ctx
            .doc
            .media
            .values()
            .filter(|m| is_file_asset(m))
            .collect();
        let mut status: BTreeMap<MediaId, MediaStatus> = BTreeMap::new();
        let mut frames: BTreeMap<MediaId, u32> = BTreeMap::new();
        let mut present: Vec<&MediaAsset> = Vec::new();
        for asset in &assets {
            let gone = ctx.asset_frames_gone(asset);
            if gone > 0 {
                frames.insert(asset.id, gone);
            }
            if ctx.is_missing(asset) {
                status.insert(asset.id, MediaStatus::Missing);
            } else if gone > 0 {
                status.insert(asset.id, MediaStatus::Incomplete);
            } else {
                present.push(asset);
            }
        }
        if self.verify == Some(true) {
            // An image sequence is read through its locator, as at import: its hash is its own.
            let paths: Vec<String> = present.iter().filter_map(|m| m.source_path()).collect();
            for (asset, probed) in present.iter().zip(ctx.probe(&paths)) {
                match probed {
                    None => {
                        status.insert(asset.id, MediaStatus::Missing);
                    }
                    Some(p) if asset.content_hash.as_deref() != Some(p.content_hash.as_str()) => {
                        status.insert(asset.id, MediaStatus::Changed);
                    }
                    Some(_) => {}
                }
            }
        }
        Ok(MediaFindMissingOutput {
            missing: assets
                .iter()
                .filter_map(|m| {
                    Some(MissingMedia {
                        media_id: m.id,
                        name: asset_name(m).into_owned(),
                        path: m.path_abs.clone(),
                        status: *status.get(&m.id)?,
                        missing_frames: frames.get(&m.id).copied(),
                    })
                })
                .collect(),
            checked: u32::try_from(assets.len()).unwrap_or(u32::MAX),
        })
    }
}

/// Registers the relink commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<MediaSetLocation>()?
        .register::<MediaRelink>()?
        .register::<MediaRelinkFolder>()?
        .register::<MediaFindMissing>()?;
    Ok(())
}
