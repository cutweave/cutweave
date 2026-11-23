// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1C.02 benchmark: what the journal costs a command round trip, per durability policy, on a
//! 400-item project (arch §19: command RTT budget 16 ms).
//!
//! - `journal/fdatasync`: one device flush of a small appended record (the price of
//!   [`SyncPolicy::EveryCommit`] per transaction);
//! - `journal/rtt_*`: doc actor submit → journal → ack round trip of a rename with no journal,
//!   with the default group commit (`write()` before the ack, `fdatasync` within 50 ms on the
//!   flusher thread) and with `fdatasync` before every ack.
//!
//! `cargo bench -p cutweave-engine --bench journal` (p50/p95/p99 lines are printed at the end).

#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

#[path = "../tests/journal_support/mod.rs"]
mod journal_support;

use std::fs::OpenOptions;
use std::io::Write;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use cutweave_commands::{DocHandle, Submit};
use cutweave_engine::journal::SyncPolicy;
use journal_support::{
    Session, SessionOptions, TempDir, append, create_project, fixture, open_session, tracks, ui,
};
use serde_json::json;

const ITEMS: usize = 400;
const SAMPLES: usize = 2_000;

fn session(dir: &TempDir, name: &str, sync: SyncPolicy) -> Session {
    let folder = create_project(&dir.0, name, &fixture(4));
    let s = open_session(
        &folder,
        &SessionOptions {
            sync,
            ..SessionOptions::default()
        },
    );
    let tr = tracks(&s.handle.snapshot().doc);
    for i in 0..ITEMS {
        append(&s.handle, tr[i % tr.len()], 30);
    }
    s
}

fn rename(h: &DocHandle, n: &mut u64) {
    *n += 1;
    h.submit(Submit::new(
        ui(),
        "project.rename",
        json!({"name": format!("p{n}")}),
    ))
    .wait()
    .unwrap();
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i]
}

fn report(name: &str, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    println!(
        "{name}: p50 {:?}, p95 {:?}, p99 {:?}, max {:?} over {} samples",
        percentile(&samples, 0.50),
        percentile(&samples, 0.95),
        percentile(&samples, 0.99),
        samples[samples.len() - 1],
        samples.len()
    );
}

fn journal(c: &mut Criterion) {
    let dir = TempDir::new("bench");

    // The raw device flush after a small append, on this machine's temp volume.
    let path = dir.0.join("fsync.bin");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    let record = vec![b'x'; 1500];
    c.bench_function("journal/fdatasync", |b| {
        b.iter(|| {
            file.write_all(&record).unwrap();
            file.sync_data().unwrap();
        });
    });
    let fsync: Vec<Duration> = (0..SAMPLES / 4)
        .map(|_| {
            file.write_all(&record).unwrap();
            let t = Instant::now();
            file.sync_data().unwrap();
            t.elapsed()
        })
        .collect();

    let mut lines = Vec::new();
    for (name, sync) in [
        ("rtt_no_fsync", SyncPolicy::Manual),
        ("rtt_group_commit_50ms", SyncPolicy::default()),
        ("rtt_fsync_every_commit", SyncPolicy::EveryCommit),
    ] {
        let s = session(&dir, name, sync);
        let mut n = 0;
        c.bench_function(&format!("journal/{name}"), |b| {
            b.iter(|| rename(&s.handle, &mut n));
        });
        let samples: Vec<Duration> = (0..SAMPLES)
            .map(|_| {
                let t = Instant::now();
                rename(&s.handle, &mut n);
                t.elapsed()
            })
            .collect();
        let stats = s.journal.stats();
        lines.push((format!("journal/{name}"), samples, stats));
    }
    report("journal/fdatasync (1.5 KB append)", fsync);
    for (name, samples, stats) in lines {
        report(&name, samples);
        println!(
            "  {name}: {} appends, {} bytes ({} B/record), {} fdatasyncs",
            stats.appends,
            stats.bytes,
            stats.bytes / stats.appends.max(1),
            stats.syncs
        );
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .without_plots()
        .sample_size(50)
        .measurement_time(Duration::from_secs(3));
    targets = journal
}
criterion_main!(benches);
