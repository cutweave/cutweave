// SPDX-License-Identifier: GPL-3.0-or-later
//! Acceptance test 4 (arch §6, §18): HTTP without/with a wrong token -> 401; wrong Host or a
//! foreign Origin -> 403 (DNS-rebinding defence, checked before auth); a UI-only scope from an
//! agent token -> `forbidden`, on every path; the actor is derived from the token server-side;
//! the run-file holds the agent token and is removed on exit.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{
    ALLOWED_ORIGIN, Revision, discover_body, engine, good_headers, http_client, http_server,
    in_process, initialize_body, raw_post,
};
use rmcp::model::CallToolRequestParams;
use s5_bus::Actor;
use s5_mcp::{
    EngineHandle, Profile,
    auth::{AgentToken, RunFile, authenticate, constant_time_eq},
    http::{self, HttpOptions},
};
use serde_json::{Value, json};

#[tokio::test]
async fn http_requires_the_agent_bearer_token() {
    let (server, token) = http_server(engine()).await;
    let host = format!("127.0.0.1:{}", server.addr().port());
    let good = format!("Bearer {}", token.secret);
    for profile in Profile::ALL {
        let path = profile.http_path();
        // Missing token.
        let r = raw_post(
            server.addr(),
            path,
            &[("Host", &host), ("Content-Type", "application/json")],
            &initialize_body(),
        )
        .await;
        assert_eq!(r.status, 401, "{path}: {}", r.body);
        assert_eq!(
            r.header("www-authenticate"),
            Some("Bearer realm=\"cutweave\"")
        );
        // Wrong token, wrong scheme, token of the right length but one char off.
        let mut off_by_one = token.secret.clone();
        off_by_one.replace_range(
            0..1,
            if off_by_one.starts_with('0') {
                "1"
            } else {
                "0"
            },
        );
        for bad in [
            "Bearer nope".to_owned(),
            format!("Basic {}", token.secret),
            format!("Bearer {off_by_one}"),
            token.secret.clone(),
        ] {
            let r = raw_post(
                server.addr(),
                path,
                &good_headers(&host, &bad),
                &initialize_body(),
            )
            .await;
            assert_eq!(r.status, 401, "{path} with `{bad}`");
        }
        // Right token: both revisions get in.
        let r = raw_post(
            server.addr(),
            path,
            &good_headers(&host, &good),
            &initialize_body(),
        )
        .await;
        assert_eq!(r.status, 200, "{path}: {}", r.body);
    }
    server.shutdown().await;
}

#[tokio::test]
async fn http_checks_host_and_origin_before_auth() {
    let (server, token) = http_server(engine()).await;
    let port = server.addr().port();
    let good = format!("Bearer {}", token.secret);
    let ok_host = format!("127.0.0.1:{port}");

    // DNS rebinding: a foreign Host is refused even with a valid token, and without one (403
    // first, so a rebinding page cannot even probe for a 401).
    for host in [
        format!("evil.example:{port}"),
        format!("127.0.0.1:{}", port.wrapping_add(1)),
        "127.0.0.1".to_owned(),
        format!("127.0.0.1.nip.io:{port}"),
    ] {
        for auth in [good.as_str(), "Bearer nope"] {
            let r = raw_post(
                server.addr(),
                "/mcp",
                &good_headers(&host, auth),
                &initialize_body(),
            )
            .await;
            assert_eq!(r.status, 403, "Host {host}");
        }
    }
    let localhost = format!("localhost:{port}");
    let r = raw_post(
        server.addr(),
        "/mcp",
        &good_headers(&localhost, &good),
        &initialize_body(),
    )
    .await;
    assert_eq!(r.status, 200, "localhost:<port> is allowed");

    // Browser Origins: only the allowlist; `null` and look-alikes are refused.
    for origin in [
        "https://evil.example",
        "null",
        "http://localhost:6275",
        "http://localhost",
        "file://",
    ] {
        let mut headers = good_headers(&ok_host, &good);
        headers.push(("Origin", origin));
        let r = raw_post(server.addr(), "/mcp", &headers, &initialize_body()).await;
        assert_eq!(r.status, 403, "Origin {origin}");
    }
    let mut headers = good_headers(&ok_host, &good);
    headers.push(("Origin", ALLOWED_ORIGIN));
    let r = raw_post(server.addr(), "/mcp", &headers, &initialize_body()).await;
    assert_eq!(r.status, 200, "allowlisted Origin: {}", r.body);

    // Same for 2026-07-28 requests.
    let evil = format!("evil.example:{port}");
    let mut headers = good_headers(&evil, &good);
    headers.push(("MCP-Protocol-Version", "2026-07-28"));
    let r = raw_post(server.addr(), "/mcp", &headers, &discover_body()).await;
    assert_eq!(r.status, 403);
    server.shutdown().await;
}

/// arch §6: an agent credential can never use a UI-only scope, whichever way it tries.
#[tokio::test]
async fn ui_only_scope_is_forbidden_for_agent_tokens() {
    let engine = engine();
    let (server, token) = http_server(engine.clone()).await;
    for revision in Revision::ALL {
        let client = http_client(&server.url(Profile::Full), &token.secret, revision).await;
        // Not listed as a tool in any profile ...
        for profile in Profile::ALL {
            assert!(
                s5_mcp::Catalog::shared(profile)
                    .tools()
                    .iter()
                    .all(|t| t.name != "ai_set_key")
            );
        }
        // ... so calling it as a tool is a protocol error (unknown tool),
        let as_tool = client
            .service
            .call_tool(
                CallToolRequestParams::new("ai_set_key").with_arguments(
                    json!({ "provider": "anthropic", "key": "sk-x" })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await;
        assert!(as_tool.is_err());
        // ... discoverable as UI-only,
        let d = client
            .ok("cutweave_describe_command", json!({ "name": "ai.set_key" }))
            .await;
        assert_eq!(d["ui_only"], true);
        assert!(d["call"].as_str().unwrap().starts_with("UI only"));
        // ... and refused by the engine through the generic path, alone or inside a batch.
        for steps in [
            json!([{ "name": "ai.set_key", "params": { "provider": "anthropic", "key": "sk-x" } }]),
            json!([
                { "name": "media.import", "params": { "path": "/a.mp4" } },
                { "name": "ai_set_key", "params": { "provider": "anthropic", "key": "sk-x" } }
            ]),
        ] {
            let e = client
                .err("cutweave_execute", json!({ "steps": steps }))
                .await;
            assert_eq!(e["code"], "forbidden", "{e}");
            assert_eq!(e["scope"], "ui");
        }
        client.close().await;
    }
    engine.with(|e| {
        assert!(!e.has_secret("anthropic"));
        assert_eq!(e.rev().0, 0, "nothing was applied");
    });
    server.shutdown().await;

    // stdio agents are agents too.
    let client = in_process(engine.clone(), Profile::Standard, Revision::Modern).await;
    let e = client
        .err(
            "cutweave_execute",
            json!({ "steps": [{ "name": "ai.set_key", "params": { "provider": "p", "key": "k" } }] }),
        )
        .await;
    assert_eq!(e["code"], "forbidden");
    client.close().await;
}

/// The actor comes from the token: two tokens, two actors, separate undo histories.
#[tokio::test]
async fn actor_is_derived_from_the_token() {
    let engine = EngineHandle::new(s5_bus::EngineConfig::deterministic(1));
    let alice = AgentToken::generate("alice").unwrap();
    let bob = AgentToken::generate("bob").unwrap();
    let server = http::serve(
        engine.clone(),
        HttpOptions {
            port: 0,
            tokens: vec![alice.clone(), bob.clone()],
            allowed_origins: vec![],
        },
    )
    .await
    .unwrap();
    let url = server.url(Profile::Standard);
    let a = http_client(&url, &alice.secret, Revision::Modern).await;
    let b = http_client(&url, &bob.secret, Revision::Legacy).await;
    a.ok("media_import", json!({ "path": "/a.mp4" })).await;
    let nothing = b.ok("edit_undo", json!({})).await;
    assert_eq!(
        nothing["result"]["step"],
        Value::Null,
        "bob cannot undo alice's edit"
    );
    engine.with(|e| {
        assert_eq!(e.undo_depth(&Actor::Agent("alice".into())), 1);
        assert_eq!(e.undo_depth(&Actor::Agent("bob".into())), 0);
    });
    let undone = a.ok("edit_undo", json!({})).await;
    assert_eq!(undone["result"]["step"]["label"], "media.import");
    a.close().await;
    b.close().await;
    server.shutdown().await;
}

#[test]
fn token_helpers() {
    let t = AgentToken::generate("x").unwrap();
    assert_eq!(t.secret.len(), 64);
    assert!(t.secret.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(t.secret, AgentToken::generate("x").unwrap().secret);
    assert!(
        !format!("{t:?}").contains(&t.secret),
        "Debug redacts the secret"
    );
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"abcd"));
    let tokens = [t.clone()];
    let header = format!("Bearer {}", t.secret);
    assert_eq!(
        authenticate(Some(&header), &tokens).map(|t| &t.label),
        Some(&"x".to_owned())
    );
    assert!(authenticate(None, &tokens).is_none());
    assert!(authenticate(Some("Bearer "), &tokens).is_none());
}

#[test]
fn run_file_holds_the_agent_token_and_is_removed() {
    let dir = std::env::temp_dir().join(format!("s5-mcp-runfile-{}", std::process::id()));
    let path = dir.join("run.json");
    let token = AgentToken::generate("agent").unwrap();
    let addr: std::net::SocketAddr = "127.0.0.1:4321".parse().unwrap();
    {
        let run_file = RunFile::write(&path, addr, &token).unwrap();
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(run_file.path()).unwrap()).unwrap();
        assert_eq!(v["port"], 4321);
        assert_eq!(v["url"], "http://127.0.0.1:4321/mcp");
        assert_eq!(
            v["endpoints"]["minimal"],
            "http://127.0.0.1:4321/mcp/minimal"
        );
        assert_eq!(v["agent_token"], token.secret.as_str());
        assert_eq!(v["pid"], std::process::id());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
    assert!(!path.exists(), "removed on drop");
    let _ = std::fs::remove_dir(dir);
}
