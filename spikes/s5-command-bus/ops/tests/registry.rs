//! The registry API WP-0H.02 builds on: iterate entries, read metadata/annotations/schemas,
//! look up by wire or MCP tool name, invoke with JSON.

// Test crate: helpers outside #[test] fns may unwrap (clippy.toml only exempts #[test] bodies).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_pass_by_value
)]

mod common;

use common::{agent, engine};
use s5_bus::{EntryKind, McpTier, Registry};
use serde_json::json;

#[test]
fn inventory_collects_entries_across_crates() {
    let names: Vec<&str> = s5_ops::registry().iter().map(|e| e.meta.name).collect();
    assert_eq!(
        names,
        [
            "ai.set_key", // UI-only (WP-0H.02)
            "clip.split",
            "clip.trim",
            "edit.redo", // registered in s5-bus
            "edit.undo", // registered in s5-bus
            "item.delete",
            "media.import",
            "project.open",
            "render.frame",
            "timeline.get",
            "timeline.insert",
        ]
    );
    assert!(
        Registry::collect().is_ok(),
        "no duplicate wire or tool names"
    );
}

#[test]
fn metadata_and_mcp_annotations() {
    let reg = s5_ops::registry();
    let split = reg.get("clip.split").unwrap();
    assert_eq!(split.meta.kind, EntryKind::Command);
    assert!(split.meta.undoable);
    assert_eq!(split.meta.mcp, McpTier::Core);
    assert_eq!(split.meta.mcp_tool_name(), "clip_split");
    assert_eq!(split.meta.rust_type, "SplitClip");
    assert_eq!(
        serde_json::to_value(split.meta.annotations).unwrap(),
        json!({ "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false })
    );

    let delete = reg.get_by_tool_name("item_delete").unwrap();
    assert!(delete.meta.annotations.destructive);
    assert!(reg.get("clip.trim").unwrap().meta.annotations.idempotent);

    let frame = reg.get("render.frame").unwrap();
    assert_eq!(frame.meta.kind, EntryKind::Query);
    assert!(frame.meta.annotations.read_only && frame.meta.annotations.idempotent);
    assert!(!frame.meta.undoable);

    assert!(!reg.get("project.open").unwrap().meta.undoable);
    assert!(reg.get("edit.undo").unwrap().is_engine_op());
    assert_eq!(reg.get("edit.redo").unwrap().meta.mcp, McpTier::Catalog);

    // UI-only scope (arch §6) is registry metadata; only `ai.set_key` has it.
    let ui_only: Vec<&str> = reg
        .iter()
        .filter(|e| e.meta.ui_only)
        .map(|e| e.meta.name)
        .collect();
    assert_eq!(ui_only, ["ai.set_key"]);
    assert_eq!(reg.get("ai.set_key").unwrap().meta.mcp, McpTier::Catalog);
}

#[test]
fn entries_invoke_through_the_registry() {
    let mut e = engine();
    let a = agent();
    let reg = s5_ops::registry();
    let imported = reg
        .get("media.import")
        .unwrap()
        .invoke(&mut e, &a, json!({ "path": "/x.mov" }))
        .unwrap();
    assert_eq!(imported.result["kind"], "video");
    // What an MCP `tools/call` handler would do: tool name -> entry -> JSON in/out.
    let entry = reg.get_by_tool_name("timeline_get").unwrap();
    let tl = entry.invoke(&mut e, &a, json!(null)).unwrap();
    assert_eq!(tl.result["media"].as_array().map(Vec::len), Some(1));
}

#[test]
fn summaries_fit_the_tool_list_budget() {
    // Rough tool-list cost (0H.02 measures properly): name + summary + schema bytes / 4.
    let reg = s5_ops::registry();
    let core: Vec<_> = reg.iter().filter(|e| e.meta.mcp == McpTier::Core).collect();
    let bytes: usize = core
        .iter()
        .map(|e| e.meta.name.len() + e.meta.summary.len() + e.params_schema().to_string().len())
        .sum();
    let approx_tokens = bytes / 4;
    println!("core tools: {}, approx tokens: {approx_tokens}", core.len());
    assert!(core.len() <= 12, "minimal profile budget: <= 12 tools");
    assert!(
        approx_tokens <= 3000,
        "minimal profile budget: <= 3k tokens (got ~{approx_tokens})"
    );
}
