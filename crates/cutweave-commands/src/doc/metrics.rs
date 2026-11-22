// SPDX-License-Identifier: GPL-3.0-or-later
//! Metrics hooks of the document actor: [`DocMetrics`] (pluggable sink, e.g. the engine's
//! metrics registry or the bench harness) and [`DocStats`] (counters every actor keeps).

use std::time::Duration;

use super::{
    request::Applied,
    sync::{AtomicU64, Ordering},
};
use crate::error::{CmdError, Rev};

/// How a request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It ran and succeeded.
    Applied,
    /// It repeated a remembered `request_id`; the stored result was returned.
    Replayed,
    /// It failed with this [`CmdError::code`].
    Failed {
        /// The error code (`conflict`, `invalid`, …).
        code: &'static str,
    },
}

impl Outcome {
    pub(crate) fn of<T>(result: &Result<T, CmdError>) -> Outcome {
        match result {
            Ok(_) => Outcome::Applied,
            Err(e) => Outcome::Failed { code: e.code() },
        }
    }

    pub(crate) fn of_applied(result: &Result<Applied, CmdError>) -> Outcome {
        match result {
            Ok(applied) if applied.replayed => Outcome::Replayed,
            other => Outcome::of(other),
        }
    }
}

/// One processed request.
#[derive(Clone, Copy, Debug)]
pub struct RequestSample<'a> {
    /// The command name (`clip.split`), or the engine task (`begin_tx`, `run`).
    pub name: &'a str,
    /// How it ended.
    pub outcome: Outcome,
    /// Time spent in the inbox before the actor picked it up.
    pub queue_wait: Duration,
    /// Time the actor spent on it (execution, journaling, publication).
    pub exec: Duration,
    /// The live revision afterwards.
    pub rev: Rev,
}

/// Receives the actor's measurements. Called on the actor thread after the reply was sent:
/// keep it cheap and never block.
pub trait DocMetrics: Send + Sync {
    /// A request was processed.
    fn request(&self, sample: &RequestSample<'_>) {
        let _ = sample;
    }

    /// `count` subscribers overflowed their queues and must resync.
    fn subscribers_lagged(&self, count: usize) {
        let _ = count;
    }

    /// A command panicked (and was rolled back).
    fn command_panicked(&self, command: &str) {
        let _ = command;
    }
}

/// Discards every measurement.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoMetrics;

impl DocMetrics for NoMetrics {}

/// Counters kept by every actor ([`DocHandle::stats`](super::DocHandle::stats)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocStats {
    /// Requests waiting in the inbox.
    pub queued: u64,
    /// Requests processed.
    pub processed: u64,
    /// Requests that succeeded (including replays).
    pub applied: u64,
    /// Requests answered from the dedup table.
    pub replayed: u64,
    /// Requests that failed.
    pub failed: u64,
    /// Commands that panicked.
    pub panicked: u64,
    /// Subscriber overflows.
    pub lagged: u64,
}

pub(crate) struct Counters {
    queued: AtomicU64,
    processed: AtomicU64,
    applied: AtomicU64,
    replayed: AtomicU64,
    failed: AtomicU64,
    panicked: AtomicU64,
    lagged: AtomicU64,
}

impl Counters {
    pub(crate) fn new() -> Counters {
        Counters {
            queued: AtomicU64::new(0),
            processed: AtomicU64::new(0),
            applied: AtomicU64::new(0),
            replayed: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            panicked: AtomicU64::new(0),
            lagged: AtomicU64::new(0),
        }
    }

    pub(crate) fn enqueued(&self) {
        self.queued.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn dequeued(&self) {
        self.queued.fetch_sub(1, Ordering::Relaxed);
    }

    pub(crate) fn record(&self, outcome: Outcome) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        match outcome {
            Outcome::Applied => self.applied.fetch_add(1, Ordering::Relaxed),
            Outcome::Replayed => {
                self.applied.fetch_add(1, Ordering::Relaxed);
                self.replayed.fetch_add(1, Ordering::Relaxed)
            }
            Outcome::Failed { .. } => self.failed.fetch_add(1, Ordering::Relaxed),
        };
    }

    pub(crate) fn panicked(&self) {
        self.panicked.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn lagged(&self, count: usize) {
        self.lagged
            .fetch_add(u64::try_from(count).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> DocStats {
        DocStats {
            queued: self.queued.load(Ordering::Relaxed),
            processed: self.processed.load(Ordering::Relaxed),
            applied: self.applied.load(Ordering::Relaxed),
            replayed: self.replayed.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            panicked: self.panicked.load(Ordering::Relaxed),
            lagged: self.lagged.load(Ordering::Relaxed),
        }
    }
}
