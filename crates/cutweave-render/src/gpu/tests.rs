// SPDX-License-Identifier: GPL-3.0-or-later
//! Headless GPU tests (WP-1F.02 acceptance). They run on the adapter `CUTWEAVE_GPU` selects,
//! **software by default** (WARP on Windows, lavapipe on Linux), so CI and dev machines agree;
//! the golden test also covers every other adapter present. Without any adapter the tests skip
//! with a message, unless `CUTWEAVE_GPU_REQUIRED=1` makes that a failure.
//!
//! GPU tests in one process run one at a time: several WARP devices used concurrently crash
//! the process (S4 findings 7 and 12). nextest runs each test in its own process anyway.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names
)]

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use cutweave_model::{
    Color, ColorPrimaries, ColorRange, ColorTags, FrameRate, ItemId, MatrixCoefficients, MediaId,
    SequenceId, Time, TrackId, TransferCharacteristics, Vec2,
};

use super::*;
use crate::color::{OutputTransfer, Transfer, from_linear, quantize8, to_linear};
use crate::frame::{ChromaSiting, PixelFormat};
use crate::golden::{GoldenStore, Verdict, update_requested};
use crate::graph::{GraphOptions, Target};
use crate::image::Image;
use crate::metrics::{IDENTITY_MIN_PSNR_DB, compare};
use crate::reference;
use crate::scene::{BlendMode, Crop, FrameRequest, Layer, LayerTransform, SceneDescription};
use crate::testcard::{Alpha, Pattern, SyntheticSource, TestCard};

static GPU_TESTS: Mutex<()> = Mutex::new(());

/// The fallback golden set (the CI software rasteriser on Windows).
pub(super) const FALLBACK_SET: &str = "dx12-warp";

pub(super) fn serial() -> MutexGuard<'static, ()> {
    GPU_TESTS.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) fn required() -> bool {
    std::env::var("CUTWEAVE_GPU_REQUIRED").is_ok_and(|v| v == "1")
}

/// A compositor on the test adapter, or `None` (skip) when there is none.
pub(super) fn compositor() -> Option<Compositor> {
    let choice = AdapterChoice::from_env_or(AdapterChoice::Software);
    match GpuContext::new(&choice)
        .and_then(|ctx| Compositor::new(ctx, CompositorOptions::default()))
    {
        Ok(c) => Some(c),
        Err(e) if !required() => {
            eprintln!("skipping GPU test: {e}");
            None
        }
        Err(e) => panic!("GPU required: {e}"),
    }
}

fn tags(
    matrix: MatrixCoefficients,
    range: ColorRange,
    transfer: TransferCharacteristics,
    primaries: ColorPrimaries,
) -> ColorTags {
    ColorTags {
        primaries: Some(primaries),
        transfer: Some(transfer),
        matrix: Some(matrix),
        range: Some(range),
    }
}

fn bt709(range: ColorRange) -> ColorTags {
    tags(
        MatrixCoefficients::Bt709,
        range,
        TransferCharacteristics::Bt709,
        ColorPrimaries::Bt709,
    )
}

fn identity() -> LayerTransform {
    LayerTransform {
        position: Vec2::ZERO,
        scale: Vec2::ONE,
        rotation_deg: 0.0,
        anchor: Vec2::new(0.5, 0.5),
    }
}

fn layer(item: u128, media: u128, size: (u32, u32)) -> Layer {
    Layer {
        item: ItemId::from_u128(item),
        track: TrackId::from_u128(item),
        request: FrameRequest {
            media: MediaId::from_u128(media),
            stream: Some(0),
            source_time: Time::ZERO,
            source_size: Some(size),
            rotation: 0,
            color: ColorTags::default(),
            still: false,
            proxy: false,
        },
        transform: identity(),
        opacity: 1.0,
        blend: BlendMode::Normal,
        crop: Crop::default(),
        effects: Vec::new(),
    }
}

fn scene(w: u32, h: u32, background: Color, layers: Vec<Layer>) -> SceneDescription {
    SceneDescription {
        sequence: SequenceId::from_u128(1),
        time: Time::ZERO,
        width: w,
        height: h,
        frame_rate: FrameRate::FPS_24,
        background,
        layers,
        audio: Vec::new(),
    }
}

fn render(c: &mut Compositor, s: &SceneDescription, src: &SyntheticSource, t: Target) -> Image {
    let f = c.render(s, src, t).expect("render");
    f.into_image().expect("CPU target")
}

fn cpu(s: &SceneDescription, src: &SyntheticSource, t: Target) -> Image {
    reference::render(s, t, src, &GraphOptions::default()).expect("reference")
}

const W: u32 = 384;
const H: u32 = 216;

/// The 3+2 tagged sources of the colour acceptance test.
fn tagged_sources() -> Vec<(&'static str, TestCard)> {
    let bars = |format, color| TestCard::new(W, H, Pattern::ColorBars, format, color);
    vec![
        (
            "bt709_limited_nv12",
            bars(PixelFormat::Nv12, bt709(ColorRange::Limited)),
        ),
        (
            "bt601_limited_yuv420p",
            bars(
                PixelFormat::Yuv420p,
                tags(
                    MatrixCoefficients::Smpte170M,
                    ColorRange::Limited,
                    TransferCharacteristics::Smpte170M,
                    ColorPrimaries::Smpte170M,
                ),
            ),
        ),
        (
            "bt709_full_nv12",
            bars(PixelFormat::Nv12, bt709(ColorRange::Full)),
        ),
        (
            "bt2020_pq_p010",
            TestCard {
                siting: ChromaSiting::TopLeft,
                ..TestCard::new(
                    W,
                    H,
                    Pattern::Sweep,
                    PixelFormat::P010,
                    tags(
                        MatrixCoefficients::Bt2020Ncl,
                        ColorRange::Limited,
                        TransferCharacteristics::Pq,
                        ColorPrimaries::Bt2020,
                    ),
                )
            },
        ),
        (
            "bt2020_hlg_p010",
            TestCard::new(
                W,
                H,
                Pattern::ColorBars,
                PixelFormat::P010,
                tags(
                    MatrixCoefficients::Bt2020Ncl,
                    ColorRange::Limited,
                    TransferCharacteristics::Hlg,
                    ColorPrimaries::Bt2020,
                ),
            ),
        ),
    ]
}

#[test]
fn tagged_color_bars_match_the_cpu_float_reference() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    for (name, card) in tagged_sources() {
        let src = SyntheticSource::new().with(MediaId::from_u128(9), card);
        let s = scene(W, H, Color::BLACK, vec![layer(1, 9, (W, H))]);
        for transfer in [OutputTransfer::Srgb, OutputTransfer::Bt709] {
            let t = Target::Rgba8Cpu {
                width: W,
                height: H,
                transfer,
            };
            let gpu = render(&mut c, &s, &src, t);
            let reference = cpu(&s, &src, t);
            let cmp = compare(&gpu, &reference, transfer);
            eprintln!("{name} ({transfer:?}): {cmp}");
            assert!(cmp.passes_reference(), "{name} {transfer:?}: {cmp}");
            assert!(cmp.max_abs_diff <= 2, "{name} {transfer:?}: {cmp}");
        }
    }
}

#[test]
fn identity_transform_reproduces_the_decoded_source() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    for (format, pattern) in [
        (PixelFormat::Nv12, Pattern::Sweep),
        (PixelFormat::Yuv420p, Pattern::ColorBars),
        (PixelFormat::P010, Pattern::Sweep),
    ] {
        let card = TestCard::new(W, H, pattern, format, bt709(ColorRange::Limited));
        let frame = card.frame(0);
        let src = SyntheticSource::new().with(MediaId::from_u128(9), card);
        let s = scene(W, H, Color::BLACK, vec![layer(1, 9, (W, H))]);
        let t = Target::Rgba8Cpu {
            width: W,
            height: H,
            transfer: OutputTransfer::Bt709,
        };
        let gpu = render(&mut c, &s, &src, t);
        let color = crate::color::InputColor::resolve(&frame.color, true, H);
        let decoded = reference::decode_signal8(&frame, &color);
        let cmp = compare(&gpu, &decoded, OutputTransfer::Bt709);
        eprintln!("identity {format:?}: {cmp}");
        assert!(
            cmp.identical || cmp.psnr_db > IDENTITY_MIN_PSNR_DB,
            "{format:?}: {cmp}"
        );
    }
}

#[test]
fn half_opacity_over_black_is_half_the_linear_light() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    for (level, format) in [
        (1.0, PixelFormat::Nv12),
        (0.75, PixelFormat::Yuv420p),
        (0.5, PixelFormat::Rgba8),
    ] {
        let color = if format == PixelFormat::Rgba8 {
            ColorTags::default()
        } else {
            bt709(ColorRange::Full)
        };
        let tf = if format == PixelFormat::Rgba8 {
            Transfer::Srgb
        } else {
            Transfer::Bt709
        };
        let card = TestCard::new(64, 36, Pattern::Solid([level; 3]), format, color);
        let frame = card.frame(0);
        let src = SyntheticSource::new().with(MediaId::from_u128(9), card);
        let mut l = layer(1, 9, (64, 36));
        l.opacity = 0.5;
        let s = scene(64, 36, Color::BLACK, vec![l]);
        for transfer in [OutputTransfer::Srgb, OutputTransfer::Bt709] {
            let t = Target::Rgba8Cpu {
                width: 64,
                height: 36,
                transfer,
            };
            let img = render(&mut c, &s, &src, t);
            // The source's linear light from its actual codes, halved, then encoded.
            // Full-range 8-bit: signal = code / 255 (luma, or R for RGBA).
            let signal = f64::from(frame.code(0, 0, 0, 0)) / 255.0;
            let want = quantize8(from_linear(
                0.5 * to_linear(signal, tf),
                transfer.transfer(),
            ));
            for px in img.data.as_chunks::<4>().0 {
                for &v in &px[..3] {
                    assert!(
                        v.abs_diff(want) <= 1,
                        "{format:?} level {level} {transfer:?}: got {v}, want {want} ±1"
                    );
                }
                assert_eq!(px[3], 255);
            }
        }
    }
}

/// The sources of the transform cases (a fresh memoising source per use).
fn sources() -> SyntheticSource {
    let nv12 = |w, h, p| TestCard::new(w, h, p, PixelFormat::Nv12, bt709(ColorRange::Limited));
    SyntheticSource::new()
        .with(MediaId::from_u128(10), nv12(W, H, Pattern::ColorBars))
        .with(
            MediaId::from_u128(11),
            TestCard::new(
                320,
                240,
                Pattern::Sweep,
                PixelFormat::Yuv420p,
                bt709(ColorRange::Limited),
            ),
        )
        .with(MediaId::from_u128(12), nv12(1536, 864, Pattern::Sweep))
        .with(
            MediaId::from_u128(13),
            TestCard {
                alpha: Alpha::RampX,
                ..TestCard::new(
                    200,
                    120,
                    Pattern::ColorBars,
                    PixelFormat::Rgba8,
                    ColorTags::default(),
                )
            },
        )
        .with(MediaId::from_u128(14), nv12(160, 90, Pattern::ColorBars))
}

/// Transform, crop, rotation, minification, alpha and multi-layer cases.
fn transform_cases() -> Vec<(&'static str, SceneDescription, SyntheticSource)> {
    let bg = Color::rgb(30, 60, 90);
    let mut pip = layer(2, 11, (320, 240));
    pip.transform = LayerTransform {
        position: Vec2::new(0.1, -0.05),
        scale: Vec2::new(0.45, 0.4),
        rotation_deg: 17.0,
        anchor: Vec2::new(0.3, 0.6),
    };
    pip.opacity = 0.8;
    pip.crop = Crop {
        left: 0.1,
        top: 0.05,
        right: 0.2,
        bottom: 0.0,
    };
    let mut minified = layer(3, 12, (1536, 864));
    minified.transform.scale = Vec2::new(0.3, 0.3);
    minified.transform.position = Vec2::new(-0.3, 0.25);
    let mut trilinear = layer(4, 12, (1536, 864));
    trilinear.transform.scale = Vec2::new(0.43, 0.43);
    trilinear.transform.position = Vec2::new(0.25, 0.25);
    let mut overlay = layer(5, 13, (200, 120));
    overlay.transform.scale = Vec2::new(0.6, 0.6);
    overlay.transform.rotation_deg = -8.0;
    let mut rotated = layer(6, 14, (160, 90));
    rotated.request.rotation = 90;
    rotated.transform.position = Vec2::new(0.35, 0.0);
    rotated.transform.scale = Vec2::new(0.7, 0.7);
    let mut magnified = layer(7, 11, (320, 240));
    magnified.transform.scale = Vec2::new(2.5, 2.5);
    magnified.transform.position = Vec2::new(0.05, 0.03);
    let clear = Color {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    vec![
        (
            "pip_rotated_cropped",
            scene(W, H, bg, vec![layer(1, 10, (W, H)), pip.clone()]),
            sources(),
        ),
        (
            "minified_mips",
            scene(W, H, bg, vec![minified, trilinear]),
            sources(),
        ),
        (
            "rgba_alpha_over_bars",
            scene(W, H, bg, vec![layer(1, 10, (W, H)), overlay]),
            sources(),
        ),
        (
            "display_rotation_90",
            scene(W, H, bg, vec![rotated]),
            sources(),
        ),
        ("magnified", scene(W, H, bg, vec![magnified]), sources()),
        (
            "transparent_background",
            scene(W, H, clear, vec![pip]),
            sources(),
        ),
    ]
}

#[test]
fn transforms_match_the_cpu_reference() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    for (name, s, src) in transform_cases() {
        for t in [Target::rgba8(W, H), Target::rgba8(W / 2 + 7, H / 2 + 3)] {
            let gpu = render(&mut c, &s, &src, t);
            let reference = cpu(&s, &src, t);
            let cmp = compare(&gpu, &reference, OutputTransfer::Srgb);
            eprintln!("{name} @ {:?}: {cmp}", t.size());
            assert!(cmp.passes_reference(), "{name} @ {:?}: {cmp}", t.size());
        }
    }
}

#[test]
fn same_adapter_renders_are_bit_identical_cold_warm_and_after_recovery() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    let cases = transform_cases();
    let (_, s, src) = &cases[0];
    let (_, other, other_src) = &cases[1];
    let t = Target::rgba8(W, H);
    let cold = c.render(s, src, t).unwrap();
    assert_eq!(cold.stats.cache_hits, 0);
    let _ = c.render(other, other_src, t).unwrap();
    let warm = c.render(s, src, t).unwrap();
    assert!(warm.stats.cache_hits > 0, "{:?}", warm.stats);
    assert_eq!(warm.key, cold.key);
    assert_eq!(
        warm.image(),
        cold.image(),
        "warm caches and pooled textures change nothing"
    );
    c.clear_cache();
    let again = c.render(s, src, t).unwrap();
    assert_eq!(again.image(), cold.image());
    c.simulate_device_loss();
    let recovered = c.render(s, src, t).unwrap();
    assert!(recovered.stats.recovered);
    assert_eq!(
        recovered.image(),
        cold.image(),
        "a recovered device renders identically"
    );
}

#[test]
fn a_lost_device_is_recovered_by_the_next_render() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    let src = sources();
    let s = scene(W, H, Color::BLACK, vec![layer(1, 10, (W, H))]);
    let before = render(&mut c, &s, &src, Target::rgba8(W, H));
    c.simulate_device_loss();
    assert!(c.context().is_lost());
    let f = c
        .render(&s, &src, Target::rgba8(W, H))
        .expect("recovered render");
    assert!(f.stats.recovered);
    assert!(!c.context().is_lost());
    assert_eq!(f.image(), Some(&before));
    // And it keeps working.
    let next = c.render(&s, &src, Target::rgba8(W, H)).unwrap();
    assert!(!next.stats.recovered);
}

#[test]
fn texture_targets_keep_the_linear_canvas_on_the_gpu() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    let src = sources();
    let s = scene(W, H, Color::BLACK, vec![layer(1, 10, (W, H))]);
    let f = c
        .render(
            &s,
            &src,
            Target::Texture {
                width: 200,
                height: 100,
            },
        )
        .unwrap();
    let RenderedPixels::Texture(t) = &f.pixels else {
        panic!("texture expected")
    };
    assert_eq!((t.width(), t.height()), (200, 100));
    assert_eq!(t.format(), wgpu::TextureFormat::Rgba16Float);
    // An empty scene is the cleared background.
    let empty = c
        .render(
            &scene(W, H, Color::BLACK, vec![]),
            &src,
            Target::Texture {
                width: 8,
                height: 8,
            },
        )
        .unwrap();
    assert!(matches!(empty.pixels, RenderedPixels::Texture(_)));
}

#[test]
fn occluded_layers_are_never_decoded() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    let src = sources();
    let bg = Color::rgb(30, 60, 90);
    let mut under = layer(2, 11, (320, 240));
    under.transform.scale = Vec2::new(0.5, 0.5);
    under.transform.rotation_deg = 30.0;
    // A full-frame opaque layer (bars, 384×216 into a 384×216 canvas) hides everything below.
    let stacked = scene(W, H, bg, vec![under, layer(1, 10, (W, H))]);
    let alone = scene(W, H, bg, vec![layer(1, 10, (W, H))]);
    for t in [Target::rgba8(W, H), Target::rgba8(W / 3, H / 3)] {
        let culled = c.render(&stacked, &src, t).unwrap();
        assert_eq!(culled.stats.graph.layers_occluded, 1);
        assert_eq!(culled.stats.decodes_run + culled.stats.cache_hits, 1);
        let single = c.render(&alone, &src, t).unwrap();
        assert_eq!(culled.image(), single.image());
        // The CPU reference composites the hidden layer too: the same picture.
        let full = cpu(&stacked, &src, t);
        let cmp = compare(culled.image().unwrap(), &full, OutputTransfer::Srgb);
        assert!(cmp.passes_reference() && cmp.max_abs_diff <= 1, "{cmp}");
    }
}

#[test]
fn missing_media_renders_the_rest_and_reports_the_item() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    let src = sources();
    let s = scene(
        64,
        36,
        Color::rgb(200, 10, 10),
        vec![layer(1, 999, (64, 36))],
    );
    let f = c.render(&s, &src, Target::rgba8(64, 36)).unwrap();
    assert_eq!(f.missing.len(), 1);
    assert_eq!(f.missing[0].0, ItemId::from_u128(1));
    let img = f.image().unwrap();
    assert!(
        img.data
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [200, 10, 10, 255])
    );
}

/// The golden scenes: small frames covering colour conversion, placement and blending.
fn golden_scenes() -> Vec<(&'static str, SceneDescription, SyntheticSource)> {
    let mut out: Vec<(&'static str, SceneDescription, SyntheticSource)> = Vec::new();
    for (name, card) in tagged_sources() {
        let name: &'static str = match name {
            "bt709_limited_nv12" => "bars_bt709_limited",
            "bt601_limited_yuv420p" => "bars_bt601_limited",
            "bt709_full_nv12" => "bars_bt709_full",
            "bt2020_pq_p010" => "sweep_pq_tonemapped",
            _ => "bars_hlg_tonemapped",
        };
        let src = SyntheticSource::new().with(MediaId::from_u128(9), card);
        out.push((
            name,
            scene(W, H, Color::BLACK, vec![layer(1, 9, (W, H))]),
            src,
        ));
    }
    for (name, s, src) in transform_cases() {
        out.push((name, s, src));
    }
    out
}

#[test]
fn golden_frames_match_per_adapter_references() {
    let _serial = serial();
    let store = GoldenStore::workspace();
    let update = update_requested();
    let strict = std::env::var("CI").is_ok() || required();
    let adapters = list_adapters();
    if adapters.is_empty() {
        assert!(!required(), "GPU required but no adapter present");
        eprintln!("skipping golden test: no GPU adapter");
        return;
    }
    let mut failures = Vec::new();
    for info in &adapters {
        let mut c = match GpuContext::new(&AdapterChoice::Index(info.index))
            .and_then(|ctx| Compositor::new(ctx, CompositorOptions::default()))
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}: cannot create a device: {e}", info.key);
                continue;
            }
        };
        for (name, s, src) in golden_scenes() {
            let img = render(&mut c, &s, &src, Target::rgba8(W, H));
            // Same adapter: a second render is bit-identical.
            let again = render(&mut c, &s, &src, Target::rgba8(W, H));
            if img != again {
                failures.push(format!("{} {name}: not deterministic", info.key));
            }
            let r = store
                .check(&info.key, FALLBACK_SET, name, &img, update)
                .expect("golden I/O");
            eprintln!("{} {r}", info.key);
            // Every adapter also stays within §20 of the WARP references (cross-vendor check).
            if !update && r.same_adapter && info.key != FALLBACK_SET {
                let cross = store
                    .check(FALLBACK_SET, FALLBACK_SET, name, &img, false)
                    .expect("golden I/O");
                eprintln!("{}   {cross}", info.key);
                if cross.verdict == Verdict::Failed {
                    failures.push(format!("{} {cross}", info.key));
                }
            }
            match r.verdict {
                Verdict::BitIdentical | Verdict::WithinTolerance | Verdict::Updated => {}
                Verdict::Unavailable if !strict => {
                    eprintln!("  (LFS pointer: run `git lfs checkout testdata/golden`)");
                }
                Verdict::Missing if !strict && !info.software => {
                    eprintln!("  (no reference for this adapter class yet)");
                }
                _ => failures.push(format!("{} {r}", info.key)),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "golden failures:\n{}",
        failures.join("\n")
    );
}

/// Informal per-adapter timings (WP-1F.02; formal budgets are WP-0F.03 / §19):
/// `cargo test -p cutweave-render --release -- --ignored --nocapture informal_timings`.
#[test]
#[ignore = "timing report, run on demand"]
fn informal_timings() {
    let _serial = serial();
    let (w, h) = (1920u32, 1080u32);
    let frames = 60i64;
    let animated = |format, pattern, color| TestCard {
        animated: true,
        ..TestCard::new(w, h, pattern, format, color)
    };
    let src = SyntheticSource::new()
        .with(
            MediaId::from_u128(1),
            animated(
                PixelFormat::Nv12,
                Pattern::ColorBars,
                bt709(ColorRange::Limited),
            ),
        )
        .with(
            MediaId::from_u128(2),
            animated(
                PixelFormat::P010,
                Pattern::Sweep,
                bt709(ColorRange::Limited),
            ),
        )
        .with(
            MediaId::from_u128(3),
            TestCard::new(
                1280,
                720,
                Pattern::Sweep,
                PixelFormat::Rgba8,
                ColorTags::default(),
            ),
        );
    let at = |l: &mut Layer, i: i64| {
        l.request.source_time =
            Time::from_flicks(i * cutweave_model::FLICKS_PER_SECOND / 25).unwrap();
    };
    let one = |i: i64| {
        let mut l = layer(1, 1, (w, h));
        at(&mut l, i);
        scene(w, h, Color::BLACK, vec![l])
    };
    let three = |i: i64| {
        let mut a = layer(1, 1, (w, h));
        at(&mut a, i);
        let mut b = layer(2, 2, (w, h));
        at(&mut b, i);
        b.transform.scale = Vec2::new(0.45, 0.45);
        b.transform.rotation_deg = 10.0;
        b.transform.position = Vec2::new(0.2, -0.2);
        let mut cc = layer(3, 3, (1280, 720));
        cc.transform.scale = Vec2::new(0.3, 0.3);
        cc.transform.position = Vec2::new(-0.3, 0.3);
        cc.opacity = 0.9;
        scene(w, h, Color::BLACK, vec![a, b, cc])
    };
    // Generate the test-card frames up front (CPU work outside the measurement).
    for i in 0..frames {
        let _ = reference::render(&one(i), Target::rgba8(4, 4), &src, &GraphOptions::default());
        let _ = reference::render(
            &three(i),
            Target::rgba8(4, 4),
            &src,
            &GraphOptions::default(),
        );
    }
    for info in list_adapters() {
        let Ok(mut c) = GpuContext::new(&AdapterChoice::Index(info.index))
            .and_then(|ctx| Compositor::new(ctx, CompositorOptions::default()))
        else {
            continue;
        };
        let n = if info.software { 10 } else { frames };
        for (label, make) in [
            ("1 layer", &one as &dyn Fn(i64) -> SceneDescription),
            ("3 layers", &three),
        ] {
            for (tname, target) in [
                ("1080p", Target::rgba8(w, h)),
                ("540p", Target::rgba8(w / 2, h / 2)),
            ] {
                let _ = c.render(&make(0), &src, target).unwrap();
                let mut ms: Vec<f64> = (1..=n)
                    .map(|i| {
                        let s = make(i);
                        let t0 = Instant::now();
                        let _ = c.render(&s, &src, target).unwrap();
                        t0.elapsed().as_secs_f64() * 1e3
                    })
                    .collect();
                ms.sort_by(f64::total_cmp);
                let p50 = ms[ms.len() / 2];
                let p95 = ms[(ms.len() * 95 / 100).min(ms.len() - 1)];
                eprintln!(
                    "{:<40} {:<8} {label:<8} {tname:<5} p50 {p50:7.2} ms  p95 {p95:7.2} ms (upload + render + readback, n={n})",
                    info.name, info.backend
                );
            }
        }
    }
}
