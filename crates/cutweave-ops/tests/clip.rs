// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.06: `clip.split` and `clip.trim`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names
)]

mod common;

use common::{
    FRAME30, H, P, assert_code, assert_invalid, clip_of, id, insert_av, item_id, secs, track_of,
};
use cutweave_model::{ItemId, MediaId, Project, Time};
use serde_json::json;

const FLICKS: i64 = 705_600_000;

/// A project with a 10 s video with sound at 2 s: V1 clip `v` and A1 clip `a`, linked.
fn setup() -> (H, P, MediaId, ItemId, ItemId) {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let (v, a) = insert_av(&mut h, p, m, "2s");
    (h, p, m, v, a)
}

fn ntsc() -> (H, P) {
    let (h, p) = H::project();
    let mut doc: Project = h.doc();
    doc.sequences
        .get_mut(&p.sequence)
        .unwrap()
        .format
        .frame_rate = cutweave_model::FrameRate::FPS_29_97;
    (H::with_doc(doc), p)
}

// ----- clip.split ---------------------------------------------------------------------------

#[test]
fn split_at_a_frame_boundary_gives_contiguous_halves() {
    let (mut h, _, _, v, _) = setup();
    let orig = clip_of(&h, v);
    let out = h.ok("clip.split", json!({"item": v, "at": "5s"}));
    let (l, r) = (item_id(&out["left"]), item_id(&out["right"]));
    assert_eq!(l, v, "the left half keeps the original id");
    assert_ne!(r, v);
    let (lc, rc) = (clip_of(&h, l), clip_of(&h, r));
    assert_eq!(
        lc.duration.flicks() + rc.duration.flicks(),
        orig.duration.flicks()
    );
    assert_eq!(lc.start, orig.start);
    assert_eq!(rc.start, secs(5));
    assert_eq!(lc.start.flicks() + lc.duration.flicks(), rc.start.flicks());
    assert_eq!(
        rc.source_in.flicks(),
        lc.source_in.flicks() + lc.duration.flicks()
    );
    assert_eq!(rc.media, orig.media);
    assert_eq!(out["at"], json!(5 * FLICKS));
}

#[test]
fn split_accepts_frame_and_timecode_inputs() {
    let (mut h, _, _, v, _) = setup();
    // frame 90 = 3 s at 30 fps.
    let out = h.ok("clip.split", json!({"item": v, "at": "f:90"}));
    assert_eq!(clip_of(&h, item_id(&out["right"])).start, secs(3));
    let out = h.ok(
        "clip.split",
        json!({"item": out["right"], "at": "00:00:04:00"}),
    );
    assert_eq!(clip_of(&h, item_id(&out["right"])).start, secs(4));
}

#[test]
fn split_snaps_an_off_grid_time_to_the_nearest_frame() {
    let (mut h, _, _, v, _) = setup();
    let out = h.ok(
        "clip.split",
        json!({"item": v, "at": 3 * FLICKS + FRAME30 * 4 / 10}),
    );
    assert_eq!(out["at"], json!(3 * FLICKS));
    let out = h.ok(
        "clip.split",
        json!({"item": out["right"], "at": 4 * FLICKS + FRAME30 * 6 / 10}),
    );
    assert_eq!(out["at"], json!(4 * FLICKS + FRAME30));
}

#[test]
fn split_at_ntsc_frame_edges_is_exact() {
    let (mut h, p) = ntsc();
    let m = h.add_video("/m/n.mp4", 10, false);
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0}),
    );
    let v = item_id(&out["clips"][0]);
    let out = h.ok("clip.split", json!({"item": v, "at": "f:100"}));
    let rate = cutweave_model::FrameRate::FPS_29_97;
    let expected = rate.frame_to_time(100).unwrap();
    let (l, r) = (
        clip_of(&h, item_id(&out["left"])),
        clip_of(&h, item_id(&out["right"])),
    );
    assert_eq!(r.start, expected);
    assert_eq!(l.duration.flicks(), expected.flicks());
    assert_eq!(r.source_in, expected);
    // The default length is the media floored to whole frames (spec INV-25): 299 of 299.7.
    assert_eq!(
        l.duration.flicks() + r.duration.flicks(),
        rate.frame_to_time(299).unwrap().flicks()
    );
    // A time a few flicks off the frame edge lands on the same boundary.
    let edge = rate.frame_to_time(200).unwrap().flicks();
    let out = h.ok("clip.split", json!({"item": r.id, "at": edge + 3}));
    assert_eq!(out["at"], json!(edge));
}

#[test]
fn split_at_the_start_or_end_is_invalid() {
    let (mut h, _, _, v, _) = setup();
    for at in ["2s", "12s", "0s", "20s", "f:60", "f:360"] {
        assert_invalid(h.run("clip.split", json!({"item": v, "at": at})), "/at");
    }
    // One frame inside works.
    h.ok("clip.split", json!({"item": v, "at": "f:61"}));
}

#[test]
fn split_that_snaps_onto_an_edge_is_invalid() {
    let (mut h, _, _, v, _) = setup();
    assert_invalid(
        h.run(
            "clip.split",
            json!({"item": v, "at": 2 * FLICKS + FRAME30 / 3}),
        ),
        "/at",
    );
}

#[test]
fn split_also_splits_the_linked_partner_and_regroups_the_right_halves() {
    let (mut h, _, _, v, a) = setup();
    let group = clip_of(&h, v).linked_group.unwrap();
    let out = h.ok("clip.split", json!({"item": v, "at": "6s"}));
    let pair = &out["linked"][0];
    assert_eq!(item_id(&pair["left"]), a);
    let (lv, ra) = (clip_of(&h, v), clip_of(&h, item_id(&pair["right"])));
    let (rv, la) = (clip_of(&h, item_id(&out["right"])), clip_of(&h, a));
    assert_eq!(lv.linked_group, Some(group));
    assert_eq!(la.linked_group, Some(group));
    let right_group = id::<Option<cutweave_model::GroupId>>(&out["right_group"]).unwrap();
    assert_ne!(right_group, group);
    assert_eq!(rv.linked_group, Some(right_group));
    assert_eq!(ra.linked_group, Some(right_group));
    assert_eq!(rv.start, ra.start);
    assert_eq!(rv.source_in, ra.source_in);
}

#[test]
fn split_from_the_audio_side_splits_the_video_too() {
    let (mut h, _, _, v, a) = setup();
    let out = h.ok("clip.split", json!({"item": a, "at": "4s"}));
    assert_eq!(out["linked"].as_array().unwrap().len(), 1);
    assert_eq!(clip_of(&h, v).duration.flicks(), 2 * FLICKS);
}

#[test]
fn split_a_clip_without_partners_has_no_right_group() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/s.mp4", 10, false);
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0}),
    );
    let v = item_id(&out["clips"][0]);
    let out = h.ok("clip.split", json!({"item": v, "at": "1s"}));
    assert_eq!(out["linked"], json!([]));
    assert_eq!(out["right_group"], json!(null));
    assert_eq!(clip_of(&h, item_id(&out["right"])).linked_group, None);
}

#[test]
fn split_keeps_props_and_a_still_keeps_source_in() {
    let (mut h, p) = H::project();
    let img = h.add_image("/m/i.png");
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": img, "at": 0}),
    );
    let i = item_id(&out["clips"][0]);
    h.ok(
        "item.set_props",
        json!({"item": i, "props": {"name": "Title", "opacity": 0.5}}),
    );
    let out = h.ok("clip.split", json!({"item": i, "at": "2s"}));
    let r = clip_of(&h, item_id(&out["right"]));
    assert_eq!(r.source_in, Time::ZERO);
    assert_eq!(r.name.as_deref(), Some("Title"));
    assert_eq!(*r.visual.opacity.value(), 0.5);
    assert_eq!(r.duration.flicks(), 3 * FLICKS);
}

#[test]
fn split_not_found_and_locked_change_nothing() {
    let (mut h, p, _, v, _) = setup();
    assert_code(
        h.run(
            "clip.split",
            json!({"item": ItemId::from_u128(9), "at": "3s"}),
        ),
        "not_found",
    );
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    assert_code(
        h.run("clip.split", json!({"item": v, "at": "3s"})),
        "locked",
    );
    // The locked partner alone blocks the whole group.
    h.ok("track.set", json!({"track": p.video, "locked": false}));
    h.ok("track.set", json!({"track": p.audio, "locked": true}));
    assert_code(
        h.run("clip.split", json!({"item": v, "at": "3s"})),
        "locked",
    );
}

#[test]
fn split_rejects_bad_params() {
    let (mut h, _, _, v, _) = setup();
    assert!(h.run("clip.split", json!({"item": v})).is_err());
    assert!(
        h.run("clip.split", json!({"item": v, "at": "soon"}))
            .is_err()
    );
    assert!(
        h.run("clip.split", json!({"item": v, "at": "3s", "x": 1}))
            .is_err()
    );
}

#[test]
fn split_is_one_commit_and_validates_as_a_dry_run() {
    let (mut h, _, _, v, _) = setup();
    let commits = h.commits();
    assert!(
        h.validate("clip.split", json!({"item": v, "at": "2s"}))
            .is_err()
    );
    h.validate("clip.split", json!({"item": v, "at": "6s"}))
        .unwrap();
    assert_eq!(h.commits(), commits, "validation does not apply");
    h.ok("clip.split", json!({"item": v, "at": "6s"}));
    assert_eq!(h.commits(), commits + 1);
}

// ----- clip.trim ----------------------------------------------------------------------------

#[test]
fn trim_start_moves_start_and_source_in_together() {
    let (mut h, _, _, v, a) = setup();
    let out = h.ok("clip.trim", json!({"item": v, "edge": "start", "to": "3s"}));
    let (c, ca) = (clip_of(&h, v), clip_of(&h, a));
    assert_eq!(c.start, secs(3));
    assert_eq!(c.source_in, secs(1));
    assert_eq!(c.duration.flicks(), 9 * FLICKS);
    assert_eq!(
        (ca.start, ca.source_in, ca.duration),
        (c.start, c.source_in, c.duration)
    );
    assert_eq!(out["items"].as_array().unwrap().len(), 2);
}

#[test]
fn trim_end_changes_only_the_length() {
    let (mut h, _, _, v, a) = setup();
    h.ok("clip.trim", json!({"item": v, "edge": "end", "to": "5s"}));
    for i in [v, a] {
        let c = clip_of(&h, i);
        assert_eq!((c.start, c.source_in), (secs(2), Time::ZERO));
        assert_eq!(c.duration.flicks(), 3 * FLICKS);
    }
}

#[test]
fn trim_by_delta_in_both_directions() {
    let (mut h, _, _, v, _) = setup();
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "end", "delta": "-1s"}),
    );
    assert_eq!(clip_of(&h, v).duration.flicks(), 9 * FLICKS);
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "end", "delta": "f:30"}),
    );
    assert_eq!(clip_of(&h, v).duration.flicks(), 10 * FLICKS);
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "start", "delta": "1s"}),
    );
    assert_eq!(clip_of(&h, v).start, secs(3));
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "start", "delta": "-1s"}),
    );
    assert_eq!(clip_of(&h, v).start, secs(2));
}

#[test]
fn trim_needs_exactly_one_of_to_and_delta() {
    let (mut h, _, _, v, _) = setup();
    assert_invalid(h.run("clip.trim", json!({"item": v, "edge": "end"})), "");
    assert_invalid(
        h.run(
            "clip.trim",
            json!({"item": v, "edge": "end", "to": "5s", "delta": "1s"}),
        ),
        "",
    );
}

#[test]
fn trim_past_the_media_bounds_is_invalid() {
    let (mut h, _, _, v, _) = setup();
    // The clip plays all 10 s of media at 2 s..12 s.
    assert_invalid(
        h.run("clip.trim", json!({"item": v, "edge": "end", "to": "13s"})),
        "/to",
    );
    assert_invalid(
        h.run(
            "clip.trim",
            json!({"item": v, "edge": "end", "delta": "1s"}),
        ),
        "/delta",
    );
    assert_invalid(
        h.run("clip.trim", json!({"item": v, "edge": "start", "to": "1s"})),
        "/to",
    );
    // After trimming the start, it can grow back up to the media start only.
    h.ok("clip.trim", json!({"item": v, "edge": "start", "to": "5s"}));
    assert_invalid(
        h.run("clip.trim", json!({"item": v, "edge": "start", "to": "1s"})),
        "/to",
    );
    h.ok("clip.trim", json!({"item": v, "edge": "start", "to": "2s"}));
}

#[test]
fn trim_to_the_exact_media_end_is_allowed() {
    let (mut h, _, _, v, _) = setup();
    h.ok("clip.trim", json!({"item": v, "edge": "end", "to": "4s"}));
    h.ok("clip.trim", json!({"item": v, "edge": "end", "to": "12s"}));
    assert_eq!(clip_of(&h, v).duration.flicks(), 10 * FLICKS);
}

#[test]
fn trim_cannot_empty_the_clip() {
    let (mut h, _, _, v, _) = setup();
    for (edge, to) in [
        ("start", "12s"),
        ("start", "13s"),
        ("end", "2s"),
        ("end", "1s"),
    ] {
        assert_invalid(
            h.run("clip.trim", json!({"item": v, "edge": edge, "to": to})),
            "/to",
        );
    }
    h.ok("clip.trim", json!({"item": v, "edge": "end", "to": "f:61"}));
    assert_eq!(clip_of(&h, v).duration.flicks(), FRAME30);
}

/// Picture-only clips on V1: A at 0 s..4 s, B at 6 s..8 s (playing 3 s..5 s of its media).
fn neighbours() -> (H, P, ItemId, ItemId) {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/n.mp4", 10, false);
    let mut put = |at: &str, source_in: &str, dur: &str| {
        let out = h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": m, "at": at,
                   "source_in": source_in, "duration": dur}),
        );
        item_id(&out["clips"][0])
    };
    let a = put("0s", "0s", "4s");
    let b = put("6s", "3s", "2s");
    (h, p, a, b)
}

#[test]
fn trim_start_into_the_previous_clip_is_invalid() {
    let (mut h, _, a, b) = neighbours();
    // Touching A's end (4 s) is fine; 3 s would overlap it.
    assert_invalid(
        h.run("clip.trim", json!({"item": b, "edge": "start", "to": "3s"})),
        "/to",
    );
    h.ok("clip.trim", json!({"item": b, "edge": "start", "to": "4s"}));
    let c = clip_of(&h, b);
    assert_eq!((c.start, c.source_in), (secs(4), secs(1)));
    assert_eq!(
        clip_of(&h, a).duration.flicks(),
        4 * FLICKS,
        "the neighbour is untouched"
    );
}

#[test]
fn trim_end_into_the_next_clip_is_invalid() {
    let (mut h, _, a, _) = neighbours();
    assert_invalid(
        h.run("clip.trim", json!({"item": a, "edge": "end", "to": "7s"})),
        "/to",
    );
    assert_invalid(
        h.run(
            "clip.trim",
            json!({"item": a, "edge": "end", "delta": "3s"}),
        ),
        "/delta",
    );
    h.ok("clip.trim", json!({"item": a, "edge": "end", "to": "6s"}));
}

#[test]
fn trim_start_before_zero_is_invalid() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/z.mp4", 10, false);
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": "1s",
               "source_in": "5s", "duration": "2s"}),
    );
    let c = item_id(&out["clips"][0]);
    // The media would allow 5 s more, the timeline only 1 s.
    assert_invalid(
        h.run(
            "clip.trim",
            json!({"item": c, "edge": "start", "delta": "-2s"}),
        ),
        "/delta",
    );
    h.ok(
        "clip.trim",
        json!({"item": c, "edge": "start", "delta": "-1s"}),
    );
    assert_eq!(clip_of(&h, c).start.flicks(), 0);
}

#[test]
fn trim_a_still_can_grow_both_ways() {
    let (mut h, p) = H::project();
    let img = h.add_image("/m/i.png");
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": img, "at": "3s", "duration": "2s"}),
    );
    let i = item_id(&out["clips"][0]);
    h.ok("clip.trim", json!({"item": i, "edge": "end", "to": "60s"}));
    h.ok("clip.trim", json!({"item": i, "edge": "start", "to": "1s"}));
    let c = clip_of(&h, i);
    assert_eq!((c.start, c.source_in), (secs(1), Time::ZERO));
    assert_eq!(c.duration.flicks(), 59 * FLICKS);
}

#[test]
fn trim_unlink_edits_one_clip_and_drops_it_from_the_group() {
    let (mut h, _, _, v, a) = setup();
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "end", "to": "6s", "unlink": true}),
    );
    assert_eq!(clip_of(&h, v).linked_group, None);
    assert_eq!(clip_of(&h, v).duration.flicks(), 4 * FLICKS);
    assert_eq!(clip_of(&h, a).duration.flicks(), 10 * FLICKS);
    // The partner would be alone in its group, so it is unlinked too.
    assert_eq!(clip_of(&h, a).linked_group, None);
}

#[test]
fn trim_on_a_locked_track_is_locked_and_unlink_of_a_locked_partner_is_locked_too() {
    let (mut h, p, _, v, _) = setup();
    h.ok("track.set", json!({"track": p.audio, "locked": true}));
    assert_code(
        h.run("clip.trim", json!({"item": v, "edge": "end", "to": "5s"})),
        "locked",
    );
    // `unlink` would leave the locked partner alone in its group: `Locked`, nothing applied.
    assert_code(
        h.run(
            "clip.trim",
            json!({"item": v, "edge": "end", "to": "5s", "unlink": true}),
        ),
        "locked",
    );
    h.ok("track.set", json!({"track": p.audio, "locked": false}));
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "end", "to": "5s", "unlink": true}),
    );
}

#[test]
fn trim_snaps_to_the_frame_grid() {
    let (mut h, _, _, v, _) = setup();
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "end", "to": 5 * FLICKS + FRAME30 * 7 / 10}),
    );
    assert_eq!(clip_of(&h, v).duration.flicks(), 3 * FLICKS + FRAME30);
}

#[test]
fn trim_to_the_current_edge_is_a_no_op_success() {
    let (mut h, _, _, v, _) = setup();
    let before = h.doc();
    h.ok("clip.trim", json!({"item": v, "edge": "end", "to": "12s"}));
    assert_eq!(h.doc(), before);
}

#[test]
fn trim_unknown_item_and_bad_edge() {
    let (mut h, _, _, v, _) = setup();
    assert_code(
        h.run(
            "clip.trim",
            json!({"item": ItemId::from_u128(3), "edge": "end", "to": "5s"}),
        ),
        "not_found",
    );
    assert!(
        h.run(
            "clip.trim",
            json!({"item": v, "edge": "middle", "to": "5s"})
        )
        .is_err()
    );
}

#[test]
fn split_then_trim_the_right_half_keeps_source_continuity() {
    let (mut h, _, _, v, _) = setup();
    let out = h.ok("clip.split", json!({"item": v, "at": "6s"}));
    let r = item_id(&out["right"]);
    assert_eq!(track_of(&h, r), track_of(&h, v));
    h.ok("clip.trim", json!({"item": r, "edge": "start", "to": "7s"}));
    assert_eq!(clip_of(&h, r).source_in.flicks(), 5 * FLICKS);
}
