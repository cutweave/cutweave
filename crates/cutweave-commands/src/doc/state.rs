// SPDX-License-Identifier: GPL-3.0-or-later
//! [`DocState`]: the single-writer document state machine (arch §5.2, ADR-0005, ADR-0017),
//! with its configuration ([`DocConfig`]) and journal seam ([`CommitSink`]).

use std::{
    any::{Any, TypeId},
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    panic::{self, AssertUnwindSafe},
    sync::Arc,
    time::Duration,
};

use cutweave_model::{IdGen, ItemId, Project, SeededGen};
use serde::Serialize;
use serde_json::{Map, Value};

use super::{
    checkpoint::{
        CheckpointId, CheckpointInfo, CheckpointLedger, CheckpointRecord, Checkpoints,
        MAX_CHECKPOINT_NAME_CHARS, MAX_CHECKPOINT_NOTE_CHARS,
    },
    clock::{Clock, ManualClock, WallTime},
    dedup::{DEDUP_CAPACITY, DEDUP_WINDOW, DedupRecord, DedupTable, Fingerprint},
    event::{
        CommandPanicked, DocEvent, DocPatched, DocReset, DocSnapshot, GestureCancelReason,
        GestureCancelled, PatchCause, TxActivity, TxEnded,
    },
    history::{
        EntryCause, History, HistoryLimits, HistoryMoved, HistoryPage, HistoryScope,
        MAX_HISTORY_PAGE, MAX_UNDO_DEPTH, NewRecord, Record, Restored, RevertedChanges,
    },
    identity::{Actor, ActorCtx, ConnId},
    locks::{ItemLocks, LockList, TxLocks},
    request::{Applied, RequestId, Submit, TxClosed, TxId, TxInfo, TxKind, TxOutcome},
    revlog::RevLog,
};
use crate::{
    diff::diff,
    edit::{TxCommitted, TxOpened},
    error::{CmdError, ConflictReason, ResourceKind, Rev, encode_output},
    meta::EntryKind,
    patch::{Patch, PatchError},
    path::DocPath,
    registry::Registry,
    tx::{Committed, Transaction},
};

/// The shortest `retry_after_ms` hint of a `Conflict{reason: item_in_gesture}` (WP-1B.09).
///
/// A collision with an open gesture hints at the time left until its idle deadline (it is
/// committed, cancelled or expired by then unless it keeps moving), but never less than this;
/// a collision with an open batch's changes hints exactly this (batches have no deadline and
/// are short-lived). A proposal gives no hint: it waits for a person to accept or reject it.
pub const SOFT_LOCK_RETRY_MS: u64 = 250;

/// How many closed connections the state remembers, so that a request racing its
/// connection's close cannot open a transaction nobody would ever close (WP-1B.09).
const CLOSED_CONNECTIONS_REMEMBERED: usize = 1024;

/// A gesture transaction that no request has named for this long is cancelled by the engine
/// (outcome [`TxOutcome::Expired`]; arch §5.2, ADR-0017). Proposals and batches never expire.
pub const GESTURE_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Mixed into the `--deterministic-ids` seed for the request-id stream, so engine-generated
/// request ids never perturb the entity id sequence (CLI and RPC runs then produce identical
/// documents, WP-1B.08).
const REQUEST_ID_STREAM: u64 = 0x5245_5155_4553_5453; // "REQUESTS"

/// Transaction ids set aside per millisecond of wall-clock time when ids are per run
/// ([`DocConfig::with_tx_ids_per_run`]): a run that starts at millisecond `t` numbers its
/// transactions from `t * TX_IDS_PER_MILLI + 1`. No run opens a million transactions per
/// millisecond, so a run never reaches the ids of one that started later.
const TX_IDS_PER_MILLI: u64 = 1_000_000;

/// The first transaction id of a run that starts at `now` with per-run ids: past every id a
/// run that started earlier issued, and never before `floor` (one past the last journaled).
/// With the clock at the Unix epoch (frozen, `--deterministic-ids`) that is `floor` itself.
fn first_tx_of_run(floor: u64, now: WallTime) -> u64 {
    now.as_millis()
        .checked_mul(TX_IDS_PER_MILLI)
        .and_then(|base| base.checked_add(1))
        .map_or(floor, |first| first.max(floor))
}

/// Persists committed transactions (the journal, WP-1C.02).
///
/// Called on the actor thread for every committed transaction, **before** the transaction
/// becomes visible or is acknowledged. An `Err` aborts the commit: nothing changes, and the
/// requester gets the error. It must not panic: unlike a command's, a sink's panic stops the
/// actor (durability can no longer be promised).
pub trait CommitSink: Send {
    /// Persists `record`.
    fn commit(&mut self, record: &CommitRecord<'_>) -> Result<(), CmdError>;

    /// Persists a change of the checkpoints (WP-2I.03): called on the actor thread before the
    /// change takes effect or is acknowledged; an `Err` aborts it. The default keeps nothing,
    /// so the checkpoints last as long as the document state.
    fn checkpoint(&mut self, record: &CheckpointRecord<'_>) -> Result<(), CmdError> {
        let _ = record;
        Ok(())
    }

    /// Persists a transaction marker (ADR-0043): a reservation of transaction ids, called
    /// before a transaction is opened (an `Err` refuses to open it), or a close that committed
    /// no change, called before its request is acknowledged (an `Err` is logged: the close
    /// stands, and its repeat after a restart is refused as before). The default keeps
    /// nothing.
    fn tx_marker(&mut self, record: &TxMarker<'_>) -> Result<(), CmdError> {
        let _ = record;
        Ok(())
    }
}

/// Closures are sinks (tests, adapters).
impl<F> CommitSink for F
where
    F: FnMut(&CommitRecord<'_>) -> Result<(), CmdError> + Send,
{
    fn commit(&mut self, record: &CommitRecord<'_>) -> Result<(), CmdError> {
        self(record)
    }
}

/// The default sink: keeps nothing (no journal yet, or a scratch document).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoJournal;

impl CommitSink for NoJournal {
    fn commit(&mut self, _: &CommitRecord<'_>) -> Result<(), CmdError> {
        Ok(())
    }
}

/// One committed transaction, as handed to the [`CommitSink`].
#[derive(Clone, Copy, Debug, Serialize)]
pub struct CommitRecord<'a> {
    /// The committed revision it creates.
    pub rev: Rev,
    /// The committed revision it builds on.
    pub prev_committed_rev: Rev,
    /// Its transaction.
    pub tx_id: TxId,
    /// `None` for an implicit single-command transaction (and for undo, redo and restore).
    pub kind: Option<TxKind>,
    /// Who committed it.
    pub actor: &'a Actor,
    /// The open transaction's label, or the history label of an undo, redo or restore;
    /// `None` for a single command (its history label is the command name).
    pub label: Option<&'a str>,
    /// When it committed (wall clock).
    pub at: WallTime,
    /// The requests it contains, in order, each with its redacted params and its dedup
    /// record (so a restarted engine keeps deduplicating them). For an undo, redo or restore:
    /// the `edit.*` request, when it came through [`DocState::submit`]. For a batch, gesture
    /// or proposal: its commands in order, then the `edit.begin_tx` and `edit.commit_tx`
    /// requests that opened and committed it (when they came through `submit`).
    pub steps: &'a [JournalStep],
    /// The net change to the committed document (coalesced, field-level).
    pub patch: &'a Patch,
    /// Why it is in the history: an edit, an undo or redo of an earlier transaction, or a
    /// checkpoint restore (so recovery can rebuild the undo stacks, WP-1C.02).
    pub cause: EntryCause,
    /// Whether it went onto its actor's undo stack (every changing command was undoable).
    pub undoable: bool,
}

impl CommitRecord<'_> {
    /// The owned form (what the journal reads back).
    #[must_use]
    pub fn to_owned(&self) -> JournaledCommit {
        JournaledCommit {
            rev: self.rev,
            prev_committed_rev: self.prev_committed_rev,
            tx_id: self.tx_id,
            kind: self.kind,
            actor: self.actor.clone(),
            label: self.label.map(str::to_owned),
            at: self.at,
            steps: self.steps.to_vec(),
            patch: self.patch.clone(),
            cause: self.cause,
            undoable: self.undoable,
        }
    }
}

const fn yes() -> bool {
    true
}

/// A [`CommitRecord`] read back from the journal (same JSON form), to rebuild the undo
/// history after recovery ([`DocConfig::with_recovered_history`], WP-1C.02).
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct JournaledCommit {
    /// The committed revision it created.
    pub rev: Rev,
    /// The committed revision it built on.
    pub prev_committed_rev: Rev,
    /// Its transaction.
    pub tx_id: TxId,
    /// Its kind (`None`: implicit).
    pub kind: Option<TxKind>,
    /// Who committed it.
    pub actor: Actor,
    /// Its label.
    pub label: Option<String>,
    /// When it committed.
    pub at: WallTime,
    /// Its requests.
    pub steps: Vec<JournalStep>,
    /// Its net patch.
    pub patch: Patch,
    /// Edit, undo, redo or restore.
    #[serde(default)]
    pub cause: EntryCause,
    /// Whether it is undoable.
    #[serde(default = "yes")]
    pub undoable: bool,
}

impl JournaledCommit {
    fn into_record(self) -> NewRecord {
        let label = self
            .label
            .or_else(|| self.steps.first().map(|s| s.command.clone()))
            .unwrap_or_else(|| "change".to_owned());
        NewRecord {
            tx_id: self.tx_id,
            actor: self.actor,
            label,
            kind: self.kind,
            cause: self.cause,
            prev_rev: self.prev_committed_rev,
            rev: self.rev,
            at: self.at,
            patch: Arc::new(self.patch),
            undoable: self.undoable && self.cause.is_change(),
        }
    }
}

/// How many transaction ids one [`TxMarker`] reserves (ADR-0043): the journal is told before
/// an id is handed to a client, once per this many transactions, and a restarted engine
/// numbers past the last reservation.
pub const TX_IDS_RESERVED: u64 = 64;

/// What the journal keeps of transactions that commit no change (ADR-0043), handed to the
/// [`CommitSink`] as a marker: it carries no revision and changes no document.
///
/// Two occasions write one. Before a transaction is opened with an id past the last
/// reservation: [`issued`](Self::issued) alone, so a restarted engine never hands out an id
/// the engine that died had given to a transaction the journal never saw. And when a request
/// closes a transaction without a committed change (a cancel, or a commit of a transaction
/// that changed nothing): [`closed`](Self::closed) and the requests to remember, so a repeat
/// of the close after a restart gets its original result.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct TxMarker<'a> {
    /// Every transaction id up to this number may have been handed out.
    pub issued: u64,
    /// When it was written (the request's time).
    pub at: WallTime,
    /// The transaction a request closed, if this marks a close.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed: Option<ClosedTx>,
    /// The requests a restarted engine keeps answering: the `edit.begin_tx` that opened the
    /// closed transaction, the commands of an empty commit, and the closing request.
    #[serde(skip_serializing_if = "<[JournalStep]>::is_empty")]
    pub steps: &'a [JournalStep],
}

impl TxMarker<'_> {
    /// The owned form (what the journal reads back).
    #[must_use]
    pub fn to_owned(&self) -> JournaledTxMarker {
        JournaledTxMarker {
            issued: self.issued,
            at: self.at,
            closed: self.closed,
            steps: self.steps.to_vec(),
        }
    }
}

/// A transaction closed without a committed change, in a [`TxMarker`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ClosedTx {
    /// Its id.
    pub tx_id: TxId,
    /// Its kind.
    pub kind: TxKind,
    /// `committed` (it had changed nothing) or `cancelled`.
    pub outcome: TxOutcome,
}

/// A [`TxMarker`] read back from the journal (same JSON form).
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct JournaledTxMarker {
    /// Every transaction id up to this number may have been handed out.
    #[serde(default)]
    pub issued: u64,
    /// When it was written.
    #[serde(default)]
    pub at: WallTime,
    /// The transaction a request closed, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<ClosedTx>,
    /// The requests to keep answering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<JournalStep>,
}

/// A close waiting for its request's result ([`DocState::closing`]).
#[derive(Debug)]
struct Closing {
    closed: ClosedTx,
    /// The requests remembered with it, before the closing one.
    steps: Vec<JournalStep>,
}

/// One request inside a committed transaction.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct JournalStep {
    /// The registry name.
    pub command: String,
    /// Its params with `#[secret]` fields redacted (arch §14).
    pub params: Value,
    /// Its dedup record (request id, fingerprint, time, result).
    pub record: DedupRecord,
}

/// How to build a [`DocState`].
pub struct DocConfig {
    registry: Arc<Registry>,
    doc: Project,
    rev: Rev,
    ids: Box<dyn IdGen + Send>,
    request_ids: Box<dyn IdGen + Send>,
    clock: Arc<dyn Clock>,
    journal: Box<dyn CommitSink>,
    dedup_window: Duration,
    dedup_capacity: usize,
    restored: Vec<DedupRecord>,
    first_tx: u64,
    tx_ids_per_run: bool,
    history_limits: HistoryLimits,
    gesture_timeout: Duration,
    recovered: Vec<JournaledCommit>,
    checkpoints: CheckpointLedger,
}

impl fmt::Debug for DocConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocConfig")
            .field("rev", &self.rev)
            .field("clock", &self.clock)
            .field("dedup_window", &self.dedup_window)
            .field("restored", &self.restored.len())
            .finish_non_exhaustive()
    }
}

impl DocConfig {
    /// Production settings for `doc` at revision 0: UUIDv7 ids, the system clock, no journal.
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    #[must_use]
    pub fn new(registry: Arc<Registry>, doc: Project) -> DocConfig {
        DocConfig::with_parts(
            registry,
            doc,
            Box::new(cutweave_model::Uuid7Gen),
            Box::new(cutweave_model::Uuid7Gen),
            Arc::new(super::clock::SystemClock),
        )
    }

    /// Deterministic settings (`--deterministic-ids <seed>`, WP-1B.08): seeded entity and
    /// request ids (separate streams) and a clock frozen at the Unix epoch.
    #[must_use]
    pub fn deterministic(registry: Arc<Registry>, doc: Project, seed: u64) -> DocConfig {
        DocConfig::with_parts(
            registry,
            doc,
            Box::new(SeededGen::new(seed)),
            Box::new(SeededGen::new(seed ^ REQUEST_ID_STREAM)),
            Arc::new(ManualClock::new(WallTime::UNIX_EPOCH)),
        )
    }

    /// Explicit id generators and clock.
    #[must_use]
    pub fn with_parts(
        registry: Arc<Registry>,
        doc: Project,
        ids: Box<dyn IdGen + Send>,
        request_ids: Box<dyn IdGen + Send>,
        clock: Arc<dyn Clock>,
    ) -> DocConfig {
        DocConfig {
            registry,
            doc,
            rev: Rev::ZERO,
            ids,
            request_ids,
            clock,
            journal: Box::new(NoJournal),
            dedup_window: DEDUP_WINDOW,
            dedup_capacity: DEDUP_CAPACITY,
            restored: Vec::new(),
            first_tx: 1,
            tx_ids_per_run: false,
            history_limits: HistoryLimits::default(),
            gesture_timeout: GESTURE_IDLE_TIMEOUT,
            recovered: Vec::new(),
            checkpoints: CheckpointLedger::new(),
        }
    }

    /// Starts at `rev` (a recovered document, WP-1C.02).
    #[must_use]
    pub fn with_rev(mut self, rev: Rev) -> DocConfig {
        self.rev = rev;
        self
    }

    /// The entity id generator.
    #[must_use]
    pub fn with_ids(mut self, ids: Box<dyn IdGen + Send>) -> DocConfig {
        self.ids = ids;
        self
    }

    /// The generator for request ids the engine fills in.
    #[must_use]
    pub fn with_request_ids(mut self, ids: Box<dyn IdGen + Send>) -> DocConfig {
        self.request_ids = ids;
        self
    }

    /// The wall clock.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> DocConfig {
        self.clock = clock;
        self
    }

    /// Where committed transactions go before they are acknowledged.
    #[must_use]
    pub fn with_journal(mut self, journal: Box<dyn CommitSink>) -> DocConfig {
        self.journal = journal;
        self
    }

    /// The dedup window (default [`DEDUP_WINDOW`]; only tests change it).
    #[must_use]
    pub fn with_dedup_window(mut self, window: Duration) -> DocConfig {
        self.dedup_window = window;
        self
    }

    /// The dedup table's bound (default [`DEDUP_CAPACITY`]).
    #[must_use]
    pub fn with_dedup_capacity(mut self, capacity: usize) -> DocConfig {
        self.dedup_capacity = capacity;
        self
    }

    /// Requests remembered from before a restart (from the journal, WP-1C.02). Records older
    /// than the window are dropped on first use.
    #[must_use]
    pub fn with_restored_requests(mut self, records: Vec<DedupRecord>) -> DocConfig {
        self.restored = records;
        self
    }

    /// The first transaction id to allocate (after recovery: one past the last journaled).
    #[must_use]
    pub fn with_first_tx_id(mut self, first: TxId) -> DocConfig {
        self.first_tx = first.get().max(1);
        self
    }

    /// Numbers this run's transactions past every id an earlier run issued, also the ids the
    /// journal never saw (issue #251): transactions cancelled, or still open when the engine
    /// died. Without it a restarted engine hands those ids out again, and a commit or cancel
    /// sent again for the old transaction would close a new one.
    ///
    /// The first id is derived from the clock when the state is built (milliseconds since the
    /// Unix epoch times 1,000,000, plus one), and is never below
    /// [`with_first_tx_id`](Self::with_first_tx_id). Ids stay `tx_<n>` and increasing. A clock
    /// frozen at the Unix epoch (`--deterministic-ids`) gives the plain numbering, so such
    /// runs stay reproducible.
    ///
    /// For a journaled document this is the second line: the journal is told which ids may
    /// be handed out before they are ([`TxMarker`], ADR-0043), on any clock. This covers what
    /// has no such record: a document without a journal, and a reservation lost with the
    /// last unsynced writes in a power cut.
    #[must_use]
    pub fn with_tx_ids_per_run(mut self) -> DocConfig {
        self.tx_ids_per_run = true;
        self
    }

    /// The undo history's bounds (default [`HistoryLimits::default`]).
    #[must_use]
    pub fn with_history_limits(mut self, limits: HistoryLimits) -> DocConfig {
        self.history_limits = limits;
        self
    }

    /// How long a gesture may stay idle (default [`GESTURE_IDLE_TIMEOUT`]).
    #[must_use]
    pub fn with_gesture_timeout(mut self, timeout: Duration) -> DocConfig {
        self.gesture_timeout = timeout;
        self
    }

    /// Rebuilds the undo history from the journal's committed transactions, in commit order
    /// (WP-1C.02), so `edit.undo` reverts the last recovered transaction. The document must
    /// already be the result of these commits.
    #[must_use]
    pub fn with_recovered_history(mut self, commits: Vec<JournaledCommit>) -> DocConfig {
        self.recovered = commits;
        self
    }

    /// The checkpoints the journal rebuilt (WP-2I.03): each one's document is the committed
    /// document at its revision. Restoring one made before the recovered history (before the
    /// last save) reverts changes of unknown authorship, so it needs `force: true`.
    #[must_use]
    pub fn with_recovered_checkpoints(mut self, checkpoints: CheckpointLedger) -> DocConfig {
        self.checkpoints = checkpoints;
        self
    }
}

/// What a request changed, for the host to publish.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// The live document (and `rev`) changed.
    pub live: bool,
    /// The committed document (and `committed_rev`) changed.
    pub committed: bool,
}

impl Changes {
    const BOTH: Changes = Changes {
        live: true,
        committed: true,
    };

    /// Whether anything changed.
    #[must_use]
    pub const fn any(self) -> bool {
        self.live || self.committed
    }
}

// Four independent facts about one open transaction (conflicted, undoable, rebased, replaced),
// each read on its own path; an enum would combine states that never interact.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug)]
struct OpenTx {
    id: TxId,
    kind: TxKind,
    label: String,
    conn: ConnId,
    actor: Actor,
    opened_rev: Rev,
    opened_at: WallTime,
    last_activity: WallTime,
    /// Every step's ops, in order.
    ops: Patch,
    /// The committed document with `ops` replayed (stale once `conflicted`).
    view: Project,
    steps: Vec<JournalStep>,
    /// The `edit.begin_tx` request that opened it (none when opened directly, not through
    /// [`DocState::submit`]). Journaled with its commit, so a restarted engine answers a
    /// repeat with this transaction instead of opening another.
    begun: Option<JournalStep>,
    conflicted: bool,
    /// Every command that changed the document in it so far is undoable.
    undoable: bool,
    /// Its view was rebased onto commits made since its commands ran: its ops still apply, but
    /// the result may break an invariant they never saw (two clips appended into one gap).
    rebased: bool,
    /// The items it soft-locks for other actors: everything its ops touched, for a gesture or
    /// proposal; nothing for a batch (WP-1B.09). Dropped with it, so closing it releases them.
    /// The union over every step, also the steps a later `_meta.replace` update rolled back.
    locks: ItemLocks,
    /// A step replaced the earlier ones (`_meta.replace`): `locks` may hold more than `ops`
    /// touch now.
    replaced: bool,
}

impl OpenTx {
    fn info(&self, gesture_timeout: Duration) -> TxInfo {
        TxInfo {
            id: self.id,
            kind: self.kind,
            label: self.label.clone(),
            conn: self.conn,
            actor: self.actor.clone(),
            opened_rev: self.opened_rev,
            opened_at: self.opened_at,
            last_activity: self.last_activity,
            expires_at: self.expires_at(gesture_timeout),
            steps: self.steps.len(),
            items: self.locks.items(),
            conflicted: self.conflicted,
        }
    }

    /// Adds the requests that opened it and now commit it to its steps, for the journal:
    /// recovery restores a commit's steps as the requests to deduplicate (issue #235).
    /// Returns how many it added, for [`unseal`](Self::unseal).
    fn seal(&mut self, commit: Option<JournalStep>) -> usize {
        let before = self.steps.len();
        self.steps.extend(self.begun.clone());
        self.steps.extend(commit);
        self.steps.len() - before
    }

    /// Takes back what [`seal`](Self::seal) added: the journal refused the commit.
    fn unseal(&mut self, added: usize) {
        self.steps.truncate(self.steps.len().saturating_sub(added));
    }

    /// Whether its changes are part of the live document.
    fn contributes_live(&self) -> bool {
        self.kind.is_live() && !self.conflicted && !self.ops.is_empty()
    }

    /// A gesture's idle deadline.
    fn expires_at(&self, gesture_timeout: Duration) -> Option<WallTime> {
        (self.kind == TxKind::Gesture).then(|| self.last_activity.saturating_add(gesture_timeout))
    }

    fn activity(&self, rev: Rev) -> TxActivity {
        TxActivity {
            tx_id: self.id,
            kind: self.kind,
            label: self.label.clone(),
            actor: self.actor.clone(),
            items: self.locks.items(),
            rev,
        }
    }
}

/// The connections closed lately (FIFO-bounded): they cannot open transactions any more.
#[derive(Debug, Default)]
struct ClosedConns {
    order: VecDeque<ConnId>,
    set: BTreeSet<ConnId>,
}

impl ClosedConns {
    fn insert(&mut self, conn: ConnId) {
        if conn == ConnId::INTERNAL || !self.set.insert(conn) {
            return;
        }
        self.order.push_back(conn);
        if self.order.len() > CLOSED_CONNECTIONS_REMEMBERED
            && let Some(oldest) = self.order.pop_front()
        {
            self.set.remove(&oldest);
        }
    }

    fn contains(&self, conn: ConnId) -> bool {
        self.set.contains(&conn)
    }
}

/// A request being processed.
#[derive(Clone)]
struct Call {
    ctx: ActorCtx,
    name: String,
    journal_params: Value,
    request_id: RequestId,
    fingerprint: Fingerprint,
    at: WallTime,
    /// The command has `#[secret]` params: never echo its panic message.
    secret: bool,
    /// Its changes may be undone (registry metadata).
    undoable: bool,
    /// A read-only engine command (`edit.history`): nothing to remember for dedup.
    read_only: bool,
}

/// The change of the live document to broadcast.
enum LiveDelta {
    Ops(Patch),
    /// Not expressible as field-level ops: subscribers must resync.
    Reset,
}

impl LiveDelta {
    fn between(old: &Project, new: &Project) -> LiveDelta {
        diff(old, new).map_or(LiveDelta::Reset, LiveDelta::Ops)
    }
}

/// The other open transactions after a commit.
struct Rebased {
    views: Vec<(TxId, Project)>,
    losers: Vec<TxId>,
    live: Project,
    live_pending: bool,
}

/// A commit computed but not applied yet: the new committed document and what follows from
/// it (open transactions rebased, the live document recomposed, the revisions).
struct Staged {
    committed: Project,
    patch: Patch,
    views: Vec<(TxId, Project)>,
    live: Project,
    delta: LiveDelta,
    committed_rev: Rev,
    rev: Rev,
}

/// What a staged commit does to the history.
#[derive(Clone, Copy)]
enum HistoryMove {
    /// A change by the committing actor.
    Change,
    /// Undoes the record with this seq.
    Undo(u64),
    /// Redoes the record with this seq.
    Redo(u64),
}

/// How to journal and log a staged commit.
struct CommitMeta<'a> {
    tx_id: TxId,
    actor: &'a Actor,
    /// The history label.
    label: String,
    /// A single command: the journal carries no label.
    implicit: bool,
    at: WallTime,
    steps: &'a [JournalStep],
    cause: PatchCause,
    entry: EntryCause,
    undoable: bool,
    request: Option<RequestId>,
    history: HistoryMove,
}

/// Undo or redo.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Undo,
    Redo,
}

impl Direction {
    const fn verb(self) -> &'static str {
        match self {
            Direction::Undo => "undo",
            Direction::Redo => "redo",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Direction::Undo => "Undo",
            Direction::Redo => "Redo",
        }
    }
}

/// An open transaction's contribution when (re)composing the live document.
struct Layer<'a> {
    id: TxId,
    ops: &'a Patch,
    view: &'a Project,
}

/// The committed document with each layer's ops replayed in order. A single layer's view is
/// exactly that already. `Err` names the layer whose ops no longer apply.
fn compose(committed: &Project, layers: &[Layer<'_>]) -> Result<Project, (TxId, PatchError)> {
    match layers {
        [] => Ok(committed.clone()),
        [only] => Ok(only.view.clone()),
        many => {
            let mut doc = committed.clone();
            for layer in many {
                doc = layer.ops.apply(&doc).map_err(|e| (layer.id, e))?;
            }
            Ok(doc)
        }
    }
}

fn tx_not_found(id: TxId) -> CmdError {
    CmdError::not_found(ResourceKind::Transaction, id)
}

fn not_owner(t: &OpenTx) -> CmdError {
    CmdError::Locked {
        kind: ResourceKind::Transaction,
        id: t.id.to_string(),
        by: Some(t.actor.to_string()),
        tx: Some(t.id.to_string()),
    }
}

fn conflicted(t: &OpenTx) -> CmdError {
    CmdError::conflict(
        ConflictReason::StaleRev,
        format!(
            "transaction {} overlaps changes committed since it began and can only be cancelled",
            t.id
        ),
    )
}

/// The text of a panic payload.
pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-text panic payload".to_owned()
    }
}

/// The document state machine: runs every request serially.
///
/// Synchronous, deterministic (given its [`Clock`] and [`IdGen`]s) and wasm32-clean. The
/// [`DocActor`](crate::DocActor) owns one on a dedicated thread and publishes what it
/// produces; engine commands (`edit.*`, WP-1B.04) receive it as their context and act for
/// [`caller`](Self::caller).
///
/// # Model
///
/// - The **committed** document is the result of every committed transaction in order: what
///   saves, autosave and the journal see. `committed_rev` is its revision.
/// - Each **open transaction** (batch, gesture or proposal) records its ops; its *view* is the
///   committed document with those ops replayed, and its commands run against that view.
/// - The **live** document is the committed document with the ops of the open *live*
///   transactions (gestures, proposals) replayed in transaction order: what
///   [`snapshot`](Self::snapshot) shows, what `doc.patched` events describe and what
///   `expected_rev` is checked against. `rev` is its revision.
/// - Implicit single-command transactions run against the committed document, so the journal
///   replays them exactly. Their patch is rebased onto every open transaction; if it overlaps
///   one (the patch no longer applies), the command fails fast with
///   `Conflict{reason: item_in_gesture}` (the field-level backstop). Between open
///   transactions, the first to commit wins; an overlapping one becomes conflicted and can
///   only be cancelled.
///
/// # Ownership and soft locks (WP-1B.09, ADR-0017)
///
/// - An open transaction belongs to the [`ConnId`] that opened it (one per connection): only
///   that connection may run commands in it, commit it or cancel it (others get
///   `Locked{kind: transaction, by, tx}`). [`close_connection`](Self::close_connection) rolls
///   it back and refuses later transactions from that connection.
/// - An open **gesture or proposal soft-locks every item its commands touched** (added,
///   removed, moved or edited, or below a path it wrote). A request by **another actor** that
///   would change a locked item (an implicit command, a step of its own transaction, the
///   commit of a batch, an undo, redo or restore) fails before anything is applied with
///   `Conflict{reason: item_in_gesture, retry_after_ms}`: the time left until a gesture's idle
///   deadline, none for a proposal (see [`SOFT_LOCK_RETRY_MS`]). Requests on other items
///   proceed; the lock holder's own actor is never blocked by the lock (its other
///   connections still meet the field-level backstop). A batch locks nothing: it is invisible
///   until it commits, and its commit waits while another actor's gesture holds its items.
/// - Locks live inside their transaction, so commit, cancel, connection close and idle expiry
///   release them; `edit.locks` lists them and `tx.opened` / `tx.locked` / `tx.closed`
///   events announce them.
///
/// # Revisions
///
/// `rev` increases whenever the live document changes: once per committed implicit
/// transaction, once per command inside an open gesture or proposal (live updates), once when
/// an open transaction commits or rolls back its live changes. Commands inside a batch change
/// nothing visible and do not bump it; nor do no-ops, queries and failures. `committed_rev` is
/// the `rev` of the last commit; when a commit leaves other live changes pending, the commit
/// takes one revision and the live document the next, so `rev == committed_rev` always means
/// the live document is the committed one.
///
/// # History (WP-1B.04)
///
/// Every committed transaction is logged; undoable ones go onto their actor's undo stack, and
/// a batch, gesture or proposal is one entry. [`undo`](Self::undo) / [`redo`](Self::redo)
/// (`edit.undo` / `edit.redo`) commit the inverse / forward patch as a new transaction after
/// the per-actor safety checks (module `history`: the entry's paths are unchanged since and
/// the document's invariants still hold). [`checkpoint`](Self::checkpoint) keeps an O(1)
/// snapshot of the committed document and [`restore`](Self::restore) brings it back as an
/// undoable change. Gestures idle for [`GESTURE_IDLE_TIMEOUT`] are cancelled
/// ([`expire_idle`](Self::expire_idle)).
pub struct DocState {
    registry: Arc<Registry>,
    ids: Box<dyn IdGen + Send>,
    request_ids: Box<dyn IdGen + Send>,
    clock: Arc<dyn Clock>,
    journal: Box<dyn CommitSink>,
    committed: Project,
    committed_rev: Rev,
    live: Project,
    rev: Rev,
    open: BTreeMap<TxId, OpenTx>,
    closed_conns: ClosedConns,
    next_tx: u64,
    /// The highest transaction id the journal knows may be handed out ([`TxMarker::issued`]):
    /// opening a transaction past it reserves the next [`TX_IDS_RESERVED`] first.
    tx_reserved: u64,
    /// The transaction the running engine command closed without committing a change, for
    /// its [`TxMarker`] (written once the request's result is known).
    closing: Option<Closing>,
    dedup: DedupTable,
    history: History,
    checkpoints: Checkpoints,
    gesture_timeout: Duration,
    /// The request while an engine command runs.
    current: Option<Call>,
    /// What an engine command's commit journaled for the current request, so the dedup table
    /// remembers exactly what the journal has.
    claimed: Option<Applied>,
    outbox: Vec<DocEvent>,
    changes: Changes,
    /// What the recent revisions touched, for `Conflict.changed_since` (WP-2I.04).
    revlog: RevLog,
}

impl fmt::Debug for DocState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocState")
            .field("rev", &self.rev)
            .field("committed_rev", &self.committed_rev)
            .field("open", &self.open.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl DocState {
    /// A state over `config`'s document.
    #[must_use]
    pub fn new(config: DocConfig) -> DocState {
        let mut dedup = DedupTable::new(config.dedup_window, config.dedup_capacity);
        for record in config.restored {
            dedup.insert(record);
        }
        // A recovered checkpoint's first history record is the first recovered commit after
        // its revision; one made before the recovered history (before the last save) has none
        // in the log (0: its later changes are of unknown authorship).
        let base = config
            .recovered
            .first()
            .map_or(config.rev, |c| c.prev_committed_rev);
        let commit_revs: Vec<Rev> = config.recovered.iter().map(|c| c.rev).collect();
        let checkpoints = Checkpoints::recovered(config.checkpoints, |rev| {
            if rev < base {
                0
            } else {
                1 + u64::try_from(commit_revs.partition_point(|r| *r <= rev))
                    .unwrap_or(u64::MAX - 1)
            }
        });
        let next_tx = if config.tx_ids_per_run {
            first_tx_of_run(config.first_tx, config.clock.now())
        } else {
            config.first_tx
        };
        // Everything below the first id is accounted for (journaled, or an earlier run's).
        let tx_reserved = next_tx.saturating_sub(1);
        let mut history = History::new(config.history_limits);
        for commit in config.recovered {
            history.replay(commit.into_record());
        }
        DocState {
            registry: config.registry,
            ids: config.ids,
            request_ids: config.request_ids,
            clock: config.clock,
            journal: config.journal,
            live: config.doc.clone(),
            committed: config.doc,
            committed_rev: config.rev,
            rev: config.rev,
            open: BTreeMap::new(),
            closed_conns: ClosedConns::default(),
            next_tx,
            tx_reserved,
            closing: None,
            dedup,
            history,
            checkpoints,
            gesture_timeout: config.gesture_timeout,
            current: None,
            claimed: None,
            outbox: Vec::new(),
            changes: Changes::default(),
            revlog: RevLog::new(config.rev),
        }
    }

    // ----- reads -----------------------------------------------------------------------------

    /// The live revision.
    #[must_use]
    pub fn rev(&self) -> Rev {
        self.rev
    }

    /// The last committed revision.
    #[must_use]
    pub fn committed_rev(&self) -> Rev {
        self.committed_rev
    }

    /// The live document (with open gestures' and proposals' changes).
    #[must_use]
    pub fn doc(&self) -> &Project {
        &self.live
    }

    /// The committed document (what saves and the journal see).
    #[must_use]
    pub fn committed_doc(&self) -> &Project {
        &self.committed
    }

    /// The live document at its revision.
    #[must_use]
    pub fn snapshot(&self) -> DocSnapshot {
        DocSnapshot {
            rev: self.rev,
            committed_rev: self.committed_rev,
            doc: self.live.clone(),
        }
    }

    /// The committed document at its revision.
    #[must_use]
    pub fn committed_snapshot(&self) -> DocSnapshot {
        DocSnapshot {
            rev: self.committed_rev,
            committed_rev: self.committed_rev,
            doc: self.committed.clone(),
        }
    }

    /// The registry commands run through.
    #[must_use]
    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// The current wall-clock time.
    #[must_use]
    pub fn now(&self) -> WallTime {
        self.clock.now()
    }

    /// The caller of the engine command being run, if any (engine commands receive the state
    /// as their context and act for this caller).
    #[must_use]
    pub fn caller(&self) -> Option<&ActorCtx> {
        self.current.as_ref().map(|call| &call.ctx)
    }

    /// The open transactions, oldest first.
    #[must_use]
    pub fn transactions(&self) -> Vec<TxInfo> {
        self.open
            .values()
            .map(|t| t.info(self.gesture_timeout))
            .collect()
    }

    /// An open transaction.
    #[must_use]
    pub fn transaction(&self, id: TxId) -> Option<TxInfo> {
        self.open.get(&id).map(|t| t.info(self.gesture_timeout))
    }

    /// The open transaction owned by `conn`, if any (one per connection).
    #[must_use]
    pub fn transaction_of(&self, conn: ConnId) -> Option<TxInfo> {
        self.open
            .values()
            .find(|t| t.conn == conn)
            .map(|t| t.info(self.gesture_timeout))
    }

    /// Every open transaction with the items it soft-locks (`edit.locks`, WP-1B.09); `mine`
    /// marks those of `conn`.
    #[must_use]
    pub fn locks(&self, conn: Option<ConnId>) -> LockList {
        LockList {
            transactions: self
                .open
                .values()
                .map(|t| TxLocks {
                    tx: t.id,
                    kind: t.kind,
                    label: t.label.clone(),
                    actor: t.actor.clone(),
                    mine: Some(t.conn) == conn,
                    items: t.locks.items(),
                    opened_at: t.opened_at,
                    last_activity: t.last_activity,
                    expires_at: t.expires_at(self.gesture_timeout),
                    conflicted: t.conflicted,
                })
                .collect(),
            rev: self.rev,
            committed_rev: self.committed_rev,
        }
    }

    /// Whether `conn` was closed ([`close_connection`](Self::close_connection)); it cannot open
    /// transactions any more. (The last 1024 closed connections are remembered: the API layer
    /// never reuses a connection id.)
    #[must_use]
    pub fn is_closed(&self, conn: ConnId) -> bool {
        self.closed_conns.contains(conn)
    }

    // ----- soft locks (WP-1B.09) -------------------------------------------------------------

    /// The `retry_after_ms` hint for a request blocked by `t`: the time left until a gesture's
    /// idle deadline (at least [`SOFT_LOCK_RETRY_MS`]); none for a proposal, which waits for a
    /// person; [`SOFT_LOCK_RETRY_MS`] for a batch.
    fn retry_after(&self, t: &OpenTx) -> Option<u64> {
        match t.kind {
            TxKind::Gesture => {
                let deadline = t.last_activity.saturating_add(self.gesture_timeout);
                let left = deadline.saturating_since(self.clock.now()).as_millis();
                Some(
                    u64::try_from(left)
                        .unwrap_or(u64::MAX)
                        .max(SOFT_LOCK_RETRY_MS),
                )
            }
            TxKind::Proposal => None,
            TxKind::Batch => Some(SOFT_LOCK_RETRY_MS),
        }
    }

    /// `Conflict{item_in_gesture, holder, retry_after_ms}`: `what` is being edited in `t`
    /// (WP-2A.12: `holder` names `t`'s actor).
    fn in_use(&self, t: &OpenTx, what: &str) -> CmdError {
        let retry_after_ms = self.retry_after(t);
        let when = match (t.kind, retry_after_ms) {
            (TxKind::Proposal, _) => "retry once it is accepted or rejected".to_owned(),
            (_, Some(ms)) => format!("retry in {ms} ms or once it closes"),
            (_, None) => "retry once it closes".to_owned(),
        };
        CmdError::Conflict {
            reason: ConflictReason::ItemInGesture,
            current_rev: None,
            stale: None,
            retry_after_ms,
            holder: Some(t.actor.clone()),
            detail: format!(
                "{what} is being edited in open {} {} by {} (\"{}\"); {when}; `edit.locks` \
                 lists who holds what",
                t.kind, t.id, t.actor, t.label
            ),
        }
    }

    /// `Conflict{item_in_gesture}` for an item soft-locked by `t`.
    fn item_locked(&self, t: &OpenTx, item: ItemId) -> CmdError {
        self.in_use(t, &format!("item `{item}`"))
    }

    /// `Conflict{item_in_gesture}` for a change that collides with `t`'s changes at `path`
    /// (the field-level backstop: `t`'s ops no longer apply on top of it).
    fn collides(&self, t: &OpenTx, path: &DocPath) -> CmdError {
        self.in_use(t, &format!("`{}`", path.as_str()))
    }

    /// The soft-lock check every change runs before anything is applied (WP-1B.09, WP-2A.12):
    /// `Conflict{item_in_gesture, holder, retry_after_ms}` if `patch` touches an item
    /// soft-locked by an open gesture or proposal of an actor other than `actor` (the holder's
    /// own actor is never blocked). Read-only; public for the lock-check budget benchmark
    /// (< 20 µs per command on a 5,000-item document).
    pub fn check_locks(&self, actor: &Actor, patch: &Patch) -> Result<(), CmdError> {
        for t in self.open.values() {
            if t.actor == *actor || t.locks.is_empty() {
                continue;
            }
            if let Some(item) = t.locks.touched_by(patch) {
                return Err(self.item_locked(t, item));
            }
        }
        Ok(())
    }

    /// Maps `NotFound` for an item that only exists in another actor's open gesture or
    /// proposal (it created the item) to the soft-lock conflict that explains it.
    fn explain_missing(&self, actor: &Actor, e: CmdError) -> CmdError {
        if let CmdError::NotFound {
            kind: ResourceKind::Item,
            id,
        } = &e
            && let Ok(item) = id.parse::<ItemId>()
            && let Some(t) = self
                .open
                .values()
                .find(|t| t.actor != *actor && t.locks.contains(item))
        {
            return self.item_locked(t, item);
        }
        e
    }

    // ----- publication (for the host) -------------------------------------------------------

    /// The events produced since the last call, in order. The host broadcasts them after
    /// publishing the new snapshots.
    pub fn take_events(&mut self) -> Vec<DocEvent> {
        std::mem::take(&mut self.outbox)
    }

    /// What changed since the last call, so the host knows which snapshots to publish.
    pub fn take_changes(&mut self) -> Changes {
        std::mem::take(&mut self.changes)
    }

    // ----- requests --------------------------------------------------------------------------

    /// The `expected_rev` check (WP-2I.04): `Conflict{reason: stale_rev, current_rev,
    /// expected_rev, changed_since}` unless the live document is at `expected`. The conflict
    /// names the items the revisions in between touched, as far as the engine remembers them
    /// (the last 200 changes; `unknown` beyond that).
    ///
    /// # Errors
    /// The conflict.
    pub fn check_expected_rev(&self, expected: Rev) -> Result<(), CmdError> {
        if expected == self.rev {
            return Ok(());
        }
        Err(CmdError::stale_rev(
            expected,
            self.rev,
            self.revlog.since(expected, self.rev),
        ))
    }

    /// Runs a request: dedup by `request_id`, `expected_rev` check, then the command in an
    /// implicit transaction, in the open transaction named by `tx`, or (engine commands)
    /// against this state. Failures change nothing; `Conflict` errors carry the current
    /// revision.
    pub fn submit(&mut self, submit: Submit) -> Result<Applied, CmdError> {
        self.expire_idle();
        let result = self.submit_inner(submit);
        result.map_err(|e| e.with_rev(self.rev))
    }

    fn submit_inner(&mut self, submit: Submit) -> Result<Applied, CmdError> {
        let Submit {
            ctx,
            name,
            params,
            request_id,
            expected_rev,
            tx,
            replace,
        } = submit;
        if replace && tx.is_none() {
            return Err(CmdError::invalid(
                "/_meta/replace",
                "needs `_meta.tx`: only an update inside an open gesture replaces the earlier ones",
            ));
        }
        let at = self.clock.now();
        let request_id = request_id.unwrap_or_else(|| RequestId::generate(&mut *self.request_ids));
        let registry = Arc::clone(&self.registry);
        let entry = registry
            .get(&name)
            .ok_or_else(|| CmdError::not_found(ResourceKind::Command, &name))?;
        ctx.access().check(&entry.meta)?;
        let params = if params.is_null() {
            Value::Object(Map::new())
        } else {
            params
        };
        let journal_params = entry.redact(&params);
        let fingerprint = Fingerprint::of(ctx.actor(), &name, &journal_params);
        if let Some(applied) = self.replay(request_id, fingerprint, at)? {
            return Ok(applied);
        }
        if let Some(expected) = expected_rev {
            self.check_expected_rev(expected)?;
        }
        let call = Call {
            ctx,
            name,
            journal_params,
            request_id,
            fingerprint,
            at,
            secret: !entry.meta.secret_fields.is_empty(),
            undoable: entry.meta.undoable,
            read_only: entry.meta.annotations.read_only,
        };
        match (entry.kind(), tx) {
            (EntryKind::Engine, None) => self.run_engine(entry.engine_context(), &call, params),
            (EntryKind::Engine, Some(_)) => Err(CmdError::unsupported(format!(
                "`{}` acts on engine state and cannot run inside a transaction",
                call.name
            ))),
            (EntryKind::Command | EntryKind::Query, None) => self.run_implicit(&call, params),
            (EntryKind::Command | EntryKind::Query, Some(tx)) => {
                self.run_in_tx(tx, &call, params, replace)
            }
        }
    }

    /// The remembered result for a repeated request, `Conflict` if the id was used for a
    /// different request, `None` for a new one.
    fn replay(
        &mut self,
        id: RequestId,
        fingerprint: Fingerprint,
        now: WallTime,
    ) -> Result<Option<Applied>, CmdError> {
        let window = self.dedup.window();
        let found = match self.dedup.lookup(id, now) {
            Some(record) => Some(record.clone()),
            // Requests inside open transactions are remembered there until they commit.
            None => self
                .open
                .values()
                .flat_map(|t| &t.steps)
                .map(|step| &step.record)
                .find(|r| r.request_id() == id && r.is_live(now, window))
                .cloned(),
        };
        match found {
            None => Ok(None),
            Some(record) if record.fingerprint == fingerprint => Ok(Some(Applied {
                replayed: true,
                ..record.applied
            })),
            Some(_) => Err(CmdError::conflict(
                ConflictReason::RequestIdReused,
                format!("request id {id} was already used for a different request; use a new one"),
            )),
        }
    }

    /// Runs the command on `base` in a scratch transaction. A panic is caught: the scratch
    /// transaction is dropped (nothing applied) and `CommandPanicked` is emitted.
    fn execute(
        &mut self,
        call: &Call,
        params: Value,
        base: Project,
    ) -> Result<(Committed, Value), CmdError> {
        let registry = Arc::clone(&self.registry);
        let ids: &mut (dyn IdGen + Send) = &mut *self.ids;
        let access = call.ctx.access();
        let result = panic::catch_unwind(AssertUnwindSafe(move || {
            let mut txn = Transaction::begin(base);
            let output = registry.invoke_json(&call.name, params, &mut txn.tx(ids, access))?;
            Ok((txn.commit(), output))
        }));
        match result {
            Ok(result) => result,
            Err(payload) => Err(self.panicked(call, payload.as_ref())),
        }
    }

    fn panicked(&mut self, call: &Call, payload: &(dyn Any + Send)) -> CmdError {
        let message = if call.secret {
            "(withheld: the command has secret params)".to_owned()
        } else {
            panic_message(payload)
        };
        tracing::error!(
            command = %call.name,
            actor = %call.ctx.actor(),
            request_id = %call.request_id,
            %message,
            "command panicked; its changes were rolled back"
        );
        self.outbox.push(DocEvent::CommandPanicked(CommandPanicked {
            command: call.name.clone(),
            actor: call.ctx.actor().clone(),
            request_id: call.request_id,
            rev: self.rev,
            message: message.clone(),
        }));
        CmdError::internal(format!(
            "`{}` panicked and nothing was applied: {message}",
            call.name
        ))
    }

    fn alloc_tx(&mut self) -> TxId {
        let id = TxId::new(self.next_tx);
        self.next_tx += 1;
        id
    }

    fn applied(&self, call: &Call, tx_id: Option<TxId>, output: Value, ops: Patch) -> Applied {
        Applied {
            request_id: call.request_id,
            rev: self.rev,
            committed_rev: self.committed_rev,
            tx_id,
            output,
            ops,
            replayed: false,
        }
    }

    fn record(call: &Call, applied: &Applied) -> DedupRecord {
        DedupRecord {
            fingerprint: call.fingerprint,
            at: call.at,
            applied: applied.clone(),
        }
    }

    fn emit(
        &mut self,
        prev_rev: Rev,
        actor: Actor,
        tx_id: TxId,
        cause: PatchCause,
        request_id: Option<RequestId>,
        delta: LiveDelta,
    ) {
        self.revlog.record(
            self.rev,
            match &delta {
                LiveDelta::Ops(ops) => Some(ops),
                LiveDelta::Reset => None,
            },
        );
        let event = match delta {
            LiveDelta::Ops(ops) => DocEvent::Patched(DocPatched {
                rev: self.rev,
                prev_rev,
                committed_rev: self.committed_rev,
                actor,
                tx_id,
                cause,
                request_id,
                ops,
            }),
            LiveDelta::Reset => {
                tracing::warn!(rev = %self.rev, "live document re-composed; subscribers must resync");
                DocEvent::Reset(DocReset {
                    rev: self.rev,
                    prev_rev,
                    committed_rev: self.committed_rev,
                })
            }
        };
        self.outbox.push(event);
    }

    fn current_request(&self) -> Option<RequestId> {
        self.current.as_ref().map(|call| call.request_id)
    }

    /// Computes the effects of committing `committed` (reached from the committed document by
    /// `patch`) as an implicit transaction by `actor`: refused if it touches an item another
    /// actor's gesture or proposal soft-locks; then every open transaction is rebased onto it
    /// (one whose changes no longer apply refuses it too: the field-level backstop, which also
    /// covers batches and the lock holder's own actor) and the live document is recomposed.
    fn stage(&self, actor: &Actor, committed: Project, patch: Patch) -> Result<Staged, CmdError> {
        self.check_locks(actor, &patch)?;
        let mut views = Vec::with_capacity(self.open.len());
        for t in self.open.values().filter(|t| !t.conflicted) {
            let view = if t.ops.is_empty() {
                committed.clone()
            } else {
                t.ops
                    .apply(&committed)
                    .map_err(|e| self.collides(t, e.path()))?
            };
            views.push((t.id, view));
        }
        let layers: Vec<Layer<'_>> = views
            .iter()
            .filter_map(|(id, view)| {
                let t = self.open.get(id)?;
                t.contributes_live().then_some(Layer {
                    id: *id,
                    ops: &t.ops,
                    view,
                })
            })
            .collect();
        let live_pending = !layers.is_empty();
        let live = compose(&committed, &layers).map_err(|(id, e)| match self.open.get(&id) {
            Some(t) => self.collides(t, e.path()),
            None => CmdError::internal("lost an open transaction"),
        })?;
        drop(layers);
        let delta = if live_pending {
            LiveDelta::between(&self.live, &live)
        } else {
            LiveDelta::Ops(patch.clone())
        };
        let committed_rev = self.rev.next();
        let rev = if live_pending {
            committed_rev.next()
        } else {
            committed_rev
        };
        Ok(Staged {
            committed,
            patch,
            views,
            live,
            delta,
            committed_rev,
            rev,
        })
    }

    /// Journals a staged commit, then applies it (the commit point), broadcasts it and logs it
    /// in the history. A journal error changes nothing.
    fn finalize(&mut self, staged: Staged, meta: CommitMeta<'_>) -> Result<(), CmdError> {
        let Staged {
            committed,
            patch,
            views,
            live,
            delta,
            committed_rev,
            rev,
        } = staged;
        self.journal.commit(&CommitRecord {
            rev: committed_rev,
            prev_committed_rev: self.committed_rev,
            tx_id: meta.tx_id,
            kind: None,
            actor: meta.actor,
            label: (!meta.implicit).then_some(meta.label.as_str()),
            at: meta.at,
            steps: meta.steps,
            patch: &patch,
            cause: meta.entry,
            undoable: meta.undoable,
        })?;

        // Commit point: nothing below fails.
        for (id, view) in views {
            if let Some(t) = self.open.get_mut(&id) {
                t.view = view;
                t.rebased |= !t.ops.is_empty();
            }
        }
        let prev_rev = self.rev;
        let prev_committed_rev = self.committed_rev;
        self.committed = committed;
        self.committed_rev = committed_rev;
        self.live = live;
        self.rev = rev;
        self.emit(
            prev_rev,
            meta.actor.clone(),
            meta.tx_id,
            meta.cause,
            meta.request,
            delta,
        );
        self.changes = Changes::BOTH;
        let record = NewRecord {
            tx_id: meta.tx_id,
            actor: meta.actor.clone(),
            label: meta.label,
            kind: None,
            cause: meta.entry,
            prev_rev: prev_committed_rev,
            rev: committed_rev,
            at: meta.at,
            patch: Arc::new(patch),
            undoable: meta.undoable,
        };
        self.log_commit(meta.history, record);
        Ok(())
    }

    /// Logs a commit in the history and queues the `edit.history_changed` events.
    fn log_commit(&mut self, history: HistoryMove, record: NewRecord) {
        let events = match history {
            HistoryMove::Change => self.history.push_change(record),
            HistoryMove::Undo(seq) => self.history.push_undo(seq, record),
            HistoryMove::Redo(seq) => self.history.push_redo(seq, record),
        };
        self.outbox
            .extend(events.into_iter().map(DocEvent::History));
    }

    /// An implicit single-command transaction, run against the committed document.
    fn run_implicit(&mut self, call: &Call, params: Value) -> Result<Applied, CmdError> {
        let (done, output) = self
            .execute(call, params, self.committed.clone())
            .map_err(|e| self.explain_missing(call.ctx.actor(), e))?;
        let tx_id = self.alloc_tx();
        if done.is_empty() {
            let applied = self.applied(call, Some(tx_id), output, Patch::new());
            self.dedup.insert(Self::record(call, &applied));
            return Ok(applied);
        }
        let Committed {
            doc: committed,
            patch,
            ..
        } = done;
        let staged = self.stage(call.ctx.actor(), committed, patch)?;
        let applied = Applied {
            request_id: call.request_id,
            rev: staged.rev,
            committed_rev: staged.committed_rev,
            tx_id: Some(tx_id),
            output,
            ops: staged.patch.clone(),
            replayed: false,
        };
        let record = Self::record(call, &applied);
        let steps = [JournalStep {
            command: call.name.clone(),
            params: call.journal_params.clone(),
            record: record.clone(),
        }];
        self.finalize(
            staged,
            CommitMeta {
                tx_id,
                actor: call.ctx.actor(),
                label: call.name.clone(),
                implicit: true,
                at: call.at,
                steps: &steps,
                cause: PatchCause::Command,
                entry: EntryCause::Edit,
                undoable: call.undoable,
                request: Some(call.request_id),
                history: HistoryMove::Change,
            },
        )?;
        self.dedup.insert(record);
        Ok(applied)
    }

    /// A command inside the caller's open transaction `tx_id`, run against its view; with
    /// `replace` (a gesture update with `_meta.replace`, spec INV-301/302), run against the
    /// committed document instead, its patch replacing the gesture's ops atomically.
    // One sequence (checks, execute, compose, publish); splitting it would scatter the order
    // in which a failed update must leave everything as it was.
    #[allow(clippy::too_many_lines)]
    fn run_in_tx(
        &mut self,
        tx_id: TxId,
        call: &Call,
        params: Value,
        replace: bool,
    ) -> Result<Applied, CmdError> {
        let t = self
            .open
            .get_mut(&tx_id)
            .ok_or_else(|| tx_not_found(tx_id))?;
        if t.conn != call.ctx.conn() {
            return Err(not_owner(t));
        }
        // Any request naming it keeps a gesture alive, even one that fails.
        t.last_activity = t.last_activity.max(call.at);
        if replace && t.kind != TxKind::Gesture {
            return Err(CmdError::invalid(
                "/_meta/replace",
                format!(
                    "transaction {} is a {}: only the updates of a gesture replace each other",
                    t.id, t.kind
                ),
            ));
        }
        if t.conflicted {
            return Err(conflicted(t));
        }
        let kind = t.kind;
        let own_view = (!replace).then(|| t.view.clone());
        let base = own_view.unwrap_or_else(|| self.committed.clone());
        let (done, output) = self
            .execute(call, params, base)
            .map_err(|e| self.explain_missing(call.ctx.actor(), e))?;
        // A replacing update that ends where the gesture already is (or an accumulating one
        // that changes nothing) changes nothing visible.
        let unchanged = if replace {
            self.open.get(&tx_id).is_some_and(|t| t.ops == done.patch)
        } else {
            done.is_empty()
        };
        if unchanged {
            let applied = self.applied(call, Some(tx_id), output, done.patch);
            self.add_step(tx_id, call, &applied, None);
            return Ok(applied);
        }
        let Committed {
            doc: view,
            patch: step,
            ..
        } = done;
        // Fail fast on another actor's soft lock, before anything is applied. A replacing
        // update's rollback only touches this gesture's own (locked) items.
        self.check_locks(call.ctx.actor(), &step)?;
        if !kind.is_live() {
            // A batch: nothing visible until it commits.
            let applied = self.applied(call, Some(tx_id), output, step.clone());
            self.add_step(tx_id, call, &applied, Some((step, view, false)));
            return Ok(applied);
        }

        let others_live = self
            .open
            .values()
            .any(|o| o.id != tx_id && o.contributes_live());
        let (live, delta) = if others_live {
            let ops = if replace {
                step.clone()
            } else {
                let mut ops = self.open.get(&tx_id).map_or_default(|t| t.ops.clone());
                ops.extend(step.iter().cloned());
                ops
            };
            let layers: Vec<Layer<'_>> = self
                .open
                .values()
                .filter(|o| o.id == tx_id || o.contributes_live())
                .map(|o| {
                    if o.id == tx_id {
                        Layer {
                            id: o.id,
                            ops: &ops,
                            view: &view,
                        }
                    } else {
                        Layer {
                            id: o.id,
                            ops: &o.ops,
                            view: &o.view,
                        }
                    }
                })
                .collect();
            let live = compose(&self.committed, &layers).map_err(|(failed, e)| {
                // Name the other transaction this step collides with.
                let other = if failed == tx_id {
                    self.open
                        .values()
                        .find(|o| o.id != tx_id && o.contributes_live())
                } else {
                    self.open.get(&failed)
                };
                other.map_or_else(
                    || CmdError::internal("lost an open transaction"),
                    |o| self.collides(o, e.path()),
                )
            })?;
            let delta = LiveDelta::between(&self.live, &live);
            (live, delta)
        } else if replace {
            // The live document is this gesture's view before the update, and `view` the one
            // after it. Subscribers get what differs between the two: for a drag that carries
            // a clip one place further along a track that ripples, the clip and the one it
            // passed, not every shifted clip put back and shifted again (issue #231). A change
            // that has no field-level form is sent as before: the gesture's ops undone, then
            // the update's.
            let delta = diff(&self.live, &view).unwrap_or_else(|_| {
                let mut delta = self.open.get(&tx_id).map_or_default(|t| t.ops.invert());
                delta.extend(step.iter().cloned());
                delta
            });
            (view.clone(), LiveDelta::Ops(delta))
        } else {
            (view.clone(), LiveDelta::Ops(step.clone()))
        };

        let prev_rev = self.rev;
        self.rev = self.rev.next();
        let applied = self.applied(call, Some(tx_id), output, step.clone());
        let locked_more = self.add_step(tx_id, call, &applied, Some((step, view, replace)));
        self.live = live;
        self.emit(
            prev_rev,
            call.ctx.actor().clone(),
            tx_id,
            PatchCause::Live,
            Some(call.request_id),
            delta,
        );
        if locked_more && let Some(t) = self.open.get(&tx_id) {
            // After the `doc.patched` that touched them (new items exist in mirrors by then).
            self.outbox.push(DocEvent::TxLocked(t.activity(self.rev)));
        }
        self.changes.live = true;
        Ok(applied)
    }

    /// Records a step in open transaction `tx_id`: its patch and the view after it, and
    /// whether it replaces the earlier steps' changes (`_meta.replace`) or adds to them. A
    /// gesture or proposal soft-locks what it touched; the lock set only grows, also when a
    /// step replaces the earlier ones (spec INV-304). Returns whether that locked items not
    /// locked before.
    fn add_step(
        &mut self,
        tx_id: TxId,
        call: &Call,
        applied: &Applied,
        change: Option<(Patch, Project, bool)>,
    ) -> bool {
        let record = Self::record(call, applied);
        let mut locked_more = false;
        if let Some(t) = self.open.get_mut(&tx_id) {
            if let Some((step, view, replace)) = change {
                if t.kind.is_live() {
                    locked_more = t.locks.extend(&step) > 0;
                }
                if replace {
                    // Built on the committed document as it is now: nothing left to rebase.
                    t.ops = step;
                    t.rebased = false;
                    t.replaced = true;
                    t.undoable = call.undoable;
                } else {
                    t.ops.extend(step);
                    t.undoable &= call.undoable;
                }
                t.view = view;
            }
            t.last_activity = t.last_activity.max(call.at);
            t.steps.push(JournalStep {
                command: call.name.clone(),
                params: call.journal_params.clone(),
                record,
            });
        }
        locked_more
    }

    /// Runs an engine command (`edit.*`) with this state as its context.
    fn run_engine(
        &mut self,
        context: Option<TypeId>,
        call: &Call,
        params: Value,
    ) -> Result<Applied, CmdError> {
        if context != Some(TypeId::of::<DocState>()) {
            return Err(CmdError::unsupported(format!(
                "`{}` does not run on the document actor",
                call.name
            )));
        }
        let registry = Arc::clone(&self.registry);
        let access = call.ctx.access();
        // Restored afterwards, so an engine command may submit further requests (even engine
        // commands) and still act for its own caller.
        let outer = self.current.replace(call.clone());
        let outer_claim = self.claimed.take();
        let outer_closing = self.closing.take();
        let start = self.outbox.len();
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            registry.invoke_engine_json(&call.name, params, &mut *self, access)
        }));
        self.current = outer;
        let claimed = std::mem::replace(&mut self.claimed, outer_claim);
        let closing = std::mem::replace(&mut self.closing, outer_closing);
        let output = match result {
            Ok(result) => result?,
            Err(payload) => return Err(self.panicked(call, payload.as_ref())),
        };
        let applied = if let Some(applied) = claimed {
            // The command committed (undo, redo, restore) and journaled this result.
            debug_assert_eq!(applied.output, output, "journaled output differs");
            applied
        } else {
            let ops: Patch = self
                .outbox
                .get(start..)
                .unwrap_or_default()
                .iter()
                .filter_map(DocEvent::as_patched)
                .flat_map(|p| p.ops.iter().cloned())
                .collect();
            self.applied(call, None, output, ops)
        };
        if !call.read_only {
            let record = Self::record(call, &applied);
            if let Some(closing) = closing {
                self.journal_close(closing, call, &record);
            }
            self.dedup.insert(record);
        }
        Ok(applied)
    }

    /// The journal step and result of the current request for an engine command whose commit
    /// produces `output` (none when called directly, not through [`submit`](Self::submit)).
    fn engine_step<T: Serialize>(
        &self,
        output: &T,
        tx_id: TxId,
        staged: &Staged,
    ) -> Result<(Vec<JournalStep>, Option<Applied>), CmdError> {
        let Some(call) = &self.current else {
            return Ok((Vec::new(), None));
        };
        let applied = Applied {
            request_id: call.request_id,
            rev: staged.rev,
            committed_rev: staged.committed_rev,
            tx_id: Some(tx_id),
            output: encode_output(output)?,
            ops: staged.patch.clone(),
            replayed: false,
        };
        let step = JournalStep {
            command: call.name.clone(),
            params: call.journal_params.clone(),
            record: Self::record(call, &applied),
        };
        Ok((vec![step], Some(applied)))
    }

    /// When the current request was made (now, for a direct call).
    fn request_time(&self) -> WallTime {
        self.current
            .as_ref()
            .map_or_else(|| self.clock.now(), |call| call.at)
    }

    // ----- open transactions (the seam for WP-1B.04 / WP-1B.09) -----------------------------

    /// Opens a transaction owned by `ctx`'s connection (one per connection; a closed
    /// connection cannot open one). Nothing changes until commands run in it
    /// ([`Submit::in_tx`]); a gesture or proposal soft-locks the items they touch for other
    /// actors until it closes. Emits `tx.opened`.
    pub fn begin_tx(
        &mut self,
        ctx: &ActorCtx,
        kind: TxKind,
        label: impl Into<String>,
    ) -> Result<TxInfo, CmdError> {
        self.expire_idle();
        if self.closed_conns.contains(ctx.conn()) {
            // A request that raced its connection's close: nobody could ever close it.
            return Err(CmdError::invalid(
                "",
                format!(
                    "connection {} is closed; reconnect to open a transaction",
                    ctx.conn()
                ),
            ));
        }
        if let Some(t) = self.open.values().find(|t| t.conn == ctx.conn()) {
            return Err(CmdError::invalid(
                "",
                format!(
                    "this connection already has open transaction {}; commit or cancel it first",
                    t.id
                ),
            ));
        }
        self.reserve_tx_ids()?;
        let id = self.alloc_tx();
        let now = self.clock.now();
        let t = OpenTx {
            id,
            kind,
            label: label.into(),
            conn: ctx.conn(),
            actor: ctx.actor().clone(),
            opened_rev: self.rev,
            opened_at: now,
            last_activity: now,
            ops: Patch::new(),
            view: self.committed.clone(),
            steps: Vec::new(),
            begun: None,
            conflicted: false,
            undoable: true,
            rebased: false,
            locks: ItemLocks::default(),
            replaced: false,
        };
        let info = t.info(self.gesture_timeout);
        let (mut begun, claim) = self.marker_step(&TxOpened::of(&info))?;
        let t = OpenTx {
            begun: begun.pop(),
            ..t
        };
        self.outbox.push(DocEvent::TxOpened(t.activity(self.rev)));
        self.open.insert(id, t);
        self.claimed = claim;
        Ok(info)
    }

    /// Queues `tx.closed` for `t`, which just ended (after its `doc.patched`, if any).
    fn push_closed(&mut self, t: &OpenTx, outcome: TxOutcome) {
        self.outbox.push(DocEvent::TxClosed(TxEnded {
            tx_id: t.id,
            kind: t.kind,
            actor: t.actor.clone(),
            outcome,
            items: t.locks.items(),
            rev: self.rev,
            committed_rev: self.committed_rev,
        }));
    }

    fn owned(&self, ctx: &ActorCtx, id: TxId) -> Result<&OpenTx, CmdError> {
        let t = self.open.get(&id).ok_or_else(|| tx_not_found(id))?;
        if t.conn == ctx.conn() {
            Ok(t)
        } else {
            Err(not_owner(t))
        }
    }

    /// Commits `ctx`'s open transaction `id`: its changes become one committed transaction
    /// (journaled first). Another open transaction whose changes no longer apply on top
    /// becomes conflicted (first committer wins).
    pub fn commit_tx(&mut self, ctx: &ActorCtx, id: TxId) -> Result<TxClosed, CmdError> {
        self.expire_idle();
        let result = self.commit_inner(ctx, id);
        result.map_err(|e| e.with_rev(self.rev))
    }

    // One commit sequence (checks, journal, commit point, events, history); splitting it would
    // scatter the commit point.
    #[allow(clippy::too_many_lines)]
    fn commit_inner(&mut self, ctx: &ActorCtx, id: TxId) -> Result<TxClosed, CmdError> {
        self.owned(ctx, id)?;
        // A field borrow (not `owned`), so the journal can be borrowed mutably below.
        let Some(t) = self.open.get(&id) else {
            return Err(tx_not_found(id));
        };
        if t.conflicted {
            let err = conflicted(t);
            if let Some(t) = self.open.remove(&id) {
                self.push_closed(&t, TxOutcome::Conflicted);
            }
            return Err(err);
        }
        if t.ops.is_empty() {
            let kind = t.kind;
            if let Some(mut t) = self.open.remove(&id) {
                self.push_closed(&t, TxOutcome::Committed);
                for step in &t.steps {
                    self.dedup.insert(step.record.clone());
                }
                // Its commands and the request that opened it, then (once its result is
                // known) this one: what the dedup table holds, for the journal's marker.
                t.seal(None);
                self.closes(id, kind, TxOutcome::Committed, t.steps);
            }
            return Ok(TxClosed {
                tx_id: id,
                kind,
                outcome: TxOutcome::Committed,
                rev: self.rev,
                committed_rev: self.committed_rev,
                patch: Patch::new(),
            });
        }
        // A batch that touched an item before another actor's gesture or proposal locked it
        // waits (stays open) until that one closes.
        self.check_locks(&t.actor, &t.ops)?;
        if t.rebased {
            self.check_rebased(t)?;
        }
        let now = self.clock.now();
        let committed = t.view.clone();
        let net = diff(&self.committed, &committed).unwrap_or_else(|_| t.ops.clone());
        let others_live_before = self
            .open
            .values()
            .any(|o| o.id != id && o.contributes_live());

        let Rebased {
            views,
            losers,
            live,
            live_pending,
        } = self.rebase_others(id, &committed);
        let delta = if others_live_before {
            LiveDelta::between(&self.live, &live)
        } else if t.kind.is_live() {
            // The live document already showed it.
            LiveDelta::Ops(Patch::new())
        } else {
            LiveDelta::Ops(net.clone())
        };
        let committed_rev = self.rev.next();
        let rev = if live_pending {
            committed_rev.next()
        } else {
            committed_rev
        };
        let closed = TxClosed {
            tx_id: id,
            kind: t.kind,
            outcome: TxOutcome::Committed,
            rev,
            committed_rev,
            patch: net,
        };
        // The journal remembers the requests that opened and commit it with its steps, so a
        // repeat of either after a crash gets its result back, as on a live engine (#235).
        let (commit_step, claim) = self.commit_step(&closed, &delta)?;
        let sealed = self.open.get_mut(&id).map_or(0, |t| t.seal(commit_step));
        let Some(t) = self.open.get(&id) else {
            return Err(tx_not_found(id));
        };
        let journaled = self.journal.commit(&CommitRecord {
            rev: committed_rev,
            prev_committed_rev: self.committed_rev,
            tx_id: id,
            kind: Some(t.kind),
            actor: &t.actor,
            label: Some(&t.label),
            at: now,
            steps: &t.steps,
            patch: &closed.patch,
            cause: EntryCause::Edit,
            undoable: t.undoable,
        });
        if let Err(e) = journaled {
            // Still open, as it was: its owner can retry or cancel.
            if let Some(t) = self.open.get_mut(&id) {
                t.unseal(sealed);
            }
            return Err(e);
        }

        // Commit point: nothing below fails.
        let Some(t) = self.open.remove(&id) else {
            return Err(CmdError::internal("lost an open transaction"));
        };
        self.claimed = claim;
        for (vid, view) in views {
            if let Some(o) = self.open.get_mut(&vid) {
                o.view = view;
                o.rebased |= !o.ops.is_empty();
            }
        }
        self.mark_conflicted(&losers);
        let prev_rev = self.rev;
        let prev_committed_rev = self.committed_rev;
        self.committed = committed;
        self.committed_rev = committed_rev;
        self.live = live;
        self.rev = rev;
        let request = self.current_request();
        self.emit(
            prev_rev,
            t.actor.clone(),
            id,
            PatchCause::Commit,
            request,
            delta,
        );
        self.push_closed(&t, TxOutcome::Committed);
        for step in t.steps {
            self.dedup.insert(step.record);
        }
        self.changes = Changes::BOTH;
        // However many commands it ran: one history entry.
        let record = NewRecord {
            tx_id: id,
            actor: t.actor,
            label: t.label,
            kind: Some(t.kind),
            cause: EntryCause::Edit,
            prev_rev: prev_committed_rev,
            rev: committed_rev,
            at: now,
            patch: Arc::new(closed.patch.clone()),
            undoable: t.undoable,
        };
        self.log_commit(HistoryMove::Change, record);
        Ok(closed)
    }

    /// The journal step and result of the current request for an `edit.commit_tx` that commits
    /// `closed` and shows subscribers `delta` (none when called directly, not through
    /// [`submit`](Self::submit)).
    fn commit_step(
        &self,
        closed: &TxClosed,
        delta: &LiveDelta,
    ) -> Result<(Option<JournalStep>, Option<Applied>), CmdError> {
        let Some(call) = &self.current else {
            return Ok((None, None));
        };
        let applied = Applied {
            request_id: call.request_id,
            rev: closed.rev,
            committed_rev: closed.committed_rev,
            tx_id: None,
            output: encode_output(&TxCommitted::of(closed))?,
            ops: match delta {
                LiveDelta::Ops(ops) => ops.clone(),
                LiveDelta::Reset => Patch::new(),
            },
            replayed: false,
        };
        let step = JournalStep {
            command: call.name.clone(),
            params: call.journal_params.clone(),
            record: Self::record(call, &applied),
        };
        Ok((Some(step), Some(applied)))
    }

    /// `Conflict{stale_rev}` if committing `t`, whose view was rebased onto later commits,
    /// would break a document invariant the committed document keeps. Nothing changes: its
    /// owner can still adjust it or cancel it.
    fn check_rebased(&self, t: &OpenTx) -> Result<(), CmdError> {
        let found = t.view.violations();
        if found.is_empty() {
            return Ok(());
        }
        let before = self.committed.violations();
        match found.iter().find(|v| !before.contains(v)) {
            None => Ok(()),
            Some(broken) => Err(CmdError::conflict(
                ConflictReason::StaleRev,
                format!(
                    "transaction {} cannot commit: with the changes committed since it began, the \
                     document would become invalid ({broken}); change it or cancel it",
                    t.id
                ),
            )),
        }
    }

    /// Rebases the open transactions other than `id` onto `committed` (`id` is committing).
    /// First committer wins: one whose ops no longer apply, or no longer compose into the live
    /// document, loses.
    fn rebase_others(&self, id: TxId, committed: &Project) -> Rebased {
        let mut views = Vec::new();
        let mut losers = Vec::new();
        for o in self.open.values().filter(|o| o.id != id && !o.conflicted) {
            if o.ops.is_empty() {
                views.push((o.id, committed.clone()));
                continue;
            }
            match o.ops.apply(committed) {
                Ok(view) => views.push((o.id, view)),
                Err(e) => {
                    tracing::warn!(tx = %o.id, committed = %id, path = e.path().as_str(), "open transaction conflicts with a commit; it can only be cancelled");
                    losers.push(o.id);
                }
            }
        }
        let live = self.compose_survivors(committed, &views, &mut losers);
        let live_pending = views.iter().any(|(vid, _)| {
            !losers.contains(vid) && self.open.get(vid).is_some_and(OpenTx::contributes_live)
        });
        Rebased {
            views,
            losers,
            live,
            live_pending,
        }
    }

    /// The live document over `committed` from the open live transactions with these
    /// `views`, leaving out `losers`. One that no longer composes joins `losers`.
    fn compose_survivors(
        &self,
        committed: &Project,
        views: &[(TxId, Project)],
        losers: &mut Vec<TxId>,
    ) -> Project {
        loop {
            let layers: Vec<Layer<'_>> = views
                .iter()
                .filter(|(vid, _)| !losers.contains(vid))
                .filter_map(|(vid, view)| {
                    let o = self.open.get(vid)?;
                    o.contributes_live().then_some(Layer {
                        id: *vid,
                        ops: &o.ops,
                        view,
                    })
                })
                .collect();
            match compose(committed, &layers) {
                Ok(live) => return live,
                Err((failed, e)) => {
                    tracing::warn!(tx = %failed, path = e.path().as_str(), "open transaction no longer composes; it can only be cancelled");
                    losers.push(failed);
                }
            }
        }
    }

    fn mark_conflicted(&mut self, losers: &[TxId]) {
        for loser in losers {
            if let Some(o) = self.open.get_mut(loser) {
                o.conflicted = true;
            }
        }
    }

    /// Cancels `ctx`'s open transaction `id`, rolling back its live changes and releasing its
    /// soft locks. Only its owner may (others get `Locked`).
    pub fn cancel_tx(&mut self, ctx: &ActorCtx, id: TxId) -> Result<TxClosed, CmdError> {
        self.expire_idle();
        let begun = self
            .owned(ctx, id)
            .map_err(|e| e.with_rev(self.rev))?
            .begun
            .clone();
        let closed = self
            .close(id, TxOutcome::Cancelled)
            .ok_or_else(|| tx_not_found(id))?;
        // Its commands are rolled back and forgotten; the request that opened it is not.
        self.closes(
            id,
            closed.kind,
            TxOutcome::Cancelled,
            begun.into_iter().collect(),
        );
        Ok(closed)
    }

    /// Tells the journal that ids up to the next block may be handed out, when the next one is
    /// past the last reservation (ADR-0043). A journal that cannot take it refuses the
    /// transaction: its id could otherwise be issued again after a restart.
    fn reserve_tx_ids(&mut self) -> Result<(), CmdError> {
        if self.next_tx <= self.tx_reserved {
            return Ok(());
        }
        let issued = self.next_tx.saturating_add(TX_IDS_RESERVED - 1);
        self.journal.tx_marker(&TxMarker {
            issued,
            at: self.request_time(),
            closed: None,
            steps: &[],
        })?;
        self.tx_reserved = issued;
        Ok(())
    }

    /// Notes that the running request closed transaction `id` without a committed change;
    /// [`run_engine`](Self::run_engine) writes the marker with the request's result. A direct
    /// call (no request) has nothing to remember.
    fn closes(&mut self, id: TxId, kind: TxKind, outcome: TxOutcome, steps: Vec<JournalStep>) {
        if self.current.is_some() {
            self.closing = Some(Closing {
                closed: ClosedTx {
                    tx_id: id,
                    kind,
                    outcome,
                },
                steps,
            });
        }
    }

    /// Journals the marker of the close `closing` made by the request `call`, whose result is
    /// `record`. Best effort: the close already stands and lost nothing.
    fn journal_close(&mut self, closing: Closing, call: &Call, record: &DedupRecord) {
        let Closing { closed, mut steps } = closing;
        steps.push(JournalStep {
            command: call.name.clone(),
            params: call.journal_params.clone(),
            record: record.clone(),
        });
        let marker = TxMarker {
            issued: self.tx_reserved.max(self.next_tx.saturating_sub(1)),
            at: call.at,
            closed: Some(closed),
            steps: &steps,
        };
        if let Err(e) = self.journal.tx_marker(&marker) {
            tracing::warn!(
                tx = %closed.tx_id,
                error = %e,
                "a closed transaction was not journaled; its close sent again after a restart is refused"
            );
        }
    }

    /// The connection `conn` closed (WebSocket close, MCP session end, stdio proxy exit;
    /// ADR-0017): rolls back its open transaction immediately (`doc.patched` with
    /// `cause: rollback`, then `tx.closed` with `outcome: rolled_back`), releasing its soft
    /// locks, and refuses transactions it would open later (a request that raced the close).
    ///
    /// Call it once the API layer has handed the actor every request it read from that
    /// connection: requests already submitted run first (the inbox is FIFO). Requests that
    /// arrive later still run, but only as implicit transactions. [`ConnId::INTERNAL`] is
    /// never marked closed.
    pub fn close_connection(&mut self, conn: ConnId) -> Vec<TxClosed> {
        self.closed_conns.insert(conn);
        let ids: Vec<TxId> = self
            .open
            .values()
            .filter(|t| t.conn == conn)
            .map(|t| t.id)
            .collect();
        ids.into_iter()
            .filter_map(|id| self.close(id, TxOutcome::RolledBack))
            .collect()
    }

    fn close(&mut self, id: TxId, outcome: TxOutcome) -> Option<TxClosed> {
        let t = self.open.remove(&id)?;
        if t.contributes_live() {
            let others_live = self.open.values().any(OpenTx::contributes_live);
            let (live, delta) = if others_live {
                let views: Vec<(TxId, Project)> = self
                    .open
                    .values()
                    .filter(|o| !o.conflicted)
                    .map(|o| (o.id, o.view.clone()))
                    .collect();
                let mut losers = Vec::new();
                let live = self.compose_survivors(&self.committed, &views, &mut losers);
                self.mark_conflicted(&losers);
                let delta = LiveDelta::between(&self.live, &live);
                (live, delta)
            } else {
                // The live document was exactly this transaction's view.
                (self.committed.clone(), LiveDelta::Ops(t.ops.invert()))
            };
            let prev_rev = self.rev;
            self.rev = self.rev.next();
            self.live = live;
            let request = self.current_request();
            self.emit(
                prev_rev,
                t.actor.clone(),
                id,
                PatchCause::Rollback,
                request,
                delta,
            );
            self.changes.live = true;
        }
        self.push_closed(&t, outcome);
        Some(TxClosed {
            tx_id: id,
            kind: t.kind,
            outcome,
            rev: self.rev,
            committed_rev: self.committed_rev,
            patch: Patch::new(),
        })
    }

    // ----- gesture idle timeout --------------------------------------------------------------

    /// Cancels every gesture no request has named for [`GESTURE_IDLE_TIMEOUT`] (by this
    /// state's [`Clock`]), rolling back its live changes (`doc.patched` with
    /// `cause: rollback`). Runs before every request; the actor also runs it on a timer.
    pub fn expire_idle(&mut self) -> Vec<TxClosed> {
        let now = self.clock.now();
        let timeout = self.gesture_timeout;
        let idle: Vec<TxId> = self
            .open
            .values()
            .filter(|t| {
                t.kind == TxKind::Gesture && now.saturating_since(t.last_activity) >= timeout
            })
            .map(|t| t.id)
            .collect();
        idle.into_iter()
            .filter_map(|id| {
                tracing::info!(tx = %id, "gesture idle for {timeout:?}; cancelled");
                let (actor, items) = self
                    .open
                    .get(&id)
                    .map(|t| (t.actor.clone(), t.locks.items()))?;
                let closed = self.close(id, TxOutcome::Expired)?;
                // After its rollback's `doc.patched` and its `tx.closed` (WP-2A.12).
                self.outbox
                    .push(DocEvent::GestureCancelled(GestureCancelled {
                        tx_id: id,
                        actor,
                        reason: GestureCancelReason::IdleTimeout,
                        items,
                        rev: self.rev,
                        committed_rev: self.committed_rev,
                    }));
                Some(closed)
            })
            .collect()
    }

    /// When the next open gesture expires, if any.
    #[must_use]
    pub fn next_expiry(&self) -> Option<WallTime> {
        self.open
            .values()
            .filter(|t| t.kind == TxKind::Gesture)
            .map(|t| t.last_activity.saturating_add(self.gesture_timeout))
            .min()
    }

    // ----- history (WP-1B.04) ----------------------------------------------------------------

    /// Undoes the newest undoable entry of `scope` (an actor's, or anyone's), for `ctx`: a new
    /// transaction tagged with `ctx`'s actor that applies the entry's inverse patch, if the
    /// per-actor safety rule allows it (module `history`). `Conflict{history_target_modified}`
    /// names the later changes in the way; `Invalid` means there is nothing to undo.
    pub fn undo(&mut self, ctx: &ActorCtx, scope: &HistoryScope) -> Result<HistoryMoved, CmdError> {
        self.undo_steps(ctx, scope, 1)
    }

    /// Redoes the most recently undone entry of `scope` (see [`undo`](Self::undo)).
    pub fn redo(&mut self, ctx: &ActorCtx, scope: &HistoryScope) -> Result<HistoryMoved, CmdError> {
        self.redo_steps(ctx, scope, 1)
    }

    /// [`undo`](Self::undo) up to `steps` times in one turn of the doc actor (nothing else lands
    /// in between). Each step is its own history record. A refused step after the first stops
    /// the run: the result reports `done` and the refusal in `stopped`.
    pub fn undo_steps(
        &mut self,
        ctx: &ActorCtx,
        scope: &HistoryScope,
        steps: u32,
    ) -> Result<HistoryMoved, CmdError> {
        self.move_steps(ctx, scope, Direction::Undo, steps)
    }

    /// [`redo`](Self::redo) up to `steps` times (see [`undo_steps`](Self::undo_steps)).
    pub fn redo_steps(
        &mut self,
        ctx: &ActorCtx,
        scope: &HistoryScope,
        steps: u32,
    ) -> Result<HistoryMoved, CmdError> {
        self.move_steps(ctx, scope, Direction::Redo, steps)
    }

    fn move_steps(
        &mut self,
        ctx: &ActorCtx,
        scope: &HistoryScope,
        dir: Direction,
        steps: u32,
    ) -> Result<HistoryMoved, CmdError> {
        self.expire_idle();
        let mut last = self
            .move_history(ctx, scope, dir)
            .map_err(|e| e.with_rev(self.rev))?;
        for _ in 1..steps {
            match self.move_history(ctx, scope, dir) {
                Ok(mut next) => {
                    next.done = last.done + 1;
                    last = next;
                }
                Err(e) => {
                    last.stopped = Some(e.to_string());
                    break;
                }
            }
        }
        // The request's remembered result is the whole run's (a repeat returns it as is).
        if let Some(claimed) = self.claimed.as_mut() {
            claimed.output = crate::error::encode_output(&last)?;
        }
        Ok(last)
    }

    fn move_history(
        &mut self,
        ctx: &ActorCtx,
        scope: &HistoryScope,
        dir: Direction,
    ) -> Result<HistoryMoved, CmdError> {
        let seq = match dir {
            Direction::Undo => self.history.next_undo(scope),
            Direction::Redo => self.history.next_redo(scope),
        }
        .ok_or_else(|| {
            CmdError::invalid(
                "",
                format!(
                    "nothing to {} for {scope}: no change of theirs is on the {} stack",
                    dir.verb(),
                    dir.verb()
                ),
            )
        })?;
        let target = self
            .history
            .get(seq)
            .cloned()
            .ok_or_else(|| CmdError::internal("lost a history entry"))?;
        let patch = match dir {
            Direction::Undo => target.patch.invert(),
            Direction::Redo => (*target.patch).clone(),
        };
        let committed = patch
            .apply(&self.committed)
            .map_err(|e| self.history_conflict(dir, &target, &e))?;
        // Nothing committed since: the result is exactly a document we had (known valid).
        let exact = match dir {
            Direction::Undo => target.applied_rev == self.committed_rev,
            Direction::Redo => target
                .undone
                .is_some_and(|(rev, _)| rev == self.committed_rev),
        };
        if !exact {
            self.check_still_valid(&committed, dir, &target)?;
        }
        let staged = self.stage(ctx.actor(), committed, patch)?;
        let tx_id = self.alloc_tx();
        let out = HistoryMoved {
            tx: tx_id,
            entry: target.tx_id,
            actor: target.actor.clone(),
            label: target.label.clone(),
            rev: staged.rev,
            committed_rev: staged.committed_rev,
            done: 1,
            stopped: None,
        };
        let (steps, claim) = self.engine_step(&out, tx_id, &staged)?;
        let (cause, entry, history) = match dir {
            Direction::Undo => (
                PatchCause::Undo,
                EntryCause::Undo { of: target.tx_id },
                HistoryMove::Undo(seq),
            ),
            Direction::Redo => (
                PatchCause::Redo,
                EntryCause::Redo { of: target.tx_id },
                HistoryMove::Redo(seq),
            ),
        };
        let at = self.request_time();
        let request = self.current_request();
        self.finalize(
            staged,
            CommitMeta {
                tx_id,
                actor: ctx.actor(),
                label: format!("{} {}", dir.title(), target.label),
                implicit: false,
                at,
                steps: &steps,
                cause,
                entry,
                undoable: false,
                request,
                history,
            },
        )?;
        self.claimed = claim;
        Ok(out)
    }

    /// `Conflict{history_target_modified}` for an undo or redo of `target` whose patch failed
    /// at `e.path()`, naming the later transactions that touched that path.
    fn history_conflict(&self, dir: Direction, target: &Record, e: &PatchError) -> CmdError {
        let after = match dir {
            Direction::Undo => target.applied_seq,
            Direction::Redo => target.undone.map_or(target.seq, |(_, seq)| seq),
        };
        let culprits = self.history.touching(after, e.path(), 3);
        let who = if culprits.is_empty() {
            "a change that is no longer in the history".to_owned()
        } else {
            culprits
                .iter()
                .map(|r| {
                    format!(
                        "{} in {} (\"{}\", rev {})",
                        r.actor, r.tx_id, r.label, r.rev
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let hint = match dir {
            Direction::Undo => "undo those changes first, or edit it by hand",
            Direction::Redo => "make the change again instead",
        };
        CmdError::conflict(
            ConflictReason::HistoryTargetModified,
            format!(
                "cannot {} \"{}\" ({} by {}): `{}` was changed afterwards by {who}; {hint}",
                dir.verb(),
                target.label,
                target.tx_id,
                target.actor,
                e.path().as_str()
            ),
        )
    }

    /// `Conflict{history_target_modified}` if `after` breaks a document invariant the
    /// committed document keeps (a later change depends on what the undo or redo reverts).
    fn check_still_valid(
        &self,
        after: &Project,
        dir: Direction,
        target: &Record,
    ) -> Result<(), CmdError> {
        let found = after.violations();
        if found.is_empty() {
            return Ok(());
        }
        let before = self.committed.violations();
        match found.iter().find(|v| !before.contains(v)) {
            None => Ok(()),
            Some(broken) => Err(CmdError::conflict(
                ConflictReason::HistoryTargetModified,
                format!(
                    "cannot {} \"{}\" ({} by {}): with the changes made since, the document \
                     would become invalid ({broken}); undo those changes first, or edit it by hand",
                    dir.verb(),
                    target.label,
                    target.tx_id,
                    target.actor
                ),
            )),
        }
    }

    /// The undo steps kept per actor (`edit.set_history_limit`, `project.info`).
    #[must_use]
    pub fn history_limit(&self) -> u32 {
        u32::try_from(self.history.undo_depth()).unwrap_or(u32::MAX)
    }

    /// Sets the undo steps kept per actor (clamped to 1..=[`MAX_UNDO_DEPTH`]) and drops each
    /// actor's oldest steps beyond it, announcing `edit.history_changed` (`trimmed`) for every
    /// actor that lost some. The document, its revision and the journal are not touched.
    /// Returns the limit now in force.
    pub fn set_history_limit(&mut self, steps: u32) -> u32 {
        let depth = usize::try_from(steps)
            .unwrap_or(MAX_UNDO_DEPTH)
            .clamp(1, MAX_UNDO_DEPTH);
        let events = self.history.set_undo_depth(depth, self.committed_rev);
        self.outbox
            .extend(events.into_iter().map(DocEvent::History));
        self.history_limit()
    }

    /// A page of the history, newest first: at most `limit` entries (capped at
    /// [`MAX_HISTORY_PAGE`]) before `cursor` (a previous page's `next_cursor`), optionally only
    /// `actor`'s, plus every actor's stacks.
    pub fn history_page(
        &self,
        limit: usize,
        cursor: Option<&str>,
        actor: Option<&Actor>,
    ) -> Result<HistoryPage, CmdError> {
        let before = match cursor {
            None => None,
            Some(c) => Some(
                c.strip_prefix("h_")
                    .and_then(|n| n.parse::<u64>().ok())
                    .ok_or_else(|| {
                        CmdError::invalid("/cursor", "not a cursor from a previous page")
                    })?,
            ),
        };
        let (entries, next) = self
            .history
            .page(limit.min(MAX_HISTORY_PAGE), before, actor);
        Ok(HistoryPage {
            entries,
            next_cursor: next.map(|seq| format!("h_{seq}")),
            stacks: self.history.all_stacks(),
            latest: self.history.latest(),
            rev: self.rev,
            committed_rev: self.committed_rev,
            usage: self.history.usage(),
        })
    }

    /// Captures the committed document (O(1): persistent collections) under an optional
    /// `name` (a name moves from an older checkpoint). At most
    /// [`MAX_CHECKPOINTS`](crate::MAX_CHECKPOINTS), least recently used first out. The
    /// journal records it (a [`CheckpointRecord`], WP-2I.03) before it exists; a journal error
    /// changes nothing.
    pub fn checkpoint(
        &mut self,
        ctx: &ActorCtx,
        name: Option<String>,
        note: Option<String>,
    ) -> Result<CheckpointInfo, CmdError> {
        if let Some(name) = &name {
            let chars = name.chars().count();
            if chars == 0 || chars > MAX_CHECKPOINT_NAME_CHARS {
                return Err(CmdError::invalid(
                    "/name",
                    format!("must be 1..={MAX_CHECKPOINT_NAME_CHARS} characters"),
                ));
            }
            if name.chars().any(char::is_control) || name.trim() != name {
                return Err(CmdError::invalid(
                    "/name",
                    "cannot contain control characters or start or end with whitespace",
                ));
            }
            if CheckpointId::parse(name).is_some() {
                return Err(CmdError::invalid(
                    "/name",
                    "`cp_<n>` is reserved for checkpoint ids",
                ));
            }
        }
        if note
            .as_ref()
            .is_some_and(|n| n.chars().count() > MAX_CHECKPOINT_NOTE_CHARS)
        {
            return Err(CmdError::invalid(
                "/note",
                format!("must be at most {MAX_CHECKPOINT_NOTE_CHARS} characters"),
            ));
        }
        let displaced = self.checkpoints.displaced(name.as_deref());
        let info = CheckpointInfo {
            checkpoint: self.checkpoints.next_id(),
            name,
            note,
            rev: self.committed_rev,
            actor: ctx.actor().clone(),
            at: self.request_time(),
        };
        let (steps, claim) = self.marker_step(&info)?;
        self.journal.checkpoint(&CheckpointRecord {
            rev: self.committed_rev,
            created: Some(&info),
            removed: &displaced,
            issued: info.checkpoint.get(),
            steps: &steps,
            doc: &self.committed,
        })?;
        self.checkpoints.insert(
            info.clone(),
            self.committed.clone(),
            self.history.next_seq(),
            &displaced,
        );
        self.claimed = claim;
        Ok(info)
    }

    /// Every checkpoint, oldest first.
    #[must_use]
    pub fn checkpoints(&self) -> Vec<CheckpointInfo> {
        self.checkpoints.list()
    }

    /// Deletes `checkpoint` (an id `cp_<n>` or a name); the document is not touched. Journaled
    /// first, like [`checkpoint`](Self::checkpoint).
    pub fn delete_checkpoint(&mut self, checkpoint: &str) -> Result<CheckpointInfo, CmdError> {
        let id = self
            .checkpoints
            .resolve(checkpoint)
            .ok_or_else(|| CmdError::not_found(ResourceKind::Checkpoint, checkpoint))?;
        let removed = [id];
        let info = self
            .checkpoints
            .list()
            .into_iter()
            .find(|c| c.checkpoint == id)
            .ok_or_else(|| CmdError::not_found(ResourceKind::Checkpoint, checkpoint))?;
        let (steps, claim) = self.marker_step(&info)?;
        self.journal.checkpoint(&CheckpointRecord {
            rev: self.committed_rev,
            created: None,
            removed: &removed,
            issued: self.checkpoints.issued(),
            steps: &steps,
            doc: &self.committed,
        })?;
        self.checkpoints.remove(id);
        self.claimed = claim;
        Ok(info)
    }

    /// The journal step and result of the current request for an engine command that changes
    /// no document but is journaled (a checkpoint marker), so a restarted engine still
    /// deduplicates it; none when called directly, not through [`submit`](Self::submit).
    fn marker_step<T: Serialize>(
        &self,
        output: &T,
    ) -> Result<(Vec<JournalStep>, Option<Applied>), CmdError> {
        let Some(call) = &self.current else {
            return Ok((Vec::new(), None));
        };
        let applied = Applied {
            request_id: call.request_id,
            rev: self.rev,
            committed_rev: self.committed_rev,
            tx_id: None,
            output: encode_output(output)?,
            ops: Patch::new(),
            replayed: false,
        };
        let step = JournalStep {
            command: call.name.clone(),
            params: call.journal_params.clone(),
            record: Self::record(call, &applied),
        };
        Ok((vec![step], Some(applied)))
    }

    /// Brings the committed document back to `checkpoint` (an id `cp_<n>` or a name) as a new,
    /// undoable transaction by `ctx`'s actor; history stays linear (nothing is truncated).
    /// Without `force`, it refuses (`Conflict{history_target_modified}`) when that would also
    /// revert changes other actors made since the checkpoint (or changes no longer in the
    /// history). A document already equal to the checkpoint is left alone (`tx: None`).
    pub fn restore(
        &mut self,
        ctx: &ActorCtx,
        checkpoint: &str,
        force: bool,
    ) -> Result<Restored, CmdError> {
        self.expire_idle();
        let result = self.restore_inner(ctx, checkpoint, force);
        result.map_err(|e| e.with_rev(self.rev))
    }

    fn restore_inner(
        &mut self,
        ctx: &ActorCtx,
        checkpoint: &str,
        force: bool,
    ) -> Result<Restored, CmdError> {
        let cp = self
            .checkpoints
            .find(checkpoint)
            .ok_or_else(|| CmdError::not_found(ResourceKind::Checkpoint, checkpoint))?;
        let (id, name, target, first_seq) = (
            cp.info.checkpoint,
            cp.info.name.clone(),
            cp.doc.clone(),
            cp.first_seq,
        );
        let patch = diff(&self.committed, &target).map_err(|e| {
            CmdError::unsupported(format!(
                "checkpoint {id} cannot be restored as a field-level change: {e}"
            ))
        })?;
        if patch.is_empty() {
            return Ok(Restored {
                checkpoint: id,
                tx: None,
                rev: self.rev,
                committed_rev: self.committed_rev,
                reverted: Vec::new(),
                reverted_unknown: false,
            });
        }
        let (authors, unknown) = self.history.authors_since(first_seq, &patch);
        let others: Vec<String> = authors
            .iter()
            .filter(|(actor, _)| *actor != ctx.actor())
            .map(|(actor, txs)| {
                let txs: Vec<String> = txs.iter().map(ToString::to_string).collect();
                format!("{actor} ({})", txs.join(", "))
            })
            .collect();
        if !force && (!others.is_empty() || unknown) {
            let mut whose = others.join(", ");
            if unknown {
                if !whose.is_empty() {
                    whose.push_str(" and ");
                }
                whose.push_str(
                    "changes no longer in the history (older ones, or made before the engine \
                     restarted)",
                );
            }
            return Err(CmdError::conflict(
                ConflictReason::HistoryTargetModified,
                format!(
                    "restoring checkpoint {id} would also revert changes by {whose}; pass \
                     `force: true` to restore anyway, or undo your own changes instead"
                ),
            ));
        }
        let staged = self.stage(ctx.actor(), target, patch)?;
        let tx_id = self.alloc_tx();
        let out = Restored {
            checkpoint: id,
            tx: Some(tx_id),
            rev: staged.rev,
            committed_rev: staged.committed_rev,
            reverted: authors
                .into_iter()
                .map(|(actor, txs)| RevertedChanges { actor, txs })
                .collect(),
            reverted_unknown: unknown,
        };
        let (steps, claim) = self.engine_step(&out, tx_id, &staged)?;
        let label = match &name {
            Some(name) => format!("Restore checkpoint \"{name}\""),
            None => format!("Restore checkpoint {id}"),
        };
        let at = self.request_time();
        let request = self.current_request();
        self.finalize(
            staged,
            CommitMeta {
                tx_id,
                actor: ctx.actor(),
                label,
                implicit: false,
                at,
                steps: &steps,
                cause: PatchCause::Restore,
                entry: EntryCause::Restore { checkpoint: id },
                undoable: true,
                request,
                history: HistoryMove::Change,
            },
        )?;
        self.claimed = claim;
        Ok(out)
    }

    // ----- diagnostics -----------------------------------------------------------------------

    /// Checks the model's invariants; expensive, for tests and diagnostics:
    /// every open transaction's view is the committed document with its ops replayed, the live
    /// document is the committed document with the live transactions' ops replayed in order,
    /// `committed_rev <= rev`, and `rev == committed_rev` implies live == committed. Soft
    /// locks (WP-1B.09): each open gesture or proposal locks exactly the items its ops touch, a
    /// batch none, every connection owns at most one open transaction, and no item is locked
    /// by two actors.
    pub fn verify(&self) -> Result<(), String> {
        if self.committed_rev > self.rev {
            return Err(format!(
                "committed_rev {} > rev {}",
                self.committed_rev, self.rev
            ));
        }
        let mut conns = BTreeSet::new();
        let mut holders: BTreeMap<ItemId, &Actor> = BTreeMap::new();
        for t in self.open.values() {
            if !conns.insert(t.conn) {
                return Err(format!("{} owns two open transactions", t.conn));
            }
            let expected = if t.kind.is_live() {
                ItemLocks::of(&t.ops)
            } else {
                ItemLocks::default()
            };
            // Replaced updates keep their items locked (spec INV-304): a superset then.
            let consistent = if t.replaced {
                t.locks.covers(&expected)
            } else {
                t.locks == expected
            };
            if !consistent {
                return Err(format!(
                    "{}: locks {:?} are not the items its ops touch {:?}",
                    t.id,
                    t.locks.items(),
                    expected.items()
                ));
            }
            for item in t.locks.items() {
                if let Some(other) = holders.insert(item, &t.actor)
                    && *other != t.actor
                {
                    return Err(format!(
                        "item {item} is locked by both {other} and {}",
                        t.actor
                    ));
                }
            }
        }
        let mut live = self.committed.clone();
        for t in self.open.values() {
            if t.conflicted {
                continue;
            }
            let view = t
                .ops
                .apply(&self.committed)
                .map_err(|e| format!("{}: ops no longer apply: {e}", t.id))?;
            if view != t.view {
                return Err(format!("{}: view is not committed + ops", t.id));
            }
            if t.contributes_live() {
                live = t
                    .ops
                    .apply(&live)
                    .map_err(|e| format!("{}: live ops no longer compose: {e}", t.id))?;
            }
        }
        if live != self.live {
            return Err("live document is not committed + live transactions".into());
        }
        if self.rev == self.committed_rev && self.live != self.committed {
            return Err(format!(
                "rev == committed_rev == {} but live != committed",
                self.rev
            ));
        }
        self.checkpoints
            .verify(self.history.next_seq())
            .map_err(|e| format!("checkpoints: {e}"))?;
        self.history.verify().map_err(|e| format!("history: {e}"))
    }
}
