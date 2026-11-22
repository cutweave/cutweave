// SPDX-License-Identifier: GPL-3.0-or-later
//! The spawn handshake between Electron main's `EngineSupervisor` and the engine
//! (arch §6; `apps/desktop/src/main/engine/supervisor.ts`).
//!
//! ```text
//! engine [engine] --port N --token-stdin [--headless] [--project P] [--run-dir D] [--allow-origin O]...
//! stdin  : first line = the UI token (kept in memory; never argv, env, disk or logs)
//! stdout : one JSON line once listening: {"event":"ready","port":P,"pid":N,"version":"..."}
//! exit   : stdin EOF or SIGINT/SIGTERM = graceful stop; 12 = port could not be bound;
//!          13 = missing or invalid handshake token; 2 = usage error
//! ```

use std::{io::Write, path::PathBuf};

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

use crate::{
    creds::Token,
    server::{ApiConfig, ApiServer},
};

/// Usage error.
pub const EXIT_USAGE: i32 = 2;
/// The handshake token was missing, malformed, or given to a headless engine.
pub const EXIT_HANDSHAKE: i32 = 13;
/// Any other startup failure.
pub const EXIT_FAILURE: i32 = 1;

/// Parsed launch arguments.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LaunchArgs {
    /// `--port N` (default 0 = random).
    pub port: u16,
    /// `--token-stdin`: read the UI token from the first stdin line.
    pub token_stdin: bool,
    /// `--headless`: no UI credential class; a UI handshake is rejected.
    pub headless: bool,
    /// `--project P`.
    pub project: Option<String>,
    /// `--run-dir D` (default `~/.cutweave/run`).
    pub run_dir: Option<PathBuf>,
    /// `--allow-origin O` (repeatable), e.g. `app://cutweave`.
    pub allowed_origins: Vec<String>,
}

impl LaunchArgs {
    /// Parses the arguments after the program name. A leading bare `engine` is accepted.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<LaunchArgs, String> {
        let mut out = LaunchArgs::default();
        let mut it = args.into_iter().peekable();
        if it.peek().is_some_and(|a| a == "engine") {
            it.next();
        }
        while let Some(arg) = it.next() {
            let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--port" => {
                    out.port = value("--port")?
                        .parse()
                        .map_err(|_| "--port must be 0..=65535".to_owned())?;
                }
                "--token-stdin" => out.token_stdin = true,
                "--headless" => out.headless = true,
                "--project" => out.project = Some(value("--project")?),
                "--run-dir" => out.run_dir = Some(PathBuf::from(value("--run-dir")?)),
                "--allow-origin" => out.allowed_origins.push(value("--allow-origin")?),
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        Ok(out)
    }
}

/// The readiness line (without the newline).
#[must_use]
pub fn ready_line(port: u16, pid: u32, version: &str) -> String {
    serde_json::json!({ "event": "ready", "port": port, "pid": pid, "version": version })
        .to_string()
}

/// Runs the engine skeleton to completion and returns the process exit code.
///
/// `stdin` is the handshake pipe; `stdout` receives the single readiness line.
pub async fn run<R, W>(args: LaunchArgs, mut stdin: R, mut stdout: W) -> i32
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: Write,
{
    if args.headless && args.token_stdin {
        eprintln!("a headless engine has no UI credential; refusing the token handshake");
        return EXIT_HANDSHAKE;
    }
    let ui_token = if args.token_stdin {
        let mut line = String::new();
        let read = (&mut stdin).take(1024).read_line(&mut line).await;
        match read.map_err(|e| e.to_string()).and_then(|_| {
            Token::parse(line.trim_end_matches(['\r', '\n'])).map_err(|e| e.to_string())
        }) {
            Ok(token) => Some(token),
            Err(reason) => {
                eprintln!("invalid handshake token: {reason}");
                return EXIT_HANDSHAKE;
            }
        }
    } else {
        None
    };

    let config = ApiConfig {
        port: args.port,
        run_dir: args.run_dir,
        ui_token,
        project: args.project,
        allowed_origins: args.allowed_origins,
        ..ApiConfig::default()
    };
    let server = match ApiServer::start(config).await {
        Ok(server) => server,
        Err(error) => {
            eprintln!("engine failed to start: {error}");
            return error.exit_code();
        }
    };

    let line = ready_line(server.port(), std::process::id(), server.version());
    if writeln!(stdout, "{line}")
        .and_then(|()| stdout.flush())
        .is_err()
    {
        server.shutdown().await;
        return EXIT_FAILURE;
    }

    // Stdin EOF (the supervisor closed the pipe, or died) is an orphan-prevention stop.
    let handle = server.shutdown_handle();
    if args.token_stdin {
        tokio::spawn(async move {
            let mut sink = String::new();
            loop {
                sink.clear();
                match stdin.read_line(&mut sink).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            handle.shutdown();
        });
    }
    let signals = shutdown_signal();
    let stopped = server.shutdown_handle();
    tokio::spawn(async move {
        signals.await;
        stopped.shutdown();
    });
    server.wait().await;
    0
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<LaunchArgs, String> {
        LaunchArgs::parse(s.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn parses_supervisor_invocation() {
        let a = parse("engine --port 4321 --token-stdin").unwrap();
        assert_eq!(a.port, 4321);
        assert!(a.token_stdin);
        assert!(!a.headless);
    }

    #[test]
    fn rejects_unknown_and_bad_values() {
        assert!(parse("--bogus").is_err());
        assert!(parse("--port 70000").is_err());
        assert!(parse("--port").is_err());
    }

    #[test]
    fn ready_line_matches_supervisor_parser() {
        let v: serde_json::Value = serde_json::from_str(&ready_line(9, 3, "x")).unwrap();
        assert_eq!(v["event"], "ready");
        assert_eq!(v["port"], 9);
        assert_eq!(v["pid"], 3);
        assert_eq!(v["version"], "x");
    }
}
