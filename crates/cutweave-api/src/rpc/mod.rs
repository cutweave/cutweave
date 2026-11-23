// SPDX-License-Identifier: GPL-3.0-or-later
//! JSON-RPC 2.0 over WebSocket at `ws://127.0.0.1:<port>/rpc` (WP-1I.02; arch §5.1, §5.2, §6;
//! ADR-0015, ADR-0016, ADR-0017). The protocol reference is `docs/reference/rpc.md`.
//!
//! - **Methods** are the registry's names (`clip.split`, `timeline.get`, `edit.undo`, …) with
//!   `params` = the command input, plus the reserved methods `rpc.hello`, `rpc.subscribe`,
//!   `rpc.unsubscribe` and `project.snapshot`. The result is the command output with
//!   `_meta{rev, committed_rev, request_id?, tx_id?}` added.
//! - **Envelope:** `params._meta{request_id, expected_rev, tx}`. `request_id` is required on
//!   every mutating call (an entry that is not read-only, `view.*` aside); the doc actor
//!   deduplicates it, so a request resent after a reconnect returns the original result. There
//!   is no actor field: the actor is the connection credential's.
//! - **Errors:** a [`CmdError`] becomes `{code, message, data}` where `data` is the error's wire
//!   form (`data.code` is the machine code, `data.path` the JSON Pointer of `invalid`,
//!   `data.current_rev` the revision of `conflict`); [`codes`] lists the numeric codes.
//! - **Events** are notifications named after the event (`doc.patched`, `job.progress`, …)
//!   for the names a connection subscribed to; per-connection bounded queues, close code 4001
//!   `resync_required` on overflow ([`hub`]).
//! - **Ordering:** a connection's mutating calls run one at a time in the order received; a
//!   read-only call waits for the mutating calls received before it, then runs concurrently
//!   (so a long `job.wait` never holds up later edits).
//! - **Connections:** each WebSocket gets one engine connection ([`RpcBackend::connect`], a
//!   connection id of its own); closing the socket closes it, which rolls back its open
//!   transaction (WP-1B.09).
//!
//! The engine implements [`RpcBackend`] (`cutweave-engine` depends on this crate, so the
//! contract is a trait here) and mounts [`router`] through `ApiOptions.mounts`.

mod hub;
mod session;
mod wire;

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{State, ws::WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use cutweave_commands::{ActorCtx, CmdError, ConnId, Registry, RequestId, Rev, TxId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    creds::Credential,
    server::{Authenticated, RequestId as HttpRequestId, WS_TOKEN_SUBPROTOCOL_PREFIX},
};

pub use wire::{codes, error_code};

/// Close code: the connection fell behind its events (queue overflow or a lagging hub); the
/// client must reconnect and resync from `project.snapshot`.
pub const CLOSE_RESYNC_REQUIRED: u16 = 4001;
/// Close code: the engine is stopping.
pub const CLOSE_GOING_AWAY: u16 = 1001;
/// `rpc.subscribe` wildcard: every event.
pub const ALL_EVENTS: &str = "*";

/// The reserved methods (not registry entries).
pub const RESERVED_METHODS: &[&str] = &[
    "rpc.hello",
    "rpc.subscribe",
    "rpc.unsubscribe",
    "project.snapshot",
];

/// A boxed `Send` future (the backend traits are object-safe).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One call of a registered command, query or engine command, with its envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcCall {
    /// The registered name.
    pub name: String,
    /// The params (a JSON object, without `_meta`).
    pub params: Value,
    /// Idempotency key (`_meta.request_id`).
    pub request_id: Option<RequestId>,
    /// `_meta.expected_rev`.
    pub expected_rev: Option<Rev>,
    /// `_meta.tx`.
    pub tx: Option<TxId>,
}

/// What a call returned.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcReply {
    /// The command's output (a JSON object).
    pub output: Value,
    /// The live revision after the call (for a deduplicated repeat: the original one).
    pub rev: Rev,
    /// The committed revision after the call.
    pub committed_rev: Rev,
    /// The request id the doc actor recorded (generated when none was sent), when the call
    /// ran on the doc actor.
    pub request_id: Option<RequestId>,
    /// The transaction it ran in, when it ran on the doc actor.
    pub tx_id: Option<TxId>,
}

/// The document as `project.snapshot` returns it.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcSnapshot {
    /// Its live revision.
    pub rev: Rev,
    /// The committed revision.
    pub committed_rev: Rev,
    /// The project document (JSON).
    pub project: Value,
}

/// What `rpc.hello` reports about the engine.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcStatus {
    /// The live revision.
    pub rev: Rev,
    /// The committed revision.
    pub committed_rev: Rev,
    /// What the engine holds (the engine's `ProjectStatus`: path, name, dirty, recovery, …).
    pub project: Value,
    /// The preview shared-memory ring, once the playback subsystem publishes one (WP-1I.05).
    pub shm: Option<String>,
}

/// One engine event.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcEvent {
    /// The wire name (`doc.patched`, `job.progress`, …): the notification's method.
    pub name: Cow<'static, str>,
    /// The payload (a JSON object): the notification's params.
    pub payload: Value,
    /// The engine connection that caused it, for events that skip their origin
    /// (`view.changed`).
    pub origin: Option<ConnId>,
}

/// Why an event stream yielded no event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RpcEventError {
    /// The subscriber fell behind and missed this many events.
    Lagged(u64),
    /// The fan-out ended (the engine stopped).
    Closed,
}

/// A subscription to the engine's event fan-out.
pub trait RpcEvents: Send + 'static {
    /// The next event.
    fn recv(&mut self) -> BoxFuture<'_, Result<RpcEvent, RpcEventError>>;
}

/// One engine connection: a credential-derived caller with a connection id of its own. The
/// engine closes it (rolling back its open transaction) when the last reference drops.
pub trait RpcConnection: Send + Sync + 'static {
    /// The caller: connection id, actor and access.
    fn ctx(&self) -> &ActorCtx;

    /// Runs `call` without blocking the async runtime.
    fn call(&self, call: RpcCall) -> BoxFuture<'static, Result<RpcReply, CmdError>>;
}

/// What the RPC layer needs from the engine.
pub trait RpcBackend: Send + Sync + 'static {
    /// The engine version (`rpc.hello`).
    fn version(&self) -> String;

    /// The registry: method names and whether they mutate.
    fn registry(&self) -> Arc<Registry>;

    /// Opens a connection for `credential` (one per WebSocket).
    fn connect(&self, credential: &Credential) -> Result<Arc<dyn RpcConnection>, CmdError>;

    /// A new subscription to the event fan-out, starting with the next event.
    fn subscribe(&self) -> Box<dyn RpcEvents>;

    /// The engine's state for `rpc.hello` (may block briefly: called off the async workers).
    fn status(&self) -> Result<RpcStatus, CmdError>;

    /// The live document for `project.snapshot` (may block briefly: called off the async
    /// workers).
    fn snapshot(&self) -> Result<RpcSnapshot, CmdError>;
}

/// Limits of the RPC endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RpcConfig {
    /// Events buffered per connection; a client that falls further behind is closed with
    /// [`CLOSE_RESYNC_REQUIRED`].
    pub event_queue: usize,
    /// Requests (or batches) a connection may have in flight; the server stops reading the
    /// socket beyond that.
    pub max_in_flight: usize,
    /// Largest accepted message.
    pub max_message_bytes: usize,
}

/// Default [`RpcConfig::event_queue`].
pub const DEFAULT_EVENT_QUEUE: usize = 1024;
/// Default [`RpcConfig::max_in_flight`].
pub const DEFAULT_MAX_IN_FLIGHT: usize = 64;
/// Default [`RpcConfig::max_message_bytes`] (16 MiB).
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 16 << 20;

impl Default for RpcConfig {
    fn default() -> RpcConfig {
        RpcConfig {
            event_queue: DEFAULT_EVENT_QUEUE,
            max_in_flight: DEFAULT_MAX_IN_FLIGHT,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
        }
    }
}

/// `rpc.hello` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcClientHello {
    /// The client's name (`cutweave-desktop`, `cutweave-cli`, a script's name).
    pub client: String,
    /// The client's version.
    pub version: String,
}

/// `rpc.hello` result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcServerHello {
    /// The engine version.
    pub engine_version: String,
    /// sha256 of the command catalog (`schema/commands.json`); compare with the client's
    /// generated `SCHEMA_HASH`.
    pub schema_hash: String,
    /// The live document revision.
    pub rev: Rev,
    /// The committed revision.
    pub committed_rev: Rev,
    /// The connection's actor (`user`, `agent:<label>`), derived from its credential.
    pub actor: String,
    /// What the engine holds (path, name, dirty, read-only, recovery).
    pub project: Value,
    /// The preview shared-memory ring, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shm: Option<String>,
}

/// The shared state of a mounted RPC router.
struct RpcState {
    backend: Arc<dyn RpcBackend>,
    registry: Arc<Registry>,
    config: RpcConfig,
    hub: Arc<hub::Hub>,
}

/// The `/rpc` routes: the WebSocket upgrade, behind the server's authentication (mount as
/// [`Mounts::rpc`](crate::Mounts)).
pub fn router(backend: Arc<dyn RpcBackend>, config: RpcConfig) -> Router {
    let state = Arc::new(RpcState {
        registry: backend.registry(),
        backend,
        config,
        hub: Arc::new(hub::Hub::default()),
    });
    Router::new().route("/", get(upgrade)).with_state(state)
}

/// Upgrades to a WebSocket owning one engine connection for the authenticated credential.
async fn upgrade(
    State(state): State<Arc<RpcState>>,
    Extension(auth): Extension<Authenticated>,
    request_id: Option<Extension<HttpRequestId>>,
    mut ws: WebSocketUpgrade,
) -> Response {
    // One engine connection per socket, for the credential (never the per-request `ctx`).
    let conn = match state.backend.connect(&auth.credential) {
        Ok(conn) => conn,
        Err(e) => {
            let body = json!({
                "error": {"code": "unavailable", "message": e.message()},
                "request_id": request_id.map(|Extension(id)| id.0),
            });
            return (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response();
        }
    };
    // A browser that presented its token as a subprotocol needs one echoed back.
    let token_protocol = ws
        .requested_protocols()
        .find(|p| {
            p.to_str()
                .is_ok_and(|p| p.starts_with(WS_TOKEN_SUBPROTOCOL_PREFIX))
        })
        .cloned();
    if let Some(protocol) = token_protocol {
        ws.set_selected_protocol(protocol);
    }
    let limit = state.config.max_message_bytes;
    ws.max_message_size(limit)
        .max_frame_size(limit)
        .on_upgrade(move |socket| session::run(state, conn, socket))
}
