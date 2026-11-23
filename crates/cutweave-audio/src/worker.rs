// SPDX-License-Identifier: GPL-3.0-or-later
//! Worker threads: keep every lane ring full (the prefetch), free what the real-time thread
//! hands back, and retry plan delivery when the plan ring was full.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cutweave_model::TrackId;
use rtrb::{Consumer, Producer};

use crate::control::Control;
use crate::dsp::CHANNELS;
use crate::lane::LaneRenderer;
use crate::rt::RtPlan;

/// Most frames rendered into a ring per step (keeps lanes interleaved fairly). Rings are this
/// much larger than the prefetch, so a ring kept within one step of full holds the prefetch.
pub(crate) const STEP_FRAMES: usize = 1024;
/// A ring is topped up once it has room for this many frames (or for the rest of its lane).
const MIN_STEP_FRAMES: usize = 256;
/// How long an idle worker sleeps before looking again (the real-time thread never wakes it).
const IDLE: Duration = Duration::from_millis(2);

/// The producer side of one lane ring and the renderer that fills it.
pub(crate) struct LaneJob {
    pub track: TrackId,
    /// Output sample from which this lane replaces the track's previous one.
    pub start: i64,
    /// End of the lane's data: nothing is written at or after it.
    pub end: i64,
    /// Stop writing here (a newer job for the track takes over).
    pub until: i64,
    /// Stop older jobs of the same track at once (seek), instead of at `start`.
    pub cancel_previous: bool,
    pub producer: Producer<f32>,
    pub renderer: LaneRenderer,
}

impl LaneJob {
    fn limit(&self) -> i64 {
        self.end.min(self.until)
    }

    fn finished(&self) -> bool {
        self.renderer.pos() >= self.limit() || self.producer.is_abandoned()
    }

    /// Frames left to write.
    fn left(&self) -> usize {
        usize::try_from(self.limit() - self.renderer.pos()).unwrap_or(0)
    }

    /// Frames the ring has room for.
    fn room(&self) -> usize {
        self.producer.slots() / CHANNELS
    }

    /// Fraction of the ring that is filled.
    #[allow(clippy::cast_precision_loss)]
    fn fill(&self) -> f64 {
        let cap = self.producer.buffer().capacity();
        (cap - self.producer.slots()) as f64 / cap.max(1) as f64
    }
}

/// Messages from the handle.
pub(crate) enum WorkerMsg {
    Job(Box<LaneJob>),
    Stop,
}

/// Duties of worker 0 besides lanes.
pub(crate) struct Janitor {
    pub garbage: Consumer<Box<RtPlan>>,
    pub control: Arc<Mutex<Control>>,
    /// Set by the handle when a plan could not be queued (plan ring full).
    pub flush_pending: Arc<AtomicBool>,
}

/// The worker loop.
pub(crate) fn run(inbox: &Receiver<WorkerMsg>, mut janitor: Option<Janitor>) {
    let mut jobs: Vec<Box<LaneJob>> = Vec::new();
    let mut scratch = vec![0.0f32; STEP_FRAMES * CHANNELS];
    loop {
        loop {
            match inbox.try_recv() {
                Ok(WorkerMsg::Job(job)) => {
                    for old in jobs.iter_mut().filter(|j| j.track == job.track) {
                        old.until = if job.cancel_previous {
                            i64::MIN
                        } else {
                            old.until.min(job.start)
                        };
                    }
                    jobs.push(job);
                }
                Ok(WorkerMsg::Stop) | Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => break,
            }
        }
        if let Some(j) = janitor.as_mut() {
            while let Ok(plan) = j.garbage.pop() {
                drop(plan);
            }
            if j.flush_pending.load(Ordering::Acquire) {
                j.control
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .flush();
            }
        }
        jobs.retain(|j| !j.finished());
        if !pump(&mut jobs, &mut scratch) {
            std::thread::park_timeout(IDLE);
        }
    }
}

/// Renders one step into the emptiest ring that has room; `false` when every ring is full or
/// finished.
fn pump(jobs: &mut [Box<LaneJob>], scratch: &mut [f32]) -> bool {
    let mut did = false;
    // Keep going while there is work, emptiest ring first, but return regularly so messages and
    // garbage are handled.
    for _ in 0..64 {
        let Some(job) = jobs
            .iter_mut()
            .filter(|j| !j.finished() && j.room() >= MIN_STEP_FRAMES.min(j.left()))
            .min_by(|a, b| a.fill().total_cmp(&b.fill()))
        else {
            break;
        };
        let n = job.room().min(job.left()).min(STEP_FRAMES);
        let block = &mut scratch[..n * CHANNELS];
        job.renderer.render(block);
        // `n` frames fit (checked above, and only this thread produces).
        if job.producer.push_entire_slice(block).is_err() {
            tracing::error!(track = %job.track, "lane ring refused a block it had room for");
        }
        did = true;
    }
    did
}
