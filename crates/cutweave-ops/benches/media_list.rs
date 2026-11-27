// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1M.12 and WP-2F.07 benchmark: `media.list` with a filter (and each sort, and a folder
//! scope) over 10,000 assets in 200 folders. The WP budget is p95 < 20 ms per call, the page
//! included.
//!
//! WP-2F.06a (ADR-0036): the same library carries tags (every asset), ratings and notes, a
//! manual collection of 2,000 members and a smart one; `media.list` with tags and text, a
//! rating and a collection has a budget of p95 < 30 ms.
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
    Collection, CollectionId, CollectionKind, Duration as Flicks, FolderId, MediaAsset,
    MediaFilter, MediaFolder, MediaId, MediaKind, Project,
};
use cutweave_ops::media::FakeProbe;
use serde_json::{Value, json};

const ASSETS: u128 = 10_000;
/// 20 top-level folders with 9 subfolders each.
const FOLDERS: u128 = 200;
const SAMPLES: usize = 60;
const BUDGET: Duration = Duration::from_millis(20);
/// ADR-0036: tags and text over 10,000 assets.
const TAG_BUDGET: Duration = Duration::from_millis(30);

fn collection_id(n: u128) -> CollectionId {
    CollectionId::from_u128(0x0190_0000_0000_7000_8000_0000_0002_0000 + n)
}

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
    let mut members = Vec::new();
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
        // Stored order: sorted ignoring case and accents.
        asset.tags = match n % 4 {
            0 => vec!["b-roll".into(), format!("Scène {}", n % 25)],
            1 => vec![
                "Interview".into(),
                "keep".into(),
                format!("Scène {}", n % 25),
            ],
            2 => vec!["interview".into()],
            _ => vec!["b-roll".into(), "Wide".into()],
        };
        asset.rating = u8::try_from(n % 6).ok().filter(|r| *r > 0);
        if n % 10 == 0 {
            asset.note = Some(format!(
                "Take {n}: naïve first answer, good second half. Check the audio at the end."
            ));
        }
        if n % 5 == 0 {
            members.push(id);
        }
        doc.media.insert(id, asset);
    }
    doc.collections.insert(
        collection_id(1),
        Collection::new(
            collection_id(1),
            "Selects",
            CollectionKind::Manual { media: members },
        ),
    );
    let filter = MediaFilter {
        kinds: Some(vec![MediaKind::Video]),
        min_duration: Flicks::from_secs(60).ok(),
        tags: Some(vec!["interview".into()]),
        used: Some(false),
        ..MediaFilter::default()
    };
    doc.collections.insert(
        collection_id(2),
        Collection::new(
            collection_id(2),
            "Unused long interviews",
            CollectionKind::Smart { filter },
        ),
    );
    doc.validate()
        .expect("the bench library is a valid document");
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
    let (selects, smart) = (collection_id(1).to_string(), collection_id(2).to_string());
    let cases: [(&str, Value, Duration); 15] = [
        (
            "filter text",
            json!({"filter": {"text": "cafe 000"}}),
            BUDGET,
        ),
        (
            "tags",
            json!({"filter": {"tags": ["interview", "keep"]}}),
            TAG_BUDGET,
        ),
        (
            "tags+text (in the name)",
            json!({"filter": {"tags": ["interview"], "text": "cafe clip"}}),
            TAG_BUDGET,
        ),
        (
            "tags+text (in a tag)",
            json!({"filter": {"tags": ["b-roll"], "text": "scene 1 clip"}}),
            TAG_BUDGET,
        ),
        (
            "text in notes, no match in most",
            json!({"filter": {"text": "naive audio"}}),
            TAG_BUDGET,
        ),
        (
            "tags+text+rating, sort name",
            json!({"filter": {"tags": ["interview"], "text": "scene 1", "min_rating": 3},
                   "sort": {"by": "name"}, "include": ["note"]}),
            TAG_BUDGET,
        ),
        (
            "manual collection+text",
            json!({"filter": {"collection": selects, "text": "beach"}}),
            TAG_BUDGET,
        ),
        (
            "smart collection",
            json!({"filter": {"collection": smart}, "sort": {"by": "duration"}}),
            TAG_BUDGET,
        ),
        (
            "filter text+length",
            json!({"filter": {"text": "beach", "min_duration": 60 * 705_600_000_i64}}),
            BUDGET,
        ),
        ("sort name", json!({"sort": {"by": "name"}}), BUDGET),
        (
            "sort duration desc",
            json!({"sort": {"by": "duration", "order": "desc"}}),
            BUDGET,
        ),
        (
            "filter+sort name, page 2",
            json!({"filter": {"text": "clip"}, "sort": {"by": "name"}, "limit": 100,
                   "cursor": "med_01900000000070008000000000000063"}),
            BUDGET,
        ),
        (
            "folder recursive",
            json!({"filter": {"folder": shoot, "recursive": true}}),
            BUDGET,
        ),
        (
            "folder recursive+sort name",
            json!({"filter": {"folder": shoot, "recursive": true}, "sort": {"by": "name"}}),
            BUDGET,
        ),
        (
            "folder one level",
            json!({"filter": {"folder": day}}),
            BUDGET,
        ),
    ];
    let mut failed = false;
    for (name, params, budget) in cases {
        let mut times = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let t = Instant::now();
            let out = h.query("media.list", params.clone()).unwrap();
            times.push(t.elapsed());
            assert!(out["matched"].as_u64().unwrap() > 0);
        }
        times.sort();
        let (p50, p95) = (percentile(&times, 0.5), percentile(&times, 0.95));
        println!("media.list {name:<32} p50 {p50:>9.3?}  p95 {p95:>9.3?}  (budget {budget:?})");
        failed |= p95 > budget;
    }
    assert!(!failed, "a media.list p95 is over its budget");
}
