// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.05: `track.add`, `track.remove`, `track.set`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names
)]

mod common;

use common::{H, assert_code, assert_invalid, id, item_id};
use cutweave_model::{GroupId, ItemId, SequenceId, TrackId, TrackKind};
use serde_json::json;

fn order(h: &H, seq: SequenceId) -> Vec<(TrackKind, String)> {
    h.doc().sequences[&seq]
        .tracks_in_order()
        .map(|t| (t.kind, t.name.clone()))
        .collect()
}

fn names(h: &H, seq: SequenceId) -> Vec<String> {
    order(h, seq).into_iter().map(|(_, n)| n).collect()
}

// ----- track.add ----------------------------------------------------------------------------

#[test]
fn add_puts_video_on_top_and_audio_at_the_bottom() {
    let (mut h, p) = H::project();
    h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    );
    h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "audio"}),
    );
    assert_eq!(names(&h, p.sequence), ["V2", "V1", "A1", "A2"]);
}

#[test]
fn add_honours_name_and_index() {
    let (mut h, p) = H::project();
    let out = h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "audio", "name": " Music ", "index": 1}),
    );
    assert_eq!(names(&h, p.sequence), ["V1", "Music", "A1"]);
    let t: TrackId = id(&out["track"]);
    assert_eq!(
        h.doc().sequences[&p.sequence].tracks[&t].kind,
        TrackKind::Audio
    );
    h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video", "index": 99}),
    );
    assert_eq!(
        names(&h, p.sequence).last().unwrap(),
        "V2",
        "index clamps to the end"
    );
}

#[test]
fn add_unknown_sequence_is_not_found() {
    let (mut h, _) = H::project();
    assert_code(
        h.run(
            "track.add",
            json!({"sequence": SequenceId::from_u128(5), "kind": "video"}),
        ),
        "not_found",
    );
}

#[test]
fn add_rejects_bad_names_and_unsupported_kinds() {
    let (mut h, p) = H::project();
    assert_invalid(
        h.run(
            "track.add",
            json!({"sequence": p.sequence, "kind": "video", "name": ""}),
        ),
        "/name",
    );
    assert_invalid(
        h.run(
            "track.add",
            json!({"sequence": p.sequence, "kind": "movie"}),
        ),
        "/kind",
    );
    assert_code(
        h.run("track.add", json!({"sequence": p.sequence, "kind": "text"})),
        "unsupported",
    );
    assert_invalid(h.run("track.add", json!({"sequence": p.sequence})), "");
}

#[test]
fn add_works_while_other_tracks_are_locked() {
    let (mut h, p) = H::project();
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    );
    assert_eq!(names(&h, p.sequence), ["V2", "V1", "A1"]);
}

#[test]
fn add_undo_redo_identity() {
    let (mut h, p) = H::project();
    h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    );
    let c = h.last.as_ref().unwrap();
    assert_eq!(c.base.sequences[&p.sequence].tracks.len(), 2);
    assert_eq!(c.doc.sequences[&p.sequence].tracks.len(), 3);
}

// ----- track.remove -------------------------------------------------------------------------

#[test]
fn remove_deletes_the_track_and_its_items() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0}),
    );
    let out = h.ok("track.remove", json!({"track": p.video}));
    assert_eq!(out["removed_items"], 1);
    assert_eq!(names(&h, p.sequence), ["A1"]);
    assert_eq!(h.doc().item_count(), 1, "the linked audio clip stays");
}

fn group_of(h: &H, item: ItemId) -> Option<GroupId> {
    h.doc().find_item(&item).unwrap().item.linked_group()
}

#[test]
fn remove_unlinks_a_partner_left_alone_and_undo_restores_the_link() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0}),
    );
    let (v, a) = (item_id(&out["clips"][0]), item_id(&out["clips"][1]));
    let group = group_of(&h, v);
    assert!(group.is_some() && group == group_of(&h, a));
    h.ok("track.remove", json!({"track": p.audio}));
    assert_eq!(group_of(&h, v), None, "the video clip is no longer linked");
    // One transaction: undo brings back the track, its item and the link.
    let c = h.last.as_ref().unwrap();
    let before = &c.base;
    assert!(before.sequences[&p.sequence].tracks.contains_key(&p.audio));
    assert_eq!(before.find_item(&v).unwrap().item.linked_group(), group);
    assert_eq!(before.find_item(&a).unwrap().item.linked_group(), group);
}

#[test]
fn remove_keeps_a_group_that_still_has_two_members() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0}),
    );
    let (v, a) = (item_id(&out["clips"][0]), item_id(&out["clips"][1]));
    let group = group_of(&h, v).unwrap();
    // A third member: a sound-only clip on a second audio track, added to the group.
    let a2 = id::<TrackId>(
        &h.ok(
            "track.add",
            json!({"sequence": p.sequence, "kind": "audio"}),
        )["track"],
    );
    let extra = item_id(
        &h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": a2, "media": m, "at": 0}),
        )["clips"][0],
    );
    let mut doc = h.doc();
    doc.sequences
        .get_mut(&p.sequence)
        .unwrap()
        .track_mut(&a2)
        .unwrap()
        .items
        .update(&extra, |i| i.set_linked_group(Some(group)))
        .unwrap();
    let mut h = H::with_doc(doc);
    // Losing one of three leaves a linked pair.
    h.ok("track.remove", json!({"track": a2}));
    assert_eq!(group_of(&h, v), Some(group));
    assert_eq!(group_of(&h, a), Some(group));
    // Losing a second one dissolves the group.
    h.ok("track.remove", json!({"track": p.audio}));
    assert_eq!(group_of(&h, v), None);
}

#[test]
fn remove_a_track_whose_items_are_all_of_one_group_leaves_no_group_behind() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0}),
    );
    let a = item_id(&out["clips"][1]);
    h.ok("track.remove", json!({"track": p.video}));
    assert_eq!(group_of(&h, a), None);
}

#[test]
fn remove_unknown_track_is_not_found() {
    let (mut h, _) = H::project();
    assert_code(
        h.run("track.remove", json!({"track": TrackId::from_u128(3)})),
        "not_found",
    );
}

#[test]
fn remove_a_locked_track_fails_with_locked() {
    let (mut h, p) = H::project();
    h.ok("track.set", json!({"track": p.audio, "locked": true}));
    assert_code(h.run("track.remove", json!({"track": p.audio})), "locked");
    assert_eq!(names(&h, p.sequence), ["V1", "A1"]);
    // The dry run agrees.
    assert_code(
        h.validate("track.remove", json!({"track": p.audio}))
            .map(|()| json!(null)),
        "locked",
    );
}

#[test]
fn remove_fails_with_locked_when_the_orphaned_partner_is_on_a_locked_track() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0}),
    );
    let a = item_id(&out["clips"][1]);
    h.ok("track.set", json!({"track": p.audio, "locked": true}));
    assert_code(h.run("track.remove", json!({"track": p.video})), "locked");
    assert!(group_of(&h, a).is_some());
}

#[test]
fn remove_rejects_a_malformed_id_with_its_path() {
    let (mut h, _) = H::project();
    assert_invalid(
        h.run("track.remove", json!({"track": "trk_nope"})),
        "/track",
    );
    assert_invalid(h.run("track.remove", json!({})), "");
}

#[test]
fn remove_undo_redo_identity_restores_items_and_order() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0}),
    );
    h.ok("track.remove", json!({"track": p.video}));
    let c = h.last.as_ref().unwrap();
    assert!(c.base.sequences[&p.sequence].tracks.contains_key(&p.video));
    assert!(!c.doc.sequences[&p.sequence].tracks.contains_key(&p.video));
}

// ----- track.set ----------------------------------------------------------------------------

#[test]
fn set_changes_only_the_given_fields() {
    let (mut h, p) = H::project();
    let out = h.ok(
        "track.set",
        json!({"track": p.audio, "name": "Dialogue", "muted": true, "gain": -6.5}),
    );
    assert_eq!(out["name"], "Dialogue");
    assert_eq!(out["muted"], true);
    assert_eq!(out["hidden"], false);
    let doc = h.doc();
    let t = &doc.sequences[&p.sequence].tracks[&p.audio];
    assert!(t.muted && !t.hidden && !t.locked && !t.solo);
    assert!((t.gain + 6.5).abs() < f64::EPSILON);
    h.ok(
        "track.set",
        json!({"track": p.video, "hidden": true, "solo": true}),
    );
    let doc = h.doc();
    let v = &doc.sequences[&p.sequence].tracks[&p.video];
    assert!(v.hidden && v.solo);
}

#[test]
fn set_unknown_track_is_not_found() {
    let (mut h, _) = H::project();
    assert_code(
        h.run(
            "track.set",
            json!({"track": TrackId::from_u128(3), "muted": true}),
        ),
        "not_found",
    );
}

#[test]
fn set_validates_with_field_paths() {
    let (mut h, p) = H::project();
    assert_invalid(h.run("track.set", json!({"track": p.video})), "");
    assert_invalid(
        h.run("track.set", json!({"track": p.video, "name": " "})),
        "/name",
    );
    assert_invalid(
        h.run("track.set", json!({"track": p.video, "gain": 100.0})),
        "/gain",
    );
    assert_invalid(
        h.run("track.set", json!({"track": p.video, "gain": -200.0})),
        "/gain",
    );
    assert_invalid(
        h.run("track.set", json!({"track": p.video, "muted": "yes"})),
        "/muted",
    );
}

#[test]
fn set_works_on_a_locked_track_so_it_can_be_unlocked() {
    let (mut h, p) = H::project();
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    h.ok(
        "track.set",
        json!({"track": p.video, "name": "Renamed while locked"}),
    );
    h.ok("track.set", json!({"track": p.video, "locked": false}));
    assert!(!h.doc().sequences[&p.sequence].tracks[&p.video].locked);
}

#[test]
fn set_is_idempotent_and_undo_redo_identity() {
    let (mut h, p) = H::project();
    h.ok("track.set", json!({"track": p.video, "muted": true}));
    let c = h.last.as_ref().unwrap();
    assert!(!c.base.sequences[&p.sequence].tracks[&p.video].muted);
    assert!(c.doc.sequences[&p.sequence].tracks[&p.video].muted);
    h.ok("track.set", json!({"track": p.video, "muted": true}));
    assert!(h.last.as_ref().unwrap().is_empty());
}
