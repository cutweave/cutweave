// SPDX-License-Identifier: GPL-3.0-or-later
//! SPDX license-header checker and fixer (WP-0A.04, docs/01-architecture.md §15).
//!
//! Every `.rs`, `.ts`, `.tsx`, `.wgsl` and `.wit` file under `crates/`, `xtask/`, `packages/`,
//! `apps/` and `wit/` must start with `// SPDX-License-Identifier: <id>`, where `<id>` is
//! `GPL-3.0-or-later` by default and `MIT OR Apache-2.0` for the plugin-facing surface.
//! `spikes/` is intentionally not scanned.

use std::fs;
use std::path::{Path, PathBuf};

/// Directories scanned, relative to the workspace root.
const ROOTS: &[&str] = &["crates", "xtask", "packages", "apps", "wit"];

/// File extensions that need a header.
const EXTENSIONS: &[&str] = &["rs", "ts", "tsx", "wgsl", "wit"];

/// Directory names never descended into.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "generated", ".turbo"];

/// Path prefixes (workspace-relative, `/`-separated) licensed `MIT OR Apache-2.0`.
const PERMISSIVE_PREFIXES: &[&str] = &["wit/", "packages/plugin-sdk/", "packages/compose-runtime/"];

const DEFAULT_ID: &str = "GPL-3.0-or-later";
const PERMISSIVE_ID: &str = "MIT OR Apache-2.0";
const TAG: &str = "// SPDX-License-Identifier:";

/// The SPDX expression a file at `rel` (workspace-relative, `/`-separated) must carry.
#[must_use]
pub fn expected_id(rel: &str) -> &'static str {
    if PERMISSIVE_PREFIXES.iter().any(|p| rel.starts_with(p)) {
        PERMISSIVE_ID
    } else {
        DEFAULT_ID
    }
}

fn header_line(rel: &str) -> String {
    format!("{TAG} {}", expected_id(rel))
}

/// Outcome of checking one file's content.
#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    /// Correct header on the first line.
    Ok,
    /// No SPDX line at the top.
    Missing,
    /// An SPDX line exists but with a different expression.
    Wrong(String),
}

/// Checks `content` of the file at `rel`.
#[must_use]
pub fn check_content(rel: &str, content: &str) -> Status {
    let first = content
        .trim_start_matches('\u{feff}')
        .lines()
        .next()
        .unwrap_or("");
    if first.trim_end() == header_line(rel) {
        Status::Ok
    } else if first.starts_with(TAG) {
        Status::Wrong(first.trim().to_owned())
    } else {
        Status::Missing
    }
}

/// Returns `content` with the correct header as its first line.
#[must_use]
pub fn fix_content(rel: &str, content: &str) -> String {
    let body = content.trim_start_matches('\u{feff}');
    let header = header_line(rel);
    match check_content(rel, content) {
        Status::Ok => content.to_owned(),
        Status::Wrong(_) => {
            let rest = body.split_once('\n').map_or("", |(_, r)| r);
            format!("{header}\n{rest}")
        }
        Status::Missing => format!("{header}\n{body}"),
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if !SKIP_DIRS.iter().any(|s| name == *s) {
                collect(&path, out);
            }
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTENSIONS.contains(&e))
        {
            out.push(path);
        }
    }
}

/// Runs the checker (`fix == false`) or fixer (`fix == true`); returns success.
#[must_use]
pub fn run(root: &Path, fix: bool) -> bool {
    let mut files = Vec::new();
    for r in ROOTS {
        collect(&root.join(r), &mut files);
    }
    files.sort();
    let mut bad = 0usize;
    for path in &files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(content) = fs::read_to_string(path) else {
            eprintln!("cannot read {rel} as UTF-8");
            bad += 1;
            continue;
        };
        let status = check_content(&rel, &content);
        if status == Status::Ok {
            continue;
        }
        if fix {
            if let Err(e) = fs::write(path, fix_content(&rel, &content)) {
                eprintln!("cannot write {rel}: {e}");
                bad += 1;
            } else {
                println!("fixed {rel}");
            }
        } else {
            let want = header_line(&rel);
            match status {
                Status::Wrong(found) => {
                    eprintln!("{rel}: wrong SPDX header `{found}`, expected `{want}`");
                }
                _ => eprintln!("{rel}: missing `{want}`"),
            }
            bad += 1;
        }
    }
    if bad == 0 {
        println!("license-headers: ok ({} files)", files.len());
    } else if !fix {
        eprintln!(
            "license-headers: {bad} file(s) need fixing; run `cargo xtask license-headers --fix`"
        );
    }
    bad == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_permissive_ids() {
        assert_eq!(expected_id("crates/cutweave-model/src/lib.rs"), DEFAULT_ID);
        assert_eq!(expected_id("packages/ui/src/index.ts"), DEFAULT_ID);
        assert_eq!(
            expected_id("packages/plugin-sdk/src/index.ts"),
            PERMISSIVE_ID
        );
        assert_eq!(
            expected_id("packages/compose-runtime/src/a.ts"),
            PERMISSIVE_ID
        );
        assert_eq!(expected_id("wit/cutweave/plugin.wit"), PERMISSIVE_ID);
    }

    #[test]
    fn detects_ok_missing_wrong() {
        let rel = "crates/x/src/lib.rs";
        let good = "// SPDX-License-Identifier: GPL-3.0-or-later\nfn a() {}\n";
        assert_eq!(check_content(rel, good), Status::Ok);
        assert_eq!(check_content(rel, "fn a() {}\n"), Status::Missing);
        assert!(matches!(
            check_content(rel, "// SPDX-License-Identifier: MIT\nfn a() {}\n"),
            Status::Wrong(_)
        ));
        assert_eq!(check_content(rel, ""), Status::Missing);
    }

    #[test]
    fn fix_adds_and_replaces() {
        let rel = "packages/plugin-sdk/src/index.ts";
        let want = "// SPDX-License-Identifier: MIT OR Apache-2.0\nexport {};\n";
        assert_eq!(fix_content(rel, "export {};\n"), want);
        assert_eq!(
            fix_content(rel, "// SPDX-License-Identifier: MIT\nexport {};\n"),
            want
        );
        assert_eq!(fix_content(rel, want), want);
    }
}
