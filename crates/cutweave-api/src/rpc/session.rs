// SPDX-License-Identifier: GPL-3.0-or-later
//! One WebSocket connection (WP-1I.02): the read/write loop, request routing and execution.
//!
//! The loop owns the socket. It writes responses and event notifications as they come, reads
//! the next message only while it holds an in-flight permit ([`RpcConfig::max_in_flight`]),
//! and closes the socket when the hub asks (4001 `resync_required`, 1001 engine stopped).
//!
//! Routing of a request (in the order messages arrive):
//! - `rpc.subscribe` / `rpc.unsubscribe`, unknown methods and envelope errors are answered at
//!   once, so a subscription is active before the next message is read;
//! - mutating calls go to the connection's **lane**: one at a time, in the order received;
//! - read-only calls (queries, `job.wait`, `edit.history`, `rpc.hello`, `project.snapshot`)
//!   wait until the mutating calls received before them are done, then run concurrently.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket};
use cutweave_commands::{CmdError, ResourceKind, SCHEMA_HASH};
use futures_util::SinkExt;
use serde_json::{Map, Value};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};

use super::hub::{CloseReason, Subscriber};
use super::wire::{self, Failure, Request, codes};
use super::{ALL_EVENTS, RpcCall, RpcClientHello, RpcConnection, RpcServerHello, RpcState};

/// How long the server waits to deliver its close frame before dropping the socket.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// The most messages written with one flush.
const MAX_COALESCED: usize = 64;

/// The longest event name `rpc.subscribe` accepts.
const MAX_EVENT_NAME: usize = 128;

/// Where a request's response goes.
enum Dest {
    /// Straight to the socket.
    Socket(mpsc::UnboundedSender<String>),
    /// Into a batch's response array (`None`: no response, a notification).
    Batch(oneshot::Sender<Option<Value>>),
}

impl Dest {
    fn deliver(self, id: Option<Value>, outcome: Result<Value, Failure>) {
        let response = wire::response(id, outcome);
        match self {
            Dest::Socket(tx) => {
                if let Some(response) = response {
                    let _ = tx.send(response.to_string());
                }
            }
            Dest::Batch(tx) => {
                let _ = tx.send(response);
            }
        }
    }
}

/// A mutating call waiting in the lane.
struct LaneJob {
    seq: u64,
    request: Request,
    dest: Dest,
    _permit: Option<OwnedSemaphorePermit>,
}

/// What executes requests for one connection (cloned into the lane and read tasks).
#[derive(Clone)]
struct Exec {
    state: Arc<RpcState>,
    conn: Arc<dyn RpcConnection>,
}

impl Exec {
    /// Runs a reserved read method or a registry entry.
    async fn execute(&self, request: Request) -> Result<Value, Failure> {
        let Request {
            method,
            params,
            meta,
            ..
        } = request;
        match method.as_str() {
            "rpc.hello" => self.hello(params).await,
            "project.snapshot" => self.snapshot(params).await,
            _ => {
                let call = RpcCall {
                    name: method,
                    params: Value::Object(params),
                    request_id: meta.request_id,
                    expected_rev: meta.expected_rev,
                    tx: meta.tx,
                    replace: meta.replace,
                };
                let reply = self.conn.call(call).await?;
                let mut output = reply.output;
                if let Value::Object(fields) = &mut output {
                    let mut envelope = Map::new();
                    envelope.insert("rev".into(), Value::from(reply.rev.0));
                    envelope.insert("committed_rev".into(), Value::from(reply.committed_rev.0));
                    if let Some(id) = reply.request_id {
                        envelope.insert("request_id".into(), Value::String(id.to_string()));
                    }
                    if let Some(tx) = reply.tx_id {
                        envelope.insert("tx_id".into(), Value::String(tx.to_string()));
                    }
                    fields.insert("_meta".into(), Value::Object(envelope));
                }
                Ok(output)
            }
        }
    }

    async fn hello(&self, params: Map<String, Value>) -> Result<Value, Failure> {
        let hello: RpcClientHello = serde_json::from_value(Value::Object(params)).map_err(|e| {
            CmdError::invalid("", format!("rpc.hello takes {{client, version}}: {e}"))
        })?;
        let backend = Arc::clone(&self.state.backend);
        let status = blocking(move || backend.status()).await?;
        tracing::info!(client = %hello.client, version = %hello.version, "rpc client hello");
        let out = RpcServerHello {
            engine_version: self.state.backend.version(),
            schema_hash: SCHEMA_HASH.to_owned(),
            rev: status.rev,
            committed_rev: status.committed_rev,
            actor: self.conn.ctx().actor().to_string(),
            project: status.project,
            shm: status.shm,
        };
        serde_json::to_value(out)
            .map_err(|e| Failure::from_cmd(CmdError::internal(format!("rpc.hello: {e}"))))
    }

    async fn snapshot(&self, params: Map<String, Value>) -> Result<Value, Failure> {
        if let Some(key) = params.keys().next() {
            return Err(
                CmdError::invalid(format!("/{key}"), "project.snapshot takes no params").into(),
            );
        }
        let backend = Arc::clone(&self.state.backend);
        let snap = blocking(move || backend.snapshot()).await?;
        let mut out = Map::new();
        out.insert("rev".into(), Value::from(snap.rev.0));
        out.insert("committed_rev".into(), Value::from(snap.committed_rev.0));
        out.insert("project".into(), snap.project);
        Ok(Value::Object(out))
    }
}

/// Runs `f` on the blocking pool.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, CmdError> + Send + 'static,
) -> Result<T, CmdError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| CmdError::internal(format!("the request task failed: {e}")))?
}

/// The per-connection routing state, owned by the loop.
struct Conn {
    exec: Exec,
    subscriber: Arc<Subscriber>,
    responses: mpsc::UnboundedSender<String>,
    lane: mpsc::UnboundedSender<LaneJob>,
    /// Sequence number of the last mutating call handed to the lane.
    queued: u64,
    /// Sequence number of the last mutating call the lane finished.
    done: watch::Receiver<u64>,
}

/// How a parsed request is handled.
enum Route {
    /// Answered at once.
    Now(Result<Value, Failure>),
    /// A mutating call: the lane.
    Lane,
    /// A read-only call: concurrent, after the earlier mutations.
    Read,
}

impl Conn {
    fn route(&self, request: &Request) -> Route {
        match request.method.as_str() {
            "rpc.subscribe" => Route::Now(self.subscribe(&request.params)),
            "rpc.unsubscribe" => Route::Now(self.unsubscribe(&request.params)),
            "rpc.hello" | "project.snapshot" => Route::Read,
            name => {
                let Some(entry) = self.exec.state.registry.get(name) else {
                    return Route::Now(
                        Err(CmdError::not_found(ResourceKind::Command, name).into()),
                    );
                };
                let meta = &entry.meta;
                if meta.annotations.read_only {
                    return Route::Read;
                }
                // `view.*` state is UI-only, never journaled nor deduplicated: no key needed.
                if !meta.view && request.meta.request_id.is_none() {
                    return Route::Now(Err(CmdError::invalid(
                        "/_meta/request_id",
                        format!(
                            "`{name}` changes the project: send a client-generated UUIDv7 as \
                             `_meta.request_id` so a resend is applied only once"
                        ),
                    )
                    .into()));
                }
                Route::Lane
            }
        }
    }

    /// Dispatches one parsed request; its response goes to `dest`.
    fn dispatch(&mut self, request: Request, dest: Dest, permit: Option<OwnedSemaphorePermit>) {
        match self.route(&request) {
            Route::Now(outcome) => dest.deliver(request.id, outcome),
            Route::Lane => {
                self.queued += 1;
                let job = LaneJob {
                    seq: self.queued,
                    request,
                    dest,
                    _permit: permit,
                };
                if let Err(mpsc::error::SendError(job)) = self.lane.send(job) {
                    job.dest.deliver(
                        job.request.id,
                        Err(CmdError::unsupported("the connection is closing").into()),
                    );
                }
            }
            Route::Read => {
                let exec = self.exec.clone();
                let mut done = self.done.clone();
                let barrier = self.queued;
                tokio::spawn(async move {
                    let _permit = permit;
                    // An error means the lane ended: nothing is left to wait for.
                    let _ = done.wait_for(|d| *d >= barrier).await;
                    let id = request.id.clone();
                    let outcome = exec.execute(request).await;
                    dest.deliver(id, outcome);
                });
            }
        }
    }

    /// Handles one text message: a request or a batch.
    fn on_text(&mut self, text: &str, permit: OwnedSemaphorePermit) {
        let value: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                let failure = Failure::framing(codes::PARSE_ERROR, "", format!("not JSON: {e}"));
                Dest::Socket(self.responses.clone()).deliver(Some(Value::Null), Err(failure));
                return;
            }
        };
        match value {
            Value::Array(items) => self.on_batch(items, permit),
            single => match wire::parse_request(single) {
                Ok(request) => {
                    self.dispatch(request, Dest::Socket(self.responses.clone()), Some(permit));
                }
                Err(rejected) => {
                    Dest::Socket(self.responses.clone())
                        .deliver(rejected.id, Err(rejected.failure));
                }
            },
        }
    }

    /// A batch: every element is dispatched like a single request (mutations still in order);
    /// one array answers them, without the notifications (nothing if all were).
    fn on_batch(&mut self, items: Vec<Value>, permit: OwnedSemaphorePermit) {
        if items.is_empty() {
            let failure = Failure::framing(codes::INVALID_REQUEST, "", "an empty batch");
            Dest::Socket(self.responses.clone()).deliver(Some(Value::Null), Err(failure));
            return;
        }
        let mut slots = Vec::with_capacity(items.len());
        for item in items {
            let (tx, rx) = oneshot::channel();
            match wire::parse_request(item) {
                Ok(request) => self.dispatch(request, Dest::Batch(tx), None),
                Err(rejected) => Dest::Batch(tx).deliver(rejected.id, Err(rejected.failure)),
            }
            slots.push(rx);
        }
        let responses = self.responses.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let mut out = Vec::with_capacity(slots.len());
            for slot in slots {
                if let Ok(Some(response)) = slot.await {
                    out.push(response);
                }
            }
            if !out.is_empty() {
                let _ = responses.send(Value::Array(out).to_string());
            }
        });
    }

    fn subscribe(&self, params: &Map<String, Value>) -> Result<Value, Failure> {
        let names = event_names(params, true)?.unwrap_or_default();
        let now = self.subscriber.subscribe(&names);
        Ok(serde_json::json!({ "events": now }))
    }

    fn unsubscribe(&self, params: &Map<String, Value>) -> Result<Value, Failure> {
        let names = event_names(params, false)?;
        let now = self.subscriber.unsubscribe(names.as_deref());
        Ok(serde_json::json!({ "events": now }))
    }
}

/// The `events` param of `rpc.subscribe` (required, non-empty) and `rpc.unsubscribe`
/// (optional: absent = all): event names (`doc.patched`) or `*`.
fn event_names(
    params: &Map<String, Value>,
    required: bool,
) -> Result<Option<Vec<String>>, Failure> {
    if let Some(key) = params.keys().find(|k| *k != "events") {
        return Err(CmdError::invalid(format!("/{key}"), "unknown field").into());
    }
    let list = match params.get("events") {
        None | Some(Value::Null) if !required => return Ok(None),
        Some(Value::Array(list)) if !list.is_empty() => list,
        _ => {
            return Err(CmdError::invalid(
                "/events",
                "a non-empty array of event names (`doc.patched`, …) or `*`",
            )
            .into());
        }
    };
    let mut names = Vec::with_capacity(list.len());
    for (i, name) in list.iter().enumerate() {
        match name.as_str() {
            Some(n) if n == ALL_EVENTS || is_event_name(n) => names.push(n.to_owned()),
            _ => {
                return Err(CmdError::invalid(
                    format!("/events/{i}"),
                    "an event name like `doc.patched` (lowercase, dot-separated) or `*`",
                )
                .into());
            }
        }
    }
    Ok(Some(names))
}

/// `namespace.name[.more]`: lowercase ASCII letters, digits and `_`, each part starting with a
/// letter.
fn is_event_name(name: &str) -> bool {
    name.len() <= MAX_EVENT_NAME
        && name.split('.').count() >= 2
        && name.split('.').all(|part| {
            part.starts_with(|c: char| c.is_ascii_lowercase())
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// The lane: runs the connection's mutating calls one at a time, in order. Calls still queued
/// when the socket closes are dropped (the client resends un-acknowledged requests after a
/// reconnect; their `request_id` makes that safe).
async fn lane(
    exec: Exec,
    mut jobs: mpsc::UnboundedReceiver<LaneJob>,
    done: watch::Sender<u64>,
    closed: Arc<AtomicBool>,
) {
    while let Some(job) = jobs.recv().await {
        let LaneJob {
            seq,
            request,
            dest,
            _permit,
        } = job;
        if !closed.load(Ordering::Acquire) {
            let id = request.id.clone();
            let outcome = exec.execute(request).await;
            dest.deliver(id, outcome);
        }
        done.send_replace(seq);
    }
}

/// Why the loop stopped.
enum Stop {
    /// The client closed the socket, or it failed.
    Gone,
    /// The server closes it.
    Close(CloseReason),
}

/// What the loop does next.
enum Next {
    /// The hub asked to close (`None`: its signal is gone).
    Close(Option<CloseReason>),
    /// A response or a notification to write.
    Write(Utf8Bytes),
    /// An in-flight permit (reading may go on).
    Permit(Option<OwnedSemaphorePermit>),
    /// What the socket delivered.
    Read(Option<Result<Message, axum::Error>>),
}

/// Writes `texts` with one flush, unless a close request comes first.
async fn send(
    socket: &mut WebSocket,
    closing: &mut watch::Receiver<Option<CloseReason>>,
    texts: Vec<Utf8Bytes>,
) -> Result<(), Stop> {
    let write = async {
        for text in texts {
            socket.feed(Message::Text(text)).await?;
        }
        socket.flush().await
    };
    tokio::select! {
        biased;
        reason = closing.wait_for(Option::is_some) => {
            Err(reason.ok().and_then(|r| *r).map_or(Stop::Gone, Stop::Close))
        }
        sent = write => sent.map_err(|_| Stop::Gone),
    }
}

/// Serves one WebSocket until either side closes it.
pub(super) async fn run(state: Arc<RpcState>, conn: Arc<dyn RpcConnection>, mut socket: WebSocket) {
    let backend = Arc::clone(&state.backend);
    state.hub.start(move || backend.subscribe());
    let config = state.config;
    let (event_tx, mut events) = mpsc::channel::<Utf8Bytes>(config.event_queue.max(1));
    let subscriber = Arc::new(Subscriber::new(conn.ctx().conn(), event_tx));
    let mut closing = subscriber.closed();
    let key = state.hub.register(Arc::clone(&subscriber));
    let (response_tx, mut responses) = mpsc::unbounded_channel::<String>();
    let (lane_tx, lane_rx) = mpsc::unbounded_channel();
    let (done_tx, done_rx) = watch::channel(0u64);
    let closed = Arc::new(AtomicBool::new(false));
    let exec = Exec {
        state: Arc::clone(&state),
        conn,
    };
    tokio::spawn(lane(exec.clone(), lane_rx, done_tx, Arc::clone(&closed)));
    let mut me = Conn {
        exec,
        subscriber,
        responses: response_tx,
        lane: lane_tx,
        queued: 0,
        done: done_rx,
    };
    let in_flight = Arc::new(Semaphore::new(config.max_in_flight.max(1)));
    let mut permit: Option<OwnedSemaphorePermit> = None;
    tracing::debug!(conn = %me.exec.conn.ctx().conn(), actor = %me.exec.conn.ctx().actor(), "rpc connection open");

    let stop = loop {
        let next = tokio::select! {
            biased;
            reason = closing.wait_for(Option::is_some) => Next::Close(reason.ok().and_then(|r| *r)),
            Some(text) = responses.recv() => Next::Write(text.into()),
            Some(text) = events.recv() => Next::Write(text),
            acquired = Arc::clone(&in_flight).acquire_owned(), if permit.is_none() => {
                Next::Permit(acquired.ok())
            }
            msg = socket.recv(), if permit.is_some() => Next::Read(msg),
        };
        match next {
            Next::Close(reason) => break reason.map_or(Stop::Gone, Stop::Close),
            Next::Write(first) => {
                // Whatever else is ready goes out with the same flush.
                let mut texts = vec![first];
                while texts.len() < MAX_COALESCED {
                    if let Ok(text) = responses.try_recv() {
                        texts.push(text.into());
                    } else if let Ok(text) = events.try_recv() {
                        texts.push(text);
                    } else {
                        break;
                    }
                }
                if let Err(stop) = send(&mut socket, &mut closing, texts).await {
                    break stop;
                }
            }
            Next::Permit(acquired) => permit = acquired,
            Next::Read(None | Some(Err(_))) => break Stop::Gone,
            Next::Read(Some(Ok(msg))) => match msg {
                Message::Text(text) => {
                    if let Some(p) = permit.take() {
                        me.on_text(text.as_str(), p);
                    }
                }
                Message::Binary(_) => {
                    let failure = Failure::framing(
                        codes::INVALID_REQUEST,
                        "",
                        "binary frames are not supported: send JSON-RPC as text",
                    );
                    Dest::Socket(me.responses.clone()).deliver(Some(Value::Null), Err(failure));
                }
                // The WebSocket layer answers pings and the client's close (reading on lets it
                // flush the reply; the stream then ends).
                Message::Ping(_) | Message::Pong(_) | Message::Close(_) => {}
            },
        }
    };

    closed.store(true, Ordering::Release);
    state.hub.unregister(key);
    if let Stop::Close(reason) = stop {
        let frame = CloseFrame {
            code: reason.code(),
            reason: Utf8Bytes::from_static(reason.reason()),
        };
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, socket.send(Message::Close(Some(frame)))).await;
    }
    tracing::debug!(conn = %me.exec.conn.ctx().conn(), "rpc connection closed");
    // Dropping `me` ends the lane once its current call returns; the engine connection closes
    // (rolling back its open transaction) when the last task holding it is done.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_follow_the_wire_grammar() {
        for ok in [
            "doc.patched",
            "project.recovery_available",
            "a.b.c",
            "job.progress2",
        ] {
            assert!(is_event_name(ok), "{ok}");
        }
        for bad in [
            "doc",
            "Doc.patched",
            "doc..patched",
            ".doc",
            "doc.",
            "doc.pa-tched",
            "1a.b",
        ] {
            assert!(!is_event_name(bad), "{bad}");
        }
        assert!(!is_event_name(&format!("a.{}", "b".repeat(200))));
    }
}
