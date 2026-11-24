// SPDX-License-Identifier: GPL-3.0-or-later
//! The full engine's GPU, opened on its own thread (WP-1I.05; arch §8, §19).
//!
//! Opening a wgpu device costs about a second, and much more the first time a user profile
//! runs the driver (it builds its shader caches: 5 to 15 s measured on a fresh Windows profile).
//! So the device opens on `cw-gpu-init` while the rest of the engine starts. A `Gui` engine's
//! `start` waits for it up to [`GPU_START_WAIT`] (so a GPU that fails exits with code 11); a
//! headless engine does not wait at all (it never exits 11: it falls back).
//!
//! - **Settled in time** (the usual case in `Gui` mode): as if synchronous. A `Gui` engine
//!   whose GPU fails exits with code 11; a headless one falls back to the software adapter,
//!   then to the CPU reference renderer (`engine.degraded{subsystem: "gpu"}`).
//! - **Still opening**: the engine reports ready anyway. `render.frame` and exports wait for
//!   the device (up to [`RENDER_WAIT`]), preview shows black frames until it arrives and then
//!   re-renders the current frame. A failure from then on cannot exit 11 any more (the engine
//!   is serving): it falls back like a headless engine and reports `engine.degraded`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cutweave_render::gpu::{
    AdapterChoice, Compositor, CompositorOptions, GpuContext, GpuFrameBackend,
};
use cutweave_render::graph::{GraphOptions, RenderGraph};
use cutweave_render::query::FrameBackend;
use cutweave_render::{FrameSource, Image, SceneDescription};

use super::frames::MediaFrames;

/// How long a `Gui` engine's `start` waits for the GPU before it reports ready without it: long
/// enough for a failing adapter search to end in exit 11 on a busy machine, short enough for the
/// supervisor's 10 s ready timeout.
pub const GPU_START_WAIT: Duration = Duration::from_secs(4);
/// How long a `render.frame` or export waits for a GPU that is still opening.
pub const RENDER_WAIT: Duration = Duration::from_secs(60);

/// A device and the two compositors (`render.frame` and export; preview).
pub(crate) type Opened = (GpuContext, Compositor, Compositor);

#[derive(Debug)]
enum Status {
    /// Still opening.
    Pending,
    /// Done: a device, or none (CPU rendering).
    Settled,
    /// Failed while `start` still waited (`Gui` mode: exit 11).
    Failed(String),
}

struct State {
    status: Status,
    /// `start` stopped waiting: failures fall back instead of failing the start.
    late: bool,
    ctx: Option<GpuContext>,
    frame: Option<Compositor>,
    preview: Option<Compositor>,
}

/// The GPU as it arrives (see the module docs).
pub(crate) struct LateGpu {
    state: Mutex<State>,
    changed: Condvar,
    settled: AtomicBool,
}

impl std::fmt::Debug for LateGpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LateGpu")
            .field("settled", &self.is_settled())
            .finish_non_exhaustive()
    }
}

impl LateGpu {
    pub(crate) fn new() -> LateGpu {
        LateGpu {
            state: Mutex::new(State {
                status: Status::Pending,
                late: false,
                ctx: None,
                frame: None,
                preview: None,
            }),
            changed: Condvar::new(),
            settled: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the GPU is open (or known to be absent).
    pub(crate) fn is_settled(&self) -> bool {
        self.settled.load(Ordering::Acquire)
    }

    /// Whether a device is open.
    pub(crate) fn has_gpu(&self) -> bool {
        self.lock().ctx.is_some()
    }

    /// The adapter's key, once open.
    pub(crate) fn adapter(&self) -> Option<String> {
        self.lock().ctx.as_ref().map(|c| c.info().key.clone())
    }

    /// Records a failure while `start` still waits (`true`: `start` fails with it); `false`
    /// when `start` stopped waiting (fall back instead).
    pub(crate) fn fail_start(&self, reason: &str) -> bool {
        let mut st = self.lock();
        if st.late {
            return false;
        }
        st.status = Status::Failed(reason.to_owned());
        self.changed.notify_all();
        true
    }

    /// Installs the result: a device, or `None` for CPU rendering.
    pub(crate) fn settle(&self, opened: Option<Opened>) {
        let mut st = self.lock();
        if let Some((ctx, frame, preview)) = opened {
            st.ctx = Some(ctx);
            st.frame = Some(frame);
            st.preview = Some(preview);
        }
        st.status = Status::Settled;
        self.settled.store(true, Ordering::Release);
        self.changed.notify_all();
    }

    /// `start`'s wait: `Err` with the reason when the GPU failed in time; `Ok` when it settled
    /// or is still opening after `timeout` (then failures fall back).
    pub(crate) fn wait_start(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut st = self.lock();
        loop {
            match &st.status {
                Status::Failed(reason) => return Err(reason.clone()),
                Status::Settled => return Ok(()),
                Status::Pending => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                st.late = true;
                if !timeout.is_zero() {
                    tracing::warn!(
                        "the GPU is still opening after {} ms; the engine starts without it",
                        timeout.as_millis()
                    );
                }
                return Ok(());
            }
            st = self
                .changed
                .wait_timeout(st, left)
                .map_or_else(|e| e.into_inner().0, |(g, _)| g);
        }
    }

    /// Waits up to `timeout` for the GPU to settle; whether it did.
    fn wait_settled(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut st = self.lock();
        while matches!(st.status, Status::Pending) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            st = self
                .changed
                .wait_timeout(st, left)
                .map_or_else(|e| e.into_inner().0, |(g, _)| g);
        }
        true
    }

    /// The preview compositor, lent to one playback session at a time.
    pub(crate) fn take_preview(&self) -> Option<Compositor> {
        self.lock().preview.take()
    }

    /// Gives the preview compositor back (the session ended).
    pub(crate) fn return_preview(&self, compositor: Compositor) {
        self.lock().preview = Some(compositor);
    }

    /// The `render.frame` / export compositor (taken once by [`LateRenderer`]).
    fn take_frame(&self) -> Option<Compositor> {
        self.lock().frame.take()
    }

    /// Drops the device and the compositors (shutdown).
    pub(crate) fn close(&self) {
        let mut st = self.lock();
        st.frame = None;
        st.preview = None;
        st.ctx = None;
    }
}

/// Opens the device and both compositors on `choice`.
pub(crate) fn open(choice: &AdapterChoice) -> Result<Opened, String> {
    let ctx = GpuContext::new(choice).map_err(|e| e.to_string())?;
    let frame =
        Compositor::new(ctx.clone(), CompositorOptions::default()).map_err(|e| e.to_string())?;
    let preview =
        Compositor::new(ctx.clone(), CompositorOptions::default()).map_err(|e| e.to_string())?;
    Ok((ctx, frame, preview))
}

/// The renderer of `render.frame` and exports: the GPU compositor once it is open, the CPU
/// reference renderer without a GPU (see the module docs).
pub(crate) struct LateRenderer {
    gpu: Arc<LateGpu>,
    frames: Arc<MediaFrames>,
    backend: Option<GpuFrameBackend>,
    cpu: bool,
}

impl LateRenderer {
    pub(crate) fn new(gpu: Arc<LateGpu>, frames: Arc<MediaFrames>) -> LateRenderer {
        LateRenderer {
            gpu,
            frames,
            backend: None,
            cpu: false,
        }
    }

    /// Fetches the GPU backend once the device settled (waiting for it up to [`RENDER_WAIT`]).
    fn ready(&mut self) {
        if self.backend.is_some() || self.cpu {
            return;
        }
        if !self.gpu.wait_settled(RENDER_WAIT) {
            tracing::warn!("the GPU did not open in time; this frame renders on the CPU");
            return;
        }
        match self.gpu.take_frame() {
            Some(compositor) => {
                let source: Arc<dyn FrameSource + Send + Sync> = Arc::clone(&self.frames) as _;
                self.backend = Some(GpuFrameBackend::new(compositor, source));
            }
            None => self.cpu = true,
        }
    }
}

impl FrameBackend for LateRenderer {
    fn frame_source(&self) -> &dyn FrameSource {
        &*self.frames
    }

    fn graph_options(&self) -> GraphOptions {
        // The compositor's (`CompositorOptions::default().graph`), the same on the CPU.
        CompositorOptions::default().graph
    }

    fn rasterize(
        &mut self,
        scene: &SceneDescription,
        graph: &RenderGraph,
    ) -> Result<Image, String> {
        self.ready();
        match self.backend.as_mut() {
            Some(gpu) => gpu.rasterize(scene, graph),
            None => cutweave_render::reference::render(
                scene,
                graph.target,
                &*self.frames,
                &self.graph_options(),
            )
            .map_err(|e| e.to_string()),
        }
    }
}
