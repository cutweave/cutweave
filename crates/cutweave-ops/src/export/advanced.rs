// SPDX-License-Identifier: GPL-3.0-or-later
//! Advanced export controls (WP-2G.05; arch §7, §8): `export.start{advanced}`, the same block in a
//! preset, and the query `export.describe_options`.
//!
//! [`AdvancedExportOptions`] sets what a preset leaves to the encoder: the rate-control mode with
//! its numbers, the keyframe interval and GOP shape, the encoder's preset, tune, profile and
//! level, the pixel format and threads, the color tags, and the audio codec, bitrate, sample rate
//! and channels. Leaving it out changes nothing: a preset exports as it did before.
//!
//! # Checked before anything is queued
//!
//! [`apply`] checks every setting against what the chosen codec and encoder can do (the matrix
//! of [`Caps`], which `export.describe_options` also answers) and refuses an impossible
//! combination with `Invalid{path, msg}` where `msg` ends in what to do instead (ProRes with a
//! CRF, constant bitrate on SVT-AV1, `closed_gop` on a hardware encoder, Opus at 44.1 kHz, PCM in
//! an MP4). No encoder starts for a refused request.
//!
//! - **Hardware.** A hardware encoder takes the rate-control modes `crf`, `vbr` and (except
//!   VideoToolbox) `cbr`, the keyframe interval, B-frames, the peak bitrate and buffer (except
//!   VideoToolbox) and the speed preset. Anything else needs software. With `hw: "auto"` such
//!   a setting makes the export use the software encoder, with a `SoftwareEncoder` warning;
//!   with `hw: {"force": ...}` it is refused.
//! - **Rate control.** `crf` (a quality), `cq` (a quality capped at `max_bitrate_kbps`), `vbr`
//!   (an average, optionally capped) and `cbr` (a constant rate; the peak equals the average).
//!   `two_pass` is `Unsupported` in this build: use `vbr` with `max_bitrate_kbps`.
//! - **Keyframes.** `keyint` is frames; `keyint_seconds` is whole seconds turned into frames at
//!   the output frame rate ([`keyint_frames`]). With `scenecut` on (the encoder's default) a cut
//!   inserts an extra keyframe; turn it off for keyframes exactly on the interval.
//! - **Color.** The tags default to BT.709 limited and are written to the bitstream and the
//!   container. The matrix and range also decide how RGB becomes YUV, unless `tag_only` is set;
//!   primaries and transfer are always only tags (the renderer's picture is not gamut-mapped).
//!   HDR transfers (PQ, HLG) are `Unsupported` in P2.
//! - **Audio.** `codec`, `bitrate_kbps` (lossy codecs), `sample_rate` (44100, 48000, 96000;
//!   the mix is made at that rate), `channels` (a stereo mix is averaged to mono) and `limiter`
//!   (WP-2E.02: the master's true-peak limiter at −1 dBTP, on unless `false`).

use cutweave_commands::__private::schemars::{Schema, SchemaGenerator, json_schema};
use cutweave_commands::{
    Annotations, CmdError, CommandSpec, Described, EngineCommand, EntryKind, McpExposure,
    Registrable, Registry, RegistryError, Scope, wire,
};
use cutweave_model::{
    ColorPrimaries, ColorRange, ColorTags, Duration, FrameRate, MatrixCoefficients,
    TransferCharacteristics,
};
use serde_json::{Value, json};

use super::presets::{
    AudioCodec, Container, PlanRate, PresetWarning, PresetWarningCode, VideoCodec,
};
use super::{ExportCtx, ExportHw, PlanAdvanced, PlanOutput};

/// Longest keyframe interval, in frames.
pub const MAX_KEYINT: u32 = 10_000;
/// Longest keyframe interval in `keyint_seconds`.
pub const MAX_KEYINT_SECONDS: u32 = 600;
/// Largest video bitrate, in kilobits per second.
pub const MAX_BITRATE_KBPS: u32 = 800_000;
/// Smallest video bitrate, in kilobits per second.
pub const MIN_BITRATE_KBPS: u32 = 50;
/// Largest rate-control buffer, in kilobits.
pub const MAX_BUFFER_KBIT: u32 = 4_000_000;
/// Most B-frames.
pub const MAX_BFRAMES: u8 = 16;
/// Most encoder threads.
pub const MAX_THREADS: u16 = 64;

/// The speed presets (x264's names; hardware encoders and SVT-AV1 map them to their own levels).
pub const SPEED_PRESETS: [&str; 9] = [
    "ultrafast",
    "superfast",
    "veryfast",
    "faster",
    "fast",
    "medium",
    "slow",
    "slower",
    "veryslow",
];

// ----- the wire types --------------------------------------------------------------------------

/// How the video encoder spends bits.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RateMode {
    /// Constant quality (`crf`).
    Crf,
    /// Constant bitrate (`bitrate_kbps`).
    Cbr,
    /// Average bitrate, optionally capped (`bitrate_kbps`, `max_bitrate_kbps`).
    Vbr,
    /// Constant quality capped at `max_bitrate_kbps` (`crf`).
    Cq,
}

impl RateMode {
    fn name(self) -> &'static str {
        match self {
            RateMode::Crf => "crf",
            RateMode::Cbr => "cbr",
            RateMode::Vbr => "vbr",
            RateMode::Cq => "cq",
        }
    }
}

/// Rate control: `mode` and the numbers it needs.
#[wire]
pub struct AdvancedRate {
    /// "crf", "cbr", "vbr" or "cq".
    pub mode: RateMode,
    /// Quality 0-51, lower is better (crf, cq).
    #[serde(default)]
    pub crf: Option<u8>,
    /// Target bitrate, kb/s (cbr, vbr).
    #[serde(default)]
    pub bitrate_kbps: Option<u32>,
    /// Peak bitrate, kb/s (vbr, cq).
    #[serde(default)]
    pub max_bitrate_kbps: Option<u32>,
    /// Rate-control buffer, kilobits (cbr, vbr, cq).
    #[serde(default)]
    pub buffer_kbit: Option<u32>,
    /// Two passes (not available yet).
    #[serde(default)]
    pub two_pass: Option<bool>,
}

/// Keyframes and the group of pictures.
#[wire]
#[derive(Default)]
pub struct AdvancedGop {
    /// Keyframe interval in frames.
    #[serde(default)]
    pub keyint: Option<u32>,
    /// Keyframe interval in whole seconds (at the output frame rate); not with `keyint`.
    #[serde(default)]
    pub keyint_seconds: Option<u32>,
    /// Shortest distance between keyframes, frames.
    #[serde(default)]
    pub keyint_min: Option<u32>,
    /// Consecutive B-frames, 0 for none.
    #[serde(default)]
    pub bframes: Option<u8>,
    /// Every keyframe is an IDR frame and nothing refers across it.
    #[serde(default)]
    pub closed_gop: Option<bool>,
    /// Scene-cut keyframes (false: keyframes only on the interval).
    #[serde(default)]
    pub scenecut: Option<bool>,
}

/// Video encoder settings.
#[wire]
#[derive(Default)]
pub struct AdvancedVideo {
    /// Rate control.
    #[serde(default)]
    pub rate_control: Option<AdvancedRate>,
    /// Keyframes and GOP.
    #[serde(default)]
    pub gop: Option<AdvancedGop>,
    /// Encoder speed: ultrafast .. veryslow.
    #[serde(default)]
    pub preset: Option<String>,
    /// Encoder tune (`film`, `grain`, `zerolatency`; AV1: `vq`, `psnr`, `ssim`).
    #[serde(default)]
    pub tune: Option<String>,
    /// Codec profile (H.264 `high`, HEVC `main10`, ProRes `422hq`).
    #[serde(default)]
    pub profile: Option<String>,
    /// Codec level (`4.1`).
    #[serde(default)]
    pub level: Option<String>,
    /// Pixel format (`yuv420p`, `yuv420p10le`, ProRes `yuv422p10le`).
    #[serde(default)]
    pub pix_fmt: Option<String>,
    /// Encoder threads, 0 = automatic.
    #[serde(default)]
    pub threads: Option<u16>,
}

/// Color tags written to the file (default BT.709 limited).
#[wire]
#[derive(Default)]
pub struct AdvancedColor {
    /// Primaries (`bt709`, `bt470bg`, `smpte170m`, `bt2020`).
    #[serde(default)]
    pub primaries: Option<ColorPrimaries>,
    /// Transfer (`bt709`, `smpte170m`, `iec61966-2-1`, `gamma22`); HDR is unsupported.
    #[serde(default)]
    pub transfer: Option<TransferCharacteristics>,
    /// Matrix (`bt709`, `bt470bg`, `smpte170m`, `bt2020nc`).
    #[serde(default)]
    pub matrix: Option<MatrixCoefficients>,
    /// "limited" or "full".
    #[serde(default)]
    pub range: Option<ColorRange>,
    /// Write the tags without converting the picture to them.
    #[serde(default)]
    pub tag_only: Option<bool>,
}

/// The audio codec of an export.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AdvancedAudioCodec {
    /// AAC-LC.
    Aac,
    /// MP3.
    Mp3,
    /// Opus (MP4).
    Opus,
    /// FLAC (MP4).
    Flac,
    /// 16-bit PCM (MOV).
    PcmS16,
    /// 24-bit PCM (MOV).
    PcmS24,
}

/// Audio channels.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AudioChannels {
    /// One channel (the stereo mix averaged).
    Mono,
    /// Two channels.
    Stereo,
}

/// Audio settings.
#[wire]
#[derive(Default)]
pub struct AdvancedAudio {
    /// "aac", "mp3", "opus", "flac", "pcm_s16", "pcm_s24".
    #[serde(default)]
    pub codec: Option<AdvancedAudioCodec>,
    /// Bitrate, kb/s (lossy codecs).
    #[serde(default)]
    pub bitrate_kbps: Option<u32>,
    /// 44100, 48000 or 96000.
    #[serde(default)]
    pub sample_rate: Option<u32>,
    /// "mono" or "stereo".
    #[serde(default)]
    pub channels: Option<AudioChannels>,
    /// The master's true-peak limiter (−1 dBTP): on unless false (WP-2E.02).
    #[serde(default)]
    pub limiter: Option<bool>,
}

/// The tool-list schema of [`AdvancedExportOptions`]: names only. The tool list of an agent is
/// size-limited (arch §14, R36), so the full per-encoder schema is the answer of
/// `export.describe_options` instead of being repeated in `export.start`.
fn compact_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "description": "Fine control of the encode; export.describe_options{codec} gives the JSON Schema of a codec's options. video{rate_control{mode crf|cbr|vbr|cq, crf, bitrate_kbps, max_bitrate_kbps, buffer_kbit}, gop{keyint|keyint_seconds, keyint_min, bframes, closed_gop, scenecut}, preset, tune, profile, level, pix_fmt, threads}, color{primaries, transfer, matrix, range, tag_only}, audio{codec, bitrate_kbps, sample_rate, channels}",
        "properties": {
            "video": {"type": "object"},
            "color": {"type": "object"},
            "audio": {"type": "object"}
        }
    })
}

/// Fine control of the encode; every field is optional and absent ones keep the preset's value.
#[wire]
#[derive(Default)]
#[schemars(schema_with = "compact_schema")]
pub struct AdvancedExportOptions {
    /// Video encoder.
    #[serde(default)]
    pub video: Option<AdvancedVideo>,
    /// Color tags.
    #[serde(default)]
    pub color: Option<AdvancedColor>,
    /// Audio.
    #[serde(default)]
    pub audio: Option<AdvancedAudio>,
}

// ----- merging ---------------------------------------------------------------------------------

/// `preset` with every setting `request` gives on top (rate control and GOP as a block, the
/// rest field by field).
#[must_use]
pub fn merge(
    preset: Option<&AdvancedExportOptions>,
    request: Option<&AdvancedExportOptions>,
) -> Option<AdvancedExportOptions> {
    let (p, r) = match (preset, request) {
        (None, None) => return None,
        (Some(only), None) | (None, Some(only)) => return Some(only.clone()),
        (Some(p), Some(r)) => (p, r),
    };
    let video = match (&p.video, &r.video) {
        (v, None) | (None, v) => v.clone(),
        (Some(p), Some(r)) => Some(AdvancedVideo {
            rate_control: r.rate_control.clone().or_else(|| p.rate_control.clone()),
            gop: r.gop.clone().or_else(|| p.gop.clone()),
            preset: r.preset.clone().or_else(|| p.preset.clone()),
            tune: r.tune.clone().or_else(|| p.tune.clone()),
            profile: r.profile.clone().or_else(|| p.profile.clone()),
            level: r.level.clone().or_else(|| p.level.clone()),
            pix_fmt: r.pix_fmt.clone().or_else(|| p.pix_fmt.clone()),
            threads: r.threads.or(p.threads),
        }),
    };
    let color = match (&p.color, &r.color) {
        (c, None) | (None, c) => c.clone(),
        (Some(p), Some(r)) => Some(AdvancedColor {
            primaries: r.primaries.clone().or_else(|| p.primaries.clone()),
            transfer: r.transfer.clone().or_else(|| p.transfer.clone()),
            matrix: r.matrix.clone().or_else(|| p.matrix.clone()),
            range: r.range.or(p.range),
            tag_only: r.tag_only.or(p.tag_only),
        }),
    };
    let audio = match (&p.audio, &r.audio) {
        (a, None) | (None, a) => a.clone(),
        (Some(p), Some(r)) => Some(AdvancedAudio {
            codec: r.codec.or(p.codec),
            bitrate_kbps: r.bitrate_kbps.or(p.bitrate_kbps),
            sample_rate: r.sample_rate.or(p.sample_rate),
            channels: r.channels.or(p.channels),
            limiter: r.limiter.or(p.limiter),
        }),
    };
    Some(AdvancedExportOptions {
        video,
        color,
        audio,
    })
}

// ----- the capability matrix -------------------------------------------------------------------

/// The family of a hardware encoder, from its name (`hevc_nvenc`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Nvenc,
    Amf,
    Qsv,
    VideoToolbox,
    Vaapi,
}

impl Family {
    fn of(name: &str) -> Option<Family> {
        let (_, suffix) = name.rsplit_once('_')?;
        Some(match suffix {
            "nvenc" => Family::Nvenc,
            "amf" => Family::Amf,
            "qsv" => Family::Qsv,
            "videotoolbox" => Family::VideoToolbox,
            "vaapi" => Family::Vaapi,
            _ => return None,
        })
    }
}

/// What an encoder takes of the advanced settings.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)] // one flag per setting: the capability matrix
pub struct Caps {
    /// The rate-control modes.
    pub rate_modes: Vec<RateMode>,
    /// Whether a peak bitrate and buffer may be given (`max_bitrate_kbps`, `buffer_kbit`).
    pub vbv: bool,
    /// `keyint` and `keyint_seconds`.
    pub keyint: bool,
    /// `keyint_min`.
    pub keyint_min: bool,
    /// `bframes`.
    pub bframes: bool,
    /// `closed_gop`.
    pub closed_gop: bool,
    /// `scenecut`.
    pub scenecut: bool,
    /// `preset`.
    pub preset: bool,
    /// The names `tune` takes (empty: no tune).
    pub tunes: Vec<&'static str>,
    /// The names `profile` takes (empty: none).
    pub profiles: Vec<&'static str>,
    /// Whether `level` is taken.
    pub level: bool,
    /// The pixel formats.
    pub pix_fmts: Vec<&'static str>,
}

const H264_TUNES: [&str; 8] = [
    "film",
    "animation",
    "grain",
    "stillimage",
    "fastdecode",
    "zerolatency",
    "psnr",
    "ssim",
];
const HEVC_TUNES: [&str; 6] = [
    "psnr",
    "ssim",
    "grain",
    "zerolatency",
    "fastdecode",
    "animation",
];
const AV1_TUNES: [&str; 3] = ["vq", "psnr", "ssim"];
const PRORES_PROFILES: [&str; 6] = ["422proxy", "422lt", "422", "422hq", "4444", "4444xq"];

/// What the software encoder of `codec` takes.
#[must_use]
pub fn software_caps(codec: VideoCodec) -> Caps {
    let all_rates = vec![RateMode::Crf, RateMode::Cbr, RateMode::Vbr, RateMode::Cq];
    match codec {
        VideoCodec::H264 => Caps {
            rate_modes: all_rates,
            vbv: true,
            keyint: true,
            keyint_min: true,
            bframes: true,
            closed_gop: true,
            scenecut: true,
            preset: true,
            tunes: H264_TUNES.to_vec(),
            profiles: vec!["baseline", "main", "high"],
            level: true,
            pix_fmts: vec!["yuv420p"],
        },
        VideoCodec::Hevc => Caps {
            rate_modes: all_rates,
            vbv: true,
            keyint: true,
            keyint_min: true,
            bframes: true,
            closed_gop: true,
            scenecut: true,
            preset: true,
            tunes: HEVC_TUNES.to_vec(),
            profiles: vec!["main", "main10"],
            level: true,
            pix_fmts: vec!["yuv420p", "yuv420p10le"],
        },
        VideoCodec::Av1 => Caps {
            // SVT-AV1 here has no constant-bitrate mode, no buffer and no B-frame count.
            rate_modes: vec![RateMode::Crf, RateMode::Vbr, RateMode::Cq],
            vbv: true,
            keyint: true,
            keyint_min: false,
            bframes: false,
            closed_gop: true,
            scenecut: true,
            preset: true,
            tunes: AV1_TUNES.to_vec(),
            profiles: Vec::new(),
            level: false,
            pix_fmts: vec!["yuv420p", "yuv420p10le"],
        },
        VideoCodec::Prores | VideoCodec::Gif => Caps {
            rate_modes: Vec::new(),
            vbv: false,
            keyint: false,
            keyint_min: false,
            bframes: false,
            closed_gop: false,
            scenecut: false,
            preset: false,
            tunes: Vec::new(),
            profiles: if codec == VideoCodec::Prores {
                PRORES_PROFILES.to_vec()
            } else {
                Vec::new()
            },
            level: false,
            pix_fmts: if codec == VideoCodec::Prores {
                vec!["yuv422p10le", "yuv444p10le", "yuva444p10le"]
            } else {
                Vec::new()
            },
        },
    }
}

/// What a hardware encoder of `codec` in `family` takes.
fn hardware_caps(codec: VideoCodec, family: Family) -> Caps {
    let sw = software_caps(codec);
    let mut rate_modes = vec![RateMode::Crf, RateMode::Vbr];
    if !matches!(family, Family::VideoToolbox) {
        rate_modes.push(RateMode::Cbr);
    }
    Caps {
        rate_modes,
        vbv: !matches!(family, Family::VideoToolbox),
        keyint: true,
        keyint_min: false,
        bframes: true,
        closed_gop: false,
        scenecut: false,
        preset: true,
        tunes: Vec::new(),
        profiles: Vec::new(),
        level: false,
        pix_fmts: if matches!(family, Family::Vaapi) {
            vec!["yuv420p"]
        } else {
            sw.pix_fmts
        },
    }
}

/// The audio codecs a container holds.
#[must_use]
pub fn audio_codecs(container: Container) -> Vec<AdvancedAudioCodec> {
    match container {
        Container::Mov => vec![
            AdvancedAudioCodec::Aac,
            AdvancedAudioCodec::Mp3,
            AdvancedAudioCodec::PcmS16,
            AdvancedAudioCodec::PcmS24,
        ],
        _ => vec![
            AdvancedAudioCodec::Aac,
            AdvancedAudioCodec::Mp3,
            AdvancedAudioCodec::Opus,
            AdvancedAudioCodec::Flac,
        ],
    }
}

impl AdvancedAudioCodec {
    fn name(self) -> &'static str {
        match self {
            AdvancedAudioCodec::Aac => "aac",
            AdvancedAudioCodec::Mp3 => "mp3",
            AdvancedAudioCodec::Opus => "opus",
            AdvancedAudioCodec::Flac => "flac",
            AdvancedAudioCodec::PcmS16 => "pcm_s16",
            AdvancedAudioCodec::PcmS24 => "pcm_s24",
        }
    }

    /// The sample rates the codec takes.
    fn rates(self) -> &'static [u32] {
        match self {
            AdvancedAudioCodec::Opus => &[48_000],
            AdvancedAudioCodec::Mp3 => &[44_100, 48_000],
            _ => &[44_100, 48_000, 96_000],
        }
    }

    /// The bitrate range of a lossy codec, kb/s.
    fn bitrates(self) -> Option<(u32, u32)> {
        match self {
            AdvancedAudioCodec::Aac => Some((32, 512)),
            AdvancedAudioCodec::Mp3 => Some((32, 320)),
            AdvancedAudioCodec::Opus => Some((6, 510)),
            _ => None,
        }
    }

    /// The default bitrate of a lossy codec, kb/s.
    #[must_use]
    pub fn default_kbps(self) -> Option<u32> {
        match self {
            AdvancedAudioCodec::Aac | AdvancedAudioCodec::Mp3 => Some(192),
            AdvancedAudioCodec::Opus => Some(128),
            _ => None,
        }
    }
}

// ----- checking and applying -------------------------------------------------------------------

fn bad(pointer: &str, msg: impl Into<String>) -> CmdError {
    CmdError::invalid(pointer, msg)
}

/// A name in `allowed`, or an `Invalid` that lists them.
fn one_of(
    pointer: &str,
    what: &str,
    value: &str,
    allowed: &[&str],
    hint: &str,
) -> Result<(), CmdError> {
    if allowed.contains(&value) {
        return Ok(());
    }
    if allowed.is_empty() {
        return Err(bad(pointer, format!("{what} `{value}`: {hint}")));
    }
    Err(bad(
        pointer,
        format!(
            "{what} `{value}` is not one of {}; {hint}",
            allowed.join(", ")
        ),
    ))
}

/// The keyframe interval in frames for `seconds` at `rate` (rounded to the nearest frame, at
/// least 1).
#[must_use]
pub fn keyint_frames(seconds: u32, rate: FrameRate) -> u32 {
    let (num, den) = (u64::from(rate.num()), u64::from(rate.den().max(1)));
    let frames = (u64::from(seconds) * num + den / 2) / den;
    u32::try_from(frames.max(1)).unwrap_or(u32::MAX)
}

/// The colors of `c` as the tags to write, after checking them.
fn color_tags(c: &AdvancedColor) -> Result<ColorTags, CmdError> {
    let hdr = |what: &str, name: &str| {
        CmdError::unsupported(format!(
            "/advanced/color/{what} `{name}`: HDR output is not exported in this version; use an \
             SDR transfer (bt709, srgb, gamma22)"
        ))
    };
    if let Some(t) = &c.transfer {
        match t {
            TransferCharacteristics::Pq | TransferCharacteristics::Hlg => {
                return Err(hdr("transfer", t.as_str()));
            }
            TransferCharacteristics::Other(_) | TransferCharacteristics::Linear => {
                return Err(bad(
                    "/advanced/color/transfer",
                    format!(
                        "`{t}` is not a transfer to tag: bt709, smpte170m, smpte240m, \
                         iec61966-2-1, gamma22, gamma28, bt2020-10 or bt2020-12"
                    ),
                ));
            }
            _ => {}
        }
    }
    if let Some(p) = &c.primaries
        && matches!(p, ColorPrimaries::Other(_))
    {
        return Err(bad(
            "/advanced/color/primaries",
            format!(
                "`{p}` is not a primaries tag: bt709, bt470m, bt470bg, smpte170m, smpte240m, film, \
                 bt2020, smpte428, smpte431, smpte432 or ebu3213"
            ),
        ));
    }
    if let Some(m) = &c.matrix {
        let ok = matches!(
            m,
            MatrixCoefficients::Bt709
                | MatrixCoefficients::Fcc
                | MatrixCoefficients::Bt470Bg
                | MatrixCoefficients::Smpte170M
                | MatrixCoefficients::Smpte240M
                | MatrixCoefficients::Bt2020Ncl
        );
        if !ok {
            return Err(bad(
                "/advanced/color/matrix",
                format!(
                    "`{m}` is not a YUV matrix to write: bt709, fcc, bt470bg, smpte170m, \
                     smpte240m or bt2020nc"
                ),
            ));
        }
    }
    Ok(ColorTags {
        primaries: c.primaries.clone(),
        transfer: c.transfer.clone(),
        matrix: c.matrix.clone(),
        range: c.range,
    })
}

/// The settings that only a software encoder takes, collected while checking.
#[derive(Default)]
struct SoftwareOnly(Vec<(&'static str, &'static str)>);

impl SoftwareOnly {
    fn add(&mut self, pointer: &'static str, label: &'static str) {
        self.0.push((pointer, label));
    }
}

/// How the request chose its encoder.
enum Target {
    /// Software (`hw: "off"`, or software chosen because a setting needs it).
    Software,
    /// Whichever works: hardware first, software after.
    Auto,
    /// This hardware encoder.
    Hardware(String, Option<Family>),
}

/// Checks `adv` against the codec and container of `output` and the hardware policy `hw`, writes
/// what it sets into `output` and `rate`, and returns the hardware policy to run with (`auto`
/// becomes `off` with a `SoftwareEncoder` warning when a setting needs the software encoder).
/// `frame_rate` turns `keyint_seconds` into frames.
///
/// # Errors
///
/// `Invalid{path, msg}` for a value out of range or a combination the codec or encoder cannot
/// do (the message says what to use instead); `Unsupported` for two-pass and HDR.
#[allow(clippy::too_many_lines)] // one flat run of checks, before anything is queued
pub fn apply(
    adv: &AdvancedExportOptions,
    output: &mut PlanOutput,
    rate: &mut Option<PlanRate>,
    hw: ExportHw,
    frame_rate: FrameRate,
    warnings: &mut Vec<PresetWarning>,
) -> Result<ExportHw, CmdError> {
    let codec = output.video_codec;
    let target = match &hw {
        ExportHw::Off => Target::Software,
        ExportHw::Auto => Target::Auto,
        ExportHw::Force(name) => Target::Hardware(name.clone(), Family::of(name)),
    };
    let caps = match &target {
        Target::Hardware(_, Some(family)) => hardware_caps(codec, *family),
        _ => software_caps(codec),
    };
    let sw = software_caps(codec);
    let mut soft = SoftwareOnly::default();
    let mut next = output.advanced.clone();
    if let Some(v) = &adv.video {
        if matches!(codec, VideoCodec::Prores) {
            prores_checks(v)?;
        }
        if let Some(rc) = &v.rate_control {
            let (planned, max, buffer) = rate_control(rc, &sw, &caps, &target, &mut soft)?;
            *rate = Some(planned);
            next.max_bitrate_kbps = max;
            next.buffer_kbit = buffer;
        }
        if let Some(g) = &v.gop {
            gop(g, &sw, &caps, frame_rate, &mut next, &mut soft)?;
        }
        if let Some(p) = &v.preset {
            one_of(
                "/advanced/video/preset",
                "preset",
                p,
                if sw.preset { &SPEED_PRESETS } else { &[] },
                "pick a speed from ultrafast to veryslow",
            )?;
            next.preset = Some(p.clone());
        }
        if let Some(t) = &v.tune {
            one_of(
                "/advanced/video/tune",
                "tune",
                t,
                &sw.tunes,
                if sw.tunes.is_empty() {
                    "this codec has no tune"
                } else {
                    "pick one of them"
                },
            )?;
            soft.add("/advanced/video/tune", "tune");
            next.tune = Some(t.clone());
        }
        if let Some(l) = &v.level {
            let ok = sw.level
                && l.len() <= 3
                && l.bytes().next().is_some_and(|b| (b'1'..=b'6').contains(&b))
                && (l.len() == 1
                    || (l.as_bytes().get(1) == Some(&b'.')
                        && l.as_bytes()
                            .get(2)
                            .is_some_and(|b| (b'0'..=b'2').contains(b))));
            if !ok {
                return Err(bad(
                    "/advanced/video/level",
                    if sw.level {
                        format!("level `{l}` is not a level like 4.1; use 1 to 6.2 or leave it out")
                    } else {
                        "this codec has no level to set; leave it out".to_owned()
                    },
                ));
            }
            soft.add("/advanced/video/level", "level");
            next.level = Some(l.clone());
        }
        if let Some(t) = v.threads {
            if t > MAX_THREADS {
                return Err(bad(
                    "/advanced/video/threads",
                    format!("at most {MAX_THREADS} threads; 0 lets the encoder choose"),
                ));
            }
            next.threads = Some(t);
        }
        profile_and_format(v, &sw, codec, output, &mut soft)?;
    }
    if let Some(c) = &adv.color {
        let tags = color_tags(c)?;
        next.color = (tags != ColorTags::default()).then_some(tags);
        next.tag_only = c.tag_only.unwrap_or(false);
    }
    if let Some(a) = &adv.audio {
        audio(a, output, &mut next)?;
    }
    // Software-only settings against the hardware policy.
    let mut effective = hw;
    if let Some((pointer, label)) = soft.0.first() {
        match &target {
            Target::Software => {}
            Target::Auto => {
                effective = ExportHw::Off;
                warnings.push(PresetWarning {
                    code: PresetWarningCode::SoftwareEncoder,
                    message: format!(
                        "`{label}` needs a software encoder, so this export does not use the \
                         hardware encoder."
                    ),
                });
            }
            Target::Hardware(name, _) => {
                return Err(bad(
                    pointer,
                    format!(
                        "`{label}` is not available on the hardware encoder `{name}`; use hw: \
                         \"off\" (software) or leave it out"
                    ),
                ));
            }
        }
    }
    output.advanced = next;
    Ok(effective)
}

/// ProRes is intra-only and set by its profile: everything about bits, frames and speed is moot.
fn prores_checks(v: &AdvancedVideo) -> Result<(), CmdError> {
    if v.rate_control.is_some() {
        return Err(bad(
            "/advanced/video/rate_control",
            "ProRes has no rate control: its quality comes from the profile; set \
             video.profile (422proxy, 422lt, 422, 422hq, 4444, 4444xq) instead",
        ));
    }
    if v.gop.is_some() {
        return Err(bad(
            "/advanced/video/gop",
            "ProRes is intra-only (every frame is a keyframe); leave the GOP out",
        ));
    }
    for (pointer, given, what) in [
        (
            "/advanced/video/preset",
            v.preset.is_some(),
            "an encoder preset",
        ),
        ("/advanced/video/tune", v.tune.is_some(), "a tune"),
        ("/advanced/video/level", v.level.is_some(), "a level"),
    ] {
        if given {
            return Err(bad(pointer, format!("ProRes has no {what}; leave it out")));
        }
    }
    Ok(())
}

/// The rate control: the plan's rate and the peak and buffer to carry.
#[allow(clippy::too_many_lines)] // one flat run of checks, before anything is queued
fn rate_control(
    rc: &AdvancedRate,
    sw: &Caps,
    caps: &Caps,
    target: &Target,
    soft: &mut SoftwareOnly,
) -> Result<(PlanRate, Option<u32>, Option<u32>), CmdError> {
    const P: &str = "/advanced/video/rate_control";
    if sw.rate_modes.is_empty() {
        return Err(bad(P, "this codec has no rate control; leave it out"));
    }
    if rc.two_pass == Some(true) {
        return Err(CmdError::unsupported(
            "/advanced/video/rate_control/two_pass: two-pass encoding is not available in this \
             version; use mode vbr with max_bitrate_kbps for a predictable size",
        ));
    }
    if !sw.rate_modes.contains(&rc.mode) {
        return Err(bad(
            &format!("{P}/mode"),
            format!(
                "this encoder has no `{}` mode: use `vbr` (an average with a cap) or `crf`",
                rc.mode.name()
            ),
        ));
    }
    if matches!(target, Target::Hardware(..)) && !caps.rate_modes.contains(&rc.mode) {
        return Err(bad(
            &format!("{P}/mode"),
            format!(
                "this hardware encoder has no `{}` mode: use `vbr` or `crf`, or use hw: \"off\" \
                 (software)",
                rc.mode.name()
            ),
        ));
    }
    if matches!(target, Target::Auto) && rc.mode == RateMode::Cq {
        soft.add("/advanced/video/rate_control/mode", "mode cq");
    }
    if rc.crf.is_some() != matches!(rc.mode, RateMode::Crf | RateMode::Cq) {
        let need = matches!(rc.mode, RateMode::Crf | RateMode::Cq);
        return Err(bad(
            &format!("{P}/crf"),
            if need {
                format!("mode `{}` needs a crf from 0 to 51", rc.mode.name())
            } else {
                format!(
                    "mode `{}` sets a bitrate, not a quality; leave crf out, or use mode `cq` \
                     for a quality with a cap",
                    rc.mode.name()
                )
            },
        ));
    }
    if let Some(crf) = rc.crf
        && crf > 51
    {
        return Err(bad(
            &format!("{P}/crf"),
            "must be 0 to 51 (lower is better)",
        ));
    }
    let kbps = |pointer: &str, v: Option<u32>| -> Result<Option<u32>, CmdError> {
        if let Some(v) = v
            && !(MIN_BITRATE_KBPS..=MAX_BITRATE_KBPS).contains(&v)
        {
            return Err(bad(
                &format!("{P}/{pointer}"),
                format!("must be {MIN_BITRATE_KBPS} to {MAX_BITRATE_KBPS} kb/s"),
            ));
        }
        Ok(v)
    };
    let bitrate = kbps("bitrate_kbps", rc.bitrate_kbps)?;
    let max = kbps("max_bitrate_kbps", rc.max_bitrate_kbps)?;
    let sets_bitrate = matches!(rc.mode, RateMode::Cbr | RateMode::Vbr);
    if sets_bitrate != bitrate.is_some() {
        return Err(bad(
            &format!("{P}/bitrate_kbps"),
            if sets_bitrate {
                format!("mode `{}` needs bitrate_kbps", rc.mode.name())
            } else {
                format!(
                    "mode `{}` sets a quality; leave bitrate_kbps out",
                    rc.mode.name()
                )
            },
        ));
    }
    match rc.mode {
        RateMode::Cq if max.is_none() => {
            return Err(bad(
                &format!("{P}/max_bitrate_kbps"),
                "mode `cq` needs max_bitrate_kbps, the cap on the quality's bitrate",
            ));
        }
        RateMode::Crf | RateMode::Cbr if max.is_some() => {
            return Err(bad(
                &format!("{P}/max_bitrate_kbps"),
                if rc.mode == RateMode::Cbr {
                    "constant bitrate already has its peak at the bitrate; use mode `vbr` for a \
                     cap above the average"
                } else {
                    "mode `crf` has no cap; use mode `cq` for a quality with a cap"
                },
            ));
        }
        _ => {}
    }
    if let (Some(b), Some(m)) = (bitrate, max)
        && m < b
    {
        return Err(bad(
            &format!("{P}/max_bitrate_kbps"),
            "the peak must not be below the average bitrate",
        ));
    }
    if (max.is_some() || rc.buffer_kbit.is_some()) && !caps.vbv {
        return Err(bad(
            &format!(
                "{P}/{}",
                if max.is_some() {
                    "max_bitrate_kbps"
                } else {
                    "buffer_kbit"
                }
            ),
            "this encoder has no peak or buffer control; leave it out, or use hw: \"off\" \
             (software)",
        ));
    }
    if let Some(buffer) = rc.buffer_kbit {
        if !(10..=MAX_BUFFER_KBIT).contains(&buffer) {
            return Err(bad(
                &format!("{P}/buffer_kbit"),
                format!("must be 10 to {MAX_BUFFER_KBIT} kilobits"),
            ));
        }
        if rc.mode == RateMode::Crf {
            return Err(bad(
                &format!("{P}/buffer_kbit"),
                "mode `crf` has no buffer; use mode `cq` with max_bitrate_kbps",
            ));
        }
    }
    let planned = match rc.mode {
        RateMode::Crf | RateMode::Cq => PlanRate::Crf(rc.crf.unwrap_or(0)),
        RateMode::Cbr => PlanRate::Cbr(bitrate.unwrap_or(0)),
        RateMode::Vbr => PlanRate::BitrateKbps(bitrate.unwrap_or(0)),
    };
    Ok((planned, max, rc.buffer_kbit))
}

/// The GOP settings.
fn gop(
    g: &AdvancedGop,
    sw: &Caps,
    caps: &Caps,
    frame_rate: FrameRate,
    next: &mut PlanAdvanced,
    soft: &mut SoftwareOnly,
) -> Result<(), CmdError> {
    const P: &str = "/advanced/video/gop";
    if g.keyint.is_some() && g.keyint_seconds.is_some() {
        return Err(bad(
            &format!("{P}/keyint_seconds"),
            "give keyint (frames) or keyint_seconds, not both",
        ));
    }
    if (g.keyint.is_some() || g.keyint_seconds.is_some()) && !caps.keyint {
        return Err(bad(
            &format!("{P}/keyint"),
            "this encoder takes no keyframe interval",
        ));
    }
    let keyint = match (g.keyint, g.keyint_seconds) {
        (Some(k), _) => {
            if !(1..=MAX_KEYINT).contains(&k) {
                return Err(bad(
                    &format!("{P}/keyint"),
                    format!("must be 1 to {MAX_KEYINT} frames"),
                ));
            }
            Some(k)
        }
        (None, Some(s)) => {
            if !(1..=MAX_KEYINT_SECONDS).contains(&s) {
                return Err(bad(
                    &format!("{P}/keyint_seconds"),
                    format!(
                        "must be 1 to {MAX_KEYINT_SECONDS} seconds; use keyint for fractions of a second"
                    ),
                ));
            }
            Some(keyint_frames(s, frame_rate).min(MAX_KEYINT))
        }
        (None, None) => None,
    };
    next.keyint = keyint;
    if let Some(min) = g.keyint_min {
        if !sw.keyint_min {
            return Err(bad(
                &format!("{P}/keyint_min"),
                "this codec takes no minimum keyframe distance; leave it out",
            ));
        }
        if min == 0 || keyint.is_some_and(|k| min > k) || min > MAX_KEYINT {
            return Err(bad(
                &format!("{P}/keyint_min"),
                "must be at least 1 and not above the keyframe interval",
            ));
        }
        soft.add("/advanced/video/gop/keyint_min", "keyint_min");
        next.keyint_min = Some(min);
    }
    if let Some(b) = g.bframes {
        if !sw.bframes {
            return Err(bad(
                &format!("{P}/bframes"),
                "this codec has no B-frame count to set; leave it out",
            ));
        }
        if b > MAX_BFRAMES {
            return Err(bad(
                &format!("{P}/bframes"),
                format!("at most {MAX_BFRAMES}"),
            ));
        }
        next.bframes = Some(b);
    }
    if let Some(c) = g.closed_gop {
        if !sw.closed_gop {
            return Err(bad(
                &format!("{P}/closed_gop"),
                "this codec has no GOP setting; leave it out",
            ));
        }
        soft.add("/advanced/video/gop/closed_gop", "closed_gop");
        next.closed_gop = Some(c);
    }
    if let Some(s) = g.scenecut {
        if !sw.scenecut {
            return Err(bad(
                &format!("{P}/scenecut"),
                "this codec has no scene-cut setting; leave it out",
            ));
        }
        soft.add("/advanced/video/gop/scenecut", "scenecut");
        next.scenecut = Some(s);
    }
    Ok(())
}

/// The profile and the pixel format, which must agree with each other and the codec.
fn profile_and_format(
    v: &AdvancedVideo,
    sw: &Caps,
    codec: VideoCodec,
    output: &mut PlanOutput,
    soft: &mut SoftwareOnly,
) -> Result<(), CmdError> {
    if let Some(p) = &v.profile {
        one_of(
            "/advanced/video/profile",
            "profile",
            p,
            &sw.profiles,
            "this codec has no profile to set; leave it out",
        )?;
        if codec == VideoCodec::H264 {
            soft.add("/advanced/video/profile", "profile");
        }
        output.profile = Some(p.clone());
        if codec == VideoCodec::Hevc && v.pix_fmt.is_none() {
            output.bit_depth = if p == "main10" { 10 } else { 8 };
        }
    }
    if let Some(f) = &v.pix_fmt {
        one_of(
            "/advanced/video/pix_fmt",
            "pixel format",
            f,
            &sw.pix_fmts,
            "this codec has no pixel format to set",
        )?;
        if codec == VideoCodec::Prores {
            let wants_444 = output
                .profile
                .as_deref()
                .is_some_and(|p| p.starts_with("4444"));
            if wants_444 != f.contains("444") {
                return Err(bad(
                    "/advanced/video/pix_fmt",
                    if wants_444 {
                        "the ProRes 4444 profiles are 4:4:4: use yuv444p10le (or yuva444p10le for alpha)"
                    } else {
                        "the 422 profiles are 4:2:2: use yuv422p10le, or a 4444 profile for 4:4:4"
                    },
                ));
            }
            output.alpha = f.starts_with("yuva");
        } else {
            output.bit_depth = if f.contains("10") { 10 } else { 8 };
        }
    }
    if codec == VideoCodec::Hevc
        && let Some(p) = &output.profile
        && (p == "main10") != (output.bit_depth == 10)
    {
        return Err(bad(
            "/advanced/video/profile",
            "HEVC `main10` is 10-bit and `main` is 8-bit: change the profile or pix_fmt to match",
        ));
    }
    Ok(())
}

/// The audio settings.
#[allow(clippy::too_many_lines)] // one flat run of checks, before anything is queued
fn audio(
    a: &AdvancedAudio,
    output: &mut PlanOutput,
    next: &mut PlanAdvanced,
) -> Result<(), CmdError> {
    const P: &str = "/advanced/audio";
    let container = output.container;
    let current = match (output.audio_codec, next.pcm_bits) {
        (AudioCodec::Aac, _) => AdvancedAudioCodec::Aac,
        (AudioCodec::Mp3, _) => AdvancedAudioCodec::Mp3,
        (AudioCodec::Opus, _) => AdvancedAudioCodec::Opus,
        (AudioCodec::Flac, _) => AdvancedAudioCodec::Flac,
        (AudioCodec::Pcm, Some(16)) => AdvancedAudioCodec::PcmS16,
        (AudioCodec::Pcm, _) => AdvancedAudioCodec::PcmS24,
    };
    let codec = a.codec.unwrap_or(current);
    let allowed = audio_codecs(container);
    if !allowed.contains(&codec) {
        let names: Vec<&str> = allowed.iter().map(|c| c.name()).collect();
        let into = match container {
            Container::Mov => "MOV",
            _ => "MP4",
        };
        return Err(bad(
            &format!("{P}/codec"),
            format!(
                "{} does not go into {into}: use {}{}",
                codec.name(),
                names.join(", "),
                if matches!(
                    codec,
                    AdvancedAudioCodec::PcmS16 | AdvancedAudioCodec::PcmS24
                ) {
                    ", or a MOV preset for PCM"
                } else {
                    ""
                }
            ),
        ));
    }
    if let Some(rate) = a.sample_rate {
        if ![44_100, 48_000, 96_000].contains(&rate) {
            return Err(bad(
                &format!("{P}/sample_rate"),
                "must be 44100, 48000 or 96000 Hz",
            ));
        }
        if !codec.rates().contains(&rate) {
            let list: Vec<String> = codec.rates().iter().map(ToString::to_string).collect();
            return Err(bad(
                &format!("{P}/sample_rate"),
                format!(
                    "{} takes {} Hz; change the sample rate or the codec",
                    codec.name(),
                    list.join(" or ")
                ),
            ));
        }
        next.audio_sample_rate = Some(rate);
    } else if let Some(rate) = next.audio_sample_rate
        && !codec.rates().contains(&rate)
    {
        return Err(bad(
            &format!("{P}/codec"),
            format!(
                "{} does not take {rate} Hz; change the sample rate too",
                codec.name()
            ),
        ));
    }
    if let Some(kbps) = a.bitrate_kbps {
        let Some((lo, hi)) = codec.bitrates() else {
            return Err(bad(
                &format!("{P}/bitrate_kbps"),
                format!(
                    "{} is lossless and has no bitrate; leave it out",
                    codec.name()
                ),
            ));
        };
        if !(lo..=hi).contains(&kbps) {
            return Err(bad(
                &format!("{P}/bitrate_kbps"),
                format!("{} takes {lo} to {hi} kb/s", codec.name()),
            ));
        }
        output.audio_bitrate_kbps = Some(kbps);
    } else if a.codec.is_some() && a.codec != Some(current) {
        // A codec change drops the old codec's bitrate.
        output.audio_bitrate_kbps = None;
    }
    if let Some(on) = a.limiter {
        next.audio_limiter = Some(on);
    }
    if let Some(c) = a.channels {
        next.audio_channels = Some(match c {
            AudioChannels::Mono => 1,
            AudioChannels::Stereo => 2,
        });
    }
    output.audio_codec = match codec {
        AdvancedAudioCodec::Aac => AudioCodec::Aac,
        AdvancedAudioCodec::Mp3 => AudioCodec::Mp3,
        AdvancedAudioCodec::Opus => AudioCodec::Opus,
        AdvancedAudioCodec::Flac => AudioCodec::Flac,
        AdvancedAudioCodec::PcmS16 | AdvancedAudioCodec::PcmS24 => AudioCodec::Pcm,
    };
    next.pcm_bits = match codec {
        AdvancedAudioCodec::PcmS16 => Some(16),
        AdvancedAudioCodec::PcmS24 => Some(24),
        _ => None,
    };
    Ok(())
}

/// Checks `adv` for a preset (a frame rate of 30 stands in for the sequence's) and the hardware
/// policy `hw`; the preset's own codec and container decide what is possible.
///
/// # Errors
///
/// As [`apply`].
pub fn check(
    adv: &AdvancedExportOptions,
    output: &PlanOutput,
    hw: &ExportHw,
) -> Result<(), CmdError> {
    let mut scratch = output.clone();
    let mut rate = None;
    apply(
        adv,
        &mut scratch,
        &mut rate,
        hw.clone(),
        FrameRate::FPS_30,
        &mut Vec::new(),
    )
    .map(drop)
}

/// The estimated size of the file in bytes from the bitrates and the length, when the rate
/// control fixes one (`vbr`, `cbr`, or a preset's bitrate); `None` for a quality (`crf`).
#[must_use]
pub fn estimated_bytes(
    rate: Option<PlanRate>,
    output: &PlanOutput,
    duration: Duration,
) -> Option<u64> {
    let video_kbps = match rate? {
        PlanRate::BitrateKbps(k) | PlanRate::Cbr(k) => u64::from(k),
        PlanRate::Crf(_) => return None,
    };
    let audio_kbps = match output.audio_codec {
        AudioCodec::Aac | AudioCodec::Mp3 | AudioCodec::Opus => {
            u64::from(output.audio_bitrate_kbps.unwrap_or(
                if output.audio_codec == AudioCodec::Opus {
                    128
                } else {
                    192
                },
            ))
        }
        AudioCodec::Pcm => {
            let channels = u64::from(output.advanced.audio_channels.unwrap_or(2));
            let hz = u64::from(output.advanced.audio_sample_rate.unwrap_or(48_000));
            let bits = u64::from(output.advanced.pcm_bits.unwrap_or(24));
            channels * hz * bits / 1000
        }
        AudioCodec::Flac => 0,
    };
    #[allow(clippy::cast_precision_loss)]
    let secs = cutweave_model::Time::from(duration).flicks() as f64
        / cutweave_model::FLICKS_PER_SECOND as f64;
    #[allow(clippy::cast_precision_loss)] // kilobits per second, far below 2^52
    let bytes = (video_kbps + audio_kbps) as f64 * 125.0 * secs;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(bytes.ceil() as u64)
}

// ----- export.describe_options -----------------------------------------------------------------

/// Describes the advanced options of a codec as a JSON Schema: which rate-control modes, presets,
/// tunes, profiles, pixel formats and audio codecs its encoder takes, and what it cannot.
/// `encoder` names a hardware encoder from `export.list_encoders` (default: the software
/// encoder).
#[wire]
pub struct DescribeOptions {
    /// The codec: h264, hevc, av1 or prores.
    pub codec: VideoCodec,
    /// A hardware encoder name (`hevc_nvenc`); the software encoder when absent.
    #[serde(default)]
    pub encoder: Option<String>,
    /// The container, for the audio codecs it holds (default: MP4, MOV for ProRes).
    #[serde(default)]
    pub container: Option<Container>,
}

/// Result of `export.describe_options`.
#[wire]
pub struct DescribeOptionsOutput {
    /// The codec.
    pub codec: VideoCodec,
    /// The encoder the schema is for.
    pub encoder: String,
    /// Whether it is a hardware encoder.
    pub hardware: bool,
    /// JSON Schema of `advanced` for this encoder.
    #[specta(type = specta_typescript::Unknown)]
    pub schema: Value,
}

impl Described for DescribeOptions {
    const SPEC: CommandSpec = CommandSpec {
        name: "export.describe_options",
        summary: "Describe the advanced export options of a codec or encoder as a JSON Schema",
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
        rust_type: "DescribeOptions",
    };
}

impl Registrable for DescribeOptions {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<DescribeOptions>().map(drop)
    }
}

fn software_encoder(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "libx264",
        VideoCodec::Hevc => "libx265",
        VideoCodec::Av1 => "libsvtav1",
        VideoCodec::Prores => "prores_ks",
        VideoCodec::Gif => "gif",
    }
}

/// The JSON Schema of the advanced options `caps` allows for `codec` in `container`.
#[must_use]
#[allow(clippy::too_many_lines)] // the schema as one literal
pub fn options_schema(caps: &Caps, container: Container) -> Value {
    let modes: Vec<&str> = caps.rate_modes.iter().map(|m| m.name()).collect();
    let mut video = serde_json::Map::new();
    if !caps.rate_modes.is_empty() {
        video.insert(
            "rate_control".into(),
            json!({
                "type": "object",
                "required": ["mode"],
                "additionalProperties": false,
                "properties": {
                    "mode": {"enum": modes},
                    "crf": {"type": "integer", "minimum": 0, "maximum": 51,
                            "description": "crf, cq"},
                    "bitrate_kbps": {"type": "integer", "minimum": MIN_BITRATE_KBPS,
                            "maximum": MAX_BITRATE_KBPS, "description": "cbr, vbr"},
                    "max_bitrate_kbps": {"type": "integer", "minimum": MIN_BITRATE_KBPS,
                            "maximum": MAX_BITRATE_KBPS, "description": "vbr, cq (needed)"},
                    "buffer_kbit": {"type": "integer", "minimum": 10, "maximum": MAX_BUFFER_KBIT}
                }
            }),
        );
    }
    let mut gop = serde_json::Map::new();
    if caps.keyint {
        gop.insert(
            "keyint".into(),
            json!({"type": "integer", "minimum": 1, "maximum": MAX_KEYINT}),
        );
        gop.insert(
            "keyint_seconds".into(),
            json!({"type": "integer", "minimum": 1, "maximum": MAX_KEYINT_SECONDS}),
        );
    }
    if caps.keyint_min {
        gop.insert(
            "keyint_min".into(),
            json!({"type": "integer", "minimum": 1}),
        );
    }
    if caps.bframes {
        gop.insert(
            "bframes".into(),
            json!({"type": "integer", "minimum": 0, "maximum": MAX_BFRAMES}),
        );
    }
    if caps.closed_gop {
        gop.insert("closed_gop".into(), json!({"type": "boolean"}));
    }
    if caps.scenecut {
        gop.insert("scenecut".into(), json!({"type": "boolean"}));
    }
    if !gop.is_empty() {
        video.insert(
            "gop".into(),
            json!({"type": "object", "additionalProperties": false, "properties": gop}),
        );
    }
    if caps.preset {
        video.insert("preset".into(), json!({"enum": SPEED_PRESETS}));
    }
    if !caps.tunes.is_empty() {
        video.insert("tune".into(), json!({"enum": caps.tunes}));
    }
    if !caps.profiles.is_empty() {
        video.insert("profile".into(), json!({"enum": caps.profiles}));
    }
    if caps.level {
        video.insert(
            "level".into(),
            json!({"type": "string", "pattern": "^[1-6](\\.[0-2])?$"}),
        );
    }
    if !caps.pix_fmts.is_empty() {
        video.insert("pix_fmt".into(), json!({"enum": caps.pix_fmts}));
    }
    video.insert(
        "threads".into(),
        json!({"type": "integer", "minimum": 0, "maximum": MAX_THREADS}),
    );
    let audio_codecs: Vec<&str> = audio_codecs(container).iter().map(|c| c.name()).collect();
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "video": {"type": "object", "additionalProperties": false, "properties": video},
            "color": {
                "type": "object",
                "additionalProperties": false,
                "description": "default bt709 limited; HDR transfers (smpte2084, arib-std-b67) are unsupported",
                "properties": {
                    "primaries": {"enum": ["bt709", "bt470bg", "smpte170m", "bt2020"]},
                    "transfer": {"enum": ["bt709", "smpte170m", "iec61966-2-1", "gamma22", "bt2020-10"]},
                    "matrix": {"enum": ["bt709", "bt470bg", "smpte170m", "bt2020nc"]},
                    "range": {"enum": ["limited", "full"]},
                    "tag_only": {"type": "boolean"}
                }
            },
            "audio": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "codec": {"enum": audio_codecs},
                    "bitrate_kbps": {"type": "integer", "minimum": 6, "maximum": 512,
                            "description": "lossy codecs: aac 32-512, mp3 32-320, opus 6-510"},
                    "sample_rate": {"enum": [44100, 48000, 96000],
                            "description": "opus 48000 only; mp3 not 96000"},
                    "channels": {"enum": ["mono", "stereo"]},
                    "limiter": {"type": "boolean",
                            "description": "the master's true-peak limiter at -1 dBTP; default true"}
                }
            }
        }
    })
}

impl EngineCommand for DescribeOptions {
    type Context = ExportCtx;
    type Output = DescribeOptionsOutput;

    fn run(&self, _ctx: &mut ExportCtx) -> Result<DescribeOptionsOutput, CmdError> {
        let (encoder, caps, hardware) = match &self.encoder {
            None => (
                software_encoder(self.codec).to_owned(),
                software_caps(self.codec),
                false,
            ),
            Some(name) => {
                let want = match self.codec {
                    VideoCodec::H264 => "h264_",
                    VideoCodec::Hevc => "hevc_",
                    VideoCodec::Av1 => "av1_",
                    _ => "",
                };
                let family =
                    Family::of(name).filter(|_| !want.is_empty() && name.starts_with(want));
                match family {
                    Some(f) => (name.clone(), hardware_caps(self.codec, f), true),
                    None if name == software_encoder(self.codec) => {
                        (name.clone(), software_caps(self.codec), false)
                    }
                    None => {
                        return Err(bad(
                            "/encoder",
                            format!(
                                "`{name}` is not an encoder of this codec; list them with \
                                 export.list_encoders (software: {})",
                                software_encoder(self.codec)
                            ),
                        ));
                    }
                }
            }
        };
        let container = self
            .container
            .unwrap_or(if self.codec == VideoCodec::Prores {
                Container::Mov
            } else {
                Container::Mp4
            });
        Ok(DescribeOptionsOutput {
            codec: self.codec,
            schema: options_schema(&caps, container),
            encoder,
            hardware,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(codec: VideoCodec) -> PlanOutput {
        let container = if codec == VideoCodec::Prores {
            Container::Mov
        } else {
            Container::Mp4
        };
        PlanOutput {
            container,
            video_codec: codec,
            bit_depth: if codec == VideoCodec::Prores { 10 } else { 8 },
            ..PlanOutput::default()
        }
    }

    fn rc(mode: RateMode) -> AdvancedRate {
        AdvancedRate {
            mode,
            crf: None,
            bitrate_kbps: None,
            max_bitrate_kbps: None,
            buffer_kbit: None,
            two_pass: None,
        }
    }

    fn video(v: AdvancedVideo) -> AdvancedExportOptions {
        AdvancedExportOptions {
            video: Some(v),
            ..AdvancedExportOptions::default()
        }
    }

    fn run(
        adv: &AdvancedExportOptions,
        codec: VideoCodec,
        hw: ExportHw,
    ) -> Result<(PlanOutput, Option<PlanRate>, ExportHw, Vec<PresetWarning>), CmdError> {
        let mut out = output(codec);
        let mut rate = None;
        let mut warnings = Vec::new();
        let hw = apply(
            adv,
            &mut out,
            &mut rate,
            hw,
            FrameRate::FPS_30,
            &mut warnings,
        )?;
        Ok((out, rate, hw, warnings))
    }

    fn pointer(e: &CmdError) -> String {
        match e {
            CmdError::Invalid { path, .. } => path.clone(),
            other => panic!("not Invalid: {other:?}"),
        }
    }

    fn message(e: &CmdError) -> String {
        e.to_string()
    }

    fn with_rate(r: AdvancedRate) -> AdvancedExportOptions {
        video(AdvancedVideo {
            rate_control: Some(r),
            ..AdvancedVideo::default()
        })
    }

    #[test]
    fn prores_takes_no_rate_control_and_says_what_to_set() {
        let adv = with_rate(AdvancedRate {
            crf: Some(18),
            ..rc(RateMode::Crf)
        });
        let e = run(&adv, VideoCodec::Prores, ExportHw::Off).unwrap_err();
        assert_eq!(pointer(&e), "/advanced/video/rate_control");
        assert!(message(&e).contains("profile"), "{e}");
        // Intra-only: no GOP either.
        let gop = video(AdvancedVideo {
            gop: Some(AdvancedGop {
                keyint: Some(30),
                ..AdvancedGop::default()
            }),
            ..AdvancedVideo::default()
        });
        assert_eq!(
            pointer(&run(&gop, VideoCodec::Prores, ExportHw::Off).unwrap_err()),
            "/advanced/video/gop"
        );
    }

    #[test]
    fn each_rate_mode_needs_its_numbers_and_refuses_the_others() {
        let ok = |r: AdvancedRate, codec| run(&with_rate(r), codec, ExportHw::Off);
        // crf
        let (_, rate, ..) = ok(
            AdvancedRate {
                crf: Some(18),
                ..rc(RateMode::Crf)
            },
            VideoCodec::H264,
        )
        .unwrap();
        assert_eq!(rate, Some(PlanRate::Crf(18)));
        assert!(
            ok(rc(RateMode::Crf), VideoCodec::H264).is_err(),
            "crf needs a crf"
        );
        let e = ok(
            AdvancedRate {
                crf: Some(18),
                bitrate_kbps: Some(8000),
                ..rc(RateMode::Crf)
            },
            VideoCodec::H264,
        )
        .unwrap_err();
        assert_eq!(pointer(&e), "/advanced/video/rate_control/bitrate_kbps");
        assert!(message(&e).contains("quality"), "{e}");
        // cbr
        let (out, rate, ..) = ok(
            AdvancedRate {
                bitrate_kbps: Some(8000),
                buffer_kbit: Some(4000),
                ..rc(RateMode::Cbr)
            },
            VideoCodec::H264,
        )
        .unwrap();
        assert_eq!(rate, Some(PlanRate::Cbr(8000)));
        assert_eq!(out.advanced.buffer_kbit, Some(4000));
        assert!(
            ok(
                AdvancedRate {
                    bitrate_kbps: Some(8000),
                    max_bitrate_kbps: Some(9000),
                    ..rc(RateMode::Cbr)
                },
                VideoCodec::H264
            )
            .is_err()
        );
        // vbr with a cap below the average
        let e = ok(
            AdvancedRate {
                bitrate_kbps: Some(8000),
                max_bitrate_kbps: Some(4000),
                ..rc(RateMode::Vbr)
            },
            VideoCodec::Hevc,
        )
        .unwrap_err();
        assert_eq!(pointer(&e), "/advanced/video/rate_control/max_bitrate_kbps");
        // cq needs the cap
        let (out, rate, ..) = ok(
            AdvancedRate {
                crf: Some(20),
                max_bitrate_kbps: Some(6000),
                ..rc(RateMode::Cq)
            },
            VideoCodec::H264,
        )
        .unwrap();
        assert_eq!(
            (rate, out.advanced.max_bitrate_kbps),
            (Some(PlanRate::Crf(20)), Some(6000))
        );
        assert!(
            ok(
                AdvancedRate {
                    crf: Some(20),
                    ..rc(RateMode::Cq)
                },
                VideoCodec::H264
            )
            .is_err()
        );
        // out of range
        assert!(
            ok(
                AdvancedRate {
                    crf: Some(52),
                    ..rc(RateMode::Crf)
                },
                VideoCodec::H264
            )
            .is_err()
        );
        assert!(
            ok(
                AdvancedRate {
                    bitrate_kbps: Some(10),
                    ..rc(RateMode::Vbr)
                },
                VideoCodec::H264
            )
            .is_err()
        );
    }

    #[test]
    fn constant_bitrate_is_refused_on_av1_and_two_pass_is_unsupported() {
        let cbr = with_rate(AdvancedRate {
            bitrate_kbps: Some(4000),
            ..rc(RateMode::Cbr)
        });
        let e = run(&cbr, VideoCodec::Av1, ExportHw::Off).unwrap_err();
        assert_eq!(pointer(&e), "/advanced/video/rate_control/mode");
        assert!(message(&e).contains("vbr"), "{e}");
        let two = with_rate(AdvancedRate {
            bitrate_kbps: Some(4000),
            two_pass: Some(true),
            ..rc(RateMode::Vbr)
        });
        let e = run(&two, VideoCodec::H264, ExportHw::Off).unwrap_err();
        assert!(matches!(e, CmdError::Unsupported { .. }), "{e:?}");
        assert!(message(&e).contains("max_bitrate_kbps"), "{e}");
    }

    #[test]
    fn a_hardware_encoder_takes_the_basics_and_software_only_settings_follow_the_policy() {
        let closed = video(AdvancedVideo {
            gop: Some(AdvancedGop {
                closed_gop: Some(true),
                ..AdvancedGop::default()
            }),
            ..AdvancedVideo::default()
        });
        // Forced hardware: refused with the way out.
        let e = run(
            &closed,
            VideoCodec::Hevc,
            ExportHw::Force("hevc_nvenc".into()),
        )
        .unwrap_err();
        assert_eq!(pointer(&e), "/advanced/video/gop/closed_gop");
        assert!(message(&e).contains("hw: \"off\""), "{e}");
        // Auto: software, with a warning.
        let (_, _, hw, warnings) = run(&closed, VideoCodec::Hevc, ExportHw::Auto).unwrap();
        assert_eq!(hw, ExportHw::Off);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, PresetWarningCode::SoftwareEncoder);
        // Off: fine, no warning.
        let (_, _, hw, warnings) = run(&closed, VideoCodec::Hevc, ExportHw::Off).unwrap();
        assert_eq!((hw, warnings.len()), (ExportHw::Off, 0));
        // The basics stay on the hardware encoder under Auto.
        let basics = video(AdvancedVideo {
            rate_control: Some(AdvancedRate {
                bitrate_kbps: Some(6000),
                ..rc(RateMode::Cbr)
            }),
            gop: Some(AdvancedGop {
                keyint: Some(60),
                bframes: Some(2),
                ..AdvancedGop::default()
            }),
            preset: Some("slow".into()),
            ..AdvancedVideo::default()
        });
        let (out, _, hw, warnings) = run(&basics, VideoCodec::H264, ExportHw::Auto).unwrap();
        assert_eq!((hw, warnings.len()), (ExportHw::Auto, 0));
        assert_eq!(
            (out.advanced.keyint, out.advanced.bframes),
            (Some(60), Some(2))
        );
        // VideoToolbox has no constant bitrate: refused up front with the way out.
        let e = run(
            &basics,
            VideoCodec::H264,
            ExportHw::Force("h264_videotoolbox".into()),
        )
        .unwrap_err();
        assert_eq!(pointer(&e), "/advanced/video/rate_control/mode");
        assert!(message(&e).contains("vbr"), "{e}");
    }

    #[test]
    fn keyint_seconds_follows_the_frame_rate() {
        assert_eq!(keyint_frames(2, FrameRate::FPS_30), 60);
        assert_eq!(keyint_frames(2, FrameRate::FPS_25), 50);
        // 2 s of 29.97 fps is 59.94 frames: 60.
        assert_eq!(keyint_frames(2, FrameRate::FPS_29_97), 60);
        assert_eq!(keyint_frames(1, FrameRate::FPS_23_976), 24);
        let adv = video(AdvancedVideo {
            gop: Some(AdvancedGop {
                keyint_seconds: Some(2),
                ..AdvancedGop::default()
            }),
            ..AdvancedVideo::default()
        });
        let mut out = output(VideoCodec::H264);
        apply(
            &adv,
            &mut out,
            &mut None,
            ExportHw::Off,
            FrameRate::FPS_25,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(out.advanced.keyint, Some(50));
        // Both at once is a mistake.
        let both = video(AdvancedVideo {
            gop: Some(AdvancedGop {
                keyint: Some(10),
                keyint_seconds: Some(2),
                ..AdvancedGop::default()
            }),
            ..AdvancedVideo::default()
        });
        assert!(run(&both, VideoCodec::H264, ExportHw::Off).is_err());
    }

    #[test]
    fn gop_limits_and_codec_support() {
        let gop = |g: AdvancedGop| {
            video(AdvancedVideo {
                gop: Some(g),
                ..AdvancedVideo::default()
            })
        };
        assert!(
            run(
                &gop(AdvancedGop {
                    keyint: Some(0),
                    ..AdvancedGop::default()
                }),
                VideoCodec::H264,
                ExportHw::Off
            )
            .is_err()
        );
        assert!(
            run(
                &gop(AdvancedGop {
                    keyint: Some(60),
                    keyint_min: Some(90),
                    ..AdvancedGop::default()
                }),
                VideoCodec::H264,
                ExportHw::Off
            )
            .is_err()
        );
        // SVT-AV1 has no B-frame count and no minimum distance.
        for g in [
            AdvancedGop {
                bframes: Some(2),
                ..AdvancedGop::default()
            },
            AdvancedGop {
                keyint_min: Some(2),
                ..AdvancedGop::default()
            },
        ] {
            assert!(run(&gop(g), VideoCodec::Av1, ExportHw::Off).is_err());
        }
        assert!(
            run(
                &gop(AdvancedGop {
                    closed_gop: Some(true),
                    scenecut: Some(false),
                    keyint: Some(48),
                    ..AdvancedGop::default()
                }),
                VideoCodec::Av1,
                ExportHw::Off
            )
            .is_ok()
        );
    }

    #[test]
    fn profile_pixel_format_tune_and_level_agree_with_the_codec() {
        let v = |v: AdvancedVideo| run(&video(v), VideoCodec::Hevc, ExportHw::Off);
        // main10 turns the video 10-bit.
        let (out, ..) = v(AdvancedVideo {
            profile: Some("main10".into()),
            ..AdvancedVideo::default()
        })
        .unwrap();
        assert_eq!(
            (out.profile.as_deref(), out.bit_depth),
            (Some("main10"), 10)
        );
        // main with 10-bit pixels disagree.
        let e = v(AdvancedVideo {
            profile: Some("main".into()),
            pix_fmt: Some("yuv420p10le".into()),
            ..AdvancedVideo::default()
        })
        .unwrap_err();
        assert!(message(&e).contains("main10"), "{e}");
        assert!(
            v(AdvancedVideo {
                tune: Some("film".into()),
                ..AdvancedVideo::default()
            })
            .is_err(),
            "film is x264's"
        );
        assert!(
            v(AdvancedVideo {
                tune: Some("grain".into()),
                level: Some("4.1".into()),
                ..AdvancedVideo::default()
            })
            .is_ok()
        );
        assert!(
            v(AdvancedVideo {
                level: Some("9".into()),
                ..AdvancedVideo::default()
            })
            .is_err()
        );
        assert!(
            v(AdvancedVideo {
                preset: Some("warp".into()),
                ..AdvancedVideo::default()
            })
            .is_err()
        );
        assert!(
            v(AdvancedVideo {
                threads: Some(500),
                ..AdvancedVideo::default()
            })
            .is_err()
        );
        // H.264 is 8-bit; ProRes pixel formats follow the profile.
        let e = run(
            &video(AdvancedVideo {
                pix_fmt: Some("yuv420p10le".into()),
                ..AdvancedVideo::default()
            }),
            VideoCodec::H264,
            ExportHw::Off,
        )
        .unwrap_err();
        assert_eq!(pointer(&e), "/advanced/video/pix_fmt");
        let pro = |profile: &str, fmt: &str| {
            run(
                &video(AdvancedVideo {
                    profile: Some(profile.into()),
                    pix_fmt: Some(fmt.into()),
                    ..AdvancedVideo::default()
                }),
                VideoCodec::Prores,
                ExportHw::Off,
            )
        };
        assert!(pro("4444", "yuva444p10le").unwrap().0.alpha);
        assert!(pro("422hq", "yuv444p10le").is_err());
        assert!(pro("4444", "yuv422p10le").is_err());
    }

    #[test]
    fn color_tags_are_checked_and_hdr_is_unsupported() {
        let c = |c: AdvancedColor| {
            run(
                &AdvancedExportOptions {
                    color: Some(c),
                    ..AdvancedExportOptions::default()
                },
                VideoCodec::H264,
                ExportHw::Off,
            )
        };
        let (out, ..) = c(AdvancedColor {
            primaries: Some(ColorPrimaries::Smpte170M),
            matrix: Some(MatrixCoefficients::Smpte170M),
            range: Some(ColorRange::Full),
            tag_only: Some(true),
            ..AdvancedColor::default()
        })
        .unwrap();
        assert!(out.advanced.tag_only);
        let tags = out.advanced.color.unwrap();
        assert_eq!(tags.range, Some(ColorRange::Full));
        assert_eq!(tags.transfer, None, "unset tags stay BT.709 in the encoder");
        for hdr in [TransferCharacteristics::Pq, TransferCharacteristics::Hlg] {
            let e = c(AdvancedColor {
                transfer: Some(hdr),
                ..AdvancedColor::default()
            })
            .unwrap_err();
            assert!(matches!(e, CmdError::Unsupported { .. }), "{e:?}");
        }
        let e = c(AdvancedColor {
            matrix: Some(MatrixCoefficients::Rgb),
            ..AdvancedColor::default()
        })
        .unwrap_err();
        assert_eq!(pointer(&e), "/advanced/color/matrix");
        let e = c(AdvancedColor {
            primaries: Some(ColorPrimaries::Other("p3".into())),
            ..AdvancedColor::default()
        })
        .unwrap_err();
        assert_eq!(pointer(&e), "/advanced/color/primaries");
    }

    #[test]
    fn audio_settings_follow_the_container_and_the_codec() {
        let a = |a: AdvancedAudio, codec| {
            run(
                &AdvancedExportOptions {
                    audio: Some(a),
                    ..AdvancedExportOptions::default()
                },
                codec,
                ExportHw::Off,
            )
        };
        let (out, ..) = a(
            AdvancedAudio {
                codec: Some(AdvancedAudioCodec::Opus),
                bitrate_kbps: Some(128),
                ..AdvancedAudio::default()
            },
            VideoCodec::H264,
        )
        .unwrap();
        assert_eq!(
            (out.audio_codec, out.audio_bitrate_kbps),
            (AudioCodec::Opus, Some(128))
        );
        // Opus runs at 48 kHz only; PCM does not go into MP4; FLAC has no bitrate.
        let e = a(
            AdvancedAudio {
                codec: Some(AdvancedAudioCodec::Opus),
                sample_rate: Some(44_100),
                ..AdvancedAudio::default()
            },
            VideoCodec::H264,
        )
        .unwrap_err();
        assert_eq!(pointer(&e), "/advanced/audio/sample_rate");
        let e = a(
            AdvancedAudio {
                codec: Some(AdvancedAudioCodec::PcmS16),
                ..AdvancedAudio::default()
            },
            VideoCodec::H264,
        )
        .unwrap_err();
        assert!(message(&e).contains("MOV"), "{e}");
        assert!(
            a(
                AdvancedAudio {
                    codec: Some(AdvancedAudioCodec::Flac),
                    bitrate_kbps: Some(500),
                    ..AdvancedAudio::default()
                },
                VideoCodec::H264
            )
            .is_err()
        );
        assert!(
            a(
                AdvancedAudio {
                    sample_rate: Some(22_050),
                    ..AdvancedAudio::default()
                },
                VideoCodec::H264
            )
            .is_err()
        );
        assert!(
            a(
                AdvancedAudio {
                    codec: Some(AdvancedAudioCodec::Mp3),
                    sample_rate: Some(96_000),
                    ..AdvancedAudio::default()
                },
                VideoCodec::H264
            )
            .is_err()
        );
        // MOV: PCM 16-bit, mono, 96 kHz.
        let (out, ..) = a(
            AdvancedAudio {
                codec: Some(AdvancedAudioCodec::PcmS16),
                sample_rate: Some(96_000),
                channels: Some(AudioChannels::Mono),
                ..AdvancedAudio::default()
            },
            VideoCodec::Prores,
        )
        .unwrap();
        assert_eq!(out.audio_codec, AudioCodec::Pcm);
        assert_eq!(
            (
                out.advanced.pcm_bits,
                out.advanced.audio_sample_rate,
                out.advanced.audio_channels
            ),
            (Some(16), Some(96_000), Some(1))
        );
        assert_eq!(out.advanced.audio_limiter, None);
    }

    /// WP-2E.02: the master limiter is on unless switched off, and nothing else changes.
    #[test]
    fn the_master_limiter_is_on_unless_switched_off() {
        let (out, ..) = run(
            &AdvancedExportOptions {
                audio: Some(AdvancedAudio {
                    limiter: Some(false),
                    ..AdvancedAudio::default()
                }),
                ..AdvancedExportOptions::default()
            },
            VideoCodec::H264,
            ExportHw::Off,
        )
        .unwrap();
        assert_eq!(out.advanced.audio_limiter, Some(false));
        assert_eq!(out.audio_codec, AudioCodec::Aac);
    }

    #[test]
    fn nothing_given_changes_nothing() {
        let (out, rate, hw, warnings) = run(
            &AdvancedExportOptions::default(),
            VideoCodec::Hevc,
            ExportHw::Auto,
        )
        .unwrap();
        assert_eq!(out, output(VideoCodec::Hevc));
        assert_eq!((rate, hw, warnings.len()), (None, ExportHw::Auto, 0));
    }

    #[test]
    fn a_request_overrides_the_preset_field_by_field() {
        let preset = AdvancedExportOptions {
            video: Some(AdvancedVideo {
                preset: Some("slow".into()),
                tune: Some("film".into()),
                rate_control: Some(AdvancedRate {
                    crf: Some(18),
                    ..rc(RateMode::Crf)
                }),
                ..AdvancedVideo::default()
            }),
            audio: Some(AdvancedAudio {
                bitrate_kbps: Some(128),
                ..AdvancedAudio::default()
            }),
            ..AdvancedExportOptions::default()
        };
        let request = AdvancedExportOptions {
            video: Some(AdvancedVideo {
                preset: Some("fast".into()),
                ..AdvancedVideo::default()
            }),
            ..AdvancedExportOptions::default()
        };
        let merged = merge(Some(&preset), Some(&request)).unwrap();
        let v = merged.video.unwrap();
        assert_eq!(v.preset.as_deref(), Some("fast"));
        assert_eq!(v.tune.as_deref(), Some("film"));
        assert_eq!(v.rate_control.unwrap().crf, Some(18));
        assert_eq!(merged.audio.unwrap().bitrate_kbps, Some(128));
        assert!(merge(None, None).is_none());
    }

    #[test]
    fn the_estimate_is_bitrate_times_length() {
        let out = output(VideoCodec::H264);
        let ten = Duration::from_secs(10).unwrap();
        // 8000 + 192 kb/s for 10 s = 10.24 MB.
        assert_eq!(
            estimated_bytes(Some(PlanRate::BitrateKbps(8000)), &out, ten),
            Some(10_240_000)
        );
        assert_eq!(estimated_bytes(Some(PlanRate::Crf(20)), &out, ten), None);
        assert_eq!(estimated_bytes(None, &out, ten), None);
    }

    #[test]
    fn the_schema_lists_what_the_encoder_takes() {
        let schema = |codec, family: Option<Family>| {
            let caps = family.map_or_else(|| software_caps(codec), |f| hardware_caps(codec, f));
            options_schema(&caps, Container::Mp4)
        };
        let x264 = schema(VideoCodec::H264, None);
        let modes = &x264["properties"]["video"]["properties"]["rate_control"]["properties"]["mode"]
            ["enum"];
        assert_eq!(modes, &json!(["crf", "cbr", "vbr", "cq"]));
        let nvenc = schema(VideoCodec::H264, Some(Family::Nvenc));
        let gop = &nvenc["properties"]["video"]["properties"]["gop"]["properties"];
        assert!(gop.get("keyint").is_some() && gop.get("closed_gop").is_none());
        let prores = schema(VideoCodec::Prores, None);
        assert!(
            prores["properties"]["video"]["properties"]
                .get("rate_control")
                .is_none()
        );
        assert!(
            prores["properties"]["video"]["properties"]["profile"]["enum"]
                .as_array()
                .is_some()
        );
    }
}
