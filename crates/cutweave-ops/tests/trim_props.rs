// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.04 acceptance (property): random sequences of trim-family commands (`clip.trim`,
//! `clip.ripple_trim`, `clip.roll`, `clip.slip`, `clip.slide`, mixed with splits and moves that
//! reshape the timeline) over timelines with a magnetic main track (or none), linked video and
//! audio, clips butted together and with gaps on a free track, stills, overlays attached to
//! main-track clips, music, and sometimes a locked track. After every command:
//!
//! - a success undoes and redoes exactly and leaves a valid document (`H::run`: undo/redo
//!   identity and `Project::validate`, which includes INV-1, 2, 4 to 9: no overlaps, source
//!   ranges inside the media, the main track gap-free from 0, aligned groups, valid
//!   attachments); a failure changes nothing (INV-20);
//! - every video item is on the frame grid (INV-3) and each sequence lasts until its last item
//!   ends (INV-10);
//! - the report is the difference, locked tracks are unchanged (INV-21, INV-28), linked
//!   partners were edited together, and the rules of each command hold with the returned
//!   `applied_delta`: slip never changes a start or a length and only the clip's group's
//!   source (INV-231); roll keeps both outer edges and the sum of the two lengths (INV-221);
//!   slide keeps the clip's length and content and its neighbours' outer edges (INV-241); a
//!   ripple trim moves everything downstream by exactly the change of the clip's end and
//!   never its start (INV-211, INV-212); a trim changes lengths by exactly the delta (INV-202)
//!   (`check_linked_edit`).
//!
//! WP-2D.01 extends it with `clip.set_speed`, `clip.reverse`, `clip.reset_speed` and
//! `clip.freeze_frame` (their rules: `check_linked_edit`), so the trims, splits and ripples run on
//! clips at every speed, reversed and frozen, and the undo identity and the gapless main track
//! hold with them.
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
use cutweave_model::{ItemId, Project, TrackId, TrackKind};
use cutweave_ops::testing::{arb_command_among, check_invariants, check_linked_edit};
use proptest::{
    collection::vec,
    prelude::*,
    strategy::ValueTree,
    test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
};
use serde_json::{Value, json};

/// The trim family, plus splits and moves so the timelines they edit keep changing, and (WP-2D.01)
/// speed changes, reverse and freeze frames, so trims run on clips at every speed.
const NAMES: &[&str] = &[
    "clip.trim",
    "clip.ripple_trim",
    "clip.roll",
    "clip.slip",
    "clip.slide",
    "clip.split",
    "item.move",
    "clip.set_speed",
    "clip.reverse",
    "clip.reset_speed",
    "clip.freeze_frame",
];

/// The speed commands (WP-2D.01).
const SPEED: &[&str] = &[
    "clip.set_speed",
    "clip.reverse",
    "clip.reset_speed",
    "clip.freeze_frame",
];

/// The trim family (counted for the "they really edit" check).
const FAMILY: &[&str] = &[
    "clip.trim",
    "clip.ripple_trim",
    "clip.roll",
    "clip.slip",
    "clip.slide",
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
    /// Main-track clips with their sound: `(length, source_in)` in frames.
    clips: Vec<(i64, i64)>,
    /// Make V1 the magnetic main track.
    magnetic: bool,
    /// Picture-only clips on V2: `(gap before it, length, source_in, still)` in frames.
    free: Vec<(i64, i64, i64, bool)>,
    /// Overlays on V3: `(parent clip index, offset in frames, length)`.
    overlays: Vec<(usize, i64, i64)>,
    /// Attach the overlays to their main-track clips.
    attach: bool,
    /// A music clip on A2, and whether A2 is locked.
    music: bool,
    lock_music: bool,
}

fn arb_start() -> impl Strategy<Value = Start> {
    (
        vec((5..60_i64, 0..120_i64), 1..=4),
        prop_oneof![3 => Just(true), 1 => Just(false)],
        vec(
            (
                prop_oneof![2 => Just(0_i64), 1 => 1..20_i64],
                3..40_i64,
                0..150_i64,
                prop_oneof![5 => Just(false), 1 => Just(true)],
            ),
            0..=4,
        ),
        vec((0..4_usize, 0..20_i64, 3..15_i64), 0..=2),
        any::<bool>(),
        any::<bool>(),
        prop_oneof![3 => Just(false), 1 => Just(true)],
    )
        .prop_map(
            |(clips, magnetic, free, overlays, attach, music, lock_music)| Start {
                clips,
                magnetic,
                free,
                overlays,
                attach,
                music,
                lock_music,
            },
        )
}

fn insert(h: &mut H, p: P, track: TrackId, media: &Value, at: i64, len: i64, src: i64) -> Value {
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": track, "media": media,
               "at": format!("f:{at}"), "duration": format!("f:{len}"),
               "source_in": format!("f:{src}")}),
    )
}

fn start(s: &Start) -> H {
    let (mut h, p): (H, P) = H::project();
    let video = json!(h.add_video("/m/v.mp4", 120, true));
    let mute = json!(h.add_video("/m/b.mp4", 60, false));
    let still = json!(h.add_image("/m/logo.png"));
    let music = h.add_audio("/m/music.wav", 120);
    let mut at = 0;
    let mut mains: Vec<(ItemId, i64, i64)> = Vec::new();
    for (len, src) in &s.clips {
        let out = insert(&mut h, p, p.video, &video, at, *len, *src);
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
    let mut t = 0;
    for (gap, len, src, is_still) in &s.free {
        t += gap;
        let (media, src) = if *is_still {
            (&still, 0)
        } else {
            (&mute, *src)
        };
        insert(&mut h, p, v2, media, t, *len, src);
        t += len;
    }
    let v3: TrackId = id(&h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    )["track"]);
    let mut children: Vec<(ItemId, ItemId)> = Vec::new();
    for (parent, offset, len) in &s.overlays {
        let (pid, pstart, plen) = mains[parent % mains.len()];
        let at = pstart + offset % plen;
        let r = h.run(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": v3, "media": mute,
                   "at": format!("f:{at}"), "duration": format!("f:{len}")}),
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
    if s.attach && !children.is_empty() {
        // Attachments are built in the document (`item.attach` is WP-2A.05's).
        let mut doc = h.doc();
        let track = doc
            .sequences
            .get_mut(&p.sequence)
            .unwrap()
            .track_mut(&v3)
            .unwrap();
        for (child, parent) in &children {
            track
                .items
                .update(child, |i| i.set_attached_to(Some(*parent)));
        }
        doc.validate().unwrap();
        h.replace_doc(doc);
    }
    h
}

/// INV-3 on video tracks (audio items may sit on the sample grid after a `fine` edit), and
/// INV-10.
fn check_grid_and_durations(doc: &Project) -> Result<(), String> {
    for s in doc.sequences.values() {
        let fpf = s.format.frame_rate.flicks_per_frame().unwrap();
        let mut end = 0;
        for t in s.tracks.values() {
            for i in t.items.iter() {
                let (start, len) = (i.start().flicks(), i.duration().flicks());
                end = end.max(start + len);
                if t.kind == TrackKind::Video && (start % fpf != 0 || len % fpf != 0) {
                    return Err(format!("INV-3: {} is off the frame grid", i.id()));
                }
            }
        }
        if s.duration().flicks() != end {
            return Err(format!(
                "INV-10: the sequence lasts {}",
                s.duration().flicks()
            ));
        }
    }
    Ok(())
}

/// Runs one sequence; how many trim-family and speed commands succeeded.
fn run_case(s: &Start, seeds: &[u64]) -> (usize, usize) {
    let mut h = start(s);
    let (mut ok, mut sped) = (0, 0);
    for (n, seed) in seeds.iter().enumerate() {
        let before = h.doc();
        let (name, params) = generate(&h, &before, *seed);
        let result = h.run(&name, params.clone());
        let after = h.doc();
        let what = format!("step {n}: {name} {params}");
        check_invariants(&after).unwrap_or_else(|e| panic!("{what}: {e}"));
        check_grid_and_durations(&after).unwrap_or_else(|e| panic!("{what}: {e}"));
        match result {
            Ok(out) => {
                if FAMILY.contains(&name.as_str()) {
                    ok += 1;
                }
                if SPEED.contains(&name.as_str()) {
                    sped += 1;
                }
                check_linked_edit(&name, &params, &out, &before, &after)
                    .unwrap_or_else(|e| panic!("{what}: {e}\noutput {out}"));
            }
            Err(_) => assert_eq!(before, after, "{what}: failed but changed the document"),
        }
    }
    (ok, sped)
}

#[test]
fn random_trims_keep_every_invariant() {
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
    let sped = std::cell::Cell::new(0_usize);
    let result = runner.run(&(arb_start(), vec(any::<u64>(), 1..=10)), |(s, seeds)| {
        let (ok, speed) = run_case(&s, &seeds);
        applied.set(applied.get() + ok);
        sped.set(sped.get() + speed);
        Ok(())
    });
    if let Err(e) = result {
        panic!("{e}");
    }
    // The sequences really trim: many trim-family requests succeed.
    assert!(
        applied.get() >= cases as usize,
        "only {} trim-family commands succeeded in {cases} sequences",
        applied.get()
    );
    assert!(
        sped.get() >= cases as usize / 2,
        "only {} speed commands succeeded in {cases} sequences",
        sped.get()
    );
}
