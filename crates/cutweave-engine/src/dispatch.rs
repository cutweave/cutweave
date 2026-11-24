// SPDX-License-Identifier: GPL-3.0-or-later
//! The in-process dispatch API (WP-1I.04): what the JSON-RPC layer (WP-1I.02), MCP (WP-1J.01),
//! the CLI (WP-1K.01) and tests call. One [`Connection`] per client connection
//! ([`EngineHandle::connect`](crate::EngineHandle::connect)); [`Connection::call`] runs any
//! registered command, query or engine command by name.
//!
//! # Routing
//!
//! - Queries (outside a transaction) run on the calling thread against the live snapshot,
//!   without touching the doc actor.
//! - Document commands, queries inside a transaction and the doc actor's engine commands
//!   (`edit.*`) are submitted to the doc actor with the envelope (`request_id` dedup,
//!   `expected_rev`, `tx`).
//! - `job.*` run against the job manager (`job.wait` blocks up to 60 s: async callers use
//!   [`Connection::call_async`], which runs the call on the blocking pool).
//! - `media.import` gets a per-call import context (the caller's path policy, a batch
//!   transaction on a connection of its own, analysis jobs).
//! - `view.*` run against the session's view state (`view.changed` to the other clients).
//! - `project.*` session commands swap or save the session.
//! - Anything else is offered to the [`Subsystems`](crate::Subsystems).
//!
//! The actor is always the connection's, derived from the credential (ADR-0016): nothing in a
//! [`Call`] can name another one. Engine commands outside the doc actor are not deduplicated
//! by `request_id` and ignore `expected_rev`.

use std::any::TypeId;
use std::sync::Arc;

use cutweave_commands::{
    ActorCtx, Applied, CmdError, ConnId, DocState, EntryKind, QueryCtx, RequestId, ResourceKind,
    Rev, Submit, TxId,
};
use serde_json::{Map, Value};

use crate::engine::Core;

/// One request: a registered name, its params and the envelope (`_meta` on the wire).
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    /// The command, query or engine command (`clip.split`, `timeline.get`, `job.wait`).
    pub name: String,
    /// Its params (a JSON object; `null` means `{}`).
    pub params: Value,
    /// Idempotency key: a repeat within 10 minutes returns the first result.
    pub request_id: Option<RequestId>,
    /// Fail with `Conflict` unless the document is at this revision.
    pub expected_rev: Option<Rev>,
    /// Run inside this open transaction of the connection.
    pub tx: Option<TxId>,
}

impl Call {
    /// `name` with `params` and no envelope.
    #[must_use]
    pub fn new(name: impl Into<String>, params: Value) -> Call {
        Call {
            name: name.into(),
            params,
            request_id: None,
            expected_rev: None,
            tx: None,
        }
    }

    /// With an idempotency key.
    #[must_use]
    pub fn with_request_id(mut self, id: RequestId) -> Call {
        self.request_id = Some(id);
        self
    }

    /// With an expected revision.
    #[must_use]
    pub fn with_expected_rev(mut self, rev: Rev) -> Call {
        self.expected_rev = Some(rev);
        self
    }

    /// Inside an open transaction.
    #[must_use]
    pub fn in_tx(mut self, tx: TxId) -> Call {
        self.tx = Some(tx);
        self
    }
}

/// The result of a [`Call`].
#[derive(Clone, Debug)]
pub struct Reply {
    /// The output (a JSON object).
    pub output: Value,
    /// The live document revision after the call (for queries: of the snapshot read).
    pub rev: Rev,
    /// The committed revision after the call.
    pub committed_rev: Rev,
    /// The doc actor's result (request id, transaction, patch, `replayed`), when the call ran
    /// there.
    pub applied: Option<Applied>,
}

struct Inner {
    core: Arc<Core>,
    ctx: ActorCtx,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Rolls back the connection's open transaction and releases its soft locks; the id is
        // retired for good. The internal connection is shared and never closed.
        if self.ctx.conn() == ConnId::INTERNAL {
            return;
        }
        if let Ok(doc) = self.core.doc() {
            drop(doc.close_connection(self.ctx.conn()));
        }
    }
}

/// A client connection: the credential-derived caller ([`ActorCtx`]) on a connection id of
/// its own. Cheap to clone (requests may run concurrently); closed when the last clone drops.
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("ctx", &self.inner.ctx)
            .finish_non_exhaustive()
    }
}

impl Connection {
    pub(crate) fn new(core: Arc<Core>, ctx: ActorCtx) -> Connection {
        Connection {
            inner: Arc::new(Inner { core, ctx }),
        }
    }

    /// The caller: connection id, actor and access.
    #[must_use]
    pub fn ctx(&self) -> &ActorCtx {
        &self.inner.ctx
    }

    /// Runs `call` and waits for the result (blocking: never call it on an async worker; use
    /// [`call_async`](Self::call_async) there).
    pub fn call(&self, call: Call) -> Result<Reply, CmdError> {
        let core = &self.inner.core;
        let ctx = &self.inner.ctx;
        let Call {
            name,
            params,
            request_id,
            expected_rev,
            tx,
        } = call;
        let registry = Arc::clone(&core.deps.registry);
        let entry = registry
            .get(&name)
            .ok_or_else(|| CmdError::not_found(ResourceKind::Command, &name))?;
        ctx.access().check(&entry.meta)?;
        let params = if params.is_null() {
            Value::Object(Map::new())
        } else {
            params
        };
        let context = entry.engine_context();
        let on_actor = match entry.kind() {
            EntryKind::Query => tx.is_some(),
            EntryKind::Command => true,
            EntryKind::Engine => context == Some(TypeId::of::<DocState>()),
        };
        if entry.kind() == EntryKind::Query && !on_actor {
            return query(core, ctx, &name, params);
        }
        if on_actor {
            let guard = core.session()?;
            let Some(session) = guard.as_ref() else {
                return Err(CmdError::unsupported("the engine is shutting down"));
            };
            let mut submit = Submit::new(ctx.clone(), name, params);
            if let Some(id) = request_id {
                submit = submit.with_request_id(id);
            }
            if let Some(rev) = expected_rev {
                submit = submit.with_expected_rev(rev);
            }
            if let Some(tx) = tx {
                submit = submit.in_tx(tx);
            }
            // The session stays read-locked until the reply: a document swap waits for
            // requests already submitted.
            let applied = session.handle().submit(submit).wait()?;
            drop(guard);
            #[cfg(debug_assertions)]
            if entry.kind() == EntryKind::Command && !applied.replayed {
                test_crash_hook::committed();
            }
            return Ok(Reply {
                output: applied.output.clone(),
                rev: applied.rev,
                committed_rev: applied.committed_rev,
                applied: Some(applied),
            });
        }
        if tx.is_some() {
            return Err(CmdError::unsupported(format!(
                "`{name}` acts on engine state and cannot run inside a transaction"
            )));
        }
        let Some(context) = context else {
            return Err(CmdError::internal(format!(
                "`{name}` is an engine command without a context"
            )));
        };
        core.run_engine(ctx, &name, params, context)
    }

    /// [`call`](Self::call) from async code: runs on the engine runtime's blocking pool, so
    /// waits (the doc actor's reply, `job.wait`, probing) never stall an async worker.
    pub async fn call_async(&self, call: Call) -> Result<Reply, CmdError> {
        let conn = self.clone();
        self.inner
            .core
            .runtime
            .spawn_blocking(move || conn.call(call))
            .await
            .map_err(|e| CmdError::internal(format!("the request task failed: {e}")))?
    }
}

/// Test hook (WP-1L.03), debug builds only: with `CUTWEAVE_TEST_CRASH_AFTER=N` the engine
/// aborts right after its Nth committed document command, before that command's reply leaves
/// (its journal record is already written), so supervisor and reconnect tests can crash the
/// engine at a known point. Release builds do not read the variable.
#[cfg(debug_assertions)]
mod test_crash_hook {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};

    static LIMIT: OnceLock<Option<u64>> = OnceLock::new();
    static COMMITTED: AtomicU64 = AtomicU64::new(0);

    pub(super) fn committed() {
        let limit = *LIMIT.get_or_init(|| {
            std::env::var("CUTWEAVE_TEST_CRASH_AFTER")
                .ok()
                .and_then(|v| v.trim().parse().ok())
        });
        let Some(limit) = limit else { return };
        if COMMITTED.fetch_add(1, Ordering::SeqCst) + 1 >= limit {
            eprintln!("cutweave engine: CUTWEAVE_TEST_CRASH_AFTER={limit} reached; aborting");
            std::process::abort();
        }
    }
}

/// A query on the calling thread against the live snapshot, with the subsystems' renderer
/// current for the `render.*` queries (`cutweave_render::query`).
fn query(core: &Core, ctx: &ActorCtx, name: &str, params: Value) -> Result<Reply, CmdError> {
    let snap = core.doc()?.snapshot();
    let qctx = QueryCtx::new(&snap.doc, ctx.access()).with_rev(snap.rev);
    let _renderer = name
        .starts_with("render.")
        .then(|| core.deps.subsystems.frame_backend())
        .flatten()
        .map(cutweave_render::query::enter);
    let output = core.deps.registry.query_json(name, params, &qctx)?;
    Ok(Reply {
        output,
        rev: snap.rev,
        committed_rev: snap.committed_rev,
        applied: None,
    })
}
