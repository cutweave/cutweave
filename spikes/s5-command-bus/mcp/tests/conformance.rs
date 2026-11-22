// SPDX-License-Identifier: GPL-3.0-or-later
//! Acceptance test 3: the same conformance suite against both MCP revisions (2025-11-25 and
//! 2026-07-28) over every transport the spike has: in-process duplex (stdio framing), a real
//! `s5-mcp stdio` child process, and Streamable HTTP. All clients are rmcp 3.11.0.
//!
//! Per revision and transport: negotiation, tools/list (identical across revisions), tools/call
//! with structured output and image content, tool errors vs protocol errors, resources/read,
//! the revision's change-notification mechanism, the other revision's mechanism being refused,
//! `ping` (legacy only; nothing depends on it), and the wire shape differences rmcp applies
//! (`resultType`, `ttlMs`/`cacheScope` only on 2026-07-28).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{
    Client, Revision, connect_with, discover_body, engine, good_headers, http_client, http_server,
    images_of, in_process, initialize_body, raw_post, stdio_child,
};
use rmcp::model::{
    ProtocolVersion, ReadResourceRequestParams, ServerNotification, SubscribeRequestParams,
    SubscriptionFilter,
};
use s5_mcp::{Catalog, PROJECT_URI, Profile};
use serde_json::json;

/// Everything both revisions must do identically.
async fn common_suite(client: &Client) {
    assert_eq!(client.negotiated(), client.revision.version());

    // tools/list: same tools, same order, regardless of revision.
    let listed = client.service.list_tools(None).await.unwrap();
    let names: Vec<String> = listed.tools.iter().map(|t| t.name.to_string()).collect();
    let expected: Vec<String> = Catalog::shared(Profile::Standard)
        .tools()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert_eq!(names, expected);
    // rmcp applies the revision's result shape: cache hints + resultType only on 2026-07-28.
    match client.revision {
        Revision::Modern => {
            assert!(listed.ttl_ms.is_some() && listed.cache_scope.is_some());
            assert!(listed.result_type.is_some());
        }
        Revision::Legacy => {
            assert!(listed.ttl_ms.is_none() && listed.cache_scope.is_none());
            assert!(listed.result_type.is_none());
        }
    }

    // tools/call: structured output.
    let imported = client
        .ok(
            "media_import",
            json!({ "path": "/a.mp4", "duration": "4s" }),
        )
        .await;
    let media = imported["result"]["media"].clone();
    let inserted = client
        .ok("timeline_insert", json!({ "media": media, "at": 0 }))
        .await;
    assert_eq!(inserted["result"]["created_track"], false);

    // Image content.
    let frame = client.call("render_frame", json!({ "at": "1s" })).await;
    let images = images_of(&frame);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].1, "image/png");
    assert_eq!(
        frame.structured_content.as_ref().unwrap()["result"]["visible"],
        json!([inserted["result"]["item"]])
    );

    // A command error is a tool result the model can read (isError + typed error)...
    let error = client
        .err(
            "clip_split",
            json!({ "item": "itm_00000000000000000000000000000000", "at": 1 }),
        )
        .await;
    assert_eq!(error["code"], "not_found");
    let invalid = client
        .err(
            "clip_split",
            json!({ "item": inserted["result"]["item"], "at": 1, "bogus": 1 }),
        )
        .await;
    assert_eq!(invalid["code"], "invalid");
    assert_eq!(invalid["path"], "/bogus");
    // ... an unknown tool is a protocol error.
    assert!(client.try_call("no_such_tool", json!({})).await.is_err());

    // resources/read.
    let read = client
        .service
        .read_resource(ReadResourceRequestParams::new(PROJECT_URI))
        .await
        .unwrap();
    let text = serde_json::to_value(&read.contents[0]).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    let summary: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(summary["rev"], 2);
    assert_eq!(summary["items"], 1);
}

/// The revision's own change-notification mechanism; the other one is refused.
#[allow(deprecated)] // `Peer::subscribe` is the legacy mechanism under test
async fn notification_suite(client: &mut Client) {
    match client.revision {
        Revision::Legacy => {
            client
                .service
                .subscribe(SubscribeRequestParams::new(PROJECT_URI))
                .await
                .expect("resources/subscribe");
            client
                .ok("media_import", json!({ "path": "/notify.mp4" }))
                .await;
            assert_eq!(client.next_update().await, PROJECT_URI);
            // 2026-07-28's mechanism is not available on a legacy session.
            let filter = SubscriptionFilter::builder()
                .resource_subscription(PROJECT_URI)
                .build();
            assert!(client.service.listen(filter).await.is_err());
            // Legacy keeps ping.
            client.service.send_request(ping()).await.expect("ping");
        }
        Revision::Modern => {
            let filter = SubscriptionFilter::builder()
                .resource_subscription(PROJECT_URI)
                .build();
            let mut subscription = client.service.listen(filter).await.expect("listen");
            assert_eq!(
                subscription.acknowledged().resource_subscriptions,
                Some(vec![PROJECT_URI.to_owned()])
            );
            client
                .ok("media_import", json!({ "path": "/notify.mp4" }))
                .await;
            let notification = tokio::time::timeout(Duration::from_secs(10), subscription.next())
                .await
                .expect("notification within 10 s")
                .expect("subscription open")
                .expect("a notification");
            match notification {
                ServerNotification::ResourceUpdatedNotification(n) => {
                    assert_eq!(n.params.uri, PROJECT_URI);
                }
                other => panic!("unexpected {other:?}"),
            }
            subscription.cancel().await.expect("cancel listen");
            // 2025-11-25's mechanism and ping are gone in 2026-07-28.
            assert!(
                client
                    .service
                    .subscribe(SubscribeRequestParams::new(PROJECT_URI))
                    .await
                    .is_err()
            );
            assert!(client.service.send_request(ping()).await.is_err());
        }
    }
}

fn ping() -> rmcp::model::ClientRequest {
    rmcp::model::ClientRequest::PingRequest(rmcp::model::PingRequest::default())
}

#[tokio::test]
async fn in_process_both_revisions() {
    for revision in Revision::ALL {
        let mut client = in_process(engine(), Profile::Standard, revision).await;
        common_suite(&client).await;
        notification_suite(&mut client).await;
        client.close().await;
    }
}

#[tokio::test]
async fn stdio_child_process_both_revisions() {
    for revision in Revision::ALL {
        let mut client = stdio_child(&["--deterministic-ids", "7"], revision).await;
        common_suite(&client).await;
        notification_suite(&mut client).await;
        client.close().await;
    }
}

#[tokio::test]
async fn streamable_http_both_revisions() {
    for revision in Revision::ALL {
        let (server, token) = http_server(engine()).await;
        let mut client = http_client(&server.url(Profile::Standard), &token.secret, revision).await;
        common_suite(&client).await;
        notification_suite(&mut client).await;
        client.close().await;
        server.shutdown().await;
    }
}

/// Profiles are per endpoint: the same engine, different listed tools, under both revisions.
#[tokio::test]
async fn http_profiles_are_per_endpoint() {
    let engine = engine();
    let (server, token) = http_server(engine).await;
    for revision in Revision::ALL {
        for profile in Profile::ALL {
            let client = http_client(&server.url(profile), &token.secret, revision).await;
            let names: Vec<String> = client
                .service
                .list_all_tools()
                .await
                .unwrap()
                .into_iter()
                .map(|t| t.name.to_string())
                .collect();
            let expected: Vec<String> = Catalog::shared(profile)
                .tools()
                .iter()
                .map(|t| t.name.to_string())
                .collect();
            assert_eq!(names, expected, "{profile:?} {revision:?}");
            client.close().await;
        }
    }
    server.shutdown().await;
}

/// Session handling on the wire: 2025-11-25 `initialize` creates an `Mcp-Session-Id` session;
/// 2026-07-28 `server/discover` is stateless (no session id).
#[tokio::test]
async fn http_sessions_per_revision() {
    let (server, token) = http_server(engine()).await;
    let host = format!("127.0.0.1:{}", server.addr().port());
    let bearer = format!("Bearer {}", token.secret);
    let headers = good_headers(&host, &bearer);

    let legacy = raw_post(server.addr(), "/mcp", &headers, &initialize_body()).await;
    assert_eq!(legacy.status, 200, "{}", legacy.body);
    assert!(legacy.header("mcp-session-id").is_some(), "legacy session");
    assert!(legacy.body.contains("\"protocolVersion\":\"2025-11-25\""));

    // 2026-07-28 (SEP-2243): the HTTP headers must repeat the `_meta` protocol version.
    let missing = raw_post(server.addr(), "/mcp", &headers, &discover_body()).await;
    assert_eq!(missing.status, 400, "{}", missing.body);
    let mut modern_headers = headers.clone();
    modern_headers.push(("MCP-Protocol-Version", "2026-07-28"));
    modern_headers.push(("Mcp-Method", "server/discover"));
    let modern = raw_post(server.addr(), "/mcp", &modern_headers, &discover_body()).await;
    assert_eq!(modern.status, 200, "{}", modern.body);
    assert!(modern.header("mcp-session-id").is_none(), "stateless");
    assert!(modern.body.contains("2026-07-28"));
    assert!(
        modern.body.contains("2025-11-25"),
        "discover lists every supported revision"
    );
    server.shutdown().await;
}

/// Negotiation edge cases: an older `initialize` version is accepted as-is (the server supports
/// every revision rmcp knows); a client that only speaks 2026-07-28 uses discover.
#[tokio::test]
async fn negotiation_edges() {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    let handler = s5_mcp::CutweaveMcp::new(
        engine(),
        Catalog::shared(Profile::Standard),
        Some(s5_bus::Caller::agent("t")),
    );
    tokio::spawn(async move {
        use rmcp::ServiceExt as _;
        if let Ok(r) = handler.serve(server_io).await {
            let _ = r.waiting().await;
        }
    });
    let client = connect_with(client_io, Revision::Legacy, ProtocolVersion::V_2025_06_18).await;
    assert_eq!(client.negotiated(), ProtocolVersion::V_2025_06_18);
    client.ok("timeline_get", json!({})).await;
    client.close().await;
}
