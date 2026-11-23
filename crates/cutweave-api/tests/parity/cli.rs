// SPDX-License-Identifier: GPL-3.0-or-later
//! The CLI surface: every step is a `cutweave` child process (`cutweave exec` / `cutweave
//! query`), attached to the run's engine through its run-file, as a script in a terminal would
//! run it. The actor is the run-file's `cli` agent token's: `agent:cli`.
//!
//! The `cutweave` binary belongs to another package, so it has no `CARGO_BIN_EXE_` here. This
//! test binary runs as it instead: [`cli_child`](crate::cli_child) calls `cutweave_cli::run`,
//! the function the `cutweave` binary's `main` is, with the arguments in [`ARGS_ENV`], when the
//! harness re-executes this binary filtered to that one test (the pattern of the engine's crash
//! tests). Libtest prints a `running 1 test` preamble on stdout before the CLI's own output.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use cutweave_commands::{EntryKind, Registry};
use serde_json::{Value, json};

use crate::script::{Call, Outcome, StepError};

/// The environment variable carrying the child's `cutweave` arguments (a JSON array of
/// strings, so nothing depends on the platform's command-line quoting).
pub const ARGS_ENV: &str = "CUTWEAVE_PARITY_CLI_ARGS";

/// The arguments that make this test binary run only [`cli_child`](crate::cli_child).
pub const CHILD_ARGS: [&str; 5] = [
    "--exact",
    "cli_child",
    "--nocapture",
    "--test-threads=1",
    "-q",
];

/// What libtest prints on stdout before the child's own output.
pub const PREAMBLE: &str = "running 1 test\n";

/// This test binary.
pub fn test_binary() -> std::path::PathBuf {
    std::env::current_exe().expect("the test binary")
}

/// Runs `cutweave <args>` in a child process and returns its output.
pub fn cutweave(args: &[String]) -> Output {
    Command::new(test_binary())
        .args(CHILD_ARGS)
        .env(ARGS_ENV, json!(args).to_string())
        // The harness, not the environment, decides determinism and media.
        .env_remove("CUTWEAVE_DETERMINISTIC_IDS")
        .env_remove("CUTWEAVE_DEV_STUB_MEDIA")
        .stdin(Stdio::null())
        .output()
        .expect("spawn the cutweave child")
}

/// The CLI's JSON on stdout: its last line (after libtest's preamble).
pub fn stdout_json(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .rev()
        .find(|l| l.starts_with('{') || l.starts_with('['))
        .unwrap_or_else(|| {
            panic!(
                "no JSON on stdout: {text}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
    serde_json::from_str(line).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {line}"))
}

/// The CLI's error JSON on stderr (its last line).
fn stderr_json(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stderr);
    let line = text
        .lines()
        .rev()
        .find(|l| l.starts_with('{'))
        .unwrap_or_else(|| {
            panic!(
                "no error JSON on stderr (exit {:?}): {text}",
                out.status.code()
            )
        });
    serde_json::from_str(line).unwrap_or_else(|e| panic!("stderr is not JSON ({e}): {line}"))
}

/// The CLI driver: attaches to the engine whose run-file is in `run_dir`.
pub struct Cli<'a> {
    pub run_dir: &'a Path,
    pub registry: &'a Registry,
}

impl Cli<'_> {
    fn target(&self) -> [String; 2] {
        [
            "--run-dir".into(),
            self.run_dir.to_string_lossy().into_owned(),
        ]
    }

    /// One call: `cutweave query` for a query, `cutweave exec` with one step otherwise.
    pub fn call(&self, call: &Call) -> Outcome {
        let is_query = self
            .registry
            .get(&call.cmd)
            .is_some_and(|e| e.kind() == EntryKind::Query);
        let mut args: Vec<String> = if is_query {
            vec!["query".into(), call.cmd.clone(), call.args.to_string()]
        } else {
            vec![
                "exec".into(),
                json!({ "cmd": call.cmd, "args": call.args }).to_string(),
            ]
        };
        args.extend(self.target());
        let out = cutweave(&args);
        finish(&out).map(|v| vec![v["output"].clone()])
    }

    /// A batch: `cutweave exec` with an array of steps (one transaction).
    pub fn batch(&self, calls: &[Call]) -> Outcome {
        let steps: Vec<Value> = calls
            .iter()
            .map(|c| json!({ "cmd": c.cmd, "args": c.args }))
            .collect();
        let mut args = vec!["exec".to_owned(), Value::Array(steps).to_string()];
        args.extend(self.target());
        let out = cutweave(&args);
        finish(&out).map(|v| {
            v["steps"]
                .as_array()
                .expect("a batch result has steps")
                .iter()
                .map(|s| s["output"].clone())
                .collect()
        })
    }
}

/// Exit 0: the stdout JSON; exit 1: the stderr error. Anything else is a harness failure.
fn finish(out: &Output) -> Result<Value, StepError> {
    match out.status.code() {
        Some(0) => Ok(stdout_json(out)),
        Some(1) => {
            let raw = stderr_json(out);
            Err(StepError {
                code: raw["code"].as_str().unwrap_or_default().to_owned(),
                failed_step: raw.get("failed_step").and_then(Value::as_u64),
                raw,
            })
        }
        other => panic!(
            "cutweave exited {other:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}
