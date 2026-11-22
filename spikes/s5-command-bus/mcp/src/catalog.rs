// SPDX-License-Identifier: GPL-3.0-or-later
//! The hybrid tool catalog (arch §14, ADR-009).
//!
//! - **Core tools**: every registry entry with `mcp = "core"` (and not UI-only) becomes a tool.
//!   Name = `namespace_verb` (`clip_split`), description = the macro `summary`, `inputSchema` =
//!   the registry params schema *verbatim* (parity with RPC), `annotations` = macro metadata.
//!   Results carry `structuredContent` = the RPC reply shape `{rev, result, dry_run?,
//!   deduplicated?}`; the matching `outputSchema` is listed only in `full` (token budget).
//! - **Generic tools** (always listed): `cutweave_search_commands`, `cutweave_describe_command`,
//!   `cutweave_execute` and `cutweave_query`. They reach every non-hidden entry, including
//!   `catalog` ones that are not listed as tools.
//! - **Profiles** pick the listed tools. They are chosen per endpoint (`/mcp/minimal`, `/mcp`,
//!   `/mcp/full`) or by `--profile` on stdio, never per connection: the 2026-07-28 revision is
//!   stateless and list results must not vary by connection.
//!
//! Image outputs: an output schema with string `mime` + `data` properties is an image. MCP
//! returns the bytes as an `image` content block and drops `data` from `structuredContent` (and
//! from the advertised `outputSchema`), so the base64 is never sent twice.

use std::{collections::HashMap, sync::Arc};

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use s5_bus::{CommandEntry, EntryKind, McpTier, Registry};
use serde_json::{Value, json};

/// Tool profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Small-context clients: 3 core tools + the 4 generic tools.
    Minimal,
    /// The default (`/mcp`): every core tool + the 4 generic tools.
    Standard,
    /// `standard` plus the search-based long tail, with no extra listed tools (arch §14). In the
    /// spike the generic tools already reach every non-hidden entry in every profile, so `full`
    /// lists the same tools as `standard`, but with their `outputSchema`s.
    Full,
}

/// A tool-list budget (arch §14, single source; CI-enforced by `tests/tool_budget.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Max listed tools.
    pub max_tools: usize,
    /// Max estimated tokens of the `tools/list` result (see [`crate::budget`]).
    pub max_tokens: usize,
}

/// Phase 1 gate for the standard profile (WBS WP-0H.02): <= 19 tools / <= 5k tokens.
pub const P1_GATE: Budget = Budget {
    max_tools: 19,
    max_tokens: 5_000,
};

impl Profile {
    /// All profiles.
    pub const ALL: [Profile; 3] = [Profile::Minimal, Profile::Standard, Profile::Full];

    /// Parses `minimal | standard | full`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Profile> {
        match s {
            "minimal" => Some(Profile::Minimal),
            "standard" => Some(Profile::Standard),
            "full" => Some(Profile::Full),
            _ => None,
        }
    }

    /// Name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Minimal => "minimal",
            Profile::Standard => "standard",
            Profile::Full => "full",
        }
    }

    /// Streamable HTTP endpoint path.
    #[must_use]
    pub fn http_path(self) -> &'static str {
        match self {
            Profile::Minimal => "/mcp/minimal",
            Profile::Standard => "/mcp",
            Profile::Full => "/mcp/full",
        }
    }

    /// Whether entry tools advertise `outputSchema`. Measured in WP-0H.02: output schemas are
    /// ~45% of the standard `tools/list` bytes (`timeline_get` alone ~3.3 KB), and the
    /// model-facing definition (name, description, inputSchema) does not need them. Results carry
    /// `structuredContent` in every profile; only `full` also lists the schemas.
    #[must_use]
    pub fn lists_output_schemas(self) -> bool {
        self == Profile::Full
    }

    /// The arch §14 budget.
    #[must_use]
    pub fn budget(self) -> Budget {
        match self {
            Profile::Minimal => Budget {
                max_tools: 12,
                max_tokens: 3_000,
            },
            Profile::Standard | Profile::Full => Budget {
                max_tools: 36,
                max_tokens: 8_000,
            },
        }
    }
}

/// Registry entries the minimal profile lists as tools: look (`timeline.get`, `render.frame`) and
/// recover (`edit.undo`). Everything else goes through the generic tools.
///
/// Finding: this belongs in registry metadata (e.g. `mcp = "essential"`), not in the transport.
pub const MINIMAL_CORE: &[&str] = &["timeline.get", "render.frame", "edit.undo"];

/// `cutweave_search_commands`.
pub const SEARCH: &str = "cutweave_search_commands";
/// `cutweave_describe_command`.
pub const DESCRIBE: &str = "cutweave_describe_command";
/// `cutweave_execute`.
pub const EXECUTE: &str = "cutweave_execute";
/// `cutweave_query`.
pub const QUERY: &str = "cutweave_query";

/// What a listed tool does.
#[derive(Clone, Copy)]
pub enum ToolTarget {
    /// A registry entry exposed directly.
    Entry(&'static CommandEntry),
    /// `cutweave_search_commands`.
    Search,
    /// `cutweave_describe_command`.
    Describe,
    /// `cutweave_execute`.
    Execute,
    /// `cutweave_query`.
    Query,
}

/// The tools of one profile, computed once and shared by every connection to that endpoint.
pub struct Catalog {
    profile: Profile,
    registry: &'static Registry,
    tools: Vec<Tool>,
    targets: HashMap<String, ToolTarget>,
}

impl Catalog {
    /// Builds the catalog of `profile`. Order is deterministic (registry entries by name, then
    /// the generic tools), which the 2026-07-28 revision recommends for prompt caching.
    #[must_use]
    pub fn new(registry: &'static Registry, profile: Profile) -> Catalog {
        let mut tools = Vec::new();
        let mut targets = HashMap::new();
        for entry in registry.iter().filter(|e| is_listed(profile, e)) {
            let tool = entry_tool(entry, profile.lists_output_schemas());
            targets.insert(tool.name.to_string(), ToolTarget::Entry(entry));
            tools.push(tool);
        }
        for (tool, target) in generic_tools() {
            targets.insert(tool.name.to_string(), target);
            tools.push(tool);
        }
        Catalog {
            profile,
            registry,
            tools,
            targets,
        }
    }

    /// Shared catalog for `profile` over the s5 registry.
    #[must_use]
    pub fn shared(profile: Profile) -> Arc<Catalog> {
        Arc::new(Catalog::new(crate::registry(), profile))
    }

    /// Profile.
    #[must_use]
    pub fn profile(&self) -> Profile {
        self.profile
    }

    /// The registry.
    #[must_use]
    pub fn registry(&self) -> &'static Registry {
        self.registry
    }

    /// Listed tools, in `tools/list` order.
    #[must_use]
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// What tool `name` does, if it is listed in this profile.
    #[must_use]
    pub fn target(&self, name: &str) -> Option<ToolTarget> {
        self.targets.get(name).copied()
    }

    /// Whether `entry` is listed as its own tool in this profile.
    #[must_use]
    pub fn lists(&self, entry: &CommandEntry) -> bool {
        is_listed(self.profile, entry)
    }

    /// Resolves a name for the generic tools: wire name (`clip.split`) or tool name
    /// (`clip_split`). Hidden entries are not reachable from MCP at all.
    #[must_use]
    pub fn reachable(&self, name: &str) -> Option<&'static CommandEntry> {
        self.registry
            .get(name)
            .or_else(|| self.registry.get_by_tool_name(name))
            .filter(|e| e.meta.mcp != McpTier::Hidden)
    }

    /// Every entry reachable through the generic tools, by name.
    pub fn all_reachable(&self) -> impl Iterator<Item = &'static CommandEntry> + '_ {
        self.registry
            .iter()
            .filter(|e| e.meta.mcp != McpTier::Hidden)
    }
}

fn is_listed(profile: Profile, entry: &CommandEntry) -> bool {
    entry.meta.mcp == McpTier::Core
        && !entry.meta.ui_only
        && (profile != Profile::Minimal || MINIMAL_CORE.contains(&entry.meta.name))
}

/// Whether an entry's output is an image (`mime` + `data` string properties).
#[must_use]
pub fn has_image_output(entry: &CommandEntry) -> bool {
    let schema = entry.output_schema();
    let props = &schema["properties"];
    props["mime"]["type"] == "string" && props["data"]["type"] == "string"
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        // The registry guarantees object roots (RegistryError::NotAnObject); keep MCP valid anyway.
        _ => JsonObject::from_iter([("type".to_owned(), json!("object"))]),
    }
}

/// The MCP `outputSchema` of an entry tool: the RPC reply `{rev, result, dry_run?,
/// deduplicated?}` with `result` = the registry output schema (minus `data` for images).
#[must_use]
pub fn entry_output_schema(entry: &CommandEntry) -> Value {
    let mut result = entry.output_schema();
    if has_image_output(entry) {
        if let Some(props) = result["properties"].as_object_mut() {
            props.remove("data");
        }
        if let Some(required) = result["required"].as_array_mut() {
            required.retain(|r| r != "data");
        }
    }
    json!({
        "type": "object",
        "properties": {
            "rev": { "type": "integer", "description": "Document revision after the call" },
            "result": result,
            "dry_run": { "type": "boolean" },
            "deduplicated": { "type": "boolean" }
        },
        "required": ["rev", "result"]
    })
}

fn annotations(read_only: bool, destructive: bool, idempotent: bool) -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(read_only)
        .destructive(destructive)
        .idempotent(idempotent)
        .open_world(false)
}

fn entry_tool(entry: &'static CommandEntry, with_output_schema: bool) -> Tool {
    let a = entry.meta.annotations;
    let mut description = entry.meta.summary.to_owned();
    if has_image_output(entry) {
        description.push_str(" (returned as image content)");
    }
    let tool = Tool::new(
        entry.meta.mcp_tool_name(),
        description,
        object(entry.params_schema()),
    )
    .with_annotations(
        annotations(a.read_only, a.destructive, a.idempotent).open_world(a.open_world),
    );
    if with_output_schema {
        tool.with_raw_output_schema(Arc::new(object(entry_output_schema(entry))))
    } else {
        tool
    }
}

fn generic_tools() -> Vec<(Tool, ToolTarget)> {
    let read_only = annotations(true, false, true);
    vec![
        (
            Tool::new(
                SEARCH,
                "Find Cutweave commands by keyword, including ones not listed as tools",
                object(json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Words matched against command names and summaries; \"\" lists all"
                        },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Max results (default 20)" }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                })),
            )
            .with_annotations(read_only.clone()),
            ToolTarget::Search,
        ),
        (
            Tool::new(
                DESCRIBE,
                "Get a command's params/result JSON Schemas and how to call it",
                object(json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "e.g. clip.split" }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                })),
            )
            .with_annotations(read_only.clone()),
            ToolTarget::Describe,
        ),
        (
            Tool::new(
                EXECUTE,
                "Run commands as one undoable, all-or-nothing transaction",
                object(json!({
                    "type": "object",
                    "properties": {
                        "steps": {
                            "type": "array",
                            "minItems": 1,
                            "description": "In order. In params, {\"$ref\":\"#/steps/N/field\"} is replaced by that field of step N's result",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string", "description": "e.g. clip.split" },
                                    "params": { "type": "object" }
                                },
                                "required": ["name"],
                                "additionalProperties": false
                            }
                        },
                        "dry_run": { "type": "boolean", "description": "Validate and return results without applying" },
                        "expected_rev": { "type": "integer", "description": "Fail unless the document is at this revision" },
                        "request_id": { "type": "string", "description": "Unique id; a retry with it is applied once" }
                    },
                    "required": ["steps"],
                    "additionalProperties": false
                })),
            )
            // Can run destructive commands (item.delete): conservative hint.
            .with_annotations(annotations(false, true, false)),
            ToolTarget::Execute,
        ),
        (
            Tool::new(
                QUERY,
                "Run a read-only query by name, e.g. timeline.get",
                object(json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "args": { "type": "object" }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                })),
            )
            .with_annotations(read_only),
            ToolTarget::Query,
        ),
    ]
}

/// Search result entry (also used by `describe`).
#[must_use]
pub fn entry_summary(catalog: &Catalog, entry: &CommandEntry) -> Value {
    let mut v = json!({
        "name": entry.meta.name,
        "summary": entry.meta.summary,
        "kind": entry.meta.kind,
    });
    if catalog.lists(entry) {
        v["tool"] = json!(entry.meta.mcp_tool_name());
    }
    if entry.meta.ui_only {
        v["ui_only"] = json!(true);
    }
    v
}

/// How an agent calls `entry` on this endpoint.
#[must_use]
pub fn how_to_call(catalog: &Catalog, entry: &CommandEntry) -> String {
    if entry.meta.ui_only {
        "UI only: agents cannot call it; ask the user to do this in the Cutweave app".into()
    } else if catalog.lists(entry) {
        format!("tool {}", entry.meta.mcp_tool_name())
    } else if entry.meta.kind == EntryKind::Query {
        format!(
            "{QUERY} {{\"name\":\"{}\",\"args\":{{...}}}}",
            entry.meta.name
        )
    } else {
        format!(
            "{EXECUTE} {{\"steps\":[{{\"name\":\"{}\",\"params\":{{...}}}}]}}",
            entry.meta.name
        )
    }
}
