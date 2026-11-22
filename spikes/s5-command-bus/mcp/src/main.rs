// SPDX-License-Identifier: GPL-3.0-or-later
//! `s5-mcp`: the spike MCP server binary (WP-0H.02). Logs go to stderr (or `--log-file`), never
//! to stdout, which is the stdio protocol channel. Log filter: `S5_MCP_LOG` (default `info`).

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Mutex, PoisonError},
};

use anyhow::{Context as _, bail};
use rmcp::ServiceExt as _;
use s5_bus::{
    Actor, Caller, Engine, EngineConfig,
    engine::{Event, Request},
};
use s5_mcp::{
    Catalog, CutweaveMcp, EngineHandle, Profile,
    auth::{AgentToken, MIN_SECRET_LEN, RunFile},
    budget, http,
};
use serde_json::{Value, json};

const HELP: &str = "s5-mcp: Cutweave spike S5 MCP server (rmcp 3.11.0)

USAGE:
    s5-mcp stdio  [--profile minimal|standard|full] [--agent-label LABEL] [--deterministic-ids SEED]
                  [--state-file PATH] [--log-file PATH]
    s5-mcp http   [--port N] [--agent-label LABEL] [--run-file PATH] [--allow-origin ORIGIN]...
                  [--deterministic-ids SEED] [--state-file PATH] [--log-file PATH]
    s5-mcp budget                       print the tools/list size of every profile
    s5-mcp tools  [--profile P]         print the tools/list `tools` array as JSON

stdio: one engine in this process; the actor is agent:<label> (default `stdio`). The server
       exits when stdin closes.
http:  127.0.0.1 only; endpoints /mcp (standard), /mcp/minimal, /mcp/full. The agent token
       (random, or $CUTWEAVE_S5_AGENT_TOKEN if set, >= 32 chars) is written to the run-file
       (default ~/.cutweave/run/<pid>.json), which is removed on exit. Requests need
       `Authorization: Bearer <token>`; Host must be 127.0.0.1:<port> or localhost:<port>;
       browser Origins are refused unless listed with --allow-origin.
--state-file: keep {rev, changes, timeline} (timeline.get) in PATH, rewritten after every
       change, so the final state survives however the agent host stops the server.
--log-file: append logs to PATH instead of stderr.";

#[derive(Default)]
struct Args {
    profile: Option<Profile>,
    label: Option<String>,
    seed: Option<u64>,
    port: u16,
    run_file: Option<PathBuf>,
    origins: Vec<String>,
    state_file: Option<PathBuf>,
    log_file: Option<PathBuf>,
}

fn parse(mut it: impl Iterator<Item = String>) -> anyhow::Result<Args> {
    let mut a = Args::default();
    while let Some(flag) = it.next() {
        let mut value = || it.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--profile" => {
                let v = value()?;
                a.profile =
                    Some(Profile::parse(&v).with_context(|| format!("unknown profile `{v}`"))?);
            }
            "--agent-label" => a.label = Some(value()?),
            "--deterministic-ids" => a.seed = Some(value()?.parse().context("seed")?),
            "--port" => a.port = value()?.parse().context("port")?,
            "--run-file" => a.run_file = Some(value()?.into()),
            "--allow-origin" => a.origins.push(value()?),
            "--state-file" => a.state_file = Some(value()?.into()),
            "--log-file" => a.log_file = Some(value()?.into()),
            other => bail!("unknown argument `{other}`\n\n{HELP}"),
        }
    }
    Ok(a)
}

fn engine(seed: Option<u64>) -> EngineHandle {
    EngineHandle::new(seed.map_or_else(EngineConfig::default, EngineConfig::deterministic))
}

fn init_logging(log_file: Option<&Path>) -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_env("S5_MCP_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    match log_file {
        // Agent hosts often swallow a stdio server's stderr; a file keeps the lifecycle log.
        Some(path) => {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("opening {}", path.display()))?;
            builder.with_writer(Mutex::new(file)).init();
        }
        None => builder.with_writer(std::io::stderr).init(),
    }
    Ok(())
}

/// Writes `{rev, changes, timeline}` to `path` (via a temp file + rename, so readers never see
/// a torn file).
fn write_state(engine: &EngineHandle, changes: &[Value], path: &Path) -> anyhow::Result<()> {
    let (rev, timeline) = engine.with(|e| {
        let timeline = e
            .invoke(
                &Caller::new(Actor::System),
                Request::new("timeline.get", json!({})),
            )
            .map_or_else(|err| json!({ "error": err.to_wire() }), |r| r.result);
        (e.rev(), timeline)
    });
    let mut body = serde_json::to_string_pretty(&json!({
        "rev": rev,
        "changes": changes,
        "timeline": timeline,
    }))?;
    body.push('\n');
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Keeps a state file current: an agent-independent record of the document for WP-0H.03's
/// "final doc state matches expectations", whether the agent host closes stdin or kills us.
struct StateTracker {
    engine: EngineHandle,
    path: PathBuf,
    changes: Arc<Mutex<Vec<Value>>>,
}

impl StateTracker {
    fn start(engine: &EngineHandle, path: PathBuf) -> anyhow::Result<StateTracker> {
        let events = engine.with(Engine::subscribe);
        write_state(engine, &[], &path)?;
        let tracker = StateTracker {
            engine: engine.clone(),
            path,
            changes: Arc::default(),
        };
        let (engine, path, changes) = (
            tracker.engine.clone(),
            tracker.path.clone(),
            tracker.changes.clone(),
        );
        std::thread::Builder::new()
            .name("s5-mcp-state".into())
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    let mut changes = changes.lock().unwrap_or_else(PoisonError::into_inner);
                    match event {
                        Event::DocPatched {
                            rev, actor, label, ..
                        } => changes.push(json!({ "rev": rev, "actor": actor, "label": label })),
                    }
                    if let Err(e) = write_state(&engine, &changes, &path) {
                        tracing::warn!("state file: {e:#}");
                    }
                }
            })?;
        Ok(tracker)
    }

    /// On a graceful exit: waits (up to 1 s) for the tracker to see the last revision.
    fn finish(&self) {
        let rev = self.engine.with(|e| e.rev().0);
        for _ in 0..100 {
            let seen = self
                .changes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .last()
                .map_or(0, |c| c["rev"].as_u64().unwrap_or(0));
            if seen >= rev {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        tracing::warn!("state file may miss the last change");
    }
}

async fn run_stdio(a: Args) -> anyhow::Result<()> {
    let profile = a.profile.unwrap_or(Profile::Standard);
    let label = a.label.unwrap_or_else(|| "stdio".into());
    let engine = engine(a.seed);
    let tracker = a
        .state_file
        .map(|path| StateTracker::start(&engine, path))
        .transpose()?;
    let handler = CutweaveMcp::new(
        engine,
        Catalog::shared(profile),
        Some(Caller::agent(&label)),
    );
    tracing::info!(profile = profile.as_str(), actor = %format!("agent:{label}"), "MCP over stdio");
    let service = handler.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    if let Some(tracker) = tracker {
        tracker.finish();
    }
    Ok(())
}

async fn run_http(a: Args) -> anyhow::Result<()> {
    if a.profile.is_some() {
        bail!("--profile is for stdio; over HTTP the profile is the endpoint path");
    }
    let label = a.label.unwrap_or_else(|| "http".into());
    let token = match std::env::var("CUTWEAVE_S5_AGENT_TOKEN") {
        Ok(secret) if secret.len() >= MIN_SECRET_LEN => AgentToken { label, secret },
        Ok(_) => bail!("CUTWEAVE_S5_AGENT_TOKEN must be at least {MIN_SECRET_LEN} chars"),
        Err(_) => AgentToken::generate(&label)?,
    };
    let engine = engine(a.seed);
    let tracker = a
        .state_file
        .map(|path| StateTracker::start(&engine, path))
        .transpose()?;
    let running = http::serve(
        engine,
        http::HttpOptions {
            port: a.port,
            tokens: vec![token.clone()],
            allowed_origins: a.origins,
        },
    )
    .await?;
    let path = a
        .run_file
        .or_else(RunFile::default_path)
        .context("no home directory for the run-file; pass --run-file")?;
    let run_file = RunFile::write(&path, running.addr(), &token)?;
    eprintln!(
        "s5-mcp: listening on {} (agent token in {})",
        running.url(Profile::Standard),
        run_file.path().display()
    );
    tokio::signal::ctrl_c().await?;
    running.shutdown().await;
    drop(run_file);
    if let Some(tracker) = tracker {
        tracker.finish();
    }
    Ok(())
}

fn print_budget() {
    println!("{}", budget::TABLE_HEADER);
    for profile in Profile::ALL {
        println!("{}", budget::measure(&Catalog::shared(profile)).row());
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{HELP}");
        return ExitCode::from(2);
    };
    let result = match (command.as_str(), parse(args)) {
        (_, Err(e)) => Err(e),
        ("stdio", Ok(a)) => match init_logging(a.log_file.as_deref()) {
            Ok(()) => run_stdio(a).await,
            Err(e) => Err(e),
        },
        ("http", Ok(a)) => match init_logging(a.log_file.as_deref()) {
            Ok(()) => run_http(a).await,
            Err(e) => Err(e),
        },
        ("budget", Ok(_)) => {
            print_budget();
            Ok(())
        }
        ("tools", Ok(a)) => {
            let catalog = Catalog::shared(a.profile.unwrap_or(Profile::Standard));
            serde_json::to_string_pretty(catalog.tools())
                .map(|s| println!("{s}"))
                .map_err(Into::into)
        }
        ("help" | "--help" | "-h", Ok(_)) => {
            println!("{HELP}");
            Ok(())
        }
        (other, Ok(_)) => Err(anyhow::anyhow!("unknown command `{other}`\n\n{HELP}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("s5-mcp: {e:#}");
            ExitCode::FAILURE
        }
    }
}
