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
//!   storage buffer, then readback: blocking ([`Compositor::render`] / [`Compositor::execute`],
//!   or [`Compositor::execute_with`], which hands the mapped pixels to the caller; one reused
//!   staging buffer) or asynchronous ([`Compositor::submit`] then [`Compositor::try_receive`], a
//!   small pool of staging buffers; see the `readback` module).
//!
//! Determinism (arch §8): the same graph renders to the same bytes on the same adapter whatever
//! the cache and pool state (cold or warm, recycled textures, after device recovery), and the
//! blocking and asynchronous readbacks deliver the same bytes; the render path reads no clock.
//!
//! # Steady-state frames allocate nothing of ours (arch §10 "Threads")
//!
//! GPU textures and buffers are pooled; texture views, bind groups and the uniform buffers are
//! made once per texture (or slot) and reused; per-frame bookkeeping (resource table, uploads,
//! ordering, draw lists) lives in buffers kept across frames; uniforms are packed on the stack;
//! [`Compositor::execute_with`] reads back without an intermediate `Vec`. Every wgpu call runs
//! in [`Scope::Wgpu`](crate::alloc_scope::Scope), so a counting allocator can tell the heap use
//! inside wgpu (command encoders and passes, queue writes and submission, the map callback, the
//! device poll) from ours, which is zero once the pools, caches and buffers are warm. The
//! decoded-frame cache evicts textures of the size it needs first, so a playing timeline cycles
//! through the same textures instead of creating new ones. [`Compositor::execute`] still returns
//! an owned [`Image`] (one `Vec` per frame).

#![allow(clippy::cast_possible_truncation)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use cutweave_model::ItemId;

use super::readback::{FrameTicket, ReadbackFrame, ReadbackPool};
use super::{GpuContext, GpuError, PIPELINE_CREATION, Watchdog, block_on};
use crate::alloc_scope::wgpu as in_wgpu;
use crate::color::{Eetf, OutputTransfer};
use crate::frame::{FrameError, FrameSource, PixelFormat, VideoFrame};
use crate::geometry::mip_size;
use crate::graph::{
    BuildStats, CompositeOp, DecodeOp, GraphError, GraphOptions, OrderScratch, Pass, PassOp,
    RenderGraph, Target, build,
};
use crate::hash::CacheKey;
use crate::image::Image;
use crate::quality::Quality;
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

/// States of the blocking readback's map callback.
const MAP_PENDING: u8 = 0;
const MAP_DONE: u8 = 1;
const MAP_FAILED: u8 = 2;

/// Compositor settings.
#[derive(Clone, Debug, PartialEq)]
pub struct CompositorOptions {
    /// Settings that change pixels (tone map).
    pub graph: GraphOptions,
    /// VRAM budget for decoded frames kept across renders, in bytes.
    pub cache_budget_bytes: u64,
    /// Frames slower than this are logged by the watchdog.
    pub watchdog_threshold: Duration,
    /// Staging buffers for asynchronous readback ([`Compositor::submit`]): at most this many
    /// frames in flight.
    pub readback_slots: usize,
}

impl Default for CompositorOptions {
    fn default() -> CompositorOptions {
        CompositorOptions {
            graph: GraphOptions::default(),
            cache_budget_bytes: 256 << 20,
            watchdog_threshold: Duration::from_millis(500),
            readback_slots: 3,
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

/// Packs uniform data as 16-byte `vec4` slots, on the stack.
struct Uniforms {
    bytes: [u8; UNIFORM_SIZE as usize],
    len: usize,
}

impl Default for Uniforms {
    fn default() -> Uniforms {
        Uniforms {
            bytes: [0; UNIFORM_SIZE as usize],
            len: 0,
        }
    }
}

impl Uniforms {
    fn put(&mut self, word: [u8; 4]) {
        // Every layout is far below the buffer size; a longer one would be a bug caught by the
        // GPU validation of the shader's binding size, so extra words are dropped.
        if let Some(dst) = self.bytes.get_mut(self.len..self.len + 4) {
            dst.copy_from_slice(&word);
            self.len += 4;
        }
    }

    fn f4(mut self, v: [f64; 4]) -> Self {
        for x in v {
            self.put((x as f32).to_le_bytes());
        }
        self
    }

    fn u4(mut self, v: [u32; 4]) -> Self {
        for x in v {
            self.put(x.to_le_bytes());
        }
        self
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
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

    fn mips(&self) -> u32 {
        match self.kind {
            TexKind::Content(m) => m.max(1),
            TexKind::Plane(_) | TexKind::Canvas => 1,
        }
    }
}

/// A decoded texture kept across frames, under its decode pass's Merkle key.
struct CacheEntry {
    id: CacheKey,
    texture: wgpu::Texture,
    key: TexKey,
    last_used: u64,
}

/// The views of a pooled texture, made once.
struct Views {
    /// Every mip level (sampling, render targets).
    full: wgpu::TextureView,
    /// One view per mip level (storage writes of the decode and mip passes); empty for
    /// textures without a mip chain to write.
    levels: Vec<wgpu::TextureView>,
}

/// The decode pass's bind group of a destination texture, valid for its uniform slot and planes.
struct DecodeBind {
    slot: usize,
    planes: [Option<wgpu::Texture>; 3],
    bind: wgpu::BindGroup,
}

/// Bind groups and views kept across frames, keyed by the textures they reference. A texture
/// leaving the pool for good is [`forgotten`](BindCache::forget) everywhere.
#[derive(Default)]
struct BindCache {
    views: HashMap<wgpu::Texture, Views>,
    /// Keyed by the decode's destination texture.
    decode: HashMap<wgpu::Texture, DecodeBind>,
    /// Keyed by the destination's mip chain: level `i - 1` → `i` for `i` in `1..mips`.
    mips: HashMap<wgpu::Texture, Vec<wgpu::BindGroup>>,
    /// Keyed by (uniform slot, content texture).
    composite: HashMap<(usize, wgpu::Texture), wgpu::BindGroup>,
    /// Keyed by (uniform slot, canvas read); valid for the output buffer it was made with.
    output: HashMap<(usize, wgpu::Texture), (wgpu::Buffer, wgpu::BindGroup)>,
}

impl BindCache {
    /// Drops everything referencing `texture` (inside wgpu: the objects free wgpu memory).
    fn forget(&mut self, texture: &wgpu::Texture) {
        in_wgpu(|| {
            self.views.remove(texture);
            self.decode.retain(|dst, d| {
                dst != texture && !d.planes.iter().flatten().any(|p| p == texture)
            });
            self.mips.remove(texture);
            self.composite.retain(|(_, t), _| t != texture);
            self.output.retain(|(_, t), _| t != texture);
        });
    }
}

/// Device-bound state: everything dropped and rebuilt on recovery.
struct DeviceState {
    pipes: Pipelines,
    dummy_view: wgpu::TextureView,
    pool: HashMap<TexKey, Vec<wgpu::Texture>>,
    /// Decoded textures (few: a VRAM budget of textures). A vector, so the churn of a playing
    /// timeline (one insert and one eviction per layer and frame) never reallocates.
    cache: Vec<CacheEntry>,
    cache_bytes: u64,
    uniforms: Vec<wgpu::Buffer>,
    /// The output pass's storage buffer (by size).
    output: Option<(u64, wgpu::Buffer)>,
    /// The blocking readback's staging buffer (by size).
    staging: Option<(u64, wgpu::Buffer)>,
    /// The blocking readback's map state (set by wgpu's callback during the poll).
    map_state: Arc<AtomicU8>,
    /// Staging buffers of asynchronous readbacks.
    readbacks: ReadbackPool,
    binds: BindCache,
    frame_no: u64,
}

impl DeviceState {
    fn new(ctx: &GpuContext, readback_slots: usize) -> Result<DeviceState, GpuError> {
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
        let dummy_view = view(&dummy_plane, None);
        Ok(DeviceState {
            pipes,
            dummy_view,
            pool: HashMap::new(),
            cache: Vec::new(),
            cache_bytes: 0,
            uniforms: Vec::new(),
            output: None,
            staging: None,
            map_state: Arc::new(AtomicU8::new(MAP_PENDING)),
            readbacks: ReadbackPool::new(readback_slots),
            binds: BindCache::default(),
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
        in_wgpu(|| {
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
        })
    }

    fn give(&mut self, key: TexKey, texture: wgpu::Texture) {
        let slot = self.pool.entry(key).or_default();
        if slot.len() < POOL_PER_KEY {
            slot.push(texture);
        } else {
            self.binds.forget(&texture);
            in_wgpu(|| drop(texture));
        }
    }

    /// Caches a decoded texture, evicting least-recently-used entries not used by this frame
    /// (of the same size first, so their textures serve the next frame's decodes from the
    /// pool). Returns the texture back when it does not fit.
    fn cache_insert(
        &mut self,
        key: CacheKey,
        tex_key: TexKey,
        texture: wgpu::Texture,
        budget: u64,
    ) -> Option<wgpu::Texture> {
        let bytes = tex_key.bytes();
        while self.cache_bytes + bytes > budget {
            let frame_no = self.frame_no;
            let oldest = |same: bool| {
                self.cache
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.last_used < frame_no && (!same || e.key == tex_key))
                    .min_by_key(|(_, e)| (e.last_used, e.id))
                    .map(|(i, _)| i)
            };
            let Some(victim) = oldest(true).or_else(|| oldest(false)) else {
                return Some(texture);
            };
            let e = self.cache.swap_remove(victim);
            self.cache_bytes -= e.key.bytes();
            self.give(e.key, e.texture);
        }
        self.cache_bytes += bytes;
        self.cache.push(CacheEntry {
            id: key,
            texture,
            key: tex_key,
            last_used: self.frame_no,
        });
        None
    }

    fn uniform(&mut self, device: &wgpu::Device, slot: usize) -> wgpu::Buffer {
        while self.uniforms.len() <= slot {
            let buffer = in_wgpu(|| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("uniforms"),
                    size: UNIFORM_SIZE,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            });
            self.uniforms.push(buffer);
        }
        self.uniforms[slot].clone()
    }

    /// A buffer of at least `bytes` with `usage` kept in `slot`, recreated only when it is too
    /// small (passes use its first `bytes`): alternating output sizes, as scrubbing does with
    /// its quarter-resolution answers, keep one buffer and its bind groups.
    fn sized_buffer(
        slot: &mut Option<(u64, wgpu::Buffer)>,
        device: &wgpu::Device,
        bytes: u64,
        label: &str,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        if let Some((size, buffer)) = slot.as_ref()
            && *size >= bytes
        {
            return buffer.clone();
        }
        in_wgpu(|| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage,
                mapped_at_creation: false,
            });
            *slot = Some((bytes, buffer.clone()));
            buffer
        })
    }

    fn output_buffer(&mut self, device: &wgpu::Device, bytes: u64) -> wgpu::Buffer {
        Self::sized_buffer(
            &mut self.output,
            device,
            bytes,
            "output",
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        )
    }

    fn staging_buffer(&mut self, device: &wgpu::Device, bytes: u64) -> wgpu::Buffer {
        Self::sized_buffer(
            &mut self.staging,
            device,
            bytes,
            "readback",
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        )
    }

    /// The views of `texture` (made on first use).
    fn views(&mut self, texture: &wgpu::Texture, key: TexKey) -> &Views {
        if !self.binds.views.contains_key(texture) {
            let mips = key.mips();
            let views = in_wgpu(|| Views {
                full: view(texture, None),
                levels: if matches!(key.kind, TexKind::Content(_)) {
                    (0..mips).map(|level| view(texture, Some(level))).collect()
                } else {
                    Vec::new()
                },
            });
            self.binds.views.insert(texture.clone(), views);
        }
        &self.binds.views[texture]
    }

    /// The full view of `texture`.
    fn full_view(&mut self, texture: &wgpu::Texture, key: TexKey) -> wgpu::TextureView {
        self.views(texture, key).full.clone()
    }
}

/// How a CPU target's pixels come back.
enum Readback<'a> {
    /// Wait for them (the reused blocking staging buffer) and hand them to the sink while they
    /// are mapped.
    Blocking(&'a mut dyn FnMut(&[u8])),
    /// Map a pool buffer and return a ticket.
    Async,
}

impl Readback<'_> {
    fn is_async(&self) -> bool {
        matches!(self, Readback::Async)
    }
}

/// What an execution produced.
enum Executed {
    /// A texture target's frame.
    Frame(RenderedFrame),
    /// A CPU target's pixels went to the blocking sink.
    Read(FrameStats),
    /// A CPU target's pixels are being mapped asynchronously.
    Submitted(FrameTicket),
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

/// Per-frame bookkeeping kept across frames (its capacity is reused).
#[derive(Default)]
struct FrameScratch {
    held: Vec<Option<Held>>,
    uploads: Vec<(TexKey, wgpu::Texture)>,
    order: Vec<usize>,
    last: Vec<Option<usize>>,
    order_scratch: OrderScratch,
    draws: Vec<(wgpu::BindGroup, [u32; 4])>,
}

/// Executes render graphs on one GPU device.
pub struct Compositor {
    ctx: GpuContext,
    options: CompositorOptions,
    dev: DeviceState,
    watchdog: Watchdog,
    scratch: FrameScratch,
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
        let dev = DeviceState::new(&ctx, options.readback_slots)?;
        let watchdog = Watchdog::spawn(options.watchdog_threshold, ctx.info().key.clone());
        Ok(Compositor {
            ctx,
            options,
            dev,
            watchdog,
            scratch: FrameScratch::default(),
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
        let entries = std::mem::take(&mut self.dev.cache);
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
        let lost = self.dev.readbacks.in_flight();
        if lost > 0 {
            tracing::warn!(frames = lost, "asynchronous readbacks lost with the device");
        }
        let ctx = self.ctx.recreate()?;
        self.dev = DeviceState::new(&ctx, self.options.readback_slots)?;
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

    /// Renders `scene` at `quality` to straight-alpha sRGB RGBA8: the output is
    /// [`Quality::output_size`] of `base` (the preview canvas or the sequence size) and layers
    /// accept proxies when the rung uses them. Export and agent frames pass
    /// [`Quality::FULL`].
    pub fn render_to_image(
        &mut self,
        scene: &SceneDescription,
        source: &dyn FrameSource,
        base: (u32, u32),
        quality: Quality,
    ) -> Result<RenderedFrame, RenderError> {
        let (width, height) = quality.output_size(base);
        let target = Target::rgba8(width, height);
        if scene
            .layers
            .iter()
            .all(|l| l.request.proxy == quality.use_proxies)
        {
            return self.render(scene, source, target);
        }
        let mut scene = scene.clone();
        quality.apply(&mut scene);
        self.render(&scene, source, target)
    }

    /// Executes a compiled graph. A lost device is recreated and the frame retried once.
    pub fn execute(&mut self, graph: &RenderGraph) -> Result<RenderedFrame, RenderError> {
        let mut data = None;
        let mut sink = |pixels: &[u8]| data = Some(pixels.to_vec());
        let executed = self.execute_recovering(graph, &mut Readback::Blocking(&mut sink))?;
        match executed {
            Executed::Frame(f) => Ok(f),
            Executed::Read(stats) => {
                let (width, height) = graph.target.size();
                Ok(RenderedFrame {
                    width,
                    height,
                    pixels: RenderedPixels::Rgba8(Image {
                        width,
                        height,
                        data: data.unwrap_or_default(),
                    }),
                    key: graph.output_key(),
                    missing: graph.missing.clone(),
                    stats,
                })
            }
            Executed::Submitted(_) => {
                Err(GraphError::Invalid("unexpected async readback".into()).into())
            }
        }
    }

    /// Executes a compiled graph with a CPU target and hands `read` its pixels (tightly packed
    /// straight-alpha RGBA8 rows, exactly what [`Compositor::execute`] returns) while they are
    /// still mapped: no per-frame allocation of ours (the preview copies them into its ring
    /// slot). A lost device is recreated and the frame retried once; `read` runs at most once.
    pub fn execute_with<R>(
        &mut self,
        graph: &RenderGraph,
        read: impl FnOnce(&[u8]) -> R,
    ) -> Result<(R, FrameStats), RenderError> {
        if !matches!(graph.target, Target::Rgba8Cpu { .. }) {
            return Err(GraphError::Invalid("a readback needs a CPU target".into()).into());
        }
        let mut read = Some(read);
        let mut out = None;
        let mut sink = |pixels: &[u8]| {
            if let Some(read) = read.take() {
                out = Some(read(pixels));
            }
        };
        let executed = self.execute_recovering(graph, &mut Readback::Blocking(&mut sink))?;
        match (executed, out) {
            (Executed::Read(stats), Some(out)) => Ok((out, stats)),
            _ => Err(GraphError::Invalid("the frame produced no pixels".into()).into()),
        }
    }

    /// Compiles `scene` for a CPU `target` and submits it **without waiting** for its pixels:
    /// they are copied into a pooled staging buffer that is mapped asynchronously. Collect the
    /// frame with [`Compositor::try_receive`] (never blocks) or [`Compositor::receive`].
    /// Fails with [`GpuError::ReadbackBusy`] when every staging buffer is in flight.
    pub fn submit(
        &mut self,
        scene: &SceneDescription,
        source: &dyn FrameSource,
        target: Target,
    ) -> Result<FrameTicket, RenderError> {
        let graph = build(scene, target, source, &self.options.graph)?;
        self.submit_graph(&graph)
    }

    /// [`Compositor::submit`] for a compiled graph (which must have a CPU target).
    pub fn submit_graph(&mut self, graph: &RenderGraph) -> Result<FrameTicket, RenderError> {
        if !matches!(graph.target, Target::Rgba8Cpu { .. }) {
            return Err(
                GraphError::Invalid("asynchronous readback needs a CPU target".into()).into(),
            );
        }
        match self.execute_recovering(graph, &mut Readback::Async)? {
            Executed::Submitted(t) => Ok(t),
            Executed::Frame(_) | Executed::Read(_) => {
                Err(GraphError::Invalid("unexpected blocking readback".into()).into())
            }
        }
    }

    /// Frames submitted and not yet received.
    #[must_use]
    pub fn readbacks_in_flight(&self) -> usize {
        self.dev.readbacks.in_flight()
    }

    /// Staging buffers created for asynchronous readback so far (on this device); constant
    /// once the output size is stable.
    #[must_use]
    pub fn readback_buffers_created(&self) -> u64 {
        self.dev.readbacks.buffers_created
    }

    /// The oldest submitted frame, if its readback has finished: `f` gets its description and
    /// the mapped pixels (tightly packed straight-alpha RGBA8 rows), valid only during the call.
    /// Never blocks; `Ok(None)` when nothing is ready.
    pub fn try_receive<R>(
        &mut self,
        f: impl FnOnce(&ReadbackFrame, &[u8]) -> R,
    ) -> Result<Option<R>, GpuError> {
        let _ = self.ctx.device().poll(wgpu::PollType::Poll);
        self.check()?;
        self.dev.readbacks.take_ready(&mut Some(f))
    }

    /// Like [`Compositor::try_receive`] but waits for the oldest wanted frame in flight;
    /// `Ok(None)` only when nothing (or only discarded frames) was in flight.
    pub fn receive<R>(
        &mut self,
        f: impl FnOnce(&ReadbackFrame, &[u8]) -> R,
    ) -> Result<Option<R>, GpuError> {
        let mut f = Some(f);
        // A wait completes every submission and runs its map callbacks; the bound only guards
        // against a driver that never reports completion.
        for _ in 0..64 {
            if self.dev.readbacks.in_flight() == 0 {
                return Ok(None);
            }
            let poll = self.ctx.device().poll(wgpu::PollType::wait_indefinitely());
            self.check()?;
            poll.map_err(|e| GpuError::Readback(format!("poll: {e}")))?;
            if let Some(r) = self.dev.readbacks.take_ready(&mut f)? {
                return Ok(Some(r));
            }
        }
        Err(GpuError::Readback(
            "a readback never completed after waiting for the device".into(),
        ))
    }

    /// Forgets every frame in flight (a seek: latest wins); their buffers are recycled as they
    /// complete.
    pub fn discard_readbacks(&mut self) {
        self.dev.readbacks.discard_all();
    }

    fn execute_recovering(
        &mut self,
        graph: &RenderGraph,
        readback: &mut Readback<'_>,
    ) -> Result<Executed, RenderError> {
        let _frame = self.watchdog.begin();
        let mut recovered = false;
        if self.ctx.is_lost() {
            self.recover()?;
            recovered = true;
        }
        let first = self.execute_once(graph, readback);
        let result = match first {
            Err(RenderError::Gpu(e))
                if !recovered && (self.ctx.is_lost() || e.is_device_lost()) =>
            {
                tracing::warn!(adapter = %self.ctx.info().key, error = %e, "render failed on a lost device; retrying");
                self.recover()?;
                recovered = true;
                self.execute_once(graph, readback)
            }
            other => other,
        };
        result.map(|e| match e {
            Executed::Frame(mut f) => {
                f.stats.recovered = recovered;
                Executed::Frame(f)
            }
            Executed::Read(mut stats) => {
                stats.recovered = recovered;
                Executed::Read(stats)
            }
            submitted @ Executed::Submitted(_) => submitted,
        })
    }

    fn execute_once(
        &mut self,
        graph: &RenderGraph,
        readback: &mut Readback<'_>,
    ) -> Result<Executed, RenderError> {
        // Reserve the staging buffer first: a busy pool must fail before anything is encoded
        // (decode passes insert their textures into the cache as they are recorded).
        let slot = if readback.is_async() {
            let (width, height) = graph.target.size();
            let bytes = u64::from(width) * u64::from(height) * 4;
            let device = self.ctx.device().clone();
            Some(in_wgpu(|| self.dev.readbacks.acquire(&device, bytes))?)
        } else {
            None
        };
        let index = slot.as_ref().map(|(i, _)| *i);
        let device = self.ctx.device().clone();
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.held.clear();
        scratch.held.resize_with(graph.resources.len(), || None);
        scratch.uploads.clear();
        let mut f = Frame {
            encoder: in_wgpu(|| {
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("frame"),
                })
            }),
            scratch,
            readback: None,
            slot,
            stats: FrameStats {
                graph: graph.stats,
                ..FrameStats::default()
            },
        };
        let encoded = self.encode_frame(&mut f, graph);
        let result = match encoded {
            Ok(()) => self.finish(f, graph, readback),
            Err(e) => {
                self.recycle_frame(f);
                Err(e)
            }
        };
        if result.is_err()
            && let Some(index) = index
        {
            self.dev.readbacks.release(index);
        }
        result
    }

    /// Records every pass of `graph` into the frame's encoder.
    fn encode_frame(&mut self, f: &mut Frame, graph: &RenderGraph) -> Result<(), RenderError> {
        let mut order = std::mem::take(&mut f.scratch.order);
        let mut last = std::mem::take(&mut f.scratch.last);
        let result = graph
            .topo_order_into(&mut order, &mut f.scratch.order_scratch)
            .map_err(RenderError::from)
            .and_then(|()| {
                graph.last_uses_into(&order, &mut last);
                self.encode_passes(f, graph, &order, &last)
            });
        f.scratch.order = order;
        f.scratch.last = last;
        result
    }

    fn encode_passes(
        &mut self,
        f: &mut Frame,
        graph: &RenderGraph,
        order: &[usize],
        last: &[Option<usize>],
    ) -> Result<(), RenderError> {
        self.dev.frame_no += 1;
        let _ = self.ctx.take_errors();
        let mut pos = 0;
        while pos < order.len() {
            let pass = &graph.passes[order[pos]];
            let mut end = pos;
            match &pass.op {
                PassOp::Decode(d) => self.run_decode(f, graph, pass, d, pos),
                PassOp::Background { color } => {
                    self.run_background(f, pass.output, *color, graph.target.size());
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
                    self.run_composites(f, graph, &order[pos..=end], pos, last_reader)?;
                }
                PassOp::Output { transfer } => {
                    self.run_output(f, pass.inputs[0], *transfer, graph.target.size(), pos)?;
                }
            }
            // Recycle transient textures after their last reader.
            for p in &order[pos..=end] {
                for &r in &graph.passes[*p].inputs {
                    if last[r].is_some_and(|l| l <= end)
                        && f.scratch.held[r].as_ref().is_some_and(|h| h.transient)
                        && let Some(h) = f.scratch.held[r].take()
                    {
                        self.dev.give(h.key, h.texture);
                    }
                }
            }
            pos = end + 1;
        }
        Ok(())
    }

    /// Submits the frame and produces its pixels (or, for an asynchronous readback, starts
    /// mapping them and returns the ticket).
    fn finish(
        &mut self,
        f: Frame,
        graph: &RenderGraph,
        readback: &mut Readback<'_>,
    ) -> Result<Executed, RenderError> {
        let (width, height) = graph.target.size();
        let Frame {
            encoder,
            mut scratch,
            readback: map,
            slot,
            stats,
        } = f;
        let queue = self.ctx.queue().clone();
        let result = match graph.target {
            Target::Rgba8Cpu { .. } => match map {
                None => {
                    in_wgpu(|| drop(encoder));
                    self.recycle(&mut scratch);
                    Err(GraphError::Invalid("CPU target without an output pass".into()).into())
                }
                Some((map, bytes)) => {
                    in_wgpu(|| {
                        queue.submit([encoder.finish()]);
                    });
                    self.recycle(&mut scratch);
                    if let Some((slot, _)) = slot {
                        let _ = in_wgpu(|| self.ctx.device().poll(wgpu::PollType::Poll));
                        self.check()
                            .map(|()| {
                                let frame = ReadbackFrame {
                                    ticket: FrameTicket(0),
                                    width,
                                    height,
                                    key: graph.output_key(),
                                    missing: graph.missing.clone(),
                                    stats,
                                };
                                Executed::Submitted(in_wgpu(|| {
                                    self.dev.readbacks.start(slot, bytes, frame)
                                }))
                            })
                            .map_err(RenderError::from)
                    } else {
                        match readback {
                            Readback::Blocking(sink) => self
                                .read_into(&map, bytes, &mut **sink)
                                .map(|()| Executed::Read(stats))
                                .map_err(RenderError::from),
                            Readback::Async => Err(GraphError::Invalid(
                                "asynchronous readback without a staging buffer".into(),
                            )
                            .into()),
                        }
                    }
                }
            },
            Target::Texture { .. } => {
                let mut encoder = encoder;
                flush_clear(&mut self.dev, &mut encoder, &mut scratch.held, graph.output);
                match scratch.held[graph.output].take() {
                    None => {
                        in_wgpu(|| drop(encoder));
                        self.recycle(&mut scratch);
                        Err(GraphError::Invalid("graph output not ready".into()).into())
                    }
                    Some(out) => {
                        in_wgpu(|| {
                            queue.submit([encoder.finish()]);
                        });
                        self.recycle(&mut scratch);
                        // The caller owns the canvas now: it leaves the pool for good.
                        self.dev.binds.forget(&out.texture);
                        let _ = in_wgpu(|| self.ctx.device().poll(wgpu::PollType::Poll));
                        self.check().map_err(RenderError::from).map(|()| {
                            Executed::Frame(RenderedFrame {
                                width,
                                height,
                                pixels: RenderedPixels::Texture(out.texture),
                                key: graph.output_key(),
                                missing: graph.missing.clone(),
                                stats,
                            })
                        })
                    }
                }
            }
        };
        self.scratch = scratch;
        result
    }

    /// Returns this frame's transient textures and uploads to the pool (after submit).
    fn recycle(&mut self, scratch: &mut FrameScratch) {
        for h in scratch.held.iter_mut().filter_map(Option::take) {
            if h.transient {
                self.dev.give(h.key, h.texture);
            }
        }
        for (k, t) in scratch.uploads.drain(..) {
            self.dev.give(k, t);
        }
        scratch.draws.clear();
    }

    /// A frame that failed while encoding: drop its encoder, keep its textures and buffers.
    fn recycle_frame(&mut self, f: Frame) {
        let Frame {
            encoder,
            mut scratch,
            readback,
            ..
        } = f;
        in_wgpu(|| {
            drop(encoder);
            drop(readback);
        });
        self.recycle(&mut scratch);
        self.scratch = scratch;
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

    /// Maps the blocking staging buffer, waits for it and hands the pixels to `sink`.
    fn read_into(
        &self,
        map: &wgpu::Buffer,
        bytes: u64,
        sink: &mut dyn FnMut(&[u8]),
    ) -> Result<(), GpuError> {
        let state = &self.dev.map_state;
        state.store(MAP_PENDING, Ordering::Release);
        let done = Arc::clone(state);
        let poll = in_wgpu(|| {
            map.slice(..bytes).map_async(wgpu::MapMode::Read, move |r| {
                done.store(
                    if r.is_ok() { MAP_DONE } else { MAP_FAILED },
                    Ordering::Release,
                );
            });
            self.ctx.device().poll(wgpu::PollType::wait_indefinitely())
        });
        self.check()?;
        poll.map_err(|e| GpuError::Readback(format!("poll: {e}")))?;
        match state.load(Ordering::Acquire) {
            MAP_DONE => {}
            MAP_FAILED => {
                return Err(GpuError::Readback(
                    "map: the buffer could not be mapped".into(),
                ));
            }
            _ => {
                return Err(GpuError::Readback(
                    "map: the readback did not complete".into(),
                ));
            }
        }
        let range = in_wgpu(|| map.get_mapped_range(..bytes))
            .map_err(|e| GpuError::Readback(format!("{e:?}")));
        let result = range.map(|range| {
            sink(&range);
            in_wgpu(|| drop(range));
        });
        in_wgpu(|| map.unmap());
        result
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
        if let Some(e) = self.dev.cache.iter_mut().find(|e| e.id == pass.key) {
            e.last_used = frame_no;
            f.stats.cache_hits += 1;
            f.scratch.held[pass.output] = Some(Held {
                texture: e.texture.clone(),
                key,
                transient: false,
                clear: None,
            });
            return;
        }
        let device = self.ctx.device().clone();
        let texture = self.dev.take(&device, key, &mut f.stats);
        self.encode_decode(f, frame, d, &texture, key, slot);
        let budget = self.options.cache_budget_bytes;
        let transient = self
            .dev
            .cache_insert(pass.key, key, texture.clone(), budget)
            .is_some();
        f.scratch.held[pass.output] = Some(Held {
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
        f.scratch.held[output] = Some(Held {
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
        f.scratch.held[graph.passes[run[run.len() - 1]].output] = Some(canvas);
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
    ) -> Result<(), RenderError> {
        flush_clear(&mut self.dev, &mut f.encoder, &mut f.scratch.held, input);
        let device = self.ctx.device().clone();
        let bytes = u64::from(width) * u64::from(height) * 4;
        let storage = self.dev.output_buffer(&device, bytes);
        let map = match &f.slot {
            Some((_, buffer)) => buffer.clone(),
            None => self.dev.staging_buffer(&device, bytes),
        };
        let (src, src_key) = f.scratch.held[input]
            .as_ref()
            .map(|h| (h.texture.clone(), h.key))
            .ok_or_else(|| GraphError::Invalid("output input not ready".into()))?;
        let uniform = self.dev.uniform(&device, slot);
        let u = Uniforms::default().u4([width, height, transfer.transfer().shader_id(), 0]);
        in_wgpu(|| self.ctx.queue().write_buffer(&uniform, 0, u.as_bytes()));
        let bind = self.output_bind(&device, slot, &src, src_key, &uniform, &storage);
        let pipeline = &self.dev.pipes.output.pipeline;
        in_wgpu(|| {
            let mut cp = f.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("output"),
                timestamp_writes: None,
            });
            cp.set_pipeline(pipeline);
            cp.set_bind_group(0, &bind, &[]);
            cp.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
            drop(cp);
            f.encoder.copy_buffer_to_buffer(&storage, 0, &map, 0, bytes);
        });
        f.readback = Some((map, bytes));
        f.stats.passes_run += 1;
        Ok(())
    }

    /// The output pass's bind group for reading `src` with uniform `slot` into `storage`.
    fn output_bind(
        &mut self,
        device: &wgpu::Device,
        slot: usize,
        src: &wgpu::Texture,
        src_key: TexKey,
        uniform: &wgpu::Buffer,
        storage: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        if let Some((buffer, bind)) = self.dev.binds.output.get(&(slot, src.clone()))
            && buffer == storage
        {
            return bind.clone();
        }
        let src_view = self.dev.full_view(src, src_key);
        let layout = &self.dev.pipes.output.layout;
        let bind = in_wgpu(|| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("output"),
                layout,
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
            })
        });
        let old = self
            .dev
            .binds
            .output
            .insert((slot, src.clone()), (storage.clone(), bind.clone()));
        in_wgpu(|| drop(old));
        bind
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
        let src = f.scratch.held[backdrop].as_ref().ok_or_else(not_ready)?;
        if last_reader && src.transient {
            return f.scratch.held[backdrop].take().ok_or_else(not_ready);
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
            in_wgpu(|| {
                f.encoder.copy_texture_to_texture(
                    source.as_image_copy(),
                    texture.as_image_copy(),
                    source.size(),
                );
            });
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
        key: TexKey,
        slot: usize,
    ) {
        let device = self.ctx.device().clone();
        let queue = self.ctx.queue().clone();
        let mut planes: [Option<wgpu::Texture>; 3] = [None, None, None];
        for (i, plane) in frame.planes.iter().enumerate().take(3) {
            let (pw, ph, _) = frame.format.plane_layout(i, frame.width, frame.height);
            let plane_key = TexKey {
                kind: TexKind::Plane(plane_format(frame.format, i)),
                width: pw,
                height: ph,
            };
            let t = self.dev.take(&device, plane_key, &mut f.stats);
            in_wgpu(|| {
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
            });
            // Views of the plane are made once (before `uploads` holds the texture).
            let _ = self.dev.views(&t, plane_key);
            planes[i] = Some(t.clone());
            f.scratch.uploads.push((plane_key, t));
        }
        let uniform = self.dev.uniform(&device, slot);
        in_wgpu(|| queue.write_buffer(&uniform, 0, decode_uniforms(frame, d).as_bytes()));
        let bind = self.decode_bind(&device, slot, texture, key, planes, &uniform);
        let pipeline = &self.dev.pipes.decode.pipeline;
        in_wgpu(|| {
            let mut cp = f.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("decode"),
                timestamp_writes: None,
            });
            cp.set_pipeline(pipeline);
            cp.set_bind_group(0, &bind, &[]);
            cp.dispatch_workgroups(frame.width.div_ceil(8), frame.height.div_ceil(8), 1);
        });
        // One compute pass per level, so each level is complete before the next reads it.
        let mips = d.mip_levels.max(1);
        if mips > 1 {
            self.mip_binds(&device, texture, key);
            let binds = &self.dev.binds.mips[texture];
            let pipeline = &self.dev.pipes.mip.pipeline;
            for level in 1..mips {
                let (w, h) = mip_size(frame.width, frame.height, level);
                let bind = &binds[(level - 1) as usize];
                in_wgpu(|| {
                    let mut cp = f.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("mip"),
                        timestamp_writes: None,
                    });
                    cp.set_pipeline(pipeline);
                    cp.set_bind_group(0, bind, &[]);
                    cp.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
                });
            }
        }
        f.stats.decodes_run += 1;
        f.stats.passes_run += mips as usize;
    }

    /// The decode pass's bind group writing `dst` (made when its slot or planes change).
    fn decode_bind(
        &mut self,
        device: &wgpu::Device,
        slot: usize,
        dst: &wgpu::Texture,
        dst_key: TexKey,
        planes: [Option<wgpu::Texture>; 3],
        uniform: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        if let Some(cached) = self.dev.binds.decode.get(dst)
            && cached.slot == slot
            && cached.planes == planes
        {
            in_wgpu(|| drop(planes));
            return cached.bind.clone();
        }
        let dst_view = self.dev.views(dst, dst_key).levels[0].clone();
        let mut plane_views: [wgpu::TextureView; 3] = [
            self.dev.dummy_view.clone(),
            self.dev.dummy_view.clone(),
            self.dev.dummy_view.clone(),
        ];
        for (v, plane) in plane_views.iter_mut().zip(&planes) {
            if let Some(p) = plane {
                *v = self.dev.binds.views[p].full.clone();
            }
        }
        let layout = &self.dev.pipes.decode.layout;
        let bind = in_wgpu(|| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("decode"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&plane_views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&plane_views[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&plane_views[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&dst_view),
                    },
                ],
            })
        });
        let old = self.dev.binds.decode.insert(
            dst.clone(),
            DecodeBind {
                slot,
                planes,
                bind: bind.clone(),
            },
        );
        in_wgpu(|| drop((old, plane_views, dst_view)));
        bind
    }

    /// Makes the mip-chain bind groups of `texture` (once).
    fn mip_binds(&mut self, device: &wgpu::Device, texture: &wgpu::Texture, key: TexKey) {
        if self.dev.binds.mips.contains_key(texture) {
            return;
        }
        let levels: Vec<wgpu::TextureView> = self.dev.views(texture, key).levels.clone();
        let layout = &self.dev.pipes.mip.layout;
        let binds = in_wgpu(|| {
            levels
                .windows(2)
                .map(|pair| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("mip"),
                        layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&pair[0]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(&pair[1]),
                            },
                        ],
                    })
                })
                .collect()
        });
        self.dev.binds.mips.insert(texture.clone(), binds);
        in_wgpu(|| drop(levels));
    }

    /// The composite pass's bind group sampling `content` with uniform `slot`.
    fn composite_bind(
        &mut self,
        device: &wgpu::Device,
        slot: usize,
        content: &wgpu::Texture,
        content_key: TexKey,
        uniform: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        let cache_key = (slot, content.clone());
        if let Some(bind) = self.dev.binds.composite.get(&cache_key) {
            return bind.clone();
        }
        let content_view = self.dev.full_view(content, content_key);
        let layout = &self.dev.pipes.composite.layout;
        let bind = in_wgpu(|| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("composite"),
                layout,
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
            })
        });
        self.dev.binds.composite.insert(cache_key, bind.clone());
        bind
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
        let mut draws = std::mem::take(&mut f.scratch.draws);
        draws.clear();
        for (i, &pi) in run.iter().enumerate() {
            let pass = &graph.passes[pi];
            let PassOp::Composite(c) = &pass.op else {
                f.scratch.draws = draws;
                return Err(GraphError::Invalid("not a composite".into()));
            };
            let Some((content, content_key)) = f.scratch.held[pass.inputs[1]]
                .as_ref()
                .map(|h| (h.texture.clone(), h.key))
            else {
                f.scratch.draws = draws;
                return Err(GraphError::Invalid("composite content not ready".into()));
            };
            let uniform = self.dev.uniform(&device, first_slot + i);
            let u = composite_uniforms(c, content_key, output);
            in_wgpu(|| self.ctx.queue().write_buffer(&uniform, 0, u.as_bytes()));
            let bind =
                self.composite_bind(&device, first_slot + i, &content, content_key, &uniform);
            draws.push((bind, c.rect));
        }
        let target = self.dev.full_view(&canvas.texture, canvas.key);
        let load = canvas
            .clear
            .take()
            .map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear);
        let pipeline = &self.dev.pipes.composite.pipeline;
        in_wgpu(|| {
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
            rp.set_pipeline(pipeline);
            for (bind, rect) in &draws {
                if rect[2] <= rect[0] || rect[3] <= rect[1] {
                    continue;
                }
                rp.set_bind_group(0, bind, &[]);
                rp.draw(0..6, 0..1);
            }
        });
        // The bind groups stay cached; the list keeps its capacity for the next run.
        draws.clear();
        f.scratch.draws = draws;
        Ok(())
    }
}

/// Per-frame execution state.
struct Frame {
    encoder: wgpu::CommandEncoder,
    /// Bookkeeping lent by the compositor for this frame: the texture of each graph resource
    /// while it is live, the plane textures written this frame (back to the pool only after
    /// submit: queue writes run before the submission's commands), ordering and draw lists.
    scratch: FrameScratch,
    /// The mappable readback buffer and its size.
    readback: Option<(wgpu::Buffer, u64)>,
    /// The pool slot and staging buffer of an asynchronous readback.
    slot: Option<(usize, wgpu::Buffer)>,
    stats: FrameStats,
}

/// Applies a pending clear with an empty render pass.
fn flush_clear(
    dev: &mut DeviceState,
    encoder: &mut wgpu::CommandEncoder,
    held: &mut [Option<Held>],
    res: usize,
) {
    if let Some(h) = held[res].as_mut()
        && let Some(color) = h.clear.take()
    {
        let v = dev.full_view(&h.texture, h.key);
        in_wgpu(|| {
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
