// SPDX-License-Identifier: GPL-3.0-or-later
//! The attach proxy's own tools (WP-1J.04; arch §14): `engines_list` and `engine_attach`.
//!
//! The proxy passes every message through unchanged ([`proxy`](super::proxy)), with three
//! exceptions that live here:
//!
//! - **`tools/list`** results get the two proxy tools appended, and the engine's own tools are
//!   remembered, to tell later whether another engine's catalog differs.
//! - **`initialize` / `server/discover`** results declare `tools.listChanged`.
//! - **`tools/call`** of a proxy tool is answered by the proxy and never reaches an engine.
//!
//! The proxy tools exist only here: the engine's `/mcp` lists none of them, so the engine's
//! tool budget (`STANDARD_BUDGET`) is not touched. They are lean on purpose (see
//! [`proxy_tools`]); the proxy's `tools/list` is the engine's plus these two.
//!
//! `engine_attach` connects to the chosen engine first (replaying the client's `initialize`
//! handshake, so the new upstream session looks like the one the client opened) and only then
//! drops the old engine, so a failed attach changes nothing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use rmcp::{
    model::{ClientJsonRpcMessage, JsonRpcMessage, RequestId, ServerJsonRpcMessage},
    service::RoleClient,
    transport::Transport,
};
use serde_json::{Value, json};

use crate::engines::{self, SelectError, Selector};
use crate::mcp::Profile;
use crate::mcp::proxy::Upstream;
use crate::runfile::{RunFileContents, process_alive};

/// The names of the tools only the proxy has.
pub const PROXY_TOOLS: [&str; 2] = ["engines_list", "engine_attach"];

/// How long the new engine may take to answer the replayed handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The agent token label the proxy authenticates with.
const MCP_LABEL: &str = "mcp";

/// Where the proxy finds the other engines: the run-files in `run_dir`.
#[derive(Clone, Debug)]
pub struct Directory {
    /// The run-file directory.
    pub run_dir: PathBuf,
    /// The profile whose endpoint the proxy connects to.
    pub profile: Profile,
}

impl Directory {
    /// The `Upstream` for `engine` (its `mcp` token), if the run-file has one.
    #[must_use]
    pub fn upstream(&self, engine: &RunFileContents) -> Option<Upstream> {
        let token = engine
            .agent_tokens
            .iter()
            .find(|t| t.label == MCP_LABEL)?
            .token
            .clone();
        let pid = engine.pid;
        Some(Upstream {
            url: Upstream::endpoint(engine.port, self.profile),
            token,
            pid,
            alive: std::sync::Arc::new(move || process_alive(pid)),
        })
    }
}

/// The two tool definitions appended to the engine's list. Lean: a short description and a
/// schema without prose, since every client pays for them on every session.
#[must_use]
pub fn proxy_tools() -> Vec<Value> {
    let hints = |read_only: bool| {
        json!({
            "readOnlyHint": read_only,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        })
    };
    vec![
        json!({
            "name": "engines_list",
            "description": "List running Cutweave engines (app and headless) with their \
                            project; `current` is the one you use.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": hints(true),
        }),
        json!({
            "name": "engine_attach",
            "description": "Switch to another engine by project name or path, or by pid; \
                            later tools act on it.",
            "inputSchema": {
                "type": "object",
                "properties": {"project": {"type": "string"}, "pid": {"type": "integer"}},
            },
            "annotations": hints(false),
        }),
    ]
}

/// What the proxy does with a client message.
pub enum Observed {
    /// Forward it.
    Forward,
    /// Answer a proxy tool's call here.
    Call {
        /// The request to answer.
        id: RequestId,
        /// `engines_list` or `engine_attach`.
        tool: &'static str,
        /// The call's `arguments`.
        args: Value,
    },
}

/// A response to patch on its way to the client.
#[derive(Clone, Copy)]
enum Patch {
    /// `tools/list`: append the proxy tools.
    Tools,
    /// `initialize` or `server/discover`: declare `tools.listChanged`.
    Capabilities,
}

/// The proxy's view of the session: what it needs to replay on another engine.
pub struct Switchboard {
    directory: Directory,
    /// The client's `initialize` request and its `notifications/initialized`.
    init: Option<ClientJsonRpcMessage>,
    initialized: Option<ClientJsonRpcMessage>,
    /// The `_meta` of the last client request (stateless revision).
    meta: Option<Value>,
    patches: HashMap<RequestId, Patch>,
    /// The engine's tools the client last saw.
    catalog: Option<Value>,
}

/// An engine the proxy switched to.
pub struct Switched {
    /// What the tool returns.
    pub result: Value,
    /// Whether its tools differ from what the client last saw.
    pub catalog_changed: bool,
}

impl Switchboard {
    /// A switchboard over `directory`.
    #[must_use]
    pub fn new(directory: Directory) -> Switchboard {
        Switchboard {
            directory,
            init: None,
            initialized: None,
            meta: None,
            patches: HashMap::new(),
            catalog: None,
        }
    }

    /// Looks at a client message on its way to the engine.
    pub fn observe(&mut self, message: &ClientJsonRpcMessage) -> Observed {
        let Ok(value) = serde_json::to_value(message) else {
            return Observed::Forward;
        };
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        let params = value.get("params");
        if let Some(meta) = params.and_then(|p| p.get("_meta")) {
            self.meta = Some(meta.clone());
        }
        let JsonRpcMessage::Request(request) = message else {
            if method == "notifications/initialized" {
                self.initialized = Some(message.clone());
            }
            return Observed::Forward;
        };
        match method {
            "initialize" => {
                self.init = Some(message.clone());
                self.patches.insert(request.id.clone(), Patch::Capabilities);
            }
            "server/discover" => {
                self.patches.insert(request.id.clone(), Patch::Capabilities);
            }
            "tools/list" => {
                self.patches.insert(request.id.clone(), Patch::Tools);
            }
            "tools/call" => {
                let name = params
                    .and_then(|p| p.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some(tool) = PROXY_TOOLS.into_iter().find(|t| *t == name) {
                    return Observed::Call {
                        id: request.id.clone(),
                        tool,
                        args: params
                            .and_then(|p| p.get("arguments"))
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                    };
                }
            }
            _ => {}
        }
        Observed::Forward
    }

    /// Patches an engine message on its way to the client.
    pub fn patch(&mut self, message: ServerJsonRpcMessage) -> ServerJsonRpcMessage {
        let Ok(mut value) = serde_json::to_value(&message) else {
            return message;
        };
        let Some(id) = value
            .get("id")
            .and_then(|id| serde_json::from_value::<RequestId>(id.clone()).ok())
        else {
            return message;
        };
        let Some(patch) = self.patches.remove(&id) else {
            return message;
        };
        let Some(result) = value.get_mut("result") else {
            return message;
        };
        match patch {
            Patch::Capabilities => {
                if let Some(tools) = result
                    .pointer_mut("/capabilities/tools")
                    .and_then(Value::as_object_mut)
                {
                    tools.insert("listChanged".into(), json!(true));
                }
            }
            Patch::Tools => {
                let Some(tools) = result.get_mut("tools").and_then(Value::as_array_mut) else {
                    return message;
                };
                self.catalog = Some(Value::Array(tools.clone()));
                tools.extend(proxy_tools());
            }
        }
        serde_json::from_value(value).unwrap_or(message)
    }

    /// The run-file directory (read it with [`engines::list`], which blocks).
    #[must_use]
    pub fn run_dir(&self) -> &std::path::Path {
        &self.directory.run_dir
    }

    /// The upstream for `engine`, if its run-file has an `mcp` token.
    #[must_use]
    pub fn upstream(&self, engine: &RunFileContents) -> Option<Upstream> {
        self.directory.upstream(engine)
    }

    /// `engines_list`: every engine of `engines`, the one in use marked `current`.
    #[must_use]
    pub fn engines_list(&self, engines: &[RunFileContents], current: u32) -> Value {
        let list: Vec<Value> = engines
            .iter()
            .map(|e| {
                let mut v = engines::describe(e);
                v["current"] = json!(e.pid == current);
                v
            })
            .collect();
        json!({ "engines": list })
    }

    /// Resolves `engine_attach`'s arguments to an engine other than `current`.
    ///
    /// # Errors
    /// The tool error to return (`code`, `message`).
    pub fn resolve(
        &self,
        args: &Value,
        engines: Vec<RunFileContents>,
        current: u32,
    ) -> Result<Option<RunFileContents>, Value> {
        let pid = match args.get("pid") {
            None | Some(Value::Null) => None,
            Some(v) => match v.as_u64().and_then(|p| u32::try_from(p).ok()) {
                Some(pid) => Some(pid),
                None => return Err(tool_error("invalid", "`pid` must be a process id")),
            },
        };
        let project = match args.get("project") {
            None | Some(Value::Null) => None,
            Some(Value::String(p)) if !p.trim().is_empty() => Some(p.clone()),
            Some(_) => {
                return Err(tool_error(
                    "invalid",
                    "`project` must be a project name or path",
                ));
            }
        };
        let selector = Selector { pid, project };
        if selector.is_empty() {
            return Err(tool_error("invalid", "give `project` or `pid`"));
        }
        let listing = engines::describe_all(&engines);
        match engines::select(engines, &selector) {
            Ok(engine) if engine.pid == current => Ok(None),
            Ok(engine) => Ok(Some(engine)),
            Err(SelectError::NoEngine | SelectError::NoMatch | SelectError::Ambiguous) => {
                let mut error = tool_error(
                    "no_engine",
                    "no running engine matches; `engines_list` shows them",
                );
                error["engines"] = listing;
                Err(error)
            }
        }
    }

    /// Connects `next` (a fresh transport to `engine`) the way the client's session was set
    /// up, and reads the engine's tool list.
    ///
    /// # Errors
    /// The tool error to return: the engine did not answer in time or refused.
    pub async fn connect(
        &mut self,
        next: &mut impl Transport<RoleClient>,
        engine: &RunFileContents,
    ) -> Result<Switched, Value> {
        let fail = |what: String| {
            tool_error(
                "attach_failed",
                &format!("cannot attach to engine {}: {what}", engine.pid),
            )
        };
        let tools = handshake(
            next,
            self.init.as_ref(),
            self.initialized.as_ref(),
            self.meta.as_ref(),
        )
        .await
        .map_err(fail)?;
        let catalog_changed = self.catalog.as_ref().is_some_and(|seen| *seen != tools);
        if self.catalog.is_some() {
            self.catalog = Some(tools);
        }
        Ok(Switched {
            result: json!({"attached": engines::describe(engine), "catalog_changed": catalog_changed}),
            catalog_changed,
        })
    }
}

/// A tool error as the engine's own look: `{code, message}`.
#[must_use]
pub fn tool_error(code: &str, message: &str) -> Value {
    json!({"code": code, "message": message})
}

/// The answer to a proxy tool's call. An error is `{"error": {code, message, ...}}`, the shape of
/// the engine's own tool errors.
#[must_use]
pub fn tool_result(
    id: &RequestId,
    structured: &Value,
    is_error: bool,
) -> Option<ServerJsonRpcMessage> {
    let structured = if is_error {
        json!({ "error": structured })
    } else {
        structured.clone()
    };
    serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": structured.to_string()}],
            "structuredContent": structured,
            "isError": is_error,
        },
    }))
    .ok()
}

/// `notifications/tools/list_changed`.
#[must_use]
pub fn list_changed() -> Option<ServerJsonRpcMessage> {
    serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "method": "notifications/tools/list_changed",
    }))
    .ok()
}

/// Replays the client's handshake on `next` and returns the engine's `tools` array.
async fn handshake(
    next: &mut impl Transport<RoleClient>,
    init: Option<&ClientJsonRpcMessage>,
    initialized: Option<&ClientJsonRpcMessage>,
    meta: Option<&Value>,
) -> Result<Value, String> {
    if let Some(init) = init {
        let id = match init {
            JsonRpcMessage::Request(r) => r.id.clone(),
            _ => return Err("the client's initialize is not a request".into()),
        };
        next.send(init.clone()).await.map_err(|e| e.to_string())?;
        answer_to(next, &id).await?;
        if let Some(initialized) = initialized {
            next.send(initialized.clone())
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    let id = RequestId::String("cutweave-proxy-tools".into());
    let mut params = json!({});
    if let Some(meta) = meta {
        params["_meta"] = meta.clone();
    }
    let request: ClientJsonRpcMessage = serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/list", "params": params,
    }))
    .map_err(|e| e.to_string())?;
    next.send(request).await.map_err(|e| e.to_string())?;
    let result = answer_to(next, &id).await?;
    Ok(result.get("tools").cloned().unwrap_or_else(|| json!([])))
}

/// Reads from `next` until the response to `id`; its `result`, or the error's message.
async fn answer_to(next: &mut impl Transport<RoleClient>, id: &RequestId) -> Result<Value, String> {
    let wanted = serde_json::to_value(id).map_err(|e| e.to_string())?;
    let read = async {
        loop {
            let Some(message) = next.receive().await else {
                return Err("the engine closed the connection".to_owned());
            };
            let Ok(value) = serde_json::to_value(&message) else {
                continue;
            };
            if value.get("id") != Some(&wanted) {
                continue;
            }
            if let Some(error) = value.get("error") {
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the engine refused");
                return Err(text.to_owned());
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    };
    tokio::time::timeout(HANDSHAKE_TIMEOUT, read)
        .await
        .map_err(|_| "the engine did not answer in time".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Switchboard {
        Switchboard::new(Directory {
            run_dir: PathBuf::from("/nowhere"),
            profile: Profile::Standard,
        })
    }

    fn client(line: &str) -> ClientJsonRpcMessage {
        serde_json::from_str(line).unwrap()
    }

    fn server(line: &str) -> ServerJsonRpcMessage {
        serde_json::from_str(line).unwrap()
    }

    #[test]
    fn proxy_tool_calls_are_answered_here_and_other_calls_pass() {
        let mut b = board();
        let call = client(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"engine_attach","arguments":{"project":"B"}}}"#,
        );
        match b.observe(&call) {
            Observed::Call { id, tool, args } => {
                assert_eq!(id, RequestId::Number(4));
                assert_eq!(tool, "engine_attach");
                assert_eq!(args["project"], "B");
            }
            Observed::Forward => panic!("a proxy tool call was forwarded"),
        }
        let other = client(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"timeline_get","arguments":{}}}"#,
        );
        assert!(matches!(b.observe(&other), Observed::Forward));
    }

    #[test]
    fn the_tool_list_gains_the_proxy_tools_and_the_engines_are_remembered() {
        let mut b = board();
        let list = client(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        assert!(matches!(b.observe(&list), Observed::Forward));
        let reply = server(
            r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"x","inputSchema":{"type":"object"}}]}}"#,
        );
        let patched = serde_json::to_value(b.patch(reply)).unwrap();
        let names: Vec<&str> = patched["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["x", "engines_list", "engine_attach"]);
        assert_eq!(b.catalog.as_ref().unwrap().as_array().unwrap().len(), 1);
        // An answer nobody asked the proxy to patch is untouched.
        let other = server(r#"{"jsonrpc":"2.0","id":9,"result":{"tools":[]}}"#);
        let same = serde_json::to_value(b.patch(other)).unwrap();
        assert_eq!(same["result"]["tools"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn the_initialize_result_declares_list_changed() {
        let mut b = board();
        let init = client(
            r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
        );
        assert!(matches!(b.observe(&init), Observed::Forward));
        assert!(b.init.is_some());
        let reply = server(
            r#"{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"s","version":"1"}}}"#,
        );
        let patched = serde_json::to_value(b.patch(reply)).unwrap();
        assert_eq!(
            patched["result"]["capabilities"]["tools"]["listChanged"],
            true
        );
    }

    #[test]
    fn engine_attach_needs_a_project_or_a_pid() {
        let b = board();
        let e = b.resolve(&json!({}), Vec::new(), 1).unwrap_err();
        assert_eq!(e["code"], "invalid");
        let e = b.resolve(&json!({"pid": "x"}), Vec::new(), 1).unwrap_err();
        assert_eq!(e["code"], "invalid");
        let e = b
            .resolve(&json!({"project": "B"}), Vec::new(), 1)
            .unwrap_err();
        assert_eq!(e["code"], "no_engine");
    }

    /// An engine's MCP endpoint that answers `initialize` and `tools/list` and remembers what
    /// it was sent.
    struct Fake {
        replies: std::collections::VecDeque<ServerJsonRpcMessage>,
        sent: Vec<Value>,
        tools: Value,
    }

    impl Transport<RoleClient> for Fake {
        type Error = std::io::Error;

        fn send(
            &mut self,
            item: ClientJsonRpcMessage,
        ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
            let value = serde_json::to_value(&item).unwrap();
            let result = match value["method"].as_str() {
                Some("initialize") => Some(json!({"capabilities": {"tools": {}}})),
                Some("tools/list") => Some(json!({"tools": self.tools})),
                _ => None,
            };
            if let Some(result) = result {
                let reply = json!({"jsonrpc": "2.0", "id": value["id"], "result": result});
                self.replies
                    .push_back(serde_json::from_value(reply).unwrap());
            }
            self.sent.push(value);
            std::future::ready(Ok(()))
        }

        fn receive(
            &mut self,
        ) -> impl std::future::Future<Output = Option<ServerJsonRpcMessage>> + Send {
            std::future::ready(self.replies.pop_front())
        }

        fn close(&mut self) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
            std::future::ready(Ok(()))
        }
    }

    fn engine() -> RunFileContents {
        RunFileContents {
            pid: 7,
            port: 1,
            agent_tokens: Vec::new(),
            project: None,
            mode: crate::runfile::RunMode::Headless,
            focused_at: None,
            started: 1,
            version: "0".into(),
            shm: None,
        }
    }

    #[tokio::test]
    async fn an_attach_replays_the_handshake_and_notices_a_different_catalog() {
        let mut b = board();
        let init = client(
            r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
        );
        b.observe(&init);
        b.observe(&client(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        ));
        b.observe(&client(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#));
        b.patch(server(
            r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"x"}]}}"#,
        ));

        let mut same = Fake {
            replies: std::collections::VecDeque::new(),
            sent: Vec::new(),
            tools: json!([{"name": "x"}]),
        };
        let switched = b.connect(&mut same, &engine()).await.unwrap();
        assert!(!switched.catalog_changed);
        let methods: Vec<&str> = same
            .sent
            .iter()
            .map(|m| m["method"].as_str().unwrap())
            .collect();
        assert_eq!(
            methods,
            ["initialize", "notifications/initialized", "tools/list"]
        );

        let mut other = Fake {
            replies: std::collections::VecDeque::new(),
            sent: Vec::new(),
            tools: json!([{"name": "x"}, {"name": "y"}]),
        };
        let switched = b.connect(&mut other, &engine()).await.unwrap();
        assert!(switched.catalog_changed);
        assert_eq!(switched.result["catalog_changed"], true);
        // What the client now sees is the new list, so the next switch compares against it.
        let mut again = Fake {
            replies: std::collections::VecDeque::new(),
            sent: Vec::new(),
            tools: json!([{"name": "x"}, {"name": "y"}]),
        };
        assert!(
            !b.connect(&mut again, &engine())
                .await
                .unwrap()
                .catalog_changed
        );
    }

    #[test]
    fn the_proxy_tools_stay_lean() {
        let size = serde_json::to_string(&proxy_tools()).unwrap().len();
        assert!(size < 1200, "{size} bytes");
    }
}
