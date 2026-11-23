// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic-ID mode across crash recovery (arch §5.2, WP-1B.08).
//!
//! Under `--deterministic-ids <seed>` the doc actor draws entity ids and engine-generated
//! request ids from two [`SeededGen`] streams. A recovered engine that restarted both streams
//! from the seed would mint ids that already exist in the recovered document (and request ids
//! already in the dedup table). [`IdTracker`] counts what each stream issued; the journal
//! stores the counts with every transaction ([`IdState`]) and recovery resumes the streams
//! right after the last recovered transaction.
//!
//! Ids drawn after the last commit (a failed command, a cancelled gesture) are never in the
//! document, so issuing them again after recovery cannot collide.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use cutweave_commands::{DocConfig, Registry};
use cutweave_model::{IdGen, Project, SeededGen};
use serde::{Deserialize, Serialize};

/// Mixed into the seed for the request-id stream. Must equal the constant
/// `cutweave_commands::DocConfig::deterministic` uses (a test pins them together), so a
/// recovered session continues exactly the stream a fresh one would have used.
pub(crate) const REQUEST_ID_STREAM: u64 = 0x5245_5155_4553_5453; // "REQUESTS"

/// How many values each deterministic id stream has issued.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdState {
    /// Entity ids (`med_…`, `itm_…`, …).
    pub entity: u64,
    /// Engine-generated request ids.
    pub request: u64,
}

/// The deterministic id streams of one session, with shared counters the journal reads.
///
/// Cloning shares the counters. Build the session's [`DocConfig`] with [`config`](Self::config)
/// (once per tracker) and give a clone to the journal
/// ([`JournalConfig::with_ids`](super::JournalConfig::with_ids)).
#[derive(Clone, Debug)]
pub struct IdTracker {
    seed: u64,
    start: IdState,
    entity: Arc<AtomicU64>,
    request: Arc<AtomicU64>,
}

impl IdTracker {
    /// Fresh streams for `seed`.
    #[must_use]
    pub fn new(seed: u64) -> IdTracker {
        IdTracker::resume(seed, IdState::default())
    }

    /// Streams for `seed` positioned after `state` (recovery).
    #[must_use]
    pub fn resume(seed: u64, state: IdState) -> IdTracker {
        IdTracker {
            seed,
            start: state,
            entity: Arc::new(AtomicU64::new(state.entity)),
            request: Arc::new(AtomicU64::new(state.request)),
        }
    }

    /// These streams moved past every entity id `doc` already holds from this seed's stream.
    ///
    /// A new engine that opens a saved project with no journaled id state (no journal
    /// transactions, or a fresh process) would otherwise restart the entity stream at the seed
    /// and mint ids the document has (arch §4.4, "decided in WP-1B.08"). The scan is a pure
    /// function of the document, so the run stays reproducible. The request-id stream needs
    /// no such step: request ids are not stored in the document.
    #[must_use]
    pub fn past(self, doc: &Project) -> IdTracker {
        let high = doc.seeded_id_high_water(self.seed);
        if high <= self.start.entity {
            return self;
        }
        IdTracker::resume(
            self.seed,
            IdState {
                entity: high,
                request: self.start.request,
            },
        )
    }

    /// The seed.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// What the streams have issued so far.
    #[must_use]
    pub fn state(&self) -> IdState {
        IdState {
            entity: self.entity.load(Ordering::Relaxed),
            request: self.request.load(Ordering::Relaxed),
        }
    }

    /// The entity-id and request-id generators, positioned where this tracker started and
    /// counting into it.
    #[must_use]
    pub fn generators(&self) -> (Box<dyn IdGen + Send>, Box<dyn IdGen + Send>) {
        (
            Box::new(Counted::new(
                self.seed,
                self.start.entity,
                Arc::clone(&self.entity),
            )),
            Box::new(Counted::new(
                self.seed ^ REQUEST_ID_STREAM,
                self.start.request,
                Arc::clone(&self.request),
            )),
        )
    }

    /// [`DocConfig::deterministic`] for `doc` (seeded ids, clock frozen at the epoch) with this
    /// tracker's generators.
    #[must_use]
    pub fn config(&self, registry: Arc<Registry>, doc: Project) -> DocConfig {
        let (ids, request_ids) = self.generators();
        DocConfig::deterministic(registry, doc, self.seed)
            .with_ids(ids)
            .with_request_ids(request_ids)
    }
}

/// A seeded stream that counts what it issues.
struct Counted {
    inner: SeededGen,
    issued: Arc<AtomicU64>,
}

impl Counted {
    fn new(seed: u64, skip: u64, issued: Arc<AtomicU64>) -> Counted {
        let mut inner = SeededGen::new(seed);
        // Cheap (a few hash rounds per id) and only once, at recovery.
        for _ in 0..skip {
            inner.next_u128();
        }
        Counted { inner, issued }
    }
}

impl IdGen for Counted {
    fn next_u128(&mut self) -> u128 {
        self.issued.fetch_add(1, Ordering::Relaxed);
        self.inner.next_u128()
    }
}
