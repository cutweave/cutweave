// SPDX-License-Identifier: GPL-3.0-or-later
//! [`DocState`]: the single-writer document state machine (arch §5.2, ADR-0005, ADR-0017),
//! with its configuration ([`DocConfig`]) and journal seam ([`CommitSink`]).

use std::{
    any::{Any, TypeId},
    collections::BTreeMap,
    fmt,
    panic::{self, AssertUnwindSafe},
    sync::Arc,
    time::Duration,
};

use cutweave_model::{IdGen, Project, SeededGen};
use serde::Serialize;
use serde_json::{Map, Value};

use super::{
    clock::{Clock, ManualClock, WallTime},
    dedup::{DEDUP_CAPACITY, DEDUP_WINDOW, DedupRecord, DedupTable, Fingerprint},
    event::{CommandPanicked, DocEvent, DocPatched, DocReset, DocSnapshot, PatchCause},
    identity::{Actor, ActorCtx, ConnId},
    request::{Applied, RequestId, Submit, TxClosed, TxId, TxInfo, TxKind, TxOutcome},
};
use crate::{
    diff::diff,
    error::{CmdError, ConflictReason, ResourceKind, Rev},
    meta::EntryKind,
    patch::{Patch, PatchError},
    registry::Registry,
    tx::{Committed, Transaction},
};

/// Suggested retry delay when a command collides with another connection's open transaction.
pub const SOFT_LOCK_RETRY_MS: u64 = 250;

/// Mixed into the `--deterministic-ids` seed for the request-id stream, so engine-generated
/// request ids never perturb the entity id sequence (CLI and RPC runs then produce identical
/// documents, WP-1B.08).
const REQUEST_ID_STREAM: u64 = 0x5245_5155_4553_5453; // "REQUESTS"

/// Persists committed transactions (the journal, WP-1C.02).
///
/// Called on the actor thread for every committed transaction, **before** the transaction
/// becomes visible or is acknowledged. An `Err` aborts the commit: nothing changes, and the
/// requester gets the error. It must not panic: unlike a command's, a sink's panic stops the
/// actor (durability can no longer be promised).
pub trait CommitSink: Send {
    /// Persists `record`.
    fn commit(&mut self, record: &CommitRecord<'_>) -> Result<(), CmdError>;
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
    /// `None` for an implicit single-command transaction.
    pub kind: Option<TxKind>,
    /// Who committed it.
    pub actor: &'a Actor,
    /// The open transaction's label.
    pub label: Option<&'a str>,
    /// When it committed (wall clock).
    pub at: WallTime,
    /// The requests it contains, in order, each with its redacted params and its dedup
    /// record (so a restarted engine keeps deduplicating them).
    pub steps: &'a [JournalStep],
    /// The net change to the committed document (coalesced, field-level).
    pub patch: &'a Patch,
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
    conflicted: bool,
}

impl OpenTx {
    fn info(&self) -> TxInfo {
        TxInfo {
            id: self.id,
            kind: self.kind,
            label: self.label.clone(),
            conn: self.conn,
            actor: self.actor.clone(),
            opened_rev: self.opened_rev,
            opened_at: self.opened_at,
            last_activity: self.last_activity,
            steps: self.steps.len(),
            conflicted: self.conflicted,
        }
    }

    /// Whether its changes are part of the live document.
    fn contributes_live(&self) -> bool {
        self.kind.is_live() && !self.conflicted && !self.ops.is_empty()
    }
}

/// A request being processed.
struct Call {
    ctx: ActorCtx,
    name: String,
    journal_params: Value,
    request_id: RequestId,
    fingerprint: Fingerprint,
    at: WallTime,
    /// The command has `#[secret]` params: never echo its panic message.
    secret: bool,
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

fn soft_locked(t: &OpenTx, e: &PatchError) -> CmdError {
    CmdError::Conflict {
        reason: ConflictReason::ItemInGesture,
        current_rev: None,
        retry_after_ms: Some(SOFT_LOCK_RETRY_MS),
        detail: format!(
            "`{}` is being edited in open {} transaction {} of {}; retry once it closes",
            e.path().as_str(),
            t.kind,
            t.id,
            t.actor
        ),
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
///   `Conflict{reason: item_in_gesture}`: the minimal soft lock of ADR-0017, enforced by the
///   ops' preimages (WP-1B.09 adds an explicit lock table with better errors). Between open
///   transactions, the first to commit wins; an overlapping one becomes conflicted and can
///   only be cancelled.
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
    next_tx: u64,
    dedup: DedupTable,
    /// The caller and request while an engine command runs.
    current: Option<(ActorCtx, RequestId)>,
    outbox: Vec<DocEvent>,
    changes: Changes,
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
            next_tx: config.first_tx,
            dedup,
            current: None,
            outbox: Vec::new(),
            changes: Changes::default(),
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
        self.current.as_ref().map(|(ctx, _)| ctx)
    }

    /// The open transactions, oldest first.
    #[must_use]
    pub fn transactions(&self) -> Vec<TxInfo> {
        self.open.values().map(OpenTx::info).collect()
    }

    /// An open transaction.
    #[must_use]
    pub fn transaction(&self, id: TxId) -> Option<TxInfo> {
        self.open.get(&id).map(OpenTx::info)
    }

    /// The open transaction owned by `conn`, if any (one per connection).
    #[must_use]
    pub fn transaction_of(&self, conn: ConnId) -> Option<TxInfo> {
        self.open
            .values()
            .find(|t| t.conn == conn)
            .map(OpenTx::info)
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

    /// Runs a request: dedup by `request_id`, `expected_rev` check, then the command in an
    /// implicit transaction, in the open transaction named by `tx`, or (engine commands)
    /// against this state. Failures change nothing; `Conflict` errors carry the current
    /// revision.
    pub fn submit(&mut self, submit: Submit) -> Result<Applied, CmdError> {
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
        } = submit;
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
        if let Some(expected) = expected_rev
            && expected != self.rev
        {
            return Err(CmdError::conflict(
                ConflictReason::StaleRev,
                format!("expected revision {expected}; refresh and retry"),
            ));
        }
        let call = Call {
            ctx,
            name,
            journal_params,
            request_id,
            fingerprint,
            at,
            secret: !entry.meta.secret_fields.is_empty(),
        };
        match (entry.kind(), tx) {
            (EntryKind::Engine, None) => self.run_engine(entry.engine_context(), &call, params),
            (EntryKind::Engine, Some(_)) => Err(CmdError::unsupported(format!(
                "`{}` acts on engine state and cannot run inside a transaction",
                call.name
            ))),
            (EntryKind::Command | EntryKind::Query, None) => self.run_implicit(&call, params),
            (EntryKind::Command | EntryKind::Query, Some(tx)) => self.run_in_tx(tx, &call, params),
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
        self.current.as_ref().map(|(_, id)| *id)
    }

    /// An implicit single-command transaction, run against the committed document.
    fn run_implicit(&mut self, call: &Call, params: Value) -> Result<Applied, CmdError> {
        let (done, output) = self.execute(call, params, self.committed.clone())?;
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

        // Rebase every open transaction; one whose changes overlap refuses the command.
        let mut views = Vec::with_capacity(self.open.len());
        for t in self.open.values().filter(|t| !t.conflicted) {
            let view = if t.ops.is_empty() {
                committed.clone()
            } else {
                t.ops.apply(&committed).map_err(|e| soft_locked(t, &e))?
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
            Some(t) => soft_locked(t, &e),
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
        let applied = Applied {
            request_id: call.request_id,
            rev,
            committed_rev,
            tx_id: Some(tx_id),
            output,
            ops: patch.clone(),
            replayed: false,
        };
        let record = Self::record(call, &applied);
        let steps = [JournalStep {
            command: call.name.clone(),
            params: call.journal_params.clone(),
            record: record.clone(),
        }];
        self.journal.commit(&CommitRecord {
            rev: committed_rev,
            prev_committed_rev: self.committed_rev,
            tx_id,
            kind: None,
            actor: call.ctx.actor(),
            label: None,
            at: call.at,
            steps: &steps,
            patch: &patch,
        })?;

        // Commit point: nothing below fails.
        for (id, view) in views {
            if let Some(t) = self.open.get_mut(&id) {
                t.view = view;
            }
        }
        let prev_rev = self.rev;
        self.committed = committed;
        self.committed_rev = committed_rev;
        self.live = live;
        self.rev = rev;
        self.dedup.insert(record);
        self.emit(
            prev_rev,
            call.ctx.actor().clone(),
            tx_id,
            PatchCause::Command,
            Some(call.request_id),
            delta,
        );
        self.changes = Changes::BOTH;
        Ok(applied)
    }

    /// A command inside the caller's open transaction `tx_id`, run against its view.
    fn run_in_tx(&mut self, tx_id: TxId, call: &Call, params: Value) -> Result<Applied, CmdError> {
        let t = self.open.get(&tx_id).ok_or_else(|| tx_not_found(tx_id))?;
        if t.conn != call.ctx.conn() {
            return Err(not_owner(t));
        }
        if t.conflicted {
            return Err(conflicted(t));
        }
        let (base, kind) = (t.view.clone(), t.kind);
        let (done, output) = self.execute(call, params, base)?;
        if done.is_empty() {
            let applied = self.applied(call, Some(tx_id), output, Patch::new());
            self.add_step(tx_id, call, &applied, None);
            return Ok(applied);
        }
        let Committed {
            doc: view,
            patch: step,
            ..
        } = done;
        if !kind.is_live() {
            // A batch: nothing visible until it commits.
            let applied = self.applied(call, Some(tx_id), output, step.clone());
            self.add_step(tx_id, call, &applied, Some((step, view)));
            return Ok(applied);
        }

        let others_live = self
            .open
            .values()
            .any(|o| o.id != tx_id && o.contributes_live());
        let (live, delta) = if others_live {
            let mut ops = self.open.get(&tx_id).map_or_default(|t| t.ops.clone());
            ops.extend(step.iter().cloned());
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
                    |o| soft_locked(o, &e),
                )
            })?;
            let delta = LiveDelta::between(&self.live, &live);
            (live, delta)
        } else {
            (view.clone(), LiveDelta::Ops(step.clone()))
        };

        let prev_rev = self.rev;
        self.rev = self.rev.next();
        let applied = self.applied(call, Some(tx_id), output, step.clone());
        self.add_step(tx_id, call, &applied, Some((step, view)));
        self.live = live;
        self.emit(
            prev_rev,
            call.ctx.actor().clone(),
            tx_id,
            PatchCause::Live,
            Some(call.request_id),
            delta,
        );
        self.changes.live = true;
        Ok(applied)
    }

    fn add_step(
        &mut self,
        tx_id: TxId,
        call: &Call,
        applied: &Applied,
        change: Option<(Patch, Project)>,
    ) {
        let record = Self::record(call, applied);
        if let Some(t) = self.open.get_mut(&tx_id) {
            if let Some((step, view)) = change {
                t.ops.extend(step);
                t.view = view;
            }
            t.last_activity = call.at;
            t.steps.push(JournalStep {
                command: call.name.clone(),
                params: call.journal_params.clone(),
                record,
            });
        }
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
        let outer = self.current.replace((call.ctx.clone(), call.request_id));
        let start = self.outbox.len();
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            registry.invoke_engine_json(&call.name, params, &mut *self, access)
        }));
        self.current = outer;
        let output = match result {
            Ok(result) => result?,
            Err(payload) => return Err(self.panicked(call, payload.as_ref())),
        };
        let ops: Patch = self
            .outbox
            .get(start..)
            .unwrap_or_default()
            .iter()
            .filter_map(DocEvent::as_patched)
            .flat_map(|p| p.ops.iter().cloned())
            .collect();
        let applied = self.applied(call, None, output, ops);
        self.dedup.insert(Self::record(call, &applied));
        Ok(applied)
    }

    // ----- open transactions (the seam for WP-1B.04 / WP-1B.09) -----------------------------

    /// Opens a transaction owned by `ctx`'s connection (one per connection). Nothing changes
    /// until commands run in it ([`Submit::in_tx`]).
    pub fn begin_tx(
        &mut self,
        ctx: &ActorCtx,
        kind: TxKind,
        label: impl Into<String>,
    ) -> Result<TxInfo, CmdError> {
        if let Some(t) = self.open.values().find(|t| t.conn == ctx.conn()) {
            return Err(CmdError::invalid(
                "",
                format!(
                    "this connection already has open transaction {}; commit or cancel it first",
                    t.id
                ),
            ));
        }
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
            conflicted: false,
        };
        let info = t.info();
        self.open.insert(id, t);
        Ok(info)
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
        let result = self.commit_inner(ctx, id);
        result.map_err(|e| e.with_rev(self.rev))
    }

    fn commit_inner(&mut self, ctx: &ActorCtx, id: TxId) -> Result<TxClosed, CmdError> {
        self.owned(ctx, id)?;
        // A field borrow (not `owned`), so the journal can be borrowed mutably below.
        let Some(t) = self.open.get(&id) else {
            return Err(tx_not_found(id));
        };
        if t.conflicted {
            let err = conflicted(t);
            self.open.remove(&id);
            return Err(err);
        }
        if t.ops.is_empty() {
            let kind = t.kind;
            if let Some(t) = self.open.remove(&id) {
                for step in t.steps {
                    self.dedup.insert(step.record);
                }
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
        self.journal.commit(&CommitRecord {
            rev: committed_rev,
            prev_committed_rev: self.committed_rev,
            tx_id: id,
            kind: Some(t.kind),
            actor: &t.actor,
            label: Some(&t.label),
            at: now,
            steps: &t.steps,
            patch: &net,
        })?;

        // Commit point: nothing below fails.
        let Some(t) = self.open.remove(&id) else {
            return Err(CmdError::internal("lost an open transaction"));
        };
        for (vid, view) in views {
            if let Some(o) = self.open.get_mut(&vid) {
                o.view = view;
            }
        }
        self.mark_conflicted(&losers);
        let prev_rev = self.rev;
        self.committed = committed;
        self.committed_rev = committed_rev;
        self.live = live;
        self.rev = rev;
        for step in t.steps {
            self.dedup.insert(step.record);
        }
        let request = self.current_request();
        self.emit(prev_rev, t.actor, id, PatchCause::Commit, request, delta);
        self.changes = Changes::BOTH;
        Ok(TxClosed {
            tx_id: id,
            kind: t.kind,
            outcome: TxOutcome::Committed,
            rev,
            committed_rev,
            patch: net,
        })
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

    /// Cancels `ctx`'s open transaction `id`, rolling back its live changes.
    pub fn cancel_tx(&mut self, ctx: &ActorCtx, id: TxId) -> Result<TxClosed, CmdError> {
        self.owned(ctx, id).map_err(|e| e.with_rev(self.rev))?;
        self.close(id, TxOutcome::Cancelled)
            .ok_or_else(|| tx_not_found(id))
    }

    /// Rolls back every transaction owned by `conn` (the connection closed, ADR-0017).
    pub fn close_connection(&mut self, conn: ConnId) -> Vec<TxClosed> {
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
        Some(TxClosed {
            tx_id: id,
            kind: t.kind,
            outcome,
            rev: self.rev,
            committed_rev: self.committed_rev,
            patch: Patch::new(),
        })
    }

    // ----- diagnostics -----------------------------------------------------------------------

    /// Checks the model's invariants; expensive, for tests and diagnostics:
    /// every open transaction's view is the committed document with its ops replayed, the live
    /// document is the committed document with the live transactions' ops replayed in order,
    /// `committed_rev <= rev`, and `rev == committed_rev` implies live == committed.
    pub fn verify(&self) -> Result<(), String> {
        if self.committed_rev > self.rev {
            return Err(format!(
                "committed_rev {} > rev {}",
                self.committed_rev, self.rev
            ));
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
        Ok(())
    }
}
