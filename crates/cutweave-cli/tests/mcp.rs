// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]
//! Process-level tests of `cutweave mcp` (WP-1J.03): an rmcp client drives the binary over its
//! stdio pipes, on both MCP spec revisions.
//!
//! - `--headless`: a scripted session (create, import stub media, wait, insert, split, undo,
//!   read) on the standard profile; `--profile minimal`; the path policy;
//! - `--attach`: a proxy to a `cutweave engine --headless` child: same document as `/rpc`,
//!   session-end rollback, a killed engine;
//! - stdout carries protocol frames only; errors before the session are JSON on stderr.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use cutweave_api::mcp::switch::PROXY_TOOLS;
use cutweave_api::mcp::{ListSize, STANDARD_BUDGET};
use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt as _, RoleClient, ServiceExt as _,
    model::{CallToolRequestParams, CallToolResult, ClientConfig, ContentBlock, ProtocolVersion},
    service::{RunningService, ServiceError},
};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;
use tokio::task::JoinHandle;

const BIN: &str = env!("CARGO_BIN_EXE_cutweave");

/// Flicks per second.
const SEC: i64 = 705_600_000;

/// The two spec revisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Revision {
    /// 2025-11-25: `initialize`.
    Stateful,
    /// 2026-07-28: `server/discover`, per-request `_meta`.
    Stateless,
}

impl Revision {
    const ALL: [Revision; 2] = [Revision::Stateful, Revision::Stateless];

    fn version(self) -> ProtocolVersion {
        match self {
            Revision::Stateful => ProtocolVersion::V_2025_11_25,
            Revision::Stateless => ProtocolVersion::V_2026_07_28,
        }
    }
}

/// A client asking for one protocol version.
#[derive(Clone)]
struct Versioned(ProtocolVersion);

impl ClientHandler for Versioned {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default().with_protocol_version(self.0.clone())
    }
}

/// `cutweave mcp <args>` as a child with an rmcp client on its stdio.
struct Agent {
    service: RunningService<RoleClient, Versioned>,
    child: tokio::process::Child,
    stderr: JoinHandle<String>,
}

impl Agent {
    async fn start(args: &[&str], revision: Revision) -> Agent {
        let mut child = tokio::process::Command::new(BIN)
            .arg("mcp")
            .args(args)
            .env_remove("CUTWEAVE_DETERMINISTIC_IDS")
            .env_remove("CUTWEAVE_DEV_STUB_MEDIA")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn cutweave mcp");
        let stdout = child.stdout.take().expect("stdout");
        let stdin = child.stdin.take().expect("stdin");
        let mut err = child.stderr.take().expect("stderr");
        let stderr = tokio::spawn(async move {
            let mut text = String::new();
            let _ = err.read_to_string(&mut text).await;
            text
        });
        let handler = Versioned(revision.version());
        let service = match revision {
            Revision::Stateful => handler.serve((stdout, stdin)).await.expect("initialize"),
            Revision::Stateless => handler
                .serve_with_lifecycle(
                    (stdout, stdin),
                    ClientLifecycleMode::Discover {
                        preferred_versions: vec![revision.version()],
                    },
                )
                .await
                .expect("server/discover"),
        };
        Agent {
            service,
            child,
            stderr,
        }
    }

    fn negotiated(&self) -> ProtocolVersion {
        self.service
            .peer_info()
            .expect("peer info")
            .protocol_version
            .clone()
    }

    async fn tools(&self) -> Vec<rmcp::model::Tool> {
        self.service.list_tools(None).await.unwrap().tools
    }

    async fn try_call(&self, tool: &str, args: Value) -> Result<CallToolResult, ServiceError> {
        let Value::Object(args) = args else {
            panic!("args must be an object")
        };
        self.service
            .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args))
            .await
    }

    /// Calls `tool`, panicking on a tool error; returns `structuredContent`.
    async fn ok(&self, tool: &str, args: Value) -> Value {
        let result = self.try_call(tool, args).await.expect("tools/call");
        assert_ne!(result.is_error, Some(true), "{tool} failed: {result:?}");
        result.structured_content.expect("structuredContent")
    }

    /// Calls `tool`, panicking on success; returns the structured error.
    async fn err(&self, tool: &str, args: Value) -> Value {
        let result = self.try_call(tool, args).await.expect("tools/call");
        assert_eq!(result.is_error, Some(true), "{tool} succeeded: {result:?}");
        assert!(matches!(result.content.last(), Some(ContentBlock::Text(_))));
        result.structured_content.expect("structuredContent")
    }

    /// Closes the client (and so the child's stdin) and returns the child's exit code and
    /// stderr.
    async fn close(self) -> (Option<i32>, String) {
        let Agent {
            service,
            mut child,
            stderr,
        } = self;
        let _ = service.cancel().await;
        let status = tokio::time::timeout(Duration::from_secs(60), child.wait())
            .await
            .expect("cutweave mcp exits when its stdin closes")
            .expect("wait");
        (status.code(), stderr.await.unwrap_or_default())
    }
}

/// The tools of the engine in `tools`: an attach proxy's list also has its own (WP-1J.04).
fn engine_tools(tools: &[rmcp::model::Tool]) -> usize {
    tools
        .iter()
        .filter(|t| !PROXY_TOOLS.contains(&&*t.name))
        .count()
}

/// A stub media file (`--dev-stub-media` probes by extension).
fn stub_clip(dir: &Path) -> String {
    let media = dir.join("media");
    std::fs::create_dir_all(&media).unwrap();
    let clip = media.join("clip.mp4");
    std::fs::write(&clip, vec![7u8; 4096]).unwrap();
    clip.to_string_lossy().into_owned()
}

/// The scripted agent: create a project, import, wait for analysis, insert, split, undo, read.
/// Returns the project name it set.
async fn scripted_edit(agent: &Agent, clip: &str) {
    let created = agent
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "project.new", "args": { "name": "Stdio cut" } }] }),
        )
        .await;
    let project = created["steps"][0]["output"].clone();
    let sequence = project["sequence"].clone();
    let info = agent.ok("project_info", json!({})).await;
    assert_eq!(info["name"], "Stdio cut");
    let imported = agent.ok("media_import", json!({ "paths": [clip] })).await;
    let media = imported["media_ids"][0].clone();
    assert!(
        media.as_str().is_some_and(|m| m.starts_with("med_")),
        "{imported}"
    );
    for job in imported["job_ids"].as_array().cloned().unwrap_or_default() {
        let done = agent
            .ok("job_wait", json!({ "job_id": job, "timeout_ms": 10_000 }))
            .await;
        assert_eq!(done["state"], "succeeded", "{done}");
    }
    let inserted = agent
        .ok(
            "timeline_insert",
            json!({
                "sequence": sequence, "track": project["video_track"], "media": media, "at": 0
            }),
        )
        .await;
    assert_eq!(inserted["duration"], 10 * SEC, "{inserted}");
    let clip_id = inserted["clips"][0].clone();
    agent
        .ok("clip_split", json!({ "item": clip_id, "at": "4s" }))
        .await;
    let split = agent
        .ok("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
        .await;
    assert_eq!(split["item_count"], 4, "video + linked audio, both split");
    agent.ok("edit_undo", json!({})).await;
    let read = agent
        .ok("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
        .await;
    assert_eq!(read["item_count"], 2, "{read}");
}

// ----- headless -----------------------------------------------------------------------------

/// WP-1J.02: without `--dev-stub-media`, `cutweave mcp --headless` runs the full engine: 20
/// tools within the standard budget (R36; `render_frame` joins the registry's 15 core tools and the 4 generic
/// ones), and `render_frame` answers with one image block. (No FFmpeg is needed: an empty
/// sequence renders its background.)
#[tokio::test]
async fn headless_runs_the_full_engine_and_renders_a_frame() {
    for revision in Revision::ALL {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap().to_owned();
        let agent =
            Agent::start(&["--headless", "--allow-root", &root, "--no-log"], revision).await;
        let tools = agent.tools().await;
        assert_eq!(tools.len(), 22, "{revision:?}");
        assert!(tools.iter().any(|t| t.name == "render_frame"));
        let size = ListSize::of(&tools);
        assert!(STANDARD_BUDGET.fits(size), "{size:?}");
        agent
            .ok(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": "project.new", "args": { "name": "Empty" } }] }),
            )
            .await;
        let result = agent
            .try_call("render_frame", json!({ "t": "0s", "max_size": 128 }))
            .await
            .expect("tools/call");
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let images = result
            .content
            .iter()
            .filter(|c| matches!(c, ContentBlock::Image(_)))
            .count();
        assert_eq!(images, 1, "{revision:?}");
        let meta = result.structured_content.unwrap();
        assert_eq!(meta["items_visible"], json!([]));
        let (code, stderr) = agent.close().await;
        assert_eq!(code, Some(0), "{stderr}");
    }
}

/// WP-1D.08: over MCP an agent reads `media.cache_status` through `cutweave_execute`, and the
/// three commands that change the caches (UI credential only) come back `forbidden`. The cache
/// commands are in the catalog, not in the core tool list: the list is the same size.
#[tokio::test]
async fn headless_agents_read_the_cache_status_but_cannot_change_the_caches() {
    for revision in Revision::ALL {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap().to_owned();
        let agent =
            Agent::start(&["--headless", "--allow-root", &root, "--no-log"], revision).await;
        assert_eq!(agent.tools().await.len(), 22, "{revision:?}");
        let status = agent
            .ok(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": "media.cache_status", "args": {} }] }),
            )
            .await;
        let text = status.to_string();
        assert!(text.contains("decode_hw"), "{revision:?}: {text}");
        for step in [
            json!({ "cmd": "media.set_cache_config", "args": { "frame_cache_bytes": 0 } }),
            json!({ "cmd": "media.clear_cache", "args": { "kinds": ["thumbnails"] } }),
            json!({ "cmd": "media.set_decode_policy", "args": { "hw": "off" } }),
        ] {
            let e = agent
                .err("cutweave_execute", json!({ "steps": [step.clone()] }))
                .await;
            assert!(
                e.to_string().contains("forbidden"),
                "{revision:?} {step}: {e}"
            );
        }
        let (code, stderr) = agent.close().await;
        assert_eq!(code, Some(0), "{stderr}");
    }
}

/// Acceptance 1: `cutweave mcp --headless` lists the standard profile (20 tools within the
/// gate) and runs the scripted session, on both revisions; the path policy confines it.
#[tokio::test]
async fn headless_runs_a_scripted_session_on_both_revisions() {
    for revision in Revision::ALL {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap().to_owned();
        let clip = stub_clip(dir.path());
        let agent = Agent::start(
            &[
                "--headless",
                "--dev-stub-media",
                "--allow-root",
                &root,
                "--no-log",
            ],
            revision,
        )
        .await;
        assert_eq!(agent.negotiated(), revision.version(), "{revision:?}");
        let tools = agent.tools().await;
        // The registry's core tools plus the 4 generic tools, the same catalog as `/mcp`: 19
        // with `export_start` (WP-1E.03), 20 with `keyframe_set` (WP-2C.03), 21 with `audio_set`
        // (WP-2E.01), 22 once `render_frame` (WP-1J.02) joins it.
        assert_eq!(tools.len(), 21, "{revision:?}");
        let size = ListSize::of(&tools);
        assert!(STANDARD_BUDGET.fits(size), "{size:?}");
        println!(
            "headless {revision:?}: negotiated {}, {} tools, ~{} tokens",
            agent.negotiated(),
            size.tools,
            size.tokens()
        );
        scripted_edit(&agent, &clip).await;
        // Outside --allow-root and the project folder: `invalid` at the path, with a hint.
        let outside = tempfile::tempdir().unwrap();
        let elsewhere = stub_clip(outside.path());
        let e = agent
            .err("media_import", json!({ "paths": [elsewhere] }))
            .await;
        assert_eq!(e["error"]["code"], "invalid", "{e}");
        assert_eq!(e["error"]["path"], "/paths/0", "{e}");
        let (code, stderr) = agent.close().await;
        assert_eq!(code, Some(0), "{stderr}");
        assert!(stderr.contains("dev stub media"), "{stderr}");
    }
}

/// Acceptance 3: `--profile minimal` lists at most 12 tools within 3k tokens.
#[tokio::test]
async fn headless_minimal_profile_lists_at_most_12_tools() {
    let agent = Agent::start(
        &["--headless", "--profile", "minimal", "--no-log"],
        Revision::Stateful,
    )
    .await;
    let tools = agent.tools().await;
    assert!(tools.len() <= 12, "{}", tools.len());
    let size = ListSize::of(&tools);
    assert!(size.tokens() <= 3_000, "{size:?}");
    assert!(tools.iter().any(|t| t.name == "timeline_get"));
    assert!(!tools.iter().any(|t| t.name == "clip_split"));
    let (code, _) = agent.close().await;
    assert_eq!(code, Some(0));
}

/// Acceptance 4: stdout carries protocol frames only. A raw 100-call session (successes, tool
/// errors, protocol errors) with the log file on and the dev-stub warning: every stdout line is
/// one JSON-RPC 2.0 message, logs went to the log file and diagnostics to stderr.
#[test]
fn stdout_carries_protocol_frames_only() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let root = dir.path().to_str().unwrap().to_owned();
    let clip = stub_clip(dir.path());
    let mut child = Command::new(BIN)
        .args([
            "mcp",
            "--headless",
            "--dev-stub-media",
            "--allow-root",
            &root,
        ])
        .arg("--log-dir")
        .arg(&logs)
        .env_remove("CUTWEAVE_DETERMINISTIC_IDS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut frames: Vec<Value> = Vec::new();
    let mut exchange = |stdin: &mut std::process::ChildStdin, request: &Value| -> Value {
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            assert_ne!(stdout.read_line(&mut line).unwrap(), 0, "stdout closed");
            let frame: Value = serde_json::from_str(line.trim_end())
                .unwrap_or_else(|e| panic!("not a JSON-RPC line ({e}): {line:?}"));
            frames.push(frame.clone());
            if frame.get("id") == request.get("id") {
                return frame;
            }
        }
    };
    let init = json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": { "name": "raw", "version": "0" }
        }
    });
    let ready = exchange(&mut stdin, &init);
    assert_eq!(ready["result"]["protocolVersion"], "2025-11-25");
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
    )
    .unwrap();
    let call = |id: u64, name: &str, args: Value| {
        json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": args }
        })
    };
    let created = exchange(
        &mut stdin,
        &call(
            1,
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "project.new", "args": { "name": "raw" } }] }),
        ),
    );
    let sequence = created["result"]["structuredContent"]["steps"][0]["output"]["sequence"].clone();
    exchange(
        &mut stdin,
        &call(2, "media_import", json!({ "paths": [clip] })),
    );
    for id in 3..=100u64 {
        let request = match id % 6 {
            0 => call(id, "project_info", json!({})),
            1 => call(
                id,
                "timeline_get",
                json!({ "sequence": sequence, "depth": 1 }),
            ),
            2 => call(id, "media_list", json!({})),
            3 => call(id, "clip_split", json!({ "item": "itm_nope", "at": 0 })),
            4 => call(id, "no_such_tool", json!({})),
            _ => json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list" }),
        };
        let response = exchange(&mut stdin, &request);
        assert!(
            response.get("result").is_some() || response.get("error").is_some(),
            "{response}"
        );
    }
    drop(stdin);
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut stdout, &mut rest).unwrap();
    for line in rest.lines().filter(|l| !l.trim().is_empty()) {
        frames.push(
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("not a JSON-RPC line ({e}): {line:?}")),
        );
    }
    let status = child.wait().unwrap();
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut stderr).unwrap();
    assert_eq!(status.code(), Some(0), "{stderr}");
    // 1 initialize + 100 calls, nothing else.
    assert_eq!(frames.len(), 101, "{frames:#?}");
    for frame in &frames {
        assert_eq!(frame["jsonrpc"], "2.0", "{frame}");
        assert!(frame.get("id").is_some(), "{frame}");
        assert!(
            frame.get("result").is_some() != frame.get("error").is_some(),
            "{frame}"
        );
    }
    // Diagnostics on stderr, logs in the log file.
    assert!(stderr.contains("dev stub media"), "{stderr}");
    let log: String = std::fs::read_dir(&logs)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("engine-"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert!(log.contains("engine ready"), "{log}");
}

// ----- attach -------------------------------------------------------------------------------

/// A `cutweave engine --headless` child (stub media, `--allow-root`), killed on drop.
struct EngineProcess {
    child: std::process::Child,
    pid: u32,
}

impl EngineProcess {
    fn start(run_dir: &Path, root: &Path) -> EngineProcess {
        let mut child = Command::new(BIN)
            .args([
                "engine",
                "--headless",
                "--port",
                "0",
                "--no-log",
                "--dev-stub-media",
            ])
            .arg("--run-dir")
            .arg(run_dir)
            .arg("--allow-root")
            .arg(root)
            .env_remove("CUTWEAVE_DETERMINISTIC_IDS")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn engine");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let ready: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert_eq!(ready["event"], "ready", "{line}");
        let pid = u32::try_from(ready["pid"].as_u64().unwrap()).unwrap();
        EngineProcess { child, pid }
    }

    fn kill(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Drop for EngineProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `cutweave <args>` against the attached engine (the CLI's `/rpc` client, actor `agent:cli`).
fn cutweave(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawn cutweave")
}

fn stdout_json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn open_transactions(run_dir: &str) -> usize {
    let out = cutweave(&["exec", r#"{"cmd":"edit.locks"}"#, "--run-dir", run_dir]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    stdout_json(&out)["output"]["transactions"]
        .as_array()
        .unwrap()
        .len()
}

/// Acceptance 2 (and 3 for attach): the proxy edits the document a running engine holds (seen
/// through `/rpc` by `cutweave query`), closing its stdin rolls back the session's open
/// transaction, and a killed engine makes the proxy answer with an error and exit 3 within
/// 2 s.
#[tokio::test]
async fn attach_edits_the_running_engines_document() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let run = run_dir.to_str().unwrap().to_owned();
    let clip = stub_clip(dir.path());
    let engine = EngineProcess::start(&run_dir, dir.path());
    let run_file = run_dir.join(format!("{}.json", engine.pid));
    let run_file = run_file.to_str().unwrap().to_owned();

    // 2025-11-25 through --run-file: one upstream session.
    let agent = Agent::start(&["--attach", "--run-file", &run_file], Revision::Stateful).await;
    assert_eq!(agent.negotiated(), Revision::Stateful.version());
    let tools = agent.tools().await;
    // The dev-stub engine's 21 (no renderer, so no `render_frame`; the full engine lists 22,
    // within the standard budget) plus the proxy's own `engines_list` and `engine_attach`.
    assert_eq!(engine_tools(&tools), 21);
    assert_eq!(tools.len(), 21 + PROXY_TOOLS.len());
    scripted_edit(&agent, &clip).await;
    // The same document through the engine's `/rpc`.
    let out = cutweave(&["query", "project.info", "--run-dir", &run]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout_json(&out)["output"]["name"], "Stdio cut");
    // A transaction the session holds dies with it.
    agent
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "edit.begin_tx", "args": { "kind": "proposal", "label": "held" } }] }),
        )
        .await;
    assert_eq!(open_transactions(&run), 1);
    let (code, stderr) = agent.close().await;
    assert_eq!(code, Some(0), "{stderr}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while open_transactions(&run) > 0 {
        assert!(
            Instant::now() < deadline,
            "the session's transaction outlived its stdin"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // 2026-07-28 through run-file discovery, minimal profile.
    let agent = Agent::start(
        &["--attach", "--run-dir", &run, "--profile", "minimal"],
        Revision::Stateless,
    )
    .await;
    assert_eq!(agent.negotiated(), Revision::Stateless.version());
    let tools = agent.tools().await;
    // The proxy adds its own two tools (WP-1J.04) to the engine's list; the budget is the engine's.
    assert!(engine_tools(&tools) <= 12, "{}", tools.len());
    let info = agent.ok("project_info", json!({})).await;
    assert_eq!(info["name"], "Stdio cut");

    // The engine goes away: the next call fails and the proxy exits 3 within 2 s.
    engine.kill();
    let gone = Instant::now();
    let failed = agent.try_call("project_info", json!({})).await;
    match &failed {
        Err(ServiceError::McpError(e)) => {
            assert_eq!(e.data.as_ref().unwrap()["code"], "connection_lost", "{e:?}");
        }
        Err(_) => {}
        Ok(result) => panic!("a call to a killed engine succeeded: {result:?}"),
    }
    let Agent {
        service,
        mut child,
        stderr,
    } = agent;
    let status = tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .expect("the proxy exits within 2 s of the engine")
        .unwrap();
    println!("proxy exited {:?} after the engine", gone.elapsed());
    assert_eq!(status.code(), Some(3));
    let stderr = stderr.await.unwrap();
    let error: Value = serde_json::from_str(stderr.lines().last().unwrap_or_default())
        .unwrap_or_else(|e| panic!("stderr is not JSON ({e}): {stderr}"));
    assert_eq!(error["code"], "connection_lost", "{error}");
    drop(service);
}

/// Errors before the session starts: one JSON object on stderr, nothing on stdout, the CLI exit
/// codes.
#[test]
fn errors_before_the_session_are_json_on_stderr() {
    let empty = tempfile::tempdir().unwrap();
    let empty_dir = empty.path().to_str().unwrap();
    let missing = empty.path().join("4242.json");
    let missing = missing.to_str().unwrap();
    for (args, exit, code) in [
        (
            vec!["mcp", "--attach", "--run-dir", empty_dir],
            3,
            "no_engine",
        ),
        (vec!["mcp", "--run-dir", empty_dir], 3, "no_engine"),
        (
            vec!["mcp", "--attach", "--run-file", missing],
            3,
            "no_engine",
        ),
        (
            vec!["mcp", "--attach", "--allow-root", empty_dir],
            2,
            "usage",
        ),
        (
            vec!["mcp", "--attach", "--run-file", missing, "--pid", "1"],
            2,
            "usage",
        ),
        (vec!["mcp", "--no-log", "--run-dir", empty_dir], 2, "usage"),
        (vec!["mcp", "--headless", "--profile", "huge"], 2, "usage"),
    ] {
        let out = cutweave(&args);
        assert_eq!(out.status.code(), Some(exit), "{args:?}: {out:?}");
        assert_eq!(out.stdout.len(), 0, "{args:?}: stdout is the protocol's");
        let error: Value = serde_json::from_slice(&out.stderr).unwrap_or_else(|e| {
            panic!(
                "{args:?}: stderr is not JSON ({e}): {}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        assert_eq!(error["code"], code, "{args:?}");
    }
}

/// A headless server whose stdin closes before any message exits 0 without output.
#[test]
fn headless_with_no_input_exits_quietly() {
    let out = Command::new(BIN)
        .args(["mcp", "--headless", "--no-log"])
        .stdin(Stdio::null())
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(out.stdout.len(), 0, "{out:?}");
}
