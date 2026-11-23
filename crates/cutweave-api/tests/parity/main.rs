// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1O.02: CLI = MCP = RPC parity (arch §14 "Parity guarantee", §20 "Client parity",
//! ADR-0002, ADR-0005).
//!
//! - **Scripts** (`testdata/scripts/*.json`, format in [`script`]): each one runs on every
//!   [`Surface`], every time against a fresh engine with `--deterministic-ids 42` (seeded ids,
//!   frozen clock) and the dev stub media backend: `cutweave exec` / `cutweave query` child
//!   processes ([`cli`]), an rmcp client over the stdio transport of `cutweave mcp --headless`
//!   ([`mcp`]; in this process, and as a real child process), and a raw JSON-RPC WebSocket
//!   client ([`rpc`]). Each run ends with `project.save_as` through the same surface. The saved
//!   `.cutweave` documents must be byte-identical (equal sha256), and the `doc.patched` ops, the
//!   step outputs and the error codes equal.
//! - **Actors**: each surface edits as its own credential (`agent:cli`, `agent:mcp`,
//!   `agent:rpc`). The document has no actor field (history lives in the engine), so the bytes
//!   are compared as they are; the actor is compared per surface instead (`doc.patched`, history
//!   results).
//! - **Registry enumeration** ([`enumeration`]): every non-`view` entry is reachable over MCP
//!   with the registry's input schema (verbatim from `cutweave_describe_command`, and
//!   validation-identical as a listed tool), over RPC, and through `cutweave describe`; a
//!   `hidden` entry needs an exemption in `schema/parity-exempt.toml`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::too_many_lines,
    clippy::must_use_candidate,
    missing_docs
)]

mod cli;
mod engine;
mod enumeration;
mod mcp;
#[path = "../mcp_support/mod.rs"]
mod mcp_support;
mod rpc;
#[path = "../rpc_support/mod.rs"]
mod rpc_support;
mod script;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::cli::Cli;
use crate::engine::{EngineKind, ParityEngine};
use crate::mcp::Mcp;
use crate::mcp_support::Revision;
use crate::rpc::Rpc;
use crate::script::{Action, Call, Outcome, Scope, Script, resolve};

/// Not a test: the `cutweave` binary, when the harness re-executes this test binary as a CLI
/// child ([`cli::cutweave`]). A no-op otherwise.
#[test]
fn cli_child() {
    let Ok(args) = std::env::var(cli::ARGS_ENV) else {
        return;
    };
    let args: Vec<String> = serde_json::from_str(&args).expect("the child's arguments");
    let code = cutweave_cli::run(std::iter::once("cutweave".to_owned()).chain(args));
    std::process::exit(code);
}

/// The surfaces a script runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// `cutweave exec` / `cutweave query` child processes attached to the engine.
    Cli,
    /// The `cutweave mcp --headless` handler over stdio pipes, in this process (MCP
    /// 2025-11-25), so the harness sees the engine's events.
    Mcp,
    /// `cutweave mcp --headless` as a child process (MCP 2026-07-28): the real binary path,
    /// documents, outputs and errors only (its engine's events stay in the child).
    McpProcess,
    /// A raw JSON-RPC WebSocket client.
    Rpc,
}

impl Surface {
    const ALL: [Surface; 4] = [
        Surface::Cli,
        Surface::Mcp,
        Surface::McpProcess,
        Surface::Rpc,
    ];

    fn name(self) -> &'static str {
        match self {
            Surface::Cli => "cli",
            Surface::Mcp => "mcp",
            Surface::McpProcess => "mcp-process",
            Surface::Rpc => "rpc",
        }
    }

    /// The actor the engine records for this surface's edits.
    fn actor(self) -> &'static str {
        match self {
            Surface::Cli => "agent:cli",
            Surface::Mcp | Surface::McpProcess => "agent:mcp",
            Surface::Rpc => "agent:rpc",
        }
    }
}

/// A surface's client for one run.
enum Driver<'a> {
    Cli(Cli<'a>),
    Mcp(Box<Mcp>),
    Rpc(Box<Rpc<'a>>),
}

impl Driver<'_> {
    fn call(&mut self, call: &Call) -> Outcome {
        match self {
            Driver::Cli(d) => d.call(call),
            Driver::Mcp(d) => d.call(call),
            Driver::Rpc(d) => d.call(call),
        }
    }

    fn batch(&mut self, calls: &[Call]) -> Outcome {
        match self {
            Driver::Cli(d) => d.batch(calls),
            Driver::Mcp(d) => d.batch(calls),
            Driver::Rpc(d) => d.batch(calls),
        }
    }

    fn close(self) {
        match self {
            Driver::Cli(_) => {}
            Driver::Mcp(d) => d.close(),
            Driver::Rpc(d) => d.close(),
        }
    }
}

/// What one run of a script produced.
struct Run {
    surface: Surface,
    outcomes: Vec<Outcome>,
    /// The `doc.patched` sequence (`None`: the engine is in another process).
    patches: Option<Vec<Value>>,
    document: Vec<u8>,
    sha256: String,
}

/// The scripts directory.
fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/scripts")
}

/// Every script, by file name.
fn scripts() -> Vec<Script> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(scripts_dir())
        .expect("testdata/scripts")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    paths.iter().map(|p| Script::load(p)).collect()
}

/// The stub media files of `script` under `root/media` (the dev stub backend probes by
/// extension; the bytes only feed the content hash).
fn create_media(script: &Script, root: &Path) -> PathBuf {
    let media = root.join("media");
    std::fs::create_dir_all(&media).unwrap();
    for (i, name) in script.media().iter().enumerate() {
        let byte = u8::try_from(i % 251).unwrap();
        std::fs::write(media.join(name), vec![byte; 4096]).unwrap();
    }
    media
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Runs `script` on `surface` in a fresh engine under `root`.
fn run(script: &Script, surface: Surface, root: &Path) -> Run {
    let media = root.join("media");
    let run_dir = root.join(format!("run-{}", surface.name()));
    let kind = match surface {
        Surface::Cli | Surface::Rpc => Some(EngineKind::Server),
        Surface::Mcp => Some(EngineKind::Stdio),
        Surface::McpProcess => None,
    };
    let mut engine = kind.map(|kind| ParityEngine::start(kind, root, &run_dir));
    let registry = std::sync::Arc::new(cutweave_engine::build_registry().unwrap());
    let mut driver = match (surface, &engine) {
        (Surface::Cli, _) => Driver::Cli(Cli {
            run_dir: &run_dir,
            registry: &registry,
        }),
        (Surface::Mcp, Some(e)) => {
            Driver::Mcp(Box::new(Mcp::connect(&e.engine, Revision::Stateful)))
        }
        (Surface::McpProcess, _) => Driver::Mcp(Box::new(Mcp::spawn(root, Revision::Stateless))),
        (Surface::Rpc, Some(e)) => {
            Driver::Rpc(Box::new(Rpc::connect(e.port(), &e.token("rpc"), &registry)))
        }
        (_, None) => unreachable!("{surface:?} runs against an engine of the harness"),
    };
    let at = |i: usize| format!("{} on {}: steps[{i}]", script.name, surface.name());
    let mut outputs: Vec<Option<Value>> = Vec::new();
    let mut outcomes = Vec::new();
    for (i, step) in script.steps.iter().enumerate() {
        let outcome = match &step.action {
            Action::Call(call) => {
                let args = resolve(&call.args, Scope::Script, &outputs, &media)
                    .unwrap_or_else(|e| panic!("{}: {e}", at(i)));
                driver.call(&Call {
                    cmd: call.cmd.clone(),
                    args,
                })
            }
            Action::Batch(calls) => {
                let calls: Vec<Call> = calls
                    .iter()
                    .map(|c| Call {
                        cmd: c.cmd.clone(),
                        args: resolve(&c.args, Scope::Batch, &outputs, &media)
                            .unwrap_or_else(|e| panic!("{}: {e}", at(i))),
                    })
                    .collect();
                driver.batch(&calls)
            }
        };
        if let Err(e) = step.check(&outcome) {
            panic!("{}: {e}", at(i));
        }
        outputs.push(match (&step.action, &outcome) {
            (Action::Call(_), Ok(out)) => Some(out[0].clone()),
            (Action::Batch(_), Ok(out)) => Some(Value::Array(out.clone())),
            (_, Err(_)) => None,
        });
        outcomes.push(outcome);
    }
    let patches = engine.as_mut().map(ParityEngine::patches);
    for patch in patches.iter().flatten() {
        assert_eq!(
            patch["actor"],
            surface.actor(),
            "{} on {}: {patch}",
            script.name,
            surface.name()
        );
    }
    // Save through the surface itself; the document lands in `<root>/<surface>/Parity`.
    let folder = root.join(surface.name()).join("Parity");
    std::fs::create_dir_all(folder.parent().unwrap()).unwrap();
    let saved = driver
        .call(&Call {
            cmd: "project.save_as".into(),
            args: json!({ "path": folder.to_string_lossy() }),
        })
        .unwrap_or_else(|e| {
            panic!(
                "{} on {}: save failed: {}",
                script.name,
                surface.name(),
                e.raw
            )
        });
    let path = saved[0]["path"]
        .as_str()
        .expect("project.save_as returns the path");
    let document = std::fs::read(path).expect("the saved document");
    driver.close();
    if let Some(engine) = engine {
        engine.shutdown();
    }
    Run {
        surface,
        outcomes,
        patches,
        sha256: sha256(&document),
        document,
    }
}

/// The first line where two documents differ, for the failure message.
fn first_difference(a: &[u8], b: &[u8]) -> String {
    let (a, b) = (String::from_utf8_lossy(a), String::from_utf8_lossy(b));
    for (n, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            return format!("line {}:\n  - {x}\n  + {y}", n + 1);
        }
    }
    format!(
        "one is a prefix of the other ({} vs {} lines)",
        a.lines().count(),
        b.lines().count()
    )
}

/// What must be equal in a `doc.patched` payload: the ops and the revisions they lead to.
///
/// Left out, because they describe the route rather than the change: the actor (derived from
/// each surface's credential; asserted per surface in [`run`]), the client's `request_id`, and
/// `cause` / `tx_id`. A catalog command has no MCP tool of its own, so an agent runs it with
/// `cutweave_execute`, which is always a `batch` transaction (`cause: "commit"`), while
/// `cutweave exec` and JSON-RPC run one call as an implicit command (`cause: "command"`); the
/// transaction ids follow from that.
fn comparable_patch(patch: &Value) -> Value {
    json!({
        "rev": patch["rev"],
        "prev_rev": patch["prev_rev"],
        "committed_rev": patch["committed_rev"],
        "ops": patch["ops"],
    })
}

/// A step output without what names the caller rather than the change. History results
/// (`edit.*`) carry the actor (each surface's own credential, checked here) and the label of
/// the transaction, which each client names itself (`cutweave exec`, `cutweave_execute: ...`,
/// or the command name for a lone call).
fn comparable_output(cmd: &str, output: &Value, surface: Surface) -> Value {
    fn scrub(value: &mut Value, actor: &str) {
        match value {
            Value::Object(map) => {
                if map.get("actor").and_then(Value::as_str) == Some(actor) {
                    map.insert("actor".into(), json!("<surface actor>"));
                }
                if map.get("label").is_some_and(Value::is_string) {
                    map.insert("label".into(), json!("<client label>"));
                }
                map.values_mut().for_each(|v| scrub(v, actor));
            }
            Value::Array(items) => items.iter_mut().for_each(|v| scrub(v, actor)),
            _ => {}
        }
    }
    let mut output = output.clone();
    if cmd.starts_with("edit.") {
        scrub(&mut output, surface.actor());
    }
    output
}

/// Compares every run with the first one; returns the differences.
fn compare(script: &Script, runs: &[Run]) -> Vec<String> {
    let mut problems = Vec::new();
    let reference = &runs[0];
    let r = reference.surface.name();
    for other in &runs[1..] {
        let o = other.surface.name();
        if other.sha256 != reference.sha256 {
            problems.push(format!(
                "document: {r} sha256 {} != {o} sha256 {}; first difference at {}",
                reference.sha256,
                other.sha256,
                first_difference(&reference.document, &other.document)
            ));
        }
        for (i, (a, b)) in reference.outcomes.iter().zip(&other.outcomes).enumerate() {
            let calls = script.steps[i].calls();
            let comparable = |outputs: &[Value], surface: Surface| -> Vec<Value> {
                outputs
                    .iter()
                    .zip(calls)
                    .map(|(v, c)| comparable_output(&c.cmd, v, surface))
                    .collect()
            };
            match (a, b) {
                (Ok(x), Ok(y))
                    if comparable(x.as_slice(), reference.surface)
                        != comparable(y.as_slice(), other.surface) =>
                {
                    problems.push(format!(
                        "steps[{i}] output: {r} {} != {o} {}",
                        json!(x),
                        json!(y)
                    ));
                }
                // `failed_step` belongs to batches. A single call has none, except over MCP when
                // the command has no tool of its own: the agent then sends a one-step
                // `cutweave_execute`, which names step 0.
                (Err(x), Err(y))
                    if x.code != y.code
                        || (matches!(script.steps[i].action, Action::Batch(_))
                            && x.failed_step != y.failed_step) =>
                {
                    problems.push(format!(
                        "steps[{i}] error: {r} {} (failed_step {:?}) != {o} {} (failed_step {:?}): {} / {}",
                        x.code, x.failed_step, y.code, y.failed_step, x.raw, y.raw
                    ));
                }
                (Ok(_), Err(e)) | (Err(e), Ok(_)) => {
                    problems.push(format!("steps[{i}]: one of {r}, {o} failed: {}", e.raw));
                }
                _ => {}
            }
        }
        let (Some(pa), Some(pb)) = (&reference.patches, &other.patches) else {
            continue;
        };
        let pa: Vec<Value> = pa.iter().map(comparable_patch).collect();
        let pb: Vec<Value> = pb.iter().map(comparable_patch).collect();
        if pa != pb {
            let at = pa
                .iter()
                .zip(&pb)
                .position(|(x, y)| x != y)
                .unwrap_or(pa.len().min(pb.len()));
            problems.push(format!(
                "doc.patched: {r} has {} events, {o} {}; first difference at #{at}: {} vs {}",
                pa.len(),
                pb.len(),
                pa.get(at).map_or("-".into(), Value::to_string),
                pb.get(at).map_or("-".into(), Value::to_string),
            ));
        }
    }
    problems
        .into_iter()
        .map(|p| format!("{}: {p}", script.name))
        .collect()
}

/// Runs `script` on every surface and checks parity.
fn check_script(script: &Script) {
    let root = tempfile::tempdir().unwrap();
    // Canonical, so the paths the engine stores are the ones the harness builds.
    let root_path = std::fs::canonicalize(root.path()).unwrap();
    let root_path = strip_verbatim(&root_path);
    create_media(script, &root_path);
    let runs: Vec<Run> = Surface::ALL
        .iter()
        .map(|&s| run(script, s, &root_path))
        .collect();
    let problems = compare(script, &runs);
    assert!(
        problems.is_empty(),
        "parity broken:\n{}",
        problems.join("\n")
    );
    // Three credentials, one document: nothing in it records who made the edits.
    let text = String::from_utf8_lossy(&runs[0].document);
    assert!(
        !text.contains("agent:"),
        "{}: the document names an actor",
        script.name
    );
    let errors = runs[0].outcomes.iter().filter(|o| o.is_err()).count();
    println!(
        "{}: {} steps ({} expected errors), {} doc.patched, sha256 {} on {}",
        script.name,
        script.steps.len(),
        errors,
        runs[0].patches.as_ref().map_or(0, Vec::len),
        runs[0].sha256,
        Surface::ALL.map(Surface::name).join(" = ")
    );
}

/// `\\?\C:\...` (what `canonicalize` returns on Windows) as `C:\...`.
fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Acceptance 1: for every script the three surfaces save byte-identical documents (equal
/// sha256), emit the same `doc.patched` ops, return the same outputs and fail with the same
/// error codes.
#[test]
fn every_script_is_byte_identical_on_cli_mcp_and_rpc() {
    let scripts = scripts();
    assert!(
        scripts.len() >= 4,
        "at least 4 scripts, found {}",
        scripts.len()
    );
    // Every script runs (and reports) even when an earlier one fails.
    let failed: Vec<&str> = scripts
        .iter()
        .filter(|s| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check_script(s))).is_err()
        })
        .map(|s| s.name.as_str())
        .collect();
    assert!(
        failed.is_empty(),
        "parity broken in {failed:?} (details above)"
    );
}

/// Arch §14 "Parity guarantee": every registered non-`view` entry is reachable over MCP (core
/// tool or the generic tools) with the registry's input schema (verbatim from
/// `cutweave_describe_command`, validation-identical as a listed tool), is a JSON-RPC method,
/// and is described identically by `cutweave describe` and `schema/{commands,queries}.json`.
/// Entries exempt in `schema/parity-exempt.toml` must really be unreachable over MCP.
#[test]
fn every_registry_entry_is_reachable_with_the_registry_schema() {
    let registry = std::sync::Arc::new(cutweave_engine::build_registry().unwrap());
    let exemptions = enumeration::exemptions();
    let problems = enumeration::check_registry(&registry, &exemptions);
    assert!(
        problems.is_empty(),
        "registry-enumeration parity broken:\n{}",
        problems.join("\n")
    );

    let root = tempfile::tempdir().unwrap();
    let server = ParityEngine::start(EngineKind::Server, root.path(), &root.path().join("run"));
    let stdio = ParityEngine::start(EngineKind::Stdio, root.path(), &root.path().join("none"));
    let mcp = Mcp::connect(&stdio.engine, Revision::Stateful);
    let mut rpc = Rpc::connect(server.port(), &server.token("rpc"), &registry);
    let published = enumeration::published_schemas();
    let listed = cli::cutweave(&["commands".into(), "--json".into()]);
    assert_eq!(listed.status.code(), Some(0), "{listed:?}");
    let mut cli_names: Vec<String> = cli::stdout_json(&listed)
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .collect();
    cli_names.sort();
    let mut names: Vec<String> = registry.list().map(|e| e.meta.name.to_owned()).collect();
    names.sort();
    assert_eq!(cli_names, names, "`cutweave commands` lists the registry");

    let probe = json!({ "__parity_probe": true });
    let mut problems = Vec::new();
    let mut checked = 0;
    for entry in registry.list().filter(|e| enumeration::in_scope(&e.meta)) {
        let meta = &entry.meta;
        let name = meta.name;
        let schema = &meta.input_schema;
        if published.get(name) != Some(schema) {
            problems.push(format!("`{name}`: schema/*.json differs from the registry"));
        }
        // MCP: describe (verbatim schema), then a call through the agent's route, which the
        // engine must reach (an unknown field is `invalid`, an unreachable name `not_found`).
        let exempt = exemptions.iter().any(|e| e.name == name);
        let described = mcp.raw("cutweave_describe_command", json!({ "name": name }));
        match (&described, exempt) {
            (Ok(d), false) if d["input_schema"] != *schema => problems.push(format!(
                "`{name}`: cutweave_describe_command's input_schema differs from the registry"
            )),
            (Err(e), false) => {
                problems.push(format!("`{name}`: not describable over MCP: {}", e.raw));
            }
            (Ok(_), true) => problems.push(format!("`{name}` is exempt but describable over MCP")),
            (Err(e), true) if e.code != "not_found" => {
                problems.push(format!("`{name}` (exempt): unexpected {}", e.raw));
            }
            _ => {}
        }
        let call = Call {
            cmd: name.to_owned(),
            args: probe.clone(),
        };
        let expected = if exempt { "not_found" } else { "invalid" };
        match mcp.call(&call) {
            Err(e) if e.code == expected => {}
            other => problems.push(format!(
                "`{name}`: MCP call with an unknown field: expected `{expected}`, got {:?}",
                other.map_err(|e| e.raw)
            )),
        }
        // RPC: a method under the registry name.
        match rpc.call(&call) {
            Err(e) if e.code == "invalid" => {}
            other => problems.push(format!(
                "`{name}`: RPC call with an unknown field: expected `invalid`, got {:?}",
                other.map_err(|e| e.raw)
            )),
        }
        // CLI: `cutweave describe`.
        let out = cli::cutweave(&["describe".into(), name.into()]);
        if out.status.code() == Some(0) {
            if cli::stdout_json(&out)["input_schema"] != *schema {
                problems.push(format!(
                    "`{name}`: `cutweave describe` input_schema differs"
                ));
            }
        } else {
            problems.push(format!("`{name}`: `cutweave describe` failed: {out:?}"));
        }
        checked += 1;
    }
    mcp.close();
    rpc.close();
    server.shutdown();
    stdio.shutdown();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    println!(
        "{checked} registry entries reachable on MCP, RPC and the CLI with identical schemas ({} exempt from MCP)",
        exemptions.len()
    );
}
