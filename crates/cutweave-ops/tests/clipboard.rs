// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.06 acceptance: the clipboard (`cargo test -p cutweave-ops --test clipboard`).
//!
//! - unit tests of `timeline.copy`, `timeline.paste` and `clip.paste_attributes`;
//! - the property test `paste(copy(x)) ≅ x`: copy a random subset of a random timeline (linked
//!   video and audio, music, attached overlays), paste it into an empty copy of the sequence,
//!   and the pasted items equal the copied ones except IDs;
//! - 200 items paste in under 20 ms; 1,000 pastes never collide IDs.
//!
//! `PROPTEST_CASES` sets the number of timelines (default 256; the nightly run uses 10,000).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::many_single_char_names
)]

mod common;

use std::{collections::BTreeSet, time::Instant};

use common::{H, P, assert_code, assert_invalid, clip_of, id, insert_av, item_id, secs};
use cutweave_commands::{Access, Command, Transaction};
use cutweave_model::{Clip, ItemId, Project, SeededGen, SequenceId, TrackId};
use cutweave_ops::timeline::clipboard::TimelinePaste;
use proptest::{collection::vec, prelude::*};
use serde_json::{Value, json};

fn add_track(h: &mut H, p: P, kind: &str) -> TrackId {
    id(&h.ok("track.add", json!({"sequence": p.sequence, "kind": kind}))["track"])
}

/// A still image of `len` seconds at `at` seconds on `track`.
fn still(
    h: &mut H,
    p: P,
    media: cutweave_model::MediaId,
    track: TrackId,
    at: i64,
    len: i64,
) -> ItemId {
    item_id(
        &h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": track, "media": media, "at": format!("{at}s"),
                   "duration": format!("{len}s"), "link": false}),
        )["clips"][0],
    )
}

fn copy(h: &H, items: &[ItemId]) -> Value {
    h.query("timeline.copy", json!({"items": items})).unwrap()["clipboard"].clone()
}

/// Every clip of the sequence with its display row, by row then start then ID.
fn rows(doc: &Project, sequence: SequenceId) -> Vec<(usize, Clip)> {
    let seq = &doc.sequences[&sequence];
    let mut out = Vec::new();
    for (row, t) in seq.tracks_in_order().enumerate() {
        for i in t.items.iter() {
            out.push((row, i.as_clip().unwrap().clone()));
        }
    }
    out
}

/// A clip with everything that names other entities blanked: what "equal except IDs" compares.
fn shape(c: &Clip) -> Value {
    let mut c = c.clone();
    c.id = ItemId::from_u128(0);
    c.linked_group = None;
    c.attached_to = None;
    c.markers = cutweave_model::Markers::new();
    serde_json::to_value(&c).unwrap()
}

// ----- unit tests ----------------------------------------------------------------------------

#[test]
fn copy_returns_relative_layout_with_partners() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, true);
    let (v, a) = insert_av(&mut h, p, media, "4s");
    let c = copy(&h, &[v]);
    assert_eq!(c["version"], 1);
    assert_eq!(
        c["items"].as_array().unwrap().len(),
        2,
        "the partner came along"
    );
    assert_eq!(c["tracks"].as_array().unwrap().len(), 2);
    assert_eq!(c["tracks"][0]["row"], 0);
    assert_eq!(c["tracks"][1]["row"], 1);
    assert_eq!(c["items"][0]["item"]["start"], json!(secs(0)));
    assert_eq!(c["span"], json!(secs(10).flicks()));
    assert_eq!(item_id(&c["items"][0]["item"]["id"]), v);
    assert_eq!(item_id(&c["items"][1]["item"]["id"]), a);
    // Nothing changed.
    let before = h.commits();
    copy(&h, &[v]);
    assert_eq!(h.commits(), before);
}

#[test]
fn copy_rejects_empty_unknown_and_mixed_sequences() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, false);
    let v = still(&mut h, p, media, p.video, 0, 2);
    let other = h.ok("sequence.create", json!({"name": "B"}));
    let w = item_id(
        &h.ok(
            "timeline.insert",
            json!({"sequence": other["sequence"], "track": other["video_track"], "media": media,
                   "at": "0s", "duration": "2s", "link": false}),
        )["clips"][0],
    );
    assert_invalid(h.query("timeline.copy", json!({"items": []})), "/items");
    assert_invalid(h.query("timeline.copy", json!({"items": [v, w]})), "/items");
    assert_code(
        h.query(
            "timeline.copy",
            json!({"items": ["itm_00000000000000000000000000000001"]}),
        ),
        "not_found",
    );
}

#[test]
fn paste_in_the_same_sequence_makes_new_items_and_a_new_group() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, true);
    let (v, a) = insert_av(&mut h, p, media, "0s");
    let payload = copy(&h, &[v]);
    let out = h.ok("timeline.paste", json!({"payload": payload, "at": "20s"}));
    let copies = out["copies"].as_array().unwrap();
    assert_eq!(copies.len(), 2);
    let (v2, a2) = (item_id(&copies[0]["copy"]), item_id(&copies[1]["copy"]));
    assert_ne!(v2, v);
    assert_eq!(item_id(&copies[0]["source"]), v);
    let (cv, ca) = (clip_of(&h, v2), clip_of(&h, a2));
    assert_eq!(cv.start, secs(20));
    assert_eq!(ca.start, secs(20));
    assert_eq!(cv.media, clip_of(&h, v).media);
    assert!(cv.linked_group.is_some());
    assert_eq!(cv.linked_group, ca.linked_group);
    assert_ne!(cv.linked_group, clip_of(&h, v).linked_group, "a new group");
    assert_eq!(clip_of(&h, a).start, secs(0), "the originals did not move");
    assert_eq!(h.doc().find_item(&v2).unwrap().track.id, p.video);
    assert_eq!(h.doc().find_item(&a2).unwrap().track.id, p.audio);
    assert_eq!(
        shape(&cv),
        shape(&{
            let mut c = clip_of(&h, v);
            c.start = secs(20);
            c
        })
    );
}

#[test]
fn paste_on_another_track_keeps_rows_and_checks_kinds() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, true);
    let (v, _) = insert_av(&mut h, p, media, "0s");
    let payload = copy(&h, &[v]);
    // V2 above V1, A1 stays below V1: pasting on V2 puts the audio on V1 (not an audio track).
    let v2 = add_track(&mut h, p, "video");
    assert_invalid(
        h.run(
            "timeline.paste",
            json!({"payload": payload, "at": "30s", "track": v2}),
        ),
        "/track",
    );
    // A single-track payload goes to any track of its kind.
    let lone = still(&mut h, p, media, v2, 0, 3);
    let only = copy(&h, &[lone]);
    let out = h.ok(
        "timeline.paste",
        json!({"payload": only, "at": "20s", "track": p.video}),
    );
    let copy_id = item_id(&out["copies"][0]["copy"]);
    assert_eq!(h.doc().find_item(&copy_id).unwrap().track.id, p.video);
    assert_invalid(
        h.run(
            "timeline.paste",
            json!({"payload": only, "at": "20s", "track": p.audio}),
        ),
        "/track",
    );
}

#[test]
fn paste_across_sequences_remaps_ids_and_keeps_media() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, true);
    let (v, _) = insert_av(&mut h, p, media, "2s");
    let payload = copy(&h, &[v]);
    let other = h.ok("sequence.create", json!({"name": "B"}));
    let out = h.ok(
        "timeline.paste",
        json!({"payload": payload, "at": "1s", "track": other["video_track"]}),
    );
    let doc = h.doc();
    let seq_b: SequenceId = id(&other["sequence"]);
    let b = rows(&doc, seq_b);
    assert_eq!(b.len(), 2);
    assert!(
        b.iter()
            .all(|(_, c)| c.media == media && c.start == secs(1))
    );
    assert_eq!(out["report"]["created"].as_array().unwrap().len(), 2);
    // The source sequence is untouched.
    assert_eq!(rows(&doc, p.sequence).len(), 2);
}

#[test]
fn paste_without_a_track_goes_back_to_the_source_tracks() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 4, true);
    let (v, _) = insert_av(&mut h, p, media, "0s");
    let payload = copy(&h, &[v]);
    h.ok("timeline.paste", json!({"payload": payload, "at": "10s"}));
    let doc = h.doc();
    let seq = &doc.sequences[&p.sequence];
    assert_eq!(seq.track(&p.video).unwrap().items.len(), 2);
    assert_eq!(seq.track(&p.audio).unwrap().items.len(), 2);
    // A payload whose source tracks are gone needs `track`.
    let other = h.ok("sequence.create", json!({"name": "B"}));
    let w = still(&mut h, p, media, p.video, 50, 1);
    let lone = copy(&h, &[w]);
    h.ok("sequence.delete", json!({"sequence": p.sequence}));
    assert_invalid(
        h.run("timeline.paste", json!({"payload": lone, "at": "0s"})),
        "/track",
    );
    h.ok(
        "timeline.paste",
        json!({"payload": lone, "at": "0s", "track": other["video_track"]}),
    );
}

#[test]
fn overlaps_follow_the_mode() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, false);
    let a = still(&mut h, p, media, p.video, 0, 4);
    let payload = copy(&h, &[a]);
    assert_invalid(
        h.run("timeline.paste", json!({"payload": payload, "at": "2s"})),
        "/at",
    );
    let out = h.ok(
        "timeline.paste",
        json!({"payload": payload, "at": "2s", "mode": "new_track"}),
    );
    assert_eq!(out["created_tracks"].as_array().unwrap().len(), 1);
    h.ok(
        "timeline.paste",
        json!({"payload": payload, "at": "6s", "mode": "overwrite"}),
    );
}

#[test]
fn pasting_over_a_locked_track_fails_with_locked_and_changes_nothing() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, true);
    let (v, _) = insert_av(&mut h, p, media, "0s");
    let payload = copy(&h, &[v]);
    h.ok("track.set", json!({"track": p.audio, "locked": true}));
    let (commits, doc) = (h.commits(), h.doc());
    assert_code(
        h.run("timeline.paste", json!({"payload": payload, "at": "20s"})),
        "locked",
    );
    assert_eq!(h.commits(), commits);
    assert_eq!(h.doc(), doc);
}

#[test]
fn a_copy_on_the_magnetic_track_inserts_as_a_block() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 30, false);
    let a = still(&mut h, p, media, p.video, 0, 3);
    let b = still(&mut h, p, media, p.video, 3, 3);
    let c = still(&mut h, p, media, p.video, 6, 3);
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    let payload = copy(&h, &[a, b]);
    h.ok("timeline.paste", json!({"payload": payload, "at": "3s"}));
    let doc = h.doc();
    let starts: Vec<i64> = doc.sequences[&p.sequence]
        .track(&p.video)
        .unwrap()
        .items
        .iter()
        .map(|i| i.start().flicks() / secs(1).flicks())
        .collect();
    assert_eq!(
        starts,
        vec![0, 3, 6, 9, 12],
        "A A2 B2 B C: gapless, C pushed later"
    );
    assert_eq!(clip_of(&h, c).start, secs(12));
}

#[test]
fn copied_children_stay_attached_to_the_copy_of_their_parent() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 30, false);
    let parent = still(&mut h, p, media, p.video, 0, 10);
    let v2 = add_track(&mut h, p, "video");
    let child = still(&mut h, p, media, v2, 2, 3);
    h.ok("item.attach", json!({"items": [child], "parent": parent}));
    // Both copied: the copy of the child follows the copy of the parent.
    let both = copy(&h, &[parent, child]);
    let out = h.ok("timeline.paste", json!({"payload": both, "at": "20s"}));
    let map = |src: ItemId| -> ItemId {
        out["copies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| item_id(&c["source"]) == src)
            .map(|c| item_id(&c["copy"]))
            .unwrap()
    };
    assert_eq!(clip_of(&h, map(child)).attached_to, Some(map(parent)));
    assert_eq!(clip_of(&h, map(child)).start, secs(22));
    // The child alone, pasted inside its parent: it keeps the original parent.
    let alone = copy(&h, &[child]);
    let out = h.ok("timeline.paste", json!({"payload": alone, "at": "6s"}));
    let kept = item_id(&out["copies"][0]["copy"]);
    assert_eq!(clip_of(&h, kept).attached_to, Some(parent));
    // Outside the parent it is detached.
    let out = h.ok("timeline.paste", json!({"payload": alone, "at": "40s"}));
    let loose = item_id(&out["copies"][0]["copy"]);
    assert_eq!(clip_of(&h, loose).attached_to, None);
    // In another sequence there is no parent.
    let other = h.ok("sequence.create", json!({"name": "B"}));
    let out = h.ok(
        "timeline.paste",
        json!({"payload": alone, "at": "0s", "track": other["video_track"]}),
    );
    assert_eq!(
        clip_of(&h, item_id(&out["copies"][0]["copy"])).attached_to,
        None
    );
}

#[test]
fn markers_get_new_ids() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, false);
    let a = still(&mut h, p, media, p.video, 0, 4);
    let mut doc = h.doc();
    {
        let seq = doc.sequences.get_mut(&p.sequence).unwrap();
        let track = seq.track_mut(&p.video).unwrap();
        track.items.update(&a, |i| {
            let m = cutweave_model::Marker::new(cutweave_model::MarkerId::from_u128(7), secs(1));
            i.markers_mut().insert(m.id, m);
        });
    }
    h.replace_doc(doc);
    let payload = copy(&h, &[a]);
    let out = h.ok("timeline.paste", json!({"payload": payload, "at": "10s"}));
    let copy_id = item_id(&out["copies"][0]["copy"]);
    let c = clip_of(&h, copy_id);
    assert_eq!(c.markers.len(), 1);
    assert_ne!(
        c.markers.keys().next(),
        clip_of(&h, a).markers.keys().next()
    );
}

#[test]
fn a_bad_payload_is_invalid() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, false);
    let a = still(&mut h, p, media, p.video, 0, 4);
    let good = copy(&h, &[a]);
    let mut v = good.clone();
    v["version"] = json!(2);
    assert_invalid(
        h.run("timeline.paste", json!({"payload": v, "at": "9s"})),
        "/payload/version",
    );
    let mut v = good.clone();
    v["items"] = json!([]);
    assert_invalid(
        h.run("timeline.paste", json!({"payload": v, "at": "9s"})),
        "/payload/items",
    );
    let mut v = good.clone();
    v["items"][0]["track"] = json!(5);
    assert_invalid(
        h.run("timeline.paste", json!({"payload": v, "at": "9s"})),
        "/payload/items",
    );
    let mut v = good;
    let dup = v["items"][0].clone();
    v["items"].as_array_mut().unwrap().push(dup);
    assert_invalid(
        h.run("timeline.paste", json!({"payload": v, "at": "9s"})),
        "/payload/items",
    );
    // A media the project does not have.
    let mut v = copy(&h, &[a]);
    v["items"][0]["item"]["media"] = json!("med_00000000000000000000000000000001");
    assert_code(
        h.run("timeline.paste", json!({"payload": v, "at": "9s"})),
        "not_found",
    );
}

#[test]
fn paste_attributes_copies_groups_with_their_keys() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, true);
    let (src, _) = insert_av(&mut h, p, media, "0s");
    let dst = still(&mut h, p, media, p.video, 20, 4);
    h.ok(
        "keyframe.set",
        json!({"item": src, "prop": "transform.scale", "keys": [
            {"t": "0s", "value": [1.0, 1.0]}, {"t": "2s", "value": [2.0, 2.0]}]}),
    );
    h.ok(
        "keyframe.set",
        json!({"item": src, "prop": "transform.rotation", "keys": [{"t": "0s", "value": 15.0}]}),
    );
    h.ok(
        "item.set_props",
        json!({"item": src, "props": {"name": "Source"}}),
    );
    let before = clip_of(&h, dst);
    let out = h.ok(
        "clip.paste_attributes",
        json!({"from": src, "to": [dst], "which": ["transform"]}),
    );
    assert_eq!(out["changed"], json!([dst]));
    let after = clip_of(&h, dst);
    assert_eq!(after.visual.transform, clip_of(&h, src).visual.transform);
    assert!(
        after.visual.transform.scale.is_animated(),
        "keys came along"
    );
    assert_eq!(after.visual.opacity, before.visual.opacity);
    assert_eq!(after.name, before.name);
    assert_eq!(after.start, before.start);
    // Pasting again changes nothing.
    let out = h.ok(
        "clip.paste_attributes",
        json!({"from": src, "to": [dst], "which": ["transform"]}),
    );
    assert_eq!(out["changed"], json!([]));
}

#[test]
fn paste_attributes_checks_its_params_and_locks() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, true);
    let (src, aud) = insert_av(&mut h, p, media, "0s");
    let dst = still(&mut h, p, media, p.video, 20, 4);
    h.ok("audio.set_volume", json!({"item": aud, "db": -6.0}));
    assert_invalid(
        h.run(
            "clip.paste_attributes",
            json!({"from": src, "to": [dst], "which": []}),
        ),
        "/which",
    );
    assert_invalid(
        h.run(
            "clip.paste_attributes",
            json!({"from": src, "to": [src], "which": ["crop"]}),
        ),
        "/to",
    );
    assert_invalid(
        h.run(
            "clip.paste_attributes",
            json!({"from": src, "to": [], "which": ["crop"]}),
        ),
        "/items",
    );
    h.ok("track.set", json!({"track": p.video, "locked": true}));
    let (commits, doc) = (h.commits(), h.doc());
    assert_code(
        h.run(
            "clip.paste_attributes",
            json!({"from": aud, "to": [dst], "which": ["audio"]}),
        ),
        "locked",
    );
    assert_eq!((h.commits(), h.doc()), (commits, doc));
}

#[test]
fn paste_attributes_audio_copies_gain_pan_and_fades() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, true);
    let (_, a1) = insert_av(&mut h, p, media, "0s");
    let (_, a2) = insert_av(&mut h, p, media, "10s");
    h.ok("audio.set_volume", json!({"item": a1, "db": -6.0}));
    h.ok(
        "audio.set",
        json!({"item": a1, "set": {"kind": "pan", "pan": 0.5}}),
    );
    h.ok(
        "clip.paste_attributes",
        json!({"from": a1, "to": [a2], "which": ["audio"]}),
    );
    assert_eq!(clip_of(&h, a2).audio, clip_of(&h, a1).audio);
}

// ----- property: paste(copy(x)) ≅ x ----------------------------------------------------------

#[derive(Clone, Debug)]
struct Case {
    /// Main-track clips (seconds), each with its sound.
    clips: Vec<i64>,
    /// Overlays on V2: `(parent index, offset, length)`, and whether to attach them.
    overlays: Vec<(usize, i64, i64)>,
    attach: bool,
    music: bool,
    /// Which items to copy (one flag per item, in document order).
    pick: Vec<bool>,
}

fn arb_case() -> impl Strategy<Value = Case> {
    (
        vec(2..9_i64, 1..=4),
        vec((0..4_usize, 0..2_i64, 1..3_i64), 0..=3),
        any::<bool>(),
        any::<bool>(),
        vec(any::<bool>(), 12),
    )
        .prop_map(|(clips, overlays, attach, music, pick)| Case {
            clips,
            overlays,
            attach,
            music,
            pick,
        })
}

fn build(case: &Case) -> (H, P, TrackId) {
    let (mut h, p) = H::project();
    let video = h.add_video("/m/v.mp4", 120, true);
    let mute = h.add_video("/m/b.mp4", 60, false);
    let music = h.add_audio("/m/music.wav", 120);
    let mut at = 0;
    let mut mains = Vec::new();
    for len in &case.clips {
        let out = h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": video,
                   "at": format!("{at}s"), "duration": format!("{len}s")}),
        );
        mains.push((item_id(&out["clips"][0]), at, *len));
        at += len;
    }
    let v2 = add_track(&mut h, p, "video");
    for (parent, offset, len) in &case.overlays {
        let (pid, pstart, plen) = mains[parent % mains.len()];
        let t = pstart + offset % plen;
        if let Ok(out) = h.run(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": v2, "media": mute,
                   "at": format!("{t}s"), "duration": format!("{len}s")}),
        ) && case.attach
        {
            let child = item_id(&out["clips"][0]);
            let _ = h.run("item.attach", json!({"items": [child], "parent": pid}));
        }
    }
    if case.music {
        let a2 = add_track(&mut h, p, "audio");
        h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": a2, "media": music, "at": "1s",
                   "duration": format!("{}s", at + 3)}),
        );
    }
    (h, p, v2)
}

/// Copies the picked items into an empty twin of the sequence and compares.
fn roundtrip(case: &Case) {
    let (mut h, p, _) = build(case);
    let doc = h.doc();
    let all: Vec<ItemId> = doc.sequences[&p.sequence]
        .tracks_in_order()
        .flat_map(|t| {
            t.items
                .iter()
                .map(cutweave_model::Item::id)
                .collect::<Vec<_>>()
        })
        .collect();
    let mut chosen: Vec<ItemId> = all
        .iter()
        .zip(case.pick.iter().cycle())
        .filter(|(_, keep)| **keep)
        .map(|(i, _)| *i)
        .collect();
    if chosen.is_empty() {
        chosen.push(all[0]);
    }
    let payload = copy(&h, &chosen);
    let copied: Vec<ItemId> = payload["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| item_id(&c["item"]["id"]))
        .collect();

    // An empty twin: duplicate the sequence, delete its items.
    let twin = h.ok("sequence.duplicate", json!({"sequence": p.sequence}));
    let twin_id: SequenceId = id(&twin["sequence"]);
    let items: Vec<ItemId> = rows(&h.doc(), twin_id).iter().map(|(_, c)| c.id).collect();
    h.ok("item.delete", json!({"items": items}));
    let doc = h.doc();
    let order: Vec<TrackId> = doc.sequences[&twin_id]
        .tracks_in_order()
        .map(|t| t.id)
        .collect();
    let src_order: Vec<TrackId> = doc.sequences[&p.sequence]
        .tracks_in_order()
        .map(|t| t.id)
        .collect();
    let top_row = src_order
        .iter()
        .position(|t| *t == id::<TrackId>(&payload["tracks"][0]["source"]))
        .unwrap();
    let out = h.ok(
        "timeline.paste",
        json!({"payload": payload, "at": "0s", "track": order[top_row]}),
    );
    assert_eq!(out["copies"].as_array().unwrap().len(), copied.len());

    // Expected: the copied clips with starts rebased to 0, parents only if copied too.
    let doc = h.doc();
    let origin = copied
        .iter()
        .map(|i| doc.find_item(i).unwrap().item.start())
        .min()
        .unwrap();
    let mut expected: Vec<(usize, Value, i64)> = copied
        .iter()
        .map(|i| {
            let at = doc.find_item(i).unwrap();
            let row = src_order.iter().position(|t| *t == at.track.id).unwrap();
            let mut c = at.item.as_clip().unwrap().clone();
            c.start =
                cutweave_model::Time::from_flicks(c.start.flicks() - origin.flicks()).unwrap();
            (row, shape(&c), c.start.flicks())
        })
        .collect();
    let mut got: Vec<(usize, Value, i64)> = rows(&doc, twin_id)
        .into_iter()
        .map(|(row, c)| (row, shape(&c), c.start.flicks()))
        .collect();
    let key = |x: &(usize, Value, i64)| (x.0, x.2, x.1.to_string());
    expected.sort_by_key(key);
    got.sort_by_key(key);
    assert_eq!(got, expected, "paste(copy(x)) must equal x except IDs");

    // Links and attachments are remapped: groups keep their member counts, children point at
    // the copy of their parent (or nowhere), and no ID of the source is reused.
    let new_ids: BTreeSet<ItemId> = rows(&doc, twin_id).iter().map(|(_, c)| c.id).collect();
    assert!(new_ids.is_disjoint(&copied.iter().copied().collect()));
    for (_, c) in rows(&doc, twin_id) {
        if let Some(parent) = c.attached_to {
            assert!(new_ids.contains(&parent));
        }
    }
    let group_sizes = |clips: Vec<Clip>| {
        let mut sizes: Vec<usize> = clips
            .iter()
            .filter_map(|c| c.linked_group)
            .fold(std::collections::BTreeMap::new(), |mut m, g| {
                *m.entry(g).or_insert(0_usize) += 1;
                m
            })
            .into_values()
            .collect();
        sizes.sort_unstable();
        sizes
    };
    let source_clips: Vec<Clip> = copied
        .iter()
        .map(|i| doc.find_item(i).unwrap().item.as_clip().unwrap().clone())
        .collect();
    let target_clips: Vec<Clip> = rows(&doc, twin_id).into_iter().map(|(_, c)| c).collect();
    assert_eq!(group_sizes(source_clips), group_sizes(target_clips));
}

proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn paste_of_copy_is_equal_except_ids(case in arb_case()) {
        roundtrip(&case);
    }
}

// ----- budgets -------------------------------------------------------------------------------

#[test]
fn pasting_200_items_takes_under_20_ms() {
    let (mut h, p) = H::project();
    let media = h.add_image("/m/logo.png");
    let ids: Vec<ItemId> = (0..200)
        .map(|n| still(&mut h, p, media, p.video, n * 2, 1))
        .collect();
    let payload = copy(&h, &ids);
    let v2 = add_track(&mut h, p, "video");
    let doc = h.doc();
    // The command as the engine runs it once the payload is parsed (parsing 200 items of JSON
    // is the transport's cost, not the paste's). Best of 15: a loaded machine misses once.
    let cmd = TimelinePaste {
        payload: serde_json::from_value(payload).unwrap(),
        at: "0s".parse().unwrap(),
        track: Some(v2),
        mode: None,
    };
    let mut best = std::time::Duration::MAX;
    for _ in 0..15 {
        let mut txn = Transaction::begin(doc.clone());
        let mut seeds = SeededGen::new(7);
        let t = Instant::now();
        let mut tx = txn.tx(&mut seeds, Access::STANDARD);
        Command::apply(&cmd, &mut tx).unwrap();
        best = best.min(t.elapsed());
    }
    assert!(best.as_millis() < 20, "200 items took {best:?}");
}

#[test]
fn a_thousand_pastes_never_collide_ids() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, true);
    let (v, _) = insert_av(&mut h, p, media, "0s");
    let payload = copy(&h, &[v]);
    for n in 1..=1000 {
        h.ok(
            "timeline.paste",
            json!({"payload": payload, "at": format!("{}s", n * 10)}),
        );
    }
    let doc = h.doc();
    let seq = &doc.sequences[&p.sequence];
    let mut seen = BTreeSet::new();
    let mut groups = BTreeSet::new();
    for t in seq.tracks_in_order() {
        assert_eq!(t.items.len(), 1001);
        for i in t.items.iter() {
            assert!(seen.insert(i.id()), "{} collides", i.id());
            groups.insert(i.linked_group());
        }
    }
    assert_eq!(groups.len(), 1001, "every paste made its own group");
}

#[test]
fn paste_is_deterministic() {
    let (mut h, p) = H::project();
    let media = h.add_video("/m/v.mp4", 10, true);
    let (v, _) = insert_av(&mut h, p, media, "0s");
    let payload = copy(&h, &[v]);
    let params = json!({"payload": payload, "at": "20s"});
    let run = |doc: &Project| {
        let mut txn = Transaction::begin(doc.clone());
        let mut seeds = SeededGen::new(99);
        let mut tx = txn.tx(&mut seeds, Access::STANDARD);
        h.registry
            .invoke_json("timeline.paste", params.clone(), &mut tx)
            .unwrap()
    };
    let doc = h.doc();
    assert_eq!(run(&doc), run(&doc));
}
