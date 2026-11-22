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
//!   detection, and the seams for transactions, undo and the journal (WP-1B.04/09, WP-1C.02).
//!
//! The crate compiles to `wasm32-unknown-unknown`; there the actor's thread host
//! ([`DocActor`], [`DocHandle`], [`EventStream`], [`Pending`]) is left out and [`DocState`] is
//! driven directly.

// Lets the attribute macros emit `::cutweave_commands::…` paths that also resolve in this crate.
extern crate self as cutweave_commands;

mod apply;
mod ctx;
mod diff;
mod doc;
mod error;
mod json;
mod meta;
mod patch;
mod path;
mod registry;
mod tx;

pub use ctx::{Access, QueryCtx};
pub use cutweave_commands_derive::{command, query, wire};
pub use diff::diff;
pub use doc::{
    Actor, ActorCtx, ActorError, Applied, Changes, Clock, CommandPanicked, CommitRecord,
    CommitSink, ConnId, DEDUP_CAPACITY, DEDUP_WINDOW, DedupRecord, DocConfig, DocEvent, DocPatched,
    DocReset, DocSnapshot, DocState, Fingerprint, JournalStep, MAX_AGENT_LABEL_CHARS,
    MAX_PLUGIN_ID_CHARS, ManualClock, NoJournal, PatchCause, RequestId, RequestIdError,
    SOFT_LOCK_RETRY_MS, Submit, TxClosed, TxId, TxIdError, TxInfo, TxKind, TxOutcome, WallTime,
};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use doc::{
    ActorOptions, DEFAULT_EVENT_CAPACITY, DocActor, DocHandle, DocMetrics, DocStats, EventStream,
    NoMetrics, Outcome, Pending, RecvError, RequestSample, SystemClock, TryRecvError,
};
pub use error::{CmdError, ConflictReason, ResourceKind, Rev};
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
