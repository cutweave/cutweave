// SPDX-License-Identifier: GPL-3.0-or-later
//! The core tool table (`mcp_core.toml`, WP-2I.01; arch §14): which tools the standard profile
//! lists next to the generic ones, and what each listing says.
//!
//! The registry stays the source of what a tool *does*: a core tool is a registered command or
//! query marked `mcp = "core"`, named after it with `.` replaced by `_`, with its input schema,
//! annotations, result and errors. The table adds what the registry's 100-character summary
//! cannot hold: a description written for an agent and one example call. It is also the
//! checked-in gate of the list: the tests fail when the table and the registry's core entries
//! differ, when an example is not valid, or when the list leaves the table's budget.

use std::{collections::BTreeMap, sync::LazyLock};

use serde::Deserialize;

/// The table, as committed.
pub const SOURCE: &str = include_str!("../../mcp_core.toml");

/// Longest listed description ([`CoreTool::listing`]), in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 300;

/// The tool `_meta` key that asks a client which defers tool definitions to load this tool at
/// the start (WP-2I.07). It is Claude Code's, read on 2026-11-27 at
/// <https://code.claude.com/docs/en/mcp> ("Mark a tool to load upfront or stay deferred"): "To
/// control how one of your server's tools loads, set `"anthropic/alwaysLoad"` in that tool's
/// `_meta` object"; with the JSON boolean `true` "the tool loads upfront". The person who
/// configures the server can still defer all of its tools. A client that does not know the key
/// ignores it: both MCP revisions allow a `_meta` key with a vendor prefix (only prefixes whose
/// second label is `modelcontextprotocol` or `mcp` are reserved).
pub const META_ALWAYS_LOAD: &str = "anthropic/alwaysLoad";

/// Most tools the table may mark `always_load`: each one costs every session its definition,
/// which is what deferring avoids.
pub const MAX_ALWAYS_LOAD: usize = 5;

/// What stands for the 32 hex digits of an id in an example.
pub const ID_PLACEHOLDER: &str = "…";

/// The tool name of a command: `.` becomes `_` (`clip.split` is `clip_split`; arch §14).
#[must_use]
pub fn tool_name(command: &str) -> String {
    command.replace('.', "_")
}

/// A tool-list budget, as the table states it for one profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableBudget {
    /// Most tools listed, the generic ones included.
    pub max_tools: usize,
    /// Most estimated tokens of the `tools/list` result.
    pub max_tokens: usize,
}

/// `[profiles]`: the budget of every profile's list. The figures are written here and nowhere
/// else (WP-2I.05); [`Profile::budget`](crate::mcp::Profile::budget) reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profiles {
    /// `/mcp/minimal` (arch §14).
    pub minimal: TableBudget,
    /// `/mcp`, and `/mcp/full`, which lists exactly the standard tools (arch §14, R36).
    pub standard: TableBudget,
    /// `/mcp/described` (ADR-0040, question 2).
    pub described: TableBudget,
}

/// One listed core tool.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreTool {
    /// The tool name (`clip_split`).
    pub name: String,
    /// The registered command or query it runs (`clip.split`).
    pub command: String,
    /// What an agent reads, without the example.
    pub description: String,
    /// One valid arguments object, as JSON text ([`ID_PLACEHOLDER`] for an id's hex digits).
    pub example: String,
    /// Whether the listing carries the [always-load hint](META_ALWAYS_LOAD).
    #[serde(default)]
    pub always_load: bool,
}

impl CoreTool {
    /// The description as listed: the text, then the example.
    #[must_use]
    pub fn listing(&self) -> String {
        format!("{} E.g. {}", self.description, self.example)
    }
}

/// A tool of the plan that is not listed yet.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingTool {
    /// The planned tool name.
    pub name: String,
    /// The work package or phase that brings it.
    pub arrives: String,
}

/// What the table says about one of the generic tools (their listings are in
/// [`catalog`](crate::mcp::catalog)).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenericTool {
    /// The tool name (`cutweave_execute`).
    pub name: String,
    /// Whether the listing carries the [always-load hint](META_ALWAYS_LOAD).
    #[serde(default)]
    pub always_load: bool,
}

/// One command namespace and what it is for, as the server instructions say it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Namespace {
    /// The namespace (`clip` of `clip.split`).
    pub name: String,
    /// One line, without a full stop: what its commands are for.
    pub purpose: String,
}

/// `mcp_core.toml`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreTable {
    /// The budget of every profile's list.
    pub profiles: Profiles,
    /// The listed core tools.
    #[serde(rename = "tool", default)]
    pub tools: Vec<CoreTool>,
    /// Planned tools that are not listed yet.
    #[serde(default)]
    pub pending: Vec<PendingTool>,
    /// What the table says about the generic tools.
    #[serde(default)]
    pub generic: Vec<GenericTool>,
    /// The command namespaces and their purposes, for the server instructions.
    #[serde(rename = "namespace", default)]
    pub namespaces: Vec<Namespace>,
    /// Words a user would say, by command or query name: `cutweave_search_commands` matches
    /// them next to names and summaries.
    #[serde(default)]
    pub keywords: BTreeMap<String, Vec<String>>,
}

impl CoreTable {
    /// Parses a table.
    pub fn parse(text: &str) -> Result<CoreTable, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// The committed table. Should the embedded file not parse (the tests rule it out), the
    /// table is empty and every tool is described by its registry summary.
    #[must_use]
    pub fn get() -> &'static CoreTable {
        static TABLE: LazyLock<CoreTable> = LazyLock::new(|| {
            CoreTable::parse(SOURCE).unwrap_or_else(|error| {
                tracing::error!(%error, "mcp_core.toml does not parse; using registry summaries");
                let none = TableBudget {
                    max_tools: 0,
                    max_tokens: 0,
                };
                CoreTable {
                    profiles: Profiles {
                        minimal: none,
                        standard: none,
                        described: none,
                    },
                    tools: Vec::new(),
                    pending: Vec::new(),
                    generic: Vec::new(),
                    namespaces: Vec::new(),
                    keywords: BTreeMap::new(),
                }
            })
        });
        &TABLE
    }

    /// The entry of the tool that runs `command`.
    #[must_use]
    pub fn by_command(&self, command: &str) -> Option<&CoreTool> {
        self.tools.iter().find(|t| t.command == command)
    }

    /// Whether the tool `name` (core or generic) is marked `always_load`.
    #[must_use]
    pub fn always_load(&self, name: &str) -> bool {
        self.always_loaded().contains(&name)
    }

    /// The names of the tools marked `always_load`, core tools first, in table order.
    #[must_use]
    pub fn always_loaded(&self) -> Vec<&str> {
        let core = self.tools.iter().filter(|t| t.always_load).map(|t| &t.name);
        let generic = self
            .generic
            .iter()
            .filter(|g| g.always_load)
            .map(|g| &g.name);
        core.chain(generic).map(String::as_str).collect()
    }

    /// The purpose of the namespace `name`.
    #[must_use]
    pub fn purpose(&self, name: &str) -> Option<&str> {
        self.namespaces
            .iter()
            .find(|n| n.name == name)
            .map(|n| n.purpose.as_str())
    }

    /// The search keywords of the command or query `command`.
    #[must_use]
    pub fn keywords_of(&self, command: &str) -> &[String] {
        self.keywords.get(command).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_table_parses_and_is_well_formed() {
        let table = CoreTable::parse(SOURCE).unwrap();
        assert_eq!(CoreTable::get(), &table);
        assert!(table.tools.len() >= 20, "the P2 core set");
        let mut names: Vec<&str> = table.tools.iter().map(|t| t.name.as_str()).collect();
        let listed = names.len();
        names.extend(table.pending.iter().map(|p| p.name.as_str()));
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "a tool is named twice");
        let mut sorted = names[..listed].to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, names[..listed], "listed tools are in name order");
        for tool in &table.tools {
            // The naming rule; an alias (a name that is not the rule's) needs an ADR.
            assert_eq!(tool.name, tool_name(&tool.command), "{}", tool.name);
            let listing = tool.listing();
            assert!(
                listing.chars().count() <= MAX_DESCRIPTION_CHARS,
                "{}: {} characters",
                tool.name,
                listing.chars().count()
            );
            assert!(!tool.description.contains('\n'), "{}", tool.name);
            assert!(
                tool.description.ends_with('.'),
                "{}: a sentence, so the example reads on",
                tool.name
            );
            let example: serde_json::Value = serde_json::from_str(&tool.example)
                .unwrap_or_else(|e| panic!("{}: the example is not JSON: {e}", tool.name));
            assert!(example.is_object(), "{}", tool.name);
        }
    }

    #[test]
    fn the_always_load_marks_are_the_five_of_wp_2i_07() {
        let table = CoreTable::get();
        assert_eq!(
            table.always_loaded(),
            [
                "project_info",
                "timeline_get",
                "cutweave_search_commands",
                "cutweave_describe_command",
                "cutweave_execute"
            ]
        );
        assert!(table.always_loaded().len() <= MAX_ALWAYS_LOAD);
        assert!(table.always_load("timeline_get") && !table.always_load("clip_split"));
        assert!(!table.always_load("cutweave_query"));
        // The key is legal in `_meta` under both revisions: one prefix label that is not
        // reserved, a slash, an alphanumeric name.
        let (prefix, name) = META_ALWAYS_LOAD.split_once('/').unwrap();
        assert!(prefix.chars().all(|c| c.is_ascii_alphabetic()));
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn namespaces_and_keywords_are_well_formed() {
        let table = CoreTable::get();
        let names: Vec<&str> = table.namespaces.iter().map(|n| n.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, names, "namespaces are in name order, once each");
        for ns in &table.namespaces {
            assert!(!ns.purpose.is_empty() && !ns.purpose.contains('\n'));
            assert!(!ns.purpose.ends_with('.'), "{}: no full stop", ns.name);
            assert!(ns.purpose.chars().count() <= 120, "{}", ns.name);
        }
        assert!(!table.keywords.is_empty());
        for (command, words) in &table.keywords {
            assert!(!words.is_empty(), "{command}");
            let mut unique = words.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), words.len(), "{command}: a keyword twice");
            for word in words {
                assert!(
                    word.len() >= 2
                        && word
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                    "{command}: `{word}` is not one lower-case word"
                );
            }
        }
    }

    #[test]
    fn a_table_with_an_unknown_key_is_refused() {
        let one = "{ max_tools = 1, max_tokens = 1 }";
        let profiles =
            format!("[profiles]\nminimal = {one}\nstandard = {one}\ndescribed = {one}\n");
        assert!(CoreTable::parse(&profiles).is_ok());
        let text = format!(
            "{profiles}[[tool]]\nname = \"a_b\"\ncommand = \"a.b\"\n\
             description = \"X.\"\nexample = '{{}}'\nextra = 1\n"
        );
        assert!(CoreTable::parse(&text).unwrap_err().contains("extra"));
        // A profile the table does not know, and a table without its budgets, are refused too.
        let unknown = format!("{profiles}huge = {one}\n");
        assert!(CoreTable::parse(&unknown).unwrap_err().contains("huge"));
        assert!(CoreTable::parse("[profiles]\n").is_err());
    }
}
