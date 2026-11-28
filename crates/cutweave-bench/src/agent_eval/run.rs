// SPDX-License-Identifier: GPL-3.0-or-later
//! Running a session: with a model ([`conduct`], [`run_session`]) and without one
//! ([`replay`], which makes a recorded session's calls again).

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use super::anthropic::{
    ClientMode, RequestShape, Stop, ToolSearch, Transport, build_request, send, tool_entries,
    tool_results_message,
};
use super::surface::{
    EvalEngine, IMPORTS_PLACEHOLDER, McpSurface, Outcome, ToolDef, ToolSurface, replace_text,
};
use super::tasks::{Scene, Task, task};
use super::transcript::{Call, FORMAT, Measures, ToolSizes, Transcript, Turn, measure};
use crate::BenchError;

/// What the runner tells the model before the server's own instructions.
pub const SYSTEM_PROMPT: &str = "You are operating the video editor Cutweave for a user, \
through the tools you are given. The project is already open. Do what the user asks; do not \
ask questions back. When the work is done, answer with one short sentence.";

/// Whether a variant's task text says anything about batching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Batching {
    /// The task text is the user's request and nothing else.
    Free,
    /// The text asks for one `cutweave_execute` call with all the changes.
    OneBatch,
    /// The text asks for one change per call.
    Forbidden,
}

/// One way of giving the model the same task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Variant {
    /// Its name on the command line and in the report.
    pub name: &'static str,
    /// What it is, for `--list` and the report.
    pub what: &'static str,
    /// The MCP endpoint whose list the model gets.
    pub endpoint: &'static str,
    /// The task's own core tools are left out of the list the model is shown, so their
    /// commands have to go through `cutweave_execute`. The server is not changed.
    pub hide_core: bool,
    /// What the task text says about batching.
    pub batching: Batching,
    /// `false`: named so that it can be asked for, but not built yet.
    pub available: bool,
}

/// The variants. `cli-skill` (the command line with a skill instead of MCP) is WP-2I.10: a
/// second [`ToolSurface`] with a shell tool, to be added here with `available: true`.
pub static VARIANTS: [Variant; 6] = [
    Variant {
        name: "standard",
        what: "the standard list (/mcp): no parameter descriptions",
        endpoint: "/mcp",
        hide_core: false,
        batching: Batching::Free,
        available: true,
    },
    Variant {
        name: "described",
        what: "the described list (/mcp/described): every parameter described",
        endpoint: "/mcp/described",
        hide_core: false,
        batching: Batching::Free,
        available: true,
    },
    Variant {
        name: "via-execute",
        what: "the standard list without the task's core tools: the same commands through cutweave_execute",
        endpoint: "/mcp",
        hide_core: true,
        batching: Batching::Free,
        available: true,
    },
    Variant {
        name: "batch",
        what: "the standard list; the task asks for all changes in one cutweave_execute call",
        endpoint: "/mcp",
        hide_core: false,
        batching: Batching::OneBatch,
        available: true,
    },
    Variant {
        name: "no-batch",
        what: "the standard list; the task asks for one change per call",
        endpoint: "/mcp",
        hide_core: false,
        batching: Batching::Forbidden,
        available: true,
    },
    Variant {
        name: "cli-skill",
        what: "the command line with a skill instead of MCP (not built: arrives with WP-2I.10)",
        endpoint: "",
        hide_core: false,
        batching: Batching::Free,
        available: false,
    },
];

/// The variant called `name`.
#[must_use]
pub fn variant(name: &str) -> Option<&'static Variant> {
    VARIANTS.iter().find(|v| v.name == name)
}

impl Variant {
    /// Whether this variant says anything about `task` that `standard` does not.
    #[must_use]
    pub fn applies_to(&self, task: &Task) -> bool {
        match self.batching {
            Batching::Free => !self.hide_core || !task.core_tools.is_empty(),
            Batching::OneBatch | Batching::Forbidden => task.batchable,
        }
    }

    /// The task text of this variant.
    #[must_use]
    pub fn prompt(&self, task: &Task, scene: &Scene) -> String {
        let asked = (task.prompt)(scene);
        match self.batching {
            Batching::Free => asked,
            Batching::OneBatch => format!(
                "{asked}\n\nMake all the changes in a single cutweave_execute call, as one batch."
            ),
            Batching::Forbidden => format!(
                "{asked}\n\nMake each change with a call of its own: never put more than one \
                 step into a cutweave_execute call."
            ),
        }
    }

    /// The tools the model is shown: the surface's list, without the task's core tools when
    /// the variant hides them.
    #[must_use]
    pub fn shown(&self, task: &Task, tools: &[ToolDef]) -> Vec<ToolDef> {
        tools
            .iter()
            .filter(|tool| !(self.hide_core && task.core_tools.contains(&tool.name.as_str())))
            .cloned()
            .collect()
    }
}

/// The system prompt of a session on a surface with these instructions.
#[must_use]
pub fn system_prompt(instructions: &str) -> String {
    if instructions.is_empty() {
        return SYSTEM_PROMPT.to_owned();
    }
    format!("{SYSTEM_PROMPT}\n\nThe editor describes itself like this:\n\n{instructions}")
}

fn json_bytes(value: &Value) -> u64 {
    value.to_string().len() as u64
}

/// The sizes of the definitions of `tools` in `mode`, with `loaded` found by tool search.
#[must_use]
pub fn tool_sizes(
    tools: &[ToolDef],
    mode: ClientMode,
    search: ToolSearch,
    loaded: &BTreeSet<String>,
) -> ToolSizes {
    let entries = tool_entries(tools, mode, search);
    let mut sizes = ToolSizes {
        count: tools.len() as u64,
        ..ToolSizes::default()
    };
    for entry in &entries {
        let bytes = json_bytes(entry);
        sizes.sent_bytes += bytes;
        if entry["defer_loading"] != true {
            sizes.upfront_bytes += bytes;
        } else if loaded.contains(entry["name"].as_str().unwrap_or_default()) {
            sizes.loaded_bytes += bytes;
        }
    }
    sizes
}

/// Runs one call for the model: on the surface when the tool is one it was shown, else an
/// "unknown tool" error that never reaches the engine.
fn dispatch(
    surface: &mut dyn ToolSurface,
    shown: &[ToolDef],
    name: &str,
    input: &Value,
) -> Result<Outcome, BenchError> {
    if shown.iter().any(|tool| tool.name == name) {
        surface.call(name, input)
    } else {
        Ok(Outcome::refused(
            "unknown_tool",
            format!("Error: unknown tool `{name}`"),
        ))
    }
}

/// What a session with a model needs besides the surface.
pub struct Conduct<'a> {
    /// The model, the client mode and the request limits.
    pub shape: &'a RequestShape,
    /// The tools the model is shown.
    pub shown: &'a [ToolDef],
    /// The task text.
    pub prompt: &'a str,
    /// The most responses a session may take.
    pub max_turns: u32,
    /// This machine's import folder, kept out of the record.
    pub imports: &'a str,
}

/// What [`conduct`] returns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conducted {
    /// The model's responses and their calls.
    pub turns: Vec<Turn>,
    /// How the session ended (see [`Transcript::ended`]).
    pub ended: String,
    /// The tools a tool search loaded.
    pub loaded: BTreeSet<String>,
    /// The API refused for a reason that ends the whole run (the key).
    pub fatal: bool,
}

/// The loop of a session: send the conversation, run the calls of the response, send the
/// results, until the model stops calling tools or `max_turns` responses were read.
pub fn conduct(
    surface: &mut dyn ToolSurface,
    transport: &mut dyn Transport,
    plan: &Conduct<'_>,
    pause: &mut dyn FnMut(Duration),
) -> Result<Conducted, BenchError> {
    let system = system_prompt(surface.instructions());
    let mut messages = vec![json!({"role": "user", "content": plan.prompt})];
    let mut done = Conducted::default();
    loop {
        if done.turns.len() >= plan.max_turns as usize {
            "max_turns".clone_into(&mut done.ended);
            return Ok(done);
        }
        let body = build_request(plan.shape, &system, plan.shown, &messages);
        let reply = match send(transport, &body, pause)? {
            Ok(reply) => reply,
            Err(refused) => {
                done.fatal = refused.ends_the_run();
                done.ended = format!("error: HTTP {} {}", refused.status, refused.message);
                return Ok(done);
            }
        };
        let mut turn = Turn {
            usage: reply.usage,
            stop_reason: reply.stop.name().to_owned(),
            searches: reply.searches.clone(),
            calls: Vec::new(),
        };
        let found = reply.searches.iter().flat_map(|search| search.found.iter());
        done.loaded.extend(found.cloned());
        messages.push(json!({"role": "assistant", "content": reply.content}));
        match reply.stop {
            Stop::ToolUse => {
                let mut results = Vec::new();
                for call in &reply.tool_uses {
                    let outcome = dispatch(surface, plan.shown, &call.name, &call.input)?;
                    turn.calls.push(Call {
                        tool: call.name.clone(),
                        input: replace_text(&call.input, plan.imports, IMPORTS_PLACEHOLDER),
                        is_error: outcome.is_error,
                        error_code: outcome.error_code,
                        result_bytes: outcome.text.len() as u64,
                    });
                    results.push((call.id.clone(), outcome.text, outcome.is_error));
                }
                done.turns.push(turn);
                if results.is_empty() {
                    // Not a documented response; an empty user message would be refused.
                    "tool_use without a call".clone_into(&mut done.ended);
                    return Ok(done);
                }
                messages.push(tool_results_message(&results));
            }
            // A server tool (the tool search) is still running: the same conversation again.
            Stop::PauseTurn => done.turns.push(turn),
            ended => {
                ended.name().clone_into(&mut done.ended);
                done.turns.push(turn);
                return Ok(done);
            }
        }
    }
}

/// One session of a run, as the result file keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The task.
    pub task: String,
    /// The variant.
    pub variant: String,
    /// The client mode.
    pub client: String,
    /// What was measured.
    pub measures: Measures,
    /// Every call the model made.
    pub transcript: Transcript,
}

/// Where and with what a run's sessions are made.
pub struct Bench<'a> {
    /// The `cutweave-bench` executable (its `engine` subcommand is the engine).
    pub exe: &'a Path,
    /// A folder for the engine's project and run files; one sub-folder per session.
    pub scratch: &'a Path,
    /// The runtime of the MCP client.
    pub rt: &'a Arc<Runtime>,
}

/// One session to run.
#[derive(Clone, Copy)]
pub struct Plan {
    /// The task.
    pub task: &'static Task,
    /// The variant.
    pub variant: &'static Variant,
    /// The client mode.
    pub client: ClientMode,
}

impl Plan {
    /// `task/variant/client`, also the name of the session's scratch folder.
    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "{}/{}/{}",
            self.task.id,
            self.variant.name,
            self.client.name()
        )
    }
}

/// A started engine with the surface and the task text of a plan.
pub struct Stage {
    /// The engine.
    pub engine: EvalEngine,
    /// The MCP session.
    pub surface: McpSurface,
    /// The project before the session.
    pub scene: Scene,
    /// The tools the model is shown.
    pub shown: Vec<ToolDef>,
    /// The task text.
    pub prompt: String,
}

impl Stage {
    /// Starts a fresh engine for `plan` and connects to the variant's endpoint.
    pub fn open(bench: &Bench<'_>, plan: &Plan) -> Result<Stage, BenchError> {
        let scratch = bench.scratch.join(plan.label().replace('/', "_"));
        let engine = EvalEngine::start(bench.exe, &scratch)?;
        let scene = Scene::new(&engine.fixture, &engine.imports_text())?;
        let surface = McpSurface::connect(bench.rt, &engine.engine, plan.variant.endpoint)?;
        let shown = plan.variant.shown(plan.task, surface.tools());
        let prompt = plan.variant.prompt(plan.task, &scene);
        Ok(Stage {
            engine,
            surface,
            scene,
            shown,
            prompt,
        })
    }
}

/// Runs one session with a model. The second value: the API refused the key, so the run
/// should stop.
pub fn run_session(
    bench: &Bench<'_>,
    plan: &Plan,
    shape: &RequestShape,
    max_turns: u32,
    transport: &mut dyn Transport,
) -> Result<(Session, bool), BenchError> {
    let mut stage = Stage::open(bench, plan)?;
    let imports = stage.engine.imports_text();
    let conducted = conduct(
        &mut stage.surface,
        transport,
        &Conduct {
            shape,
            shown: &stage.shown,
            prompt: &stage.prompt,
            max_turns,
            imports: &imports,
        },
        &mut |wait| std::thread::sleep(wait),
    )?;
    let ended_with = stage.engine.final_state()?;
    let verdict = (plan.task.check)(&stage.scene, &ended_with);
    let transcript = Transcript {
        format: FORMAT,
        task: plan.task.id.to_owned(),
        variant: plan.variant.name.to_owned(),
        client: plan.client.name().to_owned(),
        model: shape.model.clone(),
        endpoint: plan.variant.endpoint.to_owned(),
        tools: tool_sizes(&stage.shown, shape.mode, shape.search, &conducted.loaded),
        system_bytes: system_prompt(stage.surface.instructions()).len() as u64,
        turns: conducted.turns,
        ended: conducted.ended,
    };
    let session = Session {
        task: transcript.task.clone(),
        variant: transcript.variant.clone(),
        client: transcript.client.clone(),
        measures: measure(&transcript, &verdict),
        transcript,
    };
    Ok((session, conducted.fatal))
}

/// A recorded session made again on a real engine.
#[derive(Clone, Debug, PartialEq)]
pub struct Replayed {
    /// The session with the results of this run (sizes, errors).
    pub session: Session,
    /// Calls that went differently than recorded (`turn 2 call 1: clip_split failed ...`).
    pub differences: Vec<String>,
}

/// Makes the calls of `recorded` again, in order, on a fresh engine, with no model: the
/// tools shown are the recorded variant's, the checker is the recorded task's, and the
/// token counts are the recorded ones.
pub fn replay(bench: &Bench<'_>, recorded: &Transcript) -> Result<Replayed, BenchError> {
    let unknown = |what: &str, name: &str| BenchError::Spec(format!("unknown {what} `{name}`"));
    let name_or = |name: &str, default: &str| {
        if name.is_empty() {
            default.to_owned()
        } else {
            name.to_owned()
        }
    };
    let variant_name = name_or(&recorded.variant, "standard");
    let client_name = name_or(&recorded.client, "upfront");
    let plan = Plan {
        task: task(&recorded.task).ok_or_else(|| unknown("task", &recorded.task))?,
        variant: variant(&variant_name)
            .filter(|v| v.available)
            .ok_or_else(|| unknown("variant", &variant_name))?,
        client: ClientMode::parse(&client_name).ok_or_else(|| unknown("client", &client_name))?,
    };
    let mut stage = Stage::open(bench, &plan)?;
    let imports = stage.engine.imports_text();
    let (mut turns, mut differences, mut loaded) = (Vec::new(), Vec::new(), BTreeSet::new());
    for (t, turn) in recorded.turns.iter().enumerate() {
        let mut made = Turn {
            calls: Vec::new(),
            ..turn.clone()
        };
        loaded.extend(turn.searches.iter().flat_map(|s| s.found.iter().cloned()));
        for (c, call) in turn.calls.iter().enumerate() {
            let input = replace_text(&call.input, IMPORTS_PLACEHOLDER, &imports);
            let outcome = dispatch(&mut stage.surface, &stage.shown, &call.tool, &input)?;
            if outcome.is_error != call.is_error {
                let first_line = outcome.text.lines().next().unwrap_or_default();
                differences.push(format!(
                    "turn {} call {}: {} {} now ({first_line})",
                    t + 1,
                    c + 1,
                    call.tool,
                    if outcome.is_error { "fails" } else { "works" },
                ));
            }
            made.calls.push(Call {
                tool: call.tool.clone(),
                input: call.input.clone(),
                is_error: outcome.is_error,
                error_code: outcome.error_code,
                result_bytes: outcome.text.len() as u64,
            });
        }
        turns.push(made);
    }
    let ended_with = stage.engine.final_state()?;
    let verdict = (plan.task.check)(&stage.scene, &ended_with);
    let transcript = Transcript {
        format: FORMAT,
        task: plan.task.id.to_owned(),
        variant: variant_name,
        client: client_name,
        model: recorded.model.clone(),
        endpoint: plan.variant.endpoint.to_owned(),
        tools: tool_sizes(&stage.shown, plan.client, ToolSearch::Bm25, &loaded),
        system_bytes: system_prompt(stage.surface.instructions()).len() as u64,
        turns,
        ended: name_or(&recorded.ended, "end_turn"),
    };
    let session = Session {
        task: transcript.task.clone(),
        variant: transcript.variant.clone(),
        client: transcript.client.clone(),
        measures: measure(&transcript, &verdict),
        transcript,
    };
    Ok(Replayed {
        session,
        differences,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_eval::anthropic::{DEFAULT_MAX_TOKENS, DEFAULT_MODEL, HttpAnswer};
    use crate::agent_eval::tasks::TASKS;

    fn tool(name: &str, always_load: bool) -> ToolDef {
        ToolDef {
            name: name.to_owned(),
            description: format!("{name}."),
            input_schema: json!({"type": "object"}),
            always_load,
        }
    }

    /// A surface that answers every call and remembers it.
    struct Fake(Vec<ToolDef>, Vec<(String, Value)>);

    impl ToolSurface for Fake {
        fn tools(&self) -> &[ToolDef] {
            &self.0
        }
        fn instructions(&self) -> &'static str {
            "A fake editor."
        }
        fn call(&mut self, name: &str, input: &Value) -> Result<Outcome, BenchError> {
            self.1.push((name.to_owned(), input.clone()));
            let failed = input["fail"] == true;
            Ok(Outcome {
                text: format!("{name}: {}", if failed { "failed" } else { "done" }),
                is_error: failed,
                error_code: failed.then(|| "invalid".to_owned()),
            })
        }
    }

    /// A transport that answers from a list and keeps the requests.
    struct Scripted(Vec<Value>, Vec<Value>);

    impl Transport for Scripted {
        fn post(&mut self, body: &Value) -> Result<HttpAnswer, BenchError> {
            self.1.push(body.clone());
            let (status, body) = if self.0.is_empty() {
                (
                    400,
                    json!({"error": {"type": "invalid_request_error", "message": "no more"}}),
                )
            } else {
                (200, self.0.remove(0))
            };
            Ok(HttpAnswer {
                status,
                retry_after: None,
                body,
            })
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

    fn uses(calls: &[(&str, &str, Value)], input_tokens: u64) -> Value {
        let content: Vec<Value> = calls
            .iter()
            .map(|(id, name, input)| json!({"type": "tool_use", "id": id, "name": name, "input": input}))
            .collect();
        json!({"content": content, "stop_reason": "tool_use",
               "usage": {"input_tokens": input_tokens, "output_tokens": 7}})
    }

    #[test]
    fn a_session_runs_the_calls_and_sends_the_results_back() {
        let mut surface = Fake(
            vec![tool("project_info", true), tool("clip_split", false)],
            Vec::new(),
        );
        let shown = [tool("project_info", true)];
        let path = json!({"paths": ["C:/w/imports/beach.mp4"]});
        let mut transport = Scripted(
            vec![
                uses(
                    &[
                        ("toolu_1", "project_info", path),
                        ("toolu_2", "clip_split", json!({})),
                    ],
                    100,
                ),
                json!({"content": [{"type": "server_tool_use", "id": "srv_1",
                                    "name": "tool_search_tool_bm25", "input": {"query": "split"}}],
                       "stop_reason": "pause_turn"}),
                uses(&[("toolu_3", "project_info", json!({"fail": true}))], 300),
                json!({"content": [{"type": "text", "text": "Done."}], "stop_reason": "end_turn",
                       "usage": {"input_tokens": 400, "output_tokens": 3}}),
            ],
            Vec::new(),
        );
        let shape = shape(ClientMode::Upfront);
        let plan = Conduct {
            shape: &shape,
            shown: &shown,
            prompt: "Split it.",
            max_turns: 10,
            imports: "C:/w/imports",
        };
        let mut waits = 0;
        let done = conduct(&mut surface, &mut transport, &plan, &mut |_| waits += 1).unwrap();
        assert_eq!(
            (done.ended.as_str(), done.turns.len(), done.fatal),
            ("end_turn", 4, false)
        );
        // The hidden tool never reached the surface; the model got an error for it.
        assert_eq!(surface.1.len(), 2);
        let first = &done.turns[0].calls;
        assert_eq!(first[0].input, json!({"paths": ["{{imports}}/beach.mp4"]}));
        assert_eq!(
            (first[1].is_error, first[1].error_code.as_deref()),
            (true, Some("unknown_tool"))
        );
        assert_eq!(first[0].result_bytes, "project_info: done".len() as u64);
        assert_eq!(
            done.turns[2].calls[0].error_code.as_deref(),
            Some("invalid")
        );
        assert_eq!(done.turns[1].stop_reason, "pause_turn");
        // The requests: the same tools and system every time, the conversation growing.
        let requests = &transport.1;
        assert_eq!(requests.len(), 4);
        assert_eq!(
            requests[0]["messages"],
            json!([{"role": "user", "content": "Split it."}])
        );
        assert!(
            requests[0]["system"][0]["text"]
                .as_str()
                .unwrap()
                .ends_with("A fake editor.")
        );
        assert_eq!(requests[0]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(requests[3]["tools"], requests[0]["tools"]);
        let second = requests[1]["messages"].as_array().unwrap();
        assert_eq!(second.len(), 3);
        assert_eq!(second[1]["role"], "assistant");
        assert_eq!(second[1]["content"][1]["name"], "clip_split");
        assert_eq!(
            second[2]["content"],
            json!([
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "project_info: done"},
                {"type": "tool_result", "tool_use_id": "toolu_2",
                 "content": "Error: unknown tool `clip_split`", "is_error": true},
            ])
        );
        // After a paused turn the conversation goes back with the assistant's blocks only.
        assert_eq!(requests[2]["messages"].as_array().unwrap().len(), 4);
        assert_eq!(requests[2]["messages"][3]["role"], "assistant");
        assert_eq!(waits, 0);
    }

    #[test]
    fn a_session_ends_at_the_turn_limit_or_when_the_api_refuses() {
        let mut surface = Fake(vec![tool("project_info", true)], Vec::new());
        let shown = surface.0.clone();
        let shape = shape(ClientMode::Deferred);
        let plan = Conduct {
            shape: &shape,
            shown: &shown,
            prompt: "Look.",
            max_turns: 2,
            imports: "C:/w/imports",
        };
        let again = || uses(&[("toolu_1", "project_info", json!({}))], 10);
        let mut transport = Scripted(vec![again(), again(), again()], Vec::new());
        let done = conduct(&mut surface, &mut transport, &plan, &mut |_| {}).unwrap();
        assert_eq!((done.ended.as_str(), done.turns.len()), ("max_turns", 2));
        assert_eq!(transport.1.len(), 2);
        assert_eq!(
            transport.1[0]["tools"][0]["type"],
            "tool_search_tool_bm25_20251119"
        );

        let mut transport = Scripted(Vec::new(), Vec::new());
        let done = conduct(&mut surface, &mut transport, &plan, &mut |_| {}).unwrap();
        assert_eq!(done.ended, "error: HTTP 400 invalid_request_error: no more");
        assert!(done.turns.is_empty() && !done.fatal);
    }

    #[test]
    fn the_variants_and_what_they_change() {
        let split = task("split-and-trim").unwrap();
        let long_tail = task("long-tail").unwrap();
        let speed = task("set-speed").unwrap();
        let names: Vec<&str> = VARIANTS.iter().map(|v| v.name).collect();
        assert_eq!(
            names,
            [
                "standard",
                "described",
                "via-execute",
                "batch",
                "no-batch",
                "cli-skill"
            ]
        );
        assert!(!variant("cli-skill").unwrap().available);
        assert_eq!(variant("described").unwrap().endpoint, "/mcp/described");
        let via = variant("via-execute").unwrap();
        assert!(via.applies_to(split) && !via.applies_to(long_tail));
        let listed = [
            tool("clip_split", false),
            tool("clip_trim", false),
            tool("cutweave_execute", true),
        ];
        let shown: Vec<String> = via
            .shown(split, &listed)
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(shown, ["cutweave_execute"]);
        assert_eq!(variant("standard").unwrap().shown(split, &listed).len(), 3);
        let batch = variant("batch").unwrap();
        assert!(batch.applies_to(split) && !batch.applies_to(speed));
        let applicable = |v: &Variant| TASKS.iter().filter(|t| v.applies_to(t)).count();
        assert_eq!(applicable(variant("standard").unwrap()), 10);
        assert_eq!(applicable(via), 9);
        assert_eq!(applicable(batch), 4);
        assert_eq!(applicable(variant("no-batch").unwrap()), 4);
    }

    #[test]
    fn the_sizes_of_the_definitions_in_each_mode() {
        let tools = [
            tool("project_info", true),
            tool("clip_split", false),
            tool("clip_trim", false),
        ];
        let none = BTreeSet::new();
        let upfront = tool_sizes(&tools, ClientMode::Upfront, ToolSearch::Bm25, &none);
        assert_eq!(upfront.count, 3);
        assert_eq!(upfront.sent_bytes, upfront.upfront_bytes);
        assert_eq!(upfront.loaded_bytes, 0);
        let loaded = BTreeSet::from(["clip_split".to_owned()]);
        let deferred = tool_sizes(&tools, ClientMode::Deferred, ToolSearch::Bm25, &loaded);
        assert!(deferred.upfront_bytes < upfront.upfront_bytes);
        assert!(deferred.loaded_bytes > 0);
        assert!(deferred.upfront_bytes + deferred.loaded_bytes < deferred.sent_bytes);
        assert_eq!(system_prompt(""), SYSTEM_PROMPT);
    }
}
