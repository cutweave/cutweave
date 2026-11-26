// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2D.01: speed, reverse and freeze frames (timeline spec INV-47; keyframes §8.1). Exact
//! lengths, round trips, media limits, ripples on the main track and neighbours on free tracks,
//! linked partners, keyframes kept or scaled, reverse, reset, and CapCut freeze frames. Every
//! successful run also checks undo/redo identity and `Project::validate` (the harness); the
//! random sequences with these commands run in `registry_props.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{FRAME30, H, P, assert_code, assert_invalid, clip_of, id, insert_av, item_id};
use cutweave_model::{ItemId, MediaId, SourceMap, Speed, Time, TrackId, source_time};
use serde_json::{Value, json};

const S: i64 = 705_600_000;
const F: i64 = FRAME30;

/// A clip at `at` s playing `[src, src + dur)` s of `media` on `track`; returns the clips made.
fn put(h: &mut H, p: P, track: TrackId, media: MediaId, at: i64, src: i64, dur: i64) -> ItemId {
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": track, "media": media,
               "at": at * S, "source_in": src * S, "duration": dur * S}),
    );
    item_id(&out["clips"][0])
}

/// `(start, duration, source_in)` in flicks.
fn timing(h: &H, item: ItemId) -> (i64, i64, i64) {
    let c = clip_of(h, item);
    (c.start.flicks(), c.duration.flicks(), c.source_in.flicks())
}

fn speed_of(h: &H, item: ItemId) -> (u32, u32, bool) {
    let c = clip_of(h, item);
    (c.speed.num(), c.speed.den(), c.reverse)
}

fn set_speed(h: &mut H, item: ItemId, speed: Value) -> Value {
    h.ok("clip.set_speed", json!({"item": item, "speed": speed}))
}

/// A picture-only video of `secs` seconds.
fn mute(h: &mut H, secs: i64) -> MediaId {
    h.add_video("/m/mute.mp4", secs, false)
}

fn add_track(h: &mut H, p: P, kind: &str) -> TrackId {
    id(&h.ok("track.add", json!({"sequence": p.sequence, "kind": kind}))["track"])
}

/// The items of a track, `(id, start, end)` in start order.
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

fn gapless_from_zero(items: &[(ItemId, i64, i64)]) -> bool {
    let mut at = 0;
    for (_, s, e) in items {
        if *s != at {
            return false;
        }
        at = *e;
    }
    true
}

// ----- exact lengths --------------------------------------------------------------------------

#[test]
fn twice_as_fast_over_ten_seconds_is_five_seconds_exactly() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 60);
    let x = put(&mut h, p, p.video, m, 0, 3, 10);
    let out = set_speed(&mut h, x, json!(2));
    assert_eq!(timing(&h, x), (0, 5 * S, 3 * S));
    assert_eq!(speed_of(&h, x), (2, 1, false));
    assert_eq!(out["items"][0]["duration"], json!(5 * S));
    assert_eq!(out["items"][0]["speed"], json!({"num": 2, "den": 1}));
    // The media played is unchanged: 10 s from 3 s.
    assert_eq!(SourceMap::of(&clip_of(&h, x)).source_out(), 13 * S);
}

#[test]
fn a_third_of_normal_speed_is_exact() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 60);
    let x = put(&mut h, p, p.video, m, 0, 0, 10);
    set_speed(&mut h, x, json!("1/3"));
    assert_eq!(timing(&h, x), (0, 30 * S, 0));
    assert_eq!(speed_of(&h, x), (1, 3, false));
    assert_eq!(SourceMap::of(&clip_of(&h, x)).source_out(), 10 * S);
    set_speed(&mut h, x, json!({"num": 1, "den": 1}));
    assert_eq!(timing(&h, x), (0, 10 * S, 0), "back to 10 s, to the flick");
}

#[test]
fn speeds_are_given_as_numbers_text_or_fractions_and_checked() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 600);
    let x = put(&mut h, p, p.video, m, 0, 0, 10);
    for (input, want) in [
        (json!(0.5), (1, 2)),
        (json!("2x"), (2, 1)),
        (json!("50%"), (1, 2)),
        (json!(1.25), (5, 4)),
        (json!({"num": 4, "den": 8}), (1, 2)),
        (json!("1"), (1, 1)),
    ] {
        set_speed(&mut h, x, input.clone());
        let (n, d, _) = speed_of(&h, x);
        assert_eq!((n, d), want, "{input}");
    }
    for bad in [json!(0), json!(0.05), json!(101), json!("fast"), json!(-2)] {
        let r = h.run("clip.set_speed", json!({"item": x, "speed": bad.clone()}));
        assert!(r.is_err(), "{bad}");
    }
    assert_invalid(
        h.run("clip.set_speed", json!({"item": x, "speed": "1/30"})),
        "/speed",
    );
    assert_invalid(h.run("clip.set_speed", json!({"speed": 2})), "");
}

/// WP-2D.01 acceptance: no drift over 1,000 round trips. From normal speed, `set_speed(a)` then
/// `set_speed(1)` restores the length to the flick whenever the first change lands on the frame
/// grid (every slow-down, and speed-ups that divide the length); otherwise the first trip rounds
/// once and every later trip returns exactly to that state.
#[test]
fn a_thousand_speed_round_trips_never_drift() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 3600);
    let speeds = [
        (1, 10),
        (1, 8),
        (1, 4),
        (1, 3),
        (1, 2),
        (2, 3),
        (3, 4),
        (5, 4),
        (3, 2),
        (2, 1),
        (3, 1),
        (4, 1),
        (5, 1),
        (7, 1),
        (8, 1),
        (10, 1),
    ];
    let mut rng = 0x2D01_u64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let x = put(&mut h, p, p.video, m, 0, 5, 10);
    let mut exact = 0;
    for trip in 0..1000 {
        // A fresh length now and then (1 to 600 frames), on the frame grid.
        if trip % 50 == 0 {
            let frames = i64::try_from(next() % 600).unwrap() + 1;
            h.ok(
                "clip.trim",
                json!({"item": x, "edge": "end", "to": frames * F}),
            );
        }
        let before = timing(&h, x);
        let (num, den) = speeds[usize::try_from(next() % 16).unwrap()];
        set_speed(&mut h, x, json!({"num": num, "den": den}));
        let landed = clip_of(&h, x).duration.flicks();
        set_speed(&mut h, x, json!(1));
        let after = timing(&h, x);
        let len = before.1;
        let on_grid = (i128::from(len) * i128::from(den)) % (i128::from(num) * i128::from(F)) == 0;
        if num <= den || on_grid {
            assert_eq!(after, before, "trip {trip}: {num}/{den} on {len}");
            exact += 1;
        } else {
            // Rounded once: the next trip from here returns here exactly.
            assert_eq!(after.2, before.2, "trip {trip}: source_in stays");
            set_speed(&mut h, x, json!({"num": num, "den": den}));
            assert_eq!(clip_of(&h, x).duration.flicks(), landed, "trip {trip}");
            set_speed(&mut h, x, json!(1));
            assert_eq!(timing(&h, x), after, "trip {trip}: no drift");
        }
    }
    assert!(exact > 600, "most trips are exact: {exact}");
}

// ----- keep: duration and the media ----------------------------------------------------------

#[test]
fn keeping_the_duration_plays_more_media_and_stops_at_its_end() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 30);
    let x = put(&mut h, p, p.video, m, 0, 4, 10);
    h.ok(
        "clip.set_speed",
        json!({"item": x, "speed": 2, "keep": "duration"}),
    );
    assert_eq!(timing(&h, x), (0, 10 * S, 4 * S));
    assert_eq!(SourceMap::of(&clip_of(&h, x)).source_out(), 24 * S);
    let r = h.run(
        "clip.set_speed",
        json!({"item": x, "speed": 3, "keep": "duration"}),
    );
    match r {
        Err(cutweave_commands::CmdError::Invalid { path, msg }) => {
            assert_eq!(path, "/speed");
            assert!(msg.contains("2.60x"), "{msg}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn keeping_the_in_point_near_the_media_end_rounds_the_length_down() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 10);
    // 31 frames that end exactly at the media end; at 2x the nearest length (16 frames, ties
    // later) would read past it, so the clip gets 15.
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0,
               "source_in": 10 * S - 31 * F, "duration": 31 * F}),
    );
    let x = item_id(&out["clips"][0]);
    let c = clip_of(&h, x);
    assert_eq!(SourceMap::of(&c).source_out(), 10 * S);
    set_speed(&mut h, x, json!(2));
    let c = clip_of(&h, x);
    assert_eq!(c.duration.flicks(), 15 * F);
    assert!(SourceMap::of(&c).source_out() <= 10 * S);
}

// ----- ripple and neighbours -------------------------------------------------------------------

#[test]
fn on_the_main_track_a_speed_change_ripples_and_stays_gapless() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 60);
    let a = put(&mut h, p, p.video, m, 0, 0, 4);
    let b = put(&mut h, p, p.video, m, 4, 10, 6);
    let c = put(&mut h, p, p.video, m, 10, 20, 2);
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    set_speed(&mut h, b, json!(2));
    assert_eq!(
        lane(&h, p.video),
        vec![(a, 0, 4 * S), (b, 4 * S, 7 * S), (c, 7 * S, 9 * S)]
    );
    set_speed(&mut h, b, json!(0.5));
    assert_eq!(
        lane(&h, p.video),
        vec![(a, 0, 4 * S), (b, 4 * S, 16 * S), (c, 16 * S, 18 * S)]
    );
    assert!(gapless_from_zero(&lane(&h, p.video)));
}

#[test]
fn on_a_free_track_the_next_item_blocks_unless_ripple() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 60);
    let v2 = add_track(&mut h, p, "video");
    let a = put(&mut h, p, v2, m, 0, 0, 4);
    let b = put(&mut h, p, v2, m, 4, 10, 4);
    let r = h.run("clip.set_speed", json!({"item": a, "speed": 0.5}));
    match &r {
        Err(cutweave_commands::CmdError::Invalid { path, msg }) => {
            assert_eq!(path, "/speed");
            assert!(
                msg.contains(&b.to_string()) && msg.contains("ripple"),
                "{msg}"
            );
        }
        other => panic!("{other:?}"),
    }
    // Faster: a gap opens and nothing else moves.
    set_speed(&mut h, a, json!(2));
    assert_eq!(lane(&h, v2), vec![(a, 0, 2 * S), (b, 4 * S, 8 * S)]);
    h.ok(
        "clip.set_speed",
        json!({"item": a, "speed": 0.5, "ripple": true}),
    );
    assert_eq!(lane(&h, v2), vec![(a, 0, 8 * S), (b, 10 * S, 14 * S)]);
}

#[test]
fn linked_partners_take_the_same_speed_and_length() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/av.mp4", 20, true);
    let (v, a) = insert_av(&mut h, p, m, "0s");
    let out = set_speed(&mut h, v, json!(4));
    assert_eq!(out["items"].as_array().unwrap().len(), 2);
    assert_eq!(timing(&h, v), (0, 5 * S, 0));
    assert_eq!(timing(&h, a), (0, 5 * S, 0));
    assert_eq!(speed_of(&h, a), (4, 1, false));
    h.ok("clip.reverse", json!({"item": a}));
    assert_eq!(speed_of(&h, v), (4, 1, true));
}

#[test]
fn several_clips_on_the_main_track_change_in_one_step() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 60);
    let a = put(&mut h, p, p.video, m, 0, 0, 4);
    let b = put(&mut h, p, p.video, m, 4, 10, 4);
    let c = put(&mut h, p, p.video, m, 8, 20, 4);
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    let commits = h.commits();
    h.ok("clip.set_speed", json!({"items": [c, a], "speed": 2}));
    assert_eq!(h.commits(), commits + 1);
    assert_eq!(
        lane(&h, p.video),
        vec![(a, 0, 2 * S), (b, 2 * S, 6 * S), (c, 6 * S, 8 * S)]
    );
}

#[test]
fn a_locked_track_refuses_and_stills_have_no_speed() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 60);
    let x = put(&mut h, p, p.video, m, 0, 0, 4);
    let img = h.add_image("/m/still.png");
    let v2 = add_track(&mut h, p, "video");
    let s = put(&mut h, p, v2, img, 0, 0, 4);
    assert_code(
        h.run("clip.set_speed", json!({"item": s, "speed": 2})),
        "unsupported",
    );
    assert_code(h.run("clip.reverse", json!({"item": s})), "unsupported");
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    assert_code(
        h.run("clip.set_speed", json!({"item": x, "speed": 2})),
        "locked",
    );
}

// ----- keyframes ----------------------------------------------------------------------------

fn opacity_key_times(h: &H, item: ItemId) -> Vec<i64> {
    let c = serde_json::to_value(clip_of(h, item)).unwrap();
    c["visual"]["opacity"]["keyframes"]
        .as_array()
        .map(|a| a.iter().map(|k| k["t"].as_i64().unwrap()).collect())
        .unwrap_or_default()
}

#[test]
fn keyframes_stay_by_default_and_scale_on_request() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 60);
    let x = put(&mut h, p, p.video, m, 0, 0, 10);
    h.ok(
        "keyframe.set",
        json!({"item": x, "prop": "opacity", "keys": [
            {"t": 0, "value": 0.0}, {"t": 3 * S, "value": 1.0}, {"t": 9 * S, "value": 0.5}]}),
    );
    set_speed(&mut h, x, json!(2));
    assert_eq!(opacity_key_times(&h, x), vec![0, 3 * S, 9 * S], "kept");
    h.ok(
        "clip.set_speed",
        json!({"item": x, "speed": 1, "keyframes": "scale"}),
    );
    assert_eq!(
        opacity_key_times(&h, x),
        vec![0, 6 * S, 18 * S],
        "scaled by 2"
    );
    h.ok(
        "clip.set_speed",
        json!({"item": x, "speed": 3, "keyframes": "scale"}),
    );
    assert_eq!(
        opacity_key_times(&h, x),
        vec![0, 2 * S, 6 * S],
        "scaled by 1/3"
    );
}

#[test]
fn scaled_keys_snap_to_frames_and_the_later_of_two_wins() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 600);
    let x = put(&mut h, p, p.video, m, 0, 0, 10);
    h.ok(
        "keyframe.set",
        json!({"item": x, "prop": "opacity", "keys": [
            {"t": 0, "value": 0.0}, {"t": F, "value": 0.25}, {"t": 2 * F, "value": 0.75}]}),
    );
    h.ok(
        "clip.set_speed",
        json!({"item": x, "speed": 10, "keyframes": "scale"}),
    );
    // 1/10 of a frame and 2/10 snap to frame 0: the last key there wins.
    let c = serde_json::to_value(clip_of(&h, x)).unwrap();
    let keys = c["visual"]["opacity"]["keyframes"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert_eq!(keys[0]["t"], json!(0));
    assert_eq!(keys[0]["value"], json!(0.75));
}

// ----- reverse and reset --------------------------------------------------------------------

#[test]
fn reverse_plays_the_same_range_backwards_and_toggles() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 60);
    let x = put(&mut h, p, p.video, m, 2, 10, 5);
    let before = timing(&h, x);
    h.ok("clip.reverse", json!({"item": x}));
    assert_eq!(timing(&h, x), before);
    let c = clip_of(&h, x);
    assert!(c.reverse);
    // The first frame shown is the last of the range.
    assert_eq!(
        source_time(&c, Time::from_flicks(2 * S).unwrap()),
        Some(Time::from_flicks(15 * S - 1).unwrap())
    );
    h.ok("clip.reverse", json!({"item": x, "reverse": true}));
    assert!(clip_of(&h, x).reverse, "explicit true stays reversed");
    h.ok("clip.reverse", json!({"item": x}));
    assert!(!clip_of(&h, x).reverse, "toggled back");
}

#[test]
fn a_reversed_clip_keeps_its_first_frame_when_its_speed_changes() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 60);
    let x = put(&mut h, p, p.video, m, 0, 10, 10);
    h.ok("clip.reverse", json!({"item": x}));
    let first = |h: &H| source_time(&clip_of(h, x), Time::ZERO).unwrap();
    let shown = first(&h);
    set_speed(&mut h, x, json!(2));
    assert_eq!(first(&h), shown, "in_point");
    h.ok(
        "clip.set_speed",
        json!({"item": x, "speed": 0.5, "keep": "duration"}),
    );
    assert_eq!(first(&h), shown, "duration");
    assert_eq!(timing(&h, x).1, 5 * S);
    assert_eq!(timing(&h, x).2, 20 * S - (5 * S) / 2);
}

#[test]
fn reset_speed_plays_forwards_at_normal_speed_again() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 60);
    let x = put(&mut h, p, p.video, m, 0, 6, 10);
    set_speed(&mut h, x, json!(0.25));
    h.ok("clip.reverse", json!({"item": x}));
    assert_eq!(timing(&h, x), (0, 40 * S, 6 * S));
    h.ok("clip.reset_speed", json!({"item": x}));
    assert_eq!(timing(&h, x), (0, 10 * S, 6 * S));
    assert_eq!(speed_of(&h, x), (1, 1, false));
    let out = h.ok("clip.reset_speed", json!({"item": x}));
    assert_eq!(out["report"]["trimmed"], json!([]), "idempotent");
}

// ----- trims and splits at speed (INV-47) ------------------------------------------------------

#[test]
fn trims_and_splits_follow_the_speed() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 60);
    let x = put(&mut h, p, p.video, m, 0, 0, 10);
    set_speed(&mut h, x, json!(2));
    h.ok("clip.trim", json!({"item": x, "edge": "start", "delta": S}));
    assert_eq!(
        timing(&h, x),
        (S, 4 * S, 2 * S),
        "a head trim of 1 s skips 2 s"
    );
    let out = h.ok("clip.split", json!({"item": x, "at": 3 * S}));
    let right = item_id(&out["right"]);
    assert_eq!(timing(&h, right), (3 * S, 2 * S, 6 * S));
    assert_eq!(speed_of(&h, right), (2, 1, false));
}

#[test]
fn splitting_a_reversed_clip_keeps_every_frame_it_shows() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 60);
    let x = put(&mut h, p, p.video, m, 0, 3, 9);
    h.ok("clip.set_speed", json!({"item": x, "speed": "3/2"}));
    h.ok("clip.reverse", json!({"item": x}));
    let whole = clip_of(&h, x);
    let out = h.ok("clip.split", json!({"item": x, "at": "f:47"}));
    let right = clip_of(&h, item_id(&out["right"]));
    let left = clip_of(&h, x);
    for f in 0..whole.duration.flicks() / F {
        let t = Time::from_flicks(f * F).unwrap();
        let half = if t < right.start { &left } else { &right };
        assert_eq!(source_time(half, t), source_time(&whole, t), "frame {f}");
    }
}

// ----- freeze frame -------------------------------------------------------------------------

#[test]
fn freezing_on_the_main_track_makes_two_halves_and_one_still_and_stays_gapless() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/av.mp4", 30, true);
    let (v, a) = insert_av(&mut h, p, m, "0s");
    let next = put(&mut h, p, p.video, m, 30, 0, 5);
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    h.ok(
        "keyframe.set",
        json!({"item": v, "prop": "opacity", "keys": [
            {"t": 0, "value": 0.0}, {"t": 10 * S, "value": 1.0}]}),
    );
    let before_items = h.doc().item_count();
    let out = h.ok("clip.freeze_frame", json!({"item": v, "at": 5 * S}));
    let freeze = item_id(&out["freeze"]);
    let right = item_id(&out["after"]);
    assert_eq!(item_id(&out["before"]), v);
    assert_eq!(out["at"], json!(5 * S));
    assert_eq!(out["duration"], json!(3 * S));
    assert_eq!(
        out["report"]["split"].as_array().unwrap().len(),
        2,
        "video and sound"
    );
    // Two halves of the clip and of its sound, plus one still.
    assert_eq!(h.doc().item_count(), before_items + 3);
    assert_eq!(
        lane(&h, p.video),
        vec![
            (v, 0, 5 * S),
            (freeze, 5 * S, 8 * S),
            (right, 8 * S, 33 * S),
            (next, 33 * S, 38 * S)
        ]
    );
    assert!(gapless_from_zero(&lane(&h, p.video)));
    let still = clip_of(&h, freeze);
    assert_eq!(still.freeze.unwrap().at_source.flicks(), 5 * S);
    assert_eq!(out["at_source"], json!(5 * S));
    assert_eq!(still.linked_group, None);
    // The still holds the value at the freeze time, without keys.
    let sv = serde_json::to_value(&still).unwrap();
    assert_eq!(sv["visual"]["opacity"], json!(0.5));
    // Every frame of the still is the frozen one.
    for t in [5 * S, 6 * S, 8 * S - 1] {
        assert_eq!(
            source_time(&still, Time::from_flicks(t).unwrap()),
            Some(Time::from_flicks(5 * S).unwrap())
        );
    }
    // The sound pauses for the freeze: its right half starts after it.
    let sound = lane(&h, p.audio);
    assert_eq!(sound.len(), 3, "two halves, then the next clip's sound");
    assert_eq!((sound[0].0, sound[0].2), (a, 5 * S));
    assert_eq!((sound[1].1, sound[1].2), (8 * S, 33 * S));
    assert_eq!((sound[2].1, sound[2].2), (33 * S, 38 * S));
    // Its right halves still play on from 5 s.
    assert_eq!(clip_of(&h, right).source_in.flicks(), 5 * S);
}

#[test]
fn freezing_the_first_frame_splits_nothing() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 30);
    let v2 = add_track(&mut h, p, "video");
    let x = put(&mut h, p, v2, m, 2, 7, 4);
    let out = h.ok(
        "clip.freeze_frame",
        json!({"item": x, "at": 2 * S, "duration": "f:15"}),
    );
    let freeze = item_id(&out["freeze"]);
    assert_eq!(out["before"], Value::Null);
    assert_eq!(item_id(&out["after"]), x);
    assert_eq!(out["report"]["split"], json!([]));
    assert_eq!(
        lane(&h, v2),
        vec![
            (freeze, 2 * S, 2 * S + 15 * F),
            (x, 2 * S + 15 * F, 6 * S + 15 * F)
        ]
    );
    assert_eq!(
        clip_of(&h, freeze).freeze.unwrap().at_source.flicks(),
        7 * S
    );
}

#[test]
fn a_freeze_of_a_fast_reversed_clip_holds_the_frame_on_screen() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 30);
    let x = put(&mut h, p, p.video, m, 0, 0, 20);
    set_speed(&mut h, x, json!(2));
    h.ok("clip.reverse", json!({"item": x}));
    let at = Time::from_flicks(3 * S).unwrap();
    let shown = source_time(&clip_of(&h, x), at).unwrap();
    let out = h.ok("clip.freeze_frame", json!({"item": x, "at": 3 * S}));
    let still = clip_of(&h, item_id(&out["freeze"]));
    assert_eq!(still.freeze.unwrap().at_source, shown);
    assert_eq!(still.speed, Speed::ONE);
    assert!(!still.reverse);
    // Speed and freeze frames do not mix.
    assert_code(
        h.run(
            "clip.set_speed",
            json!({"item": item_id(&out["freeze"]), "speed": 2}),
        ),
        "unsupported",
    );
}

#[test]
fn freeze_needs_a_picture_and_a_time_inside_the_clip() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 30);
    let x = put(&mut h, p, p.video, m, 0, 0, 4);
    assert_invalid(
        h.run("clip.freeze_frame", json!({"item": x, "at": 4 * S})),
        "/at",
    );
    assert_invalid(
        h.run(
            "clip.freeze_frame",
            json!({"item": x, "at": S, "duration": 0}),
        ),
        "/duration",
    );
    let music = h.add_audio("/m/a.wav", 10);
    let sound = put(&mut h, p, p.audio, music, 0, 0, 4);
    assert_code(
        h.run("clip.freeze_frame", json!({"item": sound, "at": S})),
        "unsupported",
    );
    let out = h.ok("clip.freeze_frame", json!({"item": x, "at": S}));
    assert_code(
        h.run(
            "clip.freeze_frame",
            json!({"item": item_id(&out["freeze"]), "at": S}),
        ),
        "unsupported",
    );
}

#[test]
fn item_get_reports_the_speed() {
    let (mut h, p) = H::project();
    let m = mute(&mut h, 30);
    let x = put(&mut h, p, p.video, m, 0, 2, 4);
    set_speed(&mut h, x, json!(0.5));
    h.ok("clip.reverse", json!({"item": x}));
    let got = h
        .query("item.get", json!({"item": x, "fields": ["source"]}))
        .unwrap();
    assert_eq!(got["source"]["speed"], json!({"num": 1, "den": 2}));
    assert_eq!(got["source"]["reverse"], json!(true));
    assert_eq!(got["source"]["source_out"], json!(6 * S));
    assert_eq!(got["source"]["freeze"], Value::Null);
}
