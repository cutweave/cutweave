// SPDX-License-Identifier: GPL-3.0-or-later
//! [`PreviewSource`]: the transport's frame source over the render core (WP-1I.05, R31;
//! arch §8, §10): the plan in `cutweave_playback::transport::source`.
//!
//! `render` takes the live document snapshot, evaluates the first sequence at the requested
//! frame, applies the quality rung (proxy eligibility), builds the render graph for the output
//! size with source pictures from [`MediaFrames`] (decodes cancelled with the render's epoch),
//! composites it on the preview's own [`Compositor`] (the GPU device is shared with
//! `render.frame` and export, the compositor is not, so preview never waits on their lock) and
//! copies the RGBA8 readback into the ring slot layout. Without a GPU it renders with the CPU
//! reference renderer.
//!
//! Evaluation, graph building and wgpu allocate per frame; the playback crate's no-allocation
//! test covers the render loop itself (with its synthetic source). Pooled buffers for the real
//! renderer are a follow-up (see the WP-1I.05 notes).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_commands::DocHandle;
use cutweave_media::{CancelCheck, DecodeMode};
use cutweave_model::{Project, SequenceId, Time};
use cutweave_playback::shm::{FrameLayout, PixelFormat};
use cutweave_playback::transport::{
    CancelToken, FrameSource, RenderPurpose, RenderRequest, Rendered, SequenceTiming, SourceError,
};
use cutweave_render::gpu::{Compositor, RenderError};
use cutweave_render::graph::{GraphOptions, build};
use cutweave_render::{Image, Target, evaluate};

use super::frames::MediaFrames;
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
        None => SequenceTiming::new(cutweave_model::FrameRate::FPS_30, Time::ZERO),
    }
}

/// The preview compositor, lent to one playback session at a time and returned on drop.
pub(crate) struct CompositorSlot {
    compositor: Option<Compositor>,
    home: Arc<Mutex<Option<Compositor>>>,
}

impl CompositorSlot {
    /// Takes the compositor from `home` (`None` there: render on the CPU).
    pub(crate) fn take(home: &Arc<Mutex<Option<Compositor>>>) -> CompositorSlot {
        CompositorSlot {
            compositor: home.lock().unwrap_or_else(PoisonError::into_inner).take(),
            home: Arc::clone(home),
        }
    }
}

impl Drop for CompositorSlot {
    fn drop(&mut self) {
        if let Some(c) = self.compositor.take() {
            *self.home.lock().unwrap_or_else(PoisonError::into_inner) = Some(c);
        }
    }
}

/// The transport's frame source over the render core (see the module docs).
pub(crate) struct PreviewSource {
    doc: DocHandle,
    timing: SequenceTiming,
    compositor: CompositorSlot,
    frames: Arc<MediaFrames>,
    options: GraphOptions,
    events: SubsystemEvents,
    /// `engine.degraded{gpu}` was sent for a lost device.
    lost_reported: Arc<AtomicBool>,
}

impl std::fmt::Debug for PreviewSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewSource")
            .field("timing", &self.timing)
            .field("gpu", &self.compositor.compositor.is_some())
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
        let options = compositor
            .compositor
            .as_ref()
            .map_or_else(GraphOptions::default, |c| c.options().graph);
        PreviewSource {
            doc,
            timing,
            compositor,
            frames,
            options,
            events,
            lost_reported,
        }
    }

    fn rasterize(
        &mut self,
        scene: &cutweave_render::SceneDescription,
        graph: &cutweave_render::graph::RenderGraph,
    ) -> Result<Image, SourceError> {
        let Some(compositor) = self.compositor.compositor.as_mut() else {
            return cutweave_render::reference::render(
                scene,
                graph.target,
                &*self.frames,
                &self.options,
            )
            .map_err(|e| SourceError::Failed(e.to_string()));
        };
        match compositor.execute(graph) {
            Ok(frame) => frame
                .into_image()
                .ok_or_else(|| SourceError::Failed("the graph has no CPU target".into())),
            Err(RenderError::Gpu(e)) if e.is_device_lost() => {
                // The compositor already recreated the device once; tell the clients.
                if !self.lost_reported.swap(true, Ordering::AcqRel) {
                    self.events.degraded("gpu", &format!("preview: {e}"));
                }
                Err(SourceError::Failed(e.to_string()))
            }
            Err(e) => Err(SourceError::Failed(e.to_string())),
        }
    }
}

/// Copies straight RGBA8 `image` into `out` laid out as `layout` (RGBA8; other formats get an
/// opaque black frame).
fn copy_into(image: &Image, layout: &FrameLayout, out: &mut [u8]) {
    let plane = layout.planes()[0];
    let row = (layout.width() as usize) * 4;
    let stride = plane.stride as usize;
    if layout.format() != PixelFormat::Rgba8
        || image.width != layout.width()
        || image.height != layout.height()
    {
        fill_black(layout, out);
        return;
    }
    for (y, src) in image.data.chunks_exact(row).enumerate() {
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
        let rendered = Rendered {
            rev: snapshot.rev.0,
        };
        let Some(seq) = active_sequence(&snapshot.doc) else {
            fill_black(&request.layout, out);
            return Ok(rendered);
        };
        if cancel.is_cancelled() {
            return Err(SourceError::Cancelled);
        }
        let mut scene = evaluate(&snapshot.doc, seq, request.t)
            .map_err(|e| SourceError::Failed(e.to_string()))?;
        request.quality.apply(&mut scene);
        let target = Target::rgba8(request.layout.width(), request.layout.height());
        let token = cancel.clone();
        let decode_cancel =
            cutweave_media::CancelToken::linked(CancelCheck::new(move || token.is_cancelled()));
        let mode = match request.purpose {
            RenderPurpose::Playback => DecodeMode::Sequential,
            RenderPurpose::Scrub | RenderPurpose::Still => DecodeMode::Seek,
        };
        let frames = Arc::clone(&self.frames);
        let options = self.options;
        let graph = MediaFrames::scoped(decode_cancel, mode, || {
            build(&scene, target, &*frames, &options)
        })
        .map_err(|e| SourceError::Failed(format!("cannot compile the frame: {e}")))?;
        if cancel.is_cancelled() {
            return Err(SourceError::Cancelled);
        }
        let image = self.rasterize(&scene, &graph)?;
        if cancel.is_cancelled() {
            return Err(SourceError::Cancelled);
        }
        copy_into(&image, &request.layout, out);
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
        let image = Image {
            width: 3,
            height: 2,
            data: (0..24).collect(),
        };
        copy_into(&image, &layout, &mut out);
        let stride = layout.planes()[0].stride as usize;
        assert_eq!(&out[..12], &image.data[..12]);
        assert_eq!(&out[stride..stride + 12], &image.data[12..]);
        // A size mismatch gives an opaque black frame, never garbage.
        let small = Image {
            width: 1,
            height: 1,
            data: vec![9; 4],
        };
        copy_into(&small, &layout, &mut out);
        assert_eq!(&out[..4], &[0, 0, 0, 255]);
    }
}
