// SPDX-License-Identifier: GPL-3.0-or-later
//! Integration tests against a real loopback listener (WP-1I.01 acceptance criteria).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Req;
use cutweave_api::{ApiConfig, ApiError, ApiServer, RunFile, RunningServer, Token};

const UI: &str = "uiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiuiui";

async fn start(dir: &tempfile::TempDir, headless: bool) -> RunningServer {
    ApiServer::start(ApiConfig {
        run_dir: Some(dir.path().to_path_buf()),
        ui_token: (!headless).then(|| Token::parse(UI).unwrap()),
        allowed_origins: vec!["app://cutweave".into()],
        enable_test_routes: true,
        ..ApiConfig::default()
    })
    .await
    .unwrap()
}

fn agent(server: &RunningServer, label: &str) -> String {
    server
        .credentials()
        .agent_tokens()
        .iter()
        .find(|a| a.label == label)
        .unwrap()
        .token
        .expose()
        .to_owned()
}

#[tokio::test]
async fn missing_token_is_401() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    let r = Req::get("/rpc").send(s.port()).await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json()["error"]["code"], "unauthorized");
    assert_eq!(r.header("www-authenticate").as_deref(), Some("Bearer"));
}

#[tokio::test]
async fn wrong_and_malformed_tokens_are_401() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    for req in [
        Req::get("/rpc").bearer("wrong"),
        Req::get("/rpc").bearer(&format!("{UI}x")),
        Req::get("/rpc").bearer(""),
        Req::get("/rpc").header("Authorization", UI),
        Req::get("/rpc").header("Authorization", &format!("Basic {UI}")),
        Req::get(&format!("/rpc?token={UI}")),
        Req::get(&format!("/rpc?access_token={UI}")),
    ] {
        assert_eq!(req.send(s.port()).await.status, 401);
    }
}

#[tokio::test]
async fn bad_host_is_403_even_with_a_valid_token() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    let wrong_port = format!("127.0.0.1:{}", s.port().wrapping_add(1));
    for host in ["evil.com", "evil.com:80", "localhost", wrong_port.as_str()] {
        let r = Req::get("/rpc").host(host).bearer(UI).send(s.port()).await;
        assert_eq!(r.status, 403, "host {host}");
        assert_eq!(r.json()["error"]["code"], "bad_host");
    }
    let r = Req::get("/health").host("evil.com").send(s.port()).await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn bad_origin_is_403_even_with_a_valid_token() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    for origin in [
        "https://evil.com",
        "null",
        "http://localhost:1",
        "app://cutweave.evil",
    ] {
        let r = Req::get("/rpc")
            .bearer(UI)
            .header("Origin", origin)
            .send(s.port())
            .await;
        assert_eq!(r.status, 403, "origin {origin}");
        assert_eq!(r.json()["error"]["code"], "bad_origin");
    }
    // Two Origin headers are ambiguous: rejected.
    let r = Req::get("/rpc")
        .bearer(UI)
        .header("Origin", "app://cutweave")
        .header("Origin", "https://evil.com")
        .send(s.port())
        .await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn valid_requests_pass() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    let p = s.port();
    // Authenticated but nothing mounted yet.
    assert_eq!(Req::get("/rpc").bearer(UI).send(p).await.status, 501);
    assert_eq!(
        Req::get("/mcp")
            .bearer(&agent(&s, "mcp"))
            .send(p)
            .await
            .status,
        501
    );
    // Health needs no token; localhost Host is accepted.
    let h = Req::get("/health")
        .host(&format!("localhost:{p}"))
        .send(p)
        .await;
    assert_eq!(h.status, 200);
    assert_eq!(h.json()["status"], "ok");
    // The launcher-configured origin and the engine's own origin are accepted, with CORS.
    for origin in ["app://cutweave".to_owned(), format!("http://127.0.0.1:{p}")] {
        let r = Req::get("/_test/whoami")
            .bearer(UI)
            .header("Origin", &origin)
            .send(p)
            .await;
        assert_eq!(r.status, 200);
        assert_eq!(
            r.header("access-control-allow-origin").as_deref(),
            Some(origin.as_str())
        );
    }
    assert_eq!(Req::get("/nope").send(p).await.status, 404);
}

#[tokio::test]
async fn preflight_is_answered_without_a_token_for_allowed_origins_only() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    let ok = Req::get("/rpc")
        .method("OPTIONS")
        .header("Origin", "app://cutweave")
        .header("Access-Control-Request-Method", "POST")
        .send(s.port())
        .await;
    assert_eq!(ok.status, 204);
    assert!(
        ok.header("access-control-allow-headers")
            .unwrap()
            .contains("authorization")
    );
    let bad = Req::get("/rpc")
        .method("OPTIONS")
        .header("Origin", "https://evil.com")
        .header("Access-Control-Request-Method", "POST")
        .send(s.port())
        .await;
    assert_eq!(bad.status, 403);
}

#[tokio::test]
async fn websocket_subprotocol_token_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    let ok = Req::get("/_test/whoami")
        .header(
            "Sec-WebSocket-Protocol",
            &format!("other, cutweave.token.{UI}"),
        )
        .send(s.port())
        .await;
    assert_eq!(ok.status, 200);
    let bad = Req::get("/_test/whoami")
        .header("Sec-WebSocket-Protocol", "cutweave.token.nope")
        .send(s.port())
        .await;
    assert_eq!(bad.status, 401);
}

#[tokio::test]
async fn agent_token_cannot_use_ui_only_scope() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    for label in ["cli", "mcp"] {
        let r = Req::get("/_test/ui_only")
            .bearer(&agent(&s, label))
            .send(s.port())
            .await;
        assert_eq!(r.status, 403, "{label}");
        assert_eq!(r.json()["error"]["code"], "forbidden");
    }
    let ui = Req::get("/_test/ui_only").bearer(UI).send(s.port()).await;
    assert_eq!(ui.status, 200);
    assert_eq!(ui.json()["actor"], "user");
}

#[tokio::test]
async fn actor_is_derived_from_the_credential_and_never_from_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    let p = s.port();
    let forged = r#"{"params":{"_meta":{"actor":"user"}},"actor":"user"}"#;
    let ui = Req::get("/_test/whoami")
        .method("POST")
        .bearer(UI)
        .body(forged)
        .send(p)
        .await;
    assert_eq!(ui.json()["actor"], "user");
    let mcp = Req::get("/_test/whoami")
        .method("POST")
        .bearer(&agent(&s, "mcp"))
        .header("X-Cutweave-Actor", "user")
        .body(forged)
        .send(p)
        .await;
    assert_eq!(mcp.json()["actor"], "agent:mcp");
    let cli = Req::get("/_test/whoami")
        .bearer(&agent(&s, "cli"))
        .send(p)
        .await;
    assert_eq!(cli.json()["actor"], "agent:cli");
    // Each authenticated request gets its own connection id.
    assert_ne!(mcp.json()["conn"], cli.json()["conn"]);
}

#[tokio::test]
async fn headless_server_has_no_ui_credential_class() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, true).await;
    assert!(!s.credentials().has_ui());
    assert_eq!(
        Req::get("/_test/whoami")
            .bearer(UI)
            .send(s.port())
            .await
            .status,
        401
    );
    assert_eq!(
        Req::get("/_test/whoami")
            .bearer(&agent(&s, "cli"))
            .send(s.port())
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn responses_carry_request_ids() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    let a = Req::get("/health").send(s.port()).await;
    let b = Req::get("/rpc").send(s.port()).await;
    let (ia, ib) = (
        a.header("x-request-id").unwrap(),
        b.header("x-request-id").unwrap(),
    );
    assert!(ia.starts_with("req_"));
    assert_ne!(ia, ib);
    assert_eq!(b.json()["request_id"], ib.as_str());
}

#[tokio::test]
async fn run_file_has_agent_tokens_and_never_the_ui_token() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    let path = s.run_file_path().to_path_buf();
    assert_eq!(
        path,
        dir.path().join(format!("{}.json", std::process::id()))
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["pid"], std::process::id());
    assert_eq!(v["port"], s.port());
    let labels: Vec<_> = v["agent_tokens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["label"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(labels, ["cli", "mcp"]);
    assert_eq!(v["agent_tokens"][1]["token"], agent(&s, "mcp").as_str());
    assert_eq!(agent(&s, "cli").len(), 64);
    // The UI credential is nowhere on disk in the run directory.
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let content = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&content).contains(UI));
    }
    // The file is discoverable while the engine lives.
    assert_eq!(RunFile::discover(dir.path()).len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn run_file_mode_is_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    let mode = std::fs::metadata(s.run_file_path())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[cfg(windows)]
#[tokio::test]
async fn run_file_dacl_grants_only_the_current_user() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    let out = std::process::Command::new("icacls")
        .arg(s.run_file_path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let aces = text.matches(":(").count();
    let user = std::env::var("USERNAME").unwrap().to_lowercase();
    assert_eq!(aces, 1, "expected exactly one ACE:\n{text}");
    let first = text.lines().next().unwrap();
    let ace = first.split(".json").nth(1).unwrap().trim();
    let principal = ace.split(":(").next().unwrap().to_lowercase();
    assert!(
        principal.ends_with(&format!("\\{user}")),
        "ACE is not for {user}:\n{text}"
    );
    assert!(ace.ends_with(":(F)"), "unexpected rights:\n{text}");
}

#[tokio::test]
async fn shutdown_removes_the_run_file_and_stops_listening() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    let (port, path) = (s.port(), s.run_file_path().to_path_buf());
    assert!(path.exists());
    s.shutdown().await;
    assert!(!path.exists());
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn dropping_the_server_removes_the_run_file() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(&dir, false).await;
    let path = s.run_file_path().to_path_buf();
    drop(s);
    assert!(!path.exists());
}

#[tokio::test]
async fn stale_run_files_are_ignored_and_reaped_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let dead = u32::MAX - 1;
    let stale = dir.path().join(format!("{dead}.json"));
    let body = serde_json::json!({
        "pid": dead, "port": 1, "agent_tokens": [], "project": null, "started": 0, "version": "0"
    });
    std::fs::write(&stale, body.to_string()).unwrap();
    assert!(RunFile::discover(dir.path()).is_empty());
    let s = start(&dir, false).await;
    assert_eq!(RunFile::discover(dir.path()).len(), 1);
    // Reaped in the background, after the server's own run-file is written.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while stale.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the stale run-file was not reaped"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    drop(s);
}

#[tokio::test]
async fn port_in_use_is_a_bind_error_with_exit_code_12() {
    let dir = tempfile::tempdir().unwrap();
    let first = start(&dir, false).await;
    let dir2 = tempfile::tempdir().unwrap();
    let err = ApiServer::start(ApiConfig {
        port: first.port(),
        run_dir: Some(dir2.path().to_path_buf()),
        ..ApiConfig::default()
    })
    .await
    .err()
    .expect("second bind must fail");
    assert!(matches!(err, ApiError::Bind { .. }));
    assert_eq!(err.exit_code(), 12);
    // No run-file is left behind by the failed start.
    assert_eq!(std::fs::read_dir(dir2.path()).unwrap().count(), 0);
}
