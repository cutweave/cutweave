// SPDX-License-Identifier: GPL-3.0-or-later
//! Registry metadata: what each command and query declares about itself (arch §5.1, §14).

use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};
use serde_json::Value;

/// The command namespaces of arch §5.1. New namespaces require an ADR, so both the macros and
/// [`Registry`](crate::Registry) reject any other.
pub const NAMESPACES: &[&str] = &[
    "project",
    "media",
    "sequence",
    "track",
    "timeline",
    "item",
    "clip",
    "text",
    "caption",
    "transcript",
    "effect",
    "transition",
    "keyframe",
    "mask",
    "color",
    "audio",
    "composition",
    "template",
    "font",
    "asset",
    "analysis",
    "tracking",
    "ai",
    "render",
    "export",
    "plugin",
    "edit",
    "view",
];

/// Longest accepted `summary`. Summaries are MCP tool descriptions and count against the
/// tool-list budget (arch §14), so detail belongs in the doc comment (the schema description).
pub const MAX_SUMMARY_CHARS: usize = 100;

/// Why a name is not a valid command name.
///
/// The grammar is `^[a-z]+\.[a-z_]+$` (`namespace.verb_object`, arch §5.1) and the namespace
/// must be one of [`NAMESPACES`].
pub fn check_name(name: &str) -> Result<(), String> {
    let Some((namespace, verb)) = name.split_once('.') else {
        return Err("must be `namespace.verb_object`, e.g. `clip.split`".into());
    };
    let lower = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase());
    let verb_ok = !verb.is_empty() && verb.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    if !lower(namespace) || !verb_ok {
        return Err(
            "must match `^[a-z]+\\.[a-z_]+$` (lowercase `namespace.verb_object`, no digits)".into(),
        );
    }
    if !NAMESPACES.contains(&namespace) {
        return Err(format!(
            "unknown namespace `{namespace}`; new namespaces require an ADR (arch §5.1). Known: {}",
            NAMESPACES.join(", ")
        ));
    }
    Ok(())
}

/// Why a summary is not acceptable.
pub fn check_summary(summary: &str) -> Result<(), String> {
    let chars = summary.chars().count();
    if chars == 0 || chars > MAX_SUMMARY_CHARS {
        return Err(format!(
            "must be 1..={MAX_SUMMARY_CHARS} characters (it is the MCP tool description and counts \
             against the tool-list budget); put details in the doc comment"
        ));
    }
    if summary.contains(['\n', '\r']) {
        return Err("must be a single line".into());
    }
    Ok(())
}

/// What a registry entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// Mutates the document through a [`Tx`](crate::Tx) ([`Command`](crate::Command)).
    Command,
    /// Reads the document ([`Query`](crate::Query)); read-only by construction.
    Query,
    /// Runs against engine state rather than the document ([`EngineCommand`](crate::EngineCommand)):
    /// history and transaction control (`edit.*`, WP-1B.04) and UI view state (`view.*`).
    Engine,
}

/// Which credentials may invoke an entry (arch §6, ADR-0016).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    specta::Type,
)]
pub enum Scope {
    /// Any authenticated caller (UI, agents, plugins, the CLI, internal jobs).
    #[default]
    #[serde(rename = "any")]
    Any,
    /// Only the UI credential (`ai.set_key`, approvals, allowlists, budgets, plugin grants).
    /// Enforced by the registry, so batches cannot bypass it.
    #[serde(rename = "ui")]
    UiOnly,
}

/// How an entry is exposed over MCP (arch §14, ADR-0009).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpExposure {
    /// Always listed as its own tool.
    Core,
    /// Reachable through `cutweave_search_commands` / `describe` / `execute` / `query`.
    Catalog,
    /// Not reachable over MCP (internal plumbing, UI-only).
    Hidden,
}

/// MCP tool annotations (hints). Serialized with MCP's names (`readOnlyHint`, …).
///
/// `read_only` is true for every query and for engine commands that only read engine state
/// (`edit.history`, `edit.list_checkpoints`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[allow(clippy::struct_excessive_bools)] // independent MCP hints
pub struct Annotations {
    /// `readOnlyHint`: true for queries and read-only engine commands.
    pub read_only: bool,
    /// `destructiveHint`: may irreversibly destroy data the user cares about.
    pub destructive: bool,
    /// `idempotentHint`: repeating it with the same arguments has no further effect.
    pub idempotent: bool,
    /// `openWorldHint`: talks to the outside world (network, paid providers).
    pub open_world: bool,
}

impl Serialize for Annotations {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Annotations", 4)?;
        st.serialize_field("readOnlyHint", &self.read_only)?;
        st.serialize_field("destructiveHint", &self.destructive)?;
        st.serialize_field("idempotentHint", &self.idempotent)?;
        st.serialize_field("openWorldHint", &self.open_world)?;
        st.end()
    }
}

/// Static metadata of an entry, declared once on its params type (by `#[command]` /
/// `#[query]`, or by hand via [`Described`](crate::Described)) and checked when it is
/// registered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent flags of the contract
pub struct CommandSpec {
    /// Wire name, `namespace.verb_object`.
    pub name: &'static str,
    /// One-line summary (the MCP tool description).
    pub summary: &'static str,
    /// Command, query or engine command.
    pub kind: EntryKind,
    /// Whether applying it creates an undo entry.
    pub undoable: bool,
    /// A `view.*` command: UI view state, never undoable, never journaled, not part of
    /// document transactions (arch §5.1).
    pub view: bool,
    /// Which credentials may invoke it.
    pub scope: Scope,
    /// MCP exposure.
    pub mcp: McpExposure,
    /// MCP annotations.
    pub annotations: Annotations,
    /// Top-level params marked `#[secret]`: redacted before journaling, logging or
    /// deduplication (arch §14), and `writeOnly` in the input schema.
    pub secret_fields: &'static [&'static str],
    /// Rust type of the params (diagnostics only).
    pub rust_type: &'static str,
}

/// Everything the registry knows about an entry: its [`CommandSpec`] plus the generated JSON
/// Schemas (draft 2020-12, self-contained, no `$ref`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[allow(clippy::struct_excessive_bools)] // independent flags of the contract
pub struct CommandMeta {
    /// Wire name, `namespace.verb_object`.
    pub name: &'static str,
    /// One-line summary (the MCP tool description).
    pub summary: &'static str,
    /// Command, query or engine command.
    pub kind: EntryKind,
    /// Whether applying it creates an undo entry.
    pub undoable: bool,
    /// A `view.*` command (never undoable or journaled).
    pub view: bool,
    /// Which credentials may invoke it.
    pub scope: Scope,
    /// MCP exposure.
    pub mcp: McpExposure,
    /// MCP annotations.
    pub annotations: Annotations,
    /// Top-level params marked `#[secret]`.
    pub secret_fields: &'static [&'static str],
    /// Rust type of the params.
    #[serde(skip)]
    pub rust_type: &'static str,
    /// JSON Schema of the params (what the engine accepts; MCP `inputSchema`): an object with
    /// `additionalProperties: false`; doc comments become `description`s.
    pub input_schema: Value,
    /// JSON Schema of the result (MCP `outputSchema`): an object.
    pub output_schema: Value,
}

impl CommandMeta {
    pub(crate) fn new(spec: CommandSpec, input_schema: Value, output_schema: Value) -> CommandMeta {
        CommandMeta {
            name: spec.name,
            summary: spec.summary,
            kind: spec.kind,
            undoable: spec.undoable,
            view: spec.view,
            scope: spec.scope,
            mcp: spec.mcp,
            annotations: spec.annotations,
            secret_fields: spec.secret_fields,
            rust_type: spec.rust_type,
            input_schema,
            output_schema,
        }
    }

    /// The namespace (`clip` of `clip.split`).
    #[must_use]
    pub fn namespace(&self) -> &'static str {
        self.name.split_once('.').map_or(self.name, |(ns, _)| ns)
    }

    /// The MCP tool name (`clip.split` → `clip_split`, arch §14).
    #[must_use]
    pub fn mcp_tool_name(&self) -> String {
        self.name.replace('.', "_")
    }

    /// Whether a successful call is journaled (WP-1C.02): document commands and engine
    /// commands, never queries, read-only engine commands (`edit.history`) or `view.*`
    /// commands.
    #[must_use]
    pub fn is_journaled(&self) -> bool {
        self.kind != EntryKind::Query && !self.view && !self.annotations.read_only
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_grammar() {
        for ok in [
            "clip.split",
            "edit.begin_tx",
            "view.set_playhead",
            "ai.generate",
        ] {
            assert_eq!(check_name(ok), Ok(()), "{ok}");
        }
        for bad in [
            "clip",
            "Clip.split",
            "clip.Split",
            "clip.split2",
            "clip.split-at",
            "clip.",
            ".split",
            "clip.split.more",
            "clip_x.split",
            "",
        ] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        let err = check_name("widget.spin").unwrap_err();
        assert!(err.contains("unknown namespace `widget`"), "{err}");
    }

    #[test]
    fn summary_rules() {
        assert!(check_summary("Split a clip").is_ok());
        assert!(check_summary("").is_err());
        assert!(check_summary(&"x".repeat(MAX_SUMMARY_CHARS)).is_ok());
        assert!(check_summary(&"x".repeat(MAX_SUMMARY_CHARS + 1)).is_err());
        assert!(check_summary("two\nlines").is_err());
    }

    #[test]
    fn annotations_use_mcp_names() {
        let a = Annotations {
            read_only: true,
            idempotent: true,
            ..Annotations::default()
        };
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::json!({
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            })
        );
        assert_eq!(serde_json::to_value(Scope::UiOnly).unwrap(), "ui");
        assert_eq!(
            serde_json::to_value(McpExposure::Catalog).unwrap(),
            "catalog"
        );
    }
}
