// SPDX-License-Identifier: GPL-3.0-or-later
//! The spawn handshake against the real binary, as `EngineSupervisor` drives it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{process::Stdio, time::Duration};

use common::Req;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    time::timeout,
};

const UI: &str = "handshakehandshakehandshakehandshakehandshakehandshake0123456789ab";

struct Engine {
    child: Child,
    stdin: Option<ChildStdin>,
    ready: serde_json::Value,
}

fn command(run_dir: &std::path::Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cutweave-api-serve"));
    cmd.arg("engine")
        .args(args)
        .arg("--run-dir")
        .arg(run_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    cmd
}

async fn spawn(run_dir: &std::path::Path, port: u16) -> Engine {
    let mut child = command(
        run_dir,
        &[
            "--port",
            &port.to_string(),
            "--token-stdin",
            "--allow-origin",
            "app://cutweave",
        ],
    )
    .spawn()
    .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(format!("{UI}\n").as_bytes()).await.unwrap();
    stdin.flush().await.unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let line = timeout(Duration::from_secs(20), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ready: serde_json::Value = serde_json::from_str(&line).unwrap();
    // Nothing else is printed to stdout besides the readiness line (and never the token).
    assert!(!line.contains(UI));
    Engine {
        child,
        stdin: Some(stdin),
        ready,
    }
}

#[tokio::test]
async fn readiness_line_and_token_handshake() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = spawn(dir.path(), 0).await;
    // Shape expected by EngineSupervisor's parseReadyLine.
    assert_eq!(e.ready["event"], "ready");
    let port = u16::try_from(e.ready["port"].as_u64().unwrap()).unwrap();
    assert!(port > 0);
    assert_eq!(e.ready["pid"], e.child.id().unwrap());
    assert!(e.ready["version"].is_string());

    // The UI token from stdin authenticates; nothing else does.
    let ok = Req::get("/rpc").bearer(UI).send(port).await;
    assert_eq!(ok.status, 501);
    assert_eq!(Req::get("/rpc").send(port).await.status, 401);
    // The browser origin configured by the launcher is allowed.
    let origin = Req::get("/health")
        .header("Origin", "app://cutweave")
        .send(port)
        .await;
    assert_eq!(
        origin.header("access-control-allow-origin").as_deref(),
        Some("app://cutweave")
    );

    // The UI token never reached the run-file or anything else in the data dir.
    let mut found_run_file = false;
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        found_run_file = true;
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!text.contains(UI));
        assert!(text.contains("agent_tokens"));
    }
    assert!(found_run_file);

    // Closing the handshake pipe is the graceful stop: exit 0 and the run-file is removed.
    drop(e.stdin.take());
    let status = timeout(Duration::from_secs(20), e.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn exits_12_when_the_port_is_taken() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = blocker.local_addr().unwrap().port();
    let mut child = command(dir.path(), &["--port", &port.to_string(), "--token-stdin"])
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(format!("{UI}\n").as_bytes()).await.unwrap();
    let status = timeout(Duration::from_secs(20), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(12));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn bad_handshakes_exit_13() {
    let dir = tempfile::tempdir().unwrap();
    // Too-short token.
    let mut child = command(dir.path(), &["--token-stdin"]).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"short\n")
        .await
        .unwrap();
    let status = timeout(Duration::from_secs(20), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(13));
    // EOF before any token.
    let mut child = command(dir.path(), &["--token-stdin"]).spawn().unwrap();
    drop(child.stdin.take());
    let status = timeout(Duration::from_secs(20), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(13));
    // A headless engine rejects any UI-credential handshake.
    let mut child = command(dir.path(), &["--headless", "--token-stdin"])
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{UI}\n").as_bytes())
        .await
        .unwrap();
    let status = timeout(Duration::from_secs(20), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(13));
}

#[tokio::test]
async fn usage_errors_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = command(dir.path(), &["--bogus"]).spawn().unwrap();
    let status = timeout(Duration::from_secs(20), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}
