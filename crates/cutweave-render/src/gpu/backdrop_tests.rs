// SPDX-License-Identifier: GPL-3.0-or-later
//! GPU tests of WP-2B.02 (the background fill: colour, blur, image, transparent) against the
//! CPU reference, on the adapter `CUTWEAVE_GPU` selects (software by default), one at a time
//! like the rest of the GPU tests. The scene is the acceptance one: a 16:9 clip on a 9:16
//! canvas. The golden frames of the four kinds are the `background_*` golden projects
//! (`just golden`).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::unwrap_used,
    clippy::expect_used
)]

use cutweave_model::{
    BackgroundFill, BackgroundFit, BlurSource, Clip, Color, ColorRange, Duration, Extra, FrameRate,
    ItemId, MediaAsset, MediaId, MediaKind, ProbeInfo, Project, SampleRate, Sequence,
    SequenceFormat, SequenceId, StreamInfo, StreamKind, Time, Track, TrackId, TrackKind,
};

use super::RenderedPixels;
use super::tests::{H, W, bt709, compositor, cpu, layer, render, scene, serial};
use crate::color::OutputTransfer;
use crate::frame::PixelFormat;
use crate::graph::{BuildScratch, GraphOptions, PassOp, RenderGraph, Target, build, build_into};
use crate::image::Image;
use crate::metrics::compare;
use crate::quality::Quality;
use crate::reference;
use crate::scene::{BackgroundNode, FrameRequest, SceneDescription};
use crate::testcard::{Alpha, Pattern, SyntheticSource, TestCard};
use crate::{TimelineIndex, evaluate};

/// The 9:16 canvas.
const PW: u32 = 216;
const PH: u32 = 384;
const CLIP: u128 = 20;
const STILL: u128 = 30;
const STILL_SIZE: (u32, u32) = (300, 200);

fn background() -> Color {
    Color::rgb(24, 48, 96)
}

fn sources() -> SyntheticSource {
    let clip = TestCard::new(
        W,
        H,
        Pattern::Sweep,
        PixelFormat::Nv12,
        bt709(ColorRange::Limited),
    );
    let mut still = TestCard::new(
        STILL_SIZE.0,
        STILL_SIZE.1,
        Pattern::Gradient,
        PixelFormat::Rgba8,
        bt709(ColorRange::Full),
    );
    still.alpha = Alpha::RampX;
    SyntheticSource::new()
        .with(MediaId::from_u128(CLIP), clip)
        .with(MediaId::from_u128(STILL), still)
}

fn still_request() -> FrameRequest {
    let mut request = layer(1, STILL, STILL_SIZE).request;
    request.still = true;
    request
}

/// The 16:9 clip aspect-fitted on the 9:16 canvas, over `node`.
fn portrait(node: Option<BackgroundNode>) -> SceneDescription {
    let mut s = scene(PW, PH, background(), vec![layer(1, CLIP, (W, H))]);
    s.background_node = node;
    s
}

fn blur(radius: f64, dim: f64) -> Option<BackgroundNode> {
    Some(BackgroundNode::Blur {
        layer: 0,
        radius,
        dim,
    })
}

fn image(fit: BackgroundFit) -> Option<BackgroundNode> {
    Some(BackgroundNode::Image {
        request: still_request(),
        fit,
    })
}

fn cases() -> Vec<(&'static str, SceneDescription)> {
    vec![
        ("color", portrait(None)),
        ("blur", portrait(blur(0.05, 0.25))),
        ("blur_wide_no_dim", portrait(blur(0.3, 0.0))),
        ("blur_radius_0", portrait(blur(0.0, 0.5))),
        ("image_cover", portrait(image(BackgroundFit::Cover))),
        ("image_contain", portrait(image(BackgroundFit::Contain))),
        ("image_stretch", portrait(image(BackgroundFit::Stretch))),
        ("transparent", portrait(Some(BackgroundNode::Transparent))),
    ]
}

#[test]
fn every_background_matches_the_cpu_reference() {
    let _serial = serial();
    let Some(mut c) = compositor() else {
        return;
    };
    let src = sources();
    for (name, s) in cases() {
        for target in [Target::rgba8(PW, PH), Target::rgba8(PW / 2, PH / 2)] {
            let gpu = render(&mut c, &s, &src, target);
            let reference = cpu(&s, &src, target);
            let cmp = compare(&gpu, &reference, OutputTransfer::Srgb);
            // The WP-1F.02 bound of GPU against the float reference; a dithered frame may also
            // sit one code off where the two round a value to different sides of a threshold.
            assert!(
                cmp.psnr_db > 45.0 && cmp.max_abs_diff <= 2,
                "{name} at {:?}: {cmp}",
                target.size()
            );
        }
    }
}

#[test]
fn the_fills_draw_what_they_say() {
    let _serial = serial();
    let Some(mut c) = compositor() else {
        return;
    };
    let src = sources();
    let target = Target::rgba8(PW, PH);
    let frame = |c: &mut super::Compositor, node| render(c, &portrait(node), &src, target);
    let plain = frame(&mut c, None);
    let corner = (3, 3);
    let centre = (PW / 2, PH / 2);
    assert_eq!(plain.pixel(corner.0, corner.1), [24, 48, 96, 255]);
    // The clip (a band 216 px wide, 121.5 px tall, centred) is the same over every fill.
    for (name, s) in cases() {
        let img = render(&mut c, &s, &src, target);
        let (a, b) = (
            img.pixel(centre.0, centre.1),
            plain.pixel(centre.0, centre.1),
        );
        let near = a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 1);
        assert!(near, "{name}: the clip changed: {a:?} != {b:?}");
    }
    // Transparent: nothing outside the clip, not even the colour.
    let clear = frame(&mut c, Some(BackgroundNode::Transparent));
    assert_eq!(clear.pixel(corner.0, corner.1), [0, 0, 0, 0]);
    assert_eq!(clear.pixel(centre.0, centre.1)[3], 255);
    // Blur: the bars are filled with the picture, darker with more dim.
    let lit = frame(&mut c, blur(0.05, 0.0));
    let dimmed = frame(&mut c, blur(0.05, 0.6));
    let luma = |p: [u8; 4]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
    assert_ne!(
        lit.pixel(corner.0, corner.1),
        plain.pixel(corner.0, corner.1)
    );
    assert_eq!(lit.pixel(corner.0, corner.1)[3], 255);
    assert!(
        luma(dimmed.pixel(PW / 2, 20)) < luma(lit.pixel(PW / 2, 20)),
        "dim darkens the blur"
    );
    // A dim of 1 leaves black.
    let black = frame(&mut c, blur(0.05, 1.0));
    assert!(
        luma(black.pixel(PW / 2, 20)) <= 3,
        "{:?}",
        black.pixel(PW / 2, 20)
    );
    // A stretched image reaches every corner; a contained one leaves the colour above it.
    let stretched = frame(&mut c, image(BackgroundFit::Stretch));
    assert_ne!(
        stretched.pixel(PW - 2, 2),
        plain.pixel(PW - 2, 2),
        "the opaque side of the image reaches the corner"
    );
    let contained = frame(&mut c, image(BackgroundFit::Contain));
    assert_eq!(contained.pixel(PW - 2, 2), plain.pixel(PW - 2, 2));
}

#[test]
fn a_blur_under_a_full_cover_changes_nothing_and_costs_nothing() {
    let _serial = serial();
    let Some(mut c) = compositor() else {
        return;
    };
    let src = sources();
    // The same 16:9 clip on a 16:9 canvas hides its own blur.
    let mut plain = scene(W, H, background(), vec![layer(1, CLIP, (W, H))]);
    let target = Target::rgba8(W, H);
    let before = render(&mut c, &plain, &src, target);
    let reference_before = cpu(&plain, &src, target);
    plain.background_node = blur(0.05, 0.25);
    let graph = build(&plain, target, &src, &GraphOptions::default()).unwrap();
    assert!(
        !graph.passes.iter().any(|p| matches!(p.op, PassOp::Blur(_))),
        "{}",
        graph.describe()
    );
    let after = c.execute(&graph).unwrap().into_image().unwrap();
    assert!(before == after, "a covered blur must not move a pixel");
    assert!(
        reference_before == cpu(&plain, &src, target),
        "nor in the reference"
    );
}

#[test]
fn a_blurred_frame_is_the_same_cold_warm_and_on_another_compositor() {
    let _serial = serial();
    let (Some(mut a), Some(mut b)) = (compositor(), compositor()) else {
        return;
    };
    let src = sources();
    let s = portrait(blur(0.05, 0.25));
    let target = Target::rgba8(PW, PH);
    let cold = render(&mut a, &s, &src, target);
    let warm = render(&mut a, &s, &src, target);
    // Other sizes in between recycle the pooled textures.
    let _ = render(&mut a, &s, &src, Target::rgba8(PW / 2, PH / 2));
    let _ = render(&mut a, &portrait(image(BackgroundFit::Cover)), &src, target);
    let again = render(&mut a, &s, &src, target);
    let other = render(&mut b, &s, &src, target);
    assert!(cold == warm && warm == again && again == other);
    // The linear canvas (a texture target) carries the same blur, undithered.
    let graph = build(
        &s,
        Target::Texture {
            width: PW,
            height: PH,
        },
        &src,
        &GraphOptions::default(),
    )
    .unwrap();
    let frame = a.execute(&graph).unwrap();
    assert!(matches!(frame.pixels, RenderedPixels::Texture(_)));
}

#[test]
fn the_blur_looks_the_same_at_every_preview_scale() {
    let _serial = serial();
    let Some(mut c) = compositor() else {
        return;
    };
    let src = sources();
    let s = portrait(blur(0.08, 0.0));
    let full = render(&mut c, &s, &src, Target::rgba8(PW, PH));
    let half = render(&mut c, &s, &src, Target::rgba8(PW / 2, PH / 2));
    // The top bar (above the clip) of the half-size frame is the full-size one, halved.
    let mut worst = 0;
    for y in 4..40 {
        for x in 4..(PW / 2 - 4) {
            let h = half.pixel(x, y);
            let f = full.pixel(2 * x, 2 * y);
            worst = worst.max(h.iter().zip(&f).map(|(a, b)| a.abs_diff(*b)).max().unwrap());
        }
    }
    assert!(
        worst <= 6,
        "the blur changed with the preview scale: {worst}"
    );
}

fn media(id: MediaId, kind: MediaKind, (w, h): (u32, u32)) -> MediaAsset {
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
    MediaAsset::file(id, kind, format!("/media/{id}"), probe)
}

/// The acceptance project: the 16:9 clip on a 9:16 sequence with `fill`.
fn project(fill: Option<BackgroundFill>) -> (Project, SequenceId) {
    let seq = SequenceId::from_u128(1);
    let mut format = SequenceFormat::new(PW, PH, FrameRate::FPS_24, SampleRate::HZ_48_000);
    format.background = background();
    format.background_fill = fill;
    let mut project = Project::new("wp-2b02");
    let mut sequence = Sequence::new(seq, "Main", format);
    let (clip, still) = (MediaId::from_u128(CLIP), MediaId::from_u128(STILL));
    project
        .media
        .insert(clip, media(clip, MediaKind::Video, (W, H)));
    project
        .media
        .insert(still, media(still, MediaKind::Image, STILL_SIZE));
    let track = TrackId::from_u128(10);
    sequence.insert_track(0, Track::new(track, TrackKind::Video, "V1"));
    sequence.track_mut(&track).unwrap().items.insert(
        Clip::new(
            ItemId::from_u128(100),
            clip,
            Time::ZERO,
            Duration::from_secs(4).unwrap(),
        )
        .into(),
    );
    project.sequences.insert(seq, sequence);
    (project, seq)
}

#[test]
fn preview_equals_export_bit_for_bit_with_every_fill() {
    let _serial = serial();
    let (Some(mut preview), Some(mut export)) = (compositor(), compositor()) else {
        return;
    };
    let src = sources();
    let fills = [
        None,
        Some(BackgroundFill::Blur {
            source: BlurSource::Below,
            radius: 0.05,
            dim: 0.25,
        }),
        Some(BackgroundFill::Blur {
            source: BlurSource::MainTrack,
            radius: 0.2,
            dim: 0.0,
        }),
        Some(BackgroundFill::Image {
            media: MediaId::from_u128(STILL),
            fit: BackgroundFit::Cover,
        }),
        Some(BackgroundFill::Transparent),
    ];
    let target = Target::rgba8(PW, PH);
    let mut graph = RenderGraph::new(Target::rgba8(1, 1));
    let mut scratch = BuildScratch::default();
    for fill in fills {
        let (project, seq) = project(fill.clone());
        let index = TimelineIndex::new(&project, seq).unwrap();
        let mut scene = index.evaluate(Time::ZERO);
        // Frame 120 is past the clip: the fill has nothing to blur there.
        for frame in [0_i64, 30, 120] {
            let t = Time::from_flicks(frame * cutweave_model::FLICKS_PER_SECOND / 24).unwrap();
            // Preview: indexed evaluation, kept graph, asynchronous readback.
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
            // Export: one-shot evaluation, a fresh graph, blocking readback, another device.
            let mut exported_scene = evaluate(&project, seq, t).unwrap();
            Quality::EXPORT.apply(&mut exported_scene);
            assert_eq!(
                exported_scene, scene,
                "{fill:?} frame {frame}: the same scene"
            );
            let g = build(&exported_scene, target, &src, &GraphOptions::default()).unwrap();
            let exported = export.execute(&g).unwrap().into_image().unwrap();
            assert!(
                shown == exported,
                "{fill:?} frame {frame}: preview and export differ: {}",
                compare(&shown, &exported, OutputTransfer::Srgb)
            );
            let twice = export.execute(&g).unwrap().into_image().unwrap();
            assert!(
                exported == twice,
                "{fill:?} frame {frame}: not deterministic"
            );
            let reference =
                reference::render(&scene, target, &src, &GraphOptions::default()).unwrap();
            let cmp = compare(&shown, &reference, OutputTransfer::Srgb);
            assert!(cmp.psnr_db > 45.0, "{fill:?} frame {frame}: {cmp}");
        }
    }
}
