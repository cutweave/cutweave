// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-3A.01 acceptance over MCP (R37): `text.add` is a **catalog** command, not a listed tool.
//! An agent adds a titled text with defaults in one `cutweave_execute` call and reads it back
//! with `timeline_get`; `cutweave_search_commands` finds the text commands; no profile lists
//! a `text_*` tool.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    missing_docs
)]

mod mcp_support;

use cutweave_api::mcp::Profile;
use mcp_support::{Harness, Revision, must};
use serde_json::{Value, json};

#[tokio::test]
async fn one_execute_call_adds_a_titled_text_and_timeline_get_shows_it() {
    let h = Harness::start(None);
    must(
        &h.direct("setup"),
        "project.close",
        json!({ "save": false }),
    );
    for revision in Revision::ALL {
        let client = h.client(Profile::Standard, revision).await;
        let created = client
            .ok(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": "project.new", "args": { "name": "Titles" } }] }),
            )
            .await;
        let sequence = created["steps"][0]["output"]["sequence"].clone();

        // One call, only the words.
        let added = client
            .ok(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": "text.add", "args": { "text": "Launch day" } }] }),
            )
            .await;
        let out = &added["steps"][0]["output"];
        assert_eq!(out["created_track"], true, "{revision:?}");
        assert_eq!(out["duration"], 5 * 705_600_000_i64);
        let item = out["item"].clone();

        let timeline = client
            .ok("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
            .await;
        let text_track = timeline["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["kind"] == "text")
            .unwrap_or_else(|| panic!("a text track in {timeline}"));
        let shown = &text_track["items"][0];
        assert_eq!(shown["id"], item);
        assert_eq!(shown["kind"], "text");
        assert_eq!(shown["name"], "Launch day");

        // The stored item through the catalog query.
        let got = client
            .ok(
                "cutweave_query",
                json!({ "name": "text.get", "args": { "item": item } }),
            )
            .await;
        let got: &Value = got.get("output").unwrap_or(&got);
        assert_eq!(got["plain"], "Launch day");
        assert_eq!(got["style"]["font"]["family"], "Inter");

        // A bad size is an error with its path.
        let failed = client
            .err(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": "text.add",
                        "args": { "text": "x", "style": { "size": 0 } } }] }),
            )
            .await;
        assert_eq!(failed["error"]["code"], "invalid");
        assert_eq!(failed["error"]["path"], "/steps/0/args/style/size");
        client.close().await;
        must(
            &h.direct("setup"),
            "project.close",
            json!({ "save": false }),
        );
    }
}

#[tokio::test]
async fn text_is_in_the_catalog_and_no_profile_lists_a_text_tool() {
    let h = Harness::start(None);
    for profile in [Profile::Minimal, Profile::Standard, Profile::Full] {
        let client = h.client(profile, Revision::ALL[0]).await;
        let tools = client.service.list_tools(None).await.unwrap().tools;
        let listed: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(
            !listed.iter().any(|n| n.starts_with("text_")),
            "{profile:?} lists {listed:?}"
        );
        client.close().await;
    }
    let client = h.client(Profile::Standard, Revision::ALL[0]).await;
    let found = client
        .ok(
            "cutweave_search_commands",
            json!({ "query": "add a title text on the video" }),
        )
        .await;
    assert_eq!(found["matches"][0]["name"], "text.add", "{found}");
    client.close().await;
}
