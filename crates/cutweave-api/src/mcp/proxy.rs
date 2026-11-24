// SPDX-License-Identifier: GPL-3.0-or-later
//! The stdio attach proxy (WP-1J.03; arch §14, ADR-0009): `cutweave mcp --attach`.
//!
//! An agent host starts `cutweave mcp --attach` as a stdio MCP server; the proxy forwards every
//! JSON-RPC message, both ways and unchanged (requests, responses, notifications, ids), between
//! stdio and a running engine's Streamable HTTP endpoint (`/mcp`, `/mcp/minimal`, `/mcp/full`),
//! authenticated with the run-file's `mcp` agent token, so the agent edits the same document as
//! the user in the desktop app.
//!
//! The upstream side is rmcp's own Streamable HTTP client transport, driven at the message
//! level, so the revision is whatever the agent negotiates with the engine:
//!
//! - **2025-11-25:** the agent's `initialize` opens one upstream MCP session
//!   (`Mcp-Session-Id`), which is one engine connection. Closing stdin ends it: the proxy
//!   deletes the session, and the engine rolls back a transaction the session still holds.
//! - **2026-07-28:** `server/discover` and requests with `_meta`; each request is stateless
//!   upstream (a fresh engine connection per request), so no transaction outlives a request.
//!
//! **Engine gone.** A watchdog polls the engine process (every [`BUSY_CHECK`] while a request
//! is in flight, every [`IDLE_CHECK`] otherwise, and at once when a request arrives after an
//! idle spell or a POST fails). When the engine is gone, every request still in flight gets a
//! JSON-RPC error (`-32603`, `data.code = "connection_lost"`) and the proxy ends with
//! [`ProxyEnd::Lost`] (the CLI exits 3). Polling the process rather than the port matters on
//! Windows, where a refused loopback connection takes about two seconds to report.

use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::{FutureExt as _, StreamExt as _, stream::FuturesUnordered};
use rmcp::{
    model::{
        ClientJsonRpcMessage, ClientNotification, ErrorCode, ErrorData, JsonRpcMessage, RequestId,
        ServerJsonRpcMessage,
    },
    service::{RoleClient, RoleServer},
    transport::{
        StreamableHttpClientTransport, Transport, async_rw::AsyncRwTransport,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Notify;

use crate::engines;
use crate::mcp::Profile;
use crate::mcp::switch::{self, Directory, Observed, Switchboard};

/// How often the watchdog checks the engine while a request is in flight.
pub const BUSY_CHECK: Duration = Duration::from_millis(400);
/// How often the watchdog checks the engine while nothing is in flight.
pub const IDLE_CHECK: Duration = Duration::from_secs(5);
/// How long ending the upstream session (`DELETE`) may take when stdin closes.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
/// How long requests in flight when stdin closes may still take (`job_wait` blocks up to 60 s).
const DRAIN_TIMEOUT: Duration = Duration::from_secs(90);

/// The engine a proxy forwards to.
#[derive(Clone)]
pub struct Upstream {
    /// The endpoint URL ([`Upstream::endpoint`]).
    pub url: String,
    /// The bearer token: the run-file's `mcp` agent token (never the UI credential).
    pub token: String,
    /// The engine's process id (for messages).
    pub pid: u32,
    /// Whether the engine process still runs; called on the blocking pool.
    pub alive: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl std::fmt::Debug for Upstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upstream")
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

impl Upstream {
    /// `http://127.0.0.1:<port>/mcp` for the standard profile, `/mcp/minimal`, `/mcp/full`.
    #[must_use]
    pub fn endpoint(port: u16, profile: Profile) -> String {
        match profile.route() {
            "/" => format!("http://127.0.0.1:{port}/mcp"),
            route => format!("http://127.0.0.1:{port}/mcp{route}"),
        }
    }
}

/// How a proxy session ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProxyEnd {
    /// The client closed stdin; the upstream session was ended.
    InputClosed,
    /// The shutdown future completed first (Ctrl-C / SIGTERM); the upstream session was ended.
    Shutdown,
    /// The engine is gone or the connection to it failed for good. Requests in flight were
    /// answered with errors.
    Lost {
        /// What happened.
        message: String,
    },
}

/// The upstream's HTTP transport. One function builds every one, so a switched-to engine's
/// transport has the type of the first.
fn open(upstream: &Upstream) -> impl Transport<RoleClient> + 'static {
    StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(upstream.url.clone())
            .auth_header(upstream.token.clone()),
    )
}

/// What the watchdog asks: whether the engine in use still runs. `engine_attach` replaces it.
type Alive = Arc<Mutex<Arc<dyn Fn() -> bool + Send + Sync>>>;

/// Proxies the process's stdin and stdout to `upstream`.
pub async fn proxy_stdio(upstream: Upstream, shutdown: impl Future<Output = ()>) -> ProxyEnd {
    proxy_stdio_with(upstream, None, shutdown).await
}

/// [`proxy_stdio`] with the tools `engines_list` and `engine_attach` (`directory` says where
/// the engines are).
pub async fn proxy_stdio_with(
    upstream: Upstream,
    directory: Option<Directory>,
    shutdown: impl Future<Output = ()>,
) -> ProxyEnd {
    let (read, write) = rmcp::transport::stdio_pipes();
    proxy_with(upstream, directory, read, write, shutdown).await
}

/// Proxies `read` (client to server) and `write` (server to client), one JSON-RPC message per
/// line, to `upstream` until `read` ends, `shutdown` completes or the engine is gone.
pub async fn proxy<R, W>(
    upstream: Upstream,
    read: R,
    write: W,
    shutdown: impl Future<Output = ()>,
) -> ProxyEnd
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    proxy_with(upstream, None, read, write, shutdown).await
}

/// [`proxy`] with the proxy tools of [`switch`] when `directory` is given: the client can list
/// the engines and move to another one (`engine_attach`) without restarting.
// One select loop over five sources; splitting it would mean passing its whole state around.
#[allow(clippy::too_many_lines)]
pub async fn proxy_with<R, W>(
    upstream: Upstream,
    directory: Option<Directory>,
    read: R,
    write: W,
    shutdown: impl Future<Output = ()>,
) -> ProxyEnd
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let mut down = AsyncRwTransport::<RoleServer, R, W>::new_server(read, write);
    let mut up = open(&upstream);
    let mut pid = upstream.pid;
    let mut board = directory.map(Switchboard::new);
    // Upstream sends complete when their POST does; they stay alive until then, because
    // dropping one cancels its request. Pushed in arrival order, they reach the upstream
    // worker in that order.
    let mut sends = FuturesUnordered::new();
    let mut flight = InFlight::default();
    let mut last_error: Option<String> = None;
    let alive: Alive = Arc::new(Mutex::new(Arc::clone(&upstream.alive)));
    let watchdog = watch_engine(
        Arc::clone(&alive),
        Arc::clone(&flight.busy),
        Arc::clone(&flight.kick),
    );
    // After stdin closes, requests already sent are still answered (as rmcp's own server
    // does), for at most this long.
    let drained = tokio::time::sleep(Duration::MAX);
    tokio::pin!(watchdog, shutdown, drained);
    let mut input_open = true;
    let end = loop {
        if !input_open && flight.pending.is_empty() {
            break ProxyEnd::InputClosed;
        }
        tokio::select! {
            biased;
            // Not `Some((id, Err(e)))`: a refuted pattern would stop polling the other sends
            // until another branch fires.
            Some((id, sent)) = sends.next(), if !sends.is_empty() => {
                if let Err(e) = sent {
                    let message = format!("cannot reach the Cutweave engine (pid {pid}): {e}");
                    tracing::warn!(error = %e, "MCP proxy: a message did not reach the engine");
                    if let Some(id) = id
                        && flight.failed(&id)
                    {
                        let _ = down.send(lost_error(id, &message, pid)).await;
                    }
                    last_error = Some(message);
                }
            }
            message = up.receive() => {
                let Some(message) = message else {
                    break ProxyEnd::Lost {
                        message: last_error.take().unwrap_or_else(|| {
                            format!("the connection to the Cutweave engine (pid {pid}) closed")
                        }),
                    };
                };
                flight.answered(&message);
                let message = match board.as_mut() {
                    Some(board) => board.patch(message),
                    None => message,
                };
                if down.send(message).await.is_err() {
                    // Stdout is gone: so is the client.
                    break ProxyEnd::InputClosed;
                }
            }
            message = down.receive(), if input_open => {
                let Some(message) = message else {
                    input_open = false;
                    drained
                        .as_mut()
                        .reset(tokio::time::Instant::now() + DRAIN_TIMEOUT);
                    continue;
                };
                let observed = match board.as_mut() {
                    Some(board) => board.observe(&message),
                    None => Observed::Forward,
                };
                let (id, tool, args) = match observed {
                    Observed::Forward => {
                        let id = flight.sent(&message);
                        sends.push(up.send(message).map(move |sent| (id, sent)));
                        continue;
                    }
                    Observed::Call { id, tool, args } => (id, tool, args),
                };
                let Some(board) = board.as_mut() else { continue };
                let engines = {
                    let dir = board.run_dir().to_path_buf();
                    tokio::task::spawn_blocking(move || engines::list(&dir))
                        .await
                        .unwrap_or_default()
                };
                let mut notify = false;
                let outcome = if tool == "engines_list" {
                    Ok(board.engines_list(&engines, pid))
                } else {
                    match board.resolve(&args, engines, pid) {
                        Err(error) => Err(error),
                        Ok(None) => Ok(json!({"attached": {"pid": pid}, "catalog_changed": false})),
                        Ok(Some(engine)) => match board.upstream(&engine) {
                            None => Err(switch::tool_error(
                                "no_credential",
                                "the engine's run-file has no `mcp` token",
                            )),
                            Some(target) => {
                                let mut next = open(&target);
                                match board.connect(&mut next, &engine).await {
                                    Err(error) => {
                                        let _ = tokio::time::timeout(CLOSE_TIMEOUT, next.close()).await;
                                        Err(error)
                                    }
                                    Ok(switched) => {
                                        // The new engine answers: the old session ends and
                                        // whatever was in flight on it is over.
                                        sends.clear();
                                        let mut old = std::mem::replace(&mut up, next);
                                        if tokio::time::timeout(CLOSE_TIMEOUT, old.close())
                                            .await
                                            .is_err()
                                        {
                                            tracing::warn!("MCP proxy: ending the old session timed out");
                                        }
                                        let note = format!(
                                            "the client moved to engine {}; this request did not finish",
                                            target.pid
                                        );
                                        for pending in flight.pending.drain() {
                                            let _ = down.send(switched_error(pending, &note)).await;
                                        }
                                        flight.busy.store(false, Ordering::Relaxed);
                                        pid = target.pid;
                                        *alive.lock().unwrap_or_else(PoisonError::into_inner) =
                                            Arc::clone(&target.alive);
                                        if switched.catalog_changed {
                                            tracing::info!("MCP proxy: the engine's tools differ");
                                        }
                                        notify = switched.catalog_changed;
                                        Ok(switched.result)
                                    }
                                }
                            }
                        },
                    }
                };
                let (value, is_error) = match &outcome {
                    Ok(value) => (value, false),
                    Err(value) => (value, true),
                };
                if let Some(reply) = switch::tool_result(&id, value, is_error) {
                    let _ = down.send(reply).await;
                }
                if std::mem::take(&mut notify)
                    && let Some(changed) = switch::list_changed()
                {
                    let _ = down.send(changed).await;
                }
            }
            () = &mut watchdog => break ProxyEnd::Lost {
                message: format!("the Cutweave engine (pid {pid}) is gone"),
            },
            () = &mut shutdown => break ProxyEnd::Shutdown,
            () = &mut drained, if !input_open => {
                tracing::warn!(
                    pending = flight.pending.len(),
                    "MCP proxy: input closed; abandoning requests still in flight"
                );
                break ProxyEnd::InputClosed;
            }
        }
    };
    if let ProxyEnd::Lost { message } = &end {
        tracing::warn!(%message, "MCP proxy: lost the engine");
        for id in flight.pending.drain() {
            let _ = down.send(lost_error(id, message, pid)).await;
        }
        // No upstream close: the engine cannot answer a DELETE any more.
    } else {
        // Abandon what is in flight, then end the upstream session (the engine rolls back the
        // session's open transaction).
        drop(sends);
        if tokio::time::timeout(CLOSE_TIMEOUT, up.close())
            .await
            .is_err()
        {
            tracing::warn!("MCP proxy: ending the upstream session timed out");
        }
    }
    let _ = down.close().await;
    end
}

/// The client's requests still waiting for an answer, and the watchdog's view of them.
#[derive(Default)]
struct InFlight {
    pending: HashSet<RequestId>,
    /// Whether anything is pending (the watchdog's cadence).
    busy: Arc<AtomicBool>,
    /// Asks the watchdog to check the engine now.
    kick: Arc<Notify>,
}

impl InFlight {
    /// A client message on its way upstream; returns its request id, if it is a request.
    fn sent(&mut self, message: &ClientJsonRpcMessage) -> Option<RequestId> {
        if let Some(id) = cancelled(message) {
            // A cancelled request gets no response.
            self.remove(id);
        }
        let JsonRpcMessage::Request(request) = message else {
            return None;
        };
        self.pending.insert(request.id.clone());
        if !self.busy.swap(true, Ordering::Relaxed) {
            // The first request after an idle spell: check the engine now.
            self.kick.notify_one();
        }
        Some(request.id.clone())
    }

    /// An upstream message on its way to the client.
    fn answered(&mut self, message: &ServerJsonRpcMessage) {
        if let Some(id) = answered(message) {
            self.remove(id);
        }
    }

    /// A message that did not reach the engine: whether `id` was still pending. Asks the
    /// watchdog to check the engine.
    fn failed(&mut self, id: &RequestId) -> bool {
        self.kick.notify_one();
        self.remove(id)
    }

    fn remove(&mut self, id: &RequestId) -> bool {
        let removed = self.pending.remove(id);
        self.busy.store(!self.pending.is_empty(), Ordering::Relaxed);
        removed
    }
}

/// The id a response or error answers.
fn answered(message: &ServerJsonRpcMessage) -> Option<&RequestId> {
    match message {
        JsonRpcMessage::Response(response) => Some(&response.id),
        JsonRpcMessage::Error(error) => error.id.as_ref(),
        _ => None,
    }
}

/// The request a client's `notifications/cancelled` cancels.
fn cancelled(message: &ClientJsonRpcMessage) -> Option<&RequestId> {
    match message {
        JsonRpcMessage::Notification(notification) => match &notification.notification {
            ClientNotification::CancelledNotification(cancel) => cancel.params.request_id.as_ref(),
            _ => None,
        },
        _ => None,
    }
}

/// The error a request in flight gets when the client moves to another engine.
fn switched_error(id: RequestId, message: &str) -> ServerJsonRpcMessage {
    ServerJsonRpcMessage::error(
        ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            message.to_owned(),
            Some(json!({"code": "engine_switched"})),
        ),
        Some(id),
    )
}

/// The error a request in flight gets when the engine is gone.
fn lost_error(id: RequestId, message: &str, pid: u32) -> ServerJsonRpcMessage {
    ServerJsonRpcMessage::error(
        ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            message.to_owned(),
            Some(json!({
                "code": "connection_lost",
                "pid": pid,
                "hint": "start Cutweave again, then reconnect this MCP server",
            })),
        ),
        Some(id),
    )
}

/// Completes when `alive` says the engine is gone (see the module docs).
async fn watch_engine(alive: Alive, busy: Arc<AtomicBool>, kick: Arc<Notify>) {
    loop {
        let interval = if busy.load(Ordering::Relaxed) {
            BUSY_CHECK
        } else {
            IDLE_CHECK
        };
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            () = kick.notified() => {}
        }
        let check = Arc::clone(&alive.lock().unwrap_or_else(PoisonError::into_inner));
        if !tokio::task::spawn_blocking(move || check())
            .await
            .unwrap_or(true)
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(line: &str) -> ClientJsonRpcMessage {
        serde_json::from_str(line).unwrap()
    }

    fn server(line: &str) -> ServerJsonRpcMessage {
        serde_json::from_str(line).unwrap()
    }

    #[test]
    fn cancellations_and_answers_name_their_request() {
        let cancel = client(
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}"#,
        );
        assert_eq!(cancelled(&cancel), Some(&RequestId::Number(7)));
        let request = client(r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#);
        assert_eq!(cancelled(&request), None);

        let response = server(r#"{"jsonrpc":"2.0","id":"a","result":{}}"#);
        assert_eq!(answered(&response), Some(&RequestId::String("a".into())));
        let error = server(r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"no"}}"#);
        assert_eq!(answered(&error), Some(&RequestId::Number(3)));
        let note = server(r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#);
        assert_eq!(answered(&note), None);
    }

    #[test]
    fn a_lost_engine_error_is_an_internal_error_with_a_machine_code() {
        let e = serde_json::to_value(lost_error(RequestId::Number(1), "gone", 42)).unwrap();
        assert_eq!(e["id"], 1);
        assert_eq!(e["error"]["code"], -32603);
        assert_eq!(e["error"]["data"]["code"], "connection_lost");
        assert_eq!(e["error"]["data"]["pid"], 42);
    }
}
