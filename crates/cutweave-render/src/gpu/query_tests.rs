// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1F.03 on the GPU: `render.frame` / `render.contact_sheet` through the registry on **every
//! adapter present** (WARP plus the local GPUs), asynchronous readback, the contact-sheet
//! golden and the 1080p end-to-end timing. Like the WP-1F.02 tests they skip without an adapter
//! unless `CUTWEAVE_GPU_REQUIRED=1`, and run one at a time.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use cutweave_commands::{Access, QueryCtx, Registry, Rev};
use cutweave_model::{ColorTags, ItemId, MediaId};
use serde_json::{Value, json};

use super::tests::{FALLBACK_SET, compositor, required, serial};
use super::{
    AdapterChoice, AdapterInfo, Compositor, CompositorOptions, GpuContext, GpuError,
    GpuFrameBackend, RenderError, list_adapters,
};
use crate::color::OutputTransfer;
use crate::evaluate;
use crate::frame::{FrameSource, PixelFormat};
use crate::golden::{GoldenStore, Verdict, update_requested};
use crate::graph::Target;
use crate::image::Image;
use crate::inspect::PixelRect;
use crate::metrics::compare;
use crate::quality::{Quality, Scale};
use crate::query::{
    FrameImage, ReferenceBackend, RenderContactSheet, SharedBackend, compose_contact_sheet, enter,
    register,
};
use crate::testcard::{Pattern, SyntheticSource, TestCard};
use crate::testkit::{
    BLUE, GREEN, RED, ThreeClips, background_srgb, bt709, decode_jpeg, decode_png, delta_e,
    expected_srgb, mean_rgb, sec, three_clips, unbase64,
};

/// One compositor per adapter present, or none (skip) without adapters.
fn every_adapter() -> Vec<(AdapterInfo, Compositor)> {
    let adapters = list_adapters();
    assert!(
        !(adapters.is_empty() && required()),
        "GPU required but no adapter present"
    );
    let mut out = Vec::new();
    for info in adapters {
        match GpuContext::new(&AdapterChoice::Index(info.index))
            .and_then(|ctx| Compositor::new(ctx, CompositorOptions::default()))
        {
            Ok(c) => out.push((info, c)),
            Err(e) => eprintln!("{}: cannot create a device: {e}", info.key),
        }
    }
    if out.is_empty() {
        eprintln!("skipping GPU test: no usable adapter");
    }
    out
}

fn registry() -> Registry {
    let mut r = Registry::new();
    register(&mut r).unwrap();
    r
}

fn query(
    backend: &SharedBackend,
    f: &ThreeClips,
    name: &str,
    params: Value,
) -> Result<Value, cutweave_commands::CmdError> {
    let _scope = enter(Arc::clone(backend));
    let ctx = QueryCtx::new(&f.project, Access::STANDARD).with_rev(Rev(3));
    registry().query_json(name, params, &ctx)
}

fn frame(backend: &SharedBackend, f: &ThreeClips, params: Value) -> FrameImage {
    serde_json::from_value(query(backend, f, "render.frame", params).expect("render.frame"))
        .unwrap()
}

fn gpu_backend(c: Compositor, f: &ThreeClips) -> SharedBackend {
    GpuFrameBackend::new(c, Arc::clone(&f.source) as _).shared()
}

#[test]
fn render_frame_on_every_adapter() {
    let _serial = serial();
    let f = three_clips(1920, 1080);
    let (red, green, blue) = (
        expected_srgb(RED),
        expected_srgb(GREEN),
        expected_srgb(BLUE),
    );
    let cpu = ReferenceBackend::new(Arc::clone(&f.source) as _).shared();
    for (info, c) in every_adapter() {
        let backend = gpu_backend(c, &f);
        let key = &info.key;

        // Output dimensions and formats: ≤ 768 px JPEG by default, PNG on request, never
        // larger than the sequence.
        let out = frame(&backend, &f, json!({ "t": "1s" }));
        assert_eq!(
            (out.width, out.height, out.mime.as_str()),
            (768, 432, "image/jpeg")
        );
        let jpeg = decode_jpeg(&unbase64(&out.data));
        assert_eq!((jpeg.width, jpeg.height), (768, 432), "{key}");
        let out = frame(
            &backend,
            &f,
            json!({ "t": "1s", "max_size": 4096, "format": "png" }),
        );
        assert_eq!(
            (out.width, out.height, out.mime.as_str()),
            (1920, 1080, "image/png")
        );
        assert_eq!(decode_png(&unbase64(&out.data)).width, 1920);

        // Three distinct times of the 3-clip project against the analytic colours (ΔE < 2),
        // in both formats; outside the content, the background.
        for (t, top_right, bottom_left) in [
            (1.0, red, red),
            (3.0, blue, red),
            (5.0, blue, green),
            (9.0, background_srgb(), background_srgb()),
        ] {
            for format in ["png", "jpeg"] {
                let out = frame(
                    &backend,
                    &f,
                    json!({ "t": format!("{t}s"), "format": format }),
                );
                let bytes = unbase64(&out.data);
                let img = if format == "png" {
                    decode_png(&bytes)
                } else {
                    decode_jpeg(&bytes)
                };
                let tr = mean_rgb(&img, 576, 108, 32, 32);
                let bl = mean_rgb(&img, 192, 324, 32, 32);
                for (got, want, at) in [
                    (tr, top_right, "top right"),
                    (bl, bottom_left, "bottom left"),
                ] {
                    let de = delta_e(got, want);
                    assert!(
                        de < 2.0,
                        "{key} {format} {t}s {at}: {got:?} vs {want:?} ΔE {de:.2}"
                    );
                }
            }
        }

        // Metadata against evaluate(): items top first with their rectangles.
        let out = frame(&backend, &f, json!({ "t": "3s" }));
        let scene = evaluate(&f.project, f.seq, sec(3.0)).unwrap();
        let ids: Vec<ItemId> = out.items_visible.iter().map(|v| v.item).collect();
        assert_eq!(
            ids,
            scene
                .layers
                .iter()
                .rev()
                .map(|l| l.item)
                .collect::<Vec<_>>()
        );
        assert_eq!(ids, [f.c, f.a]);
        assert_eq!(
            out.items_visible[0].rect,
            PixelRect {
                x: 384,
                y: 0,
                width: 384,
                height: 216
            }
        );
        assert_eq!(
            (out.time, out.frame, out.timecode.as_deref(), out.rev),
            (sec(3.0), Some(90), Some("00:00:03:00"), Some(Rev(3)))
        );

        // Determinism: same (rev, t, quality) twice → identical bytes, JPEG and PNG.
        for params in [json!({ "t": "5s" }), json!({ "t": "5s", "format": "png" })] {
            let a = frame(&backend, &f, params.clone());
            let b = frame(&backend, &f, params);
            assert_eq!(a.data, b.data, "{key}: not deterministic");
        }

        // Same picture as the CPU reference renderer.
        let gpu = frame(
            &backend,
            &f,
            json!({ "t": "3s", "max_size": 240, "format": "png" }),
        );
        let reference = frame(
            &cpu,
            &f,
            json!({ "t": "3s", "max_size": 240, "format": "png" }),
        );
        let c = compare(
            &decode_png(&unbase64(&gpu.data)),
            &decode_png(&unbase64(&reference.data)),
            OutputTransfer::Srgb,
        );
        assert!(c.passes_reference(), "{key}: GPU vs CPU {c}");
        eprintln!("{key}: render.frame ok; GPU vs CPU {c}");
    }
}

/// Polls until the oldest readback is ready (bounded), then falls back to a blocking receive.
fn next_frame(c: &mut Compositor, out: &mut Image) -> super::ReadbackFrame {
    for _ in 0..100_000 {
        if let Some(meta) = c
            .try_receive(|meta, px| {
                meta.copy_into(px, out);
                meta.clone()
            })
            .unwrap()
        {
            return meta;
        }
        std::thread::yield_now();
    }
    c.receive(|meta, px| {
        meta.copy_into(px, out);
        meta.clone()
    })
    .unwrap()
    .expect("a frame in flight")
}

#[test]
fn async_readback_matches_blocking_and_allocates_no_buffers_per_frame() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    let f = three_clips(320, 180);
    let target = Target::rgba8(320, 180);
    let times: Vec<f64> = (0..16).map(|i| f64::from(i) * 0.6).collect();
    let scenes: Vec<_> = times
        .iter()
        .map(|&t| evaluate(&f.project, f.seq, sec(t)).unwrap())
        .collect();
    let blocking: Vec<Image> = scenes
        .iter()
        .map(|s| {
            c.render(s, &*f.source, target)
                .unwrap()
                .into_image()
                .unwrap()
        })
        .collect();

    // Two frames in flight (render N while N-1 is read back): same bytes, in order, and the
    // pool settles at two staging buffers whatever the frame count.
    let mut out = Image::filled(1, 1, [0; 4]);
    let mut tickets = Vec::new();
    let mut received = 0;
    for s in &scenes {
        tickets.push(c.submit(s, &*f.source, target).unwrap());
        if c.readbacks_in_flight() == 2 {
            let meta = next_frame(&mut c, &mut out);
            assert_eq!(meta.ticket, tickets[received]);
            assert_eq!(out, blocking[received], "frame {received}");
            received += 1;
        }
    }
    while c.readbacks_in_flight() > 0 {
        next_frame(&mut c, &mut out);
        assert_eq!(out, blocking[received], "frame {received}");
        received += 1;
    }
    assert_eq!(received, scenes.len());
    assert_eq!(
        c.readback_buffers_created(),
        2,
        "buffers reused across frames"
    );
    let capacity = out.data.capacity();

    // Every buffer in flight: the next submission fails instead of stalling.
    for s in &scenes[..3] {
        c.submit(s, &*f.source, target).unwrap();
    }
    let busy = c.submit(&scenes[3], &*f.source, target).unwrap_err();
    assert!(
        matches!(busy, RenderError::Gpu(GpuError::ReadbackBusy(3))),
        "{busy:?}"
    );
    for want in &blocking[..3] {
        next_frame(&mut c, &mut out);
        assert_eq!(&out, want);
    }
    assert_eq!(out.data.capacity(), capacity, "the output image is reused");
    assert_eq!(c.readback_buffers_created(), 3);

    // A seek discards what is in flight: only the newest frame is delivered.
    c.submit(&scenes[4], &*f.source, target).unwrap();
    c.submit(&scenes[5], &*f.source, target).unwrap();
    c.discard_readbacks();
    let newest = c.submit(&scenes[6], &*f.source, target).unwrap();
    let meta = next_frame(&mut c, &mut out);
    assert_eq!(meta.ticket, newest);
    assert_eq!(out, blocking[6]);
    assert_eq!(c.readbacks_in_flight(), 0);
    assert!(c.receive(|_, _| ()).unwrap().is_none());

    // A quality step changes the size: buffers are resized, not added.
    let half = Target::rgba8(160, 90);
    c.submit(&scenes[1], &*f.source, half).unwrap();
    next_frame(&mut c, &mut out);
    assert_eq!((out.width, out.height), (160, 90));
    assert_eq!(
        out,
        c.render(&scenes[1], &*f.source, half)
            .unwrap()
            .into_image()
            .unwrap()
    );
    assert_eq!(c.readback_buffers_created(), 4);
    // Texture targets cannot be read back asynchronously.
    let texture = Target::Texture {
        width: 16,
        height: 16,
    };
    assert!(c.submit(&scenes[0], &*f.source, texture).is_err());

    // A lost device drops what is in flight; the next frame renders on a new device.
    c.submit(&scenes[2], &*f.source, target).unwrap();
    c.simulate_device_loss();
    assert!(c.try_receive(|_, _| ()).is_err());
    c.recover().unwrap();
    assert_eq!(c.readbacks_in_flight(), 0);
    c.submit(&scenes[2], &*f.source, target).unwrap();
    next_frame(&mut c, &mut out);
    assert_eq!(out, blocking[2]);
}

#[test]
fn every_rung_renders_the_same_picture_at_its_size() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    let f = three_clips(320, 180);
    let bars = |w, h| TestCard::new(w, h, Pattern::ColorBars, PixelFormat::Nv12, bt709());
    // Clip A has a half-resolution proxy.
    let source = SyntheticSource::new()
        .with(MediaId::from_u128(21), bars(320, 180))
        .with_proxy(MediaId::from_u128(21), bars(160, 90));
    let scene = evaluate(&f.project, f.seq, sec(1.0)).unwrap();
    let full = c
        .render_to_image(&scene, &source, (320, 180), Quality::FULL)
        .unwrap()
        .into_image()
        .unwrap();
    for (quality, size) in [
        (Quality::new(Scale::Half, false), (160, 90)),
        (Quality::new(Scale::Quarter, false), (80, 45)),
        (Quality::new(Scale::Full, true), (320, 180)),
        (Quality::new(Scale::Half, true), (160, 90)),
    ] {
        let frame = c
            .render_to_image(&scene, &source, (320, 180), quality)
            .unwrap();
        let img = frame.into_image().unwrap();
        assert_eq!((img.width, img.height), size, "{quality:?}");
        // The same picture: compare with the full-quality frame rendered at this size.
        let same_size = c
            .render(&scene, &source, Target::rgba8(size.0, size.1))
            .unwrap()
            .into_image()
            .unwrap();
        let cmp = compare(&img, &same_size, OutputTransfer::Srgb);
        if quality.use_proxies {
            // A proxy is softer but the same picture, in the same place.
            assert!(cmp.psnr_db > 25.0, "{quality:?}: {cmp}");
        } else {
            assert!(cmp.identical, "{quality:?}: {cmp}");
        }
    }
    assert_eq!((full.width, full.height), (320, 180));
    // The scene passed in is not modified.
    assert!(scene.layers.iter().all(|l| !l.request.proxy));
}

/// The golden contact sheet's sources for the [`three_clips`] media: colour bars, a sweep and
/// a flat picture-in-picture.
fn golden_source() -> Arc<dyn FrameSource + Send + Sync> {
    let card = |pattern, format, color| TestCard::new(384, 216, pattern, format, color);
    Arc::new(
        SyntheticSource::new()
            .with(
                MediaId::from_u128(21),
                card(Pattern::ColorBars, PixelFormat::Nv12, bt709()),
            )
            .with(
                MediaId::from_u128(22),
                card(Pattern::Sweep, PixelFormat::Yuv420p, bt709()),
            )
            .with(
                MediaId::from_u128(23),
                card(
                    Pattern::Solid(BLUE),
                    PixelFormat::Rgba8,
                    ColorTags::default(),
                ),
            ),
    )
}

#[test]
fn contact_sheet_golden_per_adapter() {
    let _serial = serial();
    let store = GoldenStore::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/golden"));
    let update = update_requested();
    let strict = std::env::var("CI").is_ok() || required();
    let f = three_clips(384, 216);
    let source = golden_source();
    let params: RenderContactSheet = serde_json::from_value(json!({
        "count": 6, "cols": 3, "start": "0s", "end": "8s", "max_size": 640
    }))
    .unwrap();
    let ctx = QueryCtx::new(&f.project, Access::STANDARD);
    let mut failures = Vec::new();
    for (info, c) in every_adapter() {
        let mut backend = GpuFrameBackend::new(c, Arc::clone(&source));
        let sheet = compose_contact_sheet(&mut backend, &ctx, &params).unwrap();
        let again = compose_contact_sheet(&mut backend, &ctx, &params).unwrap();
        assert_eq!(sheet, again, "{}: not deterministic", info.key);
        assert_eq!((sheet.cols, sheet.rows), (3, 2));
        assert_eq!(sheet.frames.len(), 6);
        let name = "contact_sheet_3x2.golden";
        let r = store
            .check(&info.key, FALLBACK_SET, name, &sheet.image, update)
            .expect("golden I/O");
        eprintln!("{} {r}", info.key);
        if !update && r.same_adapter && info.key != FALLBACK_SET {
            let cross = store
                .check(FALLBACK_SET, FALLBACK_SET, name, &sheet.image, false)
                .expect("golden I/O");
            eprintln!("{}   {cross}", info.key);
            if cross.verdict == Verdict::Failed {
                failures.push(format!("{} {cross}", info.key));
            }
        }
        match r.verdict {
            Verdict::BitIdentical | Verdict::WithinTolerance | Verdict::Updated => {}
            Verdict::Unavailable if !strict => {
                eprintln!("  (LFS pointer: run `git lfs checkout`)");
            }
            Verdict::Missing if !strict && !info.software => {
                eprintln!("  (no reference for this adapter class yet)");
            }
            _ => failures.push(format!("{} {r}", info.key)),
        }
    }
    assert!(
        failures.is_empty(),
        "golden failures:\n{}",
        failures.join("\n")
    );
}

/// WP-1F.03 acceptance: 1080p `render.frame` to a 768 px JPEG end to end (registry dispatch,
/// evaluation, graph, GPU render and readback, inspection, JPEG, base64, JSON) with a warm
/// cache, timed per adapter. Budget: < 150 ms on the dev machine (hardware adapters).
#[test]
fn render_frame_1080p_to_768_jpeg_end_to_end() {
    let _serial = serial();
    let f = three_clips(1920, 1080);
    for (info, c) in every_adapter() {
        let backend = gpu_backend(c, &f);
        let params = json!({ "t": "3s" });
        // Warm: card generation, pipelines, decode cache.
        for _ in 0..2 {
            frame(&backend, &f, params.clone());
        }
        let n = if info.software { 8 } else { 30 };
        let mut ms: Vec<f64> = (0..n)
            .map(|_| {
                let t0 = Instant::now();
                let out = query(&backend, &f, "render.frame", params.clone()).unwrap();
                let elapsed = t0.elapsed().as_secs_f64() * 1e3;
                assert_eq!(out["width"], 768);
                elapsed
            })
            .collect();
        ms.sort_by(f64::total_cmp);
        let p50 = ms[ms.len() / 2];
        let p95 = ms[(ms.len() * 95 / 100).min(ms.len() - 1)];
        eprintln!(
            "render.frame 1080p -> 768 JPEG, warm, {:<40} {:<7} p50 {p50:7.2} ms  p95 {p95:7.2} ms (n={n})",
            info.name, info.backend
        );
        if !info.software {
            assert!(
                p50 < 150.0,
                "{}: p50 {p50:.1} ms over the 150 ms budget",
                info.key
            );
        }
    }
}
