// SPDX-License-Identifier: GPL-3.0-or-later
//! Transport acceptance tests (WP-1H.01 per R31): the synthetic source, a `ManualClock` and a
//! virtual host clock drive the render loop deterministically; a few tests run the real threads
//! (and the null audio output) to check the same properties under real concurrency.

// Timing statistics and test arithmetic convert freely between integer types.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_lines
)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration as StdDuration, Instant};

use cutweave_audio::{AudioClock, ManualClock};
use cutweave_commands::{Access, CmdError, Registry};
use cutweave_model::{Duration, EntityKind, FrameRate, MediaId, SampleRate, Time};
use cutweave_render::{AutoQualityConfig, Quality, Scale};
use serde_json::json;

use super::*;
use crate::shm::{ClockState, FrameLayout, PixelFormat, PreviewConfig};

const MS: u64 = 1_000_000;
const FLICKS_PER_MS: i64 = 705_600;
const FRAME_30: i64 = 23_520_000;
const FRAME_25: i64 = 28_224_000;

/// The marker offset of a target's frames: a source's test pattern carries `frame + (n << 32)`
/// for media `n`, so every delivered frame says which target it shows.
fn target_offset(target: PlaybackTarget) -> i64 {
    match target {
        PlaybackTarget::Sequence => 0,
        PlaybackTarget::Media(id) => (id.as_u128() as i64) << 32,
    }
}

/// The test sources: media 1 to 3.
fn media(n: u128) -> MediaId {
    MediaId::from_u128(n)
}

/// A source of `secs` seconds at `fps`.
fn source(n: u128, secs: i64, fps: FrameRate) -> SourceInfo {
    SourceInfo {
        media: media(n),
        duration: Duration::from_secs(secs).unwrap(),
        fps,
    }
}

/// Resolves media 1 (10 s at 25 fps), 2 (4 s at 30 fps) and 3 (2 s at 24 fps).
struct TestResolver;

impl SourceResolver for TestResolver {
    fn resolve(&self, id: MediaId) -> Result<SourceInfo, CmdError> {
        match id.as_u128() {
            1 => Ok(source(1, 10, FrameRate::FPS_25)),
            2 => Ok(source(2, 4, FrameRate::FPS_30)),
            3 => Ok(source(3, 2, FrameRate::FPS_24)),
            _ => Err(CmdError::not_found(EntityKind::Media, id)),
        }
    }
}

fn timing(secs: i64) -> SequenceTiming {
    SequenceTiming::new(FrameRate::FPS_30, Time::from_secs(secs).unwrap())
}

fn t(flicks: i64) -> Time {
    Time::from_flicks(flicks).unwrap()
}

/// xorshift64: deterministic randomness without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

// ----- a recording sink --------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Record {
    seq: u64,
    frame: i64,
    /// The frame number painted into the pixels.
    decoded: Option<i64>,
    /// The target offset painted into the pixels ([`target_offset`]).
    painted: Option<i64>,
    pts: Time,
    epoch: u64,
    /// The transport's epoch read inside the delivery (under the commit gate).
    epoch_at_delivery: u64,
    /// The transport's target read inside the delivery.
    target_at_delivery: PlaybackTarget,
    quality: Quality,
    lowres: bool,
    width: u32,
    host_ns: u64,
    clock: Time,
}

struct RecordingSink {
    preview: PreviewConfig,
    records: Arc<Mutex<Vec<Record>>>,
    transport: Arc<OnceLock<Transport>>,
    host: Arc<dyn HostClock>,
    clock: AudioClock,
    dropped: Arc<AtomicU64>,
    heartbeats: Arc<AtomicU64>,
    seq: u64,
}

impl FrameSink for RecordingSink {
    fn preview(&self) -> PreviewConfig {
        self.preview
    }

    fn layout(&self, scale: Scale) -> Result<FrameLayout, SinkError> {
        let c = scaled_preview(self.preview, scale);
        Ok(FrameLayout::packed(c.width, c.height, c.format)?)
    }

    fn deliver(
        &mut self,
        info: &FrameInfo,
        layout: &FrameLayout,
        pixels: &[u8],
    ) -> Result<u64, SinkError> {
        self.seq += 1;
        let epoch_at_delivery = self.transport.get().map_or(u64::MAX, Transport::epoch);
        let target_at_delivery = self
            .transport
            .get()
            .map_or(PlaybackTarget::Sequence, Transport::target);
        let raw = decode_frame_number(layout, pixels);
        self.records.lock().unwrap().push(Record {
            seq: self.seq,
            frame: info.frame,
            decoded: raw.map(|v| v & 0xFFFF_FFFF),
            painted: raw.map(|v| v & !0xFFFF_FFFF),
            pts: info.pts,
            epoch: info.epoch,
            epoch_at_delivery,
            target_at_delivery,
            quality: info.quality,
            lowres: info.lowres,
            width: layout.width(),
            host_ns: self.host.now_ns(),
            clock: self.clock.position(),
        });
        Ok(self.seq)
    }

    fn add_dropped(&mut self, frames: u64) {
        self.dropped.fetch_add(frames, Ordering::SeqCst);
    }

    fn heartbeat(&mut self) {
        self.heartbeats.fetch_add(1, Ordering::SeqCst);
    }

    fn configure(&mut self, preview: PreviewConfig) -> Result<(), SinkError> {
        self.preview = preview.fitted();
        Ok(())
    }
}

// ----- a source with simulated cost, a cache and mid-render interference ----------------------

type During = Box<dyn FnMut(&Transport, &RenderRequest) + Send>;

/// The synthetic source, plus: each render takes `cost_ns` of virtual time (the audio device
/// keeps playing meanwhile), `cached` frames are in the frame cache, and `during` runs halfway
/// through a render (to issue a newer request while it is in flight).
struct TestSource {
    inner: SyntheticSource,
    host: VirtualClock,
    manual: Arc<ManualClock>,
    /// Render cost per scale: full, half, quarter.
    cost_ns: [u64; 3],
    cached: HashSet<i64>,
    /// Poll the cancel token after the work (else rely on the commit gate).
    cooperative: bool,
    during: Option<During>,
    transport: Arc<OnceLock<Transport>>,
    hints: Arc<Mutex<Vec<PrefetchHint>>>,
}

impl TestSource {
    fn spend(&self, ns: u64) {
        self.host.advance(ns);
        if self.manual.clock().is_running() {
            let flicks = state::ns_to_flicks(ns);
            self.manual.advance(Duration::from_flicks(flicks).unwrap());
        }
    }
}

impl FrameSource for TestSource {
    fn timing(&self) -> SequenceTiming {
        self.inner.timing()
    }

    fn render(
        &mut self,
        request: &RenderRequest,
        cancel: &CancelToken,
        out: &mut [u8],
    ) -> Result<Rendered, SourceError> {
        let cost = self.cost_ns[match request.quality.scale {
            Scale::Full => 0,
            Scale::Half => 1,
            Scale::Quarter => 2,
        }];
        self.spend(cost / 2);
        if let (Some(during), Some(transport)) = (&mut self.during, self.transport.get()) {
            during(transport, request);
        }
        self.spend(cost - cost / 2);
        if self.cooperative && cancel.is_cancelled() {
            return Err(SourceError::Cancelled);
        }
        let mut marked = *request;
        marked.frame += target_offset(request.target);
        self.inner.render(&marked, &CancelToken::never(), out)
    }

    fn cached(&mut self, request: &RenderRequest, out: &mut [u8]) -> Option<Rendered> {
        // Only the sequence's frames are cached.
        if request.target != PlaybackTarget::Sequence || !self.cached.contains(&request.frame) {
            return None;
        }
        SyntheticSource::paint(request.frame, &request.layout, out);
        Some(Rendered::default())
    }

    fn prefetch(&mut self, hint: &PrefetchHint) {
        self.hints.lock().unwrap().push(*hint);
    }
}

// ----- the deterministic harness -------------------------------------------------------------

struct Setup {
    secs: i64,
    cost_ns: [u64; 3],
    cached: Vec<i64>,
    cooperative: bool,
    during: Option<During>,
    render: RenderConfig,
    /// Media the transport can play on its own (`view.set_source`).
    sources: Option<Arc<dyn SourceResolver>>,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            secs: 10,
            cost_ns: [0; 3],
            cached: Vec::new(),
            cooperative: true,
            during: None,
            render: RenderConfig::default(),
            sources: Some(Arc::new(TestResolver)),
        }
    }
}

struct Harness {
    transport: Transport,
    render: RenderLoop,
    status: StatusPump,
    host: VirtualClock,
    manual: Arc<ManualClock>,
    audio: ManualAudio,
    records: Arc<Mutex<Vec<Record>>>,
    sink_dropped: Arc<AtomicU64>,
    heartbeats: Arc<AtomicU64>,
    hints: Arc<Mutex<Vec<PrefetchHint>>>,
    /// `playback.state` events with their host time.
    events: Vec<(u64, PlaybackState)>,
}

impl Harness {
    fn new(setup: Setup) -> Harness {
        let host = VirtualClock::new(1_000 * MS);
        let manual = Arc::new(ManualClock::new(SampleRate::HZ_48_000));
        let audio = ManualAudio::new(Arc::clone(&manual));
        let cell = Arc::new(OnceLock::new());
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink_dropped = Arc::new(AtomicU64::new(0));
        let heartbeats = Arc::new(AtomicU64::new(0));
        let hints = Arc::new(Mutex::new(Vec::new()));
        let source = TestSource {
            inner: SyntheticSource::new(timing(setup.secs)),
            host: host.clone(),
            manual: Arc::clone(&manual),
            cost_ns: setup.cost_ns,
            cached: setup.cached.into_iter().collect(),
            cooperative: setup.cooperative,
            during: setup.during,
            transport: Arc::clone(&cell),
            hints: Arc::clone(&hints),
        };
        let sink = RecordingSink {
            preview: PreviewConfig {
                width: 64,
                height: 36,
                format: PixelFormat::Rgba8,
            },
            records: Arc::clone(&records),
            transport: Arc::clone(&cell),
            host: Arc::new(host.clone()),
            clock: manual.clock(),
            dropped: Arc::clone(&sink_dropped),
            heartbeats: Arc::clone(&heartbeats),
            seq: 0,
        };
        let parts = PlaybackParts::new(
            Box::new(source),
            Box::new(audio.clone()),
            Box::new(sink),
            PlaybackConfig {
                render: setup.render,
                host: Arc::new(host.clone()),
                sources: setup.sources,
            },
        )
        .unwrap();
        let _ = cell.set(parts.transport.clone());
        Harness {
            transport: parts.transport,
            render: parts.render,
            status: parts.status,
            host,
            manual,
            audio,
            records,
            sink_dropped,
            heartbeats,
            hints,
            events: Vec::new(),
        }
    }

    /// Ticks until the render loop has nothing to do at this instant.
    fn settle(&mut self) {
        for _ in 0..100_000 {
            if let Tick::Idle { wake_at_ns } = self.render.tick() {
                assert!(
                    wake_at_ns > self.host.now_ns(),
                    "idle deadlines are in the future"
                );
                return;
            }
        }
        panic!("the render loop never idles");
    }

    fn poll(&mut self) {
        let poll = self.status.poll();
        if let Some(e) = poll.event {
            self.events.push((self.host.now_ns(), e));
        }
    }

    /// One millisecond: react at this instant, then let 1 ms pass on the host clock and, if it
    /// runs, the audio clock.
    fn ms(&mut self) {
        self.settle();
        self.poll();
        self.host.advance(MS);
        if self.manual.clock().is_running() {
            self.manual
                .advance(Duration::from_flicks(FLICKS_PER_MS).unwrap());
        }
    }

    fn run_ms(&mut self, n: u64) {
        for _ in 0..n {
            self.ms();
        }
        self.settle();
        self.poll();
    }

    /// Runs until playback stops by itself; returns the milliseconds it took.
    fn run_until_stopped(&mut self, max_ms: u64) -> u64 {
        for i in 0..max_ms {
            self.ms();
            if !self.transport.state().playing {
                self.settle();
                return i;
            }
        }
        panic!("playback did not stop within {max_ms} ms");
    }

    fn records(&self) -> Vec<Record> {
        self.records.lock().unwrap().clone()
    }

    fn records_after(&self, seq: u64) -> Vec<Record> {
        self.records().into_iter().filter(|r| r.seq > seq).collect()
    }

    fn last_seq(&self) -> u64 {
        self.records().last().map_or(0, |r| r.seq)
    }
}

fn assert_never_stale(records: &[Record]) {
    let mut seq = 0;
    for r in records {
        assert!(r.seq > seq, "sequence numbers increase");
        seq = r.seq;
        assert_eq!(
            r.epoch, r.epoch_at_delivery,
            "frame {} of request {} delivered after request {}",
            r.frame, r.epoch, r.epoch_at_delivery
        );
        assert_eq!(r.decoded, Some(r.frame), "the pixels show the frame");
        assert_eq!(
            r.painted,
            Some(target_offset(r.target_at_delivery)),
            "frame {} of request {} shows another target than {:?}",
            r.frame,
            r.epoch,
            r.target_at_delivery
        );
    }
}

// ----- acceptance: A/V sync ------------------------------------------------------------------

#[test]
fn ten_seconds_at_30_fps_deliver_exactly_300_frames_in_sync() {
    let mut h = Harness::new(Setup::default());
    let started = h.transport.play();
    assert_eq!(started.state, TransportMode::Playing);
    assert!(started.playing);
    assert!((started.rate - 1.0).abs() < f64::EPSILON);
    let took = h.run_until_stopped(11_000);
    assert_eq!(took, 10_000, "stops when the audio clock reaches the end");

    let records = h.records();
    assert_eq!(records.len(), 300, "exactly 300 frames");
    assert_never_stale(&records);
    let mut worst = 0;
    for (i, r) in records.iter().enumerate() {
        assert_eq!(r.frame, i as i64, "in order, none skipped");
        assert_eq!(r.pts.flicks(), i as i64 * FRAME_30);
        assert!(r.quality.is_full() && !r.lowres);
        let offset = (r.clock.flicks() - r.pts.flicks()).abs();
        worst = worst.max(offset);
        assert!(offset <= FRAME_30, "frame {i}: A/V offset {offset} flicks");
    }
    // Delivered within the millisecond the audio clock crossed the frame start.
    assert!(worst <= FLICKS_PER_MS, "worst A/V offset {worst} flicks");
    let stats = h.transport.stats();
    println!(
        "1x, 10 s at 30 fps: {} frames in order, worst A/V offset {worst} flicks ({:.3} ms), \
         dropped {}",
        records.len(),
        worst as f64 / FLICKS_PER_MS as f64,
        stats.dropped
    );
    assert_eq!((stats.delivered, stats.dropped, stats.errors), (300, 0, 0));
    assert_eq!(h.sink_dropped.load(Ordering::SeqCst), 0);
    // One seek and one play to start; one pause at the end; nothing in between.
    let calls = h.audio.calls();
    assert_eq!((calls.seek, calls.play, calls.pause), (1, 1, 1));
    let end = h.transport.state();
    assert_eq!(end.state, TransportMode::Stopped);
    assert_eq!(end.t, Time::from_secs(10).unwrap());
    // Prefetch hints follow playback: the next frames, at the playback quality.
    let hints = h.hints.lock().unwrap();
    assert_eq!(hints.len(), 300);
    assert_eq!(hints[10].next_frame, 11);
    assert_eq!((hints[10].step, hints[10].count), (1, 8));
}

#[test]
fn a_render_three_times_slower_than_real_time_drops_frames_but_never_stalls_audio() {
    // 100 ms per frame at 30 fps: three frame periods.
    let render_ns = 100 * MS;
    let mut h = Harness::new(Setup {
        cost_ns: [render_ns; 3],
        ..Setup::default()
    });
    h.transport.play();
    let play_epoch = h.transport.epoch();
    h.run_until_stopped(20_000);
    let all = h.records();
    assert_never_stale(&all);
    let (records, after): (Vec<Record>, Vec<Record>) =
        all.into_iter().partition(|r| r.epoch == play_epoch);
    // Stopped at the end on a reduced-quality frame: one full-quality refine follows.
    assert_eq!(after.len(), 1);
    assert!(after[0].quality.is_full() && after[0].frame == 299);
    let stats = h.transport.stats();
    assert!(stats.dropped >= 150, "frames were dropped: {stats:?}");
    assert!(
        (90..=110).contains(&records.len()),
        "about one frame per render: {}",
        records.len()
    );
    assert_eq!(stats.dropped, h.sink_dropped.load(Ordering::SeqCst));
    for w in records.windows(2) {
        assert!(w[1].frame > w[0].frame, "frames stay in order");
        // The clock advanced by exactly one render each time: it never waited for video.
        assert_eq!(
            w[1].clock.flicks() - w[0].clock.flicks(),
            state::ns_to_flicks(render_ns),
            "the audio clock advances steadily"
        );
    }
    // Audio was never paused or re-seeked to wait for video.
    let calls = h.audio.calls();
    assert_eq!((calls.seek, calls.play, calls.pause), (1, 1, 1));
    // Once the render-time prediction has settled, frames arrive when they are due (until the
    // playhead runs past the end of the content).
    let end = Time::from_secs(10).unwrap();
    let mut worst = 0;
    for r in records[12..].iter().filter(|r| r.clock < end) {
        let offset = (r.clock.flicks() - r.pts.flicks()).abs();
        worst = worst.max(offset);
        assert!(offset <= FRAME_30, "frame {}: offset {offset}", r.frame);
    }
    println!(
        "render 3x slower than real time: {} frames delivered, {} dropped, clock +100 ms per \
         render throughout, worst A/V offset after warm-up {:.1} ms",
        records.len(),
        stats.dropped,
        worst as f64 / FLICKS_PER_MS as f64
    );
    // Late frames dropped the quality ladder to its bottom rung.
    assert_eq!(records.last().unwrap().quality.scale, Scale::Quarter);
    assert_eq!(u64::from(h.transport.state().dropped), stats.dropped);
}

// ----- acceptance: latest-wins seeks -----------------------------------------------------------

#[test]
fn a_hundred_random_seeks_never_deliver_a_stale_frame() {
    let targets: Arc<Mutex<HashMap<u64, i64>>> = Arc::default();
    let injected = Arc::new(AtomicU64::new(0));
    let rng = Arc::new(Mutex::new(Rng(0x9E37_79B9_7F4A_7C15)));
    let during: During = {
        let (targets, injected, rng) = (
            Arc::clone(&targets),
            Arc::clone(&injected),
            Arc::clone(&rng),
        );
        Box::new(move |transport: &Transport, _req: &RenderRequest| {
            let mut rng = rng.lock().unwrap();
            if rng.chance(30) {
                // A newer seek arrives while this frame renders.
                let at =
                    t(rng.below(60 * 30) as i64 * FRAME_30 + rng.below(FRAME_30 as u64) as i64);
                transport.seek(at);
                let frame = transport.timing().frame_at(at);
                targets.lock().unwrap().insert(transport.epoch(), frame);
                injected.fetch_add(1, Ordering::SeqCst);
            }
        })
    };
    let cached: Vec<i64> = (0..60 * 30).filter(|f| f % 7 == 0).collect();
    let mut h = Harness::new(Setup {
        secs: 60,
        cost_ns: [80 * MS, 50 * MS, 20 * MS],
        cached: cached.clone(),
        cooperative: false,
        during: Some(during),
        ..Setup::default()
    });
    h.settle(); // the initial frame at 0
    let mut rng = Rng(42);
    let mut seeks = Vec::new();
    for i in 0..100 {
        // The source never polls its token here: the commit gate alone keeps stale frames out.
        let at = t(rng.below(60 * 30) as i64 * FRAME_30 + rng.below(FRAME_30 as u64) as i64);
        h.transport.seek(at);
        let epoch = h.transport.epoch();
        targets
            .lock()
            .unwrap()
            .insert(epoch, h.transport.timing().frame_at(at));
        seeks.push(epoch);
        if i % 2 == 0 {
            h.run_ms(1_000);
        } else {
            // A burst: the next seek follows within 30 ms.
            h.run_ms(30);
        }
    }
    h.run_ms(1_000);

    let records = h.records();
    assert_never_stale(&records);
    let targets = targets.lock().unwrap();
    let mut by_epoch: HashMap<u64, Vec<&Record>> = HashMap::new();
    for r in records.iter().filter(|r| r.epoch > 0) {
        assert_eq!(
            Some(&r.frame),
            targets.get(&r.epoch),
            "the seek's own frame"
        );
        by_epoch.entry(r.epoch).or_default().push(r);
    }
    // Every request ends in at most one low-res frame then one full frame; requests that a
    // newer one superseded may end early but never deliver out of order.
    let last_epoch = h.transport.epoch();
    for (epoch, frames) in &by_epoch {
        let full: Vec<_> = frames.iter().filter(|r| r.quality.is_full()).collect();
        let low: Vec<_> = frames.iter().filter(|r| !r.quality.is_full()).collect();
        assert!(
            full.len() <= 1 && low.len() <= 1,
            "epoch {epoch}: {frames:?}"
        );
        for r in &low {
            assert!(r.lowres && r.quality.scale == Scale::Quarter);
            assert!(
                !cached.contains(&r.frame),
                "a cache hit skips the low-res frame"
            );
        }
        if let (Some(l), Some(f)) = (low.first(), full.first()) {
            assert!(l.seq < f.seq);
            // Quarter at seek + 20 ms; the refine starts at seek + 100 ms and takes 80 ms.
            assert!(
                f.host_ns >= l.host_ns + 100 * MS,
                "refined after the idle delay"
            );
        }
    }
    // The last request settled at full quality on its frame.
    let last = by_epoch.get(&last_epoch).expect("the last seek delivered");
    assert!(last.last().unwrap().quality.is_full());
    assert_eq!(h.transport.state().state, TransportMode::Stopped);
    // Every seek left alone for a second (no burst, no newer request mid-render) refined.
    let full_epochs: HashSet<u64> = records
        .iter()
        .filter(|r| r.quality.is_full())
        .map(|r| r.epoch)
        .collect();
    let undisturbed: Vec<u64> = seeks
        .iter()
        .enumerate()
        .filter(|(i, e)| {
            i % 2 == 0 && **e + 1 == seeks.get(i + 1).copied().unwrap_or(last_epoch + 1)
        })
        .map(|(_, e)| *e)
        .collect();
    assert!(
        undisturbed.len() >= 20,
        "{} undisturbed seeks",
        undisturbed.len()
    );
    for e in &undisturbed {
        assert!(full_epochs.contains(e), "undisturbed seek {e} refined");
    }
    let stats = h.transport.stats();
    println!(
        "100 random seeks (+{} injected mid-render): {} frames ({} low-res), {} renders \
         cancelled, 0 stale; {} undisturbed seeks all refined",
        injected.load(Ordering::SeqCst),
        records.len(),
        records.iter().filter(|r| r.lowres).count(),
        stats.cancelled,
        undisturbed.len()
    );
    assert!(injected.load(Ordering::SeqCst) > 10);
    assert!(
        stats.cancelled > 10,
        "in-flight renders were cancelled: {stats:?}"
    );
    assert_eq!(stats.errors, 0);
}

#[test]
fn seeks_from_another_thread_never_deliver_a_stale_frame() {
    // Real threads: the render thread renders for 0-3 ms per frame while a control thread
    // seeks, plays and pauses as fast as it can. The sink checks the request epoch inside every
    // delivery.
    struct SleepySource(SyntheticSource, u64);
    impl FrameSource for SleepySource {
        fn timing(&self) -> SequenceTiming {
            self.0.timing()
        }
        fn render(
            &mut self,
            request: &RenderRequest,
            cancel: &CancelToken,
            out: &mut [u8],
        ) -> Result<Rendered, SourceError> {
            self.1 = self
                .1
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            std::thread::sleep(StdDuration::from_micros((self.1 >> 33) % 3_000));
            // Ignore the token half of the time: the commit gate must catch those.
            let token = if self.1 & 1 == 0 {
                cancel.clone()
            } else {
                CancelToken::never()
            };
            self.0.render(request, &token, out)
        }
    }
    let manual = Arc::new(ManualClock::new(SampleRate::HZ_48_000));
    let cell = Arc::new(OnceLock::new());
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = RecordingSink {
        preview: PreviewConfig {
            width: 32,
            height: 18,
            format: PixelFormat::Nv12,
        },
        records: Arc::clone(&records),
        transport: Arc::clone(&cell),
        host: Arc::new(SystemClock),
        clock: manual.clock(),
        dropped: Arc::default(),
        heartbeats: Arc::default(),
        seq: 0,
    };
    let events = Arc::new(AtomicU64::new(0));
    let engine = {
        let events = Arc::clone(&events);
        PlaybackEngine::attach(
            Box::new(SleepySource(SyntheticSource::new(timing(60)), 7)),
            Box::new(ManualAudio::new(Arc::clone(&manual))),
            Box::new(sink),
            Box::new(move |_: &PlaybackState| {
                events.fetch_add(1, Ordering::SeqCst);
            }),
            PlaybackConfig {
                render: RenderConfig {
                    refine_after: StdDuration::from_millis(5),
                    ..RenderConfig::default()
                },
                ..PlaybackConfig::default()
            },
        )
        .unwrap()
    };
    let transport = engine.transport().clone();
    let _ = cell.set(transport.clone());
    let mut rng = Rng(7);
    let mut last_target = 0;
    for i in 0..400 {
        match i % 50 {
            10 => {
                transport.play();
            }
            12 => {
                transport.set_rate(Rate::from_milli(-2000).unwrap());
            }
            14 => {
                transport.pause();
            }
            _ => {
                let frame = rng.below(1800) as i64;
                transport.seek(t(frame * FRAME_30));
                last_target = frame;
            }
        }
        std::thread::sleep(StdDuration::from_micros(rng.below(2_000)));
    }
    let last = transport.seek(t(last_target * FRAME_30 + 1));
    assert_eq!(last.t.flicks(), last_target * FRAME_30 + 1);
    // Wait for the last seek to be refined.
    let t0 = Instant::now();
    while transport.state().state != TransportMode::Stopped
        || records
            .lock()
            .unwrap()
            .last()
            .is_none_or(|r| !r.quality.is_full())
    {
        assert!(
            t0.elapsed() < StdDuration::from_secs(10),
            "the last seek never refined"
        );
        std::thread::sleep(StdDuration::from_millis(1));
    }
    let render = engine
        .shutdown()
        .expect("the render thread returns its loop");
    drop(render);
    let records = records.lock().unwrap().clone();
    println!(
        "threaded: 400 control calls, {} frames delivered, {:?}, 0 stale",
        records.len(),
        transport.stats()
    );
    assert!(records.len() > 50, "{} frames", records.len());
    assert_never_stale(&records);
    assert_eq!(records.last().unwrap().frame, last_target);
    assert!(
        events.load(Ordering::SeqCst) > 0,
        "playback.state events were published"
    );
}

// ----- scrub scheduler details -------------------------------------------------------------------

#[test]
fn a_scrub_shows_a_quarter_frame_at_once_and_refines_after_100_ms_idle() {
    let mut h = Harness::new(Setup {
        cost_ns: [10 * MS, 5 * MS, 2 * MS],
        cached: vec![90],
        ..Setup::default()
    });
    h.settle();
    let start = h.last_seq();
    let s = h.transport.seek(t(30 * FRAME_30 + 5));
    assert_eq!(s.state, TransportMode::Scrubbing);
    h.settle();
    let first = h.records_after(start);
    assert_eq!(first.len(), 1, "one low-res frame immediately");
    assert!(first[0].lowres && first[0].quality.scale == Scale::Quarter);
    assert_eq!((first[0].frame, first[0].width), (30, 16));
    // A newer seek 50 ms later restarts the idle delay.
    h.run_ms(50);
    let seek_at = h.host.now_ns();
    h.transport.seek(t(40 * FRAME_30));
    h.run_ms(99 - 2);
    let mid = h.records_after(start);
    assert_eq!(mid.len(), 2, "{mid:?}");
    assert_eq!((mid[1].frame, mid[1].lowres), (40, true));
    assert_eq!(h.transport.state().state, TransportMode::Scrubbing);
    h.run_ms(20);
    let done = h.records_after(start);
    assert_eq!(done.len(), 3);
    let full = &done[2];
    assert_eq!((full.frame, full.width, full.lowres), (40, 64, false));
    assert!(full.quality.is_full());
    let refine_started = full.host_ns - 10 * MS;
    assert!(refine_started >= seek_at + 100 * MS && refine_started <= seek_at + 101 * MS);
    h.poll();
    assert_eq!(h.transport.state().state, TransportMode::Stopped);
    // The frame cache answers at full quality at once, without a low-res frame.
    let before = h.last_seq();
    h.transport.seek(t(90 * FRAME_30));
    h.settle();
    let hit = h.records_after(before);
    assert_eq!(hit.len(), 1);
    assert!(hit[0].quality.is_full() && hit[0].frame == 90);
    assert_eq!(h.transport.state().state, TransportMode::Stopped);
    h.run_ms(200);
    assert_eq!(h.records_after(before).len(), 1, "nothing more to do");
    // A seek within the frame on screen moves the playhead without a new request or frame.
    let epoch = h.transport.epoch();
    let s = h.transport.seek(t(90 * FRAME_30 + 1000));
    assert_eq!(
        (h.transport.epoch(), s.t.flicks()),
        (epoch, 90 * FRAME_30 + 1000)
    );
    h.run_ms(200);
    assert_eq!(h.records_after(before).len(), 1);
    assert_eq!(h.transport.stats().errors, 0);
}

#[test]
fn seeking_while_playing_continues_from_the_new_position() {
    let mut h = Harness::new(Setup::default());
    h.transport.play();
    h.run_ms(500);
    let before = h.last_seq();
    let s = h.transport.seek(t(150 * FRAME_30));
    assert!(s.playing);
    assert_eq!(h.audio.calls().seek, 2, "the audio follows the seek");
    h.run_ms(100);
    let after = h.records_after(before);
    assert_eq!(after.first().unwrap().frame, 150);
    let frames: Vec<i64> = after.iter().map(|r| r.frame).collect();
    assert_eq!(frames, (150..=153).collect::<Vec<_>>());
    assert!(after.iter().all(|r| !r.lowres && r.quality.is_full()));
}

// ----- JKL, reverse frame-step, steps ---------------------------------------------------------

#[test]
fn jkl_transitions_drive_the_audio_and_the_rate() {
    let mut h = Harness::new(Setup::default());
    h.settle();
    let rate = |h: &Harness| h.transport.rate().map(Rate::milli);
    let s = h.transport.shuttle(Shuttle::L);
    assert_eq!((s.state, rate(&h)), (TransportMode::Playing, Some(1000)));
    assert_eq!(
        h.audio.calls(),
        AudioCalls {
            play: 1,
            pause: 0,
            seek: 1,
            set_target: 0,
        }
    );
    h.run_ms(100);
    let s = h.transport.shuttle(Shuttle::L);
    assert!((s.rate - 2.0).abs() < f64::EPSILON);
    assert_eq!(h.audio.calls().pause, 1, "2x is video-only: audio muted");
    h.run_ms(100);
    h.transport.shuttle(Shuttle::L);
    let epoch = h.transport.epoch();
    assert_eq!(rate(&h), Some(4000));
    h.transport.shuttle(Shuttle::L);
    assert_eq!(
        (rate(&h), h.transport.epoch()),
        (Some(4000), epoch),
        "4x is the top"
    );
    h.run_ms(100);
    let s = h.transport.shuttle(Shuttle::K);
    assert_eq!(
        (s.state, s.playing, rate(&h)),
        (TransportMode::Stopped, false, None)
    );
    assert!(s.rate.abs() < f64::EPSILON);
    h.run_ms(10);
    // Position: 100 ms at 1x, 100 ms at 2x, 100 ms at 4x = 0.7 s.
    assert_eq!(h.transport.position().flicks(), 700 * FLICKS_PER_MS);
    for expected in [-1000, -2000, -4000, -4000] {
        h.transport.shuttle(Shuttle::J);
        assert_eq!(rate(&h), Some(expected));
        h.run_ms(20);
    }
    assert_eq!(h.audio.calls().play, 1, "reverse never plays audio");
    h.transport.shuttle(Shuttle::L);
    assert_eq!(rate(&h), Some(1000));
    assert_eq!(
        h.audio.calls(),
        AudioCalls {
            play: 2,
            pause: 1,
            seek: 2,
            set_target: 0,
        }
    );
    h.run_ms(50);
    h.transport.shuttle(Shuttle::J);
    assert_eq!(rate(&h), Some(-1000));
    assert_eq!(h.audio.calls().pause, 2, "leaving 1x mutes the audio");
    assert_never_stale(&h.records());
}

#[test]
fn reverse_playback_steps_frame_by_frame_with_audio_muted_and_stops_at_zero() {
    for (rate, stride) in [(-1000, 1), (-2000, 2), (-4000, 4)] {
        let mut h = Harness::new(Setup::default());
        h.transport.seek(t(60 * FRAME_30));
        h.run_ms(200);
        let before = h.last_seq();
        let s = h.transport.set_rate(Rate::from_milli(rate).unwrap());
        assert!(s.playing && s.rate < 0.0);
        let took = h.run_until_stopped(5_000);
        assert_eq!(
            took as i64,
            2_000 / i64::from(rate / -1000),
            "reaches 0 at rate {rate}"
        );
        let frames: Vec<i64> = h.records_after(before).iter().map(|r| r.frame).collect();
        let expected: Vec<i64> = (0..60).rev().filter(|f| (60 - f) % stride == 0).collect();
        assert_eq!(frames, expected, "rate {rate}");
        assert_eq!(
            h.transport.stats().dropped,
            0,
            "decimation at 2x/4x is not a drop"
        );
        assert_eq!(h.audio.calls().play, 0, "reverse is muted");
        let s = h.transport.state();
        assert_eq!((s.state, s.t), (TransportMode::Stopped, Time::ZERO));
        // At the start, J has nothing to play.
        let s = h.transport.shuttle(Shuttle::J);
        assert!(!s.playing);
    }
}

#[test]
fn fast_forward_shows_one_frame_per_frame_period() {
    let mut h = Harness::new(Setup::default());
    h.settle();
    let before = h.last_seq();
    h.transport.set_rate(Rate::from_milli(4000).unwrap());
    h.run_ms(1_000);
    let frames: Vec<i64> = h.records_after(before).iter().map(|r| r.frame).collect();
    assert_eq!(frames.first(), Some(&4), "frame 0 is already on screen");
    assert!(frames.windows(2).all(|w| w[1] - w[0] == 4), "{frames:?}");
    assert_eq!(frames.len(), 30);
    assert_eq!(h.audio.calls().play, 0);
    assert_eq!(h.transport.position().flicks(), 4_000 * FLICKS_PER_MS);
}

#[test]
fn steps_move_by_whole_frames_and_pause_playback() {
    let mut h = Harness::new(Setup::default());
    h.transport.seek(t(10 * FRAME_30 + 7));
    h.run_ms(200);
    let check = |h: &mut Harness, frames: i64, expected: i64| {
        let before = h.last_seq();
        let s = h.transport.step(frames);
        assert_eq!(s.state, TransportMode::Stopped);
        assert_eq!(s.t.flicks(), expected * FRAME_30);
        h.settle();
        let out = h.records_after(before);
        assert_eq!(out.len(), 1, "step {frames}");
        assert_eq!(out[0].frame, expected);
        assert!(out[0].quality.is_full() && !out[0].lowres);
    };
    check(&mut h, 1, 11);
    check(&mut h, -5, 6);
    check(&mut h, -100, 0);
    check(&mut h, 1_000, 299);
    // A step that cannot move changes nothing.
    let epoch = h.transport.epoch();
    h.transport.step(1);
    assert_eq!(h.transport.epoch(), epoch);
    // Stepping while playing pauses at the current frame first.
    h.transport.seek(t(30 * FRAME_30));
    h.run_ms(200);
    h.transport.play();
    h.run_ms(100);
    let s = h.transport.step(1);
    assert!(!s.playing);
    assert_eq!(s.t.flicks(), 34 * FRAME_30);
    assert_eq!(h.audio.calls().pause, 1);
}

// ----- sources (ADR-0022, WP-1H.04) ------------------------------------------------------------

#[test]
fn a_ten_second_25_fps_source_plays_250_frames_in_order_in_sync() {
    let mut h = Harness::new(Setup::default());
    h.transport.seek(t(40 * FRAME_30));
    h.run_ms(200);
    let info = source(1, 10, FrameRate::FPS_25);
    let s = h.transport.set_source(Some(info));
    assert_eq!(s.source, Some(info));
    assert_eq!((s.state, s.t), (TransportMode::Stopped, Time::ZERO));
    assert_eq!(h.transport.target(), PlaybackTarget::Media(media(1)));
    assert_eq!(h.transport.timing(), info.timing());
    assert_eq!(
        h.audio.calls().set_target,
        1,
        "the audio follows the target"
    );
    let switched = h.last_seq();
    let s = h.transport.play();
    assert!(s.playing && s.source == Some(info));
    let play_epoch = h.transport.epoch();
    let took = h.run_until_stopped(11_000);
    assert_eq!(
        took, 10_000,
        "stops when the audio clock reaches the source's end"
    );

    let records = h.records_after(switched);
    assert_never_stale(&records);
    let played: Vec<&Record> = records.iter().filter(|r| r.epoch == play_epoch).collect();
    assert_eq!(played.len(), 250, "exactly 250 frames");
    let mut worst = 0;
    for (i, r) in played.iter().enumerate() {
        assert_eq!(r.frame, i as i64, "in order, none skipped");
        assert_eq!(r.pts.flicks(), i as i64 * FRAME_25);
        assert_eq!(r.target_at_delivery, PlaybackTarget::Media(media(1)));
        let offset = (r.clock.flicks() - r.pts.flicks()).abs();
        worst = worst.max(offset);
        assert!(offset <= FRAME_25, "frame {i}: A/V offset {offset} flicks");
    }
    println!(
        "source, 10 s at 25 fps: 250 frames in order, worst A/V offset {:.3} ms",
        worst as f64 / FLICKS_PER_MS as f64
    );
    let end = h.transport.state();
    assert_eq!(
        (end.t, end.source),
        (Time::from_secs(10).unwrap(), Some(info))
    );
    assert_eq!(h.transport.stats().dropped, 0);
    // The sequence's playhead never moved.
    assert_eq!(h.transport.sequence_playhead().flicks(), 40 * FRAME_30);
    assert_eq!(h.transport.sequence_timing(), timing(10));
}

#[test]
fn switching_back_mid_play_shows_the_sequence_frame_at_its_unchanged_playhead() {
    let mut h = Harness::new(Setup::default());
    h.transport.seek(t(40 * FRAME_30 + 9));
    h.run_ms(200);
    let one = source(1, 10, FrameRate::FPS_25);
    h.transport.set_source(Some(one));
    h.transport.play();
    h.run_ms(500);
    let left = h.transport.state();
    assert!(left.playing && left.t > Time::ZERO, "{left:?}");
    let before = h.last_seq();
    let calls = h.audio.calls();
    let s = h.transport.set_source(None);
    assert_eq!(
        (s.state, s.playing, s.source, s.t.flicks()),
        (TransportMode::Stopped, false, None, 40 * FRAME_30 + 9),
        "the sequence's own playhead"
    );
    assert_eq!(h.transport.timing(), timing(10));
    let now = h.audio.calls();
    assert_eq!(
        (now.pause - calls.pause, now.set_target - calls.set_target),
        (1, 1),
        "the audio paused and follows the sequence again"
    );
    h.run_ms(100);
    let back = h.records_after(before);
    assert_eq!(back.len(), 1, "{back:?}");
    assert_eq!(back[0].frame, 40);
    assert_eq!(back[0].target_at_delivery, PlaybackTarget::Sequence);
    assert!(back[0].quality.is_full() && !back[0].lowres);
    assert_never_stale(&h.records());

    // Each source keeps its own playhead; a new one starts at 0.
    let s = h
        .transport
        .set_source(Some(source(2, 4, FrameRate::FPS_30)));
    assert_eq!(s.t, Time::ZERO);
    h.transport.seek(t(30 * FRAME_30));
    let s = h.transport.set_source(Some(one));
    assert_eq!(s.t, left.t, "media 1 is where it was left");
    let s = h.transport.set_source_media(Some(media(2))).unwrap();
    assert_eq!(s.t.flicks(), 30 * FRAME_30, "media 2 too");
    // The same source again changes nothing.
    let epoch = h.transport.epoch();
    h.transport
        .set_source(Some(source(2, 4, FrameRate::FPS_30)));
    assert_eq!(h.transport.epoch(), epoch);
    let s = h.transport.set_source(None);
    assert_eq!(s.t.flicks(), 40 * FRAME_30 + 9);
    let epoch = h.transport.epoch();
    h.transport.set_source(None);
    assert_eq!(
        h.transport.epoch(),
        epoch,
        "the sequence again changes nothing"
    );
}

#[test]
fn a_hundred_random_switches_and_seeks_never_deliver_a_frame_of_an_old_target() {
    let targets = [
        None,
        Some(source(1, 10, FrameRate::FPS_25)),
        Some(source(2, 4, FrameRate::FPS_30)),
        Some(source(3, 2, FrameRate::FPS_24)),
    ];
    let rng = Arc::new(Mutex::new(Rng(0xA5A5_5A5A_1234_5678)));
    let injected = Arc::new(AtomicU64::new(0));
    let during: During = {
        let (rng, injected) = (Arc::clone(&rng), Arc::clone(&injected));
        Box::new(move |transport: &Transport, _req: &RenderRequest| {
            let mut rng = rng.lock().unwrap();
            if rng.chance(25) {
                // A switch or a seek arrives while this frame renders.
                if rng.chance(50) {
                    transport.set_source(targets[rng.below(4) as usize]);
                } else {
                    let end = transport.timing().end.flicks();
                    transport.seek(t(rng.below(end.max(1) as u64) as i64));
                }
                injected.fetch_add(1, Ordering::SeqCst);
            }
        })
    };
    let mut h = Harness::new(Setup {
        cost_ns: [30 * MS, 15 * MS, 5 * MS],
        cooperative: false,
        during: Some(during),
        ..Setup::default()
    });
    h.settle();
    let mut rng = Rng(99);
    let mut switches = 0;
    for i in 0..100 {
        match rng.below(10) {
            0..=4 => {
                // The test thread's own switch: after it returns, only the new target's frames.
                let next = targets[rng.below(4) as usize];
                let before = h.last_seq();
                let s = h.transport.set_source(next);
                let epoch = h.transport.epoch();
                assert_eq!(s.source, next);
                h.run_ms(1);
                let target = next.map_or(PlaybackTarget::Sequence, |n| n.target());
                for r in h.records_after(before) {
                    // Nothing older than the switch; switches injected mid-render may follow.
                    assert!(
                        r.epoch >= epoch,
                        "switch {i}: a frame of request {}",
                        r.epoch
                    );
                    if r.epoch == epoch {
                        assert_eq!(r.target_at_delivery, target, "switch {i}");
                        assert_eq!(r.painted, Some(target_offset(target)), "switch {i}");
                    }
                }
                switches += 1;
            }
            5..=7 => {
                let end = h.transport.timing().end.flicks();
                h.transport.seek(t(rng.below(end.max(1) as u64) as i64));
            }
            8 => {
                h.transport.play();
            }
            _ => {
                h.transport.pause();
            }
        }
        h.run_ms(if i % 3 == 0 { 300 } else { 20 });
    }
    h.run_ms(1_000);
    let records = h.records();
    assert_never_stale(&records);
    let targets_seen: HashSet<i64> = records.iter().filter_map(|r| r.painted).collect();
    let stats = h.transport.stats();
    println!(
        "{switches} switches and {} seeks/plays/pauses (+{} injected mid-render): {} frames from \
         {} targets, {} renders cancelled, 0 stale",
        100 - switches,
        injected.load(Ordering::SeqCst),
        records.len(),
        targets_seen.len(),
        stats.cancelled
    );
    assert!(switches >= 30, "{switches} switches");
    assert_eq!(targets_seen.len(), 4, "every target was shown");
    assert!(injected.load(Ordering::SeqCst) > 10);
    assert_eq!(stats.errors, 0);
}

#[test]
fn edits_during_a_source_wait_and_a_removed_asset_ends_it() {
    let mut h = Harness::new(Setup::default());
    h.transport.seek(t(200 * FRAME_30));
    h.run_ms(200);
    let one = source(1, 10, FrameRate::FPS_25);
    h.transport.set_source(Some(one));
    h.transport.play();
    h.run_ms(300);
    // The timeline shrinks to 5 s while the source plays: the source is not interrupted.
    let epoch = h.transport.epoch();
    let s = h
        .transport
        .sequence_edited(SequenceTiming::new(FrameRate::FPS_30, t(150 * FRAME_30)));
    assert!(s.playing && s.source == Some(one));
    assert_eq!(h.transport.epoch(), epoch, "no new request for the source");
    h.transport
        .set_timing(SequenceTiming::new(FrameRate::FPS_30, t(100 * FRAME_30)));
    assert_eq!(h.transport.epoch(), epoch);
    assert_eq!(h.transport.timing(), one.timing());
    // Another asset changing does nothing either.
    h.transport.source_changed(media(2), None);
    assert_eq!(h.transport.epoch(), epoch);
    // The asset is re-probed shorter: playback continues on the new extent.
    let shorter = source(1, 8, FrameRate::FPS_25);
    let s = h.transport.source_changed(media(1), Some(shorter));
    assert!(s.playing && s.source == Some(shorter));
    // The same asset again (a relink): the current frame re-renders.
    let before = h.transport.epoch();
    h.transport.source_changed(media(1), Some(shorter));
    assert!(h.transport.epoch() > before);
    h.run_ms(100);
    // The asset is removed: the sequence plays again, stopped on its playhead clamped to the
    // new extent.
    let s = h.transport.source_changed(media(1), None);
    assert_eq!(
        (s.source, s.playing, s.t.flicks()),
        (None, false, 100 * FRAME_30)
    );
    assert_eq!(h.transport.timing().end.flicks(), 100 * FRAME_30);
    h.settle();
    let last = h.records().last().cloned().unwrap();
    assert_eq!(
        (last.frame, last.target_at_delivery),
        (99, PlaybackTarget::Sequence)
    );
    // Back on the sequence, edits refresh it as before.
    let before = h.transport.epoch();
    h.transport
        .sequence_edited(SequenceTiming::new(FrameRate::FPS_30, t(100 * FRAME_30)));
    assert!(h.transport.epoch() > before, "a refresh");
    assert_never_stale(&h.records());
}

// ----- pause, refresh, preview size, timing ---------------------------------------------------

#[test]
fn pausing_refines_a_reduced_playback_frame_and_keeps_a_full_one() {
    // Slow full-quality renders push playback down the ladder; pausing refines.
    let mut h = Harness::new(Setup {
        cost_ns: [50 * MS, 10 * MS, 5 * MS],
        render: RenderConfig {
            quality: AutoQualityConfig {
                late_frames: 2,
                ..AutoQualityConfig::default()
            },
            ..RenderConfig::default()
        },
        ..Setup::default()
    });
    h.transport.play();
    h.run_ms(600);
    assert!(!h.render.playback_quality().is_full());
    let before = h.last_seq();
    let s = h.transport.pause();
    h.settle();
    let refine = h.records_after(before);
    assert_eq!(refine.len(), 1);
    assert!(refine[0].quality.is_full());
    assert_eq!(refine[0].frame, h.transport.timing().frame_at(s.t));
    // Full-quality playback paused: the frame on screen is already final.
    let mut h = Harness::new(Setup::default());
    h.transport.play();
    h.run_ms(500);
    let before = h.last_seq();
    h.transport.pause();
    h.run_ms(100);
    assert!(h.records_after(before).is_empty(), "no redundant refine");
    // Play again from the frame on screen: the next frame is the first one rendered.
    h.transport.play();
    h.run_ms(40);
    let resumed = h.records_after(before);
    assert_eq!(resumed[0].frame, h.records()[before as usize - 1].frame + 1);
}

#[test]
fn refresh_resize_and_new_timing_re_render_the_current_frame() {
    let mut h = Harness::new(Setup::default());
    h.transport.step(20);
    h.settle();
    let before = h.last_seq();
    h.transport.refresh();
    h.settle();
    assert_eq!(
        h.records_after(before).len(),
        1,
        "a document change re-renders"
    );
    let before = h.last_seq();
    h.transport.set_preview_size(32, 18);
    h.settle();
    let resized = h.records_after(before);
    assert_eq!(
        (resized.len(), resized[0].width, resized[0].frame),
        (1, 32, 20)
    );
    let s = h
        .transport
        .set_timing(SequenceTiming::new(FrameRate::FPS_30, t(10 * FRAME_30)));
    assert_eq!(
        s.t.flicks(),
        10 * FRAME_30,
        "clamped to the shorter content"
    );
    h.settle();
    assert_eq!(h.records().last().unwrap().frame, 9);
    assert_eq!(h.heartbeats.load(Ordering::SeqCst), 0);
    h.run_ms(300);
    assert!(
        h.heartbeats.load(Ordering::SeqCst) >= 5,
        "≥ 10 Hz heartbeat while idle"
    );
}

#[test]
fn play_at_the_end_starts_over_and_an_empty_sequence_never_plays() {
    let mut h = Harness::new(Setup {
        secs: 1,
        ..Setup::default()
    });
    h.transport.seek(Time::from_secs(5).unwrap());
    let s = h.transport.play();
    assert!(s.playing);
    assert_eq!(s.t, Time::ZERO);
    h.run_until_stopped(2_000);
    let mut empty = Harness::new(Setup {
        secs: 0,
        ..Setup::default()
    });
    assert!(!empty.transport.play().playing);
    empty.run_ms(10);
    assert_eq!(empty.records().len(), 1, "the single (empty) frame at 0");
}

// ----- playback.state -----------------------------------------------------------------------

#[test]
fn playback_state_events_are_at_most_30_per_second() {
    let mut h = Harness::new(Setup::default());
    h.transport.play();
    for i in 0..3_000u64 {
        if i % 7 == 0 && i < 2_000 {
            // Churn: seeks change the state far more often than 30 times a second.
            h.transport.seek(t((i as i64 % 200) * FRAME_30 + 5));
        }
        h.ms();
    }
    let times: Vec<u64> = h.events.iter().map(|(at, _)| *at).collect();
    assert!(times.len() <= 3 * 30 + 1, "{} events in 3 s", times.len());
    assert!(
        times.len() >= 85,
        "{} events: one per period while playing",
        times.len()
    );
    let min_gap = times.windows(2).map(|w| w[1] - w[0]).min().unwrap();
    println!(
        "playback.state: {} events in 3 s of playback with a seek every 7 ms, min gap {:.2} ms",
        times.len(),
        min_gap as f64 / 1e6
    );
    assert!(min_gap >= 1_000_000_000 / 30, "{min_gap} ns between events");
    assert!(h.events.iter().all(|(_, e)| e.playing));
    // Stopped and unchanged: no events.
    h.transport.pause();
    h.run_ms(100);
    let n = h.events.len();
    h.run_ms(2_000);
    assert_eq!(h.events.len(), n, "nothing changes, nothing is sent");
    assert_eq!(h.events.last().unwrap().1.state, TransportMode::Stopped);
}

#[test]
fn the_ring_clock_plays_only_while_the_position_advances() {
    let mut h = Harness::new(Setup::default());
    let stopped = h.transport.clock_sample();
    assert_eq!(stopped.state, ClockState::Stopped);
    h.transport.seek(t(5 * FRAME_30));
    assert_eq!(h.transport.clock_sample().state, ClockState::Scrubbing);
    h.run_ms(200);
    assert_eq!(h.transport.clock_sample().state, ClockState::Stopped);
    h.manual.pause(); // a device that is not running yet
    let audio = h.audio.clone();
    h.transport.play();
    h.manual.set_state(cutweave_audio::ClockState::Priming);
    let priming = h.transport.clock_sample();
    assert_eq!(
        (priming.state, priming.position.flicks()),
        (ClockState::Stopped, 5 * FRAME_30)
    );
    h.manual.play();
    let playing = h.transport.clock_sample();
    assert_eq!(
        (playing.state, playing.rate_milli),
        (ClockState::Playing, 1000)
    );
    assert_eq!(audio.calls().play, 1);
    h.transport.set_rate(Rate::REVERSE);
    let rev = h.transport.clock_sample();
    assert_eq!((rev.state, rev.rate_milli), (ClockState::Playing, -1000));
    assert_eq!(rev.host_ns, h.host.now_ns());
}

// ----- commands -----------------------------------------------------------------------------

#[test]
fn view_transport_commands_run_against_the_transport() {
    let mut registry = Registry::new();
    register(&mut registry).unwrap();
    let mut h = Harness::new(Setup::default());
    let call = |name: &str, params: serde_json::Value| {
        registry.invoke_engine_json(name, params, &mut h.transport.clone(), Access::UI)
    };
    let s = call("view.play", json!({})).unwrap();
    assert_eq!(
        (s["state"].as_str(), s["playing"].as_bool()),
        (Some("playing"), Some(true))
    );
    assert_eq!(s["rate"].as_f64(), Some(1.0));
    let s = call("view.seek", json!({"t": "f:30"})).unwrap();
    assert_eq!(s["t"].as_i64(), Some(30 * FRAME_30));
    let s = call("view.seek", json!({"t": "2s"})).unwrap();
    assert_eq!(s["t"].as_i64(), Some(2 * 30 * FRAME_30));
    let s = call("view.set_rate", json!({"rate": -2})).unwrap();
    assert_eq!(s["rate"].as_f64(), Some(-2.0));
    let err = call("view.set_rate", json!({"rate": 3})).unwrap_err();
    assert!(err.to_string().contains("rate must be one of"), "{err}");
    let s = call("view.set_rate", json!({"rate": 0})).unwrap();
    assert_eq!(s["state"].as_str(), Some("stopped"));
    let s = call("view.step", json!({"frames": 2})).unwrap();
    assert_eq!(s["t"].as_i64(), Some(62 * FRAME_30));
    let s = call("view.pause", json!({})).unwrap();
    assert_eq!(s["playing"].as_bool(), Some(false));
    assert!(call("view.seek", json!({"t": -5})).is_err());
    assert!(call("view.set_preview_size", json!({"width": 0, "height": 10})).is_err());
    let size = call(
        "view.set_preview_size",
        json!({"width": 20_000, "height": 10_000}),
    )
    .unwrap();
    assert_eq!(size, json!({"width": 8192, "height": 4096}));
    // A media item on its own: the commands then act on it, in its time (25 fps).
    let id = serde_json::to_value(media(1)).unwrap();
    let s = call("view.set_source", json!({ "media": id })).unwrap();
    assert_eq!(
        s["source"],
        json!({ "media": id, "duration": 10 * 705_600_000_i64, "fps": {"num": 25, "den": 1} })
    );
    assert_eq!(
        (s["t"].as_i64(), s["playing"].as_bool()),
        (Some(0), Some(false))
    );
    let s = call("view.seek", json!({"t": "f:30"})).unwrap();
    assert_eq!(s["t"].as_i64(), Some(30 * FRAME_25));
    let s = call("view.step", json!({"frames": -1})).unwrap();
    assert_eq!(s["t"].as_i64(), Some(29 * FRAME_25));
    let err = call(
        "view.set_source",
        json!({ "media": serde_json::to_value(media(9)).unwrap() }),
    )
    .unwrap_err();
    assert_eq!(err.code(), "not_found", "{err}");
    assert!(call("view.set_source", json!({ "media": 3 })).is_err());
    // Without `media`: the timeline again, on its own playhead.
    let s = call("view.set_source", json!({})).unwrap();
    assert_eq!(
        (s["source"].clone(), s["t"].as_i64()),
        (json!(null), Some(62 * FRAME_30))
    );
    for name in [
        "view.play",
        "view.pause",
        "view.seek",
        "view.set_rate",
        "view.step",
        "view.set_source",
        "view.set_preview_size",
    ] {
        // `view` commands are never undoable, never journaled and bump no `rev` (arch §5.1).
        let meta = &registry.get(name).unwrap().meta;
        assert!(!meta.undoable && meta.view, "{name}: a view command");
    }
    // Agents reach `view.set_source` through the catalog, not `tools/list`.
    assert_eq!(
        registry.get("view.set_source").unwrap().meta.mcp,
        cutweave_commands::McpExposure::Catalog
    );
    h.settle();
    // Without a resolver, playback has no media to play on its own.
    let mut bare = Harness::new(Setup {
        sources: None,
        ..Setup::default()
    });
    let err = registry
        .invoke_engine_json(
            "view.set_source",
            json!({ "media": id }),
            &mut bare.transport.clone(),
            Access::UI,
        )
        .unwrap_err();
    assert_eq!(err.code(), "unsupported", "{err}");
    bare.settle();
}

// ----- headless with the null audio output and the SHM ring ---------------------------------

#[test]
fn headless_playback_on_the_null_output_publishes_into_the_ring() {
    use cutweave_audio::{AudioOutput, Mixer, MixerConfig, OutputConfig, ToneFactory};

    use crate::shm::{FramePublisher, RingConfig};

    /// Rendering takes three frame periods.
    struct Slow(SyntheticSource);
    impl FrameSource for Slow {
        fn timing(&self) -> SequenceTiming {
            self.0.timing()
        }
        fn render(
            &mut self,
            request: &RenderRequest,
            cancel: &CancelToken,
            out: &mut [u8],
        ) -> Result<Rendered, SourceError> {
            std::thread::sleep(StdDuration::from_millis(100));
            self.0.render(request, cancel, out)
        }
    }

    let rate = AudioOutput::preferred_rate(&OutputConfig::null()).unwrap();
    let (mixer, handle) = Mixer::new(MixerConfig::new(rate), Arc::new(ToneFactory::new())).unwrap();
    let output = AudioOutput::start(mixer, OutputConfig::null())
        .map_err(|e| e.error)
        .unwrap();
    // The engine keeps both halves (segments, master gain, meters) while the transport drives them.
    let (output, handle) = (Arc::new(output), Arc::new(handle));
    let publisher = FramePublisher::create(&RingConfig {
        prefault: false,
        ..RingConfig::new(64, 36, PixelFormat::Rgba8)
    })
    .unwrap();
    let announce = publisher.announcement();
    let writer = publisher.clock_writer();
    let reader =
        crate::shm::reader::TestReader::open(&announce.name, Some(announce.epoch)).unwrap();
    let playing_events = Arc::new(AtomicU64::new(0));
    let engine = {
        let playing_events = Arc::clone(&playing_events);
        PlaybackEngine::attach(
            Box::new(Slow(SyntheticSource::new(timing(60)))),
            Box::new(OutputAudio::new(Arc::clone(&output), Arc::clone(&handle))),
            Box::new(PublisherSink::new(publisher)),
            Box::new(move |s: &PlaybackState| {
                if s.playing {
                    playing_events.fetch_add(1, Ordering::SeqCst);
                }
            }),
            PlaybackConfig::default(),
        )
        .unwrap()
    };
    let pump = engine.spawn_clock_pump(writer).unwrap();
    let transport = engine.transport().clone();
    handle.set_master_gain_linear(0.5);
    transport.play();
    let t0 = Instant::now();
    while !transport.clock().is_running() {
        assert!(
            t0.elapsed() < StdDuration::from_secs(5),
            "the null clock never ran"
        );
        std::thread::sleep(StdDuration::from_millis(1));
    }
    let start = transport.position();
    let wall = Instant::now();
    std::thread::sleep(StdDuration::from_millis(1_200));
    let advanced = transport.position().flicks() - start.flicks();
    let elapsed = wall.elapsed();
    let ring_clock = reader.clock().unwrap();
    transport.pause();
    // The clock kept time while video fell behind (generous bounds: shared CI machines).
    let expected = state::ns_to_flicks(elapsed.as_nanos() as u64);
    assert!(
        advanced > expected / 2 && advanced <= expected + 100 * FLICKS_PER_MS,
        "clock advanced {advanced} flicks in {elapsed:?}"
    );
    assert_eq!(ring_clock.state, ClockState::Playing);
    let stats = transport.stats();
    println!(
        "headless null output: clock advanced {:.0} ms in {elapsed:?} wall, {stats:?}, {} ring \
         clock samples",
        advanced as f64 / FLICKS_PER_MS as f64,
        pump.updates()
    );
    assert!(stats.dropped > 0, "slow renders drop frames: {stats:?}");
    assert!(stats.delivered >= 3, "{stats:?}");
    assert!(reader.write_index() >= stats.delivered.min(3));
    assert!(pump.updates() > 100, "the ring clock is pumped");
    let events = playing_events.load(Ordering::SeqCst);
    let window = t0.elapsed().as_secs_f64();
    assert!(
        events >= 5 && events as f64 <= window * 30.0 + 2.0,
        "{events} playback.state events in {window} s"
    );
    assert!(output.stats().callbacks > 0);
    drop(pump);
    let stopped_loop = engine.shutdown().unwrap();
    let (_, sink) = stopped_loop.into_parts();
    drop(sink);
    // The output stops before the mixer handle goes (WP-1G.02).
    drop(transport);
    drop(output);
    drop(handle);
}
