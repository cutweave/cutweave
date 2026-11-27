// SPDX-License-Identifier: GPL-3.0-or-later
//! The two clients of the crash harness: the editor's side, a pipelined JSON-RPC WebSocket
//! (`/rpc`, the `cli` token: a headless engine has no UI credential), and an agent on `/mcp`
//! (rmcp's Streamable HTTP client, the `mcp` token). Both can leave a request in flight while
//! the engine is killed and say afterwards whether its answer still arrived.

use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, CallToolResult, MetaObject, RequestMetaObject};
use rmcp::service::{Peer, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceExt as _};
use serde_json::{Value, json};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::BenchError;
use crate::harness::{ChildEngine, WsClient};

/// How long one answer may take while the engine is alive.
pub const ANSWER_WAIT: Duration = Duration::from_secs(60);
/// How long the harness waits for the answers that were on their way when the engine died.
/// The WebSocket ends at once then; this only bounds the wait.
pub const DRAIN_WAIT: Duration = Duration::from_secs(10);
/// The same for the agent's call. rmcp's HTTP client does not always notice that the engine
/// is gone, so this wait is often spent in full: it is short. An answer that was already
/// received is read well within it; one missed here only makes the harness send the call
/// again under its request id, which is the same check.
pub const AGENT_DRAIN_WAIT: Duration = Duration::from_millis(250);

const META_REV: &str = "io.cutweave/rev";
const META_COMMITTED_REV: &str = "io.cutweave/committed_rev";
const META_REQUEST_ID: &str = "io.cutweave/request_id";

/// What the engine answered.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// The command ran: the engine acknowledged it.
    Done {
        /// Its output.
        output: Value,
        /// The live revision after it.
        rev: u64,
        /// The committed revision after it.
        committed: u64,
    },
    /// The engine refused it (nothing changed).
    Refused(String),
}

impl Answer {
    /// The output of a command that ran.
    pub fn output(&self) -> Option<&Value> {
        match self {
            Answer::Done { output, .. } => Some(output),
            Answer::Refused(_) => None,
        }
    }
}

fn other(what: &str, e: impl std::fmt::Display) -> BenchError {
    BenchError::Other(format!("{what}: {e}"))
}

/// The answer in a JSON-RPC response.
fn rpc_answer(msg: &Value) -> Answer {
    if let Some(error) = msg.get("error") {
        return Answer::Refused(error.to_string());
    }
    let meta = &msg["result"]["_meta"];
    Answer::Done {
        output: msg["result"].clone(),
        rev: meta["rev"].as_u64().unwrap_or(0),
        committed: meta["committed_rev"].as_u64().unwrap_or(0),
    }
}

/// The editor's connection: requests go out without waiting, answers are matched by id.
pub struct Editor {
    ws: WsClient,
    /// Answers read while waiting for another ticket.
    early: std::collections::BTreeMap<u64, Answer>,
}

impl Editor {
    /// Connects to `engine`'s `/rpc`.
    pub fn connect(engine: &ChildEngine) -> Result<Editor, BenchError> {
        Ok(Editor {
            ws: engine.ws()?,
            early: std::collections::BTreeMap::new(),
        })
    }

    /// The params with the request envelope.
    fn envelope(mut params: Value, request_id: Option<&str>, tx: Option<&str>) -> Value {
        let mut meta = serde_json::Map::new();
        if let Some(id) = request_id {
            meta.insert("request_id".into(), json!(id));
        }
        if let Some(tx) = tx {
            meta.insert("tx".into(), json!(tx));
            meta.insert("replace".into(), json!(true));
        }
        if let (Value::Object(map), false) = (&mut params, meta.is_empty()) {
            map.insert("_meta".into(), Value::Object(meta));
        }
        params
    }

    /// Sends a request and returns its ticket.
    pub fn send(
        &mut self,
        method: &str,
        params: &Value,
        request_id: Option<&str>,
        tx: Option<&str>,
    ) -> Result<u64, BenchError> {
        self.ws
            .send(method, &Editor::envelope(params.clone(), request_id, tx))
    }

    /// Waits for the answer to `ticket`. An error: the connection ended first (the request
    /// was in flight when the engine died).
    pub fn answer(&mut self, ticket: u64, wait: Duration) -> Result<Answer, BenchError> {
        loop {
            if let Some(answer) = self.early.remove(&ticket) {
                return Ok(answer);
            }
            let msg = self.ws.next_message(wait)?;
            if let Some(id) = msg["id"].as_u64() {
                self.early.insert(id, rpc_answer(&msg));
            }
        }
    }

    /// Sends a request and waits for its answer.
    pub fn call(
        &mut self,
        method: &str,
        params: &Value,
        request_id: Option<&str>,
        tx: Option<&str>,
    ) -> Result<Answer, BenchError> {
        let ticket = self.send(method, params, request_id, tx)?;
        self.answer(ticket, ANSWER_WAIT)
    }

    /// A read-only call that must succeed; returns its result.
    pub fn query(&mut self, method: &str, params: &Value) -> Result<Value, BenchError> {
        match self.call(method, params, None, None)? {
            Answer::Done { output, .. } => Ok(output),
            Answer::Refused(e) => Err(BenchError::command(method, &e)),
        }
    }
}

/// An MCP tool call that may still be running.
pub struct Pending(JoinHandle<Result<CallToolResult, rmcp::ServiceError>>);

/// An agent connected to `/mcp`.
pub struct Agent {
    rt: Arc<Runtime>,
    service: RunningService<RoleClient, ()>,
    peer: Peer<RoleClient>,
}

impl Agent {
    /// Connects to `engine`'s `/mcp` (initialize handshake included).
    pub fn connect(rt: &Arc<Runtime>, engine: &ChildEngine) -> Result<Agent, BenchError> {
        let url = format!("http://127.0.0.1:{}/mcp", engine.port);
        let config =
            StreamableHttpClientTransportConfig::with_uri(url).auth_header(engine.mcp_token());
        let service = rt
            .block_on(async {
                // The transport starts its worker on the runtime it is made in.
                let transport = StreamableHttpClientTransport::from_config(config);
                tokio::time::timeout(ANSWER_WAIT, ().serve(transport)).await
            })
            .map_err(|_| BenchError::Other("the engine did not answer the MCP handshake".into()))?
            .map_err(|e| other("connect to /mcp", e))?;
        let peer = service.peer().clone();
        Ok(Agent {
            rt: Arc::clone(rt),
            service,
            peer,
        })
    }

    /// Starts a tool call (`item_set_props` for `item.set_props`) with an idempotency key.
    pub fn start(&self, command: &str, args: &Value, request_id: &str) -> Pending {
        let mut params = CallToolRequestParams::new(command.replace('.', "_"));
        if let Value::Object(map) = args {
            params = params.with_arguments(map.clone());
        }
        let mut meta = MetaObject::new();
        meta.0.insert(META_REQUEST_ID.into(), json!(request_id));
        params.meta = Some(RequestMetaObject(meta));
        let peer = self.peer.clone();
        Pending(self.rt.spawn(async move { peer.call_tool(params).await }))
    }

    /// Waits for a started call. An error: no answer (it was in flight when the engine died).
    pub fn finish(&self, pending: Pending, wait: Duration) -> Result<Answer, BenchError> {
        let result = self
            .rt
            .block_on(async { tokio::time::timeout(wait, pending.0).await })
            .map_err(|_| BenchError::Other("no MCP answer in time".into()))?
            .map_err(|e| other("the MCP call task failed", e))?
            .map_err(|e| other("the MCP call failed", e))?;
        if result.is_error == Some(true) {
            let text = result
                .structured_content
                .map_or_else(|| "tool error".to_owned(), |v| v.to_string());
            return Ok(Answer::Refused(text));
        }
        let meta = result.meta.unwrap_or_default();
        let rev = |key: &str| meta.0.get(key).and_then(Value::as_u64).unwrap_or(0);
        Ok(Answer::Done {
            output: result.structured_content.unwrap_or(Value::Null),
            rev: rev(META_REV),
            committed: rev(META_COMMITTED_REV),
        })
    }

    /// Calls a tool and waits for its answer.
    pub fn call(
        &self,
        command: &str,
        args: &Value,
        request_id: &str,
    ) -> Result<Answer, BenchError> {
        let pending = self.start(command, args, request_id);
        self.finish(pending, ANSWER_WAIT)
    }

    /// Ends the session (bounded; the engine may be gone already).
    pub fn close(self) {
        let Agent { rt, service, .. } = self;
        let _ = rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), service.cancel()).await
        });
    }
}
