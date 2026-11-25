// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.04: the trim family's clamps and edge cases (timeline spec §3.7, §4.9–§4.13): every
//! limit with and without `clamp`, the messages of refused requests, stills and media of unknown
//! length, the sample grid, ripple collisions, partners, children and locks. The spec fixtures
//! run in `spec_fixtures.rs`, the properties in `trim_props.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::many_single_char_names
)]

mod common;

use common::{FRAME30, H, P, assert_code, assert_invalid, clip_of, id, item_id};
use cutweave_commands::CmdError;
use cutweave_model::{ItemId, MediaId, TrackId};
use cutweave_ops::media::FakeProbe;
use serde_json::{Value, json};

/// One frame at the harness's 30 fps.
const F: i64 = FRAME30;

/// A clip at frame `at` playing `[src, src + dur)` frames of `media` on `track`; returns the
/// clips the insert made (the picture, then its linked sound if any).
fn put(
    h: &mut H,
    p: P,
    track: TrackId,
    media: MediaId,
    at: i64,
    src: i64,
    dur: i64,
) -> Vec<ItemId> {
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": track, "media": media,
               "at": format!("f:{at}"), "source_in": format!("f:{src}"),
               "duration": format!("f:{dur}")}),
    );
    out["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(item_id)
        .collect()
}

/// `(start, duration, source_in)` of a clip, in frames.
fn frames(h: &H, item: ItemId) -> (i64, i64, i64) {
    let c = clip_of(h, item);
    (
        c.start.flicks() / F,
        c.duration.flicks() / F,
        c.source_in.flicks() / F,
    )
}

fn applied(out: &Value) -> (i64, bool) {
    (
        out["applied_delta"].as_i64().unwrap() / F,
        out["clamped"].as_bool().unwrap(),
    )
}

fn add_track(h: &mut H, p: P, kind: &str) -> TrackId {
    id(&h.ok("track.add", json!({"sequence": p.sequence, "kind": kind}))["track"])
}

/// A 10 s picture-only video.
fn mute(h: &mut H) -> MediaId {
    h.add_video("/m/mute.mp4", 10, false)
}

fn invalid_message(r: Result<Value, CmdError>, path: &str) -> String {
    match r {
        Err(CmdError::Invalid { path: p, msg }) => {
            assert_eq!(p, path, "{msg}");
            msg
        }
        other => panic!("expected Invalid at {path}, got {other:?}"),
    }
}

// ----- clip.trim ----------------------------------------------------------------------------

#[test]
fn a_clamped_start_trim_stops_at_time_zero() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let x = put(&mut h, p, p.video, m, 10, 100, 30)[0];
    let out = h.ok(
        "clip.trim",
        json!({"item": x, "edge": "start", "delta": -20 * F, "clamp": true}),
    );
    assert_eq!(applied(&out), (-10, true));
    assert_eq!(frames(&h, x), (0, 40, 90));
}

#[test]
fn a_clamped_trim_keeps_one_frame() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let x = put(&mut h, p, p.video, m, 0, 0, 30)[0];
    let out = h.ok(
        "clip.trim",
        json!({"item": x, "edge": "end", "delta": -100 * F, "clamp": true}),
    );
    assert_eq!(applied(&out), (-29, true));
    assert_eq!(frames(&h, x), (0, 1, 0));
    let out = h.ok(
        "clip.trim",
        json!({"item": x, "edge": "start", "to": "f:50", "clamp": true}),
    );
    assert_eq!(applied(&out), (0, true), "already one frame long");
    assert_eq!(out["report"]["trimmed"], json!([]));
}

#[test]
fn a_request_inside_the_limits_is_not_clamped() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let x = put(&mut h, p, p.video, m, 0, 0, 30)[0];
    let out = h.ok(
        "clip.trim",
        json!({"item": x, "edge": "end", "to": "f:40", "clamp": true}),
    );
    assert_eq!(applied(&out), (10, false));
    assert_eq!(out["items"][0]["duration"], json!(40 * F));
}

#[test]
fn a_refused_trim_names_the_limit_and_the_largest_delta() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let x = put(&mut h, p, p.video, m, 0, 0, 30)[0];
    let y = put(&mut h, p, p.video, m, 40, 40, 20)[0];
    let msg = invalid_message(
        h.run(
            "clip.trim",
            json!({"item": x, "edge": "end", "delta": "f:20"}),
        ),
        "/delta",
    );
    assert!(msg.contains("at most +10 frames"), "{msg}");
    assert!(msg.contains(&y.to_string()), "{msg}");
    // Y's head could read 40 frames earlier in its media, but X ends 10 frames before it.
    let msg = invalid_message(
        h.run(
            "clip.trim",
            json!({"item": y, "edge": "start", "delta": -45 * F}),
        ),
        "/delta",
    );
    assert!(msg.contains("at least -10 frames"), "{msg}");
}

#[test]
fn a_still_grows_without_a_media_bound_but_not_before_zero() {
    let (mut h, p) = H::project();
    let img = h.add_image("/m/logo.png");
    let x = put(&mut h, p, p.video, img, 10, 0, 30)[0];
    h.ok(
        "clip.trim",
        json!({"item": x, "edge": "end", "delta": "f:3000"}),
    );
    let out = h.ok(
        "clip.trim",
        json!({"item": x, "edge": "start", "delta": -50 * F, "clamp": true}),
    );
    assert_eq!(applied(&out), (-10, true));
    assert_eq!(
        frames(&h, x),
        (0, 3040, 0),
        "a still keeps its source position"
    );
}

/// A sound file whose length the probe did not find (a picture without one is a still).
fn unknown_length(h: &mut H) -> MediaId {
    let mut probed = FakeProbe::audio("/m/stream.wav", 10);
    probed.probe.duration = None;
    let out = h.ok(
        "media.register",
        json!({"path": "/m/stream.wav", "probe": probed.probe, "content_hash": probed.content_hash}),
    );
    id(&out["media_id"])
}

#[test]
fn media_of_unknown_length_has_no_end_bound_and_nothing_to_slip() {
    let (mut h, p) = H::project();
    let m = unknown_length(&mut h);
    let x = put(&mut h, p, p.audio, m, 100, 30, 30)[0];
    h.ok(
        "clip.trim",
        json!({"item": x, "edge": "end", "delta": "f:9000"}),
    );
    // Its start still cannot read before the media's start.
    let out = h.ok(
        "clip.trim",
        json!({"item": x, "edge": "start", "delta": -60 * F, "clamp": true}),
    );
    assert_eq!(applied(&out), (-30, true));
    assert_eq!(frames(&h, x), (70, 9060, 0));
    assert_invalid(
        h.run("clip.slip", json!({"item": x, "delta": "f:5"})),
        "/item",
    );
}

#[test]
fn fine_trims_snap_to_samples_on_audio_only() {
    let (mut h, p) = H::project();
    let music = h.add_audio("/m/music.wav", 30);
    let m = put(&mut h, p, p.audio, music, 0, 0, 60)[0];
    // 1 ms past frame 30 at 48 kHz is 48 samples: on the sample grid, not on a frame.
    let to = 30 * F + 705_600;
    let out = h.ok(
        "clip.trim",
        json!({"item": m, "edge": "end", "to": to, "fine": true}),
    );
    assert_eq!(clip_of(&h, m).duration.flicks(), to);
    assert_eq!(out["applied_delta"], json!(to - 60 * F));
    let picture = mute(&mut h);
    let v = put(&mut h, p, p.video, picture, 0, 0, 30)[0];
    assert_invalid(
        h.run(
            "clip.trim",
            json!({"item": v, "edge": "end", "delta": "f:1", "fine": true}),
        ),
        "/fine",
    );
}

#[test]
fn a_fine_edit_of_a_clip_linked_to_video_is_invalid() {
    let (mut h, p) = H::project();
    let av = h.add_video("/m/av.mp4", 10, true);
    let clips = put(&mut h, p, p.video, av, 0, 0, 30);
    assert_invalid(
        h.run(
            "clip.trim",
            json!({"item": clips[1], "edge": "end", "delta": "f:1", "fine": true}),
        ),
        "/fine",
    );
    assert_invalid(
        h.run(
            "clip.slip",
            json!({"item": clips[1], "delta": "f:1", "fine": true}),
        ),
        "/fine",
    );
}

#[test]
fn trimming_to_the_edge_changes_nothing() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let x = put(&mut h, p, p.video, m, 0, 0, 30)[0];
    let before = h.doc();
    let out = h.ok("clip.trim", json!({"item": x, "edge": "end", "to": "f:30"}));
    assert_eq!(h.doc(), before);
    assert_eq!(applied(&out), (0, false));
    assert_eq!(
        out["report"],
        json!({"created": [], "removed": [], "split": [], "moved": [],
        "trimmed": [], "relinked": [], "replaced": [], "toggled": [], "created_tracks": []})
    );
}

#[test]
fn locked_wins_over_out_of_range() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let x = put(&mut h, p, p.video, m, 0, 0, 30)[0];
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    // 400 frames is past the media's end too: the lock is reported first (INV-27).
    assert_code(
        h.run(
            "clip.trim",
            json!({"item": x, "edge": "end", "delta": "f:400"}),
        ),
        "locked",
    );
    // Clamped to nothing, the edit changes nothing and succeeds.
    let out = h.ok(
        "clip.ripple_trim",
        json!({"item": x, "edge": "end", "delta": "f:0"}),
    );
    assert_eq!(applied(&out), (0, false));
}

#[test]
fn trimming_a_parent_clamps_its_child_into_it() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let parent = put(&mut h, p, p.video, m, 0, 0, 60)[0];
    let v2 = add_track(&mut h, p, "video");
    let child = put(&mut h, p, v2, m, 50, 0, 5)[0];
    let mut doc = h.doc();
    doc.sequences
        .get_mut(&p.sequence)
        .unwrap()
        .track_mut(&v2)
        .unwrap()
        .items
        .update(&child, |i| i.set_attached_to(Some(parent)));
    h.replace_doc(doc);
    let out = h.ok(
        "clip.trim",
        json!({"item": parent, "edge": "end", "to": "f:30"}),
    );
    assert_eq!(
        frames(&h, child).0,
        29,
        "clamped to the parent's last frame"
    );
    assert_eq!(clip_of(&h, child).attached_to, Some(parent));
    assert_eq!(out["report"]["moved"][0]["item"], json!(child));
}

// ----- clip.ripple_trim ---------------------------------------------------------------------

/// V2: X 0-20 (picture only), Y 40-60 with its sound y on A2; A2 also has music M from 0 to
/// `music_end` (unlinked).
fn collision_case(music_end: i64) -> (H, P, ItemId, ItemId, ItemId) {
    let (mut h, p) = H::project();
    let v2 = add_track(&mut h, p, "video");
    let a2 = add_track(&mut h, p, "audio");
    let mute = mute(&mut h);
    let av = h.add_video("/m/av.mp4", 10, true);
    let music = h.add_audio("/m/music.wav", 30);
    let x = put(&mut h, p, v2, mute, 0, 0, 20)[0];
    let m = put(&mut h, p, a2, music, 0, 0, music_end)[0];
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": v2, "media": av, "at": "f:40",
               "source_in": "f:40", "duration": "f:20", "audio_track": a2}),
    );
    let y = item_id(&out["clips"][0]);
    (h, p, x, y, m)
}

#[test]
fn a_ripple_into_unlinked_music_is_refused_or_clamped() {
    let (mut h, _, x, y, m) = collision_case(30);
    let msg = invalid_message(
        h.run(
            "clip.ripple_trim",
            json!({"item": x, "edge": "end", "delta": -15 * F}),
        ),
        "/delta",
    );
    assert!(msg.contains(&m.to_string()), "{msg}");
    assert!(msg.contains("at least -10 frames"), "{msg}");
    let out = h.ok(
        "clip.ripple_trim",
        json!({"item": x, "edge": "end", "delta": -15 * F, "clamp": true}),
    );
    assert_eq!(applied(&out), (-10, true));
    assert_eq!(frames(&h, x), (0, 10, 0));
    assert_eq!(frames(&h, y).0, 30);
    // The start edge pulls the rest left too, by as much as it cuts.
    let out = h.ok(
        "clip.ripple_trim",
        json!({"item": x, "edge": "start", "delta": "f:5", "clamp": true}),
    );
    assert_eq!(applied(&out), (0, true), "y already touches the music");
}

#[test]
fn ripple_all_tracks_moves_unrelated_tracks_too() {
    let (mut h, p, x, y, m) = collision_case(30);
    let v3 = add_track(&mut h, p, "video");
    let title = h.add_video("/m/title.mp4", 5, false);
    let t = put(&mut h, p, v3, title, 50, 0, 10)[0];
    h.ok(
        "clip.ripple_trim",
        json!({"item": x, "edge": "end", "delta": "f:10"}),
    );
    assert_eq!(
        (frames(&h, y).0, frames(&h, t).0),
        (50, 50),
        "only y's group follows"
    );
    h.ok(
        "clip.ripple_trim",
        json!({"item": x, "edge": "end", "delta": "f:10", "ripple_all_tracks": true}),
    );
    assert_eq!((frames(&h, y).0, frames(&h, t).0), (60, 60));
    assert_eq!(
        frames(&h, m).0,
        0,
        "the music started before the edit point"
    );
}

#[test]
fn a_ripple_from_a_free_track_cannot_move_the_main_track() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    put(&mut h, p, p.video, m, 0, 0, 30);
    put(&mut h, p, p.video, m, 30, 30, 30);
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    let v2 = add_track(&mut h, p, "video");
    let x = put(&mut h, p, v2, m, 0, 0, 10)[0];
    assert_invalid(
        h.run(
            "clip.ripple_trim",
            json!({"item": x, "edge": "end", "delta": "f:5", "ripple_all_tracks": true}),
        ),
        "/delta",
    );
    h.ok(
        "clip.ripple_trim",
        json!({"item": x, "edge": "end", "delta": "f:5"}),
    );
}

#[test]
fn a_ripple_start_trim_with_to_reads_the_head_as_if_the_clip_stayed() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let x = put(&mut h, p, p.video, m, 10, 100, 30)[0];
    let y = put(&mut h, p, p.video, m, 50, 0, 10)[0];
    let out = h.ok(
        "clip.ripple_trim",
        json!({"item": x, "edge": "start", "to": "f:15"}),
    );
    assert_eq!(applied(&out), (5, false));
    assert_eq!(frames(&h, x), (10, 25, 105));
    assert_eq!(frames(&h, y).0, 45);
    // Extending the head (a negative delta) pushes the rest right; the media start bounds it.
    let out = h.ok(
        "clip.ripple_trim",
        json!({"item": x, "edge": "start", "delta": -200 * F, "clamp": true}),
    );
    assert_eq!(applied(&out), (-105, true));
    assert_eq!(frames(&h, x), (10, 130, 0));
    assert_eq!(frames(&h, y).0, 150);
}

// ----- clip.roll ----------------------------------------------------------------------------

#[test]
fn roll_to_puts_the_cut_there() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let a = put(&mut h, p, p.video, m, 0, 0, 30)[0];
    let b = put(&mut h, p, p.video, m, 30, 60, 30)[0];
    let out = h.ok("clip.roll", json!({"left": a, "right": b, "to": "f:20"}));
    assert_eq!(applied(&out), (-10, false));
    assert_eq!(frames(&h, a), (0, 20, 0));
    assert_eq!(frames(&h, b), (20, 40, 50));
}

#[test]
fn a_roll_stops_at_the_media_and_at_a_partners_neighbour() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    // A plays the last 30 frames of its media: rolling the cut later is impossible.
    let a = put(&mut h, p, p.video, m, 0, 270, 30)[0];
    let b = put(&mut h, p, p.video, m, 30, 0, 30)[0];
    let msg = invalid_message(
        h.run("clip.roll", json!({"left": a, "right": b, "delta": "f:5"})),
        "/delta",
    );
    assert!(
        msg.contains("at most +0 frames") && msg.contains("media"),
        "{msg}"
    );
    // C (with its sound c) and D (picture only), with music right after c on A1.
    let (mut h, p) = H::project();
    let av = h.add_video("/m/av.mp4", 10, true);
    let music = h.add_audio("/m/music.wav", 30);
    let m = mute(&mut h);
    let c = put(&mut h, p, p.video, av, 0, 0, 30)[0];
    let d = put(&mut h, p, p.video, m, 30, 30, 30)[0];
    put(&mut h, p, p.audio, music, 34, 0, 10);
    let out = h.ok(
        "clip.roll",
        json!({"left": c, "right": d, "delta": "f:10", "clamp": true}),
    );
    assert_eq!(applied(&out), (4, true), "c stops at the music");
    assert_eq!((frames(&h, c).1, frames(&h, d).0), (34, 34));
}

#[test]
fn roll_needs_two_touching_clips_of_one_track() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let a = put(&mut h, p, p.video, m, 0, 0, 30)[0];
    let v2 = add_track(&mut h, p, "video");
    let b = put(&mut h, p, v2, m, 30, 30, 30)[0];
    assert_invalid(
        h.run("clip.roll", json!({"left": a, "right": b, "delta": "f:5"})),
        "/right",
    );
    assert_invalid(
        h.run("clip.roll", json!({"left": a, "right": a, "delta": "f:5"})),
        "/right",
    );
    assert_code(
        h.run(
            "clip.roll",
            json!({"left": a, "right": ItemId::from_u128(9), "delta": "f:5"}),
        ),
        "not_found",
    );
    assert_invalid(h.run("clip.roll", json!({"left": a, "right": b})), "");
}

// ----- clip.slip ----------------------------------------------------------------------------

#[test]
fn slip_clamps_at_the_media_start_and_moves_nothing_on_the_timeline() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let a = put(&mut h, p, p.video, m, 0, 0, 30)[0];
    let x = put(&mut h, p, p.video, m, 30, 20, 30)[0];
    let out = h.ok(
        "clip.slip",
        json!({"item": x, "delta": -50 * F, "clamp": true}),
    );
    assert_eq!(applied(&out), (-20, true));
    assert_eq!(frames(&h, x), (30, 30, 0));
    assert_eq!(frames(&h, a), (0, 30, 0));
    assert_eq!(out["report"]["trimmed"].as_array().unwrap().len(), 1);
    assert_eq!(out["report"]["moved"], json!([]));
    let msg = invalid_message(
        h.run("clip.slip", json!({"item": x, "delta": "f:500"})),
        "/delta",
    );
    assert!(msg.contains("at most +270 frames"), "{msg}");
}

// ----- clip.slide ---------------------------------------------------------------------------

#[test]
fn a_slide_into_gaps_stops_at_time_zero_and_runs_free_at_the_end() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let x = put(&mut h, p, p.video, m, 10, 0, 30)[0];
    let out = h.ok(
        "clip.slide",
        json!({"item": x, "delta": -30 * F, "clamp": true}),
    );
    assert_eq!(applied(&out), (-10, true));
    assert_eq!(frames(&h, x), (0, 30, 0));
    let out = h.ok("clip.slide", json!({"item": x, "delta": "f:500"}));
    assert_eq!(applied(&out), (500, false));
    assert_eq!(frames(&h, x).0, 500);
}

#[test]
fn a_slide_keeps_each_neighbour_one_frame_long() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let a = put(&mut h, p, p.video, m, 0, 0, 30)[0];
    let b = put(&mut h, p, p.video, m, 30, 30, 30)[0];
    let c = put(&mut h, p, p.video, m, 60, 60, 30)[0];
    let out = h.ok(
        "clip.slide",
        json!({"item": b, "delta": -40 * F, "clamp": true}),
    );
    assert_eq!(applied(&out), (-29, true));
    assert_eq!(frames(&h, a), (0, 1, 0));
    assert_eq!(frames(&h, b), (1, 30, 30));
    assert_eq!(frames(&h, c), (31, 59, 31));
    let msg = invalid_message(
        h.run("clip.slide", json!({"item": b, "delta": -5 * F})),
        "/delta",
    );
    assert!(
        msg.contains(&a.to_string()) && msg.contains("one frame"),
        "{msg}"
    );
}

#[test]
fn sliding_the_last_main_track_clip_moves_the_track_end() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let a = put(&mut h, p, p.video, m, 0, 0, 30)[0];
    let b = put(&mut h, p, p.video, m, 30, 30, 30)[0];
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    h.ok("clip.slide", json!({"item": b, "delta": "f:10"}));
    assert_eq!((frames(&h, a).1, frames(&h, b).0), (40, 40));
    let out = h.ok(
        "clip.slide",
        json!({"item": a, "delta": "f:5", "clamp": true}),
    );
    assert_eq!(applied(&out), (0, true), "the first clip cannot slide");
}

#[test]
fn a_slid_clip_carries_its_children() {
    let (mut h, p) = H::project();
    let m = mute(&mut h);
    let a = put(&mut h, p, p.video, m, 0, 0, 30)[0];
    let b = put(&mut h, p, p.video, m, 30, 30, 30)[0];
    let v2 = add_track(&mut h, p, "video");
    let child = put(&mut h, p, v2, m, 35, 0, 5)[0];
    let mut doc = h.doc();
    doc.sequences
        .get_mut(&p.sequence)
        .unwrap()
        .track_mut(&v2)
        .unwrap()
        .items
        .update(&child, |i| i.set_attached_to(Some(b)));
    h.replace_doc(doc);
    h.ok("clip.slide", json!({"item": b, "delta": "f:10"}));
    assert_eq!(frames(&h, child).0, 45);
    assert_eq!(clip_of(&h, child).attached_to, Some(b));
    assert_eq!(frames(&h, a).1, 40);
}
