// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine launch protocol (WP-1I.04; `docs/reference/launch-protocol.md`): how Electron
//! main's `EngineSupervisor` (and the CLI) start an engine process and learn that it is ready.
//!
//! ```text
//! cutweave engine [--headless] --port N [--token-stdin] [--project P] [--run-dir D]
//!                 [--allow-origin O]... [--allow-root R]... [--deterministic-ids SEED]
//!                 [--i-know-this-is-a-test] [--log-dir D | --no-log] [--handshake-timeout-ms MS]
//!                 [--cache-dir D] [--dev-stub-media] [--progress]
//! env    : CUTWEAVE_DETERMINISTIC_IDS=SEED (the flag wins), CUTWEAVE_DEV_STUB_MEDIA=1,
//!          CUTWEAVE_CACHE_DIR=D (the cache folder; the flag wins),
//!          CUTWEAVE_GPU=auto|software|... (the adapter), CUTWEAVE_AUDIO=null|default|<device>
//!          (the GUI engine's output; a headless engine always plays on the null clock)
//! stdin : Gui mode: first line = the UI credential (never argv, env, disk or logs);
//!          EOF afterwards = stop (orphan prevention)
//! stdout : JSON lines only: {"event":"ready","port":P,"pid":N,"version":"..."} once serving,
//!          or {"event":"error","code":"...","exit_code":N,"message":"..."} before exiting;
//!          with --progress, {"event":"starting","pid":N,"version":"...","elapsed_ms":T} as soon
//!          as the engine runs and every second until then
//! exit   : 0 clean · 1 other failure · 2 usage · 10 project locked · 11 GPU init failed ·
//!          12 port bind error · 13 bad or missing handshake
//! ```
//!
//! The handshake token rules, the readiness line and the exit-12 contract are WP-1I.01's
//! ([`cutweave_api::launch`]); this module adds the engine around them.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use cutweave_api::Token;
use cutweave_api::launch::{LaunchArgs, ready_line};
use cutweave_api::mcp::McpOptions;
use cutweave_api::rpc::RpcConfig;
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

/// The environment variable equivalent of `--deterministic-ids` (WP-1B.08).
pub const DETERMINISTIC_IDS_ENV: &str = "CUTWEAVE_DETERMINISTIC_IDS";

/// How long a `Gui` engine waits for the handshake line.
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often an engine started with `--progress` says it is still starting.
pub const PROGRESS_INTERVAL: Duration = Duration::from_secs(1);

/// The longest handshake line read (a token is at most 512 characters).
const MAX_HANDSHAKE_BYTES: u64 = 1024;

/// Usage text.
pub const USAGE: &str = "usage: engine [--headless] [--port N] [--token-stdin] [--project P] \
[--run-dir D] [--allow-origin O]... [--allow-root R]... [--deterministic-ids SEED] \
[--i-know-this-is-a-test] [--log-dir D | --no-log] [--handshake-timeout-ms MS] \
[--cache-dir D] [--dev-stub-media] [--progress]";

/// Parsed launch arguments: WP-1I.01's [`LaunchArgs`] plus the engine's own flags.
#[derive(Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent command-line switches
pub struct EngineArgs {
    /// `--port`, `--token-stdin`, `--headless`, `--project`, `--run-dir`, `--allow-origin`.
    pub base: LaunchArgs,
    /// `--deterministic-ids SEED` or, without the flag, `CUTWEAVE_DETERMINISTIC_IDS` (WP-1B.08).
    pub deterministic_ids: Option<u64>,
    /// `--i-know-this-is-a-test`: lets a `Gui` engine run with deterministic ids.
    pub i_know_this_is_a_test: bool,
    /// `--allow-root R` (repeatable): folders agents may import from.
    pub allow_roots: Vec<PathBuf>,
    /// `--log-dir D` (default `~/.cutweave/logs`).
    pub log_dir: Option<PathBuf>,
    /// `--no-log`: no log file.
    pub no_log: bool,
    /// `--handshake-timeout-ms MS` (tests; default 5000).
    pub handshake_timeout: Duration,
    /// `--dev-stub-media` or `CUTWEAVE_DEV_STUB_MEDIA=1`: dev-only fake media probing.
    pub dev_stub_media: bool,
    /// `--cache-dir D`: the folder the thumbnail, waveform and frame-index caches live in
    /// (default `CUTWEAVE_CACHE_DIR`, else `<OS cache folder>/cutweave`; WP-1D.08).
    pub cache_dir: Option<PathBuf>,
    /// `--progress`: `starting` lines on stdout until the ready line, so a supervisor can tell a
    /// slow start from a hung one (issue #143).
    pub progress: bool,
}

impl EngineArgs {
    /// Parses the arguments after the program name (a leading `engine` is accepted).
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<EngineArgs, String> {
        let mut rest = Vec::new();
        let mut out = EngineArgs {
            base: LaunchArgs::default(),
            deterministic_ids: None,
            i_know_this_is_a_test: false,
            allow_roots: Vec::new(),
            log_dir: None,
            no_log: false,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            dev_stub_media: false,
            cache_dir: None,
            progress: false,
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
                "--i-know-this-is-a-test" => out.i_know_this_is_a_test = true,
                "--allow-root" => out.allow_roots.push(PathBuf::from(value("--allow-root")?)),
                "--log-dir" => out.log_dir = Some(PathBuf::from(value("--log-dir")?)),
                "--no-log" => out.no_log = true,
                "--cache-dir" => {
                    let dir = value("--cache-dir")?;
                    if dir.trim().is_empty() {
                        return Err("--cache-dir needs a folder".into());
                    }
                    out.cache_dir = Some(PathBuf::from(dir));
                }
                "--dev-stub-media" => out.dev_stub_media = true,
                "--progress" => out.progress = true,
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

    /// Like [`parse`](Self::parse), with `env` the value of [`DETERMINISTIC_IDS_ENV`]: used
    /// when `--deterministic-ids` is absent (an empty value counts as unset).
    pub fn parse_with_env<I: IntoIterator<Item = String>>(
        args: I,
        env: Option<&str>,
    ) -> Result<EngineArgs, String> {
        let mut out = EngineArgs::parse(args)?;
        if out.deterministic_ids.is_none()
            && let Some(value) = env.map(str::trim).filter(|v| !v.is_empty())
        {
            out.deterministic_ids = Some(
                value
                    .parse()
                    .map_err(|_| format!("{DETERMINISTIC_IDS_ENV} must be an unsigned integer"))?,
            );
        }
        Ok(out)
    }

    /// Turns the dev stub media backend on when `env` (the value of
    /// [`DEV_STUB_MEDIA_ENV`](crate::DEV_STUB_MEDIA_ENV)) asks for it; the flag always wins.
    #[must_use]
    pub fn with_dev_stub_env(mut self, env: Option<&str>) -> EngineArgs {
        self.dev_stub_media |= crate::dev_media::env_enables(env);
        self
    }

    /// Refuses deterministic ids in `Gui` mode unless `--i-know-this-is-a-test` was passed:
    /// seeded ids repeat across runs, so a real user's documents would collide (ADR-0015).
    pub fn check_deterministic_ids(&self) -> Result<(), String> {
        if self.deterministic_ids.is_some()
            && self.mode() == EngineMode::Gui
            && !self.i_know_this_is_a_test
        {
            return Err(
                "--deterministic-ids (or CUTWEAVE_DETERMINISTIC_IDS) is for tests: seeded \
                 ids repeat across runs. A GUI engine refuses it unless --i-know-this-is-a-test \
                 is also passed"
                    .into(),
            );
        }
        Ok(())
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

/// `{"event":"starting","pid":N,"version":"...","elapsed_ms":T}` (`--progress`).
fn starting_line(elapsed: Duration) -> Value {
    json!({
        "event": "starting",
        "pid": std::process::id(),
        "version": VERSION,
        "elapsed_ms": crate::startup::ms(elapsed),
    })
}

/// Runs `start`, then `answer` with its result (the ready or error line). With `since`
/// (`--progress`) a ticker prints a `starting` line `every` so often ([`PROGRESS_INTERVAL`])
/// until then. `answer` writes under the ticker's lock, after marking the start done, so no
/// `starting` line ever follows it, and it never waits for the ticker thread to wake up and end
/// (on a busy machine that can take a second).
fn with_progress<W: Write + Send, T, R>(
    since: Option<Instant>,
    every: Duration,
    stdout: &mut W,
    start: impl FnOnce() -> T,
    answer: impl FnOnce(&mut W, T) -> R,
) -> R {
    let Some(since) = since else {
        let started = start();
        return answer(stdout, started);
    };
    // `true` once the start is answered: the ticker writes nothing more.
    let out = Mutex::new((false, stdout));
    let (stop, stopped) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let out = &out;
        let ticker = std::thread::Builder::new()
            .name("cutweave-progress".into())
            .spawn_scoped(scope, move || {
                while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(every) {
                    let mut guard = out.lock().unwrap_or_else(PoisonError::into_inner);
                    let (answered, stdout) = &mut *guard;
                    if *answered {
                        break;
                    }
                    let _ = emit(&mut **stdout, &starting_line(since.elapsed()));
                }
            });
        if let Err(e) = ticker {
            tracing::warn!(error = %e, "cannot start the progress ticker");
        }
        let started = start();
        let answered = {
            let mut guard = out.lock().unwrap_or_else(PoisonError::into_inner);
            guard.0 = true;
            answer(&mut *guard.1, started)
        };
        drop(stop);
        answered
    })
}

/// Why the start failed: the protocol's error line.
struct StartFailure {
    code: &'static str,
    exit_code: i32,
    message: String,
    extra: Value,
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
pub fn run(
    args: &EngineArgs,
    stdin: impl Read + Send + 'static,
    stdout: &mut (impl Write + Send),
) -> i32 {
    let began = Began::now();
    if args.progress {
        // The loader is done and the engine's own code runs.
        let _ = emit(stdout, &starting_line(Duration::ZERO));
    }
    let mode = args.mode();
    if let Err(message) = args.check_deterministic_ids() {
        return fail(stdout, "usage", EXIT_USAGE, &message, Value::Null);
    }
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
    let code = run_logged(args, mode, &began, stdin, stdout);
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

/// The MCP mount's options: rmcp's second-layer Origin check also accepts the origins the
/// launcher allowed (the server's own check, which runs first, uses the same list).
fn mcp_options(args: &EngineArgs) -> McpOptions {
    let mut options = McpOptions::default();
    options
        .allowed_origins
        .extend(args.base.allowed_origins.iter().cloned());
    options
}

/// The engine process's configuration.
fn engine_config(args: &EngineArgs, mode: EngineMode, ui_token: Option<Token>) -> EngineConfig {
    // The full engine (WP-1I.05): FFmpeg media when built with it, the GPU, audio and
    // playback. `--dev-stub-media` keeps the dev stub (made-up probing, no decoding) instead.
    let subsystems: Box<dyn crate::Subsystems> = if args.dev_stub_media {
        Box::new(crate::DevStubMedia)
    } else {
        let mut config = crate::FullConfig::native();
        if let Some(dir) = &args.cache_dir {
            config = config.with_cache_dir(dir);
        }
        Box::new(crate::FullSubsystems::new(config))
    };
    EngineConfig {
        mode,
        project: args.base.project.as_ref().map(PathBuf::from),
        port: Some(args.base.port),
        ui_token,
        deterministic_ids: args.deterministic_ids,
        allowed_roots: args.allow_roots.clone(),
        // A headless server has nobody listening: the null clock (`CUTWEAVE_AUDIO` is read
        // by the GUI engine's default output).
        audio: match mode {
            EngineMode::Gui => crate::AudioConfig::Default,
            _ => crate::AudioConfig::Null,
        },
        subsystems: Some(subsystems),
        api: ApiOptions {
            run_dir: args.base.run_dir.clone(),
            allowed_origins: args.base.allowed_origins.clone(),
            // `/rpc` (WP-1I.02) and `/mcp`, `/mcp/minimal`, `/mcp/full` (WP-1J.01).
            mounts: Some(crate::api_mounts(RpcConfig::default(), mcp_options(args))),
            ..ApiOptions::default()
        },
        ..EngineConfig::default()
    }
}

/// When [`run`] began: the start of the engine's own code (what the OS loader took before it
/// is [`unix_ms`](Self::unix_ms) minus the supervisor's spawn time).
struct Began {
    at: Instant,
    unix_ms: u64,
}

impl Began {
    fn now() -> Began {
        Began {
            at: Instant::now(),
            unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, crate::startup::ms),
        }
    }
}

/// The handshake (`Gui`), the media backend and [`Engine::start`].
fn start_engine(
    args: &EngineArgs,
    mode: EngineMode,
    stdin: impl Read + Send + 'static,
    tx: &mpsc::Sender<Input>,
    rx: &mpsc::Receiver<Input>,
    phases: &mut crate::startup::Phases,
) -> Result<Engine, StartFailure> {
    let ui_token = if mode == EngineMode::Gui {
        read_stdin(stdin, tx.clone());
        let token = handshake(rx, args.handshake_timeout).map_err(|message| StartFailure {
            code: "bad_handshake",
            exit_code: EXIT_HANDSHAKE,
            message,
            extra: Value::Null,
        })?;
        Some(token)
    } else {
        drop(stdin);
        None
    };
    phases.mark("handshake");
    let config = engine_config(args, mode, ui_token);
    phases.mark("media_backend");
    Engine::start(config).map_err(|e| StartFailure {
        code: e.code(),
        exit_code: e.exit_code(),
        message: e.to_string(),
        extra: match &e {
            crate::engine::StartError::ProjectLocked {
                holder: Some(h), ..
            } => json!({"holder": {"pid": h.pid, "host": h.host, "started": h.started}}),
            _ => Value::Null,
        },
    })
}

fn run_logged(
    args: &EngineArgs,
    mode: EngineMode,
    began: &Began,
    stdin: impl Read + Send + 'static,
    stdout: &mut (impl Write + Send),
) -> i32 {
    tracing::info!(
        mode = mode.name(),
        port = args.base.port,
        pid = std::process::id(),
        version = VERSION,
        progress = args.progress,
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
    let since = args.progress.then_some(began.at);
    let (answered, phases) = with_progress(
        since,
        PROGRESS_INTERVAL,
        stdout,
        || {
            let mut phases = crate::startup::Phases::new();
            let started = start_engine(args, mode, stdin, &tx, &rx, &mut phases);
            phases.mark("engine");
            (started, phases)
        },
        |stdout, (started, mut phases)| {
            let answered = match started {
                Ok(engine) => {
                    let ready = ready_line(engine.port().unwrap_or(0), std::process::id(), VERSION);
                    if writeln!(stdout, "{ready}")
                        .and_then(|()| stdout.flush())
                        .is_ok()
                    {
                        Ok(engine)
                    } else {
                        engine.shutdown();
                        Err(EXIT_FAILURE)
                    }
                }
                Err(f) => Err(fail(stdout, f.code, f.exit_code, &f.message, f.extra)),
            };
            phases.mark("ready_line");
            (answered, phases)
        },
    );
    let engine = match answered {
        Ok(engine) => engine,
        Err(code) => return code,
    };
    tracing::info!(
        phases = %phases,
        since_run_ms = crate::startup::ms(began.at.elapsed()),
        run_at_unix_ms = began.unix_ms,
        "launch phases"
    );
    if args.dev_stub_media {
        tracing::warn!(
            "DEV ONLY: stub media backend enabled; imported files get made-up probe data and \
             nothing can be decoded or played (without --dev-stub-media the engine runs the \
             FFmpeg backend, playback and rendering)"
        );
        eprintln!("cutweave engine: warning: dev stub media is on (fake media probing, dev only)");
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
    let env = std::env::var(DETERMINISTIC_IDS_ENV).ok();
    let stub_env = std::env::var(crate::DEV_STUB_MEDIA_ENV).ok();
    match EngineArgs::parse_with_env(std::env::args().skip(1), env.as_deref()) {
        Ok(args) => run(
            &args.with_dev_stub_env(stub_env.as_deref()),
            std::io::stdin(),
            &mut stdout,
        ),
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
        assert!(!a.progress);
        assert_eq!(parse("--headless").unwrap().mode(), EngineMode::Headless);
        assert!(parse("--port 0 --progress").unwrap().progress);
    }

    fn json_lines(out: &[u8]) -> Vec<Value> {
        String::from_utf8(out.to_vec())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn progress_lines_tick_while_the_engine_starts_and_stop_before_it_answers() {
        let mut out = Vec::new();
        let answer = with_progress(
            Some(Instant::now()),
            Duration::from_millis(10),
            &mut out,
            || {
                std::thread::sleep(Duration::from_millis(500));
                42
            },
            |out: &mut Vec<u8>, n| {
                out.extend_from_slice(b"{\"event\":\"ready\"}\n");
                n + 1
            },
        );
        assert_eq!(answer, 43);
        let mut lines = json_lines(&out);
        // The answer is the last line: no starting line follows it.
        assert_eq!(lines.pop().unwrap()["event"], "ready");
        assert!(!lines.is_empty(), "no starting line in 500 ms");
        let mut last = 0;
        for line in &lines {
            assert_eq!(line["event"], "starting", "{line}");
            assert_eq!(line["pid"], std::process::id());
            assert_eq!(line["version"], VERSION);
            let elapsed = line["elapsed_ms"].as_u64().unwrap();
            assert!(elapsed >= last, "{lines:?}");
            last = elapsed;
        }
        // Without --progress nothing is printed.
        let mut quiet = Vec::new();
        with_progress(
            None,
            Duration::from_millis(1),
            &mut quiet,
            || std::thread::sleep(Duration::from_millis(50)),
            |_, ()| (),
        );
        assert_eq!(quiet, Vec::<u8>::new());
    }

    #[test]
    fn a_progress_engine_says_it_is_starting_before_anything_else() {
        let args = parse("--port 0 --no-log --deterministic-ids 42 --progress").unwrap();
        let mut out = Vec::new();
        assert_eq!(run(&args, std::io::empty(), &mut out), EXIT_USAGE);
        let lines = json_lines(&out);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0]["event"], "starting");
        assert_eq!(lines[0]["elapsed_ms"], 0);
        assert_eq!(lines[1]["event"], "error");
        assert_eq!(lines[1]["code"], "usage");
    }

    #[test]
    fn the_cache_dir_flag_names_the_cache_folder() {
        assert_eq!(parse("--headless").unwrap().cache_dir, None);
        assert_eq!(
            parse("--headless --cache-dir /c").unwrap().cache_dir,
            Some(PathBuf::from("/c"))
        );
        assert!(parse("--cache-dir").is_err());
    }

    #[test]
    fn rejects_bad_flags() {
        assert!(parse("--bogus").is_err());
        assert!(parse("--deterministic-ids x").is_err());
        assert!(parse("--deterministic-ids").is_err());
        assert!(parse("--no-log --log-dir /l").is_err());
        assert!(parse("--port 99999").is_err());
    }

    fn parse_env(s: &str, env: Option<&str>) -> Result<EngineArgs, String> {
        EngineArgs::parse_with_env(s.split_whitespace().map(str::to_owned), env)
    }

    #[test]
    fn the_env_var_seeds_ids_unless_the_flag_is_given() {
        assert_eq!(
            parse_env("--headless", Some("42"))
                .unwrap()
                .deterministic_ids,
            Some(42)
        );
        assert_eq!(
            parse_env("--headless --deterministic-ids 7", Some("42"))
                .unwrap()
                .deterministic_ids,
            Some(7)
        );
        assert_eq!(parse_env("", Some(" ")).unwrap().deterministic_ids, None);
        assert_eq!(parse_env("", None).unwrap().deterministic_ids, None);
        assert!(parse_env("", Some("forty-two")).is_err());
    }

    #[test]
    fn gui_mode_refuses_deterministic_ids_without_the_test_flag() {
        let refused = parse("--port 1 --deterministic-ids 42").unwrap();
        let err = refused.check_deterministic_ids().unwrap_err();
        assert!(err.contains("--i-know-this-is-a-test"), "{err}");
        let from_env = parse_env("--port 1", Some("42")).unwrap();
        assert!(from_env.check_deterministic_ids().is_err());
        for ok in [
            "--port 1 --deterministic-ids 42 --i-know-this-is-a-test",
            "--headless --port 1 --deterministic-ids 42",
            "--port 1",
        ] {
            assert!(parse(ok).unwrap().check_deterministic_ids().is_ok(), "{ok}");
        }
    }

    #[test]
    fn a_gui_engine_exits_2_before_the_handshake_when_deterministic_ids_are_refused() {
        let args = parse("--port 0 --no-log --deterministic-ids 42").unwrap();
        let mut out = Vec::new();
        let code = run(&args, std::io::empty(), &mut out);
        assert_eq!(code, EXIT_USAGE);
        let line: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(line["event"], "error");
        assert_eq!(line["code"], "usage");
        assert_eq!(line["exit_code"], 2);
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
