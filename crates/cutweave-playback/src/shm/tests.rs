// SPDX-License-Identifier: GPL-3.0-or-later
//! Publisher/reader tests: protocol unit tests, threaded publish-while-read stress (with
//! resizes and ring moves), multi-process 1080p60 stream, crash cleanup and the C mirror.
//!
//! Multi-process tests re-run this test binary as the writer: `child_writer` is a no-op test
//! unless `CW_SHM_CHILD` is set.

// Timing statistics convert counts and durations to f64 and back; precision is irrelevant.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
// Tests read best with the conventional p(ublisher)/r(eader)/l(ayout) names.
#![allow(clippy::many_single_char_names)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write as _};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use cutweave_model::{Duration, Time};

use super::layout::{self, FrameLayout, MAX_SLOTS, MIN_SLOTS, PixelFormat, WriterState};
use super::os::SharedMemory;
use super::reader::{ReadError, SlotMeta, TestReader};
use super::*;

// ---------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------

const FRAME_60: i64 = layout::FLICKS_PER_SECOND / 60;

fn config(w: u32, h: u32, format: PixelFormat, slots: u32) -> RingConfig {
    RingConfig {
        slot_count: slots,
        ..RingConfig::new(w, h, format)
    }
}

fn meta(seq: u64) -> FrameMeta {
    let n = i64::try_from(seq).unwrap();
    FrameMeta {
        pts: Time::from_flicks(n * FRAME_60).unwrap(),
        duration: Duration::from_flicks(FRAME_60).unwrap(),
        rev: seq.wrapping_mul(0x9E37_79B9) ^ 0x5A,
        flags: if seq.is_multiple_of(3) {
            FLAG_LOWRES
        } else {
            0
        },
        render_ns: seq + 1,
    }
}

/// A 4 KiB page whose every u64 word is `seq`; frames are tiled with it.
fn page(seq: u64) -> [u8; 4096] {
    let mut p = [0u8; 4096];
    for w in p.as_chunks_mut::<8>().0 {
        *w = seq.to_le_bytes();
    }
    p
}

fn fill(buf: &mut [u8], seq: u64) {
    let p = page(seq);
    for chunk in buf.chunks_mut(4096) {
        chunk.copy_from_slice(&p[..chunk.len()]);
    }
}

/// Number of 4 KiB chunks of `buf` that are not entirely `seq` (0 = untorn frame).
fn bad_chunks(buf: &[u8], seq: u64) -> usize {
    let p = page(seq);
    buf.chunks(4096).filter(|c| **c != p[..c.len()]).count()
}

/// Everything a reader can check about frame `m` against what the writer published for it.
fn verify(m: &SlotMeta, buf: &[u8], expected_layout: Option<&FrameLayout>) -> Result<(), String> {
    let want = meta(m.seq);
    let mut errs = Vec::new();
    if m.pts != want.pts.flicks() {
        errs.push("pts");
    }
    if m.duration != want.duration.flicks() {
        errs.push("duration");
    }
    if m.rev != want.rev {
        errs.push("rev");
    }
    if m.flags != want.flags {
        errs.push("flags");
    }
    if m.render_ns != want.render_ns {
        errs.push("render_ns");
    }
    if expected_layout.is_some_and(|l| *l != m.layout) {
        errs.push("layout");
    }
    let bad = bad_chunks(&buf[..m.layout.len()], m.seq);
    if bad > 0 {
        return Err(format!(
            "seq {}: {bad} torn 4 KiB chunks, fields {errs:?}",
            m.seq
        ));
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(format!("seq {}: wrong {errs:?}", m.seq))
    }
}

fn publish_seq(p: &mut FramePublisher, layout: &FrameLayout, src: &mut [u8]) -> u64 {
    let seq = p.write_index() + 1;
    fill(&mut src[..layout.len()], seq);
    let got = p.publish(&meta(seq), layout, src).unwrap();
    assert_eq!(got, seq);
    got
}

fn open(p: &FramePublisher) -> TestReader {
    TestReader::open(p.name(), Some(p.epoch())).unwrap()
}

// ---------------------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------------------

#[test]
fn header_is_initialised_and_announced() {
    let p = FramePublisher::create(&config(64, 32, PixelFormat::Rgba8, 4)).unwrap();
    let a = p.announcement();
    assert_eq!(a.layout_version, 1);
    assert_eq!(&a.name, p.name());
    let r = TestReader::open(&a.name, Some(a.epoch)).unwrap();
    assert!(
        TestReader::open(&a.name, Some(a.epoch ^ 1)).is_err(),
        "epoch mismatch refused"
    );
    let h = r.header();
    assert_eq!(h.writer_pid.load(Ordering::Relaxed), std::process::id());
    assert_eq!(h.host_clock.load(Ordering::Relaxed), HOST_CLOCK as u32);
    assert_eq!(
        h.payload_offset.load(Ordering::Relaxed),
        layout::PAYLOAD_OFFSET as u64
    );
    assert_eq!(h.mapping_size.load(Ordering::Relaxed), a.mapping_size);
    assert_eq!(r.slot_count(), 4);
    // 64 px RGBA rows are 256 B already; capacity = one aligned frame.
    assert_eq!(r.slot_capacity(), 64 * 4 * 32);
    assert_eq!(r.writer_state(), Some(WriterState::Live));
    assert!(r.writer_alive(host_clock_ns(), HEARTBEAT_TIMEOUT_NS));
    let c = r.config().unwrap();
    assert_eq!((c.config_gen, c.width, c.height), (1, 64, 32));
    assert_eq!(c.format, PixelFormat::Rgba8 as u32);
    assert_eq!(r.write_index(), 0);
    let mut buf = vec![0u8; r.slot_capacity()];
    assert_eq!(r.read_latest_into(&mut buf, 0), Err(ReadError::Empty));
}

#[test]
fn slot_count_outside_4_to_6_is_rejected() {
    for n in [0, 3, 7] {
        assert!(matches!(
            FramePublisher::create(&config(16, 16, PixelFormat::Rgba8, n)),
            Err(RingError::SlotCount(_))
        ));
    }
}

#[test]
fn rgba_and_nv12_frames_round_trip_with_padded_rows() {
    let mut p = FramePublisher::create(&config(100, 50, PixelFormat::Nv12, 4)).unwrap();
    let r = open(&p);
    let mut src = vec![0u8; p.slot_capacity()];
    let mut dst = vec![0u8; r.slot_capacity()];
    let nv12 = p.preview_layout().unwrap();
    assert_eq!(
        nv12.planes()[0].stride,
        256,
        "100 px Y rows padded to 256 B"
    );
    assert_eq!(nv12.planes()[1].offset, 256 * 50);
    let seq = publish_seq(&mut p, &nv12, &mut src);
    let m = r.read_latest_into(&mut dst, 0).unwrap().unwrap();
    assert_eq!(m.seq, seq);
    assert_eq!(m.layout, nv12);
    assert_eq!(m.config_gen, 1);
    assert!(m.publish_ns > 0);
    verify(&m, &dst, Some(&nv12)).unwrap();

    // A smaller, tightly packed RGBA8 frame in the same slots (e.g. a low-res scrub frame).
    let rgba = FrameLayout::packed(30, 20, PixelFormat::Rgba8).unwrap();
    let seq = publish_seq(&mut p, &rgba, &mut src);
    let m = r.read_latest_into(&mut dst, seq - 1).unwrap().unwrap();
    assert_eq!((m.seq, m.layout), (seq, rgba));
    verify(&m, &dst, Some(&rgba)).unwrap();
    assert_eq!(r.read_latest_into(&mut dst, seq), Ok(None), "nothing newer");
}

#[test]
fn wrap_around_keeps_latest_and_seq_monotonic_for_4_to_6_slots() {
    for slots in MIN_SLOTS..=MAX_SLOTS {
        let mut p = FramePublisher::create(&config(32, 16, PixelFormat::Rgba8, slots)).unwrap();
        let r = open(&p);
        let l = p.preview_layout().unwrap();
        let (mut src, mut dst) = (vec![0u8; l.len()], vec![0u8; l.len()]);
        let mut last = 0;
        for n in 1..=u64::from(slots) * 5 + 3 {
            let seq = publish_seq(&mut p, &l, &mut src);
            assert_eq!(seq, n);
            let m = r.read_latest_into(&mut dst, last).unwrap().unwrap();
            assert_eq!(m.seq, seq);
            assert_eq!(m.slot, u32::try_from((n - 1) % u64::from(slots)).unwrap());
            verify(&m, &dst, Some(&l)).unwrap();
            last = m.seq;
        }
        // After several laps every slot holds one of the newest `slots` frames.
        let mut seqs: Vec<u64> = (0..slots)
            .map(|s| r.try_meta(s).unwrap().unwrap().seq)
            .collect();
        seqs.sort_unstable();
        assert_eq!(
            seqs,
            (last + 1 - u64::from(slots)..=last).collect::<Vec<_>>()
        );
    }
}

#[test]
fn slow_reader_skips_to_the_newest_frame() {
    let mut p = FramePublisher::create(&config(32, 16, PixelFormat::Rgba8, 4)).unwrap();
    let r = open(&p);
    let l = p.preview_layout().unwrap();
    let (mut src, mut dst) = (vec![0u8; l.len()], vec![0u8; l.len()]);
    for _ in 0..10 {
        publish_seq(&mut p, &l, &mut src);
    }
    let m = r.read_latest_into(&mut dst, 2).unwrap().unwrap();
    assert_eq!(m.seq, 10, "latest wins; 3..=9 are skipped by this reader");
}

#[test]
fn clock_scheduled_read_picks_newest_frame_not_after_the_clock() {
    let mut p = FramePublisher::create(&config(32, 16, PixelFormat::Rgba8, 6)).unwrap();
    let r = open(&p);
    let l = p.preview_layout().unwrap();
    let (mut src, mut dst) = (vec![0u8; l.len()], vec![0u8; l.len()]);
    for _ in 0..6 {
        publish_seq(&mut p, &l, &mut src);
    }
    // pts(seq) = seq * FRAME_60: a clock just before frame 4's pts selects frame 3.
    let m = r
        .read_at_clock_into(&mut dst, 4 * FRAME_60 - 1, 0)
        .unwrap()
        .unwrap();
    assert_eq!(m.seq, 3);
    verify(&m, &dst, Some(&l)).unwrap();
    assert_eq!(
        r.read_at_clock_into(&mut dst, 4 * FRAME_60 - 1, 3),
        Ok(None)
    );
    assert_eq!(
        r.read_at_clock_into(&mut dst, 100 * FRAME_60, 0)
            .unwrap()
            .unwrap()
            .seq,
        6
    );
}

#[test]
fn slot_being_written_is_busy_and_an_abandoned_slot_is_empty() {
    let mut p = FramePublisher::create(&config(32, 16, PixelFormat::Rgba8, 4)).unwrap();
    let r = open(&p);
    let l = p.preview_layout().unwrap();
    let (mut src, mut dst) = (vec![0u8; l.len()], vec![0u8; l.len()]);
    for _ in 0..4 {
        publish_seq(&mut p, &l, &mut src);
    }
    // Frame 5 goes to slot 0, which still holds frame 1.
    let mut w = p.begin_frame(&meta(5), &l).unwrap();
    assert_eq!(w.seq(), 5);
    w.payload_mut()[..8].copy_from_slice(&5u64.to_le_bytes());
    assert_eq!(r.read_slot_into(0, &mut dst), Err(ReadError::Busy));
    assert_eq!(r.try_meta(0), Err(ReadError::Busy));
    // The newest frame (4, slot 3) is unaffected.
    assert_eq!(r.read_latest_into(&mut dst, 0).unwrap().unwrap().seq, 4);
    drop(w);
    assert_eq!(
        r.read_slot_into(0, &mut dst),
        Ok(None),
        "abandoned slot is empty"
    );
    assert_eq!(p.write_index(), 4, "nothing was published");
    assert_eq!(
        publish_seq(&mut p, &l, &mut src),
        5,
        "same seq and slot are reused"
    );
    assert_eq!(r.read_slot_into(0, &mut dst).unwrap().unwrap().seq, 5);
}

#[test]
fn a_lapped_reader_detects_the_overwrite() {
    let mut p = FramePublisher::create(&config(32, 16, PixelFormat::Rgba8, 4)).unwrap();
    let r = open(&p);
    let l = p.preview_layout().unwrap();
    let mut src = vec![0u8; l.len()];
    publish_seq(&mut p, &l, &mut src);
    let m = r.try_meta(0).unwrap().unwrap();
    assert!(r.still_valid(&m));
    for _ in 0..4 {
        publish_seq(&mut p, &l, &mut src);
    }
    assert!(!r.still_valid(&m), "slot 0 was reused by frame 5");
}

#[test]
fn invalid_frames_are_rejected_without_touching_the_ring() {
    let mut p = FramePublisher::create(&config(32, 16, PixelFormat::Rgba8, 4)).unwrap();
    let r = open(&p);
    let l = p.preview_layout().unwrap();
    let big = FrameLayout::packed(64, 64, PixelFormat::Rgba8).unwrap();
    let src = vec![0u8; big.len()];
    assert_eq!(
        p.publish(&meta(1), &big, &src),
        Err(PublishError::TooLarge {
            len: big.len(),
            capacity: p.slot_capacity()
        })
    );
    assert_eq!(
        p.publish(&meta(1), &l, &src[..10]),
        Err(PublishError::SourceTooShort {
            need: l.len(),
            have: 10
        })
    );
    let bad_flags = FrameMeta {
        flags: 0x80,
        ..meta(1)
    };
    assert_eq!(
        p.publish(&bad_flags, &l, &src),
        Err(PublishError::UnknownFlags(0x80))
    );
    assert_eq!(p.write_index(), 0);
    for s in 0..4 {
        assert_eq!(r.try_meta(s), Ok(None));
        assert_eq!(r.header().write_index.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn configure_in_place_bumps_the_generation_and_frames_carry_it() {
    let mut p = FramePublisher::create(&config(640, 360, PixelFormat::Nv12, 4)).unwrap();
    let r = open(&p);
    let same = PreviewConfig {
        width: 640,
        height: 360,
        format: PixelFormat::Nv12,
    };
    assert_eq!(
        p.configure(same).unwrap(),
        Reconfigured::InPlace {
            config: same,
            config_gen: 1
        }
    );
    // An odd NV12 request is fitted to even sizes; it fits the slots, so no move.
    let smaller = PreviewConfig {
        width: 321,
        height: 181,
        format: PixelFormat::Nv12,
    };
    let Reconfigured::InPlace {
        config: got,
        config_gen,
    } = p.configure(smaller).unwrap()
    else {
        panic!("320x180 fits the 640x360 slots");
    };
    assert_eq!((got.width, got.height, config_gen), (320, 180, 2));
    let c = r.config().unwrap();
    assert_eq!((c.config_gen, c.width, c.height), (2, 320, 180));
    // RGBA8 at the same size also fits (230 KB of 415 KB slots).
    let rgba = PreviewConfig {
        format: PixelFormat::Rgba8,
        ..got
    };
    assert!(matches!(
        p.configure(rgba).unwrap(),
        Reconfigured::InPlace { config_gen: 3, .. }
    ));
    let l = p.preview_layout().unwrap();
    let mut src = vec![0u8; l.len()];
    let mut dst = vec![0u8; r.slot_capacity()];
    publish_seq(&mut p, &l, &mut src);
    let m = r.read_latest_into(&mut dst, 0).unwrap().unwrap();
    assert_eq!((m.config_gen, m.layout), (3, l));
}

#[test]
fn preview_requests_are_fitted() {
    let fit = |w, h, format| {
        PreviewConfig {
            width: w,
            height: h,
            format,
        }
        .fitted()
    };
    let nv = PixelFormat::Nv12;
    assert_eq!((fit(0, 0, nv).width, fit(0, 0, nv).height), (2, 2));
    assert_eq!(fit(0, 0, PixelFormat::Rgba8).width, 1);
    let huge = fit(16384, 4096, nv);
    assert_eq!((huge.width, huge.height), (8192, 2048));
    let tall = fit(1001, 20000, PixelFormat::Rgba8);
    assert_eq!((tall.width, tall.height), (410, 8192));
}

#[test]
fn configure_beyond_capacity_moves_to_a_successor_ring() {
    let mut p = FramePublisher::create(&config(64, 64, PixelFormat::Rgba8, 4)).unwrap();
    let clock = p.clock_writer();
    let sample = ClockSample::now(Time::from_secs(3).unwrap(), ClockState::Playing, 2000);
    assert!(clock.publish(&sample));
    let old_name = p.name().clone();
    let r = open(&p);
    let l = p.preview_layout().unwrap();
    let mut src = vec![0u8; 1 << 20];
    let mut dst = vec![0u8; 1 << 20];
    for _ in 0..5 {
        publish_seq(&mut p, &l, &mut src);
    }
    p.add_dropped(2);

    let bigger = PreviewConfig {
        width: 200,
        height: 100,
        format: PixelFormat::Rgba8,
    };
    let Reconfigured::Moved {
        config_gen, ring, ..
    } = p.configure(bigger).unwrap()
    else {
        panic!("200x100 RGBA8 cannot fit 64x64 slots");
    };
    assert_eq!(config_gen, 2);
    assert_ne!(ring.name, old_name);
    assert_eq!(&ring.name, p.name());
    assert!(p.slot_capacity() >= 200 * 4 * 100);

    // The attached reader learns about the move from the old ring itself.
    assert_eq!(r.writer_state(), Some(WriterState::Retired));
    assert_eq!(r.successor(), Some((ring.name.clone(), ring.epoch)));
    let r2 = r.follow().unwrap().unwrap();
    assert_eq!(r2.name(), &ring.name);
    assert_eq!(r2.epoch(), ring.epoch);
    // State carried over: seq continues, clock, drop count, configuration.
    assert_eq!(r2.write_index(), 5);
    assert_eq!(
        r2.read_latest_into(&mut dst, 0),
        Ok(None),
        "no frame in the new ring yet"
    );
    assert_eq!(r2.clock().unwrap(), sample);
    assert_eq!(r2.header().frames_dropped.load(Ordering::Relaxed), 2);
    let c = r2.config().unwrap();
    assert_eq!((c.config_gen, c.width, c.height), (2, 200, 100));
    let l2 = p.preview_layout().unwrap();
    assert_eq!(publish_seq(&mut p, &l2, &mut src), 6);
    let m = r2.read_latest_into(&mut dst, 5).unwrap().unwrap();
    assert_eq!((m.seq, m.config_gen), (6, 2));
    verify(&m, &dst, Some(&l2)).unwrap();

    // Clock writers follow the move.
    let later = ClockSample::now(Time::from_secs(4).unwrap(), ClockState::Stopped, 1000);
    assert!(clock.publish(&later));
    assert_eq!(r2.clock().unwrap(), later);
    assert_eq!(
        r.clock().unwrap(),
        sample,
        "the retired ring no longer changes"
    );

    // The old ring's name is released; only `r`'s own mapping keeps the memory alive.
    #[cfg(unix)]
    assert!(!SharedMemory::exists(&old_name));
    drop(r);
    assert!(!SharedMemory::exists(&old_name));

    // A second, smaller-than-1.5x growth gets 1.5x anyway (no move per drag step).
    let cap = p.slot_capacity();
    let Reconfigured::Moved { .. } = p
        .configure(PreviewConfig {
            width: 202,
            height: 102,
            format: PixelFormat::Rgba8,
        })
        .unwrap()
    else {
        panic!("needs more than the current capacity");
    };
    assert!(p.slot_capacity() >= cap * 3 / 2);
    assert!(matches!(
        p.configure(PreviewConfig {
            width: 220,
            height: 110,
            format: PixelFormat::Rgba8
        })
        .unwrap(),
        Reconfigured::InPlace { .. }
    ));
}

#[test]
fn clock_heartbeat_and_close_semantics() {
    let p = FramePublisher::create(&config(16, 16, PixelFormat::Rgba8, 4)).unwrap();
    let name = p.name().clone();
    let r = open(&p);
    let clock = p.clock_writer();
    let s = ClockSample::now(Time::from_secs(1).unwrap(), ClockState::Playing, 1000);
    assert!(clock.publish(&s));
    let got = r.clock().unwrap();
    assert_eq!(got, s);
    assert!(got.extrapolate(host_clock_ns()) >= s.position.flicks());

    let hb = r.heartbeat_ns();
    std::thread::sleep(StdDuration::from_millis(2));
    p.heartbeat();
    assert!(r.heartbeat_ns() > hb);
    assert!(r.writer_alive(host_clock_ns(), HEARTBEAT_TIMEOUT_NS));

    p.close();
    assert_eq!(r.writer_state(), Some(WriterState::Closed));
    assert!(!r.writer_alive(host_clock_ns(), HEARTBEAT_TIMEOUT_NS));
    assert!(!clock.publish(&s), "clock writers stop with the publisher");
    #[cfg(unix)]
    assert!(!SharedMemory::exists(&name), "creator unlinked the name");
    drop(r);
    assert!(
        !SharedMemory::exists(&name),
        "nothing left once the reader unmaps"
    );
}

/// Run a clock pump for `run`; returns (samples, Hz) and checks the reader saw it advance.
fn run_clock_pump(run: StdDuration) -> (u64, f64) {
    let p = FramePublisher::create(&config(16, 16, PixelFormat::Rgba8, 4)).unwrap();
    let r = open(&p);
    let start = Instant::now();
    let pump = ClockPump::spawn(p.clock_writer(), DEFAULT_CLOCK_PERIOD, move || {
        let ms = i64::try_from(start.elapsed().as_millis()).unwrap();
        ClockSample::now(
            Time::from_flicks(ms * (layout::FLICKS_PER_SECOND / 1000)).unwrap(),
            ClockState::Playing,
            1000,
        )
    })
    .unwrap();
    std::thread::sleep(run);
    let n = pump.stop();
    let hz = n as f64 / start.elapsed().as_secs_f64();
    println!(
        "clock pump: {n} samples in {:?} = {hz:.0} Hz",
        start.elapsed()
    );
    assert!(
        r.clock().unwrap().position > Time::ZERO,
        "reader sees the clock advance"
    );
    // The pump also stops by itself once the publisher is gone.
    let pump = ClockPump::spawn(p.clock_writer(), DEFAULT_CLOCK_PERIOD, || {
        ClockSample::stopped(Time::ZERO)
    })
    .unwrap();
    drop(p);
    let deadline = Instant::now() + StdDuration::from_secs(10);
    while pump.thread_running() && Instant::now() < deadline {
        std::thread::sleep(StdDuration::from_millis(1));
    }
    assert!(
        !pump.thread_running(),
        "pump exits when the publisher is dropped"
    );
    (n, hz)
}

#[test]
fn clock_pump_publishes_periodically_and_stops_with_the_publisher() {
    let (n, _) = run_clock_pump(StdDuration::from_millis(300));
    // Functional check only: rates are scheduler-bound on a shared machine (the ignored
    // timing test below checks the 200 Hz budget).
    assert!(n >= 10, "{n} samples");
}

/// WBS budget: the audio clock word is updated at >= 200 Hz.
#[test]
#[ignore = "timing benchmark; run explicitly on a quiet machine"]
fn bench_clock_pump_rate_at_least_200_hz() {
    let (_, hz) = run_clock_pump(StdDuration::from_secs(2));
    assert!(hz >= 200.0, "clock published at {hz:.0} Hz");
}

// ---------------------------------------------------------------------------------------
// Threaded publish-while-read stress
// ---------------------------------------------------------------------------------------

/// Deterministic writer schedule for the small-frame stress: phase changes every 64 frames,
/// cycling through sizes and formats; two of them outgrow the slots (ring moves).
fn stress_phase(seq: u64) -> PreviewConfig {
    const PHASES: [(u32, u32, PixelFormat); 6] = [
        (64, 48, PixelFormat::Rgba8),
        (96, 64, PixelFormat::Nv12),
        (160, 90, PixelFormat::Rgba8),
        (48, 32, PixelFormat::Nv12),
        (256, 144, PixelFormat::Rgba8),
        (128, 72, PixelFormat::Nv12),
    ];
    let (width, height, format) = PHASES[usize::try_from((seq / 64) % 6).unwrap()];
    PreviewConfig {
        width,
        height,
        format,
    }
}

/// Layout of frame `seq` under the stress schedule: every 3rd frame is a packed half-size
/// LOWRES frame (scrub preview), the others use the publisher's aligned preview layout.
fn stress_layout(seq: u64) -> FrameLayout {
    let c = stress_phase(seq);
    if meta(seq).flags & FLAG_LOWRES != 0 {
        FrameLayout::packed((c.width / 2) & !1, (c.height / 2) & !1, c.format).unwrap()
    } else {
        FrameLayout::aligned(c.width, c.height, c.format, 256).unwrap()
    }
}

#[derive(Default, Debug)]
struct ReaderStats {
    frames: u64,
    torn_retries: u64,
    follows: u64,
    reopened: u64,
}

fn stress_reader(
    announce: &Mutex<RingAnnouncement>,
    stop: &AtomicBool,
    attached: &AtomicU64,
    expect_layout: bool,
) -> ReaderStats {
    // "Ask the engine" for its current ring; it may be mid-move, so retry briefly.
    let open_current = || {
        let deadline = Instant::now() + StdDuration::from_secs(10);
        loop {
            let a = announce.lock().unwrap().clone();
            match TestReader::open(&a.name, Some(a.epoch)) {
                Ok(r) => return r,
                Err(e) if Instant::now() > deadline => panic!("cannot open {}: {e}", a.name),
                Err(_) => std::thread::sleep(StdDuration::from_millis(1)),
            }
        }
    };
    let mut r = open_current();
    attached.fetch_add(1, Ordering::Release);
    let mut buf = vec![0u8; 8 << 20];
    let mut stats = ReaderStats::default();
    let mut last = 0u64;
    loop {
        let stopping = stop.load(Ordering::Acquire);
        match r.read_latest_into(&mut buf, last) {
            Ok(Some(m)) => {
                let want = expect_layout.then(|| stress_layout(m.seq));
                if let Err(e) = verify(&m, &buf, want.as_ref()) {
                    panic!("corrupt frame escaped the seqlock: {e}");
                }
                assert!(m.seq > last, "seq went backwards: {} after {last}", m.seq);
                last = m.seq;
                stats.frames += 1;
            }
            Ok(None) | Err(ReadError::Empty | ReadError::Busy) => {}
            Err(e) => panic!("{e:?}"),
        }
        if let Some(next) = r.follow() {
            stats.torn_retries += r.torn_retries.get();
            r = if let Ok(next) = next {
                stats.follows += 1;
                next
            } else {
                // The successor was itself replaced before we got there: ask "the engine".
                stats.reopened += 1;
                open_current()
            };
        }
        if stopping {
            break;
        }
    }
    stats.torn_retries += r.torn_retries.get();
    stats
}

/// A clock sample whose fields are all functions of `n`, so a torn read is detectable.
fn clock_sample(n: i64) -> ClockSample {
    ClockSample {
        position: Time::from_flicks(n).unwrap(),
        host_ns: u64::try_from(n).unwrap() * 7 + 3,
        rate_milli: i32::try_from(n % 4001).unwrap() - 2000,
        state: ClockState::Playing,
    }
}

/// Publish self-consistent clock samples as fast as possible (writer `k` of two).
fn clock_hammer(w: ClockWriter, k: i64, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut n = k;
        while !stop.load(Ordering::Relaxed) {
            n += 2;
            if !w.publish(&clock_sample(n)) {
                break;
            }
        }
    })
}

/// Read the clock of the announced ring in a loop; every sample must be self-consistent.
fn clock_checker(
    announce: Arc<Mutex<RingAnnouncement>>,
    stop: Arc<AtomicBool>,
    ok: Arc<AtomicU64>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let a = announce.lock().unwrap().clone();
            let Ok(r) = TestReader::open(&a.name, Some(a.epoch)) else {
                continue;
            };
            for _ in 0..2000 {
                let Some(s) = r.clock() else { continue };
                // 0 is the initial (or carried-over initial) sample.
                if s.position.flicks() != 0 {
                    assert_eq!(s, clock_sample(s.position.flicks()), "torn clock read");
                    ok.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    })
}

#[test]
fn stress_publish_while_read_small_frames_with_resizes_and_moves() {
    let run = StdDuration::from_millis(stress_ms(1500));
    let mut p = FramePublisher::create(&RingConfig {
        prefault: false,
        ..config(64, 48, PixelFormat::Rgba8, 4)
    })
    .unwrap();
    let announce = Arc::new(Mutex::new(p.announcement()));
    let stop = Arc::new(AtomicBool::new(false));

    // Two clock writers hammer the clock seqlock with self-consistent samples.
    let mut threads: Vec<_> = (0..2)
        .map(|k| clock_hammer(p.clock_writer(), k, Arc::clone(&stop)))
        .collect();
    let clock_ok = Arc::new(AtomicU64::new(0));
    threads.push(clock_checker(
        Arc::clone(&announce),
        Arc::clone(&stop),
        Arc::clone(&clock_ok),
    ));
    let attached = Arc::new(AtomicU64::new(0));
    let readers: Vec<_> = (0..3)
        .map(|_| {
            let (announce, stop, attached) = (
                Arc::clone(&announce),
                Arc::clone(&stop),
                Arc::clone(&attached),
            );
            std::thread::spawn(move || stress_reader(&announce, &stop, &attached, true))
        })
        .collect();
    // Every reader is attached to the first ring before anything moves.
    while attached.load(Ordering::Acquire) < 3 {
        std::thread::yield_now();
    }

    let mut src = vec![0u8; 1 << 20];
    let (mut moves, mut configs) = (0u32, 0u32);
    let mut phase = p.preview().0;
    let start = Instant::now();
    while start.elapsed() < run {
        let seq = p.write_index() + 1;
        let want = stress_phase(seq);
        if want != phase {
            configs += 1;
            if let Reconfigured::Moved { ring, .. } = p.configure(want).unwrap() {
                moves += 1;
                *announce.lock().unwrap() = ring;
                // Moves are rare in real life; give readers a moment to follow this one.
                std::thread::sleep(StdDuration::from_millis(20));
            }
            phase = want;
        }
        assert_eq!(publish_seq(&mut p, &stress_layout(seq), &mut src), seq);
        if seq.is_multiple_of(128) {
            p.heartbeat();
        }
    }
    let published = p.write_index();
    stop.store(true, Ordering::Release);
    let stats: Vec<ReaderStats> = readers.into_iter().map(|t| t.join().unwrap()).collect();
    for t in threads {
        t.join().unwrap();
    }
    println!(
        "small-frame stress: {published} frames in {:?}, {configs} reconfigurations, {moves} moves; \
         readers {stats:?}; {} consistent clock reads",
        start.elapsed(),
        clock_ok.load(Ordering::Relaxed)
    );
    assert!(moves >= 2, "schedule outgrows the slots twice");
    for s in &stats {
        assert!(s.frames > 100, "reader made progress: {s:?}");
        assert!(
            s.follows + s.reopened >= 1,
            "reader followed the moves: {s:?}"
        );
    }
    assert!(clock_ok.load(Ordering::Relaxed) > 1000);
}

fn stress_ms(default: u64) -> u64 {
    std::env::var("CW_SHM_STRESS_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

/// Publish 1080p RGBA8 (8.3 MB) frames as fast as possible while two readers copy them out.
fn stress_1080p(run: StdDuration) -> (u64, Vec<f64>, Vec<ReaderStats>) {
    let mut p = FramePublisher::create(&config(1920, 1080, PixelFormat::Rgba8, 6)).unwrap();
    let l = p.preview_layout().unwrap();
    let announce = Arc::new(Mutex::new(p.announcement()));
    let stop = Arc::new(AtomicBool::new(false));
    let attached = Arc::new(AtomicU64::new(0));
    let readers: Vec<_> = (0..2)
        .map(|_| {
            let (announce, stop, attached) = (
                Arc::clone(&announce),
                Arc::clone(&stop),
                Arc::clone(&attached),
            );
            std::thread::spawn(move || stress_reader(&announce, &stop, &attached, false))
        })
        .collect();
    let mut src = vec![0u8; l.len()];
    let mut times = Vec::new();
    let start = Instant::now();
    while start.elapsed() < run {
        let seq = p.write_index() + 1;
        fill(&mut src, seq); // the "GPU readback", outside the timed region
        let t = Instant::now();
        p.publish(&meta(seq), &l, &src).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1e3);
    }
    stop.store(true, Ordering::Release);
    let stats = readers.into_iter().map(|t| t.join().unwrap()).collect();
    times.sort_by(f64::total_cmp);
    (p.write_index(), times, stats)
}

#[test]
fn stress_publish_while_read_1080p_rgba() {
    let (published, times, stats) = stress_1080p(StdDuration::from_millis(stress_ms(1500)));
    println!(
        "1080p RGBA8 stress: {published} frames, publish p50 {:.2} ms p95 {:.2} ms max {:.2} ms; readers {stats:?}",
        percentile(&times, 0.5),
        percentile(&times, 0.95),
        times.last().copied().unwrap_or_default(),
    );
    assert!(published > 20);
    for s in &stats {
        assert!(s.frames > 5, "{s:?}");
    }
    // Sanity bound only (catches e.g. a page fault per frame); the 3 ms budget is checked by
    // the ignored bench below.
    assert!(percentile(&times, 0.95) < 100.0);
}

/// WBS budget: publish of a 1080p RGBA frame p95 < 3 ms (run with `--run-ignored all` on a
/// quiet machine; timing-sensitive, so not part of the default suite).
#[test]
#[ignore = "timing benchmark; run explicitly on a quiet machine"]
fn bench_publish_1080p_rgba_p95_under_3ms() {
    let (published, times, _) = stress_1080p(StdDuration::from_secs(3));
    let p95 = percentile(&times, 0.95);
    println!(
        "publish 1080p RGBA8: {published} frames, p50 {:.2} ms, p95 {p95:.2} ms",
        percentile(&times, 0.5)
    );
    assert!(p95 < 3.0, "p95 {p95:.2} ms");
}

// ---------------------------------------------------------------------------------------
// Multi-process: a separate writer process (this binary, re-run as `child_writer`)
// ---------------------------------------------------------------------------------------

const CHILD_ENV: &str = "CW_SHM_CHILD";

fn spawn_child(mode: &str) -> (Child, mpsc::Receiver<String>) {
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .args([
            "shm::tests::child_writer",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let out = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    (child, rx)
}

/// Wait for the child's `CWRING <name> <epoch>` line.
fn child_ring(rx: &mpsc::Receiver<String>) -> (RingName, u64) {
    let deadline = Instant::now() + StdDuration::from_secs(60);
    while Instant::now() < deadline {
        // libtest prints "test … ... " without a newline before the test's own output.
        if let Ok(line) = rx.recv_timeout(StdDuration::from_millis(100))
            && let Some((_, rest)) = line.split_once("CWRING ")
        {
            let (name, epoch) = rest.split_once(' ').unwrap();
            return (RingName::parse(name).unwrap(), epoch.parse().unwrap());
        }
    }
    panic!("child writer never announced its ring");
}

/// Entry point of the writer process. A no-op unless `CW_SHM_CHILD` is set:
/// - `stream:<secs>`: 1080p RGBA8 at 60 fps with a 500 Hz clock pump, then a clean close;
/// - `crash`: publish a few frames and wait to be killed.
#[test]
fn child_writer() {
    let Ok(mode) = std::env::var(CHILD_ENV) else {
        return;
    };
    let mut p = FramePublisher::create(&config(1920, 1080, PixelFormat::Rgba8, 6)).unwrap();
    let l = p.preview_layout().unwrap();
    let mut src = vec![0u8; l.len()];
    let start = Instant::now();
    let pump = ClockPump::spawn(p.clock_writer(), DEFAULT_CLOCK_PERIOD, move || {
        let ns = i64::try_from(start.elapsed().as_nanos()).unwrap();
        let flicks =
            i64::try_from(i128::from(ns) * i128::from(layout::FLICKS_PER_SECOND) / 1_000_000_000)
                .unwrap();
        ClockSample::now(
            Time::from_flicks(flicks).unwrap(),
            ClockState::Playing,
            1000,
        )
    })
    .unwrap();
    // Publish before announcing, so readers never see an empty ring (crash mode: 3 frames).
    let initial = if mode == "crash" { 3 } else { 1 };
    for _ in 0..initial {
        publish_seq(&mut p, &l, &mut src);
    }
    println!("CWRING {} {}", p.name(), p.epoch());
    std::io::stdout().flush().unwrap();
    if mode == "crash" {
        std::thread::sleep(StdDuration::from_secs(120));
        return;
    }
    let secs: u64 = mode
        .strip_prefix("stream:")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    // Clock-driven: frame N is published when the clock enters it (S1 finding 4). Its pts is
    // the frame's slot on the clock timeline, which the reader's clock scheduling relies on.
    let frame = StdDuration::from_nanos(1_000_000_000 / 60);
    let mut n = 1u64;
    while start.elapsed() < StdDuration::from_secs(secs) {
        let due = frame * u32::try_from(n).unwrap();
        if let Some(wait) = due.checked_sub(start.elapsed()) {
            std::thread::sleep(wait);
        }
        n += 1;
        publish_seq(&mut p, &l, &mut src);
        p.heartbeat();
    }
    println!("CWDONE {}", p.write_index());
    std::io::stdout().flush().unwrap();
    drop(pump);
    p.close();
}

#[test]
fn multiprocess_1080p60_stream_has_no_torn_frames_and_monotonic_seq() {
    let secs = std::env::var("CW_SHM_STREAM_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3u64);
    let (mut child, rx) = spawn_child(&format!("stream:{secs}"));
    let (name, epoch) = child_ring(&rx);
    let r = TestReader::open(&name, Some(epoch)).unwrap();
    let mut buf = vec![0u8; r.slot_capacity()];
    let (mut last, mut frames, mut sched_frames, mut sched_last) = (0u64, 0u64, 0u64, 0u64);
    let mut seen = BTreeMap::new();
    let mut copy_ms = Vec::new();
    let mut done = None;
    let start = Instant::now();
    while done.is_none() && start.elapsed() < StdDuration::from_secs(secs + 30) {
        // Latest-wins poll, ~2 ms (what a waiting reader would see).
        let t = Instant::now();
        if let Ok(Some(m)) = r.read_latest_into(&mut buf, last) {
            copy_ms.push(t.elapsed().as_secs_f64() * 1e3);
            verify(&m, &buf, None).unwrap();
            assert!(m.seq > last);
            last = m.seq;
            frames += 1;
            *seen.entry(m.seq).or_insert(0u32) += 1;
        }
        // Clock-scheduled choice as the present loop does it (20 ms latency).
        let now = host_clock_ns();
        if let Some(c) = r.clock() {
            let target = c.extrapolate(now) - layout::FLICKS_PER_SECOND / 50;
            if let Ok(Some(m)) = r.read_at_clock_into(&mut buf, target, sched_last) {
                verify(&m, &buf, None).unwrap();
                assert!(m.seq > sched_last && m.pts <= target);
                sched_last = m.seq;
                sched_frames += 1;
            }
        }
        if let Ok(line) = rx.try_recv()
            && let Some((_, n)) = line.split_once("CWDONE ")
        {
            done = n.parse::<u64>().ok();
        }
        std::thread::sleep(StdDuration::from_millis(2));
    }
    let status = child.wait().unwrap();
    assert!(status.success(), "writer exited with {status}");
    let published = done.expect("writer finished");
    copy_ms.sort_by(f64::total_cmp);
    println!(
        "multi-process 1080p60 RGBA8, {secs} s: published {published}, latest-wins reads {frames} \
         (distinct {}), clock-scheduled reads {sched_frames}, torn retries {}, reader copy p50 {:.2} ms p95 {:.2} ms",
        seen.len(),
        r.torn_retries.get(),
        percentile(&copy_ms, 0.5),
        percentile(&copy_ms, 0.95),
    );
    assert_eq!(r.writer_state(), Some(WriterState::Closed));
    // Correctness is asserted per frame above; these only prove the reads overlapped the
    // whole stream (a loaded machine slows this debug-build reader, so keep them loose).
    assert!(
        frames * 4 >= published,
        "reader read along: {frames}/{published}"
    );
    assert!(
        sched_frames * 4 >= published,
        "clock scheduling read along: {sched_frames}"
    );
}

#[test]
fn killed_writer_leaves_no_ring_behind_after_startup_reap() {
    // Reader attached while the writer is killed: it sees a stale heartbeat, then cleanup.
    let (mut child, rx) = spawn_child("crash");
    let (name, epoch) = child_ring(&rx);
    let r = TestReader::open(&name, Some(epoch)).unwrap();
    assert!(r.writer_alive(host_clock_ns(), 2 * HEARTBEAT_TIMEOUT_NS));
    assert_eq!(name.pid(), child.id());
    child.kill().unwrap(); // TerminateProcess / SIGKILL: no cleanup code runs
    child.wait().unwrap();
    std::thread::sleep(StdDuration::from_millis(300));
    assert_eq!(
        r.writer_state(),
        Some(WriterState::Live),
        "a killed writer cannot say goodbye"
    );
    assert!(
        !r.writer_alive(host_clock_ns(), 250_000_000),
        "stale heartbeat reveals the crash"
    );
    let mut buf = vec![0u8; r.slot_capacity()];
    assert_eq!(
        r.read_latest_into(&mut buf, 0).unwrap().unwrap().seq,
        3,
        "last frames readable"
    );
    reap_and_check_gone(&name);
    drop(r);
    assert!(!SharedMemory::exists(&name));

    // Nobody attached: the object must not outlive the next start's reap.
    let (mut child, rx) = spawn_child("crash");
    let (name, _) = child_ring(&rx);
    child.kill().unwrap();
    child.wait().unwrap();
    reap_and_check_gone(&name);
    assert!(!SharedMemory::exists(&name));
}

/// What the next engine start does about a dead engine's ring.
fn reap_and_check_gone(name: &RingName) {
    let reaped = reap_stale_rings().unwrap();
    #[cfg(target_os = "linux")]
    assert!(reaped.contains(name), "reap found {name}: {reaped:?}");
    #[cfg(not(target_os = "linux"))]
    let _ = reaped;
    // macOS has no enumeration: reap by name (from the stale run-file). Windows: no-op.
    let removed = remove_stale_ring(name).unwrap();
    #[cfg(target_vendor = "apple")]
    assert!(removed);
    #[cfg(not(target_vendor = "apple"))]
    let _ = removed;
    #[cfg(unix)]
    assert!(
        !SharedMemory::exists(name),
        "name removed even while a reader maps it"
    );
}

// ---------------------------------------------------------------------------------------
// The C mirror
// ---------------------------------------------------------------------------------------

#[test]
fn c_header_mirrors_the_rust_layout() {
    let header = include_str!("../../include/cutweave_preview_ring_v1.h");
    let defines: BTreeMap<&str, i128> = header
        .lines()
        .filter_map(|l| l.strip_prefix("#define CWPR_"))
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (name, value) = (it.next()?, it.next()?);
            let value = value.trim_end_matches('u');
            let v = match value.strip_prefix("0x") {
                Some(hex) => i128::from_str_radix(hex, 16).ok()?,
                None => value.parse().ok()?,
            };
            Some((name, v))
        })
        .collect();
    let expected = rust_constants();
    for (name, value) in &expected {
        assert_eq!(defines.get(name), Some(value), "CWPR_{name}");
    }
    assert_eq!(defines.len(), expected.len(), "every C define is checked");
}

/// Every constant the C mirror defines, keyed by its name without the `CWPR_` prefix.
fn rust_constants() -> Vec<(&'static str, i128)> {
    use layout::off;
    let u = |v: usize| i128::try_from(v).unwrap();
    vec![
        ("MAGIC", layout::MAGIC.into()),
        ("LAYOUT_VERSION", LAYOUT_VERSION.into()),
        (
            "KNOWN_REQUIRED_FEATURES",
            layout::KNOWN_REQUIRED_FEATURES.into(),
        ),
        ("HEADER_SIZE", u(layout::HEADER_SIZE)),
        ("SLOT_DESC_SIZE", u(layout::SLOT_DESC_SIZE)),
        ("MIN_SLOTS", MIN_SLOTS.into()),
        ("MAX_SLOTS", MAX_SLOTS.into()),
        ("PAYLOAD_ALIGN", u(layout::PAYLOAD_ALIGN)),
        ("NAME_FIELD_LEN", u(layout::NAME_FIELD_LEN)),
        ("MAX_DIMENSION", layout::MAX_DIMENSION.into()),
        ("FLICKS_PER_SECOND", layout::FLICKS_PER_SECOND.into()),
        ("HEARTBEAT_TIMEOUT_NS", HEARTBEAT_TIMEOUT_NS.into()),
        (
            "MAX_CLOCK_EXTRAPOLATION_NS",
            MAX_CLOCK_EXTRAPOLATION_NS.into(),
        ),
        ("RATE_MILLI_1X", layout::RATE_MILLI_1X.into()),
        ("RING_NAME_LEN", u(RING_NAME_LEN)),
        (
            "WRITER_INITIALIZING",
            (WriterState::Initializing as u32).into(),
        ),
        ("WRITER_LIVE", (WriterState::Live as u32).into()),
        ("WRITER_RETIRED", (WriterState::Retired as u32).into()),
        ("WRITER_CLOSED", (WriterState::Closed as u32).into()),
        ("CLOCK_STOPPED", (ClockState::Stopped as u32).into()),
        ("CLOCK_PLAYING", (ClockState::Playing as u32).into()),
        ("CLOCK_SCRUBBING", (ClockState::Scrubbing as u32).into()),
        (
            "HOST_CLOCK_WINDOWS_QPC",
            (HostClock::WindowsQpc as u32).into(),
        ),
        (
            "HOST_CLOCK_LINUX_MONOTONIC",
            (HostClock::LinuxMonotonic as u32).into(),
        ),
        (
            "HOST_CLOCK_MAC_UPTIME_RAW",
            (HostClock::MacUptimeRaw as u32).into(),
        ),
        ("FORMAT_RGBA8", (PixelFormat::Rgba8 as u32).into()),
        ("FORMAT_NV12", (PixelFormat::Nv12 as u32).into()),
        ("FLAG_LOWRES", FLAG_LOWRES.into()),
        ("OFF_MAGIC", u(off::MAGIC)),
        ("OFF_LAYOUT_VERSION", u(off::LAYOUT_VERSION)),
        ("OFF_REQUIRED_FEATURES", u(off::REQUIRED_FEATURES)),
        ("OFF_HEADER_SIZE", u(off::HEADER_SIZE)),
        ("OFF_SLOT_DESC_SIZE", u(off::SLOT_DESC_SIZE)),
        ("OFF_SLOT_COUNT", u(off::SLOT_COUNT)),
        ("OFF_PAYLOAD_OFFSET", u(off::PAYLOAD_OFFSET)),
        ("OFF_SLOT_STRIDE", u(off::SLOT_STRIDE)),
        ("OFF_SLOT_CAPACITY", u(off::SLOT_CAPACITY)),
        ("OFF_MAPPING_SIZE", u(off::MAPPING_SIZE)),
        ("OFF_RING_EPOCH", u(off::RING_EPOCH)),
        ("OFF_WRITER_PID", u(off::WRITER_PID)),
        ("OFF_HOST_CLOCK", u(off::HOST_CLOCK)),
        ("OFF_WRITE_INDEX", u(off::WRITE_INDEX)),
        ("OFF_WRITER_STATE", u(off::WRITER_STATE)),
        ("OFF_HEARTBEAT_NS", u(off::HEARTBEAT_NS)),
        ("OFF_FRAMES_DROPPED", u(off::FRAMES_DROPPED)),
        ("OFF_CLOCK_LOCK", u(off::CLOCK_LOCK)),
        ("OFF_CLOCK_FLICKS", u(off::CLOCK_FLICKS)),
        ("OFF_CLOCK_HOST_NS", u(off::CLOCK_HOST_NS)),
        ("OFF_CLOCK_RATE_MILLI", u(off::CLOCK_RATE_MILLI)),
        ("OFF_CLOCK_STATE", u(off::CLOCK_STATE)),
        ("OFF_CONFIG_LOCK", u(off::CONFIG_LOCK)),
        ("OFF_CONFIG_GEN", u(off::CONFIG_GEN)),
        ("OFF_PREVIEW_WIDTH", u(off::PREVIEW_WIDTH)),
        ("OFF_PREVIEW_HEIGHT", u(off::PREVIEW_HEIGHT)),
        ("OFF_PREVIEW_FORMAT", u(off::PREVIEW_FORMAT)),
        ("OFF_SUCCESSOR_EPOCH", u(off::SUCCESSOR_EPOCH)),
        ("OFF_SUCCESSOR_NAME", u(off::SUCCESSOR_NAME)),
        ("OFF_SLOT_LOCK", u(off::SLOT_LOCK)),
        ("OFF_SLOT_SEQ", u(off::SLOT_SEQ)),
        ("OFF_SLOT_PTS", u(off::SLOT_PTS)),
        ("OFF_SLOT_DURATION", u(off::SLOT_DURATION)),
        ("OFF_SLOT_REV", u(off::SLOT_REV)),
        ("OFF_SLOT_WIDTH", u(off::SLOT_WIDTH)),
        ("OFF_SLOT_HEIGHT", u(off::SLOT_HEIGHT)),
        ("OFF_SLOT_FORMAT", u(off::SLOT_FORMAT)),
        ("OFF_SLOT_FLAGS", u(off::SLOT_FLAGS)),
        ("OFF_SLOT_CONFIG_GEN", u(off::SLOT_CONFIG_GEN)),
        ("OFF_SLOT_PAYLOAD_LEN", u(off::SLOT_PAYLOAD_LEN)),
        ("OFF_SLOT_PLANE0_OFFSET", u(off::SLOT_PLANE0_OFFSET)),
        ("OFF_SLOT_PLANE1_OFFSET", u(off::SLOT_PLANE1_OFFSET)),
        ("OFF_SLOT_PLANE0_STRIDE", u(off::SLOT_PLANE0_STRIDE)),
        ("OFF_SLOT_PLANE1_STRIDE", u(off::SLOT_PLANE1_STRIDE)),
        ("OFF_SLOT_RENDER_NS", u(off::SLOT_RENDER_NS)),
        ("OFF_SLOT_PUBLISH_NS", u(off::SLOT_PUBLISH_NS)),
    ]
}
