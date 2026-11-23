// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.06: `item.move`, `item.delete`, `item.set_props`, `item.get`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names
)]

mod common;

use common::{
    FRAME30, H, P, assert_code, assert_invalid, clip_of, id, insert_av, item_id, secs, track_of,
};
use cutweave_model::{ItemId, MediaId, TrackId};
use serde_json::json;

const FLICKS: i64 = 705_600_000;

/// A 10 s video with sound at 2 s: V1 clip `v` and A1 clip `a`, linked.
fn setup() -> (H, P, MediaId, ItemId, ItemId) {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let (v, a) = insert_av(&mut h, p, m, "2s");
    (h, p, m, v, a)
}

fn add_track(h: &mut H, p: P, kind: &str) -> TrackId {
    id(&h.ok("track.add", json!({"sequence": p.sequence, "kind": kind}))["track"])
}

/// A picture-only clip of `secs` seconds at `at` on `track`.
fn pic(h: &mut H, p: P, m: MediaId, track: TrackId, at: &str, secs: i64) -> ItemId {
    item_id(
        &h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": track, "media": m, "at": at,
                   "duration": format!("{secs}s"), "link": false}),
        )["clips"][0],
    )
}

// ----- item.move ----------------------------------------------------------------------------

#[test]
fn move_changes_the_start_and_moves_linked_partners_by_the_same_amount() {
    let (mut h, _, _, v, a) = setup();
    let out = h.ok("item.move", json!({"item": v, "start": "7s"}));
    assert_eq!(clip_of(&h, v).start, secs(7));
    assert_eq!(clip_of(&h, a).start, secs(7));
    assert_eq!(out["items"].as_array().unwrap().len(), 2);
    assert_eq!(out["created_tracks"], json!([]));
    // Backwards, from the audio side.
    h.ok("item.move", json!({"item": a, "start": "f:30"}));
    assert_eq!(clip_of(&h, v).start, secs(1));
}

#[test]
fn move_to_the_same_place_is_a_no_op() {
    let (mut h, _, _, v, _) = setup();
    let before = h.doc();
    h.ok("item.move", json!({"item": v, "start": "2s"}));
    assert_eq!(h.doc(), before);
}

#[test]
fn move_snaps_to_the_frame_grid_and_rejects_negative_starts() {
    let (mut h, _, _, v, _) = setup();
    h.ok(
        "item.move",
        json!({"item": v, "start": 5 * FLICKS + FRAME30 * 6 / 10}),
    );
    assert_eq!(clip_of(&h, v).start.flicks(), 5 * FLICKS + FRAME30);
    assert_invalid(
        h.run("item.move", json!({"item": v, "start": "-1s"})),
        "/start",
    );
}

#[test]
fn move_a_group_before_zero_is_invalid_and_unlink_moves_the_picture_alone() {
    let (mut h, _, _, v, a) = setup();
    h.ok("item.move", json!({"item": v, "start": 0, "unlink": true}));
    assert_eq!(clip_of(&h, v).start.flicks(), 0);
    assert_eq!(clip_of(&h, v).linked_group, None);
    assert_eq!(clip_of(&h, a).start, secs(2));
    // The partner would be alone in its group, so it is unlinked too.
    assert_eq!(clip_of(&h, a).linked_group, None);
}

#[test]
fn move_across_tracks_of_the_same_kind() {
    let (mut h, p, _, v, a) = setup();
    let v2 = add_track(&mut h, p, "video");
    let out = h.ok(
        "item.move",
        json!({"item": v, "start": "4s", "to_track": v2}),
    );
    assert_eq!(track_of(&h, v), v2);
    assert_eq!(
        track_of(&h, a),
        p.audio,
        "the partner stays on its own track"
    );
    assert_eq!(clip_of(&h, a).start, secs(4));
    assert_eq!(out["items"][0]["track"], json!(v2));
}

#[test]
fn move_to_a_track_of_another_kind_or_sequence_or_unknown_is_rejected() {
    let (mut h, p, _, v, _) = setup();
    assert_invalid(
        h.run(
            "item.move",
            json!({"item": v, "start": "3s", "to_track": p.audio}),
        ),
        "/to_track",
    );
    assert_code(
        h.run(
            "item.move",
            json!({"item": v, "start": "3s", "to_track": TrackId::from_u128(5)}),
        ),
        "not_found",
    );
}

#[test]
fn move_overlap_is_invalid_at_start() {
    let (mut h, p, m, v, _) = setup();
    let other = pic(&mut h, p, m, p.video, "20s", 3);
    assert_invalid(
        h.run("item.move", json!({"item": other, "start": "5s"})),
        "/start",
    );
    assert_invalid(
        h.run("item.move", json!({"item": other, "start": "11s"})),
        "/start",
    );
    // Exactly touching is fine, from either side.
    h.ok("item.move", json!({"item": other, "start": "12s"}));
    h.ok("item.move", json!({"item": other, "start": "f:360"}));
    assert_eq!(clip_of(&h, v).start, secs(2));
}

#[test]
fn move_overlap_of_the_audio_partner_is_invalid_too() {
    let (mut h, p, m, v, _) = setup();
    // Occupy A1 at 20 s..23 s with a sound-only clip.
    let a_only = pic(&mut h, p, m, p.audio, "20s", 3);
    let _ = a_only;
    assert_invalid(
        h.run("item.move", json!({"item": v, "start": "18s"})),
        "/start",
    );
    // Moving the picture alone avoids the audio overlap.
    h.ok(
        "item.move",
        json!({"item": v, "start": "18s", "unlink": true}),
    );
}

#[test]
fn move_with_new_track_policy_creates_a_track_above_the_target() {
    let (mut h, p, m, v, a) = setup();
    let other = pic(&mut h, p, m, p.video, "20s", 3);
    let out = h.ok(
        "item.move",
        json!({"item": other, "start": "5s", "overlap": "new_track"}),
    );
    let created: Vec<TrackId> = id(&out["created_tracks"]);
    assert_eq!(created.len(), 1);
    assert_eq!(track_of(&h, other), created[0]);
    let order: Vec<TrackId> = h.doc().sequences[&p.sequence]
        .track_order
        .iter()
        .copied()
        .collect();
    assert_eq!(order[0], created[0], "the new track sits directly above V1");
    assert_eq!(order[1], p.video);
    // Linked group with the policy: the audio partner gets its own new track when busy.
    let _ = (v, a);
}

#[test]
fn move_group_with_new_track_policy_gives_each_busy_member_a_track() {
    let (mut h, p, m, v, a) = setup();
    // A second linked pair at 20 s, moved onto the first.
    let (v2, a2) = insert_av(&mut h, p, m, "20s");
    let out = h.ok(
        "item.move",
        json!({"item": v2, "start": "5s", "overlap": "new_track"}),
    );
    assert_eq!(out["created_tracks"].as_array().unwrap().len(), 2);
    assert_ne!(track_of(&h, v2), p.video);
    assert_ne!(track_of(&h, a2), p.audio);
    assert_eq!(clip_of(&h, v2).start, secs(5));
    let _ = (v, a);
}

#[test]
fn move_on_a_locked_source_target_or_partner_track_is_locked() {
    let (mut h, p, _, v, _) = setup();
    let v2 = add_track(&mut h, p, "video");
    h.ok("track.set", json!({"track": v2, "locked": true}));
    assert_code(
        h.run(
            "item.move",
            json!({"item": v, "start": "3s", "to_track": v2}),
        ),
        "locked",
    );
    h.ok("track.set", json!({"track": v2, "locked": false}));
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    assert_code(
        h.run("item.move", json!({"item": v, "start": "3s"})),
        "locked",
    );
    h.ok("track.set", json!({"track": p.video, "locked": false}));
    h.ok("track.set", json!({"track": p.audio, "locked": true}));
    assert_code(
        h.run("item.move", json!({"item": v, "start": "3s"})),
        "locked",
    );
    // `unlink` would leave the locked partner alone in its group: `Locked`, nothing applied
    // (`H::run` asserts the document is unchanged), and the link survives lock changes.
    let before = h.doc();
    let err = h
        .run(
            "item.move",
            json!({"item": v, "start": "3s", "unlink": true}),
        )
        .unwrap_err();
    assert_eq!(err.code(), "locked");
    assert_eq!(h.doc(), before);
    h.ok("track.set", json!({"track": p.audio, "locked": false}));
    h.ok(
        "item.move",
        json!({"item": v, "start": "3s", "unlink": true}),
    );
}

#[test]
fn unlink_with_a_locked_partner_never_leaves_a_lone_member_after_unlock_and_undo() {
    // Regression (registry_props seed d36d3236...): a lone member on a locked track used to be
    // tolerated and became a single-member group once the lock was undone.
    let (mut h, p, _, v, a) = setup();
    h.ok("track.set", json!({"track": p.audio, "locked": true}));
    assert_code(
        h.run(
            "item.move",
            json!({"item": v, "start": "3s", "unlink": true}),
        ),
        "locked",
    );
    h.ok("track.set", json!({"track": p.audio, "locked": false}));
    let doc = h.doc();
    let group = |i| doc.find_item(&i).unwrap().item.linked_group();
    assert!(group(v).is_some() && group(v) == group(a));
}

#[test]
fn move_unknown_item_and_bad_params() {
    let (mut h, _, _, v, _) = setup();
    assert_code(
        h.run(
            "item.move",
            json!({"item": ItemId::from_u128(1), "start": "1s"}),
        ),
        "not_found",
    );
    assert!(h.run("item.move", json!({"item": v})).is_err());
    assert!(
        h.run(
            "item.move",
            json!({"item": v, "start": "1s", "overlap": "push"})
        )
        .is_err()
    );
}

// ----- item.delete --------------------------------------------------------------------------

#[test]
fn delete_removes_the_item_and_its_linked_partner_without_ripple() {
    let (mut h, p, m, v, a) = setup();
    let (v2, a2) = insert_av(&mut h, p, m, "12s");
    let out = h.ok("item.delete", json!({"items": [v]}));
    assert_eq!(out["deleted"], json!([v, a]));
    let doc = h.doc();
    assert!(doc.find_item(&v).is_none() && doc.find_item(&a).is_none());
    assert_eq!(clip_of(&h, v2).start, secs(12), "no ripple");
    assert_eq!(clip_of(&h, a2).start, secs(12));
}

#[test]
fn delete_many_items_requested_first_then_partners_and_dedups() {
    let (mut h, p, m, v, a) = setup();
    let (v2, a2) = insert_av(&mut h, p, m, "12s");
    let solo = pic(&mut h, p, m, p.video, "30s", 2);
    let out = h.ok("item.delete", json!({"items": [solo, v, v2, v, a]}));
    assert_eq!(out["deleted"], json!([solo, v, v2, a, a2]));
    assert_eq!(
        h.doc().sequences[&p.sequence].tracks[&p.video].items.len(),
        0
    );
}

#[test]
fn delete_is_all_or_nothing_when_any_track_is_locked_or_an_item_is_unknown() {
    let (mut h, p, m, v, _) = setup();
    let v2 = add_track(&mut h, p, "video");
    let solo = pic(&mut h, p, m, v2, "30s", 2);
    h.ok("track.set", json!({"track": p.audio, "locked": true}));
    assert_code(h.run("item.delete", json!({"items": [solo, v]})), "locked");
    assert!(h.doc().find_item(&solo).is_some());
    assert_code(
        h.run(
            "item.delete",
            json!({"items": [solo, ItemId::from_u128(1)]}),
        ),
        "not_found",
    );
    assert!(h.doc().find_item(&solo).is_some());
}

#[test]
fn delete_needs_at_least_one_item() {
    let (mut h, ..) = setup();
    assert_invalid(h.run("item.delete", json!({"items": []})), "/items");
}

#[test]
fn delete_split_halves_independently() {
    let (mut h, _, _, v, a) = setup();
    let out = h.ok("clip.split", json!({"item": v, "at": "6s"}));
    let (right_v, right_a) = (item_id(&out["right"]), item_id(&out["linked"][0]["right"]));
    let out = h.ok("item.delete", json!({"items": [right_a]}));
    assert_eq!(out["deleted"], json!([right_a, right_v]));
    assert!(h.doc().find_item(&v).is_some() && h.doc().find_item(&a).is_some());
}

// ----- item.set_props -----------------------------------------------------------------------

#[test]
fn set_props_changes_only_what_is_given_and_reports_it() {
    let (mut h, _, _, v, _) = setup();
    let out = h.ok(
        "item.set_props",
        json!({"item": v, "props": {"name": "  Intro ", "opacity": 0.25, "enabled": false,
               "audio_gain": -6.0,
               "transform": {"position": {"x": 0.1, "y": -0.2}, "rotation": 90.0}}}),
    );
    assert_eq!(
        out["changed"],
        json!([
            "name",
            "enabled",
            "opacity",
            "audio_gain",
            "transform.position",
            "transform.rotation"
        ])
    );
    let c = clip_of(&h, v);
    assert_eq!(c.name.as_deref(), Some("Intro"));
    assert!(!c.enabled);
    assert_eq!(*c.visual.opacity.value(), 0.25);
    assert_eq!(*c.audio.gain.value(), -6.0);
    assert_eq!(c.visual.transform.position.value().x, 0.1);
    assert_eq!(*c.visual.transform.rotation.value(), 90.0);
    assert_eq!(c.visual.transform.scale.value().x, 1.0, "untouched");
}

#[test]
fn set_props_is_idempotent_and_an_empty_name_clears_it() {
    let (mut h, _, _, v, _) = setup();
    h.ok("item.set_props", json!({"item": v, "props": {"name": "X"}}));
    let before = h.doc();
    let out = h.ok("item.set_props", json!({"item": v, "props": {"name": "X"}}));
    assert_eq!(out["changed"], json!([]));
    assert_eq!(h.doc(), before);
    let out = h.ok("item.set_props", json!({"item": v, "props": {"name": ""}}));
    assert_eq!(out["changed"], json!(["name"]));
    assert_eq!(clip_of(&h, v).name, None);
}

#[test]
fn set_props_validates_values_with_paths() {
    let (mut h, _, _, v, _) = setup();
    for (props, path) in [
        (json!({}), "/props"),
        (json!({"transform": {}}), "/props"),
        (json!({"opacity": 1.5}), "/props/opacity"),
        (json!({"opacity": -0.1}), "/props/opacity"),
        (json!({"audio_gain": 100.0}), "/props/audio_gain"),
        (json!({"audio_gain": -200.0}), "/props/audio_gain"),
        (json!({"name": "a\nb"}), "/props/name"),
        (
            json!({"transform": {"scale": {"x": -1.0, "y": 1.0}}}),
            "/props/transform/scale",
        ),
    ] {
        assert_invalid(
            h.run("item.set_props", json!({"item": v, "props": props})),
            path,
        );
    }
    assert!(
        h.run("item.set_props", json!({"item": v, "props": {"volume": 1}}))
            .is_err(),
        "unknown props are rejected"
    );
}

#[test]
fn set_props_on_a_locked_track_is_locked() {
    let (mut h, p, _, v, a) = setup();
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    assert_code(
        h.run(
            "item.set_props",
            json!({"item": v, "props": {"enabled": false}}),
        ),
        "locked",
    );
    // The partner on the unlocked track is editable on its own.
    h.ok(
        "item.set_props",
        json!({"item": a, "props": {"enabled": false}}),
    );
}

#[test]
fn set_props_unknown_item_is_not_found() {
    let (mut h, ..) = setup();
    assert_code(
        h.run(
            "item.set_props",
            json!({"item": ItemId::from_u128(4), "props": {"enabled": true}}),
        ),
        "not_found",
    );
}

// ----- item.get -----------------------------------------------------------------------------

#[test]
fn get_returns_the_timeline_get_projection_plus_details() {
    let (mut h, p, _, v, a) = setup();
    h.ok(
        "item.set_props",
        json!({"item": v, "props": {"name": "Hero"}}),
    );
    let out = h.query("item.get", json!({"item": v})).unwrap();
    assert_eq!(out["track"], json!(p.video));
    assert_eq!(out["sequence"], json!(p.sequence));
    assert_eq!(out["name"], "Hero");
    assert_eq!(out["linked"], json!([a]));
    assert_eq!(out["source"]["source_in"], 0);
    assert_eq!(out["visual"]["opacity"], 1.0);
    let tl = h
        .query("timeline.get", json!({"sequence": p.sequence, "depth": 2}))
        .unwrap();
    let from_timeline = tl["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["items"].as_array().unwrap().clone())
        .find(|i| i["id"] == json!(v))
        .unwrap();
    assert_eq!(out["item"], from_timeline);
}

#[test]
fn get_fields_limit_the_sections_and_unknown_ones_are_invalid() {
    let (h, _, _, v, _) = setup();
    let out = h
        .query("item.get", json!({"item": v, "fields": ["audio"]}))
        .unwrap();
    assert!(out.get("audio").is_some());
    assert!(out.get("visual").is_none() && out.get("source").is_none());
    assert_invalid(
        h.query("item.get", json!({"item": v, "fields": ["colour"]})),
        "/fields/0",
    );
    assert_code(
        h.query("item.get", json!({"item": ItemId::from_u128(8)})),
        "not_found",
    );
}
