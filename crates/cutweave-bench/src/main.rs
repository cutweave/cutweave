// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-bench`: the benchmark tool (WP-2J.01).
//!
//! ```text
//! cutweave-bench gen [...]      write fixture projects (same as `cargo xtask gen-bench-project`)
//! cutweave-bench run [--smoke] [--with-media] [--out FILE]
//!                               the engine suite without criterion: table and result JSON
//! cutweave-bench engine [...]   an engine process (the launch protocol), which the start and
//!                               memory probes measure
//! cutweave-bench crash [...]    the crash-injection harness (WP-2J.05): kill the engine during
//!                               seeded work, restart it, check that nothing is lost
//! cutweave-bench agent-eval [...]
//!                               the agent evaluation (WP-2I.09): a model does tasks over MCP
//!                               and how it uses the tools is measured; on demand, needs a key
//! ```
//!
//! WP-2J.03 adds `playback`, `scrub`, `sync` and `export`.

use std::path::PathBuf;
use std::process::ExitCode;

use cutweave_bench::probes::{Scale, Workspace, run_all};
use cutweave_bench::{BenchError, cli, workspace_root};

const USAGE: &str = "usage: cutweave-bench <gen|run|engine|crash|agent-eval> [options]
  gen [...]     write fixture projects (`gen --help`)
  run [--smoke] [--with-media] [--out FILE]
                run the engine suite once and write the result JSON
  engine [...]  run an engine process (docs/reference/launch-protocol.md)
  crash [--count N] [--seed S] [...]
                kill the engine N times during editing work and check its recovery
                (`crash --help`)
  agent-eval [--model ID] [--dry-run] [--replay FILE] [...]
                give a model the evaluation's tasks over MCP and measure how it uses the
                tools; needs ANTHROPIC_API_KEY, never part of the gate (`agent-eval --help`)";

fn run(args: &[String]) -> Result<bool, BenchError> {
    let (mut scale, mut with_media, mut out) = (Scale::full(), false, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--smoke" => scale = Scale::smoke(),
            "--with-media" => with_media = true,
            "--out" => {
                out =
                    Some(PathBuf::from(it.next().ok_or_else(|| {
                        BenchError::Spec("--out needs a value".into())
                    })?));
            }
            other => return Err(BenchError::Spec(format!("unknown option `{other}`"))),
        }
    }
    let root = workspace_root();
    // The folder `cargo bench` uses too, so the source files are made once.
    let scratch = root
        .join("target")
        .join("tmp")
        .join("bench-scratch")
        .join(scale.suite);
    let exe = std::env::current_exe().map_err(|e| BenchError::Other(e.to_string()))?;
    let workspace = Workspace::prepare(&scale, &scratch, with_media)?;
    let outcome = run_all(&workspace, &scale, &exe)?;
    let finished = cli::finish(&root, &scale, &outcome, out.as_deref())?;
    print!("{}", finished.report());
    Ok(finished.failed.is_empty() && finished.not_run.is_empty())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match args.first().map(String::as_str) {
        // Exit directly: a blocked stdin read must not delay the exit.
        Some("engine") => std::process::exit(cutweave_engine::launch::main()),
        Some("gen") if args.iter().any(|a| a == "--help") => {
            println!("{}", cli::GEN_USAGE);
            Ok(true)
        }
        Some("gen") => cli::gen_bench_project(&workspace_root(), &args[1..]).map(|report| {
            print!("{report}");
            true
        }),
        Some("run") => run(&args[1..]),
        Some("crash") if args.iter().any(|a| a == "--help") => {
            println!("{}", cutweave_bench::crash::USAGE);
            Ok(true)
        }
        Some("crash") => std::env::current_exe()
            .map_err(|e| BenchError::Other(e.to_string()))
            .and_then(|exe| cutweave_bench::crash::command(&workspace_root(), &exe, &args[1..]))
            .map(|(report, passed)| {
                print!("{report}");
                passed
            }),
        Some("agent-eval") if args.iter().any(|a| a == "--help") => {
            println!("{}", cutweave_bench::agent_eval::USAGE);
            Ok(true)
        }
        Some("agent-eval") => std::env::current_exe()
            .map_err(|e| BenchError::Other(e.to_string()))
            .and_then(|exe| {
                cutweave_bench::agent_eval::command(&workspace_root(), &exe, &args[1..])
            })
            .map(|(report, passed)| {
                print!("{report}");
                passed
            }),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("cutweave-bench: {e}");
            ExitCode::FAILURE
        }
    }
}
