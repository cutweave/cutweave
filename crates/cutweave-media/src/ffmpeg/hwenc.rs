// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardware H.264, HEVC and AV1 encoders (WP-1E.02, WP-2G.02): which ones exist per OS, how our
//! one quality setting and speed preset map onto each, which one an encode tries, and the
//! once-per-process probe (one per codec) that fills
//! [`Capabilities::hw_encode`](crate::Capabilities::hw_encode). Also the software choices of
//! every export codec (x264, x265, SVT-AV1, `prores_ks`).
//!
//! - **Order** ([`candidates`], also the order of `hw_encode` and of [`HwEncode::Auto`]):
//!   Windows NVENC, AMF, QSV; macOS VideoToolbox; Linux NVENC, QSV, VA-API. Discrete NVIDIA
//!   first, then the vendor APIs that cover the rest of the machine.
//! - **Probe** ([`available`]): each candidate this FFmpeg build has must open for a 256×256
//!   NV12 encode at the default settings and return a packet for eight frames (a real trial
//!   encode, not just the encoder's name). Cached per codec for the process; a machine without a
//!   usable encoder gets an empty list.
//! - **Mapping** ([`Choice::settings`]): `RateControl::Crf(c)` becomes each encoder's
//!   constant-quality knob at `c` plus a measured offset, `Bitrate` its VBR mode, and
//!   `SpeedPreset` its preset scale. The offsets and the measurements behind them are in
//!   docs/reference/export-quality.md.
//! - **Input**: NV12 in system memory for every encoder (the encoder uploads it), except VA-API,
//!   which takes only GPU surfaces: its frames are uploaded through a hardware frames pool first
//!   (`sys::HwFrames`). On Windows the tests run that path through D3D12VA, which shares
//!   VA-API's FFmpeg encode framework and options.
//!
//! Devices are never picked by adapter index (S3 finding H7): NVENC opens the CUDA device, AMF
//! and QSV find their own vendor's adapter, VideoToolbox has one, and VA-API takes the default
//! render node.

use std::sync::OnceLock;

use cutweave_model::FrameRate;
use ffmpeg_next::format::Pixel;

use crate::backend::{HwEncode, VideoEncodeSpec};
use crate::{HwCodec, MediaError, PixelFormat, RateControl, SpeedPreset};

/// The encoder family: decides the option mapping and how frames reach the encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Family {
    /// NVIDIA NVENC (`h264_nvenc`).
    Nvenc,
    /// AMD AMF (`h264_amf`).
    Amf,
    /// Intel Quick Sync (`h264_qsv`).
    Qsv,
    /// Apple VideoToolbox (`h264_videotoolbox`).
    VideoToolbox,
    /// VA-API (`h264_vaapi`): GPU surfaces only.
    Vaapi,
    /// D3D12VA (`h264_d3d12va`), Windows, tests only: VA-API's FFmpeg encode framework, options
    /// and surface upload on a machine without VA-API.
    #[cfg(all(test, windows))]
    D3d12va,
    /// libx264 posing as a hardware encoder, failing where told (tests only).
    #[cfg(test)]
    Pseudo(Faults),
}

/// Where a [`Family::Pseudo`] encoder fails.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Faults {
    /// While opening.
    pub open: bool,
    /// When sending this frame index.
    pub frame: Option<i64>,
    /// While flushing (in `finish`).
    pub finish: bool,
}

/// A video codec the export encoder writes (the hardware-capable ones have a candidate list).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Codec {
    H264,
    Hevc,
    Av1,
    /// Apple ProRes (`prores_ks`, software only), as an export codec: RGBA input, a chosen
    /// profile. (The proxy ProRes of WP-2F.01 is [`Intra::ProRes`].)
    ProRes,
}

impl Codec {
    /// The codec of a codec-id name (`h265` is `hevc`).
    pub(super) fn of(name: &str) -> Option<Codec> {
        match name {
            "h264" => Some(Codec::H264),
            "hevc" | "h265" => Some(Codec::Hevc),
            "av1" => Some(Codec::Av1),
            "prores" => Some(Codec::ProRes),
            _ => None,
        }
    }

    /// The codec-id name (`hevc`), as `Capabilities` lists it.
    pub(super) const fn id(self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
            Codec::Av1 => "av1",
            Codec::ProRes => "prores",
        }
    }

    /// The software encoder choice.
    pub(super) const fn software(self) -> Choice {
        match self {
            Codec::H264 => Choice::Software,
            Codec::Hevc => Choice::X265,
            Codec::Av1 => Choice::SvtAv1,
            Codec::ProRes => Choice::ProResKs,
        }
    }

    /// Every codec with hardware candidates, in the order the probe runs.
    pub(super) const HARDWARE: [Codec; 3] = [Codec::H264, Codec::Hevc, Codec::Av1];
}

/// A hardware encoder this backend knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HwEncoder {
    /// FFmpeg encoder name, the name [`HwEncode::Force`] takes (`h264_nvenc`).
    pub name: &'static str,
    /// FFmpeg hardware device type it runs on (`cuda`), as listed in `hw_encode`.
    pub device: &'static str,
    pub family: Family,
    /// The codec it writes.
    pub codec: Codec,
}

macro_rules! hw_encoder {
    ($name:ident, $codec:ident, $family:ident, $encoder:literal, $device:literal) => {
        const $name: HwEncoder = HwEncoder {
            name: $encoder,
            device: $device,
            family: Family::$family,
            codec: Codec::$codec,
        };
    };
}

hw_encoder!(NVENC, H264, Nvenc, "h264_nvenc", "cuda");
hw_encoder!(AMF, H264, Amf, "h264_amf", "amf");
hw_encoder!(QSV, H264, Qsv, "h264_qsv", "qsv");
hw_encoder!(
    VIDEOTOOLBOX,
    H264,
    VideoToolbox,
    "h264_videotoolbox",
    "videotoolbox"
);
hw_encoder!(VAAPI, H264, Vaapi, "h264_vaapi", "vaapi");
hw_encoder!(HEVC_NVENC, Hevc, Nvenc, "hevc_nvenc", "cuda");
hw_encoder!(HEVC_AMF, Hevc, Amf, "hevc_amf", "amf");
hw_encoder!(HEVC_QSV, Hevc, Qsv, "hevc_qsv", "qsv");
hw_encoder!(
    HEVC_VIDEOTOOLBOX,
    Hevc,
    VideoToolbox,
    "hevc_videotoolbox",
    "videotoolbox"
);
hw_encoder!(HEVC_VAAPI, Hevc, Vaapi, "hevc_vaapi", "vaapi");
hw_encoder!(AV1_NVENC, Av1, Nvenc, "av1_nvenc", "cuda");
hw_encoder!(AV1_AMF, Av1, Amf, "av1_amf", "amf");
hw_encoder!(AV1_QSV, Av1, Qsv, "av1_qsv", "qsv");
hw_encoder!(AV1_VAAPI, Av1, Vaapi, "av1_vaapi", "vaapi");
/// Tests only: the VA-API path on Windows.
#[cfg(all(test, windows))]
pub(super) const D3D12VA: HwEncoder = HwEncoder {
    name: "h264_d3d12va",
    device: "d3d12va",
    family: Family::D3d12va,
    codec: Codec::H264,
};

/// Every hardware encoder of the mapping table, whatever the OS (for tests of the mapping).
#[cfg(test)]
pub(super) const ALL: [HwEncoder; 5] = [NVENC, AMF, QSV, VIDEOTOOLBOX, VAAPI];

/// The hardware encoders of `codec` for this OS, in the order `Auto` tries them (none for
/// ProRes).
pub(super) const fn candidates(codec: Codec) -> &'static [HwEncoder] {
    if cfg!(windows) {
        match codec {
            Codec::H264 => &[NVENC, AMF, QSV],
            Codec::Hevc => &[HEVC_NVENC, HEVC_AMF, HEVC_QSV],
            Codec::Av1 => &[AV1_NVENC, AV1_AMF, AV1_QSV],
            Codec::ProRes => &[],
        }
    } else if cfg!(target_os = "macos") {
        match codec {
            Codec::H264 => &[VIDEOTOOLBOX],
            Codec::Hevc => &[HEVC_VIDEOTOOLBOX],
            Codec::Av1 | Codec::ProRes => &[],
        }
    } else {
        match codec {
            Codec::H264 => &[NVENC, QSV, VAAPI],
            Codec::Hevc => &[HEVC_NVENC, HEVC_QSV, HEVC_VAAPI],
            Codec::Av1 => &[AV1_NVENC, AV1_QSV, AV1_VAAPI],
            Codec::ProRes => &[],
        }
    }
}

// Measured on the reference clip (docs/reference/export-quality.md): the constant-quality value
// that gives the PSNR of x264 at the same CRF. QSV, VideoToolbox and VA-API were not measurable
// on the reference machine; their values are estimates until a machine with each runs the
// `export_quality_table` test.

/// NVENC `cq` = CRF + this (VBR constant-quality mode, no bitrate cap). Measured.
pub(super) const NVENC_CQ_OFFSET: u8 = 6;
/// AMF `qp_i`/`qp_p`/`qp_b` = CRF + this (CQP). Measured.
pub(super) const AMF_QP_OFFSET: u8 = 1;
/// QSV `global_quality` = CRF + this (ICQ). Estimate.
pub(super) const QSV_ICQ_OFFSET: u8 = 2;
/// VA-API `qp` = CRF + this (CQP; also D3D12VA). Estimate (AMF's measured CQP offset).
pub(super) const VAAPI_QP_OFFSET: u8 = 1;
/// SVT-AV1 `crf` = CRF + this: AV1's scale (0-63) runs about ten steps above x264's for the same
/// look. Estimate (docs/reference/export-quality.md); hardware AV1 encoders use the offsets of
/// their H.264 siblings until measured.
pub(super) const SVTAV1_CRF_OFFSET: u8 = 10;
/// The largest SVT-AV1 CRF.
const SVTAV1_MAX_CRF: u8 = 63;

/// `crf + offset`, kept on the 1–51 scale (0 is "automatic" for NVENC).
fn shifted(crf: u8, offset: u8) -> u8 {
    crf.saturating_add(offset).clamp(1, RateControl::MAX_CRF)
}

/// VideoToolbox constant quality (1–100, higher is better) for a CRF. Estimate.
pub(super) fn videotoolbox_quality(crf: u8) -> u8 {
    100_u8.saturating_sub(crf.saturating_mul(2)).clamp(1, 100)
}

/// FFmpeg's `FF_QP2LAMBDA`: `global_quality` is in lambda units.
const QP2LAMBDA: u32 = 118;

/// The video encoder one encode attempt uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Choice {
    /// libx264.
    Software,
    /// libx265 (HEVC).
    X265,
    /// SVT-AV1 (`libsvtav1`).
    SvtAv1,
    /// `prores_ks`, an export (RGBA input, a profile; WP-2G.02).
    ProResKs,
    /// A hardware encoder.
    Hardware(HwEncoder),
    /// An intra-frame software encoder (proxies, WP-2F.01).
    Intra(Intra),
}

/// An intra-frame video codec the encoder writes (into MOV) for proxies (WP-2F.01): every frame
/// is a keyframe, so a seek decodes one frame. Software encoders; the input is already in the
/// codec's layout ([`Intra::input_format`]) and keeps the source's color tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Intra {
    /// Apple ProRes 422 Proxy (`prores_ks`, 10-bit 4:2:2).
    ProRes,
    /// Avid DNxHR LB (`dnxhd`, 8-bit 4:2:2).
    DnxHr,
    /// Motion JPEG (`mjpeg`, 8-bit 4:2:2 in limited range).
    Mjpeg,
}

impl Intra {
    /// The intra codec of a codec-id name (`prores`, `dnxhd`, `mjpeg`).
    pub(super) fn of(codec: &str) -> Option<Intra> {
        match codec {
            "prores" => Some(Intra::ProRes),
            "dnxhd" => Some(Intra::DnxHr),
            "mjpeg" => Some(Intra::Mjpeg),
            _ => None,
        }
    }

    /// The FFmpeg encoder.
    pub(super) const fn encoder(self) -> &'static str {
        match self {
            Intra::ProRes => "prores_ks",
            Intra::DnxHr => "dnxhd",
            Intra::Mjpeg => "mjpeg",
        }
    }

    /// The layout frames must arrive in.
    pub(super) const fn input_format(self) -> PixelFormat {
        match self {
            Intra::ProRes => PixelFormat::Yuv422p10,
            Intra::DnxHr | Intra::Mjpeg => PixelFormat::Yuv422p,
        }
    }

    /// The encoder options: the proxy flavour of each codec (the rate control of the spec does
    /// not apply: the profile sets the quality).
    fn settings(self, spec: &VideoEncodeSpec) -> Vec<(&'static str, String)> {
        let threads = if spec.threads == 0 {
            "auto".to_owned()
        } else {
            spec.threads.to_string()
        };
        let mut out = vec![("threads", threads)];
        match self {
            Intra::ProRes => out.push(("profile", "proxy".to_owned())),
            Intra::DnxHr => out.push(("profile", "dnxhr_lb".to_owned())),
            Intra::Mjpeg => {
                // q 4 (2 is best, 31 worst); limited-range 4:2:2 is "unofficial" JPEG.
                out.push(("flags", "+qscale".to_owned()));
                out.push(("global_quality", (4 * QP2LAMBDA).to_string()));
                out.push(("strict", "unofficial".to_owned()));
            }
        }
        out
    }
}

/// libx264, the software H.264 encoder.
pub(super) const X264: &str = "libx264";
/// libx265, the software HEVC encoder.
pub(super) const X265: &str = "libx265";
/// SVT-AV1, the software AV1 encoder.
pub(super) const SVTAV1: &str = "libsvtav1";
/// FFmpeg's ProRes encoder.
pub(super) const PRORES_KS: &str = "prores_ks";

impl Choice {
    /// The name reported by `Encoder::video_encoder` and in logs.
    pub(super) fn name(self) -> &'static str {
        match self {
            Choice::Software => X264,
            Choice::X265 => X265,
            Choice::SvtAv1 => SVTAV1,
            Choice::ProResKs => PRORES_KS,
            Choice::Hardware(hw) => hw.name,
            Choice::Intra(intra) => intra.encoder(),
        }
    }

    /// The FFmpeg encoder to open.
    pub(super) fn ffmpeg_name(self) -> &'static str {
        match self {
            Choice::Software => X264,
            Choice::X265 => X265,
            Choice::SvtAv1 => SVTAV1,
            Choice::ProResKs => PRORES_KS,
            #[cfg(test)]
            Choice::Hardware(HwEncoder {
                family: Family::Pseudo(_),
                ..
            }) => X264,
            Choice::Hardware(hw) => hw.name,
            Choice::Intra(intra) => intra.encoder(),
        }
    }

    pub(super) fn hardware(self) -> Option<HwEncoder> {
        match self {
            Choice::Hardware(hw) => Some(hw),
            _ => None,
        }
    }

    /// Where the encoder's frames live: `None` for system-memory NV12 (or YUV 4:2:0 for x264),
    /// else the device type and pixel format of the GPU surfaces it takes.
    pub(super) fn surfaces(self) -> Option<(&'static std::ffi::CStr, ffmpeg_next::format::Pixel)> {
        match self.hardware()?.family {
            Family::Vaapi => Some((c"vaapi", ffmpeg_next::format::Pixel::VAAPI)),
            #[cfg(all(test, windows))]
            Family::D3d12va => Some((c"d3d12va", ffmpeg_next::format::Pixel::D3D12)),
            _ => None,
        }
    }

    /// The layout frames are converted to before the encoder (or the surface upload) for
    /// `spec`: 8-bit 4:2:0 (NV12 for hardware), 10-bit 4:2:0 for `bit_depth` 10 (P010 for
    /// hardware), 4:2:2 or 4:4:4 10-bit for ProRes by profile.
    pub(super) fn pixel(self, spec: &VideoEncodeSpec) -> Pixel {
        let ten = spec.bit_depth > 8;
        match self {
            Choice::Software => Pixel::YUV420P,
            Choice::X265 | Choice::SvtAv1 => {
                if ten {
                    Pixel::YUV420P10LE
                } else {
                    Pixel::YUV420P
                }
            }
            Choice::ProResKs => match prores_profile(spec.profile.as_deref()) {
                Some(p) if p.is_444() => {
                    if spec.alpha {
                        Pixel::YUVA444P10LE
                    } else {
                        Pixel::YUV444P10LE
                    }
                }
                _ => Pixel::YUV422P10LE,
            },
            Choice::Hardware(_) => {
                if ten {
                    Pixel::P010LE
                } else {
                    Pixel::NV12
                }
            }
            Choice::Intra(intra) => super::encode::pixel(intra.input_format()),
        }
    }

    /// The encoder options (`avcodec_open2` dictionary: private and generic options) for `spec`.
    pub(super) fn settings(self, spec: &VideoEncodeSpec) -> Vec<(&'static str, String)> {
        let hw = match self {
            Choice::Software => return x264_settings(spec),
            Choice::X265 => return x265_settings(spec),
            Choice::SvtAv1 => return svtav1_settings(spec),
            Choice::ProResKs => return prores_settings(spec),
            Choice::Intra(intra) => return intra.settings(spec),
            Choice::Hardware(hw) => hw,
        };
        let family = hw.family;
        let preset = spec.preset;
        let mut out: Vec<(&'static str, String)> = match hw.codec {
            Codec::H264 => vec![("profile", "high".to_owned())],
            Codec::Hevc => vec![(
                "profile",
                if spec.bit_depth > 8 { "main10" } else { "main" }.to_owned(),
            )],
            Codec::Av1 | Codec::ProRes => Vec::new(),
        };
        let mut set = |key: &'static str, value: String| out.push((key, value));
        match (family, spec.rate_control) {
            (Family::Nvenc, rc) => {
                set("preset", nvenc_preset(preset).to_owned());
                set("tune", "hq".to_owned());
                set("rc", "vbr".to_owned());
                match rc {
                    RateControl::Crf(c) => {
                        set("cq", shifted(c, NVENC_CQ_OFFSET).to_string());
                        // No bitrate cap (the generic default is 200 kb/s).
                        set("b", "0".to_owned());
                    }
                    RateControl::Bitrate(b) => set("b", b.to_string()),
                }
            }
            (Family::Amf, rc) => {
                set("usage", "transcoding".to_owned());
                set("quality", amf_quality(preset).to_owned());
                match rc {
                    RateControl::Crf(c) => {
                        let qp = shifted(c, AMF_QP_OFFSET).to_string();
                        set("rc", "cqp".to_owned());
                        set("qp_i", qp.clone());
                        set("qp_p", qp.clone());
                        if hw.codec == Codec::H264 {
                            set("qp_b", qp);
                        }
                    }
                    RateControl::Bitrate(b) => {
                        set("rc", "vbr_peak".to_owned());
                        set("b", b.to_string());
                        set("maxrate", (u64::from(b) * 2).to_string());
                    }
                }
            }
            (Family::Qsv, rc) => {
                set("preset", qsv_preset(preset).to_owned());
                match rc {
                    // ICQ: a quality target with no maximum rate.
                    RateControl::Crf(c) => {
                        set("global_quality", shifted(c, QSV_ICQ_OFFSET).to_string());
                    }
                    RateControl::Bitrate(b) => set("b", b.to_string()),
                }
            }
            (Family::VideoToolbox, rc) => {
                set(
                    "prio_speed",
                    u8::from(preset <= SpeedPreset::Faster).to_string(),
                );
                match rc {
                    RateControl::Crf(c) => {
                        set("flags", "+qscale".to_owned());
                        let q = u32::from(videotoolbox_quality(c));
                        set("global_quality", (q * QP2LAMBDA).to_string());
                    }
                    RateControl::Bitrate(b) => set("b", b.to_string()),
                }
            }
            (Family::Vaapi, rc) => {
                set("quality", vaapi_quality(preset).to_string());
                hw_base_rate_control(rc, &mut set);
            }
            #[cfg(all(test, windows))]
            (Family::D3d12va, rc) => hw_base_rate_control(rc, &mut set),
            #[cfg(test)]
            (Family::Pseudo(_), _) => return x264_settings(spec),
        }
        out
    }
}

/// x264: preset, CRF or average bitrate, threads.
fn x264_settings(spec: &VideoEncodeSpec) -> Vec<(&'static str, String)> {
    let rate = match spec.rate_control {
        RateControl::Crf(q) => ("crf", q.to_string()),
        RateControl::Bitrate(bps) => ("b", bps.to_string()),
    };
    let threads = if spec.threads == 0 {
        "auto".to_owned()
    } else {
        spec.threads.to_string()
    };
    vec![
        ("preset", spec.preset.name().to_owned()),
        rate,
        ("threads", threads),
    ]
}

/// x265: preset, CRF or average bitrate, threads, quiet log. 10-bit follows the pixel format.
fn x265_settings(spec: &VideoEncodeSpec) -> Vec<(&'static str, String)> {
    let rate = match spec.rate_control {
        RateControl::Crf(q) => ("crf", q.to_string()),
        RateControl::Bitrate(bps) => ("b", bps.to_string()),
    };
    let threads = if spec.threads == 0 {
        "auto".to_owned()
    } else {
        spec.threads.to_string()
    };
    vec![
        ("preset", spec.preset.name().to_owned()),
        rate,
        ("threads", threads),
        // x265 logs to stderr by itself; FFmpeg's log callback gets the warnings that matter.
        ("x265-params", "log-level=error".to_owned()),
    ]
}

/// SVT-AV1's numeric preset (0 slowest to 13 fastest) for a speed preset: `medium` is 6.
pub(super) fn svtav1_preset(preset: SpeedPreset) -> u8 {
    match preset {
        SpeedPreset::Ultrafast => 12,
        SpeedPreset::Superfast => 11,
        SpeedPreset::Veryfast => 10,
        SpeedPreset::Faster => 9,
        SpeedPreset::Fast => 8,
        SpeedPreset::Medium => 6,
        SpeedPreset::Slow => 4,
        SpeedPreset::Slower => 3,
        SpeedPreset::Veryslow => 2,
    }
}

/// SVT-AV1: numeric preset, CRF (shifted onto AV1's scale) or average bitrate.
fn svtav1_settings(spec: &VideoEncodeSpec) -> Vec<(&'static str, String)> {
    let rate = match spec.rate_control {
        RateControl::Crf(q) => (
            "crf",
            q.saturating_add(SVTAV1_CRF_OFFSET)
                .min(SVTAV1_MAX_CRF)
                .to_string(),
        ),
        RateControl::Bitrate(bps) => ("b", bps.to_string()),
    };
    vec![("preset", svtav1_preset(spec.preset).to_string()), rate]
}

/// A ProRes profile (`prores_ks`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProResProfile(&'static str);

impl ProResProfile {
    /// `prores_ks`'s `profile` option value.
    pub(super) const fn option(self) -> &'static str {
        self.0
    }

    /// 4:4:4 (`4444`, `4444xq`): the others are 4:2:2.
    pub(super) fn is_444(self) -> bool {
        self.0.starts_with("4444")
    }
}

/// The ProRes profile named `name` (`422hq`, `4444`; `hq` and `standard` are accepted too);
/// `None` for an unknown name. No name is `hq`.
pub(super) fn prores_profile(name: Option<&str>) -> Option<ProResProfile> {
    let option = match name.map(str::to_ascii_lowercase).as_deref() {
        None | Some("422hq" | "hq") => "hq",
        Some("422proxy" | "proxy") => "proxy",
        Some("422lt" | "lt") => "lt",
        Some("422" | "standard") => "standard",
        Some("4444") => "4444",
        Some("4444xq") => "4444xq",
        Some(_) => return None,
    };
    Some(ProResProfile(option))
}

/// `prores_ks` with Apple's vendor tag; the profile sets the quality.
fn prores_settings(spec: &VideoEncodeSpec) -> Vec<(&'static str, String)> {
    let threads = if spec.threads == 0 {
        "auto".to_owned()
    } else {
        spec.threads.to_string()
    };
    let profile = prores_profile(spec.profile.as_deref()).map_or("hq", ProResProfile::option);
    vec![
        ("profile", profile.to_owned()),
        ("vendor", "apl0".to_owned()),
        ("threads", threads),
    ]
}

/// VA-API and D3D12VA (FFmpeg's shared hardware encode framework): CQP or VBR.
fn hw_base_rate_control(rc: RateControl, set: &mut impl FnMut(&'static str, String)) {
    match rc {
        RateControl::Crf(c) => {
            set("rc_mode", "CQP".to_owned());
            set("qp", shifted(c, VAAPI_QP_OFFSET).to_string());
        }
        RateControl::Bitrate(b) => {
            set("rc_mode", "VBR".to_owned());
            set("b", b.to_string());
            set("maxrate", (u64::from(b) * 2).to_string());
        }
    }
}

/// NVENC presets p1 (fastest) to p7; x264's `medium` is p5.
fn nvenc_preset(preset: SpeedPreset) -> &'static str {
    match preset {
        SpeedPreset::Ultrafast => "p1",
        SpeedPreset::Superfast => "p2",
        SpeedPreset::Veryfast | SpeedPreset::Faster => "p3",
        SpeedPreset::Fast => "p4",
        SpeedPreset::Medium => "p5",
        SpeedPreset::Slow => "p6",
        SpeedPreset::Slower | SpeedPreset::Veryslow => "p7",
    }
}

/// AMF has three quality presets.
fn amf_quality(preset: SpeedPreset) -> &'static str {
    match preset {
        SpeedPreset::Ultrafast
        | SpeedPreset::Superfast
        | SpeedPreset::Veryfast
        | SpeedPreset::Faster => "speed",
        SpeedPreset::Fast | SpeedPreset::Medium => "balanced",
        SpeedPreset::Slow | SpeedPreset::Slower | SpeedPreset::Veryslow => "quality",
    }
}

/// QSV presets share x264's names from `veryfast` down.
fn qsv_preset(preset: SpeedPreset) -> &'static str {
    match preset {
        SpeedPreset::Ultrafast | SpeedPreset::Superfast | SpeedPreset::Veryfast => "veryfast",
        other => other.name(),
    }
}

/// VA-API's driver quality level, 1 (best) to 7 (fastest); drivers with a shorter range clamp it.
fn vaapi_quality(preset: SpeedPreset) -> u8 {
    match preset {
        SpeedPreset::Ultrafast | SpeedPreset::Superfast => 7,
        SpeedPreset::Veryfast => 6,
        SpeedPreset::Faster | SpeedPreset::Fast => 5,
        SpeedPreset::Medium => 4,
        SpeedPreset::Slow => 3,
        SpeedPreset::Slower => 2,
        SpeedPreset::Veryslow => 1,
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: the hardware encoders this thread sees instead of the OS table and the probe.
    static OVERRIDE: std::cell::RefCell<Option<Vec<HwEncoder>>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `f` with `encoders` as this thread's known and available hardware encoders (tests).
#[cfg(test)]
pub(super) fn with_encoders<T>(encoders: Vec<HwEncoder>, f: impl FnOnce() -> T) -> T {
    OVERRIDE.with(|o| *o.borrow_mut() = Some(encoders));
    let out = f();
    OVERRIDE.with(|o| *o.borrow_mut() = None);
    out
}

#[cfg(test)]
fn overridden() -> Option<Vec<HwEncoder>> {
    OVERRIDE.with(|o| o.borrow().clone())
}

/// The hardware encoders of `codec` that `Force` may name on this OS.
fn known(codec: Codec) -> Vec<HwEncoder> {
    #[cfg(test)]
    if let Some(list) = overridden() {
        return list.into_iter().filter(|hw| hw.codec == codec).collect();
    }
    candidates(codec).to_vec()
}

/// The probe's result for this process, per hardware codec ([`Codec::HARDWARE`] order).
static PROBED: [OnceLock<Vec<HwEncoder>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];

/// The hardware encoders of `codec` that passed the probe, in `Auto` order (probed on first
/// use).
pub(super) fn available(codec: Codec) -> Vec<HwEncoder> {
    #[cfg(test)]
    if let Some(list) = overridden() {
        return list.into_iter().filter(|hw| hw.codec == codec).collect();
    }
    match Codec::HARDWARE.iter().position(|c| *c == codec) {
        Some(i) => PROBED[i]
            .get_or_init(|| probe(codec, candidates(codec)))
            .clone(),
        None => Vec::new(),
    }
}

/// [`available`] for every hardware codec, as `Capabilities::hw_encode` entries.
pub(super) fn capabilities() -> Vec<HwCodec> {
    Codec::HARDWARE
        .into_iter()
        .flat_map(available)
        .map(|hw| HwCodec {
            device: hw.device.to_owned(),
            codec: hw.codec.id().to_owned(),
            implementation: hw.name.to_owned(),
        })
        .collect()
}

/// The encoders one encode of `codec` tries, in order, for policy `hw`: the hardware ones, then
/// the codec's software encoder (ProRes has only the software one).
pub(super) fn choices(hw: &HwEncode, codec: Codec) -> Result<Vec<Choice>, MediaError> {
    match hw {
        HwEncode::Off => Ok(vec![codec.software()]),
        HwEncode::Auto => Ok(available(codec)
            .into_iter()
            .map(Choice::Hardware)
            .chain([codec.software()])
            .collect()),
        HwEncode::Force(name) => {
            let name = name.trim();
            let known = known(codec);
            known
                .iter()
                .find(|hw| hw.name.eq_ignore_ascii_case(name))
                .map(|hw| vec![Choice::Hardware(*hw)])
                .ok_or_else(|| MediaError::HwUnavailable {
                    reason: format!(
                        "`{name}` is not a hardware {} encoder on this system (known here: {})",
                        codec.id(),
                        if known.is_empty() {
                            "none".to_owned()
                        } else {
                            known
                                .iter()
                                .map(|hw| hw.name)
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                })
        }
    }
}

/// Probe size: small enough to be quick, large enough for every encoder's minimum.
pub(super) const PROBE_SIZE: u32 = 256;
/// Frames of the trial encode.
pub(super) const PROBE_FRAMES: i64 = 8;

/// The encoders of `candidates` that complete a 256x256 trial encode of eight frames here.
pub(super) fn probe(codec: Codec, candidates: &[HwEncoder]) -> Vec<HwEncoder> {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for hw in candidates {
        match probe_one(codec, *hw) {
            Ok(()) => found.push(*hw),
            Err(why) => missing.push(format!("{} ({why})", hw.name)),
        }
    }
    let names: Vec<&str> = found.iter().map(|hw| hw.name).collect();
    tracing::info!(
        target: "cutweave_media",
        "hardware {} encoders: {}{}",
        codec.id(),
        if names.is_empty() { "none".to_owned() } else { names.join(", ") },
        if missing.is_empty() {
            String::new()
        } else {
            format!("; unavailable: {}", missing.join("; "))
        }
    );
    found
}

/// Opens `hw` at the default settings, encodes eight frames and expects a packet.
fn probe_one(codec: Codec, hw: HwEncoder) -> Result<(), String> {
    if ffmpeg_next::encoder::find_by_name(Choice::Hardware(hw).ffmpeg_name()).is_none() {
        return Err("not in this FFmpeg build".to_owned());
    }
    let spec = VideoEncodeSpec {
        codec: codec.id().to_owned(),
        ..VideoEncodeSpec::h264(PROBE_SIZE, PROBE_SIZE, FrameRate::FPS_30, PixelFormat::Nv12)
    };
    super::encode::test_encode(&spec, Choice::Hardware(hw), PROBE_FRAMES)
}

#[cfg(test)]
mod encode_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(rc: RateControl, preset: SpeedPreset) -> VideoEncodeSpec {
        VideoEncodeSpec {
            rate_control: rc,
            preset,
            ..VideoEncodeSpec::h264(1920, 1080, FrameRate::FPS_30, PixelFormat::Rgba8)
        }
    }

    fn get<'a>(settings: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
        settings
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn the_default_quality_maps_to_each_encoders_constant_quality_mode() {
        let crf20 = spec(RateControl::Crf(20), SpeedPreset::Medium);
        let s = |hw: HwEncoder| Choice::Hardware(hw).settings(&crf20);
        let nvenc = s(NVENC);
        assert_eq!(get(&nvenc, "rc"), Some("vbr"));
        assert_eq!(get(&nvenc, "cq"), Some("26"));
        assert_eq!(get(&nvenc, "b"), Some("0"), "no 200 kb/s default cap");
        assert_eq!(get(&nvenc, "preset"), Some("p5"));
        let amf = s(AMF);
        assert_eq!(get(&amf, "rc"), Some("cqp"));
        for k in ["qp_i", "qp_p", "qp_b"] {
            assert_eq!(get(&amf, k), Some("21"), "{k}");
        }
        assert_eq!(get(&amf, "quality"), Some("balanced"));
        let qsv = s(QSV);
        assert_eq!(get(&qsv, "global_quality"), Some("22"));
        assert_eq!(get(&qsv, "b"), None, "ICQ needs no rate");
        assert_eq!(get(&qsv, "preset"), Some("medium"));
        let vt = s(VIDEOTOOLBOX);
        assert_eq!(get(&vt, "flags"), Some("+qscale"));
        assert_eq!(get(&vt, "global_quality"), Some("7080"), "quality 60 × 118");
        let vaapi = s(VAAPI);
        assert_eq!(get(&vaapi, "rc_mode"), Some("CQP"));
        assert_eq!(get(&vaapi, "qp"), Some("21"));
        assert_eq!(get(&vaapi, "quality"), Some("4"));
        for hw in ALL {
            assert_eq!(get(&s(hw), "profile"), Some("high"), "{}", hw.name);
        }
        // x264 keeps its own settings.
        let x264 = Choice::Software.settings(&crf20);
        assert_eq!(
            x264,
            [
                ("preset", "medium".to_owned()),
                ("crf", "20".to_owned()),
                ("threads", "auto".to_owned())
            ]
        );
    }

    #[test]
    fn quality_values_stay_on_each_scale() {
        for crf in [0, 1, 18, 20, 23, 45, 50, 51] {
            for (hw, key) in [
                (NVENC, "cq"),
                (AMF, "qp_p"),
                (QSV, "global_quality"),
                (VAAPI, "qp"),
            ] {
                let settings = Choice::Hardware(hw)
                    .settings(&spec(RateControl::Crf(crf), SpeedPreset::Medium));
                let v: u8 = get(&settings, key).and_then(|v| v.parse().ok()).expect(key);
                assert!((1..=51).contains(&v), "{} at CRF {crf}: {v}", hw.name);
            }
            assert!((1..=100).contains(&videotoolbox_quality(crf)));
        }
        // Lower CRF is better quality everywhere.
        assert!(videotoolbox_quality(18) > videotoolbox_quality(23));
        assert_eq!(shifted(18, NVENC_CQ_OFFSET), 24);
    }

    #[test]
    fn a_bitrate_maps_to_each_encoders_vbr_mode() {
        let rc = RateControl::Bitrate(8_000_000);
        for hw in ALL {
            let settings = Choice::Hardware(hw).settings(&spec(rc, SpeedPreset::Medium));
            assert_eq!(get(&settings, "b"), Some("8000000"), "{}", hw.name);
            for quality in ["cq", "qp", "qp_p", "global_quality"] {
                assert_eq!(get(&settings, quality), None, "{}: {quality}", hw.name);
            }
        }
        let amf = Choice::Hardware(AMF).settings(&spec(rc, SpeedPreset::Medium));
        assert_eq!(get(&amf, "rc"), Some("vbr_peak"));
        assert_eq!(get(&amf, "maxrate"), Some("16000000"));
        let vaapi = Choice::Hardware(VAAPI).settings(&spec(rc, SpeedPreset::Medium));
        assert_eq!(get(&vaapi, "rc_mode"), Some("VBR"));
    }

    #[test]
    fn speed_presets_keep_their_order_on_every_encoder() {
        let all = [
            SpeedPreset::Ultrafast,
            SpeedPreset::Superfast,
            SpeedPreset::Veryfast,
            SpeedPreset::Faster,
            SpeedPreset::Fast,
            SpeedPreset::Medium,
            SpeedPreset::Slow,
            SpeedPreset::Slower,
            SpeedPreset::Veryslow,
        ];
        let nvenc: Vec<&str> = all.iter().map(|p| nvenc_preset(*p)).collect();
        assert!(nvenc.is_sorted(), "{nvenc:?}"); // p1 … p7
        let vaapi: Vec<u8> = all.iter().map(|p| vaapi_quality(*p)).collect();
        assert!(vaapi.is_sorted_by(|a, b| a >= b), "{vaapi:?}"); // 7 (fast) … 1
        let qsv_rank = |p: SpeedPreset| {
            [
                "veryfast", "faster", "fast", "medium", "slow", "slower", "veryslow",
            ]
            .iter()
            .position(|n| *n == qsv_preset(p))
            .expect("a QSV preset")
        };
        assert!(all.map(qsv_rank).is_sorted());
        let amf_rank = |p: SpeedPreset| {
            ["speed", "balanced", "quality"]
                .iter()
                .position(|n| *n == amf_quality(p))
                .expect("an AMF quality")
        };
        assert!(all.map(amf_rank).is_sorted());
    }

    #[test]
    fn off_is_x264_auto_ends_with_it_and_force_names_one_encoder() {
        let pseudo = HwEncoder {
            name: "test_hw",
            device: "test",
            family: Family::Pseudo(Faults::default()),
            codec: Codec::H264,
        };
        with_encoders(vec![pseudo], || {
            assert_eq!(
                choices(&HwEncode::Off, Codec::H264),
                Ok(vec![Choice::Software])
            );
            assert_eq!(
                choices(&HwEncode::Auto, Codec::H264),
                Ok(vec![Choice::Hardware(pseudo), Choice::Software])
            );
            assert_eq!(
                choices(&HwEncode::Force(" TEST_HW ".into()), Codec::H264),
                Ok(vec![Choice::Hardware(pseudo)])
            );
            let err = choices(&HwEncode::Force("h264_nosuch".into()), Codec::H264);
            assert!(
                matches!(&err, Err(MediaError::HwUnavailable { reason }) if reason.contains("test_hw")),
                "{err:?}"
            );
        });
        with_encoders(Vec::new(), || {
            assert_eq!(
                choices(&HwEncode::Auto, Codec::H264),
                Ok(vec![Choice::Software])
            );
        });
    }

    #[test]
    fn the_os_table_is_in_auto_order() {
        let names: Vec<&str> = candidates(Codec::H264).iter().map(|hw| hw.name).collect();
        let want: &[&str] = if cfg!(windows) {
            &["h264_nvenc", "h264_amf", "h264_qsv"]
        } else if cfg!(target_os = "macos") {
            &["h264_videotoolbox"]
        } else {
            &["h264_nvenc", "h264_qsv", "h264_vaapi"]
        };
        assert_eq!(names, want);
        assert_eq!(Choice::Software.name(), "libx264");
        let s = spec(RateControl::default(), SpeedPreset::Medium);
        assert_eq!(Choice::Hardware(NVENC).pixel(&s), Pixel::NV12);
        assert!(Choice::Hardware(VAAPI).surfaces().is_some());
        assert!(Choice::Hardware(NVENC).surfaces().is_none());
    }
}
