// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.06: a mini property test over random sequences of `clip.split`, `clip.trim`,
//! `item.move` and `item.delete` (the registry-driven harness is WP-1B.07). After every step the
//! document is valid, no track has overlapping items, linked groups stay inside one sequence,
//! and `H::run` has already asserted undo/redo identity for each success and "nothing changed"
//! for each failure.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, P, insert_av};
use cutweave_model::ItemId;
use proptest::prelude::*;
use serde_json::{Value, json};

static SUCCESSES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[derive(Clone, Debug)]
enum Op {
    Split {
        pick: usize,
        frame: u32,
    },
    Trim {
        pick: usize,
        end: bool,
        frame: u32,
        delta: bool,
        unlink: bool,
    },
    Move {
        pick: usize,
        frame: u32,
        track: usize,
        unlink: bool,
        new_track: bool,
    },
    Delete {
        picks: Vec<usize>,
    },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..64usize, 0..900u32).prop_map(|(pick, frame)| Op::Split { pick, frame }),
        (
            0..64usize,
            any::<bool>(),
            0..900u32,
            any::<bool>(),
            any::<bool>()
        )
            .prop_map(|(pick, end, frame, delta, unlink)| Op::Trim {
                pick,
                end,
                frame,
                delta,
                unlink
            }),
        (
            0..64usize,
            0..900u32,
            0..4usize,
            any::<bool>(),
            any::<bool>()
        )
            .prop_map(|(pick, frame, track, unlink, new_track)| Op::Move {
                pick,
                frame,
                track,
                unlink,
                new_track
            }),
        proptest::collection::vec(0..64usize, 1..4).prop_map(|picks| Op::Delete { picks }),
    ]
}

fn items(h: &H) -> Vec<ItemId> {
    let doc = h.doc();
    let mut out = Vec::new();
    for s in doc.sequences.values() {
        for t in s.tracks_in_order() {
            out.extend(t.items.iter().map(cutweave_model::Item::id));
        }
    }
    out
}

fn tracks(h: &H, p: P) -> Vec<cutweave_model::TrackId> {
    h.doc().sequences[&p.sequence]
        .tracks_in_order()
        .map(|t| t.id)
        .collect()
}

fn check_invariants(h: &H) {
    let doc = h.doc();
    doc.validate().unwrap();
    for s in doc.sequences.values() {
        for t in s.tracks.values() {
            assert_eq!(t.items.first_overlap(), None, "overlap on {}", t.id);
        }
    }
}

fn run_op(h: &mut H, p: P, op: &Op) {
    let all = items(h);
    let pick = |i: usize| all.get(i % all.len().max(1)).copied();
    let frame = |f: u32| Value::from(format!("f:{f}"));
    let r = match op {
        Op::Split { pick: i, frame: f } => {
            pick(*i).map(|item| h.run("clip.split", json!({"item": item, "at": frame(*f)})))
        }
        Op::Trim {
            pick: i,
            end,
            frame: f,
            delta,
            unlink,
        } => pick(*i).map(|item| {
            let mut params = json!({"item": item, "edge": if *end { "end" } else { "start" },
                                    "unlink": unlink});
            if *delta {
                params["delta"] = Value::from(i64::from(*f % 60) * 23_520_000 - 30 * 23_520_000);
            } else {
                params["to"] = frame(*f);
            }
            h.run("clip.trim", params)
        }),
        Op::Move {
            pick: i,
            frame: f,
            track,
            unlink,
            new_track,
        } => pick(*i).map(|item| {
            let ts = tracks(h, p);
            let mut params = json!({"item": item, "start": frame(*f), "unlink": unlink});
            if *track < ts.len() {
                params["to_track"] = json!(ts[*track]);
            }
            if *new_track {
                params["overlap"] = json!("new_track");
            }
            h.run("item.move", params)
        }),
        Op::Delete { picks } => {
            let ids: Vec<ItemId> = picks.iter().filter_map(|i| pick(*i)).collect();
            (!ids.is_empty()).then(|| h.run("item.delete", json!({"items": ids})))
        }
    };
    if matches!(r, Some(Ok(_))) {
        SUCCESSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    // Errors are fine (random args), but only the typed kinds a client can act on.
    if let Some(Err(e)) = r {
        assert!(
            matches!(e.code(), "invalid" | "not_found" | "locked" | "unsupported"),
            "unexpected error {e}"
        );
    }
    check_invariants(h);
}

fn start() -> (H, P) {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    for at in ["0s", "12s", "24s"] {
        insert_av(&mut h, p, m, at);
    }
    let img = h.add_image("/m/i.png");
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": img, "at": "40s", "duration": "3s"}),
    );
    h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    );
    (h, p)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn random_edit_sequences_keep_the_model_invariants(ops in proptest::collection::vec(op(), 1..25)) {
        let (mut h, p) = start();
        for op in &ops {
            run_op(&mut h, p, op);
        }
        prop_assert!(SUCCESSES.load(std::sync::atomic::Ordering::Relaxed) > 0, "the generator never produced a valid edit");
    }
}

#[test]
fn split_then_delete_then_undo_chain_restores_every_step() {
    // Deterministic chain: each step's inverse was already asserted by `H::run`; here the whole
    // chain is also undone newest-first from the recorded patches and must reach the start.
    let (mut h, _) = start();
    let initial = h.doc();
    let mut inverses = Vec::new();
    let v = items(&h)[0];
    for (name, params) in [
        ("clip.split", json!({"item": v, "at": "f:100"})),
        (
            "item.move",
            json!({"item": v, "start": "f:1500", "unlink": true}),
        ),
        (
            "clip.trim",
            json!({"item": v, "edge": "end", "delta": "-1s"}),
        ),
        ("item.delete", json!({"items": [v]})),
    ] {
        h.ok(name, params);
        inverses.push(h.last.as_ref().unwrap().inverse.clone());
    }
    let mut doc = h.doc();
    for inv in inverses.iter().rev() {
        doc = cutweave_commands::apply_patch(&doc, inv).unwrap();
    }
    assert_eq!(doc, initial);
}
