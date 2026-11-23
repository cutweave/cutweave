// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.08: all ID creation goes through an injected `IdGen`, so `--deterministic-ids` covers
//! every new entity. A grep over the workspace's Rust sources forbids creating UUIDs directly
//! anywhere but `Uuid7Gen` (`cutweave-model/src/id.rs`), and forbids the process-wide
//! `XxxId::new()` shortcut in the crates that mint ids on behalf of the document.

#![allow(clippy::expect_used)] // helpers, not `#[test]` bodies

use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                rust_files(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

fn workspace_crates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ dir")
        .to_path_buf()
}

fn sources(root: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    rust_files(root, &mut files);
    files
        .into_iter()
        .filter_map(|p| {
            let text = fs::read_to_string(&p).ok()?;
            let rel = p
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            Some((rel, text))
        })
        .collect()
}

/// The patterns are assembled at run time so this file does not match itself.
fn forbidden_uuid_calls() -> Vec<String> {
    ["now_v7", "new_v4", "new_v7", "Uuid::new_v", "Uuid::now"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

#[test]
fn only_uuid7gen_creates_uuids() {
    let root = workspace_crates();
    let mut offenders = Vec::new();
    for (rel, text) in sources(&root) {
        if rel == "cutweave-model/src/id.rs" || rel.ends_with("id_discipline.rs") {
            continue;
        }
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if forbidden_uuid_calls()
                .iter()
                .any(|p| code.contains(p.as_str()))
            {
                offenders.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "create IDs through an injected `IdGen`, never directly:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn document_crates_never_use_the_process_wide_id_constructors() {
    let root = workspace_crates();
    let kinds = ["Media", "Sequence", "Track", "Item", "Group"];
    let mut offenders = Vec::new();
    for krate in ["cutweave-commands", "cutweave-ops", "cutweave-engine"] {
        for (rel, text) in sources(&root.join(krate).join("src")) {
            // Unit-test modules sit at the end of a file; fixtures there may mint real ids.
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            for (n, line) in code.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if kinds
                    .iter()
                    .any(|k| line.contains(&format!("{k}Id::new()")))
                {
                    offenders.push(format!("{krate}/src/{rel}:{}: {}", n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "mint ids with `tx.mint()` / an injected `IdGen`:\n{}",
        offenders.join("\n")
    );
}
