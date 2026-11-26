// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2B.01: `sequence.*`: create (presets), create from media, duplicate, rename, delete,
//! set_format (reframe and frame-rate re-snap), list, and `view.set_active_sequence`.
//!
//! `H::run` asserts the undo/redo identity and `Project::validate` after every success and that
//! a failure changes nothing, so every call here also covers the "undo identity" criterion.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names,
    clippy::cast_precision_loss
)]

mod common;

use std::collections::BTreeSet;

use common::{H, P, assert_code, assert_invalid, id};
use cutweave_model::{
    ASPECT_PRESETS, Clip, FrameRate, Item, ItemId, MediaId, Project, Rounding, Sequence,
    SequenceId, TrackId, Vec2,
};
use cutweave_ops::{
    sequence::reframe::{Canvas, fit_size, placed_rect},
    testing::{arb_command_among, check_invariants},
};
use proptest::{
    collection::vec,
    prelude::*,
    strategy::ValueTree,
    test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
};
use serde_json::{Value, json};

fn seq(h: &H, id: SequenceId) -> Sequence {
    h.doc().sequences[&id].clone()
}

fn clips(s: &Sequence) -> Vec<Clip> {
    let mut v: Vec<Clip> = s
        .tracks
        .values()
        .flat_map(|t| t.items.iter().filter_map(|i| i.as_clip().cloned()))
        .collect();
    v.sort_by_key(|c| c.id);
    v
}

// ----- sequence.create -----------------------------------------------------------------------

#[test]
fn create_from_each_preset() {
    let (mut h, p) = H::project();
    for preset in ASPECT_PRESETS {
        let out = h.ok(
            "sequence.create",
            json!({"name": format!("S {}", preset.id), "preset": preset.id}),
        );
        let s = seq(&h, id(&out["sequence"]));
        assert_eq!(
            (s.format.width, s.format.height),
            (preset.width, preset.height)
        );
        // The rest of the format is the project default, V1 over A1.
        assert_eq!(s.format.frame_rate, FrameRate::FPS_30);
        let names: Vec<_> = s.tracks_in_order().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["V1", "A1"]);
        assert_eq!(
            out["video_track"],
            json!(s.tracks_in_order().next().unwrap().id)
        );
    }
    assert_eq!(h.doc().sequences.len(), 1 + ASPECT_PRESETS.len());
    assert!(h.doc().sequences.contains_key(&p.sequence));
}

#[test]
fn create_with_a_format_overrides_the_preset_fields() {
    let (mut h, _) = H::project();
    let out = h.ok(
        "sequence.create",
        json!({"name": "Reel", "preset": "9:16",
               "format": {"frame_rate": {"num": 60, "den": 1}, "background": "#102030"}}),
    );
    let f = seq(&h, id(&out["sequence"])).format;
    assert_eq!(
        (f.width, f.height, f.frame_rate),
        (1080, 1920, FrameRate::FPS_60)
    );
    assert_eq!(f.background.to_string(), "#102030");
    let out = h.ok(
        "sequence.create",
        json!({"name": "Odd", "format": {"width": 800, "height": 600}}),
    );
    let f = seq(&h, id(&out["sequence"])).format;
    assert_eq!((f.width, f.height), (800, 600));
}

#[test]
fn create_rejects_bad_input() {
    let (mut h, _) = H::project();
    assert_invalid(
        h.run("sequence.create", json!({"name": "S", "preset": "2:3"})),
        "/preset",
    );
    assert_invalid(h.run("sequence.create", json!({"name": " "})), "/name");
    assert_invalid(
        h.run(
            "sequence.create",
            json!({"name": "S", "format": {"width": 0}}),
        ),
        "/format",
    );
    assert_invalid(
        h.run(
            "sequence.create",
            json!({"name": "S", "format": {"frame_rate": {"num": 1001, "den": 7}}}),
        ),
        "/format",
    );
}

// ----- sequence.create_from_media ------------------------------------------------------------

#[test]
fn create_from_media_places_the_assets_in_order() {
    let (mut h, _) = H::project();
    let video = h.add_video("/m/v.mp4", 4, true);
    let silent = h.add_video("/m/s.mp4", 2, false);
    let sound = h.add_audio("/m/a.wav", 3);
    let image = h.add_image("/m/i.png");
    let out = h.ok(
        "sequence.create_from_media",
        json!({"media": [video, silent, image, sound], "name": "From bin"}),
    );
    let s = seq(&h, id(&out["sequence"]));
    assert_eq!(s.name, "From bin");
    let v1 = &s.tracks[&id::<TrackId>(&out["video_track"])];
    let a1 = &s.tracks[&id::<TrackId>(&out["audio_track"])];
    let spans = |t: &cutweave_model::Track| -> Vec<(MediaId, i64, i64)> {
        t.items
            .iter()
            .map(|i| {
                (
                    i.media().unwrap(),
                    i.start().flicks() / 23_520_000,
                    i.duration().flicks() / 23_520_000,
                )
            })
            .collect()
    };
    assert_eq!(
        spans(v1),
        [(video, 0, 120), (silent, 120, 60), (image, 180, 150)]
    );
    // The video's sound is linked with it; the audio file follows on A1 after the still.
    assert_eq!(spans(a1), [(video, 0, 120), (sound, 330, 90)]);
    let v_clip = v1.items.iter().next().unwrap();
    let a_clip = a1.items.iter().next().unwrap();
    assert!(v_clip.linked_group().is_some());
    assert_eq!(v_clip.linked_group(), a_clip.linked_group());
}

#[test]
fn create_from_media_rejects_bad_input() {
    let (mut h, _) = H::project();
    let video = h.add_video("/m/v.mp4", 4, true);
    assert_invalid(
        h.run("sequence.create_from_media", json!({"media": []})),
        "/media",
    );
    assert_code(
        h.run(
            "sequence.create_from_media",
            json!({"media": [video, MediaId::from_u128(9)]}),
        ),
        "not_found",
    );
}

// ----- sequence.duplicate --------------------------------------------------------------------

/// A sequence with linked pairs, a still, a second video track, markers and an attachment.
fn busy() -> (H, P) {
    let (mut h, p) = H::project();
    let video = h.add_video("/m/v.mp4", 30, true);
    let image = h.add_image("/m/i.png");
    for at in ["f:0", "f:91", "f:203"] {
        h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": video, "at": at,
                   "duration": "f:61"}),
        );
    }
    let v2 = h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    );
    let still = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": v2["track"], "media": image, "at": "f:10",
               "duration": "f:40"}),
    );
    h.ok(
        "timeline.add_marker",
        json!({"sequence": p.sequence, "t": "f:15", "label": "Hook"}),
    );
    let first = h.doc().sequences[&p.sequence].tracks[&p.video]
        .items
        .iter()
        .next()
        .unwrap()
        .id();
    h.ok(
        "timeline.add_marker",
        json!({"sequence": p.sequence, "item": first, "t": "f:5", "label": "Item"}),
    );
    drop(still);
    (h, p)
}

fn ids_of(s: &Sequence) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    out.insert(s.id.to_string());
    out.extend(s.markers.keys().map(ToString::to_string));
    for t in s.tracks.values() {
        out.insert(t.id.to_string());
        for (_, i) in t.items.iter_by_id() {
            out.insert(i.id().to_string());
            out.extend(i.markers().keys().map(ToString::to_string));
            out.extend(i.linked_group().map(|g| g.to_string()));
        }
    }
    out
}

/// A structural picture of a sequence that ignores IDs: tracks in order with their items'
/// timing, media and properties, and which items share a group.
fn shape(s: &Sequence) -> Value {
    let mut groups = std::collections::BTreeMap::new();
    let tracks: Vec<Value> = s
        .tracks_in_order()
        .map(|t| {
            let items: Vec<Value> = t
                .items
                .iter()
                .map(|i| {
                    let mut v = serde_json::to_value(i).unwrap();
                    let n = groups.len();
                    let g = i.linked_group().map(|g| *groups.entry(g).or_insert(n));
                    for key in ["id", "linked_group", "attached_to", "markers"] {
                        v.as_object_mut().unwrap().remove(key);
                    }
                    v["group"] = json!(g);
                    v["marker_count"] = json!(i.markers().len());
                    v
                })
                .collect();
            json!({"kind": t.kind, "name": t.name, "magnetic": t.magnetic, "items": items})
        })
        .collect();
    json!({"format": s.format, "tracks": tracks, "markers": s.markers.len()})
}

#[test]
fn duplicate_is_a_deep_copy_with_new_ids_and_shared_media() {
    let (mut h, p) = busy();
    let out = h.ok("sequence.duplicate", json!({"sequence": p.sequence}));
    let copy = seq(&h, id(&out["sequence"]));
    let original = seq(&h, p.sequence);
    assert_eq!(copy.name, "Sequence 1 copy");
    assert!(
        ids_of(&copy).is_disjoint(&ids_of(&original)),
        "no ID is shared"
    );
    assert_eq!(shape(&copy), shape(&original));
    let media = |s: &Sequence| -> BTreeSet<MediaId> { clips(s).iter().map(|c| c.media).collect() };
    assert_eq!(media(&copy), media(&original));
    assert_eq!(out["item_count"], json!(clips(&copy).len()));
    h.ok(
        "sequence.duplicate",
        json!({"sequence": p.sequence, "name": "  For Shorts "}),
    );
    assert!(h.doc().sequences.values().any(|s| s.name == "For Shorts"));
    assert_code(
        h.run(
            "sequence.duplicate",
            json!({"sequence": SequenceId::from_u128(9)}),
        ),
        "not_found",
    );
}

#[test]
fn duplicate_remaps_attachments() {
    let (mut h, p) = H::project();
    let video = h.add_video("/m/v.mp4", 30, false);
    let a = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": video, "at": "f:0",
               "duration": "f:61"}),
    );
    let v2 = h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    );
    let b = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": v2["track"], "media": video, "at": "f:10",
               "duration": "f:20"}),
    );
    let (parent, child): (ItemId, ItemId) = (id(&a["clips"][0]), id(&b["clips"][0]));
    // No command attaches yet: build the attachment in the document.
    let mut doc: Project = h.doc();
    let sequence = doc.sequences.get_mut(&p.sequence).unwrap();
    let track = sequence
        .tracks
        .get_mut(&id::<TrackId>(&v2["track"]))
        .unwrap();
    track
        .items
        .update(&child, |i| i.set_attached_to(Some(parent)));
    h.replace_doc(doc);
    let out = h.ok("sequence.duplicate", json!({"sequence": p.sequence}));
    let copy = seq(&h, id(&out["sequence"]));
    let attached: Vec<_> = clips(&copy)
        .into_iter()
        .filter_map(|c| c.attached_to.map(|p| (c.id, p)))
        .collect();
    assert_eq!(attached.len(), 1);
    let (c, par) = attached[0];
    assert_ne!(c, child);
    assert_ne!(par, parent);
    assert!(copy.find_item(&par).is_some(), "the parent is in the copy");
}

// ----- sequence.rename, sequence.delete ------------------------------------------------------

#[test]
fn rename_trims_and_validates() {
    let (mut h, p) = H::project();
    let out = h.ok(
        "sequence.rename",
        json!({"sequence": p.sequence, "name": "  Main cut "}),
    );
    assert_eq!(out["name"], "Main cut");
    assert_eq!(seq(&h, p.sequence).name, "Main cut");
    assert_invalid(
        h.run(
            "sequence.rename",
            json!({"sequence": p.sequence, "name": ""}),
        ),
        "/name",
    );
}

#[test]
fn delete_keeps_the_last_sequence() {
    let (mut h, p) = busy();
    assert_invalid(
        h.run("sequence.delete", json!({"sequence": p.sequence})),
        "/sequence",
    );
    let out = h.ok("sequence.duplicate", json!({"sequence": p.sequence}));
    let copy: SequenceId = id(&out["sequence"]);
    let del = h.ok("sequence.delete", json!({"sequence": p.sequence}));
    assert_eq!(del["sequence_count"], 1);
    assert!(del["removed_items"].as_u64().unwrap() > 0);
    assert!(h.doc().sequences.contains_key(&copy));
    // The media stays.
    assert!(!h.doc().media.is_empty());
    assert_invalid(
        h.run("sequence.delete", json!({"sequence": copy})),
        "/sequence",
    );
}

#[test]
fn delete_refuses_a_referenced_sequence() {
    let (mut h, p) = H::project();
    let out = h.ok("sequence.create", json!({"name": "Inner"}));
    let inner: SequenceId = id(&out["sequence"]);
    // Compound clips have no command yet (WP-2B.04): the reference is built by hand.
    let mut doc = h.doc();
    let asset = cutweave_model::MediaAsset::sequence_ref(MediaId::from_u128(77), inner);
    doc.media.insert(asset.id, asset);
    h.replace_doc(doc);
    let r = h.run("sequence.delete", json!({"sequence": inner}));
    assert_invalid(r, "/sequence");
    h.ok("sequence.delete", json!({"sequence": p.sequence}));
}

// ----- sequence.list, view.set_active_sequence -----------------------------------------------

#[test]
fn list_returns_sequences_and_on_request_the_presets() {
    let (mut h, _) = H::project();
    h.ok("sequence.create", json!({"name": "Reel", "preset": "9:16"}));
    let out = h.query("sequence.list", json!({})).unwrap();
    assert_eq!(out["sequences"].as_array().unwrap().len(), 2);
    assert!(out["presets"].is_null());
    let out = h.query("sequence.list", json!({"presets": true})).unwrap();
    assert_eq!(
        out["presets"].as_array().unwrap().len(),
        ASPECT_PRESETS.len()
    );
    assert_eq!(out["frame_rates"].as_array().unwrap().len(), 5);
}

// ----- sequence.set_format: reframe ----------------------------------------------------------

/// A 16:9 sequence with a 16:9 video, a 9:16 video, a square image and an offset, shrunk clip.
fn composed() -> (H, P, Vec<ItemId>) {
    let (mut h, p) = H::project();
    let wide = h.add_video("/m/wide.mp4", 30, false);
    let image = h.add_image("/m/i.png");
    let mut ids = Vec::new();
    let mut track = p.video;
    for (media, scale, x, y) in [
        (wide, 1.0, 0.0, 0.0),
        (wide, 0.5, 0.2, -0.15),
        (image, 0.4, -0.25, 0.2),
    ] {
        let out = h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": track, "media": media, "at": "f:0",
                   "duration": "f:30", "link": false}),
        );
        let item: ItemId = id(&out["clips"][0]);
        h.ok(
            "item.set_props",
            json!({"item": item, "props": {"transform": {
                "scale": {"x": scale, "y": scale}, "position": {"x": x, "y": y}}}}),
        );
        ids.push(item);
        let t = h.ok(
            "track.add",
            json!({"sequence": p.sequence, "kind": "video"}),
        );
        track = id(&t["track"]);
    }
    (h, p, ids)
}

fn rect_of(h: &H, sequence: SequenceId, item: ItemId) -> [f64; 4] {
    let doc = h.doc();
    let s = &doc.sequences[&sequence];
    let clip = s.find_item(&item).unwrap().1.as_clip().unwrap().clone();
    let media = &doc.media[&clip.media];
    let a = cutweave_ops::sequence::reframe::picture_aspect(media).unwrap_or(16.0 / 9.0);
    let t = &clip.visual.transform;
    placed_rect(
        a,
        Canvas::new(s.format.width, s.format.height),
        *t.position.value(),
        *t.scale.value(),
        *t.anchor.value(),
    )
}

#[test]
fn fit_from_16_9_to_9_16_keeps_every_frame_visible() {
    let (mut h, p, ids) = composed();
    let before: Vec<[f64; 4]> = ids.iter().map(|i| rect_of(&h, p.sequence, *i)).collect();
    let out = h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "width": 1080, "height": 1920}),
    );
    assert_eq!(out["reframed"], 3);
    let new = Canvas::new(1080, 1920);
    for (item, b) in ids.iter().zip(&before) {
        let r = rect_of(&h, p.sequence, *item);
        let eps = 1e-6;
        assert!(
            r[0] >= -eps && r[1] >= -eps && r[2] <= new.w + eps && r[3] <= new.h + eps,
            "{item}: {r:?} is outside the 9:16 canvas"
        );
        // The composition shrank uniformly by the fit factor.
        let k = 1080.0 / 1920.0;
        assert!((((r[2] - r[0]) / ((b[2] - b[0]) * k)) - 1.0).abs() < 1e-9);
    }
    // The clip at the default transform kept it exactly.
    let first = clips(&seq(&h, p.sequence))
        .into_iter()
        .find(|c| c.id == ids[0])
        .unwrap();
    assert_eq!(*first.visual.transform.scale.value(), Vec2::ONE);
}

#[test]
fn fill_keep_scale_and_none() {
    let (mut h, p, ids) = composed();
    let none = h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "width": 1080, "height": 1920, "reframe": "none"}),
    );
    assert_eq!(none["reframed"], 0);
    // Transforms untouched: the wide clip now overflows nothing (aspect fit), the offset ones
    // sit where the old numbers say.
    let kept = clips(&seq(&h, p.sequence));
    assert_eq!(
        *kept[0].visual.transform.position.value(),
        Vec2::new(0.0, 0.0)
    );

    let (mut h, p, ids2) = composed();
    h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "width": 1080, "height": 1920, "reframe": "fill"}),
    );
    let r = rect_of(&h, p.sequence, ids2[0]);
    assert!(r[0] <= 1e-6 && r[1] <= 1e-6 && r[2] >= 1080.0 - 1e-6 && r[3] >= 1920.0 - 1e-6);

    let (mut h, p, ids3) = composed();
    let before = rect_of(&h, p.sequence, ids3[1]);
    h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "width": 1080, "height": 1920, "reframe": "keep_scale"}),
    );
    let after = rect_of(&h, p.sequence, ids3[1]);
    assert!((((after[2] - after[0]) - (before[2] - before[0])).abs()) < 1e-6);
    let _ = (ids, ids2);
}

#[test]
fn reframe_scales_keyed_values_and_leaves_audio_tracks_alone() {
    let (mut h, p) = H::project();
    let video = h.add_video("/m/v.mp4", 30, true);
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": video, "at": "f:0",
               "duration": "f:61"}),
    );
    let (v, a): (ItemId, ItemId) = (id(&out["clips"][0]), id(&out["clips"][1]));
    h.ok(
        "keyframe.set",
        json!({"item": v, "prop": "transform.position", "keys": [
            {"t": "f:0", "value": {"x": 0.1, "y": 0.2}},
            {"t": "f:30", "value": {"x": -0.1, "y": 0.0}}]}),
    );
    let audio_before = serde_json::to_value(
        clips(&seq(&h, p.sequence))
            .into_iter()
            .find(|c| c.id == a)
            .unwrap(),
    )
    .unwrap();
    h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "width": 1080, "height": 1080, "reframe": "keep_scale"}),
    );
    let c = clips(&seq(&h, p.sequence))
        .into_iter()
        .find(|c| c.id == v)
        .unwrap();
    let keys = c
        .visual
        .transform
        .position
        .keyframes()
        .unwrap()
        .keys()
        .to_vec();
    // keep_scale: x * 1920/1080, y * 1080/1080 (and the key times are unchanged).
    assert!((keys[0].value.x - 0.1 * 1920.0 / 1080.0).abs() < 1e-12);
    assert!((keys[0].value.y - 0.2).abs() < 1e-12);
    assert_eq!(keys[1].t.flicks(), 30 * 23_520_000);
    let audio_after = serde_json::to_value(
        clips(&seq(&h, p.sequence))
            .into_iter()
            .find(|c| c.id == a)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(audio_before, audio_after);
}

#[test]
fn same_size_is_a_no_op_and_background_and_sample_rate_apply() {
    let (mut h, p, _) = composed();
    let before = seq(&h, p.sequence);
    let out = h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "width": 1920, "height": 1080}),
    );
    assert_eq!(
        (out["reframed"].as_u64(), out["retimed"].as_u64()),
        (Some(0), Some(0))
    );
    assert_eq!(seq(&h, p.sequence), before);
    h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "background": "#336699", "sample_rate": 44100}),
    );
    let f = seq(&h, p.sequence).format;
    assert_eq!(f.background.to_string(), "#336699");
    assert_eq!(f.sample_rate.hz(), 44_100);
}

#[test]
fn set_format_validates_and_respects_locks() {
    let (mut h, p, ids) = composed();
    assert_invalid(
        h.run(
            "sequence.set_format",
            json!({"sequence": p.sequence, "width": 0}),
        ),
        "/",
    );
    assert_invalid(
        h.run(
            "sequence.set_format",
            json!({"sequence": p.sequence, "frame_rate": {"num": 1001, "den": 7}}),
        ),
        "/",
    );
    assert_code(
        h.run(
            "sequence.set_format",
            json!({"sequence": SequenceId::from_u128(9), "width": 10}),
        ),
        "not_found",
    );
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    let _ = ids;
    // A locked video track would be reframed or retimed: refused.
    assert_code(
        h.run(
            "sequence.set_format",
            json!({"sequence": p.sequence, "width": 1080, "height": 1920}),
        ),
        "locked",
    );
    assert_code(
        h.run(
            "sequence.set_format",
            json!({"sequence": p.sequence, "frame_rate": {"num": 24, "den": 1}}),
        ),
        "locked",
    );
    // Nothing that touches no item is refused.
    h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "background": "#000001"}),
    );
    h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "width": 1080, "height": 1920, "reframe": "none"}),
    );
}

// ----- sequence.set_format: frame rate -------------------------------------------------------

/// The new grid: every start and length is a whole number of frames.
fn on_grid(s: &Sequence) -> Result<(), String> {
    let rate = s.format.frame_rate;
    for t in s.tracks.values() {
        for i in t.items.iter() {
            for (what, f) in [
                ("start", i.start().flicks()),
                ("duration", i.duration().flicks()),
            ] {
                let time = cutweave_model::Time::from_flicks(f).unwrap();
                let snapped = rate.snap_to_frame(time, Rounding::Nearest).unwrap();
                if snapped != time {
                    return Err(format!("{} {what} {f} is off the {rate} grid", i.id()));
                }
            }
        }
    }
    Ok(())
}

#[test]
fn frame_rate_change_resnaps_and_keeps_sync() {
    let (mut h, p) = busy();
    let out = h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "frame_rate": {"num": 24, "den": 1}}),
    );
    assert!(out["retimed"].as_u64().unwrap() > 0);
    let s = seq(&h, p.sequence);
    on_grid(&s).unwrap();
    check_invariants(&h.doc()).unwrap();
    // The linked pairs are still aligned (validate), and 91 and 203 (30 fps) land on 73 and 162.
    let starts: Vec<i64> = s.tracks[&p.video]
        .items
        .iter()
        .map(|i| i.start().flicks() / 29_400_000)
        .collect();
    assert_eq!(starts, [0, 73, 162]);
    // Same rate again: nothing moves.
    let again = h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "frame_rate": {"num": 24, "den": 1}}),
    );
    assert_eq!(again["retimed"], 0);
}

#[test]
fn a_clip_that_used_all_its_media_never_reads_past_it() {
    let (mut h, p) = H::project();
    let video = h.add_video("/m/v.mp4", 3, true);
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": video, "at": "f:0"}),
    );
    for rate in [24, 25, 60, 50, 24] {
        h.ok(
            "sequence.set_format",
            json!({"sequence": p.sequence, "frame_rate": {"num": rate, "den": 1}}),
        );
        on_grid(&seq(&h, p.sequence)).unwrap();
    }
}

// ----- view.set_active_sequence --------------------------------------------------------------

#[test]
fn ui_state_keeps_the_active_sequence_and_drops_a_deleted_one() {
    use cutweave_ops::view::ViewCtx;
    let (mut h, p) = H::project();
    let out = h.ok("sequence.create", json!({"name": "Reel", "preset": "9:16"}));
    let reel: SequenceId = id(&out["sequence"]);
    let mut ctx = ViewCtx::new(h.doc());
    assert_eq!(ctx.state().active_sequence, None);
    assert!(ctx.set_active_sequence(reel).unwrap());
    assert!(
        !ctx.set_active_sequence(reel).unwrap(),
        "no change, no event"
    );
    assert!(ctx.set_active_sequence(SequenceId::from_u128(9)).is_err());
    // It survives the file round trip, and a deleted sequence falls back to none.
    let json = ctx.to_json();
    let (back, warnings) = ViewCtx::from_json(&json, h.doc());
    assert_eq!(warnings, Vec::<String>::new());
    assert_eq!(back.state().active_sequence, Some(reel));
    h.ok("sequence.delete", json!({"sequence": reel}));
    let (gone, _) = ViewCtx::from_json(&json, h.doc());
    assert_eq!(gone.state().active_sequence, None);
    assert!(ctx.set_doc(&h.doc()));
    assert_eq!(ctx.state().active_sequence, None);
    let _ = p;
}

// ----- properties ----------------------------------------------------------------------------

const BUILD: &[&str] = &[
    "timeline.insert",
    "timeline.insert",
    "item.move",
    "item.delete",
    "clip.split",
    "clip.trim",
    "track.add",
    "track.set_magnetic",
    "item.duplicate",
];

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn generate(h: &H, doc: &Project, names: &[&str], seed: u64) -> (String, Value) {
    let mut bytes = [0_u8; 32];
    let mut s = seed;
    for chunk in bytes.chunks_mut(8) {
        s = splitmix(s);
        chunk.copy_from_slice(&s.to_le_bytes());
    }
    let config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
    arb_command_among(&h.registry, doc, names)
        .new_tree(&mut runner)
        .expect("the strategies never reject")
        .current()
}

/// A project with media and one sequence, edited by up to `seeds.len()` random commands.
fn built(seeds: &[u64]) -> (H, P) {
    let (mut h, p) = H::project();
    let video = h.add_video("/m/v.mp4", 30, true);
    let audio = h.add_audio("/m/a.wav", 20);
    let image = h.add_image("/m/i.png");
    for (media, at, len) in [
        (video, "f:0", "f:61"),
        (audio, "f:70", "f:50"),
        (image, "f:130", "f:17"),
    ] {
        let track = if media == audio { p.audio } else { p.video };
        h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": track, "media": media, "at": at,
                   "duration": len, "link": media == video}),
        );
    }
    for seed in seeds {
        let doc = h.doc();
        let (name, params) = generate(&h, &doc, BUILD, *seed);
        let _ = h.run(&name, params);
    }
    (h, p)
}

fn run_property(config: Config, body: impl Fn(&[u64])) {
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::deterministic_rng(RngAlgorithm::ChaCha));
    if let Err(e) = runner.run(&vec(any::<u64>(), 0..=14), |seeds| {
        body(&seeds);
        Ok(())
    }) {
        panic!("{e}");
    }
}

#[test]
fn frame_rate_changes_keep_overlap_freeness_and_magnetic_gaplessness() {
    let config = Config {
        cases: 1000,
        failure_persistence: None,
        ..Config::default()
    };
    let magnetic = std::cell::Cell::new(0_u32);
    run_property(config, |seeds| {
        let (mut h, p) = built(seeds);
        let rates: [(u32, u32); 6] = [(24, 1), (25, 1), (60, 1), (50, 1), (30, 1), (24, 1)];
        for (i, (num, den)) in rates.iter().enumerate() {
            if i == 0 {
                // 30 to 24 is the acceptance case; the rest chain further changes.
            }
            let before = h.doc();
            let result = h.run(
                "sequence.set_format",
                json!({"sequence": p.sequence, "frame_rate": {"num": num, "den": den}}),
            );
            let after = h.doc();
            let Ok(_) = result else {
                // Only a clip with under one new frame of media left can fail.
                assert_eq!(before, after);
                continue;
            };
            check_invariants(&after).unwrap();
            let s = &after.sequences[&p.sequence];
            on_grid(s).unwrap();
            let old = &before.sequences[&p.sequence];
            for t in s.tracks.values() {
                // No item lost, order kept, magnetic track gap-free from 0.
                let was: Vec<ItemId> = old.tracks[&t.id].items.iter().map(Item::id).collect();
                let now: Vec<ItemId> = t.items.iter().map(Item::id).collect();
                assert_eq!(was, now, "{}: item order changed", t.name);
                if t.magnetic {
                    magnetic.set(magnetic.get() + 1);
                    let mut end = 0;
                    for i in t.items.iter() {
                        assert_eq!(i.start().flicks(), end, "gap on the magnetic track");
                        end = i.end().unwrap().flicks();
                    }
                }
            }
        }
    });
    assert!(
        magnetic.get() > 100,
        "only {} magnetic checks",
        magnetic.get()
    );
}

#[test]
fn editing_a_duplicate_never_changes_the_original() {
    const EDITS: &[&str] = &[
        "timeline.insert",
        "item.move",
        "item.delete",
        "clip.split",
        "clip.trim",
        "clip.ripple_trim",
        "item.set_props",
        "item.duplicate",
        "track.add",
        "track.remove",
        "track.set",
        "timeline.add_marker",
        "sequence.rename",
        "sequence.set_format",
    ];
    let config = Config {
        cases: 300,
        failure_persistence: None,
        ..Config::default()
    };
    run_property(config, |seeds| {
        let (mut h, p) = built(seeds);
        let out = h.ok("sequence.duplicate", json!({"sequence": p.sequence}));
        let copy: SequenceId = id(&out["sequence"]);
        let original = h.doc().sequences[&p.sequence].clone();
        for seed in seeds.iter().rev().take(10) {
            // Commands are drawn from a document that holds only the copy (and the media), so
            // they can name nothing of the original.
            let mut only_copy = h.doc();
            only_copy.sequences.remove(&p.sequence);
            let (name, params) = generate(&h, &only_copy, EDITS, *seed);
            let _ = h.run(&name, params);
            assert_eq!(
                h.doc().sequences[&p.sequence],
                original,
                "{name} on the copy changed the original"
            );
        }
        assert!(h.doc().sequences.contains_key(&copy));
    });
}

#[test]
fn preset_table_snapshot() {
    let table: Vec<Value> = ASPECT_PRESETS
        .iter()
        .map(|p| json!([p.id, p.aspect_w, p.aspect_h, p.width, p.height, p.use_for]))
        .collect();
    insta::assert_json_snapshot!(
        "aspect_presets",
        json!({
            "aspect": table,
            "frame_rates": cutweave_model::FRAME_RATE_PRESETS.iter().map(ToString::to_string).collect::<Vec<_>>(),
        })
    );
    // fit_size is what the picture gets on each preset canvas.
    let (w, h) = fit_size(16.0 / 9.0, Canvas::new(1080, 1920));
    assert!((w - 1080.0).abs() < 1e-9 && (h - 607.5).abs() < 1e-9);
}
