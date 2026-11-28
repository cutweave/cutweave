// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-commands`: the command bus core (arch §5, ADR-0005/0015/0016/0017).
//!
//! Every mutation of the project document is a typed, registered command applied through a
//! [`Tx`] on one bus, exposed identically to the UI (JSON-RPC), MCP and the CLI.
//!
//! - **Macros** ([`command`], [`query`], [`wire`]): declare an entry's contract on its params
//!   struct, checked at compile time (WP-1B.02).
//! - **Traits** ([`Command`], [`Query`], [`EngineCommand`], [`Described`]): a command mutates
//!   the document through a [`Tx`]; a query reads a snapshot through a [`QueryCtx`]; an engine
//!   command acts on engine state (history, transactions, view state).
//! - **Registry** ([`Registry`], [`Entry`], [`CommandMeta`], [`CommandSpec`]): explicit
//!   registration with contract checks (name grammar and namespaces, summaries, flags, object
//!   schemas, secret fields), JSON Schemas (schemars, draft 2020-12, self-contained), TypeScript
//!   hooks (specta), MCP annotations, `#[secret]` redaction and UI-only scopes checked on every
//!   call ([`Access`]).
//! - **Transactions** ([`Transaction`], [`Tx`], [`Committed`]): every change is recorded as an
//!   invertible [`PatchOp`]; failed commands roll back; commits yield a coalesced field-level
//!   [`Patch`] and its inverse.
//! - **Patches** ([`PatchOp`], [`Patch`], [`DocPath`], [`apply_patch`], [`apply_patch_json`],
//!   [`invert`], [`diff`]): JSON-Pointer-addressed, ID-keyed ops with preimages, applied to the
//!   typed `imbl` document or to its JSON form with identical results.
//! - **Errors** ([`CmdError`]): typed, with a machine code and a human message.
//! - **Document actor** ([`DocState`], [`DocActor`], [`DocHandle`], [`DocSnapshot`],
//!   [`DocEvent`], [`Applied`], [`ActorCtx`], [`Clock`]): the single writer (WP-1B.03). Commands
//!   run serially on a dedicated thread; readers get immutable snapshots without locking;
//!   revisions, `expected_rev` conflicts, `request_id` dedup, bounded event streams with lag
//!   detection, and the journal seam (WP-1C.02).
//! - **Ownership and soft locks** ([`ConnId`], [`LockList`], [`TxActivity`], [`TxEnded`]): a
//!   transaction belongs to the connection that opened it and rolls back when it closes; an
//!   open gesture or proposal soft-locks the items it touched for other actors (WP-1B.09).
//! - **History** ([`edit`], [`HistoryEntry`], [`HistoryScope`], [`CheckpointId`]): per-actor
//!   undo/redo over one linear log (WP-1B.04). The `edit.*` engine commands open, commit and
//!   cancel transactions, undo and redo with the per-actor safety rule, list the history and
//!   make and restore checkpoints; register them with [`edit::register`].
//!
//! The crate compiles to `wasm32-unknown-unknown`; there the actor's thread host
//! ([`DocActor`], [`DocHandle`], [`EventStream`], [`Pending`]) is left out and [`DocState`] is
//! driven directly.

// Lets the attribute macros emit `::cutweave_commands::…` paths that also resolve in this crate.
extern crate self as cutweave_commands;

mod apply;
mod catalog;
mod ctx;
mod diff;
mod doc;
pub mod edit;
#[doc(hidden)]
pub mod effort;
mod error;
mod json;
mod meta;
mod patch;
mod path;
mod registry;
mod schema_hash;
mod tx;

pub use catalog::{
    CATALOG_FORMAT, JSON_SCHEMA_DIALECT, commands_catalog, mcp_tools_catalog, queries_catalog,
};
pub use ctx::{Access, QueryCtx};
pub use cutweave_commands_derive::{command, query, wire};
pub use diff::diff;
pub use doc::{
    Actor, ActorCtx, ActorError, ActorStacks, Applied, CHANGED_SINCE_ITEMS, CHANGED_SINCE_REVS,
    Changes, CheckpointId, CheckpointInfo, CheckpointLedger, CheckpointRecord, Clock, ClosedTx,
    CommandPanicked, CommitRecord, CommitSink, ConnId, DEDUP_CAPACITY, DEDUP_WINDOW,
    DEFAULT_HISTORY_BYTES, DEFAULT_HISTORY_ENTRIES, DEFAULT_HISTORY_PAGE, DEFAULT_UNDO_DEPTH,
    DedupRecord, DocConfig, DocEvent, DocPatched, DocReset, DocSnapshot, DocState, EVENT_CATALOG,
    EntryCause, EntryStatus, EventInfo, Fingerprint, GESTURE_IDLE_TIMEOUT, GestureCancelReason,
    GestureCancelled, HistoryChange, HistoryChanged, HistoryEntry, HistoryLimits, HistoryMoved,
    HistoryPage, HistoryScope, HistoryUsage, JournalStep, JournaledCheckpoint, JournaledCommit,
    JournaledTxMarker, LatestStacks, LatestTop, LedgerEntry, LockList, MAX_AGENT_LABEL_CHARS,
    MAX_CHECKPOINT_NAME_CHARS, MAX_CHECKPOINT_NOTE_CHARS, MAX_CHECKPOINTS, MAX_HISTORY_PAGE,
    MAX_NAMED_CHECKPOINTS, MAX_PLUGIN_ID_CHARS, MAX_UNDO_DEPTH, ManualClock, NoJournal, PatchCause,
    RequestId, RequestIdError, Restored, RevertedChanges, SOFT_LOCK_RETRY_MS, StackTop, Submit,
    TX_IDS_RESERVED, TxActivity, TxClosed, TxEnded, TxId, TxIdError, TxInfo, TxKind, TxLocks,
    TxMarker, TxOutcome, WallTime,
};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use doc::{
    ActorOptions, DEFAULT_EVENT_CAPACITY, DocActor, DocHandle, DocMetrics, DocStats, EventStream,
    NoMetrics, Outcome, Pending, RecvError, RequestSample, SystemClock, TryRecvError,
};
pub use error::{ChangedSince, CmdError, ConflictReason, ResourceKind, Rev, StaleRev};
pub use json::json_eq;
pub use meta::{
    Annotations, CommandMeta, CommandSpec, EntryKind, MAX_SUMMARY_CHARS, McpExposure, NAMESPACES,
    Scope, check_name, check_summary,
};
pub use patch::{Patch, PatchError, PatchOp, apply_patch, apply_patch_json, invert};
pub use path::{DOC_PATH_PATTERN, DocPath, DocPathError, EntityRef};
pub use registry::{
    Command, Described, EngineCommand, Entry, Query, REDACTED, Registrable, Registry,
    RegistryError, input_schema, output_schema,
};
pub use schema_hash::SCHEMA_HASH;
pub use tx::{Committed, Mark, Transaction, Tx};

/// Re-exports for macro-generated code. Not part of the API.
#[doc(hidden)]
pub mod __private {
    pub use schemars;
    pub use serde;
    pub use serde_json;
    pub use specta;
    pub use specta_typescript;
}
