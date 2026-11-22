// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.05: the registry contract of the ops v1a commands: every entry has a summary, an
//! object schema, the right MCP exposure and annotations (arch §14), and a snapshot of its
//! full schema (a diff means the command contract changed and needs review).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use cutweave_commands::{EntryKind, McpExposure, Registry, Scope};
use serde_json::json;

/// name, kind, mcp, read_only, destructive, idempotent, undoable
type Row = (&'static str, EntryKind, McpExposure, bool, bool, bool, bool);

const CONTRACT: &[Row] = &[
    (
        "project.new",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    (
        "project.info",
        EntryKind::Query,
        McpExposure::Core,
        true,
        false,
        true,
        false,
    ),
    (
        "project.rename",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.register",
        EntryKind::Command,
        McpExposure::Hidden,
        false,
        false,
        true,
        true,
    ),
    (
        "media.import",
        EntryKind::Engine,
        McpExposure::Core,
        false,
        false,
        true,
        false,
    ),
    (
        "media.list",
        EntryKind::Query,
        McpExposure::Core,
        true,
        false,
        true,
        false,
    ),
    (
        "media.remove",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        true,
        false,
        true,
    ),
    (
        "track.add",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    (
        "track.remove",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        true,
        false,
        true,
    ),
    (
        "track.set",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "timeline.insert",
        EntryKind::Command,
        McpExposure::Core,
        false,
        false,
        false,
        true,
    ),
    (
        "timeline.get",
        EntryKind::Query,
        McpExposure::Core,
        true,
        false,
        true,
        false,
    ),
];

fn registry() -> Registry {
    let mut registry = Registry::new();
    cutweave_ops::register(&mut registry).unwrap();
    registry
}

#[test]
fn every_registered_entry_has_a_summary_and_object_schemas() {
    let registry = registry();
    assert!(
        registry.len() >= CONTRACT.len() + 5,
        "ops and view are all registered"
    );
    for entry in registry.list() {
        let m = &entry.meta;
        assert!(!m.summary.trim().is_empty(), "{} has no summary", m.name);
        assert_eq!(m.input_schema["type"], "object", "{}", m.name);
        assert_eq!(m.input_schema["additionalProperties"], false, "{}", m.name);
        assert_eq!(m.output_schema["type"], "object", "{}", m.name);
    }
}

#[test]
fn every_v1a_command_is_registered_with_its_contract() {
    let registry = registry();
    for &(name, kind, mcp, read_only, destructive, idempotent, undoable) in CONTRACT {
        let m = &registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .meta;
        assert_eq!(m.kind, kind, "{name} kind");
        assert_eq!(m.mcp, mcp, "{name} mcp");
        assert_eq!(m.annotations.read_only, read_only, "{name} readOnlyHint");
        assert_eq!(
            m.annotations.destructive, destructive,
            "{name} destructiveHint"
        );
        assert_eq!(
            m.annotations.idempotent, idempotent,
            "{name} idempotentHint"
        );
        assert!(!m.annotations.open_world, "{name} openWorldHint");
        assert_eq!(m.undoable, undoable, "{name} undoable");
        assert_eq!(m.scope, Scope::Any, "{name} scope");
        assert!(!m.view && m.secret_fields.is_empty(), "{name}");
    }
}

#[test]
fn the_arch_14_core_tools_of_this_wp_are_core_and_named_with_underscores() {
    let registry = registry();
    let core: Vec<String> = registry
        .list()
        .filter(|e| e.meta.mcp == McpExposure::Core)
        .map(|e| e.meta.mcp_tool_name())
        .collect();
    for tool in [
        "project_info",
        "media_import",
        "media_list",
        "timeline_insert",
        "timeline_get",
    ] {
        assert!(
            core.contains(&tool.to_owned()),
            "{tool} must be a core MCP tool: {core:?}"
        );
    }
    assert_eq!(
        registry.get_by_tool_name("timeline_insert").unwrap().name(),
        "timeline.insert"
    );
}

#[test]
fn mutating_core_tools_are_not_read_only_and_queries_are() {
    let registry = registry();
    for name in ["timeline.insert", "media.import"] {
        assert!(
            !registry.get(name).unwrap().meta.annotations.read_only,
            "{name}"
        );
    }
    for name in ["timeline.get", "media.list", "project.info"] {
        let m = &registry.get(name).unwrap().meta;
        assert!(
            m.annotations.read_only && m.annotations.idempotent && !m.is_journaled(),
            "{name}"
        );
    }
    for name in ["media.remove", "track.remove"] {
        assert!(
            registry.get(name).unwrap().meta.annotations.destructive,
            "{name}"
        );
    }
}

#[test]
fn optional_params_are_not_required_in_the_input_schema() {
    let registry = registry();
    let required = |n: &str| -> Vec<String> {
        registry.get(n).unwrap().meta.input_schema["required"]
            .as_array()
            .map_or_else(Vec::new, |a| {
                a.iter().map(|v| v.as_str().unwrap().to_owned()).collect()
            })
    };
    assert_eq!(
        required("timeline.insert"),
        ["sequence", "track", "media", "at"]
    );
    assert_eq!(required("media.list"), Vec::<String>::new());
    assert_eq!(required("track.set"), ["track"]);
    assert_eq!(required("project.new"), ["name"]);
}

#[test]
fn schema_snapshots() {
    let registry = registry();
    for &(name, ..) in CONTRACT {
        let meta = &registry.get(name).unwrap().meta;
        insta::assert_json_snapshot!(
            name.replace('.', "_"),
            json!({
                "meta": serde_json::to_value(meta).unwrap(),
                "input": meta.input_schema,
                "output": meta.output_schema,
            })
        );
    }
}
