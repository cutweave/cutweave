// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine benchmark suite (WP-2J.01) on the generated projects P-400 and P-5K.
//!
//! ```text
//! cargo bench -p cutweave-bench -- --save-baseline p2
//! ```
//!
//! runs the criterion benches (project open, command round trip in process and over the
//! WebSocket, `doc.patched` fan-out, placement ripple, `timeline.get`), then the budget probes
//! of `cutweave_bench::probes` (10,000 mixed commands, engine start, 60 s idle memory), prints
//! every metric with its budget from `bench/budgets.toml` and PASS or FAIL, and writes
//! `bench-results/{machine}/{date}.json` plus the round-trip histogram next to it.
//!
//! `CUTWEAVE_BENCH_PROBES=0` skips the probes (criterion only); `CUTWEAVE_BENCH_SMOKE=1` runs
//! everything on the tiny smoke project. Without `--bench` (`cargo test --benches`) it runs at
//! smoke scale, each bench once. Build with `--features ffmpeg` (after
//! `. scripts/ffmpeg/env.sh`) to measure idle memory with real media.

#![allow(missing_docs, clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use criterion::{BatchSize, Criterion};
use cutweave_bench::harness::call;
use cutweave_bench::probes::{Scale, Workspace, open_once, ripple_once, run_all};
use cutweave_bench::workload::Workload;
use cutweave_bench::{cli, workspace_root};
use serde_json::json;

fn open_and_query(c: &mut Criterion, ws: &Workspace, scale: &Scale) {
    let engine = ws.empty_engine().expect("engine");
    let conn = engine.connect().expect("connect");
    let mut group = c.benchmark_group("project");
    group.sample_size(10);
    group.bench_function(format!("open/{}", scale.edit.name), |b| {
        b.iter_batched(
            || (),
            |()| {
                open_once(&conn, ws).expect("open");
                call(&conn, "project.close", json!({})).expect("close");
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
    open_once(&conn, ws).expect("open");
    let sequence = &ws.edit_manifest.sequence;
    for depth in [1, 2] {
        c.bench_function(
            &format!("timeline_get/{}/depth{depth}", scale.edit.name),
            |b| {
                b.iter(|| {
                    call(
                        &conn,
                        "timeline.get",
                        json!({"sequence": sequence, "depth": depth}),
                    )
                    .expect("timeline.get")
                });
            },
        );
    }
}

fn editing(c: &mut Criterion, ws: &Workspace, scale: &Scale) {
    let engine = ws.stress_engine().expect("engine");
    let conn = engine.connect().expect("connect");
    let mut workload = Workload::new(&ws.stress_manifest, scale.stress.seed);
    let name = &scale.stress.name;
    c.bench_function(&format!("command_rtt/in_process/{name}"), |b| {
        b.iter_batched(
            || workload.next_step(),
            |step| call(&conn, step.command, step.params).expect("command"),
            BatchSize::PerIteration,
        );
    });
    let mut client = engine.ws().expect("websocket");
    client.subscribe_patches().expect("subscribe");
    c.bench_function(&format!("command_rtt/websocket/{name}"), |b| {
        b.iter_batched(
            || workload.next_step(),
            |step| {
                client
                    .round_trip(step.command, step.params)
                    .expect("round trip");
            },
            BatchSize::PerIteration,
        );
    });
    drop(client);
    for &count in &scale.subscribers {
        let mut clients: Vec<_> = (0..count)
            .map(|_| {
                let mut client = engine.ws().expect("websocket");
                client.subscribe_patches().expect("subscribe");
                client
            })
            .collect();
        c.bench_function(&format!("doc_patched_fanout/{count}/{name}"), |b| {
            b.iter_batched(
                || workload.next_step(),
                |step| {
                    let reply = conn
                        .call(cutweave_engine::Call::new(step.command, step.params))
                        .expect("command");
                    for client in &mut clients {
                        client.wait_patched(reply.rev.0).expect("doc.patched");
                    }
                },
                BatchSize::PerIteration,
            );
        });
    }
    c.bench_function(&format!("placement_ripple/{name}"), |b| {
        b.iter_custom(|iters| {
            (0..iters)
                .map(|_| ripple_once(&conn, &ws.stress_manifest).expect("ripple"))
                .sum::<Duration>()
        });
    });
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list") {
        return;
    }
    let flag = |name: &str| std::env::var(name).ok();
    let benching = args.iter().any(|a| a == "--bench") && !args.iter().any(|a| a == "--test");
    let smoke = !benching || flag("CUTWEAVE_BENCH_SMOKE").is_some_and(|v| v != "0");
    let scale = if smoke { Scale::smoke() } else { Scale::full() };
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("bench-scratch")
        .join(scale.suite);
    let workspace = Workspace::prepare(&scale, &scratch, benching).expect("fixtures");

    let mut criterion = Criterion::default().configure_from_args();
    open_and_query(&mut criterion, &workspace, &scale);
    editing(&mut criterion, &workspace, &scale);
    criterion.final_summary();

    if !benching || flag("CUTWEAVE_BENCH_PROBES").is_some_and(|v| v == "0") {
        return;
    }
    // The criterion benches edited the projects: the probes start from fresh ones.
    let workspace = Workspace::prepare(&scale, &scratch, benching).expect("fixtures");
    let exe = Path::new(env!("CARGO_BIN_EXE_cutweave-bench"));
    let outcome = run_all(&workspace, &scale, exe).expect("probes");
    let finished = cli::finish(&workspace_root(), &scale, &outcome, None).expect("result");
    print!("\n{}", finished.report());
}
