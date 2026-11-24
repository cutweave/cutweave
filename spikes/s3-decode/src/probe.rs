// SPDX-License-Identifier: GPL-3.0-or-later
//! Probe facts the media pipeline needs beyond the index: rotation (display matrix), color tags,
//! pixel format, VFR. Plus a full audio decode with a sample count against ffprobe.

use std::path::Path;

use ffmpeg::codec::packet::side_data::Type as SideDataType;
use ffmpeg::util::error::EAGAIN;
use ffmpeg_next as ffmpeg;

use crate::error::{Error, Result, ff};
use crate::source::{Options, VideoSource};

/// What the probe found for the best video stream.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub container: String,
    pub codec: String,
    pub profile: String,
    pub width: u32,
    pub height: u32,
    pub pix_fmt: String,
    pub bit_depth: u32,
    /// Display rotation from the display matrix, counter-clockwise degrees in [0, 360) (0 when
    /// absent): ffprobe's `rotation` and ffmpeg's `-display_rotation` convention.
    pub rotation: f64,
    pub primaries: String,
    pub transfer: String,
    pub matrix: String,
    pub range: String,
    pub vfr: bool,
    pub frames: usize,
    pub time_base: (i32, i32),
    pub start_pts: i64,
    pub gop: (usize, usize, usize, f64),
    pub delta_min_max: (i64, i64),
    pub index_ms: f64,
}

/// Rotation from a 3x3 16.16 display matrix exactly as `av_display_rotation_get` computes it
/// (counter-clockwise degrees), normalised to [0, 360). The picture is shown rotated by this
/// angle counter-clockwise; FFmpeg's autorotate applies `360 - angle` clockwise. Note that
/// `av_display_rotation_set` takes the *clockwise* angle: get(set(a)) == -a.
pub fn rotation_from_matrix(m: &[u8]) -> Option<f64> {
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
    let ccw = -(v(1) / s1).atan2(v(0) / s0).to_degrees();
    Some(ccw.rem_euclid(360.0).round() % 360.0)
}

/// Probes `path` (opens it, scans the index, decodes the first frame for color tags).
pub fn info(path: &Path) -> Result<Info> {
    let p = path.to_str().ok_or(Error::NonUtf8Path)?;
    let input = ffmpeg::format::input(p).map_err(ff("avformat_open_input"))?;
    let st = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or(Error::NoVideoStream)?;
    let rotation = st
        .side_data()
        .find(|sd| sd.kind() == SideDataType::DisplayMatrix)
        .and_then(|sd| rotation_from_matrix(sd.data()))
        .unwrap_or(0.0);
    let container = input.format().name().to_string();
    drop(input);

    let mut src = VideoSource::open(path, Options::default())?;
    src.rewind()?;
    src.next_frame()?;
    let f = src.frame();
    let d = src.decoder();
    let bit_depth = crate::ffi::luma_layout(f.format()).map_or(0, |l| l.0);
    let ix = &src.index;
    Ok(Info {
        container,
        codec: d.codec().map(|c| c.name().to_string()).unwrap_or_default(),
        profile: format!("{:?}", d.profile()),
        width: f.width(),
        height: f.height(),
        pix_fmt: format!("{:?}", f.format()),
        bit_depth,
        rotation,
        primaries: format!("{:?}", f.color_primaries()),
        transfer: format!("{:?}", f.color_transfer_characteristic()),
        matrix: format!("{:?}", f.color_space()),
        range: format!("{:?}", f.color_range()),
        vfr: ix.is_vfr(),
        frames: ix.frames.len(),
        time_base: ix.time_base,
        start_pts: ix.frames[0].pts,
        gop: ix.gop_stats(),
        delta_min_max: ix.frame_deltas(),
        index_ms: ix.scan_time.as_secs_f64() * 1e3,
    })
}

/// Result of decoding the best audio stream.
#[derive(Clone, Debug, Default)]
pub struct AudioReport {
    pub codec: String,
    pub rate: u32,
    pub channels: u16,
    pub sample_fmt: String,
    pub samples: u64,
    pub ffprobe_samples: u64,
    pub ffprobe_rate: u64,
    pub ffprobe_channels: u64,
    pub decode_errors: u64,
}

/// Decodes every audio frame of the best audio stream and counts samples per channel.
pub fn audio(path: &Path) -> Result<AudioReport> {
    let p = path.to_str().ok_or(Error::NonUtf8Path)?;
    let mut input = ffmpeg::format::input(p).map_err(ff("avformat_open_input"))?;
    let st = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or(Error::Truth("no audio stream".into()))?;
    let idx = st.index();
    let mut ctx = ffmpeg::codec::Context::from_parameters(st.parameters())
        .map_err(ff("avcodec_parameters_to_context"))?;
    ctx.set_threading(ffmpeg::codec::threading::Config {
        kind: ffmpeg::codec::threading::Type::Frame,
        count: 0,
    });
    let mut dec = ctx.decoder();
    dec.set_packet_time_base(st.time_base());
    let mut dec = dec.audio().map_err(ff("avcodec_open2(audio)"))?;
    let mut r = AudioReport {
        codec: dec
            .codec()
            .map(|c| c.name().to_string())
            .unwrap_or_default(),
        ..AudioReport::default()
    };
    let mut frame = ffmpeg::frame::Audio::empty();
    let mut drain = |dec: &mut ffmpeg::decoder::Audio, r: &mut AudioReport| -> Result<bool> {
        loop {
            match dec.receive_frame(&mut frame) {
                Ok(()) => {
                    r.samples += frame.samples() as u64;
                    r.rate = frame.rate();
                    r.channels = frame.channels();
                    r.sample_fmt = format!("{:?}", frame.format());
                }
                Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => return Ok(false),
                Err(ffmpeg::Error::Eof) => return Ok(true),
                Err(ffmpeg::Error::InvalidData) => r.decode_errors += 1,
                Err(err) => {
                    return Err(Error::Ffmpeg {
                        op: "avcodec_receive_frame(audio)",
                        err,
                    });
                }
            }
        }
    };
    loop {
        let mut pkt = ffmpeg::Packet::empty();
        match pkt.read(&mut input) {
            Ok(()) if pkt.stream() == idx => {
                if dec.send_packet(&pkt).is_err() {
                    r.decode_errors += 1;
                }
                drain(&mut dec, &mut r)?;
            }
            Ok(()) => {}
            Err(ffmpeg::Error::InvalidData) => r.decode_errors += 1,
            Err(_) => break,
        }
    }
    let _ = dec.send_eof();
    drain(&mut dec, &mut r)?;
    let (s, rate, ch) = crate::truth::audio_samples(path, idx)?;
    r.ffprobe_samples = s;
    r.ffprobe_rate = rate;
    r.ffprobe_channels = ch;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A matrix whose `av_display_rotation_get` is `deg_ccw` (= av_display_rotation_set(-deg_ccw)).
    fn matrix(deg_ccw: f64) -> Vec<u8> {
        let r = deg_ccw.to_radians();
        let (c, s) = (r.cos(), r.sin());
        let m = [c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0];
        let fixed = |i: usize, x: f64| -> i32 {
            (x * if i % 3 == 2 {
                f64::from(1 << 30)
            } else {
                65536.0
            })
            .round() as i32
        };
        m.iter()
            .enumerate()
            .flat_map(|(i, &x)| fixed(i, x).to_le_bytes())
            .collect()
    }

    #[test]
    fn display_matrix_rotation_matches_ffprobe_convention() {
        // `-display_rotation 90` -> ffprobe rotation=90; `-display_rotation 270` -> ffprobe -90.
        assert_eq!(rotation_from_matrix(&matrix(0.0)), Some(0.0));
        assert_eq!(rotation_from_matrix(&matrix(90.0)), Some(90.0));
        assert_eq!(rotation_from_matrix(&matrix(-90.0)), Some(270.0));
        assert_eq!(rotation_from_matrix(&matrix(180.0)), Some(180.0));
        assert_eq!(rotation_from_matrix(&[0u8; 8]), None);
    }
}
