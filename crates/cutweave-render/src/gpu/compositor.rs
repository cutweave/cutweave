// SPDX-License-Identifier: GPL-3.0-or-later
//! The [`Compositor`]: executes [`RenderGraph`]s on a [`GpuContext`].
//!
//! * **Decode** (compute): frame planes are uploaded into pooled integer textures (CPU path (a),
//!   arch §7) and converted to premultiplied linear Rec.709 `Rgba16Float`; when a layer is
//!   minified, a mip chain follows (2×2 box compute passes). Decoded textures live in a
//!   VRAM-budgeted LRU keyed by the pass's Merkle key, so stills and paused frames are uploaded
//!   once.
//! * **Composite** (one render pass per run of consecutive layers): each layer draws a quad over
//!   its output-pixel bounds onto the canvas with fixed-function premultiplied "over"; the
//!   canvas is cleared to the background colour by the pass's load op. Cost follows the layer
//!   area, not the canvas area (S4 finding 5a).
//! * **Output** (compute): un-premultiply, encode, explicit round-half-up quantisation into a
//!   storage buffer, then readback.
//!
//! Determinism (arch §8): the same graph renders to the same bytes on the same adapter whatever
//! the cache and pool state (cold or warm, recycled textures, after device recovery); the render
//! path reads no clock.
//!
//! Known per-frame allocations (bind groups, texture views, the command encoder, the readback
//! `Vec`) remain; WP-1F.03 brings double-buffered async readback.

#![allow(clippy::cast_possible_truncation)]

use std::collections::HashMap;
use std::sync::PoisonError;
use std::sync::mpsc;
use std::time::Duration;

use cutweave_model::ItemId;

use super::{GpuContext, GpuError, PIPELINE_CREATION, Watchdog, block_on};
use crate::color::{Eetf, OutputTransfer};
use crate::frame::{FrameError, FrameSource, PixelFormat, VideoFrame};
use crate::geometry::mip_size;
use crate::graph::{
    BuildStats, CompositeOp, DecodeOp, GraphError, GraphOptions, Pass, PassOp, RenderGraph, Target,
    build,
};
use crate::hash::CacheKey;
use crate::image::Image;
use crate::scene::{BlendMode, SceneDescription};

const COMMON_WGSL: &str = include_str!("shaders/common.wgsl");
const DECODE_WGSL: &str = include_str!("shaders/decode.wgsl");
const MIP_WGSL: &str = include_str!("shaders/mip.wgsl");
const COMPOSITE_WGSL: &str = include_str!("shaders/composite.wgsl");
const OUTPUT_WGSL: &str = include_str!("shaders/output.wgsl");

const WORKING_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const UNIFORM_SIZE: u64 = 256;
/// Pooled textures kept per (kind, size) beyond the current frame.
const POOL_PER_KEY: usize = 4;

/// Compositor settings.
#[derive(Clone, Debug, PartialEq)]
pub struct CompositorOptions {
    /// Settings that change pixels (tone map).
    pub graph: GraphOptions,
    /// VRAM budget for decoded frames kept across renders, in bytes.
    pub cache_budget_bytes: u64,
    /// Frames slower than this are logged by the watchdog.
    pub watchdog_threshold: Duration,
}

impl Default for CompositorOptions {
    fn default() -> CompositorOptions {
        CompositorOptions {
            graph: GraphOptions::default(),
            cache_budget_bytes: 256 << 20,
            watchdog_threshold: Duration::from_millis(500),
        }
    }
}

/// Why a render failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    /// The scene could not be compiled.
    #[error(transparent)]
    Graph(#[from] GraphError),
    /// The GPU failed (after one recovery attempt for device loss).
    #[error(transparent)]
    Gpu(#[from] GpuError),
}

/// Per-frame execution statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// Graph build statistics (culling, occlusion, sharing).
    pub graph: BuildStats,
    /// Passes executed (cache hits excluded).
    pub passes_run: usize,
    /// Frames uploaded and converted.
    pub decodes_run: usize,
    /// Decodes served from the texture cache.
    pub cache_hits: usize,
    /// GPU textures created (not taken from the pool or cache).
    pub textures_created: usize,
    /// The device was lost and recreated during this render.
    pub recovered: bool,
}

/// The pixels of a rendered frame.
#[derive(Debug)]
pub enum RenderedPixels {
    /// Straight-alpha RGBA8 read back to the CPU ([`Target::Rgba8Cpu`]).
    Rgba8(Image),
    /// The premultiplied linear `Rgba16Float` canvas ([`Target::Texture`]); the caller owns it.
    Texture(wgpu::Texture),
}

/// A rendered frame.
#[derive(Debug)]
pub struct RenderedFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The pixels.
    pub pixels: RenderedPixels,
    /// The frame's subgraph key (same key ⇒ same pixels on the same adapter).
    pub key: CacheKey,
    /// Layers skipped because their frame was unavailable.
    pub missing: Vec<(ItemId, FrameError)>,
    /// Execution statistics.
    pub stats: FrameStats,
}

impl RenderedFrame {
    /// The RGBA8 image, for CPU targets.
    #[must_use]
    pub fn image(&self) -> Option<&Image> {
        match &self.pixels {
            RenderedPixels::Rgba8(img) => Some(img),
            RenderedPixels::Texture(_) => None,
        }
    }

    /// Takes the RGBA8 image, for CPU targets.
    #[must_use]
    pub fn into_image(self) -> Option<Image> {
        match self.pixels {
            RenderedPixels::Rgba8(img) => Some(img),
            RenderedPixels::Texture(_) => None,
        }
    }
}

struct ComputePipe {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

struct RenderPipe {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

struct Pipelines {
    decode: ComputePipe,
    mip: ComputePipe,
    output: ComputePipe,
    composite: RenderPipe,
}

#[derive(Clone, Copy)]
enum Slot {
    Uniform,
    TexU,
    TexF,
    Store,
    Buffer,
}

fn layout_entries(
    slots: &[(u32, Slot)],
    visibility: wgpu::ShaderStages,
) -> Vec<wgpu::BindGroupLayoutEntry> {
    slots
        .iter()
        .map(|&(binding, slot)| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: match slot {
                Slot::Uniform => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                Slot::TexU | Slot::TexF => wgpu::BindingType::Texture {
                    sample_type: if matches!(slot, Slot::TexU) {
                        wgpu::TextureSampleType::Uint
                    } else {
                        wgpu::TextureSampleType::Float { filterable: false }
                    },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                Slot::Store => wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: WORKING_FORMAT,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                Slot::Buffer => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            },
            count: None,
        })
        .collect()
}

fn compute_pipe(
    device: &wgpu::Device,
    label: &str,
    source: &str,
    slots: &[(u32, Slot)],
) -> ComputePipe {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(format!("{COMMON_WGSL}\n{source}").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &layout_entries(slots, wgpu::ShaderStages::COMPUTE),
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&pl),
        module: &module,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    ComputePipe { pipeline, layout }
}

fn create_pipelines(device: &wgpu::Device) -> Result<Pipelines, GpuError> {
    use Slot::{Buffer, Store, TexF, TexU, Uniform};
    let _serialised = PIPELINE_CREATION
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let decode = compute_pipe(
        device,
        "decode",
        DECODE_WGSL,
        &[(0, Uniform), (1, TexU), (2, TexU), (3, TexU), (4, Store)],
    );
    let mip = compute_pipe(device, "mip", MIP_WGSL, &[(0, TexF), (1, Store)]);
    let output = compute_pipe(
        device,
        "output",
        OUTPUT_WGSL,
        &[(0, Uniform), (1, TexF), (2, Buffer)],
    );

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("composite"),
        source: wgpu::ShaderSource::Wgsl(format!("{COMMON_WGSL}\n{COMPOSITE_WGSL}").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("composite"),
        entries: &[
            layout_entries(
                &[(0, Uniform)],
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ),
            layout_entries(&[(1, TexF)], wgpu::ShaderStages::FRAGMENT),
        ]
        .concat(),
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("composite"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("composite"),
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: WORKING_FORMAT,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    if let Some(e) = block_on(scope.pop()) {
        return Err(GpuError::Validation(format!("pipeline creation: {e}")));
    }
    Ok(Pipelines {
        decode,
        mip,
        output,
        composite: RenderPipe { pipeline, layout },
    })
}

/// Packs uniform data as 16-byte `vec4` slots.
#[derive(Default)]
struct Uniforms(Vec<u8>);

impl Uniforms {
    fn f4(mut self, v: [f64; 4]) -> Self {
        for x in v {
            self.0.extend_from_slice(&(x as f32).to_le_bytes());
        }
        self
    }

    fn u4(mut self, v: [u32; 4]) -> Self {
        for x in v {
            self.0.extend_from_slice(&x.to_le_bytes());
        }
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum TexKind {
    Plane(wgpu::TextureFormat),
    Content(u32),
    Canvas,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct TexKey {
    kind: TexKind,
    width: u32,
    height: u32,
}

impl TexKey {
    fn bytes(&self) -> u64 {
        let px = u64::from(self.width) * u64::from(self.height);
        match self.kind {
            // A full mip chain adds at most a third.
            TexKind::Content(mips) if mips > 1 => px * 8 * 4 / 3,
            TexKind::Content(_) | TexKind::Canvas => px * 8,
            TexKind::Plane(_) => px * 4,
        }
    }
}

struct CacheEntry {
    texture: wgpu::Texture,
    key: TexKey,
    last_used: u64,
}

/// Device-bound state: everything dropped and rebuilt on recovery.
struct DeviceState {
    pipes: Pipelines,
    dummy_plane: wgpu::Texture,
    pool: HashMap<TexKey, Vec<wgpu::Texture>>,
    cache: HashMap<CacheKey, CacheEntry>,
    cache_bytes: u64,
    uniforms: Vec<wgpu::Buffer>,
    readback: Option<(u64, wgpu::Buffer, wgpu::Buffer)>,
    frame_no: u64,
}

impl DeviceState {
    fn new(ctx: &GpuContext) -> Result<DeviceState, GpuError> {
        let pipes = create_pipelines(ctx.device())?;
        let dummy_plane = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("unused plane"),
            size: extent(1, 1),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Ok(DeviceState {
            pipes,
            dummy_plane,
            pool: HashMap::new(),
            cache: HashMap::new(),
            cache_bytes: 0,
            uniforms: Vec::new(),
            readback: None,
            frame_no: 0,
        })
    }

    fn take(
        &mut self,
        device: &wgpu::Device,
        key: TexKey,
        stats: &mut FrameStats,
    ) -> wgpu::Texture {
        if let Some(t) = self.pool.get_mut(&key).and_then(Vec::pop) {
            return t;
        }
        stats.textures_created += 1;
        let (format, usage, mips) = match key.kind {
            TexKind::Plane(f) => (
                f,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                1,
            ),
            TexKind::Content(m) => (
                WORKING_FORMAT,
                wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                m,
            ),
            TexKind::Canvas => (
                WORKING_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                1,
            ),
        };
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(match key.kind {
                TexKind::Plane(_) => "plane",
                TexKind::Content(_) => "decoded",
                TexKind::Canvas => "canvas",
            }),
            size: extent(key.width, key.height),
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }

    fn give(&mut self, key: TexKey, texture: wgpu::Texture) {
        let slot = self.pool.entry(key).or_default();
        if slot.len() < POOL_PER_KEY {
            slot.push(texture);
        }
    }

    /// Caches a decoded texture, evicting least-recently-used entries not used by this frame.
    /// Returns the texture back when it does not fit.
    fn cache_insert(
        &mut self,
        key: CacheKey,
        tex_key: TexKey,
        texture: wgpu::Texture,
        budget: u64,
    ) -> Option<wgpu::Texture> {
        let bytes = tex_key.bytes();
        while self.cache_bytes + bytes > budget {
            let victim = self
                .cache
                .iter()
                .filter(|(_, e)| e.last_used < self.frame_no)
                .min_by_key(|(k, e)| (e.last_used, **k))
                .map(|(k, _)| *k);
            let Some(victim) = victim else {
                return Some(texture);
            };
            if let Some(e) = self.cache.remove(&victim) {
                self.cache_bytes -= e.key.bytes();
                self.give(e.key, e.texture);
            }
        }
        self.cache_bytes += bytes;
        self.cache.insert(
            key,
            CacheEntry {
                texture,
                key: tex_key,
                last_used: self.frame_no,
            },
        );
        None
    }

    fn uniform(&mut self, device: &wgpu::Device, slot: usize) -> wgpu::Buffer {
        while self.uniforms.len() <= slot {
            self.uniforms
                .push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("uniforms"),
                    size: UNIFORM_SIZE,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        self.uniforms[slot].clone()
    }

    fn readback(&mut self, device: &wgpu::Device, bytes: u64) -> (wgpu::Buffer, wgpu::Buffer) {
        if let Some((size, storage, map)) = &self.readback
            && *size == bytes
        {
            return (storage.clone(), map.clone());
        }
        let storage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("output"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let map = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.readback = Some((bytes, storage.clone(), map.clone()));
        (storage, map)
    }
}

fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

fn plane_format(format: PixelFormat, plane: usize) -> wgpu::TextureFormat {
    use wgpu::TextureFormat as F;
    match (format, plane) {
        (PixelFormat::Nv12, 0) | (PixelFormat::Yuv420p, _) => F::R8Uint,
        (PixelFormat::Nv12, _) => F::Rg8Uint,
        (PixelFormat::P010, 0) => F::R16Uint,
        (PixelFormat::P010, _) => F::Rg16Uint,
        (PixelFormat::Rgba8, _) => F::Rgba8Uint,
    }
}

/// A texture held for one resource during a frame.
struct Held {
    texture: wgpu::Texture,
    key: TexKey,
    /// Owned by this frame (returned to the pool after its last reader), not the cache.
    transient: bool,
    /// A clear not yet applied (the canvas is cleared by the first render pass on it).
    clear: Option<wgpu::Color>,
}

fn view(t: &wgpu::Texture, mip: Option<u32>) -> wgpu::TextureView {
    t.create_view(&wgpu::TextureViewDescriptor {
        base_mip_level: mip.unwrap_or(0),
        mip_level_count: mip.map(|_| 1),
        ..Default::default()
    })
}

/// Executes render graphs on one GPU device.
pub struct Compositor {
    ctx: GpuContext,
    options: CompositorOptions,
    dev: DeviceState,
    watchdog: Watchdog,
}

impl std::fmt::Debug for Compositor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compositor")
            .field("adapter", &self.ctx.info().key)
            .field("cached_frames", &self.dev.cache.len())
            .finish_non_exhaustive()
    }
}

impl Compositor {
    /// A compositor on `ctx`: builds the pipelines (once per device) and starts the watchdog.
    pub fn new(ctx: GpuContext, options: CompositorOptions) -> Result<Compositor, GpuError> {
        let dev = DeviceState::new(&ctx)?;
        let watchdog = Watchdog::spawn(options.watchdog_threshold, ctx.info().key.clone());
        Ok(Compositor {
            ctx,
            options,
            dev,
            watchdog,
        })
    }

    /// The GPU context.
    #[must_use]
    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    /// The settings.
    #[must_use]
    pub fn options(&self) -> &CompositorOptions {
        &self.options
    }

    /// Frames the watchdog reported as too slow.
    #[must_use]
    pub fn watchdog_stalls(&self) -> u64 {
        self.watchdog.stalls()
    }

    /// Drops every cached decoded frame (the next render is cold).
    pub fn clear_cache(&mut self) {
        let entries: Vec<CacheEntry> = self.dev.cache.drain().map(|(_, e)| e).collect();
        for e in entries {
            self.dev.give(e.key, e.texture);
        }
        self.dev.cache_bytes = 0;
    }

    /// Destroys the device (test hook for the device-lost path); the next render recovers.
    pub fn simulate_device_loss(&self) {
        self.ctx.simulate_device_loss();
    }

    /// Device-lost recovery (arch §8): a new device on the same adapter, new pipelines, empty
    /// caches.
    pub fn recover(&mut self) -> Result<(), GpuError> {
        tracing::warn!(adapter = %self.ctx.info().key, "recreating the GPU device");
        let ctx = self.ctx.recreate()?;
        self.dev = DeviceState::new(&ctx)?;
        self.ctx = ctx;
        Ok(())
    }

    /// Compiles `scene` for `target` (frames from `source`) and renders it.
    pub fn render(
        &mut self,
        scene: &SceneDescription,
        source: &dyn FrameSource,
        target: Target,
    ) -> Result<RenderedFrame, RenderError> {
        let graph = build(scene, target, source, &self.options.graph)?;
        self.execute(&graph)
    }

    /// Executes a compiled graph. A lost device is recreated and the frame retried once.
    pub fn execute(&mut self, graph: &RenderGraph) -> Result<RenderedFrame, RenderError> {
        let _frame = self.watchdog.begin();
        let mut recovered = false;
        if self.ctx.is_lost() {
            self.recover()?;
            recovered = true;
        }
        let first = self.execute_once(graph);
        let result = match first {
            Err(RenderError::Gpu(e))
                if !recovered && (self.ctx.is_lost() || e.is_device_lost()) =>
            {
                tracing::warn!(adapter = %self.ctx.info().key, error = %e, "render failed on a lost device; retrying");
                self.recover()?;
                recovered = true;
                self.execute_once(graph)
            }
            other => other,
        };
        result.map(|mut f| {
            f.stats.recovered = recovered;
            f
        })
    }

    fn execute_once(&mut self, graph: &RenderGraph) -> Result<RenderedFrame, RenderError> {
        let order = graph.topo_order()?;
        let last = graph.last_uses(&order);
        self.dev.frame_no += 1;
        let _ = self.ctx.take_errors();
        let mut f = Frame {
            encoder: self
                .ctx
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("frame"),
                }),
            held: (0..graph.resources.len()).map(|_| None).collect(),
            uploads: Vec::new(),
            readback: None,
            stats: FrameStats {
                graph: graph.stats,
                ..FrameStats::default()
            },
        };
        let mut pos = 0;
        while pos < order.len() {
            let pass = &graph.passes[order[pos]];
            let mut end = pos;
            match &pass.op {
                PassOp::Decode(d) => self.run_decode(&mut f, graph, pass, d, pos),
                PassOp::Background { color } => {
                    self.run_background(&mut f, pass.output, *color, graph.target.size());
                }
                PassOp::Composite(_) => {
                    // A run of composites chained on one canvas shares a render pass.
                    while end + 1 < order.len() {
                        let next = &graph.passes[order[end + 1]];
                        let chained = next.inputs.first() == Some(&graph.passes[order[end]].output);
                        if !(matches!(next.op, PassOp::Composite(_)) && chained) {
                            break;
                        }
                        end += 1;
                    }
                    let last_reader = last[pass.inputs[0]] == Some(pos);
                    self.run_composites(&mut f, graph, &order[pos..=end], pos, last_reader)?;
                }
                PassOp::Output { transfer } => {
                    self.run_output(&mut f, pass.inputs[0], *transfer, graph.target.size(), pos)?;
                }
            }
            // Recycle transient textures after their last reader.
            for p in &order[pos..=end] {
                for &r in &graph.passes[*p].inputs {
                    if last[r].is_some_and(|l| l <= end)
                        && f.held[r].as_ref().is_some_and(|h| h.transient)
                        && let Some(h) = f.held[r].take()
                    {
                        self.dev.give(h.key, h.texture);
                    }
                }
            }
            pos = end + 1;
        }
        self.finish(f, graph)
    }

    /// Submits the frame and produces its pixels.
    fn finish(&mut self, f: Frame, graph: &RenderGraph) -> Result<RenderedFrame, RenderError> {
        let (width, height) = graph.target.size();
        let Frame {
            mut encoder,
            mut held,
            uploads,
            readback,
            stats,
        } = f;
        let queue = self.ctx.queue().clone();
        let pixels = match graph.target {
            Target::Rgba8Cpu { .. } => {
                let (map, bytes) = readback.ok_or_else(|| {
                    GraphError::Invalid("CPU target without an output pass".into())
                })?;
                queue.submit([encoder.finish()]);
                self.recycle(&mut held, uploads);
                let data = self.read(&map, bytes)?;
                RenderedPixels::Rgba8(Image {
                    width,
                    height,
                    data,
                })
            }
            Target::Texture { .. } => {
                flush_clear(&mut encoder, &mut held, graph.output);
                let out = held[graph.output]
                    .take()
                    .ok_or_else(|| GraphError::Invalid("graph output not ready".into()))?;
                queue.submit([encoder.finish()]);
                self.recycle(&mut held, uploads);
                let _ = self.ctx.device().poll(wgpu::PollType::Poll);
                self.check()?;
                RenderedPixels::Texture(out.texture)
            }
        };
        Ok(RenderedFrame {
            width,
            height,
            pixels,
            key: graph.output_key(),
            missing: graph.missing.clone(),
            stats,
        })
    }

    /// Returns this frame's transient textures and uploads to the pool (after submit).
    fn recycle(&mut self, held: &mut [Option<Held>], uploads: Vec<(TexKey, wgpu::Texture)>) {
        for h in held.iter_mut().filter_map(Option::take) {
            if h.transient {
                self.dev.give(h.key, h.texture);
            }
        }
        for (k, t) in uploads {
            self.dev.give(k, t);
        }
    }

    fn check(&self) -> Result<(), GpuError> {
        if self.ctx.is_lost() {
            return Err(GpuError::DeviceLost(self.ctx.info().name.clone()));
        }
        match self.ctx.take_errors() {
            Some(e) => Err(GpuError::Validation(e)),
            None => Ok(()),
        }
    }

    fn read(&self, map: &wgpu::Buffer, bytes: u64) -> Result<Vec<u8>, GpuError> {
        let (tx, rx) = mpsc::channel();
        map.slice(..bytes).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let poll = self.ctx.device().poll(wgpu::PollType::wait_indefinitely());
        self.check()?;
        poll.map_err(|e| GpuError::Readback(format!("poll: {e}")))?;
        rx.recv()
            .map_err(|e| GpuError::Readback(e.to_string()))?
            .map_err(|e| GpuError::Readback(format!("map: {e}")))?;
        let data = {
            let range = map
                .get_mapped_range(..bytes)
                .map_err(|e| GpuError::Readback(format!("{e:?}")))?;
            range.to_vec()
        };
        map.unmap();
        Ok(data)
    }

    /// A decode pass: a cache hit, or upload + convert (+ mips) into a cached texture.
    fn run_decode(
        &mut self,
        f: &mut Frame,
        graph: &RenderGraph,
        pass: &Pass,
        d: &DecodeOp,
        slot: usize,
    ) {
        let frame = &graph.frames[d.frame];
        let key = TexKey {
            kind: TexKind::Content(d.mip_levels.max(1)),
            width: frame.width,
            height: frame.height,
        };
        let frame_no = self.dev.frame_no;
        if let Some(e) = self.dev.cache.get_mut(&pass.key) {
            e.last_used = frame_no;
            f.stats.cache_hits += 1;
            f.held[pass.output] = Some(Held {
                texture: e.texture.clone(),
                key,
                transient: false,
                clear: None,
            });
            return;
        }
        let device = self.ctx.device().clone();
        let texture = self.dev.take(&device, key, &mut f.stats);
        self.encode_decode(f, frame, d, &texture, slot);
        let budget = self.options.cache_budget_bytes;
        let transient = self
            .dev
            .cache_insert(pass.key, key, texture.clone(), budget)
            .is_some();
        f.held[pass.output] = Some(Held {
            texture,
            key,
            transient,
            clear: None,
        });
    }

    /// A background pass: a pooled canvas with a pending clear (applied by the next render
    /// pass on it).
    fn run_background(&mut self, f: &mut Frame, output: usize, color: [f64; 4], size: (u32, u32)) {
        let key = TexKey {
            kind: TexKind::Canvas,
            width: size.0,
            height: size.1,
        };
        let device = self.ctx.device().clone();
        let texture = self.dev.take(&device, key, &mut f.stats);
        f.held[output] = Some(Held {
            texture,
            key,
            transient: true,
            clear: Some(wgpu::Color {
                r: color[0],
                g: color[1],
                b: color[2],
                a: color[3],
            }),
        });
        f.stats.passes_run += 1;
    }

    /// A run of chained composites: one render pass on the backdrop canvas.
    fn run_composites(
        &mut self,
        f: &mut Frame,
        graph: &RenderGraph,
        run: &[usize],
        first_slot: usize,
        last_reader: bool,
    ) -> Result<(), GraphError> {
        let backdrop = graph.passes[run[0]].inputs[0];
        let mut canvas = self.take_canvas(f, backdrop, last_reader)?;
        self.encode_composites(f, graph, run, first_slot, &mut canvas)?;
        f.stats.passes_run += run.len();
        f.held[graph.passes[run[run.len() - 1]].output] = Some(canvas);
        Ok(())
    }

    /// The output pass: encode + quantise into the readback buffer.
    fn run_output(
        &mut self,
        f: &mut Frame,
        input: usize,
        transfer: OutputTransfer,
        (width, height): (u32, u32),
        slot: usize,
    ) -> Result<(), GraphError> {
        flush_clear(&mut f.encoder, &mut f.held, input);
        let src = f.held[input]
            .as_ref()
            .ok_or_else(|| GraphError::Invalid("output input not ready".into()))?;
        let device = self.ctx.device().clone();
        let bytes = u64::from(width) * u64::from(height) * 4;
        let (storage, map) = self.dev.readback(&device, bytes);
        let uniform = self.dev.uniform(&device, slot);
        let u = Uniforms::default().u4([width, height, transfer.transfer().shader_id(), 0]);
        self.ctx.queue().write_buffer(&uniform, 0, &u.0);
        let src_view = view(&src.texture, None);
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("output"),
            layout: &self.dev.pipes.output.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: storage.as_entire_binding(),
                },
            ],
        });
        {
            let mut cp = f.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("output"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.dev.pipes.output.pipeline);
            cp.set_bind_group(0, &bind, &[]);
            cp.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }
        f.encoder.copy_buffer_to_buffer(&storage, 0, &map, 0, bytes);
        f.readback = Some((map, bytes));
        f.stats.passes_run += 1;
        Ok(())
    }

    /// Takes the backdrop canvas for a composite run: moved when the run is its last reader,
    /// else copied (or, with a pending clear, a fresh canvas with the same clear).
    fn take_canvas(
        &mut self,
        f: &mut Frame,
        backdrop: usize,
        last_reader: bool,
    ) -> Result<Held, GraphError> {
        let not_ready = || GraphError::Invalid("composite backdrop not ready".into());
        let src = f.held[backdrop].as_ref().ok_or_else(not_ready)?;
        if last_reader && src.transient {
            return f.held[backdrop].take().ok_or_else(not_ready);
        }
        let key = TexKey {
            kind: TexKind::Canvas,
            width: src.key.width,
            height: src.key.height,
        };
        let (clear, source) = (src.clear, src.texture.clone());
        let device = self.ctx.device().clone();
        let texture = self.dev.take(&device, key, &mut f.stats);
        if clear.is_none() {
            f.encoder.copy_texture_to_texture(
                source.as_image_copy(),
                texture.as_image_copy(),
                source.size(),
            );
        }
        Ok(Held {
            texture,
            key,
            transient: true,
            clear,
        })
    }

    fn encode_decode(
        &mut self,
        f: &mut Frame,
        frame: &VideoFrame,
        d: &DecodeOp,
        texture: &wgpu::Texture,
        slot: usize,
    ) {
        let device = self.ctx.device().clone();
        let queue = self.ctx.queue().clone();
        let mut plane_views = Vec::with_capacity(3);
        for (i, plane) in frame.planes.iter().enumerate() {
            let (pw, ph, _) = frame.format.plane_layout(i, frame.width, frame.height);
            let key = TexKey {
                kind: TexKind::Plane(plane_format(frame.format, i)),
                width: pw,
                height: ph,
            };
            let t = self.dev.take(&device, key, &mut f.stats);
            queue.write_texture(
                t.as_image_copy(),
                &plane.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(plane.stride),
                    rows_per_image: Some(ph),
                },
                extent(pw, ph),
            );
            plane_views.push(view(&t, None));
            f.uploads.push((key, t));
        }
        while plane_views.len() < 3 {
            plane_views.push(view(&self.dev.dummy_plane, None));
        }
        let uniform = self.dev.uniform(&device, slot);
        queue.write_buffer(&uniform, 0, &decode_uniforms(frame, d).0);
        let dst = view(texture, Some(0));
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }];
        for (binding, v) in (1u32..).zip(&plane_views) {
            entries.push(wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(v),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 4,
            resource: wgpu::BindingResource::TextureView(&dst),
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("decode"),
            layout: &self.dev.pipes.decode.layout,
            entries: &entries,
        });
        {
            let mut cp = f.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("decode"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.dev.pipes.decode.pipeline);
            cp.set_bind_group(0, &bind, &[]);
            cp.dispatch_workgroups(frame.width.div_ceil(8), frame.height.div_ceil(8), 1);
        }
        // One compute pass per level, so each level is complete before the next reads it.
        let mips = d.mip_levels.max(1);
        for level in 1..mips {
            let (src, dst) = (view(texture, Some(level - 1)), view(texture, Some(level)));
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mip"),
                layout: &self.dev.pipes.mip.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&dst),
                    },
                ],
            });
            let (w, h) = mip_size(frame.width, frame.height, level);
            let mut cp = f.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("mip"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.dev.pipes.mip.pipeline);
            cp.set_bind_group(0, &bind, &[]);
            cp.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
        f.stats.decodes_run += 1;
        f.stats.passes_run += mips as usize;
    }

    fn encode_composites(
        &mut self,
        f: &mut Frame,
        graph: &RenderGraph,
        run: &[usize],
        first_slot: usize,
        canvas: &mut Held,
    ) -> Result<(), GraphError> {
        let device = self.ctx.device().clone();
        let output = graph.target.size();
        let mut draws = Vec::with_capacity(run.len());
        for (i, &pi) in run.iter().enumerate() {
            let pass = &graph.passes[pi];
            let PassOp::Composite(c) = &pass.op else {
                return Err(GraphError::Invalid("not a composite".into()));
            };
            let content = f.held[pass.inputs[1]]
                .as_ref()
                .ok_or_else(|| GraphError::Invalid("composite content not ready".into()))?;
            let uniform = self.dev.uniform(&device, first_slot + i);
            self.ctx.queue().write_buffer(
                &uniform,
                0,
                &composite_uniforms(c, content.key, output).0,
            );
            let content_view = view(&content.texture, None);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("composite"),
                layout: &self.dev.pipes.composite.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&content_view),
                    },
                ],
            });
            draws.push((bind, c.rect));
        }
        let target = view(&canvas.texture, None);
        let load = canvas
            .clear
            .take()
            .map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear);
        let mut rp = f.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rp.set_pipeline(&self.dev.pipes.composite.pipeline);
        for (bind, rect) in &draws {
            if rect[2] <= rect[0] || rect[3] <= rect[1] {
                continue;
            }
            rp.set_bind_group(0, bind, &[]);
            rp.draw(0..6, 0..1);
        }
        Ok(())
    }
}

/// Per-frame execution state.
struct Frame {
    encoder: wgpu::CommandEncoder,
    /// The texture of each graph resource while it is live.
    held: Vec<Option<Held>>,
    /// Plane textures written this frame: back to the pool only after submit (queue writes run
    /// before the submission's commands).
    uploads: Vec<(TexKey, wgpu::Texture)>,
    /// The mappable readback buffer and its size.
    readback: Option<(wgpu::Buffer, u64)>,
    stats: FrameStats,
}

/// Applies a pending clear with an empty render pass.
fn flush_clear(encoder: &mut wgpu::CommandEncoder, held: &mut [Option<Held>], res: usize) {
    if let Some(h) = held[res].as_mut()
        && let Some(color) = h.clear.take()
    {
        let v = view(&h.texture, None);
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &v,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
}

/// The decode shader's uniforms (layout: `DecodeParams` in `decode.wgsl`).
fn decode_uniforms(frame: &VideoFrame, d: &DecodeOp) -> Uniforms {
    let t = &d.transform;
    let (kr, kb) = t.kr_kb.unwrap_or((0.2126, 0.0722));
    let (cw, ch) = (frame.width.div_ceil(2), frame.height.div_ceil(2));
    let layout = match frame.format {
        PixelFormat::Nv12 | PixelFormat::P010 => 0,
        PixelFormat::Yuv420p => 1,
        PixelFormat::Rgba8 => 2,
    };
    let shift = if frame.format == PixelFormat::P010 {
        6
    } else {
        0
    };
    let flags = u32::from(t.tone_map.is_some()) | (u32::from(t.hlg_ootf.is_some()) << 1);
    let (luma, gamma) = t.hlg_ootf.unwrap_or(([0.0; 3], 0.0));
    let tm = t.tone_map.unwrap_or(Eetf {
        src_min_pq: 0.0,
        src_max_pq: 1.0,
        min_lum: 0.0,
        max_lum: 1.0,
        ks: 1.0,
        sdr_white_nits: 1.0,
    });
    let off = frame.chroma_siting.offset();
    let p = &t.primaries;
    Uniforms::default()
        .u4([frame.width, frame.height, cw, ch])
        .u4([layout, shift, t.transfer.shader_id(), flags])
        .f4([
            t.range.y_off,
            t.range.y_scale,
            t.range.c_off,
            t.range.c_scale,
        ])
        .f4([kr, kb, 1.0 - kr - kb, t.linear_scale])
        .f4([off[0], off[1], gamma, 0.0])
        .f4([p[0][0], p[0][1], p[0][2], 0.0])
        .f4([p[1][0], p[1][1], p[1][2], 0.0])
        .f4([p[2][0], p[2][1], p[2][2], 0.0])
        .f4([tm.src_min_pq, tm.src_max_pq, tm.min_lum, tm.max_lum])
        .f4([tm.ks, tm.sdr_white_nits, 0.0, 0.0])
        .f4([luma[0], luma[1], luma[2], 0.0])
}

/// The composite shader's uniforms (layout: `CompositeParams` in `composite.wgsl`).
fn composite_uniforms(c: &CompositeOp, content: TexKey, output: (u32, u32)) -> Uniforms {
    let geo = &c.geometry;
    let (i, t) = (&geo.from_output, &geo.to_texel);
    let mips = match content.kind {
        TexKind::Content(m) => m,
        TexKind::Plane(_) | TexKind::Canvas => 1,
    };
    let blend = match c.blend {
        BlendMode::Normal => 0,
    };
    Uniforms::default()
        .f4(c.rect.map(f64::from))
        .f4([f64::from(output.0), f64::from(output.1), c.opacity, c.lod])
        .f4([i[0], i[1], i[2], geo.edge_scale[0]])
        .f4([i[3], i[4], i[5], geo.edge_scale[1]])
        .f4([t[0], t[1], t[2], 0.0])
        .f4([t[3], t[4], t[5], 0.0])
        .f4(geo.visible)
        .u4([mips, u32::from(c.full_cover), blend, 0])
}
