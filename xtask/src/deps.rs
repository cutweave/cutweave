// SPDX-License-Identifier: GPL-3.0-or-later
//! Dependency-rule checker (docs/01-architecture.md §3).
//!
//! Rules:
//! * `cutweave-render`, `cutweave-audio` and `cutweave-media` never depend on `cutweave-api`.
//! * `cutweave-model` depends on no non-wasm-safe crate (workspace crates above it in the
//!   layering, or a known native-only external crate).

use std::collections::BTreeMap;

/// Crates that must never depend on `cutweave-api`.
const NO_API: &[&str] = &["cutweave-render", "cutweave-audio", "cutweave-media"];

/// Dependencies `cutweave-model` may never have (not wasm32-clean).
const MODEL_FORBIDDEN: &[&str] = &[
    "tokio",
    "rusqlite",
    "wgpu",
    "cpal",
    "ffmpeg-next",
    "ffmpeg-sys-next",
    "wasmtime",
    "rquickjs",
    "axum",
    "reqwest",
    "napi",
    "keyring",
    "mio",
];

/// Maps each workspace package name to the names of its direct dependencies.
pub type DepGraph = BTreeMap<String, Vec<String>>;

/// Returns one message per dependency-rule violation.
#[must_use]
pub fn violations(graph: &DepGraph) -> Vec<String> {
    let mut out = Vec::new();
    for (name, deps) in graph {
        if NO_API.contains(&name.as_str()) && deps.iter().any(|d| d == "cutweave-api") {
            out.push(format!("{name} must not depend on cutweave-api"));
        }
        if name == "cutweave-model" {
            for d in deps {
                let upstream = d.starts_with("cutweave-");
                if upstream || MODEL_FORBIDDEN.contains(&d.as_str()) {
                    out.push(format!(
                        "cutweave-model must not depend on non-wasm-safe `{d}`"
                    ));
                }
            }
        }
    }
    out
}

/// Builds the graph from `cargo metadata --format-version 1 --no-deps` JSON.
///
/// # Errors
/// Returns an error string if the JSON does not have the expected shape.
pub fn graph_from_metadata(json: &str) -> Result<DepGraph, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let packages = value
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("metadata has no `packages` array")?;
    let mut graph = DepGraph::new();
    for pkg in packages {
        let name = pkg
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or("package without name")?;
        let deps = pkg
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .ok_or("package without dependencies")?
            .iter()
            .filter_map(|d| d.get("name").and_then(serde_json::Value::as_str))
            .map(str::to_owned)
            .collect();
        graph.insert(name.to_owned(), deps);
    }
    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(entries: &[(&str, &[&str])]) -> DepGraph {
        entries
            .iter()
            .map(|(n, d)| ((*n).to_owned(), d.iter().map(|s| (*s).to_owned()).collect()))
            .collect()
    }

    #[test]
    fn clean_graph_passes() {
        let g = graph(&[
            ("cutweave-model", &["serde"]),
            ("cutweave-render", &["cutweave-model"]),
            ("cutweave-api", &["cutweave-render"]),
        ]);
        assert_eq!(violations(&g), Vec::<String>::new());
    }

    #[test]
    fn render_depending_on_api_is_rejected() {
        let g = graph(&[("cutweave-render", &["cutweave-api"])]);
        assert_eq!(violations(&g).len(), 1);
    }

    #[test]
    fn model_depending_on_native_crate_is_rejected() {
        let g = graph(&[("cutweave-model", &["tokio", "cutweave-ops"])]);
        assert_eq!(violations(&g).len(), 2);
    }

    #[test]
    fn parses_metadata_json() {
        let json = r#"{"packages":[{"name":"a","dependencies":[{"name":"b"}]}]}"#;
        let g = graph_from_metadata(json).unwrap();
        assert_eq!(g["a"], vec!["b".to_owned()]);
    }

    #[test]
    fn fixture_violation_is_detected_from_json() {
        let json =
            r#"{"packages":[{"name":"cutweave-media","dependencies":[{"name":"cutweave-api"}]}]}"#;
        let g = graph_from_metadata(json).unwrap();
        assert_eq!(violations(&g).len(), 1);
    }
}
