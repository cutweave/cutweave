//! The generated TS compiles under the repo's strict tsconfig, and a wrong field fails `tsc`.
//!
//! Uses the repo's pinned TypeScript (`node_modules/typescript`, found by walking up from this
//! crate; override with `S5_TSC=<path to typescript/bin/tsc>`). If none is found the tests are
//! skipped with a message, unless `S5_REQUIRE_TSC=1` (CI) makes that a failure.

// Test crate: helpers outside #[test] fns may unwrap (clippy.toml only exempts #[test] bodies).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_pass_by_value
)]

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn find_tsc() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("S5_TSC") {
        return Some(PathBuf::from(p));
    }
    let mut dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("node_modules/typescript/bin/tsc");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
}

fn tsc(project: &str) -> Option<Output> {
    let Some(tsc) = find_tsc() else {
        assert!(
            std::env::var("S5_REQUIRE_TSC").is_err(),
            "S5_REQUIRE_TSC is set but no TypeScript was found (run `pnpm install` at the repo root)"
        );
        eprintln!("SKIPPED: no node_modules/typescript found; run `pnpm install` at the repo root");
        return None;
    };
    let ts_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ts");
    let out = Command::new("node")
        .arg(&tsc)
        .args(["-p", project, "--pretty", "false"])
        .current_dir(ts_dir)
        .output()
        .expect("node is installed");
    Some(out)
}

#[test]
fn generated_client_typechecks_under_strict() {
    let Some(out) = tsc("tsconfig.json") else {
        return;
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "tsc failed:\n{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn wrong_field_fails_tsc() {
    let Some(out) = tsc("tsconfig.negative.json") else {
        return;
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "negative sample must not compile");
    assert!(
        stdout.contains("negative/wrong_field.ts")
            && stdout.contains("TS2353")
            && stdout.contains("'position'"),
        "expected an excess-property error on `position`, got:\n{stdout}"
    );
    assert_eq!(
        stdout.lines().filter(|l| l.contains("error TS")).count(),
        1,
        "{stdout}"
    );
}
