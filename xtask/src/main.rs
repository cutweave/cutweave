// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask <ci|check-wasm|check-deps|tokens|fetch-corpus|gen>`: developer automation (WP-0A.01).

mod deps;
mod tokens;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Top-level directories every checkout must have (docs/01-architecture.md §3).
#[cfg(test)]
const REQUIRED_DIRS: &[&str] = &[
    "crates", "apps", "packages", "schema", "testdata", "bench", "spikes", "xtask", "scripts",
    "docs",
];

/// Crates that must compile to `wasm32-unknown-unknown`.
const WASM_CRATES: &[&str] = &[
    "cutweave-model",
    "cutweave-commands",
    "cutweave-render",
    "cutweave-text",
    "cutweave-tokens",
];

const HELP: &str = "cargo xtask: Cutweave developer automation

USAGE:
    cargo xtask <COMMAND>

COMMANDS:
    ci            fmt --check, clippy, tests, check-wasm, check-deps
    check-wasm    cargo check the wasm-safe crates for wasm32-unknown-unknown
    check-deps    verify the crate dependency rule (architecture §3)
    fetch-corpus  fetch the test-media corpus (arrives in a later work package)
    tokens        generate design tokens (CSS, Tailwind, TS, Rust); `tokens --check` verifies
    gen           generate schemas and TS types (arrives in WP-1N)
    help          print this message";

fn workspace_root() -> PathBuf {
    // xtask lives at <root>/xtask.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(PathBuf::new, Path::to_path_buf)
}

fn cargo() -> Command {
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(workspace_root());
    cmd
}

fn run(mut cmd: Command) -> bool {
    println!("+ {cmd:?}");
    cmd.status().is_ok_and(|s| s.success())
}

fn check_wasm() -> bool {
    let mut cmd = cargo();
    cmd.args(["check", "--target", "wasm32-unknown-unknown"]);
    for c in WASM_CRATES {
        cmd.args(["-p", c]);
    }
    run(cmd)
}

fn check_deps() -> bool {
    let out = cargo()
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output();
    let Ok(out) = out else {
        eprintln!("failed to run cargo metadata");
        return false;
    };
    if !out.status.success() {
        eprintln!("cargo metadata failed");
        return false;
    }
    let graph = match deps::graph_from_metadata(&String::from_utf8_lossy(&out.stdout)) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("cannot parse cargo metadata: {e}");
            return false;
        }
    };
    let found = deps::violations(&graph);
    for v in &found {
        eprintln!("dependency rule violation: {v}");
    }
    if found.is_empty() {
        println!("check-deps: ok ({} workspace packages)", graph.len());
    }
    found.is_empty()
}

fn ci() -> bool {
    let steps: [(&str, Vec<&str>); 3] = [
        ("fmt", vec!["fmt", "--all", "--check"]),
        (
            "clippy",
            vec![
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
        ("test", vec!["test", "--workspace"]),
    ];
    for (_, args) in steps {
        let mut cmd = cargo();
        cmd.args(args);
        if !run(cmd) {
            return false;
        }
    }
    check_wasm() && check_deps()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = args.first().cloned();
    let ok = match arg.as_deref() {
        Some("ci") => ci(),
        Some("check-wasm") => check_wasm(),
        Some("check-deps") => check_deps(),
        Some("fetch-corpus") => {
            println!("fetch-corpus: not implemented yet (arrives in a later work package)");
            true
        }
        Some("tokens") => tokens::run(&workspace_root(), &args[1..]),
        Some("gen") => {
            println!("gen: not implemented yet (arrives in WP-1N)");
            true
        }
        Some("help" | "--help" | "-h") | None => {
            println!("{HELP}");
            true
        }
        Some(other) => {
            eprintln!("unknown command `{other}`\n\n{HELP}");
            false
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_top_level_dirs_exist() {
        let root = workspace_root();
        for d in REQUIRED_DIRS {
            assert!(root.join(d).is_dir(), "missing top-level directory `{d}`");
        }
    }
}
