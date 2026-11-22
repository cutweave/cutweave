//! The single-writer document actor (arch §5.1–§5.2), minus threads: callers serialize access
//! (`&mut Engine`; 0H.02 wraps it in a mutex/actor).
//!
//! Responsibilities:
//! - dispatch by name through the [`Registry`] with the sequence frame rate as timecode context;
//! - `request_id` idempotency: a repeat within the window returns the original result, a reuse
//!   with a different payload is a `Conflict`;
//! - `expected_rev` optimistic concurrency;
//! - transactions: one `Tx` per call or per batch, rollback on error, dry runs;
//! - per-actor undo/redo with "target unchanged since" checks;
//! - `doc.patched` events to subscribers.
//!
//! The actor is never read from the request: transports construct a [`Caller`] from the
//! credential (arch §6). `Request` denies unknown fields, so a client-sent `actor` is rejected.

use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    doc::Doc,
    error::{CmdError, ConflictReason, EntityKind, decode},
    ids::{IdGen, IdMode},
    registry::{CommandEntry, EntryKind, Registry, Runner},
    time::{FrameRate, with_frame_rate},
    tx::{Patch, Rev, Side, Tx},
    wire,
};

/// Who is acting (arch §5.2). Derived from the credential, never from the request.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Actor {
    /// The desktop UI.
    User,
    /// An agent (MCP/CLI) with a label.
    Agent(String),
    /// A plugin by id.
    Plugin(String),
    /// The engine itself.
    System,
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Actor::User => f.write_str("user"),
            Actor::Agent(l) => write!(f, "agent:{l}"),
            Actor::Plugin(id) => write!(f, "plugin:{id}"),
            Actor::System => f.write_str("system"),
        }
    }
}

/// The authenticated caller. In the product the transport builds this from the credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    actor: Actor,
}

impl Caller {
    /// A caller acting as `actor`.
    #[must_use]
    pub fn new(actor: Actor) -> Caller {
        Caller { actor }
    }

    /// Shorthand for an agent caller.
    #[must_use]
    pub fn agent(label: &str) -> Caller {
        Caller::new(Actor::Agent(label.into()))
    }

    /// The actor.
    #[must_use]
    pub fn actor(&self) -> &Actor {
        &self.actor
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde `skip_serializing_if` signature
fn is_false(b: &bool) -> bool {
    !*b
}

/// The call envelope (arch §5.1): name + params + idempotency and concurrency controls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Registry name, e.g. `clip.split`.
    pub name: String,
    /// Params object (`null`/missing means `{}`).
    #[serde(default)]
    pub params: Value,
    /// Client-generated id (UUIDv7) for at-most-once application.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Fail with `Conflict` unless the document is at this revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_rev: Option<Rev>,
    /// Apply, capture the result, then roll back.
    #[serde(default, skip_serializing_if = "is_false")]
    pub dry_run: bool,
}

impl Request {
    /// A plain request.
    #[must_use]
    pub fn new(name: &str, params: Value) -> Request {
        Request {
            name: name.into(),
            params,
            request_id: None,
            expected_rev: None,
            dry_run: false,
        }
    }

    /// With a request id.
    #[must_use]
    pub fn with_request_id(mut self, id: &str) -> Request {
        self.request_id = Some(id.into());
        self
    }

    /// With an expected revision.
    #[must_use]
    pub fn with_expected_rev(mut self, rev: Rev) -> Request {
        self.expected_rev = Some(rev);
        self
    }

    /// As a dry run.
    #[must_use]
    pub fn dry_run(mut self) -> Request {
        self.dry_run = true;
        self
    }
}

/// Successful call result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Response {
    /// Document revision after the call.
    pub rev: Rev,
    /// The command/query output.
    pub result: Value,
    /// True for dry runs (nothing was applied).
    pub dry_run: bool,
    /// True when this is a replay of an earlier identical request.
    pub deduplicated: bool,
}

/// One step of a transactional batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Registry name.
    pub name: String,
    /// Params.
    #[serde(default)]
    pub params: Value,
}

/// A transactional batch (`cutweave_execute`, `edit.begin_tx/commit_tx`): all steps apply in one
/// `Tx` and one undo entry, or none do.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    /// Steps in order; later steps see earlier steps' effects.
    pub steps: Vec<Step>,
    /// As in [`Request`].
    #[serde(default)]
    pub request_id: Option<String>,
    /// As in [`Request`].
    #[serde(default)]
    pub expected_rev: Option<Rev>,
    /// As in [`Request`].
    #[serde(default)]
    pub dry_run: bool,
}

/// Batch result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BatchResponse {
    /// Revision after the batch.
    pub rev: Rev,
    /// One result per step.
    pub results: Vec<Value>,
    /// Dry run.
    pub dry_run: bool,
    /// Replay of an earlier identical batch.
    pub deduplicated: bool,
}

/// Events streamed to subscribers (arch §5.1).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "event")]
pub enum Event {
    /// A transaction (or undo/redo) changed the document.
    #[serde(rename = "doc.patched")]
    DocPatched {
        /// New revision.
        rev: Rev,
        /// Actor, e.g. `agent:claude`.
        actor: String,
        /// What happened, e.g. `clip.split` or `undo clip.split`.
        label: String,
        /// Forward ops of this revision.
        ops: Vec<Patch>,
    },
}

/// A history entry as reported to callers.
#[wire]
pub struct TxSummary {
    /// Revision that originally applied the transaction.
    pub rev: Rev,
    /// Label, e.g. `clip.split`.
    pub label: String,
}

/// Monotonic time source for the dedup window (injectable for tests).
pub trait Clock: Send + Sync {
    /// Time since an arbitrary fixed origin.
    fn now(&self) -> Duration;
}

/// Wall-clock monotonic time. (Not usable on wasm32-unknown-unknown: `Instant` panics there.)
pub struct SystemClock(Instant);

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock(Instant::now())
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// A clock that only moves when told to.
#[derive(Clone, Default)]
pub struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    /// Advances by `d`.
    pub fn advance(&self, d: Duration) {
        let nanos = u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
        self.0.fetch_add(nanos, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.0.load(Ordering::SeqCst))
    }
}

/// Engine settings.
#[derive(Clone)]
pub struct EngineConfig {
    /// Id minting mode.
    pub ids: IdMode,
    /// How long applied `request_id`s are remembered (arch: 10 minutes).
    pub dedup_window: Duration,
    /// Clock for the dedup window.
    pub clock: Arc<dyn Clock>,
    /// Name of the initial project.
    pub project_name: String,
    /// Frame rate of the initial sequence.
    pub frame_rate: FrameRate,
}

impl EngineConfig {
    /// Deterministic ids (`--deterministic-ids <seed>`).
    #[must_use]
    pub fn deterministic(seed: u64) -> EngineConfig {
        EngineConfig {
            ids: IdMode::Deterministic { seed },
            dedup_window: Duration::from_secs(600),
            clock: Arc::new(SystemClock::default()),
            project_name: "Untitled".into(),
            frame_rate: FrameRate::new(30, 1),
        }
    }

    /// Replaces the clock.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> EngineConfig {
        self.clock = clock;
        self
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            ids: IdMode::Random,
            ..EngineConfig::deterministic(0)
        }
    }
}

struct TxRecord {
    rev: Rev,
    actor: Actor,
    label: String,
    patches: Vec<Patch>,
}

#[derive(Clone)]
enum Journaled {
    Single(Response),
    Batch(BatchResponse),
}

struct JournalEntry {
    fingerprint: String,
    at: Duration,
    outcome: Journaled,
}

type JournalKey = (Actor, String);

/// The document actor.
pub struct Engine {
    registry: &'static Registry,
    doc: Doc,
    rev: Rev,
    ids: IdGen,
    undo: Vec<TxRecord>,
    redo: HashMap<Actor, Vec<TxRecord>>,
    journal: HashMap<JournalKey, JournalEntry>,
    journal_order: VecDeque<(Duration, JournalKey)>,
    subscribers: Vec<mpsc::Sender<Event>>,
    config: EngineConfig,
}

impl Engine {
    /// A new engine over the global registry with a fresh project.
    #[must_use]
    pub fn new(config: EngineConfig) -> Engine {
        Engine::with_registry(Registry::global(), config)
    }

    /// A new engine over a specific registry.
    #[must_use]
    pub fn with_registry(registry: &'static Registry, config: EngineConfig) -> Engine {
        let mut ids = IdGen::new(config.ids);
        let doc = Doc::new(&config.project_name, config.frame_rate, &mut ids);
        Engine {
            registry,
            doc,
            rev: Rev(0),
            ids,
            undo: Vec::new(),
            redo: HashMap::new(),
            journal: HashMap::new(),
            journal_order: VecDeque::new(),
            subscribers: Vec::new(),
            config,
        }
    }

    /// Current document (read-only; nothing outside a `Tx` can mutate it).
    #[must_use]
    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// Current revision.
    #[must_use]
    pub fn rev(&self) -> Rev {
        self.rev
    }

    /// The registry this engine dispatches through.
    #[must_use]
    pub fn registry(&self) -> &'static Registry {
        self.registry
    }

    /// Number of undo entries owned by `actor`.
    #[must_use]
    pub fn undo_depth(&self, actor: &Actor) -> usize {
        self.undo.iter().filter(|r| &r.actor == actor).count()
    }

    /// Subscribes to [`Event`]s.
    pub fn subscribe(&mut self) -> mpsc::Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        self.subscribers.push(tx);
        rx
    }

    /// JSON in, JSON out: `{"name","params","request_id"?,"expected_rev"?,"dry_run"?}` ->
    /// `{"ok":true,"rev","result","dry_run","deduplicated"}` or `{"ok":false,"error":{code,message,..}}`.
    pub fn invoke_json(&mut self, caller: &Caller, request: Value) -> Value {
        let outcome = decode::<Request>(request)
            .and_then(|req| self.invoke(caller, req))
            .and_then(|resp| crate::error::encode(&resp));
        wrap_outcome(outcome)
    }

    /// Batch variant of [`Engine::invoke_json`]: `{"steps":[{name,params}],...}`.
    pub fn execute_json(&mut self, caller: &Caller, batch: Value) -> Value {
        let outcome = decode::<Batch>(batch)
            .and_then(|b| self.execute(caller, b))
            .and_then(|resp| crate::error::encode(&resp));
        wrap_outcome(outcome)
    }

    /// Invokes one registry entry.
    pub fn invoke(&mut self, caller: &Caller, req: Request) -> Result<Response, CmdError> {
        let result = self.invoke_inner(caller, req);
        result.map_err(|e| self.fill_rev(e))
    }

    fn invoke_inner(&mut self, caller: &Caller, req: Request) -> Result<Response, CmdError> {
        let entry = self.lookup(&req.name)?;
        let params = normalize_params(req.params);
        if let Runner::Query(run) = entry.runner {
            self.check_rev(req.expected_rev)?;
            let rate = self.doc.sequence.frame_rate;
            let result = with_frame_rate(rate, || run(&self.doc, params))?;
            return Ok(Response {
                rev: self.rev,
                result,
                dry_run: req.dry_run,
                deduplicated: false,
            });
        }
        let fingerprint = fingerprint(&json!([req.name, params]));
        if let Some(Journaled::Single(resp)) =
            self.dedup_lookup(caller, req.request_id.as_deref(), &fingerprint)?
        {
            return Ok(Response {
                deduplicated: true,
                ..resp
            });
        }
        self.check_rev(req.expected_rev)?;
        let resp = match entry.runner {
            Runner::Engine(run) => {
                if req.dry_run {
                    return Err(CmdError::Unsupported {
                        msg: format!("`{}` does not support dry_run", entry.meta.name),
                    });
                }
                let result = run(self, caller, params)?;
                Response {
                    rev: self.rev,
                    result,
                    dry_run: false,
                    deduplicated: false,
                }
            }
            Runner::Command(_) | Runner::Query(_) => {
                let label = entry.meta.name.to_owned();
                let mut results =
                    self.run_tx(caller, vec![(entry, params)], req.dry_run, label, false)?;
                Response {
                    rev: self.rev,
                    result: results.pop().unwrap_or(Value::Null),
                    dry_run: req.dry_run,
                    deduplicated: false,
                }
            }
        };
        if !req.dry_run {
            self.journal_put(
                caller,
                req.request_id,
                fingerprint,
                Journaled::Single(resp.clone()),
            );
        }
        Ok(resp)
    }

    /// Runs a batch in one transaction and one undo entry.
    pub fn execute(&mut self, caller: &Caller, batch: Batch) -> Result<BatchResponse, CmdError> {
        let result = self.execute_inner(caller, batch);
        result.map_err(|e| self.fill_rev(e))
    }

    fn execute_inner(&mut self, caller: &Caller, batch: Batch) -> Result<BatchResponse, CmdError> {
        if batch.steps.is_empty() {
            return Err(CmdError::invalid(
                "/steps",
                "a batch needs at least one step",
            ));
        }
        let mut steps = Vec::with_capacity(batch.steps.len());
        for (i, step) in batch.steps.iter().enumerate() {
            let entry = self.lookup(&step.name)?;
            let transactional = match entry.runner {
                Runner::Command(_) => entry.meta.undoable,
                Runner::Query(_) => true,
                Runner::Engine(_) => false,
            };
            if !transactional {
                return Err(CmdError::invalid(
                    format!("/steps/{i}/name"),
                    format!(
                        "`{}` cannot run inside a transaction (engine op or non-undoable)",
                        step.name
                    ),
                ));
            }
            steps.push((entry, normalize_params(step.params.clone())));
        }
        let fingerprint = fingerprint(&json!(batch.steps));
        if let Some(Journaled::Batch(resp)) =
            self.dedup_lookup(caller, batch.request_id.as_deref(), &fingerprint)?
        {
            return Ok(BatchResponse {
                deduplicated: true,
                ..resp
            });
        }
        self.check_rev(batch.expected_rev)?;
        let label = format!(
            "batch({})",
            batch
                .steps
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let results = self.run_tx(caller, steps, batch.dry_run, label, true)?;
        let resp = BatchResponse {
            rev: self.rev,
            results,
            dry_run: batch.dry_run,
            deduplicated: false,
        };
        if !batch.dry_run {
            self.journal_put(
                caller,
                batch.request_id,
                fingerprint,
                Journaled::Batch(resp.clone()),
            );
        }
        Ok(resp)
    }

    /// Runs steps in one `Tx`. For batches, errors carry `/steps/<i>/params` path prefixes.
    fn run_tx(
        &mut self,
        caller: &Caller,
        steps: Vec<(&'static CommandEntry, Value)>,
        dry_run: bool,
        label: String,
        is_batch: bool,
    ) -> Result<Vec<Value>, CmdError> {
        let rate = self.doc.sequence.frame_rate;
        let undoable = steps
            .iter()
            .all(|(e, _)| e.meta.kind == EntryKind::Query || e.meta.undoable);
        let mut tx = Tx::new(&mut self.doc, &mut self.ids);
        let mut results = Vec::with_capacity(steps.len());
        for (i, (entry, params)) in steps.into_iter().enumerate() {
            let outcome = with_frame_rate(rate, || match entry.runner {
                Runner::Command(run) => run(&mut tx, params),
                Runner::Query(run) => run(tx.doc(), params),
                Runner::Engine(_) => Err(CmdError::Unsupported {
                    msg: "engine ops cannot run inside a transaction".into(),
                }),
            });
            match outcome {
                Ok(v) => results.push(v),
                Err(e) => {
                    tx.rollback();
                    return Err(if is_batch {
                        e.under(&format!("/steps/{i}/params"))
                    } else {
                        e
                    });
                }
            }
        }
        if dry_run {
            tx.rollback();
            return Ok(results);
        }
        let patches = tx.finish();
        if !patches.is_empty() {
            if undoable {
                self.redo.remove(caller.actor());
            } else {
                // A non-undoable mutation (e.g. project.open) invalidates all history.
                self.undo.clear();
                self.redo.clear();
            }
            let rev = self.bump_and_emit(caller, label.clone(), patches.clone());
            if undoable {
                self.undo.push(TxRecord {
                    rev,
                    actor: caller.actor().clone(),
                    label,
                    patches,
                });
            }
        }
        Ok(results)
    }

    /// Undoes `caller`'s most recent undoable transaction. `Ok(None)` if there is none.
    ///
    /// Fails with `Conflict(history_target_modified)` (doc unchanged) if another transaction has
    /// since changed an entity this one touched.
    pub fn undo(&mut self, caller: &Caller) -> Result<Option<TxSummary>, CmdError> {
        let Some(idx) = self.undo.iter().rposition(|r| &r.actor == caller.actor()) else {
            return Ok(None);
        };
        let record = self.undo.remove(idx);
        let mut tx = Tx::new(&mut self.doc, &mut self.ids);
        for p in record.patches.iter().rev() {
            if !p.doc_matches(tx.doc(), Side::After) {
                tx.rollback();
                let err = CmdError::conflict(
                    ConflictReason::HistoryTargetModified,
                    format!(
                        "cannot undo `{}` (rev {}): {} was changed by a later transaction",
                        record.label,
                        record.rev,
                        p.target()
                    ),
                );
                self.undo.insert(idx, record);
                return Err(self.fill_rev(err));
            }
            tx.record(p.inverse());
        }
        let patches = tx.finish();
        self.bump_and_emit(caller, format!("undo {}", record.label), patches);
        let summary = TxSummary {
            rev: record.rev,
            label: record.label.clone(),
        };
        self.redo
            .entry(caller.actor().clone())
            .or_default()
            .push(record);
        Ok(Some(summary))
    }

    /// Redoes `caller`'s most recently undone transaction. `Ok(None)` if there is none.
    pub fn redo(&mut self, caller: &Caller) -> Result<Option<TxSummary>, CmdError> {
        let Some(record) = self.redo.get_mut(caller.actor()).and_then(Vec::pop) else {
            return Ok(None);
        };
        let mut tx = Tx::new(&mut self.doc, &mut self.ids);
        for p in &record.patches {
            if !p.doc_matches(tx.doc(), Side::Before) {
                tx.rollback();
                let err = CmdError::conflict(
                    ConflictReason::HistoryTargetModified,
                    format!(
                        "cannot redo `{}`: {} was changed since the undo",
                        record.label,
                        p.target()
                    ),
                );
                self.redo
                    .entry(caller.actor().clone())
                    .or_default()
                    .push(record);
                return Err(self.fill_rev(err));
            }
            tx.record(p.clone());
        }
        let patches = tx.finish();
        self.bump_and_emit(caller, format!("redo {}", record.label), patches);
        let summary = TxSummary {
            rev: record.rev,
            label: record.label.clone(),
        };
        self.undo.push(record);
        Ok(Some(summary))
    }

    fn bump_and_emit(&mut self, caller: &Caller, label: String, ops: Vec<Patch>) -> Rev {
        self.rev = Rev(self.rev.0 + 1);
        let event = Event::DocPatched {
            rev: self.rev,
            actor: caller.actor().to_string(),
            label,
            ops,
        };
        self.subscribers.retain(|s| s.send(event.clone()).is_ok());
        self.rev
    }

    fn lookup(&self, name: &str) -> Result<&'static CommandEntry, CmdError> {
        self.registry.get(name).ok_or_else(|| CmdError::NotFound {
            entity: EntityKind::Command,
            id: name.into(),
        })
    }

    fn check_rev(&self, expected: Option<Rev>) -> Result<(), CmdError> {
        match expected {
            Some(rev) if rev != self.rev => Err(CmdError::conflict(
                ConflictReason::StaleRev,
                format!("expected rev {rev}, document is at rev {}", self.rev),
            )),
            _ => Ok(()),
        }
    }

    fn fill_rev(&self, e: CmdError) -> CmdError {
        match e {
            CmdError::Conflict { reason, detail, .. } => CmdError::Conflict {
                reason,
                detail,
                current_rev: Some(self.rev),
            },
            other => other,
        }
    }

    fn dedup_lookup(
        &mut self,
        caller: &Caller,
        request_id: Option<&str>,
        fingerprint: &str,
    ) -> Result<Option<Journaled>, CmdError> {
        let Some(id) = request_id else {
            return Ok(None);
        };
        if id.is_empty() || id.len() > 64 {
            return Err(CmdError::invalid(
                "/request_id",
                "request_id must be 1..=64 chars",
            ));
        }
        self.journal_purge();
        let key = (caller.actor().clone(), id.to_owned());
        match self.journal.get(&key) {
            None => Ok(None),
            Some(entry) if entry.fingerprint == fingerprint => Ok(Some(entry.outcome.clone())),
            Some(_) => Err(CmdError::conflict(
                ConflictReason::RequestIdReused,
                format!("request_id `{id}` was already used for a different request"),
            )),
        }
    }

    fn journal_put(
        &mut self,
        caller: &Caller,
        request_id: Option<String>,
        fingerprint: String,
        outcome: Journaled,
    ) {
        let Some(id) = request_id else { return };
        let at = self.config.clock.now();
        let key = (caller.actor().clone(), id);
        self.journal_order.push_back((at, key.clone()));
        self.journal.insert(
            key,
            JournalEntry {
                fingerprint,
                at,
                outcome,
            },
        );
    }

    fn journal_purge(&mut self) {
        let now = self.config.clock.now();
        while let Some((at, _)) = self.journal_order.front() {
            if now.saturating_sub(*at) < self.config.dedup_window {
                break;
            }
            if let Some((at, key)) = self.journal_order.pop_front()
                && self.journal.get(&key).is_some_and(|e| e.at == at)
            {
                self.journal.remove(&key);
            }
        }
    }
}

fn normalize_params(params: Value) -> Value {
    if params.is_null() { json!({}) } else { params }
}

fn fingerprint(v: &Value) -> String {
    // serde_json maps are BTreeMaps (no `preserve_order`), so this is canonical.
    v.to_string()
}

fn wrap_outcome(outcome: Result<Value, CmdError>) -> Value {
    match outcome {
        Ok(mut v) => {
            if let Value::Object(map) = &mut v {
                map.insert("ok".into(), Value::Bool(true));
            }
            v
        }
        Err(e) => json!({ "ok": false, "error": e.to_wire() }),
    }
}
