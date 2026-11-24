// SPDX-License-Identifier: GPL-3.0-or-later
//! `DecoderPool` (WP-1D.03) against `FakeBackend`: bounded, reused, LRU-evicting, one index
//! scan per media, and cancellable within the WBS's 10 ms (a waiting checkout and a pending
//! seek). Runs without FFmpeg.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration as Wall, Instant};

use cutweave_media::testing::{FakeMedia, FakeVideo, fake_frame_number};
use cutweave_media::{
    CancelToken, DecodeMode, DecodeOpts, DecoderPool, FakeBackend, MediaBackend, MediaError,
    PixelFormat, VideoDecoder,
};
use cutweave_model::{FrameRate, Time};

const CLIP: &str = "/fake/pool.mp4";
/// The WBS budget for a cancel to take effect.
const CANCEL_BUDGET: Wall = Wall::from_millis(10);

fn fake() -> FakeBackend {
    FakeBackend::new().with_media(
        CLIP,
        FakeMedia::video(FakeVideo::new(32, 18, FrameRate::FPS_30, 600)),
    )
}

fn pool_on(backend: &FakeBackend, max: usize) -> DecoderPool {
    DecoderPool::with_max_live(Arc::new(backend.clone()), CLIP, max)
}

fn at(n: i64) -> Time {
    FrameRate::FPS_30.frame_to_time(n).unwrap()
}

fn seek_n(d: &mut dyn VideoDecoder, n: i64) -> u32 {
    fake_frame_number(&d.seek_exact(at(n)).unwrap()).unwrap()
}

#[test]
fn checkouts_reuse_decoders_and_scan_the_index_once() {
    let backend = fake();
    let pool = DecoderPool::new(Arc::new(backend.clone()), CLIP);
    assert_eq!(pool.max_live(), DecoderPool::DEFAULT_MAX_LIVE);
    assert_eq!((pool.live(), pool.index().is_some()), (0, false));
    let never = CancelToken::new();
    let opts = DecodeOpts::default();
    let mut leases: Vec<_> = (0..4)
        .map(|_| pool.checkout(&opts, None, &never).unwrap())
        .collect();
    for (n, lease) in [0, 10, 20, 30].into_iter().zip(leases.iter_mut()) {
        assert_eq!(i64::from(seek_n(lease, n)), n);
    }
    assert_eq!((pool.live(), pool.idle(), pool.opened()), (4, 0, 4));
    assert_eq!(
        backend.index_scans(),
        1,
        "only the first decoder scans the file"
    );
    assert_eq!(pool.index().unwrap().frame_count(), 600);
    for lease in leases.drain(..2) {
        pool.release(lease);
    }
    drop(leases);
    assert_eq!((pool.live(), pool.idle()), (4, 4));
    // Idle decoders come back instead of new ones.
    let lease = pool.checkout(&opts, None, &never).unwrap();
    assert_eq!((pool.opened(), backend.decoders_opened()), (4, 4));
    drop(lease);
    pool.trim();
    assert_eq!(pool.live(), 0);
    let _lease = pool.checkout(&opts, None, &never).unwrap();
    assert_eq!(
        backend.index_scans(),
        1,
        "the pool keeps the index after a trim"
    );
}

#[test]
fn a_known_index_is_never_rescanned() {
    let backend = fake();
    let index = Arc::new(backend.index_media(Path::new(CLIP)).unwrap());
    let pool = pool_on(&backend, 2).with_index(Arc::clone(&index));
    let mut lease = pool
        .checkout(&DecodeOpts::default(), None, &CancelToken::new())
        .unwrap();
    assert_eq!(seek_n(&mut lease, 42), 42);
    assert_eq!(backend.index_scans(), 1, "only the scan made above");
    assert!(Arc::ptr_eq(&lease.index(), &index));
}

#[test]
fn a_full_pool_makes_checkouts_wait_for_a_release() {
    let backend = fake();
    let pool = pool_on(&backend, 2);
    let never = CancelToken::new();
    let opts = DecodeOpts::default();
    let a = pool.checkout(&opts, None, &never).unwrap();
    let b = pool.checkout(&opts, None, &never).unwrap();
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let waiter = {
        let pool = pool.clone();
        let opts = opts.clone();
        let released = Arc::clone(&released);
        thread::spawn(move || {
            let mut c = pool.checkout(&opts, None, &CancelToken::new()).unwrap();
            let after_release = released.load(Ordering::SeqCst);
            assert_eq!(seek_n(&mut c, 5), 5);
            after_release
        })
    };
    thread::sleep(Wall::from_millis(60));
    assert_eq!(pool.live(), 2, "never more than max_live");
    released.store(true, Ordering::SeqCst);
    drop(a);
    assert!(
        waiter.join().unwrap(),
        "the third checkout got a decoder before one was released"
    );
    assert_eq!(pool.opened(), 2);
    drop(b);
}

#[test]
fn near_picks_the_decoder_that_can_decode_forward() {
    let backend = fake();
    let pool = pool_on(&backend, 4);
    let never = CancelToken::new();
    let opts = DecodeOpts::default();
    let mut first = pool.checkout(&opts, None, &never).unwrap();
    let mut second = pool.checkout(&opts, None, &never).unwrap();
    seek_n(&mut first, 100);
    seek_n(&mut second, 300);
    assert_eq!(first.position(), Some(at(100)));
    drop(first);
    drop(second);
    let for_305 = pool.checkout(&opts, Some(at(305)), &never).unwrap();
    assert_eq!(
        for_305.position(),
        Some(at(300)),
        "positioned just before 305"
    );
    let for_120 = pool.checkout(&opts, Some(at(120)), &never).unwrap();
    assert_eq!(
        for_120.position(),
        Some(at(100)),
        "positioned just before 120"
    );
    drop(for_305);
    drop(for_120);
    // Without a hint: the most recently released.
    let recent = pool.checkout(&opts, None, &never).unwrap();
    assert_eq!(recent.position(), Some(at(100)));
}

#[test]
fn other_options_evict_the_least_recently_used_idle_decoder() {
    let backend = fake();
    let pool = pool_on(&backend, 2);
    let never = CancelToken::new();
    let native = DecodeOpts::default();
    let rgba = DecodeOpts::default().with_target(PixelFormat::Rgba8);
    let mut a = pool.checkout(&native, None, &never).unwrap();
    let mut b = pool.checkout(&native, None, &never).unwrap();
    seek_n(&mut a, 1);
    seek_n(&mut b, 2);
    drop(a); // released first: least recently used
    drop(b);
    let mut thumbs = pool.checkout(&rgba, None, &never).unwrap();
    assert_eq!(pool.evicted(), 1);
    assert_eq!(pool.live(), 2);
    let f = thumbs.seek_exact(at(7)).unwrap();
    assert_eq!(f.format, PixelFormat::Rgba8);
    assert_eq!(thumbs.opts().target_pixfmt, Some(PixelFormat::Rgba8));
    // The surviving native decoder is the one used last.
    let kept = pool.checkout(&native, None, &never).unwrap();
    assert_eq!(kept.position(), Some(at(2)));
    // Modes are options too: a sequential checkout does not take a seek decoder.
    drop(kept);
    drop(thumbs);
    let seq = pool
        .checkout(
            &native.clone().with_mode(DecodeMode::Sequential),
            None,
            &never,
        )
        .unwrap();
    assert_eq!(seq.position(), None, "a new decoder");
    assert_eq!(pool.evicted(), 2);
}

#[test]
fn a_waiting_checkout_is_cancelled_within_10_ms() {
    let pool = pool_on(&fake(), 1);
    let held = pool
        .checkout(&DecodeOpts::default(), None, &CancelToken::new())
        .unwrap();
    let mut latencies = Vec::new();
    let mut token = CancelToken::new();
    for _ in 0..5 {
        token = CancelToken::new();
        let waiter = {
            let pool = pool.clone();
            let token = token.clone();
            thread::spawn(move || {
                let r = pool.checkout(&DecodeOpts::default(), None, &token);
                (r.map(|_| ()), Instant::now())
            })
        };
        thread::sleep(Wall::from_millis(30));
        let cancelled_at = Instant::now();
        token.cancel();
        let (result, returned_at) = waiter.join().unwrap();
        assert_eq!(result, Err(MediaError::Cancelled));
        latencies.push(returned_at.duration_since(cancelled_at));
    }
    latencies.sort();
    eprintln!("waiting checkout cancel latencies: {latencies:?}");
    assert!(
        latencies[latencies.len() / 2] < CANCEL_BUDGET,
        "waiting checkouts took {latencies:?} to cancel"
    );
    assert_eq!(pool.live(), 1);
    drop(held);
    // An already-cancelled token never gets a decoder.
    assert_eq!(
        pool.checkout(&DecodeOpts::default(), None, &token).err(),
        Some(MediaError::Cancelled)
    );
}

#[test]
fn a_pending_seek_is_cancelled_within_10_ms() {
    // 20 ms a frame: a seek to the end of a 12-frame GOP decodes for about 240 ms.
    let backend = fake().with_frame_cost(Wall::from_millis(20));
    let pool = pool_on(&backend, 2);
    let mut latencies = Vec::new();
    for round in 0..7_u64 {
        let token = CancelToken::new();
        let mut lease = pool.checkout(&DecodeOpts::default(), None, &token).unwrap();
        let seeker = thread::spawn(move || {
            let r = lease.seek_exact(at(11 + 120 * i64::try_from(round % 5).unwrap()));
            (r.map(|_| ()), Instant::now(), lease)
        });
        thread::sleep(Wall::from_millis(30 + 17 * round));
        let cancelled_at = Instant::now();
        token.cancel();
        let (result, returned_at, mut lease) = seeker.join().unwrap();
        assert_eq!(result, Err(MediaError::Cancelled));
        latencies.push(returned_at.duration_since(cancelled_at));
        // The decoder is still good with a fresh token, and exact.
        lease.set_cancel(Some(CancelToken::new()));
        assert_eq!(seek_n(&mut lease, 13), 13);
    }
    // The median, so that a scheduler hiccup on a loaded machine does not fail the run; the
    // worst case is printed (the cancel itself is a 1 ms poll).
    latencies.sort();
    eprintln!("pending fake seek cancel latencies: {latencies:?}");
    let median = latencies[latencies.len() / 2];
    assert!(
        median < CANCEL_BUDGET,
        "pending seeks took {latencies:?} to cancel"
    );
}

#[test]
fn discarded_and_failed_decoders_leave_the_pool() {
    let backend = fake();
    let pool = pool_on(&backend, 2);
    let never = CancelToken::new();
    let lease = pool.checkout(&DecodeOpts::default(), None, &never).unwrap();
    lease.discard();
    assert_eq!(pool.live(), 0);
    // Out of range is the caller's mistake, not the decoder's: it stays pooled.
    let mut lease = pool.checkout(&DecodeOpts::default(), None, &never).unwrap();
    assert!(matches!(
        lease.seek_exact(at(10_000)),
        Err(MediaError::OutOfRange { .. })
    ));
    drop(lease);
    assert_eq!(pool.idle(), 1);
}

#[test]
fn open_errors_pass_through_and_free_the_slot() {
    let backend = FakeBackend::new();
    let pool = DecoderPool::with_max_live(Arc::new(backend), "/no/such/file.mp4", 1);
    for _ in 0..3 {
        let err = pool
            .checkout(&DecodeOpts::default(), None, &CancelToken::new())
            .err();
        assert!(matches!(err, Some(MediaError::NotFound { .. })), "{err:?}");
    }
    assert_eq!(pool.live(), 0);
}

#[test]
fn threads_share_a_pool_without_exceeding_it() {
    let backend = fake().with_frame_cost(Wall::from_micros(200));
    let pool = pool_on(&backend, 3);
    let peak = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..8_u64)
        .map(|t| {
            let pool = pool.clone();
            let peak = Arc::clone(&peak);
            thread::spawn(move || {
                let mut x = t.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
                for _ in 0..25 {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    let n = i64::try_from(x % 600).unwrap();
                    let mut lease = pool
                        .checkout(&DecodeOpts::default(), Some(at(n)), &CancelToken::new())
                        .unwrap();
                    peak.fetch_max(pool.live(), Ordering::Relaxed);
                    assert_eq!(seek_n(&mut lease, n), u32::try_from(n).unwrap());
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert!(peak.load(Ordering::Relaxed) <= 3, "live peaked at {peak:?}");
    assert!(pool.opened() <= 3);
    assert_eq!(backend.index_scans(), 1);
}
