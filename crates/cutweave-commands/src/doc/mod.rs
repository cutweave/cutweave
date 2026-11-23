// SPDX-License-Identifier: GPL-3.0-or-later
//! The single-writer document actor (WP-1B.03; arch §5.1–§5.2, ADR-0005/0015/0016/0017).
//!
//! - [`DocState`]: the document state machine. It runs every request serially: dedup by
//!   `request_id` ([`DEDUP_WINDOW`], persisted through [`CommitSink`]), the `expected_rev`
//!   check, the command in an implicit or open transaction (batch, gesture, proposal), journal
//!   before visibility, revisions, events. wasm32-clean.
//! - [`DocActor`] / [`DocHandle`] (native): the state on a dedicated thread; lock-free
//!   snapshot reads ([`DocSnapshot`], live and committed), replies as [`Pending`] futures,
//!   events as bounded per-subscriber [`EventStream`]s with lag detection and resync.
//! - Identity: [`ActorCtx`] (derived from the credential, never client-asserted) carries the
//!   [`Actor`], the [`ConnId`] that owns open transactions, and the [`Access`](crate::Access).
//! - History (WP-1B.04): the linear log of committed transactions with per-actor undo/redo
//!   stacks ([`HistoryEntry`], [`HistoryScope`], [`HistoryLimits`]), the per-actor undo safety
//!   rule, checkpoints ([`CheckpointId`]) and `edit.history_changed` events
//!   ([`HistoryChanged`]); the `edit.*` commands that drive it are in [`crate::edit`].
//! - Ownership and soft locks (WP-1B.09): an open transaction belongs to the [`ConnId`] that
//!   opened it, and [`DocState::close_connection`] rolls it back; an open gesture or proposal
//!   soft-locks the items it touched for other actors, whose requests on them fail fast with
//!   `Conflict{reason: item_in_gesture, retry_after_ms}` ([`SOFT_LOCK_RETRY_MS`]); locks are
//!   listed by `edit.locks` ([`LockList`]) and announced by `tx.opened` / `tx.locked` /
//!   `tx.closed` events ([`TxActivity`], [`TxEnded`]).

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod actor;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod bus;
mod clock;
mod dedup;
mod event;
mod history;
mod identity;
mod locks;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod metrics;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod pending;
mod request;
mod state;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod sync;

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use actor::{ActorOptions, DEFAULT_EVENT_CAPACITY, DocActor, DocHandle};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use bus::{EventStream, RecvError, TryRecvError};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use clock::SystemClock;
pub use clock::{Clock, ManualClock, WallTime};
pub use dedup::{DEDUP_CAPACITY, DEDUP_WINDOW, DedupRecord, Fingerprint};
pub use event::{
    CommandPanicked, DocEvent, DocPatched, DocReset, DocSnapshot, PatchCause, TxActivity, TxEnded,
};
pub use history::{
    ActorStacks, CheckpointId, CheckpointInfo, DEFAULT_HISTORY_BYTES, DEFAULT_HISTORY_ENTRIES,
    DEFAULT_HISTORY_PAGE, DEFAULT_UNDO_DEPTH, EntryCause, EntryStatus, HistoryChange,
    HistoryChanged, HistoryEntry, HistoryLimits, HistoryMoved, HistoryPage, HistoryScope,
    HistoryUsage, MAX_CHECKPOINT_NAME_CHARS, MAX_CHECKPOINT_NOTE_CHARS, MAX_CHECKPOINTS,
    MAX_HISTORY_PAGE, MAX_NAMED_CHECKPOINTS, Restored, RevertedChanges, StackTop,
};
pub use identity::{
    Actor, ActorCtx, ActorError, ConnId, MAX_AGENT_LABEL_CHARS, MAX_PLUGIN_ID_CHARS,
};
pub use locks::{LockList, TxLocks};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use metrics::{DocMetrics, DocStats, NoMetrics, Outcome, RequestSample};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use pending::Pending;
pub use request::{
    Applied, RequestId, RequestIdError, Submit, TxClosed, TxId, TxIdError, TxInfo, TxKind,
    TxOutcome,
};
pub use state::{
    Changes, CommitRecord, CommitSink, DocConfig, DocState, GESTURE_IDLE_TIMEOUT, JournalStep,
    JournaledCommit, NoJournal, SOFT_LOCK_RETRY_MS,
};
