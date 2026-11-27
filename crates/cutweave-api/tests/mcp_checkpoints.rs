// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2I.03 over MCP: checkpoints stay catalog commands while the P1 tool gate is full (R33),
//! reached through `cutweave_search_commands`, `cutweave_query` and `cutweave_execute`, and
//! `cutweave_execute{checkpoint: true}` saves one checkpoint before a batch (and drops it when
//! the batch fails or is a dry run).

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod mcp_support;

use cutweave_api::mcp::Profile;
use mcp_support::{Client, Harness, Revision, must, text_of};
use serde_json::{Value, json};

/// A new project with one clip: `(sequence, item)`.
async fn project_with_a_clip(h: &Harness, client: &Client) -> (Value, Value) {
    must(
        &h.direct("setup"),
        "project.close",
        json!({ "save": false }),
    );
    let created = client
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "project.new", "args": { "name": "Checkpoints" } }] }),
        )
        .await;
    let project = &created["steps"][0]["output"];
    let (sequence, track) = (project["sequence"].clone(), project["video_track"].clone());
    let imported = client
        .ok("media_import", json!({ "paths": [h.clip(1)] }))
        .await;
    let inserted = client
        .ok(
            "timeline_insert",
            json!({ "sequence": sequence, "track": track, "media": imported["media_ids"][0], "at": 0 }),
        )
        .await;
    (sequence, inserted["clips"][0].clone())
}

async fn tracks(client: &Client, sequence: &Value) -> Value {
    client
        .ok("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
        .await["tracks"]
        .clone()
}

async fn checkpoints(client: &Client) -> Vec<Value> {
    client
        .ok(
            "cutweave_query",
            json!({ "name": "edit.list_checkpoints", "args": {} }),
        )
        .await["checkpoints"]
        .as_array()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn checkpoints_work_as_core_tools_and_through_the_generic_tools() {
    let h = Harness::start(None);
    let client = h.client(Profile::Standard, Revision::Stateful).await;
    let listed: Vec<String> = client
        .service
        .list_tools(None)
        .await
        .unwrap()
        .tools
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    // WP-2I.01: making and restoring a checkpoint are core tools; listing and deleting stay in
    // the catalog.
    for (name, core) in [
        ("edit_checkpoint", true),
        ("edit_restore", true),
        ("edit_list_checkpoints", false),
        ("edit_delete_checkpoint", false),
    ] {
        assert_eq!(listed.iter().any(|t| t == name), core, "{name}");
    }
    let found = client
        .ok("cutweave_search_commands", json!({ "query": "checkpoint" }))
        .await;
    let text = found.to_string();
    for name in [
        "edit.checkpoint",
        "edit.restore",
        "edit.list_checkpoints",
        "edit.delete_checkpoint",
    ] {
        assert!(text.contains(name), "search misses {name}: {text}");
    }

    let (sequence, item) = project_with_a_clip(&h, &client).await;
    let before = tracks(&client, &sequence).await;
    let made = client
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "edit.checkpoint", "args": { "name": "safe" } }] }),
        )
        .await;
    assert_eq!(made["steps"][0]["output"]["name"], "safe");
    client.ok("item_delete", json!({ "items": [item] })).await;
    assert_ne!(tracks(&client, &sequence).await, before);
    client
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "edit.restore", "args": { "checkpoint": "safe" } }] }),
        )
        .await;
    assert_eq!(tracks(&client, &sequence).await, before);
    // The restore is one undoable step.
    client.ok("edit_undo", json!({})).await;
    assert_ne!(tracks(&client, &sequence).await, before);
    // The same through the core tools, by the checkpoint's id.
    let made = client
        .ok("edit_checkpoint", json!({ "note": "before the restore" }))
        .await;
    let restored = client
        .ok("edit_restore", json!({ "checkpoint": "safe" }))
        .await;
    assert!(restored.is_object());
    assert_eq!(tracks(&client, &sequence).await, before);
    client
        .ok("edit_restore", json!({ "checkpoint": made["checkpoint"] }))
        .await;
    assert_ne!(tracks(&client, &sequence).await, before);
    for checkpoint in [json!("safe"), made["checkpoint"].clone()] {
        client
            .ok(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": "edit.delete_checkpoint", "args": { "checkpoint": checkpoint } }] }),
            )
            .await;
    }
    assert_eq!(checkpoints(&client).await.len(), 0);
    let missing = client
        .err(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "edit.restore", "args": { "checkpoint": "safe" } }] }),
        )
        .await;
    assert_eq!(missing["error"]["code"], "not_found");
    assert!(
        missing["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("edit.list_checkpoints"),
        "{missing}"
    );
    client.close().await;
}

#[tokio::test]
async fn execute_with_checkpoint_saves_one_before_the_batch() {
    let h = Harness::start(None);
    for revision in Revision::ALL {
        let client = h.client(Profile::Standard, revision).await;
        let (sequence, item) = project_with_a_clip(&h, &client).await;
        let before = tracks(&client, &sequence).await;
        let steps = json!([
            { "cmd": "clip.split", "args": { "item": item, "at": "2s" } },
            { "cmd": "item.delete", "args": { "items": [{ "$ref": "steps[0].output.right" }] } }
        ]);
        let result = client
            .call(
                "cutweave_execute",
                json!({ "steps": steps, "checkpoint": true }),
            )
            .await;
        let out = result.structured_content.clone().unwrap();
        let id = out["checkpoint"].as_str().unwrap().to_owned();
        assert!(id.starts_with("cp_"), "{out}");
        assert!(text_of(&result).contains(&id), "{}", text_of(&result));
        assert_ne!(tracks(&client, &sequence).await, before);
        let listed = checkpoints(&client).await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["checkpoint"], id.as_str());
        assert_eq!(listed[0]["name"], Value::Null);
        assert_eq!(
            listed[0]["note"],
            "Before cutweave_execute: clip.split, item.delete"
        );
        // Later edits, then the restore takes the document back to before the batch.
        client
            .ok(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": "project.rename", "args": { "name": "Later" } }] }),
            )
            .await;
        client
            .ok(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": "edit.restore", "args": { "checkpoint": id } }] }),
            )
            .await;
        assert_eq!(tracks(&client, &sequence).await, before);

        // A batch that fails changes nothing and leaves no checkpoint behind; nor does a dry run.
        let failed = client
            .err(
                "cutweave_execute",
                json!({ "steps": [
                    { "cmd": "item.delete", "args": { "items": ["itm_00000000000000000000000000000bad"] } }
                ], "checkpoint": true }),
            )
            .await;
        assert_eq!(failed["error"]["code"], "not_found");
        let dry = client
            .ok(
                "cutweave_execute",
                json!({ "steps": steps, "checkpoint": true, "dry_run": true }),
            )
            .await;
        assert_eq!(dry.get("checkpoint"), None);
        assert_eq!(checkpoints(&client).await.len(), 1);
        // A wrong type is refused before anything runs.
        let bad = client
            .err(
                "cutweave_execute",
                json!({ "steps": steps, "checkpoint": "yes" }),
            )
            .await;
        assert_eq!(bad["error"]["path"], "/checkpoint");
        assert_eq!(checkpoints(&client).await.len(), 1);
        client.close().await;
    }
}
