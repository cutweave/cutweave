// SPDX-License-Identifier: GPL-3.0-or-later
//! The GPU [`FrameBackend`] of the `render.*` queries: the engine's [`Compositor`] plus its
//! frame source, made current around query dispatch with [`crate::query::enter`].

use std::sync::{Arc, Mutex};

use super::Compositor;
use crate::frame::FrameSource;
use crate::graph::{GraphOptions, RenderGraph};
use crate::image::Image;
use crate::query::{FrameBackend, SharedBackend};
use crate::scene::SceneDescription;

/// Renders query frames with a [`Compositor`] (blocking readback).
pub struct GpuFrameBackend {
    compositor: Compositor,
    source: Arc<dyn FrameSource + Send + Sync>,
}

impl std::fmt::Debug for GpuFrameBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuFrameBackend")
            .field("compositor", &self.compositor)
            .finish_non_exhaustive()
    }
}

impl GpuFrameBackend {
    /// A backend rendering with `compositor`, frames from `source`.
    #[must_use]
    pub fn new(
        compositor: Compositor,
        source: Arc<dyn FrameSource + Send + Sync>,
    ) -> GpuFrameBackend {
        GpuFrameBackend { compositor, source }
    }

    /// The compositor.
    #[must_use]
    pub fn compositor(&mut self) -> &mut Compositor {
        &mut self.compositor
    }

    /// Shares it for [`crate::query::enter`].
    #[must_use]
    pub fn shared(self) -> SharedBackend {
        Arc::new(Mutex::new(self))
    }
}

impl FrameBackend for GpuFrameBackend {
    fn frame_source(&self) -> &dyn FrameSource {
        &*self.source
    }

    fn graph_options(&self) -> GraphOptions {
        self.compositor.options().graph
    }

    fn rasterize(
        &mut self,
        _scene: &SceneDescription,
        graph: &RenderGraph,
    ) -> Result<Image, String> {
        self.compositor
            .execute(graph)
            .map_err(|e| e.to_string())?
            .into_image()
            .ok_or_else(|| "the graph has no CPU target".to_owned())
    }
}
