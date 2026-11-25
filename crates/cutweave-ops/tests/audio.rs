// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2E.01: `audio.set`, `audio.set_fade`, `audio.set_volume`, `audio.set_pan` and
//! `audio.set_gain`: fades and their clamping, volume and pan as thin wrappers over
//! `item.set_props` (same data), undo identity (asserted by `H::run`), locks and trims.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]

mod common;

use common::{H, P, assert_code, assert_invalid, clip_of, insert_av};
use cutweave_model::{Animatable, FadeCurve, ItemId, MediaId};
use serde_json::json;

const S: i64 = 705_600_000;

/// A 10 s video with sound at 2 s: the linked audio clip `a` plays 2 s .. 12 s.
fn setup() -> (H, P, ItemId) {
    let (mut h, p) = H::project();
    let m: MediaId = h.add_video("/m/v.mp4", 10, true);
    let (_, a) = insert_av(&mut h, p, m, "2s");
    (h, p, a)
}

#[test]
fn set_fade_stores_durations_and_curves() {
    let (mut h, _, a) = setup();
    let out = h.ok(
        "audio.set_fade",
        json!({"item": a, "in": "0.5s", "out": 2 * S, "curve": "equal_power"}),
    );
    assert_eq!(out["changed"], json!(["audio.fade_in", "audio.fade_out"]));
    assert_eq!(out["clamped"], false);
    let clip = clip_of(&h, a);
    assert_eq!(clip.audio.fade_in.duration.flicks(), S / 2);
    assert_eq!(clip.audio.fade_out.duration.flicks(), 2 * S);
    assert_eq!(clip.audio.fade_in.curve, FadeCurve::EqualPower);
    assert_eq!(clip.audio.fade_out.curve, FadeCurve::EqualPower);
    // Changing only the curve of the fade in leaves the other fade alone.
    h.ok(
        "audio.set_fade",
        json!({"item": a, "in": "0.5s", "curve": "s_curve"}),
    );
    let clip = clip_of(&h, a);
    assert_eq!(clip.audio.fade_in.curve, FadeCurve::SCurve);
    assert_eq!(clip.audio.fade_out.curve, FadeCurve::EqualPower);
    // Setting what is already there changes nothing.
    let out = h.ok("audio.set_fade", json!({"item": a, "in": "0.5s"}));
    assert_eq!(out["changed"], json!([]));
    // Zero removes a fade.
    h.ok("audio.set_fade", json!({"item": a, "out": 0}));
    assert!(clip_of(&h, a).audio.fade_out.duration.is_zero());
}

#[test]
fn fades_never_exceed_the_clip_together() {
    let (mut h, _, a) = setup(); // 10 s long
    h.ok("audio.set_fade", json!({"item": a, "out": "7s"}));
    let out = h.ok("audio.set_fade", json!({"item": a, "in": "6s"}));
    assert_eq!(out["clamped"], true);
    assert_eq!(out["fade_in"]["duration"], 3 * S, "cut to the room left");
    assert_eq!(out["fade_out"]["duration"], 7 * S);
    let out = h.ok(
        "audio.set_fade",
        json!({"item": a, "in": "20s", "out": "20s"}),
    );
    assert_eq!(out["clamped"], true);
    let clip = clip_of(&h, a);
    assert_eq!(
        clip.audio.fade_in.duration.flicks() + clip.audio.fade_out.duration.flicks(),
        10 * S
    );
}

#[test]
fn fades_are_checked() {
    let (mut h, p, a) = setup();
    assert_invalid(h.run("audio.set_fade", json!({"item": a})), "/set");
    assert_invalid(
        h.run("audio.set_fade", json!({"item": a, "in": -S})),
        "/set/in",
    );
    assert_code(
        h.run(
            "audio.set_fade",
            json!({"item": ItemId::from_u128(9), "in": 1}),
        ),
        "not_found",
    );
    h.ok("track.set", json!({"track": p.audio, "locked": true}));
    assert_code(
        h.run("audio.set_fade", json!({"item": a, "in": S})),
        "locked",
    );
}

#[test]
fn a_trim_keeps_the_fade_length() {
    let (mut h, _, a) = setup();
    h.ok(
        "audio.set_fade",
        json!({"item": a, "in": "1s", "out": "1s"}),
    );
    h.ok("clip.trim", json!({"item": a, "edge": "end", "to": "9s"}));
    let clip = clip_of(&h, a);
    assert_eq!(clip.audio.fade_in.duration.flicks(), S);
    assert_eq!(clip.audio.fade_out.duration.flicks(), S);
    assert_eq!(clip.duration.flicks(), 7 * S);
}

/// `audio.set_volume` and `audio.set_pan` write exactly what `item.set_props` writes.
#[test]
fn volume_and_pan_are_item_set_props() {
    let (mut h1, _, a1) = setup();
    let (mut h2, _, a2) = setup();
    assert_eq!(a1, a2, "deterministic ids");
    h1.ok("audio.set_volume", json!({"item": a1, "db": -6.5}));
    h2.ok(
        "item.set_props",
        json!({"item": a2, "props": {"audio_gain": -6.5}}),
    );
    assert_eq!(h1.doc(), h2.doc());
    h1.ok("audio.set_pan", json!({"item": a1, "pan": -0.25}));
    h2.ok(
        "item.set_props",
        json!({"item": a2, "props": {"audio_pan": -0.25}}),
    );
    assert_eq!(h1.doc(), h2.doc());
    let out = h1.ok(
        "audio.set",
        json!({"item": a1, "set": {"kind": "volume", "db": 3}}),
    );
    assert_eq!(out["changed"], json!(["audio_gain"]));
    h2.ok(
        "item.set_props",
        json!({"item": a2, "props": {"audio_gain": 3}}),
    );
    assert_eq!(h1.doc(), h2.doc());
}

#[test]
fn animated_volume_is_keyed_at_the_playhead() {
    let (mut h, _, a) = setup();
    h.ok(
        "keyframe.set",
        json!({"item": a, "prop": "audio_gain", "keys": [{"at": "3s", "value": 0.0}]}),
    );
    // Animated: needs `at`.
    assert_invalid(
        h.run("audio.set_volume", json!({"item": a, "db": -12})),
        "/at",
    );
    let out = h.ok(
        "audio.set_volume",
        json!({"item": a, "db": -12, "at": "5s"}),
    );
    assert_eq!(out["keyed"].as_array().unwrap().len(), 1);
    let clip = clip_of(&h, a);
    let keys = clip.audio.gain.keyframes().unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys.keys()[1].value, -12.0);
}

#[test]
fn volume_and_pan_ranges() {
    let (mut h, _, a) = setup();
    assert_invalid(
        h.run("audio.set_volume", json!({"item": a, "db": 12.5})),
        "/set/db",
    );
    assert_invalid(
        h.run(
            "audio.set",
            json!({"item": a, "set": {"kind": "pan", "pan": 1.5}}),
        ),
        "/set/pan",
    );
    // At or below -144 dB is silence, stored as -144.
    h.ok("audio.set_volume", json!({"item": a, "db": -500}));
    assert_eq!(clip_of(&h, a).audio.gain, Animatable::Static(-144.0));
    h.ok("audio.set_volume", json!({"item": a, "db": 12}));
    assert_eq!(clip_of(&h, a).audio.gain, Animatable::Static(12.0));
}

#[test]
fn set_gain_is_flat_and_removes_keys() {
    let (mut h, _, a) = setup();
    h.ok(
        "keyframe.set",
        json!({"item": a, "prop": "audio_gain", "keys": [{"at": "3s", "value": 0.0}, {"at": "5s", "value": -9.0}]}),
    );
    assert!(clip_of(&h, a).audio.gain.keyframes().is_some());
    let out = h.ok("audio.set_gain", json!({"item": a, "db": -3}));
    assert_eq!(out["changed"], json!(["audio.gain"]));
    assert_eq!(clip_of(&h, a).audio.gain, Animatable::Static(-3.0));
    let out = h.ok("audio.set_gain", json!({"item": a, "db": -3}));
    assert_eq!(out["changed"], json!([]));
}

#[test]
fn several_items_in_one_undo_step() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let (_, a) = insert_av(&mut h, p, m, "0s");
    let m2 = h.add_video("/m/w.mp4", 5, true);
    let (_, b) = insert_av(&mut h, p, m2, "20s");
    let commits = h.commits();
    h.ok(
        "audio.set",
        json!({"items": [a, b], "set": {"kind": "fade", "in": "1s", "curve": "log"}}),
    );
    assert_eq!(h.commits(), commits + 1);
    for id in [a, b] {
        let clip = clip_of(&h, id);
        assert_eq!(clip.audio.fade_in.duration.flicks(), S);
        assert_eq!(clip.audio.fade_in.curve, FadeCurve::Log);
    }
}

#[test]
fn resetting_the_audio_group_removes_fades() {
    let (mut h, _, a) = setup();
    h.ok(
        "audio.set_fade",
        json!({"item": a, "in": "1s", "out": "2s", "curve": "log"}),
    );
    h.ok("audio.set_volume", json!({"item": a, "db": -4}));
    let out = h.ok("item.reset_props", json!({"item": a, "group": "audio"}));
    assert!(
        out["changed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "audio.fade_in")
    );
    let clip = clip_of(&h, a);
    assert!(clip.audio.fade_in.duration.is_zero() && clip.audio.fade_out.duration.is_zero());
    assert_eq!(clip.audio.gain, Animatable::Static(0.0));
}
