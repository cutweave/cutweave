// SPDX-License-Identifier: GPL-3.0-or-later
//! The `edit.*` engine commands (WP-1B.04, WP-1B.09; arch §5.1–§5.2, §14b, ADR-0017):
//! transactions, soft locks, undo/redo, history and checkpoints. They run on the document
//! actor with the [`DocState`] as their context and act for the authenticated caller
//! ([`DocState::caller`]); none can run inside a transaction.
//!
//! | Command | Does |
//! |---|---|
//! | `edit.begin_tx{kind?, label}` | opens a `batch` (default), `gesture` or `proposal` owned by the connection |
//! | `edit.commit_tx{tx?}` | commits it as **one** history entry |
//! | `edit.cancel_tx{tx?}` | cancels it, rolling back its live changes |
//! | `edit.locks{}` | lists open transactions and the items each soft-locks (read-only) |
//! | `edit.undo{actor?}` / `edit.redo{actor?}` | undo / redo the caller's (or `actor`'s, `*` = anyone's) latest entry, under the per-actor safety rule |
//! | `edit.history{limit?, cursor?, actor?}` | lists the log newest first, paginated, plus every actor's stacks (read-only) |
//! | `edit.checkpoint{name?, note?}` | captures the committed document (O(1)); journaled |
//! | `edit.restore{checkpoint, force?}` | restores a checkpoint as one undoable change |
//! | `edit.list_checkpoints{}` | lists checkpoints (read-only) |
//! | `edit.delete_checkpoint{checkpoint}` | deletes a checkpoint (the document is not touched) |
//!
//! Checkpoints are journaled (WP-2I.03, [`CheckpointRecord`](crate::CheckpointRecord)): they
//! survive an engine crash, saves, and closing and reopening the project.
//!
//! The rule for undoing one actor's change while others edit is documented on
//! [`DocState::undo`] and in the `history` module: the entry's paths must hold what it left
//! there (later changes elsewhere are kept), and the result must keep the document valid;
//! otherwise `Conflict{reason: history_target_modified}` names who changed what since.
//!
//! Transactions belong to the connection that opened them: only it may run commands in,
//! commit or cancel one (others get `Locked`), and closing it rolls the transaction back. While
//! a `gesture` or `proposal` is open, the items it touched are soft-locked for other actors:
//! their commands on them fail fast with `Conflict{reason: item_in_gesture, retry_after_ms}`
//! (see [`SOFT_LOCK_RETRY_MS`](crate::SOFT_LOCK_RETRY_MS)), commands on other items proceed.

use crate::{
    Actor, Annotations, CheckpointInfo, CmdError, CommandSpec, Described, DocState, EngineCommand,
    EntryKind, HistoryMoved, HistoryPage, HistoryScope, LockList, MAX_HISTORY_PAGE, McpExposure,
    Registrable, Registry, RegistryError, Restored, Rev, Scope, TxId, TxKind, command, wire,
};

/// Longest transaction label, in characters.
pub const MAX_TX_LABEL_CHARS: usize = 200;

/// Registers every `edit.*` command.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<BeginTx>()?
        .register::<CommitTx>()?
        .register::<CancelTx>()?
        .register::<ListLocks>()?
        .register::<Undo>()?
        .register::<Redo>()?
        .register::<GetHistory>()?
        .register::<Checkpoint>()?
        .register::<Restore>()?
        .register::<ListCheckpoints>()?
        .register::<DeleteCheckpoint>()?;
    Ok(())
}

/// The caller of the running engine command.
fn caller(state: &DocState) -> Result<crate::ActorCtx, CmdError> {
    state.caller().cloned().ok_or_else(|| {
        CmdError::internal("`edit.*` commands run through the document actor (DocState::submit)")
    })
}

/// `tx`, or the caller connection's open transaction.
fn own_tx(state: &DocState, ctx: &crate::ActorCtx, tx: Option<TxId>) -> Result<TxId, CmdError> {
    match tx {
        Some(tx) => Ok(tx),
        None => state
            .transaction_of(ctx.conn())
            .map(|t| t.id)
            .ok_or_else(|| CmdError::invalid("/tx", "this connection has no open transaction")),
    }
}

/// Opens a transaction owned by this connection (one at a time). Run commands in it by naming
/// it in the request envelope (`_meta.tx`). A `batch` stays invisible until committed; a
/// `gesture` (dragging, scrubbing) and a `proposal` (copilot turn) update the document live
/// and soft-lock the items they touch: other actors' commands on those items fail with
/// `Conflict{reason: item_in_gesture}` until it closes. Each commits as one undo step; a
/// gesture idle for 30 s is cancelled by the engine; closing the connection rolls it back.
#[command(
    name = "edit.begin_tx",
    summary = "Open a transaction (batch, gesture or proposal) that commits as one undo step",
    engine,
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false)
)]
pub struct BeginTx {
    /// `batch` (default), `gesture` or `proposal`.
    pub kind: Option<TxKind>,
    /// The undo entry's label, e.g. `Move clip`.
    pub label: String,
}

/// An opened transaction.
#[wire]
pub struct TxOpened {
    /// Its id (pass it as `_meta.tx`).
    pub tx: TxId,
    /// Its kind.
    pub kind: TxKind,
    /// Its label.
    pub label: String,
    /// The live revision it was opened at.
    pub rev: Rev,
}

impl EngineCommand for BeginTx {
    type Context = DocState;
    type Output = TxOpened;

    fn run(&self, state: &mut DocState) -> Result<TxOpened, CmdError> {
        let chars = self.label.chars().count();
        if chars == 0 || chars > MAX_TX_LABEL_CHARS {
            return Err(CmdError::invalid(
                "/label",
                format!("must be 1..={MAX_TX_LABEL_CHARS} characters"),
            ));
        }
        if self.label.chars().any(char::is_control) {
            return Err(CmdError::invalid(
                "/label",
                "cannot contain control characters",
            ));
        }
        let ctx = caller(state)?;
        let info = state.begin_tx(&ctx, self.kind.unwrap_or(TxKind::Batch), &self.label)?;
        Ok(TxOpened {
            tx: info.id,
            kind: info.kind,
            label: info.label,
            rev: info.opened_rev,
        })
    }
}

/// Commits this connection's open transaction: its changes become one committed transaction
/// and one undo entry. Fails with `Conflict` (and closes it) if another transaction committed
/// overlapping changes first.
#[command(
    name = "edit.commit_tx",
    summary = "Commit your open transaction as one undo step",
    engine,
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false)
)]
pub struct CommitTx {
    /// The transaction (default: this connection's open one).
    pub tx: Option<TxId>,
}

/// A committed transaction.
#[wire]
pub struct TxCommitted {
    /// Its id.
    pub tx: TxId,
    /// Its kind.
    pub kind: TxKind,
    /// Whether it changed the document (and made an undo entry).
    pub changed: bool,
    /// The live revision afterwards.
    pub rev: Rev,
    /// The committed revision afterwards.
    pub committed_rev: Rev,
}

impl EngineCommand for CommitTx {
    type Context = DocState;
    type Output = TxCommitted;

    fn run(&self, state: &mut DocState) -> Result<TxCommitted, CmdError> {
        let ctx = caller(state)?;
        let tx = own_tx(state, &ctx, self.tx)?;
        let closed = state.commit_tx(&ctx, tx)?;
        Ok(TxCommitted {
            tx,
            kind: closed.kind,
            changed: !closed.patch.is_empty(),
            rev: closed.rev,
            committed_rev: closed.committed_rev,
        })
    }
}

/// Cancels this connection's open transaction and rolls back its live changes (nothing is
/// committed, journaled or added to the history).
#[command(
    name = "edit.cancel_tx",
    summary = "Cancel your open transaction and roll back its changes",
    engine,
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false)
)]
pub struct CancelTx {
    /// The transaction (default: this connection's open one).
    pub tx: Option<TxId>,
}

/// A cancelled transaction.
#[wire]
pub struct TxCancelled {
    /// Its id.
    pub tx: TxId,
    /// Its kind.
    pub kind: TxKind,
    /// The live revision afterwards.
    pub rev: Rev,
}

impl EngineCommand for CancelTx {
    type Context = DocState;
    type Output = TxCancelled;

    fn run(&self, state: &mut DocState) -> Result<TxCancelled, CmdError> {
        let ctx = caller(state)?;
        let tx = own_tx(state, &ctx, self.tx)?;
        let closed = state.cancel_tx(&ctx, tx)?;
        Ok(TxCancelled {
            tx,
            kind: closed.kind,
            rev: closed.rev,
        })
    }
}

/// Lists the open transactions (oldest first) and the items each soft-locks: a `gesture` or
/// `proposal` locks every item it touched for other actors until it closes; a `batch` locks
/// nothing. Gestures carry their idle deadline (`expires_at`); `mine` marks your connection's.
#[wire]
pub struct ListLocks {}

impl Described for ListLocks {
    const SPEC: CommandSpec = CommandSpec {
        name: "edit.locks",
        summary: "List open transactions and the items they soft-lock (who is editing what)",
        kind: EntryKind::Engine,
        undoable: false,
        view: false,
        scope: Scope::Any,
        mcp: McpExposure::Catalog,
        annotations: Annotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        secret_fields: &[],
        rust_type: "ListLocks",
    };
}

impl Registrable for ListLocks {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<ListLocks>().map(drop)
    }
}

impl EngineCommand for ListLocks {
    type Context = DocState;
    type Output = LockList;

    fn run(&self, state: &mut DocState) -> Result<LockList, CmdError> {
        let ctx = caller(state)?;
        Ok(state.locks(Some(ctx.conn())))
    }
}

/// Undoes the latest change of an actor (default: yours) as a new transaction. History is per
/// actor: undoing the agent's change leaves your later edits alone. Refused with
/// `Conflict{reason: history_target_modified}` when someone changed the same things since (the
/// detail names them) or the result would be invalid; nothing changes then.
#[command(
    name = "edit.undo",
    summary = "Undo your last change, or another actor's (`actor`; `*` = anyone's latest)",
    engine,
    undoable = false,
    mcp = "core",
    annotations(destructive = false)
)]
pub struct Undo {
    /// Whose change: `user`, `agent:<label>`, `plugin:<id>`, `system`, or `*` for the most
    /// recent change by anyone. Default: the caller's own.
    pub actor: Option<HistoryScope>,
}

impl EngineCommand for Undo {
    type Context = DocState;
    type Output = HistoryMoved;

    fn run(&self, state: &mut DocState) -> Result<HistoryMoved, CmdError> {
        let ctx = caller(state)?;
        let scope = self
            .actor
            .clone()
            .unwrap_or_else(|| HistoryScope::Actor(ctx.actor().clone()));
        state.undo(&ctx, &scope)
    }
}

/// Redoes the most recently undone change of an actor (default: yours). A new change by that
/// actor clears its redo stack. Refused with `Conflict` when the same things changed since the
/// undo.
#[command(
    name = "edit.redo",
    summary = "Redo the last undone change of yours, or of another actor (`actor`; `*` = anyone)",
    engine,
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false)
)]
pub struct Redo {
    /// Whose change (see `edit.undo`). Default: the caller's own.
    pub actor: Option<HistoryScope>,
}

impl EngineCommand for Redo {
    type Context = DocState;
    type Output = HistoryMoved;

    fn run(&self, state: &mut DocState) -> Result<HistoryMoved, CmdError> {
        let ctx = caller(state)?;
        let scope = self
            .actor
            .clone()
            .unwrap_or_else(|| HistoryScope::Actor(ctx.actor().clone()));
        state.redo(&ctx, &scope)
    }
}

/// Lists committed transactions newest first (edits, undos, redos, restores; each batch,
/// gesture or proposal is one entry), a page at a time, plus every actor's undo/redo stacks
/// (what Undo/Redo buttons show) and the history's memory use.
#[wire]
pub struct GetHistory {
    /// Most entries (default 50, at most 500; 0 returns only the stacks).
    #[serde(default)]
    pub limit: Option<u32>,
    /// The previous page's `next_cursor`.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Only this actor's entries.
    #[serde(default)]
    pub actor: Option<Actor>,
}

impl Described for GetHistory {
    const SPEC: CommandSpec = CommandSpec {
        name: "edit.history",
        summary: "List the undo history (newest first, paginated) and each actor's undo/redo stacks",
        kind: EntryKind::Engine,
        undoable: false,
        view: false,
        scope: Scope::Any,
        mcp: McpExposure::Catalog,
        annotations: Annotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        secret_fields: &[],
        rust_type: "GetHistory",
    };
}

impl Registrable for GetHistory {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<GetHistory>().map(drop)
    }
}

impl EngineCommand for GetHistory {
    type Context = DocState;
    type Output = HistoryPage;

    fn run(&self, state: &mut DocState) -> Result<HistoryPage, CmdError> {
        let limit = self.limit.map_or(crate::DEFAULT_HISTORY_PAGE, |l| {
            usize::try_from(l).unwrap_or(usize::MAX)
        });
        if limit > MAX_HISTORY_PAGE {
            return Err(CmdError::invalid(
                "/limit",
                format!("at most {MAX_HISTORY_PAGE}"),
            ));
        }
        state.history_page(limit, self.cursor.as_deref(), self.actor.as_ref())
    }
}

/// Captures the committed document as a restore point (cheap: snapshots share structure).
/// Open transactions are not included. At most 50 are kept, 20 of them named (least recently
/// used first out); reusing a name moves it to the new checkpoint. Checkpoints are saved in the
/// project's journal: they survive a crash, saves and reopening the project.
#[command(
    name = "edit.checkpoint",
    summary = "Save a restore point of the committed document (cheap; optional name)",
    engine,
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false)
)]
pub struct Checkpoint {
    /// A name to restore it by (1..=64 characters, not `cp_<n>`).
    pub name: Option<String>,
    /// A note (at most 1000 characters).
    pub note: Option<String>,
}

impl EngineCommand for Checkpoint {
    type Context = DocState;
    type Output = CheckpointInfo;

    fn run(&self, state: &mut DocState) -> Result<CheckpointInfo, CmdError> {
        let ctx = caller(state)?;
        state.checkpoint(&ctx, self.name.clone(), self.note.clone())
    }
}

/// Brings the document back to a checkpoint as one new, undoable change by you (history is
/// not truncated; `edit.undo` reverts the restore). Without `force`, refused with `Conflict`
/// when it would also revert other actors' changes made since the checkpoint, or changes the
/// history no longer holds (after a restart: everything before the last save). Other
/// connections' open transactions are kept; one whose changes the restore would overwrite
/// refuses it (`Conflict`).
#[command(
    name = "edit.restore",
    summary = "Restore a checkpoint as one undoable change",
    engine,
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct Restore {
    /// The checkpoint: its id (`cp_<n>`) or name.
    pub checkpoint: String,
    /// Also revert other actors' changes made since the checkpoint.
    #[serde(default)]
    pub force: bool,
}

impl EngineCommand for Restore {
    type Context = DocState;
    type Output = Restored;

    fn run(&self, state: &mut DocState) -> Result<Restored, CmdError> {
        let ctx = caller(state)?;
        state.restore(&ctx, &self.checkpoint, self.force)
    }
}

/// Every checkpoint, oldest first.
#[wire]
pub struct ListCheckpoints {}

/// The checkpoints.
#[wire]
pub struct CheckpointList {
    /// Oldest first.
    pub checkpoints: Vec<CheckpointInfo>,
}

impl Described for ListCheckpoints {
    const SPEC: CommandSpec = CommandSpec {
        name: "edit.list_checkpoints",
        summary: "List the checkpoints",
        kind: EntryKind::Engine,
        undoable: false,
        view: false,
        scope: Scope::Any,
        mcp: McpExposure::Catalog,
        annotations: Annotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        secret_fields: &[],
        rust_type: "ListCheckpoints",
    };
}

impl Registrable for ListCheckpoints {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<ListCheckpoints>().map(drop)
    }
}

impl EngineCommand for ListCheckpoints {
    type Context = DocState;
    type Output = CheckpointList;

    fn run(&self, state: &mut DocState) -> Result<CheckpointList, CmdError> {
        Ok(CheckpointList {
            checkpoints: state.checkpoints(),
        })
    }
}

/// Deletes a checkpoint. The document and the history are not touched, and it cannot be
/// undone. Returns the deleted checkpoint.
#[command(
    name = "edit.delete_checkpoint",
    summary = "Delete a checkpoint (the document is not changed)",
    engine,
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct DeleteCheckpoint {
    /// The checkpoint: its id (`cp_<n>`) or name.
    pub checkpoint: String,
}

impl EngineCommand for DeleteCheckpoint {
    type Context = DocState;
    type Output = CheckpointInfo;

    fn run(&self, state: &mut DocState) -> Result<CheckpointInfo, CmdError> {
        state.delete_checkpoint(&self.checkpoint)
    }
}
