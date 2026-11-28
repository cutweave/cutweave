// SPDX-License-Identifier: GPL-3.0-or-later
//! What the model works with: a list of tools and a way to call one ([`ToolSurface`]), and
//! the engine a session runs against ([`EvalEngine`]).
//!
//! The one surface today is [`McpSurface`]: an MCP session on a headless engine, on `/mcp`
//! or `/mcp/described`. WP-2I.10 adds the command line with a skill as a second
//! implementation of [`ToolSurface`] (a shell tool and the skill's text as instructions);
//! the runner, the checkers and the measures do not change for it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceError, ServiceExt as _};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use crate::BenchError;
use crate::fixture::{Fixture, FixtureSpec, generate};
use crate::harness::{ChildEngine, ChildSpec};

/// How long one MCP answer may take.
const ANSWER_WAIT: Duration = Duration::from_secs(60);
/// The key a server sets in a tool's `_meta` to ask a client that defers definitions to
/// load this one at the start (`cutweave_api::mcp::core::META_ALWAYS_LOAD`, WP-2I.07).
const META_ALWAYS_LOAD: &str = "anthropic/alwaysLoad";
/// The file the import task brings in; a stub engine probes it as a 10 s video with sound.
pub const IMPORT_FILE: &str = "beach.mp4";
/// What a recorded session holds in place of this machine's import folder.
pub const IMPORTS_PLACEHOLDER: &str = "{{imports}}";

/// One tool as the model is told about it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDef {
    /// Its name.
    pub name: String,
    /// Its description.
    pub description: String,
    /// The JSON Schema of its arguments.
    pub input_schema: Value,
    /// The server asked for it to be loaded at the start.
    pub always_load: bool,
}

/// The result of one tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// What the model reads: the result's text blocks, one per line.
    pub text: String,
    /// The call failed.
    pub is_error: bool,
    /// The error's `code` (`invalid` for arguments that did not validate).
    pub error_code: Option<String>,
}

impl Outcome {
    /// A failed call that never reached the engine.
    #[must_use]
    pub fn refused(code: &str, text: String) -> Outcome {
        Outcome {
            text,
            is_error: true,
            error_code: Some(code.to_owned()),
        }
    }
}

/// A set of tools a model can be given, and the means to run a call.
pub trait ToolSurface {
    /// The tools, in the order the surface lists them.
    fn tools(&self) -> &[ToolDef];
    /// What the surface tells a model about itself (an MCP server's `instructions`).
    fn instructions(&self) -> &str;
    /// Runs one call. An error means the surface is gone, not that the call failed.
    fn call(&mut self, name: &str, input: &Value) -> Result<Outcome, BenchError>;
}

fn other(what: &str, e: impl std::fmt::Display) -> BenchError {
    BenchError::Other(format!("{what}: {e}"))
}

/// The tools of a `tools/list` result, as the model is told about them.
#[must_use]
pub fn tool_defs(listed: &[Value]) -> Vec<ToolDef> {
    listed
        .iter()
        .map(|tool| ToolDef {
            name: tool["name"].as_str().unwrap_or_default().to_owned(),
            description: tool["description"].as_str().unwrap_or_default().to_owned(),
            input_schema: tool["inputSchema"].clone(),
            always_load: tool["_meta"][META_ALWAYS_LOAD] == true,
        })
        .collect()
}

/// What the model reads of a `tools/call` result: its text blocks. An image (`render_frame`)
/// is named, not sent: no task of the evaluation needs to look at a frame.
#[must_use]
pub fn outcome(result: &Value) -> Outcome {
    let blocks = result["content"].as_array();
    let mut lines: Vec<String> = blocks
        .into_iter()
        .flatten()
        .map(|block| match block["text"].as_str() {
            Some(text) => text.to_owned(),
            None => format!(
                "[{} content left out by the evaluation runner]",
                block["type"].as_str().unwrap_or("non-text")
            ),
        })
        .collect();
    if lines.is_empty() && !result["structuredContent"].is_null() {
        lines.push(result["structuredContent"].to_string());
    }
    let is_error = result["isError"] == true;
    let code = result["structuredContent"]["error"]["code"].as_str();
    Outcome {
        text: lines.join("\n"),
        is_error,
        error_code: code.filter(|_| is_error).map(str::to_owned),
    }
}

/// An MCP session on a headless engine.
pub struct McpSurface {
    rt: Arc<Runtime>,
    service: Option<RunningService<RoleClient, ()>>,
    tools: Vec<ToolDef>,
    instructions: String,
}

impl McpSurface {
    /// Connects to `endpoint` (`/mcp`, `/mcp/described`) of `engine`, and lists its tools.
    pub fn connect(
        rt: &Arc<Runtime>,
        engine: &ChildEngine,
        endpoint: &str,
    ) -> Result<McpSurface, BenchError> {
        let url = format!("http://127.0.0.1:{}{endpoint}", engine.port);
        let config =
            StreamableHttpClientTransportConfig::with_uri(url).auth_header(engine.mcp_token());
        let service = rt
            .block_on(async {
                // The transport starts its worker on the runtime it is made in.
                let transport = StreamableHttpClientTransport::from_config(config);
                tokio::time::timeout(ANSWER_WAIT, ().serve(transport)).await
            })
            .map_err(|_| BenchError::Other("the engine did not answer the MCP handshake".into()))?
            .map_err(|e| other("connect to the MCP endpoint", e))?;
        let instructions = service
            .peer_info()
            .and_then(|info| info.instructions.clone())
            .unwrap_or_default();
        let listed = rt
            .block_on(async {
                tokio::time::timeout(ANSWER_WAIT, service.peer().list_all_tools()).await
            })
            .map_err(|_| BenchError::Other("no answer to tools/list in time".into()))?
            .map_err(|e| other("tools/list", e))?;
        let listed: Vec<Value> = listed
            .iter()
            .map(|tool| serde_json::to_value(tool).map_err(|e| other("a listed tool", e)))
            .collect::<Result<_, _>>()?;
        Ok(McpSurface {
            rt: Arc::clone(rt),
            service: Some(service),
            tools: tool_defs(&listed),
            instructions,
        })
    }
}

impl ToolSurface for McpSurface {
    fn tools(&self) -> &[ToolDef] {
        &self.tools
    }

    fn instructions(&self) -> &str {
        &self.instructions
    }

    fn call(&mut self, name: &str, input: &Value) -> Result<Outcome, BenchError> {
        let service = self
            .service
            .as_ref()
            .ok_or_else(|| BenchError::Other("the MCP session is closed".into()))?;
        let mut params = CallToolRequestParams::new(name.to_owned());
        match input {
            Value::Object(map) => params = params.with_arguments(map.clone()),
            Value::Null => {}
            // The API promises an object; the server would refuse anything else the same way.
            _ => {
                return Ok(Outcome::refused(
                    "invalid",
                    format!("{name} failed (invalid): the arguments must be a JSON object"),
                ));
            }
        }
        let peer = service.peer().clone();
        let called = self
            .rt
            .block_on(async { tokio::time::timeout(ANSWER_WAIT, peer.call_tool(params)).await })
            .map_err(|_| BenchError::Other(format!("no answer to {name} in time")))?;
        match called {
            Ok(result) => {
                let result = serde_json::to_value(&result).map_err(|e| other("a result", e))?;
                Ok(outcome(&result))
            }
            // A protocol error (an unknown tool) is an answer the model has to deal with.
            Err(ServiceError::McpError(e)) => Ok(Outcome::refused(
                "protocol",
                format!("Error: {}", e.message),
            )),
            Err(e) => Err(other(&format!("the call of {name}"), e)),
        }
    }
}

impl Drop for McpSurface {
    fn drop(&mut self) {
        if let Some(service) = self.service.take() {
            let _ = self.rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(5), service.cancel()).await
            });
        }
    }
}

/// The project every task starts from: four minutes, 24 clips on four tracks (six back to
/// back on the main track V1, six each on V2, A1 and A2), a few keyframes. Small enough to
/// read in one `timeline_get`, large enough that a clip has to be found by looking.
#[must_use]
pub fn eval_spec() -> FixtureSpec {
    FixtureSpec {
        duration_secs: 240,
        main_items: Some(6),
        ..FixtureSpec::new("P-eval", 2109, 24, 4, 16)
    }
}

/// The document a session ended with, as the checkers read it.
#[derive(Clone, Debug, PartialEq)]
pub struct FinalState {
    /// The project document (`project.snapshot`).
    pub document: Value,
    /// The names of the checkpoints (`edit.list_checkpoints`).
    pub checkpoints: Vec<String>,
}

/// A headless engine process on a fresh copy of the evaluation project. No decoders, GPU or
/// audio device (`--dev-stub-media`), and seeded ids, so a recorded session can be replayed.
pub struct EvalEngine {
    /// The engine process; killed and reaped on drop.
    pub engine: ChildEngine,
    /// The project it opened.
    pub fixture: Fixture,
    /// The folder with the file to import.
    pub imports: PathBuf,
}

impl EvalEngine {
    /// Writes the project into `scratch` (emptied first) and starts `exe engine` on it.
    pub fn start(exe: &Path, scratch: &Path) -> Result<EvalEngine, BenchError> {
        if scratch.exists() {
            std::fs::remove_dir_all(scratch).map_err(|e| BenchError::io("empty", scratch, e))?;
        }
        let imports = scratch.join("imports");
        std::fs::create_dir_all(&imports).map_err(|e| BenchError::io("create", &imports, e))?;
        let file = imports.join(IMPORT_FILE);
        std::fs::write(&file, b"stub media").map_err(|e| BenchError::io("write", &file, e))?;
        let fixture = generate(&eval_spec())?;
        let project = fixture.write(scratch)?;
        let engine = ChildEngine::spawn(&ChildSpec {
            exe: exe.to_path_buf(),
            scratch: scratch.join("run"),
            project: Some(project),
            stub_media: true,
            gui: false,
            deterministic_ids: Some(1),
            env: Vec::new(),
            roots: vec![imports.clone()],
        })?;
        Ok(EvalEngine {
            engine,
            fixture,
            imports,
        })
    }

    /// The import folder as a task names it and a call passes it: with `/`, on every system.
    #[must_use]
    pub fn imports_text(&self) -> String {
        self.imports.display().to_string().replace('\\', "/")
    }

    /// Reads the document and the checkpoints over `/rpc`.
    pub fn final_state(&self) -> Result<FinalState, BenchError> {
        let mut ws = self.engine.ws()?;
        let snapshot = ws.call("project.snapshot", &json!({}))?;
        let list = ws.call("edit.list_checkpoints", &json!({}))?;
        let checkpoints = list["checkpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["name"].as_str().map(str::to_owned))
            .collect();
        let document = match snapshot.get("project") {
            Some(project) => project.clone(),
            None => snapshot,
        };
        Ok(FinalState {
            document,
            checkpoints,
        })
    }
}

/// `value` with every `from` inside a string replaced by `to` (the import folder of this
/// machine against [`IMPORTS_PLACEHOLDER`] in a recorded session).
#[must_use]
pub fn replace_text(value: &Value, from: &str, to: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace(from, to)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| replace_text(v, from, to)).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, v)| (key.clone(), replace_text(v, from, to)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_tool_becomes_a_definition_with_its_always_load_mark() {
        let listed = [
            json!({"name": "project_info", "description": "Start here.",
                   "inputSchema": {"type": "object"},
                   "_meta": {"anthropic/alwaysLoad": true}}),
            json!({"name": "clip_split", "description": "Split a clip.",
                   "inputSchema": {"type": "object", "required": ["at"]},
                   "annotations": {"readOnlyHint": false}}),
        ];
        let tools = tool_defs(&listed);
        assert_eq!(tools[0].name, "project_info");
        assert!(tools[0].always_load);
        assert!(!tools[1].always_load);
        assert_eq!(tools[1].description, "Split a clip.");
        assert_eq!(tools[1].input_schema["required"], json!(["at"]));
    }

    #[test]
    fn the_model_reads_the_text_blocks_of_a_result() {
        let done = json!({
            "content": [{"type": "text", "text": "clip.split: done; document at rev 1"},
                        {"type": "text", "text": "{\"left\":\"itm_a\"}"}],
            "structuredContent": {"left": "itm_a"}, "isError": false
        });
        assert_eq!(
            outcome(&done),
            Outcome {
                text: "clip.split: done; document at rev 1\n{\"left\":\"itm_a\"}".into(),
                is_error: false,
                error_code: None,
            }
        );
        let failed = json!({
            "content": [{"type": "text", "text": "clip_split failed (invalid): ..."}],
            "structuredContent": {"error": {"code": "invalid", "path": "/item"}},
            "isError": true
        });
        let failed = outcome(&failed);
        assert!(failed.is_error);
        assert_eq!(failed.error_code.as_deref(), Some("invalid"));
        let frame =
            json!({"content": [{"type": "image", "data": "...", "mimeType": "image/jpeg"}]});
        assert_eq!(
            outcome(&frame).text,
            "[image content left out by the evaluation runner]"
        );
        let bare = json!({"content": [], "structuredContent": {"rev": 2}});
        assert_eq!(outcome(&bare).text, "{\"rev\":2}");
    }

    #[test]
    fn the_import_folder_is_swapped_inside_strings_at_any_depth() {
        let call =
            json!({"paths": ["C:/work/imports/beach.mp4"], "n": 1, "o": {"p": "C:/work/imports"}});
        let stored = replace_text(&call, "C:/work/imports", IMPORTS_PLACEHOLDER);
        assert_eq!(
            stored,
            json!({"paths": ["{{imports}}/beach.mp4"], "n": 1, "o": {"p": "{{imports}}"}})
        );
        assert_eq!(
            replace_text(&stored, IMPORTS_PLACEHOLDER, "C:/work/imports"),
            call
        );
    }
}
