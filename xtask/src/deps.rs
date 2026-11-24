// SPDX-License-Identifier: GPL-3.0-or-later
//! Dependency-rule checker (docs/01-architecture.md §3).
//!
//! Rules:
//! * `cutweave-render`, `cutweave-audio` and `cutweave-media` never depend on `cutweave-api`.
//! * `cutweave-model` depends on no non-wasm-safe crate (workspace crates above it in the
//!   layering, or a known native-only external crate).
//! * `cutweave-render`, `cutweave-audio` and `cutweave-model` use the media layer only through
//!   its trait half (WP-1D.01): no direct `ffmpeg-next`/`ffmpeg-sys-next` dependency, and never
//!   the `ffmpeg` feature of `cutweave-media` (neither on the dependency nor through one of their
//!   own features).

use std::collections::BTreeMap;

/// Crates that must never depend on `cutweave-api`.
const NO_API: &[&str] = &["cutweave-render", "cutweave-audio", "cutweave-media"];

/// Crates that may see `cutweave-media` only as the trait crate (default features).
const TRAIT_ONLY_MEDIA: &[&str] = &["cutweave-render", "cutweave-audio", "cutweave-model"];

/// The FFmpeg binding crates, private to `cutweave-media`.
const FFMPEG_CRATES: &[&str] = &["ffmpeg-next", "ffmpeg-sys-next"];

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

/// One direct dependency of a package and the features it enables on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dep {
    /// Package name of the dependency.
    pub name: String,
    /// Features the dependent enables on it.
    pub features: Vec<String>,
}

/// A workspace package as far as the rules care.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Package {
    /// Direct dependencies (normal, dev and build).
    pub deps: Vec<Dep>,
    /// Every value of the package's own `[features]` table (e.g. `cutweave-media/ffmpeg`).
    pub feature_values: Vec<String>,
}

/// Maps each workspace package name to its dependencies.
pub type DepGraph = BTreeMap<String, Package>;

/// Whether a feature value of a dependent enables `cutweave-media`'s `ffmpeg` feature.
fn enables_media_ffmpeg(value: &str) -> bool {
    matches!(value, "cutweave-media/ffmpeg" | "cutweave-media?/ffmpeg")
}

/// Returns one message per dependency-rule violation.
#[must_use]
pub fn violations(graph: &DepGraph) -> Vec<String> {
    let mut out = Vec::new();
    for (name, pkg) in graph {
        let deps = &pkg.deps;
        if NO_API.contains(&name.as_str()) && deps.iter().any(|d| d.name == "cutweave-api") {
            out.push(format!("{name} must not depend on cutweave-api"));
        }
        if name == "cutweave-model" {
            for d in deps {
                let upstream = d.name.starts_with("cutweave-");
                if upstream || MODEL_FORBIDDEN.contains(&d.name.as_str()) {
                    out.push(format!(
                        "cutweave-model must not depend on non-wasm-safe `{}`",
                        d.name
                    ));
                }
            }
        }
        if TRAIT_ONLY_MEDIA.contains(&name.as_str()) {
            for d in deps {
                if FFMPEG_CRATES.contains(&d.name.as_str()) {
                    out.push(format!(
                        "{name} must not depend on `{}`: FFmpeg is private to cutweave-media",
                        d.name
                    ));
                }
                if d.name == "cutweave-media" && d.features.iter().any(|f| f == "ffmpeg") {
                    out.push(format!(
                        "{name} must use cutweave-media's trait only, not its `ffmpeg` feature"
                    ));
                }
            }
            if pkg.feature_values.iter().any(|v| enables_media_ffmpeg(v)) {
                out.push(format!(
                    "{name} must not enable cutweave-media's `ffmpeg` feature from its features"
                ));
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
    let strings = |v: Option<&serde_json::Value>| -> Vec<String> {
        v.and_then(serde_json::Value::as_array).map_or_default(|a| {
            a.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
    };
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
            .filter_map(|d| {
                let name = d.get("name").and_then(serde_json::Value::as_str)?;
                Some(Dep {
                    name: name.to_owned(),
                    features: strings(d.get("features")),
                })
            })
            .collect();
        let feature_values = pkg
            .get("features")
            .and_then(serde_json::Value::as_object)
            .map_or_default(|table| table.values().flat_map(|v| strings(Some(v))).collect());
        graph.insert(
            name.to_owned(),
            Package {
                deps,
                feature_values,
            },
        );
    }
    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(entries: &[(&str, &[&str])]) -> DepGraph {
        entries
            .iter()
            .map(|(n, d)| {
                let deps = d
                    .iter()
                    .map(|s| Dep {
                        name: (*s).to_owned(),
                        features: Vec::new(),
                    })
                    .collect();
                (
                    (*n).to_owned(),
                    Package {
                        deps,
                        feature_values: Vec::new(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn clean_graph_passes() {
        let g = graph(&[
            ("cutweave-model", &["serde"]),
            ("cutweave-render", &["cutweave-model", "cutweave-media"]),
            ("cutweave-audio", &["cutweave-media"]),
            ("cutweave-media", &["cutweave-model", "ffmpeg-next"]),
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
    fn model_depending_on_media_is_rejected() {
        let g = graph(&[("cutweave-model", &["cutweave-media"])]);
        assert_eq!(violations(&g).len(), 1);
    }

    #[test]
    fn render_and_audio_must_not_reach_ffmpeg() {
        let mut g = graph(&[
            ("cutweave-render", &["ffmpeg-next"]),
            ("cutweave-audio", &["ffmpeg-sys-next"]),
            // The engine (composition root) may enable the backend.
            ("cutweave-engine", &["cutweave-media"]),
        ]);
        if let Some(engine) = g.get_mut("cutweave-engine") {
            engine.deps[0].features = vec!["ffmpeg".to_owned()];
        }
        let v = violations(&g);
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(
            v.iter().all(|m| m.contains("private to cutweave-media")),
            "{v:?}"
        );
    }

    #[test]
    fn the_ffmpeg_feature_is_rejected_on_the_dependency_and_in_feature_tables() {
        let mut g = graph(&[
            ("cutweave-audio", &["cutweave-media"]),
            ("cutweave-render", &["cutweave-media"]),
        ]);
        if let Some(audio) = g.get_mut("cutweave-audio") {
            audio.deps[0].features = vec!["ffmpeg".to_owned()];
        }
        if let Some(render) = g.get_mut("cutweave-render") {
            render.feature_values = vec!["cutweave-media?/ffmpeg".to_owned()];
        }
        let v = violations(&g);
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(v[0].starts_with("cutweave-audio must use cutweave-media's trait only"));
        assert!(v[1].starts_with("cutweave-render must not enable"));
    }

    #[test]
    fn parses_metadata_json() {
        let json = r#"{"packages":[{"name":"a","dependencies":[{"name":"b","features":["x"]}],
            "features":{"f":["b/x","dep:b"]}}]}"#;
        let g = graph_from_metadata(json).unwrap();
        assert_eq!(
            g["a"].deps,
            vec![Dep {
                name: "b".to_owned(),
                features: vec!["x".to_owned()]
            }]
        );
        assert_eq!(g["a"].feature_values, ["b/x", "dep:b"]);
    }

    #[test]
    fn fixture_violation_is_detected_from_json() {
        let json =
            r#"{"packages":[{"name":"cutweave-media","dependencies":[{"name":"cutweave-api"}]}]}"#;
        let g = graph_from_metadata(json).unwrap();
        assert_eq!(violations(&g).len(), 1);
    }

    #[test]
    fn feature_use_is_detected_from_json() {
        let json = r#"{"packages":[{"name":"cutweave-render","dependencies":[
            {"name":"cutweave-media","features":["ffmpeg"]}]}]}"#;
        let g = graph_from_metadata(json).unwrap();
        assert_eq!(violations(&g).len(), 1);
    }
}
