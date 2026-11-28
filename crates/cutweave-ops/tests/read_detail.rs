// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2I.08 (ADR-0040, question 5): `timeline.get` and `media.list` take
//! `detail: "concise" | "full"`. The `*_default` snapshots were taken before the parameter
//! existed: a call without it (and one with `"full"`) still returns those bytes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, P, assert_invalid, insert_av};
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

// ----- the detail levels ----------------------------------------------------------------------

#[test]
fn full_is_the_default_of_both_queries() {
    let (h, p) = timeline();
    for depth in 0..=2 {
        assert_eq!(
            wire(&get(&h, p, json!({"depth": depth, "detail": "full"}))),
            wire(&get(&h, p, json!({"depth": depth}))),
        );
    }
    let h = bin();
    assert_eq!(
        wire(&h.query("media.list", json!({"detail": "full"})).unwrap()),
        wire(&h.query("media.list", json!({})).unwrap()),
    );
}

#[test]
fn a_concise_timeline_keeps_ids_names_and_times_and_drops_defaults() {
    let (h, p) = timeline();
    for depth in 0..=2 {
        insta::assert_snapshot!(
            format!("timeline_get_depth_{depth}_concise"),
            wire(&get(&h, p, json!({"depth": depth, "detail": "concise"})))
        );
    }
    let (full, concise) = (
        get(&h, p, json!({"depth": 2})),
        get(&h, p, json!({"depth": 2, "detail": "concise"})),
    );
    // The sequence's own fields are the same.
    for key in [
        "id",
        "name",
        "format",
        "duration",
        "track_count",
        "item_count",
    ] {
        assert_eq!(concise[key], full[key], "{key}");
    }
    let (picture, sound) = (&concise["tracks"][0], &concise["tracks"][1]);
    for (track, all) in [(picture, &full["tracks"][0]), (sound, &full["tracks"][1])] {
        for key in ["id", "kind", "name", "item_count", "end"] {
            assert_eq!(track[key], all[key], "{key}");
        }
        for key in ["hidden", "locked", "solo", "magnetic"] {
            assert!(track.get(key).is_none(), "{key} is at its default");
        }
    }
    // A value away from its default stays.
    assert_eq!(picture["muted"], true);
    assert_eq!(picture["height"], 96);
    assert!(picture.get("gain").is_none());
    assert_eq!(sound["gain"], -3.0);
    assert!(sound.get("muted").is_none() && sound.get("height").is_none());

    let items = picture["items"].as_array().unwrap();
    let all = full["tracks"][0]["items"].as_array().unwrap();
    assert_eq!(items.len(), all.len());
    for (item, whole) in items.iter().zip(all) {
        for key in ["id", "start", "duration"] {
            assert_eq!(item[key], whole[key], "{key}");
        }
        for key in ["kind", "media", "linked_group", "color_label"] {
            assert!(item.get(key).is_none(), "{key}");
        }
    }
    assert_eq!(items[1]["name"], "Opening title");
    assert!(items[0].get("enabled").is_none(), "enabled is the default");
    assert_eq!(items[2]["enabled"], false);
}

#[test]
fn a_concise_page_continues_like_a_full_one() {
    let (h, p) = timeline();
    let args = |detail: &str| json!({"depth": 2, "limit": 2, "detail": detail});
    let (full, concise) = (get(&h, p, args("full")), get(&h, p, args("concise")));
    assert!(full["next_cursor"].is_string());
    assert_eq!(concise["next_cursor"], full["next_cursor"]);
    let next = get(
        &h,
        p,
        json!({"depth": 2, "limit": 2, "detail": "concise", "cursor": concise["next_cursor"]}),
    );
    assert!(next.get("next_cursor").is_none());
    assert_eq!(next["tracks"][0]["items"].as_array().unwrap().len(), 1);
    assert_eq!(next["tracks"][1]["items"].as_array().unwrap().len(), 1);
}

#[test]
fn a_concise_bin_keeps_ids_kinds_and_names_and_drops_file_details() {
    let h = bin();
    insta::assert_snapshot!(
        "media_list_concise",
        wire(&h.query("media.list", json!({"detail": "concise"})).unwrap())
    );
    insta::assert_snapshot!(
        "media_list_note_concise",
        wire(
            &h.query(
                "media.list",
                json!({"include": ["note"], "limit": 1, "detail": "concise"})
            )
            .unwrap()
        )
    );
    let full = h.query("media.list", json!({})).unwrap();
    let concise = h.query("media.list", json!({"detail": "concise"})).unwrap();
    for key in ["next_cursor", "total", "matched"] {
        assert_eq!(concise[key], full[key], "{key}");
    }
    let (tagged, plain) = (&concise["media"][0], &concise["media"][1]);
    for (asset, all) in [(tagged, &full["media"][0]), (plain, &full["media"][1])] {
        for key in ["id", "kind", "name", "duration"] {
            assert_eq!(asset[key], all[key], "{key}");
        }
        for key in ["path", "has_video", "has_audio"] {
            assert!(asset.get(key).is_none(), "{key}");
        }
    }
    for key in ["folder", "tags", "rating"] {
        assert_eq!(tagged[key], full["media"][0][key], "{key}");
        assert!(plain.get(key).is_none(), "{key} has no value");
    }
}

#[test]
fn an_unknown_detail_is_refused() {
    let (h, p) = timeline();
    assert_invalid(
        h.query(
            "timeline.get",
            json!({"sequence": p.sequence, "detail": "tiny"}),
        ),
        "/detail",
    );
    assert_invalid(h.query("media.list", json!({"detail": "names"})), "/detail");
}

/// The described output schema is true of both forms (ADR-0028): what concise may leave out is
/// not required, and nothing else changed its place.
#[test]
fn the_output_schemas_describe_both_forms() {
    let h = H::empty();
    let schema = |name: &str| h.registry.get(name).unwrap().meta.output_schema.clone();
    let required = |schema: &Value| -> Vec<String> {
        schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    };
    let timeline = schema("timeline.get");
    let track = &timeline["properties"]["tracks"]["items"];
    assert_eq!(required(track), ["id", "kind", "name", "item_count", "end"]);
    assert_eq!(
        track["properties"]["muted"],
        json!({"type": "boolean", "description": "Silenced. Concise leaves out `false`."})
    );
    let item = &track["properties"]["items"]["items"];
    assert_eq!(required(item), ["id", "start", "duration"]);
    let media = &schema("media.list")["properties"]["media"]["items"];
    assert_eq!(required(media), ["id", "kind", "name"]);
    assert_eq!(media["properties"]["tags"]["type"], "array");
}
