// SPDX-License-Identifier: GPL-3.0-or-later
//! The [`FrameSink`] seam: where rendered frames go. [`PublisherSink`] publishes into the
//! shared-memory preview ring (WP-1H.02); [`NullSink`] discards them (headless runs).

use cutweave_model::{Duration, Time};
use cutweave_render::{Quality, Scale};
use thiserror::Error;

use crate::shm::{
    FLAG_LOWRES, FrameLayout, FrameMeta, FramePublisher, LayoutError, PixelFormat, PreviewConfig,
    PublishError, Reconfigured, RingAnnouncement, RingError,
};

/// A sink could not take a frame or a configuration.
#[derive(Debug, Error)]
pub enum SinkError {
    /// The frame geometry is invalid.
    #[error(transparent)]
    Layout(#[from] LayoutError),
    /// The ring refused the frame.
    #[error(transparent)]
    Publish(#[from] PublishError),
    /// The ring could not be reconfigured.
    #[error(transparent)]
    Ring(#[from] RingError),
}

/// What the render loop knows about a frame it delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    /// Frame index on the sequence grid.
    pub frame: i64,
    /// Presentation timestamp (the frame's start).
    pub pts: Time,
    /// Frame duration.
    pub duration: Duration,
    /// Document revision rendered.
    pub rev: u64,
    /// The quality rung it was rendered at.
    pub quality: Quality,
    /// A reduced-size scrub frame: a full-quality frame of the same `pts` follows unless a newer
    /// request supersedes it (the ring's `LOWRES` flag, arch §10 step 5).
    pub lowres: bool,
    /// The transport epoch (request) it answers; never older than the latest request.
    pub epoch: u64,
    /// Host time ([`HostClock`](crate::transport::HostClock)) the render started.
    pub render_ns: u64,
}

/// `preview` reduced by `scale` (fitted to the format's size rules).
#[must_use]
pub fn scaled_preview(preview: PreviewConfig, scale: Scale) -> PreviewConfig {
    let (width, height) = scale.apply((preview.width, preview.height));
    PreviewConfig {
        width,
        height,
        format: preview.format,
    }
    .fitted()
}

/// Where the render loop delivers frames (the SHM ring publisher or a test sink). Called only
/// from the render thread; `deliver` must not block or allocate.
pub trait FrameSink: Send {
    /// The preview configuration full-quality frames are rendered at (the canvas size).
    fn preview(&self) -> PreviewConfig;

    /// The layout of frames rendered at `scale` (the preview reduced by it).
    fn layout(&self, scale: Scale) -> Result<FrameLayout, SinkError>;

    /// Publishes a frame (latest wins) and returns its sequence number.
    fn deliver(
        &mut self,
        info: &FrameInfo,
        layout: &FrameLayout,
        pixels: &[u8],
    ) -> Result<u64, SinkError>;

    /// Counts frames playback skipped because rendering fell behind.
    fn add_dropped(&mut self, frames: u64) {
        let _ = frames;
    }

    /// Liveness while no frames are delivered (called at ≥ 10 Hz by the render loop).
    fn heartbeat(&mut self) {}

    /// Changes the preview size or format (`view.set_preview_size`).
    fn configure(&mut self, preview: PreviewConfig) -> Result<(), SinkError>;
}

/// Called with the new ring when a reconfiguration moved frames to a bigger ring (announce it
/// in the run-file and handshake, WP-1I.01).
pub type RingMoved = Box<dyn FnMut(&RingAnnouncement) + Send>;

/// Publishes frames into the preview ring ([`FramePublisher`]): frames are copied with one
/// `memcpy` (no allocation); scrub frames carry the `LOWRES` flag.
pub struct PublisherSink {
    publisher: FramePublisher,
    on_moved: Option<RingMoved>,
}

impl std::fmt::Debug for PublisherSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublisherSink")
            .field("ring", self.publisher.name())
            .field("preview", &self.publisher.preview())
            .finish_non_exhaustive()
    }
}

impl PublisherSink {
    /// A sink over `publisher`. Take [`FramePublisher::clock_writer`] and
    /// [`FramePublisher::announcement`] before moving it in.
    #[must_use]
    pub fn new(publisher: FramePublisher) -> PublisherSink {
        PublisherSink {
            publisher,
            on_moved: None,
        }
    }

    /// Calls `f` whenever a reconfiguration moves the frames to a new ring.
    #[must_use]
    pub fn on_ring_moved(mut self, f: RingMoved) -> PublisherSink {
        self.on_moved = Some(f);
        self
    }

    /// The publisher.
    #[must_use]
    pub fn publisher(&self) -> &FramePublisher {
        &self.publisher
    }

    /// The publisher back.
    #[must_use]
    pub fn into_inner(self) -> FramePublisher {
        self.publisher
    }
}

impl FrameSink for PublisherSink {
    fn preview(&self) -> PreviewConfig {
        self.publisher.preview().0
    }

    fn layout(&self, scale: Scale) -> Result<FrameLayout, SinkError> {
        Ok(self
            .publisher
            .layout_for(scaled_preview(self.preview(), scale))?)
    }

    fn deliver(
        &mut self,
        info: &FrameInfo,
        layout: &FrameLayout,
        pixels: &[u8],
    ) -> Result<u64, SinkError> {
        let meta = FrameMeta {
            pts: info.pts,
            duration: info.duration,
            rev: info.rev,
            flags: if info.lowres { FLAG_LOWRES } else { 0 },
            render_ns: info.render_ns,
        };
        Ok(self.publisher.publish(&meta, layout, pixels)?)
    }

    fn add_dropped(&mut self, frames: u64) {
        self.publisher.add_dropped(frames);
    }

    fn heartbeat(&mut self) {
        self.publisher.heartbeat();
    }

    fn configure(&mut self, preview: PreviewConfig) -> Result<(), SinkError> {
        if let Reconfigured::Moved { ring, .. } = self.publisher.configure(preview)?
            && let Some(f) = &mut self.on_moved
        {
            f(&ring);
        }
        Ok(())
    }
}

/// Discards frames, counting them: the transport without a preview (headless MCP/CLI runs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NullSink {
    preview: PreviewConfig,
    delivered: u64,
    dropped: u64,
}

impl NullSink {
    /// A sink for frames of `width` x `height` RGBA8.
    #[must_use]
    pub fn new(width: u32, height: u32) -> NullSink {
        NullSink {
            preview: PreviewConfig {
                width,
                height,
                format: PixelFormat::Rgba8,
            }
            .fitted(),
            delivered: 0,
            dropped: 0,
        }
    }

    /// Frames delivered so far.
    #[must_use]
    pub fn delivered(&self) -> u64 {
        self.delivered
    }

    /// Frames reported dropped so far.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl FrameSink for NullSink {
    fn preview(&self) -> PreviewConfig {
        self.preview
    }

    fn layout(&self, scale: Scale) -> Result<FrameLayout, SinkError> {
        let c = scaled_preview(self.preview, scale);
        Ok(FrameLayout::packed(c.width, c.height, c.format)?)
    }

    fn deliver(&mut self, _: &FrameInfo, _: &FrameLayout, _: &[u8]) -> Result<u64, SinkError> {
        self.delivered += 1;
        Ok(self.delivered)
    }

    fn add_dropped(&mut self, frames: u64) {
        self.dropped += frames;
    }

    fn configure(&mut self, preview: PreviewConfig) -> Result<(), SinkError> {
        self.preview = preview.fitted();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn scaled_previews_keep_the_format_rules() {
        let p = PreviewConfig {
            width: 1918,
            height: 1078,
            format: PixelFormat::Nv12,
        };
        assert_eq!(scaled_preview(p, Scale::Full), p);
        let q = scaled_preview(p, Scale::Quarter);
        assert_eq!((q.width, q.height), (480, 270));
        let h = scaled_preview(p, Scale::Half);
        assert_eq!((h.width, h.height), (958, 538), "NV12 stays even");
        let mut sink = NullSink::new(64, 36);
        assert_eq!(sink.layout(Scale::Quarter).unwrap().width(), 16);
        sink.configure(PreviewConfig {
            width: 32,
            height: 18,
            format: PixelFormat::Rgba8,
        })
        .unwrap();
        assert_eq!(sink.preview().width, 32);
    }
}
