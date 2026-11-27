// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]
//! Process-level tests of what agents may reach (WP-1J.05): the settings the desktop app applies
//! to its engine (`project.set_allowed_roots`, `project.set_autosave`, `edit.set_history_limit`)
//! refuse agent tokens on every surface (RPC, MCP's `cutweave_execute`, the CLI); a folder the person removes stops an
//! agent session that is already open at once, with a message naming the setting; agent
//! connections are listed (`project.connections`) and announced.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rmcp::{
    ClientHandler, ServiceExt as _,
    model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
    service::RunningService,
};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const BIN: &str = env!("CARGO_BIN_EXE_cutweave");

/// The UI credential the GUI engine is given.
const UI_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

/// What the refusal of a path outside the allowed folders names.
const SETTING: &str = "Settings > Agents and MCP";

/// A GUI-mode `cutweave engine` child with stub media, killed on drop.
struct GuiEngine {
    child: Child,
    pid: u32,
    port: u16,
    stdin: Option<std::process::ChildStdin>,
}

impl GuiEngine {
    fn start(run_dir: &Path, project: &Path) -> GuiEngine {
        let mut child = Command::new(BIN)
            .args(["engine", "--port", "0", "--no-log", "--dev-stub-media"])
            .args(["--token-stdin", "--allow-origin", "app://cutweave"])
            .arg("--project")
            .arg(project)
            .arg("--run-dir")
            .arg(run_dir)
            .env_remove("CUTWEAVE_DETERMINISTIC_IDS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn engine");
        let mut stdin = child.stdin.take();
        writeln!(stdin.as_mut().unwrap(), "{UI_TOKEN}").expect("handshake");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let ready: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert_eq!(ready["event"], "ready", "{line}");
        GuiEngine {
            child,
            pid: u32::try_from(ready["pid"].as_u64().unwrap()).unwrap(),
            port: u16::try_from(ready["port"].as_u64().unwrap()).unwrap(),
            stdin,
        }
    }

    /// The run-file's agent token with this label (`cli`, `mcp`).
    fn agent_token(&self, run_dir: &Path, label: &str) -> String {
        let text = std::fs::read_to_string(run_dir.join(format!("{}.json", self.pid))).unwrap();
        let file: Value = serde_json::from_str(&text).unwrap();
        file["agent_tokens"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["label"] == label)
            .and_then(|t| t["token"].as_str())
            .unwrap()
            .to_owned()
    }
}

impl Drop for GuiEngine {
    fn drop(&mut self) {
        self.stdin.take();
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

fn stderr_json(out: &Output) -> Value {
    serde_json::from_slice(&out.stderr).unwrap_or_else(|e| {
        panic!(
            "stderr is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// A project folder `<dir>/<folder>` named `name` (a one-shot engine saves it).
fn make_project(dir: &Path, folder: &str, name: &str) -> PathBuf {
    let path = dir.join(folder);
    let out = cutweave(&[
        "exec",
        &json!({"cmd": "project.save_as", "args": {"path": path.to_str().unwrap()}}).to_string(),
        "--headless",
        "--allow-root",
        dir.to_str().unwrap(),
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

/// One JSON-RPC WebSocket to the engine, as the desktop editor (UI token) or a script (agent
/// token) holds it. Notifications that arrive while waiting for a reply are kept.
struct Rpc {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    notifications: Vec<Value>,
}

impl Rpc {
    async fn connect(port: u16, token: &str) -> Rpc {
        let mut request = format!("ws://127.0.0.1:{port}/rpc")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let (socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        Rpc {
            socket,
            next_id: 1,
            notifications: Vec::new(),
        }
    }

    /// The response frame of `method` with `params` (and a fresh `request_id`).
    async fn call(&mut self, method: &str, mut params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        if !method.starts_with("rpc.") {
            let request_id = cutweave_commands::RequestId::generate(&mut cutweave_model::Uuid7Gen);
            params["_meta"] = json!({ "request_id": request_id.to_string() });
        }
        let frame = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.socket
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
        loop {
            let value = self.next_frame().await;
            if value.get("id") == Some(&json!(id)) {
                return value;
            }
            self.notifications.push(value);
        }
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let reply = self.call(method, params).await;
        assert!(reply.get("error").is_none(), "{method}: {reply}");
        reply["result"].clone()
    }

    async fn next_frame(&mut self) -> Value {
        let frame = tokio::time::timeout(Duration::from_secs(20), self.socket.next())
            .await
            .expect("a frame within 20 s");
        let Some(Ok(Message::Text(text))) = frame else {
            panic!("the engine closed the socket");
        };
        serde_json::from_str(text.as_str()).unwrap()
    }

    /// Waits for the notification `method` whose params satisfy `check`.
    async fn notification(&mut self, method: &str, check: impl Fn(&Value) -> bool) -> Value {
        if let Some(i) = self
            .notifications
            .iter()
            .position(|n| n["method"] == method && check(&n["params"]))
        {
            return self.notifications.remove(i)["params"].clone();
        }
        loop {
            let value = self.next_frame().await;
            if value["method"] == method && check(&value["params"]) {
                return value["params"].clone();
            }
        }
    }

    async fn close(mut self) {
        let _ = self.socket.close(None).await;
    }
}

#[derive(Clone)]
struct Stateful;

impl ClientHandler for Stateful {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default().with_protocol_version(ProtocolVersion::V_2025_11_25)
    }
}

/// `cutweave mcp --attach` as an agent host starts it.
struct McpAgent {
    service: RunningService<rmcp::RoleClient, Stateful>,
    child: tokio::process::Child,
}

impl McpAgent {
    async fn start(run_dir: &Path, pid: u32) -> McpAgent {
        let mut child = tokio::process::Command::new(BIN)
            .args(["mcp", "--attach", "--run-dir"])
            .arg(run_dir)
            .args(["--pid", &pid.to_string()])
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
        let service = Stateful.serve((stdout, stdin)).await.expect("initialize");
        McpAgent { service, child }
    }

    /// The tool's structured result and whether it was an error.
    async fn call(&self, tool: &str, args: Value) -> (bool, Value) {
        let Value::Object(args) = args else {
            panic!("args must be an object")
        };
        let result = self
            .service
            .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args))
            .await
            .expect("tools/call");
        (
            result.is_error == Some(true),
            result.structured_content.expect("structuredContent"),
        )
    }

    async fn close(self) {
        let McpAgent { service, mut child } = self;
        let _ = service.cancel().await;
        let _ = tokio::time::timeout(Duration::from_secs(15), child.wait()).await;
    }
}

/// The CLI attached to the engine `pid`: exit code and the JSON it printed (stdout or stderr).
fn cli_exec(run_dir: &Path, pid: u32, step: &Value) -> (Option<i32>, Value) {
    let out = cutweave(&[
        "exec",
        &step.to_string(),
        "--run-dir",
        run_dir.to_str().unwrap(),
        "--pid",
        &pid.to_string(),
    ]);
    let json = if out.status.code() == Some(0) {
        serde_json::from_slice(&out.stdout).unwrap()
    } else {
        stderr_json(&out)
    };
    (out.status.code(), json)
}

/// Acceptance 1: an agent token calling the settings the app applies to its engine gets
/// `forbidden` over RPC, MCP (`cutweave_execute`) and the CLI; the UI credential is obeyed.
#[tokio::test]
async fn app_settings_refuse_agent_tokens_on_every_surface() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let footage = dir.path().join("footage");
    std::fs::create_dir_all(&footage).unwrap();
    let project = make_project(dir.path(), "proj", "Settings");
    let engine = GuiEngine::start(&run_dir, &project);
    let steps = [
        (
            "project.set_allowed_roots",
            json!({ "roots": [footage.to_str().unwrap()] }),
        ),
        (
            "project.set_autosave",
            json!({ "enabled": false, "interval_s": 60, "keep_backups": 5 }),
        ),
        // Settings > Editing > Undo steps (WP-2A.13).
        ("edit.set_history_limit", json!({ "steps": 2 })),
    ];

    let mut rpc_agent = Rpc::connect(engine.port, &engine.agent_token(&run_dir, "cli")).await;
    let mcp = McpAgent::start(&run_dir, engine.pid).await;
    for (cmd, args) in &steps {
        // RPC with an agent token.
        let refused = rpc_agent.call(cmd, args.clone()).await;
        assert_eq!(refused["error"]["data"]["code"], "forbidden", "{refused}");
        // MCP: discoverable, but refused when called.
        let (failed, out) = mcp
            .call(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": cmd, "args": args }] }),
            )
            .await;
        assert!(failed, "{cmd} over MCP: {out}");
        assert_eq!(out["error"]["code"], "forbidden", "{out}");
        let (_, described) = mcp
            .call("cutweave_describe_command", json!({ "name": cmd }))
            .await;
        assert!(
            described.to_string().contains("UI only"),
            "{cmd}: {described}"
        );
        // The CLI attached to the app's engine.
        let (code, err) = cli_exec(&run_dir, engine.pid, &json!({ "cmd": cmd, "args": args }));
        assert_eq!(code, Some(1), "{err}");
        assert_eq!(err["code"], "forbidden", "{err}");
    }
    // Nothing changed: the agents still see only the project folder.
    let mut ui = Rpc::connect(engine.port, UI_TOKEN).await;
    let info = ui.ok("project.info", json!({})).await;
    let roots = info["allowed_roots"].as_array().unwrap().clone();
    assert_eq!(roots.len(), 1, "{info}");
    assert_eq!(info["history_limit"], 1000, "{info}");
    // The app's own credential is obeyed.
    for (cmd, args) in &steps {
        ui.ok(cmd, args.clone()).await;
    }
    let info = ui.ok("project.info", json!({})).await;
    assert_eq!(info["allowed_roots"].as_array().unwrap().len(), 2, "{info}");
    assert_eq!(info["history_limit"], 2, "{info}");

    mcp.close().await;
    rpc_agent.close().await;
    ui.close().await;
    drop(engine);
}

/// Acceptance 2: with folder D allowed an attached agent imports a file from D; after D is
/// removed the same import on the same open connection (an MCP session through the attach
/// proxy, a script's RPC socket) fails with `Invalid` naming Settings > Agents and MCP. The
/// agent connections are listed while they are open, with their last command, and announced.
#[tokio::test]
async fn a_removed_folder_stops_open_agent_connections_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let project = make_project(dir.path(), "proj", "Folders");
    // D lies outside the project folder and every folder the engine was started with.
    let outside = tempfile::tempdir().unwrap();
    let d = outside.path().join("Footage");
    std::fs::create_dir_all(&d).unwrap();
    for name in ["a.mp4", "b.mp4", "c.mp4", "d.mp4"] {
        // Distinct contents: the stub prober's content hash would take equal files as one.
        std::fs::write(d.join(name), format!("stub media {name}")).unwrap();
    }
    let clip = |name: &str| d.join(name).to_str().unwrap().to_owned();
    let engine = GuiEngine::start(&run_dir, &project);
    let mut ui = Rpc::connect(engine.port, UI_TOKEN).await;
    ui.ok(
        "rpc.subscribe",
        json!({ "events": ["project.connection_opened", "project.connection_closed"] }),
    )
    .await;

    // Not allowed yet.
    let mcp = McpAgent::start(&run_dir, engine.pid).await;
    let (failed, out) = mcp
        .call("media_import", json!({ "paths": [clip("a.mp4")] }))
        .await;
    assert!(failed, "{out}");
    assert_eq!(out["error"]["code"], "invalid");
    assert!(
        out["error"]["message"].as_str().unwrap().contains(SETTING),
        "{out}"
    );
    let opened = ui
        .notification("project.connection_opened", |p| p["actor"] == "agent:mcp")
        .await;
    assert!(opened["connected_at"].as_u64().unwrap() > 0, "{opened}");

    // The person allows D (canonicalized; `..` and missing folders are refused).
    let bad = ui
        .call(
            "project.set_allowed_roots",
            json!({ "roots": [d.join("..").join("Footage").to_str().unwrap()] }),
        )
        .await;
    assert_eq!(bad["error"]["data"]["code"], "invalid", "{bad}");
    let set = ui
        .ok(
            "project.set_allowed_roots",
            json!({ "roots": [d.to_str().unwrap(), d.to_str().unwrap()] }),
        )
        .await;
    let canonical = set["roots"].as_array().unwrap().clone();
    assert_eq!(canonical.len(), 1, "duplicates collapse: {set}");

    // The same MCP session imports from D now, and project_info says where it may go.
    let (failed, out) = mcp
        .call("media_import", json!({ "paths": [clip("a.mp4")] }))
        .await;
    assert!(!failed, "{out}");
    let (_, info) = mcp.call("project_info", json!({})).await;
    assert!(
        info["allowed_roots"]
            .as_array()
            .unwrap()
            .contains(&canonical[0]),
        "{info}"
    );
    // A script's RPC socket too, kept open across the change.
    let mut script = Rpc::connect(engine.port, &engine.agent_token(&run_dir, "cli")).await;
    script
        .ok("media.import", json!({ "paths": [clip("b.mp4")] }))
        .await;
    // And a one-shot `cutweave exec`.
    let (code, out) = cli_exec(
        &run_dir,
        engine.pid,
        &json!({ "cmd": "media.import", "args": { "paths": [clip("c.mp4")] } }),
    );
    assert_eq!(code, Some(0), "{out}");

    // Who is connected, with the last command each called.
    let listed = ui.ok("project.connections", json!({})).await;
    let connections = listed["connections"].as_array().unwrap();
    let by_actor = |actor: &str| {
        connections
            .iter()
            .find(|c| c["actor"] == actor)
            .unwrap_or_else(|| panic!("{actor} is listed: {listed}"))
            .clone()
    };
    assert_eq!(by_actor("agent:mcp")["last_command"], "project.info");
    // The oldest `agent:cli` connection is the script's socket (the one-shot CLI came later).
    let script_id = by_actor("agent:cli")["id"].clone();
    assert_eq!(by_actor("agent:cli")["last_command"], "media.import");
    assert!(by_actor("agent:mcp")["last_command_at"].as_u64().unwrap() > 0);
    assert!(
        !listed
            .to_string()
            .contains(&engine.agent_token(&run_dir, "mcp")),
        "no token is ever listed"
    );

    // The person removes D: both open connections are refused at once.
    ui.ok("project.set_allowed_roots", json!({ "roots": [] }))
        .await;
    let (failed, out) = mcp
        .call("media_import", json!({ "paths": [clip("d.mp4")] }))
        .await;
    assert!(failed, "{out}");
    assert_eq!(out["error"]["code"], "invalid", "{out}");
    assert_eq!(out["error"]["path"], "/paths/0", "{out}");
    assert!(
        out["error"]["message"].as_str().unwrap().contains(SETTING),
        "{out}"
    );
    let refused = script
        .call("media.import", json!({ "paths": [clip("d.mp4")] }))
        .await;
    assert_eq!(refused["error"]["data"]["code"], "invalid", "{refused}");
    assert!(refused.to_string().contains(SETTING), "{refused}");
    let (code, out) = cli_exec(
        &run_dir,
        engine.pid,
        &json!({ "cmd": "media.import", "args": { "paths": [clip("d.mp4")] } }),
    );
    assert_eq!(code, Some(1), "{out}");
    assert!(out.to_string().contains(SETTING), "{out}");
    // Three imports went through, one per agent.
    let media = ui.ok("media.list", json!({})).await;
    assert_eq!(media["total"], 3, "{media}");

    // Closing the session ends the connection: announced with its last command.
    mcp.close().await;
    let closed = ui
        .notification("project.connection_closed", |p| p["actor"] == "agent:mcp")
        .await;
    assert_eq!(closed["last_command"], "media.import", "{closed}");
    script.close().await;
    ui.notification("project.connection_closed", |p| p["id"] == script_id)
        .await;
    // Every agent is gone (the one-shot CLIs closed when they exited).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let listed = ui.ok("project.connections", json!({})).await;
        if listed["connections"] == json!([]) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "{listed}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    ui.close().await;
    drop(engine);
}
