// SPDX-License-Identifier: GPL-3.0-or-later
//! The hybrid tool catalog (arch §14, ADR-0009), generated from the command registry.
//!
//! - **Core tools:** every registry entry with `mcp = "core"` (never UI-only) is a tool named
//!   after the command with `.` replaced by `_` (`clip_split`), with the registry's MCP
//!   annotations. Its description comes from the core tool table
//!   ([`core`](crate::mcp::core), `mcp_core.toml`, WP-2I.01): a sentence or two for an agent
//!   and one example call; an entry the table does not know is described by its `summary`.
//!   Its `inputSchema` is the registry params schema in [lean](lean_schema) form:
//!   validation-identical, without the documentation (parameter docs are one
//!   `cutweave_describe_command` away; the listed description and its example stand in for
//!   them), and with a subschema used by several params (the time input) defined once under
//!   `$defs`. `view_get` and `view_set` are the two `view` commands listed (arch §14).
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
//! Described ([`Profile::Described`], `/mcp/described`; ADR-0040, WP-2I.07): the standard
//! tools with their parameter docs, [`DESCRIBED_BUDGET`], <= 36 tools / <= 16k tokens.
//! Standard: [`STANDARD_BUDGET`], <= 36 tools / <= 8k tokens, the gate of the `/mcp` list since
//! M1 (R36; the Phase 1 gate of <= 20 tools / <= 5k tokens, R33, ended with M1). Full: standard
//! plus the search-based long tail, so it lists exactly the standard tools for now. `outputSchema`s are not listed (Spike S5: about 45% of the bytes); results
//! still carry `structuredContent`.

use std::{collections::HashMap, sync::Arc};

use cutweave_commands::{CommandMeta, Entry, EntryKind, McpExposure, Registry, Scope};
use rmcp::model::{JsonObject, MetaObject, Tool, ToolAnnotations};
use serde_json::{Value, json};

use crate::mcp::core::{CoreTable, CoreTool, META_ALWAYS_LOAD};

/// `cutweave_search_commands{query, limit?}`.
pub const SEARCH: &str = "cutweave_search_commands";
/// `cutweave_describe_command{name}`.
pub const DESCRIBE: &str = "cutweave_describe_command";
/// `cutweave_execute{steps[{cmd, args}], dry_run?, expected_rev?, checkpoint?}`.
pub const EXECUTE: &str = "cutweave_execute";
/// `cutweave_query{name, args}`.
pub const QUERY: &str = "cutweave_query";

/// The generic tools, in listing order.
pub const GENERIC_TOOLS: [&str; 4] = [SEARCH, DESCRIBE, EXECUTE, QUERY];

/// Registry entries the minimal profile lists as tools: look (`project.info`, `media.list`,
/// `timeline.get`, `render.frame`), wait (`job.wait`) and recover (`edit.undo`). Everything
/// else goes through the generic tools. Entries that are not registered (or not core) yet are
/// skipped: `render.frame` exists only with the engine's renderer (the full engine).
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

/// The arch §14 budget of the minimal profile: <= 12 tools and <= 3k tokens.
pub const MINIMAL_BUDGET: Budget = Budget {
    max_tools: 12,
    max_tokens: 3_000,
};

/// The arch §14 budget of the standard (and full) profile, the gate of the `/mcp` tool list
/// (R36, since M1; it replaced the Phase 1 gate of <= 20 tools / <= 5k tokens, R33): <= 36 tools
/// and <= 8k tokens.
pub const STANDARD_BUDGET: Budget = Budget {
    max_tools: 36,
    max_tokens: 8_000,
};

/// The budget of the described profile (ADR-0040, question 2; WP-2I.07): the standard
/// profile's tools, in at most 16k tokens. It is stated in `mcp_core.toml`
/// (`[described_budget]`) and a test holds the two equal.
pub const DESCRIBED_BUDGET: Budget = Budget {
    max_tools: 36,
    max_tokens: 16_000,
};

/// A tool profile, chosen by the endpoint (`/mcp`, `/mcp/minimal`, `/mcp/full`,
/// `/mcp/described`) or, on stdio, by `--profile` (WP-1J.03).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Small-context clients: the [`MINIMAL_CORE`] tools plus the 4 generic tools.
    Minimal,
    /// The default (`/mcp`): every core tool plus the 4 generic tools.
    Standard,
    /// Standard plus the search-based long tail, with no extra listed tools (Phase 1: the same
    /// list as standard).
    Full,
    /// For clients that defer tool definitions and load one when a model asks for it (Claude
    /// Code's tool search, Copilot CLI): the standard profile's tools, in the same order, with
    /// the same names, descriptions, annotations and hints, and with **full parameter
    /// descriptions**: each input schema is the registry's in [described](described_schema)
    /// form, not its [lean](lean_schema) form. For such a client the size of the list is not
    /// what a session costs, and the parameter docs save a `cutweave_describe_command` call
    /// per tool (ADR-0040, question 2; WP-2I.07). A schema here accepts exactly the values the
    /// standard profile's does.
    Described,
}

impl Profile {
    /// Every profile.
    pub const ALL: [Profile; 4] = [
        Profile::Minimal,
        Profile::Standard,
        Profile::Full,
        Profile::Described,
    ];

    /// `minimal`, `standard`, `full` or `described`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Profile::Minimal => "minimal",
            Profile::Standard => "standard",
            Profile::Full => "full",
            Profile::Described => "described",
        }
    }

    /// Parses [`name`](Self::name).
    #[must_use]
    pub fn parse(name: &str) -> Option<Profile> {
        Profile::ALL.into_iter().find(|p| p.name() == name)
    }

    /// The route under the `/mcp` mount: `/` (`/mcp`), `/minimal`, `/full`, `/described`.
    #[must_use]
    pub const fn route(self) -> &'static str {
        match self {
            Profile::Minimal => "/minimal",
            Profile::Standard => "/",
            Profile::Full => "/full",
            Profile::Described => "/described",
        }
    }

    /// The arch §14 budget.
    #[must_use]
    pub const fn budget(self) -> Budget {
        match self {
            Profile::Minimal => MINIMAL_BUDGET,
            Profile::Standard | Profile::Full => STANDARD_BUDGET,
            Profile::Described => DESCRIBED_BUDGET,
        }
    }

    /// Whether listed input schemas carry their parameter documentation
    /// ([`described_schema`]) instead of the [lean](lean_schema) form.
    #[must_use]
    pub const fn describes_parameters(self) -> bool {
        matches!(self, Profile::Described)
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

/// Whether `meta` is listed as a tool of its own in the standard profile: `mcp = "core"` and
/// any caller (never UI-only).
#[must_use]
pub fn is_core_tool(meta: &CommandMeta) -> bool {
    meta.mcp == McpExposure::Core && meta.scope == Scope::Any
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
    instructions: String,
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
            let tool = entry_tool(&entry.meta, profile);
            targets.insert(tool.name.to_string(), Target::Entry(entry.meta.name));
            tools.push(tool);
        }
        for (tool, target) in generic_tools(profile.describes_parameters()) {
            let tool = with_hints(tool);
            targets.insert(tool.name.to_string(), target);
            tools.push(tool);
        }
        let instructions = crate::mcp::instructions::instructions(&registry, |command| {
            registry
                .get(command)
                .is_some_and(|e| listed(profile, &e.meta))
        });
        Catalog {
            profile,
            registry,
            tools,
            targets,
            instructions,
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

    /// The server instructions of this endpoint
    /// ([`instructions`](crate::mcp::instructions)): the same for every connection.
    #[must_use]
    pub fn instructions(&self) -> &str {
        &self.instructions
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

/// What a core tool's listing says: the core table's description and example
/// (`mcp_core.toml`), or the registry summary for an entry the table does not know (a plugin's
/// core command).
fn tool_description(meta: &CommandMeta) -> String {
    CoreTable::get()
        .by_command(meta.name)
        .map_or_else(|| meta.summary.to_owned(), CoreTool::listing)
}

/// Adds what the core table says beyond the listing's text: the
/// [always-load hint](META_ALWAYS_LOAD) (WP-2I.07) on a tool marked `always_load`. A tool
/// without a mark gets no `_meta` at all.
fn with_hints(tool: Tool) -> Tool {
    if CoreTable::get().always_load(&tool.name) {
        let mut meta = MetaObject::new();
        meta.insert(META_ALWAYS_LOAD.to_owned(), json!(true));
        tool.with_meta(meta)
    } else {
        tool
    }
}

fn entry_tool(meta: &CommandMeta, profile: Profile) -> Tool {
    let schema = if profile.describes_parameters() {
        described_schema(&meta.input_schema)
    } else {
        lean_schema(&meta.input_schema)
    };
    with_hints(
        Tool::new(meta.mcp_tool_name(), tool_description(meta), object(schema))
            .with_annotations(annotations(meta)),
    )
}

/// The listed form of a registry params schema: the same validation, without the keywords that
/// only document. Removes every `description` and `title` (WP-2I.01: the core set of arch §14
/// fits the standard budget only without parameter docs; the tool description carries an
/// example, and `cutweave_describe_command` the full docs), drops `"default": null`, and
/// flattens `anyOf[{anyOf[A, B]}, {type: null}]` into `anyOf[A, B, {type: null}]`. Alternatives
/// that several params repeat (the time input: flicks or text) are defined once under `$defs`
/// and referenced (`{"$ref": "#/$defs/time"}`, next to `{"type": "null"}` for an optional one).
/// Enums, patterns, bounds, `required` and `additionalProperties` are kept as they are, so a
/// value is valid under the lean schema exactly when it is valid under the registry's.
#[must_use]
pub fn lean_schema(schema: &Value) -> Value {
    let mut out = schema.clone();
    if let Value::Object(root) = &mut out {
        root.remove("description");
    }
    trim(&mut out, Docs::Drop);
    share_repeated(&mut out);
    share_subschemas(&mut out);
    out
}

/// The form of a registry params schema the described profile lists
/// ([`Profile::Described`]): the same validation, with **every parameter description kept**,
/// at any depth. What goes is what an agent does not read as parameter documentation: the
/// root `description` (the command's long doc; the listed description stands for it and
/// `cutweave_describe_command` returns it), `title`, `x-ui`, width-only `format`s and
/// `"default": null`. A subschema that several parameters repeat with the same description (the
/// time input, an id) is defined once under `$defs`, description included, and referenced; a
/// reference stands alone, so a description is never lost to it.
#[must_use]
pub fn described_schema(schema: &Value) -> Value {
    let mut out = schema.clone();
    if let Value::Object(root) = &mut out {
        root.remove("description");
    }
    trim(&mut out, Docs::Keep);
    share_subschemas(&mut out);
    out
}

/// Whether [`trim`] keeps the `description` keywords.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Docs {
    Keep,
    Drop,
}

/// Defines each subschema that occurs several times once under `$defs` and references it: an ID pattern, a 2D vector, an `interp`. Greedy, the biggest net
/// saving first, until nothing saves bytes.
fn share_subschemas(root: &mut Value) {
    let mut taken: Vec<String> = root
        .get("$defs")
        .and_then(Value::as_object)
        .map_or_else(Vec::new, |d| d.keys().cloned().collect());
    loop {
        let mut counts: Vec<(Value, usize)> = Vec::new();
        collect_subschemas(root, true, &mut counts);
        let best = counts
            .into_iter()
            .filter(|(_, n)| *n >= 2)
            .map(|(schema, n)| {
                let size = schema.to_string().len();
                // Each use shrinks to `{"$ref":"#/$defs/NAME"}` (~24 bytes with a short name); the
                // definition costs its size plus its key.
                let saving = (n * size).saturating_sub(n * 24 + size + 8);
                (saving, schema)
            })
            .filter(|(saving, _)| *saving > 0)
            .max_by_key(|(saving, _)| *saving);
        let Some((_, schema)) = best else {
            return;
        };
        let name = subschema_name(&schema, &taken);
        taken.push(name.clone());
        replace_subschema(root, true, &schema, &name);
        if let Value::Object(map) = root {
            let defs = map
                .entry("$defs")
                .or_insert_with(|| Value::Object(JsonObject::new()));
            if let Value::Object(defs) = defs {
                defs.insert(name, schema);
            }
        }
    }
}

fn subschema_name(schema: &Value, taken: &[String]) -> String {
    let time = schema
        .get("anyOf")
        .is_some_and(is_time_input)
        .then(|| "time".to_owned());
    let base = schema["pattern"]
        .as_str()
        .and_then(|p| p.strip_prefix('^'))
        .and_then(|p| p.split_once("_[0-9a-f]{32}$"))
        .map(|(prefix, _)| prefix.to_owned())
        .or_else(|| {
            let props = schema["properties"].as_object()?;
            (props.len() == 2 && props.contains_key("x") && props.contains_key("y"))
                .then(|| "vec2".to_owned())
        })
        .or(time)
        .unwrap_or_else(|| "d".to_owned());
    let mut name = base.clone();
    let mut n = 1;
    while taken.contains(&name) {
        n += 1;
        name = format!("{base}{n}");
    }
    name
}

fn is_candidate(map: &JsonObject) -> bool {
    !map.contains_key("$ref") && !map.contains_key("$defs") && !map.is_empty()
}

/// The subschema positions: property values, `items`, `additionalProperties`, alternatives.
fn for_each_child(map: &JsonObject, mut f: impl FnMut(&Value)) {
    for (key, value) in map {
        let key = key.as_str();
        if key == "properties" {
            if let Value::Object(children) = value {
                children.values().for_each(&mut f);
            }
        } else if SCHEMA_VALUES.contains(&key) || SCHEMA_LISTS.contains(&key) {
            match value {
                Value::Array(list) => list.iter().for_each(&mut f),
                other => f(other),
            }
        }
    }
}

fn collect_subschemas(schema: &Value, is_root: bool, counts: &mut Vec<(Value, usize)>) {
    let Value::Object(map) = schema else {
        return;
    };
    if !is_root && is_candidate(map) {
        let canonical = Value::Object(map.clone());
        match counts.iter_mut().find(|(c, _)| *c == canonical) {
            Some((_, n)) => *n += 1,
            None => counts.push((canonical, 1)),
        }
    }
    for_each_child(map, |child| collect_subschemas(child, false, counts));
}

fn replace_subschema(schema: &mut Value, is_root: bool, target: &Value, name: &str) {
    let Value::Object(map) = schema else {
        return;
    };
    if !is_root && is_candidate(map) && Value::Object(map.clone()) == *target {
        let mut reference = JsonObject::new();
        reference.insert("$ref".into(), json!(format!("#/$defs/{name}")));
        *map = reference;
        return;
    }
    for (key, value) in map.iter_mut() {
        let key = key.as_str();
        if key == "properties" {
            if let Value::Object(children) = value {
                children
                    .values_mut()
                    .for_each(|c| replace_subschema(c, false, target, name));
            }
        } else if SCHEMA_VALUES.contains(&key) || SCHEMA_LISTS.contains(&key) {
            match value {
                Value::Array(list) => list
                    .iter_mut()
                    .for_each(|c| replace_subschema(c, false, target, name)),
                other => replace_subschema(other, false, target, name),
            }
        }
    }
}

/// The shortest alternatives list worth sharing, in bytes of compact JSON (a `$ref` costs about
/// 30).
const MIN_SHARED_BYTES: usize = 80;

/// Moves `anyOf` alternative lists that occur more than once into the root `$defs` (see
/// [`lean_schema`]). The time input is named `time`; any other shared list `shared<n>`.
fn share_repeated(root: &mut Value) {
    let mut counts: Vec<(Value, usize)> = Vec::new();
    collect_alternatives(root, &mut counts);
    let mut names: Vec<(Value, String)> = Vec::new();
    for (alternatives, count) in counts {
        if count < 2 || alternatives.to_string().len() < MIN_SHARED_BYTES {
            continue;
        }
        let name = if is_time_input(&alternatives) && !names.iter().any(|(_, n)| n == "time") {
            "time".to_owned()
        } else {
            format!("shared{}", names.len() + 1)
        };
        names.push((alternatives, name));
    }
    if names.is_empty() {
        return;
    }
    refer(root, &names);
    if let Value::Object(map) = root {
        let defs = map
            .entry("$defs")
            .or_insert_with(|| Value::Object(JsonObject::new()));
        if let Value::Object(defs) = defs {
            for (alternatives, name) in names {
                defs.insert(name, json!({ "anyOf": alternatives }));
            }
        }
    }
}

/// The alternatives of an `anyOf` other than `{"type": "null"}`, when there are at least two.
fn non_null_alternatives(map: &JsonObject) -> Option<Value> {
    let Some(Value::Array(list)) = map.get("anyOf") else {
        return None;
    };
    let rest: Vec<Value> = list
        .iter()
        .filter(|a| **a != json!({ "type": "null" }))
        .cloned()
        .collect();
    (rest.len() >= 2).then_some(Value::Array(rest))
}

fn collect_alternatives(schema: &Value, counts: &mut Vec<(Value, usize)>) {
    match schema {
        Value::Object(map) => {
            if let Some(alternatives) = non_null_alternatives(map) {
                match counts.iter_mut().find(|(a, _)| *a == alternatives) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((alternatives, 1)),
                }
            }
            for (key, value) in map {
                if key != "$defs" {
                    collect_alternatives(value, counts);
                }
            }
        }
        Value::Array(list) => list.iter().for_each(|v| collect_alternatives(v, counts)),
        _ => {}
    }
}

/// Replaces each shared alternatives list by a `$ref` to its definition.
fn refer(schema: &mut Value, names: &[(Value, String)]) {
    match schema {
        Value::Object(map) => {
            if let Some(alternatives) = non_null_alternatives(map)
                && let Some((_, name)) = names.iter().find(|(a, _)| *a == alternatives)
            {
                let reference = json!({ "$ref": format!("#/$defs/{name}") });
                let nullable = map
                    .get("anyOf")
                    .and_then(Value::as_array)
                    .is_some_and(|l| l.contains(&json!({ "type": "null" })));
                if nullable {
                    map.insert("anyOf".into(), json!([reference, { "type": "null" }]));
                } else {
                    map.remove("anyOf");
                    map.insert("$ref".into(), reference["$ref"].clone());
                }
                return;
            }
            for (key, value) in map.iter_mut() {
                if key != "$defs" {
                    refer(value, names);
                }
            }
        }
        Value::Array(list) => list.iter_mut().for_each(|v| refer(v, names)),
        _ => {}
    }
}

/// Whether `alternatives` is the time input: an integer (flicks) or a string with a pattern
/// that accepts frames (`"f:N"`).
fn is_time_input(alternatives: &Value) -> bool {
    let Some(list) = alternatives.as_array() else {
        return false;
    };
    list.iter().any(|a| a["type"] == "integer")
        && list.iter().any(|a| {
            a["type"] == "string" && a["pattern"].as_str().is_some_and(|p| p.contains("f:"))
        })
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

fn trim(schema: &mut Value, docs: Docs) {
    let Value::Object(map) = schema else {
        return;
    };
    map.remove("title");
    // `x-ui` (render hints for the schema-driven inspector, WP-2H.02) is for people, not agents:
    // it never costs tool-list tokens.
    map.remove("x-ui");
    // `format: "uint32"` and friends only name the Rust width; the `minimum`/`maximum` next to
    // them are what validates.
    if map.get("format").and_then(Value::as_str).is_some_and(|f| {
        ["uint", "int", "float", "double"]
            .iter()
            .any(|p| f.starts_with(p))
    }) {
        map.remove("format");
    }
    if map.get("default") == Some(&Value::Null) {
        map.remove("default");
    }
    if docs == Docs::Drop {
        flatten_nullable(map);
        // The keyword of this schema; a property that is named `description` is a key of a
        // `properties` map, which is not a schema.
        map.remove("description");
    }
    for (key, value) in map.iter_mut() {
        let key = key.as_str();
        if SCHEMA_MAPS.contains(&key) {
            if let Value::Object(children) = value {
                children.values_mut().for_each(|c| trim(c, docs));
            }
        } else if SCHEMA_VALUES.contains(&key) {
            trim(value, docs);
        } else if SCHEMA_LISTS.contains(&key)
            && let Value::Array(list) = value
        {
            list.iter_mut().for_each(|c| trim(c, docs));
        }
    }
    flatten_const_enum(map);
}

/// `oneOf[{const: A, type: string}, {const: B, type: string}, ...]` (a Rust enum of unit
/// variants) is `{type: string, enum: [A, B, ...]}`; `anyOf[that, {type: null}]` (optional) is
/// `{type: [string, null], enum: [A, B, ..., null]}`.
fn flatten_const_enum(map: &mut JsonObject) {
    fn consts(list: &[Value]) -> Option<Vec<Value>> {
        if list.len() < 2 {
            return None;
        }
        list.iter()
            .map(|v| {
                let o = v.as_object()?;
                (o.len() == 2 && o.get("type") == Some(&json!("string")))
                    .then(|| o.get("const").cloned())
                    .flatten()
            })
            .collect()
    }
    if !map.contains_key("type")
        && !map.contains_key("enum")
        && let Some(Value::Array(list)) = map.get("oneOf")
        && let Some(values) = consts(list)
    {
        map.remove("oneOf");
        map.insert("type".into(), json!("string"));
        map.insert("enum".into(), Value::Array(values));
        return;
    }
    if !map.contains_key("type")
        && !map.contains_key("enum")
        && let Some(Value::Array(list)) = map.get("anyOf")
        && list.len() == 2
        && list[1] == json!({ "type": "null" })
        && let Some(inner) = list[0].as_object()
        && inner.len() == 2
        && inner.get("type") == Some(&json!("string"))
        && let Some(Value::Array(values)) = inner.get("enum")
    {
        let mut values = values.clone();
        values.push(Value::Null);
        map.remove("anyOf");
        map.insert("type".into(), json!(["string", "null"]));
        map.insert("enum".into(), Value::Array(values));
    }
}

/// `anyOf[A, {type: null}]` (an optional field): a bare `A = {anyOf: [...]}` (once its
/// description is gone) is spliced in.
fn flatten_nullable(map: &mut JsonObject) {
    let Some(Value::Array(any_of)) = map.get_mut("anyOf") else {
        return;
    };
    if any_of.len() != 2 || any_of[1] != json!({ "type": "null" }) {
        return;
    }
    if let Value::Object(inner) = &mut any_of[0] {
        inner.remove("description");
    }
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

/// Parameter descriptions the generic tools carry on the described profile only, by tool and
/// JSON Pointer into its input schema: the parameters whose standard listing has none (their
/// names say enough where every token counts).
const GENERIC_PARAMETER_DOCS: &[(&str, &str, &str)] = &[
    (
        EXECUTE,
        "/properties/steps",
        "The commands to run, in order, as one transaction",
    ),
    (
        QUERY,
        "/properties/name",
        "A query's name, e.g. item.get; cutweave_search_commands finds them",
    ),
    (
        QUERY,
        "/properties/args",
        "The query's params; cutweave_describe_command documents them",
    ),
];

/// The generic tools; with `described`, every parameter has a description
/// ([`Profile::Described`]).
fn generic_tools(described: bool) -> Vec<(Tool, Target)> {
    let mut tools = standard_generic_tools();
    if described {
        for (tool, _) in &mut tools {
            let mut schema = Value::Object((*tool.input_schema).clone());
            for (_, pointer, doc) in GENERIC_PARAMETER_DOCS
                .iter()
                .filter(|(name, ..)| *name == tool.name)
            {
                if let Some(Value::Object(param)) = schema.pointer_mut(pointer) {
                    param.insert("description".to_owned(), json!(doc));
                }
            }
            tool.input_schema = Arc::new(object(schema));
        }
    }
    tools
}

fn standard_generic_tools() -> Vec<(Tool, Target)> {
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
                        "expected_rev": { "type": "integer", "minimum": 0, "description": "Fail unless the document is at this revision" },
                        "checkpoint": { "type": "boolean", "description": "Save a checkpoint first; edit.restore brings it back" }
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
                "title": { "type": "string", "description": "A property that is named title, not a keyword." },
                "description": { "type": "string", "description": "A property, not a keyword." },
                "to": {
                    "anyOf": [time, { "type": "null" }],
                    "default": null,
                    "description": "Where the edge goes after the trim, in flicks."
                },
                "edge": { "oneOf": [{ "const": "start", "type": "string" }] },
                "flag": {
                    "type": ["boolean", "null"],
                    "default": false,
                    "x-ui": { "widget": "toggle" }
                }
            },
            "required": ["edge"]
        });
        assert_eq!(
            lean_schema(&schema),
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "to": {
                        "anyOf": [
                            { "type": "integer", "maximum": 9, "minimum": -9 },
                            { "type": "string", "pattern": "^[0-9]+s$" },
                            { "type": "null" }
                        ]
                    },
                    "edge": { "oneOf": [{ "const": "start", "type": "string" }] },
                    "flag": { "type": ["boolean", "null"], "default": false }
                },
                "required": ["edge"]
            })
        );
    }

    #[test]
    fn repeated_time_inputs_are_defined_once_and_element_descriptions_go() {
        let time = json!({
            "anyOf": [
                { "type": "integer", "maximum": 9_007_199_254_740_991_i64, "minimum": -9_007_199_254_740_991_i64 },
                { "type": "string", "pattern": "^-?[0-9]+(\\.[0-9]+)?s$|^f:[0-9]+$" }
            ],
            "description": "A time."
        });
        let schema = json!({
            "type": "object",
            "properties": {
                "at": { "anyOf": time["anyOf"].clone(), "description": "When the clip starts on the timeline, in flicks." },
                "delta": { "anyOf": [time.clone(), { "type": "null" }], "description": "How much to move the clip by, in flicks, either direction." },
                "items": {
                    "type": "array",
                    "items": { "type": "string", "pattern": "^itm_", "description": "An id." }
                }
            }
        });
        let lean = lean_schema(&schema);
        assert_eq!(lean["$defs"]["time"]["anyOf"], time["anyOf"]);
        assert_eq!(lean["properties"]["at"], json!({ "$ref": "#/$defs/time" }));
        assert_eq!(
            lean["properties"]["delta"],
            json!({ "anyOf": [{ "$ref": "#/$defs/time" }, { "type": "null" }] })
        );
        assert_eq!(
            lean["properties"]["items"]["items"],
            json!({ "type": "string", "pattern": "^itm_" })
        );
        // Used once: inlined as before.
        let once = json!({ "type": "object", "properties": { "at": schema["properties"]["at"] } });
        assert!(lean_schema(&once).get("$defs").is_none());
    }

    #[test]
    fn no_description_is_listed_at_any_depth() {
        fn has_description_keyword(schema: &Value) -> bool {
            let Value::Object(map) = schema else {
                return false;
            };
            map.contains_key("description")
                || map.iter().any(|(key, value)| match value {
                    Value::Object(children) if SCHEMA_MAPS.contains(&key.as_str()) => {
                        children.values().any(has_description_keyword)
                    }
                    Value::Array(list) => list.iter().any(has_description_keyword),
                    other => has_description_keyword(other),
                })
        }
        let schema = json!({
            "type": "object",
            "description": "Long doc comment.",
            "properties": {
                "overlap": {
                    "anyOf": [
                        { "oneOf": [{ "const": "reject" }], "description": "What to do when the new clip overlaps another." },
                        { "type": "null" }
                    ]
                },
                "keys": {
                    "type": "array",
                    "description": "The keys.",
                    "items": {
                        "type": "object",
                        "description": "One key.",
                        "properties": { "t": { "type": "integer", "description": "Item time 0..9." } }
                    }
                }
            }
        });
        assert!(has_description_keyword(&schema));
        let lean = lean_schema(&schema);
        assert!(!has_description_keyword(&lean), "{lean}");
        assert_eq!(
            lean["properties"]["overlap"],
            json!({ "anyOf": [{ "oneOf": [{ "const": "reject" }] }, { "type": "null" }] })
        );
    }

    #[test]
    fn described_schema_keeps_parameter_docs_and_defines_repeats_once() {
        let time = json!({
            "anyOf": [
                { "type": "integer" },
                { "type": "string", "pattern": "^-?[0-9]+$|^f:[0-9]+$" }
            ],
            "description": "A time: integer flicks, or a string such as a frame \"f:60\"."
        });
        let schema = json!({
            "type": "object",
            "title": "Params",
            "description": "The command's long doc.",
            "properties": {
                "at": time,
                "to": time,
                "end": { "anyOf": [time, { "type": "null" }], "description": "Where it ends.", "default": null },
                "count": { "type": "integer", "format": "uint32", "minimum": 1, "description": "How many." },
                "keys": {
                    "type": "array",
                    "description": "The keys.",
                    "items": {
                        "type": "object",
                        "properties": { "t": { "type": "integer", "description": "Item time." } }
                    }
                }
            },
            "required": ["at"]
        });
        let described = described_schema(&schema);
        assert!(described.get("description").is_none() && described.get("title").is_none());
        assert_eq!(
            described["$defs"]["time"], time,
            "defined once, with its description"
        );
        let reference = json!({ "$ref": "#/$defs/time" });
        assert_eq!(described["properties"]["at"], reference);
        assert_eq!(described["properties"]["to"], reference);
        assert_eq!(
            described["properties"]["end"],
            json!({ "anyOf": [reference, { "type": "null" }], "description": "Where it ends." })
        );
        assert_eq!(
            described["properties"]["count"],
            json!({ "type": "integer", "minimum": 1, "description": "How many." })
        );
        assert_eq!(
            described["properties"]["keys"]["items"]["properties"]["t"]["description"],
            "Item time."
        );
        assert_eq!(described["required"], json!(["at"]));
        // The lean form of the same parameter: the validation without the documentation.
        assert_eq!(
            lean_schema(&schema)["properties"]["count"],
            json!({ "type": "integer", "minimum": 1 })
        );
    }

    #[test]
    fn a_core_tool_is_described_by_the_table_and_others_by_their_summary() {
        let table = CoreTable::get();
        let split = table.by_command("clip.split").unwrap();
        assert!(split.listing().contains(" E.g. {"));
        assert!(table.by_command("clip.nope").is_none());
    }

    #[test]
    fn profiles_and_budgets() {
        assert_eq!(Profile::parse("minimal"), Some(Profile::Minimal));
        assert_eq!(Profile::parse("nope"), None);
        assert_eq!(Profile::Standard.route(), "/");
        let size = ListSize {
            tools: 20,
            bytes: 20_000,
        };
        assert_eq!(size.tokens(), 5_000);
        assert!(STANDARD_BUDGET.fits(size));
        assert!(!STANDARD_BUDGET.fits(ListSize {
            tools: 20,
            bytes: 32_001
        }));
        assert!(!STANDARD_BUDGET.fits(ListSize {
            tools: 37,
            bytes: 100
        }));
        assert!(!Profile::Minimal.budget().fits(size));
        assert_eq!(Profile::Full.budget(), STANDARD_BUDGET);
    }

    #[test]
    fn restated_names_go_and_repeated_subschemas_are_defined_once() {
        let vec2 = |what: &str| {
            json!({
                "type": ["object", "null"],
                "properties": {
                    "x": { "type": "number", "description": "Horizontal component." },
                    "y": { "type": "number", "description": "Vertical component." }
                },
                "required": ["x", "y"],
                "description": what
            })
        };
        let schema = json!({
            "type": "object",
            "properties": {
                "position": vec2("Where the anchor sits, in canvas units."),
                "scale": vec2("Size multiplier per axis, finite and not negative."),
                "anchor": vec2("The item point the position places, in item units."),
                "edge": { "oneOf": [
                    { "type": "string", "const": "start", "description": "The start." },
                    { "type": "string", "const": "end" }
                ] },
                "mode": { "anyOf": [
                    { "oneOf": [{ "type": "string", "const": "a" }, { "type": "string", "const": "b" }] },
                    { "type": "null" }
                ] }
            }
        });
        let lean = lean_schema(&schema);
        let props = &lean["properties"];
        assert_eq!(props["scale"], json!({ "$ref": "#/$defs/vec2" }));
        assert_eq!(
            lean["$defs"]["vec2"]["properties"]["x"],
            json!({ "type": "number" })
        );
        assert_eq!(
            props["edge"],
            json!({ "type": "string", "enum": ["start", "end"] })
        );
        assert_eq!(
            props["mode"],
            json!({ "type": ["string", "null"], "enum": ["a", "b", null] })
        );
    }
}
