// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.03: the placement rules the spec fixtures (`spec_fixtures.rs`) do not pin down:
//! default lengths (INV-25), the mode alias (INV-40), linked audio on a chosen track (INV-45),
//! the main-track guard (INV-33), attachments (INV-36 to INV-38), `fine` edits (INV-26),
//! several items at once, duplicates' markers and children, range merging (INV-163), no-op
//! edits (INV-22) and determinism (INV-23). Every call goes through the common harness, which
//! also asserts undo/redo identity and that failures change nothing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::many_single_char_names)]

mod common;

use common::{H, P, assert_code, assert_invalid, id, item_id};
use cutweave_model::{FrameRate, ItemId, Marker, MarkerId, MediaId, Project, Time, TrackId};
use serde_json::{Value, json};

/// Flicks per frame at 30 fps (the harness project's rate).
const F: i64 = 23_520_000;

fn setup() -> (H, P, MediaId, MediaId) {
    let (mut h, p) = H::project();
    let video = h.add_video("/m/v.mp4", 60, true);
    let mute = h.add_video("/m/b.mp4", 60, false);
    (h, p, video, mute)
}

fn insert(h: &mut H, p: P, track: TrackId, media: MediaId, at: i64, len: i64) -> Value {
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": track, "media": media,
               "at": format!("f:{at}"), "duration": format!("f:{len}")}),
    )
}

fn clip(h: &mut H, p: P, track: TrackId, media: MediaId, at: i64, len: i64) -> ItemId {
    item_id(&insert(h, p, track, media, at, len)["clips"][0])
}

fn add_track(h: &mut H, p: P, kind: &str) -> TrackId {
    id(&h.ok("track.add", json!({"sequence": p.sequence, "kind": kind}))["track"])
}

/// `(start, duration)` in frames, and the track.
fn place(h: &H, item: ItemId) -> (TrackId, i64, i64) {
    let doc = h.doc();
    let at = doc.find_item(&item).unwrap();
    (
        at.track.id,
        at.item.start().flicks() / F,
        at.item.duration().flicks() / F,
    )
}

fn parent(h: &H, item: ItemId) -> Option<ItemId> {
    h.doc().find_item(&item).unwrap().item.attached_to()
}

/// Attaches `child` to `parent` in the document (WP-2A.05 adds the command).
fn attach(h: &mut H, child: ItemId, to: ItemId) {
    let mut doc: Project = h.doc();
    let at = doc.find_item(&child).unwrap();
    let (sequence, track) = (at.sequence.id, at.track.id);
    let seq = doc.sequences.get_mut(&sequence).unwrap();
    seq.track_mut(&track)
        .unwrap()
        .items
        .update(&child, |i| i.set_attached_to(Some(to)));
    doc.validate().unwrap();
    h.replace_doc(doc);
}

fn magnetic(h: &mut H, track: TrackId) {
    h.ok(
        "track.set_magnetic",
        json!({"track": track, "magnetic": true, "close_gaps": true}),
    );
}

// ----- timeline.insert -------------------------------------------------------------------------

#[test]
fn default_lengths_are_whole_frames_and_short_rests_are_refused() {
    let (mut h0, p) = H::project();
    let mut doc = h0.doc();
    doc.sequences
        .get_mut(&p.sequence)
        .unwrap()
        .format
        .frame_rate = FrameRate::FPS_29_97;
    h0.replace_doc(doc);
    let mut h = h0;
    let still = h.add_image("/m/i.png");
    let video = h.add_video("/m/v.mp4", 10, false);
    let rate = FrameRate::FPS_29_97;
    // INV-25: a still lasts 5 s snapped to the nearest frame (150 frames at 29.97 fps); the
    // rest of a video is floored (299 of 299.7 frames).
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": still, "at": 0}),
    );
    assert_eq!(
        out["duration"],
        json!(rate.frame_to_time(150).unwrap().flicks())
    );
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": video, "at": "f:200"}),
    );
    assert_eq!(
        out["duration"],
        json!(rate.frame_to_time(299).unwrap().flicks())
    );
    // Less than one frame left after the source position cannot be placed.
    assert_invalid(
        h.run(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": video, "at": "f:900",
                   "source_in": 10_i64 * 705_600_000 - 1000}),
        ),
        "/source_in",
    );
}

#[test]
fn mode_and_its_overlap_alias_must_agree() {
    let (mut h, p, _, mute) = setup();
    let base = json!({"sequence": p.sequence, "track": p.video, "media": mute, "at": 0,
                      "duration": "f:10"});
    let mut both = base.clone();
    both["mode"] = json!("insert");
    both["overlap"] = json!("reject");
    assert_invalid(h.run("timeline.insert", both), "/mode");
    let mut same = base;
    same["mode"] = json!("new_track");
    same["overlap"] = json!("new_track");
    h.ok("timeline.insert", same);
}

#[test]
fn linked_audio_goes_to_the_requested_track_only_when_it_is_free() {
    let (mut h, p, video, _) = setup();
    let music = h.add_audio("/m/music.wav", 60);
    let a2 = add_track(&mut h, p, "audio");
    clip(&mut h, p, a2, music, 0, 30);
    let params = |at: i64| {
        json!({"sequence": p.sequence, "track": p.video, "media": video,
               "at": format!("f:{at}"), "duration": "f:10", "audio_track": a2})
    };
    assert_invalid(h.run("timeline.insert", params(5)), "/audio_track");
    let out = h.ok("timeline.insert", params(40));
    assert_eq!(place(&h, item_id(&out["clips"][1])).0, a2);
    // An audio track of another kind or sequence is refused.
    let bad = json!({"sequence": p.sequence, "track": p.video, "media": video, "at": "f:60",
                     "duration": "f:10", "audio_track": p.video});
    assert_invalid(h.run("timeline.insert", bad), "/audio_track");
}

#[test]
fn insert_ripples_every_track_with_ripple_all_tracks() {
    let (mut h, p, _, mute) = setup();
    let v2 = add_track(&mut h, p, "video");
    let x = clip(&mut h, p, v2, mute, 0, 10);
    let y = clip(&mut h, p, v2, mute, 20, 10);
    let other = clip(&mut h, p, p.video, mute, 25, 10);
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": v2, "media": mute, "at": "f:15",
               "duration": "f:5", "mode": "insert", "ripple_all_tracks": true}),
    );
    assert_eq!(place(&h, x).1, 0);
    assert_eq!(place(&h, y).1, 25);
    assert_eq!(place(&h, other).1, 30, "V1 is rippled too");
}

// ----- INV-33: free-track ripples never move the main track ------------------------------------

#[test]
fn a_free_track_ripple_cannot_move_the_main_track() {
    let (mut h, p, _, mute) = setup();
    clip(&mut h, p, p.video, mute, 0, 30);
    let b = clip(&mut h, p, p.video, mute, 30, 30);
    magnetic(&mut h, p.video);
    let v2 = add_track(&mut h, p, "video");
    clip(&mut h, p, v2, mute, 0, 10);
    let y = clip(&mut h, p, v2, mute, 20, 10);
    let before = h.doc();
    let err = h
        .run(
            "timeline.ripple_delete",
            json!({"items": [y], "ripple_all_tracks": true}),
        )
        .unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(err.to_string().contains("main track"), "{err}");
    assert_eq!(h.doc(), before);
    // Without ripple_all_tracks the main track is not reached.
    h.ok("timeline.ripple_delete", json!({"items": [y]}));
    assert_eq!(place(&h, b).1, 30);
}

// ----- attachments (INV-36 to INV-38) ----------------------------------------------------------

#[test]
fn a_child_moved_alone_stays_attached_inside_its_parent_and_detaches_outside() {
    let (mut h, p, _, mute) = setup();
    let v2 = add_track(&mut h, p, "video");
    let parent_clip = clip(&mut h, p, p.video, mute, 0, 30);
    let child = clip(&mut h, p, v2, mute, 5, 5);
    attach(&mut h, child, parent_clip);
    h.ok("item.move", json!({"item": child, "start": "f:20"}));
    assert_eq!(parent(&h, child), Some(parent_clip));
    let out = h.ok("item.move", json!({"item": child, "start": "f:40"}));
    assert_eq!(parent(&h, child), None);
    assert_eq!(
        out["report"]["relinked"][0]["attached_to"],
        Value::Null,
        "the detachment is reported"
    );
}

#[test]
fn children_follow_a_moved_parent_and_stay_when_it_lands_on_their_track() {
    let (mut h, p, _, mute) = setup();
    let v2 = add_track(&mut h, p, "video");
    let parent_clip = clip(&mut h, p, p.video, mute, 0, 30);
    let child = clip(&mut h, p, v2, mute, 5, 5);
    attach(&mut h, child, parent_clip);
    h.ok("item.move", json!({"item": parent_clip, "start": "f:100"}));
    assert_eq!(place(&h, child), (v2, 105, 5));
    assert_eq!(parent(&h, child), Some(parent_clip));
    // Onto the child's own track: the child is detached and stays.
    h.ok(
        "item.move",
        json!({"item": parent_clip, "start": "f:200", "to_track": v2}),
    );
    assert_eq!(place(&h, child), (v2, 105, 5));
    assert_eq!(parent(&h, child), None);
}

#[test]
fn overwriting_a_parent_clamps_its_children_into_what_is_left() {
    let (mut h, p, _, mute) = setup();
    let v2 = add_track(&mut h, p, "video");
    let parent_clip = clip(&mut h, p, p.video, mute, 0, 30);
    let child = clip(&mut h, p, v2, mute, 25, 5);
    attach(&mut h, child, parent_clip);
    h.ok(
        "timeline.overwrite",
        json!({"sequence": p.sequence, "track": p.video, "media": mute, "at": "f:20",
               "duration": "f:20"}),
    );
    // The parent is [0, 20) now: the child is clamped to its last frame.
    assert_eq!(place(&h, parent_clip), (p.video, 0, 20));
    assert_eq!(place(&h, child).1, 19);
    assert_eq!(parent(&h, child), Some(parent_clip));
}

#[test]
fn a_child_on_a_locked_track_blocks_its_parent() {
    let (mut h, p, _, mute) = setup();
    let v2 = add_track(&mut h, p, "video");
    let parent_clip = clip(&mut h, p, p.video, mute, 0, 30);
    let child = clip(&mut h, p, v2, mute, 5, 5);
    attach(&mut h, child, parent_clip);
    h.ok("track.set", json!({"track": v2, "locked": true}));
    for (name, params) in [
        ("item.move", json!({"item": parent_clip, "start": "f:50"})),
        ("item.delete", json!({"items": [parent_clip]})),
    ] {
        let err = h.run(name, params).unwrap_err();
        assert_eq!(err.code(), "locked", "{name}: {err}");
        assert!(err.to_string().contains(&v2.to_string()), "{err}");
    }
}

// ----- fine edits (INV-26) ---------------------------------------------------------------------

#[test]
fn fine_edits_use_the_sample_grid_for_audio_only() {
    let (mut h, p, video, _) = setup();
    let music = h.add_audio("/m/music.wav", 60);
    let a = clip(&mut h, p, p.audio, music, 0, 90);
    // 1 s + 3 samples at 48 kHz (14,700 flicks per sample), not on the frame grid.
    let fine = 705_600_000 + 3 * 14_700;
    let out = h.ok(
        "clip.split",
        json!({"item": a, "at": fine + 100, "fine": true}),
    );
    assert_eq!(out["at"], json!(fine));
    let right = item_id(&out["right"]);
    h.ok(
        "item.move",
        json!({"item": right, "delta": 14_700, "fine": true}),
    );
    assert_eq!(
        h.doc().find_item(&right).unwrap().item.start().flicks(),
        fine + 14_700
    );
    // Video, or audio linked to video, is refused.
    let v = clip(&mut h, p, p.video, video, 40, 10);
    assert_invalid(
        h.run("clip.split", json!({"item": v, "at": "f:45", "fine": true})),
        "/fine",
    );
    assert_invalid(
        h.run(
            "item.move",
            json!({"item": v, "start": "f:60", "fine": true}),
        ),
        "/fine",
    );
}

// ----- several items -----------------------------------------------------------------------------

#[test]
fn moving_several_items_keeps_their_offsets() {
    let (mut h, p, _, mute) = setup();
    let x = clip(&mut h, p, p.video, mute, 0, 10);
    let y = clip(&mut h, p, p.video, mute, 20, 10);
    let out = h.ok("item.move", json!({"items": [y, x], "delta": "f:100"}));
    assert_eq!(out["items"][0]["item"], json!(y));
    assert_eq!((place(&h, x).1, place(&h, y).1), (100, 120));
    // Items of several tracks cannot go to one other track.
    let v2 = add_track(&mut h, p, "video");
    let z = clip(&mut h, p, v2, mute, 0, 10);
    assert_code(
        h.run(
            "item.move",
            json!({"items": [x, z], "start": 0, "to_track": v2}),
        ),
        "unsupported",
    );
    // Exactly one of item/items and of start/delta.
    assert_invalid(
        h.run("item.move", json!({"item": x, "items": [x], "start": 0})),
        "",
    );
    assert_invalid(
        h.run("item.move", json!({"item": x, "start": 0, "delta": 0})),
        "",
    );
}

#[test]
fn splitting_several_clips_at_once() {
    let (mut h, p, video, mute) = setup();
    let v2 = add_track(&mut h, p, "video");
    let (a, b) = (
        clip(&mut h, p, p.video, video, 0, 30),
        clip(&mut h, p, v2, mute, 0, 20),
    );
    let out = h.ok("clip.split", json!({"items": [a, b], "at": "f:10"}));
    // a's sound and b were split too; the report has all three.
    assert_eq!(out["linked"].as_array().unwrap().len(), 2);
    assert_eq!(out["report"]["split"].as_array().unwrap().len(), 3);
    assert_invalid(
        h.run("clip.split", json!({"items": [a, b], "at": "f:25"})),
        "/at",
    );
}

// ----- item.duplicate ------------------------------------------------------------------------

#[test]
fn duplicates_get_fresh_marker_ids_and_keep_children_with_copied_parents() {
    let (mut h, p, _, mute) = setup();
    let v2 = add_track(&mut h, p, "video");
    let parent_clip = clip(&mut h, p, p.video, mute, 0, 30);
    let child = clip(&mut h, p, v2, mute, 5, 5);
    attach(&mut h, child, parent_clip);
    let mut doc = h.doc();
    let seq = doc.sequences.get_mut(&p.sequence).unwrap();
    seq.track_mut(&p.video)
        .unwrap()
        .items
        .update(&parent_clip, |i| {
            let m = Marker::new(MarkerId::from_u128(77), Time::from_flicks(3 * F).unwrap());
            i.markers_mut().insert(m.id, m);
        });
    h.replace_doc(doc);
    let out = h.ok("item.duplicate", json!({"items": [parent_clip, child]}));
    let copies = out["copies"].as_array().unwrap();
    let copy_of = |src: ItemId| {
        copies
            .iter()
            .find(|c| item_id(&c["source"]) == src)
            .map(|c| item_id(&c["copy"]))
            .unwrap()
    };
    let (pc, cc) = (copy_of(parent_clip), copy_of(child));
    assert_eq!(place(&h, pc), (p.video, 30, 30));
    assert_eq!(place(&h, cc), (v2, 35, 5));
    assert_eq!(
        parent(&h, cc),
        Some(pc),
        "the child copy follows the parent copy"
    );
    let doc = h.doc();
    let marker = doc
        .find_item(&pc)
        .unwrap()
        .item
        .markers()
        .values()
        .next()
        .cloned();
    let marker = marker.unwrap();
    assert_ne!(marker.id, MarkerId::from_u128(77));
    assert_eq!(marker.t.flicks(), 3 * F);
    // An offset that would start before 0 is refused at `/offset`.
    assert_invalid(
        h.run(
            "item.duplicate",
            json!({"items": [child], "offset": -100 * F}),
        ),
        "/offset",
    );
}

#[test]
fn duplicate_on_the_main_track_refuses_an_offset() {
    let (mut h, p, _, mute) = setup();
    let a = clip(&mut h, p, p.video, mute, 0, 30);
    magnetic(&mut h, p.video);
    assert_invalid(
        h.run("item.duplicate", json!({"items": [a], "offset": "f:5"})),
        "/offset",
    );
}

// ----- lift, extract, ripple delete, delete gap ------------------------------------------------

#[test]
fn ranges_merge_and_default_to_the_in_out_range() {
    let (mut h, p, _, mute) = setup();
    let x = clip(&mut h, p, p.video, mute, 0, 100);
    assert_invalid(
        h.run("timeline.lift", json!({"sequence": p.sequence})),
        "/ranges",
    );
    let out = h.ok(
        "timeline.extract",
        json!({"sequence": p.sequence, "ranges": [
            {"start": "f:30", "end": "f:40"}, {"start": "f:10", "end": "f:20"},
            {"start": "f:15", "end": "f:30"}]}),
    );
    assert_eq!(
        out["ranges"],
        json!([{"start": 10 * F, "end": 40 * F}]),
        "sorted, overlapping and touching ranges merged (INV-163)"
    );
    assert_eq!(place(&h, x).2, 10);
    let (_, right_start, _) = place(&h, item_id(&out["report"]["split"][0]["right"]));
    assert_eq!(right_start, 10);
    h.ok(
        "timeline.set_in_out",
        json!({"sequence": p.sequence, "start": "f:2", "end": "f:4"}),
    );
    h.ok("timeline.lift", json!({"sequence": p.sequence}));
    assert_eq!(place(&h, x), (p.video, 0, 2));
    assert_invalid(
        h.run(
            "timeline.lift",
            json!({"sequence": p.sequence, "ranges": [{"start": "f:5", "end": "f:5"}]}),
        ),
        "/ranges/0",
    );
}

#[test]
fn ripple_delete_needs_one_track_and_delete_gap_can_ripple_all_tracks() {
    let (mut h, p, _, mute) = setup();
    let v2 = add_track(&mut h, p, "video");
    let x = clip(&mut h, p, v2, mute, 0, 10);
    let y = clip(&mut h, p, v2, mute, 20, 10);
    let z = clip(&mut h, p, p.video, mute, 40, 10);
    assert_invalid(
        h.run("timeline.ripple_delete", json!({"items": [x, z]})),
        "/items",
    );
    let out = h.ok(
        "timeline.delete_gap",
        json!({"track": v2, "at": "f:15", "ripple_all_tracks": true}),
    );
    assert_eq!(out["gap"], json!({"start": 10 * F, "end": 20 * F}));
    assert_eq!((place(&h, y).1, place(&h, z).1), (10, 30));
    assert_invalid(
        h.run("timeline.delete_gap", json!({"track": v2, "at": "f:5"})),
        "/at",
    );
}

// ----- INV-22 and INV-23 -------------------------------------------------------------------------

#[test]
fn a_move_back_to_where_it_was_changes_nothing() {
    let (mut h, p, video, _) = setup();
    let a = clip(&mut h, p, p.video, video, 0, 30);
    clip(&mut h, p, p.video, video, 30, 30);
    magnetic(&mut h, p.video);
    let out = h.ok("item.move", json!({"item": a, "start": 0}));
    assert_eq!(
        out["report"],
        serde_json::to_value(cutweave_ops::report::EditReport::default()).unwrap()
    );
    assert_eq!(h.last.as_ref().unwrap().patch.0.len(), 0);
}

#[test]
fn the_same_request_gives_the_same_result_and_ids() {
    let run = || {
        let (mut h, p, video, _) = setup();
        clip(&mut h, p, p.video, video, 0, 30);
        clip(&mut h, p, p.video, video, 30, 30);
        magnetic(&mut h, p.video);
        let out = h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": video, "at": "f:10",
                   "duration": "f:5"}),
        );
        (out, h.doc())
    };
    let (a, doc_a) = run();
    let (b, doc_b) = run();
    assert_eq!(a, b);
    assert_eq!(doc_a, doc_b);
}

#[test]
fn validate_reports_the_errors_apply_would() {
    let (mut h, p, _, mute) = setup();
    let x = clip(&mut h, p, p.video, mute, 0, 10);
    clip(&mut h, p, p.video, mute, 10, 10);
    let params = json!({"item": x, "start": "f:5"});
    let dry = h.validate("item.move", params.clone()).unwrap_err();
    let real = h.run("item.move", params).unwrap_err();
    assert_eq!(
        (dry.code(), dry.to_string()),
        (real.code(), real.to_string())
    );
}
