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

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod actor;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod bus;
mod clock;
mod dedup;
mod event;
mod identity;
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
pub use event::{CommandPanicked, DocEvent, DocPatched, DocReset, DocSnapshot, PatchCause};
pub use identity::{
    Actor, ActorCtx, ActorError, ConnId, MAX_AGENT_LABEL_CHARS, MAX_PLUGIN_ID_CHARS,
};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use metrics::{DocMetrics, DocStats, NoMetrics, Outcome, RequestSample};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use pending::Pending;
pub use request::{
    Applied, RequestId, RequestIdError, Submit, TxClosed, TxId, TxIdError, TxInfo, TxKind,
    TxOutcome,
};
pub use state::{
    Changes, CommitRecord, CommitSink, DocConfig, DocState, JournalStep, NoJournal,
    SOFT_LOCK_RETRY_MS,
};
