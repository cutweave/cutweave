// SPDX-License-Identifier: GPL-3.0-or-later
//! The `export` namespace (WP-1E.03; arch §5.1, §7, §8, §14): `export.start`, and the export
//! queue's commands ([`queue`], WP-2G.03a).
//!
//! `export.start` checks the request against the **committed** document of the moment
//! ([`ExportCtx::snapshot`]: later edits never change an export), resolves it into an
//! [`ExportPlan`] (sequence, frame-aligned range, output size, frame rate, frame count,
//! quality) and hands the plan to an [`ExportStarter`], which adds it to the engine's export
//! queue (resuming a paused queue) and returns its job id; `export.enqueue` adds it without
//! resuming. Rendering, mixing and encoding belong to the engine (`cutweave-engine`, module
//! `export`); this crate knows neither the GPU nor the media backend. `export.status` is
//! `job.status` on the returned job.
//!
//! **Output path** rules (checked here, before anything is queued): an absolute path ending in
//! the container's extension (`.mp4`, or `.mov` for ProRes presets), inside the caller's allowed folders ([`ExportCtx::allowed_roots`], the same rule as
//! `media.import`), never the source file of a media asset of the project, in an existing
//! folder the engine can write to, and an existing file only as the collision policy says
//! ([`output`], WP-2G.03b: `fail` by default, `overwrite` replaces it when the export succeeds
//! and a failed or cancelled export leaves it as it was, `rename` picks a free name). The
//! range may be the whole sequence, its in/out points, a custom range or one file per marker
//! ([`range`]); a marker set queues one job per region. Before queueing, the free disk space
//! is checked ([`output::check_disk`]).

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use cutweave_commands::{
    CmdError, DocSnapshot, EngineCommand, Registry, RegistryError, Rev, command, wire,
};
use cutweave_jobs::JobId;
use cutweave_model::{
    Duration, EntityKind, FrameRate, Project, Rounding, Sequence, SequenceId, Time,
};

pub mod advanced;
pub mod encoders;
pub mod output;
pub mod presets;
pub mod queue;
pub mod range;

use crate::media::{AllowedRoots, OUTSIDE_ROOTS_HINT};
use crate::util::check_abs_path;
pub use advanced::AdvancedExportOptions;
pub use output::Collision;
use output::{Policy, Template};
use presets::{AudioCodec, Container, PlanRate, PresetStore, PresetWarning, VideoCodec};
pub use range::{ExportRange, PairMode, RangeKind, Region};

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

/// Registers `export.start`, the preset commands and the queue commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register::<ExportStart>()?;
    registry.register::<encoders::ListEncoders>()?;
    registry.register::<advanced::DescribeOptions>()?;
    presets::register(registry)?;
    queue::register(registry)?;
    Ok(())
}

// ----- params ----------------------------------------------------------------------------------

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
    /// Base preset id; the other fields replace its values.
    #[serde(default)]
    pub preset: Option<String>,
}

/// What to make: the preset, or custom settings.
#[wire]
#[derive(Default)]
pub enum ExportPreset {
    /// MP4, H.264 + AAC, fit into 1920x1080 (1080x1920 portrait), not upscaled, high quality.
    #[default]
    #[serde(rename = "h264-mp4-1080p")]
    H264Mp4Hd,
    /// MP4, H.264 + AAC at a custom size, frame rate and quality.
    #[serde(rename = "custom")]
    Custom(CustomExport),
    /// A preset id (export.list_presets).
    #[serde(untagged)]
    Id(String),
}

/// Whether to use a hardware video encoder.
#[wire]
#[derive(Default, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ExportHw {
    /// The first hardware encoder of the codec that works here (`export.list_encoders`), else
    /// software; if it fails during the export, the export starts again in software.
    #[default]
    Auto,
    /// Software (x264, x265, SVT-AV1) only.
    Off,
    /// This hardware encoder and no other, e.g. "hevc_nvenc" (the `implementation` names of
    /// the machine's hardware encoders, `export.list_encoders`); fails instead of falling back.
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
                     letters, digits and `_`, e.g. `hevc_nvenc`)"
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
    /// Estimated file size in KiB when the rate control fixes a bitrate (`vbr`, `cbr`, a
    /// bitrate preset); absent for a quality (`crf`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_kib: Option<u32>,
    /// Every export of a marker set, in time order (empty for any other range). The fields
    /// above describe the first of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<ExportedFile>,
}

/// One file of a marker-set export.
#[wire]
pub struct ExportedFile {
    /// Its export job.
    pub job_id: JobId,
    /// The output file (after `rename`).
    pub path: String,
    /// The marker's label.
    pub label: String,
    /// Range start (sequence time, flicks).
    #[serde(rename = "in")]
    pub start: Time,
    /// Range end, exclusive.
    #[serde(rename = "out")]
    pub end: Time,
    /// Video frames the file will hold.
    pub frames: u32,
    /// Length of the file.
    pub duration: Duration,
}

/// Exports a sequence (or a range of it) to a file (MP4, H.264 + AAC by default; a preset id
/// picks HEVC, AV1 or ProRes) as a background job and returns the job id at once; wait with
/// `job.wait`, stop with `job.cancel` (the partial file is deleted). Exports queue and run one at a time; this one
/// also resumes a paused queue (`export.queue` lists it). The export uses the project as it is
/// now; later edits do not change it. Agents and headless callers may only write inside the
/// project folder and folders the user approved. `range.kind` picks the part: "sequence",
/// "in_out" (the sequence's in/out points), "custom" (`in`, `out`) or "markers" (one file per
/// marker: `path` is a folder or a name template ending in the container's extension (.mp4;
/// .mov for ProRes) with {seq}, {marker_label}, {n}; the
/// output lists every job). `collision` says what happens to an existing file: "fail"
/// (default), "rename" (pick a free name) or "overwrite" (replace it when the export
/// succeeds). Fails with `Invalid` when the drive has too little free space.
#[command(
    name = "export.start",
    summary = "Export a sequence to a video file (H.264, HEVC, AV1, ProRes) as a background job; returns job_id",
    engine,
    undoable = false,
    journaled = false,
    mcp = "core",
    annotations(destructive = true, idempotent = false)
)]
pub struct ExportStart {
    /// Sequence; optional when the project has one.
    pub sequence: Option<SequenceId>,
    /// Absolute path of the output file (`.mp4`; `.mov` for ProRes presets).
    pub path: String,
    /// Part of the sequence to export (default: all of it): {kind: "sequence" | "in_out" |
    /// "custom" | "markers", in, out, markers: [ids], pair_mode: "duration" | "pairs"}.
    pub range: Option<ExportRange>,
    /// "h264-mp4-1080p" (default), a preset id ("youtube-1080p") or {"custom": {width, height,
    /// fps, quality, preset}}.
    pub preset: Option<ExportPreset>,
    /// Hardware encoder: "auto" (default), "off", or {"force": "hevc_nvenc"}.
    pub hw: Option<ExportHw>,
    /// Replace an existing file at `path` (default false); same as `collision: "overwrite"`.
    pub overwrite: Option<bool>,
    /// An existing file: "fail" (default), "rename" or "overwrite".
    pub collision: Option<Collision>,
    /// Fine control (export.describe_options{codec} gives a codec's options as a JSON Schema):
    /// video{rate_control{mode crf|cbr|vbr|cq, crf, bitrate_kbps, max_bitrate_kbps, buffer_kbit},
    /// gop{keyint|keyint_seconds, keyint_min, bframes, closed_gop, scenecut}, preset, tune,
    /// profile, level, pix_fmt, threads}, color{primaries, transfer, matrix, range, tag_only},
    /// audio{codec, bitrate_kbps, sample_rate, channels}. Impossible combinations fail with
    /// Invalid and a hint.
    pub advanced: Option<AdvancedExportOptions>,
}

// ----- the engine seam -------------------------------------------------------------------------

/// What the file holds: container, codecs and the codec settings a preset carries. The default
/// is the classic export: MP4, 8-bit H.264, AAC.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlanOutput {
    /// The container (MP4 or MOV; the output path ends in its extension).
    pub container: Container,
    /// The video codec.
    pub video_codec: VideoCodec,
    /// The codec profile a preset names (ProRes: `422hq`, `4444`; HEVC: `main10`).
    pub profile: Option<String>,
    /// Bits per sample of the coded video (8 or 10; ProRes is always 10).
    pub bit_depth: u8,
    /// Whether the file keeps an alpha channel (ProRes 4444).
    pub alpha: bool,
    /// The audio codec.
    pub audio_codec: AudioCodec,
    /// The audio bitrate in kilobits per second (AAC); the encoder's default when absent.
    pub audio_bitrate_kbps: Option<u32>,
    /// The advanced settings (WP-2G.05); absent in jobs queued before it: the defaults.
    #[serde(default)]
    pub advanced: PlanAdvanced,
}

/// What the advanced export controls set, as the engine reads it (WP-2G.05). The default is
/// every field unset: the encoder's own behaviour. Stored with queued exports.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlanAdvanced {
    /// Peak video bitrate in kilobits per second.
    pub max_bitrate_kbps: Option<u32>,
    /// Rate-control buffer in kilobits.
    pub buffer_kbit: Option<u32>,
    /// Keyframe interval in frames.
    pub keyint: Option<u32>,
    /// Shortest distance between keyframes in frames.
    pub keyint_min: Option<u32>,
    /// Consecutive B-frames.
    pub bframes: Option<u8>,
    /// Closed (`true`) or open (`false`) GOPs.
    pub closed_gop: Option<bool>,
    /// Scene-cut keyframes on or off.
    pub scenecut: Option<bool>,
    /// The encoder speed preset (`slow`); the engine's own when absent.
    pub preset: Option<String>,
    /// The encoder's tune.
    pub tune: Option<String>,
    /// The codec level (`4.1`).
    pub level: Option<String>,
    /// Encoder threads (0: automatic); the engine's own when absent.
    pub threads: Option<u16>,
    /// The color tags to write (unset fields: BT.709 limited).
    pub color: Option<cutweave_model::ColorTags>,
    /// Tags only: the picture is converted as BT.709 limited whatever the tags say.
    pub tag_only: bool,
    /// The audio sample rate in Hz (48000 when absent).
    pub audio_sample_rate: Option<u32>,
    /// The audio channels, 1 or 2 (stereo when absent).
    pub audio_channels: Option<u16>,
    /// The PCM sample size, 16 or 24 (24 when absent).
    pub pcm_bits: Option<u8>,
}

impl Default for PlanOutput {
    fn default() -> PlanOutput {
        PlanOutput {
            container: Container::Mp4,
            video_codec: VideoCodec::H264,
            profile: None,
            bit_depth: 8,
            alpha: false,
            audio_codec: AudioCodec::Aac,
            audio_bitrate_kbps: None,
            advanced: PlanAdvanced::default(),
        }
    }
}

impl PlanOutput {
    /// The extension of the output file.
    #[must_use]
    pub fn extension(&self) -> &'static str {
        match self.container {
            Container::Mov => "mov",
            _ => "mp4",
        }
    }
}

/// A checked, resolved export: everything the engine needs to run it.
#[derive(Clone, Debug)]
pub struct ExportPlan {
    /// The committed document when the export was requested (the export renders this).
    pub snapshot: Arc<DocSnapshot>,
    /// The sequence.
    pub sequence: SequenceId,
    /// The output file (absolute, `.mp4` or `.mov` as `output` says).
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
    /// Container, codecs and codec settings.
    pub output: PlanOutput,
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

/// Queues export jobs: the engine implements it (its export queue; rendering, mixing,
/// encoding).
pub trait ExportStarter: Send {
    /// `export.start`: adds the export of `plan` to the queue, resumes a paused queue and
    /// returns its job. Fails with `Unsupported` when this engine cannot export (no media
    /// backend, renderer or the plan's encoder), with `Invalid` at `/path` when another queued
    /// or running export writes that file and with `Invalid` at `/hw/force` when the forced
    /// hardware encoder is not one this machine has for the plan's codec.
    fn start(&mut self, plan: ExportPlan) -> Result<JobId, CmdError>;

    /// `export.list_encoders`: the video encoders this engine has, software and hardware (the
    /// hardware ones passed a trial encode). `Unsupported` by default (an engine without a media
    /// backend).
    fn encoders(&mut self) -> Result<Vec<encoders::EncoderEntry>, CmdError> {
        Err(CmdError::unsupported(
            "this engine has no media backend to list encoders from",
        ))
    }

    /// `export.enqueue`: like [`start`](Self::start) without resuming a paused queue. The
    /// default starts (for starters without a queue).
    fn enqueue(&mut self, plan: ExportPlan) -> Result<JobId, CmdError> {
        self.start(plan)
    }

    /// Whether another queued or running export writes `path` (a free name for
    /// `collision: "rename"` must not be one). No queue: never.
    fn output_claimed(&self, _path: &Path) -> bool {
        false
    }

    /// Bytes free on the drive of `folder`, `None` when it cannot be told (the disk check is
    /// then skipped).
    fn free_space(&self, _folder: &Path) -> Option<u64> {
        None
    }

    /// The export queue the queue commands work on; `Unsupported` by default (an engine
    /// without one).
    fn queue(&mut self) -> Result<cutweave_jobs::ExportQueue<ExportPlan>, CmdError> {
        Err(CmdError::unsupported("this engine has no export queue"))
    }
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
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
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

/// Checks the output path (see the module docs) except that the file may exist: the
/// collision policy ([`output::settle`]) decides about that.
fn check_target(
    doc: &Project,
    path: &str,
    roots: Option<&AllowedRoots>,
    ext: &str,
) -> Result<PathBuf, CmdError> {
    check_abs_path("/path", path)?;
    let out = PathBuf::from(path);
    let has_ext = out
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext));
    if !has_ext || out.file_stem().is_none_or(std::ffi::OsStr::is_empty) {
        return Err(CmdError::invalid(
            "/path",
            format!("`{path}` must name an .{ext} file"),
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
    if out.is_dir() {
        return Err(CmdError::invalid(
            "/path",
            format!("`{path}` is a folder; name an .{ext} file"),
        ));
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

/// A starter that only answers questions: what [`ExportStart::plan`] checks against.
struct Inert;

impl ExportStarter for Inert {
    fn start(&mut self, _plan: ExportPlan) -> Result<JobId, CmdError> {
        Err(CmdError::unsupported("planning only"))
    }
}

impl ExportStart {
    /// The params as a borrowed request (one planner for `export.start` and `export.enqueue`).
    #[must_use]
    pub fn request(&self) -> ExportRequest<'_> {
        ExportRequest {
            sequence: self.sequence,
            path: &self.path,
            range: self.range.as_ref(),
            preset: self.preset.as_ref(),
            hw: self.hw.as_ref(),
            overwrite: self.overwrite,
            collision: self.collision,
            advanced: self.advanced.as_ref(),
        }
    }

    /// Checks the request against `snapshot` and resolves it into its single plan (an error
    /// for a marker set: see [`ExportRequest::plans`]).
    pub fn plan(
        &self,
        snapshot: &Arc<DocSnapshot>,
        roots: Option<&AllowedRoots>,
        presets: &PresetStore,
    ) -> Result<ExportPlan, CmdError> {
        let mut plans = self.request().plans(snapshot, roots, presets, &Inert)?.0;
        match (plans.pop(), plans.is_empty()) {
            (Some(plan), true) => Ok(plan),
            _ => Err(CmdError::invalid(
                "/range",
                "a marker set exports several files; use ExportRequest::plans",
            )),
        }
    }
}

/// The params every export command shares, borrowed.
#[derive(Clone, Copy)]
pub struct ExportRequest<'a> {
    /// Sequence.
    pub sequence: Option<SequenceId>,
    /// Output path.
    pub path: &'a str,
    /// Range.
    pub range: Option<&'a ExportRange>,
    /// Preset.
    pub preset: Option<&'a ExportPreset>,
    /// Hardware encoder.
    pub hw: Option<&'a ExportHw>,
    /// Replace an existing file.
    pub overwrite: Option<bool>,
    /// Collision policy.
    pub collision: Option<Collision>,
    /// Advanced settings.
    pub advanced: Option<&'a AdvancedExportOptions>,
}

impl ExportRequest<'_> {
    /// The extension of the file the preset writes (`mov` for ProRes), `mp4` when the request
    /// names no preset this build can resolve (that error is reported where it belongs).
    fn container_extension(&self, presets: &PresetStore) -> &'static str {
        let id = match self.preset {
            Some(ExportPreset::Id(id)) => Some(id),
            Some(ExportPreset::Custom(c)) => c.preset.as_ref(),
            _ => None,
        };
        id.and_then(|id| presets.get(id))
            .and_then(|spec| spec.plan_output().ok())
            .map_or("mp4", |out| out.extension())
    }

    /// Whether the range is a marker set.
    fn is_marker_set(&self) -> bool {
        self.range.is_some_and(|r| {
            r.kind == Some(RangeKind::Markers) || (r.kind.is_none() && r.markers.is_some())
        })
    }

    /// Checks the request and resolves it into one plan per region (see the module docs), with
    /// the marker label of each (empty for other ranges). Nothing is queued.
    pub fn plans(
        &self,
        snapshot: &Arc<DocSnapshot>,
        roots: Option<&AllowedRoots>,
        presets: &PresetStore,
        starter: &dyn ExportStarter,
    ) -> Result<(Vec<ExportPlan>, Vec<String>), CmdError> {
        let doc = &snapshot.doc;
        let seq = resolve_sequence(doc, self.sequence)?;
        let regions = range::resolve_regions(self.range, seq)?;
        let policy = Policy::of(self.collision, self.overwrite)?;
        let template = if self.is_marker_set() {
            check_abs_path("/path", self.path)?;
            let t = Template::parse(self.path, self.container_extension(presets))?;
            if regions.len() > 1 && !t.varies() {
                return Err(CmdError::invalid(
                    "/path",
                    format!(
                        "{} files are exported; the file name needs {{n}} or {{marker_label}} \
                         to tell them apart",
                        regions.len()
                    ),
                ));
            }
            Some(t)
        } else {
            None
        };
        let mut plans: Vec<ExportPlan> = Vec::with_capacity(regions.len());
        let mut taken: Vec<PathBuf> = Vec::with_capacity(regions.len());
        let mut labels = Vec::with_capacity(regions.len());
        for (i, region) in regions.iter().enumerate() {
            let length = region
                .end
                .duration_since(region.start)
                .map_err(|e| CmdError::invalid("/range", e.to_string()))?;
            let mut rate = None;
            let mut warnings = Vec::new();
            let mut preset_hw = None;
            let mut preset_advanced = None;
            let mut output = PlanOutput::default();
            let named = match self.preset {
                Some(ExportPreset::Id(id)) => Some((id, None)),
                Some(ExportPreset::Custom(c)) => c.preset.as_ref().map(|id| (id, Some(c))),
                _ => None,
            };
            let (width, height, frame_rate, quality) = if let Some((id, overrides)) = named {
                let spec = presets.get(id).ok_or_else(|| {
                    CmdError::invalid(
                        "/preset",
                        format!(
                            "there is no export preset `{id}`; list them with \
                             export.list_presets"
                        ),
                    )
                })?;
                output = spec.plan_output().map_err(CmdError::unsupported)?;
                preset_advanced.clone_from(&spec.advanced);
                let p = presets::plan_preset(&spec, seq, length, overrides)?;
                rate = p.rate;
                warnings = p.warnings;
                preset_hw = Some(p.hw);
                (p.width, p.height, p.frame_rate, p.quality)
            } else {
                resolve_preset(self.preset, seq)?
            };
            let (frames, duration) = frame_count(length, frame_rate)?;
            let mut hw = self.hw.cloned().or(preset_hw).unwrap_or_default();
            hw.check()?;
            if let Some(adv) = advanced::merge(preset_advanced.as_ref(), self.advanced) {
                hw = advanced::apply(&adv, &mut output, &mut rate, hw, frame_rate, &mut warnings)?;
            }
            let wanted = match &template {
                Some(t) => {
                    let label = region.label.as_deref().unwrap_or_default();
                    t.file(&seq.name, label, i + 1)?
                }
                None => PathBuf::from(self.path),
            };
            let target = check_target(doc, &wanted.to_string_lossy(), roots, output.extension())?;
            let path = output::settle(target, policy, &taken, starter)?;
            taken.push(path.clone());
            labels.push(region.label.clone().unwrap_or_default());
            plans.push(ExportPlan {
                snapshot: Arc::clone(snapshot),
                sequence: seq.id,
                path,
                overwrite: policy.collision == Collision::Overwrite,
                start: region.start,
                end: region.end,
                width,
                height,
                frame_rate,
                frames,
                duration,
                quality,
                hw,
                rate,
                warnings,
                output,
            });
        }
        output::check_disk(&plans, starter)?;
        Ok((plans, labels))
    }
}

impl ExportStartOutput {
    /// What `plan` will write, with a placeholder job id (the caller fills it in).
    #[must_use]
    pub fn of(plan: &ExportPlan) -> ExportStartOutput {
        ExportStartOutput {
            job_id: JobId::from_u128(0),
            path: plan.path.to_string_lossy().into_owned(),
            sequence: plan.sequence,
            start: plan.start,
            end: plan.end,
            width: plan.width,
            height: plan.height,
            frame_rate: plan.frame_rate,
            frames: plan.frames,
            duration: plan.duration,
            rev: plan.snapshot.rev,
            warnings: plan.warnings.clone(),
            estimated_kib: advanced::estimated_bytes(plan.rate, &plan.output, plan.duration)
                .map(|b| u32::try_from(b.div_ceil(1024)).unwrap_or(u32::MAX)),
            jobs: Vec::new(),
        }
    }
}

/// Plans `request`, queues every plan (`resume`: `export.start`) and describes the result. A
/// marker set that fails halfway leaves nothing queued.
pub fn queue_request(
    request: &ExportRequest<'_>,
    ctx: &mut ExportCtx,
    resume: bool,
) -> Result<ExportStartOutput, CmdError> {
    let (plans, labels) = request.plans(
        &ctx.snapshot,
        ctx.allowed_roots.as_ref(),
        &ctx.presets,
        &*ctx.starter,
    )?;
    let Some(first) = plans.first() else {
        return Err(CmdError::invalid("/range", "there is nothing to export"));
    };
    let mut output = ExportStartOutput::of(first);
    let mut ids: Vec<JobId> = Vec::with_capacity(plans.len());
    for plan in &plans {
        let queued = if resume {
            ctx.starter.start(plan.clone())
        } else {
            ctx.starter.enqueue(plan.clone())
        };
        match queued {
            Ok(id) => ids.push(id),
            Err(e) => {
                if let Ok(queue) = ctx.starter.queue() {
                    for id in ids {
                        let _ = queue.cancel(id);
                        let _ = queue.remove(id);
                    }
                }
                return Err(e);
            }
        }
    }
    output.job_id = ids[0];
    if request.is_marker_set() {
        output.jobs = plans
            .iter()
            .zip(&ids)
            .zip(labels)
            .map(|((plan, id), label)| ExportedFile {
                job_id: *id,
                path: plan.path.to_string_lossy().into_owned(),
                label,
                start: plan.start,
                end: plan.end,
                frames: plan.frames,
                duration: plan.duration,
            })
            .collect();
    }
    Ok(output)
}

impl EngineCommand for ExportStart {
    type Context = ExportCtx;
    type Output = ExportStartOutput;

    fn run(&self, ctx: &mut ExportCtx) -> Result<ExportStartOutput, CmdError> {
        queue_request(&self.request(), ctx, true)
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
