// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2C.03 acceptance: `keyframe.*`, the keyed side of `item.set_props`, `item.reset_props`
//! and the retiming of keys by splits and trims (`docs/spec/keyframes.md` §8, §9).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names,
    clippy::float_cmp
)]

mod common;

use std::sync::{Arc, LazyLock};

use common::{FRAME30, H, P, assert_code, assert_invalid, clip_of, insert_av, item_id};
use cutweave_commands::{
    ActorCtx, ConnId, DocConfig, DocState, HistoryScope, Registry, Submit, TxKind,
};
use cutweave_model::{Animatable, ItemId, MediaId, Time};
use proptest::prelude::*;
use serde_json::{Value, json};

const S: i64 = 705_600_000;

/// A 10 s video with sound at 2 s: V1 clip `v` (2 s .. 12 s) and linked A1 clip `a`.
fn setup() -> (H, P, MediaId, ItemId, ItemId) {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let (v, a) = insert_av(&mut h, p, m, "2s");
    (h, p, m, v, a)
}

fn doc_path(prop: &str) -> String {
    let spec = prop.parse::<cutweave_model::PropId>().unwrap().spec();
    format!("/{}", spec.path.replace('.', "/"))
}

/// A property of an item in its document form.
fn prop(h: &H, item: ItemId, prop: &str) -> Value {
    let clip = serde_json::to_value(clip_of(h, item)).unwrap();
    clip.pointer(&doc_path(prop)).cloned().unwrap()
}

/// `(t, value)` of the keys of a property; empty when static.
fn keys(h: &H, item: ItemId, name: &str) -> Vec<(i64, Value)> {
    prop(h, item, name)["keyframes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|k| (k["t"].as_i64().unwrap(), k["value"].clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn times(h: &H, item: ItemId, name: &str) -> Vec<i64> {
    keys(h, item, name).into_iter().map(|(t, _)| t).collect()
}

fn set(h: &mut H, item: ItemId, name: &str, keys: Value) -> Value {
    h.ok(
        "keyframe.set",
        json!({"item": item, "prop": name, "keys": keys}),
    )
}

// ----- keyframe.set -------------------------------------------------------------------------------

#[test]
fn set_animates_a_static_property_with_exactly_the_given_keys() {
    let (mut h, _, _, v, _) = setup();
    let out = set(
        &mut h,
        v,
        "opacity",
        json!([{"at": "3s", "value": 0.0, "easing": "ease_out"}, {"at": "5s", "value": 1.0}]),
    );
    // The clip starts at 2 s: item times are 1 s and 3 s.
    assert_eq!(times(&h, v, "opacity"), vec![S, 3 * S]);
    let k = &out["keys"][0];
    assert_eq!(k["t"], S);
    assert_eq!(k["at"], 3 * S);
    assert_eq!(k["created"], true);
    assert_eq!(k["outside"], false);
    let stored = prop(&h, v, "opacity");
    assert_eq!(stored["keyframes"][0]["interp"]["preset"], "ease_out");
    // The second key had no easing and no key before it with one: it inherits the first
    // key's interp, as the feel carries on.
    assert_eq!(stored["keyframes"][1]["interp"]["kind"], "bezier");
}

#[test]
fn a_key_at_an_occupied_time_replaces_it_and_keeps_its_interp() {
    let (mut h, _, _, v, _) = setup();
    set(
        &mut h,
        v,
        "opacity",
        json!([{"t": "0s", "value": 0.0, "easing": "ease_in"}, {"t": "2s", "value": 1.0}]),
    );
    let out = set(&mut h, v, "opacity", json!([{"t": "0s", "value": 0.5}]));
    assert_eq!(out["keys"][0]["created"], false);
    assert_eq!(keys(&h, v, "opacity")[0], (0, json!(0.5)));
    assert_eq!(
        prop(&h, v, "opacity")["keyframes"][0]["interp"]["preset"],
        "ease_in"
    );
    assert_eq!(times(&h, v, "opacity").len(), 2);
}

#[test]
fn an_omitted_value_keys_the_current_value() {
    let (mut h, _, _, v, _) = setup();
    h.ok(
        "item.set_props",
        json!({"item": v, "props": {"opacity": 0.25}}),
    );
    set(&mut h, v, "opacity", json!([{"t": "1s"}]));
    assert_eq!(keys(&h, v, "opacity"), vec![(S, json!(0.25))]);
    // Between two linear keys the value there is interpolated.
    set(&mut h, v, "opacity", json!([{"t": "3s", "value": 0.75}]));
    set(&mut h, v, "opacity", json!([{"t": "2s"}]));
    assert_eq!(keys(&h, v, "opacity")[1], (2 * S, json!(0.5)));
}

#[test]
fn times_snap_to_frames_in_sequence_time_and_t_equals_at() {
    let (mut h, _, _, v, _) = setup();
    // 3 s + 0.4 frame snaps down, + 0.6 frame snaps up (the clip starts on the grid).
    let near = 3 * S + FRAME30 * 2 / 5;
    let far = 4 * S + FRAME30 * 3 / 5;
    let out = set(
        &mut h,
        v,
        "crop.left",
        json!([{"at": near, "value": 0.1}, {"at": far, "value": 0.2}]),
    );
    assert_eq!(out["keys"][0]["at"], 3 * S);
    assert_eq!(out["keys"][1]["at"], 4 * S + FRAME30);
    set(
        &mut h,
        v,
        "crop.right",
        json!([{"t": "f:30", "value": 0.1}]),
    );
    assert_eq!(times(&h, v, "crop.right"), vec![S]);
    // t and at name the same key.
    let out = set(&mut h, v, "crop.right", json!([{"at": "3s", "value": 0.4}]));
    assert_eq!(out["keys"][0]["created"], false);
    for bad in [
        json!([{"value": 0.1}]),
        json!([{"t": "1s", "at": "3s", "value": 0.1}]),
    ] {
        assert_invalid(
            h.run(
                "keyframe.set",
                json!({"item": v, "prop": "crop.top", "keys": bad}),
            ),
            "/keys/0",
        );
    }
}

#[test]
fn keys_outside_the_item_are_allowed_and_flagged() {
    let (mut h, _, _, v, _) = setup();
    let out = set(
        &mut h,
        v,
        "opacity",
        json!([{"t": "-1s", "value": 0.0}, {"t": "11s", "value": 1.0}]),
    );
    assert_eq!(out["keys"][0]["outside"], true);
    assert_eq!(out["keys"][1]["outside"], true);
    assert_eq!(times(&h, v, "opacity"), vec![-S, 11 * S]);
}

#[test]
fn invalid_values_and_requests_say_where() {
    let (mut h, p, _, v, a) = setup();
    for (name, keys, path) in [
        (
            "opacity",
            json!([{"t": "0s", "value": 1.5}]),
            "/keys/0/value",
        ),
        (
            "opacity",
            json!([{"t": "0s", "value": 1.0}, {"t": "1s", "value": -0.1}]),
            "/keys/1/value",
        ),
        (
            "opacity",
            json!([{"t": "0s", "value": {"x": 1, "y": 2}}]),
            "/keys/0/value",
        ),
        (
            "transform.scale",
            json!([{"t": "0s", "value": 1.0}]),
            "/keys/0/value",
        ),
        (
            "transform.scale",
            json!([{"t": "0s", "value": {"x": -1.0, "y": 1.0}}]),
            "/keys/0/value",
        ),
        (
            "flip.horizontal",
            json!([{"t": "0s", "value": 1.0}]),
            "/keys/0/value",
        ),
        (
            "flip.horizontal",
            json!([{"t": "0s", "value": true, "easing": "linear"}]),
            "/keys/0/interp/kind",
        ),
        (
            "audio_pan",
            json!([{"t": "0s", "value": 2.0}]),
            "/keys/0/value",
        ),
        (
            "audio_gain",
            json!([{"t": "0s", "value": 99.0}]),
            "/keys/0/value",
        ),
        (
            "opacity",
            json!([{"t": "0s", "value": 1.0, "easing": "linear", "interp": {"kind": "linear"}}]),
            "/keys/0",
        ),
        (
            "opacity",
            json!([{"t": "0s", "interp": {"kind": "bezier", "x1": 2.0, "y1": 0, "x2": 0.5, "y2": 1}}]),
            "/keys/0/interp/x1",
        ),
        (
            "opacity",
            json!([{"t": "0s", "value": 1.0}, {"t": "0s", "value": 0.0}]),
            "/keys/1",
        ),
        (
            "opacity",
            json!([{"t": "0s", "value": 1.0, "preset": "ease_in"}]),
            "/keys/0/preset",
        ),
    ] {
        assert_invalid(
            h.run(
                "keyframe.set",
                json!({"item": v, "prop": name, "keys": keys}),
            ),
            path,
        );
    }
    assert_invalid(
        h.run(
            "keyframe.set",
            json!({"item": v, "prop": "opacity", "keys": []}),
        ),
        "/keys",
    );
    assert_invalid(
        h.run(
            "keyframe.set",
            json!({"prop": "opacity", "keys": [{"t": "0s"}]}),
        ),
        "",
    );
    assert_invalid(
        h.run(
            "keyframe.set",
            json!({"item": v, "prop": "opacity", "keys": [{"t": "0s"}], "fine": true}),
        ),
        "/fine",
    );
    // `fine` is for audio properties.
    h.ok(
        "keyframe.set",
        json!({"item": a, "prop": "audio_gain", "keys": [{"at": "3s", "value": -3.0}], "fine": true}),
    );
    assert!(
        h.run(
            "keyframe.set",
            json!({"item": v, "prop": "blend_mode", "keys": [{"t": "0s"}]})
        )
        .is_err()
    );
    assert_code(
        h.run(
            "keyframe.set",
            json!({"item": ItemId::from_u128(9), "prop": "opacity", "keys": [{"t": "0s"}]}),
        ),
        "not_found",
    );
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    assert_code(
        h.run(
            "keyframe.set",
            json!({"item": v, "prop": "opacity", "keys": [{"t": "0s"}]}),
        ),
        "locked",
    );
}

#[test]
fn replace_makes_the_result_exactly_the_given_keys() {
    let (mut h, _, _, v, _) = setup();
    set(
        &mut h,
        v,
        "opacity",
        json!([{"t": "0s", "value": 0.0}, {"t": "1s", "value": 1.0}, {"t": "2s", "value": 0.5}]),
    );
    h.ok(
        "keyframe.set",
        json!({"item": v, "prop": "opacity", "replace": true,
               "keys": [{"t": "1s", "value": 0.9}, {"t": "4s", "value": 0.1}]}),
    );
    assert_eq!(
        keys(&h, v, "opacity"),
        vec![(S, json!(0.9)), (4 * S, json!(0.1))]
    );
}

#[test]
fn items_get_the_same_keys() {
    let (mut h, _, _, v, a) = setup();
    let out = h.ok(
        "keyframe.set",
        json!({"items": [v, a], "prop": "audio_pan", "keys": [{"t": "0s", "value": -1.0}]}),
    );
    assert_eq!(out["keys"].as_array().unwrap().len(), 2);
    assert_eq!(times(&h, v, "audio_pan"), vec![0]);
    assert_eq!(times(&h, a, "audio_pan"), vec![0]);
    assert_invalid(
        h.run(
            "keyframe.set",
            json!({"item": v, "items": [a], "prop": "opacity", "keys": [{"t": "0s"}]}),
        ),
        "",
    );
}

#[test]
fn keying_a_video_clip_leaves_its_linked_audio_alone() {
    let (mut h, _, _, v, a) = setup();
    let before = clip_of(&h, a);
    set(&mut h, v, "opacity", json!([{"t": "0s", "value": 0.5}]));
    assert_eq!(clip_of(&h, a), before);
}

// ----- keyframe.delete and keyframe.clear -------------------------------------------------------

fn animated(h: &mut H, v: ItemId) {
    set(
        h,
        v,
        "opacity",
        json!([{"t": "0s", "value": 0.0}, {"t": "1s", "value": 1.0}, {"t": "2s", "value": 0.5}]),
    );
}

#[test]
fn delete_removes_keys_and_the_last_one_makes_the_property_static() {
    let (mut h, _, _, v, _) = setup();
    animated(&mut h, v);
    let out = h.ok(
        "keyframe.delete",
        json!({"item": v, "keys": [{"prop": "opacity", "t": S}]}),
    );
    assert_eq!(out["now_static"], json!([]));
    assert_eq!(times(&h, v, "opacity"), vec![0, 2 * S]);
    let out = h.ok(
        "keyframe.delete",
        json!({"item": v, "keys": [{"prop": "opacity", "t": 0}, {"prop": "opacity", "t": 2 * S}]}),
    );
    assert_eq!(out["now_static"], json!(["opacity"]));
    assert_eq!(
        prop(&h, v, "opacity"),
        json!(0.0),
        "static at the first key's value"
    );
    assert_code(
        h.run(
            "keyframe.delete",
            json!({"item": v, "keys": [{"prop": "opacity", "t": 0}]}),
        ),
        "not_found",
    );
}

#[test]
fn deleting_a_missing_key_changes_nothing() {
    let (mut h, _, _, v, _) = setup();
    animated(&mut h, v);
    let before = h.doc();
    assert_code(
        h.run(
            "keyframe.delete",
            json!({"item": v, "keys": [{"prop": "opacity", "t": 0}, {"prop": "opacity", "t": 7}]}),
        ),
        "not_found",
    );
    assert_eq!(h.doc(), before);
}

#[test]
fn clear_flattens_to_the_first_key_or_the_value_at_a_time() {
    let (mut h, _, _, v, _) = setup();
    animated(&mut h, v);
    let out = h.ok(
        "keyframe.clear",
        json!({"item": v, "prop": "opacity", "t": "0.5s"}),
    );
    assert_eq!(out["value"], 0.5);
    assert_eq!(prop(&h, v, "opacity"), json!(0.5));
    // Static: nothing to do.
    let before = h.doc();
    h.ok("keyframe.clear", json!({"item": v, "prop": "opacity"}));
    assert_eq!(h.doc(), before);
    animated(&mut h, v);
    let out = h.ok("keyframe.clear", json!({"item": v, "prop": "opacity"}));
    assert_eq!(out["value"], 0.0, "the first key");
}

// ----- keyframe.move ----------------------------------------------------------------------------

#[test]
fn move_by_a_delta_to_a_time_and_past_neighbours() {
    let (mut h, _, _, v, _) = setup();
    animated(&mut h, v);
    let out = h.ok(
        "keyframe.move",
        json!({"item": v, "keys": [{"prop": "opacity", "t": 0}], "delta": "f:15"}),
    );
    assert_eq!(out["moved"][0]["t"], S / 2);
    assert_eq!(out["moved"][0]["from_t"], 0);
    assert_eq!(times(&h, v, "opacity"), vec![S / 2, S, 2 * S]);
    // Past a neighbour: the list re-sorts and the key keeps its value and interp.
    h.ok(
        "keyframe.move",
        json!({"item": v, "keys": [{"prop": "opacity", "t": S / 2}], "to_t": "1.5s"}),
    );
    assert_eq!(
        keys(&h, v, "opacity"),
        vec![
            (S, json!(1.0)),
            (S + S / 2, json!(0.0)),
            (2 * S, json!(0.5))
        ]
    );
    h.ok(
        "keyframe.move",
        json!({"item": v, "keys": [{"prop": "opacity", "t": 2 * S}], "to_at": "7s"}),
    );
    assert_eq!(times(&h, v, "opacity"), vec![S, S + S / 2, 5 * S]);
}

#[test]
fn moving_onto_a_key_that_stays_is_invalid_and_changes_nothing() {
    let (mut h, _, _, v, _) = setup();
    animated(&mut h, v);
    let before = h.doc();
    let r = h.run(
        "keyframe.move",
        json!({"item": v, "keys": [{"prop": "opacity", "t": 0}], "to_t": "1s"}),
    );
    match r {
        Err(cutweave_commands::CmdError::Invalid { msg, .. }) => {
            assert!(msg.contains("already exists"), "{msg}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(h.doc(), before);
    // Two keys moving together keep their distance, so they may take each other's place.
    h.ok(
        "keyframe.move",
        json!({"item": v, "delta": "1s",
               "keys": [{"prop": "opacity", "t": S}, {"prop": "opacity", "t": 2 * S}]}),
    );
    assert_eq!(times(&h, v, "opacity"), vec![0, 2 * S, 3 * S]);
}

#[test]
fn move_validates_its_arguments() {
    let (mut h, _, _, v, _) = setup();
    animated(&mut h, v);
    let one = json!([{"prop": "opacity", "t": 0}]);
    for params in [
        json!({"item": v, "keys": one}),
        json!({"item": v, "keys": one, "delta": "1s", "to_t": "1s"}),
        json!({"item": v, "keys": [], "delta": "1s"}),
        json!({"item": v, "keys": [{"prop": "opacity", "t": 0}, {"prop": "opacity", "t": S}],
               "to_t": "5s"}),
    ] {
        assert!(h.run("keyframe.move", params.clone()).is_err(), "{params}");
    }
    assert_code(
        h.run(
            "keyframe.move",
            json!({"item": v, "keys": [{"prop": "opacity", "t": 5}], "delta": "1s"}),
        ),
        "not_found",
    );
}

// ----- keyframe.set_interp ----------------------------------------------------------------------

#[test]
fn set_interp_changes_the_segments_that_start_at_the_keys() {
    let (mut h, _, _, v, _) = setup();
    animated(&mut h, v);
    h.ok(
        "keyframe.set_interp",
        json!({"item": v, "keys": [{"prop": "opacity", "t": S}], "easing": "back_out"}),
    );
    let stored = prop(&h, v, "opacity");
    assert_eq!(stored["keyframes"][1]["interp"]["preset"], "back_out");
    assert_eq!(stored["keyframes"][0]["interp"]["kind"], "linear");
    // An explicit interp carries no preset hint unless it gives one; bounce defaults to false.
    h.ok(
        "keyframe.set_interp",
        json!({"item": v, "keys": [{"prop": "opacity", "t": S}],
               "interp": {"kind": "spring", "stiffness": 100.0, "damping": 10.0, "mass": 1.0}}),
    );
    let interp = &prop(&h, v, "opacity")["keyframes"][1]["interp"];
    assert_eq!(interp["bounce"], false);
    assert!(interp.get("preset").is_none());
    assert_invalid(
        h.run(
            "keyframe.set_interp",
            json!({"item": v, "keys": [{"prop": "opacity", "t": S}]}),
        ),
        "",
    );
    // A flip key only holds.
    set(
        &mut h,
        v,
        "flip.horizontal",
        json!([{"t": "0s", "value": true}]),
    );
    assert_invalid(
        h.run(
            "keyframe.set_interp",
            json!({"item": v, "keys": [{"prop": "flip.horizontal", "t": 0}], "easing": "ease_in"}),
        ),
        "/interp/kind",
    );
}

// ----- keyframe.copy and keyframe.paste -------------------------------------------------------------

#[test]
fn copy_and_paste_move_keys_between_properties_and_items() {
    let (mut h, _, _, v, a) = setup();
    set(
        &mut h,
        v,
        "crop.left",
        json!([{"t": "1s", "value": 0.1, "easing": "ease_in"}, {"t": "2s", "value": 0.4}]),
    );
    let copy = h
        .query(
            "keyframe.copy",
            json!({"item": v, "keys": [{"prop": "crop.left", "t": 2 * S}, {"prop": "crop.left", "t": S}]}),
        )
        .unwrap();
    let payload = copy["clipboard"].clone();
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["keys"][0]["dt"], 0);
    assert_eq!(payload["keys"][1]["dt"], S);
    h.ok(
        "keyframe.paste",
        json!({"item": v, "payload": payload, "at": "6s", "as_prop": "crop.right"}),
    );
    assert_eq!(
        keys(&h, v, "crop.right"),
        vec![(4 * S, json!(0.1)), (5 * S, json!(0.4))]
    );
    assert_eq!(
        prop(&h, v, "crop.right")["keyframes"][0]["interp"]["preset"],
        "ease_in"
    );
    // The same property on another item, replacing a key at a time.
    h.ok(
        "keyframe.set",
        json!({"item": a, "prop": "crop.left", "keys": [{"t": "1s", "value": 0.9}]}),
    );
    let out = h.ok(
        "keyframe.paste",
        json!({"items": [a], "payload": payload, "t": "1s"}),
    );
    assert_eq!(out["keys"][0]["created"], false);
    assert_eq!(keys(&h, a, "crop.left")[0], (S, json!(0.1)));
    // Type and range errors.
    assert_invalid(
        h.run(
            "keyframe.paste",
            json!({"item": v, "payload": payload, "at": "3s", "as_prop": "transform.position"}),
        ),
        "/as_prop",
    );
    assert!(
        h.run(
            "keyframe.paste",
            json!({"item": v, "payload": {"version": 2, "keys": []}, "at": "3s"}),
        )
        .is_err()
    );
    assert_code(
        h.query(
            "keyframe.copy",
            json!({"item": v, "keys": [{"prop": "opacity", "t": 1}]}),
        )
        .map(|_| Value::Null),
        "not_found",
    );
}

// ----- keyframe.eval ------------------------------------------------------------------------------

#[test]
fn eval_samples_a_property_clamped_and_in_sequence_time() {
    let (mut h, _, _, v, _) = setup();
    set(
        &mut h,
        v,
        "opacity",
        json!([{"t": "0s", "value": 0.0, "easing": "linear"}, {"t": "2s", "value": 1.0}]),
    );
    let out = h
        .query(
            "keyframe.eval",
            json!({"item": v, "prop": "opacity", "at": "3s"}),
        )
        .unwrap();
    assert_eq!(out["samples"][0]["value"], 0.5);
    assert_eq!(out["samples"][0]["t"], S);
    assert_eq!(out["samples"][0]["in_item"], true);
    let out = h
        .query(
            "keyframe.eval",
            json!({"item": v, "prop": "opacity", "from": "1s", "to": "5s", "samples": 5}),
        )
        .unwrap();
    let s = out["samples"].as_array().unwrap();
    assert_eq!(s.len(), 5);
    assert_eq!(s[0]["in_item"], false, "before the item starts");
    assert_eq!(s[0]["value"], 0.0);
    assert_eq!(s[4]["value"], 1.0);
    assert_eq!(s[2]["at"], 3 * S);
    for bad in [
        json!({"item": v, "prop": "opacity"}),
        json!({"item": v, "prop": "opacity", "from": "1s", "to": "5s", "samples": 1}),
        json!({"item": v, "prop": "opacity", "from": "5s", "to": "1s", "samples": 4}),
        json!({"item": v, "prop": "opacity", "from": "1s", "to": "5s", "samples": 5000}),
        json!({"item": v, "prop": "opacity", "at": "1s", "from": "1s"}),
    ] {
        assert!(h.query("keyframe.eval", bad.clone()).is_err(), "{bad}");
    }
    // Static properties evaluate to their value.
    let out = h
        .query(
            "keyframe.eval",
            json!({"item": v, "prop": "transform.scale", "at": "3s"}),
        )
        .unwrap();
    assert_eq!(out["samples"][0]["value"], json!({"x": 1.0, "y": 1.0}));
}

// ----- item.set_props (auto-key) and item.reset_props ----------------------------------------------

#[test]
fn set_props_takes_the_new_fields_and_lists_them_in_order() {
    let (mut h, _, _, v, _) = setup();
    let out = h.ok(
        "item.set_props",
        json!({"item": v, "props": {
            "opacity": 0.5, "audio_pan": -0.5, "blend_mode": "screen",
            "crop": {"left": 0.25, "bottom": 0.5}, "flip": {"horizontal": true}}}),
    );
    assert_eq!(
        out["changed"],
        json!([
            "opacity",
            "audio_pan",
            "crop.left",
            "crop.bottom",
            "flip.horizontal",
            "blend_mode"
        ])
    );
    let c = clip_of(&h, v);
    assert_eq!(*c.visual.crop.left.value(), 0.25);
    assert_eq!(*c.visual.crop.top.value(), 0.0);
    assert!(*c.visual.flip.horizontal.value());
    assert_eq!(c.visual.blend_mode, cutweave_model::BlendMode::Screen);
    assert_eq!(*c.audio.pan.value(), -0.5);
    for (props, path) in [
        (json!({"crop": {"left": 1.5}}), "/props/crop/left"),
        (json!({"audio_pan": 1.5}), "/props/audio_pan"),
        (json!({"crop": {}}), "/props"),
    ] {
        let r = h.run("item.set_props", json!({"item": v, "props": props}));
        if path == "/props" {
            // An empty crop patch is as good as nothing.
            assert_invalid(r, path);
        } else {
            assert_invalid(r, path);
        }
    }
    assert!(
        h.run(
            "item.set_props",
            json!({"item": v, "props": {"blend_mode": "burn"}})
        )
        .is_err()
    );
}

#[test]
fn an_animated_property_is_keyed_at_the_playhead() {
    let (mut h, _, _, v, _) = setup();
    animated(&mut h, v); // keys at item times 0, 1 s, 2 s
    // Without `at`, and with auto_key off, it is refused with a way out.
    match h.run(
        "item.set_props",
        json!({"item": v, "props": {"opacity": 0.3}}),
    ) {
        Err(cutweave_commands::CmdError::Invalid { path, msg }) => {
            assert_eq!(path, "/at");
            assert!(msg.contains("keyframe_clear"), "{msg}");
        }
        other => panic!("{other:?}"),
    }
    assert_invalid(
        h.run(
            "item.set_props",
            json!({"item": v, "props": {"opacity": 0.3}, "at": "3s", "auto_key": false}),
        ),
        "/auto_key",
    );
    let out = h.ok(
        "item.set_props",
        json!({"item": v, "props": {"opacity": 0.3, "crop": {"left": 0.2}}, "at": "3.5s"}),
    );
    assert_eq!(out["changed"], json!(["opacity", "crop.left"]));
    assert_eq!(
        out["keyed"].as_array().unwrap().len(),
        1,
        "crop.left is static"
    );
    assert_eq!(out["keyed"][0]["prop"], "opacity");
    assert_eq!(out["keyed"][0]["t"], S + S / 2);
    assert_eq!(times(&h, v, "opacity"), vec![0, S, S + S / 2, 2 * S]);
    // The same value again is not a change; a repeat at one time replaces the key.
    let again = h.ok(
        "item.set_props",
        json!({"item": v, "props": {"opacity": 0.3}, "at": "3.5s"}),
    );
    assert_eq!(again["changed"], json!([]));
    h.ok(
        "item.set_props",
        json!({"item": v, "props": {"opacity": 0.6}, "at": "3.5s"}),
    );
    assert_eq!(times(&h, v, "opacity").len(), 4);
    // The playhead must be inside the item.
    for at in ["1s", "12s", "20s"] {
        assert_invalid(
            h.run(
                "item.set_props",
                json!({"item": v, "props": {"opacity": 0.3}, "at": at}),
            ),
            "/at",
        );
    }
}

#[test]
fn reset_props_restores_defaults_by_group_by_property_or_all() {
    let (mut h, _, _, v, _) = setup();
    let fresh = clip_of(&h, v);
    h.ok(
        "item.set_props",
        json!({"item": v, "props": {
            "opacity": 0.5, "audio_gain": -3.0, "audio_pan": 0.5, "blend_mode": "add",
            "transform": {"position": {"x": 0.2, "y": 0.1}, "rotation": 30.0},
            "crop": {"left": 0.1}, "flip": {"vertical": true}}}),
    );
    set(
        &mut h,
        v,
        "transform.scale",
        json!([{"t": "0s", "value": {"x": 2.0, "y": 2.0}}]),
    );
    let dirty = clip_of(&h, v);
    // A group.
    let out = h.ok("item.reset_props", json!({"item": v, "group": "transform"}));
    assert_eq!(
        out["changed"],
        json!([
            "transform.position",
            "transform.scale",
            "transform.rotation"
        ])
    );
    let c = clip_of(&h, v);
    assert_eq!(
        c.visual.transform, fresh.visual.transform,
        "keys removed too"
    );
    assert_eq!(c.visual.crop, dirty.visual.crop, "other groups untouched");
    // Compositing is opacity and blend mode.
    let out = h.ok(
        "item.reset_props",
        json!({"item": v, "group": "compositing"}),
    );
    assert_eq!(out["changed"], json!(["opacity", "blend_mode"]));
    // Properties.
    h.ok(
        "item.reset_props",
        json!({"item": v, "props": ["crop.left", "flip.vertical"]}),
    );
    // Everything else.
    let out = h.ok("item.reset_props", json!({"items": [v]}));
    assert_eq!(out["changed"], json!(["audio_gain", "audio_pan"]));
    let c = clip_of(&h, v);
    assert_eq!(c.visual, fresh.visual);
    assert_eq!(c.audio, fresh.audio);
    // Nothing left to reset.
    let out = h.ok("item.reset_props", json!({"item": v}));
    assert_eq!(out["changed"], json!([]));
    assert_invalid(
        h.run(
            "item.reset_props",
            json!({"item": v, "group": "crop", "props": ["opacity"]}),
        ),
        "/props",
    );
    assert_invalid(
        h.run("item.reset_props", json!({"item": v, "props": []})),
        "/props",
    );
}

#[test]
fn reset_props_has_undo_identity() {
    // `H::run` asserts that the patch inverts exactly; this also checks the resulting document.
    let (mut h, _, _, v, _) = setup();
    animated(&mut h, v);
    let before = h.doc();
    h.ok(
        "item.reset_props",
        json!({"item": v, "group": "compositing"}),
    );
    let after = h.last.as_ref().unwrap().clone();
    assert_ne!(after.doc, before);
    assert_eq!(
        cutweave_commands::apply_patch(&after.doc, &after.inverse).unwrap(),
        before
    );
}

// ----- retiming: splits (acceptance: 1,000 cases) -------------------------------------------------------

fn interp_json(kind: u8) -> Value {
    match kind % 4 {
        0 => json!({"kind": "hold"}),
        1 => json!({"kind": "linear"}),
        2 => json!({"kind": "bezier", "x1": 0.42, "y1": 0.0, "x2": 0.58, "y2": 1.0}),
        _ => json!({"kind": "spring", "stiffness": 170.0, "damping": 26.0, "mass": 1.0,
                    "bounce": false}),
    }
}

fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), failure_persistence: None, ..ProptestConfig::default() })]

    /// Splitting a clip with keyframes: each half evaluates, at every matching timeline time,
    /// to what the original did (keyframes §8.1: exactly, bit for bit).
    #[test]
    fn splitting_keeps_every_evaluation(
        raw in prop::collection::vec((-30..330_i64, any::<u8>(), 0.0..=1.0_f64), 1..7),
        split_at in 1..299_i64,
        probe in prop::collection::vec(0..300_i64, 1..12),
    ) {
        let (mut h, _, _, v, _) = setup();
        // Keys on distinct frames (clip is 10 s = 300 frames), item time = 30fps frames.
        let mut frames: Vec<(i64, u8, f64)> = raw;
        frames.sort_by_key(|k| k.0);
        frames.dedup_by_key(|k| k.0);
        let keys: Vec<Value> = frames
            .iter()
            .map(|(f, kind, value)| json!({"t": f * FRAME30, "value": value, "interp": interp_json(*kind)}))
            .collect();
        let rejected = h.run("keyframe.set", json!({"item": v, "prop": "opacity", "keys": keys}));
        prop_assert!(rejected.is_ok(), "{rejected:?}");
        let original = clip_of(&h, v);
        let out = h.ok("clip.split", json!({"item": v, "at": format!("f:{}", 60 + split_at)}));
        let (left, right) = (item_id(&out["left"]), item_id(&out["right"]));
        let (l, r) = (clip_of(&h, left), clip_of(&h, right));
        for p in probe {
            let abs = original.start.flicks() + p * FRAME30;
            let reference = original.visual.opacity.eval(Time::from_flicks(abs - original.start.flicks()).unwrap());
            let (half, clip) = if abs < r.start.flicks() { (&l, &l) } else { (&r, &r) };
            let rel = Time::from_flicks(abs - clip.start.flicks()).unwrap();
            let got = half.visual.opacity.eval(rel);
            prop_assert_eq!(got.to_bits(), reference.to_bits(), "frame {} of {:?}", p, original.visual.opacity);
        }
        // The halves keep at most the keys they show plus one on each side.
        for half in [&l, &r] {
            if let Animatable::Keyframes(k) = &half.visual.opacity {
                prop_assert!(k.len() <= frames.len());
            }
        }
    }
}

// ----- retiming: trims ------------------------------------------------------------------------------

/// A 10 s clip on V1 at 2 s with opacity keys at item times 1 s, 3 s and 5 s; `(h, p, v, a)`.
fn keyed_clip() -> (H, P, ItemId, ItemId) {
    let (mut h, p, _, v, a) = setup();
    set(
        &mut h,
        v,
        "opacity",
        json!([{"t": "1s", "value": 0.1}, {"t": "3s", "value": 0.5}, {"t": "5s", "value": 0.9}]),
    );
    (h, p, v, a)
}

#[test]
fn a_start_trim_keeps_keys_on_the_same_content() {
    let (mut h, _, v, _) = keyed_clip();
    // Trim 1 s into the head: the key at 1 s now sits at the clip's first frame minus nothing:
    // item time 0, and the others shift by 1 s as well.
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "start", "delta": "1s"}),
    );
    assert_eq!(times(&h, v, "opacity"), vec![0, 2 * S, 4 * S]);
    // The key stays on the same timeline position (3 s).
    assert_eq!(clip_of(&h, v).start.flicks(), 3 * S);
    // Extending the head back brings keys back to where they were.
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "start", "delta": "-1s"}),
    );
    assert_eq!(times(&h, v, "opacity"), vec![S, 3 * S, 5 * S]);
}

#[test]
fn an_end_trim_leaves_keys_unless_pruning() {
    let (mut h, _, v, _) = keyed_clip();
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "end", "delta": "-8s"}),
    );
    assert_eq!(
        times(&h, v, "opacity"),
        vec![S, 3 * S, 5 * S],
        "kept past the end"
    );
    let (mut h, _, v, _) = keyed_clip();
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "end", "delta": "-8s", "prune_keyframes": true}),
    );
    // The clip is 2 s now: keys at 1 s, and the first key at or after the end (3 s).
    assert_eq!(times(&h, v, "opacity"), vec![S, 3 * S]);
    // A start trim with pruning drops keys the head left behind but keeps the bracketing one.
    let (mut h, _, v, _) = keyed_clip();
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "start", "delta": "4s", "prune_keyframes": true}),
    );
    assert_eq!(times(&h, v, "opacity"), vec![-S, S]);
}

#[test]
fn a_ripple_trim_of_the_start_follows_the_content_not_the_start() {
    let (mut h, _, v, _) = keyed_clip();
    h.ok(
        "clip.ripple_trim",
        json!({"item": v, "edge": "start", "delta": "2s"}),
    );
    // The clip's start stays at 2 s; its content moved 2 s earlier, so do the keys.
    assert_eq!(clip_of(&h, v).start.flicks(), 2 * S);
    assert_eq!(times(&h, v, "opacity"), vec![-S, S, 3 * S]);
}

#[test]
fn slip_move_and_end_edges_leave_keys_alone() {
    let (mut h, p, v, _) = keyed_clip();
    let before = times(&h, v, "opacity");
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "end", "delta": "-2s"}),
    );
    assert_eq!(times(&h, v, "opacity"), before);
    h.ok("clip.slip", json!({"item": v, "delta": "1s"}));
    assert_eq!(times(&h, v, "opacity"), before);
    h.ok("item.move", json!({"item": v, "start": "5s"}));
    assert_eq!(times(&h, v, "opacity"), before);
    let _ = p;
}

#[test]
fn a_roll_moves_the_keys_of_the_right_clip_only() {
    let (mut h, p, m, v, _) = setup();
    // A second clip after the first, with keys; both 10 s of the 10 s media would not leave room
    // to roll, so use shorter ones.
    h.ok(
        "clip.trim",
        json!({"item": v, "edge": "end", "delta": "-4s"}),
    );
    let right = item_id(
        &h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": m, "at": "8s",
               "duration": "4s", "source_in": "2s", "link": false}),
        )["clips"][0],
    );
    set(&mut h, v, "opacity", json!([{"t": "1s", "value": 0.2}]));
    set(&mut h, right, "opacity", json!([{"t": "1s", "value": 0.7}]));
    h.ok(
        "clip.roll",
        json!({"left": v, "right": right, "delta": "1s"}),
    );
    assert_eq!(
        times(&h, v, "opacity"),
        vec![S],
        "the left clip's tail trim keeps keys"
    );
    assert_eq!(
        times(&h, right, "opacity"),
        vec![0],
        "the right clip's head moved 1 s in"
    );
}

// ----- a drag is one undo step (gestures) -----------------------------------------------------------------

static REGISTRY: LazyLock<Arc<Registry>> = LazyLock::new(|| {
    let mut registry = Registry::new();
    cutweave_ops::register(&mut registry).unwrap();
    Arc::new(registry)
});

#[test]
fn dragging_a_key_under_a_gesture_is_one_undo_step() {
    let (mut h, _, _, v, _) = setup();
    animated(&mut h, v);
    let doc = h.doc();
    let config = DocConfig::deterministic(Arc::clone(&REGISTRY), doc.clone(), 7);
    let mut state = DocState::new(config);
    let ctx = ActorCtx::ui(ConnId::new(1));
    let tx = state
        .begin_tx(&ctx, TxKind::Gesture, "Drag keyframe")
        .unwrap()
        .id;
    // Each pointer move is a `keyframe.move` of the same key to a new frame.
    let mut at = S;
    for step in 1..=6_i64 {
        let to = S + step * FRAME30;
        let submit = Submit::new(
            ctx.clone(),
            "keyframe.move",
            json!({"item": v, "keys": [{"prop": "opacity", "t": at}], "to_t": to}),
        )
        .in_tx(tx);
        state.submit(submit).unwrap();
        at = to;
    }
    state.commit_tx(&ctx, tx).unwrap();
    assert_eq!(
        times_of(state.doc(), v),
        vec![0, S + 6 * FRAME30, 2 * S],
        "the key ended where the drag ended"
    );
    // One undo restores the document from before the drag.
    state.undo(&ctx, &HistoryScope::Any).unwrap();
    assert_eq!(state.doc(), &doc);
    // And one redo brings the whole drag back.
    state.redo(&ctx, &HistoryScope::Any).unwrap();
    assert_eq!(times_of(state.doc(), v), vec![0, S + 6 * FRAME30, 2 * S]);
}

fn times_of(doc: &cutweave_model::Project, item: ItemId) -> Vec<i64> {
    let clip = doc.find_item(&item).unwrap().item.as_clip().unwrap();
    clip.visual
        .opacity
        .sample_times()
        .map(Time::flicks)
        .collect()
}
