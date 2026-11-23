// SPDX-License-Identifier: GPL-3.0-or-later
//! The `media` namespace (WP-1B.05, WP-1D.07; arch §5.1, §7): `media.register`,
//! `media.import`, `media.list`, `media.remove`, and the `media.indexed` /
//! `media.analysis_failed` events of background analysis (which never mutates the document).
//!
//! **Import (arch §7, R21).** `media.import` returns `media_ids` synchronously: it checks the
//! caller's path allowlist, probes the files through a [`MediaProbe`] (in parallel, each with a
//! timeout; [`ImportPolicy`]) and registers the readable ones with `media.register` in one
//! transaction through a [`MediaSink`]; unreadable files are reported in `failed[]`. It then
//! queues background analysis jobs for the new assets ([`Analyzer`], [`AnalysisStage`]) and
//! returns their `job_ids`, which callers await with `job.wait`. It runs against an
//! [`ImportCtx`], so the engine decides how registrations are committed and who may read
//! which folders.

mod analysis;
mod policy;
mod probe;

use std::sync::Arc;

use cutweave_commands::{
    CmdError, Command, EngineCommand, Query, QueryCtx, Registry, RegistryError, Tx, command, query,
    wire,
};
use cutweave_jobs::JobId;
use cutweave_model::{Duration, MediaAsset, MediaId, MediaKind, ProbeInfo, Project, StreamKind};

pub use analysis::{
    AnalysisInput, AnalysisSink, AnalysisStage, AnalysisStages, Analyzer, EVENTS,
    MediaAnalysisFailed, MediaEvent, MediaIndexed, StageError,
};
pub use policy::{AllowedRoots, DEFAULT_PROBE_PARALLELISM, DEFAULT_PROBE_TIMEOUT, ImportPolicy};
pub use probe::{FakeProbe, MediaProbe, ProbeError, Probed};

use crate::util::check_abs_path;

/// Most files one `media.import` call accepts.
pub const MAX_IMPORT_PATHS: usize = 256;
/// Default and largest page size of `media.list`.
pub const DEFAULT_LIST_LIMIT: u32 = 50;
/// Largest page size of `media.list`.
pub const MAX_LIST_LIMIT: u32 = 500;
/// Longest accepted content hash.
pub const MAX_HASH_CHARS: usize = 128;

/// Registers the `media.*` commands and queries.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<MediaRegister>()?
        .register::<MediaImport>()?
        .register::<MediaList>()?
        .register::<MediaRemove>()?;
    Ok(())
}

/// The kind a probe result describes: pictures with a duration are video, pictures without
/// one are stills, sound alone is audio. `None` if there is nothing playable.
#[must_use]
pub fn kind_of_probe(probe: &ProbeInfo) -> Option<MediaKind> {
    let has = |k| probe.streams.iter().any(|s| s.kind == k);
    if has(StreamKind::Video) {
        Some(if probe.duration.is_some() {
            MediaKind::Video
        } else {
            MediaKind::Image
        })
    } else if has(StreamKind::Audio) || probe.audio.is_some() {
        Some(MediaKind::Audio)
    } else {
        None
    }
}

/// Whether a video file also carries sound (so inserting it creates a linked audio clip).
#[must_use]
pub fn has_audio_stream(media: &MediaAsset) -> bool {
    media.kind == MediaKind::Video
        && (media.probe.audio.is_some()
            || media
                .probe
                .streams
                .iter()
                .any(|s| s.kind == StreamKind::Audio))
}

// ----- media.register ------------------------------------------------------------------------

/// Result of `media.register`.
#[wire]
#[derive(Copy)]
pub struct MediaRegistered {
    /// The asset: the new one, or the existing one with the same content hash.
    pub media_id: MediaId,
    /// What the asset is.
    pub kind: MediaKind,
    /// `false` when an asset with the same content hash already existed and nothing changed.
    pub created: bool,
}

/// Registers a probed media file. Internal plumbing of `media.import` and the import job
/// (WP-1D.07): the kind (video, audio or image) derives from the probe streams. If an asset
/// with the same `content_hash` already exists it is returned unchanged (`created: false`).
#[command(
    name = "media.register",
    summary = "Register a probed media file (used by import; deduplicates by content hash)",
    undoable,
    mcp = "hidden",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaRegister {
    /// ID for the new asset; minted by the engine when absent. Must not exist yet.
    pub media_id: Option<MediaId>,
    /// Absolute path of the file.
    pub path: String,
    /// What probing found.
    pub probe: ProbeInfo,
    /// Content hash used to relink and deduplicate (non-empty, no whitespace).
    pub content_hash: String,
}

fn check_hash(hash: &str) -> Result<(), CmdError> {
    if hash.is_empty() || hash.len() > MAX_HASH_CHARS || hash.chars().any(char::is_whitespace) {
        return Err(CmdError::invalid(
            "/content_hash",
            format!("the hash must be 1..={MAX_HASH_CHARS} characters without whitespace"),
        ));
    }
    Ok(())
}

fn by_hash<'a>(doc: &'a Project, hash: &str) -> Option<&'a MediaAsset> {
    doc.media
        .values()
        .find(|m| m.content_hash.as_deref() == Some(hash))
}

impl MediaRegister {
    fn kind(&self) -> Result<MediaKind, CmdError> {
        check_abs_path("/path", &self.path)?;
        check_hash(&self.content_hash)?;
        if let Some(d) = self.probe.duration
            && d.is_zero()
        {
            return Err(CmdError::invalid(
                "/probe/duration",
                "the duration must be positive",
            ));
        }
        if !matches!(self.probe.rotation, 0 | 90 | 180 | 270) {
            return Err(CmdError::invalid(
                "/probe/rotation",
                "the rotation must be 0, 90, 180 or 270",
            ));
        }
        kind_of_probe(&self.probe).ok_or_else(|| {
            CmdError::invalid(
                "/probe/streams",
                "the file has no video or audio stream, so it cannot be used as media",
            )
        })
    }
}

impl Command for MediaRegister {
    type Output = MediaRegistered;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.kind()?;
        if let Some(id) = self.media_id
            && ctx.doc().media.contains_key(&id)
            && by_hash(ctx.doc(), &self.content_hash).is_none_or(|m| m.id != id)
        {
            return Err(CmdError::invalid(
                "/media_id",
                format!("media `{id}` already exists"),
            ));
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaRegistered, CmdError> {
        self.validate(&tx.query_ctx())?;
        let kind = self.kind()?;
        if let Some(existing) = by_hash(tx.doc(), &self.content_hash) {
            return Ok(MediaRegistered {
                media_id: existing.id,
                kind: existing.kind,
                created: false,
            });
        }
        let id = match self.media_id {
            Some(id) => id,
            None => tx.new_id(),
        };
        let mut media = MediaAsset::file(id, kind, self.path.clone(), self.probe.clone());
        media.content_hash = Some(self.content_hash.clone());
        tx.insert_media(&media)?;
        Ok(MediaRegistered {
            media_id: id,
            kind,
            created: true,
        })
    }
}

/// Applies `batch` in order inside `tx` and returns each result. For [`MediaSink`]
/// implementations: the engine runs it in one transaction, so the import is one undo step. A
/// failing entry fails the batch (the caller cancels the transaction).
pub fn register_batch(
    tx: &mut Tx<'_>,
    batch: &[MediaRegister],
) -> Result<Vec<MediaRegistered>, CmdError> {
    batch
        .iter()
        .enumerate()
        .map(|(i, r)| {
            r.validate(&tx.query_ctx())
                .and_then(|()| r.apply(tx))
                .map_err(|e| match e {
                    CmdError::Invalid { path, msg } => {
                        CmdError::invalid(format!("/paths/{i}{path}"), msg)
                    }
                    other => other,
                })
        })
        .collect()
}

// ----- media.import --------------------------------------------------------------------------

/// Applies registrations produced by `media.import`: the engine implements it by running
/// [`register_batch`] in one document transaction on the doc actor.
pub trait MediaSink: Send {
    /// Registers `batch` atomically (all or none) and returns one result per entry.
    fn register(&mut self, batch: Vec<MediaRegister>) -> Result<Vec<MediaRegistered>, CmdError>;
}

/// What `media.import` runs against (the [`EngineCommand::Context`]). The engine builds one
/// per call, with the caller's [`ImportPolicy`].
pub struct ImportCtx {
    /// Reads the files.
    pub prober: Arc<dyn MediaProbe>,
    /// Commits the registrations.
    pub sink: Box<dyn MediaSink>,
    /// The caller's allowlist, the probe timeout and parallelism.
    pub policy: ImportPolicy,
    /// Queues background analysis of new assets; `None` queues nothing.
    pub analysis: Option<Analyzer>,
}

impl ImportCtx {
    /// A context from a prober and a sink, with the default (unrestricted) policy and no
    /// analysis.
    #[must_use]
    pub fn new(prober: Arc<dyn MediaProbe>, sink: Box<dyn MediaSink>) -> ImportCtx {
        ImportCtx {
            prober,
            sink,
            policy: ImportPolicy::default(),
            analysis: None,
        }
    }

    /// The same context with `policy`.
    #[must_use]
    pub fn with_policy(mut self, policy: ImportPolicy) -> ImportCtx {
        self.policy = policy;
        self
    }

    /// The same context, queueing analysis jobs through `analyzer`.
    #[must_use]
    pub fn with_analysis(mut self, analyzer: Analyzer) -> ImportCtx {
        self.analysis = Some(analyzer);
        self
    }
}

/// One imported file.
#[wire]
pub struct ImportedMedia {
    /// The path as requested.
    pub path: String,
    /// The asset; an existing one if the same content was imported before.
    pub media_id: MediaId,
    /// What the asset is.
    pub kind: MediaKind,
    /// `true` when the content was already in the project and nothing was added.
    pub duplicate: bool,
}

/// Why a file was not imported.
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportFailureCode {
    /// There is no readable file at the path.
    NotFound,
    /// The file holds no playable audio or video.
    Unsupported,
    /// Probing took longer than the per-file timeout.
    TimedOut,
    /// Reading the file failed.
    Io,
    /// The probe result cannot be registered (for example a zero duration).
    Invalid,
}

/// A file that was not imported (no asset was created for it).
#[wire]
pub struct ImportFailure {
    /// The path as requested.
    pub path: String,
    /// Machine-readable reason.
    pub code: ImportFailureCode,
    /// Human-readable reason.
    pub error: String,
}

/// Result of `media.import`.
#[wire]
pub struct MediaImportOutput {
    /// One entry per imported path, in request order.
    pub media: Vec<ImportedMedia>,
    /// The assets of `media`, in the same order (duplicates name the existing asset).
    pub media_ids: Vec<MediaId>,
    /// The paths that could not be imported, in request order.
    pub failed: Vec<ImportFailure>,
    /// Background analysis jobs queued for the new assets (wait with `job.wait`); empty when
    /// no analysis stage is installed.
    pub job_ids: Vec<JobId>,
}

/// Imports media files and returns their `media_ids` at once (arch §7, R21). Each file is
/// probed synchronously (up to 4 at a time, 5 s each by default) and every readable one is
/// registered in one transaction (one undo step); files that are missing, unsupported, corrupt
/// or too slow to probe are listed in `failed` and get no asset. Files whose content is
/// already in the project are returned as duplicates. Background analysis (frame index,
/// thumbnails, waveform) then runs as jobs whose ids are returned in `job_ids`. Agents and
/// headless callers may only import from the project folder and user-approved folders; any
/// other path fails the call with `Invalid` at `/paths/<i>`.
#[command(
    name = "media.import",
    summary = "Import media files: probe and register them now, return media ids and analysis job ids",
    engine,
    undoable = false,
    mcp = "core",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaImport {
    /// Absolute paths of the files (1..=256, no repeats).
    pub paths: Vec<String>,
}

impl MediaImport {
    fn check(&self, policy: &ImportPolicy) -> Result<(), CmdError> {
        if self.paths.is_empty() || self.paths.len() > MAX_IMPORT_PATHS {
            return Err(CmdError::invalid(
                "/paths",
                format!("give 1..={MAX_IMPORT_PATHS} paths"),
            ));
        }
        for (i, path) in self.paths.iter().enumerate() {
            let pointer = format!("/paths/{i}");
            check_abs_path(&pointer, path)?;
            if self.paths[..i].contains(path) {
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
                         and folders the user approved); ask the user to approve its folder"
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// Rewrites the `/paths/<batch index>` pointer of a sink error (see [`register_batch`]) to the
/// file's index in the request, since files that failed to probe are not in the batch.
fn request_pointer(error: CmdError, origin: &[usize]) -> CmdError {
    let CmdError::Invalid { path, msg } = error else {
        return error;
    };
    let rewritten = path.strip_prefix("/paths/").and_then(|rest| {
        let (index, tail) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let original = origin.get(index.parse::<usize>().ok()?)?;
        Some(format!("/paths/{original}{tail}"))
    });
    CmdError::Invalid {
        path: rewritten.unwrap_or(path),
        msg,
    }
}

fn failure(path: &str, error: &ProbeError) -> ImportFailure {
    let code = match error {
        ProbeError::NotFound(_) => ImportFailureCode::NotFound,
        ProbeError::Unsupported { .. } => ImportFailureCode::Unsupported,
        ProbeError::TimedOut(_) => ImportFailureCode::TimedOut,
        ProbeError::Io { .. } => ImportFailureCode::Io,
    };
    ImportFailure {
        path: path.to_owned(),
        code,
        error: error.to_string(),
    }
}

impl EngineCommand for MediaImport {
    type Context = ImportCtx;
    type Output = MediaImportOutput;

    fn run(&self, ctx: &mut ImportCtx) -> Result<MediaImportOutput, CmdError> {
        self.check(&ctx.policy)?;
        let probed = policy::probe_all(&ctx.prober, &self.paths, &ctx.policy);
        let mut batch = Vec::with_capacity(self.paths.len());
        // The request index of each batch entry (failed files are not in the batch).
        let mut origin = Vec::with_capacity(self.paths.len());
        let mut failed = Vec::new();
        for (i, (path, result)) in self.paths.iter().zip(probed).enumerate() {
            match result {
                Ok(probed) => {
                    let register = MediaRegister {
                        media_id: None,
                        path: path.clone(),
                        probe: probed.probe,
                        content_hash: probed.content_hash,
                    };
                    match register.kind() {
                        Ok(_) => {
                            batch.push(register);
                            origin.push(i);
                        }
                        Err(e) => failed.push(ImportFailure {
                            path: path.clone(),
                            code: ImportFailureCode::Invalid,
                            error: format!("`{path}` cannot be imported: {}", e.message()),
                        }),
                    }
                }
                Err(e) => failed.push(failure(path, &e)),
            }
        }
        let results = if batch.is_empty() {
            Vec::new()
        } else {
            ctx.sink
                .register(batch.clone())
                .map_err(|e| request_pointer(e, &origin))?
        };
        if results.len() != batch.len() {
            return Err(CmdError::internal(
                "the media sink returned a different number of results than requested",
            ));
        }
        let mut out = MediaImportOutput {
            media: Vec::with_capacity(batch.len()),
            media_ids: Vec::with_capacity(batch.len()),
            failed,
            job_ids: Vec::new(),
        };
        for (register, r) in batch.into_iter().zip(results) {
            if r.created
                && let Some(analyzer) = &ctx.analysis
            {
                out.job_ids.extend(analyzer.spawn(AnalysisInput {
                    media_id: r.media_id,
                    kind: r.kind,
                    path: register.path.clone(),
                    content_hash: register.content_hash,
                    probe: register.probe,
                }));
            }
            out.media_ids.push(r.media_id);
            out.media.push(ImportedMedia {
                path: register.path,
                media_id: r.media_id,
                kind: r.kind,
                duplicate: !r.created,
            });
        }
        Ok(out)
    }
}

// ----- media.list ----------------------------------------------------------------------------

/// Summary of one media asset.
#[wire]
pub struct MediaSummary {
    /// The asset.
    pub id: MediaId,
    /// What it is.
    pub kind: MediaKind,
    /// Absolute path when it was last found (file kinds).
    pub path: Option<String>,
    /// Playable length, if known.
    pub duration: Option<Duration>,
    /// Whether it has pictures.
    pub has_video: bool,
    /// Whether it has sound.
    pub has_audio: bool,
}

/// Result of `media.list`.
#[wire]
pub struct MediaListOutput {
    /// The page, ordered by ID.
    pub media: Vec<MediaSummary>,
    /// Pass as `cursor` to get the next page; absent on the last page.
    pub next_cursor: Option<MediaId>,
    /// Number of assets in the project.
    pub total: u32,
}

/// Lists the media of the project, ordered by ID, one page at a time.
#[query(
    name = "media.list",
    summary = "List the project's media (paged, ordered by id)",
    mcp = "core"
)]
pub struct MediaList {
    /// Page size, `1..=500` (default 50).
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page; the page starts after this ID.
    pub cursor: Option<MediaId>,
}

impl Query for MediaList {
    type Output = MediaListOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<MediaListOutput, CmdError> {
        let limit = self.limit.unwrap_or(DEFAULT_LIST_LIMIT);
        if !(1..=MAX_LIST_LIMIT).contains(&limit) {
            return Err(CmdError::invalid(
                "/limit",
                format!("the limit must be 1..={MAX_LIST_LIMIT}"),
            ));
        }
        let limit = limit as usize;
        let doc = ctx.doc();
        let mut rest = doc
            .media
            .iter()
            .filter(|(id, _)| self.cursor.is_none_or(|c| **id > c))
            .map(|(_, m)| m);
        let media: Vec<MediaSummary> = rest
            .by_ref()
            .take(limit)
            .map(|m| MediaSummary {
                id: m.id,
                kind: m.kind,
                path: m.path_abs.clone(),
                duration: m.probe.duration,
                has_video: m.has_video(),
                has_audio: m.has_audio(),
            })
            .collect();
        let next_cursor = rest.next().and(media.last().map(|m| m.id));
        Ok(MediaListOutput {
            media,
            next_cursor,
            total: u32::try_from(doc.media.len()).unwrap_or(u32::MAX),
        })
    }
}

// ----- media.remove --------------------------------------------------------------------------

/// Result of `media.remove`.
#[wire]
#[derive(Copy)]
pub struct MediaRemoveOutput {
    /// The removed asset.
    pub media_id: MediaId,
}

/// Removes a media asset from the project (the file on disk is untouched). Fails with
/// `Invalid` while any clip uses it; delete those clips first.
#[command(
    name = "media.remove",
    summary = "Remove an unused media asset from the project",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct MediaRemove {
    /// The asset to remove.
    pub id: MediaId,
}

fn uses(doc: &Project, id: MediaId) -> usize {
    doc.sequences
        .values()
        .flat_map(|s| s.tracks.values())
        .flat_map(|t| t.items.iter())
        .filter(|i| i.media() == Some(id))
        .count()
}

impl Command for MediaRemove {
    type Output = MediaRemoveOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        let media = ctx.media(self.id)?;
        let n = uses(ctx.doc(), self.id);
        if n > 0 {
            return Err(CmdError::invalid(
                "/id",
                format!(
                    "media `{}` is used by {n} clip(s); delete them first",
                    media.id
                ),
            ));
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaRemoveOutput, CmdError> {
        self.validate(&tx.query_ctx())?;
        tx.remove_media(self.id)?;
        Ok(MediaRemoveOutput { media_id: self.id })
    }
}
