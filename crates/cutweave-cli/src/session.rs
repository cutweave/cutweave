// SPDX-License-Identifier: GPL-3.0-or-later
//! A connection to an engine, in process or over `/rpc`, and the request shapes `exec` and
//! `query` run on it. The CLI adds no command logic: every call reaches the engine's
//! dispatcher (arch §5.1).

use std::path::PathBuf;

use cutweave_api::Credential;
use cutweave_api::engines::{SelectError, Selector};
use cutweave_api::mcp::execute::{check_step_refs, resolve_refs};
use cutweave_commands::{CmdError, EntryKind, Registry, Rev, TxId};
use cutweave_engine::{
    AudioConfig, Call, Connection, DevStubMedia, Engine, EngineConfig, EngineMode, FullSubsystems,
    Subsystems,
};
use serde_json::{Map, Value, json};

use crate::failure::Failure;
use crate::remote::{self, Remote};

/// The label of the agent credential the CLI uses.
const CLI_AGENT: &str = "cli";

/// Most steps in one batch.
const MAX_STEPS: usize = 100;

/// A call's result.
#[derive(Debug)]
pub struct Done {
    /// The command's output (an object).
    pub output: Value,
    /// The live revision afterwards.
    pub rev: u64,
    /// The committed revision afterwards.
    pub committed_rev: u64,
}

/// How to reach an engine.
#[derive(Debug)]
pub enum Plan {
    /// Attach to a running engine.
    Attach {
        /// The run-file directory (`None`: the default).
        run_dir: Option<PathBuf>,
        /// Which engine: `--pid`, `--project`, or neither (the app's focused engine).
        selector: Selector,
        /// With `--project <path>` and no engine that has it open: open it in a one-shot
        /// engine instead (what `--project` did before engines could be chosen by project).
        fallback: Option<SpawnPlan>,
    },
    /// Start a one-shot engine in this process.
    Spawn(SpawnPlan),
}

/// What the spawned engine is started with.
#[derive(Debug, Default)]
pub struct SpawnPlan {
    /// `--project`.
    pub project: Option<PathBuf>,
    /// `--deterministic-ids` or `CUTWEAVE_DETERMINISTIC_IDS`.
    pub deterministic_ids: Option<u64>,
    /// `--allow-root`.
    pub allow_roots: Vec<PathBuf>,
    /// `--dev-stub-media` or `CUTWEAVE_DEV_STUB_MEDIA`.
    pub dev_stub_media: bool,
}

/// A connection to an engine.
pub enum Session {
    /// A one-shot engine in this process.
    Local {
        /// The engine (shut down by [`Session::close`]).
        engine: Engine,
        /// The `cli` agent connection.
        conn: Connection,
    },
    /// A running engine reached over `/rpc`.
    Remote(Remote),
}

impl Session {
    /// Opens the session `plan` describes.
    ///
    /// # Errors
    /// Exit 3 when there is no engine to attach to; exit 1 when the spawned engine cannot start
    /// (for example `project_locked`).
    pub fn open(plan: Plan) -> Result<Session, Failure> {
        match plan {
            Plan::Attach {
                run_dir,
                selector,
                fallback,
            } => match remote::find(run_dir.as_deref(), &selector) {
                Ok(engine) => Ok(Session::Remote(Remote::connect(&engine)?)),
                Err(miss) => match (fallback, miss.why) {
                    (Some(spawn), SelectError::NoEngine | SelectError::NoMatch) => {
                        Session::open(Plan::Spawn(spawn))
                    }
                    _ => Err(miss.failure),
                },
            },
            Plan::Spawn(spawn) => {
                let mut config = EngineConfig::new(EngineMode::Oneshot);
                config.project = spawn.project;
                config.deterministic_ids = spawn.deterministic_ids;
                config.allowed_roots = spawn.allow_roots;
                // The full engine on the null audio output (see `cutweave mcp --headless`);
                // `--dev-stub-media` keeps the stub.
                config.audio = AudioConfig::Null;
                config.subsystems = Some(if spawn.dev_stub_media {
                    Box::new(DevStubMedia) as Box<dyn Subsystems>
                } else {
                    Box::new(FullSubsystems::native())
                });
                let engine =
                    Engine::start(config).map_err(|e| Failure::error(e.code(), e.to_string()))?;
                let conn = engine
                    .handle()
                    .connect(&Credential::Agent(CLI_AGENT.into()))
                    .map_err(|e| Failure::from_cmd(&e))?;
                Ok(Session::Local { engine, conn })
            }
        }
    }

    /// One call: `tx` runs it inside that open transaction.
    pub fn call(
        &mut self,
        name: &str,
        params: Value,
        tx: Option<&str>,
        expected_rev: Option<u64>,
    ) -> Result<Done, Failure> {
        match self {
            Session::Remote(remote) => remote.call(name, params, tx, expected_rev),
            Session::Local { conn, .. } => {
                let mut call = Call::new(name, params);
                if let Some(tx) = tx {
                    let tx: TxId = tx
                        .parse()
                        .map_err(|_| Failure::usage(format!("`{tx}` is not a transaction id")))?;
                    call = call.in_tx(tx);
                }
                if let Some(rev) = expected_rev {
                    call = call.with_expected_rev(Rev(rev));
                }
                let reply = conn.call(call).map_err(|e| Failure::from_cmd(&e))?;
                Ok(Done {
                    output: reply.output,
                    rev: reply.rev.0,
                    committed_rev: reply.committed_rev.0,
                })
            }
        }
    }

    /// Ends the session: a spawned engine is shut down, a socket is closed (the engine rolls
    /// back anything still open on it).
    pub fn close(self) {
        match self {
            Session::Local { engine, conn } => {
                drop(conn);
                engine.shutdown();
            }
            Session::Remote(remote) => remote.close(),
        }
    }
}

/// One `{cmd, args}` step.
#[derive(Debug)]
pub struct Step {
    /// The command name.
    pub cmd: String,
    /// Its params (an object).
    pub args: Value,
}

/// A parsed `exec` request.
#[derive(Debug)]
pub enum Request {
    /// An object `{"cmd", "args"}`: one call.
    Single(Step),
    /// An array (or `{"steps": [...]}`): one transaction.
    Batch(Vec<Step>),
}

/// Parses the JSON an `exec` was given.
///
/// # Errors
/// Exit 2 for anything that is not a step, an array of steps or `{"steps": [...]}`.
pub fn parse_request(text: &str) -> Result<Request, Failure> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| Failure::usage(format!("the request is not valid JSON: {e}")))?;
    match value {
        Value::Array(steps) => parse_steps(steps).map(Request::Batch),
        Value::Object(mut map) if map.contains_key("steps") => {
            if let Some(key) = map.keys().find(|k| *k != "steps") {
                return Err(Failure::usage(format!(
                    "unknown field `{key}` next to `steps` (use --expected-rev for the revision)"
                )));
            }
            match map.remove("steps") {
                Some(Value::Array(steps)) => parse_steps(steps).map(Request::Batch),
                _ => Err(Failure::usage("`steps` must be an array of {cmd, args}")),
            }
        }
        Value::Object(map) => parse_step(0, Value::Object(map)).map(Request::Single),
        _ => Err(Failure::usage(
            "expected {\"cmd\": name, \"args\": {...}}, or an array of such steps",
        )),
    }
}

fn parse_steps(steps: Vec<Value>) -> Result<Vec<Step>, Failure> {
    if steps.is_empty() || steps.len() > MAX_STEPS {
        return Err(Failure::usage(format!("expected 1..={MAX_STEPS} steps")));
    }
    steps
        .into_iter()
        .enumerate()
        .map(|(i, step)| parse_step(i, step))
        .collect()
}

fn parse_step(i: usize, step: Value) -> Result<Step, Failure> {
    let Value::Object(mut map) = step else {
        return Err(Failure::usage(format!(
            "step {i}: expected {{\"cmd\": name, \"args\": {{...}}}}"
        )));
    };
    if let Some(key) = map.keys().find(|k| !matches!(k.as_str(), "cmd" | "args")) {
        return Err(Failure::usage(format!(
            "step {i}: unknown field `{key}`; a step is {{\"cmd\": name, \"args\": {{...}}}}"
        )));
    }
    let Some(Value::String(cmd)) = map.remove("cmd") else {
        return Err(Failure::usage(format!(
            "step {i}: `cmd` must be a command name, e.g. clip.split"
        )));
    };
    let args = match map.remove("args") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(args @ Value::Object(_)) => args,
        Some(_) => {
            return Err(Failure::usage(format!(
                "step {i}: `args` must be a JSON object"
            )));
        }
    };
    Ok(Step { cmd, args })
}

fn with_revs(done: &Done) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("rev".into(), json!(done.rev));
    map.insert("committed_rev".into(), json!(done.committed_rev));
    map
}

/// Runs a request and returns the JSON to print: `{rev, committed_rev, output}` for one step,
/// `{rev, committed_rev, steps: [{cmd, output}]}` for a batch.
///
/// # Errors
/// The failing call's error (a batch adds `failed_step`); a failed batch changes nothing.
pub fn run_request(
    session: &mut Session,
    registry: &Registry,
    request: Request,
    expected_rev: Option<u64>,
) -> Result<Value, Failure> {
    match request {
        Request::Single(step) => {
            let done = session.call(&step.cmd, step.args, None, expected_rev)?;
            let mut out = with_revs(&done);
            out.insert("output".into(), done.output);
            Ok(Value::Object(out))
        }
        Request::Batch(steps) => run_batch(session, registry, steps, expected_rev),
    }
}

/// A `$ref` error of step `i`: `Invalid` at `/steps/<i>/args/...`, as `cutweave_execute` reports it.
fn ref_failure(i: usize, error: CmdError) -> Failure {
    Failure::from_cmd(&error.under(&format!("/steps/{i}/args"))).at_step(i)
}

/// Runs steps as one transaction: `edit.begin_tx`, each step inside it, `edit.commit_tx`. A
/// failing step cancels it. An engine command (`edit.undo`, `project.open`, `media.import`, ...)
/// cannot join a transaction, so one runs alone as the only step.
///
/// A step's args may use an earlier step's output, `{"$ref": "steps[0].output.left"}`, in the
/// grammar of MCP's `cutweave_execute` (arch §14): a malformed reference, or one to the same or
/// a later step, is refused before anything runs; one to a value the earlier output lacks fails
/// that step.
fn run_batch(
    session: &mut Session,
    registry: &Registry,
    steps: Vec<Step>,
    expected_rev: Option<u64>,
) -> Result<Value, Failure> {
    for (i, step) in steps.iter().enumerate() {
        check_step_refs(&step.args, i).map_err(|e| ref_failure(i, e))?;
    }
    let alone = steps.len() == 1;
    if !alone
        && let Some((i, step)) = steps.iter().enumerate().find(|(_, s)| {
            registry
                .get(&s.cmd)
                .is_some_and(|e| e.kind() == EntryKind::Engine)
        })
    {
        return Err(Failure::from_cmd(&CmdError::invalid(
            format!("/steps/{i}/cmd"),
            format!(
                "`{}` acts on engine state (history, jobs, import, project, view) and cannot \
                 join a transaction; run it as the only step",
                step.cmd
            ),
        ))
        .at_step(i));
    }
    if alone {
        let step = steps.into_iter().next();
        let Some(step) = step else {
            return Err(Failure::usage("expected at least one step"));
        };
        let name = step.cmd.clone();
        // A batch names its failing step, also when it has only one (as `cutweave_execute`).
        let done = session
            .call(&step.cmd, step.args, None, expected_rev)
            .map_err(|e| e.at_step(0))?;
        let mut out = with_revs(&done);
        out.insert(
            "steps".into(),
            json!([{"cmd": name, "output": done.output}]),
        );
        return Ok(Value::Object(out));
    }
    let opened = session.call(
        "edit.begin_tx",
        json!({"kind": "batch", "label": "cutweave exec"}),
        None,
        expected_rev,
    )?;
    let tx = opened
        .output
        .get("tx")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Failure::error("internal", "edit.begin_tx returned no transaction id"))?;
    let mut results = Vec::with_capacity(steps.len());
    let mut outputs: Vec<Value> = Vec::with_capacity(steps.len());
    for (i, step) in steps.into_iter().enumerate() {
        let args = match resolve_refs(&step.args, &outputs, i) {
            Ok(args) => args,
            Err(e) => {
                let _ = session.call("edit.cancel_tx", json!({"tx": tx}), None, None);
                return Err(ref_failure(i, e));
            }
        };
        match session.call(&step.cmd, args, Some(&tx), None) {
            Ok(done) => {
                outputs.push(done.output.clone());
                results.push(json!({"cmd": step.cmd, "output": done.output}));
            }
            Err(e) => {
                let _ = session.call("edit.cancel_tx", json!({"tx": tx}), None, None);
                return Err(e.at_step(i));
            }
        }
    }
    let committed = match session.call("edit.commit_tx", json!({"tx": tx}), None, expected_rev) {
        Ok(done) => done,
        Err(e) => {
            let _ = session.call("edit.cancel_tx", json!({"tx": tx}), None, None);
            return Err(e);
        }
    };
    let mut out = with_revs(&committed);
    // The commit's own output carries the revisions of the transaction.
    for key in ["rev", "committed_rev"] {
        if let Some(v) = committed.output.get(key) {
            out.insert(key.into(), v.clone());
        }
    }
    out.insert("steps".into(), Value::Array(results));
    Ok(Value::Object(out))
}
