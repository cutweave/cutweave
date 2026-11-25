// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2I.03 acceptance: 50 checkpoints on a one-hour, 400-item project add less than 20 MB, and a
//! checkpoint shares the document's structure instead of copying it. Measured with the counting
//! allocator's heap balance on this thread (the document state runs here, with no actor
//! thread): the same edits run twice from the same project, with and without a checkpoint
//! after each round, and the difference in what stays allocated is the checkpoints' cost.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod journal_support;

use cutweave_audio::rt_alloc::{CountingAlloc, thread_heap_bytes};
use cutweave_commands::{ActorCtx, DocConfig, DocState, Submit};
use cutweave_model::{ItemId, Project};
use journal_support::{SEED, fixture, items, registry, tracks, ui};
use serde_json::{Value, json};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// Checkpoints in the run, one per round of edits.
const CHECKPOINTS: usize = 50;
/// Items in the project: 100 per track on 4 tracks.
const ITEMS: usize = 400;
/// 36 s clips: each track is an hour long.
const CLIP_FRAMES: u32 = 36 * 30;
const BUDGET: i64 = 20 << 20;

fn run(s: &mut DocState, ctx: &ActorCtx, name: &str, params: Value) -> Value {
    s.submit(Submit::new(ctx.clone(), name, params))
        .unwrap()
        .output
}

/// The one-hour, 400-item project.
fn hour_long_project() -> Project {
    let mut s = DocState::new(DocConfig::deterministic(registry(), fixture(4), SEED));
    let tr = tracks(s.committed_doc());
    for i in 0..ITEMS {
        run(
            &mut s,
            &ui(),
            "timeline.append_clip",
            json!({"track": tr[i % tr.len()], "frames": CLIP_FRAMES}),
        );
    }
    s.committed_doc().clone()
}

/// A round of typical edits: four renames across the timeline, a deletion and a new clip.
fn edit_round(s: &mut DocState, round: usize) {
    let all: Vec<ItemId> = items(s.committed_doc());
    let tr = tracks(s.committed_doc());
    for k in 0..4 {
        let item = all[(round * 37 + k * 101) % all.len()];
        run(
            s,
            &ui(),
            "item.rename",
            json!({"item": item, "name": format!("r{round}.{k}")}),
        );
    }
    let gone = all[(round * 53 + 7) % all.len()];
    run(s, &ui(), "item.delete", json!({"item": gone}));
    run(
        s,
        &ui(),
        "timeline.append_clip",
        json!({"track": tr[round % tr.len()], "frames": 90}),
    );
}

/// The heap retained by running every round on a fresh state over `doc` (and the state with
/// its history), with a checkpoint after each round when `checkpoints`. Also returns the most
/// any single `edit.checkpoint` call kept.
fn retained(doc: &Project, checkpoints: bool) -> (i64, i64, DocState) {
    // Another seed than the one that built doc: new ids must not collide with its items.
    let mut s = DocState::new(DocConfig::deterministic(registry(), doc.clone(), SEED + 1));
    let start = thread_heap_bytes();
    let mut largest = 0;
    for round in 0..CHECKPOINTS {
        edit_round(&mut s, round);
        if checkpoints {
            let before = thread_heap_bytes();
            // Every other one named, up to 20 (the named limit); the rest unnamed.
            let params = if round % 2 == 0 && round < 40 {
                json!({"name": format!("cp{round}")})
            } else {
                json!({})
            };
            run(&mut s, &ui(), "edit.checkpoint", params);
            largest = largest.max(thread_heap_bytes() - before);
        }
    }
    (thread_heap_bytes() - start, largest, s)
}

#[test]
fn fifty_checkpoints_on_an_hour_long_400_item_project_add_under_20_mb() {
    let doc = hour_long_project();
    assert_eq!(doc.item_count(), ITEMS);
    // What one independent copy of the document costs (nothing shared).
    let json = serde_json::to_vec(&doc).unwrap();
    let before = thread_heap_bytes();
    let copy: Project = serde_json::from_slice(&json).unwrap();
    let copy_cost = thread_heap_bytes() - before;
    drop(copy);

    let (without, _, plain) = retained(&doc, false);
    drop(plain);
    let (with, largest, mut s) = retained(&doc, true);
    let listed = run(&mut s, &ui(), "edit.list_checkpoints", json!({}));
    assert_eq!(listed["checkpoints"].as_array().unwrap().len(), CHECKPOINTS);
    s.verify().unwrap();

    let added = with - without;
    println!(
        "{CHECKPOINTS} checkpoints on {ITEMS} items: +{} KB retained (edits alone {} KB); \
         largest single checkpoint call {largest} B; one copy of the document {} KB",
        added / 1024,
        without / 1024,
        copy_cost / 1024
    );
    assert!(
        added < BUDGET,
        "{CHECKPOINTS} checkpoints added {added} bytes (budget {BUDGET})"
    );
    // Sharing, not copying: all 50 together cost under half of what 50 copies of the document
    // would, and one checkpoint call keeps a small fraction of a copy.
    assert!(
        added < copy_cost * 25,
        "{CHECKPOINTS} checkpoints kept {added} bytes; one copy is {copy_cost}"
    );
    assert!(
        largest * 10 < copy_cost,
        "a checkpoint kept {largest} bytes, one copy of the document is {copy_cost}: it copies \
         instead of sharing"
    );
}
