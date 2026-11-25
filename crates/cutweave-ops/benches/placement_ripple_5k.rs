// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.03 benchmark `placement_ripple_5k`: ripples on a 5,000-item sequence, where every
//! edit shifts nearly everything. Two layouts: 5,000 clips on the magnetic main track, and
//! 2,500 main-track clips with their linked sound (the ripple reaches the audio track). The WP
//! budget is < 5 ms for the ripple (checked on the median; the p95 is printed, and on a shared
//! machine it carries the noise of other work).
//!
//! Cases per layout: an insert at the start of the magnetic track (ripples every item right),
//! a delete of the first clip (closes the space) and a move of the first clip to the end (lift,
//! then insert). The **plan** is what `validate` computes over the snapshot (the ripple, cuts,
//! splits and every check); it has the budget. The **command** adds recording the patch in a
//! transaction, one op per shifted item; it is printed for information (the transaction
//! applies each op through the item's JSON form, which dominates at this size).
//!
//! `cargo bench -p cutweave-ops --bench placement_ripple_5k` prints p50/p95 per case and exits
//! non-zero when a plan median is over the budget.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::time::{Duration, Instant};

use cutweave_commands::{Access, QueryCtx, Registry, Transaction};
use cutweave_model::{
    Clip, Duration as Flicks, GroupId, Item, ItemId, MediaAsset, MediaId, MediaKind, Project,
    SeededGen, Sequence, SequenceFormat, SequenceId, Time, Track, TrackId, TrackKind,
};
use cutweave_ops::media::FakeProbe;
use serde_json::{Value, json};

const ITEMS: i64 = 5_000;
const SAMPLES: usize = 60;
/// Untimed runs first (allocator and caches).
const WARMUP: usize = 5;
/// The WP-2A.03 budget for a ripple (the plan over the snapshot).
const PLAN_BUDGET: Duration = Duration::from_millis(5);
/// Flicks per frame at 30 fps.
const FRAME: i64 = 23_520_000;

fn ids(n: u128) -> u128 {
    0x0190_0000_0000_7000_8000_0000_0000_0000 + n
}

/// A magnetic main track of `ITEMS` items (or half as many clips, each with linked sound).
fn timeline(linked: bool) -> (Project, SequenceId, TrackId, ItemId, MediaId) {
    let mut doc = Project::new("Bench");
    let media = MediaId::from_u128(ids(1));
    let mut probe = FakeProbe::video("/bench/v.mp4", 1, true).probe;
    probe.duration = Flicks::from_flicks(10 * FRAME).ok();
    doc.media.insert(
        media,
        MediaAsset::file(media, MediaKind::Video, "/bench/v.mp4", probe),
    );
    let sid = SequenceId::from_u128(ids(2));
    let mut seq = Sequence::new(sid, "S", SequenceFormat::default());
    let (v1, a1) = (TrackId::from_u128(ids(3)), TrackId::from_u128(ids(4)));
    let mut video = Track::new(v1, TrackKind::Video, "V1");
    video.magnetic = true;
    let mut audio = Track::new(a1, TrackKind::Audio, "A1");
    let clips = if linked { ITEMS / 2 } else { ITEMS };
    for n in 0..clips {
        let group = linked.then(|| GroupId::from_u128(ids(1_000_000 + n as u128)));
        let clip = |base: u128| {
            let mut clip = Clip::new(
                ItemId::from_u128(ids(base + n as u128)),
                media,
                Time::from_flicks(n * 10 * FRAME).unwrap(),
                Flicks::from_flicks(10 * FRAME).unwrap(),
            );
            clip.linked_group = group;
            Item::Clip(clip)
        };
        video.items.insert(clip(10_000_000));
        if linked {
            audio.items.insert(clip(20_000_000));
        }
    }
    seq.insert_track(0, video);
    seq.insert_track(1, audio);
    doc.sequences.insert(sid, seq);
    doc.validate().unwrap();
    (doc, sid, v1, ItemId::from_u128(ids(10_000_000)), media)
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    #[allow(clippy::cast_precision_loss)]
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i]
}

/// p50 and p95 of `SAMPLES` runs of `f`.
fn sample(mut f: impl FnMut()) -> (Duration, Duration) {
    for _ in 0..WARMUP {
        f();
    }
    let mut times = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let t0 = Instant::now();
        f();
        times.push(t0.elapsed());
    }
    times.sort();
    (percentile(&times, 0.5), percentile(&times, 0.95))
}

fn main() {
    let mut registry = Registry::new();
    cutweave_ops::register(&mut registry).unwrap();
    let mut over = false;
    for linked in [false, true] {
        let (doc, sequence, v1, first, media) = timeline(linked);
        let layout = if linked {
            "2,500 clips + linked sound"
        } else {
            "5,000 clips"
        };
        let cases: [(&str, &str, Value); 3] = [
            (
                "insert at 0",
                "timeline.insert",
                json!({"sequence": sequence, "track": v1, "media": media, "at": 0,
                       "duration": "f:5"}),
            ),
            ("delete the first", "item.delete", json!({"items": [first]})),
            (
                "move the first to the end",
                "item.move",
                json!({"item": first, "start": "f:999999"}),
            ),
        ];
        for (label, name, params) in cases {
            let ctx = QueryCtx::new(&doc, Access::STANDARD);
            let plan = sample(|| {
                registry
                    .validate_json(name, params.clone(), &ctx)
                    .unwrap_or_else(|e| panic!("{label}: {e}"));
            });
            let mut seeded = SeededGen::new(9);
            let command = sample(|| {
                let mut txn = Transaction::begin(doc.clone());
                let mut tx = txn.tx(&mut seeded, Access::STANDARD);
                registry
                    .invoke_json(name, params.clone(), &mut tx)
                    .unwrap_or_else(|e| panic!("{label}: {e}"));
            });
            println!(
                "{layout:<27} {label:<26} plan p50 {:>8.2?} p95 {:>8.2?}   command p50 {:>9.2?} p95 {:>9.2?}",
                plan.0, plan.1, command.0, command.1
            );
            over |= plan.0 > PLAN_BUDGET;
        }
    }
    if over {
        eprintln!("placement_ripple_5k: a plan median is over the {PLAN_BUDGET:?} budget");
        std::process::exit(1);
    }
}
