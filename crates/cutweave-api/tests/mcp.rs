// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1J.01 acceptance tests: the MCP server on a real engine over loopback HTTP, driven by
//! rmcp's own Streamable HTTP client on both spec revisions (2025-11-25 stateful, 2026-07-28
//! stateless).
//!
//! - catalog: `tools/list` = the registry's core tools (`schema/mcp-tools.json`) + the 4 generic
//!   tools, within the P1 gate; the minimal and full profiles; annotations per tool; lean
//!   schemas; the committed `tools/list` snapshot;
//! - parity: each core tool returns exactly what the engine call returns;
//! - `cutweave_execute`: failure atomicity with `failed_step`, dry runs, one undo step, `$ref`;
//! - errors, actors, untrusted text, session-end rollback, auth;
//! - an end-to-end scripted agent: create, import, wait, insert, split, undo, read.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    missing_docs
)]

mod mcp_support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use cutweave_api::mcp::{Catalog, ListSize, P1_GATE, Profile, catalog};
use cutweave_commands::Registry;
use cutweave_engine::Call;
use mcp_support::{Client, Harness, INJECTION, Revision, SEC, http_client, must, text_of};
use rmcp::model::{CallToolResult, Tool};
use serde_json::{Value, json};

fn registry() -> Arc<Registry> {
    Arc::new(cutweave_engine::build_registry().unwrap())
}

/// The committed core tool list (`cargo xtask gen`).
fn committed_core_tools() -> Vec<Value> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../schema/mcp-tools.json");
    let text = std::fs::read_to_string(path).unwrap();
    let catalog: Value = serde_json::from_str(&text).unwrap();
    catalog["tools"].as_array().unwrap().clone()
}

fn names(tools: &[Tool]) -> Vec<String> {
    tools.iter().map(|t| t.name.to_string()).collect()
}

/// The document revision a core tool call reports in `_meta`.
fn rev_of(result: &CallToolResult) -> u64 {
    result.meta.as_ref().unwrap()["io.cutweave/rev"]
        .as_u64()
        .unwrap()
}

/// A new project (the engine's current one is closed first) with one clip:
/// `(sequence, video track, media, item)`.
async fn project_with_a_clip(h: &Harness, client: &Client) -> (Value, Value, Value, Value) {
    must(&h.direct("setup"), "project.close", json!({}));
    let created = client
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "project.new", "args": { "name": "MCP" } }] }),
        )
        .await;
    let project = &created["steps"][0]["output"];
    let (sequence, track) = (project["sequence"].clone(), project["video_track"].clone());
    let imported = client
        .ok("media_import", json!({ "paths": [h.clip(1)] }))
        .await;
    let media = imported["media_ids"][0].clone();
    let inserted = client
        .ok(
            "timeline_insert",
            json!({ "sequence": sequence, "track": track, "media": media, "at": 0 }),
        )
        .await;
    (sequence, track, media, inserted["clips"][0].clone())
}

async fn timeline(client: &Client, sequence: &Value) -> Value {
    client
        .ok("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
        .await
}

async fn current_rev(client: &Client) -> u64 {
    rev_of(&client.call("project_info", json!({})).await)
}

// ----- catalog ------------------------------------------------------------------------------

/// Acceptance 1: `/mcp` lists exactly the registry's core tools plus the 4 generic tools (19 in
/// P1 since `export_start`, WP-1E.03; `render_frame` arrives with WP-1J.02), within the P1
/// gate, on both revisions.
#[tokio::test]
async fn mcp_lists_the_core_and_generic_tools_within_the_p1_gate() {
    let h = Harness::start(None);
    let mut expected: Vec<String> = committed_core_tools()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    expected.extend(catalog::GENERIC_TOOLS.map(str::to_owned));
    assert_eq!(expected.len(), 19);
    for revision in Revision::ALL {
        let client = h.client(Profile::Standard, revision).await;
        assert_eq!(client.negotiated(), revision.version());
        let listed = client.service.list_tools(None).await.unwrap();
        assert_eq!(names(&listed.tools), expected, "{revision:?}");
        let size = ListSize::of(&listed.tools);
        println!(
            "/mcp ({revision:?}): {} tools, {} bytes, ~{} tokens (gate {} / {})",
            size.tools,
            size.bytes,
            size.tokens(),
            P1_GATE.max_tools,
            P1_GATE.max_tokens
        );
        assert!(P1_GATE.fits(size), "{size:?} exceeds the P1 gate");
        assert!(Profile::Standard.budget().fits(size));
        client.close().await;
    }
}

#[tokio::test]
async fn minimal_lists_at_most_12_tools_within_3k_tokens_and_full_equals_standard() {
    let h = Harness::start(None);
    for revision in Revision::ALL {
        let minimal = h.client(Profile::Minimal, revision).await;
        let tools = minimal.service.list_tools(None).await.unwrap().tools;
        assert_eq!(
            names(&tools),
            [
                "edit_undo",
                "job_wait",
                "media_list",
                "project_info",
                "timeline_get",
                "cutweave_search_commands",
                "cutweave_describe_command",
                "cutweave_execute",
                "cutweave_query",
            ]
        );
        let size = ListSize::of(&tools);
        println!(
            "/mcp/minimal ({revision:?}): {} tools, {} bytes, ~{} tokens",
            size.tools,
            size.bytes,
            size.tokens()
        );
        assert!(Profile::Minimal.budget().fits(size), "{size:?}");
        // The long tail stays reachable: a core tool not listed here runs through execute.
        let out = minimal
            .ok("cutweave_search_commands", json!({ "query": "split clip" }))
            .await;
        assert_eq!(out["matches"][0]["name"], "clip.split");
        assert!(
            out["matches"][0]["call"]
                .as_str()
                .unwrap()
                .starts_with("cutweave_execute")
        );
        minimal.close().await;

        let full = h.client(Profile::Full, revision).await;
        let standard = h.client(Profile::Standard, revision).await;
        assert_eq!(
            full.service.list_tools(None).await.unwrap().tools,
            standard.service.list_tools(None).await.unwrap().tools
        );
        full.close().await;
        standard.close().await;
    }
}

/// The core tools are generated from the registry: the same list, descriptions and annotations
/// as the committed `schema/mcp-tools.json`.
#[test]
fn core_tools_are_the_committed_mcp_tools_catalog() {
    let catalog = Catalog::new(registry(), Profile::Standard);
    let committed = committed_core_tools();
    let core: Vec<&Tool> = catalog
        .tools()
        .iter()
        .filter(|t| !catalog::GENERIC_TOOLS.contains(&t.name.as_ref()))
        .collect();
    assert_eq!(core.len(), committed.len());
    for (tool, entry) in core.iter().zip(&committed) {
        assert_eq!(tool.name, entry["name"].as_str().unwrap());
        assert_eq!(
            tool.description.as_deref(),
            Some(entry["description"].as_str().unwrap())
        );
        assert_eq!(
            serde_json::to_value(tool.annotations.as_ref().unwrap()).unwrap(),
            entry["annotations"],
            "{}",
            tool.name
        );
    }
}

/// Acceptance 3: annotations per tool, as listed over the wire. Read-only behaviour follows
/// `annotations.read_only`, not the entry kind (`job_status`/`job_wait` are engine commands).
#[tokio::test]
async fn every_tool_has_the_right_annotations() {
    // (tool, readOnlyHint, destructiveHint, idempotentHint)
    let expected = [
        ("clip_split", false, false, false),
        ("clip_trim", false, false, true),
        ("edit_undo", false, false, false),
        ("export_start", false, true, false),
        ("item_delete", false, true, false),
        ("item_move", false, false, true),
        ("item_set_props", false, false, true),
        ("job_status", true, false, true),
        ("job_wait", true, false, true),
        ("media_import", false, false, true),
        ("media_list", true, false, true),
        ("project_info", true, false, true),
        ("project_open", false, false, false),
        ("timeline_get", true, false, true),
        ("timeline_insert", false, false, false),
        ("cutweave_search_commands", true, false, true),
        ("cutweave_describe_command", true, false, true),
        ("cutweave_execute", false, true, false),
        ("cutweave_query", true, false, true),
    ];
    let h = Harness::start(None);
    let client = h.client(Profile::Standard, Revision::Stateful).await;
    let tools = client.service.list_tools(None).await.unwrap().tools;
    assert_eq!(tools.len(), expected.len());
    for (name, read_only, destructive, idempotent) in expected {
        let tool = tools.iter().find(|t| t.name == name).unwrap();
        let a = tool.annotations.as_ref().unwrap();
        assert_eq!(a.read_only_hint, Some(read_only), "{name} readOnlyHint");
        assert_eq!(
            a.destructive_hint,
            Some(destructive),
            "{name} destructiveHint"
        );
        assert_eq!(a.idempotent_hint, Some(idempotent), "{name} idempotentHint");
        assert_eq!(a.open_world_hint, Some(false), "{name} openWorldHint");
    }
    client.close().await;
}

/// Removes documentation-only keywords and nested `anyOf` wrappers: what remains is what
/// validates (written independently of `catalog::lean_schema`).
fn validation_only(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                if matches!(key.as_str(), "description" | "title")
                    || (key == "default" && value.is_null())
                {
                    continue;
                }
                let value = if key == "properties" {
                    let props = value.as_object().unwrap();
                    Value::Object(
                        props
                            .iter()
                            .map(|(k, v)| (k.clone(), validation_only(v)))
                            .collect(),
                    )
                } else if key == "anyOf" {
                    let mut flat = Vec::new();
                    for alt in value.as_array().unwrap() {
                        let alt = validation_only(alt);
                        match alt.as_object() {
                            Some(m) if m.len() == 1 && m.contains_key("anyOf") => {
                                flat.extend(m["anyOf"].as_array().unwrap().iter().cloned());
                            }
                            _ => flat.push(alt),
                        }
                    }
                    Value::Array(flat)
                } else {
                    validation_only(value)
                };
                out.insert(key.clone(), value);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(validation_only).collect()),
        other => other.clone(),
    }
}

/// Listed `inputSchema`s are the registry params schemas minus documentation: the same
/// validation, so MCP accepts exactly what RPC accepts. The full schema is one
/// `cutweave_describe_command` away.
#[test]
fn listed_input_schemas_validate_like_the_registry_schemas() {
    let registry = registry();
    let catalog = Catalog::new(Arc::clone(&registry), Profile::Standard);
    let mut saved = 0;
    for tool in catalog.tools() {
        let Some(entry) = registry.get_by_tool_name(&tool.name) else {
            continue;
        };
        let listed = Value::Object(tool.input_schema.as_ref().clone());
        assert_eq!(
            validation_only(&listed),
            validation_only(&entry.meta.input_schema),
            "{}",
            tool.name
        );
        assert!(listed.get("description").is_none() && listed.get("title").is_none());
        saved += entry.meta.input_schema.to_string().len() - listed.to_string().len();
    }
    println!("lean schemas save {saved} bytes (~{} tokens)", saved / 4);
}

/// The committed MCP catalog snapshot: `tools/list` of each profile, exactly as served. A
/// change needs `cargo insta review` (and stays within the budgets above).
#[test]
fn tools_list_snapshot() {
    let registry = registry();
    for profile in [Profile::Standard, Profile::Minimal] {
        let catalog = Catalog::new(Arc::clone(&registry), profile);
        let size = catalog.size();
        assert!(profile.budget().fits(size));
        insta::assert_json_snapshot!(
            format!("tools_list_{}", profile.name()),
            catalog.tools_json()
        );
    }
}

// ----- parity -------------------------------------------------------------------------------

/// Replaces the engine's temp folder in paths (as given, and canonicalized), so two engines'
/// outputs compare.
fn normalized(value: &Value, dir: &std::path::Path) -> Value {
    let mut text = value.to_string();
    let canonical = std::fs::canonicalize(dir).unwrap();
    let canonical = canonical.to_string_lossy().into_owned();
    let forms = [
        canonical.clone(),
        canonical.trim_start_matches(r"\\?\").to_owned(),
        dir.to_string_lossy().into_owned(),
    ];
    for form in forms {
        let escaped = serde_json::to_string(&form).unwrap();
        text = text.replace(&escaped[1..escaped.len() - 1], "<dir>");
    }
    serde_json::from_str(&text).unwrap()
}

/// Acceptance 1 (parity): every core tool returns exactly the output of the same engine call.
/// Two engines with the same seed run the same calls: one over MCP (actor `agent:mcp`, from the
/// token), one directly on a connection with the same credential.
#[tokio::test]
async fn each_core_tool_returns_the_engine_output() {
    for revision in Revision::ALL {
        let a = Harness::start(Some(41));
        let b = Harness::start(Some(41));
        let direct = b.direct("mcp");
        // Same starting document on both (bootstrap outside the comparison).
        let new = json!({ "name": "Parity" });
        let project = must(&a.direct("mcp"), "project.new", new.clone());
        assert_eq!(project, must(&direct, "project.new", new));
        let (seq, track) = (project["sequence"].clone(), project["video_track"].clone());
        let client = a.client(Profile::Standard, revision).await;

        let mut covered = Vec::new();
        let mut step = |tool: &'static str| covered.push(tool);
        let compare = |tool: &str, mcp: &Value, engine: &Value| {
            assert_eq!(
                normalized(mcp, a.dir.path()),
                normalized(engine, b.dir.path()),
                "{tool} ({revision:?})"
            );
        };
        macro_rules! same {
            ($tool:literal, $args:expr) => {{
                let args = $args;
                step($tool);
                let mcp = client.ok($tool, args.clone()).await;
                let name = a
                    .engine
                    .handle()
                    .registry()
                    .get_by_tool_name($tool)
                    .unwrap()
                    .meta
                    .name;
                let engine = must(&direct, name, args);
                compare($tool, &mcp, &engine);
                mcp
            }};
        }

        same!("project_info", json!({}));
        // Each engine imports from its own folder; paths compare normalized.
        let imported = client
            .ok("media_import", json!({ "paths": [a.clip(1)] }))
            .await;
        let engine = must(&direct, "media.import", json!({ "paths": [b.clip(1)] }));
        compare("media_import", &imported, &engine);
        step("media_import");
        let media = imported["media_ids"][0].clone();
        let job = imported["job_ids"][0].clone();
        same!("job_wait", json!({ "job_id": job, "timeout_ms": 10_000 }));
        same!("job_status", json!({ "id": job }));
        same!("media_list", json!({}));
        let inserted = same!(
            "timeline_insert",
            json!({ "sequence": seq, "track": track, "media": media, "at": 0 })
        );
        let item = inserted["clips"][0].clone();
        let split = same!("clip_split", json!({ "item": item, "at": "2s" }));
        same!(
            "clip_trim",
            json!({ "item": split["left"], "edge": "end", "delta": "-0.5s" })
        );
        same!(
            "item_move",
            json!({ "item": split["right"], "start": "6s" })
        );
        same!(
            "item_set_props",
            json!({ "item": split["right"], "props": { "name": "B", "opacity": 0.5 } })
        );
        same!("item_delete", json!({ "items": [split["right"]] }));
        same!("edit_undo", json!({}));
        same!("timeline_get", json!({ "sequence": seq, "depth": 2 }));
        // export_start: these engines run without the media subsystem, so both validate the
        // export and refuse it the same way (`unsupported`), whichever `hw` policy is asked
        // for (the listed schema takes every form).
        let out = |h: &Harness| h.dir.path().join("out.mp4").to_string_lossy().into_owned();
        for hw in [
            Value::Null,
            json!("auto"),
            json!("off"),
            json!({ "force": "h264_nvenc" }),
        ] {
            let mcp = client
                .err("export_start", json!({ "path": out(&a), "hw": hw }))
                .await;
            let engine = direct
                .call(cutweave_engine::Call::new(
                    "export.start",
                    json!({ "path": out(&b), "hw": hw }),
                ))
                .expect_err("export.start without a media subsystem");
            assert_eq!(mcp["error"]["code"], json!(engine.code()), "{hw}: {mcp}");
            assert_eq!(engine.code(), "unsupported", "{hw}");
        }
        // A malformed encoder name fails validation the same way on both.
        let bad = json!({ "force": "H264 NVENC" });
        let mcp = client
            .err("export_start", json!({ "path": out(&a), "hw": bad }))
            .await;
        let engine = direct
            .call(cutweave_engine::Call::new(
                "export.start",
                json!({ "path": out(&b), "hw": bad }),
            ))
            .expect_err("a malformed encoder name");
        assert_eq!(mcp["error"]["code"], json!(engine.code()), "{mcp}");
        assert_eq!(engine.code(), "invalid");
        step("export_start");
        // project_open: save each document as a project, close it, open it again.
        for (h, conn) in [(&a, a.direct("mcp")), (&b, b.direct("mcp"))] {
            let folder = h.dir.path().join("saved").to_string_lossy().into_owned();
            must(&conn, "project.save_as", json!({ "path": folder }));
            must(&conn, "project.close", json!({}));
        }
        let saved = |h: &Harness| h.dir.path().join("saved").to_string_lossy().into_owned();
        let mcp = client
            .ok("project_open", json!({ "path": saved(&a) }))
            .await;
        let engine = must(&direct, "project.open", json!({ "path": saved(&b) }));
        compare("project_open", &mcp, &engine);
        step("project_open");

        let mut every: Vec<String> = committed_core_tools()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect();
        every.sort();
        let mut covered: Vec<String> = covered.into_iter().map(str::to_owned).collect();
        covered.sort();
        covered.dedup();
        assert_eq!(covered, every, "every core tool is compared");
        client.close().await;
    }
}

// ----- cutweave_execute ---------------------------------------------------------------------

/// Acceptance 2: three steps where the third fails leave the document and its revision
/// unchanged and report `failed_step: 2`.
#[tokio::test]
async fn a_failing_step_rolls_the_whole_batch_back() {
    let h = Harness::start(None);
    for revision in Revision::ALL {
        let client = h.client(Profile::Standard, revision).await;
        let (sequence, _, _, item) = project_with_a_clip(&h, &client).await;
        let before = timeline(&client, &sequence).await;
        let rev = current_rev(&client).await;
        let missing = "itm_00000000000000000000000000000bad";
        let failed = client
            .err(
                "cutweave_execute",
                json!({ "steps": [
                    { "cmd": "clip.split", "args": { "item": item, "at": "2s" } },
                    { "cmd": "clip_trim", "args": {
                        "item": { "$ref": "steps[0].output.left" }, "edge": "end", "delta": "-0.5s"
                    } },
                    { "cmd": "item.delete", "args": { "items": [missing] } }
                ] }),
            )
            .await;
        assert_eq!(failed["failed_step"], 2);
        assert_eq!(failed["rolled_back"], true);
        assert_eq!(failed["error"]["code"], "not_found");
        assert_eq!(failed["error"]["id"], missing);
        assert_eq!(failed["error"]["path"], "/steps/2/args/items/0");
        assert!(
            failed["error"]["hint"]
                .as_str()
                .unwrap()
                .contains("timeline_get")
        );
        assert_eq!(timeline(&client, &sequence).await, before, "{revision:?}");
        assert_eq!(current_rev(&client).await, rev, "{revision:?}");
        // An invalid argument names its path inside the request.
        let failed = client
            .err(
                "cutweave_execute",
                json!({ "steps": [
                    { "cmd": "clip.split", "args": { "item": item, "at": "2s" } },
                    { "cmd": "clip.split", "args": { "item": item, "at": "3s", "bogus": true } }
                ] }),
            )
            .await;
        assert_eq!(failed["failed_step"], 1);
        assert_eq!(failed["error"]["code"], "invalid");
        assert_eq!(failed["error"]["path"], "/steps/1/args/bogus");
        assert_eq!(current_rev(&client).await, rev);
        client.close().await;
    }
}

/// Acceptance 2: `dry_run` changes nothing and lists the predicted ops; the real run is one
/// undo step that `edit_undo` reverts as a whole.
#[tokio::test]
async fn dry_run_predicts_ops_and_a_batch_is_one_undo_step() {
    let h = Harness::start(None);
    for revision in Revision::ALL {
        let client = h.client(Profile::Standard, revision).await;
        let (sequence, _, _, item) = project_with_a_clip(&h, &client).await;
        let before = timeline(&client, &sequence).await;
        let rev = current_rev(&client).await;
        let steps = json!([
            { "cmd": "clip.split", "args": { "item": item, "at": "2s" } },
            { "cmd": "clip.trim", "args": {
                "item": { "$ref": "steps[0].output.left" }, "edge": "end", "delta": "-0.5s"
            } },
            { "cmd": "timeline.get", "args": { "sequence": sequence, "depth": 2 } }
        ]);
        let dry = client
            .ok(
                "cutweave_execute",
                json!({ "steps": steps, "dry_run": true, "expected_rev": rev }),
            )
            .await;
        assert_eq!(dry["dry_run"], true);
        assert_eq!(dry["rev"], rev);
        for i in 0..2 {
            let ops = dry["steps"][i]["ops"].as_array().unwrap();
            assert!(!ops.is_empty(), "step {i} predicts no ops: {dry}");
            assert!(ops.iter().all(|op| op["op"].is_string()));
        }
        // The query step reads the batch's working document: the split clips are there.
        let inside = &dry["steps"][2]["output"];
        assert!(inside["item_count"].as_u64().unwrap() > before["item_count"].as_u64().unwrap());
        assert_eq!(timeline(&client, &sequence).await, before);
        assert_eq!(current_rev(&client).await, rev);

        // A stale expected_rev fails before anything runs.
        let stale = client
            .err(
                "cutweave_execute",
                json!({ "steps": steps, "expected_rev": rev + 7 }),
            )
            .await;
        assert_eq!(stale["error"]["code"], "conflict");
        assert_eq!(stale["error"]["reason"], "stale_rev");
        assert_eq!(stale["failed_step"], Value::Null);

        let applied = client
            .ok(
                "cutweave_execute",
                json!({ "steps": steps, "expected_rev": rev }),
            )
            .await;
        assert_eq!(applied["changed"], true);
        assert_eq!(applied["rev"], rev + 1, "one commit for the whole batch");
        let left = &applied["steps"][0]["output"]["left"];
        assert!(
            applied["steps"][1]["output"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|trimmed| &trimmed["item"] == left),
            "$ref resolved: {applied}"
        );
        assert_ne!(timeline(&client, &sequence).await, before);
        let undone = client.ok("edit_undo", json!({})).await;
        assert!(
            undone["label"]
                .as_str()
                .unwrap()
                .starts_with("cutweave_execute: clip.split, clip.trim"),
            "{undone}"
        );
        assert_eq!(
            timeline(&client, &sequence).await["tracks"],
            before["tracks"]
        );
        client.close().await;
    }
}

#[tokio::test]
async fn execute_validates_steps_before_running_anything() {
    let h = Harness::start(None);
    let client = h.client(Profile::Standard, Revision::Stateful).await;
    let (_, _, _, item) = project_with_a_clip(&h, &client).await;
    let rev = current_rev(&client).await;
    let split = json!({ "cmd": "clip.split", "args": { "item": item, "at": "2s" } });

    let e = client
        .err(
            "cutweave_execute",
            json!({ "steps": [split, { "cmd": "clip.explode" }] }),
        )
        .await;
    assert_eq!(
        (e["failed_step"].clone(), e["error"]["code"].clone()),
        (json!(1), json!("not_found"))
    );
    assert!(
        e["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("cutweave_search_commands")
    );

    let e = client
        .err(
            "cutweave_execute",
            json!({ "steps": [split, { "cmd": "edit.undo" }] }),
        )
        .await;
    assert_eq!(e["failed_step"], 1);
    assert_eq!(e["error"]["path"], "/steps/1/cmd");

    let e = client
        .err(
            "cutweave_execute",
            json!({ "steps": [split, { "cmd": "clip.trim", "args": {
                "item": { "$ref": "steps[1].output.left" }, "edge": "end", "delta": "-1s"
            } }] }),
        )
        .await;
    assert_eq!(e["failed_step"], 1);
    assert_eq!(e["error"]["path"], "/steps/1/args/item");
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap()
            .contains("has not run yet")
    );

    let e = client
        .err(
            "cutweave_execute",
            json!({ "steps": [{ "name": "clip.split", "params": {} }] }),
        )
        .await;
    assert_eq!(e["error"]["path"], "/steps/0/name");
    let e = client
        .err("cutweave_execute", json!({ "steps": [split], "bogus": 1 }))
        .await;
    assert_eq!(
        (e["error"]["path"].clone(), e["failed_step"].clone()),
        (json!("/bogus"), Value::Null)
    );
    let e = client
        .err(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "edit.redo" }], "dry_run": true }),
        )
        .await;
    assert_eq!(e["error"]["code"], "unsupported");
    // Hidden entries are not reachable over MCP.
    let e = client
        .err(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "media.register", "args": {} }] }),
        )
        .await;
    assert_eq!(e["error"]["code"], "not_found");
    assert_eq!(current_rev(&client).await, rev, "nothing ran");

    // A single engine command runs on its own (outside a transaction): undo, then redo.
    client
        .ok("cutweave_execute", json!({ "steps": [split] }))
        .await;
    client.ok("edit_undo", json!({})).await;
    let redone = client
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "edit_redo" }] }),
        )
        .await;
    assert_eq!(redone["steps"][0]["cmd"], "edit.redo");
    assert_eq!(redone["steps"][0]["output"]["actor"], "agent:mcp");
    client.close().await;
}

// ----- errors, actors, untrusted text, sessions, auth ---------------------------------------

#[tokio::test]
async fn errors_are_tool_errors_with_code_message_path_and_hint() {
    let h = Harness::start(None);
    for revision in Revision::ALL {
        let client = h.client(Profile::Standard, revision).await;
        let (sequence, _, _, item) = project_with_a_clip(&h, &client).await;
        let e = client
            .err("clip_split", json!({ "item": item, "at": 1, "bogus": 1 }))
            .await;
        let error = &e["error"];
        assert_eq!(error["code"], "invalid");
        assert_eq!(error["path"], "/bogus");
        assert!(error["message"].as_str().unwrap().contains("bogus"));
        assert!(error["hint"].is_string());

        let missing = "itm_0000000000000000000000000000beef";
        let e = client
            .err("clip_split", json!({ "item": missing, "at": 1 }))
            .await;
        assert_eq!(e["error"]["code"], "not_found");
        assert_eq!(e["error"]["path"], "/item");
        let result = client
            .call("clip_split", json!({ "item": missing, "at": 1 }))
            .await;
        let text = text_of(&result);
        assert!(text.starts_with("clip_split failed (not_found)"), "{text}");
        assert!(text.contains("Hint:"), "{text}");

        // A client cannot assert an actor: it is not a param.
        let e = client
            .err(
                "clip_split",
                json!({ "item": item, "at": "2s", "actor": "user" }),
            )
            .await;
        assert_eq!(e["error"]["path"], "/actor");

        // cutweave_query runs read-only entries only, and points errors into `args`.
        let e = client
            .err(
                "cutweave_query",
                json!({ "name": "clip.split", "args": {} }),
            )
            .await;
        assert_eq!(e["error"]["path"], "/name");
        assert!(
            e["error"]["message"]
                .as_str()
                .unwrap()
                .contains("tool clip_split")
        );
        let e = client
            .err("cutweave_query", json!({ "name": "track.add", "args": {} }))
            .await;
        assert!(
            e["error"]["message"]
                .as_str()
                .unwrap()
                .contains("cutweave_execute")
        );
        let e = client
            .err(
                "cutweave_query",
                json!({ "name": "item.get", "args": { "item": item, "bogus": true } }),
            )
            .await;
        assert_eq!(e["error"]["path"], "/args/bogus");
        let got = client
            .ok(
                "cutweave_query",
                json!({ "name": "item_get", "args": { "item": item } }),
            )
            .await;
        assert_eq!(got["item"]["id"], item);
        let locks = client
            .ok("cutweave_query", json!({ "name": "edit.locks" }))
            .await;
        assert!(locks.is_object());

        // An unknown tool is a protocol error.
        assert!(client.try_call("clip_explode", json!({})).await.is_err());
        let _ = sequence;
        client.close().await;
    }
}

#[tokio::test]
async fn search_and_describe_reach_the_long_tail() {
    let h = Harness::start(None);
    let client = h.client(Profile::Standard, Revision::Stateless).await;
    let found = client
        .ok("cutweave_search_commands", json!({ "query": "redo" }))
        .await;
    assert_eq!(found["matches"][0]["name"], "edit.redo");
    let all = client
        .ok(
            "cutweave_search_commands",
            json!({ "query": "", "limit": 50 }),
        )
        .await;
    let total = all["total"].as_u64().unwrap();
    let registry = registry();
    let reachable = registry
        .list()
        .filter(|e| e.meta.mcp != cutweave_commands::McpExposure::Hidden)
        .count();
    assert_eq!(usize::try_from(total).unwrap(), reachable);
    let none = client
        .ok("cutweave_search_commands", json!({ "query": "zzzz" }))
        .await;
    assert!(none["hint"].as_str().unwrap().contains("timeline"));

    let described = client
        .ok("cutweave_describe_command", json!({ "name": "clip_trim" }))
        .await;
    let entry = registry.get("clip.trim").unwrap();
    assert_eq!(described["name"], "clip.trim");
    assert_eq!(described["call"], "tool clip_trim");
    assert_eq!(
        described["input_schema"], entry.meta.input_schema,
        "verbatim"
    );
    assert_eq!(described["output_schema"], entry.meta.output_schema);
    assert!(described["description"].as_str().unwrap().len() > 100);
    let redo = client
        .ok("cutweave_describe_command", json!({ "name": "edit.redo" }))
        .await;
    assert!(
        redo["call"]
            .as_str()
            .unwrap()
            .starts_with("cutweave_execute")
    );
    let e = client
        .err(
            "cutweave_describe_command",
            json!({ "name": "media.register" }),
        )
        .await;
    assert_eq!(e["error"]["code"], "not_found");
    client.close().await;
}

/// The actor of every MCP change is `agent:<token label>`, derived from the credential.
#[tokio::test]
async fn the_actor_comes_from_the_token() {
    let h = Harness::start(None);
    let mut events = h.engine.handle().subscribe();
    let mcp = h.client(Profile::Standard, Revision::Stateful).await;
    let (_, _, _, item) = project_with_a_clip(&h, &mcp).await;
    let cli = http_client(&h.url(Profile::Standard), &h.cli_token, Revision::Stateless).await;
    cli.ok("clip_split", json!({ "item": item, "at": "2s" }))
        .await;
    let history = mcp
        .ok(
            "cutweave_query",
            json!({ "name": "edit.history", "args": { "limit": 3 } }),
        )
        .await;
    let actors: Vec<&str> = history["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["actor"].as_str().unwrap())
        .collect();
    assert_eq!(actors, ["agent:cli", "agent:mcp", "agent:mcp"]);
    // `edit_undo` undoes the caller's own latest change.
    let undone = cli.ok("edit_undo", json!({})).await;
    assert_eq!(undone["actor"], "agent:cli");
    let mut patched = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.name() == "doc.patched" {
            patched.push(event.payload()["actor"].as_str().unwrap().to_owned());
        }
    }
    assert!(
        patched.iter().all(|a| a == "agent:mcp" || a == "agent:cli"),
        "{patched:?}"
    );
    assert!(patched.contains(&"agent:cli".to_owned()));
    mcp.close().await;
    cli.close().await;
}

/// Text from media files reaches agents wrapped as untrusted (arch §14, §18).
#[tokio::test]
async fn media_text_is_wrapped_as_untrusted() {
    let h = Harness::start(None);
    let client = h.client(Profile::Standard, Revision::Stateless).await;
    client
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "project.new", "args": { "name": "U" } }] }),
        )
        .await;
    let missing = h.clip(9);
    let imported = client
        .ok("media_import", json!({ "paths": [h.clip(3), missing] }))
        .await;
    let failed = &imported["failed"][0];
    assert_eq!(failed["path"], missing);
    assert_eq!(failed["error"]["untrusted"], true);
    assert!(failed["error"]["text"].is_string());
    let jobs = imported["job_ids"].as_array().unwrap();
    assert_eq!(jobs.len(), 2, "index + notes");
    let mut failures = 0;
    for job in jobs {
        let done = client
            .ok("job_wait", json!({ "job_id": job, "timeout_ms": 10_000 }))
            .await;
        if done["state"] == "failed" {
            failures += 1;
            assert_eq!(
                done["error"],
                json!({ "untrusted": true, "text": INJECTION })
            );
        } else {
            assert_eq!(done["error"], Value::Null);
        }
    }
    assert_eq!(failures, 1);
    let listed = client
        .ok("cutweave_query", json!({ "name": "job.list" }))
        .await;
    assert!(
        listed["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["error"]["untrusted"] == true)
    );
    client.close().await;
}

/// The engine connection of an MCP session holds its transaction; ending the session rolls it
/// back (WP-1B.09).
#[tokio::test]
async fn ending_a_session_rolls_back_its_open_transaction() {
    let h = Harness::start(None);
    let client = h.client(Profile::Standard, Revision::Stateful).await;
    let (_, _, _, item) = project_with_a_clip(&h, &client).await;
    let opened = client
        .ok(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "edit.begin_tx", "args": { "kind": "proposal", "label": "held" } }] }),
        )
        .await;
    let tx = opened["steps"][0]["output"]["tx"].clone();
    // A second batch on the same session cannot open another transaction.
    let e = client
        .err(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "clip.split", "args": { "item": item, "at": "2s" } }] }),
        )
        .await;
    assert_eq!(e["failed_step"], Value::Null);
    let observer = h.direct("observer");
    let open = |conn: &cutweave_engine::Connection| {
        must(conn, "edit.locks", json!({}))["transactions"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(open(&observer), 1);
    let locks = must(&observer, "edit.locks", json!({}));
    assert_eq!(locks["transactions"][0]["tx"], tx);
    client.close().await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while open(&observer) > 0 {
        assert!(
            Instant::now() < deadline,
            "the session's transaction outlived it"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// POSTs `body` to `/mcp` with exactly these headers and returns the HTTP status.
async fn raw_post_status(
    port: u16,
    host: String,
    token: Option<String>,
    origin: Option<&'static str>,
    body: String,
) -> u16 {
    use std::fmt::Write as _;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut text = format!(
        "POST /mcp HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Type: \
         application/json\r\nAccept: application/json, text/event-stream\r\n"
    );
    if let Some(token) = token {
        let _ = write!(text, "Authorization: Bearer {token}\r\n");
    }
    if let Some(origin) = origin {
        let _ = write!(text, "Origin: {origin}\r\n");
    }
    let _ = write!(text, "Content-Length: {}\r\n\r\n{body}", body.len());
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream.write_all(text.as_bytes()).await.unwrap();
    let mut head = vec![0u8; 64];
    let n = stream.read(&mut head).await.unwrap();
    String::from_utf8_lossy(&head[..n])
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap()
}

#[tokio::test]
async fn mcp_needs_an_agent_token_and_a_loopback_host() {
    let h = Harness::start(None);
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": { "name": "raw", "version": "0" }
        }
    })
    .to_string();
    let post = |host: String, token: Option<String>, origin: Option<&'static str>| {
        raw_post_status(h.port, host, token, origin, body.clone())
    };
    let host = format!("127.0.0.1:{}", h.port);
    assert_eq!(post(host.clone(), None, None).await, 401);
    assert_eq!(post(host.clone(), Some("nope".into()), None).await, 401);
    assert_eq!(
        post("evil.example".into(), Some(h.token.clone()), None).await,
        403
    );
    assert_eq!(
        post(
            host.clone(),
            Some(h.token.clone()),
            Some("http://evil.example")
        )
        .await,
        403
    );
    assert_eq!(post(host, Some(h.token.clone()), None).await, 200);
}

// ----- end to end ---------------------------------------------------------------------------

/// A scripted agent does the whole job over MCP only, on each revision: create a project,
/// import (fake prober), wait for analysis, insert, split, undo, read the timeline.
#[tokio::test]
async fn a_scripted_agent_edits_entirely_over_mcp() {
    for revision in Revision::ALL {
        let h = Harness::start(None);
        let agent = h.client(Profile::Standard, revision).await;
        let tools = agent.service.list_tools(None).await.unwrap().tools;
        assert!(tools.iter().any(|t| t.name == "clip_split"));

        let found = agent
            .ok(
                "cutweave_search_commands",
                json!({ "query": "new project" }),
            )
            .await;
        assert_eq!(found["matches"][0]["name"], "project.new");
        let created = agent
            .ok(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": "project.new", "args": { "name": "Agent cut" } }] }),
            )
            .await;
        let project = &created["steps"][0]["output"];
        let info = agent.ok("project_info", json!({})).await;
        assert_eq!(info["name"], "Agent cut");
        let sequence = info["sequences"][0]["id"].clone();
        assert_eq!(sequence, project["sequence"]);

        let imported = agent
            .ok("media_import", json!({ "paths": [h.clip(2)] }))
            .await;
        let media = imported["media_ids"][0].clone();
        for job in imported["job_ids"].as_array().unwrap() {
            let done = agent
                .ok("job_wait", json!({ "job_id": job, "timeout_ms": 10_000 }))
                .await;
            assert_eq!(done["state"], "succeeded");
        }
        let listed = agent.ok("media_list", json!({})).await;
        assert_eq!(listed["total"], 1);

        let inserted = agent
            .ok(
                "timeline_insert",
                json!({
                    "sequence": sequence, "track": project["video_track"],
                    "media": media, "at": 0
                }),
            )
            .await;
        let clip = inserted["clips"][0].clone();
        assert_eq!(inserted["duration"], 11 * SEC);
        let split = agent
            .ok("clip_split", json!({ "item": clip, "at": "4s" }))
            .await;
        let seq_after_split = agent
            .ok("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
            .await;
        assert_eq!(
            seq_after_split["item_count"], 4,
            "video + linked audio, both split"
        );
        let undo = agent.call("edit_undo", json!({})).await;
        assert!(
            text_of(&undo).contains("edit.undo: done"),
            "{}",
            text_of(&undo)
        );
        let read = agent
            .ok("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
            .await;
        assert_eq!(read["item_count"], 2);
        let video = read["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == project["video_track"])
            .unwrap();
        let items = video["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], clip, "the original clip is back");
        assert_ne!(items[0]["id"], split["right"]);
        // Every edit was the agent's, as the engine recorded it.
        let conn = h.direct("observer");
        let history = conn
            .call(Call::new("edit.history", json!({ "limit": 10 })))
            .unwrap()
            .output;
        assert!(
            history["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["actor"] == "agent:mcp")
        );
        agent.close().await;
    }
}

/// WP-1M.12: `media_list` takes `filter` and `sort` over MCP, and `filter.text` returns the
/// assets whose file names contain every term, in the order asked for (the editor's bin applies
/// the same predicate to the same names; both are held to `media-find.json`).
#[tokio::test]
async fn media_list_filters_and_sorts_over_mcp() {
    for revision in Revision::ALL {
        let h = Harness::start(None);
        let agent = h.client(Profile::Standard, revision).await;
        agent.ok("project_info", json!({})).await;
        let imported = agent
            .ok(
                "media_import",
                json!({ "paths": [h.clip(1), h.clip(2), h.clip(3)] }),
            )
            .await;
        let ids = imported["media_ids"].clone();
        for job in imported["job_ids"].as_array().unwrap() {
            agent
                .ok("job_wait", json!({ "job_id": job, "timeout_ms": 10_000 }))
                .await;
        }
        let id_of = |out: &Value| -> Vec<Value> {
            out["media"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["id"].clone())
                .collect()
        };
        let all = agent.ok("media_list", json!({})).await;
        assert_eq!(all["total"], 3);
        assert_eq!(all["matched"], 3);

        let one = agent
            .ok("media_list", json!({ "filter": { "text": "CLIP2" } }))
            .await;
        assert_eq!(id_of(&one), vec![ids[1].clone()]);
        assert_eq!(
            (one["total"].clone(), one["matched"].clone()),
            (json!(3), json!(1))
        );

        let none = agent
            .ok("media_list", json!({ "filter": { "text": "clip2 clip3" } }))
            .await;
        assert_eq!(none["matched"], 0);

        // Longest first: clip3 (12 s), clip2, clip1; the cursor continues in that order.
        let longest = agent
            .ok(
                "media_list",
                json!({ "sort": { "by": "duration", "order": "desc" }, "limit": 2 }),
            )
            .await;
        assert_eq!(id_of(&longest), vec![ids[2].clone(), ids[1].clone()]);
        let rest = agent
            .ok(
                "media_list",
                json!({ "sort": { "by": "duration", "order": "desc" }, "cursor": longest["next_cursor"] }),
            )
            .await;
        assert_eq!(id_of(&rest), vec![ids[0].clone()]);
        agent.close().await;
    }
}
