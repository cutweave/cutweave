// SPDX-License-Identifier: GPL-3.0-or-later
//! The preview's frame source makes no heap allocation of its own per steady-state frame (arch
//! §10 "Threads", AGENTS.md "Real-time and render threads"), on the real path: a document with
//! two clips, `FakeBackend` decoders behind [`MediaFrames`], the software GPU adapter, the
//! counting allocator of `cutweave-audio` (feature `testing`). The same holds with a media item
//! playing on its own (a source, ADR-0022) and when the preview switches between the sequence
//! and a source it has shown before.
//!
//! Heap operations are counted per [`Scope`]: our own code must make none; wgpu's are bounded
//! by [`WGPU_OPS_PER_FRAME`]; the media layer's (decoding a picture the preview does not hold,
//! the shared frame cache) are reported, and must be zero when the picture does not change.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use cutweave_audio::rt_alloc::{CountingAlloc, RtSection, Scope};
use cutweave_cache::FrameCache;
use cutweave_commands::{DocActor, DocConfig, Registry};
use cutweave_media::PixelFormat as MediaFormat;
use cutweave_media::testing::{FakeBackend, FakeMedia, FakeVideo};
use cutweave_model::{
    AnimValue, Animatable, BlendMode, Clip, Duration, Extra, FrameRate, Interp, ItemId, Keyframe,
    Keyframes, MediaAsset, MediaId, MediaKind, ProbeInfo, Project, SampleRate, Sequence,
    SequenceFormat, SequenceId, StreamInfo, StreamKind, Time, Track, TrackId, TrackKind, Vec2,
};
use cutweave_playback::shm::{FrameLayout, PixelFormat};
use cutweave_playback::transport::{
    CancelToken, FrameSource as _, PlaybackTarget, RenderPurpose, RenderRequest,
};
use cutweave_render::gpu::{AdapterChoice, Compositor, CompositorOptions, GpuContext};
use cutweave_render::{Quality, Scale};

use super::frames::MediaFrames;
use super::gpu::LateGpu;
use super::media::CurrentDoc;
use super::preview::{CompositorSlot, PreviewSource};
use crate::events::EventBus;
use crate::subsystems::SubsystemEvents;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// The most heap operations (allocations, frees, reallocations) wgpu (wgpu-core, wgpu-hal) may
/// make on the render thread for one two-layer preview frame: the command encoder and its
/// passes, the queue writes (planes and uniforms) and the submission, the readback's map
/// callback and the device poll. Texture views, bind groups and buffers are not among them:
/// the compositor keeps those. Measured per frame on WARP (D3D12): 502-508 playing, 171 for a
/// still whose textures are cached, up to 651 scrubbing (two output sizes); on Vulkan (NVIDIA):
/// 520-526, 179-181 and up to 669. The bound leaves room for other backends (lavapipe on Linux
/// CI) and wgpu updates, and catches a regression per layer or per pass.
const WGPU_OPS_PER_FRAME: u64 = 1_000;

const FRAME_30: i64 = 23_520_000;
const OUT: (u32, u32) = (320, 180);
/// Frames rendered before counting: the pools, caches and scratch buffers grow to the
/// timeline's needs, and the compositor's decoded-frame cache (32 MiB, about 14 frames of the
/// fixture) fills and its textures cycle once.
const WARM_UP: i64 = 60;
const COUNTED: usize = 40;

fn hash(n: u8) -> String {
    format!("blake3-fast:{}", format!("{n:02x}").repeat(32))
}

fn asset(id: MediaId, path: &str, size: (u32, u32), n: u8) -> MediaAsset {
    let mut probe = ProbeInfo::default();
    probe.streams.push(StreamInfo {
        index: 0,
        kind: StreamKind::Video,
        codec: None,
        profile: None,
        width: Some(size.0),
        height: Some(size.1),
        pixel_format: None,
        sample_rate: None,
        channels: None,
        language: None,
        extra: Extra::new(),
    });
    probe.duration = Some(Duration::from_secs(10).unwrap());
    let mut asset = MediaAsset::file(id, MediaKind::Video, path, probe);
    asset.content_hash = Some(hash(n));
    asset
}

/// Linear keys from `a` at 0 s to `b` at 10 s.
fn keys<T: AnimValue>(a: T, b: T) -> Animatable<T> {
    Animatable::Keyframes(
        Keyframes::new(vec![
            Keyframe::new(Time::ZERO, a, Interp::Linear),
            Keyframe::new(Time::from_secs(10).unwrap(), b, Interp::Linear),
        ])
        .unwrap(),
    )
}

/// A 640×360 @ 30 fps sequence: an NV12 clip filling the frame (V1) under a half-size 4:2:0
/// picture-in-picture (V2), both 10 s long; and the backend that decodes them. The
/// picture-in-picture moves and turns on keyframes, is flipped and blends in screen mode
/// (WP-2C.04): keyframe evaluation and the blend path's backdrop copy allocate nothing either.
fn fixture() -> (Project, FakeBackend) {
    let dir = std::env::temp_dir();
    let (pa, pb) = (
        dir.join("cutweave-preview-alloc-a.mp4")
            .to_string_lossy()
            .into_owned(),
        dir.join("cutweave-preview-alloc-b.mp4")
            .to_string_lossy()
            .into_owned(),
    );
    let ten = Duration::from_secs(10).unwrap();
    let backend = FakeBackend::new()
        .with_media(
            &pa,
            FakeMedia::video(
                FakeVideo::lasting(640, 360, FrameRate::FPS_30, ten).with_format(MediaFormat::Nv12),
            ),
        )
        .with_media(
            &pb,
            FakeMedia::video(FakeVideo::lasting(320, 180, FrameRate::FPS_30, ten)),
        );
    let (ma, mb) = (MediaId::from_u128(21), MediaId::from_u128(22));
    let mut project = Project::new("preview alloc");
    project.media.insert(ma, asset(ma, &pa, (640, 360), 1));
    project.media.insert(mb, asset(mb, &pb, (320, 180), 2));
    let seq = SequenceId::from_u128(1);
    let format = SequenceFormat::new(640, 360, FrameRate::FPS_30, SampleRate::HZ_48_000);
    let mut sequence = Sequence::new(seq, "Main", format);
    let (v1, v2) = (TrackId::from_u128(11), TrackId::from_u128(12));
    sequence.insert_track(0, Track::new(v1, TrackKind::Video, "V1"));
    sequence.insert_track(0, Track::new(v2, TrackKind::Video, "V2"));
    let mut pip = Clip::new(ItemId::from_u128(32), mb, Time::ZERO, ten);
    pip.visual.transform.scale = Animatable::Static(Vec2::new(0.5, 0.5));
    pip.visual.transform.position = keys(Vec2::new(0.25, -0.25), Vec2::new(-0.25, 0.25));
    pip.visual.transform.rotation = keys(0.0, 90.0);
    pip.visual.flip.horizontal.set(true);
    pip.visual.blend_mode = BlendMode::Screen;
    sequence
        .track_mut(&v1)
        .unwrap()
        .items
        .insert(Clip::new(ItemId::from_u128(31), ma, Time::ZERO, ten).into());
    sequence.track_mut(&v2).unwrap().items.insert(pip.into());
    project.sequences.insert(seq, sequence);
    (project, backend)
}

struct Rig {
    _actor: DocActor,
    source: PreviewSource,
    cancel: CancelToken,
    /// What the frames show: the sequence, or a media item on its own.
    target: PlaybackTarget,
}

/// The NV12 clip's media (V1), as a source.
const CLIP_A: PlaybackTarget = PlaybackTarget::Media(MediaId::from_u128(21));

/// Heap operations of one frame, per scope.
type Ops = [u64; Scope::COUNT];

const OWN: usize = Scope::Own as usize;
const WGPU: usize = Scope::Wgpu as usize;
const MEDIA: usize = Scope::Media as usize;

impl Rig {
    /// `None` (skip) without the GPU adapter: the software one (WARP, lavapipe) unless
    /// `CUTWEAVE_GPU` names another.
    fn new() -> Option<Rig> {
        // A 32 MiB decoded-frame cache, so the warm-up fills and cycles it.
        let options = CompositorOptions {
            cache_budget_bytes: 32 << 20,
            ..CompositorOptions::default()
        };
        Rig::with(fixture(), options)
    }

    /// [`Rig::new`] over another timeline and compositor options.
    fn with((project, backend): (Project, FakeBackend), options: CompositorOptions) -> Option<Rig> {
        let ctx = match GpuContext::new(&AdapterChoice::from_env_or(AdapterChoice::Software)) {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("skipping: no GPU adapter ({e})");
                return None;
            }
        };
        eprintln!("adapter: {}", ctx.info().key);
        let frame = Compositor::new(ctx.clone(), options.clone()).unwrap();
        let preview = Compositor::new(ctx.clone(), options).unwrap();
        let gpu = Arc::new(LateGpu::new());
        gpu.settle(Some((ctx, frame, preview)));
        let actor = DocActor::spawn(DocConfig::new(Arc::new(Registry::new()), project)).unwrap();
        let doc = actor.handle().clone();
        let current = CurrentDoc::default();
        current.set(Some(doc.clone()));
        let frames = Arc::new(MediaFrames::new(
            Some(Arc::new(backend)),
            current,
            Arc::new(FrameCache::new(256 << 20)),
            None,
            Arc::default(),
        ));
        let source = PreviewSource::new(
            doc,
            CompositorSlot::new(gpu),
            frames,
            SubsystemEvents(EventBus::new(16)),
            Arc::new(AtomicBool::new(false)),
        );
        Some(Rig {
            _actor: actor,
            source,
            cancel: CancelToken::never(),
            target: PlaybackTarget::Sequence,
        })
    }

    /// Renders `frames` at `quality` (output size scaled like the render loop's) and returns
    /// each frame's heap operations per scope.
    fn render(
        &mut self,
        frames: impl IntoIterator<Item = i64>,
        quality: Quality,
        purpose: RenderPurpose,
    ) -> Vec<Ops> {
        let (w, h) = quality.output_size(OUT);
        let layout = FrameLayout::aligned(w, h, PixelFormat::Rgba8, 256).unwrap();
        let mut out = vec![0u8; layout.len()];
        let mut ops = Vec::new();
        for frame in frames {
            let request = RenderRequest {
                target: self.target,
                frame,
                t: Time::from_flicks(frame * FRAME_30).unwrap(),
                quality,
                layout,
                purpose,
            };
            let section = RtSection::enter();
            let rendered = self.source.render(&request, &self.cancel, &mut out);
            ops.push(section.exit_by_scope());
            rendered.unwrap();
        }
        // The picture is the fixture's, not black.
        assert!(out.as_chunks::<4>().0.iter().any(|px| px[..3] != [0, 0, 0]));
        ops
    }
}

/// Asserts what `ops` may contain and returns the most wgpu and media operations of a frame.
fn check(what: &str, ops: &[Ops]) -> (u64, u64) {
    for (i, o) in ops.iter().enumerate() {
        assert_eq!(
            o[OWN], 0,
            "{what}: frame {i} made {} heap operations of its own ({o:?} own/wgpu/media)",
            o[OWN]
        );
        assert!(
            o[WGPU] <= WGPU_OPS_PER_FRAME,
            "{what}: frame {i} made {} heap operations inside wgpu (bound {WGPU_OPS_PER_FRAME})",
            o[WGPU]
        );
    }
    let max = |scope: usize| ops.iter().map(|o| o[scope]).max().unwrap_or(0);
    let min = |scope: usize| ops.iter().map(|o| o[scope]).min().unwrap_or(0);
    let total = |scope: usize| ops.iter().map(|o| o[scope]).sum::<u64>();
    eprintln!(
        "{what} ({} frames), heap operations per frame: own 0; wgpu {}..={} (total {}); \
         media {}..={} (total {})",
        ops.len(),
        min(WGPU),
        max(WGPU),
        total(WGPU),
        min(MEDIA),
        max(MEDIA),
        total(MEDIA),
    );
    (max(WGPU), max(MEDIA))
}

#[test]
fn preview_frames_make_no_heap_allocation_of_their_own() {
    let Some(mut rig) = Rig::new() else { return };
    let full = Quality::FULL;
    let playback = RenderPurpose::Playback;
    rig.render(0..WARM_UP, full, playback);

    // Playback: a new picture from each clip every frame (decoded by the media layer).
    let n = i64::try_from(COUNTED).unwrap();
    let played = rig.render(WARM_UP..WARM_UP + n, full, playback);
    let (_, media) = check("playback", &played);
    assert!(media > 0, "new pictures come from the media layer");

    // Playing the same stretch again: the pictures come from the frame cache.
    check("replay", &rig.render(WARM_UP..WARM_UP + n, full, playback));

    // A paused frame rendered again (a refresh, a quality refine, a step back and forth): the
    // preview holds its pictures, the GPU its decoded textures.
    let paused = WARM_UP + 7;
    rig.render([paused], full, RenderPurpose::Still);
    let still = rig.render(
        std::iter::repeat_n(paused, COUNTED),
        full,
        RenderPurpose::Still,
    );
    let (_, media) = check("still", &still);
    assert_eq!(media, 0, "a picture the preview holds needs no media layer");

    // Scrubbing: a quarter-resolution answer, then the full refine, frame after frame.
    let quarter = Quality::new(Scale::Quarter, false);
    rig.render([WARM_UP, WARM_UP + 1], quarter, RenderPurpose::Scrub);
    let mut scrub = Vec::new();
    for f in (WARM_UP..WARM_UP + n).step_by(6) {
        scrub.extend(rig.render([f], quarter, RenderPurpose::Scrub));
        scrub.extend(rig.render([f], full, RenderPurpose::Still));
    }
    check("scrub", &scrub);
}

/// WP-1H.04: the same with the V1 clip's media playing on its own (ADR-0022). Its transient
/// timeline is indexed on the first frame after the switch (the warm-up), like the sequence
/// after an edit; from then on its frames and the switches between it and the sequence make no
/// heap allocation of our own.
#[test]
fn source_frames_make_no_heap_allocation_of_their_own() {
    let Some(mut rig) = Rig::new() else { return };
    let full = Quality::FULL;
    let playback = RenderPurpose::Playback;
    rig.target = CLIP_A;
    rig.render(0..WARM_UP, full, playback);

    let n = i64::try_from(COUNTED).unwrap();
    let played = rig.render(WARM_UP..WARM_UP + n, full, playback);
    let (_, media) = check("source playback", &played);
    assert!(media > 0, "new pictures come from the media layer");
    check(
        "source replay",
        &rig.render(WARM_UP..WARM_UP + n, full, playback),
    );

    let paused = WARM_UP + 7;
    rig.render([paused], full, RenderPurpose::Still);
    let still = rig.render(
        std::iter::repeat_n(paused, COUNTED),
        full,
        RenderPurpose::Still,
    );
    let (_, media) = check("source still", &still);
    assert_eq!(media, 0, "a picture the preview holds needs no media layer");

    let quarter = Quality::new(Scale::Quarter, false);
    rig.render([WARM_UP, WARM_UP + 1], quarter, RenderPurpose::Scrub);
    let mut scrub = Vec::new();
    for f in (WARM_UP..WARM_UP + n).step_by(6) {
        scrub.extend(rig.render([f], quarter, RenderPurpose::Scrub));
        scrub.extend(rig.render([f], full, RenderPurpose::Still));
    }
    check("source scrub", &scrub);

    // Back and forth between the sequence (indexed once, here) and the source: both stay
    // indexed, so a switch is a frame like any other.
    rig.target = PlaybackTarget::Sequence;
    rig.render([paused, paused + 1], full, RenderPurpose::Still);
    let mut switches = Vec::new();
    for i in 0..COUNTED {
        rig.target = if i % 2 == 0 {
            CLIP_A
        } else {
            PlaybackTarget::Sequence
        };
        switches.extend(rig.render([paused], full, RenderPurpose::Still));
    }
    check("switches", &switches);
}

/// WP-2D.01b: playing backwards through the clips' long GOPs (the fake's keyframe every 12
/// frames), each GOP is read back once by the media layer; the frames between come out of the
/// read-back with no heap operation at all, of ours or of the media layer.
#[test]
fn reverse_playback_frames_come_from_the_read_back_without_heap_allocation() {
    let Some(mut rig) = Rig::new() else { return };
    let full = Quality::FULL;
    let playback = RenderPurpose::Playback;
    rig.render(0..WARM_UP, full, playback);

    // Backwards over a stretch not played before (the frame cache does not hold it).
    let n = i64::try_from(COUNTED).unwrap();
    let start = 4 * WARM_UP + n;
    rig.render([start], full, playback);
    let reversed = rig.render((start - n..start).rev(), full, playback);
    check("reverse playback", &reversed);
    let from_read_back = reversed.iter().filter(|o| o[MEDIA] == 0).count();
    // One window per GOP of 12 frames (both clips' GOPs line up).
    let windows = COUNTED.div_ceil(12) + 1;
    assert!(
        from_read_back >= COUNTED - windows,
        "{from_read_back} of {COUNTED} frames came from the read-back"
    );
}

/// A 640×360 @ 30 fps sequence with one 10 s clip of a 4000×3000 still (a 12-megapixel photo:
/// an image asset, no duration), and the backend that decodes it.
fn still_fixture() -> (Project, FakeBackend) {
    let path = std::env::temp_dir()
        .join("cutweave-preview-alloc-photo.png")
        .to_string_lossy()
        .into_owned();
    let backend = FakeBackend::new().with_media(
        &path,
        FakeMedia::video(FakeVideo::new(4000, 3000, FrameRate::FPS_30, 1)),
    );
    let id = MediaId::from_u128(23);
    let mut photo = asset(id, &path, (4000, 3000), 3);
    photo.kind = MediaKind::Image;
    photo.probe.duration = None;
    let mut project = Project::new("preview alloc still");
    project.media.insert(id, photo);
    let seq = SequenceId::from_u128(1);
    let format = SequenceFormat::new(640, 360, FrameRate::FPS_30, SampleRate::HZ_48_000);
    let mut sequence = Sequence::new(seq, "Main", format);
    let v1 = TrackId::from_u128(11);
    sequence.insert_track(0, Track::new(v1, TrackKind::Video, "V1"));
    let ten = Duration::from_secs(10).unwrap();
    sequence
        .track_mut(&v1)
        .unwrap()
        .items
        .insert(Clip::new(ItemId::from_u128(33), id, Time::ZERO, ten).into());
    project.sequences.insert(seq, sequence);
    (project, backend)
}

/// WP-2F.05a: a large still is decoded once. Its first frame decodes the 12-megapixel picture;
/// every later frame of its clip (playing across it, scrubbing in it, a paused refresh) shows
/// the picture the preview holds and the texture the compositor cached: nothing from the media
/// layer, no allocation of our own, and one decoder opened in all.
#[test]
fn a_large_still_is_decoded_once_for_every_frame_of_its_clip() {
    let (project, backend) = still_fixture();
    let probe = backend.clone();
    let Some(mut rig) = Rig::with((project, backend), CompositorOptions::default()) else {
        return;
    };
    let full = Quality::FULL;
    let playback = RenderPurpose::Playback;
    let first = rig.render([0], full, playback);
    assert!(first[0][MEDIA] > 0, "the first frame decodes the picture");
    rig.render(1..WARM_UP, full, playback);

    let n = i64::try_from(COUNTED).unwrap();
    let played = rig.render(WARM_UP..WARM_UP + n, full, playback);
    let (_, media) = check("still clip playback", &played);
    assert_eq!(media, 0, "a still the preview holds needs no media layer");

    let quarter = Quality::new(Scale::Quarter, false);
    rig.render([WARM_UP, WARM_UP + 1], quarter, RenderPurpose::Scrub);
    let mut scrub = Vec::new();
    for f in (WARM_UP..WARM_UP + n).step_by(6) {
        scrub.extend(rig.render([f], quarter, RenderPurpose::Scrub));
        scrub.extend(rig.render([f], full, RenderPurpose::Still));
    }
    let (_, media) = check("still clip scrub", &scrub);
    assert_eq!(media, 0, "scrubbing inside a still needs no media layer");
    assert_eq!(probe.decoders_opened(), 1, "one decoder, one decode");
}
