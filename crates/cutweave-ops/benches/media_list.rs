// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1M.12 and WP-2F.07 benchmark: `media.list` with a filter (and each sort, and a folder
//! scope) over 10,000 assets in 200 folders. The WP budget is p95 < 20 ms per call, the page
//! included.
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
use cutweave_model::{
    Duration as Flicks, FolderId, MediaAsset, MediaFolder, MediaId, MediaKind, Project,
};
use cutweave_ops::media::FakeProbe;
use serde_json::{Value, json};

const ASSETS: u128 = 10_000;
/// 20 top-level folders with 9 subfolders each.
const FOLDERS: u128 = 200;
const SAMPLES: usize = 60;
const BUDGET: Duration = Duration::from_millis(20);

fn folder_id(n: u128) -> FolderId {
    FolderId::from_u128(0x0190_0000_0000_7000_8000_0000_0001_0000 + n)
}

fn library() -> Project {
    let mut doc = Project::new("Bench");
    for n in 0..FOLDERS {
        let (parent, name) = if n % 10 == 0 {
            (None, format!("Shoot {}", n / 10))
        } else {
            (Some(folder_id(n - n % 10)), format!("Day {}", n % 10))
        };
        doc.folders
            .insert(folder_id(n), MediaFolder::new(folder_id(n), name, parent));
    }
    for n in 0..ASSETS {
        let path = format!(
            "/media/Shoot {}/clip_{n:05}_{}.mp4",
            n % 40,
            ["beach", "café", "city"][(n % 3) as usize]
        );
        let mut probe = FakeProbe::video(&path, 1, n % 2 == 0).probe;
        probe.duration = Flicks::from_secs(1 + (n % 600) as i64).ok();
        let id = MediaId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n);
        let mut asset = MediaAsset::file(id, MediaKind::Video, path, probe);
        asset.folder = Some(folder_id(n % FOLDERS));
        doc.media.insert(id, asset);
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
    let shoot = folder_id(30).to_string();
    let day = folder_id(37).to_string();
    let cases: [(&str, Value); 8] = [
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
        (
            "folder recursive",
            json!({"filter": {"folder": shoot, "recursive": true}}),
        ),
        (
            "folder recursive+sort name",
            json!({"filter": {"folder": shoot, "recursive": true}, "sort": {"by": "name"}}),
        ),
        ("folder one level", json!({"filter": {"folder": day}})),
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
