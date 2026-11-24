// SPDX-License-Identifier: GPL-3.0-or-later
//! Attach mode: a blocking JSON-RPC client of a running engine's `/rpc` WebSocket
//! (`docs/reference/rpc.md`). One connection per CLI invocation, so a batch's transaction
//! lives on one socket and closing it rolls back anything left open.
//!
//! The server does not make up request ids for RPC clients, and a mutating call without one is
//! `invalid` at `/_meta/request_id`: every call here carries a fresh UUIDv7.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cutweave_api::RunFileContents;
use cutweave_api::engines::{self, SelectError, Selector};
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

/// Why no engine was chosen: the reason, and the failure to print (exit 3).
#[derive(Debug)]
pub struct Miss {
    /// What went wrong.
    pub why: SelectError,
    /// The failure the command ends with.
    pub failure: Failure,
}

/// The run-file directory: `dir`, or `~/.cutweave/run`.
///
/// # Errors
/// Exit 3 when there is no home directory.
pub fn run_dir(dir: Option<&Path>) -> Result<PathBuf, Failure> {
    engines::run_dir(dir).ok_or_else(|| {
        Failure::no_engine(
            "no_engine",
            "cannot find the run-file directory (no home directory); pass --run-dir",
            Value::Null,
        )
    })
}

/// Picks the engine to attach to from the run-files in `dir`: the one `selector` names, or
/// without a selector the app's engine whose window had the focus last (`engines::select`).
///
/// # Errors
/// Exit 3: no engine, none matches, or several headless ones without a selector.
pub fn find(dir: Option<&Path>, selector: &Selector) -> Result<RunFileContents, Miss> {
    let dir = run_dir(dir).map_err(|failure| Miss {
        why: SelectError::NoEngine,
        failure,
    })?;
    let found = engines::list(&dir);
    let listing = engines::describe_all(&found);
    engines::select(found, selector).map_err(|why| {
        let failure = match &why {
            SelectError::NoEngine => Failure::no_engine(
                "no_engine",
                format!(
                    "no running engine found in {} (start one with `cutweave engine`, or pass \
                     --headless to run a one-shot engine in this process)",
                    dir.display()
                ),
                Value::Null,
            ),
            SelectError::NoMatch => Failure::no_engine(
                "no_engine",
                format!(
                    "no running engine matches {} (`cutweave engines` lists them)",
                    describe_selector(selector)
                ),
                json!({"engines": listing}),
            ),
            SelectError::Ambiguous => Failure::no_engine(
                "ambiguous_engine",
                "several headless engines are running and no app: choose one with \
                 --project <name or path> or --pid <pid>",
                json!({"engines": listing}),
            ),
        };
        Miss { why, failure }
    })
}

fn describe_selector(selector: &Selector) -> String {
    match (&selector.pid, &selector.project) {
        (Some(pid), Some(project)) => format!("pid {pid} with project `{project}`"),
        (Some(pid), None) => format!("pid {pid}"),
        (None, Some(project)) => format!("project `{project}`"),
        (None, None) => "the request".to_owned(),
    }
}

/// Reads the run-file at `path` (`cutweave mcp --attach --run-file`).
///
/// # Errors
/// Exit 3 when the file cannot be read or parsed, or its engine is not running.
pub fn read_run_file(path: &std::path::Path) -> Result<RunFileContents, Failure> {
    let gone = |what: String| {
        Failure::no_engine(
            "no_engine",
            format!("no running engine at run-file {}: {what}", path.display()),
            Value::Null,
        )
    };
    let text = std::fs::read_to_string(path).map_err(|e| gone(e.to_string()))?;
    let engine: RunFileContents =
        serde_json::from_str(&text).map_err(|e| gone(format!("not a run-file ({e})")))?;
    if !cutweave_api::runfile::process_alive(engine.pid) {
        return Err(gone(format!("process {} is not running", engine.pid)));
    }
    Ok(engine)
}

/// The agent token of `engine` labelled `label` (`cli` for `/rpc`, `mcp` for `/mcp`).
///
/// # Errors
/// Exit 1 (`no_credential`) when the run-file has no such token.
pub fn agent_token<'a>(engine: &'a RunFileContents, label: &str) -> Result<&'a str, Failure> {
    engine
        .agent_tokens
        .iter()
        .find(|t| t.label == label)
        .map(|t| t.token.as_str())
        .ok_or_else(|| {
            Failure::error(
                "no_credential",
                format!(
                    "the run-file of engine {} has no `{label}` agent token",
                    engine.pid
                ),
            )
        })
}

impl Remote {
    /// Connects to `engine` with its `cli` agent token.
    ///
    /// # Errors
    /// Exit 3 when the engine does not answer (a stale run-file), exit 1 without a `cli` token.
    pub fn connect(engine: &RunFileContents) -> Result<Remote, Failure> {
        let token = agent_token(engine, CLI_LABEL)?;
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
        let bearer =
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| lost(e.to_string()))?;
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
