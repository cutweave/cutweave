// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]
//! Process-level tests of choosing among several engines (WP-1J.04): `cutweave engines`,
//! `--project` and the default of the app's focused engine for `exec` and `query`, the attach
//! proxy's `engines_list` and `engine_attach`, and the focus report (`project.report_focus`).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use cutweave_api::mcp::switch::PROXY_TOOLS;
use cutweave_api::mcp::{ListSize, P1_GATE};
use futures_util::{SinkExt, StreamExt};
use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt as _, RoleClient, ServiceExt as _,
    model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
    service::RunningService,
};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;

const BIN: &str = env!("CARGO_BIN_EXE_cutweave");

/// The UI credential the GUI engines are given.
const UI_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

/// A `cutweave engine` child, killed on drop.
struct EngineProc {
    child: Child,
    pid: u32,
    port: u16,
    /// A GUI engine stops when its stdin closes: held open here.
    stdin: Option<std::process::ChildStdin>,
}

impl EngineProc {
    fn start(run_dir: &Path, root: &Path, project: Option<&Path>, gui: bool) -> EngineProc {
        let mut cmd = Command::new(BIN);
        cmd.args(["engine", "--port", "0", "--no-log", "--dev-stub-media"]);
        if gui {
            cmd.args(["--token-stdin", "--allow-origin", "app://cutweave"]);
        } else {
            cmd.arg("--headless");
        }
        if let Some(project) = project {
            cmd.arg("--project").arg(project);
        }
        cmd.arg("--run-dir")
            .arg(run_dir)
            .arg("--allow-root")
            .arg(root)
            .env_remove("CUTWEAVE_DETERMINISTIC_IDS")
            .stdin(if gui { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().expect("spawn engine");
        let mut stdin = child.stdin.take();
        if let Some(stdin) = stdin.as_mut() {
            writeln!(stdin, "{UI_TOKEN}").expect("handshake");
        }
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let ready: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert_eq!(ready["event"], "ready", "{line}");
        EngineProc {
            child,
            pid: u32::try_from(ready["pid"].as_u64().unwrap()).unwrap(),
            port: u16::try_from(ready["port"].as_u64().unwrap()).unwrap(),
            stdin,
        }
    }

    /// Stops the engine and waits until it is gone.
    fn stop(mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for EngineProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn cutweave(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env_remove("CUTWEAVE_DETERMINISTIC_IDS")
        .env_remove("CUTWEAVE_DEV_STUB_MEDIA")
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

fn stderr_json(out: &Output) -> Value {
    serde_json::from_slice(&out.stderr).unwrap_or_else(|e| {
        panic!(
            "stderr is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// Creates a project folder `<dir>/<folder>` named `name` (a one-shot engine saves it).
fn make_project(dir: &Path, folder: &str, name: &str) -> PathBuf {
    let path = dir.join(folder);
    let root = dir.to_str().unwrap();
    let out = cutweave(&[
        "exec",
        &json!({"cmd": "project.save_as", "args": {"path": path.to_str().unwrap()}}).to_string(),
        "--headless",
        "--allow-root",
        root,
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let out = cutweave(&[
        "exec",
        &json!({"cmd": "project.new", "args": {"name": name}}).to_string(),
        "--project",
        path.to_str().unwrap(),
        "--save",
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    path
}

/// `cutweave engines --json` in `run_dir`.
fn engines(run_dir: &str) -> Vec<Value> {
    let out = cutweave(&["engines", "--json", "--run-dir", run_dir]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    stdout_json(&out).as_array().cloned().unwrap()
}

fn project_name(out: &Output) -> String {
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    stdout_json(out)["output"]["name"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Calls `project.report_focus` over `/rpc` with `token`; the response frame.
async fn report_focus(port: u16, token: &str) -> Value {
    let mut request = format!("ws://127.0.0.1:{port}/rpc")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let request_id = cutweave_commands::RequestId::generate(&mut cutweave_model::Uuid7Gen);
    let frame = json!({
        "jsonrpc": "2.0", "id": 1, "method": "project.report_focus",
        "params": {"_meta": {"request_id": request_id.to_string()}},
    });
    socket
        .send(Message::Text(frame.to_string().into()))
        .await
        .unwrap();
    loop {
        let Some(Ok(Message::Text(text))) = socket.next().await else {
            panic!("the engine closed the socket");
        };
        let value: Value = serde_json::from_str(text.as_str()).unwrap();
        if value.get("id") == Some(&json!(1)) {
            let _ = socket.close(None).await;
            return value;
        }
    }
}

/// Waits until `check` holds for the engines listing.
fn wait_for_listing(run_dir: &str, what: &str, check: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let found = engines(run_dir);
        if check(&found) {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {found:#?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Acceptance 1: two headless engines on projects A and B plus a GUI-mode engine on C.
#[test]
fn engines_are_listed_and_chosen_by_project_or_focus() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let run = run_dir.to_str().unwrap().to_owned();
    let a = make_project(dir.path(), "proj-a", "Alpha");
    let b = make_project(dir.path(), "proj-b", "Beta");
    let c = make_project(dir.path(), "proj-c", "Gamma");

    let engine_a = EngineProc::start(&run_dir, dir.path(), Some(&a), false);
    let engine_b = EngineProc::start(&run_dir, dir.path(), Some(&b), false);
    let engine_c = EngineProc::start(&run_dir, dir.path(), Some(&c), true);

    // `engines --json` lists all three with their projects, mode and start.
    let listed = engines(&run);
    assert_eq!(listed.len(), 3, "{listed:#?}");
    let by_pid = |pid: u32| listed.iter().find(|e| e["pid"] == pid).unwrap().clone();
    assert_eq!(by_pid(engine_a.pid)["project"]["name"], "Alpha");
    assert_eq!(by_pid(engine_a.pid)["mode"], "headless");
    assert_eq!(by_pid(engine_b.pid)["project"]["name"], "Beta");
    assert_eq!(by_pid(engine_c.pid)["project"]["name"], "Gamma");
    assert_eq!(by_pid(engine_c.pid)["mode"], "gui");
    assert!(by_pid(engine_b.pid)["started"].as_u64().unwrap() > 0);
    assert_eq!(by_pid(engine_b.pid)["port"], engine_b.port);
    assert!(
        !listed.iter().any(|e| e.to_string().contains("token")),
        "the listing never carries a token"
    );
    // The GUI engine is the one with a window: first, though it never reported a focus.
    assert_eq!(listed[0]["pid"], engine_c.pid);
    // The text table names the projects too.
    let table = cutweave(&["engines", "--run-dir", &run]);
    let table = String::from_utf8_lossy(&table.stdout).into_owned();
    assert!(table.contains("Alpha") && table.contains("Beta") && table.contains("Gamma"));

    // `exec --project B` edits B only (a name in any case, then a path).
    let rename = |name: &str| json!({"cmd": "project.rename", "args": {"name": name}}).to_string();
    let out = cutweave(&[
        "exec",
        &rename("Beta 2"),
        "--project",
        "beta",
        "--run-dir",
        &run,
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let info = |extra: &[&str]| {
        let mut args = vec!["query", "project.info", "--run-dir", &run];
        args.extend_from_slice(extra);
        cutweave(&args)
    };
    let pid_a = engine_a.pid.to_string();
    let pid_b = engine_b.pid.to_string();
    assert_eq!(project_name(&info(&["--pid", &pid_a])), "Alpha");
    assert_eq!(project_name(&info(&["--pid", &pid_b])), "Beta 2");
    assert_eq!(
        project_name(&info(&["--project", b.to_str().unwrap()])),
        "Beta 2"
    );
    assert_eq!(
        project_name(&info(&[
            "--project",
            b.join("main.cutweave").to_str().unwrap()
        ])),
        "Beta 2"
    );
    // The folder's name works, too, and a run-file that was not rewritten yet is no problem.
    assert_eq!(project_name(&info(&["--project", "PROJ-A"])), "Alpha");

    // With no flags the GUI engine is chosen.
    assert_eq!(project_name(&info(&[])), "Gamma");

    // No engine has the project: exit 3, the engines listed.
    let out = info(&["--project", "nobody"]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let e = stderr_json(&out);
    assert_eq!(e["code"], "no_engine");
    assert_eq!(e["engines"].as_array().unwrap().len(), 3);

    // Without the GUI engine two headless engines are ambiguous: exit 3 with both listed.
    engine_c.stop();
    let out = info(&[]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let e = stderr_json(&out);
    assert_eq!(e["code"], "ambiguous_engine");
    assert_eq!(e["engines"].as_array().unwrap().len(), 2, "{e}");
    assert!(e["message"].as_str().unwrap().contains("--project"));
    // The stale run-file of the stopped engine is not listed.
    assert_eq!(engines(&run).len(), 2);
    // Still choosable, and with one headless engine left the default is that engine.
    assert_eq!(project_name(&info(&["--project", "alpha"])), "Alpha");
    engine_b.stop();
    assert_eq!(project_name(&info(&[])), "Alpha");
    drop(engine_a);
}

/// The run-file follows the open project: a project opened in a running engine is the one
/// `--project` finds.
#[test]
fn the_run_file_follows_the_open_project() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let run = run_dir.to_str().unwrap().to_owned();
    let a = make_project(dir.path(), "proj-a", "Alpha");
    let engine = EngineProc::start(&run_dir, dir.path(), None, false);
    assert!(engines(&run)[0]["project"].is_null());

    let open = json!({"cmd": "project.open", "args": {"path": a.to_str().unwrap()}}).to_string();
    let out = cutweave(&["exec", &open, "--run-dir", &run]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let listed = wait_for_listing(&run, "the opened project", |e| {
        e[0]["project"]["name"] == "Alpha"
    });
    assert_eq!(listed[0]["pid"], engine.pid);
    let out = cutweave(&[
        "query",
        "project.info",
        "--project",
        "alpha",
        "--run-dir",
        &run,
    ]);
    assert_eq!(project_name(&out), "Alpha");

    let out = cutweave(&["exec", r#"{"cmd":"project.close"}"#, "--run-dir", &run]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    wait_for_listing(&run, "the closed project", |e| e[0]["project"].is_null());
}

/// The focus report: only the UI credential may send it; the engine with the newest focus is
/// listed first and is the default.
#[tokio::test]
async fn the_focused_gui_engine_is_the_default() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let run = run_dir.to_str().unwrap().to_owned();
    let a = make_project(dir.path(), "proj-a", "Alpha");
    let b = make_project(dir.path(), "proj-b", "Beta");
    let headless = EngineProc::start(&run_dir, dir.path(), None, false);
    let one = EngineProc::start(&run_dir, dir.path(), Some(&a), true);
    let two = EngineProc::start(&run_dir, dir.path(), Some(&b), true);
    let default_project = || project_name(&cutweave(&["query", "project.info", "--run-dir", &run]));

    // An agent cannot report a focus: the command is UI-only and not an MCP tool.
    let token = {
        let text = std::fs::read_to_string(run_dir.join(format!("{}.json", one.pid))).unwrap();
        let file: Value = serde_json::from_str(&text).unwrap();
        file["agent_tokens"][0]["token"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let refused = report_focus(one.port, &token).await;
    assert_eq!(refused["error"]["data"]["code"], "forbidden", "{refused}");

    // Window one gains the focus, then window two, then window one again.
    for (engine, expect) in [(&one, "Alpha"), (&two, "Beta"), (&one, "Alpha")] {
        let reply = report_focus(engine.port, UI_TOKEN).await;
        assert!(
            reply["result"]["focused_at"].as_u64().unwrap() > 0,
            "{reply}"
        );
        let at = reply["result"]["focused_at"].as_u64().unwrap();
        let listed = wait_for_listing(&run, "the focus time", |e| {
            e.iter()
                .any(|x| x["pid"] == engine.pid && x["focused_at"] == at)
        });
        assert_eq!(
            listed[0]["pid"], engine.pid,
            "newest focus first: {listed:#?}"
        );
        assert_eq!(default_project(), expect);
        // Focus times differ by at least a millisecond between reports.
        std::thread::sleep(Duration::from_millis(5));
    }
    // A headless engine has no window and no focus time.
    let listed = engines(&run);
    let headless_entry = listed.iter().find(|e| e["pid"] == headless.pid).unwrap();
    assert!(headless_entry.get("focused_at").is_none());
    // `--pid` and `--project` still win over the focus.
    let pid_two = two.pid.to_string();
    let out = cutweave(&[
        "query",
        "project.info",
        "--run-dir",
        &run,
        "--pid",
        &pid_two,
    ]);
    assert_eq!(project_name(&out), "Beta");
}

// ----- the attach proxy ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Revision {
    Stateful,
    Stateless,
}

#[derive(Clone)]
struct Versioned(ProtocolVersion);

impl ClientHandler for Versioned {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default().with_protocol_version(self.0.clone())
    }
}

struct Agent {
    service: RunningService<RoleClient, Versioned>,
    child: tokio::process::Child,
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
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn cutweave mcp");
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        let (handler, version) = match revision {
            Revision::Stateful => (Versioned(ProtocolVersion::V_2025_11_25), None),
            Revision::Stateless => (
                Versioned(ProtocolVersion::V_2026_07_28),
                Some(ProtocolVersion::V_2026_07_28),
            ),
        };
        let service = match version {
            None => handler.serve((stdout, stdin)).await.expect("initialize"),
            Some(version) => handler
                .serve_with_lifecycle(
                    (stdout, stdin),
                    ClientLifecycleMode::Discover {
                        preferred_versions: vec![version],
                    },
                )
                .await
                .expect("server/discover"),
        };
        Agent { service, child }
    }

    async fn ok(&self, tool: &str, args: Value) -> Value {
        let Value::Object(args) = args else {
            panic!("args must be an object")
        };
        let result = self
            .service
            .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args))
            .await
            .expect("tools/call");
        assert_ne!(result.is_error, Some(true), "{tool} failed: {result:?}");
        result.structured_content.expect("structuredContent")
    }

    async fn err(&self, tool: &str, args: Value) -> Value {
        let Value::Object(args) = args else {
            panic!("args must be an object")
        };
        let result = self
            .service
            .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args))
            .await
            .expect("tools/call");
        assert_eq!(result.is_error, Some(true), "{tool} succeeded: {result:?}");
        result.structured_content.expect("structuredContent")
    }

    async fn close(self) {
        let Agent { service, mut child } = self;
        let _ = service.cancel().await;
        let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
            .await
            .expect("cutweave mcp exits when its stdin closes")
            .unwrap();
        assert_eq!(status.code(), Some(0));
    }
}

/// Acceptance 2: a scripted client on `mcp --attach` calls `engines_list`, then
/// `engine_attach{project:"B"}`, then `timeline_get`, and gets B's timeline; it needs no pid in
/// its config. Both spec revisions.
#[tokio::test]
async fn an_agent_lists_engines_and_attaches_to_a_project() {
    for revision in [Revision::Stateful, Revision::Stateless] {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("run");
        let run = run_dir.to_str().unwrap().to_owned();
        let a = make_project(dir.path(), "proj-a", "Alpha");
        let b = make_project(dir.path(), "proj-b", "Beta");
        let c = make_project(dir.path(), "proj-c", "Gamma");
        // B's timeline is told apart by its sequence id, which no other project has.
        let sequence_b = {
            let engine_b = EngineProc::start(&run_dir, dir.path(), Some(&b), false);
            let out = cutweave(&[
                "query",
                "project.info",
                "--pid",
                &engine_b.pid.to_string(),
                "--run-dir",
                &run,
            ]);
            let sequence = stdout_json(&out)["output"]["sequences"][0]["id"].clone();
            engine_b.stop();
            sequence
        };
        let engine_a = EngineProc::start(&run_dir, dir.path(), Some(&a), false);
        let engine_b = EngineProc::start(&run_dir, dir.path(), Some(&b), false);
        let engine_c = EngineProc::start(&run_dir, dir.path(), Some(&c), true);

        // No pid, no project in the config: the app's engine is chosen.
        let agent = Agent::start(&["--attach", "--run-dir", &run], revision).await;
        let tools = agent.service.list_tools(None).await.unwrap().tools;
        for name in PROXY_TOOLS {
            assert!(tools.iter().any(|t| t.name == name), "{name} is listed");
        }
        // The engine's own tools stay within the P1 gate; the proxy's two come on top and are
        // counted apart (they are the proxy's, not the engine's).
        let (engine_tools, proxy_tools): (Vec<_>, Vec<_>) = tools
            .iter()
            .cloned()
            .partition(|t| !PROXY_TOOLS.contains(&&*t.name));
        let (engine_size, proxy_size, total) = (
            ListSize::of(&engine_tools),
            ListSize::of(&proxy_tools),
            ListSize::of(&tools),
        );
        assert!(P1_GATE.fits(engine_size), "{engine_size:?}");
        assert_eq!(proxy_size.tools, 2);
        assert!(proxy_size.tokens() <= 300, "{proxy_size:?}");
        println!(
            "attach proxy tools/list ({revision:?}): {} tools, ~{} tokens (engine {} tools, ~{} tokens; proxy {} tools, ~{} tokens)",
            total.tools,
            total.tokens(),
            engine_size.tools,
            engine_size.tokens(),
            proxy_size.tools,
            proxy_size.tokens()
        );
        assert_eq!(agent.ok("project_info", json!({})).await["name"], "Gamma");
        // B's sequence is not on the engine the agent started on.
        let missing = agent
            .err("timeline_get", json!({"sequence": sequence_b}))
            .await;
        assert_eq!(missing["error"]["code"], "not_found", "{missing}");

        let listed = agent.ok("engines_list", json!({})).await;
        let list = listed["engines"].as_array().unwrap();
        assert_eq!(list.len(), 3, "{listed}");
        let current: Vec<&Value> = list.iter().filter(|e| e["current"] == true).collect();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0]["pid"], engine_c.pid);
        assert!(!listed.to_string().contains("token"));

        // Attach to B: the later tools act on B.
        let attached = agent.ok("engine_attach", json!({"project": "beta"})).await;
        assert_eq!(attached["attached"]["pid"], engine_b.pid, "{attached}");
        assert_eq!(attached["attached"]["project"]["name"], "Beta");
        assert_eq!(attached["catalog_changed"], false);
        assert_eq!(agent.ok("project_info", json!({})).await["name"], "Beta");
        let timeline = agent
            .ok("timeline_get", json!({"sequence": sequence_b}))
            .await;
        assert_eq!(timeline["id"], sequence_b, "B's timeline: {timeline}");
        // A rename through the agent lands in B and in no other engine.
        agent
            .ok(
                "cutweave_execute",
                json!({"steps": [{"cmd": "project.rename", "args": {"name": "Beta 3"}}]}),
            )
            .await;
        let read = |pid: u32| {
            project_name(&cutweave(&[
                "query",
                "project.info",
                "--run-dir",
                &run,
                "--pid",
                &pid.to_string(),
            ]))
        };
        assert_eq!(read(engine_b.pid), "Beta 3");
        assert_eq!(read(engine_a.pid), "Alpha");
        assert_eq!(read(engine_c.pid), "Gamma");

        // By pid; attaching to the engine already in use changes nothing; refusals.
        let again = agent
            .ok("engine_attach", json!({"pid": engine_a.pid}))
            .await;
        assert_eq!(again["attached"]["pid"], engine_a.pid);
        assert_eq!(agent.ok("project_info", json!({})).await["name"], "Alpha");
        let same = agent
            .ok("engine_attach", json!({"pid": engine_a.pid}))
            .await;
        assert_eq!(same["attached"]["pid"], engine_a.pid);
        let none = agent
            .err("engine_attach", json!({"project": "nobody"}))
            .await;
        assert_eq!(none["error"]["code"], "no_engine");
        assert_eq!(none["error"]["engines"].as_array().unwrap().len(), 3);
        let empty = agent.err("engine_attach", json!({})).await;
        assert_eq!(empty["error"]["code"], "invalid");
        // A failed attach left the session where it was.
        assert_eq!(agent.ok("project_info", json!({})).await["name"], "Alpha");

        // Closing stdin ends the session cleanly.
        agent.close().await;
        engine_a.stop();
        engine_b.stop();
        engine_c.stop();
    }
}

/// `--project` on `mcp --attach` picks the engine at the start, and no engine for it is exit 3
/// with the engines listed.
#[test]
fn mcp_attach_takes_project_and_fails_when_nothing_matches() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let run = run_dir.to_str().unwrap().to_owned();
    let a = make_project(dir.path(), "proj-a", "Alpha");
    let engine_a = EngineProc::start(&run_dir, dir.path(), Some(&a), false);
    let engine_b = EngineProc::start(&run_dir, dir.path(), None, false);

    // Two headless engines and no choice: exit 3, nothing on stdout.
    let out = cutweave(&["mcp", "--attach", "--run-dir", &run]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert_eq!(out.stdout.len(), 0);
    let e = stderr_json(&out);
    assert_eq!(e["code"], "ambiguous_engine");
    assert_eq!(e["engines"].as_array().unwrap().len(), 2);

    let out = cutweave(&["mcp", "--attach", "--run-dir", &run, "--project", "nobody"]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert_eq!(stderr_json(&out)["code"], "no_engine");
    drop((engine_a, engine_b));
}

/// The proxy tools are the proxy's own: the engine's `/mcp` lists neither.
#[tokio::test]
async fn the_engines_own_tool_list_has_no_proxy_tools() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let engine = EngineProc::start(&run_dir, dir.path(), None, false);
    let text = std::fs::read_to_string(run_dir.join(format!("{}.json", engine.pid))).unwrap();
    let file: Value = serde_json::from_str(&text).unwrap();
    let token = file["agent_tokens"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["label"] == "mcp")
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = format!("http://127.0.0.1:{}/mcp", engine.port);
    let transport = rmcp::transport::StreamableHttpClientTransport::from_config(
        rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(url)
            .auth_header(token),
    );
    let client = Versioned(ProtocolVersion::V_2025_11_25)
        .serve(transport)
        .await
        .expect("initialize");
    let tools = client.list_tools(None).await.unwrap().tools;
    for name in PROXY_TOOLS {
        assert!(
            !tools.iter().any(|t| t.name == name),
            "{name} is an engine tool"
        );
    }
    let _ = client.cancel().await;
}
