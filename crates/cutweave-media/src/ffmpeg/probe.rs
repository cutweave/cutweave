// SPDX-License-Identifier: GPL-3.0-or-later
//! `probe` for the FFmpeg backend (WP-1D.02): streams, duration, frame rate and VFR, rotation,
//! color tags, audio format and codec profile, mapped to [`ProbeInfo`].
//!
//! A still image's rotation is its EXIF orientation ([`still_orientation`](crate::still_orientation)).
//!
//! Cheap by design (import probes synchronously under a timeout, arch §7): it reads the container
//! header and `avformat_find_stream_info`'s sample, not the whole file. The only thing that looks
//! at many packets is VFR detection, which uses the demuxer's seek index where the container has
//! a per-frame one (MP4/MOV, AVI) and otherwise samples the first [`VFR_SAMPLE_PACKETS`]
//! packets. The frame index ([`index`](super::index)) is authoritative for frame timing; a file
//! whose first seconds are constant-rate but which turns variable later is only caught there.

use std::path::Path;

use cutweave_model::{
    AudioInfo, ColorTags, Duration, Extra, FrameRate, ProbeInfo, SampleRate, StreamInfo, StreamKind,
};
use ffmpeg_next::codec::packet::side_data::Type as SideDataType;
use ffmpeg_next::format::context::Input;
use ffmpeg_next::format::stream::{Disposition, Stream};
use ffmpeg_next::media::Type;
use ffmpeg_next::{Error, Packet};

use super::{input, sys};
use crate::MediaError;
use crate::index::{TimeBase, is_vfr_range};

/// Packets sampled for VFR detection when the container has no per-frame seek index.
const VFR_SAMPLE_PACKETS: usize = 2000;

/// `AV_NOPTS_VALUE`.
const NOPTS: i64 = i64::MIN;

pub(super) fn probe(path: &Path) -> Result<ProbeInfo, MediaError> {
    let mut input = input::open(path)?;
    if input.nb_streams() == 0 {
        return Err(MediaError::Corrupt {
            path: crate::path::display(path),
            message: "the container has no streams".to_owned(),
        });
    }
    let container = input.format().name().to_owned();
    let still_image = is_image_container(&container);

    let mut info = ProbeInfo::default();
    for stream in input.streams() {
        info.streams.push(stream_info(&stream));
    }

    // The main video stream: FFmpeg's pick (best resolution, not cover art unless it is all there
    // is), and the first audio stream FFmpeg would play.
    let video = input.streams().best(Type::Video).map(|s| s.index());
    let audio = input.streams().best(Type::Audio).map(|s| s.index());

    if let Some(i) = video
        && let Some(stream) = input.stream(i)
    {
        let attached_pic = stream.disposition().contains(Disposition::ATTACHED_PIC);
        let params = sys::stream_params(&stream.parameters());
        info.color = ColorTags {
            primaries: params.primaries.as_deref().map(parse_name),
            transfer: params.transfer.as_deref().map(parse_name),
            matrix: params.matrix.as_deref().map(parse_name),
            range: params.range,
        };
        info.rotation = rotation(&stream);
        if !still_image && !attached_pic {
            info.fps = frame_rate(&stream);
            info.vfr = detect_vfr(&mut input, i);
        }
    }

    if let Some(i) = audio
        && let Some(stream) = input.stream(i)
    {
        let params = sys::stream_params(&stream.parameters());
        if let (Ok(rate), Ok(channels)) = (
            SampleRate::new(params.sample_rate),
            u16::try_from(params.channels),
        ) && channels > 0
        {
            info.audio = Some(AudioInfo { rate, channels });
        }
    }

    if !still_image {
        info.duration = duration(&input, video);
    } else if let Some(orientation) = crate::still_orientation(path) {
        // A photo's EXIF orientation (WP-2F.05a). FFmpeg's image demuxers put no display matrix
        // on the stream, and its decoders return the picture as coded, like video.
        info.rotation = orientation.rotation;
        if orientation.mirrored {
            tracing::debug!(
                path = %crate::path::display(path),
                "the still's EXIF orientation is mirrored; it is shown turned but not mirrored"
            );
        }
    }
    Ok(info)
}

/// A color name as the model's enum (unknown names are kept as `Other`, never an error).
pub(super) fn parse_name<T: std::str::FromStr<Err = std::convert::Infallible>>(name: &str) -> T {
    match name.parse() {
        Ok(v) => v,
        Err(e) => match e {},
    }
}

/// Image demuxers (`image2`, `png_pipe`, `jpeg_pipe`, ...): one picture, no timeline of its own.
fn is_image_container(name: &str) -> bool {
    name.split(',')
        .any(|n| n == "image2" || n.ends_with("_pipe") || n == "ppm" || n == "pgm")
}

fn stream_kind(medium: Type) -> StreamKind {
    match medium {
        Type::Video => StreamKind::Video,
        Type::Audio => StreamKind::Audio,
        Type::Subtitle => StreamKind::Subtitle,
        Type::Attachment => StreamKind::Attachment,
        Type::Data | Type::Unknown => StreamKind::Data,
    }
}

fn stream_info(stream: &Stream<'_>) -> StreamInfo {
    let par = stream.parameters();
    let kind = stream_kind(par.medium());
    let p = sys::stream_params(&par);
    let video = kind == StreamKind::Video;
    let audio = kind == StreamKind::Audio;
    StreamInfo {
        index: u32::try_from(stream.index()).unwrap_or(u32::MAX),
        kind,
        codec: p.codec,
        profile: p.profile,
        width: (video && p.width > 0).then_some(p.width),
        height: (video && p.height > 0).then_some(p.height),
        pixel_format: if video { p.pixel_format } else { None },
        sample_rate: if audio {
            SampleRate::new(p.sample_rate).ok()
        } else {
            None
        },
        channels: if audio {
            u16::try_from(p.channels).ok().filter(|&c| c > 0)
        } else {
            None
        },
        language: stream
            .metadata()
            .get("language")
            .filter(|l| !l.is_empty() && *l != "und")
            .map(str::to_owned),
        extra: Extra::new(),
    }
}

/// The average frame rate (or the container's base rate when there is none) of a video stream.
fn frame_rate(stream: &Stream<'_>) -> Option<FrameRate> {
    [stream.avg_frame_rate(), stream.rate()]
        .into_iter()
        .find_map(|r| {
            let (num, den) = (
                u32::try_from(r.numerator()).ok()?,
                u32::try_from(r.denominator()).ok()?,
            );
            if num == 0 || den == 0 {
                return None;
            }
            FrameRate::new(num, den).ok()
        })
}

/// Display rotation in degrees clockwise (0, 90, 180, 270) from the stream's display matrix.
fn rotation(stream: &Stream<'_>) -> u16 {
    stream
        .side_data()
        .find(|sd| sd.kind() == SideDataType::DisplayMatrix)
        .and_then(|sd| rotation_from_matrix(sd.data()))
        .unwrap_or(0)
}

/// Clockwise rotation, rounded to the nearest quarter turn, from a 3x3 16.16 fixed-point display
/// matrix (36 bytes, little endian) as `av_display_rotation_get` reads it. That function (and
/// `ffprobe`'s `rotation`) report the counter-clockwise angle by which the picture is shown
/// rotated; the clockwise angle of [`ProbeInfo::rotation`] is its negation. `None` for a matrix
/// that is not a rotation.
fn rotation_from_matrix(m: &[u8]) -> Option<u16> {
    if m.len() < 36 {
        return None;
    }
    let v = |i: usize| {
        f64::from(i32::from_le_bytes([
            m[4 * i],
            m[4 * i + 1],
            m[4 * i + 2],
            m[4 * i + 3],
        ])) / 65536.0
    };
    let s0 = v(0).hypot(v(3));
    let s1 = v(1).hypot(v(4));
    if s0 == 0.0 || s1 == 0.0 {
        return None;
    }
    // Counter-clockwise degrees, as ffprobe prints them (mod the sign convention).
    let ccw = -(v(1) / s1).atan2(v(0) / s0).to_degrees();
    let cw = (-ccw).rem_euclid(360.0);
    // Quarter turns only: a tilted matrix is not a "rotation" the editor can apply as metadata.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=4 after rounding
    let quarters = (cw / 90.0).round() as u16 % 4;
    Some(quarters * 90)
}

/// The container duration, else the longest stream duration, as a [`Duration`]; `None` if the
/// file does not say or says zero.
fn duration(input: &Input, video: Option<usize>) -> Option<Duration> {
    // AV_TIME_BASE units (microseconds).
    let container = input.duration();
    let from_container = (container != NOPTS && container > 0)
        .then(|| TimeBase::new(1, 1_000_000)?.to_time(container))
        .flatten();
    let time = from_container.or_else(|| {
        // No container duration (MPEG-TS, raw streams): the longest stream, preferring picture.
        let from_stream = |s: &Stream<'_>| {
            let d = s.duration();
            let tb = s.time_base();
            (d != NOPTS && d > 0)
                .then(|| {
                    TimeBase::new(
                        u32::try_from(tb.numerator()).ok()?,
                        u32::try_from(tb.denominator()).ok()?,
                    )?
                    .to_time(d)
                })
                .flatten()
        };
        let main = video
            .and_then(|i| input.stream(i))
            .and_then(|s| from_stream(&s));
        main.or_else(|| input.streams().filter_map(|s| from_stream(&s)).max())
    })?;
    Duration::try_from(time).ok().filter(|d| !d.is_zero())
}

/// Whether the main video stream's frame times are irregular (see the module docs).
fn detect_vfr(input: &mut Input, stream_index: usize) -> bool {
    // The demuxer's own per-sample index, when it has one entry per frame.
    let from_index = input.stream(stream_index).and_then(|s| {
        let n = s.frames();
        let ts = sys::seek_index_timestamps(&s)?;
        (n > 0 && i64::try_from(ts.len()).is_ok_and(|len| len >= n)).then_some(ts)
    });
    let mut ts = match from_index {
        Some(ts) => ts,
        None => sample_packet_times(input, stream_index),
    };
    // Entries are in decode order and packets can be too (B-frames): frame times, sorted.
    ts.sort_unstable();
    let mut deltas = ts.windows(2).map(|w| w[1] - w[0]);
    let (Some(first), Some(second)) = (deltas.next(), deltas.next()) else {
        return false; // fewer than three frames: no pattern to break
    };
    let (mut lo, mut hi) = (first.min(second), first.max(second));
    for d in deltas {
        lo = lo.min(d);
        hi = hi.max(d);
    }
    is_vfr_range(lo, hi)
}

/// The pts (else dts) of the first [`VFR_SAMPLE_PACKETS`] packets of the stream.
fn sample_packet_times(input: &mut Input, stream_index: usize) -> Vec<i64> {
    let mut times = Vec::new();
    let mut errors = 0;
    while times.len() < VFR_SAMPLE_PACKETS {
        let mut pkt = Packet::empty();
        match pkt.read(input) {
            Ok(()) => {
                if pkt.stream() == stream_index
                    && let Some(t) = pkt.pts().or(pkt.dts())
                {
                    times.push(t);
                }
            }
            Err(Error::InvalidData) if errors < 100 => errors += 1,
            Err(_) => break,
        }
    }
    times
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3x3 16.16 matrix as av_display_rotation_set writes it for a clockwise angle `deg`.
    fn matrix(deg: f64) -> Vec<u8> {
        let (s, c) = deg.to_radians().sin_cos();
        #[allow(clippy::cast_possible_truncation)] // small fixed-point values
        let fixed = |v: f64| ((v * 65536.0).round() as i32).to_le_bytes();
        // Row-major: [c, -s, 0; s, c, 0; 0, 0, 1<<30] with av_display_rotation_set's signs.
        [c, s, 0.0, -s, c, 0.0, 0.0, 0.0]
            .iter()
            .flat_map(|&v| fixed(v))
            .chain((1i32 << 30).to_le_bytes())
            .collect()
    }

    #[test]
    fn rotation_is_clockwise_quarter_turns() {
        // av_display_rotation_set takes the clockwise angle; ffprobe's `rotation` field for the
        // same matrix is its negation (the counter-clockwise angle of av_display_rotation_get).
        assert_eq!(rotation_from_matrix(&matrix(0.0)), Some(0));
        assert_eq!(rotation_from_matrix(&matrix(90.0)), Some(90));
        assert_eq!(rotation_from_matrix(&matrix(180.0)), Some(180));
        assert_eq!(rotation_from_matrix(&matrix(270.0)), Some(270));
        assert_eq!(rotation_from_matrix(&matrix(-90.0)), Some(270));
        // Near a quarter turn rounds to it; garbage is no rotation.
        assert_eq!(rotation_from_matrix(&matrix(91.4)), Some(90));
        assert_eq!(rotation_from_matrix(&[0; 36]), None);
        assert_eq!(rotation_from_matrix(&[0; 8]), None);
    }

    #[test]
    fn image_containers_are_recognized_by_demuxer_name() {
        for name in ["image2", "png_pipe", "jpeg_pipe", "webp_pipe", "ppm"] {
            assert!(is_image_container(name), "{name}");
        }
        for name in [
            "mov,mp4,m4a,3gp,3g2,mj2",
            "matroska,webm",
            "wav",
            "mpegts",
            "gif",
        ] {
            assert!(!is_image_container(name), "{name}");
        }
    }
}
