// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.02: `track.set_magnetic` (timeline spec §4.17; the `set-magnetic-*` fixtures of
//! `docs/spec/fixtures/`), `track.reorder`, `track.set_height`, the soft track limit, and the
//! rule that track flags never touch items (INV-29). Also the P1 edit commands' refusal to
//! edit the magnetic track until the placement work package ripples (documented stopgap).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, P, assert_code, assert_invalid, id, item_id};
use cutweave_model::{ItemId, MediaId, Project, SequenceId, TrackId};
use serde_json::{Value, json};

/// A project with V1/A1, plus a 20 s video with sound.
fn setup() -> (H, P, MediaId) {
    let (mut h, p) = H::project();
    let video = h.add_video("/m/v.mp4", 20, true);
    (h, p, video)
}

/// `video` with its audio on `p.video` and `p.audio` at frame `at`, `dur` frames long, reading
/// from frame `src`.
fn av(h: &mut H, p: P, media: MediaId, at: i64, dur: i64, src: i64) -> (ItemId, ItemId) {
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": media,
               "at": format!("f:{at}"), "duration": format!("f:{dur}"),
               "source_in": format!("f:{src}")}),
    );
    (item_id(&out["clips"][0]), item_id(&out["clips"][1]))
}

fn start_of(h: &H, item: ItemId) -> i64 {
    h.doc().find_item(&item).unwrap().item.start().flicks() / FRAME
}

const FRAME: i64 = 23_520_000;

/// INV-6 and INV-281: at most one magnetic track per sequence, and the document is valid
/// (which includes INV-7, gap-free).
fn assert_one_magnetic(h: &H, sequence: SequenceId) {
    let doc = h.doc();
    let magnetic: Vec<TrackId> = doc.sequences[&sequence]
        .tracks
        .values()
        .filter(|t| t.magnetic)
        .map(|t| t.id)
        .collect();
    assert!(magnetic.len() <= 1, "{magnetic:?}");
    doc.validate().unwrap();
}

fn magnetic_of(h: &H, track: TrackId) -> bool {
    h.doc()
        .sequences
        .values()
        .find_map(|s| s.tracks.get(&track))
        .unwrap()
        .magnetic
}

// ----- the spec fixtures -----------------------------------------------------------------------

#[test]
fn fixture_set_magnetic_audio_invalid() {
    let (mut h, p, _) = setup();
    assert_invalid(
        h.run(
            "track.set_magnetic",
            json!({"track": p.audio, "magnetic": true}),
        ),
        "/track",
    );
}

#[test]
fn fixture_set_magnetic_gaps_invalid_without_close_gaps() {
    let (mut h, p, video) = setup();
    av(&mut h, p, video, 10, 10, 0);
    let before = h.doc();
    let r = h.run(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    assert_invalid(r, "/track");
    assert_eq!(h.doc(), before);
    // The message names the gap and the way out.
    let err = h
        .run(
            "track.set_magnetic",
            json!({"track": p.video, "magnetic": true}),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("gap") && err.contains("close_gaps"), "{err}");
}

#[test]
fn fixture_set_magnetic_close_gaps_ripples_linked_partners() {
    let (mut h, p, video) = setup();
    let (x, xa) = av(&mut h, p, video, 10, 10, 0);
    let (y, ya) = av(&mut h, p, video, 40, 10, 40);
    let out = h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true, "close_gaps": true}),
    );
    assert_eq!(out["magnetic"], true);
    assert!(out.get("previous").is_none());
    assert_eq!(
        [x, y, xa, ya].map(|i| start_of(&h, i)),
        [0, 10, 0, 10],
        "the track is packed from 0 and the audio follows"
    );
    assert!(magnetic_of(&h, p.video));
    // The report is the exact diff: the four items moved, nothing else.
    let moved: Vec<ItemId> = out["report"]["moved"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| item_id(&m["item"]))
        .collect();
    let mut expected = vec![x, y, xa, ya];
    expected.sort();
    assert_eq!(moved, expected);
    for list in [
        "created",
        "removed",
        "split",
        "trimmed",
        "relinked",
        "created_tracks",
    ] {
        assert_eq!(out["report"][list], json!([]), "{list}");
    }
    assert_one_magnetic(&h, p.sequence);
}

#[test]
fn close_gaps_also_closes_the_space_before_the_first_item() {
    let (mut h, p, video) = setup();
    let (x, _) = av(&mut h, p, video, 7, 10, 0);
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true, "close_gaps": true}),
    );
    assert_eq!(start_of(&h, x), 0);
}

#[test]
fn fixture_set_magnetic_locked_close_gaps_is_refused() {
    let (mut h, p, video) = setup();
    av(&mut h, p, video, 10, 10, 0);
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    let before = h.doc();
    let err = h
        .run(
            "track.set_magnetic",
            json!({"track": p.video, "magnetic": true, "close_gaps": true}),
        )
        .unwrap_err();
    assert_eq!(err.code(), "locked");
    assert!(err.to_string().contains(&p.video.to_string()), "{err}");
    assert_eq!(h.doc(), before);
    // Without close_gaps nothing would change, so the gap (not the lock) is the problem.
    assert_invalid(
        h.run(
            "track.set_magnetic",
            json!({"track": p.video, "magnetic": true}),
        ),
        "/track",
    );
}

#[test]
fn a_locked_partner_track_blocks_close_gaps() {
    let (mut h, p, video) = setup();
    av(&mut h, p, video, 10, 10, 0);
    h.ok("track.set", json!({"track": p.audio, "locked": true}));
    let err = h
        .run(
            "track.set_magnetic",
            json!({"track": p.video, "magnetic": true, "close_gaps": true}),
        )
        .unwrap_err();
    assert_eq!(err.code(), "locked");
    assert!(err.to_string().contains(&p.audio.to_string()), "{err}");
}

#[test]
fn fixture_set_magnetic_switches_the_main_track() {
    let (mut h, p, video) = setup();
    let v2: TrackId = id(&h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    )["track"]);
    // P on V2 at 0, X on V1 at 0.
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": v2, "media": video, "at": "f:0",
               "duration": "f:20", "link": false}),
    );
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": video, "at": "f:0",
               "duration": "f:30", "link": false}),
    );
    h.ok("track.set_magnetic", json!({"track": v2, "magnetic": true}));
    assert!(magnetic_of(&h, v2));
    let out = h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    assert_eq!(id::<TrackId>(&out["previous"]), v2);
    assert!(!magnetic_of(&h, v2));
    assert!(magnetic_of(&h, p.video));
    assert_eq!(
        out["report"],
        serde_json::to_value(cutweave_ops::report::EditReport::default()).unwrap()
    );
    assert_one_magnetic(&h, p.sequence);
}

#[test]
fn closing_gaps_that_would_collide_fails_naming_the_items() {
    let (mut h, p, video) = setup();
    let (x, xa) = av(&mut h, p, video, 10, 10, 0);
    // An audio clip before the gap that does not shift: x' [0, 10) would overlap it.
    let audio = h.add_audio("/m/a.wav", 20);
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.audio, "media": audio, "at": "f:0",
               "duration": "f:10"}),
    );
    let before = h.doc();
    let err = h
        .run(
            "track.set_magnetic",
            json!({"track": p.video, "magnetic": true, "close_gaps": true}),
        )
        .unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(err.to_string().contains(&xa.to_string()), "{err}");
    assert_eq!(h.doc(), before);
    let _ = x;
}

#[test]
fn turning_the_magnet_off_clears_only_the_flag_even_when_locked() {
    let (mut h, p, video) = setup();
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": video, "at": "f:0",
               "duration": "f:30", "link": false}),
    );
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    let out = h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": false}),
    );
    assert_eq!(out["magnetic"], false);
    assert!(!magnetic_of(&h, p.video));
    // Again: nothing to do, no commit.
    let commits = h.commits();
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": false}),
    );
    assert_eq!(h.commits(), commits + 1, "a no-op still answers");
    assert_eq!(
        h.last.as_ref().unwrap().patch.0.len(),
        0,
        "but changes nothing"
    );
}

#[test]
fn making_an_empty_or_gap_free_track_magnetic_needs_no_close_gaps() {
    let (mut h, p, video) = setup();
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": false}),
    );
    for at in [0, 30] {
        h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": video, "at": format!("f:{at}"),
                   "duration": "f:30", "link": false}),
        );
    }
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    assert_one_magnetic(&h, p.sequence);
}

#[test]
fn unknown_tracks_and_malformed_params_are_reported() {
    let (mut h, _, _) = setup();
    assert_code(
        h.run(
            "track.set_magnetic",
            json!({"track": TrackId::from_u128(9), "magnetic": true}),
        ),
        "not_found",
    );
    assert_invalid(h.run("track.set_magnetic", json!({"magnetic": true})), "");
}

// ----- the P1 edits stop at the magnetic track (until WP-2A.03) ---------------------------------

#[test]
fn p1_edits_refuse_the_magnetic_track_instead_of_leaving_a_gap() {
    let (mut h, p, video) = setup();
    let first = item_id(
        &h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": video, "at": "f:0",
                   "duration": "f:30", "link": false}),
        )["clips"][0],
    );
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    for (name, params) in [
        ("item.delete", json!({"items": [first]})),
        ("item.move", json!({"item": first, "start": "f:5"})),
        (
            "clip.trim",
            json!({"item": first, "edge": "end", "delta": "-0.2s"}),
        ),
        (
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": video, "at": "f:30",
                   "duration": "f:5", "link": false}),
        ),
    ] {
        let before = h.doc();
        let r = h.run(name, params);
        assert_code(r, "unsupported");
        assert_eq!(h.doc(), before, "{name}");
    }
    // Splitting keeps the track gap-free, so it still works.
    h.ok("clip.split", json!({"item": first, "at": "f:10"}));
    assert_one_magnetic(&h, p.sequence);
}

// ----- track.reorder ---------------------------------------------------------------------------

fn names(h: &H, seq: SequenceId) -> Vec<String> {
    h.doc().sequences[&seq]
        .tracks_in_order()
        .map(|t| t.name.clone())
        .collect()
}

#[test]
fn reorder_moves_a_track_and_clamps() {
    let (mut h, p, _) = setup();
    h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    );
    h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "audio"}),
    );
    assert_eq!(names(&h, p.sequence), ["V2", "V1", "A1", "A2"]);
    let out = h.ok("track.reorder", json!({"track": p.audio, "index": 0}));
    assert_eq!(out["index"], 0);
    assert_eq!(names(&h, p.sequence), ["A1", "V2", "V1", "A2"]);
    let out = h.ok("track.reorder", json!({"track": p.audio, "index": 99}));
    assert_eq!(out["index"], 3);
    assert_eq!(names(&h, p.sequence), ["V2", "V1", "A2", "A1"]);
    // Same place: no change.
    h.ok("track.reorder", json!({"track": p.audio, "index": 3}));
    assert_eq!(h.last.as_ref().unwrap().patch.0.len(), 0);
    assert_code(
        h.run(
            "track.reorder",
            json!({"track": TrackId::from_u128(3), "index": 0}),
        ),
        "not_found",
    );
}

#[test]
fn reorder_works_on_a_locked_track_and_never_touches_items() {
    let (mut h, p, video) = setup();
    av(&mut h, p, video, 5, 10, 0);
    let before: Project = h.doc();
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    h.ok("track.reorder", json!({"track": p.video, "index": 1}));
    let after = h.doc();
    assert_eq!(items_of(&before), items_of(&after));
}

/// Every item with its track and timing, ignoring track order and flags.
fn items_of(doc: &Project) -> Vec<(TrackId, cutweave_model::Item)> {
    let mut v: Vec<_> = doc
        .sequences
        .values()
        .flat_map(|s| s.tracks.values())
        .flat_map(|t| t.items.iter().map(|i| (t.id, i.clone())))
        .collect();
    v.sort_by_key(|(_, i)| i.id());
    v
}

// ----- track.set_height ------------------------------------------------------------------------

#[test]
fn set_height_sets_resets_and_validates() {
    let (mut h, p, video) = setup();
    av(&mut h, p, video, 0, 10, 0);
    let out = h.ok("track.set_height", json!({"track": p.video, "height": 64}));
    assert_eq!(out["height"], 64);
    let doc = h.doc();
    assert_eq!(doc.sequences[&p.sequence].tracks[&p.video].height, Some(64));
    let info = h
        .query("track.list", json!({"sequence": p.sequence}))
        .unwrap();
    assert_eq!(info["tracks"][0]["height"], 64);
    h.ok(
        "track.set_height",
        json!({"track": p.video, "height": null}),
    );
    assert_eq!(h.doc().sequences[&p.sequence].tracks[&p.video].height, None);
    for bad in [0, 7, 513] {
        assert_invalid(
            h.run("track.set_height", json!({"track": p.video, "height": bad})),
            "/height",
        );
    }
    // Locked tracks can still be resized; items are untouched.
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    let before = items_of(&h.doc());
    h.ok("track.set_height", json!({"track": p.video, "height": 20}));
    assert_eq!(items_of(&h.doc()), before);
}

// ----- INV-29 and the soft limit ----------------------------------------------------------------

#[test]
fn flags_never_change_item_positions() {
    let (mut h, p, video) = setup();
    av(&mut h, p, video, 3, 10, 0);
    av(&mut h, p, video, 20, 10, 20);
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true, "close_gaps": true}),
    );
    let before = items_of(&h.doc());
    for params in [
        json!({"track": p.video, "muted": true}),
        json!({"track": p.audio, "solo": true, "muted": true}),
        json!({"track": p.video, "hidden": true}),
        json!({"track": p.audio, "gain": -12.0}),
        json!({"track": p.video, "name": "Main"}),
    ] {
        h.ok("track.set", params);
        assert_eq!(items_of(&h.doc()), before);
    }
    h.ok("track.set_height", json!({"track": p.audio, "height": 100}));
    h.ok("track.reorder", json!({"track": p.audio, "index": 0}));
    assert_eq!(items_of(&h.doc()), before);
    assert_one_magnetic(&h, p.sequence);
}

#[test]
fn many_tracks_are_allowed_with_a_soft_warning_above_64() {
    let (mut h, p, _) = setup();
    let mut warned_at = None;
    for n in 3..=70 {
        let out = h.ok(
            "track.add",
            json!({"sequence": p.sequence, "kind": "audio"}),
        );
        assert_eq!(out["track_count"], n);
        if !out["warnings"].as_array().unwrap().is_empty() && warned_at.is_none() {
            warned_at = Some(n);
        }
    }
    assert_eq!(warned_at, Some(65));
}

// ----- track.remove with attachments ----------------------------------------------------------

#[test]
fn removing_a_track_detaches_the_items_attached_to_its_items() {
    let (mut h, p, video) = setup();
    let v2: TrackId = id(&h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    )["track"]);
    let (parent, _) = av(&mut h, p, video, 0, 30, 0);
    let child = item_id(
        &h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": v2, "media": video, "at": "f:10",
                   "duration": "f:5", "link": false}),
        )["clips"][0],
    );
    // No command attaches yet (WP-2A.05): build the attachment in the document.
    let mut doc = h.doc();
    {
        let seq = doc.sequences.get_mut(&p.sequence).unwrap();
        seq.track_mut(&v2)
            .unwrap()
            .items
            .update(&child, |i| i.set_attached_to(Some(parent)));
    }
    doc.validate().unwrap();
    let mut h = H::with_doc(doc);
    h.ok("track.remove", json!({"track": p.video}));
    let doc = h.doc();
    let child_now = doc.find_item(&child).unwrap().item;
    assert_eq!(child_now.attached_to(), None, "detached, in place");
    assert_eq!(child_now.start().flicks(), 10 * FRAME);
    doc.validate().unwrap();
}

#[test]
fn removing_a_track_whose_children_sit_on_a_locked_track_is_locked() {
    let (mut h, p, video) = setup();
    let v2: TrackId = id(&h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    )["track"]);
    let (parent, _) = av(&mut h, p, video, 0, 30, 0);
    let child = item_id(
        &h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": v2, "media": video, "at": "f:10",
                   "duration": "f:5", "link": false}),
        )["clips"][0],
    );
    let mut doc = h.doc();
    {
        let seq = doc.sequences.get_mut(&p.sequence).unwrap();
        let t = seq.track_mut(&v2).unwrap();
        t.items.update(&child, |i| i.set_attached_to(Some(parent)));
        t.locked = true;
    }
    let mut h = H::with_doc(doc);
    let err = h
        .run("track.remove", json!({"track": p.video}))
        .unwrap_err();
    assert_eq!(err.code(), "locked");
    assert!(err.to_string().contains(&v2.to_string()), "{err}");
}

#[test]
fn track_list_describes_the_stack() {
    let (mut h, p, _) = setup();
    h.ok(
        "track.set",
        json!({"track": p.audio, "muted": true, "gain": -3.0}),
    );
    let out: Value = h
        .query("track.list", json!({"sequence": p.sequence}))
        .unwrap();
    let tracks = out["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0]["name"], "V1");
    assert_eq!(tracks[0]["index"], 0);
    assert_eq!(tracks[1]["muted"], true);
    assert_eq!(tracks[1]["gain"], -3.0);
    assert_eq!(tracks[1]["magnetic"], false);
    assert_code(
        h.query("track.list", json!({"sequence": SequenceId::from_u128(4)})),
        "not_found",
    );
}
