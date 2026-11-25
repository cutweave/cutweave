// SPDX-License-Identifier: GPL-3.0-or-later
//! The `export` namespace (WP-1E.03; arch §5.1, §7, §8, §14): `export.start`.
//!
//! `export.start` checks the request against the **committed** document of the moment
//! ([`ExportCtx::snapshot`]: later edits never change an export), resolves it into an
//! [`ExportPlan`] (sequence, frame-aligned range, output size, frame rate, frame count,
//! quality) and hands the plan to an [`ExportStarter`], which queues the export job and returns
//! its id. Rendering, mixing and encoding belong to the engine (`cutweave-engine`, module
//! `export`); this crate knows neither the GPU nor the media backend. `export.status` is
//! `job.status` on the returned job.
//!
//! **Output path** rules (checked here, before anything is queued): an absolute path ending in
//! `.mp4`, inside the caller's allowed folders ([`ExportCtx::allowed_roots`], the same rule as
//! `media.import`), never the source file of a media asset of the project, in an existing
//! folder the engine can write to, and an existing file only with `overwrite: true` (the
//! encoder replaces it when the export succeeds; a failed or cancelled export leaves it as it
//! was).

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use cutweave_commands::{
    CmdError, DocSnapshot, EngineCommand, Registry, RegistryError, Rev, command, wire,
};
use cutweave_jobs::JobId;
use cutweave_model::{
    Duration, EntityKind, FrameRate, Project, Rounding, Sequence, SequenceId, Time, TimeInput,
};

pub mod presets;

use crate::media::{AllowedRoots, OUTSIDE_ROOTS_HINT};
use crate::util::{check_abs_path, resolve_frame_time};
use presets::{PlanRate, PresetStore, PresetWarning};

/// The preset's bounding box: 1920×1080 for landscape sequences (1080×1920 for portrait).
pub const PRESET_LONG_SIDE: u32 = 1920;
/// The short side of the preset's bounding box.
pub const PRESET_SHORT_SIDE: u32 = 1080;
/// Smallest output width or height.
pub const MIN_EXPORT_SIDE: u32 = 16;
/// Largest output width or height.
pub const MAX_EXPORT_SIDE: u32 = 8192;
/// Fastest output frame rate.
pub const MAX_EXPORT_FPS: u32 = 240;

/// Registers `export.start`.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register::<ExportStart>()?;
    presets::register(registry)?;
    Ok(())
}

// ----- params ----------------------------------------------------------------------------------

/// The part of the sequence to export: `[in, out)`, both snapped to the nearest frame.
#[wire]
pub struct ExportRange {
    /// Start: flicks, "2.5s", "f:60" or a timecode.
    #[serde(rename = "in")]
    pub start: TimeInput,
    /// End, exclusive: at most the sequence end.
    #[serde(rename = "out")]
    pub end: TimeInput,
}

/// How hard the encoder works for picture quality (and file size).
#[wire]
#[derive(Copy, Default, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ExportQuality {
    /// Small files, visible compression.
    Low,
    /// Balanced.
    Medium,
    /// Close to the original (the default).
    #[default]
    High,
}

/// Custom output settings; absent fields take the sequence's value (quality: high).
#[wire]
#[derive(Default)]
pub struct CustomExport {
    /// Frame width in pixels: even, 16-8192.
    #[serde(default)]
    pub width: Option<u32>,
    /// Frame height in pixels: even, 16-8192.
    #[serde(default)]
    pub height: Option<u32>,
    /// Frame rate, e.g. {"num": 30, "den": 1}: at most 240 fps, a whole number of flicks per
    /// frame.
    #[serde(default)]
    pub fps: Option<FrameRate>,
    /// "low", "medium" or "high".
    #[serde(default)]
    pub quality: Option<ExportQuality>,
}

/// What to make: the preset, or custom settings.
#[wire]
#[derive(Default)]
pub enum ExportPreset {
    /// MP4, H.264 + AAC 48 kHz stereo, at the sequence frame rate, fit into 1920x1080
    /// (1080x1920 when portrait) without upscaling, high quality.
    #[default]
    #[serde(rename = "h264-mp4-1080p")]
    H264Mp4Hd,
    /// MP4, H.264 + AAC at a custom size, frame rate and quality.
    #[serde(rename = "custom")]
    Custom(CustomExport),
    /// A preset by id (`export.list_presets`): "youtube-1080p", "tiktok", a preset of your own.
    #[serde(untagged)]
    Id(String),
}

/// Whether to use a hardware video encoder.
#[wire]
#[derive(Default, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ExportHw {
    /// The first hardware H.264 encoder that works here, else software (x264); if it fails
    /// during the export, the export starts again in software.
    #[default]
    Auto,
    /// Software (x264) only.
    Off,
    /// This hardware encoder and no other, e.g. "h264_nvenc" (the `implementation` names of
    /// the machine's hardware encoders); fails instead of falling back.
    Force(String),
}

/// Longest accepted encoder name of `hw: {"force": …}`.
pub const MAX_ENCODER_NAME_CHARS: usize = 64;

impl ExportHw {
    /// Checks the encoder name of `Force` (lowercase letters, digits and `_`).
    fn check(&self) -> Result<(), CmdError> {
        if let ExportHw::Force(name) = self
            && (name.is_empty()
                || name.len() > MAX_ENCODER_NAME_CHARS
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
        {
            return Err(CmdError::invalid(
                "/hw/force",
                format!(
                    "`{name}` is not an encoder name (1..={MAX_ENCODER_NAME_CHARS} lowercase \
                     letters, digits and `_`, e.g. `h264_nvenc`)"
                ),
            ));
        }
        Ok(())
    }
}

/// Result of `export.start`: the job and what it will write.
#[wire]
pub struct ExportStartOutput {
    /// The export job: follow it with `job.wait` or `job.status`, stop it with `job.cancel`.
    pub job_id: JobId,
    /// The output file.
    pub path: String,
    /// The exported sequence.
    pub sequence: SequenceId,
    /// Exported range start (sequence time, flicks).
    #[serde(rename = "in")]
    pub start: Time,
    /// Exported range end, exclusive.
    #[serde(rename = "out")]
    pub end: Time,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frame rate of the file.
    pub frame_rate: FrameRate,
    /// Video frames the file will hold.
    pub frames: u32,
    /// Length of the file.
    pub duration: Duration,
    /// The document revision exported (edits made later are not in the file).
    pub rev: Rev,
    /// Platform-limit and fit warnings of a preset (never errors; the export runs).
    pub warnings: Vec<PresetWarning>,
}

/// Exports a sequence (or a range of it) to an MP4 file (H.264 video, AAC audio) as a
/// background job and returns the job id at once; wait with `job.wait`, stop with
/// `job.cancel` (the partial file is deleted). The export uses the project as it is now; later
/// edits do not change it. Agents and headless callers may only write inside the project
/// folder and folders the user approved. An existing file is replaced only with
/// `overwrite: true`.
#[command(
    name = "export.start",
    summary = "Export a sequence to an MP4 file (H.264 + AAC) as a background job; returns job_id",
    engine,
    undoable = false,
    journaled = false,
    mcp = "core",
    annotations(destructive = true, idempotent = false)
)]
pub struct ExportStart {
    /// Sequence; optional when the project has one.
    pub sequence: Option<SequenceId>,
    /// Absolute path of the output file (`.mp4`).
    pub path: String,
    /// Part of the sequence to export (default: all of it).
    pub range: Option<ExportRange>,
    /// "h264-mp4-1080p" (default), a preset id from export.list_presets ("youtube-1080p",
    /// "tiktok", ...) or {"custom": {width, height, fps, quality}}.
    pub preset: Option<ExportPreset>,
    /// With a preset id: size, frame rate or quality to use instead of the preset's.
    pub overrides: Option<CustomExport>,
    /// Hardware encoder: "auto" (default), "off", or {"force": "h264_nvenc"}.
    pub hw: Option<ExportHw>,
    /// Replace an existing file at `path` (default false).
    pub overwrite: Option<bool>,
}

// ----- the engine seam -------------------------------------------------------------------------

/// A checked, resolved export: everything the engine needs to run it.
#[derive(Clone, Debug)]
pub struct ExportPlan {
    /// The committed document when the export was requested (the export renders this).
    pub snapshot: Arc<DocSnapshot>,
    /// The sequence.
    pub sequence: SequenceId,
    /// The output file (absolute, `.mp4`).
    pub path: PathBuf,
    /// Whether an existing file at `path` may be replaced.
    pub overwrite: bool,
    /// Range start on the sequence timeline (frame-aligned).
    pub start: Time,
    /// Range end on the sequence timeline, exclusive (frame-aligned).
    pub end: Time,
    /// Output frame width (even).
    pub width: u32,
    /// Output frame height (even).
    pub height: u32,
    /// Output frame rate.
    pub frame_rate: FrameRate,
    /// Output frames: frame `i` shows sequence time `start + frame_rate.frame_to_time(i)`.
    pub frames: u32,
    /// Output length: `frame_rate.frame_to_time(frames)`.
    pub duration: Duration,
    /// Encoder quality.
    pub quality: ExportQuality,
    /// Hardware encoder policy.
    pub hw: ExportHw,
    /// The preset's rate control, which takes the place of `quality` when present.
    pub rate: Option<PlanRate>,
    /// Platform-limit and fit warnings of the preset.
    pub warnings: Vec<PresetWarning>,
}

impl ExportPlan {
    /// The sequence time shown by output frame `index`.
    pub fn time_of(&self, index: u32) -> Result<Time, CmdError> {
        let offset = self
            .frame_rate
            .frame_to_time(i64::from(index))
            .map_err(|e| CmdError::internal(e.to_string()))?;
        self.start
            .checked_add(Duration::try_from(offset).map_err(|e| CmdError::internal(e.to_string()))?)
            .map_err(|e| CmdError::internal(e.to_string()))
    }
}

/// Queues export jobs: the engine implements it (rendering, mixing, encoding).
pub trait ExportStarter: Send {
    /// Queues the export of `plan` and returns its job. Fails with `Unsupported` when this
    /// engine cannot export (no media backend, renderer or encoder), with `Invalid` at `/path`
    /// when another export is already writing that file and with `Invalid` at `/hw/force` when
    /// the forced hardware encoder is not one this machine has.
    fn start(&mut self, plan: ExportPlan) -> Result<JobId, CmdError>;
}

/// What `export.start` runs against (the [`EngineCommand::Context`]); the engine builds one
/// per call.
pub struct ExportCtx {
    /// The committed document of the moment.
    pub snapshot: Arc<DocSnapshot>,
    /// The folders the caller may write into: `Some` for agents and headless callers, `None`
    /// for the editor UI (whatever the user picked).
    pub allowed_roots: Option<AllowedRoots>,
    /// Queues the job.
    pub starter: Box<dyn ExportStarter>,
    /// The export presets (built-in and the user's).
    pub presets: Arc<PresetStore>,
}

// ----- validation ------------------------------------------------------------------------------

/// The sequence named by `id`, or the project's only sequence.
fn resolve_sequence(doc: &Project, id: Option<SequenceId>) -> Result<&Sequence, CmdError> {
    if let Some(id) = id {
        return doc
            .sequences
            .get(&id)
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, id));
    }
    let mut all = doc.sequences.values();
    match (all.next(), all.next()) {
        (Some(s), None) => Ok(s),
        (None, _) => Err(CmdError::invalid(
            "/sequence",
            "the project has no sequence to export",
        )),
        (Some(_), Some(_)) => Err(CmdError::invalid(
            "/sequence",
            format!(
                "the project has {} sequences; name the one to export",
                doc.sequences.len()
            ),
        )),
    }
}

/// `n` rounded down to an even number, at least 2.
fn even(n: u32) -> u32 {
    (n & !1).max(2)
}

/// The preset size: the sequence size fit into 1920×1080 (1080×1920 when portrait) keeping
/// its aspect, never upscaled, rounded down to even numbers.
#[must_use]
pub fn preset_size(width: u32, height: u32) -> (u32, u32) {
    let (box_w, box_h) = if width >= height {
        (PRESET_LONG_SIDE, PRESET_SHORT_SIDE)
    } else {
        (PRESET_SHORT_SIDE, PRESET_LONG_SIDE)
    };
    if width <= box_w && height <= box_h {
        return (even(width), even(height));
    }
    let (w, h) = (u64::from(width), u64::from(height));
    // Scale by min(box_w / w, box_h / h), rounding to the nearest pixel.
    let (out_w, out_h) = if u64::from(box_w) * h <= u64::from(box_h) * w {
        (u64::from(box_w), (h * u64::from(box_w) + w / 2) / w)
    } else {
        ((w * u64::from(box_h) + h / 2) / h, u64::from(box_h))
    };
    let clamp = |v: u64| u32::try_from(v).unwrap_or(u32::MAX);
    (even(clamp(out_w)), even(clamp(out_h)))
}

fn check_side(pointer: &str, value: u32) -> Result<u32, CmdError> {
    if !(MIN_EXPORT_SIDE..=MAX_EXPORT_SIDE).contains(&value) || !value.is_multiple_of(2) {
        return Err(CmdError::invalid(
            pointer,
            format!("must be an even number of pixels, {MIN_EXPORT_SIDE}..={MAX_EXPORT_SIDE}"),
        ));
    }
    Ok(value)
}

fn check_fps(pointer: &str, rate: FrameRate) -> Result<FrameRate, CmdError> {
    if !rate.is_exact() || u64::from(rate.num()) > u64::from(MAX_EXPORT_FPS) * u64::from(rate.den())
    {
        return Err(CmdError::invalid(
            pointer,
            format!(
                "must be at most {MAX_EXPORT_FPS} fps with a whole number of flicks per frame \
                 (e.g. 24, 25, 30000/1001, 30, 50, 60)"
            ),
        ));
    }
    Ok(rate)
}

/// Output size, frame rate and quality from the preset.
fn resolve_preset(
    preset: Option<&ExportPreset>,
    seq: &Sequence,
) -> Result<(u32, u32, FrameRate, ExportQuality), CmdError> {
    let format = &seq.format;
    match preset {
        None | Some(ExportPreset::H264Mp4Hd) => {
            let (w, h) = preset_size(format.width, format.height);
            Ok((w, h, format.frame_rate, ExportQuality::High))
        }
        Some(ExportPreset::Id(_)) => Err(CmdError::internal("preset ids are resolved by plan()")),
        Some(ExportPreset::Custom(c)) => {
            let width = match c.width {
                Some(w) => check_side("/preset/custom/width", w)?,
                None => even(format.width).clamp(MIN_EXPORT_SIDE, MAX_EXPORT_SIDE),
            };
            let height = match c.height {
                Some(h) => check_side("/preset/custom/height", h)?,
                None => even(format.height).clamp(MIN_EXPORT_SIDE, MAX_EXPORT_SIDE),
            };
            let fps = check_fps("/preset/custom/fps", c.fps.unwrap_or(format.frame_rate))?;
            Ok((width, height, fps, c.quality.unwrap_or_default()))
        }
    }
}

/// The frame-aligned range `[start, end)`: the whole sequence, or the requested range.
fn resolve_range(range: Option<&ExportRange>, seq: &Sequence) -> Result<(Time, Time), CmdError> {
    let rate = seq.format.frame_rate;
    let seq_end = Time::from(seq.duration());
    let Some(range) = range else {
        if seq_end <= Time::ZERO {
            return Err(CmdError::invalid(
                "/sequence",
                "the sequence is empty: there is nothing to export",
            ));
        }
        return Ok((Time::ZERO, seq_end));
    };
    let start = resolve_frame_time("/range/in", range.start, rate)?;
    let end = resolve_frame_time("/range/out", range.end, rate)?;
    if start < Time::ZERO {
        return Err(CmdError::invalid("/range/in", "must not be negative"));
    }
    if end <= start {
        return Err(CmdError::invalid(
            "/range/out",
            format!("must be after `in` ({start}) by at least one frame"),
        ));
    }
    if end > seq_end {
        return Err(CmdError::invalid(
            "/range/out",
            format!("the sequence ends at {seq_end}"),
        ));
    }
    Ok((start, end))
}

/// The number of output frames covering `length` at `rate` (the last one may run past it).
fn frame_count(length: Duration, rate: FrameRate) -> Result<(u32, Duration), CmdError> {
    let t = Time::from(length);
    let mut n = rate.time_to_frame(t, Rounding::Floor);
    if rate.frame_to_time(n).is_ok_and(|s| s < t) {
        n += 1;
    }
    let frames = u32::try_from(n)
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| CmdError::invalid("/range", "the range is too long to export"))?;
    let duration = rate
        .frame_to_time(i64::from(frames))
        .ok()
        .and_then(|t| Duration::try_from(t).ok())
        .ok_or_else(|| CmdError::invalid("/range", "the range is too long to export"))?;
    Ok((frames, duration))
}

/// The path's components without `.`, compared as the platform does (ASCII case-insensitive on
/// Windows).
fn same_path(a: &Path, b: &Path) -> bool {
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .map(|c| {
                let s = c.as_os_str().to_string_lossy();
                if cfg!(windows) {
                    s.replace('/', "\\").to_ascii_lowercase()
                } else {
                    s.into_owned()
                }
            })
            .collect()
    };
    if parts(a) == parts(b) {
        return true;
    }
    // The same file through different spellings (symlinks, short names) when both exist.
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Checks the output path (see the module docs) and returns it.
fn check_output(
    doc: &Project,
    path: &str,
    overwrite: bool,
    roots: Option<&AllowedRoots>,
) -> Result<PathBuf, CmdError> {
    check_abs_path("/path", path)?;
    let out = PathBuf::from(path);
    let is_mp4 = out
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
    if !is_mp4 || out.file_stem().is_none_or(std::ffi::OsStr::is_empty) {
        return Err(CmdError::invalid(
            "/path",
            format!("`{path}` must name an .mp4 file"),
        ));
    }
    if out.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(CmdError::invalid(
            "/path",
            format!("`{path}` must not contain `..`"),
        ));
    }
    if let Some(roots) = roots
        && !roots.allows(path)
    {
        return Err(CmdError::invalid(
            "/path",
            format!(
                "`{path}` is outside the folders you may write to (the project folder and \
                 the folders the user allowed); {OUTSIDE_ROOTS_HINT}"
            ),
        ));
    }
    for media in doc.media.values() {
        if let Some(source) = &media.path_abs
            && same_path(&out, Path::new(source))
        {
            return Err(CmdError::invalid(
                "/path",
                format!(
                    "`{path}` is the source file of media `{}`; export to another file",
                    media.id
                ),
            ));
        }
    }
    let folder = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| CmdError::invalid("/path", format!("`{path}` has no folder")))?;
    if !folder.is_dir() {
        return Err(CmdError::invalid(
            "/path",
            format!("the folder `{}` does not exist", folder.display()),
        ));
    }
    match std::fs::metadata(&out) {
        Ok(meta) if meta.is_dir() => {
            return Err(CmdError::invalid(
                "/path",
                format!("`{path}` is a folder; name an .mp4 file"),
            ));
        }
        Ok(_) if !overwrite => {
            return Err(CmdError::invalid(
                "/overwrite",
                format!("`{path}` already exists; pass `overwrite: true` to replace it"),
            ));
        }
        _ => {}
    }
    check_writable(folder).map_err(|e| {
        CmdError::invalid(
            "/path",
            format!("cannot write into `{}`: {e}", folder.display()),
        )
    })?;
    Ok(out)
}

/// Creates and removes a small file in `folder`, so a read-only folder fails the call instead
/// of the job.
fn check_writable(folder: &Path) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let probe = folder.join(format!(
        ".cutweave-export-check-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)?;
    drop(file);
    std::fs::remove_file(&probe)
}

impl ExportStart {
    /// Checks the request against `doc` and resolves it into a plan.
    pub fn plan(
        &self,
        snapshot: &Arc<DocSnapshot>,
        roots: Option<&AllowedRoots>,
        presets: &PresetStore,
    ) -> Result<ExportPlan, CmdError> {
        let doc = &snapshot.doc;
        let seq = resolve_sequence(doc, self.sequence)?;
        let (start, end) = resolve_range(self.range.as_ref(), seq)?;
        let length = end
            .duration_since(start)
            .map_err(|e| CmdError::invalid("/range", e.to_string()))?;
        let mut rate = None;
        let mut warnings = Vec::new();
        let mut preset_hw = None;
        let (width, height, frame_rate, quality) = if let Some(ExportPreset::Id(id)) = &self.preset
        {
            let spec = presets.get(id).ok_or_else(|| {
                CmdError::invalid(
                    "/preset",
                    format!("there is no export preset `{id}`; list them with export.list_presets"),
                )
            })?;
            spec.engine_support().map_err(CmdError::unsupported)?;
            let p = presets::plan_preset(&spec, seq, length, self.overrides.as_ref())?;
            rate = p.rate;
            warnings = p.warnings;
            preset_hw = Some(p.hw);
            (p.width, p.height, p.frame_rate, p.quality)
        } else {
            if self.overrides.is_some() {
                return Err(CmdError::invalid(
                    "/overrides",
                    "overrides go with a preset id; the custom preset has its own fields",
                ));
            }
            resolve_preset(self.preset.as_ref(), seq)?
        };
        let (frames, duration) = frame_count(length, frame_rate)?;
        let hw = self.hw.clone().or(preset_hw).unwrap_or_default();
        hw.check()?;
        let overwrite = self.overwrite.unwrap_or(false);
        let path = check_output(doc, &self.path, overwrite, roots)?;
        Ok(ExportPlan {
            snapshot: Arc::clone(snapshot),
            sequence: seq.id,
            path,
            overwrite,
            start,
            end,
            width,
            height,
            frame_rate,
            frames,
            duration,
            quality,
            hw,
            rate,
            warnings,
        })
    }
}

impl EngineCommand for ExportStart {
    type Context = ExportCtx;
    type Output = ExportStartOutput;

    fn run(&self, ctx: &mut ExportCtx) -> Result<ExportStartOutput, CmdError> {
        let plan = self.plan(&ctx.snapshot, ctx.allowed_roots.as_ref(), &ctx.presets)?;
        let path = plan.path.to_string_lossy().into_owned();
        let (sequence, start, end) = (plan.sequence, plan.start, plan.end);
        let (width, height, frame_rate) = (plan.width, plan.height, plan.frame_rate);
        let (frames, duration, rev) = (plan.frames, plan.duration, plan.snapshot.rev);
        let warnings = plan.warnings.clone();
        let job_id = ctx.starter.start(plan)?;
        Ok(ExportStartOutput {
            job_id,
            path,
            sequence,
            start,
            end,
            width,
            height,
            frame_rate,
            frames,
            duration,
            rev,
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preset_fits_1080p_without_upscaling() {
        assert_eq!(preset_size(1920, 1080), (1920, 1080));
        assert_eq!(preset_size(3840, 2160), (1920, 1080));
        assert_eq!(preset_size(1080, 1920), (1080, 1920));
        assert_eq!(preset_size(2160, 3840), (1080, 1920));
        assert_eq!(preset_size(1280, 720), (1280, 720));
        // Odd sizes: 1919x1081 scales by 1080/1081 to 1917.2 px, rounded down to even.
        assert_eq!(preset_size(1919, 1081), (1916, 1080));
        assert_eq!(preset_size(1279, 719), (1278, 718));
        // 21:9 is limited by the width; 4:3 by the height.
        assert_eq!(preset_size(2560, 1080), (1920, 810));
        assert_eq!(preset_size(2880, 2160), (1440, 1080));
        assert_eq!(preset_size(1, 1), (2, 2));
    }

    #[test]
    fn frame_counts_cover_the_range() {
        let sec = Duration::from_secs(1).unwrap();
        assert_eq!(frame_count(sec, FrameRate::FPS_30).unwrap().0, 30);
        assert_eq!(frame_count(sec, FrameRate::FPS_29_97).unwrap().0, 30);
        let (n, d) = frame_count(Duration::from_flicks(1).unwrap(), FrameRate::FPS_25).unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            d,
            Duration::try_from(FrameRate::FPS_25.frame_to_time(1).unwrap()).unwrap()
        );
    }

    #[test]
    fn path_spellings_compare_like_the_platform() {
        assert!(same_path(Path::new("/a/./b.mp4"), Path::new("/a/b.mp4")));
        assert!(!same_path(Path::new("/a/b.mp4"), Path::new("/a/c.mp4")));
        if cfg!(windows) {
            assert!(same_path(
                Path::new("C:\\A\\B.MP4"),
                Path::new("c:/a/b.mp4")
            ));
        }
    }
}
