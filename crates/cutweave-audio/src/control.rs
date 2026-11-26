// SPDX-License-Identifier: GPL-3.0-or-later
//! The control half of the mixer: configuration, [`MixerHandle`] and plan building.
//!
//! # Threading model
//!
//! ```text
//!  control thread(s)                 worker threads (N)                   real-time thread
//!  ─────────────────                 ──────────────────                   ────────────────
//!  MixerHandle::set_mix ──────┐      LaneRenderer per lane job:           Mixer::render
//!  MixerHandle::seek ─────────┼─▶    source → downmix → rubato →          ├ pop queued plans (rtrb)
//!    build plan, new rings    │      clip gain → Σ clips → ring ───────▶  ├ hand-offs at their sample
//!    jobs ──(mpsc)──────────▶ │      (emptiest ring first, ≥ prefetch)    ├ lanes × bus gains (ramped)
//!    plan ──(rtrb SPSC)─────────────────────────────────────────────────▶ ├ master gain, limiter
//!  set_master_gain ──(atomic)───────────────────────────────────────────▶ └ publish position/stats
//!                                    worker 0 frees replaced plans  ◀──(rtrb SPSC garbage)──┘
//! ```
//!
//! * **Lanes.** Each audio track is one lane: a worker renders the sum of the track's clips
//!   (each through its own source chain) into a lock-free SPSC ring of output-rate stereo
//!   samples, from a known output sample on. The rings hold `prefetch` (≥ 500 ms) of audio; the
//!   worker keeps them full, emptiest first.
//! * **Plans.** Every graph change (segments, bus settings, seek) becomes an immutable plan sent
//!   through an
//!   SPSC queue; the real-time thread applies queued plans at the start of `render`. A plan
//!   names, per bus, the reader it plays now and the reader that takes over at a given sample.
//!   Unchanged lanes keep their ring; a changed lane gets a new ring that the workers fill from
//!   `edit_latency` ahead of the playhead, and the real-time thread switches to it exactly at
//!   that sample, so an edit during playback does not glitch. A seek gives every lane a new ring
//!   from the target (flush and refill); the mixer then outputs silence without advancing until
//!   the lanes hold `prime` of audio.
//! * **Freeing.** The real-time thread never drops anything that owns memory: a replaced plan,
//!   with the readers it still holds, goes back through a garbage ring that worker 0 drains.
//!   Workers notice dropped readers (`is_abandoned`) and stop their jobs.
//! * **Wake-ups.** The real-time thread never signals anyone; workers poll every 2 ms when idle
//!   and the handle unparks them when it sends work.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{JoinHandle, Thread};

use cutweave_model::{FLICKS_PER_SECOND, Rounding, SampleRate, Time, TrackId};
use cutweave_render::AudioSegment;
use rtrb::{Producer, RingBuffer};

use crate::bus::BusGraph;
use crate::dsp::{CHANNELS, db_to_gain};
use crate::error::MixError;
use crate::lane::LaneRenderer;
use crate::limiter::{Limiter, LimiterConfig};
use crate::plan::{BusPlan, ClipPlan, plan_buses};
use crate::rt::{LaneReader, Mixer, RtBus, RtConfig, RtPlan, SILENT, Shared};
use crate::source::SourceFactory;
use crate::worker::{Janitor, LaneJob, STEP_FRAMES, WorkerMsg};

/// The smallest accepted [`MixerConfig::prefetch_ms`] (WBS 1G.01: workers stay ≥ 500 ms ahead).
pub const MIN_PREFETCH_MS: u32 = 500;

/// Plans the handle can queue before the real-time thread takes them.
const PLAN_QUEUE: usize = 8;
/// Replaced plans the real-time thread can hand back before worker 0 frees them.
const GARBAGE_QUEUE: usize = 64;

/// Mixer construction parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixerConfig {
    /// Output (device) rate. The output is always 2-channel interleaved `f32`.
    pub rate: SampleRate,
    /// Audio each lane ring holds ahead of playback (≥ [`MIN_PREFETCH_MS`]).
    pub prefetch_ms: u32,
    /// Look-ahead every playing lane needs after a seek before the mixer starts advancing
    /// (≤ `prefetch_ms`).
    pub prime_ms: u32,
    /// After a seek, start anyway once this much output time was spent priming.
    pub prime_timeout_ms: u32,
    /// How far ahead of the playhead an edit during playback takes effect (the time workers
    /// get to fill the new lane rings).
    pub edit_latency_ms: u32,
    /// Length of gain ramps (track gain changes, master gain).
    pub ramp_ms: u32,
    /// Worker threads that render lanes (≥ 1).
    pub workers: usize,
    /// Run the workers above normal priority (Windows `THREAD_PRIORITY_HIGHEST`), so a loaded
    /// machine cannot starve the prefetch (Spike S2 finding 6). No effect on other platforms yet.
    pub high_priority_workers: bool,
    /// Frames the real-time thread mixes per inner step (its scratch size; larger callbacks
    /// are processed in several steps).
    pub max_render_frames: usize,
    /// The master limiter (its look-ahead must be shorter than `edit_latency_ms`).
    pub limiter: LimiterConfig,
}

impl MixerConfig {
    /// The defaults at `rate`: 500 ms prefetch, 100 ms prime (1 s timeout), 100 ms edit latency,
    /// 10 ms ramps, 2 high-priority workers, 2048-frame steps, the default limiter (−1 dBTP,
    /// 5 ms look-ahead, 100 ms release).
    #[must_use]
    pub const fn new(rate: SampleRate) -> MixerConfig {
        MixerConfig {
            rate,
            prefetch_ms: MIN_PREFETCH_MS,
            prime_ms: 100,
            prime_timeout_ms: 1000,
            edit_latency_ms: 100,
            ramp_ms: 10,
            workers: 2,
            high_priority_workers: true,
            max_render_frames: 2048,
            limiter: LimiterConfig::DEFAULT,
        }
    }
}

/// Output frames of `ms` milliseconds at `rate` (flicks-exact, nearest sample).
pub(crate) fn ms_to_frames(rate: SampleRate, ms: u32) -> Result<i64, MixError> {
    let t = Time::from_flicks(i64::from(ms) * (FLICKS_PER_SECOND / 1000))?;
    Ok(rate.time_to_samples(t, Rounding::Nearest))
}

/// A snapshot of the mixer's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MixerStats {
    /// Output sample after the last rendered frame.
    pub position: i64,
    /// Plans built by the handle (every `set_segments` / `seek` that changed something).
    pub plans_sent: u64,
    /// Plans the real-time thread has applied (`== plans_sent` once it caught up).
    pub plans_applied: u64,
    /// A change is waiting for room in the plan queue (worker 0 delivers it as soon as the
    /// real-time thread takes the queued plans).
    pub plans_pending: bool,
    /// Whether the mixer is priming after a seek.
    pub priming: bool,
    /// `render` calls.
    pub callbacks: u64,
    /// Lane frames played as silence because a ring was late (0 in a healthy run).
    pub underrun_frames: u64,
    /// Callbacks with any late lane.
    pub underrun_callbacks: u64,
    /// Clip sources that failed to open or read (those clips play silence).
    pub source_errors: u64,
    /// Smallest look-ahead (frames) a playing lane had at the start of a callback, since the
    /// mixer started or the last [`MixerHandle::reset_lookahead`].
    pub min_lookahead_frames: Option<i64>,
    /// Plans the real-time thread could not hand back for freeing (leaked; 0 in a healthy run).
    pub leaked_plans: u64,
}

/// The handle's model of one lane reader.
#[derive(Clone, Debug)]
struct LaneModel {
    id: u64,
    from: i64,
    /// The lane's content; `None` for "no track" (a removed track fading to silence).
    clips: Option<Arc<[ClipPlan]>>,
}

impl LaneModel {
    const fn none(from: i64) -> LaneModel {
        LaneModel {
            id: SILENT,
            from,
            clips: None,
        }
    }

    fn plays(&self, bus: &BusPlan) -> bool {
        self.clips
            .as_ref()
            .is_some_and(|c| Arc::ptr_eq(c, &bus.clips) || **c == *bus.clips)
    }
}

/// The handle's model of one bus, as the real-time thread will have it after the last plan.
#[derive(Clone, Debug)]
struct BusModel {
    track: TrackId,
    gains: [f32; CHANNELS],
    current: LaneModel,
    next: Option<LaneModel>,
}

impl BusModel {
    fn effective(&self) -> &LaneModel {
        self.next.as_ref().unwrap_or(&self.current)
    }
}

/// State behind the handle's lock (shared with worker 0, which retries delivery).
pub(crate) struct Control {
    rate: SampleRate,
    sources: Arc<dyn SourceFactory>,
    ring_frames: usize,
    edit_latency: i64,
    plans: Producer<Box<RtPlan>>,
    workers: Vec<(Sender<WorkerMsg>, Option<Thread>)>,
    slots: HashMap<TrackId, usize>,
    /// The segments and bus settings of the last `set_mix` (plans are rebuilt from them).
    segments: Vec<AudioSegment>,
    graph: BusGraph,
    desired: Vec<BusPlan>,
    desired_seek: Option<i64>,
    dirty: bool,
    model: Vec<BusModel>,
    seq: u64,
    next_reader: u64,
    last_seek: Option<(u64, i64)>,
    shared: Arc<Shared>,
    pub(crate) flush_pending: Arc<AtomicBool>,
}

impl Control {
    /// Builds and queues a plan for the desired state if it changed and the queue has room;
    /// otherwise leaves `flush_pending` set so worker 0 retries.
    pub(crate) fn flush(&mut self) {
        if !self.dirty {
            self.flush_pending.store(false, Ordering::Release);
            return;
        }
        if self.plans.slots() == 0 {
            self.flush_pending.store(true, Ordering::Release);
            return;
        }
        let plan = self.build();
        // Only this (locked) side produces, and a slot was free.
        if self.plans.push(plan).is_err() {
            tracing::error!("mixer plan queue refused a plan it had room for");
        }
        self.dirty = false;
        self.flush_pending.store(false, Ordering::Release);
    }

    /// The positions plans are built against, `(heard, mixed)`: the pending seek target until
    /// the real-time thread applied it; otherwise the published output position (the playhead
    /// heard, which edits hand off `edit_latency` after) and mix position (where the lanes are,
    /// which tells the hand-offs that already happened; the limiter latency ahead).
    fn playhead(&self) -> (i64, i64) {
        let applied = self.shared.applied_seq.load(Ordering::Acquire);
        match self.last_seek {
            Some((seq, target)) if applied < seq => (target, target),
            _ => (
                self.shared.position.load(Ordering::Acquire),
                self.shared.mix_position.load(Ordering::Acquire),
            ),
        }
    }

    // The plan crosses into the real-time thread as a box so that thread never allocates or
    // frees it (it moves the box back through the garbage ring).
    #[allow(clippy::unnecessary_box_returns)]
    fn build(&mut self) -> Box<RtPlan> {
        self.seq += 1;
        let seq = self.seq;
        let seek = self.desired_seek.take();
        let (heard, pos) = self.playhead();
        let desired = self.desired.clone();
        let mut deliver = Vec::new();
        let mut model = Vec::with_capacity(desired.len() + self.model.len());

        if let Some(target) = seek {
            self.last_seek = Some((seq, target));
            for bus in &desired {
                let lane = self.open_lane(bus, target, true, &mut deliver);
                model.push(BusModel {
                    track: bus.track,
                    gains: bus.gains,
                    current: lane,
                    next: None,
                });
            }
        } else {
            for m in &mut self.model {
                if m.next.as_ref().is_some_and(|n| n.from <= pos)
                    && let Some(n) = m.next.take()
                {
                    m.current = n;
                }
            }
            // Never before the mix position (the edit latency exceeds the limiter's).
            let soon = heard.saturating_add(self.edit_latency).max(pos);
            let switch_at = |m: &BusModel| match &m.next {
                Some(n) if n.from > pos => n.from,
                _ => soon,
            };
            let old = std::mem::take(&mut self.model);
            for bus in &desired {
                let entry = match old.iter().find(|m| m.track == bus.track) {
                    Some(m) if m.effective().plays(bus) => BusModel {
                        gains: bus.gains,
                        ..m.clone()
                    },
                    Some(m) => {
                        let at = switch_at(m);
                        BusModel {
                            track: bus.track,
                            gains: bus.gains,
                            current: m.current.clone(),
                            next: Some(self.open_lane(bus, at, false, &mut deliver)),
                        }
                    }
                    None => BusModel {
                        track: bus.track,
                        gains: bus.gains,
                        current: LaneModel::none(pos),
                        next: Some(self.open_lane(bus, soon, false, &mut deliver)),
                    },
                };
                model.push(entry);
            }
            for m in old
                .iter()
                .filter(|m| !desired.iter().any(|b| b.track == m.track))
            {
                if m.effective().clips.is_none() {
                    if m.next.is_some() {
                        model.push(m.clone()); // its hand-off to silence is still ahead
                    }
                    continue;
                }
                model.push(BusModel {
                    next: Some(LaneModel::none(switch_at(m))),
                    ..m.clone()
                });
            }
        }
        self.model = model;
        let buses = self
            .model
            .iter()
            .map(|m| {
                RtBus::new(
                    m.track,
                    m.gains,
                    (m.current.id, m.current.from),
                    m.next.as_ref().map(|n| (n.id, n.from)),
                )
            })
            .collect();
        Box::new(RtPlan {
            seq,
            seek,
            buses,
            deliver,
        })
    }

    /// Starts a lane for `bus` that takes over at output sample `from`: a new ring, and a job on
    /// the track's worker that fills it. Lanes with no audio after `from` get no ring.
    fn open_lane(
        &mut self,
        bus: &BusPlan,
        from: i64,
        cancel_previous: bool,
        deliver: &mut Vec<Option<LaneReader>>,
    ) -> LaneModel {
        let clips = Some(bus.clips.clone());
        let Some((first, end)) = bus.span() else {
            return LaneModel {
                id: SILENT,
                from,
                clips,
            };
        };
        let data_start = first.max(from);
        if end <= data_start {
            return LaneModel {
                id: SILENT,
                from,
                clips,
            };
        }
        self.next_reader += 1;
        let id = self.next_reader;
        let (producer, consumer) = RingBuffer::<f32>::new(self.ring_frames * CHANNELS);
        let renderer = LaneRenderer::new(
            bus.clips.clone(),
            self.sources.clone(),
            self.rate,
            data_start,
            self.shared.source_errors_counter(),
        );
        let job = LaneJob {
            track: bus.track,
            start: from,
            end,
            until: i64::MAX,
            cancel_previous,
            producer,
            renderer,
        };
        let next_slot = self.slots.len();
        let slot = *self.slots.entry(bus.track).or_insert(next_slot);
        let (tx, thread) = &self.workers[slot % self.workers.len()];
        if tx.send(WorkerMsg::Job(Box::new(job))).is_err() {
            tracing::error!(track = %bus.track, "audio worker is gone; lane stays silent");
        }
        if let Some(thread) = thread {
            thread.unpark();
        }
        deliver.push(Some(LaneReader {
            id,
            from,
            data_start,
            data_end: end,
            next: data_start,
            ring: Some(consumer),
        }));
        LaneModel { id, from, clips }
    }
}

/// The control half of the mixer: feeds it segments, seeks, sets the master gain and reads its
/// statistics. `Send + Sync`; calls are serialized by an internal lock that the real-time
/// thread never takes. Dropping it stops the worker threads.
pub struct MixerHandle {
    control: Arc<Mutex<Control>>,
    shared: Arc<Shared>,
    rate: SampleRate,
    threads: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for MixerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MixerHandle")
            .field("rate", &self.rate)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl Mixer {
    /// A mixer at `config.rate` reading clips through `sources`, with its control handle.
    /// Starts `config.workers` worker threads (they stop when the handle is dropped).
    #[allow(clippy::too_many_lines)] // validation, threads and wiring in one place
    pub fn new(
        config: MixerConfig,
        sources: Arc<dyn SourceFactory>,
    ) -> Result<(Mixer, MixerHandle), MixError> {
        if config.prefetch_ms < MIN_PREFETCH_MS {
            return Err(MixError::Config("prefetch_ms must be at least 500"));
        }
        if config.prime_ms > config.prefetch_ms {
            return Err(MixError::Config("prime_ms must not exceed prefetch_ms"));
        }
        if config.workers == 0 {
            return Err(MixError::Config("at least one worker thread is needed"));
        }
        if config.max_render_frames < 64 {
            return Err(MixError::Config("max_render_frames must be at least 64"));
        }
        if config.limiter.lookahead_ms >= config.edit_latency_ms {
            return Err(MixError::Config(
                "the limiter look-ahead must be shorter than edit_latency_ms",
            ));
        }
        config.limiter.validate()?;
        let rate = config.rate;
        let frames = |ms| ms_to_frames(rate, ms);
        let ring_frames = usize::try_from(frames(config.prefetch_ms)?)
            .map_err(|_| MixError::Config("prefetch_ms out of range"))?
            + STEP_FRAMES;
        let shared = Arc::new(Shared::new());
        let (plan_tx, plan_rx) = RingBuffer::new(PLAN_QUEUE);
        let (garbage_tx, garbage_rx) = RingBuffer::new(GARBAGE_QUEUE);
        let flush_pending = Arc::new(AtomicBool::new(false));

        let mut inboxes = Vec::new();
        let mut workers = Vec::new();
        for _ in 0..config.workers {
            let (tx, rx) = channel();
            workers.push((tx, None));
            inboxes.push(rx);
        }
        let control = Arc::new(Mutex::new(Control {
            rate,
            sources,
            ring_frames,
            edit_latency: frames(config.edit_latency_ms)?,
            plans: plan_tx,
            workers,
            slots: HashMap::new(),
            segments: Vec::new(),
            graph: BusGraph::new(),
            desired: Vec::new(),
            desired_seek: None,
            dirty: false,
            model: Vec::new(),
            seq: 0,
            next_reader: SILENT,
            last_seek: None,
            shared: shared.clone(),
            flush_pending: flush_pending.clone(),
        }));

        let mut janitor = Some(Janitor {
            garbage: garbage_rx,
            control: control.clone(),
            flush_pending,
        });
        let mut threads = Vec::new();
        let high_priority = config.high_priority_workers;
        for (i, inbox) in inboxes.into_iter().enumerate() {
            let duties = janitor.take();
            let spawned = std::thread::Builder::new()
                .name(format!("cutweave-audio-worker-{i}"))
                .spawn(move || {
                    if high_priority && !crate::priority::raise_current_thread() {
                        tracing::debug!("audio worker {i} runs at normal priority");
                    }
                    crate::worker::run(&inbox, duties);
                });
            match spawned {
                Ok(handle) => threads.push(handle),
                Err(e) => {
                    stop_workers(&control, threads);
                    return Err(MixError::Spawn(e.to_string()));
                }
            }
        }
        {
            let mut c = control.lock().unwrap_or_else(PoisonError::into_inner);
            for ((_, thread), handle) in c.workers.iter_mut().zip(&threads) {
                *thread = Some(handle.thread().clone());
            }
        }

        let rt = RtConfig {
            rate,
            prime_frames: frames(config.prime_ms)?,
            prime_timeout_frames: u64::try_from(frames(config.prime_timeout_ms)?).unwrap_or(0),
            ramp_frames: u32::try_from(frames(config.ramp_ms)?).unwrap_or(u32::MAX),
            max_frames: config.max_render_frames,
        };
        let limiter = Limiter::new(config.limiter, rate)?;
        let mixer = Mixer::from_parts(rt, limiter, plan_rx, garbage_tx, shared.clone());
        let handle = MixerHandle {
            control,
            shared,
            rate,
            threads,
        };
        Ok((mixer, handle))
    }
}

fn stop_workers(control: &Arc<Mutex<Control>>, threads: Vec<JoinHandle<()>>) {
    {
        let c = control.lock().unwrap_or_else(PoisonError::into_inner);
        for (tx, thread) in &c.workers {
            // A worker that already exited has dropped its inbox; nothing to stop.
            let _ = tx.send(WorkerMsg::Stop);
            if let Some(thread) = thread {
                thread.unpark();
            }
        }
    }
    for t in threads {
        if t.join().is_err() {
            tracing::error!("an audio worker thread panicked");
        }
    }
}

impl MixerHandle {
    fn control(&self) -> std::sync::MutexGuard<'_, Control> {
        self.control.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The output rate.
    #[must_use]
    pub fn rate(&self) -> SampleRate {
        self.rate
    }

    /// Replaces the mixed segments (the output of `evaluate_audio` over the whole sequence),
    /// every track at its segments' track gain: [`set_mix`](Self::set_mix) with
    /// [`BusGraph::from_segments`].
    pub fn set_segments(&self, segments: &[AudioSegment]) -> Result<(), MixError> {
        self.set_mix(segments, &BusGraph::from_segments(segments))
    }

    /// Replaces the mixed segments and the track buses' settings (gain, pan, mute, solo).
    ///
    /// Tracks whose clips did not change keep their prefetched audio; bus changes (gain, pan,
    /// mute, solo) ramp in at the next callback; changed tracks switch to their new content
    /// `edit_latency` after the playhead. Pass the segments of muted and unsoloed tracks too
    /// (`evaluate_mix`), so muting and soloing never re-render a lane. Fails without changing
    /// anything if a segment is unsupported.
    pub fn set_mix(&self, segments: &[AudioSegment], buses: &BusGraph) -> Result<(), MixError> {
        let plan = plan_buses(segments, self.rate, buses)?;
        let mut c = self.control();
        c.segments = segments.to_vec();
        c.graph = buses.clone();
        if c.desired != plan {
            c.desired = plan;
            c.dirty = true;
        }
        c.flush();
        Ok(())
    }

    /// Replaces only the bus settings (gain, pan, mute, solo) of the current segments: ramps in
    /// at the next callback, no lane is re-rendered.
    pub fn set_buses(&self, buses: &BusGraph) -> Result<(), MixError> {
        let segments = self.control().segments.clone();
        self.set_mix(&segments, buses)
    }

    /// The bus settings in effect (as last set).
    #[must_use]
    pub fn buses(&self) -> BusGraph {
        self.control().graph.clone()
    }

    /// Moves the playhead to `t` (nearest output sample): every lane is flushed and refilled
    /// from there, and the mixer primes before it advances again.
    pub fn seek(&self, t: Time) {
        let sample = self.rate.time_to_samples(t, Rounding::Nearest);
        let mut c = self.control();
        c.desired_seek = Some(sample);
        c.dirty = true;
        c.flush();
    }

    /// Sets the master gain in dB (ramped; NaN means 0 dB, ≤ −144 dB is silence).
    pub fn set_master_gain(&self, db: f64) {
        self.shared
            .master_gain
            .store(db_to_gain(db).to_bits(), Ordering::Relaxed);
    }

    /// Sets the master gain as a linear factor, the unit of the `view` state's `master_gain`
    /// (`view.set {patch: {master_gain}}`; 1 = unity, 0 = silence). Ramped like
    /// [`set_master_gain`](Self::set_master_gain); NaN means unity, negative values are silence,
    /// values above +24 dB are clamped.
    pub fn set_master_gain_linear(&self, gain: f64) {
        let db = if gain.is_nan() {
            0.0
        } else if gain <= 0.0 {
            f64::NEG_INFINITY
        } else {
            20.0 * gain.log10()
        };
        self.set_master_gain(db);
    }

    /// The counters.
    #[must_use]
    pub fn stats(&self) -> MixerStats {
        let s = &self.shared;
        let lookahead = s.min_lookahead.load(Ordering::Relaxed);
        let (plans_sent, plans_pending) = {
            let c = self.control();
            (c.seq, c.dirty)
        };
        MixerStats {
            position: s.position.load(Ordering::Acquire),
            plans_sent,
            plans_applied: s.applied_seq.load(Ordering::Acquire),
            plans_pending,
            priming: s.priming.load(Ordering::Relaxed),
            callbacks: s.callbacks.load(Ordering::Relaxed),
            underrun_frames: s.underrun_frames.load(Ordering::Relaxed),
            underrun_callbacks: s.underrun_callbacks.load(Ordering::Relaxed),
            source_errors: s.source_errors.load(Ordering::Relaxed),
            min_lookahead_frames: (lookahead != i64::MAX).then_some(lookahead),
            leaked_plans: s.leaked.load(Ordering::Relaxed),
        }
    }

    /// Restarts the [`MixerStats::min_lookahead_frames`] measurement.
    pub fn reset_lookahead(&self) {
        self.shared.min_lookahead.store(i64::MAX, Ordering::Relaxed);
    }
}

impl MixerStats {
    /// Whether every change made through the handle has reached the real-time thread.
    #[must_use]
    pub const fn settled(&self) -> bool {
        !self.plans_pending && self.plans_applied == self.plans_sent
    }
}

impl Drop for MixerHandle {
    fn drop(&mut self) {
        stop_workers(&self.control, std::mem::take(&mut self.threads));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_cross_threads() {
        fn send<T: Send>() {}
        fn send_sync<T: Send + Sync>() {}
        send::<Mixer>();
        send_sync::<MixerHandle>();
    }

    #[test]
    fn limiter_settings_are_validated() {
        let sources: Arc<dyn SourceFactory> = Arc::new(crate::ToneFactory::new());
        let mut c = MixerConfig::new(SampleRate::HZ_48_000);
        c.limiter.lookahead_ms = 100;
        assert!(
            Mixer::new(c, sources.clone()).is_err(),
            "look-ahead >= edit latency"
        );
        let mut c = MixerConfig::new(SampleRate::HZ_48_000);
        c.limiter.ceiling_dbtp = 3.0;
        assert!(Mixer::new(c, sources).is_err());
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn milliseconds_convert_through_flicks() {
        assert_eq!(ms_to_frames(SampleRate::HZ_48_000, 500).unwrap(), 24_000);
        assert_eq!(ms_to_frames(SampleRate::HZ_44_100, 10).unwrap(), 441);
        assert_eq!(ms_to_frames(SampleRate::HZ_44_100, 1).unwrap(), 44); // 44.1 → nearest
    }
}
