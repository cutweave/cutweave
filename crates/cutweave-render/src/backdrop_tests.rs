// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2B.02 without a GPU: how a sequence's background fill becomes a scene node, the passes it
//! compiles to, and the CPU reference's pixels.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    clippy::needless_range_loop,
    clippy::similar_names,
    clippy::unwrap_used,
    clippy::expect_used
)]

use cutweave_model::{
    BackgroundFill, BackgroundFit, BlurSource, Color, MediaId, MediaKind, Project, SequenceId,
    TrackId,
};

use crate::color::{OutputTransfer, encode_output, srgb8_to_linear_premul};
use crate::frame::PixelFormat;
use crate::graph::{GraphOptions, PassOp, Target, build};
use crate::reference;
use crate::scene::{BackgroundNode, SceneDescription};
use crate::testcard::{Pattern, SyntheticSource, TestCard};
use crate::testkit::{BACKGROUND, GREEN, RED, bt709, sec, three_clips};
use crate::{TimelineIndex, evaluate};

fn set_fill(project: &mut Project, seq: SequenceId, fill: BackgroundFill) {
    let mut sequence = project.sequences.get(&seq).unwrap().clone();
    sequence.format.background_fill = Some(fill);
    project.sequences.insert(seq, sequence);
}

fn blur(source: BlurSource) -> BackgroundFill {
    BackgroundFill::Blur {
        source,
        radius: 0.05,
        dim: 0.25,
    }
}

/// The scene at `t`, checked to be the same through the index.
fn scene_at(project: &Project, seq: SequenceId, t: f64) -> SceneDescription {
    let scene = evaluate(project, seq, sec(t)).unwrap();
    let index = TimelineIndex::new(project, seq).unwrap();
    assert_eq!(index.evaluate(sec(t)), scene);
    let mut reused = index.evaluate(sec(0.0));
    index.evaluate_into(sec(t), &mut reused);
    assert_eq!(reused, scene);
    scene
}

#[test]
fn a_sequence_without_a_fill_has_no_node_and_serialises_as_before() {
    let f = three_clips(64, 36);
    let scene = scene_at(&f.project, f.seq, 3.0);
    assert_eq!(scene.background_node, None);
    let json = serde_json::to_value(&scene).unwrap();
    assert!(json.get("background_node").is_none());
}

#[test]
fn a_blur_names_the_lowest_layer_or_the_main_tracks() {
    let mut f = three_clips(64, 36);
    set_fill(&mut f.project, f.seq, blur(BlurSource::Below));
    // 3 s: A on V1 (bottom) and C on V2.
    let scene = scene_at(&f.project, f.seq, 3.0);
    assert_eq!(scene.layers.len(), 2);
    assert_eq!(
        scene.background_node,
        Some(BackgroundNode::Blur {
            layer: 0,
            radius: 0.05,
            dim: 0.25
        })
    );
    assert_eq!(scene.layers[0].item, f.a);
    // Past every clip there is nothing to blur: the colour shows.
    assert_eq!(scene_at(&f.project, f.seq, 9.0).background_node, None);

    // The main track is the magnetic one; without one, the lowest video track.
    set_fill(&mut f.project, f.seq, blur(BlurSource::MainTrack));
    let scene = scene_at(&f.project, f.seq, 3.0);
    assert!(matches!(
        scene.background_node,
        Some(BackgroundNode::Blur { layer: 0, .. })
    ));
    let mut sequence = f.project.sequences.get(&f.seq).unwrap().clone();
    sequence.track_mut(&f.v2).unwrap().magnetic = true;
    f.project.sequences.insert(f.seq, sequence);
    let scene = scene_at(&f.project, f.seq, 3.0);
    assert!(matches!(
        scene.background_node,
        Some(BackgroundNode::Blur { layer: 1, .. })
    ));
    assert_eq!(scene.layers[1].item, f.c);
    // At 1 s the main track (V2) shows nothing: no blur, though V1 has a picture.
    let scene = scene_at(&f.project, f.seq, 1.0);
    assert_eq!((scene.layers.len(), &scene.background_node), (1, &None));
    // `below` skips a hidden track like the layers do.
    set_fill(&mut f.project, f.seq, blur(BlurSource::Below));
    let mut sequence = f.project.sequences.get(&f.seq).unwrap().clone();
    let v1 = sequence
        .tracks
        .keys()
        .copied()
        .find(|t| *t != f.v2)
        .unwrap();
    sequence.track_mut(&v1).unwrap().hidden = true;
    f.project.sequences.insert(f.seq, sequence);
    let scene = scene_at(&f.project, f.seq, 3.0);
    assert_eq!(scene.layers[0].item, f.c);
    assert!(matches!(
        scene.background_node,
        Some(BackgroundNode::Blur { layer: 0, .. })
    ));
    let _: TrackId = v1;
}

#[test]
fn an_image_fill_needs_a_still_that_exists() {
    let mut f = three_clips(64, 36);
    let still = MediaId::from_u128(21);
    let image = BackgroundFill::Image {
        media: still,
        fit: BackgroundFit::Contain,
    };
    // Media 21 is a video: no node.
    set_fill(&mut f.project, f.seq, image.clone());
    assert_eq!(scene_at(&f.project, f.seq, 3.0).background_node, None);
    let mut asset = f.project.media.get(&still).unwrap().clone();
    asset.kind = MediaKind::Image;
    f.project.media.insert(still, asset);
    let scene = scene_at(&f.project, f.seq, 9.0);
    let Some(BackgroundNode::Image { request, fit }) = &scene.background_node else {
        panic!("an image node: {:?}", scene.background_node)
    };
    assert_eq!((request.media, *fit), (still, BackgroundFit::Contain));
    assert!(request.still && !request.proxy);
    assert_eq!(request.source_size, Some((64, 36)));
    // An asset that is gone draws nothing.
    set_fill(
        &mut f.project,
        f.seq,
        BackgroundFill::Image {
            media: MediaId::from_u128(999),
            fit: BackgroundFit::Cover,
        },
    );
    assert_eq!(scene_at(&f.project, f.seq, 3.0).background_node, None);
    set_fill(&mut f.project, f.seq, BackgroundFill::Transparent);
    assert_eq!(
        scene_at(&f.project, f.seq, 3.0).background_node,
        Some(BackgroundNode::Transparent)
    );
}

fn ops(scene: &SceneDescription, source: &SyntheticSource, target: Target) -> Vec<&'static str> {
    let graph = build(scene, target, source, &GraphOptions::default()).unwrap();
    let order = graph.topo_order().unwrap();
    order.iter().map(|&i| graph.passes[i].op.name()).collect()
}

/// A 16:9 solid card on a 9:16 canvas: bars above and below the clip.
fn portrait() -> (SceneDescription, SyntheticSource) {
    let mut f = three_clips(64, 36);
    let mut sequence = f.project.sequences.get(&f.seq).unwrap().clone();
    sequence.format.width = 36;
    sequence.format.height = 64;
    f.project.sequences.insert(f.seq, sequence);
    // 1 s: only A (red) shows.
    let scene = evaluate(&f.project, f.seq, sec(1.0)).unwrap();
    assert_eq!(scene.layers.len(), 1);
    let card = |signal| TestCard::new(64, 36, Pattern::Solid(signal), PixelFormat::Nv12, bt709());
    let source = SyntheticSource::new()
        .with(MediaId::from_u128(21), card(RED))
        .with(MediaId::from_u128(22), card(GREEN));
    (scene, source)
}

#[test]
fn the_fills_compile_to_the_documented_passes() {
    let (mut scene, source) = portrait();
    let target = Target::rgba8(36, 64);
    let plain = build(&scene, target, &source, &GraphOptions::default()).unwrap();
    assert_eq!(
        ops(&scene, &source, target),
        ["decode", "background", "composite", "output"]
    );

    scene.background_node = Some(BackgroundNode::Blur {
        layer: 0,
        radius: 0.1,
        dim: 0.5,
    });
    let graph = build(&scene, target, &source, &GraphOptions::default()).unwrap();
    assert_eq!(
        ops(&scene, &source, target),
        [
            "decode",
            "background",
            "background",
            "composite",
            "blur",
            "blur",
            "composite",
            "composite",
            "output"
        ]
    );
    // One decode serves the blur and the layer; the output dithers; the key changes.
    assert_eq!(graph.frames.len(), 1);
    assert_eq!(graph.stats.layers_visible, 1);
    assert!(
        graph
            .passes
            .iter()
            .any(|p| matches!(p.op, PassOp::Output { dither: true, .. }))
    );
    assert!(
        plain
            .passes
            .iter()
            .any(|p| matches!(p.op, PassOp::Output { dither: false, .. }))
    );
    assert_ne!(graph.output_key(), plain.output_key());
    // The blur passes: x then y, the dim on the second; the small canvas is the plan's.
    let blurs: Vec<_> = graph
        .passes
        .iter()
        .filter_map(|p| match p.op {
            PassOp::Blur(b) => Some((b, graph.resources[p.output])),
            _ => None,
        })
        .collect();
    let plan = crate::backdrop::blur_plan(0.1, (36, 64));
    assert_eq!(blurs.len(), 2);
    assert!(blurs[0].0.horizontal && !blurs[1].0.horizontal);
    assert_eq!((blurs[0].0.dim, blurs[1].0.dim), (0.0, 0.5));
    assert_eq!(
        (blurs[0].0.taps, blurs[1].0.taps),
        (plan.taps[0], plan.taps[1])
    );
    assert_eq!((blurs[1].1.width, blurs[1].1.height), plan.size);
    // The background's draws carry no item, so frame inspection sees only the clip.
    let (visible, hidden) = crate::inspect::frame_items(&scene, &graph);
    assert_eq!((visible.len(), hidden.len()), (1, 0));
    assert!(visible[0].rect.width <= 36);

    // Transparent starts from nothing; the colour is not in the graph.
    scene.background_node = Some(BackgroundNode::Transparent);
    let graph = build(&scene, target, &source, &GraphOptions::default()).unwrap();
    assert!(graph.passes.iter().any(|p| matches!(
        p.op,
        PassOp::Background { color } if color == [0.0; 4]
    )));

    // An image is one more composite at the bottom, and its frame one more decode.
    let mut request = scene.layers[0].request.clone();
    request.media = MediaId::from_u128(22);
    request.still = true;
    scene.background_node = Some(BackgroundNode::Image {
        request: request.clone(),
        fit: BackgroundFit::Cover,
    });
    assert_eq!(
        ops(&scene, &source, target),
        [
            "decode",
            "decode",
            "background",
            "composite",
            "composite",
            "output"
        ]
    );
    // One whose frame the source cannot give draws nothing, without failing the frame.
    request.media = MediaId::from_u128(404);
    scene.background_node = Some(BackgroundNode::Image {
        request,
        fit: BackgroundFit::Cover,
    });
    let graph = build(&scene, target, &source, &GraphOptions::default()).unwrap();
    assert_eq!(graph.frames.len(), 1);
    assert_eq!(graph.missing, Vec::new());
}

fn srgb_of(linear: [f64; 4]) -> [u8; 4] {
    encode_output(linear, OutputTransfer::Srgb)
}

#[test]
fn the_reference_fills_the_bars() {
    let (mut scene, source) = portrait();
    let target = Target::rgba8(36, 64);
    let options = GraphOptions::default();
    let plain = reference::render(&scene, target, &source, &options).unwrap();
    let bar = (18, 4);
    let clip = (18, 32);
    assert_eq!(plain.pixel(bar.0, bar.1)[..3], [16, 16, 16]);
    let red = plain.pixel(clip.0, clip.1);

    // A blur of a flat picture is that picture, whatever the radius: the bars turn red.
    scene.background_node = Some(BackgroundNode::Blur {
        layer: 0,
        radius: 0.1,
        dim: 0.0,
    });
    let linear = reference::render_linear(&scene, (36, 64), &source, &options).unwrap();
    let at = |p: (u32, u32)| linear[(p.1 * 36 + p.0) as usize];
    for c in 0..4 {
        assert!((at(bar)[c] - at(clip)[c]).abs() < 1e-9, "{:?}", at(bar));
    }
    let blurred = reference::render(&scene, target, &source, &options).unwrap();
    let close = |a: [u8; 4], b: [u8; 4]| a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 1);
    assert!(
        close(blurred.pixel(bar.0, bar.1), red),
        "dither stays within a code"
    );
    // The dim scales the light: half of it at 0.5.
    scene.background_node = Some(BackgroundNode::Blur {
        layer: 0,
        radius: 0.1,
        dim: 0.5,
    });
    let dimmed = reference::render_linear(&scene, (36, 64), &source, &options).unwrap();
    let d = dimmed[(bar.1 * 36 + bar.0) as usize];
    for c in 0..3 {
        assert!((d[c] - at(bar)[c] * 0.5).abs() < 1e-9);
    }
    assert!((d[3] - 1.0).abs() < 1e-12);
    // The clip itself is untouched by its background.
    assert!((dimmed[(clip.1 * 36 + clip.0) as usize][0] - at(clip)[0]).abs() < 1e-12);

    // Transparent: nothing in the bars.
    scene.background_node = Some(BackgroundNode::Transparent);
    let clear = reference::render(&scene, target, &source, &options).unwrap();
    assert_eq!(clear.pixel(bar.0, bar.1), [0, 0, 0, 0]);
    assert_eq!(clear.pixel(clip.0, clip.1), red);

    // An image (the green card) covering the canvas fills the bars with green.
    let mut request = scene.layers[0].request.clone();
    request.media = MediaId::from_u128(22);
    request.still = true;
    for fit in [BackgroundFit::Cover, BackgroundFit::Stretch] {
        scene.background_node = Some(BackgroundNode::Image {
            request: request.clone(),
            fit,
        });
        let filled = reference::render(&scene, target, &source, &options).unwrap();
        let green = filled.pixel(bar.0, bar.1);
        assert!(green[1] > 150 && green[0] < 120, "{fit:?}: {green:?}");
        assert_eq!(filled.pixel(clip.0, clip.1), red);
    }
    // Contained, a 16:9 image on 9:16 sits where the clip does: the bars keep the colour.
    scene.background_node = Some(BackgroundNode::Image {
        request,
        fit: BackgroundFit::Contain,
    });
    let contained = reference::render(&scene, target, &source, &options).unwrap();
    assert_eq!(
        contained.pixel(bar.0, bar.1),
        srgb_of(srgb8_to_linear_premul(BACKGROUND))
    );
    let _ = Color::BLACK;
}
