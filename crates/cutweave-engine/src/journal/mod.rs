// SPDX-License-Identifier: GPL-3.0-or-later
//! The command journal: append-only, crash-safe log of committed transactions, replayed on
//! recovery (WP-1C.02; arch §5.1, §5.2, §14, §17; ADR-0006).
//!
//! # What is journaled
//!
//! One entry per **committed** transaction, written by the doc actor through its
//! [`CommitSink`](cutweave_commands::CommitSink) **before** the transaction becomes visible or
//! is acknowledged: committed revision and the one it builds on, transaction id and kind,
//! actor, label, wall-clock time, every request in it (command name, params with `#[secret]`
//! fields redacted, `request_id` with its dedup record and result), the net patch (with
//! preimages, so it is the transaction's resulting ids and values), the history cause, and in
//! `--deterministic-ids` mode the id streams' positions ([`IdState`]). Gestures and proposals
//! are journaled once, on commit, as their collapsed patch; cancelled ones and `view.*`
//! commands never reach the journal (they never commit a document change).
//!
//! Changes of the checkpoints (WP-2I.03) are journaled the same way, as [`CheckpointEntry`]
//! markers: the checkpoint made (metadata only: recovery rebuilds its document by replaying to
//! its revision), the ones removed, the highest id issued and the request. A save carries the
//! checkpoints made before it into the new segment ([`Carry`]) as patches from the saved
//! document, so they survive saves, crashes and reopening the project.
//!
//! # Format
//!
//! `.cutweave/journal/<base-rev>.jnl` segments of length-prefixed, CRC-32-checked JSON
//! entries (see [`format`](self::format) for the byte layout). Each segment starts with a
//! [`SegmentHeader`] naming the saved document it continues (its committed revision and the
//! SHA-256 of its bytes), then what it carries over from before the save (requests,
//! checkpoints), then the transactions and checkpoint markers.
//!
//! # Durability ([`SyncPolicy`])
//!
//! Every transaction is `write()`n to the OS before its acknowledgement, so **an acknowledged
//! transaction survives the engine process being killed** at any point (the page cache belongs
//! to the OS). By default `fdatasync` is a group commit on a flusher thread at most
//! [`DEFAULT_GROUP_COMMIT`] (50 ms) after the first unsynced write, so on **power loss** at
//! most the last ~50 ms of acknowledged edits can be lost; every save, [`Journal::sync`] and
//! [`Journal::close`] sync immediately.
//!
//! Measured (`cargo bench -p cutweave-engine --bench journal`, rename on a 400-item project,
//! Windows 11 / NVMe, release): one `fdatasync` after a ~0.6 KB append costs p50 0.59 ms,
//! p99 0.77 ms. Round trip submit → journal → ack: group commit p50 77 µs, p95 148 µs, p99
//! 244 µs (same as no `fdatasync` at all, within noise); `fdatasync` per commit p50 658 µs,
//! p95 788 µs, p99 925 µs. Per-commit sync would fit the 16 ms round-trip budget (arch §19) on
//! this machine, but device flushes vary by orders of magnitude (HDDs, macOS `F_FULLFSYNC`:
//! 5–30 ms), they serialize every commit on the actor (here ~1,500 commits/s at most), and the
//! kill-safety that recovery relies on comes from the `write()` alone. So the default keeps the
//! acknowledgement independent of the device and bounds the power-loss window instead;
//! [`SyncPolicy::EveryCommit`] is available where a zero window is worth the latency.
//!
//! # Save, rotation and compaction
//!
//! A save ([`ProjectStore::save_snapshot`](crate::project_io::ProjectStore::save_snapshot))
//! [rotates](Journal::rotate) first: a new segment over the document about to be written,
//! holding the requests still inside the dedup window and any transaction committed while the
//! save was prepared; both segments are synced; then the document is written atomically; then
//! the superseded segment is kept as the previous journal and older ones are deleted. A crash
//! at any step leaves a segment chain that continues whichever document is on disk.
//!
//! # Recovery
//!
//! [`ProjectStore::open`](crate::project_io::ProjectStore::open) reports
//! [`RecoveryAvailable`] when the chain continuing the saved document holds transactions.
//! [`ProjectStore::recover`](crate::project_io::ProjectStore::recover) replays them through the
//! model, validating each (checksum, revision continuity, the patch's preimages, model
//! invariants), and stops at the first record that fails: a torn tail after a crash (never an
//! acknowledged transaction) or corruption, reported in [`ReplayReport`]. The [`Recovered`]
//! state rebuilds the doc actor ([`Recovered::configure`]): document and revision, undo
//! history ([`DocConfig::with_recovered_history`]), checkpoints
//! ([`DocConfig::with_recovered_checkpoints`]), the dedup table with each request's
//! persisted wall-clock time (a resend within 10 minutes of the original still returns the
//! original result after a restart), and the deterministic id streams.

mod crc;
pub mod format;
mod ids;
mod scan;
mod writer;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cutweave_commands::{
    CheckpointLedger, Clock, DEDUP_WINDOW, DedupRecord, DocConfig, JournaledCommit, Rev,
    SystemClock, TxId, WallTime,
};
use cutweave_model::Project;

pub use format::{
    CarriedCheckpoint, Carry, CheckpointEntry, CommitEntry, FORMAT_VERSION, SegmentHeader,
    StopReason,
};
pub use ids::{IdState, IdTracker};
pub use scan::{RecoveryAvailable, ReplayReport, ReplayStop};
pub use writer::{
    DEFAULT_GROUP_COMMIT, Journal, JournalConfig, JournalStats, Rotation, SyncPolicy,
};

use scan::{DirScan, ResumePoint, replay};
use writer::Start;

/// Errors from the journal.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JournalError {
    /// A filesystem operation failed.
    #[error("{op} failed for {}: {source}", path.display())]
    Io {
        /// What was being done.
        op: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The OS error.
        #[source]
        source: io::Error,
    },
    /// A record could not be encoded.
    #[error("cannot encode a journal record: {0}")]
    Encode(#[source] serde_json::Error),
    /// A record exceeds the frame size limit.
    #[error("a journal record of {0} bytes exceeds the 4 GiB frame limit")]
    TooLarge(usize),
    /// The journal was closed.
    #[error("the journal is closed")]
    Closed,
    /// An earlier write or sync failed, so the journal may miss a transaction.
    #[error(
        "the journal refuses changes after a failure ({0}); save the project to start a new journal segment"
    )]
    Failed(String),
    /// A transaction arrived out of revision order (a programming error).
    #[error("transaction at revision {rev} is not newer than the last journaled revision {last}")]
    OutOfOrder {
        /// The last journaled revision.
        last: Rev,
        /// The offending revision.
        rev: Rev,
    },
    /// A save of an older revision than the last save (two saves raced).
    #[error("cannot save revision {rev}: the project was already saved at revision {base}")]
    StaleBase {
        /// The last saved revision.
        base: Rev,
        /// The revision being saved.
        rev: Rev,
    },
    /// A save of a revision the journal has not seen (not a committed snapshot of this doc).
    #[error("cannot save revision {rev}: the journal ends at revision {last}")]
    FutureBase {
        /// The last journaled revision.
        last: Rev,
        /// The revision being saved.
        rev: Rev,
    },
    /// Unsaved work in the journal must be recovered or discarded first.
    #[error("the journal holds unsaved changes; recover or discard them before editing or saving")]
    RecoveryPending,
    /// The project already has a journal running.
    #[error("the project's journal is already running")]
    AlreadyStarted,
    /// A journaled project must be saved with its committed revision
    /// ([`save_snapshot`](crate::project_io::ProjectStore::save_snapshot)).
    #[error(
        "this project is journaled: save a committed snapshot (with its revision) so the journal can rotate"
    )]
    RevisionRequired,
    /// The snapshot to save has uncommitted (live gesture or proposal) changes.
    #[error(
        "cannot save revision {rev}: it is not a committed snapshot (committed revision {committed_rev})"
    )]
    NotCommitted {
        /// The snapshot's live revision.
        rev: Rev,
        /// Its committed revision.
        committed_rev: Rev,
    },
}

impl JournalError {
    pub(crate) fn io(op: &'static str, path: &Path, source: io::Error) -> JournalError {
        JournalError::Io {
            op,
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Options for [`ProjectStore::recover`](crate::project_io::ProjectStore::recover).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoverOptions {
    /// The current wall-clock time: requests older than `dedup_window` are not restored.
    pub now: WallTime,
    /// The doc actor's dedup window.
    pub dedup_window: Duration,
}

impl RecoverOptions {
    /// At `now` with the 10-minute dedup window.
    #[must_use]
    pub fn at(now: WallTime) -> RecoverOptions {
        RecoverOptions {
            now,
            dedup_window: DEDUP_WINDOW,
        }
    }

    /// At the time of `clock` (the doc actor's clock).
    #[must_use]
    pub fn from_clock(clock: &dyn Clock) -> RecoverOptions {
        RecoverOptions::at(clock.now())
    }
}

impl Default for RecoverOptions {
    fn default() -> RecoverOptions {
        RecoverOptions::from_clock(&SystemClock)
    }
}

/// The document state a journal replay produced: what the doc actor continues from.
#[derive(Clone, Debug)]
pub struct Recovered {
    /// The document after the last replayed transaction.
    pub project: Project,
    /// Its committed revision.
    pub rev: Rev,
    /// The replayed transactions, in commit order (the undo history).
    pub commits: Vec<JournaledCommit>,
    /// The checkpoints (WP-2I.03): carried over the last save, made and removed since.
    pub checkpoints: CheckpointLedger,
    /// The requests to keep deduplicating (inside the window at recovery time).
    pub requests: Vec<DedupRecord>,
    /// The first transaction id the doc actor may allocate.
    pub next_tx: TxId,
    /// The deterministic id streams' positions after the last replayed transaction.
    pub ids: Option<IdState>,
    /// The deterministic seed the journal was written with.
    pub seed: Option<u64>,
    /// What the replay did.
    pub report: ReplayReport,
}

impl Recovered {
    /// Applies the recovered state to `config`, which must be built over
    /// [`project`](Self::project): revision, next transaction id, remembered requests, undo
    /// history ([`DocConfig::with_recovered_history`]) and checkpoints
    /// ([`DocConfig::with_recovered_checkpoints`]).
    #[must_use]
    pub fn configure(&self, config: DocConfig) -> DocConfig {
        config
            .with_rev(self.rev)
            .with_first_tx_id(self.next_tx)
            .with_restored_requests(self.requests.clone())
            .with_recovered_history(self.commits.clone())
            .with_recovered_checkpoints(self.checkpoints.clone())
    }

    /// The deterministic id streams for a session with `seed`, continuing where the journal's
    /// left off (fresh streams if the journal was written with another seed or none).
    #[must_use]
    pub fn id_tracker(&self, seed: u64) -> IdTracker {
        match (self.seed, self.ids) {
            (Some(journaled), Some(state)) if journaled == seed => IdTracker::resume(seed, state),
            (Some(journaled), _) if journaled != seed => {
                tracing::warn!(
                    journaled,
                    seed,
                    "the journal was written with another --deterministic-ids seed; id streams restart"
                );
                IdTracker::new(seed)
            }
            _ => IdTracker::new(seed),
        }
    }
}

impl Journal {
    /// What recovery would produce from the journal in `dir` for the saved document `document`
    /// (its bytes on disk) parsed as `base`, without changing anything on disk. For tools and
    /// diagnostics; the engine recovers through
    /// [`ProjectStore::recover`](crate::project_io::ProjectStore::recover).
    pub fn replay(
        dir: &Path,
        document: &[u8],
        base: &Project,
        options: &RecoverOptions,
    ) -> Result<Recovered, JournalError> {
        let sha = crate::project_io::document_sha256(document);
        let mut pending = PendingJournal::scan(dir, dir, sha)
            .map_err(|e| JournalError::io("read journal", dir, e))?;
        Ok(pending.recover(base, options, false))
    }
}

/// Where the journal continues once recovery (or discard) has been decided.
#[derive(Debug)]
struct Plan {
    /// The segment to continue, the offset just past its last valid entry, and its
    /// transactions; `None`: a new segment.
    resume: Option<ResumePoint>,
    /// Keep a copy of the resume segment before truncating it (corruption, or discarded work).
    preserve_resume: bool,
    last_rev: Rev,
    requests: Vec<DedupRecord>,
    checkpoints: CheckpointLedger,
}

/// A project's journal between opening the project and starting the journal: the scan, the
/// recovery decision, and the identity of the document on disk.
#[derive(Debug)]
pub(crate) struct PendingJournal {
    dir: PathBuf,
    backups: PathBuf,
    doc_sha256: String,
    scan: DirScan,
    plan: Option<Plan>,
}

impl PendingJournal {
    /// Scans `dir` for the chain continuing the document with hash `doc_sha256`.
    pub(crate) fn scan(
        dir: &Path,
        backups: &Path,
        doc_sha256: String,
    ) -> io::Result<PendingJournal> {
        let scan = DirScan::read(dir, &doc_sha256)?;
        Ok(PendingJournal {
            dir: dir.to_path_buf(),
            backups: backups.to_path_buf(),
            doc_sha256,
            scan,
            plan: None,
        })
    }

    /// Unsaved work to offer, if any (and not yet decided).
    pub(crate) fn available(&self) -> Option<RecoveryAvailable> {
        if self.plan.is_some() {
            return None;
        }
        self.scan.available()
    }

    /// Replays the chain onto `base` (or, with `discard`, keeps only the saved document).
    pub(crate) fn recover(
        &mut self,
        base: &Project,
        options: &RecoverOptions,
        discard: bool,
    ) -> Recovered {
        let live = |r: &DedupRecord| r.is_live(options.now, options.dedup_window);
        let Some(replayed) = replay(&self.scan, base, !discard) else {
            // Nothing continues this document (a new project, or a stale journal).
            self.plan = Some(Plan {
                resume: None,
                preserve_resume: false,
                last_rev: Rev::ZERO,
                requests: Vec::new(),
                checkpoints: CheckpointLedger::new(),
            });
            return Recovered {
                project: base.clone(),
                rev: Rev::ZERO,
                commits: Vec::new(),
                checkpoints: CheckpointLedger::new(),
                requests: Vec::new(),
                next_tx: TxId::new(1),
                ids: None,
                seed: None,
                report: ReplayReport::default(),
            };
        };
        let scan::Replayed {
            project,
            rev,
            commits,
            checkpoints,
            mut requests,
            ids,
            seed,
            max_tx,
            report,
            resume,
        } = replayed;
        requests.retain(live);
        if let Some(stop) = &report.stopped {
            tracing::warn!(
                segment = %stop.segment.display(),
                offset = stop.offset,
                reason = %stop.reason,
                applied = report.applied,
                "journal replay stopped early; later records are ignored"
            );
        }
        let file_len = self.scan.segments[resume.segment].file_len;
        let damaged = report
            .stopped
            .as_ref()
            .is_some_and(|s| s.reason != StopReason::Truncated);
        let preserve = (discard && file_len > resume.offset) || damaged;
        self.plan = Some(Plan {
            resume: Some(resume),
            preserve_resume: preserve,
            last_rev: rev,
            requests: requests.clone(),
            checkpoints: checkpoints.clone(),
        });
        Recovered {
            project,
            rev,
            commits,
            checkpoints,
            requests,
            next_tx: TxId::new(max_tx.saturating_add(1)),
            ids,
            seed,
            report,
        }
    }

    /// Whether recovery or discard was decided.
    pub(crate) fn decided(&self) -> bool {
        self.plan.is_some()
    }

    /// Starts appending where the decided plan says.
    pub(crate) fn start(self, config: JournalConfig) -> Result<Journal, JournalError> {
        let Some(plan) = self.plan else {
            return Err(JournalError::RecoveryPending);
        };
        let now = config.now();
        let PendingJournal {
            dir,
            backups,
            doc_sha256,
            scan,
            ..
        } = self;
        fs::create_dir_all(&dir).map_err(|e| JournalError::io("create directory", &dir, e))?;
        let next_generation = scan.max_generation() + 1;
        let preserve = |path: &Path, kind: &str, keep_original: bool| {
            let name = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let target = backups.join(format!("journal-{kind}-{}-{name}", now.as_millis()));
            let result = fs::create_dir_all(&backups).and_then(|()| {
                if keep_original {
                    fs::copy(path, &target).map(|_| ())
                } else {
                    fs::rename(path, &target)
                }
            });
            match &result {
                Ok(()) => {
                    tracing::warn!(from = %path.display(), to = %target.display(), "journal segment preserved in backups");
                }
                Err(e) => {
                    tracing::error!(path = %path.display(), error = %e, "could not preserve a journal segment");
                }
            }
            result
        };
        for path in &scan.unreadable {
            let _ = preserve(path, "unreadable", false);
        }
        let start = match plan.resume {
            None => {
                // No segment continues the document on disk: whatever is there is stale.
                for segment in &scan.segments {
                    let _ = preserve(&segment.path, "stale", false);
                }
                Start::Fresh {
                    base_rev: plan.last_rev,
                    doc_sha256,
                    generation: next_generation,
                    requests: Vec::new(),
                    retained: Vec::new(),
                }
            }
            Some(ResumePoint {
                segment: index,
                offset,
                revs,
            }) => {
                let chain_start = scan.chain_start.unwrap_or(index);
                for later in &scan.segments[index + 1..] {
                    let _ = preserve(&later.path, "unreachable", false);
                }
                let segment = &scan.segments[index];
                if plan.preserve_resume && segment.file_len > offset {
                    preserve(&segment.path, "replaced", true).map_err(|e| {
                        JournalError::io("preserve journal segment", &segment.path, e)
                    })?;
                }
                let header = segment
                    .header
                    .clone()
                    .ok_or_else(|| JournalError::Failed("resume segment has no header".into()))?;
                Start::Resume {
                    path: segment.path.clone(),
                    header,
                    valid_len: offset,
                    revs,
                    last_rev: plan.last_rev,
                    requests: plan.requests,
                    chain: scan.segments[chain_start..index]
                        .iter()
                        .map(|s| s.path.clone())
                        .collect(),
                    retained: scan.segments[..chain_start]
                        .iter()
                        .map(|s| s.path.clone())
                        .collect(),
                    next_generation,
                    checkpoints: Box::new(plan.checkpoints),
                }
            }
        };
        Journal::start(&dir, start, config)
    }
}
