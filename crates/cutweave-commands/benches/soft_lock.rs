// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.12 benchmark: the soft-lock check every command runs before it is applied
//! ([`DocState::check_locks`]), on a 5,000-item document while the person's gesture locks
//! every item (a ripple across all four tracks). The budget is < 20 µs per command.
//!
//! `cargo bench -p cutweave-commands --bench soft_lock` (a summary line with the budget verdict
//! is printed at the end).

#![allow(missing_docs, clippy::unwrap_used)]

#[path = "../tests/doc_support/mod.rs"]
mod doc_support;

use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use cutweave_commands::{DocPath, DocState, Patch, PatchOp, Submit, TxKind};
use cutweave_model::{ItemId, SequenceId, TrackId};
use doc_support::{agent, config, project, ui};
use serde_json::json;

const ITEMS: usize = 5_000;
const TRACKS: usize = 4;
const BUDGET: Duration = Duration::from_micros(20);

struct Setup {
    state: DocState,
    sequence: SequenceId,
    tracks: Vec<TrackId>,
    /// One item per track.
    samples: Vec<ItemId>,
}

/// 5,000 one-frame clips on four tracks; the person's open gesture shifts (and so locks) all
/// of them.
fn setup() -> Setup {
    let (doc, tracks, _) = project(TRACKS);
    let sequence = *doc.sequences.keys().next().unwrap();
    let mut state = DocState::new(config(doc));
    for i in 0..ITEMS {
        state
            .submit(Submit::new(
                ui(),
                "timeline.append_clip",
                json!({"track": tracks[i % TRACKS], "frames": 1}),
            ))
            .unwrap();
    }
    let gesture = state.begin_tx(&ui(), TxKind::Gesture, "Ripple").unwrap().id;
    for track in &tracks {
        state
            .submit(
                Submit::new(
                    ui(),
                    "timeline.shift_after",
                    json!({"track": track, "at": 0, "frames": 1}),
                )
                .in_tx(gesture),
            )
            .unwrap();
    }
    assert_eq!(state.locks(None).transactions[0].items.len(), ITEMS);
    let samples = tracks
        .iter()
        .map(|t| {
            *state.committed_doc().sequences[&sequence].tracks[t]
                .items
                .iter_by_id()
                .next()
                .unwrap()
                .0
        })
        .collect();
    Setup {
        state,
        sequence,
        tracks,
        samples,
    }
}

fn rename(path: &DocPath) -> Patch {
    Patch(vec![PatchOp::Replace {
        path: path.join("name"),
        old: json!(null),
        value: json!("x"),
    }])
}

fn soft_lock(c: &mut Criterion) {
    let setup = setup();
    let s = &setup.state;
    let bot = agent();
    let actor = bot.actor();
    let (seq, track, item) = (setup.sequence, setup.tracks[1], setup.samples[1]);
    // A command on a locked item (refused), above locked items (refused), and the person's
    // own actor (never blocked: the cheapest path).
    let on_item = rename(&DocPath::item(seq, track, item));
    let above = Patch(vec![PatchOp::Replace {
        path: DocPath::track(seq, track),
        old: json!(null),
        value: json!(null),
    }]);
    // A typical clip edit: four field ops on one item.
    let mut edit = rename(&DocPath::item(seq, track, item));
    for field in ["start", "duration", "source_in"] {
        edit.0.push(PatchOp::Replace {
            path: DocPath::item(seq, track, item).join(field),
            old: json!(0),
            value: json!(1),
        });
    }
    assert!(s.check_locks(actor, &on_item).is_err());
    assert!(s.check_locks(actor, &above).is_err());
    assert!(s.check_locks(ui().actor(), &on_item).is_ok());

    c.bench_function("soft_lock/refused_item_edit_5000_locked", |b| {
        b.iter(|| s.check_locks(actor, &on_item));
    });
    c.bench_function("soft_lock/refused_track_change_5000_locked", |b| {
        b.iter(|| s.check_locks(actor, &above));
    });
    c.bench_function("soft_lock/clip_edit_4_ops_5000_locked", |b| {
        b.iter(|| s.check_locks(actor, &edit));
    });
    c.bench_function("soft_lock/holder_own_edit", |b| {
        b.iter(|| s.check_locks(ui().actor(), &edit));
    });

    // The budget: the slowest case, per check, over many runs.
    let worst = [&on_item, &above, &edit]
        .iter()
        .map(|patch| {
            let runs = 20_000_u32;
            let t = Instant::now();
            for _ in 0..runs {
                let _ = std::hint::black_box(s.check_locks(actor, patch));
            }
            t.elapsed() / runs
        })
        .max()
        .unwrap();
    println!(
        "soft_lock/check on {ITEMS} items, all locked: worst mean {worst:?} per command (budget < {BUDGET:?}: {})",
        if worst < BUDGET { "met" } else { "MISSED" }
    );
}

criterion_group! {
    name = benches;
    config = Criterion::default().without_plots();
    targets = soft_lock
}
criterion_main!(benches);
