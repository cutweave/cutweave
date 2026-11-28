// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-3A.01 acceptance: the `text.*` commands and queries (arch §4.2, §11). The harness checks
//! undo/redo identity and document validity after every successful command; here the
//! behaviour: one call adds a titled text, bad sizes and colours are `Invalid{path,msg}`, a
//! locked track is `Locked`, run styles split and merge runs, the style properties are keyed
//! with `keyframe.*`, and the generic edits (move, delete, trim) treat a text item like any
//! other item.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, P, assert_code, assert_invalid, id, item_id};
use cutweave_model::{Item, ItemId, Project, TrackKind, open_str, to_document_string};
use serde_json::{Value, json};

fn text_of(h: &H, item: ItemId) -> cutweave_model::TextItem {
    h.doc()
        .find_item(&item)
        .unwrap()
        .item
        .as_text()
        .unwrap()
        .clone()
}

fn add(h: &mut H, params: Value) -> Value {
    h.ok("text.add", params)
}

fn project() -> (H, P) {
    H::project()
}

#[test]
fn one_call_adds_a_titled_text_with_defaults() {
    let (mut h, p) = project();
    let out = add(&mut h, json!({"text": "Hello"}));
    assert_eq!(out["created_track"], true);
    assert_eq!(out["start"], 0);
    assert_eq!(out["duration"], 5 * 705_600_000_i64);
    let item = item_id(&out["item"]);
    let doc = h.doc();
    let seq = &doc.sequences[&p.sequence];
    let track = seq.track(&id(&out["track"])).unwrap();
    assert_eq!(track.kind, TrackKind::Text);
    assert_eq!(
        seq.track_order[0], track.id,
        "the text track is on top of the stack"
    );
    let text = text_of(&h, item);
    assert_eq!(text.content.to_plain(), "Hello");
    assert_eq!(
        text.resolved_style().size,
        96.0.into(),
        "the built-in title"
    );
    // `timeline.get` shows it.
    let got = h
        .query("timeline.get", json!({"sequence": p.sequence, "depth": 2}))
        .unwrap();
    let shown = got.to_string();
    assert!(shown.contains(r#""kind":"text""#), "{shown}");
    assert!(shown.contains(r#""name":"Hello""#), "{shown}");
    // A second text at the same time goes on a second track; at another time, on the first.
    let second = add(&mut h, json!({"text": "Under", "at": "6s"}));
    assert_eq!(second["created_track"], false);
    assert_eq!(second["track"], out["track"]);
    let third = add(&mut h, json!({"text": "Over", "at": "1s"}));
    assert_eq!(third["created_track"], true);
    assert_ne!(third["track"], out["track"]);
}

#[test]
fn the_document_with_text_round_trips() {
    let (mut h, _) = project();
    let out = add(
        &mut h,
        json!({"text": "Round trip", "style_name": "subtitle", "box": {"width": 0.5},
               "position": {"x": 0.0, "y": 0.3}}),
    );
    let item = item_id(&out["item"]);
    h.ok(
        "text.set_style",
        json!({"item": item, "patch": {"size": 50.0, "strokes": [
            {"width": 3.0, "join": "round", "color": "#000000", "order": "behind"}]}}),
    );
    h.ok(
        "text.set_run_style",
        json!({"item": item, "start": 0, "end": 5, "patch": {"size": 70.0}}),
    );
    let doc = h.doc();
    let text = to_document_string(&doc).unwrap();
    let cutweave_model::OpenOutcome::Editable { project, .. } = open_str(&text).unwrap() else {
        panic!("editable");
    };
    assert_eq!(project, doc);
    assert_eq!(to_document_string(&project).unwrap(), text);
}

#[test]
fn bad_values_are_invalid_with_a_path() {
    let (mut h, _) = project();
    assert_invalid(
        h.run("text.add", json!({"text": "x", "style": {"size": 0}})),
        "/style/size",
    );
    assert_invalid(
        h.run(
            "text.add",
            json!({"text": "x", "style": {"fill": {"kind": "solid", "color": "red"}}}),
        ),
        "/style/fill/color",
    );
    assert_invalid(h.run("text.add", json!({"text": ""})), "/text");
    assert_invalid(
        h.run("text.add", json!({"text": "x", "style_name": "huge"})),
        "/style_name",
    );
    assert_invalid(
        h.run("text.add", json!({"text": "x", "box": {"width": 0.0}})),
        "/box/width",
    );
    let item = item_id(&add(&mut h, json!({"text": "Hello"}))["item"]);
    assert_invalid(
        h.run(
            "text.set_style",
            json!({"item": item, "patch": {"size": 100_000}}),
        ),
        "/patch/size",
    );
    assert_invalid(
        h.run(
            "text.set_style",
            json!({"item": item, "patch": {"strokes": [{"width": -2.0, "join": "round",
                   "color": "#000000", "order": "behind"}]}}),
        ),
        "/patch/strokes/0/width",
    );
    assert_invalid(
        h.run(
            "text.set_style",
            json!({"item": item, "patch": {"nope": 1}}),
        ),
        "/patch/nope",
    );
    assert_invalid(
        h.run(
            "text.set_run_style",
            json!({"item": item, "start": 0, "end": 99, "patch": {}}),
        ),
        "/end",
    );
    assert_invalid(
        h.run(
            "text.set_run_style",
            json!({"item": item, "start": 0, "end": 2, "patch": {"size": -1.0}}),
        ),
        "/patch/size",
    );
    assert_invalid(
        h.run("text.add_emoji", json!({"item": item, "emoji": "abc"})),
        "/emoji",
    );
    assert_invalid(
        h.run(
            "text.set_box",
            json!({"item": item, "box": {"height": -1.0}}),
        ),
        "/box/height",
    );
    assert_invalid(
        h.run("text.set_content", json!({"item": item, "text": ""})),
        "/text",
    );
    assert_invalid(h.run("text.set_content", json!({"item": item})), "");
    // A bad value changed nothing (the harness asserts it); the text is as it was.
    assert_eq!(text_of(&h, item).content.to_plain(), "Hello");
}

#[test]
fn a_locked_track_is_locked() {
    let (mut h, _) = project();
    let out = add(&mut h, json!({"text": "Hello"}));
    let item = item_id(&out["item"]);
    h.ok("track.set", json!({"track": out["track"], "locked": true}));
    for (name, params) in [
        ("text.set_content", json!({"item": item, "text": "x"})),
        (
            "text.set_style",
            json!({"item": item, "patch": {"size": 40.0}}),
        ),
        (
            "text.set_run_style",
            json!({"item": item, "start": 0, "end": 2, "patch": {"size": 40.0}}),
        ),
        ("text.set_box", json!({"item": item, "box": {"width": 0.5}})),
        (
            "text.add_emoji",
            json!({"item": item, "emoji": "\u{1F389}"}),
        ),
        (
            "text.add",
            json!({"text": "x", "track": out["track"], "at": "9s"}),
        ),
    ] {
        assert_code(h.run(name, params), "locked");
    }
    // Without `track`, `text.add` goes round the locked one: a new track.
    let other = add(&mut h, json!({"text": "Free"}));
    assert_eq!(other["created_track"], true);
    // Reading is fine.
    h.query("text.get", json!({"item": item})).unwrap();
}

#[test]
fn content_run_styles_emoji_and_box() {
    let (mut h, _) = project();
    let item = item_id(&add(&mut h, json!({"text": "Hello world"}))["item"]);
    h.ok(
        "text.set_run_style",
        json!({"item": item, "start": 6, "end": 11,
               "patch": {"size": 120.0, "fill": {"kind": "solid", "color": "#ffd400"}}}),
    );
    let t = text_of(&h, item);
    let runs: Vec<_> = t.content.runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(runs, ["Hello ", "world"]);
    assert_eq!(t.content.runs[1].style.as_ref().unwrap().size, Some(120.0));
    // Removing the override merges the runs again.
    h.ok(
        "text.set_run_style",
        json!({"item": item, "start": 6, "end": 11, "patch": {"size": null, "fill": null}}),
    );
    assert_eq!(text_of(&h, item).content.runs.len(), 1);
    // Emoji at the end and in the middle.
    h.ok(
        "text.add_emoji",
        json!({"item": item, "emoji": "\u{1F389}"}),
    );
    h.ok(
        "text.add_emoji",
        json!({"item": item, "emoji": "\u{1F469}\u{200D}\u{1F4BB}", "at": 5}),
    );
    assert_eq!(
        text_of(&h, item).content.to_plain(),
        "Hello\u{1F469}\u{200D}\u{1F4BB} world\u{1F389}"
    );
    // Runs in `set_content`.
    h.ok(
        "text.set_content",
        json!({"item": item, "runs": [{"text": "A"},
               {"text": "B", "style": {"size": 10.0}}, {"text": "C"}]}),
    );
    assert_eq!(text_of(&h, item).content.runs.len(), 3);
    // The box.
    let out = h.ok(
        "text.set_box",
        json!({"item": item, "box": {"width": 0.6, "height": 0.2}}),
    );
    assert_eq!(out["box"], json!({"width": 0.6, "height": 0.2}));
    h.ok("text.set_box", json!({"item": item, "box": {}}));
    assert_eq!(
        text_of(&h, item).text_box,
        cutweave_model::text::TextBox::default()
    );
}

#[test]
fn set_style_merges_and_detaches_the_built_in() {
    let (mut h, _) = project();
    let item = item_id(&add(&mut h, json!({"text": "Hello"}))["item"]);
    assert!(matches!(
        text_of(&h, item).style,
        cutweave_model::text::TextStyleRef::Named { .. }
    ));
    let out = h.ok(
        "text.set_style",
        json!({"item": item, "patch": {"font": {"family": "Lora"}, "align": "left"}}),
    );
    assert_eq!(out["style"]["font"]["family"], "Lora");
    assert_eq!(
        out["style"]["font"]["weight"], 800,
        "the rest of the title stays"
    );
    assert!(matches!(
        text_of(&h, item).style,
        cutweave_model::text::TextStyleRef::Inline { .. }
    ));
    // A glow, then cleared with null.
    h.ok(
        "text.set_style",
        json!({"item": item, "patch": {"glow": {"radius": 8.0, "color": "#ffffff", "intensity": 1.0}}}),
    );
    assert!(text_of(&h, item).resolved_style().glow.is_some());
    h.ok(
        "text.set_style",
        json!({"item": item, "patch": {"glow": null}}),
    );
    assert!(text_of(&h, item).resolved_style().glow.is_none());
    // Starting over from another built-in.
    let out = h.ok(
        "text.set_style",
        json!({"item": item, "style_name": "body", "patch": {}}),
    );
    assert_eq!(out["style"]["size"], 40.0);
}

#[test]
fn queries() {
    let (mut h, _) = project();
    let item = item_id(&add(&mut h, json!({"text": "Hello", "name": "Opening"}))["item"]);
    let got = h.query("text.get", json!({"item": item})).unwrap();
    assert_eq!(got["plain"], "Hello");
    assert_eq!(got["item"]["name"], "Opening");
    assert_eq!(got["item"]["style"]["name"], "title");
    assert_eq!(got["style"]["font"]["family"], "Inter");
    let styles = h.query("text.list_styles", json!({})).unwrap();
    let names: Vec<&str> = styles["styles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["title", "subtitle", "body"]);
    // A clip is not a text item.
    let v = h.add_video("/m/a.mp4", 10, false);
    let clip = item_id(
        &h.ok(
            "timeline.insert",
            json!({"sequence": h.doc().sequences.keys().next().unwrap(),
                   "track": h.doc().sequences.values().next().unwrap()
                       .tracks_in_order().find(|t| t.kind == TrackKind::Video).unwrap().id,
                   "media": v, "at": 0, "link": false}),
        )["clips"][0],
    );
    assert_invalid(h.query("text.get", json!({"item": clip})), "/item");
    assert_invalid(
        h.run("text.set_content", json!({"item": clip, "text": "x"})),
        "/item",
    );
}

#[test]
fn style_properties_are_keyed_with_the_keyframe_commands() {
    let (mut h, _) = project();
    let item = item_id(&add(&mut h, json!({"text": "Hello", "duration": "4s"}))["item"]);
    // Keying `text.size` gives the item its own copy of the built-in style.
    h.ok(
        "keyframe.set",
        json!({"item": item, "prop": "text.size", "keys": [
            {"t": "0s", "value": 40.0, "easing": "ease_out"}, {"t": "1s", "value": 96.0}]}),
    );
    let text = text_of(&h, item);
    let style = match &text.style {
        cutweave_model::text::TextStyleRef::Inline { style } => style,
        other @ cutweave_model::text::TextStyleRef::Named { .. } => panic!("detached: {other:?}"),
    };
    assert!(style.size.is_animated());
    // The item's visual properties key the same way.
    h.ok(
        "keyframe.set",
        json!({"item": item, "prop": "opacity", "keys": [{"t": "0s", "value": 0.0},
               {"t": "0.5s", "value": 1.0}]}),
    );
    let out = h
        .query(
            "keyframe.eval",
            json!({"item": item, "prop": "text.size", "at": "0.5s"}),
        )
        .unwrap();
    assert!(out["samples"][0]["value"].as_f64().unwrap() > 40.0);
    // A value out of range, and a property a text item does not have.
    assert_code(
        h.run(
            "keyframe.set",
            json!({"item": item, "prop": "text.size", "keys": [{"t": "2s", "value": 0.0}]}),
        ),
        "invalid",
    );
    assert_code(
        h.run(
            "keyframe.set",
            json!({"item": item, "prop": "audio_gain", "keys": [{"t": "2s", "value": 3.0}]}),
        ),
        "unsupported",
    );
    // Delete and clear work on text too.
    let keys = h
        .query(
            "keyframe.copy",
            json!({"item": item, "keys": [{"prop": "text.size", "t": 0}]}),
        )
        .unwrap();
    assert_eq!(keys["clipboard"]["keys"].as_array().unwrap().len(), 1);
    h.ok("keyframe.clear", json!({"item": item, "prop": "text.size"}));
    // `item.set_props` sets static opacity and position on a text item.
    h.ok(
        "item.set_props",
        json!({"item": item, "props": {"transform": {"position": {"x": 0.1, "y": 0.2}}}}),
    );
    let t = text_of(&h, item);
    assert_eq!(t.visual.transform.position.value().y, 0.2);
    // A clip-only property is refused.
    assert_code(
        h.run(
            "item.set_props",
            json!({"item": item, "props": {"audio_gain": 3.0}}),
        ),
        "unsupported",
    );
}

#[test]
fn the_generic_edits_treat_text_like_any_item() {
    let (mut h, ids) = project();
    let first = add(&mut h, json!({"text": "A", "at": "0s", "duration": "4s"}));
    let item = item_id(&first["item"]);
    // Trimming and splitting are clip edits.
    assert_code(
        h.run("clip.split", json!({"item": item, "at": "2s"})),
        "unsupported",
    );
    // Moving works, within a text track only.
    h.ok("item.move", json!({"item": item, "start": "10s"}));
    assert_eq!(text_of(&h, item).start.flicks(), 10 * 705_600_000);
    assert_code(
        h.run("item.move", json!({"item": item, "to_track": ids.video})),
        "invalid",
    );
    // Duplicating copies it with a new id.
    let dup = h.ok("item.duplicate", json!({"items": [item], "offset": "5s"}));
    assert!(dup.to_string().contains("itm_"), "{dup}");
    // A text track takes no media.
    let video = h.add_video("/m/a.mp4", 10, false);
    assert_code(
        h.run(
            "timeline.insert",
            json!({"sequence": ids.sequence, "track": first["track"], "media": video, "at": 0}),
        ),
        "invalid",
    );
    // Deleting removes it; undo brings it back (the harness checks the identity).
    h.ok("item.delete", json!({"items": [item]}));
    assert!(h.doc().find_item(&item).is_none());
    // `track.add` takes `text` now.
    let added = h.ok(
        "track.add",
        json!({"sequence": ids.sequence, "kind": "text"}),
    );
    assert!(added["track"].is_string());
    let _: &Project = &h.doc();
    let _ = Item::Text;
}

/// Copies of text: `item.duplicate` makes a new item and leaves the first where it is, a
/// copy and paste does the same, and `sequence.duplicate` carries the text over with new IDs.
#[test]
fn copies_of_text_are_new_items() {
    let (mut h, ids) = project();
    let first = add(
        &mut h,
        json!({"text": "Copy me", "at": "1s", "duration": "2s"}),
    );
    let item = item_id(&first["item"]);
    let dup = h.ok(
        "item.duplicate",
        json!({"items": [item], "offset": "5s", "mode": "new_track"}),
    );
    let copy = item_id(&dup["copies"][0]["copy"]);
    assert_ne!(copy, item, "{dup}");
    let (a, b) = (text_of(&h, item), text_of(&h, copy));
    assert_eq!(a.start.flicks(), 705_600_000);
    assert_eq!(b.start.flicks(), 6 * 705_600_000, "the copy is 5 s later");
    assert_eq!(a.content, b.content);
    // Paste (copy first, then paste at another time).
    let copied = h.query("timeline.copy", json!({"items": [item]})).unwrap();
    let pasted = h.ok(
        "timeline.paste",
        json!({"payload": copied["clipboard"], "at": "20s"}),
    );
    assert!(pasted.to_string().contains("itm_"), "{pasted}");
    assert_eq!(
        h.doc().sequences[&ids.sequence]
            .tracks
            .values()
            .flat_map(|t| t.items.iter())
            .filter(|i| i.as_text().is_some())
            .count(),
        3
    );
    // A duplicated sequence carries its text.
    let seq = h.ok("sequence.duplicate", json!({"sequence": ids.sequence}));
    let new_seq: cutweave_model::SequenceId = id(&seq["sequence"]);
    let texts: Vec<_> = h.doc().sequences[&new_seq]
        .tracks
        .values()
        .flat_map(|t| t.items.iter())
        .filter_map(|i| i.as_text().map(|t| t.content.to_plain()))
        .collect();
    assert_eq!(texts.len(), 3);
}
