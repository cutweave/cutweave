// SPDX-License-Identifier: GPL-3.0-or-later
//! GPU tests of WP-2C.04 (transform, crop, flip, opacity, the core blend modes and filtering),
//! on the adapter `CUTWEAVE_GPU` selects (software by default), one at a time like the rest of
//! the GPU tests. The golden frames of these scenes are part of
//! [`golden_frames_match_per_adapter_references`](super::tests).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names
)]

use cutweave_model::{
    Animatable, Clip, Color, ColorRange, ColorTags, Duration, Extra, FLICKS_PER_SECOND, FrameRate,
    Interp, ItemId, Keyframe, Keyframes, MediaAsset, MediaId, MediaKind, ProbeInfo, Project,
    SampleRate, Sequence, SequenceFormat, SequenceId, StreamInfo, StreamKind, Time, Track, TrackId,
    TrackKind, Vec2,
};

use super::tests::{H, W, bt709, compositor, cpu, layer, render, scene, serial};
use super::{Compositor, RenderedPixels};
use crate::color::OutputTransfer;
use crate::geometry::{LayerGeometry, apply_affine};
use crate::graph::{BuildScratch, Filter, GraphOptions, RenderGraph, Target, build, build_into};
use crate::image::Image;
use crate::metrics::compare;
use crate::quality::Quality;
use crate::reference;
use crate::scene::{BlendMode, Crop, SceneDescription};
use crate::testcard::{Alpha, Pattern, SyntheticSource, TestCard};
use crate::{TimelineIndex, evaluate};

/// The acceptance bound of the blend modes against the analytic reference, linear light: the
/// GPU rounds the decoded picture, the canvas and each composite to half floats (about
/// 2.4e-4 each near 1.0); the reference does not round.
const LINEAR_MAX_ABS: f32 = 1e-3;

/// The bound for minified layers: each mip level of the chain is one more half-float
/// rounding of a box average.
const LINEAR_MAX_ABS_MIPS: f32 = 2e-3;

/// Converts an IEEE half to `f32` (the readback of `Rgba16Float`).
fn half(h: u16) -> f32 {
    let sign = if h & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exp = i32::from((h >> 10) & 0x1f);
    let man = f32::from(h & 0x3ff);
    match exp {
        0 => sign * man * 2f32.powi(-24),
        31 if man == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        e => sign * (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Reads a `Texture` target's canvas back: premultiplied linear-light RGBA, row by row.
fn read_linear(c: &Compositor, texture: &wgpu::Texture) -> Vec<[f32; 4]> {
    let (w, h) = (texture.width(), texture.height());
    let row = (w * 8).div_ceil(256) * 256;
    let device = c.context().device();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test readback"),
        size: u64::from(row) * u64::from(h),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        texture.size(),
    );
    c.context().queue().submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let view = buffer.get_mapped_range(..).expect("mapped");
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h as usize {
        let line = &view[y * row as usize..][..w as usize * 8];
        for px in line.as_chunks::<8>().0 {
            let c = |i: usize| half(u16::from_le_bytes([px[2 * i], px[2 * i + 1]]));
            out.push([c(0), c(1), c(2), c(3)]);
        }
    }
    drop(view);
    buffer.unmap();
    out
}

/// Renders `scene` to a `Texture` target with `options` and reads the linear canvas back.
fn gpu_linear(
    c: &mut Compositor,
    s: &SceneDescription,
    src: &SyntheticSource,
    (w, h): (u32, u32),
    options: &GraphOptions,
) -> Vec<[f32; 4]> {
    let graph = build(
        s,
        Target::Texture {
            width: w,
            height: h,
        },
        src,
        options,
    )
    .expect("graph");
    let f = c.execute(&graph).expect("render");
    let RenderedPixels::Texture(t) = &f.pixels else {
        panic!("texture target")
    };
    read_linear(c, t)
}

/// The largest per-channel difference between a GPU canvas and the CPU reference, with where.
fn max_linear_diff(gpu: &[[f32; 4]], cpu: &[[f64; 4]], width: u32) -> (f32, (u32, u32)) {
    assert_eq!(gpu.len(), cpu.len());
    let mut worst = (0.0f32, (0, 0));
    for (i, (g, r)) in gpu.iter().zip(cpu).enumerate() {
        for ch in 0..4 {
            let d = (g[ch] - r[ch] as f32).abs();
            if d > worst.0 || d.is_nan() {
                worst = (d, (i as u32 % width, i as u32 / width));
            }
        }
    }
    worst
}

fn background() -> Color {
    Color::rgb(30, 60, 90)
}

/// Gradient charts and bars: the sources of the blend-mode scenes.
fn blend_sources() -> SyntheticSource {
    let rgba = |w, h, pattern, alpha| TestCard {
        alpha,
        ..TestCard::new(
            w,
            h,
            pattern,
            crate::frame::PixelFormat::Rgba8,
            ColorTags::default(),
        )
    };
    SyntheticSource::new()
        .with(
            MediaId::from_u128(20),
            rgba(W, H, Pattern::Gradient, Alpha::Opaque),
        )
        .with(
            MediaId::from_u128(21),
            rgba(200, 120, Pattern::Gradient, Alpha::RampX),
        )
        .with(
            MediaId::from_u128(22),
            TestCard::new(
                192,
                108,
                Pattern::ColorBars,
                crate::frame::PixelFormat::Nv12,
                bt709(ColorRange::Limited),
            ),
        )
}

/// A blend layer over a gradient chart and bars: the top chart turned 90° (so the two
/// gradients cross), cropped, flipped, at 85% with an alpha ramp.
fn blend_scene(mode: BlendMode) -> SceneDescription {
    let bottom = layer(1, 20, (W, H));
    let mut bars = layer(2, 22, (192, 108));
    bars.transform.scale = Vec2::new(0.5, 0.5);
    bars.transform.position = Vec2::new(-0.25, 0.25);
    let mut top = layer(3, 21, (200, 120));
    top.transform.rotation_deg = 90.0;
    top.transform.scale = Vec2::new(1.4, 1.4);
    top.transform.position = Vec2::new(0.05, 0.0);
    top.opacity = 0.85;
    top.crop = Crop {
        left: 0.05,
        top: 0.0,
        right: 0.0,
        bottom: 0.1,
    };
    top.flip.horizontal = true;
    top.blend = mode;
    scene(W, H, background(), vec![bottom, bars, top])
}

/// The blend scene plus minified copies of the chart in the same mode (trilinear mips).
fn minified_blend_scene(mode: BlendMode) -> SceneDescription {
    let mut s = blend_scene(mode);
    for (n, (scale, x)) in [(0.22, 0.36), (0.13, -0.38)].into_iter().enumerate() {
        let mut small = layer(4 + n as u128, 21, (200, 120));
        small.transform.scale = Vec2::new(scale, scale);
        small.transform.position = Vec2::new(x, -0.36);
        small.transform.rotation_deg = 12.0;
        small.blend = mode;
        s.layers.push(small);
    }
    s
}

/// The blend-mode golden frame names, in `BlendMode::ALL` order.
const BLEND_NAMES: [&str; 8] = [
    "blend_normal",
    "blend_multiply",
    "blend_screen",
    "blend_overlay",
    "blend_darken",
    "blend_lighten",
    "blend_add",
    "blend_soft_light",
];

/// A gradient chart with a 90° display rotation tag, flipped, turned and cropped.
fn phone_scene(flip_h: bool, flip_v: bool, crop: Crop) -> SceneDescription {
    let mut l = layer(1, 30, (160, 90));
    l.request.rotation = 90;
    l.flip.horizontal = flip_h;
    l.flip.vertical = flip_v;
    l.transform.rotation_deg = 90.0;
    l.transform.scale = Vec2::new(0.8, 0.8);
    l.crop = crop;
    scene(W, H, background(), vec![l])
}

fn phone_source() -> SyntheticSource {
    SyntheticSource::new().with(
        MediaId::from_u128(30),
        TestCard::new(
            160,
            90,
            Pattern::Gradient,
            crate::frame::PixelFormat::Rgba8,
            ColorTags::default(),
        ),
    )
}

/// The 1 px grid moved right by `k` quarter pixels, unscaled (card = canvas size).
fn grid_scene(k: u32) -> SceneDescription {
    let mut l = layer(1, 40, (W, H));
    l.transform.position = Vec2::new(f64::from(k) * 0.25 / f64::from(W), 0.0);
    scene(W, H, Color::BLACK, vec![l])
}

fn grid_source() -> SyntheticSource {
    SyntheticSource::new().with(
        MediaId::from_u128(40),
        TestCard::new(
            W,
            H,
            Pattern::Grid { period: 4 },
            crate::frame::PixelFormat::Rgba8,
            ColorTags::default(),
        ),
    )
}

/// The golden scenes of WP-2C.04, rendered with the default (bilinear) options.
pub(super) fn golden_cases() -> Vec<(&'static str, SceneDescription, SyntheticSource)> {
    let mut out = Vec::new();
    for (mode, name) in BlendMode::ALL.into_iter().zip(BLEND_NAMES) {
        out.push((name, blend_scene(mode), blend_sources()));
    }
    let crop = Crop {
        left: 0.1,
        top: 0.05,
        right: 0.0,
        bottom: 0.2,
    };
    out.push((
        "flip_h_display_rotation_90",
        phone_scene(true, false, Crop::default()),
        phone_source(),
    ));
    out.push((
        "flip_hv_cropped_display_rotation_90",
        phone_scene(true, true, crop),
        phone_source(),
    ));
    for (k, name) in [
        "subpixel_grid_0_00",
        "subpixel_grid_0_25",
        "subpixel_grid_0_50",
        "subpixel_grid_0_75",
    ]
    .into_iter()
    .enumerate()
    {
        out.push((name, grid_scene(k as u32), grid_source()));
    }
    out
}

#[test]
fn blend_modes_match_the_analytic_reference_in_linear_light() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    let src = blend_sources();
    let options = GraphOptions::default();
    for mode in BlendMode::ALL {
        for (s, bound) in [
            (blend_scene(mode), LINEAR_MAX_ABS),
            (minified_blend_scene(mode), LINEAR_MAX_ABS_MIPS),
        ] {
            let gpu = gpu_linear(&mut c, &s, &src, (W, H), &options);
            let want = reference::render_linear(&s, (W, H), &src, &options).unwrap();
            let (worst, at) = max_linear_diff(&gpu, &want, W);
            eprintln!(
                "{mode:?} ({} layers): max |GPU − reference| = {worst:.2e} at {at:?} (linear)",
                s.layers.len()
            );
            assert!(
                worst <= bound,
                "{mode:?}: {worst:e} at {at:?}: GPU {:?}, reference {:?}",
                gpu[(at.1 * W + at.0) as usize],
                want[(at.1 * W + at.0) as usize]
            );
            // The encoded frame agrees too (§20 reference tolerance).
            let t = Target::rgba8(W, H);
            let img = render(&mut c, &s, &src, t);
            let cmp = compare(&img, &cpu(&s, &src, t), OutputTransfer::Srgb);
            assert!(
                cmp.passes_reference() && cmp.max_abs_diff <= 2,
                "{mode:?}: {cmp}"
            );
        }
    }
}

#[test]
fn blend_layers_leave_every_pixel_they_do_not_cover_bit_identical() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    let src = blend_sources();
    let below = scene(
        W,
        H,
        background(),
        blend_scene(BlendMode::Normal).layers[..2].to_vec(),
    );
    let base = render(&mut c, &below, &src, Target::rgba8(W, H));
    for mode in BlendMode::ALL {
        // The turned top layer's quad covers pixels its picture does not (the rotated
        // rectangle's corners): there the backdrop copy and the blend must change nothing.
        let mut s = blend_scene(mode);
        s.layers.truncate(3);
        let img = render(&mut c, &s, &src, Target::rgba8(W, H));
        let geo = LayerGeometry::new(&s.layers[2], (W, H), (W, H), (200, 120)).unwrap();
        let rect = geo.pixel_rect((W, H));
        let (mut outside, mut changed) = (0, 0);
        for y in 0..H {
            for x in 0..W {
                let p = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                let d = geo.edge_distances(apply_affine(&geo.from_output, p));
                if d.iter().any(|&d| d < -1.0) {
                    outside += 1;
                    let inside_quad =
                        (rect[0]..rect[2]).contains(&x) && (rect[1]..rect[3]).contains(&y);
                    if img.pixel(x, y) != base.pixel(x, y) {
                        changed += 1;
                        eprintln!("{mode:?} ({x}, {y}) in quad: {inside_quad}");
                    }
                }
            }
        }
        assert!(outside > 1000, "{outside}");
        assert_eq!(changed, 0, "{mode:?}");
        // And where it does cover, a non-normal mode differs from normal.
        if mode != BlendMode::Normal {
            let mut normal = s.clone();
            normal.layers[2].blend = BlendMode::Normal;
            assert_ne!(
                img,
                render(&mut c, &normal, &src, Target::rgba8(W, H)),
                "{mode:?}"
            );
        }
    }
}

#[test]
fn flip_and_the_display_rotation_tag_combine_like_the_reference() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    let src = phone_source();
    let crop = Crop {
        left: 0.1,
        top: 0.05,
        right: 0.0,
        bottom: 0.2,
    };
    for (h, v, crop) in [
        (false, false, Crop::default()),
        (true, false, Crop::default()),
        (false, true, crop),
        (true, true, crop),
    ] {
        let s = phone_scene(h, v, crop);
        for t in [Target::rgba8(W, H), Target::rgba8(W / 2 + 7, H / 2 + 3)] {
            let gpu = render(&mut c, &s, &src, t);
            let cmp = compare(&gpu, &cpu(&s, &src, t), OutputTransfer::Srgb);
            eprintln!("flip h={h} v={v} @ {:?}: {cmp}", t.size());
            assert!(cmp.passes_reference() && cmp.max_abs_diff <= 2, "{cmp}");
        }
    }
    // Orientation on the GPU image: the gradient's coded top-left (dark: red and green 0) and
    // bottom-right (red and green 1) land where the geometry puts them.
    let s = phone_scene(true, false, Crop::default());
    let img = render(&mut c, &s, &src, Target::rgba8(W, H));
    let geo = LayerGeometry::new(&s.layers[0], (W, H), (W, H), (160, 90)).unwrap();
    let probe = |item: [f64; 2]| {
        let p = apply_affine(&geo.to_output, item);
        img.pixel(p[0] as u32, p[1] as u32)
    };
    // The item is 90 × 160 display px; probe 8 px inside two opposite corners.
    let near_coded_origin = probe([8.0, 8.0]);
    let near_coded_end = probe([82.0, 152.0]);
    assert!(
        near_coded_origin[0] < 40 && near_coded_origin[1] < 40,
        "{near_coded_origin:?}"
    );
    assert!(
        near_coded_end[0] > 215 && near_coded_end[1] > 215,
        "{near_coded_end:?}"
    );
    // On screen, the flipped and turned item has its coded origin top-right.
    let o = apply_affine(&geo.to_output, [0.0, 0.0]);
    assert!(
        o[0] > f64::from(W) / 2.0 && o[1] < f64::from(H) / 2.0,
        "{o:?}"
    );
}

#[test]
fn subpixel_steps_move_a_1px_grid_smoothly_without_shimmer() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    let src = grid_source();
    let options = GraphOptions::default();
    let steps: Vec<Vec<[f32; 4]>> = (0..=8)
        .map(|k| {
            let s = grid_scene(k);
            let gpu = gpu_linear(&mut c, &s, &src, (W, H), &options);
            let want = reference::render_linear(&s, (W, H), &src, &options).unwrap();
            let (worst, at) = max_linear_diff(&gpu, &want, W);
            assert!(worst <= LINEAR_MAX_ABS, "step {k}: {worst:e} at {at:?}");
            gpu
        })
        .collect();
    // The interior (8 px margin; the window spans whole grid periods).
    let window = |x: u32, y: u32| (8..W - 8).contains(&x) && (8..H - 8).contains(&y);
    let at = |img: &[[f32; 4]], x: u32, y: u32| img[(y * W + x) as usize][0];
    let energy = |img: &[[f32; 4]]| -> f64 {
        let mut sum = 0.0;
        for y in 8..H - 8 {
            for x in 8..W - 8 {
                sum += f64::from(at(img, x, y));
            }
        }
        sum
    };
    // 1. The light in the window is the same at every step: nothing flickers as it moves.
    let e0 = energy(&steps[0]);
    for (k, img) in steps.iter().enumerate() {
        let e = energy(img);
        assert!(((e - e0) / e0).abs() < 1e-3, "step {k}: energy {e} vs {e0}");
    }
    // 2. A whole-pixel move is an exact shift.
    for y in 8..H - 8 {
        for x in 8..W - 8 {
            for whole in [4, 8] {
                let n = (whole / 4) as u32;
                assert_eq!(
                    at(&steps[whole], x + n, y),
                    at(&steps[0], x, y),
                    "({x}, {y}) moved {n} px"
                );
            }
        }
    }
    // 3. In between, each pixel changes linearly with the offset (bilinear is exact-weight):
    //    no pixel jumps or wobbles between quarter steps.
    let mut worst = 0.0f32;
    for y in 0..H {
        for x in 0..W {
            if !window(x, y) {
                continue;
            }
            for base in [0, 4] {
                let (a, b) = (at(&steps[base], x, y), at(&steps[base + 4], x, y));
                for q in 1..4 {
                    let want = a + (b - a) * (q as f32 / 4.0);
                    worst = worst.max((at(&steps[base + q], x, y) - want).abs());
                }
            }
        }
    }
    eprintln!("sub-pixel linearity: worst {worst:.2e}");
    assert!(worst <= 2e-3, "shimmer: {worst}");
}

#[test]
fn bicubic_matches_the_cpu_reference() {
    let _serial = serial();
    let Some(mut c) = compositor() else { return };
    let options = GraphOptions {
        filter: Filter::Bicubic,
        ..GraphOptions::default()
    };
    let src = blend_sources();
    let mut magnified = layer(1, 21, (200, 120));
    magnified.transform.scale = Vec2::new(2.2, 2.2);
    magnified.transform.rotation_deg = 7.0;
    let mut minified = layer(2, 20, (W, H));
    minified.transform.scale = Vec2::new(0.3, 0.3);
    minified.transform.position = Vec2::new(-0.3, 0.25);
    minified.blend = BlendMode::Screen;
    for (s, bound) in [
        (
            scene(W, H, background(), vec![magnified.clone()]),
            LINEAR_MAX_ABS,
        ),
        (
            scene(W, H, background(), vec![magnified, minified]),
            LINEAR_MAX_ABS_MIPS,
        ),
    ] {
        let gpu = gpu_linear(&mut c, &s, &src, (W, H), &options);
        let want = reference::render_linear(&s, (W, H), &src, &options).unwrap();
        let (worst, at) = max_linear_diff(&gpu, &want, W);
        eprintln!(
            "bicubic ({} layers): max |GPU − reference| = {worst:.2e} at {at:?}",
            s.layers.len()
        );
        assert!(worst <= bound, "{worst:e} at {at:?}");
        // It differs from bilinear (sharper), and bilinear stays the default.
        let bilinear = gpu_linear(&mut c, &s, &src, (W, H), &GraphOptions::default());
        assert_ne!(gpu, bilinear);
    }
}

// ---- preview equals export ------------------------------------------------------------------

const S: i64 = FLICKS_PER_SECOND;

fn video_media(id: MediaId, (w, h): (u32, u32)) -> MediaAsset {
    let mut probe = ProbeInfo::default();
    probe.streams.push(StreamInfo {
        index: 0,
        kind: StreamKind::Video,
        codec: None,
        profile: None,
        width: Some(w),
        height: Some(h),
        pixel_format: None,
        sample_rate: None,
        channels: None,
        language: None,
        extra: Extra::new(),
    });
    probe.duration = Some(Duration::from_secs(60).unwrap());
    MediaAsset::file(id, MediaKind::Video, format!("/media/{id}"), probe)
}

fn keyed<T: cutweave_model::AnimValue>(list: Vec<(i64, T)>) -> Animatable<T> {
    Animatable::Keyframes(
        Keyframes::new(
            list.into_iter()
                .map(|(at, v)| Keyframe::new(Time::from_flicks(at).unwrap(), v, Interp::Linear))
                .collect(),
        )
        .unwrap(),
    )
}

/// Three animated layers (transform, crop, opacity keys), flipped and blended.
fn animated_project() -> (Project, SequenceId) {
    let seq = SequenceId::from_u128(1);
    let format = SequenceFormat::new(W, H, FrameRate::FPS_24, SampleRate::HZ_48_000);
    let mut project = Project::new("wp-2c04");
    let mut sequence = Sequence::new(seq, "Main", format);
    let media = [
        (MediaId::from_u128(20), (W, H)),
        (MediaId::from_u128(22), (192, 108)),
        (MediaId::from_u128(21), (200, 120)),
    ];
    for (i, (id, size)) in media.into_iter().enumerate() {
        project.media.insert(id, video_media(id, size));
        let track = TrackId::from_u128(10 + i as u128);
        sequence.insert_track(
            0,
            Track::new(track, TrackKind::Video, format!("V{}", i + 1)),
        );
        let mut clip = Clip::new(
            ItemId::from_u128(100 + i as u128),
            id,
            Time::ZERO,
            Duration::from_secs(4).unwrap(),
        );
        let v = &mut clip.visual;
        match i {
            0 => {
                v.transform.scale = keyed(vec![(0, Vec2::ONE), (4 * S, Vec2::new(1.3, 1.3))]);
            }
            1 => {
                v.transform.position = keyed(vec![
                    (0, Vec2::new(-0.3, 0.2)),
                    (4 * S, Vec2::new(0.3, -0.2)),
                ]);
                v.transform.scale = Vec2::new(0.45, 0.45).into();
                v.transform.rotation = keyed(vec![(0, 0.0), (4 * S, 45.0)]);
                v.blend_mode = cutweave_model::BlendMode::Overlay;
                v.flip.vertical.set(true);
            }
            _ => {
                v.transform.rotation = keyed(vec![(0, 90.0), (4 * S, -30.0)]);
                v.crop.left = keyed(vec![(0, 0.0), (4 * S, 0.3)]);
                v.opacity = keyed(vec![(0, 1.0), (4 * S, 0.4)]);
                v.flip.horizontal.set(true);
                v.blend_mode = cutweave_model::BlendMode::SoftLight;
            }
        }
        sequence
            .track_mut(&track)
            .unwrap()
            .items
            .insert(clip.into());
    }
    project.sequences.insert(seq, sequence);
    (project, seq)
}

#[test]
fn preview_equals_export_bit_for_bit() {
    let _serial = serial();
    let Some(mut preview) = compositor() else {
        return;
    };
    let Some(mut export) = compositor() else {
        return;
    };
    let (project, seq) = animated_project();
    let src = blend_sources();
    let index = TimelineIndex::new(&project, seq).unwrap();
    // The preview's kept state (arch §10): one scene, graph and scratch reused every frame.
    let mut scene = index.evaluate(Time::ZERO);
    let mut graph = RenderGraph::new(Target::rgba8(1, 1));
    let mut scratch = BuildScratch::default();
    let target = Target::rgba8(W, H);
    for frame in [0, 7, 30, 61, 95] {
        let t = Time::from_flicks(frame * S / 24).unwrap();
        // Preview: indexed evaluation, full quality, kept graph, asynchronous readback.
        index.evaluate_into(t, &mut scene);
        Quality::FULL.apply(&mut scene);
        build_into(
            &mut graph,
            &mut scratch,
            &scene,
            target,
            &src,
            &GraphOptions::default(),
        )
        .unwrap();
        preview.submit_graph(&graph).unwrap();
        let shown: Image = preview
            .receive(|f, bytes| Image {
                width: f.width,
                height: f.height,
                data: bytes.to_vec(),
            })
            .unwrap()
            .expect("a frame");
        // Export: one-shot evaluation, export quality, a fresh graph, blocking readback on
        // another compositor.
        let mut exported_scene = evaluate(&project, seq, t).unwrap();
        Quality::EXPORT.apply(&mut exported_scene);
        assert_eq!(exported_scene, scene, "frame {frame}: the same scene");
        let g = build(&exported_scene, target, &src, &GraphOptions::default()).unwrap();
        let exported = export.execute(&g).unwrap().into_image().unwrap();
        assert!(
            shown == exported,
            "frame {frame}: preview and export differ: {}",
            compare(&shown, &exported, OutputTransfer::Srgb)
        );
        assert!(scene.layers.len() == 3, "{frame}: {:?}", scene.layers.len());
    }
}
