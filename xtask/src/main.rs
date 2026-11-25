// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask <ci|check-wasm|check-deps|license-headers|tokens|fetch-corpus|corpus-verify|bench|bench-check|gen|golden|golden-projects>`: developer automation (WP-0A.01).

mod bench;
mod corpus;
mod corpus_probe;
mod deps;
mod golden;
mod headers;
mod schema_gen;
mod tokens;
mod ts_gen;

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
    license-headers [--check|--fix]
                  verify (default) or add SPDX headers; alias: check-headers
    fetch-corpus  [--tier small|full] [--pin] [--verify]: fetch/generate the test-media corpus into testdata/corpus/
    corpus-verify [--tier small|full] [--allow-unpinned] [--probe|--no-probe]: check cached corpus files against the
                  manifest (sha256) and probe generated items with ffprobe against their `expect` table
    bench         --suite <name> --out <file>: run a benchmark suite (smoke) and write result JSON
    bench-check   <results.json> [--baseline <results.json>]: compare against bench/budgets.toml
    tokens        generate design tokens (CSS, Tailwind, TS, Rust); `tokens --check` verifies
    gen           [--check]: generate schema/ catalogs and docs/reference/commands.md; --check fails with a diff if committed files are stale
    golden-projects
                  build the CLI with FFmpeg, the synthetic sources and the golden projects into target/golden/
    golden        [--adapter CHOICE]... [--all-adapters] [--update --i-know [--reason TEXT]]
                  render the golden projects headless (default: the software adapter) and compare
                  with testdata/golden/expected/<adapter>/ (needs . scripts/ffmpeg/env.sh)
    golden --check-refs [--base REV] [--pr-labels A,B]
                  the reference-update guard alone (testdata/golden/README.md)
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
    check_wasm() && check_deps() && headers::run(&workspace_root(), false)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = args.first().cloned();
    let ok = match arg.as_deref() {
        Some("ci") => ci(),
        Some("check-wasm") => check_wasm(),
        Some("check-deps") => check_deps(),
        Some("check-headers") => headers::run(&workspace_root(), false),
        Some("license-headers") => match std::env::args().nth(2).as_deref() {
            None | Some("--check") => headers::run(&workspace_root(), false),
            Some("--fix") => headers::run(&workspace_root(), true),
            Some(other) => {
                eprintln!("license-headers: unknown flag `{other}` (use --check or --fix)");
                false
            }
        },
        Some("fetch-corpus") => corpus::fetch(&workspace_root(), &args[1..]),
        Some("corpus-verify") => corpus::verify(&workspace_root(), &args[1..]),
        Some("bench") => bench::bench(&workspace_root(), &args[1..]),
        Some("bench-check") => bench::bench_check(&workspace_root(), &args[1..]),
        Some("tokens") => tokens::run(&workspace_root(), &args[1..]),
        Some("gen") => schema_gen::run(&workspace_root(), &args[1..]),
        Some("golden") => golden::golden(&workspace_root(), &args[1..]),
        Some("golden-projects") => golden::golden_projects(&workspace_root(), &args[1..]),
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
