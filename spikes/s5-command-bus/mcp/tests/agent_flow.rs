// SPDX-License-Identifier: GPL-3.0-or-later
//! Acceptance test 1: an in-process rmcp client drives "import, insert, split, render frame,
//! undo" through the MCP tools and the document state is asserted (via MCP and directly on the
//! engine). Runs once per spec revision, plus once through the generic tools only (minimal
//! profile: `cutweave_execute` with `$ref`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Revision, decode_png, engine, images_of, in_process, secs};
use s5_mcp::Profile;
use serde_json::{Value, json};

async fn import_insert_split_render_undo(revision: Revision) {
    let engine = engine();
    let client = in_process(engine.clone(), Profile::Standard, revision).await;
    assert_eq!(client.negotiated(), revision.version());

    let tools: Vec<String> = client
        .service
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    for needed in [
        "media_import",
        "timeline_insert",
        "clip_split",
        "render_frame",
        "edit_undo",
        "timeline_get",
    ] {
        assert!(tools.iter().any(|t| t == needed), "{needed} listed");
    }

    let imported = client
        .ok("media_import", json!({ "path": "/footage/interview.mp4" }))
        .await;
    assert_eq!(imported["rev"], 1);
    let media = imported["result"]["media"].clone();

    let inserted = client
        .ok("timeline_insert", json!({ "media": media, "at": 0 }))
        .await;
    let item = inserted["result"]["item"].clone();

    // Timecode-string time: "cut it at 2 s".
    let split = client
        .ok("clip_split", json!({ "item": item, "at": "2s" }))
        .await;
    assert_eq!(split["rev"], 3);
    assert_eq!(split["result"]["left"], item);
    let right = split["result"]["right"].clone();

    // "show me the frame at 3 s": an MCP image content block + structured metadata.
    let frame = client
        .call("render_frame", json!({ "at": "00:00:03:00", "width": 320 }))
        .await;
    assert_ne!(frame.is_error, Some(true));
    let images = images_of(&frame);
    assert_eq!(images.len(), 1, "one image block");
    let (data, mime) = &images[0];
    assert_eq!(mime, "image/png");
    let (w, h, top_left) = decode_png(data);
    assert_eq!((w, h), (320, 180), "16:9 sequence");
    assert_ne!(top_left, [16, 16, 16], "a clip is visible, not black");
    let meta = frame.structured_content.clone().unwrap();
    assert_eq!(meta["result"]["visible"], json!([right]));
    assert_eq!(meta["result"]["at"], secs(3));
    assert!(
        meta["result"].get("data").is_none(),
        "base64 is sent once, as image content"
    );

    let undone = client.ok("edit_undo", json!({})).await;
    assert_eq!(undone["result"]["step"]["label"], "clip.split");
    assert_eq!(undone["rev"], 4);

    // Document state through MCP ...
    let timeline = client.ok("timeline_get", json!({})).await;
    let items = timeline["result"]["tracks"][0]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "split undone");
    assert_eq!(items[0]["id"], item);
    assert_eq!(items[0]["start"], 0);
    assert_eq!(items[0]["end"], secs(10));
    // ... and directly on the engine.
    engine.with(|e| {
        assert_eq!(e.doc().items.len(), 1);
        assert_eq!(e.doc().media.len(), 1);
        assert_eq!(e.rev().0, 4);
        assert_eq!(
            e.undo_depth(&s5_bus::Actor::Agent("test".into())),
            2,
            "import + insert remain undoable"
        );
    });
    client.close().await;
}

#[tokio::test]
async fn agent_flow_2025_11_25() {
    import_insert_split_render_undo(Revision::Legacy).await;
}

#[tokio::test]
async fn agent_flow_2026_07_28() {
    import_insert_split_render_undo(Revision::Modern).await;
}

/// The same task on the minimal profile, where import/insert/split are not listed: one
/// `cutweave_execute` transaction with `$ref` between steps, then `cutweave_query`.
#[tokio::test]
async fn agent_flow_through_generic_tools() {
    for revision in Revision::ALL {
        let engine = engine();
        let client = in_process(engine.clone(), Profile::Minimal, revision).await;
        let tools: Vec<String> = client
            .service
            .list_all_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        assert!(!tools.contains(&"clip_split".to_owned()));

        let found = client
            .ok("cutweave_search_commands", json!({ "query": "split clip" }))
            .await;
        assert_eq!(found["matches"][0]["name"], "clip.split");
        let described = client
            .ok("cutweave_describe_command", json!({ "name": "clip.split" }))
            .await;
        assert!(
            described["call"]
                .as_str()
                .unwrap()
                .starts_with("cutweave_execute")
        );
        assert_eq!(
            described["params_schema"]["required"],
            json!(["item", "at"])
        );

        let batch = client
            .ok(
                "cutweave_execute",
                json!({ "steps": [
                    { "name": "media.import", "params": { "path": "/footage/interview.mp4" } },
                    { "name": "timeline.insert", "params": { "media": { "$ref": "#/steps/0/media" }, "at": 0 } },
                    { "name": "clip_split", "params": { "item": { "$ref": "#/steps/1/item" }, "at": "2s" } },
                    { "name": "render.frame", "params": { "at": "3s" } }
                ] }),
            )
            .await;
        assert_eq!(batch["rev"], 1, "one transaction, one revision");
        let results = batch["results"].as_array().unwrap();
        assert_eq!(results[3]["visible"], json!([results[2]["right"]]));
        assert!(results[3].get("data").is_none());

        // An image produced inside a batch is still returned as image content.
        let frame = client
            .call(
                "cutweave_query",
                json!({ "name": "render.frame", "args": { "at": "3s" } }),
            )
            .await;
        assert_eq!(images_of(&frame).len(), 1);

        let undone = client.ok("edit_undo", json!({})).await;
        assert!(
            undone["result"]["step"]["label"]
                .as_str()
                .unwrap()
                .starts_with("batch(")
        );
        engine.with(|e| assert!(e.doc().items.is_empty() && e.doc().media.is_empty()));
        let tl = client
            .ok("cutweave_query", json!({ "name": "timeline.get" }))
            .await;
        assert_eq!(tl["result"]["media"], Value::Array(vec![]));
        client.close().await;
    }
}
