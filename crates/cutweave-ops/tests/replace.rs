// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.11: `clip.replace`. A keyframed clip keeps everything but `media`, `source_in` and
//! `duration`; the three modes; the main track stays gapless and a free track never overlaps;
//! kind rules; linked sound; locks. Every successful run also checks undo/redo identity and
//! `Project::validate` (the harness); the random sequences with this command run in
//! `registry_props.rs` and `trim_props.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::many_single_char_names
)]

mod common;

use common::{H, P, assert_code, assert_invalid, clip_of, id, insert_av, item_id, track_of};
use cutweave_model::{ItemId, MediaId, TrackId};
use serde_json::{Value, json};

const S: i64 = 705_600_000;

fn put(h: &mut H, p: P, track: TrackId, media: MediaId, at: i64, src: i64, dur: i64) -> ItemId {
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": track, "media": media,
               "at": at * S, "source_in": src * S, "duration": dur * S, "link": false}),
    );
    item_id(&out["clips"][0])
}

fn lane(h: &H, track: TrackId) -> Vec<(ItemId, i64, i64)> {
    let doc = h.doc();
    let t = doc
        .sequences
        .values()
        .find_map(|s| s.track(&track))
        .unwrap();
    t.items
        .iter()
        .map(|i| {
            (
                i.id(),
                i.start().flicks(),
                i.start().flicks() + i.duration().flicks(),
            )
        })
        .collect()
}

fn replace(
    h: &mut H,
    item: ItemId,
    media: MediaId,
    extra: &Value,
) -> Result<Value, cutweave_commands::CmdError> {
    let mut params = json!({"item": item, "media": media});
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    h.run("clip.replace", params)
}

fn add_track(h: &mut H, p: P, kind: &str) -> TrackId {
    id(&h.ok("track.add", json!({"sequence": p.sequence, "kind": kind}))["track"])
}

/// The clip as JSON without the three fields a replace may change.
fn but_media(h: &H, item: ItemId) -> Value {
    let mut v = serde_json::to_value(clip_of(h, item)).unwrap();
    for k in ["media", "source_in", "duration"] {
        v.as_object_mut().unwrap().remove(k);
    }
    v
}

fn timing(h: &H, item: ItemId) -> (i64, i64, i64) {
    let c = clip_of(h, item);
    (c.start.flicks(), c.duration.flicks(), c.source_in.flicks())
}

#[test]
fn a_keyframed_clip_keeps_everything_but_media_source_in_and_duration() {
    let (mut h, p) = H::project();
    let old = h.add_video("/m/old.mp4", 30, false);
    let new = h.add_video("/m/new.mp4", 60, false);
    let x = put(&mut h, p, p.video, old, 3, 5, 10);
    h.ok(
        "keyframe.set",
        json!({"item": x, "prop": "transform.scale", "keys": [
            {"t": "0s", "value": [1.0, 1.0]}, {"t": "2s", "value": [2.0, 2.0]}]}),
    );
    h.ok(
        "keyframe.set",
        json!({"item": x, "prop": "opacity", "keys": [
            {"t": "0s", "value": 0.2}, {"t": "4s", "value": 1.0}]}),
    );
    h.ok(
        "item.set_props",
        json!({"item": x, "props": {"name": "Hero", "crop": {"left": 0.1}, "blend_mode": "multiply"}}),
    );
    h.ok(
        "clip.set_speed",
        json!({"item": x, "speed": 2, "keep": "duration"}),
    );
    let before = but_media(&h, x);
    let (start, len, _) = timing(&h, x);
    let out = replace(&mut h, x, new, &json!({})).unwrap();
    assert_eq!(item_id(&out["item"]), x);
    assert_eq!(out["shortened"], json!(false));
    assert_eq!(out["speed_reset"], json!(false));
    assert_eq!(but_media(&h, x), before);
    let c = clip_of(&h, x);
    assert_eq!(c.media, new);
    assert_eq!((c.start.flicks(), c.duration.flicks()), (start, len));
    assert_eq!(
        c.source_in.flicks(),
        0,
        "keep_duration starts at the new media's start"
    );
    // Shortened: the new media is shorter than what is needed.
    let short = h.add_video("/m/short.mp4", 6, false);
    let out = replace(&mut h, x, short, &json!({})).unwrap();
    assert_eq!(out["shortened"], json!(true));
    assert_eq!(
        but_media(&h, x),
        before,
        "keys and settings stay when it shortens"
    );
    assert_eq!(
        clip_of(&h, x).duration.flicks(),
        3 * S,
        "6 s of media at 2x is 3 s"
    );
}

#[test]
fn keep_duration_starts_at_source_in_and_shortens_to_what_is_left() {
    let (mut h, p) = H::project();
    let old = h.add_video("/m/old.mp4", 30, false);
    let new = h.add_video("/m/new.mp4", 10, false);
    let x = put(&mut h, p, p.video, old, 0, 0, 6);
    let out = replace(&mut h, x, new, &json!({"source_in": "7s"})).unwrap();
    assert_eq!(out["shortened"], json!(true));
    assert_eq!(timing(&h, x), (0, 3 * S, 7 * S));
    // Past the end of the media.
    assert_invalid(
        replace(&mut h, x, old, &json!({"source_in": "40s"})),
        "/source_in",
    );
    assert_invalid(
        replace(&mut h, x, old, &json!({"source_in": "-1s"})),
        "/source_in",
    );
}

#[test]
fn keep_source_in_plays_from_where_the_old_media_was() {
    let (mut h, p) = H::project();
    let old = h.add_video("/m/old.mp4", 30, false);
    let new = h.add_video("/m/new.mp4", 12, false);
    let x = put(&mut h, p, p.video, old, 0, 4, 6);
    let out = replace(&mut h, x, new, &json!({"mode": "keep_source_in"})).unwrap();
    assert_eq!(out["shortened"], json!(false));
    assert_eq!(timing(&h, x), (0, 6 * S, 4 * S));
    let y = put(&mut h, p, p.video, old, 10, 9, 6);
    let out = replace(&mut h, y, new, &json!({"mode": "keep_source_in"})).unwrap();
    assert_eq!(out["shortened"], json!(true));
    assert_eq!(timing(&h, y), (10 * S, 3 * S, 9 * S));
    assert_invalid(
        replace(
            &mut h,
            y,
            old,
            &json!({"mode": "keep_source_in", "source_in": "1s"}),
        ),
        "/source_in",
    );
}

#[test]
fn fit_to_source_changes_the_length_and_the_main_track_ripples_gapless() {
    let (mut h, p) = H::project();
    let old = h.add_video("/m/old.mp4", 60, false);
    let new = h.add_video("/m/new.mp4", 8, false);
    let big = h.add_video("/m/big.mp4", 20, false);
    let a = put(&mut h, p, p.video, old, 0, 0, 4);
    let b = put(&mut h, p, p.video, old, 4, 10, 6);
    let c = put(&mut h, p, p.video, old, 10, 20, 2);
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    replace(&mut h, b, new, &json!({"mode": "fit_to_source"})).unwrap();
    assert_eq!(
        lane(&h, p.video),
        vec![(a, 0, 4 * S), (b, 4 * S, 12 * S), (c, 12 * S, 14 * S)]
    );
    replace(
        &mut h,
        b,
        big,
        &json!({"mode": "fit_to_source", "source_in": "2s"}),
    )
    .unwrap();
    assert_eq!(
        lane(&h, p.video),
        vec![(a, 0, 4 * S), (b, 4 * S, 22 * S), (c, 22 * S, 24 * S)]
    );
    // keep_duration shortening also closes up.
    let short = h.add_video("/m/short.mp4", 2, false);
    let out = replace(&mut h, b, short, &json!({})).unwrap();
    assert_eq!(out["shortened"], json!(true));
    assert_eq!(
        lane(&h, p.video),
        vec![(a, 0, 4 * S), (b, 4 * S, 6 * S), (c, 6 * S, 8 * S)]
    );
}

#[test]
fn on_a_free_track_growing_into_the_next_item_is_invalid() {
    let (mut h, p) = H::project();
    let old = h.add_video("/m/old.mp4", 60, false);
    let big = h.add_video("/m/big.mp4", 20, false);
    let v2 = add_track(&mut h, p, "video");
    let a = put(&mut h, p, v2, old, 0, 0, 4);
    let b = put(&mut h, p, v2, old, 6, 10, 4);
    assert_invalid(
        replace(&mut h, a, big, &json!({"mode": "fit_to_source"})),
        "/mode",
    );
    // Fits in the gap.
    let mid = h.add_video("/m/mid.mp4", 6, false);
    replace(&mut h, a, mid, &json!({"mode": "fit_to_source"})).unwrap();
    assert_eq!(lane(&h, v2), vec![(a, 0, 6 * S), (b, 6 * S, 10 * S)]);
    // Shrinking leaves the gap.
    let tiny = h.add_video("/m/tiny.mp4", 2, false);
    replace(&mut h, a, tiny, &json!({"mode": "fit_to_source"})).unwrap();
    assert_eq!(lane(&h, v2), vec![(a, 0, 2 * S), (b, 6 * S, 10 * S)]);
}

#[test]
fn video_to_still_resets_speed_and_audio_video_swaps_are_invalid() {
    let (mut h, p) = H::project();
    let vid = h.add_video("/m/v.mp4", 30, false);
    let img = h.add_image("/m/i.png");
    let aud = h.add_audio("/m/a.wav", 30);
    let x = put(&mut h, p, p.video, vid, 0, 2, 6);
    h.ok(
        "clip.set_speed",
        json!({"item": x, "speed": 2, "keep": "duration"}),
    );
    h.ok("clip.reverse", json!({"item": x, "reverse": true}));
    assert_invalid(
        replace(&mut h, x, img, &json!({"source_in": "1s"})),
        "/source_in",
    );
    let out = replace(&mut h, x, img, &json!({})).unwrap();
    assert_eq!(out["speed_reset"], json!(true));
    let c = clip_of(&h, x);
    assert!(c.speed.is_one() && !c.reverse);
    assert_eq!((c.duration.flicks(), c.source_in.flicks()), (6 * S, 0));
    // Back to video: allowed, speed stays at normal.
    replace(&mut h, x, vid, &json!({})).unwrap();
    // Sound and picture do not swap.
    assert_invalid(replace(&mut h, x, aud, &json!({})), "/media");
    let s = put(&mut h, p, p.audio, aud, 0, 0, 4);
    assert_invalid(replace(&mut h, s, vid, &json!({})), "/media");
    assert_invalid(replace(&mut h, s, img, &json!({})), "/media");
    // Same media, unknown ones.
    assert_invalid(replace(&mut h, x, vid, &json!({})), "/media");
    assert_code(
        h.run(
            "clip.replace",
            json!({"item": x, "media": "med_00000000000000000000000000000001"}),
        ),
        "not_found",
    );
    assert_code(
        h.run(
            "clip.replace",
            json!({"item": "itm_00000000000000000000000000000001", "media": vid}),
        ),
        "not_found",
    );
}

#[test]
fn linked_sound_is_replaced_with_the_picture_or_muted_and_unlinked() {
    let (mut h, p) = H::project();
    let old = h.add_video("/m/old.mp4", 20, true);
    let with = h.add_video("/m/with.mp4", 30, true);
    let without = h.add_video("/m/without.mp4", 30, false);
    let (v, a) = insert_av(&mut h, p, old, "0s");
    h.ok("audio.set_volume", json!({"item": a, "db": -6.0}));
    let before = but_media(&h, a);
    let out = replace(&mut h, v, with, &json!({})).unwrap();
    assert_eq!(out["replaced"], json!([v, a]));
    assert_eq!(out["muted"], json!([]));
    assert_eq!(clip_of(&h, a).media, with);
    assert_eq!(clip_of(&h, v).linked_group, clip_of(&h, a).linked_group);
    assert!(clip_of(&h, v).linked_group.is_some());
    assert_eq!(but_media(&h, a), before, "the sound keeps its volume");
    // No sound in the new media.
    let out = replace(&mut h, v, without, &json!({})).unwrap();
    assert_eq!(out["replaced"], json!([v]));
    assert_eq!(out["muted"], json!([a]));
    let sound = clip_of(&h, a);
    assert!(!sound.enabled);
    assert_eq!(sound.linked_group, None);
    assert_eq!(sound.media, with);
    assert_eq!(
        clip_of(&h, v).linked_group,
        None,
        "a single member is unlinked"
    );
    assert_eq!(track_of(&h, a), p.audio);
}

#[test]
fn a_locked_track_refuses_and_changes_nothing() {
    let (mut h, p) = H::project();
    let old = h.add_video("/m/old.mp4", 20, true);
    let new = h.add_video("/m/new.mp4", 30, true);
    let (v, _) = insert_av(&mut h, p, old, "0s");
    for track in [p.video, p.audio] {
        h.ok("track.set", json!({"track": track, "locked": true}));
        let before = h.doc();
        assert_code(replace(&mut h, v, new, &json!({})), "locked");
        assert_eq!(h.doc(), before);
        h.ok("track.set", json!({"track": track, "locked": false}));
    }
    // A clip linked to one that plays other media is refused.
    let other = h.add_video("/m/other.mp4", 20, true);
    let w = put(&mut h, p, p.video, old, 40, 0, 4);
    let z = put(&mut h, p, p.audio, other, 40, 0, 4);
    h.ok("item.link", json!({"items": [w, z]}));
    assert_invalid(replace(&mut h, w, new, &json!({})), "/item");
}

#[test]
fn it_is_a_catalog_command_that_validates_without_changing() {
    let (mut h, p) = H::project();
    let old = h.add_video("/m/old.mp4", 20, false);
    let new = h.add_video("/m/new.mp4", 30, false);
    let x = put(&mut h, p, p.video, old, 0, 0, 4);
    h.validate("clip.replace", json!({"item": x, "media": new}))
        .unwrap();
    assert_eq!(clip_of(&h, x).media, old);
    assert!(
        h.validate("clip.replace", json!({"item": x, "media": old}))
            .is_err()
    );
}
