// SPDX-License-Identifier: GPL-3.0-or-later
//! The FFmpeg [`Encoder`]: H.264 (libx264 or a hardware encoder) and AAC (FFmpeg's native
//! encoder) in MP4 (WP-1E.01, hardware WP-1E.02), HEVC, AV1 and ProRes exports (WP-2G.02), and
//! the intra-frame proxy codecs (ProRes Proxy, DNxHR LB, MJPEG) with 16-bit PCM in MOV
//! (WP-2F.01).
//!
//! - **Export codecs** (WP-2G.02). H.264 and HEVC go into MP4 or MOV, AV1 into MP4, ProRes into
//!   MOV. HEVC is libx265 or a hardware encoder (`hevc_nvenc`, `hevc_amf`, `hevc_qsv`, ...), tagged
//!   `hvc1` so Apple's players open it; AV1 is SVT-AV1 or a hardware encoder; ProRes is
//!   `prores_ks` with a profile from proxy to 4444 XQ (4444 keeps alpha) in 10-bit. `bit_depth`
//!   10 gives 10-bit HEVC and AV1 (RGBA, `P010` or `Yuv420p10` input). Every export tags BT.709
//!   limited range in the bitstream and the container, and writes the MP4/MOV index in front
//!   (`+faststart`). A hardware encoder that fails to open is skipped; one that fails midway ends
//!   the encode with [`MediaError::HwUnavailable`] and the caller encodes again in software
//!   ([`encode_with_fallback`](crate::encode_with_fallback)). A ProRes export takes RGBA frames
//!   (or 4:2:2 10-bit ones for 4:2:2 profiles); the proxy path below takes only the codec's own
//!   layout.
//! - **Proxies** (WP-2F.01). Codec `prores`, `dnxhd` or `mjpeg` in container `mov`: frames
//!   arrive in the codec's own layout (`Yuv422p10` for ProRes, `Yuv422p` otherwise) and are
//!   copied, never converted; the stream is tagged with the spec's color tags as they are (an
//!   unset tag stays unspecified, so a proxy reads like its original). Audio may be `pcm_s16le`
//!   (1 to 8 channels, any rate): pushed f32 samples are interleaved and rounded to 16 bits.
//!
//! - **Video.** libx264 in `yuv420p` with CRF or an average bitrate and an x264 speed preset,
//!   or a hardware encoder in NV12 with the same settings mapped onto its own (`hwenc`). Which
//!   one follows [`EncodeSpec::hw`]: `Auto` tries the probed hardware encoders in order and then
//!   x264, skipping (and logging) any that fails to open; `Force` opens one hardware encoder;
//!   `Off` opens x264. A hardware encoder that fails after it opened ends the encode with
//!   [`MediaError::HwUnavailable`] (the caller re-encodes in software). BT.709 primaries,
//!   transfer and matrix in limited range go into the SPS (VUI) and the MP4 `colr` box. RGBA
//!   input is converted by swscale with the BT.709 matrix into limited range; NV12 and YUV
//!   4:2:0 input are only re-laid out. Constant frame rate: the codec time base is one frame,
//!   so frame `n` (pts `rate.frame_to_time(n)` in flicks, checked by `InputCheck`) gets pts
//!   `n`. This is the only place flicks become a codec time base (arch §4.1).
//! - **Audio.** FFmpeg's native `aac` takes planar f32, the layout of [`AudioChunk`], so pushed
//!   samples are only cut into its 1024-sample frames (the last may be short). Pts count
//!   samples.
//! - **Container.** MP4 with `movflags=+faststart` (the index ahead of the media data);
//!   `av_interleaved_write_frame` interleaves the streams.
//! - **Output file.** Written to a sibling `.<name>.<pid>-<n>.partial` through a Rust writer
//!   (custom AVIO) and renamed over the target when `finish` succeeds; removed on cancel, failure
//!   or drop, so an existing target survives until then. The writer keeps the OS error that
//!   FFmpeg only sees as `EIO` (a full disk becomes [`MediaError::Io`] with the OS message) and
//!   polls the cancel check, so a cancel also stops a `finish` that is flushing or moving the
//!   index.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_model::{
    ColorPrimaries, ColorRange, ColorTags, Duration, FrameRate, MatrixCoefficients, SampleRate,
    TransferCharacteristics,
};
use ffmpeg_next::codec::{self, encoder};
use ffmpeg_next::format::context::{Output, StreamIo};
use ffmpeg_next::format::{self, Pixel, Sample, sample};
use ffmpeg_next::util::color;
use ffmpeg_next::{ChannelLayout, Dictionary, Packet, Rational, frame};

use super::hwenc::{self, Choice, Codec, Intra};
use super::sys::{HwFrames, Scaler};
use crate::backend::{
    AudioEncodeSpec, EncodeSpec, EncodeSummary, Encoder, VideoAdvanced, VideoEncodeSpec,
};
use crate::encode::{InputCheck, output_color};
use crate::path::display;
use crate::{
    AudioChunk, CancelCheck, MediaError, PixelFormat, ProgressSink, RateControl, VideoFrame,
};

/// The containers this encoder writes.
const MP4: &str = "mp4";
const MOV: &str = "mov";
/// The AAC encoder (FFmpeg's own; no libfdk).
const AAC: &str = "aac";
/// 16-bit little-endian PCM (proxies, in MOV).
const PCM_S16LE: &str = "pcm_s16le";
/// 24-bit little-endian PCM (ProRes exports, in MOV).
const PCM_S24LE: &str = "pcm_s24le";
/// The codec-id names of the other audio codecs of the advanced export controls.
const MP3: &str = "mp3";
const OPUS: &str = "opus";
const FLAC: &str = "flac";
/// swscale's BT.709 matrix.
const SWS_CS_ITU709: std::ffi::c_int = ffmpeg_next::ffi::SWS_CS_ITU709;

/// What the encoder's file writer needs: the partial file, or a test's wrapper around it.
pub(super) trait WriteSeek: Write + Seek + Send {}
impl<T: Write + Seek + Send> WriteSeek for T {}

/// Opens an encoder for `spec` (already validated) writing through the partial file itself.
pub(super) fn open(spec: &EncodeSpec) -> Result<Box<dyn Encoder>, MediaError> {
    open_with(spec, &|file| Box::new(file))
}

/// [`open`], with the partial file wrapped by `wrap` (tests simulate a full disk with it).
///
/// Tries the video encoders [`hwenc::choices`] lists for `spec.hw` in order: a hardware encoder
/// that fails to open is logged and skipped while another choice is left (`Auto` always ends
/// with x264); the last choice's error is returned as it is.
pub(super) fn open_with(
    spec: &EncodeSpec,
    wrap: &dyn Fn(File) -> Box<dyn WriteSeek>,
) -> Result<Box<dyn Encoder>, MediaError> {
    let container = check_container(spec)?;
    let video = spec.video.as_ref().map(VideoPlan::new).transpose()?;
    let audio = spec
        .audio
        .as_ref()
        .map(|a| AudioPlan::new(a, container))
        .transpose()?;
    check_target(&spec.path)?;
    if let Some(plan) = &video {
        plan.check_container(container)?;
    }
    let choices = match &video {
        Some(plan) => match (plan.intra, plan.codec) {
            (Some(intra), _) => vec![Choice::Intra(intra)],
            (None, codec) => hwenc::choices(&spec.hw, codec)?,
        },
        None => vec![Choice::Software],
    };
    // (Formatted before the log macros, whose scope has a `display` of their own.)
    let path = display(&spec.path);
    let mut choices = choices.into_iter().peekable();
    while let Some(choice) = choices.next() {
        let video = video.as_ref().map(|plan| (plan, choice));
        match open_attempt(spec, container, video, audio.as_ref(), wrap) {
            Ok(enc) => {
                if let Some(name) = enc.video_encoder() {
                    tracing::info!(
                        target: "cutweave_media",
                        "encoding the video of `{path}` with {name}"
                    );
                }
                return Ok(enc);
            }
            Err(MediaError::HwUnavailable { reason }) if choices.peek().is_some() => {
                let next = choices.peek().map_or("another encoder", |c| c.name());
                tracing::warn!(
                    target: "cutweave_media",
                    "{reason}; trying {next} for `{path}`"
                );
            }
            Err(e) => return Err(e),
        }
    }
    Err(MediaError::Backend {
        message: "no video encoder to try".to_owned(),
    })
}

/// One attempt of [`open_with`] with one video encoder.
fn open_attempt(
    spec: &EncodeSpec,
    container: &'static str,
    video: Option<(&VideoPlan, Choice)>,
    audio: Option<&AudioPlan>,
    wrap: &dyn Fn(File) -> Box<dyn WriteSeek>,
) -> Result<Box<FfmpegEncoder>, MediaError> {
    let (partial, file) = PartialFile::create(&spec.path)?;
    let state = Arc::new(SinkState::default());
    let sink = Sink {
        inner: wrap(file),
        pos: 0,
        state: Arc::clone(&state),
        cancel: spec.cancel.clone(),
    };
    let mut enc = FfmpegEncoder {
        out: None,
        video: None,
        audio: None,
        partial: Some(partial),
        path: spec.path.clone(),
        video_encoder: video.map(|(_, choice)| choice.name()),
        input: InputCheck::new(spec),
        cancel: spec.cancel.clone(),
        progress: spec.progress.clone(),
        io: state,
        encoded: Duration::ZERO,
        failed: None,
    };
    // From here on, an error drops `enc`, which closes and removes the partial file.
    match enc.start(sink, container, video, audio) {
        Ok(()) => Ok(Box::new(enc)),
        Err(fault) => Err(enc.fail(fault)),
    }
}

/// MP4 (named, or from a `.mp4` / `.m4v` extension) or MOV (`.mov`): the muxer's name.
fn check_container(spec: &EncodeSpec) -> Result<&'static str, MediaError> {
    let name = match &spec.container {
        Some(name) => name.to_ascii_lowercase(),
        None => match spec
            .path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("mp4" | "m4v") => MP4.to_owned(),
            Some(ext) => ext.to_owned(),
            None => String::new(),
        },
    };
    match name.as_str() {
        MP4 => return Ok(MP4),
        MOV => return Ok(MOV),
        _ => {}
    }
    Err(MediaError::Unsupported {
        message: format!(
            "`{}`: the {} container; this backend writes MP4 and MOV (other containers arrive \
             with the export presets, P2)",
            display(&spec.path),
            if name.is_empty() { "unnamed" } else { &name }
        ),
    })
}

/// The path must not be a folder, and its folder must exist.
fn check_target(path: &Path) -> Result<(), MediaError> {
    let invalid = |reason: String| MediaError::InvalidPath {
        path: display(path),
        reason,
    };
    if path.is_dir() {
        return Err(invalid("is a folder".to_owned()));
    }
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    if !parent.is_dir() {
        return Err(invalid(format!(
            "the folder `{}` does not exist",
            display(parent)
        )));
    }
    Ok(())
}

fn unsupported(message: String) -> MediaError {
    MediaError::Unsupported { message }
}

/// A [`FrameRate`] or sample rate as an FFmpeg rational.
fn rational(num: u32, den: u32) -> Result<Rational, MediaError> {
    match (i32::try_from(num), i32::try_from(den)) {
        (Ok(n), Ok(d)) => Ok(Rational::new(n, d)),
        _ => Err(unsupported(format!("the rate {num}/{den} is out of range"))),
    }
}

/// The checked video settings, before anything is opened (whichever encoder then opens).
pub(super) struct VideoPlan {
    spec: VideoEncodeSpec,
    /// The intra-frame proxy codec, for `prores` (4:2:2 10-bit input), `dnxhd` and `mjpeg`;
    /// `None` for the export codecs.
    intra: Option<Intra>,
    /// The export codec (H.264 for a proxy plan, where it is not used).
    codec: Codec,
    /// One frame.
    time_base: Rational,
    fps: Rational,
}

/// Whether a `prores` spec is the proxy encode of WP-2F.01 (the codec's own layout, no profile)
/// rather than an export.
fn is_proxy_prores(spec: &VideoEncodeSpec) -> bool {
    spec.input_format == PixelFormat::Yuv422p10 && spec.profile.is_none() && !spec.alpha
}

/// The input and settings of a ProRes export: a known profile, RGBA frames (or 4:2:2 10-bit
/// ones for the 4:2:2 profiles), alpha only in 4444.
fn check_prores(spec: &VideoEncodeSpec, is_444: bool) -> Result<(), MediaError> {
    if hwenc::prores_profile(spec.profile.as_deref()).is_none() {
        return Err(unsupported(format!(
            "ProRes profile `{}`: proxy, lt, standard, hq, 4444 or 4444xq",
            spec.profile.as_deref().unwrap_or_default()
        )));
    }
    let ok = spec.input_format == PixelFormat::Rgba8
        || (spec.input_format == PixelFormat::Yuv422p10 && !is_444 && !spec.alpha);
    if !ok {
        return Err(unsupported(format!(
            "{:?} input: ProRes takes Rgba8 frames (Yuv422p10 for the 4:2:2 profiles)",
            spec.input_format
        )));
    }
    if spec.alpha && !is_444 {
        return Err(unsupported(
            "alpha needs the ProRes 4444 or 4444xq profile".to_owned(),
        ));
    }
    Ok(())
}

/// The advanced settings (WP-2G.05) an export codec cannot take: ProRes none (it is intra-only
/// and set by its profile), SVT-AV1 no constant bitrate, B-frame count, minimum keyframe
/// distance or level.
fn check_advanced(spec: &VideoEncodeSpec, codec: Codec) -> Result<(), MediaError> {
    let adv = &spec.advanced;
    let refuse = |what: &str| {
        Err(unsupported(format!(
            "{} takes no {what} here (use the settings `export.describe_options` lists)",
            codec.id()
        )))
    };
    match codec {
        Codec::ProRes => {
            let given = VideoAdvanced {
                tag_only: false,
                ..adv.clone()
            };
            if given != VideoAdvanced::default() {
                return refuse("encoder settings: ProRes is set by its profile");
            }
        }
        Codec::Av1 => {
            if matches!(spec.rate_control, RateControl::Cbr(_)) {
                return refuse("constant bitrate (use a variable bitrate)");
            }
            if adv.bframes.is_some() {
                return refuse("B-frame count");
            }
            if adv.keyint_min.is_some() {
                return refuse("minimum keyframe distance");
            }
            if adv.level.is_some() {
                return refuse("level");
            }
        }
        Codec::H264 | Codec::Hevc => {}
    }
    Ok(())
}

/// The input and settings of an H.264, HEVC or AV1 export: 8-bit input for 8-bit video, 10-bit
/// for 10-bit, no alpha, HEVC's profile following the bit depth.
fn check_coded(spec: &VideoEncodeSpec, codec: Codec) -> Result<(), MediaError> {
    let ten = spec.bit_depth > 8;
    let formats: [PixelFormat; 3] = if ten {
        [
            PixelFormat::Rgba8,
            PixelFormat::P010,
            PixelFormat::Yuv420p10,
        ]
    } else {
        [PixelFormat::Rgba8, PixelFormat::Nv12, PixelFormat::Yuv420p]
    };
    if !formats.contains(&spec.input_format) {
        return Err(unsupported(format!(
            "{:?} input: {}-bit {} takes {:?} frames",
            spec.input_format,
            if ten { 10 } else { 8 },
            codec.id(),
            formats
        )));
    }
    if spec.alpha {
        return Err(unsupported(format!("{} has no alpha here", codec.id())));
    }
    let want = if ten { "main10" } else { "main" };
    if codec == Codec::Hevc
        && let Some(profile) = spec.profile.as_deref()
        && profile != want
    {
        return Err(unsupported(format!(
            "HEVC profile `{profile}` at {} bits: `{want}`",
            if ten { 10 } else { 8 },
        )));
    }
    Ok(())
}

impl VideoPlan {
    #[allow(clippy::too_many_lines)] // one flat run of checks, before anything opens
    pub(super) fn new(spec: &VideoEncodeSpec) -> Result<VideoPlan, MediaError> {
        if let Some(intra) = Intra::of(&spec.codec)
            && (intra != Intra::ProRes || is_proxy_prores(spec))
        {
            return VideoPlan::intra(spec, intra);
        }
        let Some(codec) = Codec::of(&spec.codec) else {
            return Err(unsupported(format!(
                "no `{}` encoder: this backend writes H.264, HEVC, AV1 and ProRes",
                spec.codec
            )));
        };
        let software = codec.software().name();
        if encoder::find_by_name(software).is_none() {
            return Err(unsupported(format!("this FFmpeg build has no {software}")));
        }
        let ten = spec.bit_depth > 8;
        if codec == Codec::H264 && ten {
            return Err(unsupported(
                "10-bit H.264 is not exported: use HEVC or AV1 for 10-bit video".to_owned(),
            ));
        }
        let prores_444 = codec == Codec::ProRes
            && hwenc::prores_profile(spec.profile.as_deref())
                .is_some_and(hwenc::ProResProfile::is_444);
        if codec == Codec::ProRes {
            check_prores(spec, prores_444)?;
        } else {
            check_coded(spec, codec)?;
        }
        check_advanced(spec, codec)?;
        let even_height = codec != Codec::ProRes;
        if !spec.width.is_multiple_of(2) && !prores_444
            || even_height && !spec.height.is_multiple_of(2)
        {
            return Err(MediaError::Invalid {
                message: format!(
                    "{} needs an even width{} here, not {}x{}",
                    codec.id(),
                    if even_height { " and height" } else { "" },
                    spec.width,
                    spec.height
                ),
            });
        }
        let color = output_color(&spec.color);
        if matches!(
            color.transfer,
            Some(TransferCharacteristics::Pq | TransferCharacteristics::Hlg)
        ) {
            return Err(unsupported(format!(
                "output transfer {}: HDR output is not exported yet",
                color
                    .transfer
                    .as_ref()
                    .map_or("", TransferCharacteristics::as_str)
            )));
        }
        if let Some(name) = unknown_color_name(&color) {
            return Err(unsupported(format!(
                "output color `{name}`: not a color tag this encoder writes"
            )));
        }
        if !spec.advanced.tag_only
            && spec.input_format == PixelFormat::Rgba8
            && sws_matrix(color.matrix.as_ref()).is_none()
        {
            return Err(unsupported(format!(
                "matrix {}: the picture cannot be converted with it; write it as a tag only (tag_only)",
                color.matrix.as_ref().map_or("", MatrixCoefficients::as_str)
            )));
        }
        let (num, den) = (spec.rate.num(), spec.rate.den());
        Ok(VideoPlan {
            spec: spec.clone(),
            intra: None,
            codec,
            time_base: rational(den, num)?,
            fps: rational(num, den)?,
        })
    }

    /// The plan of an intra-frame proxy encode (see the module docs).
    fn intra(spec: &VideoEncodeSpec, intra: Intra) -> Result<VideoPlan, MediaError> {
        if encoder::find_by_name(intra.encoder()).is_none() {
            return Err(unsupported(format!(
                "this FFmpeg build has no {}",
                intra.encoder()
            )));
        }
        if spec.input_format != intra.input_format() {
            return Err(unsupported(format!(
                "{:?} input: {} takes {:?} frames",
                spec.input_format,
                intra.encoder(),
                intra.input_format()
            )));
        }
        if !spec.width.is_multiple_of(2) {
            return Err(MediaError::Invalid {
                message: format!("4:2:2 needs an even width, not {}", spec.width),
            });
        }
        let (num, den) = (spec.rate.num(), spec.rate.den());
        Ok(VideoPlan {
            spec: spec.clone(),
            intra: Some(intra),
            codec: Codec::H264,
            time_base: rational(den, num)?,
            fps: rational(num, den)?,
        })
    }

    /// Whether the codec goes into `container`: H.264 and HEVC into MP4 or MOV, AV1 into MP4,
    /// ProRes into MOV (proxies are not checked: they are MOV by construction).
    fn check_container(&self, container: &'static str) -> Result<(), MediaError> {
        if self.intra.is_some() {
            return Ok(());
        }
        let ok = match self.codec {
            Codec::H264 | Codec::Hevc => true,
            Codec::Av1 => container == MP4,
            Codec::ProRes => container == MOV,
        };
        if ok {
            Ok(())
        } else {
            Err(unsupported(format!(
                "{} does not go into {}: {}",
                self.codec.id(),
                container.to_ascii_uppercase(),
                if self.codec == Codec::Av1 {
                    "AV1 goes into MP4"
                } else {
                    "ProRes goes into MOV"
                }
            )))
        }
    }
}

/// The first tag of `tags` that is a name this build does not know (`Other`), if any.
fn unknown_color_name(tags: &ColorTags) -> Option<String> {
    if let Some(ColorPrimaries::Other(n)) = &tags.primaries {
        return Some(n.clone());
    }
    if let Some(TransferCharacteristics::Other(n)) = &tags.transfer {
        return Some(n.clone());
    }
    if let Some(MatrixCoefficients::Other(n)) = &tags.matrix {
        return Some(n.clone());
    }
    None
}

/// swscale's matrix constant for `matrix`, and whether swscale can convert with it (RGB and the
/// constant-luminance, ICtCp and YCgCo matrices cannot).
fn sws_matrix(matrix: Option<&MatrixCoefficients>) -> Option<std::ffi::c_int> {
    use ffmpeg_next::ffi as f;
    match matrix {
        None | Some(MatrixCoefficients::Bt709) => Some(f::SWS_CS_ITU709),
        Some(MatrixCoefficients::Bt470Bg | MatrixCoefficients::Smpte170M) => Some(f::SWS_CS_ITU601),
        Some(MatrixCoefficients::Smpte240M) => Some(f::SWS_CS_SMPTE240M),
        Some(MatrixCoefficients::Fcc) => Some(f::SWS_CS_FCC),
        Some(MatrixCoefficients::Bt2020Ncl) => Some(f::SWS_CS_BT2020),
        _ => None,
    }
}

/// The FFmpeg color properties for `tags`; unset tags (and names without a code here) are
/// unspecified.
fn ffmpeg_color(
    tags: &ColorTags,
) -> (
    color::Primaries,
    color::TransferCharacteristic,
    color::Space,
    color::Range,
) {
    use color::{Primaries as P, Space as S, TransferCharacteristic as T};
    let primaries = match tags.primaries {
        Some(ColorPrimaries::Bt709) => P::BT709,
        Some(ColorPrimaries::Bt470M) => P::BT470M,
        Some(ColorPrimaries::Bt470Bg) => P::BT470BG,
        Some(ColorPrimaries::Smpte170M) => P::SMPTE170M,
        Some(ColorPrimaries::Smpte240M) => P::SMPTE240M,
        Some(ColorPrimaries::Film) => P::Film,
        Some(ColorPrimaries::Bt2020) => P::BT2020,
        Some(ColorPrimaries::Smpte428) => P::SMPTE428,
        Some(ColorPrimaries::Smpte431) => P::SMPTE431,
        Some(ColorPrimaries::Smpte432) => P::SMPTE432,
        Some(ColorPrimaries::Ebu3213) => P::EBU3213,
        _ => P::Unspecified,
    };
    let transfer = match tags.transfer {
        Some(TransferCharacteristics::Bt709) => T::BT709,
        Some(TransferCharacteristics::Gamma22) => T::GAMMA22,
        Some(TransferCharacteristics::Gamma28) => T::GAMMA28,
        Some(TransferCharacteristics::Smpte170M) => T::SMPTE170M,
        Some(TransferCharacteristics::Smpte240M) => T::SMPTE240M,
        Some(TransferCharacteristics::Linear) => T::Linear,
        Some(TransferCharacteristics::Srgb) => T::IEC61966_2_1,
        Some(TransferCharacteristics::Bt2020_10) => T::BT2020_10,
        Some(TransferCharacteristics::Bt2020_12) => T::BT2020_12,
        Some(TransferCharacteristics::Pq) => T::SMPTE2084,
        Some(TransferCharacteristics::Hlg) => T::ARIB_STD_B67,
        _ => T::Unspecified,
    };
    let matrix = match tags.matrix {
        Some(MatrixCoefficients::Rgb) => S::RGB,
        Some(MatrixCoefficients::Bt709) => S::BT709,
        Some(MatrixCoefficients::Fcc) => S::FCC,
        Some(MatrixCoefficients::Bt470Bg) => S::BT470BG,
        Some(MatrixCoefficients::Smpte170M) => S::SMPTE170M,
        Some(MatrixCoefficients::Smpte240M) => S::SMPTE240M,
        Some(MatrixCoefficients::YCgCo) => S::YCGCO,
        Some(MatrixCoefficients::Bt2020Ncl) => S::BT2020NCL,
        Some(MatrixCoefficients::Bt2020Cl) => S::BT2020CL,
        Some(MatrixCoefficients::ICtCp) => S::ICTCP,
        _ => S::Unspecified,
    };
    let range = match tags.range {
        Some(ColorRange::Limited) => color::Range::MPEG,
        Some(ColorRange::Full) => color::Range::JPEG,
        None => color::Range::Unspecified,
    };
    (primaries, transfer, matrix, range)
}

/// The checked audio settings, before anything is opened.
struct AudioPlan {
    spec: AudioEncodeSpec,
    codec: ffmpeg_next::Codec,
    /// The encoder's name (`aac`, `pcm_s16le`).
    name: &'static str,
    rate: i32,
}

/// The layout of a stream of `channels` channels: mono, stereo, else FFmpeg's default order.
fn layout(channels: u16) -> ChannelLayout {
    match channels {
        1 => ChannelLayout::MONO,
        2 => ChannelLayout::STEREO,
        n => ChannelLayout::default(i32::from(n)),
    }
}

impl AudioPlan {
    fn new(spec: &AudioEncodeSpec, container: &'static str) -> Result<AudioPlan, MediaError> {
        let (name, encoder_name) = match (spec.codec.as_str(), container) {
            (AAC, _) => (AAC, "aac"),
            (MP3, _) => (MP3, "libmp3lame"),
            (OPUS, MP4) => (OPUS, "libopus"),
            (FLAC, MP4) => (FLAC, "flac"),
            (PCM_S16LE, MOV) => (PCM_S16LE, PCM_S16LE),
            (PCM_S24LE, MOV) => (PCM_S24LE, PCM_S24LE),
            _ => {
                return Err(unsupported(format!(
                    "no `{}` encoder for {}: this backend writes AAC and MP3 (MP4 and MOV), \
                     Opus and FLAC (MP4) and 16 or 24-bit PCM (MOV)",
                    spec.codec,
                    container.to_ascii_uppercase()
                )));
            }
        };
        let codec = encoder::find_by_name(encoder_name).ok_or_else(|| {
            unsupported(format!("this FFmpeg build has no {encoder_name} encoder"))
        })?;
        let channels_ok = if matches!(name, AAC | MP3 | OPUS) {
            matches!(spec.channels, 1 | 2)
        } else {
            (1..=8).contains(&spec.channels)
        };
        if !channels_ok {
            return Err(unsupported(format!(
                "{} audio channels: {name} here takes {}",
                spec.channels,
                if matches!(name, AAC | MP3 | OPUS) {
                    "mono or stereo"
                } else {
                    "1 to 8 channels"
                }
            )));
        }
        let hz = spec.rate.hz();
        let rate = i32::try_from(hz).map_err(|_| unsupported(format!("{hz} Hz")))?;
        let supported = codec
            .audio()
            .ok()
            .and_then(|a| a.rates())
            .is_none_or(|mut rates| rates.any(|r| r == rate));
        if !supported {
            return Err(unsupported(format!("{name} at {}", spec.rate)));
        }
        Ok(AudioPlan {
            spec: spec.clone(),
            codec,
            name,
            rate,
        })
    }
}

/// The partial output file: removed on drop unless [`commit`](Self::commit)ted.
struct PartialFile {
    path: PathBuf,
    /// Set once the file became the output.
    committed: bool,
}

impl PartialFile {
    /// Creates `.<name>.<pid>-<n>.partial` next to `target`.
    fn create(target: &Path) -> Result<(PartialFile, File), MediaError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = target
            .file_name()
            .map_or_else(|| "output".into(), |n| n.to_string_lossy());
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = target.with_file_name(format!(".{name}.{}-{n}.partial", std::process::id()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| MediaError::Io {
                path: display(target),
                message: format!("cannot create `{}`: {e}", display(&path)),
            })?;
        Ok((
            PartialFile {
                path,
                committed: false,
            },
            file,
        ))
    }

    /// Renames the file over `target` (replacing a file there). On failure the partial file is
    /// still removed on drop.
    fn commit(mut self, target: &Path) -> io::Result<()> {
        fs::rename(&self.path, target)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Err(e) = fs::remove_file(&self.path)
            && e.kind() != io::ErrorKind::NotFound
        {
            let path = display(&self.path);
            tracing::warn!(
                target: "cutweave_media",
                "could not remove the partial output `{path}`: {e}"
            );
        }
    }
}

/// What the writer shares with the encoder.
#[derive(Debug, Default)]
struct SinkState {
    /// The first I/O error, as the OS reported it.
    error: Mutex<Option<String>>,
    /// Bytes in the file so far (the furthest position written).
    size: AtomicU64,
}

impl SinkState {
    fn record(&self, e: &io::Error) {
        let mut slot = self.error.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(e.to_string());
        }
    }

    fn error(&self) -> Option<String> {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The custom AVIO target: the partial file, counting bytes, keeping errors and failing every
/// write once cancelled.
struct Sink {
    inner: Box<dyn WriteSeek>,
    pos: u64,
    state: Arc<SinkState>,
    cancel: Option<CancelCheck>,
}

impl Sink {
    fn kept<T>(&self, r: io::Result<T>) -> io::Result<T> {
        if let Err(e) = &r
            && e.kind() != io::ErrorKind::Interrupted
        {
            self.state.record(e);
        }
        r
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.cancel.as_ref().is_some_and(CancelCheck::is_cancelled) {
            // Not recorded: the encoder reports the cancel itself.
            return Err(io::Error::other("cancelled"));
        }
        let r = self.inner.write(buf);
        let n = self.kept(r)?;
        self.pos += n as u64;
        self.state.size.fetch_max(self.pos, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        let r = self.inner.flush();
        self.kept(r)
    }
}

impl Seek for Sink {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let r = self.inner.seek(to);
        self.pos = self.kept(r)?;
        Ok(self.pos)
    }
}

/// Why an FFmpeg step failed, before it becomes a [`MediaError`].
#[derive(Debug)]
enum Fault {
    /// Opening or running an encoder.
    Encode(&'static str, ffmpeg_next::Error),
    /// A hardware video encoder failed (opening, uploading, encoding or flushing; the message
    /// starts with its name): becomes [`MediaError::HwUnavailable`], which `Auto` answers with
    /// the next encoder or a software re-encode.
    Hardware(String),
    /// The muxer or the writer.
    Write(ffmpeg_next::Error),
    /// The cancel check answered `true`.
    Cancelled,
    /// Anything already typed.
    Media(MediaError),
}

impl Fault {
    /// The failure as text (the probe's reason).
    fn text(self) -> String {
        match self {
            Fault::Hardware(message) => message,
            Fault::Encode(which, e) => format!("{which}: {e}"),
            Fault::Write(e) => e.to_string(),
            Fault::Cancelled => "cancelled".to_owned(),
            Fault::Media(e) => e.to_string(),
        }
    }
}

/// How the input reaches the encoder's layout (`yuv420p` for x264, NV12 for hardware).
enum Convert {
    /// Already in that layout: copy the planes.
    Copy,
    /// swscale from the staging frame's layout.
    Scale {
        scaler: Scaler,
        staging: frame::Video,
    },
}

/// The FFmpeg layout of an input format.
pub(super) fn pixel(format: PixelFormat) -> Pixel {
    match format {
        PixelFormat::Rgba8 => Pixel::RGBA,
        PixelFormat::Nv12 => Pixel::NV12,
        PixelFormat::P010 => Pixel::P010LE,
        PixelFormat::Yuv420p10 => Pixel::YUV420P10LE,
        PixelFormat::Yuv422p => Pixel::YUV422P,
        PixelFormat::Yuv422p10 => Pixel::YUV422P10LE,
        PixelFormat::Yuv420p => Pixel::YUV420P,
    }
}

impl Convert {
    /// From `input` frames to `output` frames (RGBA in; 4:2:0, 4:2:2 or 4:4:4 out, 8 or 10 bit)
    /// at `size`.
    #[cfg(test)]
    fn new(
        input: PixelFormat,
        output: Pixel,
        size: (u32, u32),
    ) -> Result<Convert, ffmpeg_next::Error> {
        Convert::with(input, output, size, SWS_CS_ITU709, false)
    }

    /// Like [`new`](Self::new) with the output's swscale `matrix` and range (`full`).
    fn with(
        input: PixelFormat,
        output: Pixel,
        size: (u32, u32),
        matrix: std::ffi::c_int,
        full: bool,
    ) -> Result<Convert, ffmpeg_next::Error> {
        let src = pixel(input);
        if src == output {
            return Ok(Convert::Copy);
        }
        // RGB input is full range; YUV input is already limited range like the output.
        let src_full = input == PixelFormat::Rgba8;
        Ok(Convert::Scale {
            scaler: Scaler::new(src, output, size, matrix, (src_full, full))?,
            staging: frame::Video::new(src, size.0, size.1),
        })
    }

    /// `frame` as a new `output` frame (new, because the encoder may keep a reference).
    fn run(&mut self, frame: &VideoFrame, output: Pixel) -> Result<frame::Video, Fault> {
        let mut out = frame::Video::new(output, frame.width, frame.height);
        match self {
            Convert::Copy => copy_planes(frame, &mut out)?,
            Convert::Scale { scaler, staging } => {
                copy_planes(frame, staging)?;
                scaler
                    .run(staging, &mut out)
                    .map_err(|e| Fault::Encode("swscale", e))?;
            }
        }
        Ok(out)
    }
}

/// Copies the planes of `frame` (validated, same layout) into `dst`, row by row.
fn copy_planes(frame: &VideoFrame, dst: &mut frame::Video) -> Result<(), Fault> {
    let short = || {
        Fault::Media(MediaError::Backend {
            message: "an FFmpeg frame is smaller than its layout".to_owned(),
        })
    };
    for i in 0..frame.format.plane_count() {
        let (_, height, _) = frame.format.plane_layout(i, frame.width, frame.height);
        let stride = dst.stride(i);
        let data = dst.data_mut(i);
        for y in 0..height {
            let row = frame.row(i, y).ok_or_else(short)?;
            let at = y as usize * stride;
            data.get_mut(at..at + row.len())
                .ok_or_else(short)?
                .copy_from_slice(row);
        }
    }
    Ok(())
}

/// One open video encoder, without a muxer: input conversion, the upload to GPU surfaces for
/// encoders that take only those, and the FFmpeg encoder. The muxed stream ([`VideoOut`]) and
/// the hardware probe ([`test_encode`]) both use it.
struct VideoEncoder {
    encoder: encoder::video::Encoder,
    codec: ffmpeg_next::Codec,
    convert: Convert,
    /// The layout the encoder takes (before any GPU upload).
    pixel: Pixel,
    /// The pool frames are uploaded to (VA-API).
    surfaces: Option<HwFrames>,
    choice: Choice,
}

/// The faults a [`hwenc::Family::Pseudo`] encoder injects (tests).
#[cfg(test)]
fn injected(choice: Choice) -> Option<hwenc::Faults> {
    if let hwenc::Family::Pseudo(faults) = choice.hardware()?.family {
        Some(faults)
    } else {
        None
    }
}

impl VideoEncoder {
    /// Opens `choice` for `plan`.
    fn open(plan: &VideoPlan, choice: Choice, global_header: bool) -> Result<VideoEncoder, Fault> {
        let spec = &plan.spec;
        let name = choice.name();
        let fault = |message: String| match choice.hardware() {
            Some(_) => Fault::Hardware(message),
            None => Fault::Media(unsupported(message)),
        };
        #[cfg(test)]
        if injected(choice).is_some_and(|f| f.open) {
            return Err(fault(format!("{name} could not open (injected)")));
        }
        if spec.bit_depth > 8
            && choice
                .hardware()
                .is_some_and(|hw| matches!(hw.family, hwenc::Family::Vaapi))
        {
            return Err(fault(format!("{name}: 10-bit input is not supported here")));
        }
        if choice.hardware().is_some() {
            let adv = &spec.advanced;
            let needs_software = adv.closed_gop.is_some()
                || adv.scenecut.is_some()
                || adv.keyint_min.is_some()
                || adv.tune.is_some()
                || adv.level.is_some();
            if needs_software {
                return Err(fault(format!(
                    "{name}: closed_gop, scenecut, keyint_min, tune and level need a software \
                     encoder"
                )));
            }
        }
        let codec = encoder::find_by_name(choice.ffmpeg_name())
            .ok_or_else(|| fault(format!("{name} is not in this FFmpeg build")))?;
        let mut ctx = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .map_err(|e| fault(format!("{name}: {e}")))?;
        ctx.set_width(spec.width);
        ctx.set_height(spec.height);
        ctx.set_time_base(plan.time_base);
        ctx.set_frame_rate(Some(plan.fps));
        ctx.set_aspect_ratio(Rational::new(1, 1));
        // H.264 exports are BT.709 limited range (checked by the plan); a proxy keeps the
        // source's tags as they are.
        let tags = if plan.intra.is_some() {
            spec.color.clone()
        } else {
            output_color(&spec.color)
        };
        let (primaries, transfer, space, range) = ffmpeg_color(&tags);
        ctx.set_colorspace(space);
        ctx.set_color_range(range);
        ctx.set_color_primaries(primaries);
        ctx.set_color_transfer_characteristic(transfer);
        if global_header {
            ctx.set_flags(codec::Flags::GLOBAL_HEADER);
        }
        let surfaces = if let Some((device, format)) = choice.surfaces() {
            let pool = HwFrames::new(device, format, (spec.width, spec.height))
                .map_err(|e| fault(format!("{name}: no {device:?} surfaces: {e}")))?;
            pool.attach(&mut ctx)
                .map_err(|e| fault(format!("{name}: {e}")))?;
            ctx.set_format(format);
            Some(pool)
        } else {
            ctx.set_format(choice.pixel(spec));
            None
        };
        let mut options = Dictionary::new();
        for (key, value) in choice.settings(spec) {
            options.set(key, &value);
        }
        let encoder = ctx.open_with(options).map_err(|e| {
            fault(format!(
                "{name} refused {}x{} at {} fps: {e}",
                spec.width, spec.height, spec.rate
            ))
        })?;
        let pixel = choice.pixel(spec);
        // RGB input is converted with the output's matrix and range unless the tags are only
        // labels (and always for a proxy, whose tags are the source's).
        let (matrix, full) = if plan.intra.is_some() || spec.advanced.tag_only {
            (SWS_CS_ITU709, false)
        } else {
            (
                sws_matrix(tags.matrix.as_ref()).unwrap_or(SWS_CS_ITU709),
                tags.range == Some(ColorRange::Full),
            )
        };
        let convert = Convert::with(
            spec.input_format,
            pixel,
            (spec.width, spec.height),
            matrix,
            full,
        )
        .map_err(|e| Fault::Encode("swscale", e))?;
        Ok(VideoEncoder {
            encoder,
            codec,
            convert,
            pixel,
            surfaces,
            choice,
        })
    }

    /// What an encoder call failing with `e` means.
    fn fault(&self, e: ffmpeg_next::Error) -> Fault {
        match self.choice.hardware() {
            Some(hw) => Fault::Hardware(format!("{}: {e}", hw.name)),
            None => Fault::Encode(self.choice.name(), e),
        }
    }

    /// Converts (and uploads) `frame` and sends it as frame `index`.
    fn send(&mut self, frame: &VideoFrame, index: i64) -> Result<(), Fault> {
        #[cfg(test)]
        if injected(self.choice).is_some_and(|f| f.frame == Some(index)) {
            let name = self.choice.name();
            return Err(Fault::Hardware(format!(
                "{name}: device lost at frame {index} (injected)"
            )));
        }
        let mut input = self.convert.run(frame, self.pixel)?;
        if let Some(pool) = &self.surfaces {
            input = pool.upload(&input).map_err(|e| self.fault(e))?;
        }
        input.set_pts(Some(index));
        self.encoder.send_frame(&input).map_err(|e| self.fault(e))
    }

    /// Signals the end of the stream.
    fn send_eof(&mut self) -> Result<(), Fault> {
        #[cfg(test)]
        if injected(self.choice).is_some_and(|f| f.finish) {
            let name = self.choice.name();
            return Err(Fault::Hardware(format!("{name}: flush failed (injected)")));
        }
        self.encoder.send_eof().map_err(|e| self.fault(e))
    }

    /// The next packet into `packet`: `false` when none is ready (or at the end).
    fn receive(&mut self, packet: &mut Packet) -> Result<bool, Fault> {
        receive(&mut self.encoder, packet).map_err(|e| self.fault(e))
    }
}

/// `receive_packet`: `Ok(false)` when the encoder needs input or is drained.
fn receive(enc: &mut encoder::Encoder, packet: &mut Packet) -> Result<bool, ffmpeg_next::Error> {
    match enc.receive_packet(packet) {
        Ok(()) => Ok(true),
        Err(ffmpeg_next::Error::Other { errno }) if errno == ffmpeg_next::error::EAGAIN => {
            Ok(false)
        }
        Err(ffmpeg_next::Error::Eof) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Encodes `frames` frames of a moving NV12 ramp at `spec` with `choice`, without a file, and
/// expects at least one packet: the hardware probe. The error is the reason, for the log.
pub(super) fn test_encode(
    spec: &VideoEncodeSpec,
    choice: Choice,
    frames: i64,
) -> Result<(), String> {
    let plan = VideoPlan::new(spec).map_err(|e| e.to_string())?;
    let mut enc = VideoEncoder::open(&plan, choice, false).map_err(Fault::text)?;
    let mut packet = Packet::empty();
    let mut packets = 0_usize;
    for n in 0..frames {
        let frame = ramp(spec, n).map_err(|e| e.to_string())?;
        enc.send(&frame, n).map_err(Fault::text)?;
        while enc.receive(&mut packet).map_err(Fault::text)? {
            packets += 1;
        }
    }
    enc.send_eof().map_err(Fault::text)?;
    while enc.receive(&mut packet).map_err(Fault::text)? {
        packets += 1;
    }
    if packets == 0 {
        return Err(format!("no packet for {frames} frames"));
    }
    Ok(())
}

/// Frame `n` for `spec` (NV12): a diagonal luma ramp moving with `n`, neutral chroma.
fn ramp(spec: &VideoEncodeSpec, n: i64) -> Result<VideoFrame, MediaError> {
    let (w, h) = (spec.width as usize, spec.height as usize);
    let shift = usize::try_from(n).unwrap_or(0) * 8;
    #[allow(clippy::cast_possible_truncation)] // `% 220` keeps it below 256
    let luma = (0..w * h)
        .map(|i| 16 + ((i % w + i / w + shift) % 220) as u8)
        .collect();
    Ok(VideoFrame {
        pts: spec
            .rate
            .frame_to_time(n)
            .map_err(|e| unsupported(e.to_string()))?,
        width: spec.width,
        height: spec.height,
        format: PixelFormat::Nv12,
        planes: vec![
            crate::Plane {
                data: luma,
                stride: spec.width,
            },
            crate::Plane {
                data: vec![128; w * h / 2],
                stride: spec.width,
            },
        ],
        color: cutweave_model::ColorTags::default(),
        damaged: false,
    })
}

/// The open video stream.
struct VideoOut {
    encoder: VideoEncoder,
    stream: usize,
    time_base: Rational,
    stream_time_base: Rational,
    rate: FrameRate,
}

/// The open audio stream.
struct AudioOut {
    encoder: encoder::audio::Encoder,
    /// The encoder's name (`aac`, `pcm_s16le`).
    name: &'static str,
    stream: usize,
    time_base: Rational,
    stream_time_base: Rational,
    rate: SampleRate,
    /// Channel count (`ChannelLayout` itself is not `Send`).
    channels: u16,
    /// Samples per encoder frame.
    frame_size: usize,
    /// Whether the last frame may be shorter (else it is padded with silence).
    small_last_frame: bool,
    /// Samples pushed but not yet sent, per channel.
    pending: Vec<Vec<f32>>,
    /// Samples sent to the encoder (the pts of the next frame).
    sent: i64,
}

impl AudioOut {
    /// Sends the first `n` pending samples as one frame of `len ≥ n` samples (zero-padded):
    /// planar f32 for AAC, interleaved 16-bit for PCM.
    fn send(&mut self, n: usize, len: usize) -> Result<(), Fault> {
        let mut f = if self.name == OPUS {
            // libopus takes interleaved 32-bit float.
            let mut f = frame::Audio::new(
                Sample::F32(sample::Type::Packed),
                len,
                layout(self.channels),
            );
            let channels = self.pending.len();
            let data = f.plane_mut::<f32>(0);
            data.fill(0.0);
            for (c, pending) in self.pending.iter().enumerate() {
                for (i, v) in pending[..n].iter().enumerate() {
                    if let Some(slot) = data.get_mut(i * channels + c) {
                        *slot = *v;
                    }
                }
            }
            f
        } else if self.name == PCM_S16LE {
            let mut f = frame::Audio::new(
                Sample::I16(sample::Type::Packed),
                len,
                layout(self.channels),
            );
            let channels = self.pending.len();
            let data = f.plane_mut::<i16>(0);
            data.fill(0);
            for (c, pending) in self.pending.iter().enumerate() {
                for (i, v) in pending[..n].iter().enumerate() {
                    #[allow(clippy::cast_possible_truncation)] // clamped to the i16 range first
                    let q = (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                    if let Some(slot) = data.get_mut(i * channels + c) {
                        *slot = q;
                    }
                }
            }
            f
        } else if self.name == PCM_S24LE || self.name == FLAC {
            // `pcm_s24le` takes 32-bit samples and writes their top 24 bits; FLAC does the same
            // from 32-bit input (24-bit FLAC).
            let mut f = frame::Audio::new(
                Sample::I32(sample::Type::Packed),
                len,
                layout(self.channels),
            );
            let channels = self.pending.len();
            let data = f.plane_mut::<i32>(0);
            data.fill(0);
            for (c, pending) in self.pending.iter().enumerate() {
                for (i, v) in pending[..n].iter().enumerate() {
                    #[allow(clippy::cast_possible_truncation)] // clamped to the i32 range first
                    let q = (f64::from(v.clamp(-1.0, 1.0)) * f64::from(i32::MAX)).round() as i32;
                    if let Some(slot) = data.get_mut(i * channels + c) {
                        *slot = q;
                    }
                }
            }
            f
        } else {
            let mut f = frame::Audio::new(
                Sample::F32(sample::Type::Planar),
                len,
                layout(self.channels),
            );
            for (c, pending) in self.pending.iter().enumerate() {
                let plane = f.plane_mut::<f32>(c);
                plane[..n].copy_from_slice(&pending[..n]);
                plane[n..].fill(0.0);
            }
            f
        };
        for pending in &mut self.pending {
            pending.drain(..n);
        }
        f.set_rate(self.rate.hz());
        f.set_pts(Some(self.sent));
        self.sent += i64::try_from(n).unwrap_or(i64::MAX);
        let name = self.name;
        self.encoder
            .send_frame(&f)
            .map_err(|e| Fault::Encode(name, e))
    }
}

/// The FFmpeg encoder: H.264 + AAC in MP4, or an intra proxy codec + PCM in MOV (see the module
/// docs).
struct FfmpegEncoder {
    // Field order matters: `out` (which owns the writer and so the open file) must close before
    // `partial` removes the file.
    out: Option<Output>,
    video: Option<VideoOut>,
    audio: Option<AudioOut>,
    partial: Option<PartialFile>,
    /// The target.
    path: PathBuf,
    /// The video encoder's name (`libx264`, `h264_nvenc`).
    video_encoder: Option<&'static str>,
    input: InputCheck,
    cancel: Option<CancelCheck>,
    progress: Option<ProgressSink>,
    io: Arc<SinkState>,
    /// Output media time the encoders have produced.
    encoded: Duration,
    /// The error that ended this encode; every later call returns it.
    failed: Option<MediaError>,
}

impl FfmpegEncoder {
    /// Creates the muxer on `sink`, opens the encoders and writes the header.
    fn start(
        &mut self,
        sink: Sink,
        container: &'static str,
        video: Option<(&VideoPlan, Choice)>,
        audio: Option<&AudioPlan>,
    ) -> Result<(), Fault> {
        let io = StreamIo::from_write_seek(sink).map_err(Fault::Write)?;
        let url = self
            .partial
            .as_ref()
            .map_or_else(String::new, |p| display(&p.path));
        // The URL is the partial file's own path: `+faststart` re-opens it to move the index.
        let mut out =
            format::output_to_stream(io, Some(&url), Some(container)).map_err(Fault::Write)?;
        let global_header = out.format().flags().contains(format::Flags::GLOBAL_HEADER);
        let mut video = video
            .map(|(plan, choice)| open_video(&mut out, plan, choice, global_header))
            .transpose()?;
        let mut audio = audio
            .map(|plan| open_audio(&mut out, plan, global_header))
            .transpose()?;
        let mut options = Dictionary::new();
        options.set("movflags", "+faststart");
        let rest = out.write_header_with(options).map_err(Fault::Write)?;
        if rest.get("movflags").is_some() {
            return Err(Fault::Media(MediaError::Backend {
                message: format!("the {container} muxer ignored `movflags`"),
            }));
        }
        drop(rest);
        // The muxer picks the stream time bases while writing the header.
        if let Some(v) = &mut video {
            v.stream_time_base = stream_time_base(&out, v.stream)?;
        }
        if let Some(a) = &mut audio {
            a.stream_time_base = stream_time_base(&out, a.stream)?;
        }
        self.out = Some(out);
        self.video = video;
        self.audio = audio;
        Ok(())
    }

    fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(CancelCheck::is_cancelled)
    }

    /// The error a fault means: a cancel wins; then an I/O error the writer saw.
    fn error(&self, fault: Fault) -> MediaError {
        if self.cancelled() || matches!(fault, Fault::Cancelled) {
            return MediaError::Cancelled;
        }
        if let Some(message) = self.io.error() {
            return MediaError::Io {
                path: display(&self.path),
                message: format!("writing the output failed: {message}"),
            };
        }
        match fault {
            Fault::Encode(which, e) => MediaError::Backend {
                message: format!("{which}: {e}"),
            },
            Fault::Hardware(reason) => MediaError::HwUnavailable { reason },
            Fault::Write(e) => MediaError::Backend {
                message: format!("writing `{}`: {e}", display(&self.path)),
            },
            Fault::Media(e) => e,
            Fault::Cancelled => MediaError::Cancelled,
        }
    }

    /// Ends the encode: closes and removes the partial output, and remembers the error.
    fn fail(&mut self, fault: Fault) -> MediaError {
        let err = self.error(fault);
        self.out = None;
        self.video = None;
        self.audio = None;
        self.partial = None;
        self.failed = Some(err.clone());
        err
    }

    /// `Err` once the encode failed or was cancelled.
    fn ready(&mut self) -> Result<(), MediaError> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        if self.cancelled() {
            return Err(self.fail(Fault::Cancelled));
        }
        Ok(())
    }

    fn report(&self) {
        if let Some(sink) = &self.progress {
            sink.report(
                &self
                    .input
                    .progress(self.encoded, self.io.size.load(Ordering::Relaxed)),
            );
        }
    }

    fn encode_video(&mut self, frame: &VideoFrame, index: i64) -> Result<(), Fault> {
        let v = self.video.as_mut().ok_or(Fault::Cancelled)?;
        v.encoder.send(frame, index)?;
        self.drain_video()
    }

    /// [`fail`](Self::fail) after the encoder opened, logging a hardware encoder's failure
    /// (the caller then re-encodes in software, or reports it).
    fn fail_midway(&mut self, fault: Fault) -> MediaError {
        let err = self.fail(fault);
        if let MediaError::HwUnavailable { reason } = &err {
            let (path, frames) = (display(&self.path), self.input.summary().video_frames);
            tracing::warn!(
                target: "cutweave_media",
                "the hardware video encoder failed after {frames} frames of `{path}`: {reason}"
            );
        }
        err
    }

    fn encode_audio(&mut self, chunk: &AudioChunk) -> Result<(), Fault> {
        let a = self.audio.as_mut().ok_or(Fault::Cancelled)?;
        for (pending, plane) in a.pending.iter_mut().zip(&chunk.planes) {
            pending.extend_from_slice(plane);
        }
        loop {
            let a = self.audio.as_mut().ok_or(Fault::Cancelled)?;
            if a.pending.first().map_or(0, Vec::len) < a.frame_size {
                return Ok(());
            }
            a.send(a.frame_size, a.frame_size)?;
            self.drain_audio()?;
        }
    }

    fn drain_video(&mut self) -> Result<(), Fault> {
        let (Some(v), Some(out)) = (self.video.as_mut(), self.out.as_mut()) else {
            return Err(Fault::Cancelled);
        };
        let end = drain(
            |p| v.encoder.receive(p),
            out,
            (v.stream, v.time_base, v.stream_time_base),
            1,
            self.cancel.as_ref(),
        )?;
        if let Some(t) = end.and_then(|n| v.rate.frame_to_time(n).ok()) {
            self.encoded = self
                .encoded
                .max(Duration::try_from(t).unwrap_or(Duration::ZERO));
        }
        Ok(())
    }

    fn drain_audio(&mut self) -> Result<(), Fault> {
        let (Some(a), Some(out)) = (self.audio.as_mut(), self.out.as_mut()) else {
            return Err(Fault::Cancelled);
        };
        let name = a.name;
        let end = drain(
            |p| receive(&mut a.encoder, p).map_err(|e| Fault::Encode(name, e)),
            out,
            (a.stream, a.time_base, a.stream_time_base),
            0,
            self.cancel.as_ref(),
        )?;
        if let Some(t) = end.and_then(|n| a.rate.samples_to_time(n.max(0)).ok()) {
            self.encoded = self
                .encoded
                .max(Duration::try_from(t).unwrap_or(Duration::ZERO));
        }
        Ok(())
    }

    /// Sends what is pending, drains both encoders and writes the trailer (which moves the index
    /// to the front).
    fn flush(&mut self) -> Result<(), Fault> {
        if let Some(a) = self.audio.as_mut() {
            let n = a.pending.first().map_or(0, Vec::len);
            if n > 0 {
                let len = if a.small_last_frame { n } else { a.frame_size };
                a.send(n, len)?;
            }
            let name = a.name;
            a.encoder.send_eof().map_err(|e| Fault::Encode(name, e))?;
            self.drain_audio()?;
            self.report();
        }
        if let Some(v) = self.video.as_mut() {
            v.encoder.send_eof()?;
            self.drain_video()?;
            self.report();
        }
        if self.cancelled() {
            return Err(Fault::Cancelled);
        }
        let out = self.out.as_mut().ok_or(Fault::Cancelled)?;
        out.write_trailer().map_err(Fault::Write)
    }
}

/// Moves every packet `receive` has ready into the muxer, giving packets without a duration
/// `default_duration` (one frame for video: CFR). Returns the end of the latest packet in the
/// codec time base.
fn drain(
    mut receive: impl FnMut(&mut Packet) -> Result<bool, Fault>,
    out: &mut Output,
    (stream, time_base, stream_time_base): (usize, Rational, Rational),
    default_duration: i64,
    cancel: Option<&CancelCheck>,
) -> Result<Option<i64>, Fault> {
    let mut packet = Packet::empty();
    let mut end = None;
    loop {
        if !receive(&mut packet)? {
            return Ok(end);
        }
        if packet.duration() == 0 {
            packet.set_duration(default_duration);
        }
        if let Some(pts) = packet.pts() {
            let e = pts.saturating_add(packet.duration());
            end = Some(end.map_or(e, |m: i64| m.max(e)));
        }
        packet.set_stream(stream);
        packet.rescale_ts(time_base, stream_time_base);
        packet.write_interleaved(out).map_err(Fault::Write)?;
        if cancel.is_some_and(CancelCheck::is_cancelled) {
            return Err(Fault::Cancelled);
        }
    }
}

fn stream_time_base(out: &Output, index: usize) -> Result<Rational, Fault> {
    out.stream(index).map(|s| s.time_base()).ok_or_else(|| {
        Fault::Media(MediaError::Backend {
            message: format!("the muxer lost stream {index}"),
        })
    })
}

fn open_video(
    out: &mut Output,
    plan: &VideoPlan,
    choice: Choice,
    global_header: bool,
) -> Result<VideoOut, Fault> {
    let encoder = VideoEncoder::open(plan, choice, global_header)?;
    let mut stream = out.add_stream(encoder.codec).map_err(Fault::Write)?;
    stream.set_parameters(&encoder.encoder);
    if plan.intra.is_none() && plan.codec == Codec::Hevc {
        // MP4/MOV default to `hev1`, which Apple's players refuse; `hvc1` plays everywhere.
        super::sys::set_stream_codec_tag(&mut stream, *b"hvc1");
    }
    stream.set_time_base(plan.time_base);
    stream.set_rate(plan.fps);
    stream.set_avg_frame_rate(plan.fps);
    let index = stream.index();
    Ok(VideoOut {
        encoder,
        stream: index,
        time_base: plan.time_base,
        stream_time_base: plan.time_base,
        rate: plan.spec.rate,
    })
}

fn open_audio(out: &mut Output, plan: &AudioPlan, global_header: bool) -> Result<AudioOut, Fault> {
    let spec = &plan.spec;
    let name = plan.name;
    let mut ctx = codec::context::Context::new_with_codec(plan.codec)
        .encoder()
        .audio()
        .map_err(|e| Fault::Encode(name, e))?;
    let time_base = Rational::new(1, plan.rate);
    ctx.set_rate(plan.rate);
    ctx.set_channel_layout(layout(spec.channels));
    if name == PCM_S16LE {
        ctx.set_format(Sample::I16(sample::Type::Packed));
    } else if name == PCM_S24LE || name == FLAC {
        ctx.set_format(Sample::I32(sample::Type::Packed));
    } else if name == OPUS {
        ctx.set_format(Sample::F32(sample::Type::Packed));
        ctx.set_bit_rate(spec.bitrate as usize);
    } else {
        ctx.set_format(Sample::F32(sample::Type::Planar));
        ctx.set_bit_rate(spec.bitrate as usize);
    }
    ctx.set_time_base(time_base);
    if global_header {
        ctx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let encoder = ctx.open_with(Dictionary::new()).map_err(|e| {
        Fault::Media(unsupported(format!(
            "{name} refused {} x{} at {} b/s: {e}",
            spec.rate, spec.channels, spec.bitrate
        )))
    })?;
    let mut stream = out.add_stream(plan.codec).map_err(Fault::Write)?;
    stream.set_parameters(&encoder);
    stream.set_time_base(time_base);
    let index = stream.index();
    let frame_size = match encoder.frame_size() {
        0 => 1024,
        n => n as usize,
    };
    let caps = plan.codec.capabilities();
    Ok(AudioOut {
        name,
        stream: index,
        time_base,
        stream_time_base: time_base,
        rate: spec.rate,
        channels: spec.channels,
        frame_size,
        small_last_frame: caps.intersects(
            codec::capabilities::Capabilities::SMALL_LAST_FRAME
                | codec::capabilities::Capabilities::VARIABLE_FRAME_SIZE,
        ),
        pending: vec![Vec::with_capacity(2 * frame_size); usize::from(spec.channels)],
        sent: 0,
        encoder,
    })
}

impl Encoder for FfmpegEncoder {
    fn push_video(&mut self, frame: &VideoFrame) -> Result<(), MediaError> {
        self.ready()?;
        let index = self.input.video(frame)?;
        if let Err(fault) = self.encode_video(frame, index) {
            return Err(self.fail_midway(fault));
        }
        self.input.add_video();
        self.report();
        Ok(())
    }

    fn push_audio(&mut self, chunk: &AudioChunk) -> Result<(), MediaError> {
        self.ready()?;
        self.input.audio(chunk)?;
        if let Err(fault) = self.encode_audio(chunk) {
            return Err(self.fail_midway(fault));
        }
        self.input.add_audio(chunk.frames());
        self.report();
        Ok(())
    }

    fn video_encoder(&self) -> Option<&str> {
        self.video_encoder
    }

    fn finish(mut self: Box<Self>) -> Result<EncodeSummary, MediaError> {
        self.ready()?;
        if let Err(fault) = self.flush() {
            return Err(self.fail_midway(fault));
        }
        // Close the file (the writer goes with the muxer) before renaming it.
        self.out = None;
        self.video = None;
        self.audio = None;
        if let Some(message) = self.io.error() {
            return Err(self.fail(Fault::Media(MediaError::Io {
                path: display(&self.path),
                message: format!("writing the output failed: {message}"),
            })));
        }
        if self.cancelled() {
            return Err(self.fail(Fault::Cancelled));
        }
        let partial = self.partial.take().ok_or(MediaError::Cancelled)?;
        if let Err(e) = partial.commit(&self.path) {
            return Err(self.fail(Fault::Media(MediaError::Io {
                path: display(&self.path),
                message: format!("cannot move the finished output into place: {e}"),
            })));
        }
        let summary = self.input.summary();
        self.encoded = summary.duration;
        self.report();
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use cutweave_model::{ColorTags, Time};

    use super::*;
    use crate::testing::contract::TempDir;
    use crate::{FfmpegBackend, MediaBackend, Plane};

    fn solid(format: PixelFormat, (w, h): (u32, u32), rgb: [u8; 3]) -> VideoFrame {
        let planes = match format {
            PixelFormat::Rgba8 => vec![Plane {
                data: [rgb[0], rgb[1], rgb[2], 255].repeat((w * h) as usize),
                stride: 4 * w,
            }],
            _ => unreachable!("test frames are RGBA"),
        };
        VideoFrame {
            pts: Time::ZERO,
            width: w,
            height: h,
            format,
            planes,
            color: ColorTags::default(),
            damaged: false,
        }
    }

    #[allow(clippy::cast_precision_loss)] // small test planes
    fn mean(plane: &[u8], stride: usize, (w, h): (usize, usize)) -> f64 {
        let sum: u64 = (0..h)
            .flat_map(|y| {
                plane[y * stride..y * stride + w]
                    .iter()
                    .map(|&v| u64::from(v))
            })
            .sum();
        sum as f64 / (w * h) as f64
    }

    #[test]
    fn rgba_becomes_bt709_limited_range_yuv() {
        FfmpegBackend::new().expect("backend");
        let size = (16, 8);
        // BT.709 limited range: Y = 16 + 219·Y', Cb/Cr = 128 + 224·(B'−Y')/1.8556, (R'−Y')/1.5748.
        // BT.601 would give red Y 81, green Y 145: the matrix is checked, not just the range.
        // Both the x264 layout (planar) and the hardware layout (NV12, interleaved chroma).
        for output in [PixelFormat::Yuv420p, PixelFormat::Nv12] {
            let mut convert =
                Convert::new(PixelFormat::Rgba8, pixel(output), size).expect("swscale");
            for (rgb, want) in [
                ([0, 0, 0], [16.0, 128.0, 128.0]),
                ([255, 255, 255], [235.0, 128.0, 128.0]),
                ([255, 0, 0], [62.6, 102.3, 240.0]),
                ([0, 255, 0], [172.6, 41.7, 26.3]),
                ([0, 0, 255], [31.8, 240.0, 117.7]),
            ] {
                let yuv = convert
                    .run(&solid(PixelFormat::Rgba8, size, rgb), pixel(output))
                    .expect("convert");
                assert_eq!(yuv.format(), pixel(output));
                let chroma = |c: usize| -> Vec<u8> {
                    match output {
                        PixelFormat::Nv12 => (0..4)
                            .flat_map(|r| {
                                yuv.data(1)[r * yuv.stride(1)..][..16]
                                    .iter()
                                    .skip(c)
                                    .step_by(2)
                                    .copied()
                                    .collect::<Vec<_>>()
                            })
                            .collect(),
                        _ => (0..4)
                            .flat_map(|r| yuv.data(1 + c)[r * yuv.stride(1 + c)..][..8].to_vec())
                            .collect(),
                    }
                };
                let got = [
                    mean(yuv.data(0), yuv.stride(0), (16, 8)),
                    mean(&chroma(0), 8, (8, 4)),
                    mean(&chroma(1), 8, (8, 4)),
                ];
                for (g, w) in got.iter().zip(want) {
                    assert!(
                        (g - w).abs() <= 1.0,
                        "{output:?} {rgb:?}: got {got:?}, want {want:?}"
                    );
                }
            }
        }
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)] // sample values stay below 256
    fn nv12_and_yuv420p_are_only_relaid_out() {
        FfmpegBackend::new().expect("backend");
        let (w, h) = (8_u32, 4_u32);
        let y: Vec<u8> = (0..w * h).map(|i| (16 + i * 5 % 200) as u8).collect();
        let cb: Vec<u8> = (0..(w / 2) * (h / 2)).map(|i| (60 + i * 7) as u8).collect();
        let cr: Vec<u8> = (0..(w / 2) * (h / 2))
            .map(|i| (200 - i * 3) as u8)
            .collect();
        let uv: Vec<u8> = cb.iter().zip(&cr).flat_map(|(&u, &v)| [u, v]).collect();
        let nv12 = VideoFrame {
            pts: Time::ZERO,
            width: w,
            height: h,
            format: PixelFormat::Nv12,
            planes: vec![
                Plane {
                    data: y.clone(),
                    stride: w,
                },
                Plane {
                    data: uv,
                    stride: w,
                },
            ],
            color: ColorTags::default(),
            damaged: false,
        };
        let planar = VideoFrame {
            format: PixelFormat::Yuv420p,
            planes: vec![
                Plane {
                    data: y.clone(),
                    stride: w,
                },
                Plane {
                    data: cb.clone(),
                    stride: w / 2,
                },
                Plane {
                    data: cr.clone(),
                    stride: w / 2,
                },
            ],
            ..nv12.clone()
        };
        let uv: Vec<u8> = cb.iter().zip(&cr).flat_map(|(&u, &v)| [u, v]).collect();
        for frame in [nv12, planar] {
            // To x264's planar layout.
            let mut convert = Convert::new(frame.format, Pixel::YUV420P, (w, h)).expect("swscale");
            let out = convert.run(&frame, Pixel::YUV420P).expect("convert");
            let plane = |out: &frame::Video, i: usize, (pw, ph): (u32, u32)| -> Vec<u8> {
                (0..ph as usize)
                    .flat_map(|r| out.data(i)[r * out.stride(i)..][..pw as usize].to_vec())
                    .collect()
            };
            assert_eq!(plane(&out, 0, (w, h)), y, "{:?} luma", frame.format);
            assert_eq!(plane(&out, 1, (w / 2, h / 2)), cb, "{:?} Cb", frame.format);
            assert_eq!(plane(&out, 2, (w / 2, h / 2)), cr, "{:?} Cr", frame.format);
            // To the hardware encoders' NV12.
            let mut convert = Convert::new(frame.format, Pixel::NV12, (w, h)).expect("swscale");
            let out = convert.run(&frame, Pixel::NV12).expect("convert");
            assert_eq!(out.format(), Pixel::NV12);
            assert_eq!(plane(&out, 0, (w, h)), y, "{:?} → NV12 luma", frame.format);
            assert_eq!(
                plane(&out, 1, (w, h / 2)),
                uv,
                "{:?} → NV12 CbCr",
                frame.format
            );
        }
    }

    /// The partial file behind a disk that is full after `budget` bytes; counts what it took.
    struct FullDisk {
        file: File,
        budget: u64,
        written: Arc<AtomicU64>,
    }

    impl Write for FullDisk {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let len = buf.len() as u64;
            if len > self.budget {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "no space left on device (simulated)",
                ));
            }
            let n = self.file.write(buf)?;
            self.budget -= n as u64;
            self.written.fetch_add(n as u64, Ordering::Relaxed);
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    impl Seek for FullDisk {
        fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
            self.file.seek(to)
        }
    }

    fn frame(n: u64, (w, h): (u32, u32)) -> VideoFrame {
        crate::testing::FakeVideo::new(w, h, FrameRate::FPS_30, n + 1)
            .frame(n, PixelFormat::Rgba8)
            .expect("fake frame")
    }

    const SIZE: (u32, u32) = (320, 180);
    const FRAMES: u64 = 30;

    /// A one-thread x264 (so byte-identical every run) lossless 320×180 encode of 30 frames on
    /// a disk with `budget` bytes free: its result, the error of a failed push if any, and the
    /// bytes the disk took.
    fn encode_on_disk(path: &Path, budget: u64) -> (Result<EncodeSummary, MediaError>, u64) {
        let spec = EncodeSpec::new(path)
            .with_video(VideoEncodeSpec {
                preset: crate::SpeedPreset::Ultrafast,
                rate_control: crate::RateControl::Crf(0),
                threads: 1,
                ..VideoEncodeSpec::h264(SIZE.0, SIZE.1, FrameRate::FPS_30, PixelFormat::Rgba8)
            })
            .with_hw(crate::HwEncode::Off);
        let written = Arc::new(AtomicU64::new(0));
        let disk = {
            let written = Arc::clone(&written);
            move |file| -> Box<dyn WriteSeek> {
                Box::new(FullDisk {
                    file,
                    budget,
                    written: Arc::clone(&written),
                })
            }
        };
        let mut enc = match open_with(&spec, &disk) {
            Ok(enc) => enc,
            Err(e) => return (Err(e), written.load(Ordering::Relaxed)),
        };
        for n in 0..FRAMES {
            if let Err(e) = enc.push_video(&frame(n, SIZE)) {
                // Later calls repeat the error.
                assert_eq!(enc.push_video(&frame(n + 1, SIZE)), Err(e.clone()));
                assert_eq!(enc.finish().err(), Some(e.clone()));
                return (Err(e), written.load(Ordering::Relaxed));
            }
        }
        (enc.finish(), written.load(Ordering::Relaxed))
    }

    fn assert_disk_full(result: Result<EncodeSummary, MediaError>, path: &Path, when: &str) {
        match result {
            Err(MediaError::Io { path: p, message }) => {
                assert_eq!(p, display(path), "{when}");
                assert!(message.contains("simulated"), "{when}: {message}");
            }
            other => panic!("{when}: want Io, got {other:?}"),
        }
    }

    #[test]
    fn a_full_disk_is_an_io_error_and_leaves_no_file() {
        FfmpegBackend::new().expect("backend");
        let dir = TempDir::new("disk-full");
        let path = dir.path().join("out.mp4");
        let listing = || {
            std::fs::read_dir(dir.path())
                .expect("list")
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        // Enough room: the size of the whole encode, faststart rewrite included.
        let (ok, total) = encode_on_disk(&path, u64::MAX);
        assert_eq!(ok.map(|s| s.video_frames), Ok(FRAMES));
        assert_eq!(listing(), ["out.mp4"]);
        std::fs::remove_file(&path).expect("remove");

        // Full while frames are pushed: lossless frames overflow 64 KiB long before the end.
        let (result, _) = encode_on_disk(&path, 64 * 1024);
        assert_disk_full(result, &path, "while pushing");
        assert!(listing().is_empty(), "{:?}", listing());

        // Full on the very last write, inside `finish` (trailer and index move).
        let (result, took) = encode_on_disk(&path, total - 1);
        assert_disk_full(result, &path, "while finishing");
        assert!(took < total);
        assert!(listing().is_empty(), "{:?}", listing());

        // No room at all.
        let (result, _) = encode_on_disk(&path, 0);
        assert_disk_full(result, &path, "with no room at all");
        assert!(listing().is_empty(), "{:?}", listing());
    }

    #[test]
    fn the_backend_opens_this_encoder() {
        let backend = FfmpegBackend::new().expect("backend");
        let dir = TempDir::new("open");
        let path = dir.path().join("a.mp4");
        let spec =
            EncodeSpec::new(&path).with_audio(AudioEncodeSpec::aac(SampleRate::HZ_48_000, 1));
        let mut enc = backend.open_encoder(&spec).expect("open");
        let tone = crate::testing::FakeAudio::tone(
            SampleRate::HZ_48_000,
            1,
            Duration::from_secs(1).expect("1 s"),
        );
        enc.push_audio(&AudioChunk {
            pts: cutweave_model::Time::ZERO,
            rate: SampleRate::HZ_48_000,
            channels: 1,
            planes: vec![(0..48_000).map(|n| tone.sample(0, n)).collect()],
        })
        .expect("push");
        let summary = enc.finish().expect("finish");
        assert_eq!(summary.audio_samples, 48_000);
        assert!(path.exists());
    }
}
