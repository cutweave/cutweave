// SPDX-License-Identifier: GPL-3.0-or-later
//! `FakeBackend::index_media` (WP-1D.02): the synthetic index agrees with the decoder's frame
//! times, and indexing needs pictures.
#![allow(clippy::unwrap_used)] // test helpers unwrap known-good times

use std::path::Path;

use cutweave_media::testing::{FAKE_GOP_FRAMES, FakeAudio, FakeMedia};
use cutweave_media::{FakeBackend, FrameIndex, MediaBackend, MediaError};
use cutweave_model::{Duration, FLICKS_PER_SECOND, FrameRate, SampleRate, StreamKind, Time};

const CLIP: &str = "/fake/clip.mp4";

fn secs(s: i64) -> Duration {
    Duration::from_secs(s).unwrap()
}

fn t(flicks: i64) -> Time {
    Time::from_flicks(flicks).unwrap()
}

#[test]
fn the_fake_index_maps_time_to_frames_and_lists_keyframes() {
    let backend = FakeBackend::new().with_media(CLIP, FakeMedia::clip(secs(10), FrameRate::FPS_30));
    let ix = backend.index_media(Path::new(CLIP)).unwrap();
    assert_eq!(ix.frame_count(), 300);
    assert!(!ix.is_vfr());
    assert_eq!(ix.duration(), Some(secs(10)));
    let gop = usize::try_from(FAKE_GOP_FRAMES).unwrap();
    assert_eq!(ix.gops().len(), 300usize.div_ceil(gop));
    assert_eq!(ix.key_frames().nth(1), Some(gop));
    // The index agrees with the decoder on every frame time.
    let frame_flicks = FLICKS_PER_SECOND / 30;
    for n in [0_usize, 1, 11, 12, 299] {
        let flicks = i64::try_from(n).unwrap() * frame_flicks;
        assert_eq!(ix.time_of(n), Some(t(flicks)));
        assert_eq!(ix.frame_at(t(flicks + 5)), n);
    }
    // Round trip through the stored form.
    assert_eq!(FrameIndex::from_bytes(&ix.to_bytes()).unwrap(), ix);
}

#[test]
fn indexing_needs_pictures() {
    let backend = FakeBackend::new().with_media(
        "/fake/tone.wav",
        FakeMedia::audio(FakeAudio::tone(SampleRate::HZ_48_000, 1, secs(1))),
    );
    assert!(matches!(
        backend.index_media(Path::new("/fake/tone.wav")),
        Err(MediaError::NoStream {
            kind: StreamKind::Video,
            ..
        })
    ));
}
