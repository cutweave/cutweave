// SPDX-License-Identifier: GPL-3.0-or-later
//! The hybrid tool catalog (arch §14, ADR-0009), generated from the command registry.
//!
//! - **Core tools:** every registry entry with `mcp = "core"` (never UI-only or `view`) is a
//!   tool named after the command with `.` replaced by `_` (`clip_split`), described by its
//!   `summary`, with the registry's MCP annotations. Its `inputSchema` is the registry params
//!   schema in [lean](lean_schema) form: validation-identical, without the documentation-only
//!   keywords that would blow the token budget (the full schema and docs are one
//!   `cutweave_describe_command` away).
//! - **Generic tools** (always listed): [`SEARCH`], [`DESCRIBE`], [`EXECUTE`] and [`QUERY`]. They
//!   reach every entry that is not `hidden`, including `catalog` ones (`edit.redo`, `track.add`).
//! - **Profiles** pick the listed tools per endpoint, never per connection (the 2026-07-28
//!   revision is stateless, so list results must not depend on the connection).
//!
//! # Tool-list budget (single source, arch §14)
//!
//! Tokens are estimated as `ceil(bytes / 4)` of the compact JSON of the `tools/list` result
//! (`{"tools":[...]}`), [`ListSize`]. There is no offline tokenizer; the estimate is close for
//! prose and low for dense JSON, so budgets keep headroom. Minimal: <= 12 tools / <= 3k tokens.
//! Standard: <= 36 tools / <= 8k tokens, and [`P1_GATE`] (<= 19 tools / <= 5k tokens) during
//! Phase 1. Full: standard plus the search-based long tail, so in Phase 1 it lists exactly the
//! standard tools. `outputSchema`s are not listed (Spike S5: about 45% of the bytes); results
//! still carry `structuredContent`.

use std::{collections::HashMap, sync::Arc};

use cutweave_commands::{CommandMeta, Entry, EntryKind, McpExposure, Registry, Scope};
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde_json::{Value, json};

/// `cutweave_search_commands{query, limit?}`.
pub const SEARCH: &str = "cutweave_search_commands";
/// `cutweave_describe_command{name}`.
pub const DESCRIBE: &str = "cutweave_describe_command";
/// `cutweave_execute{steps[{cmd, args}], dry_run?, expected_rev?}`.
pub const EXECUTE: &str = "cutweave_execute";
/// `cutweave_query{name, args}`.
pub const QUERY: &str = "cutweave_query";

/// The generic tools, in listing order.
pub const GENERIC_TOOLS: [&str; 4] = [SEARCH, DESCRIBE, EXECUTE, QUERY];

/// Registry entries the minimal profile lists as tools: look (`project.info`, `media.list`,
/// `timeline.get`, `render.frame`), wait (`job.wait`) and recover (`edit.undo`). Everything
/// else goes through the generic tools. Entries that are not registered (or not core) yet are
/// skipped: `render.frame` arrives with WP-1J.02.
pub const MINIMAL_CORE: &[&str] = &[
    "project.info",
    "media.list",
    "timeline.get",
    "render.frame",
    "job.wait",
    "edit.undo",
];

/// A tool-list budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Most tools listed.
    pub max_tools: usize,
    /// Most estimated tokens of the `tools/list` result ([`ListSize::tokens`]).
    pub max_tokens: usize,
}

impl Budget {
    /// Whether `size` fits.
    #[must_use]
    pub const fn fits(self, size: ListSize) -> bool {
        size.tools <= self.max_tools && size.tokens() <= self.max_tokens
    }
}

/// The Phase 1 gate of the standard profile (WP-1J.01): <= 19 tools and <= 5k tokens.
pub const P1_GATE: Budget = Budget {
    max_tools: 19,
    max_tokens: 5_000,
};

/// A tool profile, chosen by the endpoint (`/mcp`, `/mcp/minimal`, `/mcp/full`) or, on stdio,
/// by `--profile` (WP-1J.03).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Small-context clients: the [`MINIMAL_CORE`] tools plus the 4 generic tools.
    Minimal,
    /// The default (`/mcp`): every core tool plus the 4 generic tools.
    Standard,
    /// Standard plus the search-based long tail, with no extra listed tools (Phase 1: the same
    /// list as standard).
    Full,
}

impl Profile {
    /// Every profile.
    pub const ALL: [Profile; 3] = [Profile::Minimal, Profile::Standard, Profile::Full];

    /// `minimal`, `standard` or `full`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Profile::Minimal => "minimal",
            Profile::Standard => "standard",
            Profile::Full => "full",
        }
    }

    /// Parses [`name`](Self::name).
    #[must_use]
    pub fn parse(name: &str) -> Option<Profile> {
        Profile::ALL.into_iter().find(|p| p.name() == name)
    }

    /// The route under the `/mcp` mount: `/` (`/mcp`), `/minimal`, `/full`.
    #[must_use]
    pub const fn route(self) -> &'static str {
        match self {
            Profile::Minimal => "/minimal",
            Profile::Standard => "/",
            Profile::Full => "/full",
        }
    }

    /// The arch §14 budget.
    #[must_use]
    pub const fn budget(self) -> Budget {
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

/// What a listed tool runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// A registry entry, by wire name.
    Entry(&'static str),
    /// [`SEARCH`].
    Search,
    /// [`DESCRIBE`].
    Describe,
    /// [`EXECUTE`].
    Execute,
    /// [`QUERY`].
    Query,
}

/// Whether `meta` is listed as a tool of its own in the standard profile: `mcp = "core"`, any
/// caller (never UI-only) and not a `view` command.
#[must_use]
pub fn is_core_tool(meta: &CommandMeta) -> bool {
    meta.mcp == McpExposure::Core && meta.scope == Scope::Any && !meta.view
}

fn listed(profile: Profile, meta: &CommandMeta) -> bool {
    is_core_tool(meta) && (profile != Profile::Minimal || MINIMAL_CORE.contains(&meta.name))
}

/// The tools of one profile, built once and shared by every session of its endpoint.
pub struct Catalog {
    profile: Profile,
    registry: Arc<Registry>,
    tools: Vec<Tool>,
    targets: HashMap<String, Target>,
}

impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Catalog")
            .field("profile", &self.profile)
            .field("tools", &self.tools.len())
            .finish_non_exhaustive()
    }
}

impl Catalog {
    /// The catalog of `profile` over `registry`. The order is deterministic (core tools by
    /// tool name, then the generic tools), which also keeps clients' prompt caches warm.
    #[must_use]
    pub fn new(registry: Arc<Registry>, profile: Profile) -> Catalog {
        let mut entries: Vec<&Entry> = registry
            .list()
            .filter(|e| listed(profile, &e.meta))
            .collect();
        entries.sort_by_key(|e| e.meta.mcp_tool_name());
        let mut tools = Vec::new();
        let mut targets = HashMap::new();
        for entry in entries {
            let tool = entry_tool(&entry.meta);
            targets.insert(tool.name.to_string(), Target::Entry(entry.meta.name));
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

    /// The profile.
    #[must_use]
    pub fn profile(&self) -> Profile {
        self.profile
    }

    /// The registry.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The listed tools, in `tools/list` order.
    #[must_use]
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// The listed tool `name`.
    #[must_use]
    pub fn tool(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.name == name)
    }

    pub(crate) fn target(&self, name: &str) -> Option<Target> {
        self.targets.get(name).copied()
    }

    /// Whether `meta` is listed as a tool of its own on this endpoint.
    #[must_use]
    pub fn lists(&self, meta: &CommandMeta) -> bool {
        listed(self.profile, meta)
    }

    /// The entry the generic tools resolve `name` to: a wire name (`clip.split`) or a tool name
    /// (`clip_split`). `hidden` entries are not reachable over MCP.
    #[must_use]
    pub fn reachable(&self, name: &str) -> Option<&Entry> {
        self.registry
            .get(name)
            .or_else(|| self.registry.get_by_tool_name(name))
            .filter(|e| e.meta.mcp != McpExposure::Hidden)
    }

    /// Every entry reachable through the generic tools, by name.
    pub fn reachable_entries(&self) -> impl Iterator<Item = &Entry> + '_ {
        self.registry
            .list()
            .filter(|e| e.meta.mcp != McpExposure::Hidden)
    }

    /// The `tools/list` result as JSON (`{"tools": [...]}`), exactly as served.
    #[must_use]
    pub fn tools_json(&self) -> Value {
        json!({ "tools": self.tools })
    }

    /// The size of this endpoint's `tools/list` result.
    #[must_use]
    pub fn size(&self) -> ListSize {
        ListSize::of(&self.tools)
    }
}

/// The measured size of a `tools/list` result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListSize {
    /// Listed tools.
    pub tools: usize,
    /// Bytes of the compact JSON of `{"tools": [...]}`.
    pub bytes: usize,
}

impl ListSize {
    /// Measures `tools`.
    #[must_use]
    pub fn of(tools: &[Tool]) -> ListSize {
        ListSize {
            tools: tools.len(),
            bytes: json!({ "tools": tools }).to_string().len(),
        }
    }

    /// Estimated tokens: `ceil(bytes / 4)`.
    #[must_use]
    pub const fn tokens(self) -> usize {
        self.bytes.div_ceil(4)
    }
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        // The registry guarantees object roots (`RegistryError::NotAnObject`).
        _ => JsonObject::from_iter([("type".to_owned(), json!("object"))]),
    }
}

fn annotations(meta: &CommandMeta) -> ToolAnnotations {
    let a = meta.annotations;
    ToolAnnotations::new()
        .read_only(a.read_only)
        .destructive(a.destructive)
        .idempotent(a.idempotent)
        .open_world(a.open_world)
}

fn entry_tool(meta: &CommandMeta) -> Tool {
    Tool::new(
        meta.mcp_tool_name(),
        meta.summary,
        object(lean_schema(&meta.input_schema)),
    )
    .with_annotations(annotations(meta))
}

/// The listed form of a registry params schema: the same validation, without the keywords that
/// only document. Removes the root `description` (the long doc comment; the tool description is
/// the summary) and every `title`, drops `"default": null`, and flattens
/// `anyOf[{anyOf[A, B], description}, {type: null}]` into `anyOf[A, B, {type: null}]` (keeping
/// the outer description). Property descriptions, enums, patterns, bounds, `required` and
/// `additionalProperties` are kept as they are, so a value is valid under the lean schema
/// exactly when it is valid under the registry's.
#[must_use]
pub fn lean_schema(schema: &Value) -> Value {
    let mut out = schema.clone();
    if let Value::Object(root) = &mut out {
        root.remove("description");
    }
    trim(&mut out);
    out
}

/// Keywords whose value is a map of subschemas (keys are names, not keywords).
const SCHEMA_MAPS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
];
/// Keywords whose value is a subschema.
const SCHEMA_VALUES: &[&str] = &[
    "items",
    "additionalProperties",
    "unevaluatedProperties",
    "unevaluatedItems",
    "propertyNames",
    "contains",
    "not",
    "if",
    "then",
    "else",
];
/// Keywords whose value is a list of subschemas.
const SCHEMA_LISTS: &[&str] = &["anyOf", "oneOf", "allOf", "prefixItems"];

fn trim(schema: &mut Value) {
    let Value::Object(map) = schema else {
        return;
    };
    map.remove("title");
    if map.get("default") == Some(&Value::Null) {
        map.remove("default");
    }
    flatten_nullable(map);
    for (key, value) in map.iter_mut() {
        let key = key.as_str();
        if SCHEMA_MAPS.contains(&key) {
            if let Value::Object(children) = value {
                children.values_mut().for_each(trim);
            }
        } else if SCHEMA_VALUES.contains(&key) {
            trim(value);
        } else if SCHEMA_LISTS.contains(&key)
            && let Value::Array(list) = value
        {
            list.iter_mut().for_each(trim);
        }
    }
}

/// `anyOf[A, {type: null}]` (an optional field): A's own description is dropped when the field
/// has one, and a bare `A = {anyOf: [...]}` is spliced in.
fn flatten_nullable(map: &mut JsonObject) {
    let has_description = map.contains_key("description");
    let Some(Value::Array(any_of)) = map.get_mut("anyOf") else {
        return;
    };
    if any_of.len() != 2 || any_of[1] != json!({ "type": "null" }) {
        return;
    }
    let Value::Object(inner) = &mut any_of[0] else {
        return;
    };
    if let Some(description) = inner.remove("description")
        && !has_description
    {
        map.insert("description".into(), description);
    }
    let Some(Value::Array(any_of)) = map.get_mut("anyOf") else {
        return;
    };
    if let Value::Object(inner) = &any_of[0]
        && inner.len() == 1
        && let Some(Value::Array(alternatives)) = inner.get("anyOf")
    {
        let mut spliced = alternatives.clone();
        spliced.push(json!({ "type": "null" }));
        *any_of = spliced;
    }
}

fn generic_annotations(read_only: bool) -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(read_only)
        .destructive(!read_only)
        .idempotent(read_only)
        .open_world(false)
}

fn generic_tools() -> Vec<(Tool, Target)> {
    vec![
        (
            Tool::new(
                SEARCH,
                "Find commands and queries by keyword, including ones not listed as tools",
                object(json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Words matched against names and summaries; \"\" lists all"
                        },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Default 20" }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                })),
            )
            .with_annotations(generic_annotations(true)),
            Target::Search,
        ),
        (
            Tool::new(
                DESCRIBE,
                "Get one command's docs, annotations and full input/output JSON Schemas",
                object(json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "e.g. clip.split" }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                })),
            )
            .with_annotations(generic_annotations(true)),
            Target::Describe,
        ),
        (
            Tool::new(
                EXECUTE,
                "Run commands as one all-or-nothing transaction and one undo step",
                object(json!({
                    "type": "object",
                    "properties": {
                        "steps": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": crate::mcp::execute::MAX_STEPS,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "cmd": { "type": "string", "description": "e.g. clip.split" },
                                    "args": {
                                        "type": "object",
                                        "description": "Params; {\"$ref\":\"steps[0].output.left\"} inserts an earlier step's output"
                                    }
                                },
                                "required": ["cmd"],
                                "additionalProperties": false
                            }
                        },
                        "dry_run": { "type": "boolean", "description": "Validate and list the predicted ops; change nothing" },
                        "expected_rev": { "type": "integer", "minimum": 0, "description": "Fail unless the document is at this revision" }
                    },
                    "required": ["steps"],
                    "additionalProperties": false
                })),
            )
            // It can run any command, `item.delete` included: the conservative hints.
            .with_annotations(generic_annotations(false)),
            Target::Execute,
        ),
        (
            Tool::new(
                QUERY,
                "Run a read-only query by name, e.g. item.get or edit.history",
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
            .with_annotations(generic_annotations(true)),
            Target::Query,
        ),
    ]
}

/// How an agent calls `meta` on an endpoint listing `catalog`'s tools.
#[must_use]
pub fn how_to_call(catalog: &Catalog, meta: &CommandMeta) -> String {
    if meta.scope != Scope::Any {
        "UI only: agents cannot call it; ask the user to do this in the Cutweave app".into()
    } else if catalog.lists(meta) {
        format!("tool {}", meta.mcp_tool_name())
    } else if meta.annotations.read_only {
        format!("{QUERY} {{\"name\":\"{}\",\"args\":{{...}}}}", meta.name)
    } else {
        format!(
            "{EXECUTE} {{\"steps\":[{{\"cmd\":\"{}\",\"args\":{{...}}}}]}}",
            meta.name
        )
    }
}

/// A search hit / the head of a description.
#[must_use]
pub fn entry_summary(catalog: &Catalog, meta: &CommandMeta) -> Value {
    let mut v = json!({
        "name": meta.name,
        "summary": meta.summary,
        "kind": meta.kind,
        "call": how_to_call(catalog, meta),
    });
    if meta.annotations.read_only {
        v["read_only"] = json!(true);
    }
    if meta.kind == EntryKind::Engine && !meta.annotations.read_only {
        v["transactional"] = json!(false);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lean_schema_drops_only_documentation() {
        let time = json!({
            "anyOf": [
                { "type": "integer", "maximum": 9, "minimum": -9 },
                { "type": "string", "pattern": "^[0-9]+s$" }
            ],
            "description": "A time."
        });
        let schema = json!({
            "title": "ClipTrim",
            "description": "Long doc comment.",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "title": { "type": "string", "description": "A property named title." },
                "to": {
                    "anyOf": [time, { "type": "null" }],
                    "default": null,
                    "description": "Where the edge goes."
                },
                "edge": { "oneOf": [{ "const": "start", "type": "string" }] },
                "flag": { "type": ["boolean", "null"], "default": false }
            },
            "required": ["edge"]
        });
        assert_eq!(
            lean_schema(&schema),
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "title": { "type": "string", "description": "A property named title." },
                    "to": {
                        "anyOf": [
                            { "type": "integer", "maximum": 9, "minimum": -9 },
                            { "type": "string", "pattern": "^[0-9]+s$" },
                            { "type": "null" }
                        ],
                        "description": "Where the edge goes."
                    },
                    "edge": { "oneOf": [{ "const": "start", "type": "string" }] },
                    "flag": { "type": ["boolean", "null"], "default": false }
                },
                "required": ["edge"]
            })
        );
    }

    #[test]
    fn an_undescribed_optional_field_keeps_the_inner_description() {
        let schema = json!({
            "type": "object",
            "properties": {
                "overlap": {
                    "anyOf": [
                        { "oneOf": [{ "const": "reject" }], "description": "What to do." },
                        { "type": "null" }
                    ]
                }
            }
        });
        assert_eq!(
            lean_schema(&schema)["properties"]["overlap"],
            json!({
                "anyOf": [{ "oneOf": [{ "const": "reject" }] }, { "type": "null" }],
                "description": "What to do."
            })
        );
    }

    #[test]
    fn profiles_and_budgets() {
        assert_eq!(Profile::parse("minimal"), Some(Profile::Minimal));
        assert_eq!(Profile::parse("nope"), None);
        assert_eq!(Profile::Standard.route(), "/");
        let size = ListSize {
            tools: 19,
            bytes: 20_000,
        };
        assert_eq!(size.tokens(), 5_000);
        assert!(P1_GATE.fits(size));
        assert!(!P1_GATE.fits(ListSize {
            tools: 19,
            bytes: 20_001
        }));
        assert!(!Profile::Minimal.budget().fits(size));
    }
}
