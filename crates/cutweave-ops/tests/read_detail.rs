// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2I.08 (ADR-0040, question 5): `timeline.get` and `media.list` take
//! `detail: "concise" | "full"`. The `*_default` snapshots were taken before the parameter
//! existed: a call without it (and one with `"full"`) still returns those bytes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, P, insert_av};
use serde_json::{Value, json};

/// The result as it goes over the wire: compact JSON, fields in their serialized order.
fn wire(out: &Value) -> String {
    serde_json::to_string(out).unwrap()
}

/// A timeline with every item and track property away from its default somewhere: a linked
/// picture and sound, a named clip, a disabled one, a color label; a muted, taller track and a
/// quieter one.
fn timeline() -> (H, P) {
    let (mut h, p) = H::project();
    let av = h.add_video("/m/interview.mp4", 60, true);
    let still = h.add_image("/m/title.png");
    let (v, _a) = insert_av(&mut h, p, av, "0s");
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": still, "at": "70s",
               "duration": "4s"}),
    );
    let title = out["clips"][0].clone();
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": still, "at": "80s",
               "duration": "2s"}),
    );
    let off = out["clips"][0].clone();
    h.ok(
        "item.set_props",
        json!({"item": title, "props": {"name": "Opening title"}}),
    );
    h.ok(
        "item.set_props",
        json!({"item": off, "props": {"enabled": false}}),
    );
    h.ok(
        "item.set_label_color",
        json!({"items": [v], "color": "#ff8800"}),
    );
    h.ok(
        "track.set",
        json!({"track": p.video, "muted": true, "name": "Picture"}),
    );
    h.ok("track.set_height", json!({"track": p.video, "height": 96}));
    h.ok("track.set", json!({"track": p.audio, "gain": -3.0}));
    (h, p)
}

/// A bin with an asset in a folder with tags, a rating and a note, and a plain one.
fn bin() -> H {
    let (mut h, _) = H::project();
    let a = h.add_video("/m/interview.mp4", 60, true);
    h.add_audio("/m/music.wav", 120);
    let folder = h.ok("media.create_folder", json!({"name": "Day 1"}));
    h.ok(
        "media.move",
        json!({"media": [a], "to": folder["folder_id"]}),
    );
    h.ok(
        "media.add_tags",
        json!({"media": [a], "tags": ["interview", "b-roll"]}),
    );
    h.ok("media.set_rating", json!({"media": [a], "rating": 4}));
    h.ok("media.set_note", json!({"media": a, "note": "Good take."}));
    h
}

fn get(h: &H, p: P, extra: Value) -> Value {
    let mut params = json!({"sequence": p.sequence});
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    h.query("timeline.get", params).unwrap()
}

// ----- today's results (snapshots from before WP-2I.08) ---------------------------------------

#[test]
fn timeline_get_without_detail_is_unchanged() {
    let (h, p) = timeline();
    for depth in 0..=2 {
        insta::assert_snapshot!(
            format!("timeline_get_depth_{depth}_default"),
            wire(&get(&h, p, json!({"depth": depth})))
        );
    }
    insta::assert_snapshot!(
        "timeline_get_page_default",
        wire(&get(
            &h,
            p,
            json!({"depth": 2, "limit": 2, "fields": ["tracks"]})
        ))
    );
}

#[test]
fn media_list_without_detail_is_unchanged() {
    let h = bin();
    insta::assert_snapshot!(
        "media_list_default",
        wire(&h.query("media.list", json!({})).unwrap())
    );
    insta::assert_snapshot!(
        "media_list_note_default",
        wire(
            &h.query("media.list", json!({"include": ["note"], "limit": 1}))
                .unwrap()
        )
    );
}
