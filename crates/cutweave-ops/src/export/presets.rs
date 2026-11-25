// SPDX-License-Identifier: GPL-3.0-or-later
//! Export presets (WP-2G.01; arch §9, §14): the preset format, the built-in L0 pack, the
//! user's own presets and the commands `export.list_presets`, `export.resolve_preset`,
//! `export.save_preset` and `export.delete_preset`.
//!
//! # The format ([`ExportPresetSpec`], `schema_version` 1)
//!
//! A preset is data: container, video and audio settings, an output size rule, a frame-rate
//! rule, optional limits and, for platform presets, where the numbers came from
//! ([`PlatformInfo`]: spec URL, the date they were checked and the fields nobody could verify,
//! listed in `unverified`). `crates/cutweave-ops/tests/presets.rs` validates every built-in
//! preset and snapshots their ids.
//!
//! # Where presets live
//!
//! - **Built-in**: the L0 pack `plugins/presets-core/presets.json`, compiled in.
//! - **User**: saved with `export.save_preset` into `export-presets.json` in the engine's
//!   presets folder (`--presets-dir`); without a folder they live for the engine's lifetime
//!   only. A user preset never replaces a built-in id.
//!
//! # How export uses a preset
//!
//! `export.start{preset: "<id>", overrides?}` resolves the preset against the sequence
//! ([`plan_preset`]): the output size, the frame rate, the rate control and the hardware
//! policy. Platform limits are **warnings**, never errors ([`PresetWarning`]): the export runs
//! and the answer lists them. Resolving never changes the sequence.
//!
//! - **Size.** `fixed` (platform presets): exactly that size when the sequence has the same
//!   aspect (within 1 %); otherwise the sequence's shape fitted into the preset's frame
//!   without stretching, and an `AspectMismatch` warning. (Drawing bars to fill the frame is a
//!   render feature, not part of this data format.) `fit`: the sequence fitted into the box,
//!   never upscaled, the box turned to match a portrait sequence. `follow_sequence`: the
//!   sequence's size.
//! - **Frame rate.** `follow_sequence` with an optional `max`: the sequence's rate; above the
//!   maximum, half of it when that is within the maximum, else the maximum (`FrameRateCapped`).
//!   `fixed`: that rate.
//! - **Hardware.** `video.hw_preferred: true` runs as `hw: "auto"` (the first working hardware
//!   H.264 encoder, else software); `false` as `hw: "off"`. An explicit `hw` on `export.start`
//!   wins.
//! - **Codecs.** This build encodes H.264 + AAC in MP4 only; any other preset resolves and
//!   lists, but `export.start` fails with `Unsupported` until WP-2G.02 adds the encoders.
//!   Today the encoder takes a CRF or a video bitrate; the audio bitrate and `profile`/`preset`
//!   fields are carried for WP-2G.02 and not applied yet.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use cutweave_commands::{
    Annotations, CmdError, CommandSpec, Described, DocSnapshot, EngineCommand, EntryKind,
    McpExposure, Registrable, Registry, RegistryError, Scope, command, wire,
};
use cutweave_model::{Duration, FLICKS_PER_SECOND, FrameRate, Sequence, SequenceId};
use serde::Deserialize;

use super::{CustomExport, ExportHw, ExportQuality, ExportRange};

/// The preset format version this build reads and writes.
pub const PRESET_SCHEMA_VERSION: u32 = 1;
/// Longest preset id.
pub const MAX_PRESET_ID_CHARS: usize = 64;
/// Longest preset name.
pub const MAX_PRESET_NAME_CHARS: usize = 80;
/// File of the user's presets inside the presets folder.
pub const USER_PRESETS_FILE: &str = "export-presets.json";
/// Most user presets.
pub const MAX_USER_PRESETS: usize = 200;

/// The built-in pack (`plugins/presets-core/presets.json`), compiled in.
const BUILTIN_JSON: &str = include_str!("../../../../plugins/presets-core/presets.json");
/// The pack manifest (`plugins/presets-core/manifest.json`), compiled in.
const MANIFEST_JSON: &str = include_str!("../../../../plugins/presets-core/manifest.json");

// ----- the format ------------------------------------------------------------------------------

/// The file format of the output.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    /// MP4.
    Mp4,
    /// QuickTime MOV.
    Mov,
    /// Animated GIF.
    Gif,
    /// WAV (audio only).
    Wav,
    /// MP3 (audio only).
    Mp3,
    /// M4A (AAC audio only).
    M4a,
    /// FLAC (audio only).
    Flac,
    /// Ogg (Opus audio only).
    Ogg,
}

impl Container {
    fn audio_only(self) -> bool {
        matches!(
            self,
            Container::Wav | Container::Mp3 | Container::M4a | Container::Flac | Container::Ogg
        )
    }
}

/// The video codec.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    /// H.264 / AVC.
    H264,
    /// H.265 / HEVC.
    Hevc,
    /// Apple ProRes.
    Prores,
    /// GIF (palette).
    Gif,
}

/// The audio codec.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    /// AAC-LC.
    Aac,
    /// MP3.
    Mp3,
    /// Uncompressed PCM.
    Pcm,
    /// FLAC.
    Flac,
    /// Opus.
    Opus,
}

/// How the video encoder spends bits.
#[wire]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum RateControlSpec {
    /// Constant quality on the x264 CRF scale (0-51, lower is better).
    Crf {
        /// The CRF value.
        crf: u8,
    },
    /// Constant bitrate.
    Cbr {
        /// Video bitrate in kilobits per second.
        bitrate_kbps: u32,
    },
    /// Variable bitrate around a target.
    Vbr {
        /// Target video bitrate in kilobits per second.
        bitrate_kbps: u32,
    },
    /// Set by the codec profile (ProRes, GIF).
    Profile,
}

/// Video settings.
#[wire]
pub struct VideoSpec {
    /// The codec.
    pub codec: VideoCodec,
    /// How bits are spent.
    pub rate_control: RateControlSpec,
    /// Codec profile (`high`, `main10`, `422hq`).
    #[serde(default)]
    pub profile: Option<String>,
    /// Encoder speed preset (`medium`, `slow`).
    #[serde(default)]
    pub preset: Option<String>,
    /// Pixel format (`yuv420p`).
    #[serde(default)]
    pub pix_fmt: Option<String>,
    /// Use a hardware encoder when one works here (software otherwise); `false` always uses
    /// software, for quality-first presets.
    pub hw_preferred: bool,
}

/// Audio settings.
#[wire]
pub struct AudioSpec {
    /// The codec.
    pub codec: AudioCodec,
    /// Bitrate in kilobits per second (absent for lossless codecs).
    #[serde(default)]
    pub bitrate_kbps: Option<u32>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
}

/// The output size rule.
#[wire]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PresetSize {
    /// Exactly this size for a sequence of the same aspect; a different aspect is fitted into
    /// the frame without stretching, with an `AspectMismatch` warning.
    Fixed {
        /// Frame width in pixels.
        width: u32,
        /// Frame height in pixels.
        height: u32,
    },
    /// The sequence fitted into this box, never upscaled (the box turns for portrait).
    Fit {
        /// Box width in pixels.
        width: u32,
        /// Box height in pixels.
        height: u32,
    },
    /// The sequence's own size.
    FollowSequence,
}

/// The frame-rate rule.
#[wire]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PresetFps {
    /// The sequence's rate, capped at `max` when given.
    FollowSequence {
        /// The platform's highest frame rate.
        #[serde(default)]
        max: Option<FrameRate>,
    },
    /// Always this rate.
    Fixed {
        /// The rate.
        rate: FrameRate,
    },
}

/// Limits of the target; exceeding one is a warning.
#[wire]
#[derive(Default)]
pub struct PresetLimits {
    /// Longest accepted video, in seconds.
    #[serde(default)]
    pub max_duration_secs: Option<u32>,
    /// Largest accepted file, in MiB (1024 x 1024 bytes).
    #[serde(default)]
    pub max_size_mb: Option<u32>,
}

/// Where a platform preset's numbers come from.
#[wire]
pub struct PlatformInfo {
    /// Platform surface id (`youtube`, `instagram-reels`); shared with the safe-area ids.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The platform's published upload specification (`https://`).
    pub spec_url: String,
    /// The date the values were checked against it (`YYYY-MM-DD`).
    pub checked: String,
    /// The fields (`limits.max_duration_secs`, `size`) nobody could verify at `checked`.
    #[serde(default)]
    pub unverified: Vec<String>,
}

/// The loudness target of a preset (applied by the mixer once WP-2G.05 lands).
#[wire]
pub struct LoudnessSpec {
    /// Integrated loudness target in LUFS (for example -14).
    pub target_lufs: i32,
}

/// An export preset: the data the export dialog, the CLI and agents choose from.
#[wire]
pub struct ExportPresetSpec {
    /// The format version (1).
    pub schema_version: u32,
    /// Stable id: lowercase letters, digits and `-`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One line for the picker.
    #[serde(default)]
    pub summary: String,
    /// The file format.
    pub container: Container,
    /// Video settings; absent for audio-only presets.
    #[serde(default)]
    pub video: Option<VideoSpec>,
    /// Audio settings; absent for GIF.
    #[serde(default)]
    pub audio: Option<AudioSpec>,
    /// The output size rule.
    pub size: PresetSize,
    /// The frame-rate rule.
    pub fps: PresetFps,
    /// Loudness target.
    #[serde(default)]
    pub loudness: Option<LoudnessSpec>,
    /// GIF loop count (0 = forever).
    #[serde(default, rename = "loop")]
    pub looping: Option<u32>,
    /// Limits of the target (warnings).
    #[serde(default)]
    pub limits: PresetLimits,
    /// Where the numbers come from (platform presets).
    #[serde(default)]
    pub platform: Option<PlatformInfo>,
    /// The UI-safe zone to show in the preview: an id of WP-2H.06's `safe-areas.json`.
    #[serde(default)]
    pub safe_area: Option<String>,
}

// ----- validation ------------------------------------------------------------------------------

fn bad(pointer: &str, msg: impl Into<String>) -> CmdError {
    CmdError::invalid(pointer, msg)
}

fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_PRESET_ID_CHARS
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// `YYYY-MM-DD` with plausible month and day.
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |r: std::ops::Range<usize>| s.get(r).and_then(|p| p.parse::<u32>().ok());
    matches!((num(0..4), num(5..7), num(8..10)),
        (Some(y), Some(m), Some(d)) if y >= 2000 && (1..=12).contains(&m) && (1..=31).contains(&d))
}

fn check_box(pointer: &str, w: u32, h: u32) -> Result<(), CmdError> {
    let ok = |v: u32| (16..=8192).contains(&v) && v % 2 == 0;
    if !ok(w) || !ok(h) {
        return Err(bad(
            pointer,
            "width and height must be even, 16..=8192 pixels",
        ));
    }
    Ok(())
}

fn check_rate(pointer: &str, r: FrameRate) -> Result<(), CmdError> {
    if !r.is_exact() || u64::from(r.num()) > 240 * u64::from(r.den()) {
        return Err(bad(
            pointer,
            "must be at most 240 fps with a whole number of flicks per frame",
        ));
    }
    Ok(())
}

impl ExportPresetSpec {
    /// Checks the preset: the format version, ids, codec and container pairs, rate control,
    /// sizes, rates and the platform block. Errors point into the preset (`/video/codec`).
    pub fn validate(&self) -> Result<(), CmdError> {
        if self.schema_version != PRESET_SCHEMA_VERSION {
            return Err(bad(
                "/schema_version",
                format!(
                    "this build reads preset format {PRESET_SCHEMA_VERSION}, not {}",
                    self.schema_version
                ),
            ));
        }
        if !is_slug(&self.id) {
            return Err(bad(
                "/id",
                format!(
                    "`{}` is not a preset id (1..={MAX_PRESET_ID_CHARS} lowercase letters, \
                     digits and `-`)",
                    self.id
                ),
            ));
        }
        if self.name.trim().is_empty()
            || self.name.chars().count() > MAX_PRESET_NAME_CHARS
            || self.name.chars().any(char::is_control)
        {
            return Err(bad(
                "/name",
                format!("the name must be 1..={MAX_PRESET_NAME_CHARS} characters"),
            ));
        }
        self.check_codecs()?;
        if let Some(v) = &self.video {
            self.check_rate_control(v)?;
        }
        if let Some(a) = &self.audio {
            if !(8_000..=192_000).contains(&a.sample_rate) {
                return Err(bad("/audio/sample_rate", "must be 8000..=192000 Hz"));
            }
            if !(1..=8).contains(&a.channels) {
                return Err(bad("/audio/channels", "must be 1..=8"));
            }
            if a.bitrate_kbps.is_some_and(|b| !(8..=3_000).contains(&b)) {
                return Err(bad("/audio/bitrate_kbps", "must be 8..=3000"));
            }
        }
        match &self.size {
            PresetSize::Fixed { width, height } | PresetSize::Fit { width, height } => {
                check_box("/size", *width, *height)?;
            }
            PresetSize::FollowSequence => {}
        }
        match &self.fps {
            PresetFps::FollowSequence { max } => {
                if let Some(m) = max {
                    check_rate("/fps/max", *m)?;
                }
            }
            PresetFps::Fixed { rate } => check_rate("/fps/rate", *rate)?,
        }
        if self.looping.is_some() && self.container != Container::Gif {
            return Err(bad("/loop", "only GIF presets loop"));
        }
        if self.limits.max_duration_secs == Some(0) {
            return Err(bad("/limits/max_duration_secs", "must be at least 1"));
        }
        if self.limits.max_size_mb == Some(0) {
            return Err(bad("/limits/max_size_mb", "must be at least 1"));
        }
        if let Some(l) = &self.loudness
            && !(-70..=0).contains(&l.target_lufs)
        {
            return Err(bad("/loudness/target_lufs", "must be -70..=0"));
        }
        if let Some(p) = &self.platform {
            self.check_platform(p)?;
        } else if self.safe_area.is_some() {
            return Err(bad(
                "/safe_area",
                "a safe area belongs to a platform preset",
            ));
        }
        if let Some(s) = &self.safe_area
            && !is_slug(s)
        {
            return Err(bad("/safe_area", format!("`{s}` is not a safe-area id")));
        }
        Ok(())
    }

    fn check_codecs(&self) -> Result<(), CmdError> {
        let c = self.container;
        if c.audio_only() {
            if self.video.is_some() {
                return Err(bad("/video", "an audio-only container takes no video"));
            }
            let Some(a) = &self.audio else {
                return Err(bad("/audio", "an audio-only preset needs audio settings"));
            };
            let want = match c {
                Container::Wav => AudioCodec::Pcm,
                Container::Mp3 => AudioCodec::Mp3,
                Container::M4a => AudioCodec::Aac,
                Container::Flac => AudioCodec::Flac,
                _ => AudioCodec::Opus,
            };
            if a.codec != want {
                return Err(bad(
                    "/audio/codec",
                    format!("this container holds {want:?} audio"),
                ));
            }
            return Ok(());
        }
        let Some(v) = &self.video else {
            return Err(bad("/video", "this container needs video settings"));
        };
        let ok = match c {
            Container::Mp4 => matches!(v.codec, VideoCodec::H264 | VideoCodec::Hevc),
            Container::Mov => matches!(
                v.codec,
                VideoCodec::Prores | VideoCodec::H264 | VideoCodec::Hevc
            ),
            _ => v.codec == VideoCodec::Gif,
        };
        if !ok {
            return Err(bad(
                "/video/codec",
                format!("{:?} does not go into {c:?}", v.codec),
            ));
        }
        if c == Container::Gif {
            if self.audio.is_some() {
                return Err(bad("/audio", "GIF has no sound"));
            }
        } else if let Some(a) = &self.audio {
            let ok = match c {
                Container::Mp4 => matches!(a.codec, AudioCodec::Aac | AudioCodec::Mp3),
                _ => matches!(a.codec, AudioCodec::Aac | AudioCodec::Pcm),
            };
            if !ok {
                return Err(bad(
                    "/audio/codec",
                    format!("{:?} does not go into {c:?}", a.codec),
                ));
            }
        }
        Ok(())
    }

    fn check_rate_control(&self, v: &VideoSpec) -> Result<(), CmdError> {
        let coded = matches!(v.codec, VideoCodec::H264 | VideoCodec::Hevc);
        match v.rate_control {
            RateControlSpec::Crf { crf } if coded && crf <= 51 => Ok(()),
            RateControlSpec::Cbr { bitrate_kbps } | RateControlSpec::Vbr { bitrate_kbps }
                if coded && (100..=1_000_000).contains(&bitrate_kbps) =>
            {
                Ok(())
            }
            RateControlSpec::Profile if !coded => Ok(()),
            _ => Err(bad(
                "/video/rate_control",
                "CRF (0..=51) and bitrates (100..=1000000 kbps) belong to H.264 and HEVC; \
                 `profile` to ProRes and GIF",
            )),
        }
    }

    fn check_platform(&self, p: &PlatformInfo) -> Result<(), CmdError> {
        if !is_slug(&p.id) {
            return Err(bad("/platform/id", "not a platform id"));
        }
        if p.name.trim().is_empty() {
            return Err(bad("/platform/name", "must not be empty"));
        }
        if !p.spec_url.starts_with("https://") || p.spec_url.contains(char::is_whitespace) {
            return Err(bad("/platform/spec_url", "must be an https:// URL"));
        }
        if !is_date(&p.checked) {
            return Err(bad("/platform/checked", "must be a date, YYYY-MM-DD"));
        }
        if p.unverified.iter().any(String::is_empty) {
            return Err(bad("/platform/unverified", "names must not be empty"));
        }
        Ok(())
    }

    /// Whether this build can encode the preset (H.264 + AAC in MP4 only until WP-2G.02);
    /// the reason when it cannot.
    pub fn engine_support(&self) -> Result<(), String> {
        let h264 = matches!(&self.video, Some(v) if v.codec == VideoCodec::H264);
        let aac = matches!(&self.audio, Some(a) if a.codec == AudioCodec::Aac);
        if self.container == Container::Mp4 && h264 && aac {
            Ok(())
        } else {
            Err(format!(
                "this build exports H.264 + AAC in MP4 only; `{}` ({:?}) needs the encoders of \
                 WP-2G.02",
                self.id, self.container
            ))
        }
    }
}

// ----- the built-in pack -----------------------------------------------------------------------

/// The shape of `presets.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetPack {
    schema_version: u32,
    presets: Vec<ExportPresetSpec>,
}

/// The built-in presets (and what went wrong loading them: nothing, unless the pack is broken).
#[derive(Debug)]
pub struct BuiltinPresets {
    /// The presets, in pack order.
    pub presets: Vec<ExportPresetSpec>,
    /// Why the pack could not be used, if so.
    pub problem: Option<String>,
}

/// Parses and validates a pack; the first problem fails it.
pub fn parse_pack(json: &str) -> Result<Vec<ExportPresetSpec>, String> {
    let pack: PresetPack = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if pack.schema_version != PRESET_SCHEMA_VERSION {
        return Err(format!(
            "pack format {} is not readable",
            pack.schema_version
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for p in &pack.presets {
        p.validate()
            .map_err(|e| format!("preset `{}`: {e}", p.id))?;
        if !seen.insert(p.id.clone()) {
            return Err(format!("preset id `{}` appears twice", p.id));
        }
    }
    Ok(pack.presets)
}

/// The built-in presets of `plugins/presets-core`.
pub fn builtin() -> &'static BuiltinPresets {
    static CELL: OnceLock<BuiltinPresets> = OnceLock::new();
    CELL.get_or_init(|| match parse_pack(BUILTIN_JSON) {
        Ok(presets) => BuiltinPresets {
            presets,
            problem: None,
        },
        Err(e) => BuiltinPresets {
            presets: Vec::new(),
            problem: Some(format!("the built-in preset pack is unusable: {e}")),
        },
    })
}

/// The built-in pack's source text (tests and tooling).
#[must_use]
pub const fn builtin_json() -> &'static str {
    BUILTIN_JSON
}

/// The pack manifest's text (tests and tooling).
#[must_use]
pub const fn manifest_json() -> &'static str {
    MANIFEST_JSON
}

/// The safe-area ids the pack's platform presets may name.
pub fn known_safe_areas() -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(MANIFEST_JSON)
        .ok()
        .and_then(|v| {
            v.get("safe_area_ids").and_then(|a| {
                a.as_array().map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect()
                })
            })
        })
        .unwrap_or_default()
}

// ----- the user's presets ----------------------------------------------------------------------

/// The user's presets: kept in memory and, with a folder, in `export-presets.json` there.
#[derive(Debug)]
pub struct PresetStore {
    dir: Option<PathBuf>,
    user: Mutex<BTreeMap<String, ExportPresetSpec>>,
    problems: Vec<String>,
}

impl Default for PresetStore {
    fn default() -> PresetStore {
        PresetStore::open(None)
    }
}

impl PresetStore {
    /// Opens the store; a missing file is an empty store, a damaged one is reported in
    /// [`problems`](Self::problems) and its valid presets are kept.
    #[must_use]
    pub fn open(dir: Option<PathBuf>) -> PresetStore {
        let mut user = BTreeMap::new();
        let mut problems = Vec::new();
        if let Some(dir) = &dir {
            let file = dir.join(USER_PRESETS_FILE);
            match std::fs::read_to_string(&file) {
                Ok(text) => match serde_json::from_str::<Vec<serde_json::Value>>(&text) {
                    Ok(entries) => {
                        for (i, entry) in entries.into_iter().enumerate() {
                            match serde_json::from_value::<ExportPresetSpec>(entry) {
                                Ok(p) => match p.validate() {
                                    Ok(()) if builtin_has(&p.id) => problems
                                        .push(format!("preset {i}: `{}` is a built-in id", p.id)),
                                    Ok(()) => {
                                        user.insert(p.id.clone(), p);
                                    }
                                    Err(e) => problems.push(format!("preset {i}: {e}")),
                                },
                                Err(e) => problems.push(format!("preset {i}: {e}")),
                            }
                        }
                    }
                    Err(e) => problems.push(format!("{}: {e}", file.display())),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => problems.push(format!("{}: {e}", file.display())),
            }
        }
        PresetStore {
            dir,
            user: Mutex::new(user),
            problems,
        }
    }

    /// What was wrong with the saved file at start (empty when fine).
    #[must_use]
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, ExportPresetSpec>> {
        self.user.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The user's presets, by id.
    #[must_use]
    pub fn user_presets(&self) -> Vec<ExportPresetSpec> {
        self.lock().values().cloned().collect()
    }

    /// A preset by id: built-in first, then the user's.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<ExportPresetSpec> {
        builtin()
            .presets
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .or_else(|| self.lock().get(id).cloned())
    }

    /// Saves (adds or replaces) a user preset; `true` when it replaced one.
    pub fn save(&self, preset: ExportPresetSpec) -> Result<bool, CmdError> {
        preset.validate()?;
        if builtin_has(&preset.id) {
            return Err(bad(
                "/preset/id",
                format!(
                    "`{}` is a built-in preset; save your copy under another id",
                    preset.id
                ),
            ));
        }
        if preset.platform.is_some() || preset.safe_area.is_some() {
            return Err(bad(
                "/preset/platform",
                "platform data belongs to built-in presets; remove `platform` and `safe_area`",
            ));
        }
        let mut user = self.lock();
        if !user.contains_key(&preset.id) && user.len() >= MAX_USER_PRESETS {
            return Err(bad(
                "/preset",
                format!("at most {MAX_USER_PRESETS} user presets; delete one first"),
            ));
        }
        let mut next = user.clone();
        let replaced = next.insert(preset.id.clone(), preset).is_some();
        self.persist(&next)?;
        *user = next;
        Ok(replaced)
    }

    /// Deletes a user preset.
    pub fn delete(&self, id: &str) -> Result<(), CmdError> {
        if builtin_has(id) {
            return Err(bad(
                "/id",
                format!("`{id}` is a built-in preset and cannot be deleted"),
            ));
        }
        let mut user = self.lock();
        if !user.contains_key(id) {
            return Err(unknown_preset(id));
        }
        let mut next = user.clone();
        next.remove(id);
        self.persist(&next)?;
        *user = next;
        Ok(())
    }

    /// Writes the file (temp file, then rename) when the store has a folder.
    fn persist(&self, all: &BTreeMap<String, ExportPresetSpec>) -> Result<(), CmdError> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        let list: Vec<&ExportPresetSpec> = all.values().collect();
        let text = serde_json::to_string_pretty(&list)
            .map_err(|e| CmdError::internal(format!("cannot write presets: {e}")))?;
        let io = |e: std::io::Error| {
            CmdError::internal(format!("cannot save presets in {}: {e}", dir.display()))
        };
        std::fs::create_dir_all(dir).map_err(io)?;
        let tmp = dir.join(format!("{USER_PRESETS_FILE}.tmp"));
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::rename(&tmp, dir.join(USER_PRESETS_FILE)).map_err(io)
    }
}

fn builtin_has(id: &str) -> bool {
    builtin().presets.iter().any(|p| p.id == id)
}

fn unknown_preset(id: &str) -> CmdError {
    bad(
        "/preset",
        format!("there is no export preset `{id}`; list them with export.list_presets"),
    )
}

// ----- resolving a preset against a sequence ---------------------------------------------------

/// A platform-limit or fit warning: the export still runs.
#[wire]
#[derive(Copy, Eq, Hash)]
pub enum PresetWarningCode {
    /// The sequence's aspect is not the preset's.
    AspectMismatch,
    /// The exported part is longer than the platform accepts.
    DurationOverLimit,
    /// The estimated file is larger than the platform accepts.
    SizeOverLimit,
    /// The sequence's frame rate is above the platform's maximum, so the file uses a lower one.
    FrameRateCapped,
}

/// One warning about a preset on a sequence.
#[wire]
pub struct PresetWarning {
    /// What kind.
    pub code: PresetWarningCode,
    /// A sentence for the person.
    pub message: String,
}

/// The rate control the encoder gets from a preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanRate {
    /// Constant quality, x264 CRF scale.
    Crf(u8),
    /// Average video bitrate, kilobits per second.
    BitrateKbps(u32),
}

/// A preset resolved against one sequence and range.
#[derive(Clone, Debug)]
pub struct PresetPlan {
    /// Output width.
    pub width: u32,
    /// Output height.
    pub height: u32,
    /// Output frame rate.
    pub frame_rate: FrameRate,
    /// The preset's rate control (`None` when an override named a quality instead).
    pub rate: Option<PlanRate>,
    /// The quality named by an override (or the default).
    pub quality: ExportQuality,
    /// The hardware policy the preset implies.
    pub hw: ExportHw,
    /// Limit and fit warnings.
    pub warnings: Vec<PresetWarning>,
    /// Estimated file size in bytes (bitrate presets only).
    pub estimated_bytes: Option<u64>,
}

fn even(n: u64) -> u32 {
    u32::try_from(n & !1).unwrap_or(u32::MAX).max(2)
}

/// `(sw, sh)` fitted into `(bw, bh)`, rounded down to even numbers; no upscaling unless asked.
fn fit_size(sw: u32, sh: u32, bw: u32, bh: u32, upscale: bool) -> (u32, u32) {
    if !upscale && sw <= bw && sh <= bh {
        return (even(u64::from(sw)), even(u64::from(sh)));
    }
    let (w, h) = (u64::from(sw.max(1)), u64::from(sh.max(1)));
    let (bw, bh) = (u64::from(bw), u64::from(bh));
    if bw * h <= bh * w {
        (even(bw), even((h * bw + w / 2) / w))
    } else {
        (even((w * bh + h / 2) / h), even(bh))
    }
}

fn same_aspect(aw: u32, ah: u32, bw: u32, bh: u32) -> bool {
    let (x, y) = (u64::from(aw) * u64::from(bh), u64::from(ah) * u64::from(bw));
    x.abs_diff(y) * 100 <= x.max(y)
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn ratio(w: u32, h: u32) -> String {
    let g = gcd(w, h).max(1);
    format!("{}:{}", w / g, h / g)
}

fn rate_above(a: FrameRate, b: FrameRate) -> bool {
    u64::from(a.num()) * u64::from(b.den()) > u64::from(b.num()) * u64::from(a.den())
}

fn secs(d: Duration) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let flicks = cutweave_model::Time::from(d).flicks() as f64;
    #[allow(clippy::cast_precision_loss)]
    let per = FLICKS_PER_SECOND as f64;
    flicks / per
}

fn check_side(pointer: &str, v: u32) -> Result<u32, CmdError> {
    if !(16..=8192).contains(&v) || v % 2 != 0 {
        return Err(bad(pointer, "must be an even number of pixels, 16..=8192"));
    }
    Ok(v)
}

/// Resolves `spec` for `seq` and an export of `length`, with the caller's `overrides` (size,
/// frame rate, quality). Never changes the sequence. Limits and aspect mismatches are warnings.
pub fn plan_preset(
    spec: &ExportPresetSpec,
    seq: &Sequence,
    length: Duration,
    overrides: Option<&CustomExport>,
) -> Result<PresetPlan, CmdError> {
    let fmt = &seq.format;
    let mut warnings = Vec::new();
    let (mut width, mut height) =
        match &spec.size {
            PresetSize::FollowSequence => (even(u64::from(fmt.width)), even(u64::from(fmt.height))),
            PresetSize::Fit { width, height } => {
                let (bw, bh) = if fmt.height > fmt.width && width > height {
                    (*height, *width)
                } else {
                    (*width, *height)
                };
                fit_size(fmt.width, fmt.height, bw, bh, false)
            }
            PresetSize::Fixed { width, height } => {
                if same_aspect(fmt.width, fmt.height, *width, *height) {
                    (*width, *height)
                } else {
                    let fitted = fit_size(fmt.width, fmt.height, *width, *height, true);
                    warnings.push(PresetWarning {
                    code: PresetWarningCode::AspectMismatch,
                    message: format!(
                        "The sequence is {} ({}x{}) but {} wants {} ({width}x{height}). The file \
                         keeps the sequence's shape ({}x{}) instead of stretching it.",
                        ratio(fmt.width, fmt.height),
                        fmt.width,
                        fmt.height,
                        spec.platform.as_ref().map_or(spec.name.as_str(), |p| &p.name),
                        ratio(*width, *height),
                        fitted.0,
                        fitted.1,
                    ),
                });
                    fitted
                }
            }
        };
    let mut frame_rate = match &spec.fps {
        PresetFps::Fixed { rate } => *rate,
        PresetFps::FollowSequence { max } => {
            let own = fmt.frame_rate;
            match max {
                Some(m) if rate_above(own, *m) => {
                    let half = FrameRate::new(own.num(), own.den().saturating_mul(2))
                        .ok()
                        .filter(|h| h.is_exact() && !rate_above(*h, *m));
                    let used = half.unwrap_or(*m);
                    warnings.push(PresetWarning {
                        code: PresetWarningCode::FrameRateCapped,
                        message: format!(
                            "The sequence runs at {:.2} fps; {} takes at most {:.2}, so the file \
                             uses {:.2} fps.",
                            own.as_f64(),
                            spec.name,
                            m.as_f64(),
                            used.as_f64()
                        ),
                    });
                    used
                }
                _ => own,
            }
        }
    };
    let mut rate = spec.video.as_ref().and_then(|v| match v.rate_control {
        RateControlSpec::Crf { crf } => Some(PlanRate::Crf(crf)),
        RateControlSpec::Cbr { bitrate_kbps } | RateControlSpec::Vbr { bitrate_kbps } => {
            Some(PlanRate::BitrateKbps(bitrate_kbps))
        }
        RateControlSpec::Profile => None,
    });
    let mut quality = ExportQuality::default();
    if let Some(o) = overrides {
        if let Some(w) = o.width {
            width = check_side("/overrides/width", w)?;
        }
        if let Some(h) = o.height {
            height = check_side("/overrides/height", h)?;
        }
        if let Some(f) = o.fps {
            check_rate("/overrides/fps", f)?;
            frame_rate = f;
        }
        if let Some(q) = o.quality {
            quality = q;
            rate = None;
        }
    }
    let length_secs = secs(length);
    if let Some(max) = spec.limits.max_duration_secs
        && length_secs > f64::from(max)
    {
        warnings.push(PresetWarning {
            code: PresetWarningCode::DurationOverLimit,
            message: format!(
                "The export is {length_secs:.0} s long; {} accepts at most {max} s.",
                spec.platform
                    .as_ref()
                    .map_or(spec.name.as_str(), |p| &p.name)
            ),
        });
    }
    let estimated_bytes = match rate {
        Some(PlanRate::BitrateKbps(v)) => {
            let audio = spec
                .audio
                .as_ref()
                .and_then(|a| a.bitrate_kbps)
                .unwrap_or(0);
            let bytes = f64::from(v + audio) * 125.0 * length_secs;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Some(bytes.ceil() as u64)
        }
        _ => None,
    };
    if let (Some(est), Some(max)) = (estimated_bytes, spec.limits.max_size_mb)
        && est > u64::from(max) * 1024 * 1024
    {
        warnings.push(PresetWarning {
            code: PresetWarningCode::SizeOverLimit,
            message: format!(
                "The file would be about {} MiB; {} accepts at most {max} MiB.",
                est / (1024 * 1024),
                spec.platform
                    .as_ref()
                    .map_or(spec.name.as_str(), |p| &p.name)
            ),
        });
    }
    let hw = if spec.video.as_ref().is_some_and(|v| v.hw_preferred) {
        ExportHw::Auto
    } else {
        ExportHw::Off
    };
    Ok(PresetPlan {
        width,
        height,
        frame_rate,
        rate,
        quality,
        hw,
        warnings,
        estimated_bytes,
    })
}

// ----- the commands ----------------------------------------------------------------------------

/// What the preset commands run against: the preset store and the committed document.
pub struct PresetCtx {
    /// The user's presets.
    pub store: Arc<PresetStore>,
    /// The committed document, when a project is open.
    pub snapshot: Option<Arc<DocSnapshot>>,
}

/// Where a listed preset comes from.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PresetSource {
    /// The `presets-core` pack.
    Builtin,
    /// Saved by the user.
    User,
}

/// A preset in a listing.
#[wire]
pub struct ExportPresetEntry {
    /// Built-in or the user's.
    pub source: PresetSource,
    /// Whether this build can export it now.
    pub supported: bool,
    /// Why not, when unsupported.
    #[serde(default)]
    pub unsupported_reason: Option<String>,
    /// The preset.
    pub preset: ExportPresetSpec,
}

/// Result of `export.list_presets`.
#[wire]
pub struct ListPresetsOutput {
    /// The presets: built-in first (pack order), then the user's by id.
    pub presets: Vec<ExportPresetEntry>,
    /// Problems loading the presets (a damaged user file); empty when fine.
    pub problems: Vec<String>,
}

/// Lists the export presets (built-in and the user's): the platform presets (YouTube,
/// Instagram, TikTok, X) with their sources and `checked` dates, and the general formats. Give
/// `platform` to list one platform's presets. `export.start{preset: "<id>"}` exports with one.
#[wire]
pub struct ListPresets {
    /// Only the presets of this platform id (`youtube`, `instagram-reels`, `tiktok`, `x`).
    #[serde(default)]
    pub platform: Option<String>,
}

const fn reader(name: &'static str, summary: &'static str, rust_type: &'static str) -> CommandSpec {
    CommandSpec {
        name,
        summary,
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
        rust_type,
    }
}

impl Described for ListPresets {
    const SPEC: CommandSpec = reader(
        "export.list_presets",
        "List export presets (YouTube, Instagram, TikTok, X and formats) and what this build can export",
        "ListPresets",
    );
}

impl Registrable for ListPresets {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<ListPresets>().map(drop)
    }
}

fn entry(source: PresetSource, preset: ExportPresetSpec) -> ExportPresetEntry {
    let support = preset.engine_support();
    ExportPresetEntry {
        source,
        supported: support.is_ok(),
        unsupported_reason: support.err(),
        preset,
    }
}

impl EngineCommand for ListPresets {
    type Context = PresetCtx;
    type Output = ListPresetsOutput;

    fn run(&self, ctx: &mut PresetCtx) -> Result<ListPresetsOutput, CmdError> {
        let b = builtin();
        let mut presets: Vec<ExportPresetEntry> = b
            .presets
            .iter()
            .cloned()
            .map(|p| entry(PresetSource::Builtin, p))
            .collect();
        presets.extend(
            ctx.store
                .user_presets()
                .into_iter()
                .map(|p| entry(PresetSource::User, p)),
        );
        if let Some(want) = &self.platform {
            presets.retain(|e| e.preset.platform.as_ref().is_some_and(|p| &p.id == want));
        }
        let mut problems = ctx.store.problems().to_vec();
        problems.extend(b.problem.clone());
        Ok(ListPresetsOutput { presets, problems })
    }
}

/// Shows what a preset does to a sequence without exporting: the output size, frame rate,
/// estimated size and the warnings (`AspectMismatch`, `DurationOverLimit`, `SizeOverLimit`,
/// `FrameRateCapped`). Changes nothing.
#[wire]
pub struct ResolvePreset {
    /// The preset id.
    pub preset: String,
    /// Sequence; optional when the project has one.
    #[serde(default)]
    pub sequence: Option<SequenceId>,
    /// Part of the sequence (default: all of it).
    #[serde(default)]
    pub range: Option<ExportRange>,
    /// Size, frame rate or quality to use instead of the preset's.
    #[serde(default)]
    pub overrides: Option<CustomExport>,
}

/// Result of `export.resolve_preset`.
#[wire]
pub struct ResolvePresetOutput {
    /// The preset id.
    pub preset: String,
    /// The sequence.
    pub sequence: SequenceId,
    /// Output width.
    pub width: u32,
    /// Output height.
    pub height: u32,
    /// Output frame rate.
    pub frame_rate: FrameRate,
    /// Estimated file size in KiB, when the preset has a bitrate.
    pub estimated_kib: Option<u32>,
    /// Whether this build can export it now.
    pub supported: bool,
    /// Why not, when unsupported.
    pub unsupported_reason: Option<String>,
    /// Limit and fit warnings; the export would still run.
    pub warnings: Vec<PresetWarning>,
}

impl Described for ResolvePreset {
    const SPEC: CommandSpec = reader(
        "export.resolve_preset",
        "Show what an export preset does to a sequence (size, rate, warnings) without exporting",
        "ResolvePreset",
    );
}

impl Registrable for ResolvePreset {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<ResolvePreset>().map(drop)
    }
}

impl EngineCommand for ResolvePreset {
    type Context = PresetCtx;
    type Output = ResolvePresetOutput;

    fn run(&self, ctx: &mut PresetCtx) -> Result<ResolvePresetOutput, CmdError> {
        let spec = ctx
            .store
            .get(&self.preset)
            .ok_or_else(|| unknown_preset(&self.preset))?;
        let snapshot = ctx
            .snapshot
            .as_ref()
            .ok_or_else(|| bad("/sequence", "no project is open"))?;
        let seq = super::resolve_sequence(&snapshot.doc, self.sequence)?;
        let (start, end) = super::resolve_range(self.range.as_ref(), seq)?;
        let length = end
            .duration_since(start)
            .map_err(|e| bad("/range", e.to_string()))?;
        let plan = plan_preset(&spec, seq, length, self.overrides.as_ref())?;
        let support = spec.engine_support();
        Ok(ResolvePresetOutput {
            preset: spec.id,
            sequence: seq.id,
            width: plan.width,
            height: plan.height,
            frame_rate: plan.frame_rate,
            estimated_kib: plan
                .estimated_bytes
                .map(|b| u32::try_from(b.div_ceil(1024)).unwrap_or(u32::MAX)),
            supported: support.is_ok(),
            unsupported_reason: support.err(),
            warnings: plan.warnings,
        })
    }
}

/// Saves an export preset of your own (a new id, or replaces your earlier one). Built-in ids
/// cannot be replaced; save a copy under another id.
#[command(
    name = "export.save_preset",
    summary = "Save your own export preset (replaces your preset with the same id)",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct SavePreset {
    /// The preset (see export.list_presets for examples).
    pub preset: ExportPresetSpec,
}

/// Result of `export.save_preset`.
#[wire]
pub struct SavePresetOutput {
    /// The saved preset's id.
    pub id: String,
    /// Whether it replaced an earlier preset of yours.
    pub replaced: bool,
}

impl EngineCommand for SavePreset {
    type Context = PresetCtx;
    type Output = SavePresetOutput;

    fn run(&self, ctx: &mut PresetCtx) -> Result<SavePresetOutput, CmdError> {
        let id = self.preset.id.clone();
        let replaced = ctx.store.save(self.preset.clone())?;
        Ok(SavePresetOutput { id, replaced })
    }
}

/// Deletes one of your export presets. Built-in presets cannot be deleted.
#[command(
    name = "export.delete_preset",
    summary = "Delete one of your export presets",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = true, idempotent = true)
)]
pub struct DeletePreset {
    /// The preset id.
    pub id: String,
}

/// Result of `export.delete_preset`.
#[wire]
pub struct DeletePresetOutput {
    /// The deleted preset's id.
    pub id: String,
}

impl EngineCommand for DeletePreset {
    type Context = PresetCtx;
    type Output = DeletePresetOutput;

    fn run(&self, ctx: &mut PresetCtx) -> Result<DeletePresetOutput, CmdError> {
        ctx.store.delete(&self.id)?;
        Ok(DeletePresetOutput {
            id: self.id.clone(),
        })
    }
}

/// Registers the preset commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<ListPresets>()?
        .register::<ResolvePreset>()?
        .register::<SavePreset>()?
        .register::<DeletePreset>()?;
    Ok(())
}
