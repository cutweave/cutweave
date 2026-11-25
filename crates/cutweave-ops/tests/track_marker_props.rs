// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.02 acceptance: for 1,000 random sequences of track, marker and in/out commands (mixed
//! with the P1 edits that make tracks gappy), after every command:
//!
//! - a success undoes and redoes exactly (`H::run` asserts `undo(apply(doc)) == doc`, redo and
//!   `Project::validate`), a failure changes nothing;
//! - the document survives serialize → deserialize and the `.cutweave` text round trip;
//! - every sequence has at most one magnetic track and it is gap-free (INV-6, INV-7, INV-281);
//! - track flags, heights, the display order, markers and the in/out range never change an
//!   item's track, start, duration or source (INV-29, INV-34).
//!
//! The registry-driven harness of WP-1B.07 (`registry_props.rs`) also drives these commands
//! together with every other one; this test concentrates on them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, P};
use cutweave_model::{Item, OpenOutcome, Project, TrackId, open_str, to_document_string};
use cutweave_ops::testing::arb_command_among;
use proptest::{
    collection::vec,
    prelude::*,
    strategy::ValueTree,
    test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
};
use serde_json::{Value, json};

/// The commands under test plus the P1 edits and inserts that create and remove items.
const NAMES: &[&str] = &[
    "track.add",
    "track.remove",
    "track.set",
    "track.reorder",
    "track.set_height",
    "track.set_magnetic",
    "track.list",
    "timeline.add_marker",
    "timeline.move_marker",
    "timeline.remove_marker",
    "timeline.set_in_out",
    "timeline.clear_in_out",
    "timeline.list_markers",
    "timeline.insert",
    "item.move",
    "item.delete",
    "clip.split",
    "clip.trim",
];

/// Commands that must leave every item's place and timing alone.
const ITEM_NEUTRAL: &[&str] = &[
    "track.set",
    "track.reorder",
    "track.set_height",
    "timeline.add_marker",
    "timeline.move_marker",
    "timeline.remove_marker",
    "timeline.set_in_out",
    "timeline.clear_in_out",
];

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The request `seed` picks for `doc`.
fn generate(h: &H, doc: &Project, seed: u64) -> (String, Value) {
    let mut bytes = [0_u8; 32];
    let mut s = seed;
    for chunk in bytes.chunks_mut(8) {
        s = splitmix(s);
        chunk.copy_from_slice(&s.to_le_bytes());
    }
    let config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
    arb_command_among(&h.registry, doc, NAMES)
        .new_tree(&mut runner)
        .expect("the strategies never reject")
        .current()
}

/// A project with a video with sound, a sound file and a still; two linked pairs and a still
/// on V1/A1, and a second video track with a clip.
fn start() -> (H, P) {
    let (mut h, p) = H::project();
    let video = h.add_video("/m/v.mp4", 30, true);
    let image = h.add_image("/m/i.png");
    for at in ["f:0", "f:90", "f:200"] {
        h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": video, "at": at,
                   "duration": "f:60"}),
        );
    }
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": image, "at": "f:300",
               "duration": "f:30"}),
    );
    let v2 = h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    );
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": v2["track"], "media": video, "at": "f:10",
               "duration": "f:40", "link": false}),
    );
    (h, p)
}

fn items(doc: &Project) -> Vec<(TrackId, Item)> {
    let mut v: Vec<_> = doc
        .sequences
        .values()
        .flat_map(|s| s.tracks.values())
        .flat_map(|t| {
            t.items.iter().map(|i| {
                // Item markers are metadata of the item, not part of its place and timing.
                let mut i = i.clone();
                i.markers_mut().clear();
                (t.id, i)
            })
        })
        .collect();
    v.sort_by_key(|(_, i)| i.id());
    v
}

fn check_after(name: &str, before: &Project, after: &Project) {
    for s in after.sequences.values() {
        let magnetic = s.tracks.values().filter(|t| t.magnetic).count();
        assert!(magnetic <= 1, "{name}: {magnetic} magnetic tracks");
    }
    after
        .validate()
        .unwrap_or_else(|e| panic!("{name}: the document is invalid: {e}"));
    let json = serde_json::to_string(after).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, after, "{name}: serde round trip");
    if ITEM_NEUTRAL.contains(&name) {
        assert_eq!(items(before), items(after), "{name} changed an item");
    }
}

/// Runs one sequence of commands; whether a main track existed at some point.
fn run_case(seeds: &[u64]) -> bool {
    let (mut h, _) = start();
    let mut magnetic_seen = false;
    for (n, seed) in seeds.iter().enumerate() {
        let before = h.doc();
        let (name, params) = generate(&h, &before, *seed);
        let result = h.run(&name, params);
        let after = h.doc();
        if result.is_err() {
            assert_eq!(
                before, after,
                "step {n}: {name} failed but changed the document"
            );
        } else {
            magnetic_seen |= after
                .sequences
                .values()
                .any(|s| s.tracks.values().any(|t| t.magnetic));
        }
        check_after(&name, &before, &after);
    }
    // The saved file round trips byte for byte.
    let doc = h.doc();
    let text = to_document_string(&doc).unwrap();
    let OpenOutcome::Editable { project, report } = open_str(&text).unwrap() else {
        panic!("a current document opens editable");
    };
    assert!(!report.migrated());
    assert_eq!(project, doc);
    assert_eq!(to_document_string(&project).unwrap(), text);
    magnetic_seen
}

#[test]
fn random_track_and_marker_commands_keep_the_document_valid_and_undoable() {
    let config = Config {
        cases: 1000,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::deterministic_rng(RngAlgorithm::ChaCha));
    let with_main_track = std::cell::Cell::new(0_usize);
    let result = runner.run(&vec(any::<u64>(), 1..=24), |seeds| {
        if run_case(&seeds) {
            with_main_track.set(with_main_track.get() + 1);
        }
        Ok(())
    });
    if let Err(e) = result {
        panic!("{e}");
    }
    // The sequences really exercise the magnet: many of them end up with a main track.
    let with_main_track = with_main_track.get();
    assert!(
        with_main_track >= 50,
        "only {with_main_track} of 1000 sequences made a track magnetic"
    );
}
