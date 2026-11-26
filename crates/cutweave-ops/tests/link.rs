// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.05 acceptance: linked clips and groups (`cargo test -p cutweave-ops link::`).
//!
//! - unit tests of `item.link`, `item.unlink`, `item.attach`, `item.detach` and
//!   `item.group_select` and of the group rules of the edits that already existed (move, delete,
//!   split, trim, duplicate, insert with `link`);
//! - the property test: random link, unlink, attach and detach commands mixed with the edits
//!   that move, trim, split, delete and duplicate, over timelines with linked video and audio,
//!   overlays attached to main-track clips and sometimes a locked track. After every command the
//!   members of a group have identical `(start, duration)`, every attached child starts inside
//!   its parent, a success undoes and redoes exactly, a failure changes nothing, and the rules
//!   of each command hold (`check_linked_edit`).
//!
//! `PROPTEST_CASES` sets the number of sequences (default 500; the nightly run uses 10,000).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::many_single_char_names
)]

mod common;

use common::{H, P, assert_code, assert_invalid, clip_of, id, insert_av, item_id, secs};
use cutweave_model::{ItemId, MediaId, Project, TrackId};
use cutweave_ops::testing::{arb_command_among, check_invariants, check_linked_edit};
use proptest::{
    collection::vec,
    prelude::*,
    strategy::ValueTree,
    test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
};
use serde_json::{Value, json};

mod link {
    use super::*;

    fn add_track(h: &mut H, p: P, kind: &str) -> TrackId {
        id(&h.ok("track.add", json!({"sequence": p.sequence, "kind": kind}))["track"])
    }

    /// A picture-only clip of `len` seconds at `at` on `track`.
    fn pic(h: &mut H, p: P, m: MediaId, track: TrackId, at: &str, len: i64) -> ItemId {
        item_id(
            &h.ok(
                "timeline.insert",
                json!({"sequence": p.sequence, "track": track, "media": m, "at": at,
                       "duration": format!("{len}s"), "link": false}),
            )["clips"][0],
        )
    }

    fn group(h: &H, item: ItemId) -> Option<cutweave_model::GroupId> {
        clip_of(h, item).linked_group
    }

    /// V1: `v` and A1: `a` (linked, 10 s at 2 s), plus an unlinked music clip `m` on A2 over the
    /// same range and an overlay `o` on V2 inside it.
    fn setup() -> (H, P, ItemId, ItemId, ItemId, ItemId) {
        let (mut h, p) = H::project();
        let media = h.add_video("/m/v.mp4", 10, true);
        let (v, a) = insert_av(&mut h, p, media, "2s");
        let still = h.add_image("/m/logo.png");
        let a2 = add_track(&mut h, p, "audio");
        let music = h.add_audio("/m/music.wav", 20);
        let m = item_id(
            &h.ok(
                "timeline.insert",
                json!({"sequence": p.sequence, "track": a2, "media": music, "at": "2s",
                       "duration": "10s"}),
            )["clips"][0],
        );
        let v2 = add_track(&mut h, p, "video");
        let o = pic(&mut h, p, still, v2, "4s", 3);
        (h, p, v, a, m, o)
    }

    #[test]
    fn insert_links_video_and_audio_unless_link_is_false() {
        let (mut h, p) = H::project();
        let media = h.add_video("/m/v.mp4", 10, true);
        let (v, a) = insert_av(&mut h, p, media, "0s");
        assert!(group(&h, v).is_some());
        assert_eq!(group(&h, v), group(&h, a));
        let out = h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": media, "at": "10s",
                   "link": false}),
        );
        assert_eq!(group(&h, item_id(&out["clips"][0])), None);
        assert_eq!(out["clips"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn link_joins_aligned_items_into_one_new_group() {
        let (mut h, _, v, a, m, _) = setup();
        let old = group(&h, v).unwrap();
        let out = h.ok("item.link", json!({"items": [v, m]}));
        let g = group(&h, m).unwrap();
        assert_ne!(g, old, "a new group, the old one no longer exists");
        assert_eq!(group(&h, v), Some(g));
        assert_eq!(group(&h, a), Some(g), "the partner came along");
        assert_eq!(out["members"].as_array().unwrap().len(), 3);
        assert_eq!(out["group"], json!(g));
        assert_eq!(out["report"]["relinked"].as_array().unwrap().len(), 3);
        // They now edit together.
        h.ok("item.move", json!({"item": m, "start": "5s"}));
        assert_eq!(clip_of(&h, v).start, secs(5));
        assert_eq!(clip_of(&h, a).start, secs(5));
    }

    #[test]
    fn link_needs_aligned_distinct_items() {
        let (mut h, _, v, _, m, o) = setup();
        assert_invalid(h.run("item.link", json!({"items": [v]})), "/items");
        assert_invalid(h.run("item.link", json!({"items": [v, v]})), "/items");
        assert_invalid(h.run("item.link", json!({"items": []})), "/items");
        // The overlay starts and ends elsewhere.
        assert_invalid(h.run("item.link", json!({"items": [m, o]})), "/items");
        assert_code(
            h.run(
                "item.link",
                json!({"items": [v, "itm_0000000000000000000000000000beef"]}),
            ),
            "not_found",
        );
    }

    #[test]
    fn link_refuses_items_with_different_parents() {
        let (mut h, p, v, _, _, o) = setup();
        let still = h.add_image("/m/other.png");
        let v3 = add_track(&mut h, p, "video");
        let twin = pic(&mut h, p, still, v3, "4s", 3);
        h.ok("item.attach", json!({"items": [o], "parent": v}));
        // `o` is attached, its aligned twin is not: the group could not satisfy INV-9.
        let before = h.doc();
        assert_invalid(h.run("item.link", json!({"items": [o, twin]})), "/items");
        assert_eq!(h.doc(), before);
        // Attached to the same parent, they link.
        h.ok("item.attach", json!({"items": [twin], "parent": v}));
        h.ok("item.link", json!({"items": [o, twin]}));
        assert_eq!(group(&h, o), group(&h, twin));
        // Attaching one member attaches its partners too.
        h.ok("item.detach", json!({"items": [twin]}));
        assert_eq!(clip_of(&h, o).attached_to, None);
        h.ok("item.attach", json!({"items": [o], "parent": v}));
        assert_eq!(clip_of(&h, twin).attached_to, Some(v));
    }

    #[test]
    fn unlink_then_move_affects_only_one_item() {
        let (mut h, _, v, a, _, _) = setup();
        h.ok("item.unlink", json!({"items": [v]}));
        assert_eq!(group(&h, v), None);
        assert_eq!(group(&h, a), None, "a pair dissolves");
        h.ok("item.move", json!({"item": v, "start": "8s"}));
        assert_eq!(clip_of(&h, v).start, secs(8));
        assert_eq!(clip_of(&h, a).start, secs(2), "the audio stayed");
    }

    #[test]
    fn unlink_takes_out_only_the_given_members_of_a_bigger_group() {
        let (mut h, _, v, a, m, _) = setup();
        h.ok("item.link", json!({"items": [v, m]}));
        let g = group(&h, v);
        h.ok("item.unlink", json!({"items": [m]}));
        assert_eq!(group(&h, m), None);
        assert_eq!(group(&h, v), g);
        assert_eq!(group(&h, a), g);
        // Unlinking an item that is not linked is a no-op that still succeeds.
        let out = h.ok("item.unlink", json!({"items": [m]}));
        assert_eq!(out["report"]["relinked"], json!([]));
    }

    #[test]
    fn edits_apply_to_every_member() {
        let (mut h, _, v, a, _, _) = setup();
        // Split, trim and delete go to both members.
        let out = h.ok("clip.split", json!({"item": v, "at": "5s"}));
        let (right_v, right_a) = (item_id(&out["right"]), item_id(&out["linked"][0]["right"]));
        assert_eq!(group(&h, right_v), group(&h, right_a));
        assert_ne!(
            group(&h, right_v),
            group(&h, v),
            "the right halves are a new group"
        );
        assert_eq!(clip_of(&h, a).duration, clip_of(&h, v).duration);
        h.ok(
            "clip.trim",
            json!({"item": a, "edge": "end", "delta": "-1s"}),
        );
        assert_eq!(clip_of(&h, v).duration, clip_of(&h, a).duration);
        h.ok("item.delete", json!({"items": [right_a]}));
        assert!(h.doc().find_item(&right_v).is_none());
        // Duplicate: the copy is a new group.
        let dup = h.ok("item.duplicate", json!({"items": [v], "offset": "20s"}));
        let copies: Vec<ItemId> = dup["copies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| item_id(&c["copy"]))
            .collect();
        assert_eq!(copies.len(), 2);
        assert_eq!(group(&h, copies[0]), group(&h, copies[1]));
        assert_ne!(group(&h, copies[0]), group(&h, v));
    }

    #[test]
    fn attach_makes_the_child_follow_and_detach_lets_go() {
        let (mut h, _, v, a, _, o) = setup();
        let out = h.ok("item.attach", json!({"items": [o], "parent": v}));
        assert_eq!(out["items"], json!([o]));
        assert_eq!(clip_of(&h, o).attached_to, Some(v));
        h.ok("item.move", json!({"item": v, "delta": "3s"}));
        assert_eq!(clip_of(&h, o).start, secs(7), "the child followed");
        assert_eq!(clip_of(&h, a).start, secs(5));
        h.ok("item.detach", json!({"items": [o]}));
        assert_eq!(clip_of(&h, o).attached_to, None);
        h.ok("item.move", json!({"item": v, "delta": "-3s"}));
        assert_eq!(clip_of(&h, o).start, secs(7), "a detached item stays");
    }

    #[test]
    fn attach_checks_everything_up_front() {
        let (mut h, _, v, a, m, o) = setup();
        // Not the item's own track, not itself, not its own group.
        assert_invalid(
            h.run("item.attach", json!({"items": [v], "parent": v})),
            "/items",
        );
        assert_invalid(
            h.run("item.attach", json!({"items": [v], "parent": a})),
            "/items",
        );
        // Starts outside the parent: the overlay is at 4 s, `o` is not moved to fit.
        h.ok("item.move", json!({"item": o, "start": "20s"}));
        let moved = h.doc();
        assert_invalid(
            h.run("item.attach", json!({"items": [o], "parent": v})),
            "/items",
        );
        assert_eq!(h.doc(), moved);
        assert_code(
            h.run(
                "item.attach",
                json!({"items": [o], "parent": "itm_0000000000000000000000000000beef"}),
            ),
            "not_found",
        );
        // One level deep: a child cannot be a parent, nor a parent a child.
        h.ok("item.move", json!({"item": o, "start": "4s"}));
        h.ok("item.attach", json!({"items": [o], "parent": v}));
        assert_invalid(
            h.run("item.attach", json!({"items": [m], "parent": o})),
            "/parent",
        );
        assert_invalid(
            h.run("item.attach", json!({"items": [v], "parent": m})),
            "/items",
        );
    }

    #[test]
    fn group_select_returns_every_member_once() {
        let (h, _, v, a, m, _) = setup();
        let out = h
            .query("item.group_select", json!({"items": [v, a, m]}))
            .unwrap();
        let ids: Vec<ItemId> = out["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| item_id(&x["item"]))
            .collect();
        assert_eq!(ids, [v, a, m]);
        let one = h.query("item.group_select", json!({"items": [a]})).unwrap();
        assert_eq!(one["items"].as_array().unwrap().len(), 2);
        assert!(h.query("item.group_select", json!({"items": []})).is_err());
    }

    #[test]
    fn item_get_shows_the_attachment_and_children() {
        let (mut h, _, v, _, _, o) = setup();
        h.ok("item.attach", json!({"items": [o], "parent": v}));
        let child = h.query("item.get", json!({"item": o})).unwrap();
        assert_eq!(child["attached_to"], json!(v));
        let parent = h.query("item.get", json!({"item": v})).unwrap();
        assert_eq!(parent["children"], json!([o]));
        assert!(parent.get("attached_to").is_none());
    }

    #[test]
    fn a_locked_track_blocks_the_link_commands() {
        let (mut h, _, v, a, m, _) = setup();
        let locked = h.doc().find_item(&m).unwrap().track.id;
        h.ok("track.set", json!({"track": locked, "locked": true}));
        let before = h.doc();
        assert_code(h.run("item.link", json!({"items": [v, m]})), "locked");
        assert_code(
            h.run("item.attach", json!({"items": [m], "parent": v})),
            "locked",
        );
        assert_eq!(h.doc(), before);
        // The unlocked pair can still be unlinked.
        h.ok("item.unlink", json!({"items": [v]}));
        assert_eq!(group(&h, a), None);
    }
}

// ----- the property test ---------------------------------------------------------------------

/// The link commands and the edits they interact with.
const NAMES: &[&str] = &[
    "item.link",
    "item.unlink",
    "item.attach",
    "item.detach",
    "item.move",
    "item.delete",
    "item.duplicate",
    "clip.split",
    "clip.trim",
    "clip.ripple_trim",
];

const LINK_COMMANDS: &[&str] = &["item.link", "item.unlink", "item.attach", "item.detach"];

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

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
    /// Picture-only clips on V2 and V3 and music on A2: `(track, start, length)` in frames.
    others: Vec<(u8, i64, i64)>,
    /// Attach every overlay that lies inside a main-track clip to it.
    attach: bool,
    /// Lock A2.
    lock_music: bool,
}

fn arb_start() -> impl Strategy<Value = Start> {
    (
        vec((5..60_i64, 0..60_i64), 1..=4),
        vec((0..3_u8, 0..120_i64, 3..40_i64), 0..=5),
        any::<bool>(),
        prop_oneof![4 => Just(false), 1 => Just(true)],
    )
        .prop_map(|(clips, others, attach, lock_music)| Start {
            clips,
            others,
            attach,
            lock_music,
        })
}

fn start(s: &Start) -> H {
    let (mut h, p): (H, P) = H::project();
    let video = json!(h.add_video("/m/v.mp4", 120, true));
    let mute = json!(h.add_video("/m/b.mp4", 60, false));
    let music = json!(h.add_audio("/m/music.wav", 120));
    let mut at = 0;
    let mut mains: Vec<(ItemId, i64, i64)> = Vec::new();
    for (len, src) in &s.clips {
        let out = h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": video,
                   "at": format!("f:{at}"), "duration": format!("f:{len}"),
                   "source_in": format!("f:{src}")}),
        );
        mains.push((item_id(&out["clips"][0]), at, *len));
        at += len;
    }
    let tracks: Vec<TrackId> = ["video", "video", "audio"]
        .iter()
        .map(|kind| id(&h.ok("track.add", json!({"sequence": p.sequence, "kind": kind}))["track"]))
        .collect();
    let mut overlays: Vec<ItemId> = Vec::new();
    for (which, from, len) in &s.others {
        let track = tracks[usize::from(*which)];
        let media = if *which == 2 { &music } else { &mute };
        let r = h.run(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": track, "media": media,
                   "at": format!("f:{from}"), "duration": format!("f:{len}"), "link": false}),
        );
        if let Ok(out) = r
            && *which < 2
        {
            overlays.push(item_id(&out["clips"][0]));
        }
    }
    if s.attach {
        for o in overlays {
            let doc = h.doc();
            let at_o = doc.find_item(&o).unwrap().item.start().flicks();
            let parent = mains.iter().find(|(pid, _, _)| {
                let it = doc.find_item(pid).unwrap().item;
                it.start().flicks() <= at_o && at_o < it.start().flicks() + it.duration().flicks()
            });
            if let Some((parent, _, _)) = parent {
                let _ = h.run("item.attach", json!({"items": [o], "parent": parent}));
            }
        }
    }
    if s.lock_music {
        h.ok("track.set", json!({"track": tracks[2], "locked": true}));
    }
    h
}

/// INV-8 and INV-9 stated directly (`Project::validate` checks them too).
fn check_groups_and_children(doc: &Project) -> Result<(), String> {
    for s in doc.sequences.values() {
        let items: Vec<_> = s.tracks.values().flat_map(|t| t.items.iter()).collect();
        for i in &items {
            if let Some(g) = i.linked_group() {
                for o in items.iter().filter(|o| o.linked_group() == Some(g)) {
                    if (o.start(), o.duration()) != (i.start(), i.duration()) {
                        return Err(format!(
                            "group {g}: {} and {} are not aligned",
                            i.id(),
                            o.id()
                        ));
                    }
                }
            }
            if let Some(p) = i.attached_to() {
                let parent = items
                    .iter()
                    .find(|o| o.id() == p)
                    .ok_or(format!("{} is attached to a missing {p}", i.id()))?;
                let inside = parent.start() <= i.start()
                    && i.start().flicks() < parent.start().flicks() + parent.duration().flicks();
                if !inside || parent.attached_to().is_some() {
                    return Err(format!("{} is attached to {p} but not inside it", i.id()));
                }
            }
        }
    }
    Ok(())
}

/// Runs one sequence; how many link commands succeeded.
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
        check_groups_and_children(&after).unwrap_or_else(|e| panic!("{what}: {e}"));
        match result {
            Ok(out) => {
                if LINK_COMMANDS.contains(&name.as_str()) {
                    ok += 1;
                }
                check_linked_edit(&name, &params, &out, &before, &after)
                    .unwrap_or_else(|e| panic!("{what}: {e}\noutput {out}"));
            }
            Err(_) => assert_eq!(before, after, "{what}: failed but changed the document"),
        }
    }
    ok
}

#[test]
fn random_links_and_attachments_keep_every_invariant() {
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
    let result = runner.run(&(arb_start(), vec(any::<u64>(), 1..=10)), |(s, seeds)| {
        applied.set(applied.get() + run_case(&s, &seeds));
        Ok(())
    });
    if let Err(e) = result {
        panic!("{e}");
    }
    // The commands really link: many of the requests succeed.
    assert!(
        applied.get() >= cases as usize / 2,
        "only {} link commands succeeded in {cases} sequences",
        applied.get()
    );
}
