// SPDX-License-Identifier: GPL-3.0-or-later
//! Attach mode: a blocking JSON-RPC client of a running engine's `/rpc` WebSocket
//! (`docs/reference/rpc.md`). One connection per CLI invocation, so a batch's transaction
//! lives on one socket and closing it rolls back anything left open.
//!
//! The server does not make up request ids for RPC clients, and a mutating call without one is
//! `invalid` at `/_meta/request_id`: every call here carries a fresh UUIDv7.

use std::time::Duration;

use cutweave_api::{RunFile, RunFileContents};
use cutweave_commands::RequestId;
use cutweave_model::Uuid7Gen;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::failure::Failure;
use crate::session::Done;

/// The label of the agent token the CLI uses (`ApiOptions::agent_labels`).
const CLI_LABEL: &str = "cli";

/// How long connecting may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A connection to a running engine.
pub struct Remote {
    rt: tokio::runtime::Runtime,
    socket: Socket,
    next_id: u64,
}

/// Picks the engine to attach to from the run-files in `dir`.
///
/// # Errors
/// Exit 3: no engine, or several without `--pid`.
pub fn discover(
    dir: Option<&std::path::Path>,
    pid: Option<u32>,
) -> Result<RunFileContents, Failure> {
    let dir = match dir {
        Some(dir) => dir.to_path_buf(),
        None => RunFile::default_dir().ok_or_else(|| {
            Failure::no_engine(
                "no_engine",
                "cannot find the run-file directory (no home directory); pass --run-dir",
                Value::Null,
            )
        })?,
    };
    let mut found = RunFile::discover(&dir);
    if let Some(pid) = pid {
        found.retain(|c| c.pid == pid);
        return found.into_iter().next().ok_or_else(|| {
            Failure::no_engine(
                "no_engine",
                format!("no running engine with pid {pid} in {}", dir.display()),
                Value::Null,
            )
        });
    }
    match found.len() {
        0 => Err(Failure::no_engine(
            "no_engine",
            format!(
                "no running engine found in {} (start one with `cutweave engine`, or pass \
                 --headless to run a one-shot engine in this process)",
                dir.display()
            ),
            Value::Null,
        )),
        1 => Ok(found.remove(0)),
        _ => Err(Failure::no_engine(
            "ambiguous_engine",
            "several engines are running; pass --pid to choose one",
            json!({"engines": found.iter().map(|c| json!({
                "pid": c.pid, "port": c.port, "project": c.project, "version": c.version,
            })).collect::<Vec<_>>()}),
        )),
    }
}

impl Remote {
    /// Connects to `engine` with its `cli` agent token.
    ///
    /// # Errors
    /// Exit 3 when the engine does not answer (a stale run-file), exit 1 without a `cli` token.
    pub fn connect(engine: &RunFileContents) -> Result<Remote, Failure> {
        let token = engine
            .agent_tokens
            .iter()
            .find(|t| t.label == CLI_LABEL)
            .ok_or_else(|| {
                Failure::error(
                    "no_credential",
                    format!(
                        "the run-file of engine {} has no `{CLI_LABEL}` agent token",
                        engine.pid
                    ),
                )
            })?;
        let lost = |what: String| {
            Failure::no_engine(
                "no_engine",
                format!(
                    "cannot connect to engine {} on port {}: {what}",
                    engine.pid, engine.port
                ),
                Value::Null,
            )
        };
        let url = format!("ws://127.0.0.1:{}/rpc", engine.port);
        let mut request = url.into_client_request().map_err(|e| lost(e.to_string()))?;
        let bearer = HeaderValue::from_str(&format!("Bearer {}", token.token))
            .map_err(|e| lost(e.to_string()))?;
        request.headers_mut().insert("Authorization", bearer);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| lost(e.to_string()))?;
        let socket = rt
            .block_on(async {
                tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request))
                    .await
            })
            .map_err(|_| lost("timed out".into()))?
            .map_err(|e| lost(e.to_string()))?
            .0;
        Ok(Remote {
            rt,
            socket,
            next_id: 1,
        })
    }

    /// One JSON-RPC call; `params` must be an object (or null).
    pub fn call(
        &mut self,
        name: &str,
        params: Value,
        tx: Option<&str>,
        expected_rev: Option<u64>,
    ) -> Result<Done, Failure> {
        let mut params = match params {
            Value::Null => Map::new(),
            Value::Object(map) => map,
            _ => return Err(Failure::usage("the params must be a JSON object")),
        };
        let mut meta = Map::new();
        meta.insert(
            "request_id".into(),
            Value::String(RequestId::generate(&mut Uuid7Gen).to_string()),
        );
        if let Some(tx) = tx {
            meta.insert("tx".into(), json!(tx));
        }
        if let Some(rev) = expected_rev {
            meta.insert("expected_rev".into(), json!(rev));
        }
        params.insert("_meta".into(), Value::Object(meta));
        let id = self.next_id;
        self.next_id += 1;
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": name, "params": params});
        let response = self.rt.block_on(exchange(&mut self.socket, id, &frame))?;
        if let Some(error) = response.get("error") {
            let data = error.get("data").cloned().unwrap_or_else(|| {
                json!({
                    "code": "internal",
                    "message": error.get("message").cloned().unwrap_or(Value::Null),
                })
            });
            return Err(Failure::wire(data));
        }
        let mut output = response.get("result").cloned().unwrap_or(Value::Null);
        let meta = match &mut output {
            Value::Object(map) => map.remove("_meta"),
            _ => None,
        };
        let number = |key: &str| {
            meta.as_ref()
                .and_then(|m| m.get(key))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        Ok(Done {
            output,
            rev: number("rev"),
            committed_rev: number("committed_rev"),
        })
    }

    /// Closes the socket (the engine rolls back anything still open on it).
    pub fn close(mut self) {
        let _ = self.rt.block_on(self.socket.close(None));
    }
}

/// Sends `frame` and reads until the response with `id` arrives (events are not subscribed to,
/// so only that response is expected).
async fn exchange(socket: &mut Socket, id: u64, frame: &Value) -> Result<Value, Failure> {
    let lost = |what: &str| {
        Failure::no_engine(
            "connection_lost",
            format!("the engine closed the connection: {what}"),
            Value::Null,
        )
    };
    socket
        .send(Message::Text(frame.to_string().into()))
        .await
        .map_err(|e| lost(&e.to_string()))?;
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let Ok(value) = serde_json::from_str::<Value>(text.as_str()) else {
                    continue;
                };
                if value.get("id").and_then(Value::as_u64) == Some(id) {
                    return Ok(value);
                }
            }
            Some(Ok(Message::Close(_))) | None => return Err(lost("end of stream")),
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(lost(&e.to_string())),
        }
    }
}
