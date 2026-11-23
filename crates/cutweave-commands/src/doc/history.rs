// SPDX-License-Identifier: GPL-3.0-or-later
//! Undo history (WP-1B.04; arch §5.2, §14b, ADR-0005, ADR-0017): one linear log of committed
//! transactions plus per-actor undo and redo stacks.
//!
//! # Model
//!
//! - The **log** records every committed transaction in commit order: its net patch (with
//!   preimages), actor, label and revision range. Undo, redo and checkpoint restores are
//!   committed transactions too, so the log is the document's linear history (the journal's
//!   order, WP-1C.02). It backs `edit.history` and conflict reports.
//! - Each **actor** has an undo stack (its applied, undoable changes, newest on top) and a redo
//!   stack (its changes that were undone, most recently undone on top). The stacks belong to
//!   the actor who *made* the change, whoever undoes it: "undo the agent's last change" moves
//!   the agent's entry to the agent's redo stack and leaves the user's stacks alone.
//! - A gesture, proposal or batch transaction is **one** entry, however many commands it ran.
//! - A new change by an actor (anything but undo/redo) **clears that actor's redo stack**.
//!   Other actors' redo stacks are kept; the safety rule below guards their redo.
//! - A transaction is undoable when every command in it that changed the document is
//!   `undoable` (registry metadata); otherwise it is logged but never on a stack.
//!
//! # The per-actor undo rule
//!
//! Undoing entry `E` applies `invert(E.patch)` to the **current committed document**, as a new
//! transaction tagged with the caller. It succeeds only if both checks pass:
//!
//! 1. **Unchanged since:** every inverse op's preimage still matches, i.e. no later
//!    transaction (by anyone) left a different value at any path `E` wrote. Later changes to
//!    other paths are kept: the undo is rebased past them, which is trivially safe because
//!    patches address entities by ID, never by index.
//! 2. **Still valid:** unless nothing was committed since `E` (then the result is exactly the
//!    document before `E`), the result must not break a document invariant that held before
//!    (`Project::violations`): e.g. re-adding a clip into a gap another actor has filled since,
//!    or removing media that a later clip uses.
//!
//! Otherwise it fails with `Conflict{reason: history_target_modified}` naming the paths and the
//! later transactions and actors that changed them, and nothing changes. Redo is the mirror
//! image (re-apply `E.patch`, checked against changes made since `E` was undone). Open
//! transactions are respected as for any implicit transaction: an undo touching an item in
//! another connection's open gesture fails with `Conflict{reason: item_in_gesture}`.
//!
//! # Limits and memory
//!
//! [`HistoryLimits`] bounds the log (entries and estimated bytes) and each actor's undo depth.
//! When the log is over budget, entries no stack refers to (undo/redo records, non-undoable
//! changes, entries beyond an actor's depth or dropped from a redo stack) are evicted first,
//! oldest first, so a busy agent cannot push the user's undo history out; only then the oldest
//! entry overall. Patches are shared (`Arc`), and redo records reuse the redone entry's patch.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    str::FromStr,
    sync::Arc,
};

use cutweave_model::Project;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;

use super::{
    clock::WallTime,
    identity::Actor,
    request::{TxId, TxKind},
};
use crate::{
    error::Rev,
    patch::{Patch, PatchOp},
    path::DocPath,
};

/// Default bound on logged transactions.
pub const DEFAULT_HISTORY_ENTRIES: usize = 10_000;
/// Default bound on the log's estimated memory: 64 MiB.
pub const DEFAULT_HISTORY_BYTES: usize = 64 << 20;
/// Default undo depth per actor.
pub const DEFAULT_UNDO_DEPTH: usize = 1_000;
/// Default page size of `edit.history`.
pub const DEFAULT_HISTORY_PAGE: usize = 50;
/// Largest page of `edit.history`.
pub const MAX_HISTORY_PAGE: usize = 500;

/// Bounds on the undo history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryLimits {
    /// Most transactions kept in the log (at least 1).
    pub max_entries: usize,
    /// Most estimated bytes of patches and metadata kept in the log. The newest entry is kept
    /// even if it alone exceeds this.
    pub max_bytes: usize,
    /// Deepest undo stack per actor (at least 1); older entries can no longer be undone.
    pub max_undo_depth: usize,
}

impl Default for HistoryLimits {
    fn default() -> HistoryLimits {
        HistoryLimits {
            max_entries: DEFAULT_HISTORY_ENTRIES,
            max_bytes: DEFAULT_HISTORY_BYTES,
            max_undo_depth: DEFAULT_UNDO_DEPTH,
        }
    }
}

/// Whose history `edit.undo` / `edit.redo` act on.
///
/// On the wire: an actor string (`user`, `agent:<label>`, `plugin:<id>`, `system`), or `*` for
/// the most recent change by anyone (undo) / the most recently undone change (redo).
#[derive(Clone, Debug, PartialEq, Eq, Hash, specta::Type)]
#[specta(type = String)]
pub enum HistoryScope {
    /// One actor's stacks.
    Actor(Actor),
    /// Every actor's: the newest applicable entry.
    Any,
}

impl fmt::Display for HistoryScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HistoryScope::Actor(actor) => actor.fmt(f),
            HistoryScope::Any => f.write_str("*"),
        }
    }
}

impl FromStr for HistoryScope {
    type Err = super::identity::ActorError;

    fn from_str(s: &str) -> Result<HistoryScope, Self::Err> {
        if s == "*" {
            Ok(HistoryScope::Any)
        } else {
            s.parse().map(HistoryScope::Actor)
        }
    }
}

impl Serialize for HistoryScope {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for HistoryScope {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<HistoryScope, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        s.parse().map_err(de::Error::custom)
    }
}

impl JsonSchema for HistoryScope {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "HistoryScope".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "Whose history: `user`, `agent:<label>`, `plugin:<id>`, `system`, or `*` for anyone's most recent change.",
            "pattern": "^(\\*|user|system|agent:.+|plugin:[A-Za-z0-9._@-]+)$"
        })
    }
}

/// A checkpoint (`cp_<n>` on the wire), see `edit.checkpoint`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, specta::Type)]
#[specta(type = String)]
pub struct CheckpointId(u64);

impl CheckpointId {
    /// The checkpoint with this number.
    #[must_use]
    pub const fn new(n: u64) -> CheckpointId {
        CheckpointId(n)
    }

    /// The number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Parses `cp_<n>`.
    #[must_use]
    pub fn parse(s: &str) -> Option<CheckpointId> {
        s.strip_prefix("cp_")
            .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse().ok())
            .map(CheckpointId)
    }
}

impl fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cp_{}", self.0)
    }
}

impl fmt::Debug for CheckpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Serialize for CheckpointId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CheckpointId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<CheckpointId, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        CheckpointId::parse(&s).ok_or_else(|| {
            de::Error::custom(format!("invalid checkpoint id `{s}`: expected `cp_<n>`"))
        })
    }
}

impl JsonSchema for CheckpointId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CheckpointId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^cp_[0-9]+$",
            "description": "A checkpoint id."
        })
    }
}

/// Why a transaction is in the history.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    specta::Type,
)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntryCause {
    /// A change: an implicit command, or a committed batch, gesture or proposal.
    #[default]
    Edit,
    /// Undid the entry of transaction `of`.
    Undo {
        /// The undone transaction.
        of: TxId,
    },
    /// Redid the entry of transaction `of`.
    Redo {
        /// The redone transaction.
        of: TxId,
    },
    /// Restored a checkpoint (an undoable change by the caller).
    Restore {
        /// The checkpoint.
        checkpoint: CheckpointId,
    },
}

impl EntryCause {
    /// Whether it is a change of its own (an edit or a restore), as opposed to an undo or redo
    /// of another entry.
    #[must_use]
    pub const fn is_change(self) -> bool {
        matches!(self, EntryCause::Edit | EntryCause::Restore { .. })
    }
}

/// Whether an entry's changes are in the document.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    /// Applied (undo and redo records always are).
    Applied,
    /// Undone.
    Undone,
}

/// One logged transaction, as `edit.history` lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct HistoryEntry {
    /// The transaction.
    pub tx_id: TxId,
    /// Who committed it (for an undo or redo: who undid or redid).
    pub actor: Actor,
    /// The transaction's label, or the command name for a single command.
    pub label: String,
    /// The open transaction's kind; `None` for a single command, an undo, redo or restore.
    pub kind: Option<TxKind>,
    /// Edit, undo, redo or restore.
    pub cause: EntryCause,
    /// The committed revision it was built on.
    pub prev_rev: Rev,
    /// The committed revision it created.
    pub rev: Rev,
    /// When it committed (wall clock).
    pub at: WallTime,
    /// Whether its changes are currently applied.
    pub status: EntryStatus,
    /// It is on its actor's undo stack (`edit.undo{actor}` reaches it).
    pub can_undo: bool,
    /// It is on its actor's redo stack.
    pub can_redo: bool,
    /// Number of patch ops.
    pub ops: u32,
}

/// The next entry of a stack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct StackTop {
    /// Its transaction.
    pub tx_id: TxId,
    /// Its label ("Undo <label>").
    pub label: String,
}

/// One actor's undo and redo stacks (what an Undo / Redo button needs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct ActorStacks {
    /// The actor.
    pub actor: Actor,
    /// Entries `edit.undo` can revert, newest first.
    pub undo_depth: u32,
    /// Entries `edit.redo` can re-apply.
    pub redo_depth: u32,
    /// What `edit.undo{actor}` would revert next.
    pub next_undo: Option<StackTop>,
    /// What `edit.redo{actor}` would re-apply next.
    pub next_redo: Option<StackTop>,
}

/// The log's size against its limits.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, specta::Type,
)]
pub struct HistoryUsage {
    /// Logged transactions.
    pub entries: u32,
    /// Their estimated memory, in bytes.
    #[specta(type = specta_typescript::Number)]
    pub bytes: u64,
    /// The entry limit.
    pub max_entries: u32,
    /// The byte limit.
    #[specta(type = specta_typescript::Number)]
    pub max_bytes: u64,
    /// Entries evicted so far.
    #[specta(type = specta_typescript::Number)]
    pub evicted: u64,
}

/// The result of `edit.undo` / `edit.redo`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct HistoryMoved {
    /// The new transaction that undid or redid it.
    pub tx: TxId,
    /// The entry undone or redone (its original transaction).
    pub entry: TxId,
    /// Whose entry it is.
    pub actor: Actor,
    /// Its label.
    pub label: String,
    /// The live revision afterwards.
    pub rev: Rev,
    /// The committed revision afterwards.
    pub committed_rev: Rev,
}

/// A page of `edit.history`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct HistoryPage {
    /// Logged transactions, newest first.
    pub entries: Vec<HistoryEntry>,
    /// Pass as `cursor` for the next (older) page; absent on the last page.
    pub next_cursor: Option<String>,
    /// Every actor with something to undo or redo.
    pub stacks: Vec<ActorStacks>,
    /// The live revision.
    pub rev: Rev,
    /// The committed revision.
    pub committed_rev: Rev,
    /// The log's size against its limits.
    pub usage: HistoryUsage,
}

/// Changes a checkpoint restore reverted, by actor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct RevertedChanges {
    /// The actor.
    pub actor: Actor,
    /// Its transactions (since the checkpoint) whose changes were reverted, oldest first.
    pub txs: Vec<TxId>,
}

/// The result of `edit.restore`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct Restored {
    /// The checkpoint.
    pub checkpoint: CheckpointId,
    /// The restoring transaction (undo it with `edit.undo`); `None` if the document already
    /// equalled the checkpoint.
    pub tx: Option<TxId>,
    /// The live revision afterwards.
    pub rev: Rev,
    /// The committed revision afterwards.
    pub committed_rev: Rev,
    /// Whose changes since the checkpoint it reverted.
    pub reverted: Vec<RevertedChanges>,
    /// It also reverted changes that are no longer in the history (authors unknown).
    pub reverted_unknown: bool,
}

/// What happened to an actor's stacks.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    specta::Type,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HistoryChange {
    /// It committed a change (pushed onto its undo stack if undoable; its redo stack cleared).
    Recorded,
    /// One of its entries was undone (moved to its redo stack).
    Undone,
    /// One of its entries was redone (moved back to its undo stack).
    Redone,
    /// Its stacks lost entries to the history limits.
    Trimmed,
}

/// `edit.history_changed`: an actor's undo/redo stacks changed (Undo/Redo buttons, "undo agent
/// turn"). Not tied to a revision: after `Lagged`, re-read `edit.history`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct HistoryChanged {
    /// What happened.
    pub change: HistoryChange,
    /// The entry concerned (`None` for trims).
    pub tx_id: Option<TxId>,
    /// The committed revision afterwards.
    pub committed_rev: Rev,
    /// The actor's stacks afterwards.
    pub stacks: ActorStacks,
}

/// Where a record sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// On no stack (undo/redo records, non-undoable changes, expired entries).
    None,
    Undo,
    Redo,
}

/// One logged transaction.
#[derive(Clone, Debug)]
pub(crate) struct Record {
    pub(crate) seq: u64,
    pub(crate) tx_id: TxId,
    pub(crate) actor: Actor,
    pub(crate) label: String,
    pub(crate) kind: Option<TxKind>,
    pub(crate) cause: EntryCause,
    pub(crate) prev_rev: Rev,
    pub(crate) rev: Rev,
    pub(crate) at: WallTime,
    /// The net committed patch (for an undo record: the inverse that was applied).
    pub(crate) patch: Arc<Patch>,
    pub(crate) status: EntryStatus,
    /// The committed revision when its changes were last applied (its commit or last redo):
    /// undoing it with nothing committed since restores a known-valid document exactly.
    pub(crate) applied_rev: Rev,
    /// The record that last applied its changes (itself, or its last redo): later records
    /// may have changed what it wrote.
    pub(crate) applied_seq: u64,
    /// The committed revision and record of its last undo.
    pub(crate) undone: Option<(Rev, u64)>,
    slot: Slot,
    bytes: usize,
}

impl Record {
    fn entry(&self) -> HistoryEntry {
        HistoryEntry {
            tx_id: self.tx_id,
            actor: self.actor.clone(),
            label: self.label.clone(),
            kind: self.kind,
            cause: self.cause,
            prev_rev: self.prev_rev,
            rev: self.rev,
            at: self.at,
            status: self.status,
            can_undo: self.slot == Slot::Undo,
            can_redo: self.slot == Slot::Redo,
            ops: u32::try_from(self.patch.len()).unwrap_or(u32::MAX),
        }
    }

    fn top(&self) -> StackTop {
        StackTop {
            tx_id: self.tx_id,
            label: self.label.clone(),
        }
    }
}

/// A committed transaction to log.
#[derive(Clone, Debug)]
pub(crate) struct NewRecord {
    pub(crate) tx_id: TxId,
    pub(crate) actor: Actor,
    pub(crate) label: String,
    pub(crate) kind: Option<TxKind>,
    pub(crate) cause: EntryCause,
    pub(crate) prev_rev: Rev,
    pub(crate) rev: Rev,
    pub(crate) at: WallTime,
    pub(crate) patch: Arc<Patch>,
    /// Pushed onto its actor's undo stack (changes only).
    pub(crate) undoable: bool,
}

#[derive(Clone, Debug, Default)]
struct Stacks {
    /// Seqs, oldest at the bottom.
    undo: Vec<u64>,
    /// Seqs, most recently undone on top.
    redo: Vec<u64>,
}

/// Estimated heap and inline size of a JSON value.
fn value_bytes(v: &Value) -> usize {
    std::mem::size_of::<Value>()
        + match v {
            Value::String(s) => s.len(),
            Value::Array(a) => a.iter().map(value_bytes).sum(),
            Value::Object(m) => m
                .iter()
                .map(|(k, v)| std::mem::size_of::<String>() + k.len() + value_bytes(v))
                .sum(),
            Value::Null | Value::Bool(_) | Value::Number(_) => 0,
        }
}

/// Estimated memory of a patch.
pub(crate) fn patch_bytes(patch: &Patch) -> usize {
    std::mem::size_of::<Patch>()
        + patch
            .iter()
            .map(|op| {
                let paths: usize = op.paths().map(|p| p.as_str().len()).sum();
                let values = match op {
                    PatchOp::Add { value, .. } => value_bytes(value),
                    PatchOp::Remove { old, .. } => value_bytes(old),
                    PatchOp::Replace { old, value, .. } => value_bytes(old) + value_bytes(value),
                    PatchOp::Move { .. } => 0,
                };
                std::mem::size_of::<PatchOp>() + paths + values
            })
            .sum::<usize>()
}

/// Whether two paths overlap (one is the other or an ancestor of it).
fn overlaps(a: &DocPath, b: &DocPath) -> bool {
    a.is_prefix_of(b) || b.is_prefix_of(a)
}

/// The undo log and stacks.
#[derive(Debug)]
pub(crate) struct History {
    limits: HistoryLimits,
    next_seq: u64,
    log: BTreeMap<u64, Record>,
    stacks: BTreeMap<Actor, Stacks>,
    /// Records on no stack: evicted first.
    dead: BTreeSet<u64>,
    bytes: usize,
    evicted: u64,
}

impl History {
    pub(crate) fn new(limits: HistoryLimits) -> History {
        History {
            limits: HistoryLimits {
                max_entries: limits.max_entries.max(1),
                max_bytes: limits.max_bytes,
                max_undo_depth: limits.max_undo_depth.max(1),
            },
            next_seq: 1,
            log: BTreeMap::new(),
            stacks: BTreeMap::new(),
            dead: BTreeSet::new(),
            bytes: 0,
            evicted: 0,
        }
    }

    /// The sequence number the next record gets.
    pub(crate) fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub(crate) fn get(&self, seq: u64) -> Option<&Record> {
        self.log.get(&seq)
    }

    /// The record of transaction `tx`, newest first (recovery).
    fn find_tx(&self, tx: TxId) -> Option<u64> {
        self.log
            .values()
            .rev()
            .find(|r| r.tx_id == tx)
            .map(|r| r.seq)
    }

    /// The entry `edit.undo{scope}` reverts. For `*`: the one applied most recently (committed,
    /// or redone since), so `edit.undo{*}` right after `edit.redo{*}` reverts that redo.
    pub(crate) fn next_undo(&self, scope: &HistoryScope) -> Option<u64> {
        match scope {
            HistoryScope::Actor(actor) => self.stacks.get(actor)?.undo.last().copied(),
            HistoryScope::Any => self
                .stacks
                .values()
                .filter_map(|s| s.undo.last().copied())
                .max_by_key(|seq| self.log.get(seq).map_or(*seq, |r| r.applied_seq)),
        }
    }

    /// The entry `edit.redo{scope}` re-applies.
    pub(crate) fn next_redo(&self, scope: &HistoryScope) -> Option<u64> {
        match scope {
            HistoryScope::Actor(actor) => self.stacks.get(actor)?.redo.last().copied(),
            HistoryScope::Any => self
                .stacks
                .values()
                .filter_map(|s| s.redo.last().copied())
                .max_by_key(|seq| {
                    self.log
                        .get(seq)
                        .and_then(|r| r.undone)
                        .map_or(0, |(_, undo_seq)| undo_seq)
                }),
        }
    }

    fn stacks_of(&self, actor: &Actor) -> ActorStacks {
        let stacks = self.stacks.get(actor);
        let top = |seqs: Option<&Vec<u64>>| {
            seqs.and_then(|s| s.last())
                .and_then(|seq| self.log.get(seq))
                .map(Record::top)
        };
        let depth = |seqs: Option<&Vec<u64>>| {
            seqs.map_or(0, |s| u32::try_from(s.len()).unwrap_or(u32::MAX))
        };
        ActorStacks {
            actor: actor.clone(),
            undo_depth: depth(stacks.map(|s| &s.undo)),
            redo_depth: depth(stacks.map(|s| &s.redo)),
            next_undo: top(stacks.map(|s| &s.undo)),
            next_redo: top(stacks.map(|s| &s.redo)),
        }
    }

    /// Every actor with a non-empty stack.
    pub(crate) fn all_stacks(&self) -> Vec<ActorStacks> {
        self.stacks
            .iter()
            .filter(|(_, s)| !s.undo.is_empty() || !s.redo.is_empty())
            .map(|(actor, _)| self.stacks_of(actor))
            .collect()
    }

    pub(crate) fn usage(&self) -> HistoryUsage {
        let wide = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
        let narrow = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        HistoryUsage {
            entries: narrow(self.log.len()),
            bytes: wide(self.bytes),
            max_entries: narrow(self.limits.max_entries),
            max_bytes: wide(self.limits.max_bytes),
            evicted: self.evicted,
        }
    }

    fn event(
        &self,
        change: HistoryChange,
        tx_id: Option<TxId>,
        rev: Rev,
        actor: &Actor,
    ) -> HistoryChanged {
        HistoryChanged {
            change,
            tx_id,
            committed_rev: rev,
            stacks: self.stacks_of(actor),
        }
    }

    fn set_slot(&mut self, seq: u64, slot: Slot) {
        if let Some(r) = self.log.get_mut(&seq) {
            r.slot = slot;
            if slot == Slot::None {
                self.dead.insert(seq);
            } else {
                self.dead.remove(&seq);
            }
        }
    }

    fn clear_redo(&mut self, actor: &Actor) -> bool {
        let redo = self
            .stacks
            .get_mut(actor)
            .map_or_else(Vec::new, |s| std::mem::take(&mut s.redo));
        for seq in &redo {
            self.set_slot(*seq, Slot::None);
        }
        !redo.is_empty()
    }

    fn insert(&mut self, new: NewRecord, slot: Slot, shared_patch: bool) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        let bytes = std::mem::size_of::<Record>()
            + new.label.len()
            + if shared_patch {
                0
            } else {
                patch_bytes(&new.patch)
            };
        self.bytes += bytes;
        self.log.insert(
            seq,
            Record {
                seq,
                tx_id: new.tx_id,
                actor: new.actor,
                label: new.label,
                kind: new.kind,
                cause: new.cause,
                prev_rev: new.prev_rev,
                rev: new.rev,
                at: new.at,
                patch: new.patch,
                status: EntryStatus::Applied,
                applied_rev: new.rev,
                applied_seq: seq,
                undone: None,
                slot,
                bytes,
            },
        );
        if slot == Slot::None {
            self.dead.insert(seq);
        }
        seq
    }

    /// Logs a change (an edit or a restore) by `new.actor`: onto its undo stack if undoable;
    /// its redo stack is cleared.
    pub(crate) fn push_change(&mut self, new: NewRecord) -> Vec<HistoryChanged> {
        let actor = new.actor.clone();
        let (tx_id, rev, undoable) = (new.tx_id, new.rev, new.undoable);
        self.clear_redo(&actor);
        let seq = self.insert(new, if undoable { Slot::Undo } else { Slot::None }, false);
        let mut touched = BTreeSet::new();
        if undoable {
            let depth = self.limits.max_undo_depth;
            let stacks = self.stacks.entry(actor.clone()).or_default();
            stacks.undo.push(seq);
            let excess = stacks.undo.len().saturating_sub(depth);
            let dropped: Vec<u64> = stacks.undo.drain(..excess).collect();
            for old in dropped {
                self.set_slot(old, Slot::None);
            }
        }
        self.evict(&mut touched);
        touched.remove(&actor);
        let mut events = vec![self.event(HistoryChange::Recorded, Some(tx_id), rev, &actor)];
        events.extend(
            touched
                .iter()
                .map(|a| self.event(HistoryChange::Trimmed, None, rev, a)),
        );
        events
    }

    /// Logs the undo of `target` (an undo record by `new.actor`) and moves the target to its
    /// actor's redo stack.
    pub(crate) fn push_undo(&mut self, target: u64, new: NewRecord) -> Vec<HistoryChanged> {
        let rev = new.rev;
        let Some(owner) = self.log.get(&target).map(|r| r.actor.clone()) else {
            return Vec::new();
        };
        let seq = self.insert(new, Slot::None, false);
        if let Some(stacks) = self.stacks.get_mut(&owner) {
            stacks.undo.retain(|s| *s != target);
            stacks.redo.push(target);
        }
        let tx_id = self.log.get_mut(&target).map(|r| {
            r.status = EntryStatus::Undone;
            r.undone = Some((rev, seq));
            r.tx_id
        });
        self.set_slot(target, Slot::Redo);
        self.finish_move(HistoryChange::Undone, tx_id, rev, &owner)
    }

    /// Logs the redo of `target` (a redo record by `new.actor`, sharing the target's patch)
    /// and moves the target back to its actor's undo stack.
    pub(crate) fn push_redo(&mut self, target: u64, new: NewRecord) -> Vec<HistoryChanged> {
        let rev = new.rev;
        let Some(owner) = self.log.get(&target).map(|r| r.actor.clone()) else {
            return Vec::new();
        };
        let seq = self.insert(new, Slot::None, true);
        if let Some(stacks) = self.stacks.get_mut(&owner) {
            stacks.redo.retain(|s| *s != target);
            stacks.undo.push(target);
        }
        let tx_id = self.log.get_mut(&target).map(|r| {
            r.status = EntryStatus::Applied;
            r.applied_rev = rev;
            r.applied_seq = seq;
            r.undone = None;
            r.tx_id
        });
        self.set_slot(target, Slot::Undo);
        self.finish_move(HistoryChange::Redone, tx_id, rev, &owner)
    }

    fn finish_move(
        &mut self,
        change: HistoryChange,
        tx_id: Option<TxId>,
        rev: Rev,
        owner: &Actor,
    ) -> Vec<HistoryChanged> {
        let mut touched = BTreeSet::new();
        self.evict(&mut touched);
        touched.remove(owner);
        let mut events = vec![self.event(change, tx_id, rev, owner)];
        events.extend(
            touched
                .iter()
                .map(|a| self.event(HistoryChange::Trimmed, None, rev, a)),
        );
        events
    }

    /// Evicts records while over the limits (never the newest), dead ones first.
    fn evict(&mut self, touched: &mut BTreeSet<Actor>) {
        while self.log.len() > 1
            && (self.log.len() > self.limits.max_entries || self.bytes > self.limits.max_bytes)
        {
            let newest = self.log.keys().next_back().copied();
            let victim = self
                .dead
                .iter()
                .copied()
                .find(|s| Some(*s) != newest)
                .or_else(|| self.log.keys().next().copied());
            let Some(victim) = victim else { break };
            let Some(record) = self.log.remove(&victim) else {
                self.dead.remove(&victim);
                continue;
            };
            self.dead.remove(&victim);
            self.bytes = self.bytes.saturating_sub(record.bytes);
            self.evicted += 1;
            match record.slot {
                Slot::None => {}
                Slot::Undo => {
                    if let Some(stacks) = self.stacks.get_mut(&record.actor) {
                        stacks.undo.retain(|s| *s != victim);
                    }
                    touched.insert(record.actor.clone());
                }
                Slot::Redo => {
                    // The entries undone after it depend on it: the whole redo stack goes.
                    if let Some(stacks) = self.stacks.get_mut(&record.actor) {
                        stacks.redo.retain(|s| *s != victim);
                    }
                    self.clear_redo(&record.actor);
                    touched.insert(record.actor.clone());
                }
            }
        }
    }

    /// The records after `after` whose patches touch `path` (or an ancestor or descendant of
    /// it), newest first, at most `max`.
    pub(crate) fn touching(&self, after: u64, path: &DocPath, max: usize) -> Vec<&Record> {
        self.log
            .range(after.saturating_add(1)..)
            .rev()
            .map(|(_, r)| r)
            .filter(|r| {
                r.patch
                    .iter()
                    .any(|op| op.paths().any(|p| overlaps(p, path)))
            })
            .take(max)
            .collect()
    }

    /// The actors of the records from `from` on whose patches overlap `patch`, and whether
    /// some records in that range are no longer in the log (their authors are unknown).
    pub(crate) fn authors_since(
        &self,
        from: u64,
        patch: &Patch,
    ) -> (BTreeMap<Actor, Vec<TxId>>, bool) {
        let paths: Vec<&DocPath> = patch.iter().flat_map(PatchOp::paths).collect();
        let mut authors: BTreeMap<Actor, Vec<TxId>> = BTreeMap::new();
        let mut retained = 0_u64;
        for record in self.log.range(from..).map(|(_, r)| r) {
            retained += 1;
            let hit = record
                .patch
                .iter()
                .flat_map(PatchOp::paths)
                .any(|p| paths.iter().any(|q| overlaps(p, q)));
            if hit {
                authors
                    .entry(record.actor.clone())
                    .or_default()
                    .push(record.tx_id);
            }
        }
        let expected = self.next_seq.saturating_sub(from);
        (authors, retained < expected)
    }

    /// A page of entries, newest first, before `cursor` (a seq), optionally only `actor`'s.
    pub(crate) fn page(
        &self,
        limit: usize,
        before: Option<u64>,
        actor: Option<&Actor>,
    ) -> (Vec<HistoryEntry>, Option<u64>) {
        let end = before.unwrap_or(u64::MAX);
        let mut matching = self
            .log
            .range(..end)
            .rev()
            .map(|(_, r)| r)
            .filter(|r| actor.is_none_or(|a| &r.actor == a));
        let entries: Vec<&Record> = matching.by_ref().take(limit).collect();
        let more = matching.next().is_some();
        let next = if more {
            entries.last().map(|r| r.seq)
        } else {
            None
        };
        (entries.into_iter().map(Record::entry).collect(), next)
    }

    /// Rebuilds the history from recovered commits (WP-1C.02), in commit order.
    pub(crate) fn replay(&mut self, commit: NewRecord) {
        match commit.cause {
            EntryCause::Edit | EntryCause::Restore { .. } => {
                self.push_change(commit);
            }
            EntryCause::Undo { of } => match self.find_tx(of) {
                Some(target) => {
                    self.push_undo(target, commit);
                }
                None => {
                    self.insert(commit, Slot::None, false);
                }
            },
            EntryCause::Redo { of } => match self.find_tx(of) {
                Some(target) => {
                    self.push_redo(target, commit);
                }
                None => {
                    self.insert(commit, Slot::None, false);
                }
            },
        }
    }

    /// Checks the bookkeeping (tests and diagnostics).
    pub(crate) fn verify(&self) -> Result<(), String> {
        let mut bytes = 0;
        for (seq, r) in &self.log {
            if *seq != r.seq {
                return Err(format!("record {seq} has seq {}", r.seq));
            }
            bytes += r.bytes;
            if (r.slot == Slot::None) != self.dead.contains(seq) {
                return Err(format!("record {seq}: dead set out of sync"));
            }
        }
        if bytes != self.bytes {
            return Err(format!("byte count {} != {bytes}", self.bytes));
        }
        if self.dead.iter().any(|s| !self.log.contains_key(s)) {
            return Err("dead set has evicted records".into());
        }
        for (actor, stacks) in &self.stacks {
            if stacks.undo.len() > self.limits.max_undo_depth {
                return Err(format!("{actor}: undo stack deeper than the limit"));
            }
            for (seqs, slot) in [(&stacks.undo, Slot::Undo), (&stacks.redo, Slot::Redo)] {
                for seq in seqs {
                    let r = self
                        .log
                        .get(seq)
                        .ok_or_else(|| format!("{actor}: stack has evicted record {seq}"))?;
                    if r.slot != slot || &r.actor != actor || !r.cause.is_change() {
                        return Err(format!("{actor}: record {seq} misfiled"));
                    }
                }
            }
            if stacks.undo.windows(2).any(|w| w[0] >= w[1]) {
                return Err(format!("{actor}: undo stack out of commit order"));
            }
        }
        Ok(())
    }
}

/// The document a checkpoint captured, with its metadata.
#[derive(Clone, Debug)]
pub(crate) struct Checkpoint {
    pub(crate) id: CheckpointId,
    pub(crate) name: Option<String>,
    pub(crate) note: Option<String>,
    pub(crate) rev: Rev,
    pub(crate) actor: Actor,
    pub(crate) at: WallTime,
    pub(crate) doc: Project,
    /// The first history record after it.
    pub(crate) first_seq: u64,
    last_used: u64,
}

impl Checkpoint {
    pub(crate) fn info(&self) -> CheckpointInfo {
        CheckpointInfo {
            checkpoint: self.id,
            name: self.name.clone(),
            note: self.note.clone(),
            rev: self.rev,
            actor: self.actor.clone(),
            at: self.at,
        }
    }
}

/// A checkpoint, as `edit.checkpoint` and `edit.list_checkpoints` report it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct CheckpointInfo {
    /// Its id.
    pub checkpoint: CheckpointId,
    /// Its name, if any.
    pub name: Option<String>,
    /// A free-form note.
    pub note: Option<String>,
    /// The committed revision it captured.
    pub rev: Rev,
    /// Who made it.
    pub actor: Actor,
    /// When.
    pub at: WallTime,
}

/// Most checkpoints kept (least recently used unnamed ones go first).
pub const MAX_CHECKPOINTS: usize = 50;
/// Most named checkpoints kept (least recently used go first).
pub const MAX_NAMED_CHECKPOINTS: usize = 20;
/// Longest checkpoint name, in characters.
pub const MAX_CHECKPOINT_NAME_CHARS: usize = 64;
/// Longest checkpoint note, in characters.
pub const MAX_CHECKPOINT_NOTE_CHARS: usize = 1_000;

/// In-memory checkpoints: O(1) snapshots of the committed document thanks to the persistent
/// collections (only what changes later costs memory).
#[derive(Debug, Default)]
pub(crate) struct Checkpoints {
    next: u64,
    tick: u64,
    map: BTreeMap<CheckpointId, Checkpoint>,
}

impl Checkpoints {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create(
        &mut self,
        name: Option<String>,
        note: Option<String>,
        rev: Rev,
        actor: Actor,
        at: WallTime,
        doc: Project,
        first_seq: u64,
    ) -> CheckpointInfo {
        // A name moves to the new checkpoint.
        if let Some(name) = &name {
            self.map.retain(|_, c| c.name.as_ref() != Some(name));
        }
        self.next += 1;
        self.tick += 1;
        let id = CheckpointId(self.next);
        let checkpoint = Checkpoint {
            id,
            name,
            note,
            rev,
            actor,
            at,
            doc,
            first_seq,
            last_used: self.tick,
        };
        let info = checkpoint.info();
        self.map.insert(id, checkpoint);
        self.trim(id);
        info
    }

    fn trim(&mut self, keep: CheckpointId) {
        let lru = |map: &BTreeMap<CheckpointId, Checkpoint>, named: bool| {
            map.values()
                .filter(|c| c.id != keep && c.name.is_some() == named)
                .min_by_key(|c| c.last_used)
                .map(|c| c.id)
        };
        while self.map.values().filter(|c| c.name.is_some()).count() > MAX_NAMED_CHECKPOINTS {
            match lru(&self.map, true) {
                Some(id) => self.map.remove(&id),
                None => break,
            };
        }
        while self.map.len() > MAX_CHECKPOINTS {
            match lru(&self.map, false).or_else(|| lru(&self.map, true)) {
                Some(id) => self.map.remove(&id),
                None => break,
            };
        }
    }

    /// The checkpoint with this id (`cp_<n>`) or name, marked as used.
    pub(crate) fn find(&mut self, key: &str) -> Option<&Checkpoint> {
        let id = CheckpointId::parse(key)
            .filter(|id| self.map.contains_key(id))
            .or_else(|| {
                self.map
                    .values()
                    .find(|c| c.name.as_deref() == Some(key))
                    .map(|c| c.id)
            })?;
        self.tick += 1;
        let tick = self.tick;
        let c = self.map.get_mut(&id)?;
        c.last_used = tick;
        Some(c)
    }

    /// Every checkpoint, oldest first.
    pub(crate) fn list(&self) -> Vec<CheckpointInfo> {
        self.map.values().map(Checkpoint::info).collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;

    use super::*;

    fn rec(tx: u64, actor: &Actor, path: &str) -> NewRecord {
        NewRecord {
            tx_id: TxId::new(tx),
            actor: actor.clone(),
            label: format!("t{tx}"),
            kind: None,
            cause: EntryCause::Edit,
            prev_rev: Rev(tx - 1),
            rev: Rev(tx),
            at: WallTime::UNIX_EPOCH,
            patch: Arc::new(Patch(vec![PatchOp::Replace {
                path: DocPath::parse(path).unwrap(),
                old: json!(tx - 1),
                value: json!(tx),
            }])),
            undoable: true,
        }
    }

    #[test]
    fn scopes_and_ids_round_trip() {
        for s in ["*", "user", "agent:claude", "plugin:acme.tools", "system"] {
            let scope: HistoryScope = serde_json::from_value(json!(s)).unwrap();
            assert_eq!(serde_json::to_value(&scope).unwrap(), json!(s));
        }
        assert!(serde_json::from_value::<HistoryScope>(json!("bob")).is_err());
        assert_eq!(CheckpointId::parse("cp_12"), Some(CheckpointId::new(12)));
        for bad in ["cp_", "cp_x", "12", "CP_1"] {
            assert_eq!(CheckpointId::parse(bad), None, "{bad}");
        }
        assert_eq!(
            serde_json::to_value(EntryCause::Undo { of: TxId::new(3) }).unwrap(),
            json!({"type": "undo", "of": "tx_3"})
        );
    }

    #[test]
    fn per_actor_stacks_and_redo_invalidation() {
        let (u, a) = (Actor::User, Actor::agent("claude").unwrap());
        let mut h = History::new(HistoryLimits::default());
        h.push_change(rec(1, &u, "/meta/name"));
        h.push_change(rec(2, &a, "/settings"));
        h.push_change(rec(3, &u, "/meta/name"));
        assert_eq!(h.next_undo(&HistoryScope::Actor(u.clone())), Some(3));
        assert_eq!(h.next_undo(&HistoryScope::Actor(a.clone())), Some(2));
        assert_eq!(h.next_undo(&HistoryScope::Any), Some(3));
        let mut undo = rec(4, &u, "/meta/name");
        undo.cause = EntryCause::Undo { of: TxId::new(3) };
        h.push_undo(3, undo);
        assert_eq!(h.next_undo(&HistoryScope::Actor(u.clone())), Some(1));
        assert_eq!(h.next_redo(&HistoryScope::Actor(u.clone())), Some(3));
        assert_eq!(h.next_redo(&HistoryScope::Any), Some(3));
        // The agent's new edit leaves the user's redo stack alone ...
        h.push_change(rec(5, &a, "/settings"));
        assert_eq!(h.next_redo(&HistoryScope::Actor(u.clone())), Some(3));
        // ... the user's own clears it.
        h.push_change(rec(6, &u, "/custom"));
        assert_eq!(h.next_redo(&HistoryScope::Actor(u.clone())), None);
        h.verify().unwrap();
    }

    #[test]
    fn eviction_prefers_entries_no_stack_needs() {
        let (u, a) = (Actor::User, Actor::agent("claude").unwrap());
        let mut h = History::new(HistoryLimits {
            max_entries: 4,
            max_bytes: usize::MAX,
            max_undo_depth: 2,
        });
        h.push_change(rec(1, &u, "/meta/name"));
        for tx in 2..=6 {
            h.push_change(rec(tx, &a, "/settings"));
        }
        // The agent's undo depth is 2: tx 2..=4 fell off its stack and were evicted first; the
        // user's only entry survives although it is the oldest.
        assert_eq!(h.next_undo(&HistoryScope::Actor(u.clone())), Some(1));
        let (entries, _) = h.page(10, None, None);
        let txs: Vec<u64> = entries.iter().map(|e| e.tx_id.get()).collect();
        assert_eq!(txs, [6, 5, 4, 1]);
        assert!(!entries[2].can_undo);
        assert_eq!(h.usage().evicted, 2);
        h.verify().unwrap();
    }

    #[test]
    fn pages_follow_the_cursor() {
        let u = Actor::User;
        let mut h = History::new(HistoryLimits::default());
        for tx in 1..=5 {
            h.push_change(rec(tx, &u, "/meta/name"));
        }
        let (first, cursor) = h.page(2, None, None);
        assert_eq!(first.len(), 2);
        let (second, cursor) = h.page(2, cursor, None);
        let (third, cursor) = h.page(2, cursor, None);
        assert_eq!(cursor, None);
        let all: Vec<u64> = [first, second, third]
            .concat()
            .iter()
            .map(|e| e.tx_id.get())
            .collect();
        assert_eq!(all, [5, 4, 3, 2, 1]);
    }

    #[test]
    fn checkpoint_names_move_and_lru_trims() {
        let mut c = Checkpoints::default();
        let doc = Project::new("x");
        let mk = |c: &mut Checkpoints, name: Option<&str>| {
            c.create(
                name.map(str::to_owned),
                None,
                Rev(1),
                Actor::User,
                WallTime::UNIX_EPOCH,
                doc.clone(),
                1,
            )
            .checkpoint
        };
        let first = mk(&mut c, Some("a"));
        let second = mk(&mut c, Some("a"));
        assert_ne!(first, second);
        assert_eq!(c.find("a").unwrap().id, second);
        assert!(c.find(&first.to_string()).is_none());
        for _ in 0..MAX_CHECKPOINTS + 5 {
            mk(&mut c, None);
        }
        assert_eq!(c.list().len(), MAX_CHECKPOINTS);
        // The named one survives the unnamed churn.
        assert!(c.find("a").is_some());
        for i in 0..MAX_NAMED_CHECKPOINTS + 3 {
            mk(&mut c, Some(&format!("n{i}")));
        }
        let named = c.list().iter().filter(|i| i.name.is_some()).count();
        assert_eq!(named, MAX_NAMED_CHECKPOINTS);
    }
}
