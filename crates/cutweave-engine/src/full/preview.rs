// SPDX-License-Identifier: GPL-3.0-or-later
//! [`PreviewSource`]: the transport's frame source over the render core (WP-1I.05, R31;
//! arch §8, §10): the plan in `cutweave_playback::transport::source`.
//!
//! `render` takes the live document snapshot, evaluates the first sequence at the requested
//! frame, applies the quality rung (the proxy toggle and each layer's drawn size), builds the render graph for the output
//! size with source pictures from [`PreviewFrames`] (decodes cancelled with the render's epoch),
//! composites it on the preview's own [`Compositor`] (the GPU device is shared with
//! `render.frame` and export, the compositor is not, so preview never waits on their lock) and
//! copies the RGBA8 readback into the ring slot layout. Without a GPU it renders with the CPU
//! reference renderer.
//!
//! **Sources (ADR-0022, WP-1H.04).** A request whose target is a media asset renders that asset
//! on its own: [`source_timeline`] builds a transient one-clip timeline from the asset's probe
//! (its picture and its sound, at its own frame rate; audio-only assets on a 30 fps grid over
//! the black background) in a project of its own, never in the document. It is indexed and
//! rendered exactly like the sequence, with the canvas set to the output size, so the picture is
//! fitted into the preview with the asset's aspect. [`DocSources`] answers `view.set_source`
//! from the same rules ([`source_info`]).
//!
//! **No per-frame allocation of its own** (arch §10 "Threads"): the sequence and the source
//! shown last are each indexed once per document revision ([`TimelineIndex`]: an edit costs one
//! re-index on the next frame, a switch to a new source one index; an edit that leaves a
//! source's asset alone costs it nothing), the scene, the render graph and its scratch are
//! reused, and the pixels go from the mapped readback straight into the ring slot. What still
//! allocates on the render thread is inside wgpu and inside the media layer when a picture the
//! preview does not hold must be decoded or fetched from the frame cache (see
//! [`PreviewFrames`]); the CPU fallback and error paths allocate too. The engine's
//! `preview_alloc` test holds it to that.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cutweave_commands::{CmdError, DocHandle, DocSnapshot, Rev};
use cutweave_media::DecodeMode;
use cutweave_model::{
    Clip, Duration, EntityKind, FrameRate, ItemId, MediaAsset, MediaId, MediaKind, Project,
    SampleRate, Sequence, SequenceFormat, SequenceId, StreamKind, Time, Track, TrackId, TrackKind,
};
use cutweave_ops::media::has_audio_stream;
use cutweave_playback::shm::{FrameLayout, PixelFormat};
use cutweave_playback::transport::{
    CancelToken, FrameSource, PlaybackTarget, RenderPurpose, RenderRequest, Rendered,
    SequenceTiming, SourceError, SourceInfo, SourceResolver,
};
use cutweave_render::alloc_scope::{self, Scope};
use cutweave_render::gpu::{Compositor, CompositorOptions, RenderError};
use cutweave_render::graph::{BuildScratch, GraphOptions, RenderGraph, build_into};
use cutweave_render::{SceneDescription, Target, TimelineIndex};

use super::frames::{MediaFrames, PreviewFrames};
use super::gpu::LateGpu;
use crate::subsystems::SubsystemEvents;

/// The first sequence of `doc` (the one the editor shows: lowest id).
#[must_use]
pub(crate) fn active_sequence(doc: &Project) -> Option<SequenceId> {
    doc.sequences.keys().next().copied()
}

/// The frame grid and extent of the sequence playback shows (30 fps and empty without one).
#[must_use]
pub(crate) fn timing_of(doc: &Project) -> SequenceTiming {
    match active_sequence(doc).and_then(|id| doc.sequences.get(&id)) {
        Some(seq) => SequenceTiming::new(seq.format.frame_rate, Time::from(seq.duration())),
        None => SequenceTiming::new(FrameRate::FPS_30, Time::ZERO),
    }
}

/// The sequence of a source's transient timeline ([`source_timeline`]); its own project holds
/// nothing else, so these ids cannot meet the document's.
pub(crate) const SOURCE_SEQUENCE: SequenceId = SequenceId::from_u128(0x5ce_0001);
const SOURCE_VIDEO_TRACK: TrackId = TrackId::from_u128(0x5ce_0002);
const SOURCE_AUDIO_TRACK: TrackId = TrackId::from_u128(0x5ce_0003);
const SOURCE_VIDEO_CLIP: ItemId = ItemId::from_u128(0x5ce_0004);
const SOURCE_AUDIO_CLIP: ItemId = ItemId::from_u128(0x5ce_0005);
/// The canvas of an asset without a picture size (its pictures are fitted to the output).
const DEFAULT_CANVAS: (u32, u32) = (1920, 1080);

/// The source that plays `media` of `doc` on its own (ADR-0022): its length and frame grid, the
/// asset's own frame rate (30 fps for audio-only assets).
///
/// # Errors
/// `NotFound` for an asset `doc` does not have; `Invalid` (at `/media`) for one with nothing to
/// play: a still image, a compound clip, a file without a known length.
pub(crate) fn source_info(doc: &Project, media: MediaId) -> Result<SourceInfo, CmdError> {
    let asset = doc
        .media
        .get(&media)
        .ok_or_else(|| CmdError::not_found(EntityKind::Media, media))?;
    let fps = match asset.kind {
        MediaKind::Video | MediaKind::ImageSequence => asset.probe.fps.unwrap_or(FrameRate::FPS_30),
        MediaKind::Audio => FrameRate::FPS_30,
        MediaKind::Image => {
            return Err(CmdError::invalid(
                "/media",
                format!("media {media} is a still image: it has nothing to play"),
            ));
        }
        MediaKind::SequenceRef => {
            return Err(CmdError::invalid(
                "/media",
                format!("media {media} is a compound clip, which cannot play on its own yet"),
            ));
        }
    };
    let duration = asset
        .probe
        .duration
        .filter(|d| *d > Duration::ZERO)
        .ok_or_else(|| {
            CmdError::invalid(
                "/media",
                format!("media {media} has no known length to play"),
            )
        })?;
    Ok(SourceInfo {
        media,
        duration,
        fps,
    })
}

/// The picture size of `asset` upright (the display rotation applied), if the probe knows it.
fn display_size(asset: &MediaAsset) -> Option<(u32, u32)> {
    let video = asset
        .probe
        .streams
        .iter()
        .find(|s| s.kind == StreamKind::Video)?;
    let (w, h) = (video.width?, video.height?);
    if w == 0 || h == 0 {
        return None;
    }
    Some(if asset.probe.rotation % 180 == 90 {
        (h, w)
    } else {
        (w, h)
    })
}

/// The transient one-clip timeline a source plays (ADR-0022): a project of its own holding the
/// asset and [`SOURCE_SEQUENCE`], whose clips cover the whole asset from 0: its picture on a
/// video track (video files) and its sound on an audio track (when it has some). `None` when
/// `media` cannot play ([`source_info`]). Never part of the document.
pub(crate) fn source_timeline(doc: &Project, media: MediaId) -> Option<Project> {
    let info = source_info(doc, media).ok()?;
    let asset = doc.media.get(&media)?;
    let (width, height) = display_size(asset).unwrap_or(DEFAULT_CANVAS);
    let format = SequenceFormat::new(width, height, info.fps, SampleRate::HZ_48_000);
    let mut sequence = Sequence::new(SOURCE_SEQUENCE, "Source", format);
    let clip = |id: ItemId| Clip::new(id, media, Time::ZERO, info.duration);
    let mut tracks = 0;
    if matches!(asset.kind, MediaKind::Video | MediaKind::ImageSequence) {
        sequence.insert_track(
            tracks,
            Track::new(SOURCE_VIDEO_TRACK, TrackKind::Video, "V1"),
        );
        let track = sequence.track_mut(&SOURCE_VIDEO_TRACK)?;
        track.items.insert(clip(SOURCE_VIDEO_CLIP).into());
        tracks += 1;
    }
    if asset.kind == MediaKind::Audio || has_audio_stream(asset) {
        sequence.insert_track(
            tracks,
            Track::new(SOURCE_AUDIO_TRACK, TrackKind::Audio, "A1"),
        );
        let track = sequence.track_mut(&SOURCE_AUDIO_TRACK)?;
        track.items.insert(clip(SOURCE_AUDIO_CLIP).into());
    }
    let mut project = Project::new("Source");
    project.media.insert(media, asset.clone());
    project.sequences.insert(SOURCE_SEQUENCE, sequence);
    Some(project)
}

/// Finds the media `view.set_source` names in the open document ([`source_info`]).
pub(crate) struct DocSources {
    doc: DocHandle,
}

impl DocSources {
    pub(crate) fn new(doc: DocHandle) -> DocSources {
        DocSources { doc }
    }
}

impl SourceResolver for DocSources {
    fn resolve(&self, media: MediaId) -> Result<SourceInfo, CmdError> {
        source_info(&self.doc.snapshot().doc, media)
    }
}

/// The preview compositor, lent to one playback session at a time (taken from the GPU once it
/// is open) and returned on drop.
pub(crate) struct CompositorSlot {
    compositor: Option<Compositor>,
    gpu: Arc<LateGpu>,
    /// The GPU settled without a device: render on the CPU.
    cpu: bool,
}

/// How a frame can be rendered right now.
enum Renderer<'a> {
    Gpu(&'a mut Compositor),
    Cpu,
    /// The GPU is still opening.
    Pending,
}

impl CompositorSlot {
    /// A slot for the preview compositor of `gpu` (taken when the device is open).
    pub(crate) fn new(gpu: Arc<LateGpu>) -> CompositorSlot {
        let mut slot = CompositorSlot {
            compositor: None,
            gpu,
            cpu: false,
        };
        let _ = slot.renderer();
        slot
    }

    fn renderer(&mut self) -> Renderer<'_> {
        if self.compositor.is_none() && !self.cpu {
            if !self.gpu.is_settled() {
                return Renderer::Pending;
            }
            self.compositor = self.gpu.take_preview();
            self.cpu = self.compositor.is_none() && !self.gpu.has_gpu();
        }
        match self.compositor.as_mut() {
            Some(c) => Renderer::Gpu(c),
            None if self.cpu => Renderer::Cpu,
            // The device exists but another session still holds the compositor.
            None => Renderer::Pending,
        }
    }
}

impl Drop for CompositorSlot {
    fn drop(&mut self) {
        if let Some(c) = self.compositor.take() {
            self.gpu.return_preview(c);
        }
    }
}

/// The active sequence indexed at one document revision.
struct Indexed {
    rev: Rev,
    /// `None`: the document has no sequence.
    index: Option<TimelineIndex>,
}

/// A source's transient timeline indexed for the asset as it was at `rev`.
struct SourceIndexed {
    media: MediaId,
    rev: Rev,
    /// The asset it was built from: an edit that leaves it alone needs no new index.
    asset: Option<MediaAsset>,
    /// `None`: the asset is gone or cannot play.
    index: Option<TimelineIndex>,
}

/// What a render reuses from the last one (see the module docs).
struct Reused {
    sequence: Option<Indexed>,
    source: Option<SourceIndexed>,
    scene: Option<SceneDescription>,
    graph: RenderGraph,
    scratch: BuildScratch,
}

/// The transport's frame source over the render core (see the module docs).
pub(crate) struct PreviewSource {
    doc: DocHandle,
    timing: SequenceTiming,
    compositor: CompositorSlot,
    frames: PreviewFrames,
    options: GraphOptions,
    events: SubsystemEvents,
    /// `engine.degraded{gpu}` was sent for a lost device.
    lost_reported: Arc<AtomicBool>,
    reused: Reused,
}

impl std::fmt::Debug for PreviewSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewSource")
            .field("timing", &self.timing)
            .field("gpu", &self.compositor.compositor.is_some())
            .field("cpu", &self.compositor.cpu)
            .finish_non_exhaustive()
    }
}

impl PreviewSource {
    pub(crate) fn new(
        doc: DocHandle,
        compositor: CompositorSlot,
        frames: Arc<MediaFrames>,
        events: SubsystemEvents,
        lost_reported: Arc<AtomicBool>,
    ) -> PreviewSource {
        let timing = timing_of(&doc.snapshot().doc);
        // The compositor's graph options (`CompositorOptions::default().graph`).
        let options = CompositorOptions::default().graph;
        PreviewSource {
            doc,
            timing,
            compositor,
            frames: PreviewFrames::new(frames),
            options,
            events,
            lost_reported,
            reused: Reused {
                sequence: None,
                source: None,
                scene: None,
                graph: RenderGraph::new(Target::rgba8(1, 1)),
                scratch: BuildScratch::default(),
            },
        }
    }
}

/// Copies straight RGBA8 `pixels` (`width × height`, tightly packed rows) into `out` laid out
/// as `layout` (RGBA8; other formats or sizes get an opaque black frame).
fn copy_rgba(pixels: &[u8], (width, height): (u32, u32), layout: &FrameLayout, out: &mut [u8]) {
    let plane = layout.planes()[0];
    let row = (layout.width() as usize) * 4;
    let stride = plane.stride as usize;
    if layout.format() != PixelFormat::Rgba8 || width != layout.width() || height != layout.height()
    {
        fill_black(layout, out);
        return;
    }
    for (y, src) in pixels.chunks_exact(row).enumerate() {
        let start = plane.offset + y * stride;
        if let Some(dst) = out.get_mut(start..start + row) {
            dst.copy_from_slice(src);
        }
    }
}

/// An opaque black frame in `layout`'s format.
fn fill_black(layout: &FrameLayout, out: &mut [u8]) {
    match layout.format() {
        PixelFormat::Rgba8 => {
            for px in out.as_chunks_mut::<4>().0 {
                *px = [0, 0, 0, 255];
            }
        }
        PixelFormat::Nv12 => {
            let planes = layout.planes();
            let luma_end = planes[1].offset.min(out.len());
            out[..luma_end].fill(16);
            out[luma_end..].fill(128);
        }
    }
}

/// The sequence of `snapshot` indexed for rendering (re-indexed when the revision moved).
fn indexed<'a>(
    slot: &'a mut Option<Indexed>,
    frames: &PreviewFrames,
    snapshot: &DocSnapshot,
) -> Option<&'a TimelineIndex> {
    if slot.as_ref().is_none_or(|i| i.rev != snapshot.rev) {
        // An edit: index the new revision (allocates, once) and let go of media it dropped.
        let index = active_sequence(&snapshot.doc)
            .and_then(|seq| TimelineIndex::new(&snapshot.doc, seq).ok());
        frames.retain_current(&snapshot.doc);
        *slot = Some(Indexed {
            rev: snapshot.rev,
            index,
        });
    }
    slot.as_ref().and_then(|i| i.index.as_ref())
}

/// The transient timeline of the source `media` in `snapshot`, indexed for rendering: built
/// when the source is new or its asset changed (allocates, once), kept across other edits.
fn source_indexed<'a>(
    slot: &'a mut Option<SourceIndexed>,
    frames: &PreviewFrames,
    snapshot: &DocSnapshot,
    media: MediaId,
) -> Option<&'a TimelineIndex> {
    let asset = snapshot.doc.media.get(&media);
    let fresh = match slot.as_mut() {
        Some(s) if s.media == media && s.rev == snapshot.rev => true,
        Some(s) if s.media == media && s.asset.as_ref() == asset => {
            // An edit elsewhere: the same picture and sound.
            s.rev = snapshot.rev;
            frames.retain_current(&snapshot.doc);
            true
        }
        _ => false,
    };
    if !fresh {
        let index = source_timeline(&snapshot.doc, media)
            .and_then(|project| TimelineIndex::new(&project, SOURCE_SEQUENCE).ok());
        frames.retain_current(&snapshot.doc);
        *slot = Some(SourceIndexed {
            media,
            rev: snapshot.rev,
            asset: asset.cloned(),
            index,
        });
    }
    slot.as_ref().and_then(|s| s.index.as_ref())
}

impl FrameSource for PreviewSource {
    fn timing(&self) -> SequenceTiming {
        self.timing
    }

    fn render(
        &mut self,
        request: &RenderRequest,
        cancel: &CancelToken,
        out: &mut [u8],
    ) -> Result<Rendered, SourceError> {
        let snapshot = self.doc.snapshot();
        let mut rendered = Rendered {
            rev: snapshot.rev.0,
            proxy: false,
        };
        let PreviewSource {
            compositor,
            frames,
            options,
            events,
            lost_reported,
            reused,
            ..
        } = self;
        let index = match request.target {
            PlaybackTarget::Sequence => indexed(&mut reused.sequence, frames, &snapshot),
            PlaybackTarget::Media(media) => {
                source_indexed(&mut reused.source, frames, &snapshot, media)
            }
        };
        let Some(index) = index else {
            fill_black(&request.layout, out);
            return Ok(rendered);
        };
        if cancel.is_cancelled() {
            return Err(SourceError::Cancelled);
        }
        let scene = match &mut reused.scene {
            Some(scene) => {
                index.evaluate_into(request.t, scene);
                scene
            }
            None => reused.scene.insert(index.evaluate(request.t)),
        };
        if request.target != PlaybackTarget::Sequence {
            // A source fills the preview with the asset's own aspect (letterboxed): its canvas
            // is the output.
            scene.width = request.layout.width();
            scene.height = request.layout.height();
        }
        // Proxies as the preview's toggle says, each layer with the size it is drawn at: the
        // media layer serves a proxy only when it holds those pixels (WP-2F.02).
        request
            .quality
            .apply_for_output(scene, (request.layout.width(), request.layout.height()));
        let target = Target::rgba8(request.layout.width(), request.layout.height());
        let mode = match request.purpose {
            RenderPurpose::Playback => DecodeMode::Sequential,
            RenderPurpose::Scrub | RenderPurpose::Still => DecodeMode::Seek,
        };
        let source = frames.request(&snapshot, cancel, mode);
        let result = build_into(
            &mut reused.graph,
            &mut reused.scratch,
            scene,
            target,
            &source,
            options,
        )
        .map_err(|e| SourceError::Failed(format!("cannot compile the frame: {e}")))
        .and_then(|()| {
            if cancel.is_cancelled() {
                return Err(SourceError::Cancelled);
            }
            let layout = &request.layout;
            match compositor.renderer() {
                Renderer::Gpu(c) => {
                    match c.execute_with(&reused.graph, |pixels| {
                        copy_rgba(pixels, target.size(), layout, out);
                    }) {
                        Ok(((), _stats)) => Ok(()),
                        Err(RenderError::Gpu(e)) if e.is_device_lost() => {
                            // The compositor already recreated the device once; tell the clients.
                            if !lost_reported.swap(true, Ordering::AcqRel) {
                                events.degraded("gpu", &format!("preview: {e}"));
                            }
                            Err(SourceError::Failed(e.to_string()))
                        }
                        Err(e) => Err(SourceError::Failed(e.to_string())),
                    }
                }
                Renderer::Cpu => {
                    cutweave_render::reference::render(scene, target, &source, options)
                        .map(|image| copy_rgba(&image.data, target.size(), layout, out))
                        .map_err(|e| SourceError::Failed(e.to_string()))
                }
                // Black until the device is open (it then re-renders the current frame).
                Renderer::Pending => {
                    fill_black(layout, out);
                    Ok(())
                }
            }
        });
        // The pictures belong to the media layer: let go of them where it is accounted for.
        alloc_scope::within(Scope::Media, || {
            reused.graph.release_frames();
            reused.scratch.release_frames();
        });
        rendered.proxy = source.showed_proxy();
        result?;
        if cancel.is_cancelled() {
            return Err(SourceError::Cancelled);
        }
        Ok(rendered)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn rgba_frames_land_in_the_slot_rows() {
        let layout = FrameLayout::aligned(3, 2, PixelFormat::Rgba8, 256).unwrap();
        let mut out = vec![0u8; layout.len()];
        let pixels: Vec<u8> = (0..24).collect();
        copy_rgba(&pixels, (3, 2), &layout, &mut out);
        let stride = layout.planes()[0].stride as usize;
        assert_eq!(&out[..12], &pixels[..12]);
        assert_eq!(&out[stride..stride + 12], &pixels[12..]);
        // A size mismatch gives an opaque black frame, never garbage.
        copy_rgba(&[9; 4], (1, 1), &layout, &mut out);
        assert_eq!(&out[..4], &[0, 0, 0, 255]);
    }
}
