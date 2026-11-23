// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1C.03 benchmark: command acknowledgement latency during a 500-command burst with and
//! without autosave running (acceptance: autosave adds no more than 5 ms at p95).
//!
//! The autosave runs with a 5 ms interval, far more aggressive than the 30 s / 2 s defaults, so
//! many saves overlap the burst. `cargo bench -p cutweave-engine --bench autosave` prints
//! p50/p95/p99 for both runs.

#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

#[path = "../tests/journal_support/mod.rs"]
mod journal_support;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use cutweave_commands::DocHandle;
use cutweave_commands::{Rev, Submit, SystemClock};
use cutweave_engine::autosave::{AutosaveConfig, AutosaveService};
use journal_support::{
    Session, SessionOptions, TempDir, append, create_project, fixture, open_session, tracks, ui,
};
use serde_json::json;

static RUNS: AtomicU32 = AtomicU32::new(0);
const ITEMS: usize = 400;
const BURST: usize = 500;
const BURSTS: usize = 8;

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i]
}

fn burst(h: &DocHandle, n: &mut u64) -> Vec<Duration> {
    (0..BURST)
        .map(|_| {
            *n += 1;
            let t = Instant::now();
            h.submit(Submit::new(
                ui(),
                "project.rename",
                json!({"name": format!("p{n}")}),
            ))
            .wait()
            .unwrap();
            t.elapsed()
        })
        .collect()
}

/// Ack latencies over `BURSTS` bursts of `BURST` commands on a 400-item project, and how many
/// autosaves ran meanwhile.
fn run(dir: &TempDir, name: &str, with_autosave: bool) -> (Vec<Duration>, usize) {
    let folder = create_project(&dir.0, name, &fixture(4));
    let Session {
        store,
        actor,
        handle,
        ..
    } = open_session(&folder, &SessionOptions::default());
    let tr = tracks(&handle.snapshot().doc);
    for i in 0..ITEMS {
        append(&handle, tr[i % tr.len()], 30);
    }
    let svc = AutosaveService::new(
        handle.clone(),
        Arc::new(Mutex::new(store)),
        Arc::new(SystemClock),
        AutosaveConfig {
            interval: Duration::from_millis(5),
            idle: Duration::from_millis(1),
            poll: Duration::from_millis(1),
            ..AutosaveConfig::default()
        },
        Rev::ZERO,
    );
    let events = svc.subscribe();
    let task = with_autosave.then(|| svc.spawn().unwrap());
    let mut n = 0;
    let mut samples = Vec::new();
    for _ in 0..BURSTS {
        samples.extend(burst(&handle, &mut n));
    }
    drop(task);
    let saves = events
        .try_iter()
        .filter(|e| e.name() == "project.saved")
        .count();
    drop(actor);
    (samples, saves)
}

fn report(name: &str, mut samples: Vec<Duration>, saves: usize) {
    samples.sort_unstable();
    println!(
        "{name}: p50 {:?}, p95 {:?}, p99 {:?}, max {:?} over {} acks, {saves} autosaves",
        percentile(&samples, 0.50),
        percentile(&samples, 0.95),
        percentile(&samples, 0.99),
        samples[samples.len() - 1],
        samples.len()
    );
}

fn autosave(c: &mut Criterion) {
    let dir = TempDir::new("autosave-bench");
    let mut lines = Vec::new();
    for (name, on) in [
        ("burst_without_autosave", false),
        ("burst_with_autosave", true),
    ] {
        c.bench_function(&format!("autosave/{name}"), |b| {
            b.iter_custom(|iters| {
                let t = Instant::now();
                for _ in 0..iters {
                    let k = RUNS.fetch_add(1, Ordering::Relaxed);
                    let _ = run(&dir, &format!("{name}-{k}"), on);
                }
                t.elapsed()
            });
        });
        let (samples, saves) = run(&dir, &format!("{name}-report"), on);
        lines.push((format!("autosave/{name}"), samples, saves));
    }
    for (name, samples, saves) in lines {
        report(&name, samples, saves);
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .without_plots()
        .sample_size(10)
        .measurement_time(Duration::from_secs(2))
        .warm_up_time(Duration::from_millis(200));
    targets = autosave
}
criterion_main!(benches);
