// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine launch protocol (WP-1I.04; `docs/reference/launch-protocol.md`): how Electron
//! main's `EngineSupervisor` (and the CLI) start an engine process and learn that it is ready.
//!
//! ```text
//! cutweave engine [--headless] --port N [--token-stdin] [--project P] [--run-dir D]
//!                 [--allow-origin O]... [--allow-root R]... [--deterministic-ids SEED]
//!                 [--log-dir D | --no-log] [--handshake-timeout-ms MS]
//! stdin  : Gui mode: first line = the UI credential (never argv, env, disk or logs);
//!          EOF afterwards = stop (orphan prevention)
//! stdout : JSON lines only: {"event":"ready","port":P,"pid":N,"version":"..."} once serving,
//!          or {"event":"error","code":"...","exit_code":N,"message":"..."} before exiting
//! exit   : 0 clean · 1 other failure · 2 usage · 10 project locked · 11 GPU init failed ·
//!          12 port bind error · 13 bad or missing handshake
//! ```
//!
//! The handshake token rules, the readiness line and the exit-12 contract are WP-1I.01's
//! ([`cutweave_api::launch`]); this module adds the engine around them.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use cutweave_api::Token;
use cutweave_api::launch::{LaunchArgs, ready_line};
use serde_json::{Value, json};

use crate::config::{ApiOptions, EngineConfig, EngineMode};
use crate::engine::{Engine, VERSION};
use crate::logging::{self, LogConfig};

/// Clean exit.
pub const EXIT_OK: i32 = 0;
/// Any failure without a code of its own.
pub const EXIT_FAILURE: i32 = cutweave_api::launch::EXIT_FAILURE;
/// Bad arguments.
pub const EXIT_USAGE: i32 = cutweave_api::launch::EXIT_USAGE;
/// The project is open in another process.
pub const EXIT_PROJECT_LOCKED: i32 = 10;
/// The GPU could not be initialized (`Gui` mode; WP-1I.05).
pub const EXIT_GPU_INIT_FAILED: i32 = 11;
/// The port could not be bound: the supervisor picks another one and restarts.
pub const EXIT_PORT_IN_USE: i32 = cutweave_api::EXIT_PORT_IN_USE;
/// The handshake line was missing, late, malformed, or given to a headless engine.
pub const EXIT_HANDSHAKE: i32 = cutweave_api::launch::EXIT_HANDSHAKE;

/// How long a `Gui` engine waits for the handshake line.
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// The longest handshake line read (a token is at most 512 characters).
const MAX_HANDSHAKE_BYTES: u64 = 1024;

/// Usage text.
pub const USAGE: &str = "usage: engine [--headless] [--port N] [--token-stdin] [--project P] \
[--run-dir D] [--allow-origin O]... [--allow-root R]... [--deterministic-ids SEED] \
[--log-dir D | --no-log] [--handshake-timeout-ms MS]";

/// Parsed launch arguments: WP-1I.01's [`LaunchArgs`] plus the engine's own flags.
#[derive(Debug, PartialEq, Eq)]
pub struct EngineArgs {
    /// `--port`, `--token-stdin`, `--headless`, `--project`, `--run-dir`, `--allow-origin`.
    pub base: LaunchArgs,
    /// `--deterministic-ids SEED` (WP-1B.08 adds the env var and the GUI refusal).
    pub deterministic_ids: Option<u64>,
    /// `--allow-root R` (repeatable): folders agents may import from.
    pub allow_roots: Vec<PathBuf>,
    /// `--log-dir D` (default `~/.cutweave/logs`).
    pub log_dir: Option<PathBuf>,
    /// `--no-log`: no log file.
    pub no_log: bool,
    /// `--handshake-timeout-ms MS` (tests; default 5000).
    pub handshake_timeout: Duration,
}

impl EngineArgs {
    /// Parses the arguments after the program name (a leading `engine` is accepted).
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<EngineArgs, String> {
        let mut rest = Vec::new();
        let mut out = EngineArgs {
            base: LaunchArgs::default(),
            deterministic_ids: None,
            allow_roots: Vec::new(),
            log_dir: None,
            no_log: false,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
        };
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--deterministic-ids" => {
                    out.deterministic_ids = Some(
                        value("--deterministic-ids")?
                            .parse()
                            .map_err(|_| "--deterministic-ids must be an unsigned integer")?,
                    );
                }
                "--allow-root" => out.allow_roots.push(PathBuf::from(value("--allow-root")?)),
                "--log-dir" => out.log_dir = Some(PathBuf::from(value("--log-dir")?)),
                "--no-log" => out.no_log = true,
                "--handshake-timeout-ms" => {
                    let ms: u64 = value("--handshake-timeout-ms")?
                        .parse()
                        .map_err(|_| "--handshake-timeout-ms must be an unsigned integer")?;
                    out.handshake_timeout = Duration::from_millis(ms);
                }
                _ => rest.push(arg),
            }
        }
        out.base = LaunchArgs::parse(rest)?;
        if out.no_log && out.log_dir.is_some() {
            return Err("--no-log and --log-dir exclude each other".into());
        }
        Ok(out)
    }

    /// `Headless` with `--headless`, otherwise `Gui`.
    #[must_use]
    pub fn mode(&self) -> EngineMode {
        if self.base.headless {
            EngineMode::Headless
        } else {
            EngineMode::Gui
        }
    }
}

/// One JSON line on stdout.
fn emit(stdout: &mut impl Write, line: &Value) -> std::io::Result<()> {
    writeln!(stdout, "{line}")?;
    stdout.flush()
}

/// The error line of the protocol (stdout) plus a human line on stderr; returns `exit_code`.
fn fail(stdout: &mut impl Write, code: &str, exit_code: i32, message: &str, extra: Value) -> i32 {
    let mut line = json!({
        "event": "error",
        "code": code,
        "exit_code": exit_code,
        "message": message,
    });
    if let (Value::Object(line), Value::Object(extra)) = (&mut line, extra) {
        line.extend(extra);
    }
    let _ = emit(stdout, &line);
    eprintln!("cutweave engine: {message}");
    tracing::error!(code, exit_code, "{message}");
    exit_code
}

enum Input {
    /// The handshake line (without its line ending).
    Line(String),
    /// Stdin closed.
    Eof,
    /// SIGINT / SIGTERM / Ctrl-C.
    Signal,
}

/// Reads the handshake line, then waits for EOF (the supervisor closed the pipe or died).
fn read_stdin(stdin: impl Read + Send + 'static, tx: mpsc::Sender<Input>) {
    let spawned = std::thread::Builder::new()
        .name("cutweave-stdin".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdin);
            let mut line = String::new();
            match reader
                .by_ref()
                .take(MAX_HANDSHAKE_BYTES)
                .read_line(&mut line)
            {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Input::Eof);
                    return;
                }
                Ok(_) => {
                    let token = line.trim_end_matches(['\r', '\n']).to_owned();
                    line.clear();
                    let _ = tx.send(Input::Line(token));
                }
            }
            let mut sink = Vec::new();
            loop {
                sink.clear();
                match reader.read_until(b'\n', &mut sink) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            let _ = tx.send(Input::Eof);
        });
    if spawned.is_err() {
        tracing::error!("cannot start the stdin reader");
    }
}

/// Runs an engine process to completion and returns its exit code: handshake (`Gui`), start,
/// readiness line, then wait for stdin EOF (`Gui`) or a signal, then shut down.
pub fn run(args: &EngineArgs, stdin: impl Read + Send + 'static, stdout: &mut impl Write) -> i32 {
    let mode = args.mode();
    let log_config = if args.no_log {
        LogConfig::disabled()
    } else {
        LogConfig {
            dir: args.log_dir.clone(),
            ..LogConfig::default()
        }
    };
    let log = if log_config.enabled() {
        match logging::init(&log_config) {
            Ok(guard) => Some(guard),
            Err(e) => {
                eprintln!("cutweave engine: no log file: {e}");
                None
            }
        }
    } else {
        None
    };
    let code = run_logged(args, mode, stdin, stdout);
    if let Some(log) = log {
        if code == EXIT_OK {
            logging::remove_crash_marker(log.dir());
        }
        log.finish();
    }
    code
}

/// The UI credential: the first stdin line, within `timeout`.
fn handshake(rx: &mpsc::Receiver<Input>, timeout: Duration) -> Result<Token, String> {
    match rx.recv_timeout(timeout) {
        Ok(Input::Line(line)) => {
            Token::parse(&line).map_err(|e| format!("invalid handshake token: {e}"))
        }
        Ok(Input::Eof) => Err("stdin closed before the handshake line".into()),
        Ok(Input::Signal) | Err(_) => Err(format!(
            "no handshake line on stdin within {} ms",
            timeout.as_millis()
        )),
    }
}

fn run_logged(
    args: &EngineArgs,
    mode: EngineMode,
    stdin: impl Read + Send + 'static,
    stdout: &mut impl Write,
) -> i32 {
    tracing::info!(
        mode = mode.name(),
        port = args.base.port,
        pid = std::process::id(),
        version = VERSION,
        "engine starting"
    );
    if mode == EngineMode::Headless && args.base.token_stdin {
        return fail(
            stdout,
            "bad_handshake",
            EXIT_HANDSHAKE,
            "a headless engine has no UI credential; refusing the token handshake",
            Value::Null,
        );
    }
    let (tx, rx) = mpsc::channel();
    let ui_token = if mode == EngineMode::Gui {
        read_stdin(stdin, tx.clone());
        match handshake(&rx, args.handshake_timeout) {
            Ok(token) => Some(token),
            Err(message) => {
                return fail(
                    stdout,
                    "bad_handshake",
                    EXIT_HANDSHAKE,
                    &message,
                    Value::Null,
                );
            }
        }
    } else {
        drop(stdin);
        None
    };
    let config = EngineConfig {
        mode,
        project: args.base.project.as_ref().map(PathBuf::from),
        port: Some(args.base.port),
        ui_token,
        deterministic_ids: args.deterministic_ids,
        allowed_roots: args.allow_roots.clone(),
        api: ApiOptions {
            run_dir: args.base.run_dir.clone(),
            allowed_origins: args.base.allowed_origins.clone(),
            ..ApiOptions::default()
        },
        ..EngineConfig::default()
    };
    let engine = match Engine::start(config) {
        Ok(engine) => engine,
        Err(e) => {
            let extra = match &e {
                crate::engine::StartError::ProjectLocked {
                    holder: Some(h), ..
                } => json!({"holder": {"pid": h.pid, "host": h.host, "started": h.started}}),
                _ => Value::Null,
            };
            return fail(stdout, e.code(), e.exit_code(), &e.to_string(), extra);
        }
    };
    let port = engine.port().unwrap_or(0);
    let ready = ready_line(port, std::process::id(), VERSION);
    if writeln!(stdout, "{ready}")
        .and_then(|()| stdout.flush())
        .is_err()
    {
        engine.shutdown();
        return EXIT_FAILURE;
    }
    let signals = tx;
    engine.handle().runtime().spawn(async move {
        shutdown_signal().await;
        let _ = signals.send(Input::Signal);
    });
    let reason = loop {
        match rx.recv() {
            Ok(Input::Eof) if mode == EngineMode::Gui => break "stdin closed",
            Ok(Input::Signal) => break "signal",
            Ok(Input::Line(_) | Input::Eof) => {}
            Err(_) => break "no more inputs",
        }
    };
    tracing::info!(reason, "engine stopping");
    engine.shutdown();
    EXIT_OK
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        }
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

/// The process entry point: parses `std::env::args`, runs, returns the exit code (the caller
/// exits with it, so a blocked stdin read never delays the exit).
#[must_use]
pub fn main() -> i32 {
    let mut stdout = std::io::stdout();
    match EngineArgs::parse(std::env::args().skip(1)) {
        Ok(args) => run(&args, std::io::stdin(), &mut stdout),
        Err(message) => fail(
            &mut stdout,
            "usage",
            EXIT_USAGE,
            &format!("{message}\n{USAGE}"),
            Value::Null,
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn parse(s: &str) -> Result<EngineArgs, String> {
        EngineArgs::parse(s.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn parses_supervisor_and_engine_flags() {
        let a = parse(
            "engine --port 4321 --token-stdin --deterministic-ids 42 --allow-root /m \
             --log-dir /l --handshake-timeout-ms 250 --allow-origin app://cutweave",
        )
        .unwrap();
        assert_eq!(a.base.port, 4321);
        assert!(a.base.token_stdin);
        assert_eq!(a.mode(), EngineMode::Gui);
        assert_eq!(a.deterministic_ids, Some(42));
        assert_eq!(a.allow_roots, vec![PathBuf::from("/m")]);
        assert_eq!(a.log_dir, Some(PathBuf::from("/l")));
        assert_eq!(a.handshake_timeout, Duration::from_millis(250));
        assert_eq!(a.base.allowed_origins, vec!["app://cutweave".to_owned()]);
        assert_eq!(parse("--headless").unwrap().mode(), EngineMode::Headless);
    }

    #[test]
    fn rejects_bad_flags() {
        assert!(parse("--bogus").is_err());
        assert!(parse("--deterministic-ids x").is_err());
        assert!(parse("--deterministic-ids").is_err());
        assert!(parse("--no-log --log-dir /l").is_err());
        assert!(parse("--port 99999").is_err());
    }

    #[test]
    fn exit_codes_match_the_protocol() {
        assert_eq!(
            [
                EXIT_OK,
                EXIT_FAILURE,
                EXIT_USAGE,
                EXIT_PROJECT_LOCKED,
                EXIT_GPU_INIT_FAILED,
                EXIT_PORT_IN_USE,
                EXIT_HANDSHAKE
            ],
            [0, 1, 2, 10, 11, 12, 13]
        );
    }
}
