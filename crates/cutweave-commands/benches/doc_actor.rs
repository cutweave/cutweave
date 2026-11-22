// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.03 benchmark: document-actor round trip (submit → apply → publish → reply) and
//! snapshot loads, on a 400-item project with the production config (UUIDv7 ids, system
//! clock). Feeds the arch §19 command round-trip budget; the WP target is p95 < 1 ms for a
//! no-op command on the actor.
//!
//! `cargo bench -p cutweave-commands --bench doc_actor` (the p50/p95/p99 line is printed at
//! the end).

#![allow(missing_docs, clippy::unwrap_used)]

#[path = "../tests/doc_support/mod.rs"]
mod doc_support;

use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use cutweave_commands::{DocActor, DocConfig, DocHandle, Submit};
use doc_support::{append, project, registry, ui};
use serde_json::json;

const ITEMS: usize = 400;
const SAMPLES: usize = 20_000;

fn actor() -> (DocActor, DocHandle) {
    let (doc, tracks, _) = project(4);
    let actor = DocActor::spawn(DocConfig::new(registry(), doc)).unwrap();
    let h = actor.handle();
    for i in 0..ITEMS {
        append(&h, &ui(), tracks[i % tracks.len()], 30);
    }
    (actor, h)
}

fn noop(h: &DocHandle) {
    h.submit(Submit::new(ui(), "project.noop", json!({})))
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

fn doc_actor(c: &mut Criterion) {
    let (_actor, h) = actor();
    c.bench_function("doc_actor/noop_roundtrip", |b| b.iter(|| noop(&h)));
    let mut n = 0_u64;
    c.bench_function("doc_actor/rename_roundtrip", |b| {
        b.iter(|| {
            n += 1;
            h.submit(Submit::new(
                ui(),
                "project.rename",
                json!({"name": format!("p{n}")}),
            ))
            .wait()
            .unwrap()
        });
    });
    c.bench_function("doc_actor/snapshot_load", |b| b.iter(|| h.snapshot()));

    let mut samples: Vec<Duration> = (0..SAMPLES)
        .map(|_| {
            let t = Instant::now();
            noop(&h);
            t.elapsed()
        })
        .collect();
    samples.sort_unstable();
    let p95 = percentile(&samples, 0.95);
    println!(
        "doc_actor/noop_roundtrip latency over {SAMPLES} requests: p50 {:?}, p95 {p95:?}, p99 {:?}, max {:?} (target p95 < 1 ms: {})",
        percentile(&samples, 0.50),
        percentile(&samples, 0.99),
        samples[samples.len() - 1],
        if p95 < Duration::from_millis(1) {
            "met"
        } else {
            "MISSED"
        }
    );
}

criterion_group! {
    name = benches;
    config = Criterion::default().without_plots();
    targets = doc_actor
}
criterion_main!(benches);
