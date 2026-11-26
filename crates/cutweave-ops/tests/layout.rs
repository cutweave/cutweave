// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2C.08 acceptance: `item.apply_layout` and `item.layouts`. Every layout puts each clip's
//! visible rectangle on its cell (within 0.5 sequence px) on 16:9, 9:16 and 1:1 canvases with
//! 16:9 and 9:16 sources, for `crop` and `fit`; `H::run` asserts that undo restores everything.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::float_cmp,
    clippy::needless_range_loop,
    clippy::many_single_char_names,
    clippy::cast_possible_truncation
)]

mod common;

use common::{H, P, assert_invalid, clip_of, id, item_id};
use cutweave_model::{ItemId, MediaId};
use cutweave_ops::media::FakeProbe;
use cutweave_ops::sequence::reframe::{Canvas, picture_aspect, placed_rect};
use serde_json::json;

const CANVASES: [(u32, u32); 3] = [(1920, 1080), (1080, 1920), (1080, 1080)];

fn media(h: &mut H, path: &str, portrait: bool) -> MediaId {
    let mut p = FakeProbe::video(path, 30, false);
    p.probe.rotation = if portrait { 90 } else { 0 };
    let out = h.ok(
        "media.register",
        json!({"path": path, "probe": p.probe, "content_hash": p.content_hash}),
    );
    id(&out["media_id"])
}

/// A project on `canvas` with `n` clips, one per video track (the first on the top track).
fn setup(canvas: (u32, u32), sources: &[bool]) -> (H, P, Vec<ItemId>) {
    let (mut h, p) = H::project();
    h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "width": canvas.0, "height": canvas.1}),
    );
    let mut tracks = vec![p.video];
    for _ in 1..sources.len() {
        let t = h.ok(
            "track.add",
            json!({"sequence": p.sequence, "kind": "video"}),
        );
        tracks.push(id(&t["track"]));
    }
    let mut items = Vec::new();
    for (i, portrait) in sources.iter().enumerate() {
        let m = media(&mut h, &format!("/m/{i}.mp4"), *portrait);
        let out = h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": tracks[i], "media": m, "at": "f:0",
                   "duration": "f:60", "link": false}),
        );
        items.push(item_id(&out["clips"][0]));
    }
    // Tracks added later sit above: the last one is the top track.
    items.reverse();
    (h, p, items)
}

/// The visible rectangle of a clip: its picture's rectangle minus the cropped edges.
fn visible(h: &H, p: P, item: ItemId) -> [f64; 4] {
    let doc = h.doc();
    let s = &doc.sequences[&p.sequence];
    let clip = clip_of(h, item);
    let a = picture_aspect(&doc.media[&clip.media]).unwrap();
    let t = &clip.visual.transform;
    let r = placed_rect(
        a,
        Canvas::new(s.format.width, s.format.height),
        *t.position.value(),
        *t.scale.value(),
        *t.anchor.value(),
    );
    let (w, hh) = (r[2] - r[0], r[3] - r[1]);
    let c = &clip.visual.crop;
    [
        r[0] + c.left.value() * w,
        r[1] + c.top.value() * hh,
        r[2] - c.right.value() * w,
        r[3] - c.bottom.value() * hh,
    ]
}

fn layouts(h: &H) -> Vec<(String, usize)> {
    h.query("item.layouts", json!({})).unwrap()["layouts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["id"].as_str().unwrap().to_string(),
                l["count"].as_u64().unwrap() as usize,
            )
        })
        .collect()
}

#[test]
fn the_table_lists_layouts_for_two_to_four_clips() {
    let (h, _, _) = setup((1920, 1080), &[false, false]);
    let all = layouts(&h);
    let ids: Vec<&str> = all.iter().map(|(i, _)| i.as_str()).collect();
    for want in [
        "side_by_side",
        "top_bottom",
        "pip_top_right",
        "columns_3",
        "one_large_two_small",
        "grid_2x2",
    ] {
        assert!(ids.contains(&want), "{want} missing");
    }
    assert!(all.iter().all(|(_, n)| (2..=4).contains(n)));
    let two = h.query("item.layouts", json!({"count": 4})).unwrap();
    assert_eq!(two["layouts"].as_array().unwrap().len(), 1);
}

#[test]
fn every_layout_puts_every_clip_on_its_cell_for_crop_and_fit() {
    for canvas in CANVASES {
        for portrait in [false, true] {
            for fill in ["crop", "fit"] {
                let probe = {
                    let (h, _, _) = setup(canvas, &[false, false]);
                    layouts(&h)
                };
                for (layout, count) in probe {
                    let (mut h, p, items) = setup(canvas, &vec![portrait; count]);
                    let out = h.ok(
                        "item.apply_layout",
                        json!({"items": items, "layout": layout, "fill": fill, "gap": 0}),
                    );
                    let cells = out["cells"].as_array().unwrap();
                    assert_eq!(cells.len(), count);
                    for (i, c) in cells.iter().enumerate() {
                        // Default order: the top track first.
                        assert_eq!(item_id(&c["item"]), items[i]);
                        let cell = [
                            c["cell"]["x"].as_f64().unwrap(),
                            c["cell"]["y"].as_f64().unwrap(),
                            c["cell"]["x"].as_f64().unwrap() + c["cell"]["w"].as_f64().unwrap(),
                            c["cell"]["y"].as_f64().unwrap() + c["cell"]["h"].as_f64().unwrap(),
                        ];
                        let v = visible(&h, p, items[i]);
                        let ctx = format!("{layout} {canvas:?} portrait={portrait} {fill} #{i}");
                        if fill == "crop" {
                            for k in 0..4 {
                                assert!((v[k] - cell[k]).abs() < 0.5, "{ctx}: {v:?} vs {cell:?}");
                            }
                        } else {
                            // Whole picture inside the cell, touching it on one axis.
                            let (vw, vh) = (v[2] - v[0], v[3] - v[1]);
                            let (cw, ch) = (cell[2] - cell[0], cell[3] - cell[1]);
                            assert!(vw <= cw + 0.5 && vh <= ch + 0.5, "{ctx}: too big");
                            assert!(
                                (vw - cw).abs() < 0.5 || (vh - ch).abs() < 0.5,
                                "{ctx}: not touching"
                            );
                            let (mx, my) = (
                                f64::midpoint(v[0], v[2]) - f64::midpoint(cell[0], cell[2]),
                                f64::midpoint(v[1], v[3]) - f64::midpoint(cell[1], cell[3]),
                            );
                            assert!(mx.abs() < 0.5 && my.abs() < 0.5, "{ctx}: not centred");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_gap_separates_the_cells() {
    let (mut h, p, items) = setup((1920, 1080), &[false, false]);
    h.ok(
        "item.apply_layout",
        json!({"items": items, "layout": "side_by_side", "gap": 20}),
    );
    let (a, b) = (visible(&h, p, items[0]), visible(&h, p, items[1]));
    assert!((a[2] - 950.0).abs() < 0.5 && (b[0] - 970.0).abs() < 0.5);
}

#[test]
fn order_decides_the_cells_and_swap_is_a_reversed_order() {
    let (mut h, p, items) = setup((1920, 1080), &[false, false]);
    h.ok(
        "item.apply_layout",
        json!({"items": items, "layout": "side_by_side", "order": [items[1], items[0]]}),
    );
    assert!((visible(&h, p, items[1])[2] - 960.0).abs() < 0.5);
    assert!((visible(&h, p, items[0])[0] - 960.0).abs() < 0.5);
    assert_invalid(
        h.run(
            "item.apply_layout",
            json!({"items": items, "layout": "side_by_side", "order": [items[0], items[0]]}),
        ),
        "/order/1",
    );
}

#[test]
fn picture_in_picture_puts_the_top_clip_small_inside_the_safe_margin() {
    let (mut h, p, items) = setup((1920, 1080), &[false, false]);
    h.ok(
        "item.apply_layout",
        json!({"items": items, "layout": "pip_bottom_right"}),
    );
    let small = visible(&h, p, items[0]);
    assert!((small[2] - small[0] - 0.3 * 1920.0).abs() < 0.5);
    assert!((small[2] - 0.95 * 1920.0).abs() < 0.5 && (small[3] - 0.95 * 1080.0).abs() < 0.5);
    let big = visible(&h, p, items[1]);
    assert!(big[0].abs() < 0.5 && (big[2] - 1920.0).abs() < 0.5);
}

#[test]
fn the_wrong_number_of_clips_is_invalid_and_names_the_count() {
    let (mut h, _, items) = setup((1920, 1080), &[false, false, false]);
    let r = h.run(
        "item.apply_layout",
        json!({"items": items, "layout": "side_by_side"}),
    );
    assert_invalid(r.clone(), "/items");
    assert!(r.unwrap_err().to_string().contains("takes 2"));
    assert_invalid(
        h.run(
            "item.apply_layout",
            json!({"items": &items[..1], "layout": "grid_2x2"}),
        ),
        "/items",
    );
    assert_invalid(
        h.run(
            "item.apply_layout",
            json!({"items": &items[..2], "layout": "nope"}),
        ),
        "/layout",
    );
    assert_invalid(
        h.run(
            "item.apply_layout",
            json!({"items": &items[..2], "layout": "side_by_side", "gap": 5000}),
        ),
        "/gap",
    );
}

#[test]
fn an_animated_scale_gets_a_key_not_a_static_value() {
    let (mut h, p, items) = setup((1920, 1080), &[false, false]);
    h.ok(
        "keyframe.set",
        json!({"item": items[0], "prop": "transform.scale",
               "keys": [{"t": "f:0", "value": {"x": 1.0, "y": 1.0}},
                        {"t": "f:30", "value": {"x": 2.0, "y": 2.0}}]}),
    );
    // Without a playhead the animated property cannot be written.
    assert_invalid(
        h.run(
            "item.apply_layout",
            json!({"items": items, "layout": "side_by_side"}),
        ),
        "/at",
    );
    let out = h.ok(
        "item.apply_layout",
        json!({"items": items, "layout": "side_by_side", "at": "f:15"}),
    );
    let keyed = out["keyed"].as_array().unwrap();
    assert!(keyed.iter().any(|k| k["prop"] == "transform.scale"));
    let clip = serde_json::to_value(clip_of(&h, items[0])).unwrap();
    let scale = &clip["visual"]["transform"]["scale"];
    assert_eq!(scale["keyframes"].as_array().unwrap().len(), 3);
    let _ = p;
}

#[test]
fn audio_clips_in_the_selection_are_ignored_and_a_repeat_changes_nothing() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, true);
    let a = common::insert_av(&mut h, p, m, "f:0");
    let t = h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    );
    let m2 = h.add_video("/m/w.mp4", 10, true);
    let b = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": t["track"].clone(), "media": m2, "at": "f:0"}),
    );
    let items = vec![a.0, a.1, item_id(&b["clips"][0])];
    h.ok(
        "item.apply_layout",
        json!({"items": items, "layout": "top_bottom"}),
    );
    let before = h.doc();
    let out = h.ok(
        "item.apply_layout",
        json!({"items": items, "layout": "top_bottom"}),
    );
    assert_eq!(out["changed"].as_array().unwrap().len(), 0);
    assert_eq!(before, h.doc());
}
