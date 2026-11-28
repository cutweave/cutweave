// SPDX-License-Identifier: GPL-3.0-or-later
//! The MCP surface: an rmcp client speaking to the handler `cutweave mcp --headless` serves,
//! over stdio pipes (`cutweave_api::mcp::stdio::serve`, the function its `serve_stdio` wraps),
//! with the fixed credential `agent:mcp` and the standard profile.
//!
//! [`Mcp::spawn`] runs the real thing instead: `cutweave mcp --headless --deterministic-ids 42
//! --dev-stub-media --allow-root <root> --no-log` as a child process (this test binary as
//! `cutweave`, see [`crate::cli`]), the client on its stdin and stdout. Its engine is inside the
//! child, so the harness cannot observe its `doc.patched` events.
//!
//! A call goes where an agent would send it (`catalog::how_to_call`): the core tool when the
//! standard profile lists it (`clip_split`), `cutweave_query` for a read-only entry, otherwise
//! `cutweave_execute` with one step. A batch is one `cutweave_execute`.

use std::cell::Cell;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use cutweave_api::Credential;
use cutweave_api::mcp::stdio::{self, Ended, StdioError};
use cutweave_api::mcp::{Catalog, CutweaveMcp, Dispatch, Profile};
use cutweave_engine::Engine;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::mcp_support::{Client, Revision, client_over, text_of};
use crate::script::{Call, Outcome, StepError};

/// The server end of a session.
enum Server {
    /// `stdio::serve` on this runtime.
    InProcess(JoinHandle<Result<Ended, StdioError>>),
    /// A `cutweave mcp --headless` child.
    Process(Box<tokio::process::Child>),
}

/// An MCP session over stdio.
pub struct Mcp {
    rt: Runtime,
    client: Option<Client>,
    server: Option<Server>,
    catalog: Arc<Catalog>,
    /// The revision the last successful call reported: a listed tool's `_meta`
    /// (`io.cutweave/rev`), `cutweave_execute`'s `rev`.
    rev: Cell<Option<u64>>,
}

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime")
}

impl Mcp {
    /// Serves `engine` the way `cutweave mcp --headless` does and connects a client speaking
    /// `revision`.
    pub fn connect(engine: &Engine, revision: Revision) -> Mcp {
        let rt = runtime();
        let handle = engine.handle();
        let catalog = Arc::new(Catalog::new(handle.registry(), Profile::Standard));
        let handler = CutweaveMcp::new(
            Arc::new(handle) as Arc<dyn Dispatch>,
            Arc::clone(&catalog),
            Some(Credential::Agent("mcp".into())),
        );
        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let (read, write) = tokio::io::split(server_io);
        let server = rt.spawn(stdio::serve(
            handler,
            read,
            write,
            std::future::pending::<()>(),
        ));
        let client = rt.block_on(client_over(client_io, revision));
        Mcp {
            rt,
            client: Some(client),
            server: Some(Server::InProcess(server)),
            catalog,
            rev: Cell::new(None),
        }
    }

    /// Starts `cutweave mcp --headless` confined to `root` as a child process and connects a
    /// client speaking `revision` over its stdio.
    pub fn spawn(root: &Path, revision: Revision) -> Mcp {
        use tokio::io::AsyncReadExt as _;

        let rt = runtime();
        let args: Vec<String> = vec![
            "mcp".into(),
            "--headless".into(),
            "--deterministic-ids".into(),
            crate::engine::SEED.to_string(),
            "--dev-stub-media".into(),
            "--allow-root".into(),
            root.to_string_lossy().into_owned(),
            "--no-log".into(),
        ];
        let (child, client) = rt.block_on(async {
            let mut child = tokio::process::Command::new(crate::cli::test_binary())
                .args(crate::cli::CHILD_ARGS)
                .env(crate::cli::ARGS_ENV, json!(args).to_string())
                .env_remove("CUTWEAVE_DETERMINISTIC_IDS")
                .env_remove("CUTWEAVE_DEV_STUB_MEDIA")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .expect("spawn cutweave mcp --headless");
            let mut stdout = child.stdout.take().expect("stdout");
            let stdin = child.stdin.take().expect("stdin");
            // Libtest's preamble comes before the server's first byte (it writes nothing until
            // the client speaks).
            let mut preamble = Vec::new();
            tokio::time::timeout(Duration::from_secs(30), async {
                while !preamble.ends_with(crate::cli::PREAMBLE.as_bytes()) {
                    preamble.push(stdout.read_u8().await.expect("the child's preamble"));
                }
            })
            .await
            .expect("the child starts");
            let client = client_over((stdout, stdin), revision).await;
            (child, client)
        });
        let registry = Arc::new(cutweave_engine::build_registry().expect("the registry"));
        Mcp {
            rt,
            client: Some(client),
            server: Some(Server::Process(Box::new(child))),
            catalog: Arc::new(Catalog::new(registry, Profile::Standard)),
            rev: Cell::new(None),
        }
    }

    fn tool(&self, name: &str, args: Value) -> CallToolResult {
        let client = self.client.as_ref().expect("an open session");
        self.rt
            .block_on(client.try_call(name, args))
            .unwrap_or_else(|e| panic!("tools/call {name}: {e}"))
    }

    /// Any tool call; `Ok(structuredContent)` or the tool error.
    pub fn raw(&self, name: &str, args: Value) -> Result<Value, StepError> {
        let result = self.tool(name, args);
        let structured = result
            .structured_content
            .clone()
            .unwrap_or_else(|| panic!("{name}: no structuredContent: {}", text_of(&result)));
        if result.is_error == Some(true) {
            return Err(StepError {
                code: structured["error"]["code"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                failed_step: structured.get("failed_step").and_then(Value::as_u64),
                raw: structured,
            });
        }
        let reported = result
            .meta
            .as_ref()
            .and_then(|meta| meta.get("io.cutweave/rev"))
            .or_else(|| structured.get("rev"));
        self.rev.set(reported.and_then(Value::as_u64));
        Ok(structured)
    }

    /// The revision the last successful call reported.
    pub fn rev(&self) -> Option<u64> {
        self.rev.get()
    }

    /// One call, the way an agent makes it. A step that expects a revision sends it as the
    /// tool's `expected_rev` argument (WP-2I.04), or as `cutweave_execute`'s for a command
    /// without a tool of its own; a read takes none.
    pub fn call(&self, call: &Call, expected_rev: Option<u64>) -> Outcome {
        let entry = self.catalog.reachable(&call.cmd);
        match entry {
            Some(e) if self.catalog.lists(&e.meta) => {
                let mut args = call.args.clone();
                if let Some(rev) = expected_rev.filter(|_| !e.meta.annotations.read_only) {
                    args["expected_rev"] = json!(rev);
                }
                self.raw(&e.meta.mcp_tool_name(), args)
                    .map(|out| vec![trusted(out)])
            }
            Some(e) if e.meta.annotations.read_only => self
                .raw(
                    "cutweave_query",
                    json!({ "name": call.cmd, "args": call.args }),
                )
                .map(|out| vec![trusted(out)]),
            // Not reachable (`hidden`, unknown): `cutweave_execute` reports it.
            _ => self.batch(std::slice::from_ref(call), expected_rev),
        }
    }

    /// A batch: one `cutweave_execute`.
    pub fn batch(&self, calls: &[Call], expected_rev: Option<u64>) -> Outcome {
        let steps: Vec<Value> = calls
            .iter()
            .map(|c| json!({ "cmd": c.cmd, "args": c.args }))
            .collect();
        let mut request = json!({ "steps": steps });
        if let Some(rev) = expected_rev {
            request["expected_rev"] = json!(rev);
        }
        self.raw("cutweave_execute", request).map(|out| {
            out["steps"]
                .as_array()
                .expect("cutweave_execute returns steps")
                .iter()
                .map(|s| trusted(s["output"].clone()))
                .collect()
        })
    }

    /// Closes the session (the server ends when its input closes).
    pub fn close(mut self) {
        if let Some(client) = self.client.take() {
            self.rt.block_on(client.close());
        }
        match self.server.take() {
            Some(Server::InProcess(server)) => {
                let ended = self
                    .rt
                    .block_on(async { tokio::time::timeout(Duration::from_secs(10), server).await })
                    .expect("the stdio session ends when its input closes");
                assert!(
                    matches!(ended, Ok(Ok(Ended::InputClosed))),
                    "the stdio session ended with {ended:?}"
                );
            }
            Some(Server::Process(mut child)) => {
                let status = self
                    .rt
                    .block_on(async {
                        tokio::time::timeout(Duration::from_secs(15), child.wait()).await
                    })
                    .expect("cutweave mcp exits when its stdin closes")
                    .expect("wait");
                assert_eq!(status.code(), Some(0), "cutweave mcp --headless: {status}");
            }
            None => {}
        }
    }
}

/// `output` with MCP's untrusted-text wrappers (`{"untrusted": true, "text": ...}`, arch §14)
/// replaced by their text: the engine's output, as the other surfaces return it.
pub fn trusted(mut output: Value) -> Value {
    unwrap_untrusted(&mut output);
    output
}

fn unwrap_untrusted(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.len() == 2
                && map.get("untrusted") == Some(&Value::Bool(true))
                && let Some(Value::String(text)) = map.get("text")
            {
                *value = Value::String(text.clone());
                return;
            }
            map.values_mut().for_each(unwrap_untrusted);
        }
        Value::Array(items) => items.iter_mut().for_each(unwrap_untrusted),
        _ => {}
    }
}
