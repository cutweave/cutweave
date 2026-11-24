// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1I.04 launch-protocol tests against the real engine binary (`cutweave-engine-serve`), as
//! `EngineSupervisor` drives it: handshake, readiness line, stdin-close stop, exit codes 10, 12
//! and 13 with their JSON error lines, the JSON log file, and no outbound sockets.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod engine_support;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cutweave_engine::project_io::ProjectStore;
use engine_support::{Socket, TempDir, empty_project, sockets_of};
use serde_json::Value;

const UI: &str = "handshakehandshakehandshakehandshakehandshakehandshake0123456789ab";
const WAIT: Duration = Duration::from_secs(30);

struct Engine {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl Engine {
    fn spawn(dir: &Path, args: &[&str]) -> Engine {
        Engine::spawn_with_env(dir, args, &[])
    }

    fn spawn_with_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Engine {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cutweave-engine-serve"))
            .arg("engine")
            .args(args)
            .arg("--run-dir")
            .arg(dir.join("run"))
            .arg("--log-dir")
            .arg(dir.join("logs"))
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Engine {
            child,
            stdin,
            lines,
        }
    }

    fn handshake(&mut self, token: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(format!("{token}\n").as_bytes()).unwrap();
        stdin.flush().unwrap();
    }

    fn line(&self) -> Value {
        let line = self.lines.recv_timeout(WAIT).expect("no stdout line");
        assert!(!line.contains(UI), "a token reached stdout");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"))
    }

    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the engine did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn gui_engine_handshakes_reports_ready_and_stops_when_stdin_closes() {
    let t = TempDir::new("gui");
    let mut e = Engine::spawn(
        t.path(),
        &[
            "--port",
            "0",
            "--token-stdin",
            "--allow-origin",
            "app://cutweave",
        ],
    );
    e.handshake(UI);
    let ready = e.line();
    // The shape `parseReadyLine` in apps/desktop/src/main/engine/supervisor.ts expects.
    assert_eq!(ready["event"], "ready");
    assert!(ready["port"].as_u64().unwrap() > 0);
    assert_eq!(ready["pid"], e.child.id());
    assert_eq!(ready["version"], cutweave_engine::VERSION);
    let run_files = files_under(&t.path().join("run"));
    assert_eq!(run_files.len(), 1);

    drop(e.stdin.take());
    let status = e.wait();
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        files_under(&t.path().join("run")),
        Vec::<PathBuf>::new(),
        "run-file left"
    );

    // The log is JSON lines, without the UI credential; a clean exit leaves no crash marker.
    let pid = e.child.id();
    let log = t.path().join("logs").join(format!("engine-{pid}.log"));
    let text = std::fs::read_to_string(&log).unwrap();
    let mut saw_ready = false;
    for line in text.lines() {
        let v: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert!(v["ts"].as_str().unwrap().ends_with('Z'));
        assert!(v["level"].is_string() && v["target"].is_string());
        saw_ready |= v["msg"] == "engine ready";
    }
    assert!(saw_ready, "no `engine ready` line in {text}");
    assert!(
        !t.path()
            .join("logs")
            .join(format!("crash-{pid}.json"))
            .exists()
    );
    for file in files_under(t.path()) {
        let bytes = std::fs::read(&file).unwrap();
        assert!(
            !bytes.windows(UI.len()).any(|w| w == UI.as_bytes()),
            "the UI credential reached {}",
            file.display()
        );
    }
}

#[test]
fn bad_handshakes_exit_13() {
    let t = TempDir::new("handshake");
    // No line within the (shortened) timeout.
    let mut e = Engine::spawn(t.path(), &["--port", "0", "--handshake-timeout-ms", "300"]);
    let started = Instant::now();
    let line = e.line();
    assert_eq!(e.wait().code(), Some(13));
    assert!(started.elapsed() >= Duration::from_millis(250));
    assert_eq!(line["event"], "error");
    assert_eq!(line["code"], "bad_handshake");
    assert_eq!(line["exit_code"], 13);
    // A malformed token.
    let mut e = Engine::spawn(t.path(), &["--port", "0", "--token-stdin"]);
    e.handshake("short");
    assert_eq!(e.line()["code"], "bad_handshake");
    assert_eq!(e.wait().code(), Some(13));
    // Stdin closed before the handshake.
    let mut e = Engine::spawn(t.path(), &["--port", "0"]);
    drop(e.stdin.take());
    assert_eq!(e.wait().code(), Some(13));
    // A headless engine rejects any UI-credential handshake (WP-1I.01 handoff).
    let mut e = Engine::spawn(t.path(), &["--headless", "--port", "0", "--token-stdin"]);
    e.handshake(UI);
    assert_eq!(e.line()["code"], "bad_handshake");
    assert_eq!(e.wait().code(), Some(13));
    assert_eq!(files_under(&t.path().join("run")), Vec::<PathBuf>::new());
}

#[test]
fn a_locked_project_exits_10_with_a_json_error_line() {
    let t = TempDir::new("locked");
    let folder = empty_project(t.path(), "Held");
    let holder = ProjectStore::open(&folder).unwrap();
    let project = folder.to_string_lossy().into_owned();
    let mut e = Engine::spawn(
        t.path(),
        &["--headless", "--port", "0", "--project", &project],
    );
    let line = e.line();
    assert_eq!(e.wait().code(), Some(10));
    assert_eq!(line["event"], "error");
    assert_eq!(line["code"], "project_locked");
    assert_eq!(line["exit_code"], 10);
    assert_eq!(line["holder"]["pid"], std::process::id());
    assert!(line["message"].as_str().unwrap().contains("already open"));
    assert_eq!(files_under(&t.path().join("run")), Vec::<PathBuf>::new());
    drop(holder);
}

#[test]
fn an_occupied_port_exits_12() {
    let t = TempDir::new("port");
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = blocker.local_addr().unwrap().port().to_string();
    let folder = empty_project(t.path(), "Port");
    let project = folder.to_string_lossy().into_owned();
    let mut e = Engine::spawn(
        t.path(),
        &["--headless", "--port", &port, "--project", &project],
    );
    let line = e.line();
    assert_eq!(e.wait().code(), Some(12));
    assert_eq!(line["code"], "port_in_use");
    assert_eq!(line["exit_code"], 12);
    assert_eq!(files_under(&t.path().join("run")), Vec::<PathBuf>::new());
    // The project opened before the bind failed is released again.
    drop(ProjectStore::open(&folder).unwrap());
}

/// WP-1I.05: a `Gui` engine whose GPU cannot start exits 11 with a JSON error line; a headless
/// engine falls back to the software adapter (or the CPU) and serves.
#[test]
fn a_gui_engine_without_a_usable_gpu_exits_11_and_a_headless_one_falls_back() {
    let t = TempDir::new("gpu");
    let no_gpu = [
        ("CUTWEAVE_GPU", "no-adapter-is-called-this"),
        ("CUTWEAVE_AUDIO", "null"),
    ];
    let mut gui = Engine::spawn_with_env(t.path(), &["--port", "0", "--token-stdin"], &no_gpu);
    gui.handshake(UI);
    let line = gui.line();
    assert_eq!(gui.wait().code(), Some(11), "{line}");
    assert_eq!(line["event"], "error");
    assert_eq!(line["code"], "gpu_init_failed");
    assert_eq!(line["exit_code"], 11);
    assert_eq!(files_under(&t.path().join("run")), Vec::<PathBuf>::new());

    let mut headless = Engine::spawn_with_env(t.path(), &["--headless", "--port", "0"], &no_gpu);
    let ready = headless.line();
    assert_eq!(ready["event"], "ready", "{ready}");
    drop(headless.stdin.take());
}

#[test]
fn usage_errors_exit_2() {
    let t = TempDir::new("usage");
    let mut e = Engine::spawn(t.path(), &["--bogus"]);
    assert_eq!(e.line()["code"], "usage");
    assert_eq!(e.wait().code(), Some(2));
}

/// WP-1J.01: the engine process serves MCP next to `/rpc`. An agent with the `mcp` token from
/// the run-file (never the UI credential) lists the tools on `/mcp` and `/mcp/minimal` and
/// calls one, with rmcp's own client.
#[test]
fn the_engine_process_serves_mcp_to_the_run_file_agent_token() {
    use rmcp::ServiceExt as _;
    use rmcp::model::CallToolRequestParams;
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let t = TempDir::new("mcp");
    let e = Engine::spawn(t.path(), &["--headless", "--port", "0"]);
    let ready = e.line();
    assert_eq!(ready["event"], "ready");
    let port = ready["port"].as_u64().unwrap();
    let run = cutweave_api::RunFile::discover(&t.path().join("run"));
    assert_eq!(run.len(), 1);
    assert_eq!(u64::from(run[0].port), port);
    let token = run[0]
        .agent_tokens
        .iter()
        .find(|a| a.label == "mcp")
        .unwrap()
        .token
        .clone();

    let connect = |path: &str| {
        StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}{path}"))
                .auth_header(token.clone()),
        )
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let client = ().serve(connect("/mcp")).await.expect("initialize");
        let tools = client.list_tools(None).await.unwrap().tools;
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        // 16 core tools (`render_frame` arrives with the full engine's renderer, WP-1I.05) and
        // the 4 generic ones. NOTE for the integrator: one above the P1 gate of 19
        // (`cutweave_api::mcp::P1_GATE`); WP-1J.02 decides (see the WP-1I.05 report).
        assert_eq!(names.len(), 20, "{names:?}");
        for name in [
            "clip_split",
            "timeline_get",
            "export_start",
            "render_frame",
            "cutweave_execute",
        ] {
            assert!(names.contains(&name), "{name} missing from {names:?}");
        }
        let info = client
            .call_tool(CallToolRequestParams::new("project_info"))
            .await
            .unwrap();
        assert_ne!(info.is_error, Some(true));
        let info = info.structured_content.unwrap();
        assert!(info["sequences"].is_array(), "{info}");
        // The actor is the token's: the run-file label.
        let created = client
            .call_tool(
                CallToolRequestParams::new("cutweave_execute").with_arguments(
                    serde_json::json!({
                        "steps": [{ "cmd": "project.new", "args": { "name": "Launched" } }]
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .await
            .unwrap();
        assert_ne!(created.is_error, Some(true), "{created:?}");
        let history = client
            .call_tool(
                CallToolRequestParams::new("cutweave_query").with_arguments(
                    serde_json::json!({ "name": "edit.history", "args": { "limit": 1 } })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(history["entries"][0]["actor"], "agent:mcp");
        let _ = client.cancel().await;

        let minimal = ().serve(connect("/mcp/minimal")).await.expect("initialize");
        // `render_frame` is one of the minimal profile's tools.
        assert_eq!(minimal.list_tools(None).await.unwrap().tools.len(), 10);
        let _ = minimal.cancel().await;
    });
}

/// No telemetry: from spawn to exit, every socket the engine process owns is a loopback one
/// (its listener, and the test's connection to it). Sampled with the OS socket table.
#[test]
fn the_engine_opens_no_outbound_sockets_during_startup_and_shutdown() {
    let t = TempDir::new("network");
    let folder = empty_project(t.path(), "Offline");
    let project = folder.to_string_lossy().into_owned();
    // The socket table is read (netstat / lsof) before the token is sent, which can take longer
    // than the default 5 s handshake timeout on a loaded machine.
    let mut e = Engine::spawn(
        t.path(),
        &[
            "--port",
            "0",
            "--token-stdin",
            "--project",
            &project,
            "--handshake-timeout-ms",
            "60000",
        ],
    );
    let pid = e.child.id();
    if sockets_of(pid).is_none() {
        println!("skipped: no socket table tool (netstat / lsof) on this machine");
        return;
    }
    let seen: Arc<Mutex<Vec<Socket>>> = Arc::default();
    let rounds = Arc::new(Mutex::new(0_u32));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sampler = {
        let (seen, rounds, stop) = (Arc::clone(&seen), Arc::clone(&rounds), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if let Some(sockets) = sockets_of(pid) {
                    seen.lock().unwrap().extend(sockets);
                    *rounds.lock().unwrap() += 1;
                }
            }
        })
    };
    e.handshake(UI);
    let ready = e.line();
    let port = ready["port"].as_u64().unwrap();
    // Hold the engine up until the sampler has seen it serving.
    let deadline = Instant::now() + WAIT;
    while !seen
        .lock()
        .unwrap()
        .iter()
        .any(|s| s.local.ends_with(&format!(":{port}")))
    {
        assert!(Instant::now() < deadline, "the listener never showed up");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(e.stdin.take());
    assert_eq!(e.wait().code(), Some(0));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    sampler.join().unwrap();
    let seen = seen.lock().unwrap();
    println!(
        "{} socket-table samples, {} sockets seen",
        rounds.lock().unwrap(),
        seen.len()
    );
    let outbound: Vec<&Socket> = seen.iter().filter(|s| s.is_outbound()).collect();
    assert!(outbound.is_empty(), "outbound sockets: {outbound:?}");
}
