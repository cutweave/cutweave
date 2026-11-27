// SPDX-License-Identifier: GPL-3.0-or-later
//! The render bench (WP-2C.04; arch §19's playback row, gated on the reference iGPU in WP-2J.03):
//! three 1080p video layers with keyframed transforms, evaluated, compiled and composited per
//! frame at 60 fps timeline steps, read back as the preview's RGBA8 frame.
//!
//! - `composite/3_layers`: a full-frame layer zooming, a picture-in-picture moving and turning,
//!   and a flipped one turning and fading; decoded pictures cached (stills): the compositing
//!   cost.
//! - `composite/3_layers_screen`: the same with the top layer in screen mode (a backdrop copy
//!   and a pass split).
//! - `composite/3_layers_uploads`: animated sources, so every frame uploads and decodes three
//!   1080p NV12 pictures (cycling through 8 pre-generated ones).
//!
//! - `background@…/portrait_color` and `portrait_blur` (WP-2B.02): one 1080p clip on a
//!   1080 × 1920 sequence over the plain colour, then over a blurred copy of the picture (5% of
//!   the height). Their difference is the cost of the blur: the small-canvas draw, the two
//!   Gaussian passes and the upsample (budget < 1.5 ms on the iGPU, gated in 2J).
//!
//! Each iteration is one frame: evaluate (`TimelineIndex::evaluate_into`) → `build_into` →
//! GPU execution → blocking readback, so the time is an upper bound of the GPU time per frame
//! (the budget is ≤ 8 ms on the iGPU for the 16.7 ms frame of 1080p60). The adapter is
//! `CUTWEAVE_GPU` (default `integrated`, else whatever exists).
//!
//! `cargo bench -p cutweave-render --bench composite`

#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration as StdDuration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use cutweave_model::{
    Animatable, BackgroundFill, BlendMode, BlurSource, Clip, ColorPrimaries, ColorRange, ColorTags,
    Duration, Extra, FLICKS_PER_SECOND, FrameRate, Interp, ItemId, Keyframe, Keyframes,
    MatrixCoefficients, MediaAsset, MediaId, MediaKind, ProbeInfo, Project, SampleRate, Sequence,
    SequenceFormat, SequenceId, StreamInfo, StreamKind, Time, Track, TrackId, TrackKind,
    TransferCharacteristics, Vec2,
};
use cutweave_render::frame::PixelFormat;
use cutweave_render::gpu::{AdapterChoice, Compositor, CompositorOptions, GpuContext};
use cutweave_render::graph::{BuildScratch, GraphOptions, RenderGraph, Target, build_into};
use cutweave_render::testcard::{Pattern, SyntheticSource, TestCard};
use cutweave_render::{Quality, SceneDescription, TimelineIndex};

const W: u32 = 1920;
const H: u32 = 1080;
const S: i64 = FLICKS_PER_SECOND;
/// Distinct source pictures per layer when sources are animated.
const CYCLE: i64 = 8;

fn time(flicks: i64) -> Time {
    Time::from_flicks(flicks).unwrap()
}

fn keyed<T: cutweave_model::AnimValue>(list: Vec<(i64, T)>) -> Animatable<T> {
    Animatable::Keyframes(
        Keyframes::new(
            list.into_iter()
                .map(|(at, v)| Keyframe::new(time(at), v, Interp::Linear))
                .collect(),
        )
        .unwrap(),
    )
}

fn media(id: MediaId) -> MediaAsset {
    let mut probe = ProbeInfo::default();
    probe.streams.push(StreamInfo {
        index: 0,
        kind: StreamKind::Video,
        codec: None,
        profile: None,
        width: Some(W),
        height: Some(H),
        pixel_format: None,
        sample_rate: None,
        channels: None,
        language: None,
        extra: Extra::new(),
    });
    probe.duration = Some(Duration::from_secs(600).unwrap());
    MediaAsset::file(id, MediaKind::Video, format!("/bench/{id}"), probe)
}

/// Three 1080p clips over 10 s with animated transforms; the top one in `top_blend`.
fn project(top_blend: BlendMode) -> (Project, SequenceId) {
    let seq = SequenceId::from_u128(1);
    let format = SequenceFormat::new(W, H, FrameRate::FPS_60, SampleRate::HZ_48_000);
    let mut project = Project::new("render bench");
    let mut sequence = Sequence::new(seq, "Main", format);
    for i in 0..3u128 {
        let id = MediaId::from_u128(10 + i);
        project.media.insert(id, media(id));
        let track = TrackId::from_u128(20 + i);
        sequence.insert_track(0, Track::new(track, TrackKind::Video, format!("V{i}")));
        let mut clip = Clip::new(
            ItemId::from_u128(30 + i),
            id,
            Time::ZERO,
            Duration::from_secs(10).unwrap(),
        );
        let v = &mut clip.visual;
        match i {
            0 => v.transform.scale = keyed(vec![(0, Vec2::ONE), (10 * S, Vec2::new(1.25, 1.25))]),
            1 => {
                v.transform.scale = Vec2::new(0.45, 0.45).into();
                v.transform.position = keyed(vec![
                    (0, Vec2::new(-0.25, 0.2)),
                    (10 * S, Vec2::new(0.25, -0.2)),
                ]);
                v.transform.rotation = keyed(vec![(0, -10.0), (10 * S, 30.0)]);
            }
            _ => {
                v.transform.scale = Vec2::new(0.35, 0.35).into();
                v.transform.position = keyed(vec![
                    (0, Vec2::new(0.3, 0.3)),
                    (10 * S, Vec2::new(-0.3, 0.25)),
                ]);
                v.transform.rotation = keyed(vec![(0, 0.0), (10 * S, 360.0)]);
                v.opacity = keyed(vec![(0, 1.0), (10 * S, 0.6)]);
                v.flip.horizontal.set(true);
                v.blend_mode = top_blend;
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

fn sources(animated: bool) -> SyntheticSource {
    let tags = ColorTags {
        primaries: Some(ColorPrimaries::Bt709),
        transfer: Some(TransferCharacteristics::Bt709),
        matrix: Some(MatrixCoefficients::Bt709),
        range: Some(ColorRange::Limited),
    };
    let card = |pattern| TestCard {
        animated,
        ..TestCard::new(W, H, pattern, PixelFormat::Nv12, tags.clone())
    };
    SyntheticSource::new()
        .with(MediaId::from_u128(10), card(Pattern::ColorBars))
        .with(MediaId::from_u128(11), card(Pattern::Sweep))
        .with(MediaId::from_u128(12), card(Pattern::Gradient))
}

/// The preview's per-frame state, kept across frames as on the render thread.
struct Player {
    index: TimelineIndex,
    scene: SceneDescription,
    graph: RenderGraph,
    scratch: BuildScratch,
    source: SyntheticSource,
    target: Target,
    frame: i64,
}

impl Player {
    fn new(blend: BlendMode, animated: bool) -> Player {
        let (project, seq) = project(blend);
        Player::of(&project, seq, animated, Target::rgba8(W, H))
    }

    fn of(project: &Project, seq: SequenceId, animated: bool, target: Target) -> Player {
        let index = TimelineIndex::new(project, seq).unwrap();
        let scene = index.evaluate(Time::ZERO);
        Player {
            index,
            scene,
            graph: RenderGraph::new(Target::rgba8(1, 1)),
            scratch: BuildScratch::default(),
            source: sources(animated),
            target,
            frame: 0,
        }
    }

    /// Renders the next 60 fps frame (wrapping at 10 s); returns how long it took.
    fn frame(&mut self, c: &mut Compositor) -> StdDuration {
        let t = time((self.frame % 600) * S / 60);
        let pick = (self.frame % CYCLE) * S / 25;
        self.frame += 1;
        let started = Instant::now();
        self.index.evaluate_into(t, &mut self.scene);
        Quality::FULL.apply(&mut self.scene);
        for layer in &mut self.scene.layers {
            // Animated cards show frame `source_time · 25`: cycle through a few pictures.
            layer.request.source_time = time(pick);
        }
        build_into(
            &mut self.graph,
            &mut self.scratch,
            &self.scene,
            self.target,
            &self.source,
            &GraphOptions::default(),
        )
        .unwrap();
        let ((), _) = c.execute_with(&self.graph, |_| ()).unwrap();
        started.elapsed()
    }
}

fn bench(cr: &mut Criterion) {
    let choice = AdapterChoice::from_env_or(AdapterChoice::Integrated);
    let ctx = GpuContext::new(&choice)
        .or_else(|_| GpuContext::new(&AdapterChoice::Auto))
        .expect("a GPU adapter");
    let key = ctx.info().key.clone();
    let mut c = Compositor::new(ctx, CompositorOptions::default()).expect("compositor");
    let mut g = cr.benchmark_group(format!("composite@{key}"));
    g.sample_size(30);
    g.measurement_time(StdDuration::from_secs(8));
    for (name, blend, animated) in [
        ("3_layers", BlendMode::Normal, false),
        ("3_layers_screen", BlendMode::Screen, false),
        ("3_layers_uploads", BlendMode::Normal, true),
    ] {
        let mut player = Player::new(blend, animated);
        // Generate every source picture and warm the caches before measuring.
        for _ in 0..2 * CYCLE {
            player.frame(&mut c);
        }
        g.bench_function(name, |b| {
            b.iter_custom(|iters| (0..iters).map(|_| player.frame(&mut c)).sum());
        });
    }
    g.finish();
}

/// One 1080p clip on a 1080 × 1920 sequence with `fill` behind it.
fn portrait(fill: Option<BackgroundFill>) -> (Project, SequenceId) {
    let seq = SequenceId::from_u128(1);
    let mut format = SequenceFormat::new(H, W, FrameRate::FPS_60, SampleRate::HZ_48_000);
    format.background_fill = fill;
    let mut project = Project::new("background bench");
    let mut sequence = Sequence::new(seq, "Main", format);
    let id = MediaId::from_u128(11);
    project.media.insert(id, media(id));
    let track = TrackId::from_u128(20);
    sequence.insert_track(0, Track::new(track, TrackKind::Video, "V1"));
    let clip = Clip::new(
        ItemId::from_u128(30),
        id,
        Time::ZERO,
        Duration::from_secs(10).unwrap(),
    );
    sequence
        .track_mut(&track)
        .unwrap()
        .items
        .insert(clip.into());
    project.sequences.insert(seq, sequence);
    (project, seq)
}

fn bench_background(cr: &mut Criterion) {
    let choice = AdapterChoice::from_env_or(AdapterChoice::Integrated);
    let ctx = GpuContext::new(&choice)
        .or_else(|_| GpuContext::new(&AdapterChoice::Auto))
        .expect("a GPU adapter");
    let key = ctx.info().key.clone();
    let mut c = Compositor::new(ctx, CompositorOptions::default()).expect("compositor");
    let mut g = cr.benchmark_group(format!("background@{key}"));
    g.sample_size(30);
    g.measurement_time(StdDuration::from_secs(8));
    let blur = BackgroundFill::Blur {
        source: BlurSource::Below,
        radius: 0.05,
        dim: 0.25,
    };
    for (name, fill) in [("portrait_color", None), ("portrait_blur", Some(blur))] {
        let (project, seq) = portrait(fill);
        let mut player = Player::of(&project, seq, false, Target::rgba8(H, W));
        for _ in 0..2 * CYCLE {
            player.frame(&mut c);
        }
        g.bench_function(name, |b| {
            b.iter_custom(|iters| (0..iters).map(|_| player.frame(&mut c)).sum());
        });
    }
    g.finish();
}

criterion_group!(benches, bench, bench_background);
criterion_main!(benches);
