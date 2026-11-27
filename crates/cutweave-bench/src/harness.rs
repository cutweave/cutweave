// SPDX-License-Identifier: GPL-3.0-or-later
//! What the probes drive: an engine in this process ([`BenchEngine`], core only, serving
//! `/rpc`), a JSON-RPC WebSocket client ([`WsClient`]) and a real engine process started through
//! the launch protocol ([`ChildEngine`]).

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use cutweave_api::rpc::RpcConfig;
use cutweave_api::{Credential, RunFile, Token};
use cutweave_engine::{Call, Connection, Engine, EngineConfig, EngineMode, rpc_mounts};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use crate::BenchError;

/// How long a probe waits for any one answer before it gives up.
pub const WAIT: Duration = Duration::from_secs(60);
/// How long an engine process may take to say it is ready. Generous: on a machine whose GPU
/// is busy the full engine has been seen to take a minute, and the probe should report that
/// number, not fail.
pub const READY_WAIT: Duration = Duration::from_secs(240);

fn other(what: &str, e: impl std::fmt::Display) -> BenchError {
    BenchError::Other(format!("{what}: {e}"))
}

/// A headless core engine in this process, serving `/rpc` on a free port.
pub struct BenchEngine {
    engine: Option<Engine>,
    /// The server's port.
    pub port: u16,
    token: String,
}

impl BenchEngine {
    /// Starts an engine whose callers may use paths in `roots`; its run-file goes to
    /// `<scratch>/run`. `project`: the folder to open at start.
    pub fn start(
        scratch: &Path,
        roots: &[PathBuf],
        project: Option<&Path>,
    ) -> Result<BenchEngine, BenchError> {
        let mut config = EngineConfig::new(EngineMode::Headless);
        config.port = Some(0);
        config.project = project.map(Path::to_path_buf);
        config.api.run_dir = Some(scratch.join("run"));
        config.api.mounts = Some(rpc_mounts(RpcConfig::default()));
        config.allowed_roots = roots.to_vec();
        let engine = Engine::start(config).map_err(|e| other("start the engine", e))?;
        let token = engine
            .credentials()
            .and_then(|c| c.agent_tokens().iter().find(|t| t.label == "cli"))
            .map(|t| t.token.expose().to_owned())
            .ok_or_else(|| BenchError::Other("the engine has no `cli` agent token".into()))?;
        let port = engine
            .port()
            .ok_or_else(|| BenchError::Other("the engine serves no port".into()))?;
        Ok(BenchEngine {
            engine: Some(engine),
            port,
            token,
        })
    }

    /// An in-process connection with the `cli` agent's access.
    pub fn connect(&self) -> Result<Connection, BenchError> {
        self.engine
            .as_ref()
            .ok_or_else(|| BenchError::Other("the engine stopped".into()))?
            .handle()
            .connect(&Credential::Agent("cli".into()))
            .map_err(|e| other("connect", e))
    }

    /// A WebSocket client of this engine.
    pub fn ws(&self) -> Result<WsClient, BenchError> {
        WsClient::connect(self.port, &self.token)
    }
}

impl Drop for BenchEngine {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            engine.shutdown();
        }
    }
}

/// Calls `name` on an in-process connection and returns its output.
pub fn call(conn: &Connection, name: &str, params: Value) -> Result<Value, BenchError> {
    conn.call(Call::new(name, params))
        .map(|reply| reply.output)
        .map_err(|e| BenchError::command(name, &e))
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A JSON-RPC client on one WebSocket connection (blocking: it owns a small runtime).
pub struct WsClient {
    rt: Runtime,
    ws: Socket,
    next_id: u64,
    /// The newest revision a `doc.patched` notification announced.
    patched: u64,
}

impl WsClient {
    /// Connects to `ws://127.0.0.1:<port>/rpc` with a bearer token.
    pub fn connect(port: u16, token: &str) -> Result<WsClient, BenchError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| other("start a runtime", e))?;
        let mut request = format!("ws://127.0.0.1:{port}/rpc")
            .into_client_request()
            .map_err(|e| other("build the request", e))?;
        let bearer = format!("Bearer {token}")
            .parse()
            .map_err(|_| BenchError::Other("the token is not a header value".into()))?;
        request.headers_mut().insert("Authorization", bearer);
        let (ws, _) = rt
            .block_on(async { tokio::time::timeout(WAIT, connect_async(request)).await })
            .map_err(|_| BenchError::Other("the engine did not accept the connection".into()))?
            .map_err(|e| other("connect to /rpc", e))?;
        Ok(WsClient {
            rt,
            ws,
            next_id: 1,
            patched: 0,
        })
    }

    /// The next JSON message.
    fn next(&mut self) -> Result<Value, BenchError> {
        self.next_within(WAIT)
    }

    fn next_within(&mut self, wait: Duration) -> Result<Value, BenchError> {
        let ws = &mut self.ws;
        self.rt.block_on(async {
            loop {
                let msg = tokio::time::timeout(wait, ws.next())
                    .await
                    .map_err(|_| BenchError::Other("no message from the engine in time".into()))?;
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        return serde_json::from_str(text.as_str())
                            .map_err(|e| other("a message that is not JSON", e));
                    }
                    Some(Ok(Message::Close(frame))) => {
                        return Err(BenchError::Other(format!(
                            "the engine closed the connection: {frame:?}"
                        )));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(other("the connection failed", e)),
                    None => return Err(BenchError::Other("the connection ended".into())),
                }
            }
        })
    }

    fn note(&mut self, msg: &Value) {
        if msg["method"] == "doc.patched"
            && let Some(rev) = msg["params"]["rev"].as_u64()
        {
            self.patched = self.patched.max(rev);
        }
    }

    /// Sends a request without waiting for its answer and returns its JSON-RPC id: several
    /// requests can be in flight at once (the crash harness kills the engine under them).
    /// Read the answers with [`next_message`](Self::next_message).
    pub fn send(&mut self, method: &str, params: &Value) -> Result<u64, BenchError> {
        let id = self.next_id;
        self.next_id += 1;
        let text =
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        let ws = &mut self.ws;
        self.rt
            .block_on(ws.send(Message::text(text)))
            .map_err(|e| other("send", e))?;
        Ok(id)
    }

    /// The next message of the engine, a response or a notification, waiting at most `wait`
    /// (at most [`WAIT`]). An error once the connection is gone.
    pub fn next_message(&mut self, wait: Duration) -> Result<Value, BenchError> {
        let msg = self.next_within(wait.min(WAIT))?;
        self.note(&msg);
        Ok(msg)
    }

    /// Calls `method` and returns its result (with `_meta`).
    pub fn call(&mut self, method: &str, params: &Value) -> Result<Value, BenchError> {
        let id = self.send(method, params)?;
        loop {
            let msg = self.next()?;
            if msg.get("id").is_none() {
                self.note(&msg);
                continue;
            }
            if msg["id"].as_u64() != Some(id) {
                continue; // the late answer of an earlier `send`
            }
            if let Some(error) = msg.get("error") {
                return Err(BenchError::command(method, error));
            }
            return Ok(msg["result"].clone());
        }
    }

    /// Calls a mutating `method` (with a request id of its own) and returns its result.
    pub fn mutate(&mut self, method: &str, mut params: Value) -> Result<Value, BenchError> {
        // UUIDv7-shaped and unique per client and call; the doc actor deduplicates on it.
        let id = format!(
            "0190f5c2-{:04x}-7000-8000-{:012x}",
            std::process::id() & 0xFFFF,
            (u64::from(self.ws_port()) << 32) | self.next_id
        );
        if let Value::Object(map) = &mut params {
            map.insert("_meta".into(), json!({"request_id": id}));
        }
        self.call(method, &params)
    }

    fn ws_port(&self) -> u16 {
        match self.ws.get_ref() {
            MaybeTlsStream::Plain(tcp) => tcp.local_addr().map_or(0, |a| a.port()),
            _ => 0,
        }
    }

    /// Subscribes to `doc.patched`.
    pub fn subscribe_patches(&mut self) -> Result<(), BenchError> {
        self.call("rpc.subscribe", &json!({"events": ["doc.patched"]}))
            .map(|_| ())
    }

    /// Waits until the `doc.patched` for revision `rev` (or a later one) arrived.
    pub fn wait_patched(&mut self, rev: u64) -> Result<(), BenchError> {
        while self.patched < rev {
            let msg = self.next()?;
            self.note(&msg);
        }
        Ok(())
    }

    /// One full round trip of a mutating command: the request, its response and the
    /// `doc.patched` it caused (the connection must have subscribed).
    pub fn round_trip(&mut self, method: &str, params: Value) -> Result<(), BenchError> {
        let result = self.mutate(method, params)?;
        let rev = result["_meta"]["rev"]
            .as_u64()
            .ok_or_else(|| BenchError::Other(format!("`{method}` returned no revision")))?;
        self.wait_patched(rev)
    }
}

/// A real engine process (`cutweave-bench engine …`, the launch protocol of
/// `docs/reference/launch-protocol.md`), killed on drop.
pub struct ChildEngine {
    child: Child,
    /// Kept open: a `Gui` engine stops when its stdin ends.
    _stdin: Option<ChildStdin>,
    /// Its port.
    pub port: u16,
    token: String,
    mcp_token: String,
    /// What it prints after the `ready` line; it ends when the process does.
    lines: Receiver<String>,
    /// From the spawn to the `ready` line.
    pub ready_after: Duration,
}

/// How to start a [`ChildEngine`].
#[derive(Clone, Debug)]
pub struct ChildSpec {
    /// The `cutweave-bench` executable.
    pub exe: PathBuf,
    /// A folder for its run-file and caches.
    pub scratch: PathBuf,
    /// The project to open at start.
    pub project: Option<PathBuf>,
    /// `--dev-stub-media`: no decoders, GPU or audio (the smoke run).
    pub stub_media: bool,
    /// Start it the way the app does (`Gui` mode: the UI token over stdin, the GPU and the
    /// default audio output) instead of `--headless`.
    pub gui: bool,
    /// `--deterministic-ids SEED` (headless engines only).
    pub deterministic_ids: Option<u64>,
    /// Extra environment variables of the process.
    pub env: Vec<(String, String)>,
    /// More folders its agents may use (`--allow-root`), besides `scratch`.
    pub roots: Vec<PathBuf>,
}

impl ChildEngine {
    /// Starts the engine and waits for its `ready` line.
    pub fn spawn(spec: &ChildSpec) -> Result<ChildEngine, BenchError> {
        let run_dir = spec.scratch.join("run");
        let began = Instant::now();
        let mut cmd = Command::new(&spec.exe);
        cmd.args(["engine", "--port", "0", "--no-log"])
            .arg(if spec.gui {
                "--token-stdin"
            } else {
                "--headless"
            })
            .arg("--run-dir")
            .arg(&run_dir)
            .arg("--cache-dir")
            .arg(spec.scratch.join("cache"))
            .arg("--allow-root")
            .arg(&spec.scratch);
        if let Some(project) = &spec.project {
            cmd.arg("--project").arg(project);
        }
        if spec.stub_media {
            cmd.arg("--dev-stub-media");
        }
        if let Some(seed) = spec.deterministic_ids {
            cmd.arg("--deterministic-ids").arg(seed.to_string());
        }
        cmd.envs(spec.env.iter().map(|(k, v)| (k, v)));
        for root in &spec.roots {
            cmd.arg("--allow-root").arg(root);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| BenchError::io("start", &spec.exe, e))?;
        let mut stdin = child.stdin.take();
        if spec.gui {
            // The handshake: one line with a fresh UI token (never logged, never reused).
            let sent = Token::generate()
                .map_err(|e| other("make a token", e))
                .and_then(|token| {
                    let pipe = stdin
                        .as_mut()
                        .ok_or_else(|| BenchError::Other("the engine has no stdin".into()))?;
                    writeln!(pipe, "{}", token.expose())
                        .and_then(|()| pipe.flush())
                        .map_err(|e| other("send the handshake", e))
                });
            if let Err(e) = sent {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        }
        let lines = child.stdout.take().map(stdout_lines);
        let ready = lines
            .ok_or_else(|| BenchError::Other("the engine has no stdout".into()))
            .and_then(|lines| wait_ready(&lines).map(|port| (port, lines)));
        let (port, lines) = match ready {
            Ok(ready) => ready,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        let ready_after = began.elapsed();
        let pid = child.id();
        let tokens = RunFile::discover(&run_dir)
            .into_iter()
            .find(|f| f.pid == pid)
            .map(|f| f.agent_tokens)
            .unwrap_or_default();
        let token = |label: &str| {
            tokens
                .iter()
                .find(|t| t.label == label)
                .map(|t| t.token.clone())
        };
        let (cli, mcp) = (token("cli"), token("mcp"));
        let mut engine = ChildEngine {
            child,
            _stdin: stdin,
            port,
            token: String::new(),
            mcp_token: mcp.unwrap_or_default(),
            lines,
            ready_after,
        };
        // On an error the drop above kills the process.
        engine.token =
            cli.ok_or_else(|| BenchError::Other("the engine wrote no run-file".into()))?;
        Ok(engine)
    }

    /// The process id.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The `cli` agent's token (`/rpc`).
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The `mcp` agent's token (`/mcp`); empty when the run-file named none.
    #[must_use]
    pub fn mcp_token(&self) -> &str {
        &self.mcp_token
    }

    /// Kills the process now (`TerminateProcess` / `SIGKILL`: none of its cleanup runs) and
    /// reaps it.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Waits at most `wait` for the process to end by itself (its stdout closes) and reaps
    /// it. `false`: it is still running.
    pub fn wait_exit(&mut self, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => return false,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = self.child.wait();
                    return true;
                }
            }
        }
    }

    /// A WebSocket client of this engine.
    pub fn ws(&self) -> Result<WsClient, BenchError> {
        WsClient::connect(self.port, &self.token)
    }

    /// The memory the process holds now (resident set / working set), in megabytes, as the
    /// operating system's own tool reports it.
    pub fn resident_mb(&self) -> Result<f64, BenchError> {
        resident_kb(self.child.id()).map(|kb| kb / 1024.0)
    }
}

impl Drop for ChildEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn stdout_lines(stdout: std::process::ChildStdout) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// The port of the `ready` line; an `error` line or the end of stdout fails.
fn wait_ready(lines: &Receiver<String>) -> Result<u16, BenchError> {
    let deadline = Instant::now() + READY_WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = lines
            .recv_timeout(left)
            .map_err(|_| BenchError::Other("the engine did not become ready".into()))?;
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match event["event"].as_str() {
            Some("ready") => {
                return event["port"]
                    .as_u64()
                    .and_then(|p| u16::try_from(p).ok())
                    .ok_or_else(|| BenchError::Other("the ready line has no port".into()));
            }
            Some("error") => {
                return Err(BenchError::Other(format!("the engine failed: {line}")));
            }
            _ => {}
        }
    }
}

/// Resident memory of process `pid` in kilobytes: `tasklist` on Windows, `ps` elsewhere.
fn resident_kb(pid: u32) -> Result<f64, BenchError> {
    let pid_text = pid.to_string();
    let output = if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid_text}"), "/FO", "CSV", "/NH"])
            .output()
    } else {
        Command::new("ps")
            .args(["-o", "rss=", "-p", &pid_text])
            .output()
    }
    .map_err(|e| other("ask the OS for the memory of the engine", e))?;
    let text = String::from_utf8_lossy(&output.stdout);
    // Windows: `"name","pid","session","n","123,456 K"` (the grouping follows the locale).
    let field = if cfg!(windows) {
        text.lines()
            .find(|l| l.contains(&format!("\"{pid_text}\"")))
            .and_then(|l| l.rsplit("\",\"").next())
            .unwrap_or("")
    } else {
        text.trim()
    };
    let digits: String = field.chars().filter(char::is_ascii_digit).collect();
    digits
        .parse::<f64>()
        .map_err(|_| BenchError::Other(format!("no memory figure for process {pid} in `{text}`")))
}
