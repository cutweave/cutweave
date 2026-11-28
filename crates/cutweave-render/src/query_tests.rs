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

/// The sheet's pixels before encoding (the query returns a JPEG).
fn composed(f: &ThreeClips, params: Value) -> crate::query::ComposedSheet {
    let ctx = QueryCtx::new(&f.project, Access::STANDARD);
    let params: RenderContactSheet = serde_json::from_value(params).unwrap();
    let mut backend = ReferenceBackend::new(Arc::clone(&f.source) as _);
    crate::query::compose_contact_sheet(&mut backend, &ctx, &params).unwrap()
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
    // `render.contact_sheet` is a catalog query: listed, it does not fit the standard list's
    // budget (ADR-0039, WP-2I.02).
    for (name, tool, exposure) in [
        ("render.frame", "render_frame", McpExposure::Core),
        (
            "render.contact_sheet",
            "render_contact_sheet",
            McpExposure::Catalog,
        ),
    ] {
        let e = r.get(name).unwrap();
        assert_eq!(e.kind(), EntryKind::Query);
        assert_eq!(e.meta.mcp, exposure);
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

/// WP-2F.03c: `render.frame` shows the "media offline" slate where a clip's file is gone,
/// pixel for pixel when the clip fills a frame of the slate's size, and says so in its notes.
#[test]
fn an_offline_clip_renders_the_media_offline_slate() {
    use crate::slate;
    let mut f = three_clips(slate::WIDTH, slate::HEIGHT);
    let source = crate::testcard::SyntheticSource::new()
        .with_offline(cutweave_model::MediaId::from_u128(21));
    f.source = Arc::new(source);
    // 1 s: only clip A (offline) is on screen, at full size.
    let out = frame(&f, json!({ "t": "1s", "format": "png", "max_size": 320 })).unwrap();
    assert_eq!((out.width, out.height), (slate::WIDTH, slate::HEIGHT));
    let png = png_of(&out);
    assert_eq!(png, slate::image(), "the slate, exactly");
    // The clip is on screen (as the slate), and the note names it.
    let visible: Vec<ItemId> = out.items_visible.iter().map(|v| v.item).collect();
    assert_eq!(visible, [f.a]);
    assert!(out.items_hidden.is_empty());
    assert!(
        out.notes
            .iter()
            .any(|n| n.contains(&f.a.to_string()) && n.contains("media offline")),
        "{:?}",
        out.notes
    );

    // At half size the slate is scaled with the frame: bands and plate keep their places.
    let half = frame(&f, json!({ "t": "1s", "format": "png", "max_size": 160 })).unwrap();
    let small = png_of(&half);
    assert_eq!((small.width, small.height), (160, 90));
    for (x, y, want) in [
        (2, 2, slate::BAND),
        (8, 5, slate::FIELD),
        (17, 36, slate::PLATE),
        (155, 85, slate::BAND),
    ] {
        let got = small.pixel(x, y);
        assert!(
            delta_e([got[0], got[1], got[2]], [want[0], want[1], want[2]]) < 2.0,
            "({x}, {y}): {got:?} for {want:?}"
        );
    }
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
    let out = sheet(&f, json!({ "count": 4 })).unwrap();
    assert_eq!(out.mime, "image/jpeg");
    assert_eq!((out.columns, out.rows), (2, 2));
    assert!(out.width <= 768 && out.height <= 768);
    let times: Vec<_> = out.frames.iter().map(|c| c.t).collect();
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
    let items: Vec<Vec<ItemId>> = out
        .frames
        .iter()
        .map(|c| c.items_visible.iter().map(|i| i.item).collect())
        .collect();
    // Each item names its track; the fixture's clips and media have no name.
    let first = &out.frames[0].items_visible[0];
    assert_eq!(
        f.project.sequences[&out.sequence]
            .find_item(&first.item)
            .map(|(track, _)| track.id),
        Some(first.track)
    );
    assert_eq!(first.name, None);
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
    let jpeg = decode_jpeg(&unbase64(&out.data));
    assert_eq!((jpeg.width, jpeg.height), (out.width, out.height));
    let img = composed(&f, json!({ "count": 4 })).image;
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
    let bare = composed(&f, json!({ "count": 4, "labels": false }));
    assert!(bare.image.height < out.height);
    let img = bare.image;
    let c = bare.frames[0].cell;
    assert_eq!(img.pixel(c.x, c.y + c.height + 1), BACKGROUND);

    // Intervals, explicit ranges and columns.
    let by_interval = sheet(
        &f,
        json!({ "from": "1s", "to": "7s", "every": "2s", "columns": 3 }),
    )
    .unwrap();
    let times: Vec<_> = by_interval.frames.iter().map(|c| c.t).collect();
    assert_eq!(times, [sec(1.0), sec(3.0), sec(5.0)]);
    assert_eq!((by_interval.columns, by_interval.rows), (3, 1));
    assert_eq!(by_interval.mime, "image/jpeg");
    let jpeg = decode_jpeg(&unbase64(&by_interval.data));
    assert_eq!(
        (jpeg.width, jpeg.height),
        (by_interval.width, by_interval.height)
    );

    for (params, path) in [
        (json!({ "count": 3, "every": "1s" }), ""),
        (json!({ "count": 3, "at": "cuts" }), ""),
        (json!({ "every": "1s", "at": ["1s"] }), ""),
        (json!({ "count": 0 }), "/count"),
        (json!({ "count": 65 }), "/count"),
        (json!({ "every": "0s" }), "/every"),
        (
            json!({ "every": "1s", "from": 0, "to": "100s" }),
            "/every",
        ),
        (json!({ "from": "5s", "to": "5s" }), "/to"),
        (json!({ "at": "markers" }), "/at"),
        (json!({ "at": [] }), "/at"),
        (json!({ "at": ["1s", "9s"] }), "/at/1"),
        (json!({ "at": "cuts", "from": "5s", "to": "6s" }), "/at"),

        (json!({ "count": 64, "max_size": 64 }), "/max_size"),
        (json!({ "max_size": 4096 }), "/max_size"),
    ] {
        let e = sheet(&f, params.clone()).unwrap_err();
        assert_eq!(invalid_path(&e), path, "{params}: {e}");
    }
}

/// WP-2I.02: frames at the cuts, at the markers and at given times.
#[test]
fn contact_sheets_at_cuts_markers_and_times() {
    let mut f = three_clips(W, H);
    // The clips start at 0 s and 4 s on V1 and at 2 s on V2.
    let cuts = sheet(&f, json!({ "at": "cuts" })).unwrap();
    let times: Vec<_> = cuts.frames.iter().map(|c| c.t).collect();
    assert_eq!(times, [sec(0.0), sec(2.0), sec(4.0)]);
    assert_eq!((cuts.columns, cuts.rows), (2, 2));
    let later = sheet(&f, json!({ "at": "cuts", "from": "1s" })).unwrap();
    assert_eq!(later.frames.len(), 2);

    // Times as given (the order kept), each moved to the start of its frame.
    let picked = sheet(&f, json!({ "at": ["5s", "f:30", 0], "columns": 3 })).unwrap();
    let times: Vec<_> = picked.frames.iter().map(|c| c.t).collect();
    assert_eq!(times, [sec(5.0), sec(1.0), sec(0.0)]);
    assert_eq!(picked.frames[0].timecode.as_deref(), Some("00:00:05:00"));

    // Markers, in time order, whatever the order they were added in.
    let seq = *f.project.sequences.keys().next().unwrap();
    for (n, at) in [(2u128, 6.0), (1, 3.0)] {
        let id = cutweave_model::MarkerId::from_u128(n);
        f.project
            .sequences
            .get_mut(&seq)
            .unwrap()
            .markers
            .insert(id, cutweave_model::Marker::new(id, sec(at)));
    }
    let marked = sheet(&f, json!({ "at": "markers" })).unwrap();
    let times: Vec<_> = marked.frames.iter().map(|c| c.t).collect();
    assert_eq!(times, [sec(3.0), sec(6.0)]);
}

/// A backend that paints every frame one flat colour at the size asked for: the layout and the
/// encoding of a sheet without the cost of rendering it.
struct FlatBackend(Arc<dyn crate::FrameSource + Send + Sync>);

impl crate::query::FrameBackend for FlatBackend {
    fn frame_source(&self) -> &dyn crate::FrameSource {
        &*self.0
    }

    fn rasterize(
        &mut self,
        _scene: &crate::SceneDescription,
        graph: &crate::graph::RenderGraph,
    ) -> Result<Image, String> {
        let (width, height) = graph.target.size();
        Ok(Image::filled(width, height, [90, 140, 200, 255]))
    }
}

mod max_size {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        /// WP-2I.02 acceptance: `max_size` is honoured for every value from 128 to 2048: the
        /// sheet and the JPEG that carries it are never larger, whatever the frame count, the
        /// columns, the labels and the shape of the sequence; a size too small for its cells is
        /// refused at `/max_size`.
        #[test]
        fn a_sheet_is_never_larger_than_max_size(
            max_size in 128u32..=2048,
            count in 1u32..=24,
            columns in proptest::option::of(1u32..=8),
            labels in any::<bool>(),
            shape in prop_oneof![Just((1920u32, 1080u32)), Just((1080, 1920)), Just((640, 640))],
        ) {
            let f = three_clips(shape.0, shape.1);
            let mut params = json!({ "count": count, "max_size": max_size, "labels": labels });
            if let Some(columns) = columns {
                params["columns"] = json!(columns);
            }
            let params: RenderContactSheet = serde_json::from_value(params).unwrap();
            let ctx = QueryCtx::new(&f.project, Access::STANDARD);
            let mut backend = FlatBackend(Arc::clone(&f.source) as _);
            match crate::query::render_contact_sheet(&mut backend, &ctx, &params) {
                Ok(sheet) => {
                    prop_assert!(sheet.width <= max_size && sheet.height <= max_size);
                    prop_assert_eq!(sheet.frames.len(), count as usize);
                    prop_assert_eq!(sheet.mime.as_str(), "image/jpeg");
                    let jpeg = decode_jpeg(&unbase64(&sheet.data));
                    prop_assert_eq!((jpeg.width, jpeg.height), (sheet.width, sheet.height));
                    for cell in &sheet.frames {
                        let c = cell.cell;
                        prop_assert!(c.x + c.width <= sheet.width);
                        prop_assert!(c.y + c.height <= sheet.height);
                    }
                }
                Err(e) => prop_assert_eq!(invalid_path(&e), "/max_size"),
            }
        }
    }

    /// The bounds of `max_size` themselves.
    #[test]
    fn max_size_has_bounds() {
        let f = three_clips(W, H);
        for (max_size, ok) in [(63, false), (64, true), (2048, true), (2049, false)] {
            let got = sheet(&f, json!({ "count": 1, "max_size": max_size }));
            assert_eq!(got.is_ok(), ok, "{max_size}");
        }
    }
}
