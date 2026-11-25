// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.03 acceptance (property): random sequences of placement commands over timelines with
//! a magnetic main track (or none), linked video and audio, overlays attached to main-track
//! clips, music, and sometimes a locked track. After every command:
//!
//! - a success undoes and redoes exactly and leaves a valid document (`H::run`: undo/redo
//!   identity, `Project::validate`, which includes INV-1, INV-2, INV-4 to INV-9: no overlaps, at
//!   most one magnetic track, gap-free from 0, aligned groups, valid attachments); a failure
//!   changes nothing (INV-20);
//! - no track has overlapping items, including auto-created ones, and no group has a single
//!   member (`check_invariants`);
//! - the items of a locked track did not change (INV-28) and the returned `report` is exactly
//!   the difference (INV-21), and linked partners moved, split and were deleted together
//!   (`check_linked_edit`);
//! - each sequence's duration is its largest item end (INV-10).
//!
//! `PROPTEST_CASES` sets the number of sequences (default 500; the nightly run uses 10,000).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::struct_excessive_bools,
    clippy::many_single_char_names
)]

mod common;

use common::{H, P, id, item_id};
use cutweave_model::{ItemId, Project, TrackId};
use cutweave_ops::testing::{arb_command_among, check_invariants, check_linked_edit};
use proptest::{
    collection::vec,
    prelude::*,
    strategy::ValueTree,
    test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
};
use serde_json::{Value, json};

/// The placement commands, plus what makes timelines interesting for them (locks, the magnet,
/// new tracks, trims).
const NAMES: &[&str] = &[
    "timeline.insert",
    "timeline.overwrite",
    "timeline.lift",
    "timeline.extract",
    "timeline.ripple_delete",
    "timeline.delete_gap",
    "item.move",
    "item.delete",
    "item.duplicate",
    "clip.split",
    "clip.trim",
    "track.set_magnetic",
    "track.set",
    "track.add",
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

/// How a case starts.
#[derive(Clone, Debug)]
struct Start {
    /// Main-track clips (lengths in frames, each with its sound).
    clips: Vec<i64>,
    /// Make V1 the magnetic main track.
    magnetic: bool,
    /// Overlays on V2: `(parent clip index, offset in frames, length)`.
    overlays: Vec<(usize, i64, i64)>,
    /// Attach the overlays to their main-track clips.
    attach: bool,
    /// A music clip on A2, and whether A2 is locked.
    music: bool,
    lock_music: bool,
}

fn arb_start() -> impl Strategy<Value = Start> {
    (
        vec(5..60_i64, 1..=4),
        prop_oneof![3 => Just(true), 1 => Just(false)],
        vec((0..4_usize, 0..20_i64, 3..15_i64), 0..=3),
        any::<bool>(),
        any::<bool>(),
        prop_oneof![3 => Just(false), 1 => Just(true)],
    )
        .prop_map(
            |(clips, magnetic, overlays, attach, music, lock_music)| Start {
                clips,
                magnetic,
                overlays,
                attach,
                music,
                lock_music,
            },
        )
}

fn start(s: &Start) -> H {
    let (mut h, p): (H, P) = H::project();
    let video = h.add_video("/m/v.mp4", 120, true);
    let mute = h.add_video("/m/b.mp4", 60, false);
    let music = h.add_audio("/m/music.wav", 120);
    let mut at = 0;
    let mut mains: Vec<(ItemId, i64, i64)> = Vec::new();
    for len in &s.clips {
        let out = h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": video,
                   "at": format!("f:{at}"), "duration": format!("f:{len}"),
                   "source_in": format!("f:{at}")}),
        );
        mains.push((item_id(&out["clips"][0]), at, *len));
        at += len;
    }
    if s.magnetic {
        h.ok(
            "track.set_magnetic",
            json!({"track": p.video, "magnetic": true}),
        );
    }
    let v2: TrackId = id(&h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    )["track"]);
    let mut children: Vec<(ItemId, ItemId)> = Vec::new();
    for (parent, offset, len) in &s.overlays {
        let (pid, pstart, plen) = mains[parent % mains.len()];
        let t = pstart + offset % plen;
        let r = h.run(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": v2, "media": mute,
                   "at": format!("f:{t}"), "duration": format!("f:{len}")}),
        );
        if let Ok(out) = r {
            children.push((item_id(&out["clips"][0]), pid));
        }
    }
    let a2: TrackId = id(&h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "audio"}),
    )["track"]);
    if s.music {
        h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": a2, "media": music, "at": "f:3",
                   "duration": format!("f:{}", at + 10)}),
        );
    }
    if s.lock_music {
        h.ok("track.set", json!({"track": a2, "locked": true}));
    }
    if !s.attach || children.is_empty() {
        return h;
    }
    // No command attaches yet (WP-2A.05): build the attachments in the document.
    let mut doc = h.doc();
    {
        let seq = doc.sequences.get_mut(&p.sequence).unwrap();
        let track = seq.track_mut(&v2).unwrap();
        for (child, parent) in &children {
            track
                .items
                .update(child, |i| i.set_attached_to(Some(*parent)));
        }
    }
    doc.validate().unwrap();
    h.replace_doc(doc);
    h
}

fn max_end(doc: &Project) -> Vec<i64> {
    doc.sequences
        .values()
        .map(|s| {
            s.tracks
                .values()
                .flat_map(|t| t.items.iter())
                .map(|i| i.start().flicks() + i.duration().flicks())
                .max()
                .unwrap_or(0)
        })
        .collect()
}

/// Runs one sequence; how many commands succeeded.
fn run_case(s: &Start, seeds: &[u64]) -> usize {
    let mut h = start(s);
    let mut ok = 0;
    for (n, seed) in seeds.iter().enumerate() {
        let before = h.doc();
        let (name, params) = generate(&h, &before, *seed);
        let result = h.run(&name, params.clone());
        let after = h.doc();
        let what = format!("step {n}: {name} {params}");
        check_invariants(&after).unwrap_or_else(|e| panic!("{what}: {e}"));
        let durations: Vec<i64> = after
            .sequences
            .values()
            .map(|s| s.duration().flicks())
            .collect();
        assert_eq!(durations, max_end(&after), "{what}: INV-10");
        match result {
            Ok(out) => {
                ok += 1;
                check_linked_edit(&name, &params, &out, &before, &after)
                    .unwrap_or_else(|e| panic!("{what}: {e}"));
            }
            Err(_) => assert_eq!(before, after, "{what}: failed but changed the document"),
        }
    }
    ok
}

#[test]
fn random_placements_keep_every_invariant() {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500);
    let config = Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::deterministic_rng(RngAlgorithm::ChaCha));
    let applied = std::cell::Cell::new(0_usize);
    let result = runner.run(&(arb_start(), vec(any::<u64>(), 1..=12)), |(s, seeds)| {
        applied.set(applied.get() + run_case(&s, &seeds));
        Ok(())
    });
    if let Err(e) = result {
        panic!("{e}");
    }
    // The sequences really edit: most requests are valid and many succeed.
    assert!(
        applied.get() >= cases as usize,
        "only {} commands succeeded in {cases} sequences",
        applied.get()
    );
}
