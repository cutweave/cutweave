// SPDX-License-Identifier: GPL-3.0-or-later
//! The reader against the real engine-side `FramePublisher` (WP-1H.03 acceptance criteria):
//! no torn frames, epoch rejection, successor following, liveness, clock-scheduled choice.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration as StdDuration, Instant};

use cutweave_bridge::BridgeError;
use cutweave_bridge::reader::{RingReader, now_ns};
use cutweave_bridge::session::{EndReason, RingSession, SessionState};
use cutweave_model::{Duration, Time};
use cutweave_playback::shm::{
    ClockSample, ClockState, FrameLayout, FrameMeta, FramePublisher, PixelFormat, PreviewConfig,
    Reconfigured, RingConfig,
};

#[allow(clippy::cast_possible_truncation)]
/// Byte `i` of the payload of frame `seq`: changes every 4 KiB page and with every frame, so a
/// mix of two frames (a torn copy) cannot pass for either.
fn pattern(seq: u64, i: usize) -> u8 {
    (seq.wrapping_mul(131) as usize)
        .wrapping_add(i / 4096)
        .wrapping_add(i % 7) as u8
}

fn fill_pattern(buf: &mut [u8], seq: u64) {
    for (i, b) in buf.iter_mut().enumerate() {
        *b = pattern(seq, i);
    }
}

fn check_pattern(buf: &[u8], seq: u64) -> bool {
    buf.iter().enumerate().all(|(i, &b)| b == pattern(seq, i))
}

fn config(w: u32, h: u32, row_align: u32) -> RingConfig {
    RingConfig {
        row_align,
        prefault: false,
        ..RingConfig::new(w, h, PixelFormat::Rgba8)
    }
}

fn publish(p: &mut FramePublisher, layout: &FrameLayout, pts_flicks: i64) -> u64 {
    let meta = FrameMeta {
        pts: Time::from_flicks(pts_flicks).unwrap(),
        duration: Duration::from_flicks(11_760_000).unwrap(),
        rev: 7,
        ..FrameMeta::default()
    };
    let mut slot = p.begin_frame(&meta, layout).unwrap();
    let seq = slot.seq();
    fill_pattern(slot.payload_mut(), seq);
    slot.commit()
}

fn reader(p: &FramePublisher) -> RingReader {
    RingReader::open(p.name().as_str(), p.epoch()).unwrap()
}

#[test]
fn reads_the_newest_frame_exactly_as_published() {
    let mut p = FramePublisher::create(&config(64, 48, 1)).unwrap();
    let layout = p.preview_layout().unwrap();
    let r = reader(&p);
    let mut buf = Vec::new();
    assert!(r.read(0, None, &mut buf).unwrap().is_none(), "empty ring");
    for i in 0..9 {
        publish(&mut p, &layout, i * 1000);
    }
    let m = r.read(0, None, &mut buf).unwrap().unwrap();
    assert_eq!(m.seq, 9, "latest wins: 9 frames into 6 slots");
    assert_eq!((m.layout.width(), m.layout.height()), (64, 48));
    assert_eq!(m.layout.format(), PixelFormat::Rgba8);
    assert_eq!(m.layout.len(), 64 * 48 * 4);
    assert_eq!((m.pts, m.rev), (8000, 7));
    assert!(check_pattern(&buf[..m.layout.len()], 9));
    assert!(
        r.read(9, None, &mut buf).unwrap().is_none(),
        "nothing newer"
    );
}

#[test]
fn padded_rows_are_reported_not_repacked() {
    // wgpu pads rows to 256 bytes; 100 px * 4 = 400 -> 512.
    let mut p = FramePublisher::create(&config(100, 20, 256)).unwrap();
    let layout = p.preview_layout().unwrap();
    publish(&mut p, &layout, 0);
    let r = reader(&p);
    let mut buf = Vec::new();
    let m = r.read(0, None, &mut buf).unwrap().unwrap();
    assert_eq!(m.layout.planes()[0].stride, 512);
    assert_eq!(m.layout.len(), 512 * 20);
    assert!(check_pattern(&buf[..m.layout.len()], 1));
}

#[test]
fn nv12_planes_survive() {
    let cfg = RingConfig {
        prefault: false,
        row_align: 1,
        ..RingConfig::new(32, 16, PixelFormat::Nv12)
    };
    let mut p = FramePublisher::create(&cfg).unwrap();
    let layout = p.preview_layout().unwrap();
    publish(&mut p, &layout, 0);
    let r = reader(&p);
    let mut buf = Vec::new();
    let m = r.read(0, None, &mut buf).unwrap().unwrap();
    assert_eq!(m.layout.format(), PixelFormat::Nv12);
    assert_eq!(m.layout.planes()[1].offset, 32 * 16);
    assert_eq!(m.layout.len(), 32 * 16 * 3 / 2);
    assert!(check_pattern(&buf[..m.layout.len()], 1));
}

#[test]
fn no_torn_frames_under_a_fast_writer() {
    // 4 slots (the minimum) and a writer that never waits: the reader is lapped constantly.
    let mut cfg = config(640, 360, 1);
    cfg.slot_count = 4;
    let mut p = FramePublisher::create(&cfg).unwrap();
    let layout = p.preview_layout().unwrap();
    let r = reader(&p);
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut pts = 0;
            while !stop.load(Ordering::Relaxed) {
                publish(&mut p, &layout, pts);
                pts += 1;
                std::thread::yield_now();
            }
            p
        })
    };
    let (mut buf, mut last, mut frames) = (Vec::new(), 0u64, 0u64);
    let end = Instant::now() + StdDuration::from_millis(1500);
    while Instant::now() < end {
        if let Some(m) = r.read(last, None, &mut buf).unwrap() {
            assert!(m.seq > last, "seq goes forward");
            assert!(
                check_pattern(&buf[..m.layout.len()], m.seq),
                "frame {} is torn",
                m.seq
            );
            last = m.seq;
            frames += 1;
        }
    }
    stop.store(true, Ordering::Relaxed);
    let p = writer.join().unwrap();
    assert!(frames > 50, "only {frames} frames read");
    assert!(last <= p.write_index());
    eprintln!(
        "{frames} frames of {} published read, {} slot copies discarded by the seqlock",
        p.write_index(),
        r.torn_retries()
    );
}

#[test]
fn a_copy_that_the_writer_laps_is_reported_torn() {
    use cutweave_bridge::reader::Copied;
    let mut cfg = config(32, 32, 1);
    cfg.slot_count = 4;
    let mut p = FramePublisher::create(&cfg).unwrap();
    let layout = p.preview_layout().unwrap();
    let r = reader(&p);
    publish(&mut p, &layout, 0);
    let chosen = r.select(0, None).unwrap().unwrap();
    let mut buf = vec![0u8; layout.len()];
    assert_eq!(r.copy_payload(&chosen, &mut buf), Copied::Valid);
    // The writer comes around to the same slot (4 slots) before the reader's copy is done.
    for i in 1..=4 {
        publish(&mut p, &layout, i);
    }
    assert_eq!(r.copy_payload(&chosen, &mut buf), Copied::Torn);
    assert_eq!(r.torn_retries(), 1);
    // `read` starts over from the new state and delivers a consistent, newer frame.
    let m = r.read(0, None, &mut buf).unwrap().unwrap();
    assert_eq!(m.seq, 5);
    assert!(check_pattern(&buf[..m.layout.len()], 5));
}

#[test]
fn rejects_a_ring_with_another_epoch() {
    let p = FramePublisher::create(&config(16, 16, 1)).unwrap();
    let e = RingReader::open(p.name().as_str(), p.epoch() ^ 1)
        .err()
        .unwrap();
    assert!(
        matches!(e, BridgeError::EpochMismatch { expected, found }
            if expected == p.epoch() ^ 1 && found == p.epoch()),
        "{e:?}"
    );
    assert_eq!(e.code(), "ERR_RING_EPOCH");
    assert!(RingSession::open(p.name().as_str(), p.epoch()).is_ok());
}

#[test]
fn missing_and_malformed_names_are_typed_errors() {
    let p = FramePublisher::create(&config(16, 16, 1)).unwrap();
    let gone = {
        let q = FramePublisher::create(&config(16, 16, 1)).unwrap();
        q.name().to_string()
    };
    let e = RingReader::open(&gone, 1).err().unwrap();
    assert!(matches!(e, BridgeError::NotFound(_)), "{e:?}");
    assert_eq!(e.code(), "ERR_RING_NOT_FOUND");
    for bad in ["", "nope", "cwp-zzzzzzzz-0000000000000000", "../etc/passwd"] {
        let e = RingReader::open(bad, p.epoch()).err().unwrap();
        assert_eq!(e.code(), "ERR_RING_INVALID", "{bad}: {e:?}");
    }
}

#[test]
fn follows_a_retired_ring_to_its_successor_without_losing_frames() {
    let mut p = FramePublisher::create(&config(64, 48, 1)).unwrap();
    let layout = p.preview_layout().unwrap();
    let mut s = RingSession::open(p.name().as_str(), p.epoch()).unwrap();
    let first_ring = p.name().to_string();
    let mut buf = Vec::new();
    for i in 0..3 {
        publish(&mut p, &layout, i);
    }
    // Move to a bigger ring while frames 1..=3 are still unread.
    let moved = p
        .configure(PreviewConfig {
            width: 160,
            height: 90,
            format: PixelFormat::Rgba8,
        })
        .unwrap();
    let Reconfigured::Moved { ring, .. } = moved else {
        panic!("expected a move, got {moved:?}");
    };
    assert_ne!(ring.name.as_str(), first_ring);
    // Status does not abandon the unread frames of the retired ring ...
    assert_eq!(s.status().state, SessionState::Live);
    assert_eq!(s.switches(), 0);
    // ... the read delivers the newest of them from the old ring first ...
    let (m, r) = s.read(None, &mut buf).unwrap().unwrap();
    assert_eq!((m.seq, r.name().as_str()), (3, first_ring.as_str()));
    assert!(check_pattern(&buf[..m.layout.len()], 3));
    // ... then moves on, and seq keeps increasing across rings.
    let layout = p.preview_layout().unwrap();
    assert_eq!(layout.width(), 160);
    let seq = publish(&mut p, &layout, 3);
    assert_eq!(seq, 4);
    let (m, r) = s.read(None, &mut buf).unwrap().unwrap();
    assert_eq!(s.switches(), 1);
    assert_eq!(r.name(), &ring.name);
    assert_eq!(r.epoch(), ring.epoch);
    assert_eq!((m.seq, m.layout.width(), m.layout.height()), (4, 160, 90));
    assert!(check_pattern(&buf[..m.layout.len()], 4));
    assert_eq!(s.status().state, SessionState::Live);
}

#[test]
fn an_idle_session_follows_once_it_has_delivered_everything() {
    let mut p = FramePublisher::create(&config(64, 48, 1)).unwrap();
    let layout = p.preview_layout().unwrap();
    publish(&mut p, &layout, 0);
    let mut s = RingSession::open(p.name().as_str(), p.epoch()).unwrap();
    let mut buf = Vec::new();
    s.read(None, &mut buf).unwrap().unwrap();
    p.configure(PreviewConfig {
        width: 200,
        height: 120,
        format: PixelFormat::Rgba8,
    })
    .unwrap();
    let st = s.status();
    assert_eq!(st.state, SessionState::Live);
    assert_eq!(s.switches(), 1, "nothing left to drain: status follows");
    assert_eq!(st.name.as_deref(), Some(p.name().as_str()));
}

#[test]
fn two_quick_moves_end_the_session_with_successor_gone() {
    let mut p = FramePublisher::create(&config(32, 32, 1)).unwrap();
    let mut s = RingSession::open(p.name().as_str(), p.epoch()).unwrap();
    for w in [128, 512] {
        p.configure(PreviewConfig {
            width: w,
            height: w,
            format: PixelFormat::Rgba8,
        })
        .unwrap();
    }
    // The ring we map named a successor that has been retired and released since.
    let st = s.status();
    assert_eq!(st.state, SessionState::Ended(EndReason::SuccessorGone));
    assert!(s.reader().is_none(), "mapping released");
    let mut buf = Vec::new();
    assert!(s.read(None, &mut buf).unwrap().is_none());
}

#[test]
fn in_place_reconfiguration_keeps_the_ring() {
    let mut p = FramePublisher::create(&config(64, 48, 1)).unwrap();
    let s = RingSession::open(p.name().as_str(), p.epoch()).unwrap();
    let r = s.reader().unwrap();
    assert_eq!(r.config().unwrap().config_gen, 1);
    let moved = p
        .configure(PreviewConfig {
            width: 32,
            height: 24,
            format: PixelFormat::Rgba8,
        })
        .unwrap();
    assert!(matches!(moved, Reconfigured::InPlace { config_gen: 2, .. }));
    let c = r.config().unwrap();
    assert_eq!((c.config_gen, c.width, c.height), (2, 32, 24));
}

#[test]
fn writer_is_dead_after_500_ms_without_a_heartbeat() {
    let mut p = FramePublisher::create(&config(16, 16, 1)).unwrap();
    let layout = p.preview_layout().unwrap();
    publish(&mut p, &layout, 0);
    let mut s = RingSession::open(p.name().as_str(), p.epoch()).unwrap();
    assert_eq!(s.status().state, SessionState::Live);
    // Heartbeats keep an idle engine alive for well over 500 ms ...
    let end = Instant::now() + StdDuration::from_millis(700);
    while Instant::now() < end {
        p.heartbeat();
        assert_eq!(s.status().state, SessionState::Live);
        std::thread::sleep(StdDuration::from_millis(20));
    }
    // ... and silence kills it.
    std::thread::sleep(StdDuration::from_millis(560));
    let st = s.status();
    assert_eq!(st.state, SessionState::Ended(EndReason::HeartbeatStale));
    assert!(st.heartbeat_age_ms.is_none() && s.reader().is_none());
    let mut buf = Vec::new();
    assert!(s.read(None, &mut buf).unwrap().is_none());
}

#[test]
fn a_hung_writer_is_detected_at_the_threshold_not_before() {
    let p = FramePublisher::create(&config(16, 16, 1)).unwrap();
    let r = reader(&p);
    let beat = r.heartbeat_ns();
    assert!(!r.heartbeat_stale(beat + 499_000_000));
    assert!(r.heartbeat_stale(beat + 501_000_000));
    assert_eq!(
        r.heartbeat_age_ns(beat.saturating_sub(5)),
        0,
        "never negative"
    );
}

#[test]
fn a_cleanly_closed_writer_ends_the_session() {
    let p = FramePublisher::create(&config(16, 16, 1)).unwrap();
    let mut s = RingSession::open(p.name().as_str(), p.epoch()).unwrap();
    drop(p);
    assert_eq!(
        s.status().state,
        SessionState::Ended(EndReason::WriterClosed)
    );
}

#[test]
fn close_is_final() {
    let p = FramePublisher::create(&config(16, 16, 1)).unwrap();
    let mut s = RingSession::open(p.name().as_str(), p.epoch()).unwrap();
    s.close();
    let mut buf = Vec::new();
    assert_eq!(s.read(None, &mut buf).err(), Some(BridgeError::Closed));
    assert_eq!(
        s.status().state,
        SessionState::Ended(EndReason::ClosedByCaller)
    );
}

#[test]
fn the_audio_clock_is_read_and_extrapolated_like_the_writer_does() {
    let p = FramePublisher::create(&config(16, 16, 1)).unwrap();
    let r = reader(&p);
    let w = p.clock_writer();
    let t0 = Time::from_flicks(705_600_000).unwrap(); // 1 s
    let sample = ClockSample::now(t0, ClockState::Playing, 2000); // 2x
    assert!(w.publish(&sample));
    let c = r.clock().unwrap();
    assert_eq!((c.flicks, c.rate_milli), (705_600_000, 2000));
    assert_eq!(c.state, ClockState::Playing);
    for at in [0, 10_000_000, 50_000_000, 500_000_000] {
        assert_eq!(
            c.extrapolate(sample.host_ns + at),
            sample.extrapolate(sample.host_ns + at),
            "{at} ns after the sample"
        );
    }
    // Stopped clocks do not advance.
    assert!(w.publish(&ClockSample::now(t0, ClockState::Stopped, 1000)));
    let c = r.clock().unwrap();
    assert_eq!(c.extrapolate(c.host_ns + 50_000_000), 705_600_000);
    assert_eq!(r.clock_at(now_ns()), Some(705_600_000));
}

#[test]
fn clock_scheduled_choice_picks_the_newest_frame_not_after_the_clock() {
    let mut p = FramePublisher::create(&config(16, 16, 1)).unwrap();
    let layout = p.preview_layout().unwrap();
    // Frames at 0, 16.67 ms, ... (60 fps) in flicks: 11_760_000 each.
    for i in 0..5 {
        publish(&mut p, &layout, i * 11_760_000);
    }
    let mut s = RingSession::open(p.name().as_str(), p.epoch()).unwrap();
    let w = p.clock_writer();
    // Paused at 2.5 frames in: frame 3 (pts 2 * 11.76M) is the newest not after the clock.
    assert!(w.publish(&ClockSample::now(
        Time::from_flicks(2 * 11_760_000 + 5_000_000).unwrap(),
        ClockState::Stopped,
        1000
    )));
    let mut buf = Vec::new();
    let (m, _) = s.read_at_clock(0, &mut buf).unwrap().unwrap();
    assert_eq!((m.seq, m.pts), (3, 2 * 11_760_000));
    assert!(check_pattern(&buf[..m.layout.len()], 3));
    // The same frame is not delivered twice; later frames wait for the clock.
    assert!(s.read_at_clock(0, &mut buf).unwrap().is_none());
    // Clock at frame 5 (pts 4 * 11.76M). With 20 ms (14_112_000 flicks) of latency the target
    // is 32.9M flicks: only frame 3 qualifies, and it was delivered already.
    assert!(w.publish(&ClockSample::now(
        Time::from_flicks(4 * 11_760_000).unwrap(),
        ClockState::Stopped,
        1000
    )));
    assert!(s.read_at_clock(14_112_000, &mut buf).unwrap().is_none());
    // 5 ms of latency (target 43.5M) admits frame 4 (35.28M) ...
    let (m, _) = s.read_at_clock(3_528_000, &mut buf).unwrap().unwrap();
    assert_eq!(m.seq, 4);
    // ... and no latency admits frame 5 (47.04M).
    let (m, _) = s.read_at_clock(0, &mut buf).unwrap().unwrap();
    assert_eq!(m.seq, 5);
}
