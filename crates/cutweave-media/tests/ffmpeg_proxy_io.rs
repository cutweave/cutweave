// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.01: what the proxy generator needs from the FFmpeg backend. Intra-frame encodes (ProRes
//! Proxy, MJPEG) with 16-bit PCM in MOV that read back with their size, layout, color tags and
//! length; decoding scaled to a smaller size in the encoder's layout; joining parts end to end
//! by copying packets. Inputs are synthetic (`FakeVideo` / `FakeAudio` pictures and tones), and
//! outputs go to a temporary directory: no media files, no network.
#![cfg(feature = "ffmpeg")]
#![allow(clippy::expect_used, clippy::unwrap_used)] // test helpers

use std::path::{Path, PathBuf};

use cutweave_media::testing::contract::TempDir;
use cutweave_media::testing::{FakeAudio, FakeVideo};
use cutweave_media::{
    AudioChunk, AudioEncodeSpec, CancelCheck, ConcatSpec, DecodeMode, DecodeOpts, EncodeSpec,
    FfmpegBackend, HwMode, MediaBackend, MediaError, PixelFormat, RateControl, SpeedPreset,
    VideoEncodeSpec,
};
use cutweave_model::{
    ColorPrimaries, ColorRange, ColorTags, FrameRate, MatrixCoefficients, SampleRate, StreamKind,
    Time, TransferCharacteristics,
};

fn backend() -> FfmpegBackend {
    FfmpegBackend::new().expect("the loaded FFmpeg matches the build")
}

fn hdr_tags() -> ColorTags {
    ColorTags {
        primaries: Some(ColorPrimaries::Bt2020),
        transfer: Some(TransferCharacteristics::Pq),
        matrix: Some(MatrixCoefficients::Bt2020Ncl),
        range: Some(ColorRange::Limited),
    }
}

/// An intra encode of `frames` frames of `w × h` at 25 fps with a stereo 48 kHz tone.
fn intra_spec(path: &Path, codec: &str, format: PixelFormat, (w, h): (u32, u32)) -> EncodeSpec {
    EncodeSpec::new(path)
        .with_video(VideoEncodeSpec {
            codec: codec.to_owned(),
            width: w,
            height: h,
            rate: FrameRate::FPS_25,
            input_format: format,
            rate_control: RateControl::default(),
            preset: SpeedPreset::default(),
            color: hdr_tags(),
            threads: 2,
        })
        .with_audio(AudioEncodeSpec {
            codec: "pcm_s16le".to_owned(),
            rate: SampleRate::HZ_48_000,
            channels: 2,
            bitrate: 1_536_000,
        })
}

/// Writes `frames` frames (and the matching tone) with `spec`; returns the path.
fn write(backend: &FfmpegBackend, spec: &EncodeSpec, frames: u64) -> PathBuf {
    let v = spec.video.as_ref().unwrap();
    let video = FakeVideo::new(v.width, v.height, v.rate, frames).with_color(v.color.clone());
    let tone = FakeAudio::tone(SampleRate::HZ_48_000, 2, video.duration().unwrap());
    let mut enc = backend.open_encoder(spec).unwrap();
    let per_frame = 48_000 / 25;
    for n in 0..frames {
        enc.push_video(&video.frame(n, v.input_format).unwrap())
            .unwrap();
        let first = n * per_frame;
        let planes = (0..2)
            .map(|c| {
                (first..first + per_frame)
                    .map(|i| tone.sample(c, i))
                    .collect()
            })
            .collect();
        enc.push_audio(&AudioChunk {
            pts: SampleRate::HZ_48_000
                .samples_to_time(i64::try_from(first).unwrap())
                .unwrap(),
            rate: SampleRate::HZ_48_000,
            channels: 2,
            planes,
        })
        .unwrap();
    }
    let summary = enc.finish().unwrap();
    assert_eq!(summary.video_frames, frames);
    assert_eq!(summary.audio_samples, frames * per_frame);
    spec.path.clone()
}

#[test]
fn prores_proxy_and_pcm_in_mov_read_back_with_their_tags() {
    let b = backend();
    let dir = TempDir::new("proxy-prores");
    let spec = intra_spec(
        &dir.path().join("a.mov"),
        "prores",
        PixelFormat::Yuv422p10,
        (320, 180),
    );
    let path = write(&b, &spec, 30);
    let probe = b.probe(&path).unwrap();
    let video = probe
        .streams
        .iter()
        .find(|s| s.kind == StreamKind::Video)
        .unwrap();
    assert_eq!(video.codec.as_deref(), Some("prores"));
    assert_eq!((video.width, video.height), (Some(320), Some(180)));
    assert_eq!(video.profile.as_deref(), Some("Proxy"));
    assert_eq!(probe.color.primaries, Some(ColorPrimaries::Bt2020));
    assert_eq!(probe.color.transfer, Some(TransferCharacteristics::Pq));
    assert_eq!(probe.color.matrix, Some(MatrixCoefficients::Bt2020Ncl));
    let audio = probe
        .streams
        .iter()
        .find(|s| s.kind == StreamKind::Audio)
        .unwrap();
    assert_eq!(audio.codec.as_deref(), Some("pcm_s16le"));
    assert_eq!(audio.channels, Some(2));
    // Every frame is a keyframe, and the whole clip decodes in its own layout.
    let index = b.index_media(&path).unwrap();
    assert_eq!(index.frame_count(), 30);
    assert_eq!(index.key_frames().count(), 30);
    let mut d = b
        .open_decoder(&path, &DecodeOpts::default().with_hw(HwMode::Off))
        .unwrap();
    let f = d.seek_exact(index.time_of(17).unwrap()).unwrap();
    assert_eq!(
        (f.width, f.height, f.format),
        (320, 180, PixelFormat::Yuv422p10)
    );
}

#[test]
fn mjpeg_writes_limited_range_422_and_bad_input_is_refused() {
    let b = backend();
    let dir = TempDir::new("proxy-mjpeg");
    let spec = intra_spec(
        &dir.path().join("m.mov"),
        "mjpeg",
        PixelFormat::Yuv422p,
        (160, 90),
    );
    let path = write(&b, &spec, 5);
    let probe = b.probe(&path).unwrap();
    assert_eq!(probe.streams[0].codec.as_deref(), Some("mjpeg"));
    assert_eq!(b.index_media(&path).unwrap().frame_count(), 5);
    // The intra encoders take their own layout only, and PCM only in MOV.
    let wrong = intra_spec(
        &dir.path().join("w.mov"),
        "prores",
        PixelFormat::Yuv420p,
        (160, 90),
    );
    assert!(matches!(
        b.open_encoder(&wrong),
        Err(MediaError::Unsupported { .. })
    ));
    let mut mp4 = intra_spec(
        &dir.path().join("p.mp4"),
        "prores",
        PixelFormat::Yuv422p10,
        (160, 90),
    );
    mp4.video = None;
    assert!(matches!(
        b.open_encoder(&mp4),
        Err(MediaError::Unsupported { .. })
    ));
}

#[test]
fn decoders_scale_to_the_target_size_in_the_target_layout() {
    let b = backend();
    let dir = TempDir::new("proxy-scale");
    let spec = intra_spec(
        &dir.path().join("src.mov"),
        "prores",
        PixelFormat::Yuv422p10,
        (640, 360),
    );
    let path = write(&b, &spec, 10);
    let opts = DecodeOpts::default()
        .with_hw(HwMode::Off)
        .with_mode(DecodeMode::Sequential)
        .with_target(PixelFormat::Yuv422p)
        .with_size(320, 180);
    let mut d = b.open_decoder(&path, &opts).unwrap();
    let mut n = 0;
    while let Some(f) = d.next_frame().unwrap() {
        assert_eq!(
            (f.width, f.height, f.format),
            (320, 180, PixelFormat::Yuv422p)
        );
        assert_eq!(f.validate(), Ok(()));
        n += 1;
    }
    assert_eq!(n, 10);
    // Same layout, another size: still scaled.
    let opts = DecodeOpts::default()
        .with_hw(HwMode::Off)
        .with_size(160, 90);
    let f = b
        .open_decoder(&path, &opts)
        .unwrap()
        .seek_exact(Time::ZERO)
        .unwrap();
    assert_eq!(
        (f.width, f.height, f.format),
        (160, 90, PixelFormat::Yuv422p10)
    );
}

#[test]
fn joined_parts_play_back_to_back() {
    let b = backend();
    let dir = TempDir::new("proxy-concat");
    let parts: Vec<PathBuf> = [12_u64, 7, 20]
        .iter()
        .enumerate()
        .map(|(i, frames)| {
            let spec = intra_spec(
                &dir.path().join(format!("part{i}.mov")),
                "prores",
                PixelFormat::Yuv422p10,
                (160, 90),
            );
            write(&b, &spec, *frames)
        })
        .collect();
    let out = dir.path().join("joined.mov");
    let summary = b.concat(&ConcatSpec::new(parts.clone(), &out)).unwrap();
    assert_eq!(summary.video_frames, 39);
    assert_eq!(summary.audio_samples, 39 * 1920);
    let index = b.index_media(&out).unwrap();
    assert_eq!(index.frame_count(), 39);
    // Frame n of the joined file is at n frames: no gap or overlap at the seams.
    for n in [0, 11, 12, 18, 19, 38] {
        assert_eq!(
            index.time_of(n),
            FrameRate::FPS_25
                .frame_to_time(i64::try_from(n).unwrap())
                .ok(),
            "frame {n}"
        );
    }
    let probe = b.probe(&out).unwrap();
    assert_eq!(probe.audio.unwrap().channels, 2);
    let mut a = b.open_audio_decoder(&out, None).unwrap();
    let all = a
        .decode_range(Time::ZERO, cutweave_model::Duration::from_secs(10).unwrap())
        .unwrap();
    assert_eq!(all.frames(), 39 * 1920);
    // No leftovers next to the output.
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.contains("partial")), "{names:?}");

    // A cancelled join leaves no file; parts that do not match are refused.
    let cancelled = dir.path().join("cancelled.mov");
    let err = b
        .concat(&ConcatSpec::new(parts.clone(), &cancelled).with_cancel(CancelCheck::new(|| true)))
        .unwrap_err();
    assert_eq!(err, MediaError::Cancelled);
    assert!(!cancelled.exists());
    let odd = intra_spec(
        &dir.path().join("odd.mov"),
        "mjpeg",
        PixelFormat::Yuv422p,
        (160, 90),
    );
    let odd = write(&b, &odd, 3);
    assert!(matches!(
        b.concat(&ConcatSpec::new(
            vec![parts[0].clone(), odd],
            dir.path().join("x.mov")
        )),
        Err(MediaError::Invalid { .. })
    ));
    assert!(matches!(
        b.concat(&ConcatSpec::new(Vec::new(), dir.path().join("y.mov"))),
        Err(MediaError::Invalid { .. })
    ));
}
