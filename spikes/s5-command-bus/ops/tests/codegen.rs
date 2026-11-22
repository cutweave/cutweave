//! Snapshot tests of the generated artifacts (arch §5.3): any change to a command's fields,
//! docs, flags or annotations changes a snapshot and fails until reviewed (`cargo insta review`).
//! Plus: the committed files under `schema/` and `ts/src/generated/` must equal fresh codegen.

// Test crate: helpers outside #[test] fns may unwrap (clippy.toml only exempts #[test] bodies).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_pass_by_value
)]

use std::path::Path;

use s5_bus::codegen::{Artifact, artifacts};
use serde_json::Value;

fn generated() -> Vec<Artifact> {
    artifacts(s5_ops::registry()).expect("codegen succeeds")
}

fn get(name: &str) -> String {
    generated()
        .into_iter()
        .find(|a| a.path == name)
        .expect("artifact exists")
        .contents
}

#[test]
fn snapshot_commands_json() {
    insta::assert_snapshot!("commands_json", get("schema/commands.json"));
}

#[test]
fn snapshot_queries_json() {
    insta::assert_snapshot!("queries_json", get("schema/queries.json"));
}

#[test]
fn snapshot_typescript() {
    insta::assert_snapshot!("commands_ts", get("ts/src/generated/commands.ts"));
}

#[test]
fn committed_artifacts_are_up_to_date() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for a in generated() {
        let on_disk = std::fs::read_to_string(root.join(a.path))
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            on_disk == a.contents,
            "{} is stale: run `cargo run -p s5-ops --bin s5-codegen`",
            a.path
        );
    }
}

#[test]
fn time_exports_as_number_never_bigint() {
    let ts = get("ts/src/generated/commands.ts");
    assert!(ts.contains("export type Time = number;"));
    assert!(ts.contains("export type Rev = number;"));
    assert!(!ts.contains("bigint"), "no i64 may leak as bigint");
}

fn walk(v: &Value, f: &mut impl FnMut(&Value)) {
    f(v);
    match v {
        Value::Array(a) => a.iter().for_each(|x| walk(x, f)),
        Value::Object(o) => o.values().for_each(|x| walk(x, f)),
        _ => {}
    }
}

#[test]
fn schemas_are_mcp_ready() {
    for entry in s5_ops::registry().iter() {
        let params = entry.params_schema();
        let name = entry.meta.name;
        // MCP requires inputSchema/outputSchema roots of type object.
        assert_eq!(params["type"], "object", "{name} params");
        assert_eq!(
            params["additionalProperties"], false,
            "{name}: unknown args must be rejected"
        );
        assert_eq!(entry.output_schema()["type"], "object", "{name} output");
        // Self-contained: no $ref/$defs for clients that cannot resolve references.
        for schema in [params, entry.output_schema()] {
            walk(&schema, &mut |v| {
                if let Value::Object(o) = v {
                    assert!(
                        !o.contains_key("$ref") && !o.contains_key("$defs"),
                        "{name}: {v}"
                    );
                }
            });
        }
    }
}

#[test]
fn time_schema_depends_on_direction() {
    let reg = s5_ops::registry();
    let split = reg.get("clip.split").unwrap();
    let at = &split.params_schema()["properties"]["at"];
    assert_eq!(at["anyOf"][0]["type"], "integer");
    assert_eq!(at["anyOf"][0]["maximum"], 9_007_199_254_740_991_i64);
    assert_eq!(
        at["anyOf"][1]["type"], "string",
        "inputs accept timecode strings"
    );
    let trim = reg.get("clip.trim").unwrap();
    let start = &trim.output_schema()["properties"]["start"];
    assert_eq!(start["type"], "integer", "outputs are always integers");
    assert!(start.get("anyOf").is_none());
}
