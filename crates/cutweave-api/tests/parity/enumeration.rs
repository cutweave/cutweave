// SPDX-License-Identifier: GPL-3.0-or-later
//! Registry-enumeration parity (arch §14 "Parity guarantee"): every registered entry that is
//! not a `view` command must be reachable over MCP with the registry's input schema, over RPC,
//! and through `cutweave describe`.
//!
//! - [`check_registry`] is the static part: MCP reachability (a core tool of the standard
//!   profile, or the generic tools `cutweave_describe_command` / `cutweave_execute` /
//!   `cutweave_query`) and the listed tool schema. A `hidden` entry needs an exemption in
//!   `schema/parity-exempt.toml`; an exemption that no longer applies is an error too.
//! - The live checks in `main.rs` call every entry through each surface.
//!
//! Listed tool schemas are trimmed to fit the tool-list budget (WP-1J.01: no titles, no long
//! descriptions, flattened nullable wrappers) while `cutweave_describe_command` returns the
//! registry schema verbatim. [`validation_form`] removes exactly what JSON Schema treats as
//! annotation (it never changes which values are valid), so two schemas with the same
//! validation form accept the same values.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cutweave_api::mcp::{Catalog, Profile};
use cutweave_commands::{CommandMeta, Registry};
use serde_json::{Map, Value};

/// The exemption list (reviewed by CODEOWNERS).
pub fn exemptions_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/parity-exempt.toml")
}

/// One exemption: a registry entry MCP does not reach, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exemption {
    pub name: String,
    pub reason: String,
}

/// Parses `schema/parity-exempt.toml`: `[[exempt]]` tables with `name` and a non-empty
/// `reason`, nothing else.
pub fn parse_exemptions(text: &str) -> Result<Vec<Exemption>, String> {
    let table: toml::Table = text.parse().map_err(|e| format!("not TOML: {e}"))?;
    if let Some(key) = table.keys().find(|k| *k != "exempt") {
        return Err(format!("unknown key `{key}` (expected [[exempt]] tables)"));
    }
    let Some(list) = table.get("exempt") else {
        return Ok(Vec::new());
    };
    let list = list
        .as_array()
        .ok_or("`exempt` must be an array of tables ([[exempt]])")?;
    let mut out: Vec<Exemption> = Vec::new();
    for (i, item) in list.iter().enumerate() {
        let item = item
            .as_table()
            .ok_or_else(|| format!("exempt[{i}] is not a table"))?;
        if let Some(key) = item
            .keys()
            .find(|k| !matches!(k.as_str(), "name" | "reason"))
        {
            return Err(format!("exempt[{i}]: unknown key `{key}`"));
        }
        let field = |key: &str| {
            item.get(key)
                .and_then(toml::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("exempt[{i}]: `{key}` must be a non-empty string"))
        };
        let exemption = Exemption {
            name: field("name")?,
            reason: field("reason")?,
        };
        if out.iter().any(|e| e.name == exemption.name) {
            return Err(format!("exempt[{i}]: `{}` is listed twice", exemption.name));
        }
        out.push(exemption);
    }
    Ok(out)
}

/// The committed exemption list.
pub fn exemptions() -> Vec<Exemption> {
    let path = exemptions_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    parse_exemptions(&text).unwrap_or_else(|e| panic!("schema/parity-exempt.toml: {e}"))
}

/// Whether the parity guarantee covers `meta`: everything but `view` state.
pub fn in_scope(meta: &CommandMeta) -> bool {
    !meta.view && meta.namespace() != "view"
}

fn exposure(meta: &CommandMeta) -> String {
    serde_json::to_value(meta.mcp)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{:?}", meta.mcp))
}

/// The static enumeration check over `registry`: one message per problem, each naming the
/// entry. Empty when parity holds.
pub fn check_registry(registry: &Arc<Registry>, exemptions: &[Exemption]) -> Vec<String> {
    let catalog = Catalog::new(Arc::clone(registry), Profile::Standard);
    let mut problems = Vec::new();
    for entry in registry.list() {
        let meta = &entry.meta;
        if !in_scope(meta) {
            continue;
        }
        let exempt = exemptions.iter().any(|e| e.name == meta.name);
        match (catalog.reachable(meta.name).is_some(), exempt) {
            (false, false) => problems.push(format!(
                "`{}` is not reachable over MCP (mcp = \"{}\"): it is neither a core tool nor \
                 reachable through cutweave_execute / cutweave_describe_command, and \
                 schema/parity-exempt.toml has no exemption for it. Expose it (mcp = \"core\" \
                 or \"catalog\") or add an [[exempt]] entry with the reason.",
                meta.name,
                exposure(meta)
            )),
            (true, true) => problems.push(format!(
                "`{}` is exempt in schema/parity-exempt.toml but reachable over MCP: remove the \
                 stale exemption",
                meta.name
            )),
            _ => {}
        }
        if catalog.lists(meta) {
            let tool = catalog
                .tool(&meta.mcp_tool_name())
                .expect("a listed entry has a tool");
            let listed = Value::Object(tool.input_schema.as_ref().clone());
            if validation_form(&listed) != validation_form(&meta.input_schema) {
                problems.push(format!(
                    "`{}`: the listed inputSchema of tool `{}` does not validate like the \
                     registry schema\n  listed:   {}\n  registry: {}",
                    meta.name,
                    tool.name,
                    validation_form(&listed),
                    validation_form(&meta.input_schema)
                ));
            }
        }
    }
    for e in exemptions {
        match registry.get(&e.name) {
            None => problems.push(format!(
                "schema/parity-exempt.toml exempts `{}`, which is not registered",
                e.name
            )),
            Some(entry) if !in_scope(&entry.meta) => problems.push(format!(
                "schema/parity-exempt.toml exempts `{}`, a `view` entry (out of scope anyway)",
                e.name
            )),
            Some(_) => {}
        }
    }
    problems
}

/// Annotation keywords (JSON Schema 2020-12 meta-data vocabulary and `$comment`): they never
/// change which instances are valid.
const ANNOTATIONS: &[&str] = &[
    "title",
    "description",
    "default",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
    "$comment",
];
/// Keywords whose value maps names (not keywords) to subschemas.
const SCHEMA_MAPS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
];
/// Keywords whose value is one subschema.
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

/// The published input schema of every entry: `schema/commands.json` and `schema/queries.json`
/// (generated from the registry; what RPC clients and the TS types are built from).
pub fn published_schemas() -> std::collections::BTreeMap<String, Value> {
    let mut out = std::collections::BTreeMap::new();
    for (file, list) in [("commands.json", "commands"), ("queries.json", "queries")] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../schema")
            .join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let doc: Value = serde_json::from_str(&text).expect("a schema catalog");
        for entry in doc[list].as_array().expect("an entry list") {
            out.insert(
                entry["name"].as_str().expect("a name").to_owned(),
                entry["input_schema"].clone(),
            );
        }
    }
    out
}

/// `schema` without annotations, with nested `anyOf`s that carry nothing else spliced into
/// their parent `anyOf` (`anyOf[{anyOf[A, B]}, C]` accepts what `anyOf[A, B, C]` accepts).
pub fn validation_form(schema: &Value) -> Value {
    let Value::Object(map) = schema else {
        return schema.clone();
    };
    let mut out = Map::new();
    for (key, value) in map {
        let key = key.as_str();
        if ANNOTATIONS.contains(&key) {
            continue;
        }
        let value = if SCHEMA_MAPS.contains(&key) {
            match value {
                Value::Object(children) => Value::Object(
                    children
                        .iter()
                        .map(|(name, child)| (name.clone(), validation_form(child)))
                        .collect(),
                ),
                other => other.clone(),
            }
        } else if SCHEMA_VALUES.contains(&key) {
            validation_form(value)
        } else if SCHEMA_LISTS.contains(&key)
            && let Value::Array(list) = value
        {
            let mut items = Vec::new();
            for item in list.iter().map(validation_form) {
                match item {
                    Value::Object(inner)
                        if key == "anyOf"
                            && inner.len() == 1
                            && matches!(inner.get("anyOf"), Some(Value::Array(_))) =>
                    {
                        if let Some(Value::Array(nested)) = inner.get("anyOf") {
                            items.extend(nested.iter().cloned());
                        }
                    }
                    item => items.push(item),
                }
            }
            Value::Array(items)
        } else {
            value.clone()
        };
        out.insert(key.to_owned(), value);
    }
    Value::Object(out)
}

// ----- tests of the check itself -------------------------------------------------------------

/// A new command declared `mcp = "hidden"` (a test fixture; never registered by the engine).
#[cutweave_commands::command(
    name = "media.parity_fixture",
    summary = "Parity-test fixture: a hidden command nobody exempted",
    undoable,
    mcp = "hidden",
    annotations(destructive = false)
)]
pub struct ParityFixture {
    /// Anything.
    pub note: Option<String>,
}

/// Result of `media.parity_fixture`.
#[cutweave_commands::wire]
pub struct ParityFixtureOutput {}

impl cutweave_commands::Command for ParityFixture {
    type Output = ParityFixtureOutput;

    fn apply(
        &self,
        _: &mut cutweave_commands::Tx<'_>,
    ) -> Result<ParityFixtureOutput, cutweave_commands::CmdError> {
        Ok(ParityFixtureOutput {})
    }
}

fn registry_with_fixture() -> Arc<Registry> {
    let mut registry = cutweave_engine::build_registry().expect("the engine registry");
    registry
        .register::<ParityFixture>()
        .expect("the fixture registers");
    Arc::new(registry)
}

/// Acceptance 2: a new `mcp = "hidden"` command without an exemption fails the enumeration
/// check with a message naming it; with an exemption it passes.
#[test]
fn a_hidden_command_without_an_exemption_fails_naming_it() {
    let exemptions = exemptions();
    let problems = check_registry(&registry_with_fixture(), &exemptions);
    assert_eq!(problems.len(), 1, "{problems:#?}");
    assert!(
        problems[0].contains("`media.parity_fixture`")
            && problems[0].contains("mcp = \"hidden\"")
            && problems[0].contains("schema/parity-exempt.toml"),
        "{}",
        problems[0]
    );
    let mut with = exemptions;
    with.push(Exemption {
        name: "media.parity_fixture".into(),
        reason: "fixture".into(),
    });
    assert_eq!(
        check_registry(&registry_with_fixture(), &with),
        Vec::<String>::new()
    );
}

/// Stale and unknown exemptions are errors too.
#[test]
fn stale_or_unknown_exemptions_fail() {
    let registry = Arc::new(cutweave_engine::build_registry().unwrap());
    let mut list = exemptions();
    list.push(Exemption {
        name: "clip.split".into(),
        reason: "reachable".into(),
    });
    list.push(Exemption {
        name: "clip.nope".into(),
        reason: "not registered".into(),
    });
    let problems = check_registry(&registry, &list);
    assert_eq!(problems.len(), 2, "{problems:#?}");
    assert!(problems[0].contains("`clip.split`") && problems[0].contains("stale"));
    assert!(problems[1].contains("`clip.nope`") && problems[1].contains("not registered"));
}

#[test]
fn the_exemption_file_format_is_strict() {
    assert_eq!(parse_exemptions("").unwrap(), Vec::new());
    let ok = parse_exemptions("[[exempt]]\nname = \"a.b\"\nreason = \"why\"\n").unwrap();
    assert_eq!(ok[0].name, "a.b");
    for bad in [
        "[[exempt]]\nname = \"a.b\"\n",
        "[[exempt]]\nname = \"a.b\"\nreason = \"  \"\n",
        "[[exempt]]\nname = \"a.b\"\nreason = \"x\"\nsurfaces = []\n",
        "[[exempt]]\nname = \"a.b\"\nreason = \"x\"\n[[exempt]]\nname = \"a.b\"\nreason = \"y\"\n",
        "other = 1\n",
    ] {
        assert!(parse_exemptions(bad).is_err(), "{bad}");
    }
}

/// The validation form keeps every validation keyword: a listed schema that lost a `pattern`, a
/// bound or a `required` entry no longer matches the registry's.
#[test]
fn the_validation_form_drops_annotations_only() {
    let registry = cutweave_engine::build_registry().unwrap();
    let meta = &registry.get("clip.trim").unwrap().meta;
    let lean = cutweave_api::mcp::catalog::lean_schema(&meta.input_schema);
    assert_eq!(validation_form(&lean), validation_form(&meta.input_schema));
    assert_ne!(lean, meta.input_schema, "the listed form is trimmed");
    let form = validation_form(&meta.input_schema);
    for pointer in [
        "/properties/item/pattern",
        "/required",
        "/additionalProperties",
        "/properties/edge/oneOf",
    ] {
        let mut tampered = lean.clone();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        tampered
            .pointer_mut(if parent.is_empty() { "" } else { parent })
            .and_then(Value::as_object_mut)
            .unwrap()
            .remove(key)
            .unwrap_or_else(|| panic!("{pointer} in {lean}"));
        assert_ne!(validation_form(&tampered), form, "{pointer}");
    }
}
