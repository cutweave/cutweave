// SPDX-License-Identifier: GPL-3.0-or-later
//! Serializable views of the [`Registry`] (arch §5.3, WP-1N.01): the catalogs that `cargo xtask
//! gen` writes to `schema/`.
//!
//! Everything here is a pure function of the registry and static tables, so the output is
//! deterministic: entries are sorted by name, there is no wall clock and no randomness. Key
//! order inside an entry is declaration order; the generator canonicalizes (sorts) keys when it
//! writes files.

use serde_json::{Value, json};

use crate::{
    CmdError,
    meta::{CommandMeta, EntryKind, McpExposure},
    registry::Registry,
};

/// Version of the catalog file layout. Bump when the shape of `schema/*.json` changes.
pub const CATALOG_FORMAT: u32 = 1;

/// The JSON Schema dialect of every `input_schema` / `output_schema`.
pub const JSON_SCHEMA_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

fn error_codes() -> Value {
    Value::Array(
        CmdError::CODES
            .iter()
            .map(|(code, summary)| json!({"code": code, "summary": summary}))
            .collect(),
    )
}

fn entry_json(meta: &CommandMeta) -> Value {
    let mut v = serde_json::to_value(meta).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut v {
        map.insert("namespace".into(), meta.namespace().into());
        map.insert(
            "mcp_tool".into(),
            if meta.mcp == McpExposure::Hidden {
                Value::Null
            } else {
                meta.mcp_tool_name().into()
            },
        );
    }
    v
}

fn catalog(registry: &Registry, key: &str, include: impl Fn(EntryKind) -> bool) -> Value {
    let entries: Vec<Value> = registry
        .list()
        .filter(|e| include(e.kind()))
        .map(|e| entry_json(&e.meta))
        .collect();
    let mut root = serde_json::Map::new();
    root.insert("format".into(), CATALOG_FORMAT.into());
    root.insert("json_schema_dialect".into(), JSON_SCHEMA_DIALECT.into());
    root.insert("error_codes".into(), error_codes());
    root.insert(key.into(), Value::Array(entries));
    Value::Object(root)
}

/// `schema/commands.json`: every document command and engine command (`edit.*`, `view.*`),
/// sorted by name, with flags, MCP exposure and annotations, input and output JSON Schema.
/// `error_codes` lists the machine codes of [`CmdError`] (any command can return them).
#[must_use]
pub fn commands_catalog(registry: &Registry) -> Value {
    catalog(registry, "commands", |k| k != EntryKind::Query)
}

/// `schema/queries.json`: every query, in the same shape as [`commands_catalog`].
#[must_use]
pub fn queries_catalog(registry: &Registry) -> Value {
    catalog(registry, "queries", |k| k == EntryKind::Query)
}

/// `schema/mcp-tools.json`: the core MCP tools (`mcp = "core"`), sorted by tool name. The tool
/// name is the dotted command name with `.` replaced by `_` (`item.move` is `item_move`).
/// Schemas stay in the commands/queries catalogs (look up `command`).
#[must_use]
pub fn mcp_tools_catalog(registry: &Registry) -> Value {
    let mut tools: Vec<Value> = registry
        .list()
        .filter(|e| e.meta.mcp == McpExposure::Core)
        .map(|e| {
            json!({
                "name": e.meta.mcp_tool_name(),
                "command": e.meta.name,
                "kind": e.meta.kind,
                "description": e.meta.summary,
                "annotations": e.meta.annotations,
            })
        })
        .collect();
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    json!({
        "format": CATALOG_FORMAT,
        "naming": "tool name = command name with '.' replaced by '_'",
        "tools": tools,
    })
}
