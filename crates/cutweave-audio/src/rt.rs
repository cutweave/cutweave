// SPDX-License-Identifier: GPL-3.0-or-later
//! The real-time half of the mixer: [`Mixer::render`] and the data it owns.
//!
//! Everything reachable from `render` is allocation-, lock- and syscall-free: rings are `rtrb`
//! SPSC queues, statistics are relaxed atomics, scratch space is allocated in [`Mixer::new`],
//! and nothing is dropped here that could free memory: replaced plans (with the readers they
//! still hold) go back to a worker thread through the garbage ring.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};

use cutweave_model::{SampleRate, TrackId};
use rtrb::{Consumer, Producer};

use crate::dsp::{CHANNELS, Ramp, master, peak};

/// Reader id of a silent lane (no ring).
pub(crate) const SILENT: u64 = 0;

/// Counters and published state shared by the real-time thread, the handle and the workers.
#[derive(Debug)]
pub(crate) struct Shared {
    /// Output sample of the frame after the last rendered one.
    pub position: AtomicI64,
    /// Sequence number of the last applied plan.
    pub applied_seq: AtomicU64,
    /// Whether the mixer is waiting for its rings to fill after a seek.
    pub priming: AtomicBool,
    /// Requested master gain (linear, `f32` bits).
    pub master_gain: AtomicU32,
    /// `render` calls.
    pub callbacks: AtomicU64,
    /// Frames a lane had to play as silence because its ring was late.
    pub underrun_frames: AtomicU64,
    /// Callbacks in which at least one lane was late.
    pub underrun_callbacks: AtomicU64,
    /// Sources that failed to open or read (counted by the lane renderers).
    pub source_errors: Arc<AtomicU64>,
    /// Smallest look-ahead (frames) any active lane had at the start of a callback since the
    /// last reset (`i64::MAX`: none measured).
    pub min_lookahead: AtomicI64,
    /// Plans the real-time thread could not hand back for freeing (garbage ring full).
    pub leaked: AtomicU64,
}

impl Shared {
    pub fn new() -> Shared {
        Shared {
            position: AtomicI64::new(0),
            applied_seq: AtomicU64::new(0),
            priming: AtomicBool::new(false),
            master_gain: AtomicU32::new(1.0f32.to_bits()),
            callbacks: AtomicU64::new(0),
            underrun_frames: AtomicU64::new(0),
            underrun_callbacks: AtomicU64::new(0),
            source_errors: Arc::new(AtomicU64::new(0)),
            min_lookahead: AtomicI64::new(i64::MAX),
            leaked: AtomicU64::new(0),
        }
    }

    pub fn source_errors_counter(&self) -> Arc<AtomicU64> {
        self.source_errors.clone()
    }
}

/// The consumer end of one lane ring: the input of one bus from output sample `from` on.
///
/// The ring holds the contiguous output samples `[data_start, data_end)`; outside that range
/// the lane is silent by construction (not an underrun).
pub(crate) struct LaneReader {
    /// Identity, assigned by the handle ([`SILENT`] for a lane without a ring).
    pub id: u64,
    /// The output sample from which this reader replaces the bus's previous one.
    pub from: i64,
    pub data_start: i64,
    pub data_end: i64,
    /// Output sample of the next frame in the ring.
    pub next: i64,
    pub ring: Option<Consumer<f32>>,
}

impl LaneReader {
    /// A silent lane from `from` on.
    pub const fn silent(from: i64) -> LaneReader {
        LaneReader {
            id: SILENT,
            from,
            data_start: from,
            data_end: from,
            next: from,
            ring: None,
        }
    }

    /// Frames of data the lane owes for `[p0, p1)`, and how many of them are in the ring now.
    fn coverage(&self, p0: i64, p1: i64) -> (i64, i64) {
        let a = p0.max(self.data_start);
        let b = p1.min(self.data_end);
        if b <= a {
            return (0, 0);
        }
        let Some(ring) = &self.ring else {
            return (0, 0);
        };
        let buffered = frames_i64(ring.slots());
        // Frames still queued before `a` are skipped first.
        let ahead = (self.next + buffered).min(b) - a.max(self.next);
        (b - a, ahead.max(0))
    }

    /// Writes the lane's samples for `[pos, pos + dst.len() / 2)` into `dst` and returns how
    /// many frames were missing (late ring: played as silence; the late frames are skipped when
    /// they arrive, so the lane stays on the timeline).
    fn read(&mut self, pos: i64, dst: &mut [f32]) -> u64 {
        dst.fill(0.0);
        let end = pos + frames_i64(dst.len());
        let a = pos.max(self.data_start);
        let b = end.min(self.data_end);
        let Some(ring) = self.ring.as_mut() else {
            return 0;
        };
        if b <= a {
            return 0;
        }
        // Skip frames that arrived after their time (an earlier underrun).
        if self.next < a {
            let skip = usize::try_from(a - self.next)
                .unwrap_or(usize::MAX)
                .min(ring.slots() / CHANNELS);
            if skip > 0
                && let Ok(chunk) = ring.read_chunk(skip * CHANNELS)
            {
                chunk.commit_all();
                self.next += frames_i64(skip * CHANNELS);
            }
            if self.next < a {
                return u64::try_from(b - a).unwrap_or(0);
            }
        }
        let want = usize::try_from(b - a).unwrap_or(0);
        let got = want.min(ring.slots() / CHANNELS);
        let at = usize::try_from(a - pos).unwrap_or(0) * CHANNELS;
        if got > 0 {
            // `got` frames are available, so this cannot fail; on failure they count as missing.
            if ring
                .pop_entire_slice(&mut dst[at..at + got * CHANNELS])
                .is_err()
            {
                return want as u64;
            }
            self.next += frames_i64(got * CHANNELS);
        }
        (want - got) as u64
    }
}

/// `samples / 2` as frames.
fn frames_i64(samples: usize) -> i64 {
    i64::try_from(samples / CHANNELS).unwrap_or(i64::MAX)
}

/// One bus of a plan: the track's lane reader(s) and gain.
pub(crate) struct RtBus {
    pub track: TrackId,
    /// Linear track gain.
    pub gain: f32,
    /// Reader the bus plays now, and the reader that takes over at `next_from`.
    pub current_id: (u64, i64),
    pub next_id: Option<(u64, i64)>,
    pub current: Option<LaneReader>,
    pub next: Option<LaneReader>,
    /// A reader replaced at a hand-off, kept until the plan is freed off the real-time thread.
    pub retired: Option<LaneReader>,
    pub ramp: Ramp,
    /// Peak (L, R) after the track gain since the meter last took it.
    pub peak: [f32; 2],
}

impl RtBus {
    pub fn new(track: TrackId, gain: f32, current: (u64, i64), next: Option<(u64, i64)>) -> RtBus {
        RtBus {
            track,
            gain,
            current_id: current,
            next_id: next,
            current: None,
            next: None,
            retired: None,
            ramp: Ramp::at(gain),
            peak: [0.0; 2],
        }
    }
}

/// A plan as the real-time thread runs it. Built by the handle, moved in through the plan ring,
/// moved out through the garbage ring.
pub(crate) struct RtPlan {
    pub seq: u64,
    /// Jump to this output sample (flushing every lane) when applied.
    pub seek: Option<i64>,
    pub buses: Vec<RtBus>,
    /// New readers referenced by `buses`.
    pub deliver: Vec<Option<LaneReader>>,
}

impl RtPlan {
    pub fn empty() -> RtPlan {
        RtPlan {
            seq: 0,
            seek: None,
            buses: Vec::new(),
            deliver: Vec::new(),
        }
    }
}

/// Real-time parameters fixed at construction.
pub(crate) struct RtConfig {
    pub rate: SampleRate,
    /// Look-ahead every active lane needs before playback starts after a seek.
    pub prime_frames: i64,
    /// Give up priming after this many output frames of waiting.
    pub prime_timeout_frames: u64,
    /// Length of gain ramps.
    pub ramp_frames: u32,
    /// Frames mixed per inner step (scratch size).
    pub max_frames: usize,
}

/// The peak level of one track bus over a meter window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusPeak {
    /// The track.
    pub track: TrackId,
    /// Peak absolute sample value (linear, L and R) after the track gain, before the master.
    pub peak: [f32; 2],
}

impl BusPeak {
    /// A silent entry (placeholder in fixed-size arrays).
    pub const SILENT: BusPeak = BusPeak {
        track: TrackId::from_u128(0),
        peak: [0.0; 2],
    };
}

/// What one [`Mixer::render`] call produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderInfo {
    /// Output sample (timeline position at the output rate) of the buffer's first frame;
    /// `None` when the buffer is priming silence after a seek (the position did not advance).
    pub position: Option<i64>,
    /// Frames written.
    pub frames: usize,
}

/// The real-time half of the mixer. Owned by the audio callback (or a test sink); see
/// [`MixerHandle`](crate::MixerHandle) for the control half.
pub struct Mixer {
    cfg: RtConfig,
    plans: Consumer<Box<RtPlan>>,
    garbage: Producer<Box<RtPlan>>,
    live: Box<RtPlan>,
    pos: i64,
    /// Frames waited while priming, `None` when not priming.
    priming: Option<u64>,
    /// Seeks applied so far.
    seeks: u64,
    master: Ramp,
    master_bits: u32,
    lane: Vec<f32>,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Mixer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mixer")
            .field("rate", &self.cfg.rate)
            .field("position", &self.pos)
            .field("buses", &self.live.buses.len())
            .field("priming", &self.priming.is_some())
            .finish_non_exhaustive()
    }
}

impl Mixer {
    pub(crate) fn from_parts(
        cfg: RtConfig,
        plans: Consumer<Box<RtPlan>>,
        garbage: Producer<Box<RtPlan>>,
        shared: Arc<Shared>,
    ) -> Mixer {
        let master_bits = shared.master_gain.load(Ordering::Relaxed);
        Mixer {
            lane: vec![0.0; cfg.max_frames * CHANNELS],
            cfg,
            plans,
            garbage,
            live: Box::new(RtPlan::empty()),
            pos: 0,
            priming: None,
            seeks: 0,
            master: Ramp::at(f32::from_bits(master_bits)),
            master_bits,
            shared,
        }
    }

    /// The output rate (frames per second of [`render`](Self::render) output).
    #[must_use]
    pub fn rate(&self) -> SampleRate {
        self.cfg.rate
    }

    /// Output sample of the next frame `render` will produce.
    #[must_use]
    pub fn position(&self) -> i64 {
        self.pos
    }

    /// Writes the peak of every track bus since the last call into `out` (as many as fit) and
    /// restarts the measurement; returns how many were written. Allocation- and lock-free.
    pub fn take_bus_peaks(&mut self, out: &mut [BusPeak]) -> usize {
        let mut n = 0;
        for (slot, bus) in out.iter_mut().zip(self.live.buses.iter_mut()) {
            *slot = BusPeak {
                track: bus.track,
                peak: bus.peak,
            };
            bus.peak = [0.0; 2];
            n += 1;
        }
        n
    }

    /// How many seeks this mixer has applied. The output callback compares it across calls to
    /// tell a seek (a discontinuity of the clock) from ordinary priming.
    #[must_use]
    pub fn seeks(&self) -> u64 {
        self.seeks
    }

    /// Whether every lane that plays during the next `frames` frames already has its data in
    /// its ring (applies pending plans first). Allocation- and lock-free.
    pub fn ready(&mut self, frames: usize) -> bool {
        self.apply_plans();
        self.lookahead_ok(i64::try_from(frames).unwrap_or(i64::MAX))
    }

    fn lookahead_ok(&self, frames: i64) -> bool {
        let (p0, p1) = (self.pos, self.pos.saturating_add(frames));
        self.live.buses.iter().all(|bus| {
            let switch = bus.next.as_ref().map_or(i64::MAX, |n| n.from);
            let cur = bus
                .current
                .as_ref()
                .map_or((0, 0), |r| r.coverage(p0, p1.min(switch)));
            let next = bus
                .next
                .as_ref()
                .map_or((0, 0), |r| r.coverage(p0.max(switch), p1));
            cur.1 >= cur.0 && next.1 >= next.0
        })
    }

    /// Renders `out.len() / 2` interleaved stereo frames (an odd trailing sample is zeroed).
    ///
    /// Real-time safe: no allocation, no lock, no syscall. Applies plans queued by the handle
    /// (hand-offs exactly at their sample), mixes every bus (lane × track gain), applies the
    /// master gain and limiter, and advances the position. After a seek it plays silence without
    /// advancing until the lanes hold the prime look-ahead (or the prime timeout passes).
    pub fn render(&mut self, out: &mut [f32]) -> RenderInfo {
        let frames = out.len() / CHANNELS;
        out[frames * CHANNELS..].fill(0.0);
        let out = &mut out[..frames * CHANNELS];
        self.shared.callbacks.fetch_add(1, Ordering::Relaxed);
        self.apply_plans();

        let bits = self.shared.master_gain.load(Ordering::Relaxed);
        if bits != self.master_bits {
            self.master_bits = bits;
            self.master
                .set_target(f32::from_bits(bits), self.cfg.ramp_frames);
        }

        if let Some(waited) = self.priming {
            if self.lookahead_ok(self.cfg.prime_frames) || waited >= self.cfg.prime_timeout_frames {
                self.priming = None;
                self.shared.priming.store(false, Ordering::Relaxed);
            } else {
                out.fill(0.0);
                self.priming = Some(waited + frames as u64);
                return RenderInfo {
                    position: None,
                    frames,
                };
            }
        }

        self.note_lookahead();
        let start = self.pos;
        let mut missing = 0;
        let mut done = 0;
        while done < frames {
            let mut n = (frames - done).min(self.cfg.max_frames);
            for bus in &mut self.live.buses {
                if let Some(next_from) = bus.next.as_ref().map(|r| r.from) {
                    if next_from <= self.pos {
                        bus.retired = bus.current.take();
                        bus.current = bus.next.take();
                    } else {
                        let until = usize::try_from(next_from - self.pos).unwrap_or(usize::MAX);
                        n = n.min(until);
                    }
                }
            }
            missing += self.mix(&mut out[done * CHANNELS..(done + n) * CHANNELS]);
            self.pos += i64::try_from(n).unwrap_or(0);
            done += n;
        }
        if missing > 0 {
            self.shared
                .underrun_frames
                .fetch_add(missing, Ordering::Relaxed);
            self.shared
                .underrun_callbacks
                .fetch_add(1, Ordering::Relaxed);
        }
        self.shared.position.store(self.pos, Ordering::Release);
        RenderInfo {
            position: Some(start),
            frames,
        }
    }

    /// Mixes `out.len() / 2` frames at the current position; returns missing lane frames.
    fn mix(&mut self, out: &mut [f32]) -> u64 {
        out.fill(0.0);
        let lane = &mut self.lane[..out.len()];
        let mut missing = 0;
        for bus in &mut self.live.buses {
            missing += bus
                .current
                .as_mut()
                .map_or(0, |reader| reader.read(self.pos, lane));
            if bus.current.is_none() {
                lane.fill(0.0);
            }
            let p = peak(lane);
            let g = bus.ramp.max_gain();
            bus.peak = [bus.peak[0].max(p[0] * g), bus.peak[1].max(p[1] * g)];
            bus.ramp.add_scaled(out, lane);
        }
        master(out, &mut self.master);
        missing
    }

    /// Records the smallest look-ahead of the lanes playing now (a headroom statistic).
    fn note_lookahead(&self) {
        let mut least = i64::MAX;
        for bus in &self.live.buses {
            if let Some(r) = &bus.current
                && let Some(ring) = &r.ring
                && self.pos >= r.data_start
                && self.pos < r.data_end
            {
                let ahead = (r.next + frames_i64(ring.slots())).min(r.data_end) - self.pos;
                least = least.min(ahead);
            }
        }
        if least < i64::MAX {
            self.shared
                .min_lookahead
                .fetch_min(least, Ordering::Relaxed);
        }
    }

    /// Applies every plan the handle queued, in order.
    fn apply_plans(&mut self) {
        while let Ok(plan) = self.plans.pop() {
            self.apply(plan);
        }
    }

    fn apply(&mut self, mut plan: Box<RtPlan>) {
        let ramp_frames = self.cfg.ramp_frames;
        if let Some(target) = plan.seek {
            self.pos = target;
            self.seeks += 1;
            self.priming = Some(0);
            self.shared.priming.store(true, Ordering::Relaxed);
            self.shared.position.store(target, Ordering::Release);
        }
        {
            let RtPlan { buses, deliver, .. } = &mut *plan;
            for bus in buses.iter_mut() {
                let target = bus.gain;
                bus.ramp = match self.live.buses.iter().find(|b| b.track == bus.track) {
                    Some(old) => {
                        let mut ramp = old.ramp;
                        ramp.set_target(target, ramp_frames);
                        ramp
                    }
                    None => Ramp::at(target),
                };
                bus.current = Some(take_reader(bus.current_id, &mut self.live, deliver));
                bus.next = bus
                    .next_id
                    .map(|id| take_reader(id, &mut self.live, deliver));
            }
        }
        self.shared.applied_seq.store(plan.seq, Ordering::Release);
        let old = std::mem::replace(&mut self.live, plan);
        if let Err(rtrb::PushError::Full(old)) = self.garbage.push(old) {
            // Never free on the real-time thread: leak instead (the garbage ring is sized so
            // this does not happen while a worker runs).
            std::mem::forget(old);
            self.shared.leaked.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Moves the reader `(id, from)` out of the live plan or the plan's deliveries.
fn take_reader(
    (id, from): (u64, i64),
    live: &mut RtPlan,
    deliver: &mut [Option<LaneReader>],
) -> LaneReader {
    if id == SILENT {
        return LaneReader::silent(from);
    }
    for bus in &mut live.buses {
        for slot in [&mut bus.current, &mut bus.next] {
            if slot.as_ref().is_some_and(|r| r.id == id)
                && let Some(r) = slot.take()
            {
                return r;
            }
        }
    }
    for slot in deliver.iter_mut() {
        if slot.as_ref().is_some_and(|r| r.id == id)
            && let Some(r) = slot.take()
        {
            return r;
        }
    }
    LaneReader::silent(from)
}
