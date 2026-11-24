// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1M.12 benchmark: `media.list` with a filter (and each sort) over 10,000 assets. The WP
//! budget is p95 < 20 ms per call, the page included.
//!
//! `cargo bench -p cutweave-ops --bench media_list` prints p50/p95 per case and exits non-zero
//! when a p95 is over the budget.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation
)]

#[path = "../tests/common/mod.rs"]
mod common;

use std::time::{Duration, Instant};

use common::H;
use cutweave_model::{Duration as Flicks, MediaAsset, MediaId, MediaKind, Project};
use cutweave_ops::media::FakeProbe;
use serde_json::{Value, json};

const ASSETS: u128 = 10_000;
const SAMPLES: usize = 60;
const BUDGET: Duration = Duration::from_millis(20);

fn library() -> Project {
    let mut doc = Project::new("Bench");
    for n in 0..ASSETS {
        let path = format!(
            "/media/Shoot {}/clip_{n:05}_{}.mp4",
            n % 40,
            ["beach", "café", "city"][(n % 3) as usize]
        );
        let mut probe = FakeProbe::video(&path, 1, n % 2 == 0).probe;
        probe.duration = Flicks::from_secs(1 + (n % 600) as i64).ok();
        let id = MediaId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n);
        doc.media
            .insert(id, MediaAsset::file(id, MediaKind::Video, path, probe));
    }
    doc
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

fn main() {
    let h = H::with_doc(library());
    let cases: [(&str, Value); 5] = [
        ("filter text", json!({"filter": {"text": "cafe 000"}})),
        (
            "filter text+length",
            json!({"filter": {"text": "beach", "min_duration": 60 * 705_600_000_i64}}),
        ),
        ("sort name", json!({"sort": {"by": "name"}})),
        (
            "sort duration desc",
            json!({"sort": {"by": "duration", "order": "desc"}}),
        ),
        (
            "filter+sort name, page 2",
            json!({"filter": {"text": "clip"}, "sort": {"by": "name"}, "limit": 100,
                   "cursor": "med_01900000000070008000000000000063"}),
        ),
    ];
    let mut failed = false;
    for (name, params) in cases {
        let mut times = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let t = Instant::now();
            let out = h.query("media.list", params.clone()).unwrap();
            times.push(t.elapsed());
            assert!(out["matched"].as_u64().unwrap() > 0);
        }
        times.sort();
        let (p50, p95) = (percentile(&times, 0.5), percentile(&times, 0.95));
        println!("media.list {name:<28} p50 {p50:>9.3?}  p95 {p95:>9.3?}  (budget {BUDGET:?})");
        failed |= p95 > BUDGET;
    }
    assert!(!failed, "a media.list p95 is over {BUDGET:?}");
}
