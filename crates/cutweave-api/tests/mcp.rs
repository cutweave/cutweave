// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1J.01 acceptance tests: the MCP server on a real engine over loopback HTTP, driven by
//! rmcp's own Streamable HTTP client on both spec revisions (2025-11-25 stateful, 2026-07-28
//! stateless).
//!
//! - catalog: `tools/list` = the registry's core tools (`schema/mcp-tools.json`) + the 4 generic
//!   tools, within the standard budget (R36); the minimal and full profiles; annotations per tool; lean
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

use cutweave_api::mcp::core::CoreTable;
use cutweave_api::mcp::{
    Catalog, DESCRIBED_BUDGET, ListSize, Profile, STANDARD_BUDGET, catalog, instructions, search,
};
use cutweave_commands::Registry;
use cutweave_engine::Call;
use mcp_support::{
    Client, Harness, INJECTION, Revision, SEC, Watcher, example_args, http_client, must,
    schema_error, text_of,
};
use rmcp::model::{CallToolResult, ServerNotification, SubscriptionFilter, Tool};
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
    must(
        &h.direct("setup"),
        "project.close",
        json!({ "save": false }),
    );
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

/// Acceptance 1: `/mcp` lists exactly the registry's core tools plus the 4 generic tools: 31
/// since WP-2I.01 listed the P2 core set (`mcp_core.toml`). The core engine has no renderer, so
/// no `render_frame`: the full engine lists 32 (tests/mcp_frames.rs). Within the standard budget
/// (R36), on both revisions.
#[tokio::test]
async fn mcp_lists_the_core_and_generic_tools_within_the_standard_budget() {
    let h = Harness::start(None);
    let mut expected: Vec<String> = committed_core_tools()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    expected.extend(catalog::GENERIC_TOOLS.map(str::to_owned));
    assert_eq!(expected.len(), 31);
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
            STANDARD_BUDGET.max_tools,
            STANDARD_BUDGET.max_tokens
        );
        assert!(
            STANDARD_BUDGET.fits(size),
            "{size:?} exceeds the standard budget"
        );
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

/// The core tools are generated from the registry: the same list and annotations as the
/// committed `schema/mcp-tools.json`, each described by the core table (`mcp_core.toml`).
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
        let described = CoreTable::get()
            .by_command(entry["command"].as_str().unwrap())
            .unwrap_or_else(|| panic!("{} is not in mcp_core.toml", tool.name));
        assert_eq!(tool.description.as_deref(), Some(&*described.listing()));
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
        // WP-2A.03: a `delta` move repeated moves again, so `item_move` is not idempotent.
        ("item_move", false, false, false),
        ("item_set_props", false, false, true),
        ("keyframe_set", false, false, true),
        ("audio_set", false, false, true),
        // WP-2I.01: the rest of the P2 core set.
        ("clip_set_speed", false, false, true),
        ("edit_checkpoint", false, false, false),
        ("edit_redo", false, false, false),
        ("edit_restore", false, false, true),
        ("timeline_add_marker", false, false, false),
        ("track_add", false, false, false),
        // The registry keeps `readOnlyHint` for queries; `view.get` is an engine command.
        ("view_get", false, false, true),
        ("view_set", false, false, true),
        ("job_status", true, false, true),
        ("job_wait", true, false, true),
        ("media_import", false, false, true),
        ("media_list", true, false, true),
        ("project_info", true, false, true),
        ("project_open", false, true, false),
        ("sequence_create", false, false, false),
        ("sequence_set_format", false, true, true),
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
                if matches!(key.as_str(), "description" | "title" | "x-ui")
                    || (key == "default" && value.is_null())
                    // `format: "uint32"` names a Rust width; it does not validate.
                    || (key == "format"
                        && value.as_str().is_some_and(|f| {
                            ["uint", "int", "float", "double"]
                                .iter()
                                .any(|p| f.starts_with(p))
                        }))
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
            enum_form(&mut out);
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(validation_only).collect()),
        other => other.clone(),
    }
}

/// Writes a string enum one way: `oneOf` of string `const`s (the registry's form) and
/// `anyOf[that, null]` become `{type: string, enum: [...]}` (the lean form), nullable or not.
fn enum_form(out: &mut serde_json::Map<String, Value>) {
    let consts = |list: &Value| -> Option<Vec<Value>> {
        let list = list.as_array().filter(|l| l.len() >= 2)?;
        list.iter()
            .map(|v| {
                let o = v.as_object()?;
                (o.len() == 2 && o.get("type") == Some(&json!("string")))
                    .then(|| o.get("const").cloned())
                    .flatten()
            })
            .collect()
    };
    if let Some(values) = out.get("oneOf").and_then(consts) {
        out.remove("oneOf");
        out.insert("type".into(), json!("string"));
        out.insert("enum".into(), Value::Array(values));
    }
    if let Some(Value::Array(list)) = out.get("anyOf")
        && list.len() == 2
        && list[1] == json!({ "type": "null" })
        && let Some(inner) = list[0].as_object()
        && inner.len() == 2
        && inner.get("type") == Some(&json!("string"))
        && let Some(Value::Array(values)) = inner.get("enum")
    {
        let mut values = values.clone();
        values.push(Value::Null);
        out.remove("anyOf");
        out.insert("type".into(), json!(["string", "null"]));
        out.insert("enum".into(), Value::Array(values));
    }
}

/// The listed schema with its shared `$defs` written back where they are referenced
/// (`{"$ref": "#/$defs/x", ...}` becomes `x`'s keywords plus the siblings), as the registry has
/// them. A reference to a definition that does not exist fails the test.
fn inline_defs(schema: &Value) -> Value {
    fn walk(value: &Value, defs: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
                    let name = reference
                        .strip_prefix("#/$defs/")
                        .unwrap_or_else(|| panic!("a non-local $ref: {reference}"));
                    let Some(Value::Object(def)) = defs.get(name) else {
                        panic!("$ref to a missing definition: {reference}");
                    };
                    for (k, v) in def {
                        out.insert(k.clone(), walk(v, defs));
                    }
                }
                for (k, v) in map {
                    if k != "$ref" && k != "$defs" {
                        out.insert(k.clone(), walk(v, defs));
                    }
                }
                Value::Object(out)
            }
            Value::Array(list) => Value::Array(list.iter().map(|v| walk(v, defs)).collect()),
            other => other.clone(),
        }
    }
    walk(schema, &schema["$defs"])
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
            validation_only(&inline_defs(&listed)),
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
    for profile in [Profile::Standard, Profile::Minimal, Profile::Described] {
        let catalog = Catalog::new(Arc::clone(&registry), profile);
        let size = catalog.size();
        assert!(profile.budget().fits(size));
        insta::assert_json_snapshot!(
            format!("tools_list_{}", profile.name()),
            catalog.tools_json()
        );
    }
}

// ----- ready for tool search (WP-2I.07) -----------------------------------------------------

/// Acceptance: the always-load hint is on the five tools an agent needs before it can search,
/// and on no other; no other tool has a `_meta` at all. The hint is `_meta`, so no input schema
/// changes: a listed schema is still the lean form of the registry's.
#[test]
fn the_always_load_hint_is_on_five_tools_and_nowhere_else() {
    let registry = registry();
    let five = [
        "project_info",
        "timeline_get",
        "cutweave_search_commands",
        "cutweave_describe_command",
        "cutweave_execute",
    ];
    for profile in Profile::ALL {
        let catalog = Catalog::new(Arc::clone(&registry), profile);
        let listed = catalog.tools_json();
        let mut marked = Vec::new();
        for tool in listed["tools"].as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            match tool.get("_meta") {
                None => {}
                Some(meta) => {
                    assert_eq!(meta, &json!({ "anthropic/alwaysLoad": true }), "{name}");
                    marked.push(name);
                }
            }
            if let Some(entry) = registry.get_by_tool_name(name) {
                let expected = catalog::listed_schema(&entry.meta, profile);
                assert_eq!(tool["inputSchema"], expected, "{name}");
            }
        }
        marked.sort_unstable();
        let mut expected = five.to_vec();
        expected.sort_unstable();
        assert_eq!(marked, expected, "{profile:?}");
    }
    // What the hint costs the standard list.
    let with = Catalog::new(Arc::clone(&registry), Profile::Standard).size();
    let hint = r#","_meta":{"anthropic/alwaysLoad":true}"#.len();
    println!(
        "always-load hint: {} bytes (~{} tokens) of {} bytes (~{} tokens)",
        hint * five.len(),
        (hint * five.len()).div_ceil(4),
        with.bytes,
        with.tokens()
    );
    assert!(STANDARD_BUDGET.fits(with));
}

/// Every `[[generic]]` entry of the table is a generic tool and the reverse; every
/// `[[namespace]]` is a namespace of the registry (plus `render`, which only the full engine
/// registers) and the reverse; every `[keywords]` key is a command or query an agent can reach.
#[test]
fn the_core_table_names_real_generic_tools_namespaces_and_commands() {
    let table = CoreTable::get();
    let registry = registry();
    let generic: Vec<&str> = table.generic.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(generic, catalog::GENERIC_TOOLS);
    let mut namespaces = instructions::namespaces(&registry);
    namespaces.push("render");
    namespaces.sort_unstable();
    let described: Vec<&str> = table.namespaces.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(
        described, namespaces,
        "a namespace is added or removed in mcp_core.toml and in the registry together"
    );
    let catalog = Catalog::new(Arc::clone(&registry), Profile::Standard);
    for command in table.keywords.keys() {
        // Only the full engine registers it (tests/mcp_visual.rs searches there).
        if command == "render.contact_sheet" {
            continue;
        }
        let entry = catalog
            .reachable(command)
            .unwrap_or_else(|| panic!("[keywords]: `{command}` is not a reachable command"));
        assert_eq!(
            entry.meta.name, command,
            "keyed by command name, not tool name"
        );
    }
}

/// The server instructions are a map of the catalog in at most 4,096 characters, on every
/// profile: every namespace with its purpose, describe before first use, batch through
/// `cutweave_execute`, the revision, undo and checkpoints. They name a command as a tool only
/// where it is listed.
#[tokio::test]
async fn the_instructions_are_a_map_of_the_catalog_within_4096_characters() {
    let registry = registry();
    let table = CoreTable::get();
    for profile in Profile::ALL {
        let catalog = Catalog::new(Arc::clone(&registry), profile);
        let text = catalog.instructions();
        let chars = text.chars().count();
        println!("instructions ({}): {chars} characters", profile.name());
        assert!(
            chars <= instructions::MAX_INSTRUCTIONS_CHARS,
            "{profile:?}: {chars} characters"
        );
        for namespace in instructions::namespaces(&registry) {
            let line = format!("- {namespace}: {}\n", table.purpose(namespace).unwrap());
            assert!(text.contains(&line), "{profile:?}: {namespace}");
        }
        for needle in [
            "Describe a command before its first use",
            "Batch related changes through cutweave_execute",
            "one undo step",
            "io.cutweave/rev",
            "expected_rev",
            "cutweave_search_commands",
            "cutweave_query",
            "untrusted",
        ] {
            assert!(text.contains(needle), "{profile:?}: `{needle}`");
        }
    }
    let standard = Catalog::new(Arc::clone(&registry), Profile::Standard);
    assert!(standard.instructions().contains("edit_checkpoint"));
    assert!(standard.instructions().contains("edit_redo brings it back"));
    let minimal = Catalog::new(Arc::clone(&registry), Profile::Minimal);
    assert!(minimal.instructions().contains("edit_undo takes back"));
    assert!(minimal.instructions().contains("edit.redo brings it back"));
    assert!(minimal.instructions().contains("edit.checkpoint"));

    // Served as built, on both revisions.
    let h = Harness::start(None);
    for revision in Revision::ALL {
        let client = h.client(Profile::Standard, revision).await;
        assert_eq!(
            client.service.peer_info().unwrap().instructions.as_deref(),
            Some(standard.instructions()),
            "{revision:?}"
        );
        client.close().await;
    }
}

/// Acceptance: 30 phrases in a user's words each find the right command among the first three
/// results of `cutweave_search_commands`.
#[test]
fn search_understands_the_words_a_user_would_say() {
    const PHRASES: [(&str, &str); 30] = [
        ("make the clip faster", "clip.set_speed"),
        ("slow this down", "clip.set_speed"),
        ("fade the music out", "audio.set_fade"),
        ("cut here", "clip.split"),
        ("remove the gap", "timeline.delete_gap"),
        ("turn the volume down", "audio.set_volume"),
        ("make it louder", "audio.set_volume"),
        ("mute the track", "track.set"),
        ("play it backwards", "clip.reverse"),
        ("freeze the picture", "clip.freeze_frame"),
        ("crop the video", "item.set_props"),
        ("rotate the clip", "item.set_props"),
        ("make the picture bigger", "item.set_props"),
        ("shorten the clip", "clip.trim"),
        ("undo that", "edit.undo"),
        ("save a restore point", "edit.checkpoint"),
        ("export for youtube", "export.start"),
        ("add my footage", "media.import"),
        ("make it vertical for tiktok", "sequence.set_format"),
        ("picture in picture", "item.apply_layout"),
        ("split screen", "item.apply_layout"),
        ("swap the footage", "clip.replace"),
        ("animate the opacity", "keyframe.set"),
        ("delete the clip", "item.delete"),
        ("find missing files", "media.find_missing"),
        ("bookmark this moment", "timeline.add_marker"),
        ("change the frame rate", "sequence.set_format"),
        ("separate the sound from the video", "item.unlink"),
        ("pan the sound to the left", "audio.set_pan"),
        ("how loud is it", "audio.get_levels"),
    ];
    let catalog = Catalog::new(registry(), Profile::Standard);
    let mut failures = Vec::new();
    for (phrase, command) in PHRASES {
        assert!(catalog.reachable(command).is_some(), "{command}");
        let found = search::find(&catalog, phrase, 3);
        let names: Vec<&str> = found.entries.iter().map(|e| e.meta.name).collect();
        if !names.contains(&command) {
            failures.push(format!("\"{phrase}\": {command} is not in {names:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ----- the described profile (WP-2I.07 part 2, ADR-0040) ------------------------------------

fn schema_of(tool: &Tool) -> Value {
    Value::Object(tool.input_schema.as_ref().clone())
}

/// Values to try a tool's two listed schemas with: the example (when there is one), nothing,
/// an unknown parameter, and each parameter of the example in turn missing or of a wrong type.
fn samples(example: Option<Value>) -> Vec<Value> {
    let mut out = vec![json!({}), json!({ "no_such_parameter": 1 })];
    let Some(example) = example else {
        return out;
    };
    let fields = example.as_object().unwrap().clone();
    for key in fields.keys() {
        let mut without = fields.clone();
        without.remove(key);
        out.push(Value::Object(without));
        for wrong in [
            json!([[true]]),
            json!(-7.25),
            json!("no such value"),
            json!(null),
        ] {
            let mut changed = fields.clone();
            changed.insert(key.clone(), wrong);
            out.push(Value::Object(changed));
        }
    }
    let mut extra = fields;
    extra.insert("no_such_parameter".to_owned(), json!(1));
    out.push(Value::Object(extra));
    out.push(example);
    out
}

/// Whether the parameter schema `param` says what the parameter is: a description of its own.
fn documented(param: &Value) -> bool {
    param
        .get("description")
        .and_then(Value::as_str)
        .is_some_and(|d| !d.trim().is_empty())
}

/// Acceptance (part 2): `/mcp/described` lists the same names in the same order as `/mcp`,
/// with the same descriptions, annotations and hints; every listed parameter has a
/// description; both lists accept and refuse the same values; and the list fits its own budget
/// of 16,000 tokens, which `mcp_core.toml` states.
#[test]
fn the_described_profile_is_the_standard_list_with_parameter_docs() {
    let registry = registry();
    let table = CoreTable::get();
    assert_eq!(table.profiles.described, *DESCRIBED_BUDGET);
    assert_eq!(Profile::Described.budget(), *DESCRIBED_BUDGET);
    assert_eq!(Profile::parse("described"), Some(Profile::Described));
    assert_eq!(Profile::Described.route(), "/described");

    let standard = Catalog::new(Arc::clone(&registry), Profile::Standard);
    let described = Catalog::new(Arc::clone(&registry), Profile::Described);
    assert_eq!(names(described.tools()), names(standard.tools()));
    assert_eq!(described.instructions(), standard.instructions());
    let size = described.size();
    println!(
        "/mcp/described: {} tools, {} bytes, ~{} tokens (gate {} / {}); /mcp: ~{} tokens",
        size.tools,
        size.bytes,
        size.tokens(),
        DESCRIBED_BUDGET.max_tools,
        DESCRIBED_BUDGET.max_tokens,
        standard.size().tokens()
    );
    assert!(
        DESCRIBED_BUDGET.fits(size),
        "{size:?} exceeds the described budget"
    );
    assert!(
        size.bytes > standard.size().bytes,
        "the parameter docs are there"
    );

    let mut undocumented = Vec::new();
    let mut tried = 0;
    for (with_docs, lean) in described.tools().iter().zip(standard.tools()) {
        let name = lean.name.as_ref();
        // Everything but the input schema is the same listing.
        let mut a = serde_json::to_value(with_docs).unwrap();
        let mut b = serde_json::to_value(lean).unwrap();
        a.as_object_mut().unwrap().remove("inputSchema");
        b.as_object_mut().unwrap().remove("inputSchema");
        assert_eq!(a, b, "{name}");

        let (full, listed) = (schema_of(with_docs), schema_of(lean));
        let entry = registry.get_by_tool_name(name);
        if let Some(entry) = entry {
            // A core tool: both are forms of the registry's schema, one with the parameter
            // docs, one without, and both accept what the registry's accepts. The described
            // list also shows the envelope's `expected_rev` on a mutating tool (WP-2I.04):
            // the generator's property, not the command's, and accepted on `/mcp` as well.
            let mut own = full.clone();
            if catalog::takes_expected_rev(&entry.meta) {
                let properties = own["properties"].as_object_mut().unwrap();
                let envelope = properties.remove(catalog::EXPECTED_REV).unwrap();
                assert!(documented(&envelope), "{name}");
                if properties.is_empty() && entry.meta.input_schema.get("properties").is_none() {
                    own.as_object_mut().unwrap().remove("properties");
                }
            }
            assert_eq!(
                own,
                catalog::described_schema(&entry.meta.input_schema),
                "{name}"
            );
            assert_eq!(
                listed,
                catalog::lean_schema(&entry.meta.input_schema),
                "{name}"
            );
        } else {
            assert!(catalog::GENERIC_TOOLS.contains(&name), "{name}");
        }
        let registry_schema = entry.map(|e| e.meta.input_schema.clone());
        let full = inline_defs(&full);
        for (param, schema) in full["properties"].as_object().into_iter().flatten() {
            if !documented(schema) {
                undocumented.push(format!("{name}.{param}"));
            }
        }
        // The same values pass and fail under both.
        let example = table
            .tools
            .iter()
            .find(|t| t.name == name)
            .map(example_args);
        let listed = inline_defs(&listed);
        for value in samples(example.clone()) {
            let (by_full, by_listed) = (
                schema_error(&full, &value, "").is_none(),
                schema_error(&listed, &value, "").is_none(),
            );
            assert_eq!(by_full, by_listed, "{name}: the lists disagree on {value}");
            if let Some(schema) = &registry_schema {
                let by_registry = schema_error(schema, &value, "").is_none();
                assert_eq!(by_full, by_registry, "{name}: not the command's on {value}");
            }
            if Some(&value) == example.as_ref() {
                assert!(by_full, "{name}: the example is valid");
            }
            tried += 1;
        }
    }
    assert!(tried > 300, "{tried} samples");
    assert!(
        undocumented.is_empty(),
        "parameters without a description on /mcp/described: {undocumented:?}"
    );
}

/// `/mcp/described` is served on both revisions, as built, and its tools run the same commands.
#[tokio::test]
async fn the_described_endpoint_serves_its_list_and_runs_the_same_tools() {
    let h = Harness::start(None);
    let catalog = Catalog::new(registry(), Profile::Described);
    assert!(h.url(Profile::Described).ends_with("/mcp/described"));
    for revision in Revision::ALL {
        let client = h.client(Profile::Described, revision).await;
        let listed = client.service.list_tools(None).await.unwrap();
        assert_eq!(
            json!({ "tools": listed.tools }),
            catalog.tools_json(),
            "{revision:?}"
        );
        let info = client.ok("project_info", json!({})).await;
        assert!(info["sequences"].is_array(), "{info}");
        let found = client
            .ok("cutweave_search_commands", json!({ "query": "cut here" }))
            .await;
        assert_eq!(found["matches"][0]["name"], "clip.split");
        assert_eq!(found["matches"][0]["call"], "tool clip_split");
        client.close().await;
    }
}

// ----- expected_rev on every mutating tool (WP-2I.04) ------------------------------------------

/// WP-2I.04 acceptance, over MCP: two clients. A reads at rev N, B edits (rev N+1), A's call
/// with `expected_rev: N` is a conflict that names both revisions and the item B touched, says
/// to read `timeline_get` again, and changes nothing; A retries with the new revision and
/// succeeds, and the result carries the next revision. On an endpoint whose list shows the
/// argument and on one whose list does not, on both revisions.
#[tokio::test]
async fn two_clients_a_stale_tool_call_conflicts_and_the_retry_succeeds() {
    let h = Harness::start(None);
    for profile in [Profile::Standard, Profile::Described] {
        for revision in Revision::ALL {
            let a = h.client(profile, revision).await;
            let b = h.client(profile, revision).await;
            let (sequence, _, _, item) = project_with_a_clip(&h, &a).await;
            let read = a
                .call("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
                .await;
            let n = rev_of(&read);

            let by_b = b
                .call(
                    "item_set_props",
                    json!({ "item": item, "props": { "name": "From B" } }),
                )
                .await;
            assert_eq!(rev_of(&by_b), n + 1);
            let before = timeline(&a, &sequence).await;

            let stale = a
                .err(
                    "clip_split",
                    json!({ "item": item, "at": "2s", "expected_rev": n }),
                )
                .await;
            let error = &stale["error"];
            assert_eq!(error["code"], "conflict", "{stale}");
            assert_eq!(error["reason"], "stale_rev");
            assert_eq!(error["current_rev"], n + 1);
            assert_eq!(error["expected_rev"], n);
            assert_eq!(error["changed_since"], json!({ "items": [item] }));
            assert!(
                error["hint"].as_str().unwrap().contains("timeline_get"),
                "{error}"
            );
            assert_eq!(timeline(&a, &sequence).await, before);
            assert_eq!(current_rev(&a).await, n + 1);

            let retried = a
                .call(
                    "clip_split",
                    json!({ "item": item, "at": "2s", "expected_rev": n + 1 }),
                )
                .await;
            assert_ne!(retried.is_error, Some(true), "{}", text_of(&retried));
            assert_eq!(rev_of(&retried), n + 2);
            assert!(text_of(&retried).contains(&format!("document at rev {}", n + 2)));
            a.close().await;
            b.close().await;
        }
    }
}

/// Every mutating core tool takes the argument (a wrong type is `invalid` at `/expected_rev`,
/// before the command runs); a read-only tool does not know it.
#[tokio::test]
async fn expected_rev_is_an_argument_of_every_mutating_core_tool_only() {
    let h = Harness::start(None);
    let client = h.client(Profile::Standard, Revision::Stateful).await;
    project_with_a_clip(&h, &client).await;
    let rev = current_rev(&client).await;
    let registry = registry();
    let catalog = Catalog::new(Arc::clone(&registry), Profile::Standard);
    let mut mutating = 0;
    for entry in registry.list().filter(|e| catalog.lists(&e.meta)) {
        let tool = entry.meta.mcp_tool_name();
        // No command has a parameter of that name: the envelope owns it.
        assert!(
            entry.meta.input_schema["properties"]
                .get(catalog::EXPECTED_REV)
                .is_none(),
            "{tool}"
        );
        if entry.meta.annotations.read_only {
            assert!(!catalog::takes_expected_rev(&entry.meta), "{tool}");
            continue;
        }
        assert!(catalog::takes_expected_rev(&entry.meta), "{tool}");
        mutating += 1;
        // Stale: refused before the command validates its own arguments.
        let stale = client.err(&tool, json!({ "expected_rev": rev + 9 })).await;
        assert_eq!(stale["error"]["code"], "conflict", "{tool}: {stale}");
        assert_eq!(stale["error"]["reason"], "stale_rev", "{tool}");
        assert_eq!(stale["error"]["current_rev"], rev, "{tool}");
        assert_eq!(stale["error"]["expected_rev"], rev + 9, "{tool}");
        let wrong = client.err(&tool, json!({ "expected_rev": "7" })).await;
        assert_eq!(wrong["error"]["code"], "invalid", "{tool}: {wrong}");
        assert_eq!(wrong["error"]["path"], "/expected_rev", "{tool}");
    }
    assert_eq!(mutating, 22);
    assert_eq!(current_rev(&client).await, rev);

    // A read-only tool: the command's schema refuses the unknown argument.
    let read = client
        .err("media_list", json!({ "expected_rev": rev }))
        .await;
    assert_eq!(read["error"]["code"], "invalid", "{read}");

    // The current revision passes, and `null` is no expectation at all.
    let made = client
        .call(
            "sequence_create",
            json!({ "name": "At the revision", "expected_rev": rev }),
        )
        .await;
    assert_ne!(made.is_error, Some(true), "{}", text_of(&made));
    assert_eq!(rev_of(&made), rev + 1);
    let made = client
        .call(
            "sequence_create",
            json!({ "name": "No expectation", "expected_rev": null }),
        )
        .await;
    assert_ne!(made.is_error, Some(true), "{}", text_of(&made));
    client.close().await;
}

/// The listed schemas: one generator adds the same property to every mutating tool of a list
/// that shows it (the described profile), and to no other tool; the lists that do not show it
/// (standard, full, minimal: ADR-0041) are unchanged, and `cutweave_execute` lists
/// `expected_rev` on every endpoint.
#[test]
fn the_generator_lists_expected_rev_uniformly() {
    let registry = registry();
    for profile in Profile::ALL {
        let catalog = Catalog::new(Arc::clone(&registry), profile);
        let execute = catalog.tool(catalog::EXECUTE).unwrap();
        assert_eq!(
            execute.input_schema["properties"]["expected_rev"]["type"], "integer",
            "{profile:?}"
        );
        let mut shown = Vec::new();
        for entry in registry.list().filter(|e| catalog.lists(&e.meta)) {
            let tool = catalog.tool(&entry.meta.mcp_tool_name()).unwrap();
            let property = tool
                .input_schema
                .get("properties")
                .and_then(|p| p.get(catalog::EXPECTED_REV));
            assert_eq!(
                property.is_some(),
                profile.lists_expected_rev() && catalog::takes_expected_rev(&entry.meta),
                "{profile:?}: {}",
                tool.name
            );
            if let Some(property) = property {
                shown.push(property.clone());
                let required = tool.input_schema.get("required").cloned();
                assert!(
                    !required
                        .unwrap_or_default()
                        .as_array()
                        .is_some_and(|r| r.contains(&json!("expected_rev"))),
                    "{}",
                    tool.name
                );
            }
        }
        // No per-command drift: every tool shows the very same subschema.
        shown.dedup();
        assert!(shown.len() <= 1, "{profile:?}: {shown:?}");
        assert_eq!(
            shown.len() == 1,
            profile.lists_expected_rev(),
            "{profile:?}"
        );
    }
    assert!(Profile::Described.lists_expected_rev());
    assert!(!Profile::Standard.lists_expected_rev());
}

// ----- profiles and their budgets (WP-2I.05) ------------------------------------------------

/// The budget check of every profile: each list, as served, fits the budget `mcp_core.toml`
/// (`[profiles]`) gives its profile. `[profiles]` is the only place the figures are written:
/// raising one is a diff to that file.
#[test]
fn every_profile_list_fits_its_budget() {
    let registry = registry();
    let table = CoreTable::get();
    for profile in Profile::ALL {
        let catalog = Catalog::new(Arc::clone(&registry), profile);
        let budget = profile.budget();
        assert_eq!(budget, profile.budget_in(&table.profiles));
        let size = catalog
            .check_budget()
            .unwrap_or_else(|over| panic!("/mcp{} ({}): {over}", profile.route(), profile.name()));
        println!(
            "{}: {} tools of {}, {} bytes, ~{} tokens of {}",
            profile.name(),
            size.tools,
            budget.max_tools,
            size.bytes,
            size.tokens(),
            budget.max_tokens
        );
        assert!(budget.max_tools > 0 && budget.max_tokens > 0);
    }
    // Full has no figures of its own: it lists the standard tools within the standard budget.
    assert_eq!(Profile::Full.budget(), table.profiles.standard);
}

/// The failing sample: the same check refuses a list that leaves its budget. For each profile,
/// a table whose budget is one token (or one tool) below the list as served fails it, and so
/// does the real budget once the list grows by one more tool's worth.
#[test]
fn the_budget_check_fails_a_list_that_leaves_its_budget() {
    let registry = registry();
    let real = CoreTable::get().profiles;
    for profile in Profile::ALL {
        let catalog = Catalog::new(Arc::clone(&registry), profile);
        let size = catalog.size();
        let sample = |max_tools: usize, max_tokens: usize| {
            let budget = format!("{{ max_tools = {max_tools}, max_tokens = {max_tokens} }}");
            let text = format!(
                "[profiles]\nminimal = {budget}\nstandard = {budget}\ndescribed = {budget}\n"
            );
            profile.budget_in(&CoreTable::parse(&text).unwrap().profiles)
        };
        // Exactly the list's size passes; one token or one tool less does not.
        assert_eq!(sample(size.tools, size.tokens()).check(size), Ok(()));
        assert_eq!(
            sample(size.tools, size.tokens() - 1).check(size),
            Err(catalog::OverBudget::Tokens {
                listed: size.tokens(),
                max: size.tokens() - 1
            }),
            "{profile:?}"
        );
        assert_eq!(
            sample(size.tools - 1, size.tokens()).check(size),
            Err(catalog::OverBudget::Tools {
                listed: size.tools,
                max: size.tools - 1
            }),
            "{profile:?}"
        );
        // The committed budget refuses the list once every tool is listed twice.
        let mut grown = catalog.tools().to_vec();
        grown.extend(catalog.tools().iter().cloned());
        let over = profile.budget_in(&real).check(ListSize::of(&grown));
        assert!(over.is_err(), "{profile:?}: {over:?}");
    }
    // The standard list is close to its budget: its largest tool listed once more leaves it.
    let standard = Catalog::new(registry, Profile::Standard);
    let mut grown = standard.tools().to_vec();
    let largest = grown
        .iter()
        .max_by_key(|t| ListSize::of(std::slice::from_ref(*t)).bytes)
        .unwrap()
        .clone();
    grown.push(largest);
    assert!(matches!(
        Profile::Standard.budget().check(ListSize::of(&grown)),
        Err(catalog::OverBudget::Tokens { .. })
    ));
}

/// Full lists exactly the standard tools; minimal lists a part of them, the same definitions in
/// the same order; described lists the same names in the same order. A list is built the same
/// way every time, so it is the same for every connection (ADR-0040).
#[test]
fn the_profiles_list_one_set_of_tools_in_one_order() {
    let registry = registry();
    let list = |profile| Catalog::new(Arc::clone(&registry), profile);
    let standard = list(Profile::Standard);
    assert_eq!(list(Profile::Full).tools(), standard.tools());
    assert_eq!(
        names(list(Profile::Described).tools()),
        names(standard.tools())
    );
    let minimal = list(Profile::Minimal);
    assert!(minimal.tools().len() < standard.tools().len());
    let mut rest = standard.tools().iter();
    for tool in minimal.tools() {
        assert!(
            rest.any(|t| t == tool),
            "{}: not the standard tool, or out of order",
            tool.name
        );
    }
    for profile in Profile::ALL {
        assert_eq!(
            list(profile).tools_json().to_string(),
            list(profile).tools_json().to_string(),
            "{profile:?}"
        );
    }
}

/// `cutweave_execute` reaches every command under minimal: the generic tools resolve every
/// entry that is not hidden, whatever the endpoint lists, and a live `/mcp/minimal` connection
/// gets past "unknown command" for each of them (a dry run with no arguments: nothing changes).
#[tokio::test]
async fn execute_reaches_every_command_under_minimal() {
    let registry = registry();
    let minimal = Catalog::new(Arc::clone(&registry), Profile::Minimal);
    let standard = Catalog::new(Arc::clone(&registry), Profile::Standard);
    let reachable: Vec<&str> = minimal.reachable_entries().map(|e| e.meta.name).collect();
    assert_eq!(
        reachable,
        standard
            .reachable_entries()
            .map(|e| e.meta.name)
            .collect::<Vec<_>>()
    );
    assert!(reachable.len() > 150, "{} entries", reachable.len());
    for entry in registry.list() {
        let hidden = entry.meta.mcp == cutweave_commands::McpExposure::Hidden;
        assert_eq!(minimal.reachable(entry.meta.name).is_some(), !hidden);
    }

    let h = Harness::start(None);
    let client = h.client(Profile::Minimal, Revision::Stateful).await;
    let listed = names(&client.service.list_tools(None).await.unwrap().tools);
    let mut unlisted = 0;
    for name in &reachable {
        unlisted += usize::from(!listed.contains(&name.replace('.', "_")));
        let result = client
            .call(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": name }], "dry_run": true }),
            )
            .await;
        let unknown = result.is_error == Some(true)
            && result.structured_content.as_ref().is_some_and(|e| {
                e["error"]["code"] == "not_found"
                    && e["error"]["hint"]
                        .as_str()
                        .is_some_and(|hint| hint.contains("cutweave_search_commands"))
            });
        assert!(!unknown, "{name}: {}", text_of(&result));
    }
    assert!(unlisted > 150, "most of them are not listed under minimal");
    // A name that is not a command is told so, so the loop above proves something.
    let e = client
        .err(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "clip.explode" }], "dry_run": true }),
        )
        .await;
    assert_eq!(e["error"]["code"], "not_found");
    assert!(
        e["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("cutweave_search_commands")
    );
    client.close().await;
}

/// A change of the tool list (what loading a plugin will be, from Phase 6) reaches a client of
/// every endpoint through the mechanism of its revision: `notifications/tools/list_changed` on
/// a 2025-11-25 session, and on a 2026-07-28 `subscriptions/listen` that asked for it. Nothing
/// else sends it, and it moves no connection to another list: a client that re-reads gets its
/// endpoint's list again, so `/mcp/minimal` never sees the standard tools.
#[tokio::test]
async fn a_tool_list_change_reaches_both_revisions_and_no_list_moves() {
    let h = Harness::start(None);
    let registry = h.engine.handle().registry();
    let mut watchers = Vec::new();
    for profile in Profile::ALL {
        for revision in Revision::ALL {
            let watcher = Watcher::http(&h, profile, revision).await;
            assert!(watcher.declared(), "{profile:?} {revision:?}");
            let before = watcher.tools().await;
            assert_eq!(
                before,
                Catalog::new(Arc::clone(&registry), profile).tools(),
                "{profile:?} {revision:?}"
            );
            watchers.push((profile, watcher, before));
        }
    }
    // A 2026-07-28 client that did not ask for it is not told.
    let quiet = h.client(Profile::Standard, Revision::Stateless).await;
    let filter = SubscriptionFilter::builder()
        .resource_subscription("cutweave://project")
        .build();
    let mut unasked = quiet.service.peer().listen(filter).await.unwrap();
    assert_eq!(unasked.acknowledged().tools_list_changed, None);

    // Editing, and replacing the document, change no tool list: nobody is told.
    let setup = h.direct("setup");
    must(&setup, "project.new", json!({ "name": "Lists" }));
    must(&setup, "sequence.create", json!({ "name": "Second" }));
    for (profile, watcher, _) in &mut watchers {
        let (revision, heard) = (
            watcher.revision,
            watcher.hears_a_change(Duration::from_millis(100)).await,
        );
        assert!(
            !heard,
            "{profile:?} {revision:?}: an edit is not a list change"
        );
    }

    h.notifier.tools_changed();
    for (profile, watcher, before) in &mut watchers {
        let revision = watcher.revision;
        assert!(
            watcher.hears_a_change(Duration::from_secs(10)).await,
            "{profile:?} {revision:?}: no notifications/tools/list_changed"
        );
        // The list is the endpoint's, as before: the same tools in the same order.
        let after = watcher.tools().await;
        assert_eq!(&after, before, "{profile:?} {revision:?}");
        if *profile == Profile::Minimal {
            assert!(after.len() <= Profile::Minimal.budget().max_tools);
            assert!(!after.iter().any(|t| t.name == "clip_split"));
        }
        // One change is one notification.
        assert!(
            !watcher.hears_a_change(Duration::from_millis(100)).await,
            "{profile:?} {revision:?}: a second notification"
        );
    }
    // The client that asked for the project only got the project's update (the edit above) and
    // no list change.
    loop {
        match tokio::time::timeout(Duration::from_millis(200), unasked.next()).await {
            Err(_) => break,
            Ok(note) => assert!(
                matches!(
                    note,
                    Ok(Some(ServerNotification::ResourceUpdatedNotification(_)))
                ),
                "{note:?}"
            ),
        }
    }
    // A client that connects after the change is not told about it.
    let mut late = Watcher::http(&h, Profile::Standard, Revision::Stateful).await;
    let _ = late.tools().await;
    assert!(!late.hears_a_change(Duration::from_millis(100)).await);
    h.notifier.tools_changed();
    assert!(late.hears_a_change(Duration::from_secs(10)).await);
    late.close().await;
    quiet.close().await;
    for (_, watcher, _) in watchers {
        watcher.close().await;
    }
}

// ----- the core tool table (WP-2I.01) -------------------------------------------------------

/// `mcp_core.toml` is the checked-in gate of the list: it names exactly the registry's core
/// tools (plus `render_frame`, which only the full engine registers: tests/mcp_frames.rs), its
/// budget is the standard profile's, and the whole planned core set fits the tool count.
#[test]
fn the_core_table_names_the_registry_core_tools_and_the_budget() {
    let table = CoreTable::get();
    let registry = registry();
    let mut expected: Vec<String> = registry
        .list()
        .filter(|e| catalog::is_core_tool(&e.meta))
        .map(|e| e.meta.mcp_tool_name())
        .collect();
    expected.push("render_frame".to_owned());
    expected.sort();
    let listed: Vec<&str> = table.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        listed, expected,
        "a core tool is added or removed in mcp_core.toml and in the registry together"
    );
    assert_eq!(table.profiles.standard, *STANDARD_BUDGET);
    // WBS WP-2I.01 planned 31 core tools. ADR-0039 keeps `media_filmstrip` and
    // `audio_waveform` in the catalog, so the P2 set is 29: the 28 listed and
    // `render_contact_sheet`, whose command exists (WP-2I.02) and which waits for room in the
    // standard list.
    let planned = table.tools.len()
        + table
            .pending
            .iter()
            .filter(|p| p.name == "render_contact_sheet")
            .count();
    assert_eq!(planned, 29);
    for catalog_only in ["media_filmstrip", "audio_waveform"] {
        assert!(table.pending.iter().all(|p| p.name != catalog_only));
        let entry = registry.get_by_tool_name(catalog_only).unwrap();
        assert_eq!(entry.meta.mcp, cutweave_commands::McpExposure::Catalog);
        assert!(entry.meta.annotations.read_only);
    }
    assert!(planned + catalog::GENERIC_TOOLS.len() <= table.profiles.standard.max_tools);
    for pending in &table.pending {
        assert!(
            registry
                .get_by_tool_name(&pending.name)
                .is_none_or(|e| !catalog::is_core_tool(&e.meta)),
            "{} is listed: move it out of [[pending]]",
            pending.name
        );
    }
}

/// Every core tool's example is a valid call: it validates against the registry's params schema
/// (what RPC and the CLI accept), and so against the listed one.
#[test]
fn every_core_tool_example_is_valid_under_the_command_schema() {
    let registry = registry();
    let catalog = Catalog::new(Arc::clone(&registry), Profile::Standard);
    for tool in &CoreTable::get().tools {
        // `render_frame`: tests/mcp_frames.rs, on the full engine.
        let Some(entry) = registry.get(&tool.command) else {
            assert_eq!(tool.name, "render_frame");
            continue;
        };
        let args = example_args(tool);
        assert_eq!(
            schema_error(&entry.meta.input_schema, &args, ""),
            None,
            "{}",
            tool.name
        );
        let listed = Value::Object(
            catalog
                .tool(&tool.name)
                .unwrap()
                .input_schema
                .as_ref()
                .clone(),
        );
        assert_eq!(
            schema_error(&inline_defs(&listed), &args, ""),
            None,
            "{}",
            tool.name
        );
    }
    // The check does reject: a wrong type, an unknown param, a bad id, a missing param.
    let split = &registry.get("clip.split").unwrap().meta.input_schema;
    let id = format!("itm_{}", "0".repeat(32));
    for (bad, at) in [
        (json!({ "item": id, "at": true }), "/at"),
        (json!({ "item": id, "at": 1, "bogus": 1 }), "/bogus"),
        (json!({ "item": "itm_1", "at": "2s" }), "/item"),
        (json!({ "item": id }), ""),
        (json!({ "item": id, "at": "2 seconds" }), "/at"),
    ] {
        let error = schema_error(split, &bad, "").unwrap_or_else(|| panic!("{bad} passes"));
        assert!(error.starts_with(&format!("{at}:")), "{error}");
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
        same!(
            "keyframe_set",
            json!({ "item": split["right"], "prop": "opacity",
                    "keys": [{ "at": "6s", "value": 0.0, "easing": "ease_out" },
                             { "at": "8s", "value": 1.0 }] })
        );
        same!(
            "audio_set",
            json!({ "item": split["right"], "set": { "kind": "fade", "in": "0.5s", "curve": "log" } })
        );
        same!(
            "clip_set_speed",
            json!({ "item": split["left"], "speed": "2x" })
        );
        same!(
            "timeline_add_marker",
            json!({ "sequence": seq, "t": "1s", "label": "A" })
        );
        same!("track_add", json!({ "sequence": seq, "kind": "video" }));
        // edit_checkpoint: each engine stamps it with its own clock.
        let timeless = |mut v: Value| {
            assert!(v["at"].is_string() || v["at"].is_number(), "{v}");
            v["at"] = Value::Null;
            v
        };
        let mcp = client
            .ok("edit_checkpoint", json!({ "name": "safe" }))
            .await;
        let engine = must(&direct, "edit.checkpoint", json!({ "name": "safe" }));
        compare("edit_checkpoint", &timeless(mcp), &timeless(engine));
        step("edit_checkpoint");
        same!("item_delete", json!({ "items": [split["right"]] }));
        same!("edit_undo", json!({}));
        same!("edit_redo", json!({}));
        same!("edit_restore", json!({ "checkpoint": "safe" }));
        same!("timeline_get", json!({ "sequence": seq, "depth": 2 }));
        let page = same!(
            "timeline_get",
            json!({ "sequence": seq, "depth": 2, "limit": 1 })
        );
        same!(
            "timeline_get",
            json!({ "sequence": seq, "depth": 2, "limit": 1, "cursor": page["next_cursor"],
                    "range": { "from": "0s", "to": "20s" } })
        );
        same!(
            "view_set",
            json!({ "patch": { "playhead": "1s",
                    "selection": [{ "kind": "item", "id": split["left"] }] } })
        );
        same!("view_get", json!({}));
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
        let reel = same!(
            "sequence_create",
            json!({ "name": "Reel", "preset": "9:16" })
        );
        same!(
            "sequence_set_format",
            json!({ "sequence": reel["sequence"], "width": 1080, "height": 1080 })
        );
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
            .err(
                "cutweave_query",
                json!({ "name": "track.remove", "args": {} }),
            )
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
        .ok(
            "cutweave_describe_command",
            json!({ "name": "edit.history" }),
        )
        .await;
    assert!(redo["call"].as_str().unwrap().starts_with("cutweave_query"));
    let redo = client
        .ok(
            "cutweave_describe_command",
            json!({ "name": "track.remove" }),
        )
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

/// WP-2I.08: the snapshots are from before the discovery tools took `detail` and `names`: a
/// call without them still returns those bytes.
#[tokio::test]
async fn search_and_describe_without_the_new_parameters_are_unchanged() {
    let h = Harness::start(None);
    let client = h.client(Profile::Standard, Revision::Stateless).await;
    for (snapshot, tool, args) in [
        (
            "search_default",
            "cutweave_search_commands",
            json!({ "query": "split clip", "limit": 3 }),
        ),
        (
            "search_no_match_default",
            "cutweave_search_commands",
            json!({ "query": "zzzz" }),
        ),
        (
            "describe_default",
            "cutweave_describe_command",
            json!({ "name": "edit.redo" }),
        ),
    ] {
        let out = client.ok(tool, args).await;
        insta::assert_snapshot!(snapshot, serde_json::to_string(&out).unwrap());
    }
    client.close().await;
}

/// WP-2I.08 (ADR-0040, question 5): `detail` of `cutweave_search_commands`. `summary` is the
/// default; `names` returns the names only; `schema` adds each hit's input schema, for at most
/// 5 hits.
#[tokio::test]
async fn search_returns_names_summaries_or_schemas() {
    let h = Harness::start(None);
    let client = h.client(Profile::Standard, Revision::Stateless).await;
    let search = |args: Value| client.ok("cutweave_search_commands", args);
    let default = search(json!({ "query": "split clip", "limit": 3 })).await;
    let summary = search(json!({ "query": "split clip", "limit": 3, "detail": "summary" })).await;
    assert_eq!(
        serde_json::to_string(&summary).unwrap(),
        serde_json::to_string(&default).unwrap(),
        "summary is the default"
    );

    let names = search(json!({ "query": "split clip", "limit": 3, "detail": "names" })).await;
    insta::assert_snapshot!("search_names", serde_json::to_string(&names).unwrap());
    assert_eq!(names["total"], default["total"]);
    let expected: Vec<&Value> = default["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| &m["name"])
        .collect();
    assert_eq!(
        names["matches"]
            .as_array()
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        expected
    );

    // Schemas are big: 5 hits at most, whatever the limit; `total` still counts every match.
    let registry = registry();
    let all = search(json!({ "query": "clip" })).await;
    for limit in [json!(null), json!(50)] {
        let mut args = json!({ "query": "clip", "detail": "schema" });
        if !limit.is_null() {
            args["limit"] = limit;
        }
        let with = search(args).await;
        let hits = with["matches"].as_array().unwrap();
        assert_eq!(hits.len(), 5);
        assert_eq!(with["total"], all["total"]);
        for (hit, plain) in hits.iter().zip(all["matches"].as_array().unwrap()) {
            let entry = registry.get(hit["name"].as_str().unwrap()).unwrap();
            assert_eq!(hit["input_schema"], entry.meta.input_schema, "verbatim");
            let mut rest = hit.clone();
            rest.as_object_mut().unwrap().remove("input_schema");
            assert_eq!(&rest, plain, "the summary, plus the schema");
        }
    }
    let two = search(json!({ "query": "clip", "detail": "schema", "limit": 2 })).await;
    assert_eq!(two["matches"].as_array().unwrap().len(), 2);
    let one = search(json!({ "query": "redo", "detail": "schema", "limit": 1 })).await;
    insta::assert_snapshot!("search_schema", serde_json::to_string(&one).unwrap());

    // No match: the same hint at every level.
    let none = search(json!({ "query": "zzzz", "detail": "names" })).await;
    assert_eq!(none["matches"], json!([]));
    assert!(none["hint"].as_str().unwrap().contains("timeline"));

    let e = client
        .err(
            "cutweave_search_commands",
            json!({ "query": "clip", "detail": "full" }),
        )
        .await;
    assert_eq!(e["error"]["code"], "invalid");
    assert_eq!(e["error"]["path"], "/detail");
    client.close().await;
}

/// WP-2I.08: `cutweave_describe_command{names}` describes up to 8 commands in one call; each
/// entry is exactly what `{name}` returns for it.
#[tokio::test]
async fn describe_takes_several_names() {
    let h = Harness::start(None);
    let client = h.client(Profile::Standard, Revision::Stateless).await;
    let describe = |args: Value| client.ok("cutweave_describe_command", args);
    let several = describe(json!({ "names": ["edit.redo", "edit.undo"] })).await;
    insta::assert_snapshot!("describe_names", serde_json::to_string(&several).unwrap());
    let commands = several["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 2);
    for (command, name) in commands.iter().zip(["edit.redo", "edit.undo"]) {
        let single = describe(json!({ "name": name })).await;
        assert_eq!(
            serde_json::to_string(command).unwrap(),
            serde_json::to_string(&single).unwrap(),
            "{name}"
        );
    }
    // A tool name works like a command name, and one name is still a list.
    let one = describe(json!({ "names": ["clip_trim"] })).await;
    assert_eq!(one["commands"][0]["name"], "clip.trim");
    // `null` is an absent parameter.
    let single = describe(json!({ "name": "edit.redo", "names": null })).await;
    assert_eq!(single["name"], "edit.redo");

    let eight = [
        "clip.split",
        "clip.trim",
        "edit.undo",
        "edit.redo",
        "item.move",
        "item.delete",
        "timeline.get",
        "media.list",
    ];
    let most = describe(json!({ "names": eight })).await;
    assert_eq!(most["commands"].as_array().unwrap().len(), 8);

    let invalid = |args: Value| client.err("cutweave_describe_command", args);
    let mut nine = eight.to_vec();
    nine.push("track.add");
    for (args, path) in [
        (json!({ "names": nine }), "/names"),
        (json!({ "names": [] }), "/names"),
        (json!({ "names": "clip.split" }), "/names"),
        (json!({ "names": ["clip.split", 7] }), "/names/1"),
        (
            json!({ "name": "clip.split", "names": ["clip.trim"] }),
            "/names",
        ),
    ] {
        let e = invalid(args).await;
        assert_eq!(e["error"]["code"], "invalid", "{path}");
        assert_eq!(e["error"]["path"], path);
    }
    // One unknown (or hidden) name fails the call, like the single form.
    let e = invalid(json!({ "names": ["clip.split", "media.register"] })).await;
    assert_eq!(e["error"]["code"], "not_found");
    client.close().await;
}

/// WP-2I.08: `detail` reaches `timeline_get` and `media_list` over MCP like any other parameter
/// (the tools are the registered queries), also through `cutweave_query`, and the result keeps
/// its revision.
#[tokio::test]
async fn concise_reads_over_mcp() {
    let h = Harness::start(None);
    let client = h.client(Profile::Standard, Revision::Stateless).await;
    let (sequence, _, _, item) = project_with_a_clip(&h, &client).await;
    let full = client
        .ok("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
        .await;
    let concise = client
        .ok(
            "timeline_get",
            json!({ "sequence": sequence, "depth": 2, "detail": "concise" }),
        )
        .await;
    let (whole, short) = (
        &full["tracks"][0]["items"][0],
        &concise["tracks"][0]["items"][0],
    );
    assert_eq!(short["id"], json!(item));
    assert_eq!(short["start"], whole["start"]);
    assert_eq!(short["duration"], whole["duration"]);
    assert!(whole["media"].is_string() && short.get("media").is_none());
    assert!(
        serde_json::to_string(&concise).unwrap().len()
            < serde_json::to_string(&full).unwrap().len()
    );
    let queried = client
        .ok(
            "cutweave_query",
            json!({ "name": "timeline.get",
                    "args": { "sequence": sequence, "depth": 2, "detail": "concise" } }),
        )
        .await;
    assert_eq!(queried, concise);

    let media = client.ok("media_list", json!({})).await;
    let brief = client
        .ok("media_list", json!({ "detail": "concise" }))
        .await;
    assert!(media["media"][0]["path"].is_string());
    assert!(brief["media"][0].get("path").is_none());
    assert_eq!(brief["media"][0]["id"], media["media"][0]["id"]);
    assert_eq!(brief["media"][0]["name"], media["media"][0]["name"]);

    // The revision is on a concise result as on a full one.
    let result = client
        .call(
            "timeline_get",
            json!({ "sequence": sequence, "detail": "concise" }),
        )
        .await;
    let meta = serde_json::to_value(&result.meta).unwrap();
    assert!(meta["io.cutweave/rev"].is_u64(), "{meta}");
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
    // The fan-out forwards events on its own task, so the undo's `doc.patched` may arrive
    // after the response (#242): wait for it, with a bound, instead of draining once.
    let mut patched = Vec::new();
    let wait = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("the event stream stays open");
            if event.name() != "doc.patched" {
                continue;
            }
            patched.push(event.payload()["actor"].as_str().unwrap().to_owned());
            // The split and its undo are the two changes made with the CLI token.
            if patched.iter().filter(|a| *a == "agent:cli").count() == 2 {
                break;
            }
        }
    })
    .await;
    assert!(
        wait.is_ok(),
        "no doc.patched for the undo within 10 s: {patched:?}"
    );
    assert!(
        patched.iter().all(|a| a == "agent:mcp" || a == "agent:cli"),
        "{patched:?}"
    );
    assert_eq!(patched.last().map(String::as_str), Some("agent:cli"));
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
