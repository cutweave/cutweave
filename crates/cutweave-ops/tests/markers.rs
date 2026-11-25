// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.02: `timeline.add_marker`, `timeline.move_marker`, `timeline.remove_marker`,
//! `timeline.set_in_out`, `timeline.clear_in_out` and `timeline.list_markers` (timeline spec
//! §1.5, INV-24 snapping, INV-27 locks, INV-34 markers never ripple).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, P, assert_code, assert_invalid, id, item_id};
use cutweave_model::{ItemId, MarkerId, MediaId, SequenceId};
use serde_json::json;

const FRAME: i64 = 23_520_000; // 30 fps, the `project.new` default

fn setup() -> (H, P, MediaId) {
    let (mut h, p) = H::project();
    let video = h.add_video("/m/v.mp4", 20, true);
    (h, p, video)
}

fn clip(h: &mut H, p: P, video: MediaId, at: i64, dur: i64) -> ItemId {
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": video,
               "at": format!("f:{at}"), "duration": format!("f:{dur}")}),
    );
    item_id(&out["clips"][0])
}

fn marker_t(h: &H, p: P, marker: MarkerId) -> i64 {
    h.doc().sequences[&p.sequence].markers[&marker].t.flicks()
}

// ----- add / move / remove -----------------------------------------------------------------------

#[test]
fn add_marker_defaults_and_snaps_to_the_frame_grid() {
    let (mut h, p, _) = setup();
    let out = h.ok(
        "timeline.add_marker",
        json!({"sequence": p.sequence, "t": 10 * FRAME + 5_000_000}),
    );
    let m: MarkerId = id(&out["marker"]);
    assert_eq!(out["t"], 10 * FRAME, "snapped to the nearest frame");
    assert!(out.get("duration").is_none());
    let doc = h.doc();
    let stored = &doc.sequences[&p.sequence].markers[&m];
    assert_eq!(stored.label, "");
    assert_eq!(stored.kind, cutweave_model::MarkerKind::User);
    assert_eq!(stored.color, cutweave_model::Marker::DEFAULT_COLOR);
    assert_eq!(stored.duration, None);
    // Ties go later.
    let out = h.ok(
        "timeline.add_marker",
        json!({"sequence": p.sequence, "t": 3 * FRAME + FRAME / 2}),
    );
    assert_eq!(out["t"], 4 * FRAME);
}

#[test]
fn add_marker_takes_every_field() {
    let (mut h, p, _) = setup();
    let out = h.ok(
        "timeline.add_marker",
        json!({"sequence": p.sequence, "t": "f:30", "duration": "f:15", "label": "  Intro  ",
               "color": "#336699", "kind": "chapter"}),
    );
    let m: MarkerId = id(&out["marker"]);
    assert_eq!(out["duration"], 15 * FRAME);
    let doc = h.doc();
    let stored = &doc.sequences[&p.sequence].markers[&m];
    assert_eq!(stored.label, "Intro");
    assert_eq!(stored.color.to_string(), "#336699");
    assert_eq!(stored.kind, cutweave_model::MarkerKind::Chapter);
    assert_eq!(stored.duration.unwrap().flicks(), 15 * FRAME);
}

#[test]
fn add_marker_rejects_bad_params_with_their_paths() {
    let (mut h, p, _) = setup();
    let s = p.sequence;
    assert_invalid(
        h.run("timeline.add_marker", json!({"sequence": s, "t": "-1.0s"})),
        "/t",
    );
    assert_invalid(
        h.run(
            "timeline.add_marker",
            json!({"sequence": s, "t": 0, "duration": 0}),
        ),
        "/duration",
    );
    assert_invalid(
        h.run(
            "timeline.add_marker",
            json!({"sequence": s, "t": 0, "label": "x".repeat(201)}),
        ),
        "/label",
    );
    assert_invalid(
        h.run(
            "timeline.add_marker",
            json!({"sequence": s, "t": 0, "label": "a\tb"}),
        ),
        "/label",
    );
    assert_invalid(
        h.run(
            "timeline.add_marker",
            json!({"sequence": s, "t": 0, "color": "red"}),
        ),
        "/color",
    );
    assert_invalid(
        h.run(
            "timeline.add_marker",
            json!({"sequence": s, "t": 0, "kind": "bar"}),
        ),
        "/kind",
    );
    assert_code(
        h.run(
            "timeline.add_marker",
            json!({"sequence": SequenceId::from_u128(3), "t": 0}),
        ),
        "not_found",
    );
    assert_code(
        h.run(
            "timeline.add_marker",
            json!({"sequence": s, "t": 0, "item": ItemId::from_u128(4)}),
        ),
        "not_found",
    );
}

#[test]
fn move_marker_snaps_keeps_the_rest_and_reports_the_previous_time() {
    let (mut h, p, _) = setup();
    let m: MarkerId = id(&h.ok(
        "timeline.add_marker",
        json!({"sequence": p.sequence, "t": "f:5", "label": "A", "duration": "f:3"}),
    )["marker"]);
    let out = h.ok(
        "timeline.move_marker",
        json!({"marker": m, "t": 20 * FRAME + 3}),
    );
    assert_eq!(out["t"], 20 * FRAME);
    assert_eq!(out["previous"], 5 * FRAME);
    assert_eq!(marker_t(&h, p, m), 20 * FRAME);
    let doc = h.doc();
    assert_eq!(doc.sequences[&p.sequence].markers[&m].label, "A");
    assert_eq!(
        doc.sequences[&p.sequence].markers[&m]
            .duration
            .unwrap()
            .flicks(),
        3 * FRAME
    );
    assert_invalid(
        h.run("timeline.move_marker", json!({"marker": m, "t": "-0.5s"})),
        "/t",
    );
    // Moving to where it is changes nothing.
    h.ok("timeline.move_marker", json!({"marker": m, "t": "f:20"}));
    assert!(h.last.as_ref().unwrap().patch.0.is_empty());
    assert_code(
        h.run(
            "timeline.move_marker",
            json!({"marker": MarkerId::from_u128(8), "t": 0}),
        ),
        "not_found",
    );
}

#[test]
fn remove_marker_and_undo_identity() {
    let (mut h, p, _) = setup();
    let m: MarkerId = id(&h.ok(
        "timeline.add_marker",
        json!({"sequence": p.sequence, "t": 0}),
    )["marker"]);
    h.ok("timeline.remove_marker", json!({"marker": m}));
    assert!(h.doc().sequences[&p.sequence].markers.is_empty());
    assert_code(
        h.run("timeline.remove_marker", json!({"marker": m})),
        "not_found",
    );
}

// ----- item markers -----------------------------------------------------------------------------

#[test]
fn item_markers_use_item_relative_time_and_travel_with_the_item() {
    let (mut h, p, video) = setup();
    let a = clip(&mut h, p, video, 0, 30);
    let m: MarkerId = id(&h.ok(
        "timeline.add_marker",
        json!({"sequence": p.sequence, "item": a, "t": "f:12", "kind": "beat"}),
    )["marker"]);
    // Not a sequence marker.
    assert!(h.doc().sequences[&p.sequence].markers.is_empty());
    let found = h
        .doc()
        .find_item(&a)
        .unwrap()
        .item
        .markers()
        .get(&m)
        .cloned()
        .unwrap();
    assert_eq!(found.t.flicks(), 12 * FRAME);
    h.ok("item.move", json!({"item": a, "start": "f:50"}));
    let moved = h
        .doc()
        .find_item(&a)
        .unwrap()
        .item
        .markers()
        .get(&m)
        .cloned()
        .unwrap();
    assert_eq!(
        moved.t.flicks(),
        12 * FRAME,
        "item-relative time does not change"
    );
    // Move and remove find it by ID.
    h.ok("timeline.move_marker", json!({"marker": m, "t": "f:4"}));
    let listed = h
        .query(
            "timeline.list_markers",
            json!({"sequence": p.sequence, "item": a}),
        )
        .unwrap();
    assert_eq!(listed["markers"][0]["t"], 4 * FRAME);
    assert_eq!(listed["item"], json!(a));
    h.ok("timeline.remove_marker", json!({"marker": m}));
    assert!(h.doc().find_item(&a).unwrap().item.markers().is_empty());
}

#[test]
fn item_markers_on_a_locked_track_are_locked_but_sequence_markers_are_not() {
    let (mut h, p, video) = setup();
    let a = clip(&mut h, p, video, 0, 30);
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    let before = h.doc();
    let err = h
        .run(
            "timeline.add_marker",
            json!({"sequence": p.sequence, "item": a, "t": 0}),
        )
        .unwrap_err();
    assert_eq!(err.code(), "locked");
    assert_eq!(h.doc(), before);
    h.ok(
        "timeline.add_marker",
        json!({"sequence": p.sequence, "t": 0}),
    );
    h.ok(
        "timeline.set_in_out",
        json!({"sequence": p.sequence, "start": "f:1", "end": "f:9"}),
    );
}

#[test]
fn splitting_a_clip_shares_out_its_item_markers() {
    let (mut h, p, video) = setup();
    let a = clip(&mut h, p, video, 10, 40);
    let mut ids = Vec::new();
    for frame in [5, 15, 30] {
        let out = h.ok(
            "timeline.add_marker",
            json!({"sequence": p.sequence, "item": a, "t": format!("f:{frame}")}),
        );
        ids.push(id::<MarkerId>(&out["marker"]));
    }
    let out = h.ok("clip.split", json!({"item": a, "at": "f:30"}));
    let right = item_id(&out["right"]);
    let doc = h.doc();
    let left_markers: Vec<MarkerId> = doc
        .find_item(&a)
        .unwrap()
        .item
        .markers()
        .keys()
        .copied()
        .collect();
    let right_item = doc.find_item(&right).unwrap().item;
    // Cut at item time 20: the markers at 5 and 15 stay, the one at 30 goes right at 10.
    assert_eq!(left_markers.len(), 2);
    assert!(left_markers.contains(&ids[0]) && left_markers.contains(&ids[1]));
    assert_eq!(right_item.markers().len(), 1);
    assert_eq!(right_item.markers()[&ids[2]].t.flicks(), 10 * FRAME);
    doc.validate().unwrap();
}

// ----- in / out --------------------------------------------------------------------------------

#[test]
fn set_and_clear_in_out() {
    let (mut h, p, _) = setup();
    let out = h.ok(
        "timeline.set_in_out",
        json!({"sequence": p.sequence, "start": 2 * FRAME + 1, "end": "f:30"}),
    );
    assert_eq!(out["start"], 2 * FRAME);
    assert_eq!(out["end"], 30 * FRAME);
    let range = h.doc().sequences[&p.sequence].in_out.clone().unwrap();
    assert_eq!(
        (range.start.flicks(), range.end.flicks()),
        (2 * FRAME, 30 * FRAME)
    );
    h.ok(
        "timeline.set_in_out",
        json!({"sequence": p.sequence, "start": "f:3", "end": "f:9"}),
    );
    let listed = h
        .query("timeline.list_markers", json!({"sequence": p.sequence}))
        .unwrap();
    assert_eq!(listed["in_out"]["start"], 3 * FRAME);
    assert_eq!(
        h.ok("timeline.clear_in_out", json!({"sequence": p.sequence}))["cleared"],
        true
    );
    assert!(h.doc().sequences[&p.sequence].in_out.is_none());
    assert_eq!(
        h.ok("timeline.clear_in_out", json!({"sequence": p.sequence}))["cleared"],
        false
    );
    assert!(h.last.as_ref().unwrap().patch.0.is_empty());
}

#[test]
fn in_out_validation() {
    let (mut h, p, _) = setup();
    let s = p.sequence;
    assert_invalid(
        h.run(
            "timeline.set_in_out",
            json!({"sequence": s, "start": "f:9", "end": "f:9"}),
        ),
        "/end",
    );
    assert_invalid(
        h.run(
            "timeline.set_in_out",
            json!({"sequence": s, "start": "f:9", "end": "f:3"}),
        ),
        "/end",
    );
    assert_invalid(
        h.run(
            "timeline.set_in_out",
            json!({"sequence": s, "start": "-1.0s", "end": "f:3"}),
        ),
        "/start",
    );
    // Snapping to the same frame is also an empty range.
    assert_invalid(
        h.run(
            "timeline.set_in_out",
            json!({"sequence": s, "start": FRAME * 5, "end": FRAME * 5 + 100}),
        ),
        "/end",
    );
    assert_code(
        h.run(
            "timeline.set_in_out",
            json!({"sequence": SequenceId::from_u128(9), "start": 0, "end": FRAME}),
        ),
        "not_found",
    );
}

// ----- INV-34: markers and in/out stay when the timeline changes -----------------------------

#[test]
fn edits_never_move_sequence_markers_or_the_in_out_range() {
    let (mut h, p, video) = setup();
    let a = clip(&mut h, p, video, 0, 30);
    let m: MarkerId = id(&h.ok(
        "timeline.add_marker",
        json!({"sequence": p.sequence, "t": "f:15"}),
    )["marker"]);
    h.ok(
        "timeline.set_in_out",
        json!({"sequence": p.sequence, "start": "f:2", "end": "f:20"}),
    );
    let before = h.doc().sequences[&p.sequence].clone();
    h.ok("item.move", json!({"item": a, "start": "f:100"}));
    h.ok("clip.split", json!({"item": a, "at": "f:110"}));
    h.ok("item.delete", json!({"items": [a]}));
    let after = h.doc().sequences[&p.sequence].clone();
    assert_eq!(after.markers, before.markers);
    assert_eq!(after.in_out, before.in_out);
    assert_eq!(marker_t(&h, p, m), 15 * FRAME);
}

#[test]
fn list_markers_sorts_by_time() {
    let (mut h, p, _) = setup();
    for t in [9, 3, 6] {
        h.ok(
            "timeline.add_marker",
            json!({"sequence": p.sequence, "t": format!("f:{t}")}),
        );
    }
    let out = h
        .query("timeline.list_markers", json!({"sequence": p.sequence}))
        .unwrap();
    let times: Vec<i64> = out["markers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["t"].as_i64().unwrap() / FRAME)
        .collect();
    assert_eq!(times, [3, 6, 9]);
    assert!(out.get("item").is_none());
    assert!(out.get("in_out").is_none());
}
