// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1J.03 in-process tests of the stdio transports, on a real engine (the WP-1J.01 harness),
//! with rmcp clients on both spec revisions over in-memory pipes:
//!
//! - [`stdio::serve`]: the handler over stdio (what `cutweave mcp --headless` runs);
//! - [`proxy::proxy`]: the attach proxy to the engine's HTTP endpoints (what
//!   `cutweave mcp --attach` runs), including the session-end rollback and a vanished engine.
//!
//! The process-level tests (the `cutweave` binary, its stdout) are in `cutweave-cli`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    missing_docs
)]

mod mcp_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cutweave_api::Credential;
use cutweave_api::mcp::proxy::{self, ProxyEnd, Upstream};
use cutweave_api::mcp::stdio::{self, Ended};
use cutweave_api::mcp::{Catalog, CutweaveMcp, Dispatch, ListSize, P1_GATE, Profile};
use cutweave_engine::Call;
use mcp_support::{Client, Harness, Revision, SEC, client_over, must};
use rmcp::model::Tool;
use rmcp::service::ServiceError;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

fn names(tools: &[Tool]) -> Vec<String> {
    tools.iter().map(|t| t.name.to_string()).collect()
}

/// The handler `cutweave mcp --headless` builds: the engine as dispatch, the profile's catalog,
/// the fixed credential `agent:mcp`.
fn stdio_handler(h: &Harness, profile: Profile) -> CutweaveMcp {
    let handle = h.engine.handle();
    let catalog = Arc::new(Catalog::new(handle.registry(), profile));
    CutweaveMcp::new(
        Arc::new(handle) as Arc<dyn Dispatch>,
        catalog,
        Some(Credential::Agent("mcp".into())),
    )
}

/// Serves `handler` over an in-memory pipe and connects a client speaking `revision`.
async fn serve_stdio(
    handler: CutweaveMcp,
    revision: Revision,
) -> (Client, JoinHandle<Result<Ended, stdio::StdioError>>) {
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (read, write) = tokio::io::split(server_io);
    let server = tokio::spawn(stdio::serve(
        handler,
        read,
        write,
        std::future::pending::<()>(),
    ));
    (client_over(client_io, revision).await, server)
}

/// Runs the attach proxy to `h`'s `profile` endpoint over an in-memory pipe and connects a
/// client speaking `revision`. `alive` is what the proxy's watchdog sees.
async fn attach(
    h: &Harness,
    profile: Profile,
    revision: Revision,
    alive: Arc<AtomicBool>,
) -> (Client, JoinHandle<ProxyEnd>) {
    let upstream = upstream(h.port, &h.token, profile, alive);
    let (client_io, proxy_io) = tokio::io::duplex(1 << 20);
    let (read, write) = tokio::io::split(proxy_io);
    let proxy = tokio::spawn(proxy::proxy(
        upstream,
        read,
        write,
        std::future::pending::<()>(),
    ));
    (client_over(client_io, revision).await, proxy)
}

fn upstream(port: u16, token: &str, profile: Profile, alive: Arc<AtomicBool>) -> Upstream {
    Upstream {
        url: Upstream::endpoint(port, profile),
        token: token.to_owned(),
        pid: 4242,
        alive: Arc::new(move || alive.load(Ordering::SeqCst)),
    }
}

fn open_transactions(h: &Harness) -> usize {
    must(&h.direct("observer"), "edit.locks", json!({}))["transactions"]
        .as_array()
        .unwrap()
        .len()
}

/// Waits until the engine holds no open transaction.
fn wait_for_no_transactions(h: &Harness) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while open_transactions(h) > 0 {
        assert!(
            Instant::now() < deadline,
            "the session's transaction outlived it"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The scripted agent of WP-1J.01, over whatever transport `agent` uses: create a project,
/// import, wait for analysis, insert, split, undo, read the timeline.
async fn scripted_edit(h: &Harness, agent: &Client) {
    let created = agent
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "project.new", "args": { "name": "Stdio cut" } }] }),
        )
        .await;
    let project = created["steps"][0]["output"].clone();
    let sequence = project["sequence"].clone();
    let imported = agent
        .ok("media_import", json!({ "paths": [h.clip(1)] }))
        .await;
    let media = imported["media_ids"][0].clone();
    for job in imported["job_ids"].as_array().unwrap() {
        let done = agent
            .ok("job_wait", json!({ "job_id": job, "timeout_ms": 10_000 }))
            .await;
        assert_eq!(done["state"], "succeeded");
    }
    let inserted = agent
        .ok(
            "timeline_insert",
            json!({
                "sequence": sequence, "track": project["video_track"], "media": media, "at": 0
            }),
        )
        .await;
    assert_eq!(inserted["duration"], 10 * SEC);
    let clip = inserted["clips"][0].clone();
    agent
        .ok("clip_split", json!({ "item": clip, "at": "4s" }))
        .await;
    let split = agent
        .ok("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
        .await;
    assert_eq!(split["item_count"], 4, "video + linked audio, both split");
    agent.ok("edit_undo", json!({})).await;
    let read = agent
        .ok("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
        .await;
    assert_eq!(read["item_count"], 2);
    let history = h
        .direct("observer")
        .call(Call::new("edit.history", json!({ "limit": 10 })))
        .unwrap()
        .output;
    let entries = history["entries"].as_array().unwrap();
    assert_ne!(entries.len(), 0);
    assert!(
        entries.iter().all(|e| e["actor"] == "agent:mcp"),
        "{history}"
    );
}

// ----- stdio (headless) ---------------------------------------------------------------------

/// The stdio server negotiates each revision, lists the same catalog as `/mcp`, and runs the
/// scripted agent as `agent:mcp`; closing the input ends it normally.
#[tokio::test]
async fn stdio_serves_the_catalog_and_the_scripted_agent_on_both_revisions() {
    for revision in Revision::ALL {
        let h = Harness::start(None);
        let over_http = h.client(Profile::Standard, revision).await;
        let expected = names(&over_http.service.list_tools(None).await.unwrap().tools);
        over_http.close().await;

        let (agent, server) = serve_stdio(stdio_handler(&h, Profile::Standard), revision).await;
        assert_eq!(agent.negotiated(), revision.version(), "{revision:?}");
        let tools = agent.service.list_tools(None).await.unwrap().tools;
        assert_eq!(names(&tools), expected, "{revision:?}");
        assert_eq!(tools.len(), 18);
        assert!(P1_GATE.fits(ListSize::of(&tools)));
        scripted_edit(&h, &agent).await;
        agent.close().await;
        let ended = tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .expect("the stdio session ends when its input closes")
            .unwrap();
        assert_eq!(ended.unwrap(), Ended::InputClosed);
    }
}

#[tokio::test]
async fn stdio_profiles_use_the_endpoint_catalogs() {
    let h = Harness::start(None);
    for profile in Profile::ALL {
        let (agent, _server) = serve_stdio(stdio_handler(&h, profile), Revision::Stateful).await;
        let tools = agent.service.list_tools(None).await.unwrap().tools;
        let size = ListSize::of(&tools);
        assert!(profile.budget().fits(size), "{profile:?}: {size:?}");
        if profile == Profile::Minimal {
            assert!(tools.len() <= 12, "{}", tools.len());
        }
        agent.close().await;
    }
}

/// One stdio process is one session: a transaction it opened lives until the input closes,
/// then the engine rolls it back.
#[tokio::test]
async fn closing_stdin_rolls_back_the_sessions_open_transaction() {
    let h = Harness::start(None);
    let (agent, server) =
        serve_stdio(stdio_handler(&h, Profile::Standard), Revision::Stateful).await;
    agent
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "project.new", "args": { "name": "tx" } }] }),
        )
        .await;
    agent
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "edit.begin_tx", "args": { "kind": "proposal", "label": "held" } }] }),
        )
        .await;
    // The transaction outlives the request (unlike stateless HTTP).
    assert_eq!(open_transactions(&h), 1);
    agent.close().await;
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    wait_for_no_transactions(&h);
}

#[tokio::test]
async fn an_input_closed_before_the_session_started_is_a_normal_end() {
    let h = Harness::start(None);
    let (client_io, server_io) = tokio::io::duplex(1024);
    let (read, write) = tokio::io::split(server_io);
    drop(client_io);
    let ended = stdio::serve(
        stdio_handler(&h, Profile::Standard),
        read,
        write,
        std::future::pending::<()>(),
    )
    .await;
    assert_eq!(ended.unwrap(), Ended::InputClosed);
}

#[tokio::test]
async fn the_shutdown_future_ends_a_stdio_session() {
    let h = Harness::start(None);
    let (client_io, server_io) = tokio::io::duplex(1024);
    let (read, write) = tokio::io::split(server_io);
    let ended = stdio::serve(
        stdio_handler(&h, Profile::Standard),
        read,
        write,
        tokio::time::sleep(Duration::from_millis(50)),
    )
    .await;
    assert_eq!(ended.unwrap(), Ended::Shutdown);
    drop(client_io);
}

// ----- attach proxy -------------------------------------------------------------------------

/// The proxy forwards both revisions unchanged: the agent negotiates with the engine, lists
/// exactly what `/mcp` lists, and its edits land in the engine's document as `agent:mcp`.
#[tokio::test]
async fn the_proxy_forwards_both_revisions_to_the_engine() {
    for revision in Revision::ALL {
        let h = Harness::start(None);
        let over_http = h.client(Profile::Standard, revision).await;
        let expected = over_http.service.list_tools(None).await.unwrap().tools;
        over_http.close().await;

        let alive = Arc::new(AtomicBool::new(true));
        let (agent, proxy) = attach(&h, Profile::Standard, revision, alive).await;
        assert_eq!(agent.negotiated(), revision.version(), "{revision:?}");
        let tools = agent.service.list_tools(None).await.unwrap().tools;
        assert_eq!(tools, expected, "{revision:?}: the proxy changes nothing");
        scripted_edit(&h, &agent).await;
        // The same document: what the agent made is what the engine has.
        let info = must(&h.direct("observer"), "project.info", json!({}));
        assert_eq!(info["name"], "Stdio cut");
        agent.close().await;
        let end = tokio::time::timeout(Duration::from_secs(10), proxy)
            .await
            .expect("the proxy ends when its input closes")
            .unwrap();
        assert_eq!(end, ProxyEnd::InputClosed);
    }
}

#[tokio::test]
async fn the_proxy_reaches_each_profile_endpoint() {
    let h = Harness::start(None);
    for profile in Profile::ALL {
        let alive = Arc::new(AtomicBool::new(true));
        let (agent, _proxy) = attach(&h, profile, Revision::Stateful, alive).await;
        let tools = agent.service.list_tools(None).await.unwrap().tools;
        let size = ListSize::of(&tools);
        assert!(profile.budget().fits(size), "{profile:?}: {size:?}");
        let direct = Catalog::new(h.engine.handle().registry(), profile);
        assert_eq!(names(&tools), names(direct.tools()), "{profile:?}");
        agent.close().await;
    }
}

/// 2025-11-25: the stdio session is one upstream MCP session (one engine connection); closing
/// stdin deletes it, and the engine rolls back the transaction the session held.
#[tokio::test]
async fn closing_the_proxy_input_ends_the_upstream_session_and_rolls_back() {
    let h = Harness::start(None);
    let alive = Arc::new(AtomicBool::new(true));
    let (agent, proxy) = attach(&h, Profile::Standard, Revision::Stateful, alive).await;
    agent
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "project.new", "args": { "name": "tx" } }] }),
        )
        .await;
    agent
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "edit.begin_tx", "args": { "kind": "proposal", "label": "held" } }] }),
        )
        .await;
    assert_eq!(open_transactions(&h), 1);
    agent.close().await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), proxy)
            .await
            .unwrap()
            .unwrap(),
        ProxyEnd::InputClosed
    );
    wait_for_no_transactions(&h);
}

/// When the engine goes away, a request in flight (or the next one) gets a JSON-RPC error with
/// `data.code = "connection_lost"`, and the proxy ends with `Lost` well within 2 s.
#[tokio::test]
async fn a_vanished_engine_answers_with_an_error_and_ends_the_proxy() {
    let h = Harness::start(None);
    let alive = Arc::new(AtomicBool::new(true));
    let (agent, proxy) = attach(
        &h,
        Profile::Standard,
        Revision::Stateful,
        Arc::clone(&alive),
    )
    .await;
    agent.ok("project_info", json!({})).await;
    // Let the watchdog fall back to its idle cadence, then take the engine away.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let Harness {
        engine, dir: _dir, ..
    } = h;
    alive.store(false, Ordering::SeqCst);
    tokio::task::spawn_blocking(move || engine.shutdown())
        .await
        .unwrap();
    let gone = Instant::now();
    let failed = agent.try_call("project_info", json!({})).await;
    match failed {
        Err(ServiceError::McpError(e)) => {
            let data = e.data.clone().unwrap_or(Value::Null);
            assert_eq!(data["code"], "connection_lost", "{e:?}");
            assert_eq!(data["pid"], 4242);
        }
        // The proxy may also end before the answer is read: either way the call fails.
        Err(ServiceError::TransportClosed) => {}
        other => panic!("expected an error, got {other:?}"),
    }
    let end = tokio::time::timeout(Duration::from_secs(2), proxy)
        .await
        .expect("the proxy ends within 2 s of the engine")
        .unwrap();
    assert!(matches!(end, ProxyEnd::Lost { .. }), "{end:?}");
    println!("proxy ended {:?} after the engine", gone.elapsed());
    agent.close().await;
}

/// Like rmcp's own server, the proxy still answers the requests it received before stdin
/// closed (`printf '...' | cutweave mcp --attach` works), then ends the session.
#[tokio::test]
async fn the_proxy_answers_requests_sent_before_stdin_closed() {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

    let h = Harness::start(None);
    let alive = Arc::new(AtomicBool::new(true));
    let upstream = upstream(h.port, &h.token, Profile::Standard, alive);
    let (client_io, proxy_io) = tokio::io::duplex(1 << 20);
    let (read, write) = tokio::io::split(proxy_io);
    let proxy = tokio::spawn(proxy::proxy(
        upstream,
        read,
        write,
        std::future::pending::<()>(),
    ));
    let (from_proxy, mut to_proxy) = tokio::io::split(client_io);
    let lines = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": { "name": "pipe", "version": "0" }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "project_info", "arguments": {} }
        }),
    ];
    for line in &lines {
        to_proxy
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
    }
    to_proxy.shutdown().await.unwrap();
    let mut answers = Vec::new();
    let mut reader = tokio::io::BufReader::new(from_proxy).lines();
    while let Some(line) = tokio::time::timeout(Duration::from_secs(10), reader.next_line())
        .await
        .unwrap()
        .unwrap()
    {
        answers.push(serde_json::from_str::<Value>(&line).unwrap()["id"].clone());
    }
    // Every request sent before stdin closed is answered. JSON-RPC lets responses to concurrent
    // requests arrive in any order, so compare them as a set.
    answers.sort_by_key(Value::as_i64);
    assert_eq!(answers, [json!(1), json!(2), json!(3)]);
    assert_eq!(proxy.await.unwrap(), ProxyEnd::InputClosed);
}

#[test]
fn endpoints_follow_the_profile_routes() {
    assert_eq!(
        Upstream::endpoint(4321, Profile::Standard),
        "http://127.0.0.1:4321/mcp"
    );
    assert_eq!(
        Upstream::endpoint(4321, Profile::Minimal),
        "http://127.0.0.1:4321/mcp/minimal"
    );
    assert_eq!(
        Upstream::endpoint(4321, Profile::Full),
        "http://127.0.0.1:4321/mcp/full"
    );
}
