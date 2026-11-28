// SPDX-License-Identifier: GPL-3.0-or-later
//! A session as it is kept: every tool call the model made, turn by turn ([`Transcript`]),
//! and what is measured from it ([`Measures`]).
//!
//! The same record serves a run with a model (it goes into the result file), the replay
//! without one (`--replay`, the smoke test in the gate) and a session written by hand
//! (`--record`). It holds the calls and their arguments, not the results: a replay gets
//! those from the engine again.
//!
//! Token counts of requests and responses are the API's own (`usage`). The sizes of tool
//! results and of tool definitions are **estimates**: bytes of the JSON or text divided by
//! four, rounded up, the rule `docs/reference/mcp-core-tools.md` uses for the tool list.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::anthropic::{SearchUse, Usage};

/// The version of the transcript and result format.
pub const FORMAT: u32 = 1;

/// One tool call and how it went.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Call {
    /// The tool.
    pub tool: String,
    /// Its arguments.
    #[serde(default)]
    pub input: Value,
    /// The call failed.
    #[serde(default)]
    pub is_error: bool,
    /// The error's code (`invalid`: the arguments did not validate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Bytes of the result text the model was sent.
    #[serde(default)]
    pub result_bytes: u64,
}

/// One response of the model: its token counts and the calls it made.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// The API's token counts for the request and the response.
    #[serde(default)]
    pub usage: Usage,
    /// Why the response ended.
    #[serde(default)]
    pub stop_reason: String,
    /// Searches the model ran with the API's tool search (the deferred mode).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub searches: Vec<SearchUse>,
    /// The tool calls, in order.
    #[serde(default)]
    pub calls: Vec<Call>,
}

/// The size of the tool definitions of a session, in bytes of their JSON.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSizes {
    /// Tools the model could use.
    pub count: u64,
    /// Every definition sent with a request.
    pub sent_bytes: u64,
    /// The definitions in the model's context from the start (all of them in the upfront
    /// mode; the always-load ones and the search tool in the deferred mode).
    pub upfront_bytes: u64,
    /// The definitions a tool search loaded later (the deferred mode).
    #[serde(default)]
    pub loaded_bytes: u64,
}

/// A recorded session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transcript {
    /// [`FORMAT`].
    pub format: u32,
    /// The task's name.
    pub task: String,
    /// The variant's name.
    pub variant: String,
    /// `upfront` or `deferred`.
    pub client: String,
    /// The model id; a session written by hand says so here.
    pub model: String,
    /// The MCP endpoint.
    pub endpoint: String,
    /// The tool definitions' sizes.
    pub tools: ToolSizes,
    /// Bytes of the system prompt (the runner's two sentences and the server instructions).
    pub system_bytes: u64,
    /// The model's responses, in order.
    pub turns: Vec<Turn>,
    /// How the session ended: `end_turn`, `max_turns`, `max_tokens`, `refusal`, or
    /// `error: ...` when the API refused a request.
    pub ended: String,
}

/// What is measured for one task and variant.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Measures {
    /// The checker passed.
    pub success: bool,
    /// The checker's reason when it did not.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure: String,
    /// Responses of the model (requests to the API).
    pub turns: u64,
    /// Tool calls.
    pub tool_calls: u64,
    /// Tool calls that failed.
    pub tool_errors: u64,
    /// Distinct tools called; a command run through `cutweave_execute` or `cutweave_query`
    /// counts as a tool of its own (`cutweave_execute:clip.split`).
    pub tools_used: u64,
    /// How many of them worked at their first call.
    pub first_calls_valid: u64,
    /// The ones that did not, with the error code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub first_calls_invalid: Vec<String>,
    /// Calls of `cutweave_describe_command`.
    pub describe_calls: u64,
    /// Calls of `cutweave_search_commands`.
    pub search_calls: u64,
    /// Searches with the API's tool search (the deferred mode).
    pub tool_search_calls: u64,
    /// Calls of `cutweave_execute` with two or more steps.
    pub batches: u64,
    /// Input tokens over all requests, cached or not (the API's count).
    pub input_tokens: u64,
    /// Of those, tokens read from the prompt cache.
    pub cache_read_tokens: u64,
    /// Output tokens over all responses (the API's count).
    pub output_tokens: u64,
    /// Tokens of all tool results sent to the model (estimate: bytes / 4).
    pub tool_result_tokens: u64,
    /// Tokens of the tool definitions sent with each request (estimate: bytes / 4).
    pub tool_definition_tokens: u64,
    /// Tokens of the definitions that were in the model's context at the end: all of them
    /// in the upfront mode, the loaded ones in the deferred mode (estimate: bytes / 4).
    pub tool_definition_tokens_in_context: u64,
}

/// The estimate used for everything the API does not count: a quarter of the bytes, rounded
/// up.
#[must_use]
pub fn tokens_estimate(bytes: u64) -> u64 {
    bytes.div_ceil(4)
}

/// The names under which a call counts for "was the first call valid".
fn first_call_keys(call: &Call) -> Vec<String> {
    let inner: Vec<&str> = match call.tool.as_str() {
        "cutweave_execute" => call.input["steps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|step| step["cmd"].as_str())
            .collect(),
        "cutweave_query" => call.input["name"].as_str().into_iter().collect(),
        _ => Vec::new(),
    };
    if inner.is_empty() {
        return vec![call.tool.clone()];
    }
    inner
        .iter()
        .map(|command| format!("{}:{command}", call.tool))
        .collect()
}

/// Computes the measures of a session from its transcript and the checker's verdict.
#[must_use]
pub fn measure(transcript: &Transcript, verdict: &Result<(), String>) -> Measures {
    let mut m = Measures {
        success: verdict.is_ok(),
        failure: verdict.clone().err().unwrap_or_default(),
        turns: transcript.turns.len() as u64,
        tool_definition_tokens: tokens_estimate(transcript.tools.sent_bytes),
        tool_definition_tokens_in_context: tokens_estimate(
            transcript.tools.upfront_bytes + transcript.tools.loaded_bytes,
        ),
        ..Measures::default()
    };
    let mut seen = BTreeSet::new();
    let mut result_bytes = 0;
    for turn in &transcript.turns {
        m.input_tokens += turn.usage.total_input();
        m.cache_read_tokens += turn.usage.cache_read_input_tokens;
        m.output_tokens += turn.usage.output_tokens;
        m.tool_search_calls += turn.searches.len() as u64;
        for call in &turn.calls {
            m.tool_calls += 1;
            m.tool_errors += u64::from(call.is_error);
            result_bytes += call.result_bytes;
            match call.tool.as_str() {
                "cutweave_describe_command" => m.describe_calls += 1,
                "cutweave_search_commands" => m.search_calls += 1,
                "cutweave_execute" => {
                    let steps = call.input["steps"].as_array().map_or(0, Vec::len);
                    m.batches += u64::from(steps >= 2);
                }
                _ => {}
            }
            for key in first_call_keys(call) {
                if !seen.insert(key.clone()) {
                    continue;
                }
                m.tools_used += 1;
                if call.is_error {
                    let code = call.error_code.as_deref().unwrap_or("error");
                    m.first_calls_invalid.push(format!("{key} ({code})"));
                } else {
                    m.first_calls_valid += 1;
                }
            }
        }
    }
    m.tool_result_tokens = tokens_estimate(result_bytes);
    m
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn call(tool: &str, input: Value, error: Option<&str>, result_bytes: u64) -> Call {
        Call {
            tool: tool.to_owned(),
            input,
            is_error: error.is_some(),
            error_code: error.map(str::to_owned),
            result_bytes,
        }
    }

    fn usage(input: u64, written: u64, read: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: written,
            cache_read_input_tokens: read,
        }
    }

    #[test]
    fn the_measures_of_a_session() {
        let split = json!({"steps": [{"cmd": "clip.split", "args": {}},
                                     {"cmd": "clip.trim", "args": {}}]});
        let transcript = Transcript {
            tools: ToolSizes {
                count: 32,
                sent_bytes: 30_965,
                upfront_bytes: 5_001,
                loaded_bytes: 999,
            },
            turns: vec![
                Turn {
                    usage: usage(100, 9_000, 0, 50),
                    searches: vec![SearchUse::default()],
                    calls: vec![
                        call("project_info", json!({}), None, 401),
                        call("timeline_get", json!({"depth": 2}), Some("invalid"), 200),
                    ],
                    ..Turn::default()
                },
                Turn {
                    usage: usage(300, 500, 9_000, 70),
                    calls: vec![
                        call("timeline_get", json!({"sequence": "seq_1"}), None, 4_000),
                        call(
                            "cutweave_describe_command",
                            json!({"name": "clip.trim"}),
                            None,
                            800,
                        ),
                        call(
                            "cutweave_search_commands",
                            json!({"query": "trim"}),
                            None,
                            600,
                        ),
                    ],
                    ..Turn::default()
                },
                Turn {
                    usage: usage(10, 0, 9_800, 30),
                    calls: vec![
                        call("cutweave_execute", split.clone(), Some("conflict"), 100),
                        call("cutweave_execute", split, None, 900),
                        call(
                            "cutweave_query",
                            json!({"name": "item.get", "args": {}}),
                            None,
                            2,
                        ),
                    ],
                    ..Turn::default()
                },
                Turn {
                    usage: usage(5, 0, 9_900, 20),
                    ..Turn::default()
                },
            ],
            ..Transcript::default()
        };
        let m = measure(&transcript, &Err("V1 has 6 clips, not 7".into()));
        assert!(!m.success);
        assert_eq!(m.failure, "V1 has 6 clips, not 7");
        assert_eq!((m.turns, m.tool_calls, m.tool_errors), (4, 8, 2));
        // project_info, timeline_get, describe, search, execute:clip.split, execute:clip.trim,
        // query:item.get; timeline_get and both commands of the batch failed at first.
        assert_eq!((m.tools_used, m.first_calls_valid), (7, 4));
        assert_eq!(
            m.first_calls_invalid,
            [
                "timeline_get (invalid)",
                "cutweave_execute:clip.split (conflict)",
                "cutweave_execute:clip.trim (conflict)"
            ]
        );
        assert_eq!(
            (m.describe_calls, m.search_calls, m.tool_search_calls),
            (1, 1, 1)
        );
        assert_eq!(m.batches, 2);
        assert_eq!(m.input_tokens, 9_100 + 9_800 + 9_810 + 9_905);
        assert_eq!(m.cache_read_tokens, 9_000 + 9_800 + 9_900);
        assert_eq!(m.output_tokens, 170);
        // 7,003 bytes of results: a quarter, rounded up.
        assert_eq!(m.tool_result_tokens, 1_751);
        assert_eq!(m.tool_definition_tokens, 7_742);
        assert_eq!(m.tool_definition_tokens_in_context, 1_500);
        assert!(measure(&transcript, &Ok(())).success);
    }

    #[test]
    fn a_session_written_by_hand_needs_only_tools_and_arguments() {
        let script = json!({
            "task": "set-speed",
            "turns": [{"calls": [{"tool": "clip_set_speed", "input": {"speed": "2x"}}]}]
        });
        let transcript: Transcript = serde_json::from_value(script).unwrap();
        assert_eq!(transcript.task, "set-speed");
        assert_eq!(transcript.turns[0].calls[0].tool, "clip_set_speed");
        assert_eq!(transcript.turns[0].usage, Usage::default());
        assert_eq!(tokens_estimate(0), 0);
        assert_eq!(tokens_estimate(5), 2);
    }
}
