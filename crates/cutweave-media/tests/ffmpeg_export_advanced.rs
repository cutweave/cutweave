// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2G.05 acceptance, encoder side: the advanced encoder settings through the FFmpeg backend,
//! measured with `ffprobe`. **Software encoders only** (`hw: Off`): the output is deterministic
//! and no GPU is needed. Inputs are generated (no media files, no network).
//!
//! - Constant bitrate: the average of 8 Mb/s H.264 is within 5 % of the target and no second of
//!   the file holds more than 1.2 times the target.
//! - CRF 18 and CRF 28 differ in size the right way round.
//! - `keyint` puts keyframes exactly on the interval (scene-cut off) for H.264, HEVC and AV1;
//!   a closed GOP has no picture that is shown before its keyframe and decoded after it.
//! - Color tags read back exactly as set, and convert mode changes the pixels where tag-only
//!   mode does not.
//! - Audio: AAC at 192 kb/s and Opus at 128 kb/s read back within 10 %; MP3, FLAC and PCM, mono
//!   and 44.1 and 96 kHz.
//! - Settings an encoder cannot take are refused when it opens.
//!
//! `ffprobe` / `ffmpeg` come from the bundle (`$FFMPEG_DIR/bin`, else `PATH`), as set up by
//! `scripts/ffmpeg/env.sh` (docs/dev/ffmpeg.md).
#![cfg(feature = "ffmpeg")]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::expect_used,
    clippy::unwrap_used
)] // test helpers

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use cutweave_media::testing::contract::TempDir;
use cutweave_media::{
    AudioChunk, AudioEncodeSpec, EncodeSpec, FfmpegBackend, HwEncode, MediaBackend, MediaError,
    PixelFormat, Plane, RateControl, SpeedPreset, VideoAdvanced, VideoEncodeSpec, VideoFrame,
};
use cutweave_model::{
    ColorPrimaries, ColorRange, ColorTags, FrameRate, MatrixCoefficients, SampleRate,
    TransferCharacteristics,
};

const SIZE: (u32, u32) = (480, 270);
const RATE: FrameRate = FrameRate::FPS_30;

fn backend() -> FfmpegBackend {
    FfmpegBackend::new().expect("the loaded FFmpeg matches the build")
}

fn tool(name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("FFMPEG_DIR") {
        let exe = Path::new(&dir)
            .join("bin")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if exe.is_file() {
            return exe;
        }
    }
    PathBuf::from(name)
}

fn ffprobe(args: &[&str], path: &Path) -> BTreeMap<String, String> {
    let out = Command::new(tool("ffprobe"))
        .args(["-v", "error"])
        .args(args)
        .args(["-of", "flat=s=_"])
        .arg(path)
        .output()
        .expect("run ffprobe (source scripts/ffmpeg/env.sh)");
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "ffprobe: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.trim_matches('"').to_owned()))
        .collect()
}

fn get<'a>(info: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    info.get(key).map_or_else(
        || panic!("ffprobe has no `{key}`: {info:#?}"),
        String::as_str,
    )
}

/// One packet of a stream.
#[derive(Clone, Debug)]
struct Packet {
    pts: f64,
    dts: f64,
    bytes: u64,
    key: bool,
}

/// The packets of the first stream of `kind` (`v` or `a`), in decode order.
fn packets(path: &Path, kind: &str) -> Vec<Packet> {
    let info = ffprobe(
        &[
            "-select_streams",
            kind,
            "-show_packets",
            "-show_entries",
            "packet=pts_time,dts_time,size,flags",
        ],
        path,
    );
    let mut rows: BTreeMap<usize, Packet> = BTreeMap::new();
    for (k, v) in &info {
        let Some(rest) = k.strip_prefix("packets_packet_") else {
            continue;
        };
        let (index, field) = rest.split_once('_').unwrap();
        let row = rows.entry(index.parse().unwrap()).or_insert(Packet {
            pts: 0.0,
            dts: 0.0,
            bytes: 0,
            key: false,
        });
        match field {
            "pts_time" => row.pts = v.parse().unwrap(),
            "dts_time" => row.dts = v.parse().unwrap(),
            "size" => row.bytes = v.parse().unwrap(),
            "flags" => row.key = v.starts_with('K'),
            _ => {}
        }
    }
    rows.into_values().collect()
}

/// A deterministic noisy RGBA frame: a drifting gradient with per-pixel noise (hard to compress,
/// so a bitrate target is reached).
fn frame(n: u64, amplitude: u32) -> VideoFrame {
    let (w, h) = SIZE;
    let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ n.wrapping_mul(0x2545_f491_4f6c_dd1d);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let noise = (state % u64::from(amplitude * 2 + 1)) as i32 - amplitude as i32;
            let base = ((x + y + n as u32 * 3) % 200) as i32 + 20;
            let v = (base + noise).clamp(0, 255) as u8;
            data.extend_from_slice(&[v, 255 - v, (v / 2).saturating_add(40), 255]);
        }
    }
    VideoFrame {
        pts: RATE.frame_to_time(n as i64).unwrap(),
        width: w,
        height: h,
        format: PixelFormat::Rgba8,
        planes: vec![Plane {
            data,
            stride: w * 4,
        }],
        color: ColorTags::default(),
        damaged: false,
    }
}

/// A solid frame.
fn solid(n: u64, rgb: [u8; 3]) -> VideoFrame {
    let (w, h) = SIZE;
    let mut f = frame(n, 0);
    f.planes[0].data = (0..w * h)
        .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
        .collect();
    f
}

fn video_spec(codec: &str) -> VideoEncodeSpec {
    let mut v = VideoEncodeSpec::h264(SIZE.0, SIZE.1, RATE, PixelFormat::Rgba8).with_codec(codec);
    v.preset = SpeedPreset::Veryfast;
    v.threads = 2;
    v
}

/// Encodes `frames` noisy frames with `video` (no audio) into `name` in `dir`.
fn encode_video(
    dir: &TempDir,
    name: &str,
    video: VideoEncodeSpec,
    frames: u64,
) -> Result<PathBuf, MediaError> {
    let path = dir.path().join(name);
    let spec = EncodeSpec::new(&path)
        .with_video(video)
        .with_hw(HwEncode::Off);
    let mut enc = backend().open_encoder(&spec)?;
    for n in 0..frames {
        enc.push_video(&frame(n, 40))?;
    }
    enc.finish()?;
    Ok(path)
}

/// Size of the video packets in bits, and the most any second holds.
fn bit_stats(path: &Path) -> (f64, f64) {
    let packets = packets(path, "v");
    let total: u64 = packets.iter().map(|p| p.bytes * 8).sum();
    let first = packets.iter().map(|p| p.pts).fold(f64::MAX, f64::min);
    let last = packets.iter().map(|p| p.pts).fold(0.0_f64, f64::max);
    let seconds = last - first + 1.0 / 30.0;
    let mut peak = 0_u64;
    for start in &packets {
        let window: u64 = packets
            .iter()
            .filter(|p| p.dts >= start.dts && p.dts < start.dts + 1.0)
            .map(|p| p.bytes * 8)
            .sum();
        // A window running past the end is not a full second.
        if start.dts + 1.0 <= last {
            peak = peak.max(window);
        }
    }
    (total as f64 / seconds, peak as f64)
}

#[test]
fn constant_bitrate_8_mbps_holds_the_average_and_the_one_second_peak() {
    let dir = TempDir::new("adv-cbr");
    for codec in ["h264", "hevc"] {
        let mut v = video_spec(codec);
        v.rate_control = RateControl::Cbr(8_000_000);
        let path = encode_video(&dir, &format!("cbr-{codec}.mp4"), v, 120).unwrap();
        let (average, peak) = bit_stats(&path);
        // x265's CBR is looser than x264's (it has no filler): 10 % there, 5 % for H.264.
        let slack = if codec == "h264" { 0.05 } else { 0.10 };
        assert!(
            (average / 8e6 - 1.0).abs() <= slack,
            "{codec}: the average is {average:.0} b/s, not 8 Mb/s +- {slack}"
        );
        assert!(
            peak <= 8e6 * 1.2,
            "{codec}: a second holds {peak:.0} bits, more than 1.2 x 8 Mb"
        );
    }
}

#[test]
fn a_capped_variable_bitrate_stays_under_its_peak() {
    let dir = TempDir::new("adv-vbr");
    let mut v = video_spec("h264");
    v.rate_control = RateControl::Bitrate(6_000_000);
    v.advanced = VideoAdvanced {
        max_bitrate: Some(7_000_000),
        buffer_size: Some(3_500_000),
        ..VideoAdvanced::default()
    };
    let path = encode_video(&dir, "vbr.mp4", v, 120).unwrap();
    let (average, peak) = bit_stats(&path);
    assert!(average <= 7e6 * 1.05, "average {average}");
    assert!(peak <= 7e6 * 1.5, "peak {peak}");
    // Constrained quality: a quality that wants far more than the cap is held to it.
    let mut cq = video_spec("h264");
    cq.rate_control = RateControl::Crf(10);
    cq.advanced = VideoAdvanced {
        max_bitrate: Some(4_000_000),
        buffer_size: Some(2_000_000),
        ..VideoAdvanced::default()
    };
    let path = encode_video(&dir, "cq.mp4", cq, 120).unwrap();
    let (average, _) = bit_stats(&path);
    assert!(
        average <= 4e6 * 1.1,
        "the cap holds the quality's rate: {average}"
    );
}

#[test]
fn crf_18_is_larger_than_crf_28() {
    let dir = TempDir::new("adv-crf");
    let size = |crf: u8| {
        let mut v = video_spec("h264");
        v.rate_control = RateControl::Crf(crf);
        let path = encode_video(&dir, &format!("crf{crf}.mp4"), v, 60).unwrap();
        std::fs::metadata(path).unwrap().len()
    };
    let (good, small) = (size(18), size(28));
    assert!(
        good as f64 > small as f64 * 1.5,
        "crf 18 is {good} bytes, crf 28 is {small}"
    );
}

/// The frame numbers of the keyframes, in presentation order.
fn keyframes(path: &Path) -> Vec<usize> {
    let mut all = packets(path, "v");
    all.sort_by(|a, b| a.pts.total_cmp(&b.pts));
    all.iter()
        .enumerate()
        .filter(|(_, p)| p.key)
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn keyframes_fall_exactly_on_the_interval() {
    let dir = TempDir::new("adv-keyint");
    for (codec, keyint, bframes) in [
        ("h264", 60_u32, Some(2_u8)),
        ("hevc", 40, Some(2)),
        ("av1", 30, None),
    ] {
        let mut v = video_spec(codec);
        v.rate_control = RateControl::Crf(30);
        v.advanced = VideoAdvanced {
            keyint: Some(keyint),
            scenecut: Some(false),
            bframes,
            // SVT-AV1's default keyframes are not sync points (open GOP); IDR ones are.
            closed_gop: (codec == "av1").then_some(true),
            ..VideoAdvanced::default()
        };
        let path = encode_video(&dir, &format!("keyint-{codec}.mp4"), v, 130).unwrap();
        let keys = keyframes(&path);
        let want: Vec<usize> = (0..130).step_by(keyint as usize).collect();
        assert_eq!(keys, want, "{codec}: keyframes of keyint {keyint}");
    }
}

/// Whether any picture is shown before a keyframe it is decoded after (an open GOP).
fn has_leading_pictures(path: &Path) -> bool {
    let all = packets(path, "v");
    all.iter()
        .enumerate()
        .any(|(i, k)| k.key && i > 0 && all[i + 1..].iter().any(|later| later.pts < k.pts - 1e-6))
}

#[test]
fn a_closed_gop_has_no_picture_across_a_keyframe_and_an_open_one_does() {
    let dir = TempDir::new("adv-gop");
    for codec in ["h264", "hevc"] {
        let encode = |closed: bool| {
            let mut v = video_spec(codec);
            v.rate_control = RateControl::Crf(30);
            v.advanced = VideoAdvanced {
                keyint: Some(30),
                scenecut: Some(false),
                bframes: Some(3),
                closed_gop: Some(closed),
                ..VideoAdvanced::default()
            };
            encode_video(&dir, &format!("gop-{codec}-{closed}.mp4"), v, 100).unwrap()
        };
        assert!(!has_leading_pictures(&encode(true)), "{codec}: closed GOP");
        assert!(
            has_leading_pictures(&encode(false)),
            "{codec}: the open GOP is what the check can tell apart"
        );
    }
}

#[test]
fn the_encoder_settings_read_back() {
    let dir = TempDir::new("adv-settings");
    let mut v = video_spec("h264");
    v.profile = Some("baseline".to_owned());
    v.advanced.level = Some("4.1".to_owned());
    v.advanced.tune = Some("zerolatency".to_owned());
    let path = encode_video(&dir, "baseline.mp4", v, 20).unwrap();
    let info = ffprobe(&["-show_streams", "-select_streams", "v:0"], &path);
    assert_eq!(
        get(&info, "streams_stream_0_profile"),
        "Constrained Baseline"
    );
    assert_eq!(get(&info, "streams_stream_0_level"), "41");
    assert_eq!(get(&info, "streams_stream_0_has_b_frames"), "0");
    // HEVC level.
    let mut v = video_spec("hevc");
    v.advanced.level = Some("4.1".to_owned());
    let path = encode_video(&dir, "level.mp4", v, 20).unwrap();
    let info = ffprobe(&["-show_streams", "-select_streams", "v:0"], &path);
    assert_eq!(get(&info, "streams_stream_0_level"), "123");
}

fn tags(p: &str, t: &str, m: &str, range: ColorRange) -> ColorTags {
    ColorTags {
        primaries: Some(p.parse::<ColorPrimaries>().unwrap()),
        transfer: Some(t.parse::<TransferCharacteristics>().unwrap()),
        matrix: Some(m.parse::<MatrixCoefficients>().unwrap()),
        range: Some(range),
    }
}

#[test]
fn color_tags_read_back_exactly_as_set() {
    let dir = TempDir::new("adv-color");
    let cases = [
        (
            "h264",
            "mp4",
            tags("smpte170m", "smpte170m", "smpte170m", ColorRange::Limited),
            ("smpte170m", "smpte170m", "smpte170m", "tv"),
        ),
        (
            "h264",
            "mp4",
            tags("bt709", "iec61966-2-1", "bt709", ColorRange::Full),
            ("bt709", "iec61966-2-1", "bt709", "pc"),
        ),
        (
            "hevc",
            "mp4",
            tags("bt2020", "bt2020-10", "bt2020nc", ColorRange::Limited),
            ("bt2020", "bt2020-10", "bt2020nc", "tv"),
        ),
        (
            "av1",
            "mp4",
            tags("bt470bg", "gamma22", "bt470bg", ColorRange::Full),
            ("bt470bg", "bt470m", "bt470bg", "pc"),
        ),
        (
            "prores",
            "mov",
            tags("smpte170m", "bt709", "smpte170m", ColorRange::Limited),
            ("smpte170m", "bt709", "smpte170m", "tv"),
        ),
    ];
    for (codec, ext, color, (primaries, transfer, matrix, range)) in cases {
        let mut v = video_spec(codec);
        v.color = color;
        let path = encode_video(&dir, &format!("tags-{codec}.{ext}"), v, 10).unwrap();
        let info = ffprobe(&["-show_streams", "-select_streams", "v:0"], &path);
        let got = |k: &str| get(&info, &format!("streams_stream_0_{k}")).to_owned();
        assert_eq!(
            (
                got("color_primaries"),
                got("color_transfer"),
                got("color_space"),
                got("color_range")
            ),
            (
                primaries.to_owned(),
                transfer.to_owned(),
                matrix.to_owned(),
                range.to_owned()
            ),
            "{codec}"
        );
    }
}

/// The luma of the first frame of `path` read as full range, whatever the file's tags say.
fn first_luma(path: &Path) -> u8 {
    let out = Command::new(tool("ffmpeg"))
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=in_range=full:out_range=full,format=gray",
            "-f",
            "rawvideo",
            "-",
        ])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success() && !out.stdout.is_empty());
    out.stdout[out.stdout.len() / 2]
}

#[test]
fn convert_changes_the_pixels_and_tag_only_does_not() {
    let dir = TempDir::new("adv-convert");
    let white = |name: &str, advanced: VideoAdvanced| {
        let mut v = video_spec("h264");
        v.rate_control = RateControl::Crf(12);
        v.color = tags("bt709", "bt709", "bt709", ColorRange::Full);
        v.advanced = advanced;
        let path = dir.path().join(name);
        let spec = EncodeSpec::new(&path).with_video(v).with_hw(HwEncode::Off);
        let mut enc = backend().open_encoder(&spec).unwrap();
        for n in 0..5 {
            enc.push_video(&solid(n, [255, 255, 255])).unwrap();
        }
        enc.finish().unwrap();
        path
    };
    let converted = first_luma(&white("convert.mp4", VideoAdvanced::default()));
    let tagged = first_luma(&white(
        "tag-only.mp4",
        VideoAdvanced {
            tag_only: true,
            ..VideoAdvanced::default()
        },
    ));
    assert!(converted >= 250, "full-range white is 255, not {converted}");
    assert!(
        (233..=237).contains(&tagged),
        "limited-range white is 235, not {tagged}"
    );
}

#[test]
fn hdr_transfers_are_unsupported() {
    let dir = TempDir::new("adv-hdr");
    for transfer in ["smpte2084", "arib-std-b67"] {
        let mut v = video_spec("hevc");
        v.color = tags("bt2020", transfer, "bt2020nc", ColorRange::Limited);
        let e = encode_video(&dir, "hdr.mp4", v, 2).unwrap_err();
        assert!(matches!(e, MediaError::Unsupported { .. }), "{e:?}");
    }
}

#[test]
fn settings_an_encoder_cannot_take_are_refused_when_it_opens() {
    let dir = TempDir::new("adv-refused");
    let refused = |codec: &str, f: &dyn Fn(&mut VideoEncodeSpec)| {
        let mut v = video_spec(codec);
        f(&mut v);
        let spec = EncodeSpec::new(dir.path().join("x.mp4"))
            .with_video(v)
            .with_hw(HwEncode::Off);
        let e = backend().open_encoder(&spec).err();
        assert!(
            matches!(e, Some(MediaError::Unsupported { .. })),
            "{codec}: {e:?}"
        );
    };
    refused("av1", &|v| v.rate_control = RateControl::Cbr(4_000_000));
    refused("av1", &|v| v.advanced.bframes = Some(2));
    refused("av1", &|v| v.advanced.keyint_min = Some(2));
    refused("av1", &|v| v.advanced.level = Some("4.1".to_owned()));
    refused("prores", &|v| v.advanced.keyint = Some(10));
    // An unknown tune is the encoder's own refusal.
    refused("h264", &|v| v.advanced.tune = Some("nonsense".to_owned()));
}

// ----- audio -----------------------------------------------------------------------------------

/// White noise, `channels` planes, deterministic.
fn noise_chunk(rate: SampleRate, channels: u16, first: i64, len: usize) -> AudioChunk {
    let mut state = 0x1234_5678_9abc_def1_u64 ^ (first as u64).wrapping_mul(0x9e37_79b9);
    AudioChunk {
        pts: rate.samples_to_time(first).unwrap(),
        rate,
        channels,
        planes: (0..channels)
            .map(|_| {
                (0..len)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        ((state % 2001) as f32 - 1000.0) / 2500.0
                    })
                    .collect()
            })
            .collect(),
    }
}

/// Encodes 4 s of noise audio with `codec` and a few frames of video into `name`.
fn encode_audio(
    dir: &TempDir,
    name: &str,
    codec: &str,
    rate: SampleRate,
    channels: u16,
    bitrate: u32,
) -> PathBuf {
    let path = dir.path().join(name);
    let audio = AudioEncodeSpec {
        codec: codec.to_owned(),
        bitrate,
        ..AudioEncodeSpec::aac(rate, channels)
    };
    let spec = EncodeSpec::new(&path)
        .with_video(video_spec("h264"))
        .with_audio(audio)
        .with_hw(HwEncode::Off);
    let mut enc = backend()
        .open_encoder(&spec)
        .unwrap_or_else(|e| panic!("{codec}: open: {e}"));
    let per_frame = (rate.hz() / 30) as usize;
    for n in 0..120_u64 {
        enc.push_video(&frame(n, 5)).unwrap();
        enc.push_audio(&noise_chunk(
            rate,
            channels,
            n as i64 * per_frame as i64,
            per_frame,
        ))
        .unwrap_or_else(|e| panic!("{codec}: push_audio: {e}"));
    }
    enc.finish()
        .unwrap_or_else(|e| panic!("{codec}: finish: {e}"));
    path
}

/// The audio stream's measured bitrate in bits per second.
fn audio_bitrate(path: &Path) -> f64 {
    let packets = packets(path, "a");
    let bits: u64 = packets.iter().map(|p| p.bytes * 8).sum();
    bits as f64 / 4.0
}

#[test]
fn aac_192_and_opus_128_read_back_within_ten_percent() {
    let dir = TempDir::new("adv-audio-rates");
    for (codec, name, kbps) in [
        ("aac", "aac", 192.0),
        ("opus", "opus", 128.0),
        ("mp3", "mp3", 192.0),
    ] {
        let path = encode_audio(
            &dir,
            &format!("{codec}.mp4"),
            codec,
            SampleRate::HZ_48_000,
            2,
            (kbps * 1000.0) as u32,
        );
        let info = ffprobe(&["-show_streams", "-select_streams", "a:0"], &path);
        assert_eq!(get(&info, "streams_stream_0_codec_name"), name);
        assert_eq!(get(&info, "streams_stream_0_channels"), "2");
        let measured = audio_bitrate(&path) / 1000.0;
        assert!(
            (measured / kbps - 1.0).abs() <= 0.10,
            "{codec}: {measured:.1} kb/s, wanted {kbps} +- 10 %"
        );
    }
}

#[test]
fn audio_codecs_rates_and_channels() {
    let dir = TempDir::new("adv-audio-formats");
    // (codec, file, rate, channels, codec_name, sample_fmt contains)
    let cases = [
        ("flac", "mp4", SampleRate::HZ_44_100, 2_u16, "flac"),
        ("flac", "mp4", SampleRate::HZ_96_000, 1, "flac"),
        ("aac", "mp4", SampleRate::HZ_44_100, 1, "aac"),
        ("aac", "mp4", SampleRate::HZ_96_000, 2, "aac"),
        ("mp3", "mp4", SampleRate::HZ_44_100, 2, "mp3"),
        ("pcm_s16le", "mov", SampleRate::HZ_44_100, 2, "pcm_s16le"),
        ("pcm_s24le", "mov", SampleRate::HZ_96_000, 1, "pcm_s24le"),
    ];
    for (codec, ext, rate, channels, name) in cases {
        let file = format!("{codec}-{}-{channels}.{ext}", rate.hz());
        let path = dir.path().join(&file);
        let audio = AudioEncodeSpec {
            codec: codec.to_owned(),
            ..AudioEncodeSpec::aac(rate, channels)
        };
        let spec = EncodeSpec::new(&path)
            .with_video(video_spec("h264"))
            .with_audio(audio)
            .with_hw(HwEncode::Off);
        let mut enc = backend()
            .open_encoder(&spec)
            .unwrap_or_else(|e| panic!("{file}: open: {e}"));
        let per_frame = (rate.hz() / 30) as usize;
        for n in 0..30_u64 {
            enc.push_video(&frame(n, 5)).unwrap();
            enc.push_audio(&noise_chunk(
                rate,
                channels,
                n as i64 * per_frame as i64,
                per_frame,
            ))
            .unwrap_or_else(|e| panic!("{file}: push_audio: {e}"));
        }
        enc.finish()
            .unwrap_or_else(|e| panic!("{file}: finish: {e}"));
        let info = ffprobe(&["-show_streams", "-select_streams", "a:0"], &path);
        let a = |k: &str| get(&info, &format!("streams_stream_0_{k}")).to_owned();
        assert_eq!(a("codec_name"), name, "{file}");
        assert_eq!(a("sample_rate"), rate.hz().to_string(), "{file}");
        assert_eq!(a("channels"), channels.to_string(), "{file}");
        // The file decodes without a complaint.
        let out = Command::new(tool("ffmpeg"))
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-f", "null", "-"])
            .output()
            .unwrap();
        assert!(
            out.status.success() && out.stderr.is_empty(),
            "{file}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn opus_and_flac_stay_out_of_mov_and_pcm_out_of_mp4() {
    let dir = TempDir::new("adv-audio-refused");
    for (codec, ext) in [("opus", "mov"), ("flac", "mov"), ("pcm_s16le", "mp4")] {
        let audio = AudioEncodeSpec {
            codec: codec.to_owned(),
            ..AudioEncodeSpec::aac(SampleRate::HZ_48_000, 2)
        };
        let spec = EncodeSpec::new(dir.path().join(format!("x.{ext}")))
            .with_video(video_spec("h264"))
            .with_audio(audio)
            .with_hw(HwEncode::Off);
        let e = backend().open_encoder(&spec).err();
        assert!(
            matches!(e, Some(MediaError::Unsupported { .. })),
            "{codec} in {ext}: {e:?}"
        );
    }
}
