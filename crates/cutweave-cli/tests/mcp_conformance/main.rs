// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::panic,
    missing_docs
)]
//! WP-1O.03: the rmcp-based MCP conformance suite and the scripted-agent e2e, against the real
//! `cutweave` binary (arch §14, ADR-0009, R31).
//!
//! # Matrix
//!
//! One conformance run per (transport, revision), four in all:
//!
//! | transport | how the server is reached |
//! |---|---|
//! | stdio | `cutweave mcp --headless` child, rmcp client over its pipes (actor `agent:mcp`) |
//! | HTTP | `cutweave engine --headless` child, rmcp Streamable HTTP client on `/mcp` with the run-file's `mcp` bearer token |
//!
//! | revision | handshake |
//! |---|---|
//! | 2025-11-25 | `initialize`, `ping` |
//! | 2026-07-28 | `server/discover`, per-request `_meta` (no `ping`: the revision dropped it) |
//!
//! Each run checks `tools/list` (the P1 budget and the exact catalog), `ping`, the absent
//! `resources` capability, then calls **every** core tool and the four generic tools on a
//! fixture project and validates each result's `structuredContent` against the command's
//! `output_schema` as served by `cutweave_describe_command` ([`schema`]). A violation panics
//! with the tool name; [`a_broken_output_schema_fails_the_suite_naming_the_tool`] proves it.
//!
//! # Scripted agent
//!
//! [`scripted_agent_edits_over_stdio_with_mcp_tools_only`]: a deterministic client (no LLM)
//! imports 3 stub clips, arranges, trims, splits, moves, reads back, and compares the final
//! timeline (ids normalized) with `tests/fixtures/scripted_agent_timeline.json`.
//!
//! Update the fixture with `CUTWEAVE_UPDATE_FIXTURES=1 cargo test -p cutweave-cli --test
//! mcp_conformance`. Run the suite with `just mcp-conformance`.

mod schema;

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cutweave_api::mcp::{ListSize, P1_GATE};
use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt as _, RoleClient, ServiceExt as _,
    model::{
        CallToolRequestParams, CallToolResult, ClientConfig, ClientRequest, ContentBlock,
        PingRequest, ProtocolVersion, ServerResult,
    },
    service::{RunningService, ServiceError},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::task::JoinHandle;

const BIN: &str = env!("CARGO_BIN_EXE_cutweave");

/// Flicks per second.
const SEC: i64 = 705_600_000;

/// The standard profile's tools in Phase 1: the registry's 15 core tools and the 4 generic
/// ones. `render_frame` (WP-1J.02) is not here yet: the count in the assertions goes to 20
/// with it (P1 gate: 19 tools; the gate is widened by the WP that adds it).
const CORE_TOOLS: [&str; 15] = [
    "clip_split",
    "clip_trim",
    "edit_undo",
    "export_start",
    "item_delete",
    "item_move",
    "item_set_props",
    "job_status",
    "job_wait",
    "media_import",
    "media_list",
    "project_info",
    "project_open",
    "timeline_get",
    "timeline_insert",
];
const GENERIC_TOOLS: [&str; 4] = [
    "cutweave_search_commands",
    "cutweave_describe_command",
    "cutweave_execute",
    "cutweave_query",
];

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transport {
    Stdio,
    Http,
}

/// A client asking for one protocol version.
#[derive(Clone)]
struct Versioned(ProtocolVersion);

impl ClientHandler for Versioned {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default().with_protocol_version(self.0.clone())
    }
}

/// A `cutweave engine --headless` child (stub media), killed on drop.
struct EngineProcess {
    child: std::process::Child,
    /// Kept open so the engine never writes to a closed pipe.
    _stdout: BufReader<std::process::ChildStdout>,
    port: u16,
    token: String,
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
            .env_remove("CUTWEAVE_DEV_STUB_MEDIA")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn engine");
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let ready: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert_eq!(ready["event"], "ready", "{line}");
        let pid = ready["pid"].as_u64().unwrap();
        let run_file = run_dir.join(format!("{pid}.json"));
        let run: Value = serde_json::from_slice(&std::fs::read(&run_file).unwrap()).unwrap();
        let token = run["agent_tokens"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["label"] == "mcp")
            .expect("the run-file has an mcp token")["token"]
            .as_str()
            .unwrap()
            .to_owned();
        EngineProcess {
            child,
            _stdout: stdout,
            port: u16::try_from(run["port"].as_u64().unwrap()).unwrap(),
            token,
        }
    }
}

impl Drop for EngineProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// An rmcp client of one server, on one transport and revision.
struct Session {
    service: RunningService<RoleClient, Versioned>,
    transport: Transport,
    revision: Revision,
    /// Holds the stub media and the saved project; the engine's allowed root.
    dir: tempfile::TempDir,
    /// The stdio child (`cutweave mcp --headless`), with its stderr.
    child: Option<(tokio::process::Child, JoinHandle<String>)>,
    /// Lines the stdio child wrote to stdout that are not JSON-RPC 2.0 messages.
    stray: Option<Arc<Mutex<Vec<String>>>>,
    /// The HTTP engine.
    _engine: Option<EngineProcess>,
}

/// Stub media files (`--dev-stub-media` probes by extension): `media/clip{1..=3}.mp4`.
fn stub_media(dir: &Path) -> Vec<String> {
    let media = dir.join("media");
    std::fs::create_dir_all(&media).unwrap();
    (1..=3_u8)
        .map(|n| {
            let clip = media.join(format!("clip{n}.mp4"));
            std::fs::write(&clip, vec![n; 4096]).unwrap();
            clip.to_string_lossy().into_owned()
        })
        .collect()
}

impl Session {
    /// Starts the server and connects. `extra` are more `cutweave mcp` arguments (stdio only).
    async fn start(transport: Transport, revision: Revision, extra: &[&str]) -> Session {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap().to_owned();
        let handler = Versioned(revision.version());
        let (service, child, engine, stray) = match transport {
            Transport::Stdio => {
                let mut child = tokio::process::Command::new(BIN)
                    .args(["mcp", "--headless", "--dev-stub-media", "--no-log"])
                    .args(["--allow-root", &root])
                    .args(extra)
                    .env_remove("CUTWEAVE_DETERMINISTIC_IDS")
                    .env_remove("CUTWEAVE_DEV_STUB_MEDIA")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
                    .expect("spawn cutweave mcp");
                let stdout = child.stdout.take().unwrap();
                let stdin = child.stdin.take().unwrap();
                let mut err = child.stderr.take().unwrap();
                let stderr = tokio::spawn(async move {
                    let mut text = String::new();
                    let _ = err.read_to_string(&mut text).await;
                    text
                });
                // Relay stdout line by line to the client, recording every line that is not a
                // JSON-RPC 2.0 message: stdout is the protocol's and nothing else.
                let stray = Arc::new(Mutex::new(Vec::new()));
                let (relay_read, mut relay_write) = tokio::io::duplex(1 << 20);
                let seen = Arc::clone(&stray);
                tokio::spawn(async move {
                    let mut lines = tokio::io::BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let frame = serde_json::from_str::<Value>(&line).ok();
                        if frame.is_none_or(|f| f["jsonrpc"] != "2.0") {
                            seen.lock().unwrap().push(line.clone());
                        }
                        if relay_write
                            .write_all(format!("{line}\n").as_bytes())
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                });
                let service = connect(handler, (relay_read, stdin), revision).await;
                (service, Some((child, stderr)), None, Some(stray))
            }
            Transport::Http => {
                let engine = EngineProcess::start(&dir.path().join("run"), dir.path());
                let config = StreamableHttpClientTransportConfig::with_uri(format!(
                    "http://127.0.0.1:{}/mcp",
                    engine.port
                ))
                .auth_header(engine.token.clone());
                let service = connect(
                    handler,
                    StreamableHttpClientTransport::from_config(config),
                    revision,
                )
                .await;
                (service, None, Some(engine), None)
            }
        };
        Session {
            service,
            transport,
            revision,
            dir,
            child,
            stray,
            _engine: engine,
        }
    }

    fn label(&self) -> String {
        format!("{:?}/{:?}", self.transport, self.revision)
    }

    fn clips(&self) -> Vec<String> {
        stub_media(self.dir.path())
    }

    fn negotiated(&self) -> ProtocolVersion {
        self.service
            .peer_info()
            .expect("peer info")
            .protocol_version
            .clone()
    }

    async fn try_call(&self, tool: &str, args: Value) -> Result<CallToolResult, ServiceError> {
        let Value::Object(args) = args else {
            panic!("args must be an object")
        };
        self.service
            .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args))
            .await
    }

    /// Calls `tool`, panicking on a tool error; returns `structuredContent`. The text content
    /// must be the human summary, never empty.
    async fn ok(&self, tool: &str, args: Value) -> Value {
        let result = self.try_call(tool, args).await.expect("tools/call");
        assert_ne!(
            result.is_error,
            Some(true),
            "{tool} failed: {}",
            text_of(&result)
        );
        assert!(!text_of(&result).is_empty(), "{tool}: no text content");
        result
            .structured_content
            .unwrap_or_else(|| panic!("{tool}: no structuredContent"))
    }

    /// Calls `tool`, panicking on success; returns the structured error.
    async fn err(&self, tool: &str, args: Value) -> Value {
        let result = self.try_call(tool, args).await.expect("tools/call");
        assert_eq!(
            result.is_error,
            Some(true),
            "{tool} unexpectedly succeeded: {}",
            text_of(&result)
        );
        result.structured_content.expect("structuredContent")
    }

    /// Closes the client; for stdio returns the child's exit code and stderr.
    async fn close(self) -> Option<(Option<i32>, String)> {
        let Session {
            service,
            child,
            stray,
            ..
        } = self;
        let _ = service.cancel().await;
        let (mut child, stderr) = child?;
        let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
            .await
            .expect("cutweave mcp exits when its stdin closes")
            .expect("wait");
        // Let the relay drain the child's last output.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let stray = stray.expect("stdio session").lock().unwrap().clone();
        assert!(stray.is_empty(), "non-protocol output on stdout: {stray:?}");
        Some((status.code(), stderr.await.unwrap_or_default()))
    }
}

async fn connect<T, E, A>(
    handler: Versioned,
    transport: T,
    revision: Revision,
) -> RunningService<RoleClient, Versioned>
where
    T: rmcp::transport::IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    match revision {
        Revision::Stateful => handler.serve(transport).await.expect("initialize"),
        Revision::Stateless => handler
            .serve_with_lifecycle(
                transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![revision.version()],
                },
            )
            .await
            .expect("server/discover"),
    }
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ----- output schema checking ---------------------------------------------------------------

/// Edits the output schema of a command before it is used (the negative test's fixture).
type Tamper = fn(command: &str, schema: &mut Value);

/// Calls tools and validates what they return against the command's `output_schema`, which it
/// asks the server for with `cutweave_describe_command` (cached). A violation panics with the
/// tool name.
struct Checker<'a> {
    session: &'a Session,
    tamper: Option<Tamper>,
    schemas: Mutex<HashMap<String, Value>>,
    called: Mutex<BTreeSet<String>>,
}

/// `clip_split` -> `clip.split` (no namespace contains an underscore).
fn command_of(tool: &str) -> String {
    tool.replacen('_', ".", 1)
}

impl<'a> Checker<'a> {
    fn new(session: &'a Session, tamper: Option<Tamper>) -> Self {
        Checker {
            session,
            tamper,
            schemas: Mutex::new(HashMap::new()),
            called: Mutex::new(BTreeSet::new()),
        }
    }

    async fn output_schema(&self, command: &str) -> Value {
        if let Some(schema) = self.schemas.lock().unwrap().get(command) {
            return schema.clone();
        }
        let described = self
            .session
            .ok("cutweave_describe_command", json!({ "name": command }))
            .await;
        assert_eq!(described["name"], command, "{described}");
        let mut schema = described["output_schema"].clone();
        assert!(
            schema.is_object(),
            "`{command}` has no output_schema: {described}"
        );
        if let Some(tamper) = self.tamper {
            tamper(command, &mut schema);
        }
        self.schemas
            .lock()
            .unwrap()
            .insert(command.to_owned(), schema.clone());
        schema
    }

    /// Validates `output` as the result of `tool` (command `command`).
    async fn validate(&self, tool: &str, command: &str, output: &Value) {
        let schema = self.output_schema(command).await;
        if let Err(violations) = schema::check(&schema, output) {
            panic!(
                "output schema violation in tool `{tool}` (command `{command}`) on {}: \
                 {violations}\nresult: {output}",
                self.session.label()
            );
        }
    }

    /// A core tool call: success, and the result matches the command's output schema.
    async fn tool(&self, tool: &str, args: Value) -> Value {
        let out = self.session.ok(tool, args).await;
        self.validate(tool, &command_of(tool), &out).await;
        self.called.lock().unwrap().insert(tool.to_owned());
        out
    }

    /// `cutweave_execute`: every step's output matches its command's output schema.
    async fn execute(&self, steps: Value) -> Value {
        let out = self
            .session
            .ok("cutweave_execute", json!({ "steps": steps }))
            .await;
        for step in out["steps"].as_array().expect("steps") {
            let cmd = step["cmd"].as_str().expect("step cmd");
            self.validate("cutweave_execute", cmd, &step["output"])
                .await;
        }
        self.called
            .lock()
            .unwrap()
            .insert("cutweave_execute".into());
        out
    }

    /// `cutweave_query`: the output matches the query's output schema.
    async fn query(&self, name: &str, args: Value) -> Value {
        let out = self
            .session
            .ok("cutweave_query", json!({ "name": name, "args": args }))
            .await;
        // `cutweave_query` returns the query's output as its structured content.
        self.validate("cutweave_query", name, &out).await;
        self.called.lock().unwrap().insert("cutweave_query".into());
        out
    }
}

// ----- the conformance run ------------------------------------------------------------------

/// `ping` on 2025-11-25; the 2026-07-28 revision has none, so the server must refuse it (and
/// stay usable).
async fn check_ping(session: &Session) {
    let ping = session
        .service
        .send_request(ClientRequest::PingRequest(PingRequest::default()))
        .await;
    match session.revision {
        Revision::Stateful => {
            assert!(
                matches!(ping, Ok(ServerResult::EmptyResult(_))),
                "{}: ping: {ping:?}",
                session.label()
            );
        }
        Revision::Stateless => {
            // 2026-07-28 dropped `ping`; the server still answers one (or refuses it), and
            // either way the session stays usable. Liveness there is any request.
            println!("{}: ping on 2026-07-28: {ping:?}", session.label());
            session.service.list_tools(None).await.expect("alive");
        }
    }
}

async fn conformance(transport: Transport, revision: Revision, tamper: Option<Tamper>) {
    let session = Session::start(transport, revision, &[]).await;
    let label = session.label();
    assert_eq!(session.negotiated(), revision.version(), "{label}");

    check_ping(&session).await;

    // `tools/list`: the exact Phase 1 catalog, within the P1 budget.
    let tools = session.service.list_tools(None).await.unwrap().tools;
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    let mut expected: Vec<String> = CORE_TOOLS
        .iter()
        .chain(&GENERIC_TOOLS)
        .map(|s| (*s).to_owned())
        .collect();
    expected.sort();
    assert_eq!(
        names, expected,
        "{label}: tools/list changed; render_frame (WP-1J.02) joins it"
    );
    let size = ListSize::of(&tools);
    assert!(
        P1_GATE.fits(size),
        "{label}: {} tools / ~{} tokens exceeds the P1 gate (<= 19 / 5k)",
        size.tools,
        size.tokens()
    );
    println!(
        "{label}: negotiated {}, {} tools, ~{} tokens",
        session.negotiated(),
        size.tools,
        size.tokens()
    );
    for tool in &tools {
        assert!(
            tool.description.as_deref().is_some_and(|d| !d.is_empty()),
            "{label}: `{}` has no description",
            tool.name
        );
        assert_eq!(
            tool.input_schema.get("type"),
            Some(&json!("object")),
            "{label}: `{}`",
            tool.name
        );
        assert!(tool.annotations.is_some(), "{label}: `{}`", tool.name);
    }

    // The server exposes no resources yet (WP-1J.02 may add some): the capability is absent.
    // TODO(WP-1J.02): when `resources` appears, run `resources/list` and `resources/read` here
    // and validate each resource's content type.
    let info = session.service.peer_info().unwrap();
    assert!(
        info.capabilities.resources.is_none(),
        "{label}: the server now offers resources; add resources/list and read checks"
    );
    assert!(info.capabilities.tools.is_some(), "{label}");

    exercise(&session, tamper).await;

    if let Some((code, stderr)) = session.close().await {
        assert_eq!(code, Some(0), "{label}: {stderr}");
    }
}

/// Calls every core tool and the generic tools on a fixture project.
async fn exercise(session: &Session, tamper: Option<Tamper>) {
    let c = Checker::new(session, tamper);
    let clips = session.clips();

    // A project, via the generic tool (project.new is a catalog command, not a core tool).
    let created = c
        .execute(json!([{ "cmd": "project.new", "args": { "name": "Conformance" } }]))
        .await;
    let project = created["steps"][0]["output"].clone();
    let (sequence, video, audio) = (
        project["sequence"].clone(),
        project["video_track"].clone(),
        project["audio_track"].clone(),
    );
    let info = c.tool("project_info", json!({})).await;
    assert_eq!(info["name"], "Conformance", "{info}");

    // media_import -> job_wait / job_status
    let imported = c.tool("media_import", json!({ "paths": clips })).await;
    let media: Vec<Value> = imported["media_ids"].as_array().unwrap().clone();
    assert_eq!(media.len(), 3, "{imported}");
    let jobs = imported["job_ids"].as_array().unwrap().clone();
    for job in &jobs {
        let done = c
            .tool("job_wait", json!({ "job_id": job, "timeout_ms": 10_000 }))
            .await;
        assert_eq!(done["state"], "succeeded", "{done}");
        let status = c.tool("job_status", json!({ "id": job })).await;
        assert_eq!(status["state"], "succeeded", "{status}");
    }
    if jobs.is_empty() {
        // `--dev-stub-media` registers no analysis stages (FFmpeg-backed analysis arrives with
        // WP-1D.x), so an import queues no job: exercise both job tools on an unknown job and
        // require the structured `not_found` error. TODO(WP-1D.x): once an import queues
        // analysis jobs this branch is dead; the loop above validates `job_wait`/`job_status`
        // results against their output schemas.
        let unknown_job = format!("job_{}", "0".repeat(32));
        for (tool, args) in [
            (
                "job_wait",
                json!({ "job_id": unknown_job, "timeout_ms": 100 }),
            ),
            ("job_status", json!({ "id": unknown_job })),
        ] {
            let error = session.err(tool, args).await;
            assert_eq!(error["error"]["code"], "not_found", "{tool}: {error}");
            c.called.lock().unwrap().insert(tool.to_owned());
        }
    }
    let listed = c.tool("media_list", json!({})).await;
    assert!(listed.to_string().contains("clip1.mp4"), "{listed}");

    // timeline_insert x2, timeline_get
    let first = c
        .tool(
            "timeline_insert",
            json!({ "sequence": sequence, "track": video, "media": media[0], "at": 0 }),
        )
        .await;
    let second = c
        .tool(
            "timeline_insert",
            json!({
                "sequence": sequence, "track": video, "media": media[1],
                "at": first["duration"]
            }),
        )
        .await;
    let tl = c
        .tool("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
        .await;
    assert_eq!(
        tl["duration"],
        first["duration"].as_i64().unwrap() + second["duration"].as_i64().unwrap(),
        "{tl}"
    );
    let _ = audio;

    // clip_trim, clip_split, item_move, item_set_props, item_delete, edit_undo
    let a = first["clips"][0].clone();
    let b = second["clips"][0].clone();
    c.tool(
        "clip_trim",
        json!({ "item": a, "edge": "end", "delta": -SEC }),
    )
    .await;
    let split = c
        .tool("clip_split", json!({ "item": b, "at": "12s" }))
        .await;
    let right = split["right"].clone();
    c.tool("item_move", json!({ "item": right, "start": "40s" }))
        .await;
    c.tool(
        "item_set_props",
        json!({ "item": right, "props": { "name": "renamed", "opacity": 0.5 } }),
    )
    .await;
    c.tool("item_delete", json!({ "items": [right] })).await;
    c.tool("edit_undo", json!({})).await;

    // The undo history (before project_open replaces the document): every edit is the agent's.
    let history = c.query("edit.history", json!({})).await;
    assert!(
        history["entries"].as_array().unwrap().len() >= 5,
        "{history}"
    );
    assert!(
        history["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["actor"] == "agent:mcp"),
        "{history}"
    );

    // export_start: this engine runs without the media subsystem (WP-1I.05 wires it), so a
    // valid export is refused with the structured `unsupported` error after validation.
    let exported = session
        .dir
        .path()
        .join("export.mp4")
        .to_string_lossy()
        .into_owned();
    let error = session
        .err("export_start", json!({ "path": exported }))
        .await;
    assert_eq!(error["error"]["code"], "unsupported", "{error}");
    c.called.lock().unwrap().insert("export_start".into());

    // project_open: save, then reopen the saved folder.
    let saved_at = session
        .dir
        .path()
        .join("saved")
        .to_string_lossy()
        .into_owned();
    c.execute(json!([{ "cmd": "project.save_as", "args": { "path": saved_at } }]))
        .await;
    let opened = c.tool("project_open", json!({ "path": saved_at })).await;
    assert_eq!(opened["project"]["name"], "Conformance", "{opened}");

    // The generic tools.
    let found = session
        .ok("cutweave_search_commands", json!({ "query": "split" }))
        .await;
    assert!(found.to_string().contains("clip.split"), "{found}");
    let described = session
        .ok("cutweave_describe_command", json!({ "name": "clip.split" }))
        .await;
    assert!(described["output_schema"].is_object(), "{described}");
    c.query("project.info", json!({})).await;
    c.execute(json!([{ "cmd": "edit.locks", "args": {} }]))
        .await;

    // Every tool of the catalog was called.
    let mut called = c.called.lock().unwrap().clone();
    called.extend([
        "cutweave_search_commands".into(),
        "cutweave_describe_command".into(),
    ]);
    let missing: Vec<&str> = CORE_TOOLS
        .iter()
        .chain(&GENERIC_TOOLS)
        .copied()
        .filter(|t| !called.contains(*t))
        .collect();
    assert!(missing.is_empty(), "tools never called: {missing:?}");
    // TODO(WP-1J.02): `render_frame` on the fixture project, decode the image content block.
    // TODO(WP-1I.05): `export_start` + `job_wait` on the fixture project once the CLI engine
    // exports (it answers `unsupported` above until then).
}

#[tokio::test]
async fn conformance_stdio_2025_11_25() {
    conformance(Transport::Stdio, Revision::Stateful, None).await;
}

#[tokio::test]
async fn conformance_stdio_2026_07_28() {
    conformance(Transport::Stdio, Revision::Stateless, None).await;
}

#[tokio::test]
async fn conformance_http_2025_11_25() {
    conformance(Transport::Http, Revision::Stateful, None).await;
}

#[tokio::test]
async fn conformance_http_2026_07_28() {
    conformance(Transport::Http, Revision::Stateless, None).await;
}

/// Acceptance: breaking the output schema of one tool makes the suite fail, with a message
/// naming the tool. The break is a test-only fixture applied to the schema the suite fetches
/// (`media.list` must carry a property its results do not have); production code is untouched.
#[tokio::test]
async fn a_broken_output_schema_fails_the_suite_naming_the_tool() {
    fn break_media_list(command: &str, schema: &mut Value) {
        if command == "media.list" {
            let required = schema
                .as_object_mut()
                .unwrap()
                .entry("required")
                .or_insert_with(|| json!([]));
            required
                .as_array_mut()
                .unwrap()
                .push(json!("no_such_field"));
        }
    }
    let run = tokio::spawn(conformance(
        Transport::Stdio,
        Revision::Stateful,
        Some(break_media_list),
    ));
    let error = run.await.expect_err("the suite must fail");
    assert!(error.is_panic());
    let panic = error.into_panic();
    let message = panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default();
    assert!(
        message.contains("`media_list`") && message.contains("no_such_field"),
        "the failure must name the tool and the violation: {message}"
    );
}

// ----- the scripted agent -------------------------------------------------------------------

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Replaces every entity id (`itm_<32 hex>`, ...) with `<prefix>#<n>`, n in order of first
/// appearance, so the comparison does not depend on id generation.
fn normalize_ids(value: &Value, seen: &mut HashMap<String, String>) -> Value {
    match value {
        Value::String(s) => {
            let prefixes = ["med_", "seq_", "trk_", "itm_", "grp_"];
            let is_id = prefixes.iter().any(|p| {
                s.strip_prefix(p)
                    .is_some_and(|h| h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            });
            if is_id {
                let n = seen.len();
                let short = seen
                    .entry(s.clone())
                    .or_insert_with(|| format!("{}#{n}", &s[..3]));
                Value::String(short.clone())
            } else {
                value.clone()
            }
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| normalize_ids(v, seen)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), normalize_ids(v, seen)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

/// The M1 exit-2 flow, as far as the tools exist: a deterministic client over `cutweave mcp
/// --headless` imports three clips, arranges, trims, splits, moves, reads the timeline back.
/// It uses MCP tools only, and asserts: stdout carries protocol frames only (rmcp would fail
/// the session on anything else, and the child exits 0), the final timeline equals the JSON
/// fixture, and every edit is attributed to `agent:mcp`.
#[tokio::test]
async fn scripted_agent_edits_over_stdio_with_mcp_tools_only() {
    for revision in Revision::ALL {
        let agent = Session::start(Transport::Stdio, revision, &[]).await;
        let clips = agent.clips();
        let created = agent
            .ok(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": "project.new", "args": { "name": "Scripted cut" } }] }),
            )
            .await;
        let project = created["steps"][0]["output"].clone();
        let (sequence, video) = (project["sequence"].clone(), project["video_track"].clone());

        // 1. import 3 clips, wait for analysis.
        let imported = agent.ok("media_import", json!({ "paths": clips })).await;
        let media = imported["media_ids"].as_array().unwrap().clone();
        assert_eq!(media.len(), 3, "{imported}");
        for job in imported["job_ids"].as_array().unwrap() {
            let done = agent
                .ok("job_wait", json!({ "job_id": job, "timeout_ms": 10_000 }))
                .await;
            assert_eq!(done["state"], "succeeded", "{done}");
        }

        // 2. arrange: the clips back to back on V1.
        let mut at = json!(0);
        let mut placed = Vec::new();
        for m in &media {
            let inserted = agent
                .ok(
                    "timeline_insert",
                    json!({ "sequence": sequence, "track": video, "media": m, "at": at }),
                )
                .await;
            placed.push(inserted["clips"][0].clone());
            at = json!(at.as_i64().unwrap() + inserted["duration"].as_i64().unwrap());
        }

        // 3. trim the first clip's tail by one second, split the second at 15 s, move the
        //    third to 40 s.
        agent
            .ok(
                "clip_trim",
                json!({ "item": placed[0], "edge": "end", "delta": -SEC }),
            )
            .await;
        agent
            .ok("clip_split", json!({ "item": placed[1], "at": "15s" }))
            .await;
        agent
            .ok("item_move", json!({ "item": placed[2], "start": "40s" }))
            .await;

        // 4. read back.
        let timeline = agent
            .ok("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
            .await;
        let normalized = normalize_ids(&timeline, &mut HashMap::new());
        let path = fixture_path("scripted_agent_timeline.json");
        if std::env::var_os("CUTWEAVE_UPDATE_FIXTURES").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let pretty = serde_json::to_string_pretty(&normalized).unwrap();
            std::fs::write(&path, format!("{pretty}\n")).unwrap();
        }
        let expected: Value = serde_json::from_slice(
            &std::fs::read(&path).expect("fixture (CUTWEAVE_UPDATE_FIXTURES=1 writes it)"),
        )
        .unwrap();
        assert_eq!(
            normalized,
            expected,
            "{revision:?}: the final timeline differs from {}",
            path.display()
        );

        // 5. the actor of every edit is `agent:mcp`.
        let history = agent
            .ok(
                "cutweave_query",
                json!({ "name": "edit.history", "args": {} }),
            )
            .await;
        let entries = history["entries"]
            .as_array()
            .unwrap_or_else(|| panic!("{history}"));
        assert!(entries.len() >= 6, "{history}");
        assert!(
            entries.iter().all(|e| e["actor"] == "agent:mcp"),
            "{history}"
        );

        // TODO(WP-1J.02): `render_frame` at a time inside the second clip; decode the image
        // content block (non-empty PNG, expected size).
        // TODO(WP-1I.05): `export_start` + `job_wait`; check the output file exists.

        // Nothing but protocol frames on stdout: rmcp would have failed on anything else; the
        // child exits 0 with only diagnostics on stderr.
        let (code, stderr) = agent.close().await.unwrap();
        assert_eq!(code, Some(0), "{stderr}");
        assert!(stderr.contains("dev stub media"), "{stderr}");
    }
}
