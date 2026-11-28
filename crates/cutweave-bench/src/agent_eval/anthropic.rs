// SPDX-License-Identifier: GPL-3.0-or-later
//! The Anthropic Messages API as the evaluation uses it: building a request with tools,
//! reading a response, and the one HTTPS POST.
//!
//! Written against the documentation as of 2026-11-28 (API version `2026-10-21`); the tests
//! below hold payloads copied from these pages:
//!
//! - tool use, `tool_use` and `tool_result` blocks, `is_error`:
//!   <https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls>
//! - tool search and `defer_loading` on client-defined tools:
//!   <https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool>
//! - prompt caching (the request's top-level `cache_control`) and the `usage` fields:
//!   <https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
//!
//! Two client modes ([`ClientMode`]): `upfront` sends every definition into the model's
//! context, as a client that loads the whole `tools/list` does; `deferred` marks every tool
//! `defer_loading: true` except the ones the server marked always-load, and adds the API's
//! tool search tool, as a client that defers definitions does (ADR-0040). In both modes the
//! request carries every definition: "You still send every tool's full definition in the
//! `tools` array on every request, including the deferred ones" (the tool search page).
//!
//! The request sets no `thinking`, no sampling parameter and no forced `tool_choice`: the
//! current models reject some of those, and the defaults are what an ordinary client gets.
//!
//! The POST itself is not made here. The workspace has no HTTP client with TLS (reqwest is
//! built without it, for loopback only), and the evaluation must not add one, so
//! [`PythonTransport`] hands the body to `scripts/agent-eval-post.py` (Python's standard
//! library). That script reads the key from `ANTHROPIC_API_KEY`; this process only asks
//! whether the variable is set and never reads its value.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::surface::ToolDef;
use crate::BenchError;

/// Where requests go.
pub const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
/// The `anthropic-version` header (`scripts/agent-eval-post.py` sends the same).
pub const API_VERSION: &str = "2026-10-21";
/// The model when `--model` names none.
pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";
/// The only place the key is read from (by the Python script, not by this process).
pub const KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// `max_tokens` of a request: room for a turn's thinking and tool calls, and small enough
/// for a request that is not streamed.
pub const DEFAULT_MAX_TOKENS: u32 = 16_000;
/// How often one request is sent before the session gives up (429, 5xx, no answer).
const ATTEMPTS: u32 = 4;
/// The longest pause before the next attempt.
const MAX_PAUSE: Duration = Duration::from_secs(60);

/// How the client hands the tool definitions to the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientMode {
    /// Every definition is in the model's context from the first request.
    Upfront,
    /// Definitions are deferred behind the API's tool search, except the always-load ones.
    Deferred,
}

impl ClientMode {
    /// `upfront` or `deferred`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ClientMode::Upfront => "upfront",
            ClientMode::Deferred => "deferred",
        }
    }

    /// The mode called `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<ClientMode> {
        match name {
            "upfront" => Some(ClientMode::Upfront),
            "deferred" => Some(ClientMode::Deferred),
            _ => None,
        }
    }
}

/// Which of the API's two tool search tools the deferred mode uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolSearch {
    /// `tool_search_tool_bm25_20251119`: the model searches with plain words.
    Bm25,
    /// `tool_search_tool_regex_20251119`: the model searches with a regular expression.
    Regex,
}

impl ToolSearch {
    /// The search tool's entry in `tools`.
    #[must_use]
    pub fn entry(self) -> Value {
        match self {
            ToolSearch::Bm25 => {
                json!({"type": "tool_search_tool_bm25_20251119", "name": "tool_search_tool_bm25"})
            }
            ToolSearch::Regex => {
                json!({"type": "tool_search_tool_regex_20251119", "name": "tool_search_tool_regex"})
            }
        }
    }

    /// `bm25` or `regex`.
    #[must_use]
    pub fn parse(name: &str) -> Option<ToolSearch> {
        match name {
            "bm25" => Some(ToolSearch::Bm25),
            "regex" => Some(ToolSearch::Regex),
            _ => None,
        }
    }
}

/// What every request of a session shares.
#[derive(Clone, Debug)]
pub struct RequestShape {
    /// The model id, sent as given.
    pub model: String,
    /// `max_tokens`.
    pub max_tokens: u32,
    /// `output_config.effort`; `None` leaves the model's default.
    pub effort: Option<String>,
    /// The client mode.
    pub mode: ClientMode,
    /// The search tool of the deferred mode.
    pub search: ToolSearch,
}

/// The `tools` array: one entry per tool, in the order given. In the deferred mode the tool
/// search tool comes first and every tool that is not always-load is deferred.
#[must_use]
pub fn tool_entries(tools: &[ToolDef], mode: ClientMode, search: ToolSearch) -> Vec<Value> {
    let mut entries = Vec::with_capacity(tools.len() + 1);
    if mode == ClientMode::Deferred {
        entries.push(search.entry());
    }
    for tool in tools {
        let mut entry = json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": tool.input_schema,
        });
        if mode == ClientMode::Deferred && !tool.always_load {
            entry["defer_loading"] = json!(true);
        }
        entries.push(entry);
    }
    entries
}

/// The body of one `POST /v1/messages`. The top-level `cache_control` asks for automatic
/// prompt caching: every request of a session repeats the tools, the system prompt and the
/// conversation so far.
#[must_use]
pub fn build_request(
    shape: &RequestShape,
    system: &str,
    tools: &[ToolDef],
    messages: &[Value],
) -> Value {
    let mut body = json!({
        "model": shape.model,
        "max_tokens": shape.max_tokens,
        "cache_control": {"type": "ephemeral"},
        "system": [{"type": "text", "text": system}],
        "tools": tool_entries(tools, shape.mode, shape.search),
        "messages": messages,
    });
    if let Some(effort) = &shape.effort {
        body["output_config"] = json!({"effort": effort});
    }
    body
}

/// The user message that answers a turn's tool calls: one `tool_result` per `tool_use`, all
/// in one message and nothing before them.
#[must_use]
pub fn tool_results_message(results: &[(String, String, bool)]) -> Value {
    let blocks: Vec<Value> = results
        .iter()
        .map(|(id, text, is_error)| {
            let mut block = json!({"type": "tool_result", "tool_use_id": id, "content": text});
            if *is_error {
                block["is_error"] = json!(true);
            }
            block
        })
        .collect();
    json!({"role": "user", "content": blocks})
}

/// The token counts of one response. The three input counts add up to the prompt's size
/// (`total_input_tokens = cache_read_input_tokens + cache_creation_input_tokens +
/// input_tokens`, the prompt caching page).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens that were neither read from nor written to the cache.
    pub input_tokens: u64,
    /// Output tokens (thinking included).
    pub output_tokens: u64,
    /// Input tokens written to the cache.
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    /// Input tokens read from the cache.
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

impl Usage {
    /// The size of the prompt of this request.
    #[must_use]
    pub fn total_input(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }
}

/// A `tool_use` block: a call the client has to run.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolUse {
    /// The block id the `tool_result` answers.
    pub id: String,
    /// The tool.
    pub name: String,
    /// Its arguments.
    pub input: Value,
}

/// One search the model ran with the API's tool search tool.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchUse {
    /// What it searched for (`query` of the BM25 tool, `pattern` of the regex one).
    pub query: String,
    /// The tools the search loaded.
    pub found: Vec<String>,
    /// The error code of a search that failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Why a response ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The model is done.
    EndTurn,
    /// The model called client tools: run them and send the results.
    ToolUse,
    /// A server tool is still running: send the conversation again as it is.
    PauseTurn,
    /// `max_tokens` cut the response off; its tool calls must not be run.
    MaxTokens,
    /// A safety classifier declined; its tool calls must not be run.
    Refusal,
    /// Anything else, by name.
    Other(String),
}

impl Stop {
    fn parse(name: &str) -> Stop {
        match name {
            "end_turn" => Stop::EndTurn,
            "tool_use" => Stop::ToolUse,
            "pause_turn" => Stop::PauseTurn,
            "max_tokens" => Stop::MaxTokens,
            "refusal" => Stop::Refusal,
            other => Stop::Other(other.to_owned()),
        }
    }

    /// The API's name of the stop reason.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Stop::EndTurn => "end_turn",
            Stop::ToolUse => "tool_use",
            Stop::PauseTurn => "pause_turn",
            Stop::MaxTokens => "max_tokens",
            Stop::Refusal => "refusal",
            Stop::Other(name) => name,
        }
    }
}

/// One response, as far as the evaluation reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// The `content` array, to be sent back unchanged as the assistant's message (thinking,
    /// `server_tool_use` and `tool_search_tool_result` blocks included).
    pub content: Value,
    /// Why it ended.
    pub stop: Stop,
    /// Its token counts.
    pub usage: Usage,
    /// The client tool calls, in order.
    pub tool_uses: Vec<ToolUse>,
    /// The tool searches the API ran in this response.
    pub searches: Vec<SearchUse>,
}

fn text_of(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

/// The tool searches of a response: each `server_tool_use` of a search tool, with the
/// `tool_search_tool_result` that answers it.
fn searches(content: &[Value]) -> Vec<SearchUse> {
    let mut found: Vec<(String, SearchUse)> = Vec::new();
    for block in content {
        match block["type"].as_str() {
            Some("server_tool_use") if text_of(&block["name"]).starts_with("tool_search") => {
                let input = &block["input"];
                let query = input["query"]
                    .as_str()
                    .or_else(|| input["pattern"].as_str());
                let search = SearchUse {
                    query: query.unwrap_or_default().to_owned(),
                    ..SearchUse::default()
                };
                found.push((text_of(&block["id"]), search));
            }
            Some("tool_search_tool_result") => {
                let id = text_of(&block["tool_use_id"]);
                let Some((_, search)) = found.iter_mut().find(|(known, _)| *known == id) else {
                    continue;
                };
                let result = &block["content"];
                if result["type"] == "tool_search_tool_result_error" {
                    search.error = Some(text_of(&result["error_code"]));
                }
                let references = result["tool_references"].as_array();
                search.found = references
                    .into_iter()
                    .flatten()
                    .map(|reference| text_of(&reference["tool_name"]))
                    .collect();
            }
            _ => {}
        }
    }
    found.into_iter().map(|(_, search)| search).collect()
}

/// Reads the body of a 200 response.
pub fn parse_reply(body: &Value) -> Result<Reply, BenchError> {
    let bad = |what: &str| BenchError::Other(format!("the API's response has no {what}"));
    let content = body["content"].as_array().ok_or_else(|| bad("content"))?;
    let stop = body["stop_reason"]
        .as_str()
        .ok_or_else(|| bad("stop_reason"))?;
    // A response without usage (none of the documented ones) counts as zero tokens.
    let usage = serde_json::from_value(body["usage"].clone()).unwrap_or_default();
    let tool_uses = content
        .iter()
        .filter(|block| block["type"] == "tool_use")
        .map(|block| ToolUse {
            id: text_of(&block["id"]),
            name: text_of(&block["name"]),
            input: block["input"].clone(),
        })
        .collect();
    Ok(Reply {
        content: Value::Array(content.clone()),
        stop: Stop::parse(stop),
        usage,
        tool_uses,
        searches: searches(content),
    })
}

/// What came back from one POST.
#[derive(Clone, Debug, PartialEq)]
pub struct HttpAnswer {
    /// The HTTP status; 0 when no answer arrived.
    pub status: u16,
    /// The `retry-after` header in seconds, when the API sent one.
    pub retry_after: Option<f64>,
    /// The response body (JSON when it parsed, else the text).
    pub body: Value,
}

/// Something that can carry a request to the API. The evaluation's own is
/// [`PythonTransport`]; the tests use a scripted one.
pub trait Transport {
    /// Sends `body` and returns the answer; an error means the request could not be made
    /// at all (no Python, the script is missing).
    fn post(&mut self, body: &Value) -> Result<HttpAnswer, BenchError>;
}

/// A request the API refused for good (bad request, no access, unknown model).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiFailure {
    /// The HTTP status.
    pub status: u16,
    /// The API's error type and message.
    pub message: String,
}

impl ApiFailure {
    /// The key is missing, wrong or may not use the model: no later request can succeed.
    #[must_use]
    pub fn ends_the_run(&self) -> bool {
        matches!(self.status, 401 | 403)
    }
}

fn failure(answer: &HttpAnswer) -> ApiFailure {
    let error = &answer.body["error"];
    let message = match (error["type"].as_str(), error["message"].as_str()) {
        (Some(kind), Some(message)) => format!("{kind}: {message}"),
        _ if answer.status == 0 => text_of(&answer.body["error"]),
        _ => answer.body.to_string().chars().take(300).collect(),
    };
    ApiFailure {
        status: answer.status,
        message,
    }
}

/// Sends `body`, trying again after a rate limit (429), a server error (5xx) or no answer,
/// at most [`ATTEMPTS`] times and never waiting longer than [`MAX_PAUSE`] between tries.
/// `pause` does the waiting (the tests pass one that does not sleep).
pub fn send(
    transport: &mut dyn Transport,
    body: &Value,
    pause: &mut dyn FnMut(Duration),
) -> Result<Result<Reply, ApiFailure>, BenchError> {
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        let answer = transport.post(body)?;
        if answer.status == 200 {
            return parse_reply(&answer.body).map(Ok);
        }
        let again = answer.status == 0 || answer.status == 429 || answer.status >= 500;
        if !again {
            return Ok(Err(failure(&answer)));
        }
        if attempt + 1 < ATTEMPTS {
            let backoff = Duration::from_secs(2u64.pow(attempt + 1));
            let asked = answer
                .retry_after
                .filter(|secs| secs.is_finite() && *secs >= 0.0)
                .map(Duration::from_secs_f64);
            pause(asked.unwrap_or(backoff).min(MAX_PAUSE));
        }
        last = Some(failure(&answer));
    }
    Ok(Err(last.unwrap_or(ApiFailure {
        status: 0,
        message: "no answer".into(),
    })))
}

/// The transport of a real run: `python scripts/agent-eval-post.py`, one process per request.
#[derive(Clone, Debug)]
pub struct PythonTransport {
    python: String,
    script: PathBuf,
    url: String,
    timeout: Duration,
    env: Vec<(String, String)>,
}

impl PythonTransport {
    /// Sets a variable in the environment of the script's process. For the test, which gives
    /// the script a made-up key this way; a real run passes nothing and the script inherits
    /// the environment this process was started in.
    #[must_use]
    pub fn env(mut self, name: &str, value: &str) -> PythonTransport {
        self.env.push((name.to_owned(), value.to_owned()));
        self
    }

    /// The transport of the checkout at `root`, posting to `url`. Fails when the script is
    /// missing or no Python answers `--version`.
    pub fn new(root: &Path, url: &str, timeout: Duration) -> Result<PythonTransport, BenchError> {
        let script = root.join("scripts").join("agent-eval-post.py");
        if !script.is_file() {
            return Err(BenchError::Other(format!(
                "{} is missing",
                script.display()
            )));
        }
        let python = ["python", "python3", "py"]
            .into_iter()
            .find(|name| {
                Command::new(name)
                    .arg("--version")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success())
            })
            .ok_or_else(|| BenchError::Other("no Python 3 on PATH (python, python3, py)".into()))?;
        Ok(PythonTransport {
            python: python.to_owned(),
            script,
            url: url.to_owned(),
            timeout,
            env: Vec::new(),
        })
    }
}

impl Transport for PythonTransport {
    fn post(&mut self, body: &Value) -> Result<HttpAnswer, BenchError> {
        let failed = |what: &str, e: &dyn std::fmt::Display| {
            BenchError::Other(format!("agent-eval-post.py: {what}: {e}"))
        };
        // The script's own timeout bounds the wait: urllib gives up after `timeout`.
        let mut child = Command::new(&self.python)
            .arg(&self.script)
            .arg("--url")
            .arg(&self.url)
            .arg("--timeout")
            .arg(self.timeout.as_secs().to_string())
            .envs(self.env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| failed("cannot start", &e))?;
        let payload = serde_json::to_vec(body).map_err(|e| failed("cannot encode", &e))?;
        if let Some(mut stdin) = child.stdin.take() {
            // A script that stopped early (no key) closes its stdin: its exit says why.
            let _ = stdin.write_all(&payload);
        }
        let output = child
            .wait_with_output()
            .map_err(|e| failed("did not finish", &e))?;
        if !output.status.success() {
            let said = String::from_utf8_lossy(&output.stderr);
            return Err(failed("stopped", &said.trim()));
        }
        let answer: Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| failed("printed something else than JSON", &e))?;
        let status = answer["status"]
            .as_u64()
            .and_then(|s| u16::try_from(s).ok());
        Ok(HttpAnswer {
            status: status.unwrap_or(0),
            retry_after: answer["retry_after"]
                .as_str()
                .and_then(|secs| secs.trim().parse().ok()),
            body: if status == Some(0) || status.is_none() {
                json!({"error": answer["error"]})
            } else {
                answer["body"].clone()
            },
        })
    }
}

/// What `--dry-run` prints: the first request as it would be sent, the key redacted.
#[must_use]
pub fn dry_run_text(url: &str, key_is_set: bool, body: &Value) -> String {
    let key = if key_is_set {
        "<redacted: set in the environment>"
    } else {
        "<redacted: not set>"
    };
    let pretty = serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string());
    format!(
        "POST {url}\ncontent-type: application/json\nx-api-key: {key}\n\
         anthropic-version: {API_VERSION}\n\n{pretty}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, always_load: bool) -> ToolDef {
        ToolDef {
            name: name.to_owned(),
            description: format!("Does {name}"),
            input_schema: json!({"type": "object", "properties": {"location": {"type": "string"}}}),
            always_load,
        }
    }

    fn shape(mode: ClientMode) -> RequestShape {
        RequestShape {
            model: DEFAULT_MODEL.to_owned(),
            max_tokens: DEFAULT_MAX_TOKENS,
            effort: None,
            mode,
            search: ToolSearch::Bm25,
        }
    }

    /// The request of the "Quick start" of the tool search page has this shape: the search
    /// tool, then client tools with `name`, `description`, `input_schema` and
    /// `"defer_loading": true`.
    #[test]
    fn a_deferred_request_defers_all_but_the_always_load_tools() {
        let tools = [tool("project_info", true), tool("clip_split", false)];
        let user = json!({"role": "user", "content": "Split the clip."});
        let body = build_request(
            &shape(ClientMode::Deferred),
            "sys",
            &tools,
            std::slice::from_ref(&user),
        );
        assert_eq!(
            body["tools"],
            json!([
                {"type": "tool_search_tool_bm25_20251119", "name": "tool_search_tool_bm25"},
                {"name": "project_info", "description": "Does project_info",
                 "input_schema": tools[0].input_schema},
                {"name": "clip_split", "description": "Does clip_split",
                 "input_schema": tools[1].input_schema, "defer_loading": true},
            ])
        );
        assert_eq!(body["model"], "claude-sonnet-5-5");
        assert_eq!(body["max_tokens"], 16_000);
        assert_eq!(body["messages"], json!([user]));
        assert_eq!(body["system"], json!([{"type": "text", "text": "sys"}]));
        // The automatic caching request of the prompt caching page.
        assert_eq!(body["cache_control"], json!({"type": "ephemeral"}));
        // Nothing the current models reject.
        for absent in [
            "temperature",
            "top_p",
            "top_k",
            "thinking",
            "tool_choice",
            "output_config",
        ] {
            assert!(body.get(absent).is_none(), "{absent}");
        }
        let regex = ToolSearch::Regex.entry();
        assert_eq!(regex["type"], "tool_search_tool_regex_20251119");
        assert_eq!(regex["name"], "tool_search_tool_regex");
    }

    #[test]
    fn an_upfront_request_sends_every_definition_into_context() {
        let tools = [tool("project_info", true), tool("clip_split", false)];
        let mut shape = shape(ClientMode::Upfront);
        shape.effort = Some("low".into());
        shape.model = "claude-fable-5-2".into();
        let body = build_request(&shape, "sys", &tools, &[]);
        let entries = body["tools"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.get("defer_loading").is_none()));
        assert!(entries.iter().all(|e| e.get("type").is_none()));
        assert_eq!(body["output_config"], json!({"effort": "low"}));
        assert_eq!(body["model"], "claude-fable-5-2");
    }

    /// "Example API response with a `tool_use` content block" of the handle-tool-calls page,
    /// with the `usage` object of the prompt caching page.
    #[test]
    fn a_tool_use_response_is_read() {
        let body = json!({
            "id": "msg_01Aq9w938a90dw8q",
            "model": "claude-opus-5-5",
            "stop_reason": "tool_use",
            "role": "assistant",
            "content": [
                {"type": "text",
                 "text": "I'll check the current weather in San Francisco for you."},
                {"type": "tool_use", "id": "toolu_01A09q90qw90lq917835lq9",
                 "name": "get_weather",
                 "input": {"location": "San Francisco, CA", "unit": "celsius"}}
            ],
            "usage": {
                "input_tokens": 2048,
                "cache_read_input_tokens": 1800,
                "cache_creation_input_tokens": 248,
                "output_tokens": 503,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": 148,
                    "ephemeral_1h_input_tokens": 100
                }
            }
        });
        let reply = parse_reply(&body).unwrap();
        assert_eq!(reply.stop, Stop::ToolUse);
        assert_eq!(
            reply.tool_uses,
            [ToolUse {
                id: "toolu_01A09q90qw90lq917835lq9".into(),
                name: "get_weather".into(),
                input: json!({"location": "San Francisco, CA", "unit": "celsius"}),
            }]
        );
        assert_eq!(reply.content, body["content"]);
        assert_eq!(reply.usage.total_input(), 2048 + 1800 + 248);
        assert_eq!(reply.usage.output_tokens, 503);
        assert_eq!(reply.searches, []);
    }

    /// "Response format" of the tool search page: a search, its result, then the call of the
    /// tool the search found. Only the `tool_use` is the client's to run.
    #[test]
    fn a_tool_search_response_is_read() {
        let body = json!({
            "role": "assistant",
            "content": [
                {"type": "text",
                 "text": "I'll search for tools to help with the weather information."},
                {"type": "server_tool_use", "id": "srvtoolu_01ABC123",
                 "name": "tool_search_tool_regex",
                 "input": {"pattern": "weather", "limit": 10}},
                {"type": "tool_search_tool_result", "tool_use_id": "srvtoolu_01ABC123",
                 "content": {
                     "type": "tool_search_tool_search_result",
                     "tool_references": [{"type": "tool_reference", "tool_name": "get_weather"}]
                 }},
                {"type": "text",
                 "text": "I found a weather tool. Let me get the weather for San Francisco."},
                {"type": "tool_use", "id": "toolu_01XYZ789", "name": "get_weather",
                 "input": {"location": "San Francisco", "unit": "fahrenheit"}}
            ],
            "stop_reason": "tool_use"
        });
        let reply = parse_reply(&body).unwrap();
        assert_eq!(reply.tool_uses.len(), 1);
        assert_eq!(reply.tool_uses[0].id, "toolu_01XYZ789");
        assert_eq!(
            reply.searches,
            [SearchUse {
                query: "weather".into(),
                found: vec!["get_weather".into()],
                error: None,
            }]
        );
        // The search blocks go back with the rest of the content, unchanged.
        assert_eq!(reply.content.as_array().unwrap().len(), 5);
        assert_eq!(reply.usage, Usage::default());
    }

    /// "Tool result errors (200 status)" of the tool search page.
    #[test]
    fn a_failed_tool_search_is_recorded_with_its_code() {
        let body = json!({
            "content": [
                {"type": "server_tool_use", "id": "srvtoolu_01ABC123",
                 "name": "tool_search_tool_bm25", "input": {"query": "fade the music"}},
                {"type": "tool_search_tool_result", "tool_use_id": "srvtoolu_01ABC123",
                 "content": {
                     "type": "tool_search_tool_result_error",
                     "error_code": "invalid_tool_input",
                     "error_message": "Invalid regular expression pattern: missing ) at position 1"
                 }}
            ],
            "stop_reason": "end_turn"
        });
        let reply = parse_reply(&body).unwrap();
        assert_eq!(reply.stop, Stop::EndTurn);
        assert_eq!(reply.searches[0].query, "fade the music");
        assert_eq!(
            reply.searches[0].error.as_deref(),
            Some("invalid_tool_input")
        );
        assert_eq!(reply.searches[0].found, Vec::<String>::new());
    }

    /// The `tool_result` examples of the handle-tool-calls page: a result, and an error with
    /// `"is_error": true`.
    #[test]
    fn tool_results_go_back_in_one_user_message() {
        let message = tool_results_message(&[
            (
                "toolu_01A09q90qw90lq917835lq9".into(),
                "15 degrees".into(),
                false,
            ),
            (
                "toolu_02".into(),
                "Error: Missing required 'location' parameter".into(),
                true,
            ),
        ]);
        assert_eq!(
            message,
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_01A09q90qw90lq917835lq9",
                 "content": "15 degrees"},
                {"type": "tool_result", "tool_use_id": "toolu_02",
                 "content": "Error: Missing required 'location' parameter", "is_error": true}
            ]})
        );
    }

    #[test]
    fn the_stop_reasons_and_a_response_without_content() {
        for (name, stop) in [
            ("end_turn", Stop::EndTurn),
            ("pause_turn", Stop::PauseTurn),
            ("max_tokens", Stop::MaxTokens),
            ("refusal", Stop::Refusal),
            ("stop_sequence", Stop::Other("stop_sequence".into())),
        ] {
            let reply = parse_reply(&json!({"content": [], "stop_reason": name})).unwrap();
            assert_eq!(reply.stop, stop);
            assert_eq!(reply.stop.name(), name);
        }
        assert!(parse_reply(&json!({"stop_reason": "end_turn"})).is_err());
        assert!(parse_reply(&json!({"content": []})).is_err());
    }

    struct Scripted(Vec<HttpAnswer>, u32);

    impl Transport for Scripted {
        fn post(&mut self, _: &Value) -> Result<HttpAnswer, BenchError> {
            self.1 += 1;
            Ok(self.0.remove(0))
        }
    }

    fn answer(status: u16, retry_after: Option<f64>, body: Value) -> HttpAnswer {
        HttpAnswer {
            status,
            retry_after,
            body,
        }
    }

    /// The error body of the tool search page ("All tools deferred"), and the retry rules.
    #[test]
    fn a_rate_limit_is_tried_again_and_a_bad_request_is_not() {
        let ok = json!({"content": [], "stop_reason": "end_turn"});
        let overloaded = json!({"type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}});
        let mut waits = Vec::new();
        let mut transport = Scripted(
            vec![
                answer(429, Some(3.0), overloaded.clone()),
                answer(529, None, overloaded.clone()),
                answer(0, None, json!({"error": "URLError: timed out"})),
                answer(200, None, ok),
            ],
            0,
        );
        let reply = send(&mut transport, &json!({}), &mut |d| waits.push(d)).unwrap();
        assert_eq!(reply.unwrap().stop, Stop::EndTurn);
        assert_eq!(
            waits,
            [
                Duration::from_secs(3),
                Duration::from_secs(4),
                Duration::from_secs(8)
            ]
        );

        let bad = json!({"type": "error", "error": {
            "type": "invalid_request_error",
            "message": "At least one tool must have defer_loading=false. All tools cannot be deferred."
        }});
        let mut transport = Scripted(vec![answer(400, None, bad)], 0);
        let refused = send(&mut transport, &json!({}), &mut |_| {})
            .unwrap()
            .unwrap_err();
        assert_eq!(transport.1, 1, "a bad request is sent once");
        assert_eq!(refused.status, 400);
        assert!(
            refused
                .message
                .starts_with("invalid_request_error: At least one tool")
        );
        assert!(!refused.ends_the_run());

        let mut transport = Scripted(vec![answer(529, None, overloaded); 4], 0);
        let gave_up = send(&mut transport, &json!({}), &mut |_| {})
            .unwrap()
            .unwrap_err();
        assert_eq!((transport.1, gave_up.status), (4, 529));
        let denied = ApiFailure {
            status: 401,
            message: String::new(),
        };
        assert!(denied.ends_the_run());
    }

    #[test]
    fn a_dry_run_never_shows_a_key() {
        let body = build_request(&shape(ClientMode::Upfront), "sys", &[], &[]);
        let text = dry_run_text(MESSAGES_URL, true, &body);
        assert!(text.starts_with("POST https://api.anthropic.com/v1/messages\n"));
        assert!(text.contains("x-api-key: <redacted: set in the environment>"));
        assert!(text.contains("anthropic-version: 2026-10-21"));
        assert!(text.contains("\"model\": \"claude-sonnet-5-5\""));
        assert!(dry_run_text(MESSAGES_URL, false, &body).contains("<redacted: not set>"));
    }

    #[test]
    fn the_names_of_the_modes() {
        for mode in [ClientMode::Upfront, ClientMode::Deferred] {
            assert_eq!(ClientMode::parse(mode.name()), Some(mode));
        }
        assert_eq!(ClientMode::parse("both"), None);
        assert_eq!(ToolSearch::parse("regex"), Some(ToolSearch::Regex));
        assert_eq!(ToolSearch::parse("bm25"), Some(ToolSearch::Bm25));
        assert_eq!(ToolSearch::parse("embeddings"), None);
    }
}
