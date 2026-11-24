// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1E.01 acceptance: the FFmpeg encoder writes H.264 + AAC MP4 that `ffprobe` and `ffmpeg`
//! read back cleanly. Inputs are generated here (no media files, no network); outputs go to a
//! temporary directory. `ffprobe` / `ffmpeg` come from the bundle (`$FFMPEG_DIR/bin`, else
//! `PATH`), as set up by `scripts/ffmpeg/env.sh` (docs/dev/ffmpeg.md).
//!
//! WP-1E.02 acceptance (at the end): every hardware encoder the machine has passes the same
//! checks on 10 s of 1080p30 at twice realtime or better, `hw: Off` and a hardware encoder that
//! fails produce x264 output, and the quality table of docs/reference/export-quality.md. Those
//! need a GPU: `#[ignore]`d, and they skip with a message unless `CUTWEAVE_HW_TESTS=1`
//! (`CUTWEAVE_HW_TESTS=1 cargo nextest run -p cutweave-media --features ffmpeg --run-ignored
//! all`). The quality table also needs the corpus (`CUTWEAVE_CORPUS_DIR` or `testdata/corpus`).
#![cfg(feature = "ffmpeg")]
#![allow(clippy::expect_used, clippy::unwrap_used)] // test helpers

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cutweave_media::testing::contract::TempDir;
use cutweave_media::testing::{FakeAudio, FakeVideo};
use cutweave_media::{
    AudioChunk, AudioEncodeSpec, CancelCheck, EncodeProgress, EncodeSpec, Encoder, FfmpegBackend,
    HwEncode, MediaBackend, MediaError, PixelFormat, Plane, ProgressSink, RateControl, SpeedPreset,
    VideoEncodeSpec, VideoFrame,
};
use cutweave_model::{
    ColorRange, ColorTags, Duration, FrameRate, MatrixCoefficients, SampleRate, Time,
};

fn backend() -> FfmpegBackend {
    FfmpegBackend::new().expect("the loaded FFmpeg matches the build")
}

/// `ffprobe` or `ffmpeg` from the bundle the tests link against.
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

/// `ffprobe -show_format -show_streams` as flat `key → value` pairs
/// (`streams_stream_0_codec_name → h264`).
fn probe(path: &Path) -> BTreeMap<String, String> {
    let out = Command::new(tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "flat=s=_",
        ])
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
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.trim_matches('"').to_owned()))
        .collect()
}

fn get<'a>(info: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    info.get(key).map_or_else(
        || panic!("ffprobe has no `{key}`: {info:#?}"),
        String::as_str,
    )
}

fn seconds(info: &BTreeMap<String, String>, key: &str) -> f64 {
    get(info, key).parse().expect(key)
}

/// `ffmpeg -v error -i <path> -f null -` exits 0 with nothing on stderr.
fn assert_decodes_cleanly(path: &Path) {
    let out = Command::new(tool("ffmpeg"))
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg exit {:?}", out.status);
    assert!(
        out.stderr.is_empty(),
        "ffmpeg stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// How often the stream changes walking the packets in file order (ffprobe `pos`), and the
/// largest distance in seconds between the latest video and audio packet written so far.
fn interleave(path: &Path) -> (usize, f64) {
    let out = Command::new(tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_entries",
            "packet=stream_index,pos,pts_time",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .expect("run ffprobe");
    assert!(out.status.success());
    let mut packets: Vec<(u64, usize, f64)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.split(',');
            let stream = f.next()?.parse().ok()?;
            let pts = f.next()?.parse().ok()?;
            let pos = f.next()?.parse().ok()?;
            Some((pos, stream, pts))
        })
        .collect();
    packets.sort_by_key(|p| p.0);
    let switches = packets.windows(2).filter(|w| w[0].1 != w[1].1).count();
    let mut latest = [0.0_f64; 2];
    let mut spread = 0.0_f64;
    for &(_, stream, pts) in &packets {
        latest[stream] = latest[stream].max(pts);
        if latest[0] > 0.0 && latest[1] > 0.0 {
            spread = spread.max((latest[0] - latest[1]).abs());
        }
    }
    (switches, spread)
}

/// The top-level MP4 boxes, in file order.
fn top_level_boxes(bytes: &[u8]) -> Vec<(String, std::ops::Range<usize>)> {
    let mut boxes = Vec::new();
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let size32 = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        let kind = String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned();
        let size = match size32 {
            0 => bytes.len() - at,
            1 => usize::try_from(u64::from_be_bytes(
                bytes[at + 8..at + 16].try_into().unwrap(),
            ))
            .unwrap(),
            n => n as usize,
        };
        assert!(
            size >= 8 && at + size <= bytes.len(),
            "bad box {kind} at {at}"
        );
        boxes.push((kind, at..at + size));
        at += size;
    }
    boxes
}

/// The `colr` box of the video sample entry: `(primaries, transfer, matrix, full_range)`.
fn colr_nclx(moov: &[u8]) -> (u16, u16, u16, bool) {
    let at = moov
        .windows(8)
        .position(|w| w == b"colrnclx")
        .expect("a colr nclx box in moov");
    let b = &moov[at + 8..at + 15];
    let u16_at = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]);
    (u16_at(0), u16_at(2), u16_at(4), b[6] & 0x80 != 0)
}

/// Pushes `frames` video frames from `picture` and the matching 48 kHz audio from `tone`, in
/// step (one frame's worth of samples after each frame).
fn push_in_step(
    enc: &mut dyn Encoder,
    frames: u64,
    rate: FrameRate,
    picture: impl Fn(u64) -> VideoFrame,
    tone: Option<&FakeAudio>,
) {
    let mut sent = 0_u64;
    for n in 0..frames {
        enc.push_video(&picture(n)).expect("push_video");
        let Some(tone) = tone else { continue };
        let end: Time = rate.frame_to_time(i64::try_from(n + 1).unwrap()).unwrap();
        let until = u64::try_from(
            tone.rate
                .time_to_samples(end, cutweave_model::Rounding::Floor),
        )
        .unwrap()
        .min(tone.samples);
        enc.push_audio(&chunk(tone, sent, until - sent))
            .expect("push_audio");
        sent = until;
    }
}

/// Samples `first..first + n` of `tone`.
fn chunk(tone: &FakeAudio, first: u64, n: u64) -> AudioChunk {
    AudioChunk {
        pts: tone
            .rate
            .samples_to_time(i64::try_from(first).unwrap())
            .unwrap(),
        rate: tone.rate,
        channels: tone.channels,
        planes: (0..tone.channels)
            .map(|c| (first..first + n).map(|i| tone.sample(c, i)).collect())
            .collect(),
    }
}

/// The acceptance criterion: 5 s of synthetic 1080p30 RGBA frames and a 440 Hz tone, through
/// x264 (`hw: Off`; the hardware encoders run the same checks below).
#[test]
fn five_seconds_of_1080p30_and_a_tone_make_a_clean_mp4() {
    let dir = TempDir::new("accept");
    let path = dir.path().join("out.mp4");
    let rate = FrameRate::FPS_30;
    let five = Duration::from_secs(5).unwrap();
    let video = FakeVideo::lasting(1920, 1080, rate, five).with_format(PixelFormat::Rgba8);
    assert_eq!(video.frames, 150);
    // Channel 0 at 440 Hz, channel 1 at 880 Hz, −6 dBFS.
    let tone = FakeAudio::tone(SampleRate::HZ_48_000, 2, five);
    let reports = Arc::new(Mutex::new(Vec::<EncodeProgress>::new()));
    let spec = EncodeSpec::new(&path)
        .with_video(VideoEncodeSpec::h264(1920, 1080, rate, PixelFormat::Rgba8))
        .with_audio(AudioEncodeSpec::aac(SampleRate::HZ_48_000, 2))
        .with_hw(HwEncode::Off)
        .with_progress({
            let reports = Arc::clone(&reports);
            ProgressSink::new(move |p| reports.lock().unwrap().push(*p))
        });
    let mut enc = backend().open_encoder(&spec).expect("open");
    assert_eq!(enc.video_encoder(), Some("libx264"));
    push_in_step(
        &mut *enc,
        video.frames,
        rate,
        |n| video.frame(n, PixelFormat::Rgba8).unwrap(),
        Some(&tone),
    );
    let summary = enc.finish().expect("finish");
    assert_eq!(summary.video_frames, 150);
    assert_eq!(summary.audio_samples, 240_000);
    assert_eq!(summary.duration, five);
    let reports = reports.lock().unwrap().clone();
    let last = reports.last().expect("progress");
    assert_eq!((last.video_frames, last.encoded), (150, five));
    assert_eq!(
        last.bytes,
        std::fs::metadata(&path).unwrap().len(),
        "the last report has the file size"
    );
    // Only the output is left: no partial file.
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["out.mp4"]);
    assert_clean_1080p30_mp4(&path, 150);
}

/// The acceptance checks of WP-1E.01 on `path`: `frames` frames of H.264 High 1080p30 yuv420p
/// in BT.709 limited range (VUI and `colr`), 48 kHz stereo AAC at about 192 kb/s, durations
/// within a frame, the streams interleaved, `moov` first, and a clean decode.
fn assert_clean_1080p30_mp4(path: &Path, frames: u64) {
    let info = probe(path);
    eprintln!("{info:#?}");
    // Video: H.264 High, 1080p, exactly 30/1, yuv420p, BT.709 limited range.
    assert_eq!(get(&info, "streams_stream_0_codec_type"), "video");
    assert_eq!(get(&info, "streams_stream_0_codec_name"), "h264");
    assert_eq!(get(&info, "streams_stream_0_profile"), "High");
    assert_eq!(get(&info, "streams_stream_0_width"), "1920");
    assert_eq!(get(&info, "streams_stream_0_height"), "1080");
    assert_eq!(get(&info, "streams_stream_0_pix_fmt"), "yuv420p");
    assert_eq!(get(&info, "streams_stream_0_r_frame_rate"), "30/1");
    assert_eq!(get(&info, "streams_stream_0_avg_frame_rate"), "30/1");
    assert_eq!(get(&info, "streams_stream_0_nb_frames"), frames.to_string());
    assert_eq!(get(&info, "streams_stream_0_color_range"), "tv");
    assert_eq!(get(&info, "streams_stream_0_color_space"), "bt709");
    assert_eq!(get(&info, "streams_stream_0_color_transfer"), "bt709");
    assert_eq!(get(&info, "streams_stream_0_color_primaries"), "bt709");
    // Audio: AAC LC, 48 kHz stereo, about 192 kb/s.
    assert_eq!(get(&info, "streams_stream_1_codec_type"), "audio");
    assert_eq!(get(&info, "streams_stream_1_codec_name"), "aac");
    assert_eq!(get(&info, "streams_stream_1_sample_rate"), "48000");
    assert_eq!(get(&info, "streams_stream_1_channels"), "2");
    let kbps = seconds(&info, "streams_stream_1_bit_rate") / 1000.0;
    assert!((150.0..=230.0).contains(&kbps), "audio at {kbps} kb/s");
    // Duration ± 1 frame; A/V start offset < 40 ms.
    let frame = 1.0 / 30.0;
    #[allow(clippy::cast_precision_loss)] // a few hundred frames
    let want = frames as f64 / 30.0;
    let duration = seconds(&info, "format_duration");
    assert!((duration - want).abs() <= frame, "duration {duration}");
    for s in 0..2 {
        let d = seconds(&info, &format!("streams_stream_{s}_duration"));
        assert!((d - want).abs() <= frame, "stream {s} duration {d}");
    }
    let offset = (seconds(&info, "streams_stream_0_start_time")
        - seconds(&info, "streams_stream_1_start_time"))
    .abs();
    assert!(offset < 0.040, "A/V start offset {offset} s");
    // Interleaved: the streams alternate through the file, never more than a second apart.
    let (switches, spread) = interleave(path);
    eprintln!("interleave: {switches} stream switches, at most {spread:.3} s apart");
    assert!(switches >= 50, "{switches} stream switches");
    assert!(
        spread <= 1.0,
        "video and audio {spread} s apart in the file"
    );

    // The container: `moov` before `mdat` (+faststart) and a BT.709 limited `colr` box.
    let bytes = std::fs::read(path).unwrap();
    let boxes = top_level_boxes(&bytes);
    let kinds: Vec<&str> = boxes.iter().map(|(k, _)| k.as_str()).collect();
    let pos = |k: &str| kinds.iter().position(|x| *x == k).expect(k);
    assert_eq!(kinds[0], "ftyp", "{kinds:?}");
    assert!(pos("moov") < pos("mdat"), "faststart: {kinds:?}");
    let moov = &bytes[boxes[pos("moov")].1.clone()];
    assert_eq!(colr_nclx(moov), (1, 1, 1, false), "colr: BT.709, limited");

    assert_decodes_cleanly(path);
}

/// Solid RGB frames decode back to BT.709 limited-range YUV (BT.601 would put red at Y 81).
/// Lossless x264 (`Crf(0)`, `hw: Off`), so only the conversion is measured.
#[test]
fn rgb_colors_survive_as_bt709_limited_range() {
    let dir = TempDir::new("colors");
    let (w, h) = (64_u32, 64_u32);
    for (rgb, want) in [
        ([255_u8, 0, 0], [63.0, 102.0, 240.0]),
        ([0, 255, 0], [173.0, 42.0, 26.0]),
        ([255, 255, 255], [235.0, 128.0, 128.0]),
    ] {
        let path = dir
            .path()
            .join(format!("{}-{}-{}.mp4", rgb[0], rgb[1], rgb[2]));
        let spec = EncodeSpec::new(&path)
            .with_video(VideoEncodeSpec {
                rate_control: RateControl::Crf(0),
                ..VideoEncodeSpec::h264(w, h, FrameRate::FPS_25, PixelFormat::Rgba8)
            })
            .with_hw(HwEncode::Off);
        let mut enc = backend().open_encoder(&spec).expect("open");
        for n in 0..5 {
            enc.push_video(&VideoFrame {
                pts: FrameRate::FPS_25.frame_to_time(n).unwrap(),
                width: w,
                height: h,
                format: PixelFormat::Rgba8,
                planes: vec![Plane {
                    data: [rgb[0], rgb[1], rgb[2], 255].repeat((w * h) as usize),
                    stride: 4 * w,
                }],
                color: ColorTags::default(),
                damaged: false,
            })
            .expect("push");
        }
        enc.finish().expect("finish");
        let out = Command::new(tool("ffmpeg"))
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args([
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "yuv420p",
                "-",
            ])
            .output()
            .expect("run ffmpeg");
        assert!(out.status.success() && out.stderr.is_empty());
        let (luma, chroma) = ((w * h) as usize, (w * h / 4) as usize);
        assert_eq!(out.stdout.len(), luma + 2 * chroma);
        #[allow(clippy::cast_precision_loss)]
        let mean = |p: &[u8]| p.iter().map(|&v| f64::from(v)).sum::<f64>() / p.len() as f64;
        let got = [
            mean(&out.stdout[..luma]),
            mean(&out.stdout[luma..luma + chroma]),
            mean(&out.stdout[luma + chroma..]),
        ];
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() <= 1.5, "{rgb:?}: got {got:?}, want {want:?}");
        }
    }
}

/// NV12 input (the GPU readback layout), a bitrate target, another preset, an NTSC rate, mono
/// audio, and video-only and audio-only outputs all make valid files.
#[test]
fn other_inputs_and_settings_make_valid_files() {
    let dir = TempDir::new("variants");
    let rate = FrameRate::FPS_29_97;
    let two = Duration::from_secs(2).unwrap();
    let video = FakeVideo::lasting(320, 180, rate, two);
    let tone = FakeAudio::tone(SampleRate::HZ_44_100, 1, two);
    let nv12 = |n: u64| {
        let mut f = video.frame(n, PixelFormat::Nv12).unwrap();
        f.color.matrix = Some(MatrixCoefficients::Bt709);
        f.color.range = Some(ColorRange::Limited);
        f
    };

    let av = dir.path().join("nv12.mp4");
    let spec = EncodeSpec::new(&av)
        .with_video(VideoEncodeSpec {
            rate_control: RateControl::Bitrate(400_000),
            preset: SpeedPreset::Veryfast,
            ..VideoEncodeSpec::h264(320, 180, rate, PixelFormat::Nv12)
        })
        .with_audio(AudioEncodeSpec {
            bitrate: 96_000,
            ..AudioEncodeSpec::aac(SampleRate::HZ_44_100, 1)
        });
    let mut enc = backend().open_encoder(&spec).expect("open");
    push_in_step(&mut *enc, video.frames, rate, nv12, Some(&tone));
    enc.finish().expect("finish");
    let info = probe(&av);
    assert_eq!(get(&info, "streams_stream_0_codec_name"), "h264");
    assert_eq!(get(&info, "streams_stream_0_r_frame_rate"), "30000/1001");
    assert_eq!(get(&info, "streams_stream_0_nb_frames"), "60");
    assert_eq!(get(&info, "streams_stream_1_sample_rate"), "44100");
    assert_eq!(get(&info, "streams_stream_1_channels"), "1");
    assert_decodes_cleanly(&av);

    let silent = dir.path().join("video-only.mp4");
    let spec = EncodeSpec::new(&silent).with_video(VideoEncodeSpec::h264(
        320,
        180,
        rate,
        PixelFormat::Yuv420p,
    ));
    let mut enc = backend().open_encoder(&spec).expect("open");
    push_in_step(
        &mut *enc,
        10,
        rate,
        |n| video.frame(n, PixelFormat::Yuv420p).unwrap(),
        None,
    );
    enc.finish().expect("finish");
    let info = probe(&silent);
    assert_eq!(get(&info, "format_nb_streams"), "1");
    assert_decodes_cleanly(&silent);

    let sound = dir.path().join("audio-only.m4v");
    let spec = EncodeSpec::new(&sound).with_audio(AudioEncodeSpec::aac(SampleRate::HZ_44_100, 1));
    let mut enc = backend().open_encoder(&spec).expect("open");
    enc.push_audio(&chunk(&tone, 0, tone.samples))
        .expect("push");
    let summary = enc.finish().expect("finish");
    assert_eq!(summary.duration, two);
    let info = probe(&sound);
    assert_eq!(get(&info, "format_nb_streams"), "1");
    assert_eq!(get(&info, "streams_stream_0_codec_name"), "aac");
    let d = seconds(&info, "format_duration");
    assert!((d - 2.0).abs() < 0.03, "audio-only duration {d}");
    assert_decodes_cleanly(&sound);
}

/// A cancel that arrives while `finish` flushes stops it: no output, no partial file, and the
/// file that was there before is untouched. A successful encode replaces it.
#[test]
fn cancelling_during_finish_leaves_the_old_file_and_success_replaces_it() {
    let dir = TempDir::new("cancel-finish");
    let path = dir.path().join("out.mp4");
    std::fs::write(&path, b"the previous export").unwrap();
    let rate = FrameRate::FPS_30;
    let video = FakeVideo::new(640, 360, rate, 60).with_format(PixelFormat::Rgba8);

    let finishing = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let spec = EncodeSpec::new(&path)
        .with_video(VideoEncodeSpec::h264(640, 360, rate, PixelFormat::Rgba8))
        .with_cancel(CancelCheck::from_flag(Arc::clone(&cancel)))
        .with_progress({
            let (finishing, cancel) = (Arc::clone(&finishing), Arc::clone(&cancel));
            // The first report from inside `finish` (after the encoders flushed) cancels.
            ProgressSink::new(move |_| {
                if finishing.load(Ordering::Relaxed) {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
        });
    let mut enc = backend().open_encoder(&spec).expect("open");
    push_in_step(
        &mut *enc,
        60,
        rate,
        |n| video.frame(n, PixelFormat::Rgba8).unwrap(),
        None,
    );
    finishing.store(true, Ordering::Relaxed);
    assert_eq!(enc.finish().err(), Some(MediaError::Cancelled));
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["out.mp4"]);
    assert_eq!(std::fs::read(&path).unwrap(), b"the previous export");

    let spec = EncodeSpec::new(&path).with_video(VideoEncodeSpec::h264(
        640,
        360,
        rate,
        PixelFormat::Rgba8,
    ));
    let mut enc = backend().open_encoder(&spec).expect("open");
    push_in_step(
        &mut *enc,
        30,
        rate,
        |n| video.frame(n, PixelFormat::Rgba8).unwrap(),
        None,
    );
    enc.finish().expect("finish");
    assert_eq!(get(&probe(&path), "streams_stream_0_nb_frames"), "30");
}

/// Outputs this backend does not write, with what makes each so.
fn unsupported_specs(dir: &Path) -> Vec<(&'static str, EncodeSpec)> {
    let h264 = VideoEncodeSpec::h264(64, 36, FrameRate::FPS_30, PixelFormat::Rgba8);
    let aac = AudioEncodeSpec::aac(SampleRate::HZ_48_000, 2);
    let at = |name: &str| EncodeSpec::new(dir.join(name));
    vec![
        ("MKV", at("out.mkv").with_video(h264.clone())),
        (
            "MOV container",
            EncodeSpec {
                container: Some("mov".into()),
                ..at("out.mp4").with_video(h264.clone())
            },
        ),
        (
            "HEVC",
            at("out.mp4").with_video(VideoEncodeSpec {
                codec: "hevc".into(),
                ..h264.clone()
            }),
        ),
        (
            "P010 input",
            at("out.mp4").with_video(VideoEncodeSpec {
                input_format: PixelFormat::P010,
                ..h264.clone()
            }),
        ),
        (
            "BT.601 output",
            at("out.mp4").with_video(VideoEncodeSpec {
                color: ColorTags {
                    matrix: Some(MatrixCoefficients::Smpte170M),
                    ..ColorTags::default()
                },
                ..h264.clone()
            }),
        ),
        (
            "full-range output",
            at("out.mp4").with_video(VideoEncodeSpec {
                color: ColorTags {
                    range: Some(ColorRange::Full),
                    ..ColorTags::default()
                },
                ..h264.clone()
            }),
        ),
        (
            "5.1 audio",
            at("out.mp4").with_audio(AudioEncodeSpec {
                channels: 6,
                ..aac.clone()
            }),
        ),
        (
            "AAC at 47 999 Hz",
            at("out.mp4").with_audio(AudioEncodeSpec {
                rate: SampleRate::new(47_999).unwrap(),
                ..aac.clone()
            }),
        ),
        (
            "MP3",
            at("out.mp4").with_audio(AudioEncodeSpec {
                codec: "mp3".into(),
                ..aac
            }),
        ),
    ]
}

/// What this backend cannot write is refused before any file exists.
#[test]
fn unsupported_and_invalid_outputs_are_refused_up_front() {
    let dir = TempDir::new("refused");
    let b = backend();
    for (what, spec) in unsupported_specs(dir.path()) {
        let err = b.open_encoder(&spec).err();
        assert!(
            matches!(err, Some(MediaError::Unsupported { .. })),
            "{what}: {err:?}"
        );
    }
    let h264 = VideoEncodeSpec::h264(64, 36, FrameRate::FPS_30, PixelFormat::Rgba8);
    let at = |name: &str| EncodeSpec::new(dir.path().join(name));
    let odd = at("out.mp4").with_video(VideoEncodeSpec {
        width: 63,
        ..h264.clone()
    });
    assert!(matches!(
        b.open_encoder(&odd).err(),
        Some(MediaError::Invalid { .. })
    ));
    let no_folder =
        EncodeSpec::new(dir.path().join("missing").join("out.mp4")).with_video(h264.clone());
    assert!(matches!(
        b.open_encoder(&no_folder).err(),
        Some(MediaError::InvalidPath { .. })
    ));
    std::fs::create_dir(dir.path().join("folder.mp4")).unwrap();
    let folder = at("folder.mp4").with_video(h264);
    assert!(matches!(
        b.open_encoder(&folder).err(),
        Some(MediaError::InvalidPath { .. })
    ));
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["folder.mp4"]);
}

// ===== WP-1E.02: hardware encoders ===========================================================

/// Whether the hardware cases run (`CUTWEAVE_HW_TESTS=1`); prints why not.
fn hw_tests() -> bool {
    let on = std::env::var("CUTWEAVE_HW_TESTS").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipping: set CUTWEAVE_HW_TESTS=1 on a machine with a hardware encoder");
    }
    on
}

/// x264 signs its streams with an SEI message (`x264 - core …`); hardware encoders do not.
fn written_by_x264(path: &Path) -> bool {
    std::fs::read(path)
        .unwrap()
        .windows(11)
        .any(|w| w == b"x264 - core")
}

/// 1080p30 tagged BT.709 limited (YUV input must not contradict the output).
fn ten_second_video() -> FakeVideo {
    FakeVideo::lasting(
        1920,
        1080,
        FrameRate::FPS_30,
        Duration::from_secs(10).unwrap(),
    )
    .with_color(ColorTags {
        matrix: Some(MatrixCoefficients::Bt709),
        range: Some(ColorRange::Limited),
        ..ColorTags::default()
    })
}

/// Encodes 10 s of 1080p30 `format` video (15 pictures, cycled) and a stereo tone to `path`
/// with `hw`, the way an export pushes them; returns the video encoder and the wall time from
/// open to finish.
fn encode_ten_seconds(
    b: &FfmpegBackend,
    path: &Path,
    hw: HwEncode,
    format: PixelFormat,
) -> (String, std::time::Duration) {
    let rate = FrameRate::FPS_30;
    let video = ten_second_video();
    let tone = FakeAudio::tone(SampleRate::HZ_48_000, 2, Duration::from_secs(10).unwrap());
    let mut pictures: Vec<VideoFrame> = (0..15).map(|n| video.frame(n, format).unwrap()).collect();
    let spec = EncodeSpec::new(path)
        .with_video(VideoEncodeSpec::h264(1920, 1080, rate, format))
        .with_audio(AudioEncodeSpec::aac(SampleRate::HZ_48_000, 2))
        .with_hw(hw);
    let start = std::time::Instant::now();
    let mut enc = b.open_encoder(&spec).expect("open");
    let name = enc.video_encoder().expect("a video encoder").to_owned();
    // As `push_in_step`, but re-stamping the cycled pictures instead of copying them.
    let mut sent = 0_u64;
    for n in 0..video.frames {
        let picture = &mut pictures[usize::try_from(n % 15).unwrap()];
        picture.pts = rate.frame_to_time(i64::try_from(n).unwrap()).unwrap();
        enc.push_video(picture).expect("push_video");
        let end = rate.frame_to_time(i64::try_from(n + 1).unwrap()).unwrap();
        let until = u64::try_from(
            tone.rate
                .time_to_samples(end, cutweave_model::Rounding::Floor),
        )
        .unwrap()
        .min(tone.samples);
        enc.push_audio(&chunk(&tone, sent, until - sent))
            .expect("push_audio");
        sent = until;
    }
    enc.finish().expect("finish");
    (name, start.elapsed())
}

/// WP-1E.02 acceptance: every hardware encoder found here writes 10 s of 1080p30 (RGBA, the
/// render readback, and NV12) that passes the WP-1E.01 checks, at twice realtime or better
/// (arch §19 export budget), and it is not x264's.
#[test]
#[ignore = "hardware: CUTWEAVE_HW_TESTS=1, --run-ignored all"]
fn each_hardware_encoder_writes_ten_seconds_of_1080p30_at_twice_realtime() {
    if !hw_tests() {
        return;
    }
    let b = backend();
    let found = b.capabilities().hw_encode;
    assert!(!found.is_empty(), "no hardware H.264 encoder found");
    let dir = TempDir::new("hw-accept");
    let mut lines = Vec::new();
    let mut slow = Vec::new();
    let runs = found
        .iter()
        .map(|hw| HwEncode::Force(hw.implementation.clone()))
        .chain([HwEncode::Off]);
    for hw in runs {
        for format in [PixelFormat::Rgba8, PixelFormat::Nv12] {
            let path = dir
                .path()
                .join(format!("{hw:?}-{format:?}.mp4").replace('"', ""));
            let (name, took) = encode_ten_seconds(&b, &path, hw.clone(), format);
            let speed = 10.0 / took.as_secs_f64();
            #[allow(clippy::cast_precision_loss)] // file sizes
            let kbps = std::fs::metadata(&path).unwrap().len() as f64 * 8.0 / 10.0 / 1000.0;
            lines.push(format!(
                "{name} {format:?}: {:.2} s for 10 s ({speed:.1}× realtime), {kbps:.0} kb/s",
                took.as_secs_f64()
            ));
            eprintln!("{}", lines.last().unwrap());
            assert_clean_1080p30_mp4(&path, 300);
            if let HwEncode::Force(want) = &hw {
                assert_eq!(&name, want);
                assert!(!written_by_x264(&path), "{name} output carries x264's SEI");
                if speed < 2.0 {
                    slow.push(lines.last().unwrap().clone());
                }
            } else {
                assert!(written_by_x264(&path));
            }
        }
    }
    eprintln!("\n{}", lines.join("\n"));
    assert!(slow.is_empty(), "below 2× realtime: {slow:#?}");
}

/// WP-1E.02 acceptance: `hw: Off`, and an `Auto` encode that every hardware encoder here
/// refuses (wider than the 4096 pixels H.264 hardware takes), are written by x264; forcing a
/// hardware encoder there fails with no file.
#[test]
#[ignore = "hardware: CUTWEAVE_HW_TESTS=1, --run-ignored all"]
fn off_and_a_failing_hardware_encoder_produce_x264_output() {
    if !hw_tests() {
        return;
    }
    let b = backend();
    let first = b
        .capabilities()
        .hw_encode
        .first()
        .cloned()
        .expect("a hardware encoder");
    let dir = TempDir::new("hw-fallback");

    let off = dir.path().join("off.mp4");
    let (name, _) = encode_ten_seconds(&b, &off, HwEncode::Off, PixelFormat::Rgba8);
    assert_eq!(name, "libx264");
    assert!(written_by_x264(&off));

    let (w, h) = (4160_u32, 240_u32);
    let wide = FakeVideo::new(w, h, FrameRate::FPS_30, 10);
    let spec = |path: &Path, hw: HwEncode| {
        EncodeSpec::new(path)
            .with_video(VideoEncodeSpec::h264(
                w,
                h,
                FrameRate::FPS_30,
                PixelFormat::Rgba8,
            ))
            .with_hw(hw)
    };
    let auto = dir.path().join("auto.mp4");
    let mut enc = b.open_encoder(&spec(&auto, HwEncode::Auto)).expect("open");
    assert_eq!(enc.video_encoder(), Some("libx264"));
    push_in_step(
        &mut *enc,
        10,
        FrameRate::FPS_30,
        |n| wide.frame(n, PixelFormat::Rgba8).unwrap(),
        None,
    );
    enc.finish().expect("finish");
    assert!(written_by_x264(&auto));
    assert_decodes_cleanly(&auto);
    assert_eq!(get(&probe(&auto), "streams_stream_0_nb_frames"), "10");

    let forced = dir.path().join("forced.mp4");
    let err = b
        .open_encoder(&spec(
            &forced,
            HwEncode::Force(first.implementation.clone()),
        ))
        .err();
    assert!(
        matches!(&err, Some(MediaError::HwUnavailable { reason }) if reason.contains(&first.implementation)),
        "{err:?}"
    );
    assert!(!forced.exists());
}

/// The corpus file `name`, or `None` with a skip message.
fn corpus_file(name: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("CUTWEAVE_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/corpus"),
        PathBuf::from,
    );
    let path = dir.join(name);
    if path.is_file() {
        Some(path)
    } else {
        eprintln!(
            "skipping: {name} is not in {} (cargo xtask fetch-corpus --tier small, or \
             CUTWEAVE_CORPUS_DIR)",
            dir.display()
        );
        None
    }
}

/// A 1080p30 reference made from a corpus clip by the `ffmpeg` CLI: input options and filter.
struct Reference {
    file: PathBuf,
    input: &'static [&'static str],
    filter: &'static str,
}

impl Reference {
    /// Encodes the reference (piped from `ffmpeg` as YUV 4:2:0) to `out` with `crf` and `hw`;
    /// returns the frame count.
    fn encode(&self, b: &FfmpegBackend, out: &Path, crf: u8, hw: HwEncode) -> u64 {
        use std::io::Read;
        let mut child = Command::new(tool("ffmpeg"))
            .args(["-v", "error"])
            .args(self.input)
            .arg("-i")
            .arg(&self.file)
            .args([
                "-vf",
                self.filter,
                "-pix_fmt",
                "yuv420p",
                "-f",
                "rawvideo",
                "-",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("run ffmpeg");
        let mut stdout = child.stdout.take().unwrap();
        let rate = FrameRate::FPS_30;
        let spec = EncodeSpec::new(out)
            .with_video(VideoEncodeSpec {
                rate_control: RateControl::Crf(crf),
                ..VideoEncodeSpec::h264(1920, 1080, rate, PixelFormat::Yuv420p)
            })
            .with_hw(hw);
        let mut enc = b.open_encoder(&spec).expect("open");
        let (luma, chroma) = (1920 * 1080, 960 * 540);
        let mut buf = vec![0_u8; luma + 2 * chroma];
        let mut n = 0_i64;
        while stdout.read_exact(&mut buf).is_ok() {
            enc.push_video(&VideoFrame {
                pts: rate.frame_to_time(n).unwrap(),
                width: 1920,
                height: 1080,
                format: PixelFormat::Yuv420p,
                planes: vec![
                    Plane {
                        data: buf[..luma].to_vec(),
                        stride: 1920,
                    },
                    Plane {
                        data: buf[luma..luma + chroma].to_vec(),
                        stride: 960,
                    },
                    Plane {
                        data: buf[luma + chroma..].to_vec(),
                        stride: 960,
                    },
                ],
                color: ColorTags::default(),
                damaged: false,
            })
            .expect("push");
            n += 1;
        }
        assert!(child.wait().unwrap().success());
        enc.finish().expect("finish");
        u64::try_from(n).unwrap()
    }

    /// Mean luma PSNR of `out` against the reference, frame by frame. Both sides get the same
    /// color tags first: FFmpeg 8 negotiates color spaces in filter graphs, so a tagged output
    /// and an untagged reference would otherwise be converted to match (about 25 dB lost).
    fn psnr_y(&self, out: &Path) -> f64 {
        let tags = "setparams=range=tv:colorspace=bt709:color_primaries=bt709:color_trc=bt709";
        let graph = format!(
            "[1:v]{},format=yuv420p,{tags},settb=1/30,setpts=N[r];\
             [0:v]{tags},settb=1/30,setpts=N[e];[e][r]psnr",
            self.filter
        );
        let run = Command::new(tool("ffmpeg"))
            .args(["-hide_banner", "-i"])
            .arg(out)
            .args(self.input)
            .arg("-i")
            .arg(&self.file)
            .args(["-lavfi", &graph, "-f", "null", "-"])
            .output()
            .expect("run ffmpeg");
        let text = String::from_utf8_lossy(&run.stderr);
        let line = text
            .lines()
            .rfind(|l| l.contains("PSNR y:"))
            .unwrap_or_else(|| panic!("no PSNR in {text}"));
        line.split("PSNR y:")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("bad PSNR line {line}"))
    }
}

/// The quality table of docs/reference/export-quality.md: luma PSNR and bitrate of x264 and
/// every hardware encoder here at CRF 18, 20 and 23 (each encoder's mapped equivalent) on two
/// corpus clips scaled to 1080p30. Checks the mapping's claim: at CRF 20 every hardware encoder
/// is within 1 dB of x264 on the natural clip.
#[test]
#[ignore = "hardware + corpus: CUTWEAVE_HW_TESTS=1, --run-ignored all"]
fn export_quality_table() {
    if !hw_tests() {
        return;
    }
    let (Some(sintel), Some(testsrc)) = (
        corpus_file("sintel_trailer_480p.mp4"),
        corpus_file("h264_1080p60.mp4"),
    ) else {
        return;
    };
    let b = backend();
    let encoders: Vec<HwEncode> = [HwEncode::Off]
        .into_iter()
        .chain(
            b.capabilities()
                .hw_encode
                .into_iter()
                .map(|hw| HwEncode::Force(hw.implementation)),
        )
        .collect();
    let clips = [
        (
            "Sintel trailer, 480p upscaled to 1080p30, 15–25 s",
            Reference {
                file: sintel,
                input: &["-ss", "15", "-t", "10"],
                filter: "scale=1920:1080:flags=lanczos,fps=30",
            },
            &[18_u8, 20, 23][..],
        ),
        (
            "testsrc2 1080p60 at 30 fps, 8 s",
            Reference {
                file: testsrc,
                input: &[],
                filter: "fps=30",
            },
            &[20_u8][..],
        ),
    ];
    let dir = TempDir::new("quality");
    let mut table = vec![
        "| Clip | Encoder | CRF | PSNR-Y (dB) | kb/s |".to_owned(),
        "|---|---|---|---|---|".to_owned(),
    ];
    let mut at_20 = Vec::new();
    for (label, reference, crfs) in &clips {
        for &crf in *crfs {
            for hw in &encoders {
                let name = match hw {
                    HwEncode::Force(name) => name.clone(),
                    _ => "libx264".to_owned(),
                };
                let out = dir.path().join(format!("{name}-{crf}.mp4"));
                let frames = reference.encode(&b, &out, crf, hw.clone());
                let psnr = reference.psnr_y(&out);
                #[allow(clippy::cast_precision_loss)] // file sizes and frame counts
                let kbps = std::fs::metadata(&out).unwrap().len() as f64 * 8.0 * 30.0
                    / frames as f64
                    / 1000.0;
                let row = format!("| {label} | {name} | {crf} | {psnr:.2} | {kbps:.0} |");
                eprintln!("{row}");
                table.push(row);
                if crf == 20 && label.starts_with("Sintel") {
                    at_20.push((name, psnr));
                }
            }
        }
    }
    eprintln!("\n{}", table.join("\n"));
    let x264 = at_20[0].1;
    for (name, psnr) in &at_20[1..] {
        assert!(
            (psnr - x264).abs() <= 1.0,
            "{name} at CRF 20: {psnr:.2} dB vs x264 {x264:.2} dB"
        );
    }
}
