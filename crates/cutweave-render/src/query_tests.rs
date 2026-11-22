// SPDX-License-Identifier: GPL-3.0-or-later
//! `render.frame` / `render.contact_sheet` through the registry, on the CPU reference backend
//! (the GPU backend is covered by `gpu::query_tests`): registration and MCP exposure, the
//! backend scope, parameters, metadata against `evaluate()`, analytic colours, determinism.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::assert_is_empty
)]

use std::sync::Arc;

use cutweave_commands::{
    Access, CmdError, EntryKind, McpExposure, QueryCtx, Registry, Rev, input_schema,
};
use cutweave_model::{ItemId, Sequence, SequenceFormat, SequenceId};
use serde_json::{Value, json};

use crate::evaluate;
use crate::image::Image;
use crate::inspect::{HiddenReason, PixelRect};
use crate::query::{
    ContactSheet, FrameImage, ReferenceBackend, RenderContactSheet, RenderFrame, enter, register,
};
use crate::sheet::{BACKGROUND, GAP};
use crate::testkit::{
    BLUE, GREEN, RED, ThreeClips, background_srgb, decode_jpeg, decode_png, delta_e, expected_srgb,
    mean_rgb, sec, three_clips, unbase64,
};

const W: u32 = 192;
const H: u32 = 108;

fn registry() -> Registry {
    let mut r = Registry::new();
    register(&mut r).unwrap();
    r
}

fn backend(f: &ThreeClips) -> crate::query::SharedBackend {
    ReferenceBackend::new(Arc::clone(&f.source) as _).shared()
}

fn frame(f: &ThreeClips, params: Value) -> Result<FrameImage, CmdError> {
    let _scope = enter(backend(f));
    let ctx = QueryCtx::new(&f.project, Access::STANDARD).with_rev(Rev(7));
    let out = registry().query_json("render.frame", params, &ctx)?;
    Ok(serde_json::from_value(out).unwrap())
}

fn sheet(f: &ThreeClips, params: Value) -> Result<ContactSheet, CmdError> {
    let _scope = enter(backend(f));
    let ctx = QueryCtx::new(&f.project, Access::STANDARD);
    let out = registry().query_json("render.contact_sheet", params, &ctx)?;
    Ok(serde_json::from_value(out).unwrap())
}

fn png_of(frame: &FrameImage) -> Image {
    assert_eq!(frame.mime, "image/png");
    let bytes = unbase64(&frame.data);
    assert_eq!(bytes.len(), frame.bytes as usize);
    decode_png(&bytes)
}

fn invalid_path(e: &CmdError) -> &str {
    match e {
        CmdError::Invalid { path, .. } => path,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn registered_as_read_only_idempotent_core_mcp_tools() {
    let r = registry();
    for (name, tool) in [
        ("render.frame", "render_frame"),
        ("render.contact_sheet", "render_contact_sheet"),
    ] {
        let e = r.get(name).unwrap();
        assert_eq!(e.kind(), EntryKind::Query);
        assert_eq!(e.meta.mcp, McpExposure::Core);
        assert_eq!(e.meta.mcp_tool_name(), tool);
        assert!(e.meta.annotations.read_only && e.meta.annotations.idempotent);
        assert!(!e.meta.annotations.destructive && !e.meta.annotations.open_world);
        assert!(!e.meta.is_journaled());
        assert_eq!(r.get_by_tool_name(tool).unwrap().name(), name);
        // The S5 image convention: `mime` + base64 `data` (moved into an image content block).
        let props = &e.meta.output_schema["properties"];
        assert_eq!(props["mime"]["type"], "string");
        assert_eq!(props["data"]["type"], "string");
    }
    let input = input_schema::<RenderFrame>();
    assert_eq!(input["required"], json!(["t"]));
    assert_eq!(input["additionalProperties"], json!(false));
    let sheet_input = input_schema::<RenderContactSheet>();
    assert!(sheet_input.get("required").is_none_or(|v| v == &json!([])));
}

#[test]
fn queries_render_only_inside_a_backend_scope() {
    let f = three_clips(W, H);
    let r = registry();
    let ctx = QueryCtx::new(&f.project, Access::STANDARD);
    let params = json!({ "t": "1s" });
    let err = r
        .query_json("render.frame", params.clone(), &ctx)
        .unwrap_err();
    assert!(matches!(err, CmdError::Unsupported { .. }), "{err:?}");
    {
        let _outer = enter(backend(&f));
        assert!(r.query_json("render.frame", params.clone(), &ctx).is_ok());
        {
            let _inner = enter(backend(&f));
            assert!(r.query_json("render.frame", params.clone(), &ctx).is_ok());
        }
        assert!(r.query_json("render.frame", params.clone(), &ctx).is_ok());
    }
    assert!(r.query_json("render.frame", params, &ctx).is_err());
    // Another thread has its own (empty) scope.
    let _scope = enter(backend(&f));
    let project = f.project.clone();
    std::thread::spawn(move || {
        let ctx = QueryCtx::new(&project, Access::STANDARD);
        assert!(
            registry()
                .query_json("render.frame", json!({ "t": 0 }), &ctx)
                .is_err()
        );
    })
    .join()
    .unwrap();
}

#[test]
fn metadata_matches_evaluate_at_every_time() {
    let f = three_clips(W, H);
    for (t, top_first) in [
        (1.0, vec![f.a]),
        (3.0, vec![f.c, f.a]),
        (5.5, vec![f.c, f.b]),
        (7.0, vec![f.b]),
        (9.0, vec![]),
    ] {
        let out = frame(&f, json!({ "t": format!("{t}s"), "format": "png" })).unwrap();
        let scene = evaluate(&f.project, f.seq, sec(t)).unwrap();
        let evaluated: Vec<ItemId> = scene.layers.iter().rev().map(|l| l.item).collect();
        let listed: Vec<ItemId> = out.items_visible.iter().map(|v| v.item).collect();
        assert_eq!(listed, evaluated, "at {t}s");
        assert_eq!(listed, top_first, "at {t}s");
        assert!(out.items_hidden.is_empty());
        for (v, layer) in out.items_visible.iter().zip(scene.layers.iter().rev()) {
            assert_eq!(v.track, layer.track);
            assert_eq!(v.media, layer.request.media);
            assert_eq!(v.source_time, layer.request.source_time);
            assert!((v.opacity - layer.opacity).abs() < 1e-12);
        }
        assert_eq!(out.time, sec(t));
        assert_eq!(out.sequence, f.seq);
        assert_eq!(out.rev, Some(Rev(7)));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frame_no = (t * 30.0) as u32;
        assert_eq!(out.frame, Some(frame_no));
        assert_eq!(
            out.timecode.as_deref(),
            Some(format!("00:00:{:02}:{:02}", frame_no / 30, frame_no % 30).as_str())
        );
        assert_eq!(
            out.notes.is_empty(),
            !top_first.is_empty(),
            "{:?}",
            out.notes
        );
    }
    // Rectangles: the PiP fills the top-right quadrant, the bottom clip the whole image.
    let out = frame(&f, json!({ "t": "3s", "max_size": 96 })).unwrap();
    assert_eq!((out.width, out.height), (96, 54));
    assert_eq!(
        out.items_visible[0].rect,
        PixelRect {
            x: 48,
            y: 0,
            width: 48,
            height: 27
        }
    );
    assert_eq!(
        out.items_visible[1].rect,
        PixelRect {
            x: 0,
            y: 0,
            width: 96,
            height: 54
        }
    );
    // Negative times render the background with no frame number.
    let out = frame(&f, json!({ "t": "-1s" })).unwrap();
    assert!(out.items_visible.is_empty() && out.frame.is_none() && out.timecode.is_none());
}

#[test]
fn frames_show_the_analytic_clip_colours() {
    let f = three_clips(W, H);
    let (red, green, blue) = (
        expected_srgb(RED),
        expected_srgb(GREEN),
        expected_srgb(BLUE),
    );
    // (time, colour at the top-right quadrant centre, colour at the bottom-left centre)
    for (t, top_right, bottom_left) in [
        (1.0, red, red),
        (3.0, blue, red),
        (5.0, blue, green),
        (9.0, background_srgb(), background_srgb()),
    ] {
        for format in ["png", "jpeg"] {
            let out = frame(&f, json!({ "t": format!("{t}s"), "format": format })).unwrap();
            let bytes = unbase64(&out.data);
            let img = if format == "png" {
                decode_png(&bytes)
            } else {
                decode_jpeg(&bytes)
            };
            assert_eq!((img.width, img.height), (W, H));
            let tr = mean_rgb(&img, W * 3 / 4, H / 4, 16, 16);
            let bl = mean_rgb(&img, W / 4, H * 3 / 4, 16, 16);
            for (got, want, at) in [
                (tr, top_right, "top right"),
                (bl, bottom_left, "bottom left"),
            ] {
                let de = delta_e(got, want);
                assert!(
                    de < 2.0,
                    "{format} {t}s {at}: {got:?} vs {want:?} (ΔE {de:.2})"
                );
            }
        }
    }
}

#[test]
fn same_revision_time_and_quality_give_identical_bytes() {
    let f = three_clips(W, H);
    for params in [
        json!({ "t": "3s" }),
        json!({ "t": "3s", "format": "png" }),
        json!({ "t": "f:150", "quality": 50, "max_size": 64 }),
    ] {
        let a = frame(&f, params.clone()).unwrap();
        let b = frame(&f, params).unwrap();
        assert_eq!(a, b);
    }
    let a = sheet(&f, json!({ "count": 4 })).unwrap();
    let b = sheet(&f, json!({ "count": 4 })).unwrap();
    assert_eq!(a, b);
}

#[test]
fn sizes_formats_and_parameter_errors() {
    let f = three_clips(W, H);
    // Never upscaled; `max_size` bounds the longer side; aliases and timecodes are accepted.
    let out = frame(&f, json!({ "t": 0 })).unwrap();
    assert_eq!(
        (out.width, out.height, out.mime.as_str()),
        (W, H, "image/jpeg")
    );
    let jpeg = unbase64(&out.data);
    assert_eq!(&jpeg[..3], &[0xFF, 0xD8, 0xFF]);
    let out = frame(
        &f,
        json!({ "t": "00:00:01:00", "max_size": 100, "fmt": "png" }),
    )
    .unwrap();
    assert_eq!((out.width, out.height), (100, 56));
    let img = png_of(&out);
    assert_eq!((img.width, img.height), (100, 56));
    // `limit` truncates the item list and says so.
    let out = frame(&f, json!({ "t": "3s", "limit": 1 })).unwrap();
    assert_eq!(out.items_visible.len(), 1);
    assert_eq!(out.items_visible[0].item, f.c);
    assert!(out.notes.iter().any(|n| n.contains("truncated")));

    for (params, path) in [
        (json!({ "t": 0, "max_size": 8 }), "/max_size"),
        (json!({ "t": 0, "max_size": 5000 }), "/max_size"),
        (json!({ "t": 0, "quality": 0 }), "/quality"),
        (json!({ "t": 0, "quality": 101 }), "/quality"),
        (json!({ "t": "00:00:01:45" }), "/t"),
        (json!({ "t": 0, "format": "gif" }), "/format"),
        (json!({ "t": 0, "zoom": 2 }), "/zoom"),
        (json!({}), ""),
    ] {
        let e = frame(&f, params.clone()).unwrap_err();
        assert_eq!(invalid_path(&e), path, "{params}: {e}");
    }
    let unknown = SequenceId::from_u128(999);
    let e = frame(&f, json!({ "t": 0, "sequence": unknown })).unwrap_err();
    assert!(matches!(e, CmdError::NotFound { .. }), "{e:?}");

    // With two sequences the sequence must be named.
    let mut two = three_clips(W, H);
    let other = SequenceId::from_u128(2);
    two.project.sequences.insert(
        other,
        Sequence::new(other, "Other", SequenceFormat::default()),
    );
    let e = frame(&two, json!({ "t": 0 })).unwrap_err();
    assert_eq!(invalid_path(&e), "/sequence");
    let out = frame(&two, json!({ "t": "1s", "sequence": f.seq })).unwrap();
    assert_eq!(out.items_visible[0].item, f.a);
}

#[test]
fn hidden_items_are_reported() {
    let mut f = three_clips(W, H);
    // Make the top clip fully transparent and the bottom one offline.
    let seq = f.project.sequences.get_mut(&f.seq).unwrap();
    seq.track_mut(&f.v2).unwrap().items.update(&f.c, |i| {
        i.as_clip_mut().unwrap().visual.opacity = cutweave_model::Animatable::Static(0.0);
    });
    let mut source = crate::testcard::SyntheticSource::new();
    for (media, signal) in [(22, GREEN), (23, BLUE)] {
        source.insert(
            cutweave_model::MediaId::from_u128(media),
            crate::testcard::TestCard::new(
                W,
                H,
                crate::testcard::Pattern::Solid(signal),
                crate::frame::PixelFormat::Nv12,
                crate::testkit::bt709(),
            ),
        );
    }
    f.source = Arc::new(source);
    let out = frame(&f, json!({ "t": "3s" })).unwrap();
    assert!(out.items_visible.is_empty());
    let hidden: Vec<(ItemId, HiddenReason)> = out
        .items_hidden
        .iter()
        .map(|h| (h.item, h.reason))
        .collect();
    assert_eq!(
        hidden,
        [
            (f.c, HiddenReason::Transparent),
            (f.a, HiddenReason::Unavailable)
        ]
    );
    assert!(out.notes.iter().any(|n| n.contains(&f.a.to_string())));
}

#[test]
fn contact_sheets_sample_lay_out_and_label() {
    let f = three_clips(W, H);
    let out = sheet(&f, json!({ "count": 4, "format": "png" })).unwrap();
    assert_eq!((out.cols, out.rows), (2, 2));
    assert!(out.width <= 768 && out.height <= 768);
    let times: Vec<_> = out.frames.iter().map(|c| c.time).collect();
    assert_eq!(
        times,
        [sec(0.0), sec(2.0), sec(4.0), sec(6.0)],
        "end defaults to 8 s"
    );
    let labels: Vec<_> = out
        .frames
        .iter()
        .map(|c| c.timecode.clone().unwrap())
        .collect();
    assert_eq!(
        labels,
        ["00:00:00:00", "00:00:02:00", "00:00:04:00", "00:00:06:00"]
    );
    let items: Vec<Vec<ItemId>> = out.frames.iter().map(|c| c.items_visible.clone()).collect();
    assert_eq!(
        items,
        [vec![f.a], vec![f.c, f.a], vec![f.c, f.b], vec![f.b]]
    );
    // Cells are the sequence size (never upscaled) and do not overlap.
    let c0 = out.frames[0].cell;
    assert_eq!((c0.x, c0.y, c0.width, c0.height), (GAP, GAP, W, H));
    assert!(out.frames[1].cell.x >= c0.x + c0.width + GAP);
    assert_eq!(out.rev, None);

    // Pixels: each cell shows its frame; the label strip carries the timecode.
    let img = decode_png(&unbase64(&out.data));
    assert_eq!((img.width, img.height), (out.width, out.height));
    let (red, green, blue) = (
        expected_srgb(RED),
        expected_srgb(GREEN),
        expected_srgb(BLUE),
    );
    for (i, top_right, bottom_left) in [(0, red, red), (1, blue, red), (2, blue, green)] {
        let c = out.frames[i].cell;
        let tr = mean_rgb(&img, c.x + c.width * 3 / 4, c.y + c.height / 4, 16, 16);
        let bl = mean_rgb(&img, c.x + c.width / 4, c.y + c.height * 3 / 4, 16, 16);
        assert!(delta_e(tr, top_right) < 2.0, "cell {i} top right {tr:?}");
        assert!(
            delta_e(bl, bottom_left) < 2.0,
            "cell {i} bottom left {bl:?}"
        );
        let label_px = (c.y + c.height..c.y + c.height + 12)
            .flat_map(|y| (c.x..c.x + c.width).map(move |x| (x, y)))
            .filter(|&(x, y)| img.pixel(x, y) == crate::sheet::LABEL)
            .count();
        assert!(label_px > 30, "cell {i}: {label_px} label pixels");
    }
    // Without labels the strip is gone.
    let bare = sheet(&f, json!({ "count": 4, "labels": false, "format": "png" })).unwrap();
    assert!(bare.height < out.height);
    let img = decode_png(&unbase64(&bare.data));
    let c = bare.frames[0].cell;
    assert_eq!(img.pixel(c.x, c.y + c.height + 1), BACKGROUND);

    // Intervals, explicit ranges, aliases and columns.
    let by_interval = sheet(
        &f,
        json!({ "from": "1s", "to": "7s", "every": "2s", "columns": 3 }),
    )
    .unwrap();
    let times: Vec<_> = by_interval.frames.iter().map(|c| c.time).collect();
    assert_eq!(times, [sec(1.0), sec(3.0), sec(5.0)]);
    assert_eq!((by_interval.cols, by_interval.rows), (3, 1));
    assert_eq!(by_interval.mime, "image/jpeg");
    let jpeg = decode_jpeg(&unbase64(&by_interval.data));
    assert_eq!(
        (jpeg.width, jpeg.height),
        (by_interval.width, by_interval.height)
    );

    for (params, path) in [
        (json!({ "count": 3, "interval": "1s" }), ""),
        (json!({ "count": 0 }), "/count"),
        (json!({ "count": 65 }), "/count"),
        (json!({ "interval": "0s" }), "/interval"),
        (
            json!({ "interval": "1s", "start": 0, "end": "100s" }),
            "/interval",
        ),
        (json!({ "start": "5s", "end": "5s" }), "/end"),
        (json!({ "count": 64, "max_size": 64 }), "/max_size"),
        (json!({ "max_size": 4096 }), "/max_size"),
    ] {
        let e = sheet(&f, params.clone()).unwrap_err();
        assert_eq!(invalid_path(&e), path, "{params}: {e}");
    }
}
