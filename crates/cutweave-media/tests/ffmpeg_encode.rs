// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1E.01 acceptance: the FFmpeg encoder writes H.264 + AAC MP4 that `ffprobe` and `ffmpeg`
//! read back cleanly. Inputs are generated here (no media files, no network); outputs go to a
//! temporary directory. `ffprobe` / `ffmpeg` come from the bundle (`$FFMPEG_DIR/bin`, else
//! `PATH`), as set up by `scripts/ffmpeg/env.sh` (docs/dev/ffmpeg.md).
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
    MediaBackend, MediaError, PixelFormat, Plane, ProgressSink, RateControl, SpeedPreset,
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

/// The acceptance criterion: 5 s of synthetic 1080p30 RGBA frames and a 440 Hz tone.
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
        .with_progress({
            let reports = Arc::clone(&reports);
            ProgressSink::new(move |p| reports.lock().unwrap().push(*p))
        });
    let mut enc = backend().open_encoder(&spec).expect("open");
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

    let info = probe(&path);
    eprintln!("{info:#?}");
    // Video: H.264 High, 1080p, exactly 30/1, yuv420p, BT.709 limited range.
    assert_eq!(get(&info, "streams_stream_0_codec_type"), "video");
    assert_eq!(get(&info, "streams_stream_0_codec_name"), "h264");
    assert_eq!(get(&info, "streams_stream_0_width"), "1920");
    assert_eq!(get(&info, "streams_stream_0_height"), "1080");
    assert_eq!(get(&info, "streams_stream_0_pix_fmt"), "yuv420p");
    assert_eq!(get(&info, "streams_stream_0_r_frame_rate"), "30/1");
    assert_eq!(get(&info, "streams_stream_0_avg_frame_rate"), "30/1");
    assert_eq!(get(&info, "streams_stream_0_nb_frames"), "150");
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
    // Duration 5.00 s ± 1 frame; A/V start offset < 40 ms.
    let frame = 1.0 / 30.0;
    let duration = seconds(&info, "format_duration");
    assert!((duration - 5.0).abs() <= frame, "duration {duration}");
    for s in 0..2 {
        let d = seconds(&info, &format!("streams_stream_{s}_duration"));
        assert!((d - 5.0).abs() <= frame, "stream {s} duration {d}");
    }
    let offset = (seconds(&info, "streams_stream_0_start_time")
        - seconds(&info, "streams_stream_1_start_time"))
    .abs();
    assert!(offset < 0.040, "A/V start offset {offset} s");
    // Interleaved: the streams alternate through the file, never more than a second apart.
    let (switches, spread) = interleave(&path);
    eprintln!("interleave: {switches} stream switches, at most {spread:.3} s apart");
    assert!(switches >= 50, "{switches} stream switches");
    assert!(
        spread <= 1.0,
        "video and audio {spread} s apart in the file"
    );

    // The container: `moov` before `mdat` (+faststart) and a BT.709 limited `colr` box.
    let bytes = std::fs::read(&path).unwrap();
    let boxes = top_level_boxes(&bytes);
    let kinds: Vec<&str> = boxes.iter().map(|(k, _)| k.as_str()).collect();
    let pos = |k: &str| kinds.iter().position(|x| *x == k).expect(k);
    assert_eq!(kinds[0], "ftyp", "{kinds:?}");
    assert!(pos("moov") < pos("mdat"), "faststart: {kinds:?}");
    let moov = &bytes[boxes[pos("moov")].1.clone()];
    assert_eq!(colr_nclx(moov), (1, 1, 1, false), "colr: BT.709, limited");

    assert_decodes_cleanly(&path);
}

/// Solid RGB frames decode back to BT.709 limited-range YUV (BT.601 would put red at Y 81).
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
        let spec = EncodeSpec::new(&path).with_video(VideoEncodeSpec {
            rate_control: RateControl::Crf(0),
            ..VideoEncodeSpec::h264(w, h, FrameRate::FPS_25, PixelFormat::Rgba8)
        });
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
