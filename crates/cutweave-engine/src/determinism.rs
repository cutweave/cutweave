// SPDX-License-Identifier: GPL-3.0-or-later
//! The double-run determinism harness (WP-1B.08, reused by WP-1O.02 and WP-1O.04).
//!
//! `--deterministic-ids <seed>` promises that two identical command scripts produce
//! byte-identical saved project documents (arch §5.1, ADR-0015). [`double_run`] checks exactly
//! that: it runs a script in two fresh engines with the same seed and returns both results;
//! [`run_script`] runs it once. [`normalize_ids`] rewrites every entity id to its order of first
//! appearance, so runs with different seeds can be compared by structure.
//!
//! ```no_run
//! use cutweave_engine::determinism::{Step, double_run};
//! use serde_json::json;
//!
//! let steps = vec![
//!     Step::fixed("project.new", json!({ "name": "Demo" })),
//!     // A step can use what earlier steps returned (`prev[0]` is the first output).
//!     Step::new("track.add", |prev| {
//!         json!({ "sequence": prev[0]["sequence"], "kind": "video" })
//!     }),
//! ];
//! let dir = std::env::temp_dir().join("determinism-demo");
//! let (a, b) = double_run(42, &dir, "Demo", &steps);
//! assert_eq!(a.sha256, b.sha256);
//! ```
//!
//! Behind the cargo feature `testing` of `cutweave-engine`: enable it with
//! `cutweave-engine = { path = "../cutweave-engine", features = ["testing"] }` in the
//! consuming crate's `[dev-dependencies]` (WP-1O.02, WP-1O.04). It panics on failure and is
//! never part of a normal build.
//!
//! The harness is plain code (no test framework) and has no new dependencies, so any crate's
//! tests can call it.

// Test-support code: a script that cannot run is a test failure, so it panics with a message.
#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use cutweave_api::Credential;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{Call, Engine, EngineConfig, EngineMode};

type Build = Box<dyn Fn(&[Value]) -> Value>;

/// One command of a script: a name and parameters, which may depend on earlier outputs.
pub struct Step {
    name: String,
    params: Build,
}

impl Step {
    /// A step whose parameters are computed from the outputs of the steps before it.
    pub fn new(name: &str, params: impl Fn(&[Value]) -> Value + 'static) -> Step {
        Step {
            name: name.to_owned(),
            params: Box::new(params),
        }
    }

    /// A step with fixed parameters.
    #[must_use]
    pub fn fixed(name: &str, params: Value) -> Step {
        Step::new(name, move |_| params.clone())
    }
}

/// What one run of a script produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// The output of each step, in order.
    pub outputs: Vec<Value>,
    /// The saved `.cutweave` document, byte for byte.
    pub document: Vec<u8>,
    /// Lowercase hex SHA-256 of [`document`](Self::document).
    pub sha256: String,
}

impl Run {
    /// The document as text with every id replaced by [`normalize_ids`]'s stand-in.
    #[must_use]
    pub fn normalized_document(&self) -> String {
        normalize_ids(&String::from_utf8_lossy(&self.document))
    }
}

/// Runs `steps` in a fresh engine seeded with `seed`, then saves the document as project
/// `project_name` inside `dir` and returns the saved bytes. Any failing step panics with its
/// name and error (a script is test code). `dir` must exist; the project folder
/// `dir/<project_name>` is removed first and left in place afterwards.
///
/// # Panics
///
/// On any engine, command or I/O error.
#[must_use]
pub fn run_script(seed: u64, dir: &Path, project_name: &str, steps: &[Step]) -> Run {
    let folder = dir.join(project_name);
    if folder.exists() {
        std::fs::remove_dir_all(&folder).expect("remove the previous project folder");
    }
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.deterministic_ids = Some(seed);
    config.allowed_roots = vec![dir.to_path_buf()];
    let engine = Engine::start(config).expect("start the engine");
    let conn = engine
        .handle()
        .connect(&Credential::Agent("determinism".into()))
        .expect("connect");
    let mut outputs: Vec<Value> = Vec::new();
    for step in steps {
        let params = (step.params)(&outputs);
        match conn.call(Call::new(&step.name, params)) {
            Ok(reply) => outputs.push(reply.output),
            Err(e) => panic!("step {} failed: {e}", step.name),
        }
    }
    let saved = conn
        .call(Call::new(
            "project.save_as",
            serde_json::json!({ "path": folder.to_string_lossy() }),
        ))
        .unwrap_or_else(|e| panic!("project.save_as failed: {e}"));
    let path = saved
        .output
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .expect("project.save_as returns the document path");
    let document = std::fs::read(&path).expect("read the saved document");
    engine.shutdown();
    let sha256 = Sha256::digest(&document)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        });
    Run {
        outputs,
        document,
        sha256,
    }
}

/// Runs `steps` twice with `seed`, each in a fresh engine and over a fresh project folder, and
/// returns both runs. The determinism contract is `a.document == b.document`.
///
/// # Panics
///
/// As [`run_script`].
#[must_use]
pub fn double_run(seed: u64, dir: &Path, project_name: &str, steps: &[Step]) -> (Run, Run) {
    (
        run_script(seed, dir, project_name, steps),
        run_script(seed, dir, project_name, steps),
    )
}

/// Replaces every entity id (`med_`, `seq_`, `trk_`, `itm_`, `grp_` plus 32 hex digits) in `text`
/// by `<prefix>_<n>`, numbering the distinct ids by first appearance. Two documents with the
/// same structure but different seeds normalize to the same text.
#[must_use]
pub fn normalize_ids(text: &str) -> String {
    const PREFIXES: [&str; 5] = ["med_", "seq_", "trk_", "itm_", "grp_"];
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    let mut i = 0;
    while i < bytes.len() {
        let id = PREFIXES.iter().find_map(|p| {
            let end = i + p.len() + 32;
            (text.get(i..end)?.starts_with(p)
                && bytes[i + p.len()..end]
                    .iter()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
            .then_some((*p, end))
        });
        if let Some((prefix, end)) = id {
            let full = &text[i..end];
            let next = seen.len() + 1;
            let n = *seen.entry(full).or_insert(next);
            let _ = write!(out, "{prefix}{n}");
            i = end;
        } else {
            let ch = text[i..].chars().next().unwrap_or_default();
            out.push(ch);
            i += ch.len_utf8().max(1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_numbers_ids_by_first_appearance() {
        let a = format!("itm_{0} trk_{1} itm_{0} x", "a".repeat(32), "b".repeat(32));
        assert_eq!(normalize_ids(&a), "itm_1 trk_2 itm_1 x");
        let short = "itm_abc and médiá";
        assert_eq!(normalize_ids(short), short);
    }
}
