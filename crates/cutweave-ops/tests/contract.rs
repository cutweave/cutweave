// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.05/1B.06: the registry contract of the ops v1a and v1b commands: every entry has a summary, an
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
        "media.import_images_as_slideshow",
        EntryKind::Engine,
        McpExposure::Catalog,
        false,
        false,
        false,
        false,
    ),
    (
        "media.import_frame",
        EntryKind::Engine,
        McpExposure::Catalog,
        false,
        false,
        false,
        false,
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
        "media.register_tree",
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
        "media.import_sequence",
        EntryKind::Engine,
        McpExposure::Catalog,
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
        "media.create_folder",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    (
        "media.rename_folder",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.rename",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.move",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.delete_folder",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        true,
        false,
        true,
    ),
    (
        "media.list_folders",
        EntryKind::Query,
        McpExposure::Catalog,
        true,
        false,
        true,
        false,
    ),
    (
        "media.add_tags",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.remove_tags",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.rename_tag",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.set_rating",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.set_note",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.list_tags",
        EntryKind::Query,
        McpExposure::Catalog,
        true,
        false,
        true,
        false,
    ),
    (
        "media.create_collection",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    (
        "media.rename_collection",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.delete_collection",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        true,
        false,
        true,
    ),
    (
        "media.add_to_collection",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.remove_from_collection",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.set_collection_filter",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "media.list_collections",
        EntryKind::Query,
        McpExposure::Catalog,
        true,
        false,
        true,
        false,
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
        "track.reorder",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "track.set_height",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "track.set_magnetic",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "track.list",
        EntryKind::Query,
        McpExposure::Catalog,
        true,
        false,
        true,
        false,
    ),
    (
        "timeline.add_marker",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    (
        "timeline.move_marker",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "timeline.remove_marker",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        true,
        false,
        true,
    ),
    (
        "timeline.set_in_out",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "timeline.clear_in_out",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "timeline.list_markers",
        EntryKind::Query,
        McpExposure::Catalog,
        true,
        false,
        true,
        false,
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
    (
        "clip.split",
        EntryKind::Command,
        McpExposure::Core,
        false,
        false,
        false,
        true,
    ),
    (
        "clip.trim",
        EntryKind::Command,
        McpExposure::Core,
        false,
        false,
        true,
        true,
    ),
    // WP-2A.03: a `delta` move repeated moves again, so the idempotent hint is off.
    (
        "item.move",
        EntryKind::Command,
        McpExposure::Core,
        false,
        false,
        false,
        true,
    ),
    (
        "item.delete",
        EntryKind::Command,
        McpExposure::Core,
        false,
        true,
        false,
        true,
    ),
    // WP-2A.03 (timeline spec §4, ADR-0023): every new placement command is in the catalog
    // (R33: no new core MCP tools).
    (
        "item.duplicate",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    // WP-2A.05 (timeline spec 4.15, 4.16): links and attachments, catalog only.
    (
        "item.link",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    (
        "item.unlink",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "item.attach",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "item.detach",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "item.group_select",
        EntryKind::Query,
        McpExposure::Catalog,
        true,
        false,
        true,
        false,
    ),
    (
        "timeline.overwrite",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        true,
        false,
        true,
    ),
    (
        "timeline.lift",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        true,
        false,
        true,
    ),
    (
        "timeline.extract",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        true,
        false,
        true,
    ),
    (
        "timeline.ripple_delete",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        true,
        false,
        true,
    ),
    (
        "timeline.delete_gap",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    (
        "timeline.copy",
        EntryKind::Query,
        McpExposure::Catalog,
        true,
        false,
        true,
        false,
    ),
    (
        "timeline.paste",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    (
        "clip.paste_attributes",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "clip.replace",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    (
        "item.set_props",
        EntryKind::Command,
        McpExposure::Core,
        false,
        false,
        true,
        true,
    ),
    (
        "item.get",
        EntryKind::Query,
        McpExposure::Catalog,
        true,
        false,
        true,
        false,
    ),
    // WP-2C.03: keyframes and property resets.
    (
        "keyframe.set",
        EntryKind::Command,
        McpExposure::Core,
        false,
        false,
        true,
        true,
    ),
    (
        "keyframe.delete",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        true,
        false,
        true,
    ),
    (
        "keyframe.move",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    (
        "keyframe.set_interp",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "keyframe.clear",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        true,
        true,
        true,
    ),
    (
        "keyframe.paste",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "keyframe.copy",
        EntryKind::Query,
        McpExposure::Catalog,
        true,
        false,
        true,
        false,
    ),
    (
        "keyframe.eval",
        EntryKind::Query,
        McpExposure::Catalog,
        true,
        false,
        true,
        false,
    ),
    (
        "item.reset_props",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "item.apply_layout",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "item.layouts",
        EntryKind::Query,
        McpExposure::Catalog,
        true,
        false,
        true,
        false,
    ),
    (
        "audio.set",
        EntryKind::Command,
        McpExposure::Core,
        false,
        false,
        true,
        true,
    ),
    (
        "audio.set_fade",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "audio.set_volume",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "audio.set_pan",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "audio.set_gain",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    // WP-1D.07: engine-state readers are read-only engine commands (like `edit.history`).
    (
        "job.status",
        EntryKind::Engine,
        McpExposure::Core,
        true,
        false,
        true,
        false,
    ),
    (
        "job.list",
        EntryKind::Engine,
        McpExposure::Catalog,
        true,
        false,
        true,
        false,
    ),
    (
        "job.wait",
        EntryKind::Engine,
        McpExposure::Core,
        true,
        false,
        true,
        false,
    ),
    (
        "job.cancel",
        EntryKind::Engine,
        McpExposure::Catalog,
        false,
        false,
        true,
        false,
    ),
    // WP-1E.03: starts a job (not journaled); may replace an existing file (`overwrite`).
    (
        "export.start",
        EntryKind::Engine,
        McpExposure::Core,
        false,
        true,
        false,
        false,
    ),
    // WP-2B.01: sequences.
    (
        "sequence.create",
        EntryKind::Command,
        McpExposure::Core,
        false,
        false,
        false,
        true,
    ),
    (
        "sequence.create_from_media",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    (
        "sequence.duplicate",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        false,
        true,
    ),
    (
        "sequence.rename",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        false,
        true,
        true,
    ),
    (
        "sequence.delete",
        EntryKind::Command,
        McpExposure::Catalog,
        false,
        true,
        false,
        true,
    ),
    (
        "sequence.set_format",
        EntryKind::Command,
        McpExposure::Core,
        false,
        true,
        true,
        true,
    ),
    (
        "sequence.list",
        EntryKind::Query,
        McpExposure::Catalog,
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
fn every_v1_command_is_registered_with_its_contract() {
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
        "clip_split",
        "clip_trim",
        "item_move",
        "item_delete",
        "item_set_props",
        "job_status",
        "job_wait",
        "export_start",
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
    for name in [
        "timeline.insert",
        "media.import",
        "clip.split",
        "clip.trim",
        "item.move",
        "item.set_props",
    ] {
        assert!(
            !registry.get(name).unwrap().meta.annotations.read_only,
            "{name}"
        );
    }
    for name in [
        "timeline.get",
        "media.list",
        "project.info",
        "item.get",
        "job.status",
        "job.list",
        "job.wait",
    ] {
        let m = &registry.get(name).unwrap().meta;
        assert!(
            m.annotations.read_only && m.annotations.idempotent && !m.is_journaled(),
            "{name}"
        );
    }
    for name in [
        "media.remove",
        "media.delete_folder",
        "track.remove",
        "item.delete",
    ] {
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
    assert_eq!(required("media.import"), ["paths"]);
    assert_eq!(required("media.import_images_as_slideshow"), ["files"]);
    assert_eq!(required("media.import_frame"), ["source", "at"]);
    assert_eq!(required("media.import_sequence"), ["path"]);
    assert_eq!(required("media.create_folder"), Vec::<String>::new());
    assert_eq!(required("media.rename_folder"), ["folder", "name"]);
    assert_eq!(required("media.rename"), ["media"]);
    assert_eq!(required("media.move"), Vec::<String>::new());
    assert_eq!(required("media.delete_folder"), ["folder", "contents"]);
    assert_eq!(required("track.set"), ["track"]);
    assert_eq!(required("clip.split"), ["at"]);
    assert_eq!(required("clip.trim"), ["item", "edge"]);
    assert_eq!(required("item.move"), Vec::<String>::new());
    assert_eq!(
        required("timeline.overwrite"),
        ["sequence", "track", "media", "at"]
    );
    assert_eq!(required("timeline.lift"), ["sequence"]);
    assert_eq!(required("timeline.extract"), ["sequence"]);
    assert_eq!(required("timeline.ripple_delete"), ["items"]);
    assert_eq!(required("timeline.delete_gap"), ["track", "at"]);
    assert_eq!(required("item.duplicate"), ["items"]);
    assert_eq!(required("timeline.copy"), ["items"]);
    assert_eq!(required("timeline.paste"), ["payload", "at"]);
    assert_eq!(required("clip.paste_attributes"), ["from", "to", "which"]);
    assert_eq!(required("clip.replace"), ["item", "media"]);
    assert_eq!(required("item.delete"), ["items"]);
    assert_eq!(required("item.link"), ["items"]);
    assert_eq!(required("item.unlink"), ["items"]);
    assert_eq!(required("item.attach"), ["items", "parent"]);
    assert_eq!(required("item.detach"), ["items"]);
    assert_eq!(required("item.group_select"), ["items"]);
    assert_eq!(required("item.set_props"), ["props"]);
    assert_eq!(required("keyframe.set"), ["prop", "keys"]);
    assert_eq!(required("keyframe.delete"), ["item", "keys"]);
    assert_eq!(required("keyframe.move"), ["item", "keys"]);
    assert_eq!(required("keyframe.eval"), ["item", "prop"]);
    assert_eq!(required("item.reset_props"), Vec::<String>::new());
    assert_eq!(required("item.apply_layout"), ["items", "layout"]);
    assert_eq!(required("item.layouts"), Vec::<String>::new());
    assert_eq!(required("item.get"), ["item"]);
    assert_eq!(required("project.new"), ["name"]);
    assert_eq!(required("job.status"), ["id"]);
    assert_eq!(required("job.wait"), ["job_id"]);
    assert_eq!(required("job.cancel"), ["id"]);
    assert_eq!(required("job.list"), Vec::<String>::new());
    assert_eq!(required("export.start"), ["path"]);
}

#[test]
fn job_cancel_is_not_journaled_and_analysis_writes_no_document_command() {
    let registry = registry();
    // Job state is not persisted, so there is nothing to replay (WP-1D.07).
    let cancel = &registry.get("job.cancel").unwrap().meta;
    assert!(!cancel.journaled && !cancel.is_journaled());
    // An export is a job, not a document change: nothing to replay (WP-1E.03).
    let export = &registry.get("export.start").unwrap().meta;
    assert!(!export.journaled && !export.is_journaled());
    for name in ["media.import", "media.register", "clip.split"] {
        assert!(registry.get(name).unwrap().meta.journaled, "{name}");
    }
    // Background analysis reports through jobs and events, never through the document.
    assert!(registry.get("media.set_analysis_error").is_none());
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
