// SPDX-License-Identifier: GPL-3.0-or-later
//! `FakeBackend`: synthetic clips decode with exact pts (WP-1D.01 acceptance), tones are exact,
//! and the fake encoder checks what it is fed.
#![allow(clippy::unwrap_used)] // test helpers unwrap known-good times

use std::path::Path;

use cutweave_media::testing::{FakeAudio, FakeMedia, FakeVideo, fake_frame_number};
use cutweave_media::{
    AudioEncodeSpec, DecodeOpts, EncodeSpec, FakeBackend, MediaBackend, MediaError, PixelFormat,
    VideoEncodeSpec,
};
use cutweave_model::{Duration, FLICKS_PER_SECOND, FrameRate, SampleRate, StreamKind, Time};

const CLIP: &str = "/fake/clip.mp4";

fn secs(s: i64) -> Duration {
    Duration::from_secs(s).unwrap()
}

fn t(flicks: i64) -> Time {
    Time::from_flicks(flicks).unwrap()
}

fn clip_backend() -> FakeBackend {
    FakeBackend::new().with_media(CLIP, FakeMedia::clip(secs(10), FrameRate::FPS_30))
}

#[test]
fn a_10_s_clip_at_30_fps_decodes_every_frame_with_exact_pts() {
    let backend = clip_backend();
    let probe = backend.probe(Path::new(CLIP)).unwrap();
    assert_eq!(probe.duration, Some(secs(10)));
    assert_eq!(probe.fps, Some(FrameRate::FPS_30));
    assert!(!probe.vfr);
    let kinds: Vec<_> = probe.streams.iter().map(|s| (s.index, s.kind)).collect();
    assert_eq!(kinds, [(0, StreamKind::Video), (1, StreamKind::Audio)]);

    let mut decoder = backend
        .open_decoder(Path::new(CLIP), &DecodeOpts::default())
        .unwrap();
    assert!(!decoder.is_hw());
    let frame_flicks = FLICKS_PER_SECOND / 30; // 23 520 000: exact at 30 fps
    let mut n = 0_i64;
    while let Some(frame) = decoder.next_frame().unwrap() {
        assert_eq!(frame.pts, t(n * frame_flicks), "frame {n}");
        assert_eq!(fake_frame_number(&frame), u32::try_from(n).ok());
        assert_eq!((frame.width, frame.height), (64, 36));
        assert_eq!(frame.format, PixelFormat::Yuv420p);
        frame.validate().unwrap();
        n += 1;
    }
    assert_eq!(n, 300);
    assert!(decoder.next_frame().unwrap().is_none(), "end stays the end");
    assert_eq!(backend.decoded_frames(), 300);
}

#[test]
fn ntsc_rates_use_the_exact_flick_grid() {
    let backend = FakeBackend::new().with_media(
        CLIP,
        FakeMedia::video(FakeVideo::lasting(16, 8, FrameRate::FPS_29_97, secs(10))),
    );
    let mut decoder = backend
        .open_decoder(Path::new(CLIP), &DecodeOpts::default())
        .unwrap();
    let mut n = 0_i64;
    while let Some(frame) = decoder.next_frame().unwrap() {
        // 1001/30000 s = 23 543 520 flicks exactly.
        assert_eq!(frame.pts, t(n * 23_543_520));
        n += 1;
    }
    assert_eq!(n, 300);
}

#[test]
fn seek_exact_returns_the_frame_on_screen() {
    let backend = clip_backend();
    let mut d = backend
        .open_decoder(Path::new(CLIP), &DecodeOpts::default())
        .unwrap();
    let period = FLICKS_PER_SECOND / 30;
    for (at, want) in [
        (0, 0),
        (period, 1),
        (period - 1, 0),
        (150 * period + period / 2, 150),
        (299 * period, 299),
        (300 * period - 1, 299),
        (-5, 0),
    ] {
        let frame = d.seek_exact(t(at)).unwrap();
        assert_eq!(fake_frame_number(&frame), Some(want), "seek {at}");
        assert_eq!(frame.pts, t(i64::from(want) * period), "seek {at}");
    }
    // Decoding continues after the sought frame.
    d.seek_exact(t(100 * period)).unwrap();
    let next = d.next_frame().unwrap().unwrap();
    assert_eq!(fake_frame_number(&next), Some(101));
    // Past the end.
    assert_eq!(
        d.seek_exact(t(300 * period)).err(),
        Some(MediaError::OutOfRange { t: t(300 * period) })
    );
}

#[test]
fn target_layouts_are_honoured() {
    let backend = clip_backend();
    for format in [PixelFormat::Nv12, PixelFormat::P010, PixelFormat::Rgba8] {
        let opts = DecodeOpts {
            target_pixfmt: Some(format),
            ..DecodeOpts::default()
        };
        let mut d = backend.open_decoder(Path::new(CLIP), &opts).unwrap();
        let f = d.seek_exact(t(FLICKS_PER_SECOND)).unwrap();
        assert_eq!(f.format, format);
        f.validate().unwrap();
        assert_eq!(fake_frame_number(&f), Some(30), "{format:?}");
    }
}

#[test]
fn tones_are_exact_and_ranges_join_without_gaps() {
    let backend = clip_backend();
    let mut a = backend.open_audio_decoder(Path::new(CLIP), None).unwrap();
    let format = a.format();
    assert_eq!((format.rate, format.channels), (SampleRate::HZ_48_000, 2));

    let whole = a.decode_range(Time::ZERO, secs(1)).unwrap();
    whole.validate().unwrap();
    assert_eq!((whole.pts, whole.frames()), (Time::ZERO, 48_000));
    let tone = FakeAudio::tone(SampleRate::HZ_48_000, 2, secs(10));
    for c in 0..2_u16 {
        for n in [0_u64, 1, 109, 47_999] {
            assert_eq!(
                whole.planes[usize::from(c)][usize::try_from(n).unwrap()],
                tone.sample(c, n)
            );
        }
    }
    // Channel 0 is 440 Hz at −6 dBFS: a quarter period in (109.09 samples) it peaks near 0.5.
    let peak = whole.planes[0].iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    assert!((0.49..=0.5).contains(&peak), "peak {peak}");

    let half = Duration::from_flicks(FLICKS_PER_SECOND / 2).unwrap();
    let first = a.decode_range(Time::ZERO, half).unwrap();
    let second = a.decode_range(Time::from(half), half).unwrap();
    assert_eq!(second.pts, Time::from(half));
    let joined: Vec<f32> = first.planes[1]
        .iter()
        .chain(&second.planes[1])
        .copied()
        .collect();
    assert_eq!(joined, whole.planes[1]);

    // A range that starts between samples begins at the next sample.
    let one = SampleRate::HZ_48_000.samples_to_time(1).unwrap();
    let odd = a
        .decode_range(
            t(one.flicks() - 1),
            Duration::from_flicks(one.flicks()).unwrap(),
        )
        .unwrap();
    assert_eq!((odd.pts, odd.frames()), (one, 1));

    // The end cuts ranges short.
    let tail = a.decode_range(t(9 * FLICKS_PER_SECOND), secs(5)).unwrap();
    assert_eq!(tail.frames(), 48_000);
    let past = a.decode_range(t(20 * FLICKS_PER_SECOND), secs(1)).unwrap();
    assert_eq!(past.frames(), 0);
    assert_eq!(past.pts, t(10 * FLICKS_PER_SECOND));
}

#[test]
fn missing_streams_are_typed_errors() {
    let backend = clip_backend()
        .with_media(
            "/fake/silent.mov",
            FakeMedia::video(FakeVideo::new(8, 8, FrameRate::FPS_25, 10)),
        )
        .with_media(
            "/fake/tone.wav",
            FakeMedia::audio(FakeAudio::tone(SampleRate::HZ_44_100, 1, secs(1))),
        );
    assert!(backend.open_audio_decoder(Path::new(CLIP), Some(1)).is_ok());
    assert!(matches!(
        backend.open_audio_decoder(Path::new(CLIP), Some(0)).err(),
        Some(MediaError::NoStream {
            kind: StreamKind::Audio,
            index: Some(0),
            ..
        })
    ));
    assert!(matches!(
        backend
            .open_audio_decoder(Path::new("/fake/silent.mov"), None)
            .err(),
        Some(MediaError::NoStream {
            kind: StreamKind::Audio,
            index: None,
            ..
        })
    ));
    assert!(matches!(
        backend
            .open_decoder(Path::new("/fake/tone.wav"), &DecodeOpts::default())
            .err(),
        Some(MediaError::NoStream {
            kind: StreamKind::Video,
            ..
        })
    ));
    let probe = backend.probe(Path::new("/fake/tone.wav")).unwrap();
    assert_eq!(probe.streams[0].index, 0);
    assert_eq!(probe.duration, Some(secs(1)));
    assert_eq!(probe.fps, None);
}

#[test]
fn injected_errors_are_returned_everywhere() {
    let err = MediaError::Corrupt {
        path: "/fake/broken.mp4".into(),
        message: "moov atom not found".into(),
    };
    let backend = FakeBackend::new().with_error("/fake/broken.mp4", err.clone());
    let p = Path::new("/fake/broken.mp4");
    assert_eq!(backend.probe(p).err(), Some(err.clone()));
    assert_eq!(
        backend.open_decoder(p, &DecodeOpts::default()).err(),
        Some(err.clone())
    );
    assert_eq!(backend.open_audio_decoder(p, None).err(), Some(err));
}

#[test]
fn the_fake_encoder_takes_a_decoded_clip_and_checks_its_input() {
    let backend = clip_backend();
    let out = Path::new("/fake/out.mp4");
    let spec = EncodeSpec {
        path: out.to_path_buf(),
        container: None,
        video: Some(VideoEncodeSpec {
            codec: "rawvideo".into(),
            width: 64,
            height: 36,
            rate: FrameRate::FPS_30,
            input_format: PixelFormat::Yuv420p,
        }),
        audio: Some(AudioEncodeSpec {
            codec: "pcm_f32le".into(),
            rate: SampleRate::HZ_48_000,
            channels: 2,
        }),
    };
    let mut enc = backend.open_encoder(&spec).unwrap();
    let mut video = backend
        .open_decoder(Path::new(CLIP), &DecodeOpts::default())
        .unwrap();
    let mut last = None;
    while let Some(frame) = video.next_frame().unwrap() {
        enc.push_video(&frame).unwrap();
        last = Some(frame);
    }
    let last = last.unwrap();
    assert!(matches!(
        enc.push_video(&last),
        Err(MediaError::Invalid { .. })
    ));
    let mut audio = backend.open_audio_decoder(Path::new(CLIP), None).unwrap();
    for s in 0..10 {
        let chunk = audio
            .decode_range(t(s * FLICKS_PER_SECOND), secs(1))
            .unwrap();
        enc.push_audio(&chunk).unwrap();
    }
    // A chunk that does not continue the stream is rejected.
    let early = audio.decode_range(Time::ZERO, secs(1)).unwrap();
    assert!(matches!(
        enc.push_audio(&early),
        Err(MediaError::Invalid { .. })
    ));
    let summary = enc.finish().unwrap();
    assert_eq!(summary.video_frames, 300);
    assert_eq!(summary.audio_samples, 480_000);
    assert_eq!(summary.duration, secs(10));
    assert_eq!(backend.encodes(), vec![(out.to_path_buf(), summary)]);

    // Unknown codecs and mismatched frames.
    let h264 = EncodeSpec {
        video: Some(VideoEncodeSpec {
            codec: "h264".into(),
            ..spec.video.clone().unwrap()
        }),
        ..spec.clone()
    };
    assert!(matches!(
        backend.open_encoder(&h264).err(),
        Some(MediaError::Unsupported { .. })
    ));
    let mut enc = backend.open_encoder(&spec).unwrap();
    let nv12 = backend
        .open_decoder(
            Path::new(CLIP),
            &DecodeOpts {
                target_pixfmt: Some(PixelFormat::Nv12),
                ..DecodeOpts::default()
            },
        )
        .unwrap()
        .next_frame()
        .unwrap()
        .unwrap();
    assert!(matches!(
        enc.push_video(&nv12),
        Err(MediaError::Invalid { .. })
    ));
}

#[test]
fn clones_share_counters() {
    let a = clip_backend();
    let b = a.clone();
    let mut d = b
        .open_decoder(Path::new(CLIP), &DecodeOpts::default())
        .unwrap();
    d.next_frame().unwrap();
    d.next_frame().unwrap();
    assert_eq!(a.decoded_frames(), 2);
}
