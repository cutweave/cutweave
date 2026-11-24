// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave mcp` (WP-1J.03; arch §14, ADR-0009): MCP over stdio for agent hosts.
//!
//! - **Attach** (the default, `--attach`): a stdio proxy to a running engine's MCP endpoint
//!   (`cutweave_api::mcp::proxy`), found through its run-file and authenticated with the
//!   run-file's `mcp` agent token. The agent and the desktop app edit the same document.
//! - **Headless** (`--headless`, `--project`, `--allow-root`, ...): an engine in this process
//!   (no HTTP server, no run-file) served over stdio (`cutweave_api::mcp::stdio`), actor
//!   `agent:mcp`.
//!
//! Stdout belongs to the protocol from the first byte: errors before the session starts are
//! JSON on stderr ([`Failure`]), and nothing else here prints to stdout.

use std::path::Path;
use std::sync::Arc;

use cutweave_api::engines::{SelectError, Selector};
use cutweave_api::launch::shutdown_signal;
use cutweave_api::mcp::proxy::{self, ProxyEnd, Upstream};
use cutweave_api::mcp::switch::Directory;
use cutweave_api::mcp::{Catalog, CutweaveMcp, Dispatch, Profile as McpProfile, stdio};
use cutweave_api::{Credential, RunFileContents};
use cutweave_engine::logging::{self, LogGuard};
use cutweave_engine::{AudioConfig, Engine, EngineConfig, EngineMode, LogConfig};
use serde_json::json;

use crate::cli::{McpCmd, Profile};
use crate::failure::{EXIT_OK, Failure};
use crate::remote::{self, Miss};
use crate::session::{Plan, SpawnPlan};

/// The agent token label (and actor `agent:mcp`) of MCP clients.
const MCP_AGENT: &str = "mcp";

/// Runs `cutweave mcp` with the engine `plan` chose and returns the exit code.
pub fn run(cmd: &McpCmd, plan: Plan) -> i32 {
    let result = match plan {
        Plan::Attach {
            run_dir,
            selector,
            fallback,
        } => match find_engine(cmd, run_dir.as_deref(), &selector) {
            Ok(engine) => attach(cmd, &engine, run_dir.as_deref()),
            // `--project <path>` that no engine has open: the one-shot engine it always was.
            Err(miss) => match (fallback, miss.why) {
                (Some(spawn), SelectError::NoEngine | SelectError::NoMatch) => headless(cmd, spawn),
                _ => Err(miss.failure),
            },
        },
        Plan::Spawn(spawn) => headless(cmd, spawn),
    };
    match result {
        Ok(code) => code,
        Err(failure) => {
            failure.print();
            failure.exit
        }
    }
}

fn profile(profile: Profile) -> McpProfile {
    match profile {
        Profile::Minimal => McpProfile::Minimal,
        Profile::Standard => McpProfile::Standard,
        Profile::Full => McpProfile::Full,
    }
}

/// The runtime of the stdio session (the engine has its own).
fn runtime() -> Result<tokio::runtime::Runtime, Failure> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Failure::error("internal", format!("cannot start the runtime: {e}")))
}

/// The engine to attach to: the one `--run-file` names, else the one `selector` chooses (the
/// app's focused engine without `--project` and `--pid`).
fn find_engine(
    cmd: &McpCmd,
    run_dir: Option<&Path>,
    selector: &Selector,
) -> Result<RunFileContents, Miss> {
    match &cmd.run_file {
        Some(path) => remote::read_run_file(path).map_err(|failure| Miss {
            why: SelectError::NoEngine,
            failure,
        }),
        None => remote::find(run_dir, selector),
    }
}

/// `--attach`: proxies stdio to the engine's MCP endpoint until stdin closes (exit 0) or the
/// engine is gone (exit 3). The agent can list the engines and move to another one with the
/// proxy's `engines_list` and `engine_attach` tools; they look in the run-file directory of
/// `--run-dir`, or the folder of `--run-file`.
fn attach(cmd: &McpCmd, engine: &RunFileContents, run_dir: Option<&Path>) -> Result<i32, Failure> {
    let token = remote::agent_token(engine, MCP_AGENT)?.to_owned();
    let pid = engine.pid;
    let upstream = Upstream {
        url: Upstream::endpoint(engine.port, profile(cmd.profile)),
        token,
        pid,
        alive: Arc::new(move || cutweave_api::runfile::process_alive(pid)),
    };
    let directory = match &cmd.run_file {
        Some(file) => file.parent().map(Path::to_path_buf),
        None => cutweave_api::engines::run_dir(run_dir),
    }
    .map(|run_dir| Directory {
        run_dir,
        profile: profile(cmd.profile),
    });
    let rt = runtime()?;
    let end = rt.block_on(proxy::proxy_stdio_with(
        upstream,
        directory,
        shutdown_signal(),
    ));
    // A stdin read may still be pending on the blocking pool: do not wait for it.
    rt.shutdown_background();
    match end {
        ProxyEnd::InputClosed | ProxyEnd::Shutdown => Ok(EXIT_OK),
        ProxyEnd::Lost { message } => Err(Failure::no_engine(
            "connection_lost",
            message,
            json!({ "pid": pid }),
        )),
    }
}

/// The in-process engine's log file, like `cutweave engine`'s (a failure only warns).
fn start_log(cmd: &McpCmd) -> Option<LogGuard> {
    if cmd.no_log {
        return None;
    }
    let config = LogConfig {
        dir: cmd.log_dir.clone(),
        ..LogConfig::default()
    };
    match logging::init(&config) {
        Ok(guard) => Some(guard),
        Err(e) => {
            eprintln!("cutweave mcp: no log file: {e}");
            None
        }
    }
}

/// `--headless`: an engine in this process, served over stdio until stdin closes.
fn headless(cmd: &McpCmd, spawn: SpawnPlan) -> Result<i32, Failure> {
    let log = start_log(cmd);
    let result = serve_headless(cmd, spawn);
    if let Some(log) = log {
        if result.is_ok() {
            logging::remove_crash_marker(log.dir());
        }
        log.finish();
    }
    result
}

fn serve_headless(cmd: &McpCmd, spawn: SpawnPlan) -> Result<i32, Failure> {
    let subsystems = spawn.subsystems();
    let mut config = EngineConfig::new(EngineMode::Headless);
    config.project = spawn.project;
    config.deterministic_ids = spawn.deterministic_ids;
    config.allowed_roots = spawn.allow_roots;
    // The full engine (FFmpeg media when this build has it, the GPU compositor, playback) on the
    // null audio output: `render_frame` and real imports work. `--dev-stub-media` opts into the
    // stub (made-up probing, no decoding).
    config.audio = AudioConfig::Null;
    config.subsystems = Some(subsystems);
    let engine = Engine::start(config).map_err(|e| Failure::error(e.code(), e.to_string()))?;
    if spawn.dev_stub_media {
        eprintln!("cutweave mcp: warning: dev stub media is on (fake media probing, dev only)");
    }
    let handle = engine.handle();
    let catalog = Arc::new(Catalog::new(handle.registry(), profile(cmd.profile)));
    let handler = CutweaveMcp::new(
        Arc::new(handle) as Arc<dyn Dispatch>,
        catalog,
        Some(Credential::Agent(MCP_AGENT.into())),
    );
    let rt = runtime()?;
    let served = rt.block_on(stdio::serve_stdio(handler, shutdown_signal()));
    rt.shutdown_background();
    // Nothing is saved implicitly: unsaved work stays in the journal, as for any engine.
    engine.shutdown();
    served
        .map(|_| EXIT_OK)
        .map_err(|e| Failure::error("mcp_failed", e.to_string()))
}
