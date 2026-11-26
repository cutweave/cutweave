// SPDX-License-Identifier: GPL-3.0-or-later
//! The wgpu executor (WP-1F.02; arch §8): adapter and device management ([`GpuContext`]) and
//! the [`Compositor`] that runs [`RenderGraph`](crate::graph::RenderGraph)s in linear-light
//! `Rgba16Float`, with blocking or asynchronous readback ([`Compositor::submit`],
//! [`FrameTicket`], WP-1F.03), plus [`GpuFrameBackend`], the renderer of the `render.*`
//! queries.
//!
//! Native targets only (feature `gpu`); the wasm32 build of the crate is the pure core.
//!
//! # Adapter selection
//!
//! [`AdapterChoice`] picks an adapter; `CUTWEAVE_GPU` sets it from the environment
//! ([`AdapterChoice::from_env`]): `auto` (discrete, then integrated, then anything, software
//! last), `software` (WARP on Windows, lavapipe/llvmpipe on Linux), `integrated` / `igpu`,
//! `discrete` / `dgpu`, an index into [`list_adapters`], `key:<adapter key>`, or a
//! case-insensitive name substring. `WGPU_BACKEND` (`dx12`, `vulkan`, `metal`) restricts the
//! backends, as in every wgpu application.
//!
//! # Robustness (arch §8 "GPU robustness")
//!
//! * **Device loss** is detected through the device-lost callback and failed submissions; the
//!   compositor then recreates the device from a fresh `Instance` on the same adapter (wgpu
//!   cannot request a second device from one `Adapter`: S4 finding 8), rebuilds pipelines and
//!   drops every cache, and retries the frame once.
//! * **Validation errors** never panic: they are captured and returned as [`GpuError`].
//! * **Pipeline creation is serialised process-wide**: concurrent pipeline creation on several
//!   WARP devices crashes the process (S4 finding 7).
//! * A **watchdog** thread logs render frames slower than a threshold (500 ms by default)
//!   without putting a clock in the render path.

mod backend;
#[cfg(test)]
mod composite_tests;
mod compositor;
#[cfg(test)]
mod query_tests;
mod readback;
#[cfg(test)]
mod tests;
mod watchdog;

use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

pub use backend::GpuFrameBackend;
pub use compositor::{
    Compositor, CompositorOptions, FrameStats, RenderError, RenderedFrame, RenderedPixels,
};
pub use readback::{FrameTicket, ReadbackFrame};
pub use watchdog::Watchdog;

/// Process-wide lock around pipeline creation (S4 finding 7).
pub(crate) static PIPELINE_CREATION: Mutex<()> = Mutex::new(());

/// Environment variable selecting the adapter (see [`AdapterChoice::parse`]).
pub const ADAPTER_ENV: &str = "CUTWEAVE_GPU";

/// Errors from the GPU layer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GpuError {
    /// No adapter matches the choice.
    #[error("no GPU adapter matches `{choice}` (available: {available})")]
    NoAdapter {
        /// The requested choice.
        choice: String,
        /// The adapters present, comma-separated.
        available: String,
    },
    /// The adapter refused to create a device.
    #[error("cannot create a device on {adapter}: {reason}")]
    RequestDevice {
        /// Adapter name.
        adapter: String,
        /// wgpu's reason.
        reason: String,
    },
    /// The device was lost (driver reset, removal, or a simulated loss).
    #[error("GPU device lost on {0}")]
    DeviceLost(String),
    /// A validation or out-of-memory error (a bug or an exhausted device).
    #[error("GPU error: {0}")]
    Validation(String),
    /// Reading the frame back failed.
    #[error("readback failed: {0}")]
    Readback(String),
    /// Every readback staging buffer is in flight: receive frames before submitting more.
    #[error("all {0} readback buffers are in flight; receive frames before submitting more")]
    ReadbackBusy(usize),
}

impl GpuError {
    /// Whether the error means the device is gone.
    #[must_use]
    pub fn is_device_lost(&self) -> bool {
        match self {
            GpuError::DeviceLost(_) => true,
            GpuError::Validation(m) | GpuError::Readback(m) => {
                let m = m.to_ascii_lowercase();
                m.contains("lost") || m.contains("destroyed") || m.contains("invalid device")
            }
            _ => false,
        }
    }
}

/// Which adapter to use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AdapterChoice {
    /// Discrete, then integrated, then virtual/other GPUs; software last.
    #[default]
    Auto,
    /// The software rasteriser (WARP on Windows, lavapipe/llvmpipe on Linux).
    Software,
    /// The first integrated GPU.
    Integrated,
    /// The first discrete GPU.
    Discrete,
    /// Index into [`list_adapters`].
    Index(usize),
    /// Exact [`AdapterInfo::key`].
    Key(String),
    /// Case-insensitive substring of `"<name> <backend>"`.
    Name(String),
}

impl AdapterChoice {
    /// Parses `auto`, `software` (`warp`, `cpu`, `lavapipe`, `llvmpipe`), `integrated` (`igpu`),
    /// `discrete` (`dgpu`), an index, `key:<key>` or a name substring.
    #[must_use]
    pub fn parse(s: &str) -> AdapterChoice {
        let t = s.trim();
        match t.to_ascii_lowercase().as_str() {
            "" | "auto" => AdapterChoice::Auto,
            "software" | "warp" | "cpu" | "lavapipe" | "llvmpipe" => AdapterChoice::Software,
            "integrated" | "igpu" => AdapterChoice::Integrated,
            "discrete" | "dgpu" => AdapterChoice::Discrete,
            _ => {
                if let Some(k) = t.strip_prefix("key:") {
                    AdapterChoice::Key(k.to_string())
                } else if let Ok(i) = t.parse::<usize>() {
                    AdapterChoice::Index(i)
                } else {
                    AdapterChoice::Name(t.to_string())
                }
            }
        }
    }

    /// From `CUTWEAVE_GPU`, else [`AdapterChoice::Auto`].
    #[must_use]
    pub fn from_env() -> AdapterChoice {
        AdapterChoice::from_env_or(AdapterChoice::Auto)
    }

    /// From `CUTWEAVE_GPU`, else `default`.
    #[must_use]
    pub fn from_env_or(default: AdapterChoice) -> AdapterChoice {
        std::env::var(ADAPTER_ENV).map_or(default, |v| AdapterChoice::parse(&v))
    }
}

/// Description of one adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterInfo {
    /// Index in [`list_adapters`].
    pub index: usize,
    /// Adapter name.
    pub name: String,
    /// Backend (`dx12`, `vulkan`, `metal`).
    pub backend: String,
    /// `DiscreteGpu`, `IntegratedGpu`, `VirtualGpu`, `Cpu` or `Other`.
    pub device_type: String,
    /// Driver name and version.
    pub driver: String,
    /// Whether this is a software rasteriser.
    pub software: bool,
    /// Stable adapter-class key (`<backend>-<adapter>`): names golden reference sets.
    pub key: String,
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The adapter-class key: software rasterisers by family (their names carry versions), GPUs by
/// backend and name.
fn adapter_key(backend: &str, name: &str, software: bool) -> String {
    let lower = name.to_ascii_lowercase();
    let family = if !software {
        slug(name)
    } else if lower.contains("basic render") || lower.contains("warp") {
        "warp".to_string()
    } else if lower.contains("llvmpipe") || lower.contains("lavapipe") {
        "llvmpipe".to_string()
    } else if lower.contains("swiftshader") {
        "swiftshader".to_string()
    } else {
        slug(name)
    };
    format!("{backend}-{family}")
}

fn summarize(index: usize, adapter: &wgpu::Adapter) -> AdapterInfo {
    let info = adapter.get_info();
    let backend = format!("{:?}", info.backend).to_ascii_lowercase();
    let software = info.device_type == wgpu::DeviceType::Cpu;
    AdapterInfo {
        index,
        key: adapter_key(&backend, &info.name, software),
        name: info.name.clone(),
        backend,
        device_type: format!("{:?}", info.device_type),
        driver: format!("{} {}", info.driver, info.driver_info)
            .trim()
            .to_string(),
        software,
    }
}

fn instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
    // Validation layers off unless WGPU_* asks for them (they distort timings).
    desc.flags = wgpu::InstanceFlags::empty().with_env();
    desc.backend_options = std::mem::take(&mut desc.backend_options).with_env();
    wgpu::Instance::new(desc)
}

fn enumerate() -> Vec<wgpu::Adapter> {
    let inst = instance();
    let mut adapters = block_on(inst.enumerate_adapters(wgpu::Backends::all()));
    adapters.retain(|a| {
        matches!(
            a.get_info().backend,
            wgpu::Backend::Dx12 | wgpu::Backend::Vulkan | wgpu::Backend::Metal
        )
    });
    adapters
}

/// Every usable adapter on this machine.
#[must_use]
pub fn list_adapters() -> Vec<AdapterInfo> {
    enumerate()
        .iter()
        .enumerate()
        .map(|(i, a)| summarize(i, a))
        .collect()
}

fn pick(
    adapters: Vec<wgpu::Adapter>,
    choice: &AdapterChoice,
) -> Result<(AdapterInfo, wgpu::Adapter), GpuError> {
    let infos: Vec<AdapterInfo> = adapters
        .iter()
        .enumerate()
        .map(|(i, a)| summarize(i, a))
        .collect();
    let of_type = |t: &str| infos.iter().find(|s| s.device_type == t).map(|s| s.index);
    let idx = match choice {
        AdapterChoice::Auto => {
            let rank = |s: &AdapterInfo| match s.device_type.as_str() {
                "DiscreteGpu" => 0,
                "IntegratedGpu" => 1,
                "Cpu" => 3,
                _ => 2,
            };
            infos
                .iter()
                .min_by_key(|s| (rank(s), s.index))
                .map(|s| s.index)
        }
        AdapterChoice::Software => infos.iter().find(|s| s.software).map(|s| s.index),
        AdapterChoice::Integrated => of_type("IntegratedGpu"),
        AdapterChoice::Discrete => of_type("DiscreteGpu"),
        AdapterChoice::Index(i) => infos.get(*i).map(|s| s.index),
        AdapterChoice::Key(k) => infos.iter().find(|s| &s.key == k).map(|s| s.index),
        AdapterChoice::Name(n) => {
            let n = n.to_lowercase();
            infos
                .iter()
                .find(|s| {
                    format!("{} {}", s.name, s.backend)
                        .to_lowercase()
                        .contains(&n)
                })
                .map(|s| s.index)
        }
    };
    let no_adapter = || GpuError::NoAdapter {
        choice: format!("{choice:?}"),
        available: infos
            .iter()
            .map(|s| format!("{} ({})", s.key, s.device_type))
            .collect::<Vec<_>>()
            .join(", "),
    };
    let idx = idx.ok_or_else(no_adapter)?;
    let info = infos[idx].clone();
    let adapter = adapters.into_iter().nth(idx).ok_or_else(no_adapter)?;
    Ok((info, adapter))
}

/// A wgpu device and queue on one adapter, with device-lost and error tracking.
#[derive(Clone)]
pub struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    info: AdapterInfo,
    lost: Arc<AtomicBool>,
    errors: Arc<Mutex<Vec<String>>>,
    /// Set after an error is pushed: lets [`GpuContext::take_errors`] skip the lock on the
    /// render path while there is nothing to take.
    has_errors: Arc<AtomicBool>,
}

impl std::fmt::Debug for GpuContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuContext")
            .field("adapter", &self.info.key)
            .field("lost", &self.is_lost())
            .finish_non_exhaustive()
    }
}

impl GpuContext {
    /// Creates a device on the chosen adapter.
    pub fn new(choice: &AdapterChoice) -> Result<GpuContext, GpuError> {
        let (info, adapter) = pick(enumerate(), choice)?;
        let base = adapter.limits();
        let request = |limits: wgpu::Limits| {
            block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("cutweave-compositor"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits {
                    max_storage_buffer_binding_size: base.max_storage_buffer_binding_size,
                    max_buffer_size: base.max_buffer_size,
                    ..limits.using_resolution(base.clone())
                },
                ..Default::default()
            }))
        };
        let (device, queue) = request(wgpu::Limits::default())
            .or_else(|_| request(wgpu::Limits::downlevel_defaults()))
            .map_err(|e| GpuError::RequestDevice {
                adapter: info.name.clone(),
                reason: e.to_string(),
            })?;

        let lost = Arc::new(AtomicBool::new(false));
        let lost_cb = Arc::clone(&lost);
        let name = info.key.clone();
        device.set_device_lost_callback(move |reason, message| {
            lost_cb.store(true, Ordering::SeqCst);
            tracing::warn!(adapter = %name, ?reason, %message, "GPU device lost");
        });
        let errors = Arc::new(Mutex::new(Vec::new()));
        let has_errors = Arc::new(AtomicBool::new(false));
        let (errors_cb, flag_cb) = (Arc::clone(&errors), Arc::clone(&has_errors));
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            errors_cb
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(e.to_string());
            flag_cb.store(true, Ordering::Release);
        }));
        tracing::debug!(adapter = %info.key, driver = %info.driver, "GPU device created");
        Ok(GpuContext {
            device,
            queue,
            info,
            lost,
            errors,
            has_errors,
        })
    }

    /// Creates a device on the adapter `CUTWEAVE_GPU` selects (default: auto).
    pub fn from_env() -> Result<GpuContext, GpuError> {
        GpuContext::new(&AdapterChoice::from_env())
    }

    /// The adapter.
    #[must_use]
    pub fn info(&self) -> &AdapterInfo {
        &self.info
    }

    /// The device.
    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The queue.
    #[must_use]
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Whether the device has been lost.
    #[must_use]
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    /// Destroys the device (a test hook for the device-lost path).
    pub fn simulate_device_loss(&self) {
        self.device.destroy();
        // Deliver the lost callback now rather than on the next poll.
        let _ = self.device.poll(wgpu::PollType::Poll);
        self.lost.store(true, Ordering::SeqCst);
    }

    /// A new device on the same adapter, from a fresh instance.
    pub fn recreate(&self) -> Result<GpuContext, GpuError> {
        GpuContext::new(&AdapterChoice::Key(self.info.key.clone()))
            .or_else(|_| GpuContext::new(&AdapterChoice::Index(self.info.index)))
    }

    /// Takes the uncaptured errors reported since the last call, joined. Lock-free while there
    /// are none (the render path calls it every frame).
    pub(crate) fn take_errors(&self) -> Option<String> {
        if !self.has_errors.swap(false, Ordering::AcqRel) {
            return None;
        }
        let mut v = self.errors.lock().unwrap_or_else(PoisonError::into_inner);
        if v.is_empty() {
            None
        } else {
            Some(v.drain(..).collect::<Vec<_>>().join("; "))
        }
    }
}

struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs a future to completion on the current thread (wgpu's native futures resolve
/// immediately or after a device poll; no executor is needed).
pub(crate) fn block_on<F: Future>(fut: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        thread::park();
    }
}
