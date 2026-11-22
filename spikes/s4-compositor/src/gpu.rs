//! wgpu executor for [`RenderGraph`]s: adapter selection, pipelines, transient texture pool,
//! cross-frame cache of cacheable pass outputs, timestamp queries, readback and device-lost
//! recovery. All passes are compute shaders writing `Rgba16Float` storage textures; the output
//! pass quantises explicitly into a storage buffer.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

use crate::color::{Eetf, kr_kb, primaries_conversion, range_params};
use crate::graph::{PassOp, RenderGraph, blur_weights};
use crate::hash::CacheKey;
use crate::image::Image;
use crate::scene::{Primaries, Scene, Transfer};
use crate::source::{PixelData, SourceProvider};
use crate::{Error, Result};

const COMMON_WGSL: &str = include_str!("shaders/common.wgsl");
const DECODE_WGSL: &str = include_str!("shaders/decode.wgsl");
const COMPOSITE_WGSL: &str = include_str!("shaders/composite.wgsl");
const EFFECTS_WGSL: &str = include_str!("shaders/effects.wgsl");
const OUTPUT_WGSL: &str = include_str!("shaders/output.wgsl");

const WORKING_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const UNIFORM_SIZE: u64 = 256;

/// Which adapter to use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterChoice {
    /// Discrete GPU, else integrated, else anything else.
    Auto,
    /// The software rasteriser (WARP on Windows, lavapipe/llvmpipe on Linux).
    Software,
    /// Index into [`list_adapters`].
    Index(usize),
    /// Exact [`AdapterSummary::key`].
    Key(String),
    /// Case-insensitive substring of "name backend".
    Name(String),
}

impl AdapterChoice {
    /// `auto`, `software`/`warp`/`cpu`, an index, `key:<key>` or a name substring.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        let t = s.trim();
        match t.to_ascii_lowercase().as_str() {
            "" | "auto" => Self::Auto,
            "software" | "warp" | "cpu" | "lavapipe" | "llvmpipe" => Self::Software,
            _ => {
                if let Some(k) = t.strip_prefix("key:") {
                    Self::Key(k.to_string())
                } else if let Ok(i) = t.parse::<usize>() {
                    Self::Index(i)
                } else {
                    Self::Name(t.to_string())
                }
            }
        }
    }

    /// From `S4_ADAPTER`, else `default`.
    #[must_use]
    pub fn from_env_or(default: Self) -> Self {
        std::env::var("S4_ADAPTER")
            .map(|v| Self::parse(&v))
            .unwrap_or(default)
    }
}

/// Description of one adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterSummary {
    pub index: usize,
    pub name: String,
    pub backend: String,
    pub device_type: String,
    pub driver: String,
    pub software: bool,
    pub timestamps: bool,
    /// Stable slug `backend-name` used for per-adapter golden directories.
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

fn instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
    // Backend validation layers off by default (they distort timings); WGPU_* env can enable.
    desc.flags = wgpu::InstanceFlags::empty().with_env();
    wgpu::Instance::new(desc)
}

fn summarize(index: usize, a: &wgpu::Adapter) -> AdapterSummary {
    let info = a.get_info();
    let backend = format!("{:?}", info.backend).to_lowercase();
    AdapterSummary {
        index,
        key: slug(&format!("{backend}-{}", info.name)),
        name: info.name.clone(),
        backend,
        device_type: format!("{:?}", info.device_type),
        driver: format!("{} {}", info.driver, info.driver_info)
            .trim()
            .to_string(),
        software: info.device_type == wgpu::DeviceType::Cpu,
        timestamps: a.features().contains(wgpu::Features::TIMESTAMP_QUERY),
    }
}

fn enumerate() -> Vec<wgpu::Adapter> {
    let inst = instance();
    let mut adapters = pollster::block_on(inst.enumerate_adapters(wgpu::Backends::all()));
    // GL is not a target for the compositor (no storage-texture parity); keep PRIMARY only.
    adapters.retain(|a| a.get_info().backend != wgpu::Backend::Gl);
    adapters
}

/// All usable adapters on this machine.
#[must_use]
pub fn list_adapters() -> Vec<AdapterSummary> {
    enumerate()
        .iter()
        .enumerate()
        .map(|(i, a)| summarize(i, a))
        .collect()
}

fn pick(adapters: Vec<wgpu::Adapter>, choice: &AdapterChoice) -> Result<(usize, wgpu::Adapter)> {
    let summaries: Vec<AdapterSummary> = adapters
        .iter()
        .enumerate()
        .map(|(i, a)| summarize(i, a))
        .collect();
    let idx = match choice {
        AdapterChoice::Auto => {
            let rank = |s: &AdapterSummary| match s.device_type.as_str() {
                "DiscreteGpu" => 0,
                "IntegratedGpu" => 1,
                "Cpu" => 3,
                _ => 2,
            };
            summaries
                .iter()
                .min_by_key(|s| (rank(s), s.index))
                .map(|s| s.index)
        }
        AdapterChoice::Software => summaries.iter().find(|s| s.software).map(|s| s.index),
        AdapterChoice::Index(i) => summaries.get(*i).map(|s| s.index),
        AdapterChoice::Key(k) => summaries.iter().find(|s| &s.key == k).map(|s| s.index),
        AdapterChoice::Name(n) => {
            let n = n.to_lowercase();
            summaries
                .iter()
                .find(|s| {
                    format!("{} {}", s.name, s.backend)
                        .to_lowercase()
                        .contains(&n)
                })
                .map(|s| s.index)
        }
    };
    let idx = idx.ok_or_else(|| Error::NoAdapter(format!("{choice:?}")))?;
    let adapter = adapters
        .into_iter()
        .nth(idx)
        .ok_or_else(|| Error::NoAdapter(format!("{choice:?}")))?;
    Ok((idx, adapter))
}

#[derive(Clone, Copy)]
enum Slot {
    Uniform,
    TexF,
    TexU,
    Tex3d,
    StoreTex,
    StoreBuf,
}

struct Pipe {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

/// A pass ready to encode: pipeline, uniform data, input bindings, uploads to keep alive.
type Prepared = (PipeId, U, Vec<(u32, wgpu::TextureView)>, Vec<wgpu::Texture>);

#[derive(Clone, Copy)]
enum PipeId {
    DecodeYuv,
    DecodeRgba,
    Composite,
    Fill,
    Shape,
    Dissolve,
    Blur,
    Lut,
    Output,
}

struct Pipelines {
    decode_yuv: Pipe,
    decode_rgba: Pipe,
    composite: Pipe,
    fill: Pipe,
    shape: Pipe,
    dissolve: Pipe,
    blur: Pipe,
    lut: Pipe,
    output: Pipe,
}

fn make_pipe(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    entry: &str,
    slots: &[(u32, Slot)],
) -> Pipe {
    let entries: Vec<wgpu::BindGroupLayoutEntry> = slots
        .iter()
        .map(|&(binding, slot)| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: match slot {
                Slot::Uniform => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                Slot::TexF | Slot::TexU | Slot::Tex3d => wgpu::BindingType::Texture {
                    sample_type: if matches!(slot, Slot::TexU) {
                        wgpu::TextureSampleType::Uint
                    } else {
                        wgpu::TextureSampleType::Float { filterable: false }
                    },
                    view_dimension: if matches!(slot, Slot::Tex3d) {
                        wgpu::TextureViewDimension::D3
                    } else {
                        wgpu::TextureViewDimension::D2
                    },
                    multisampled: false,
                },
                Slot::StoreTex => wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: WORKING_FORMAT,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                Slot::StoreBuf => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            },
            count: None,
        })
        .collect();
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(entry),
        entries: &entries,
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(entry),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: Some(&pl),
        module,
        entry_point: Some(entry),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    Pipe { pipeline, layout }
}

fn create_pipelines(device: &wgpu::Device) -> Result<Pipelines> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = |label: &str, src: &str| {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(format!("{COMMON_WGSL}\n{src}").into()),
        })
    };
    let decode = module("decode", DECODE_WGSL);
    let composite = module("composite", COMPOSITE_WGSL);
    let effects = module("effects", EFFECTS_WGSL);
    let output = module("output", OUTPUT_WGSL);
    use Slot::{StoreBuf, StoreTex, Tex3d, TexF, TexU, Uniform};
    let p = Pipelines {
        decode_yuv: make_pipe(
            device,
            &decode,
            "decode_yuv420",
            &[(0, Uniform), (1, TexU), (2, TexU), (3, TexU), (4, StoreTex)],
        ),
        decode_rgba: make_pipe(
            device,
            &decode,
            "decode_rgba8",
            &[(0, Uniform), (5, TexF), (4, StoreTex)],
        ),
        composite: make_pipe(
            device,
            &composite,
            "main",
            &[(0, Uniform), (1, TexF), (2, TexF), (3, StoreTex)],
        ),
        fill: make_pipe(device, &effects, "fill", &[(0, Uniform), (3, StoreTex)]),
        shape: make_pipe(device, &effects, "shape", &[(0, Uniform), (3, StoreTex)]),
        dissolve: make_pipe(
            device,
            &effects,
            "dissolve",
            &[(0, Uniform), (1, TexF), (2, TexF), (3, StoreTex)],
        ),
        blur: make_pipe(
            device,
            &effects,
            "blur",
            &[(0, Uniform), (1, TexF), (3, StoreTex)],
        ),
        lut: make_pipe(
            device,
            &effects,
            "lut3d",
            &[(0, Uniform), (1, TexF), (4, Tex3d), (3, StoreTex)],
        ),
        output: make_pipe(
            device,
            &output,
            "main",
            &[(0, Uniform), (1, TexF), (2, StoreBuf)],
        ),
    };
    if let Some(e) = pollster::block_on(scope.pop()) {
        return Err(Error::Gpu(format!("pipeline creation: {e}")));
    }
    Ok(p)
}

/// Packs uniform data as 16-byte vec4 slots.
#[derive(Default)]
struct U(Vec<u32>);

impl U {
    fn f4(mut self, v: [f32; 4]) -> Self {
        self.0.extend(v.map(f32::to_bits));
        self
    }
    fn u4(mut self, v: [u32; 4]) -> Self {
        self.0.extend(v);
        self
    }
    fn bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.0)
    }
}

fn f(v: f64) -> f32 {
    v as f32
}

/// Per-frame execution statistics.
#[derive(Clone, Debug, Default)]
pub struct FrameStats {
    /// GPU time from the first pass start to the last pass end (timestamp queries).
    pub gpu_ns: Option<u64>,
    /// Per executed pass: (label, GPU ns).
    pub pass_ns: Vec<(String, u64)>,
    /// Wall time of `render` (build excluded): encode + upload + submit + wait + readback.
    pub cpu_ms: f64,
    pub passes_run: usize,
    pub cache_hits: usize,
    pub textures_created: usize,
}

/// A rendered frame.
#[derive(Clone, Debug)]
pub struct Frame {
    pub image: Image,
    pub stats: FrameStats,
}

struct Readback {
    size: u64,
    storage: wgpu::Buffer,
    map: wgpu::Buffer,
}

struct Queries {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    capacity: u32,
}

/// Headless GPU renderer bound to one adapter.
pub struct Renderer {
    summary: AdapterSummary,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipes: Pipelines,
    pool: Vec<(u32, u32, wgpu::Texture)>,
    cache: HashMap<CacheKey, wgpu::Texture>,
    lut_tex: HashMap<u64, wgpu::Texture>,
    uniforms: Vec<wgpu::Buffer>,
    readback: Option<Readback>,
    queries: Option<Queries>,
    lost: Arc<AtomicBool>,
    errors: Arc<Mutex<Vec<String>>>,
}

impl Renderer {
    /// Creates a renderer on the chosen adapter.
    pub fn new(choice: &AdapterChoice) -> Result<Self> {
        // Finding: creating compute pipelines concurrently on several WARP devices from different
        // threads crashes the process (STATUS_ACCESS_VIOLATION in ~50% of parallel test runs on
        // Win11 26200; hardware DX12/Vulkan adapters were fine). Serialising pipeline creation
        // process-wide fixed it (0 crashes in 20 runs).
        static PIPELINE_CREATION: Mutex<()> = Mutex::new(());
        let (index, adapter) = pick(enumerate(), choice)?;
        let summary = summarize(index, &adapter);
        let features = if summary.timestamps {
            wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("s4"),
            required_features: features,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .map_err(|e| Error::RequestDevice(e.to_string()))?;

        let lost = Arc::new(AtomicBool::new(false));
        let lost_cb = Arc::clone(&lost);
        device.set_device_lost_callback(move |_reason, _msg| {
            lost_cb.store(true, Ordering::SeqCst);
        });
        let errors = Arc::new(Mutex::new(Vec::new()));
        let errors_cb = Arc::clone(&errors);
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            if let Ok(mut v) = errors_cb.lock() {
                v.push(e.to_string());
            }
        }));
        let pipes = {
            let _serialised = PIPELINE_CREATION.lock();
            create_pipelines(&device)?
        };
        Ok(Self {
            summary,
            device,
            queue,
            pipes,
            pool: Vec::new(),
            cache: HashMap::new(),
            lut_tex: HashMap::new(),
            uniforms: Vec::new(),
            readback: None,
            queries: None,
            lost,
            errors,
        })
    }

    /// The adapter this renderer runs on.
    #[must_use]
    pub fn adapter(&self) -> &AdapterSummary {
        &self.summary
    }

    /// Drops cached pass outputs (forces a cold frame).
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Destroys the device (test hook for the device-lost path).
    pub fn simulate_device_loss(&self) {
        self.device.destroy();
    }

    /// Device-lost recovery (§8): recreate the device on the same adapter and drop all caches.
    pub fn recover(&mut self) -> Result<()> {
        *self = Self::new(&AdapterChoice::Key(self.summary.key.clone()))?;
        Ok(())
    }

    /// Builds the graph for `scene` and renders it.
    pub fn render_scene(&mut self, scene: &Scene, provider: &dyn SourceProvider) -> Result<Frame> {
        let g = crate::graph::build(scene, provider)?;
        self.render(&g)
    }

    fn texture(&self, w: u32, h: u32, label: &str) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORKING_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    fn upload(
        &self,
        w: u32,
        h: u32,
        format: wgpu::TextureFormat,
        bpp: u32,
        data: &[u8],
    ) -> wgpu::Texture {
        let t = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("upload"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            t.as_image_copy(),
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * bpp),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        t
    }

    fn lut_texture(&mut self, lut: &crate::lut::Lut3d) -> wgpu::Texture {
        let hash = lut.content_hash();
        if let Some(t) = self.lut_tex.get(&hash) {
            return t.clone();
        }
        let n = lut.size as u32;
        let data: Vec<f32> = lut
            .data
            .iter()
            .flat_map(|c| [f(c[0]), f(c[1]), f(c[2]), 1.0])
            .collect();
        let t = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lut"),
            size: wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            t.as_image_copy(),
            bytemuck::cast_slice(&data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * 16),
                rows_per_image: Some(n),
            },
            wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
        );
        self.lut_tex.insert(hash, t.clone());
        t
    }

    fn ensure_buffers(&mut self, passes: usize, out_bytes: u64) {
        while self.uniforms.len() < passes {
            self.uniforms
                .push(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("uniform"),
                    size: UNIFORM_SIZE,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        if self.readback.as_ref().is_none_or(|r| r.size != out_bytes) {
            let storage = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("output"),
                size: out_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let map = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size: out_bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.readback = Some(Readback {
                size: out_bytes,
                storage,
                map,
            });
        }
        let need = (2 * passes) as u32;
        if self.summary.timestamps && self.queries.as_ref().is_none_or(|q| q.capacity < need) {
            let capacity = need.next_power_of_two().max(64);
            let set = self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: capacity,
            });
            let bytes = u64::from(capacity) * 8;
            let resolve = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ts-resolve"),
                size: bytes,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let read = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ts-read"),
                size: bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queries = Some(Queries {
                set,
                resolve,
                read,
                capacity,
            });
        }
    }

    fn take_errors(&self) -> Option<String> {
        let mut v = self.errors.lock().ok()?;
        if v.is_empty() {
            None
        } else {
            Some(v.drain(..).collect::<Vec<_>>().join("; "))
        }
    }

    /// Encodes the work of one (non-cached) pass: returns the pipeline, its uniform data, the
    /// input bindings and any per-pass upload textures to keep alive until submit.
    fn prepare_pass(
        &self,
        graph: &RenderGraph,
        op: &PassOp,
        size: [u32; 2],
        inputs: &[wgpu::TextureView],
        luts: &[wgpu::Texture],
    ) -> Result<Prepared> {
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let input = |i: usize| {
            inputs
                .get(i)
                .cloned()
                .ok_or_else(|| Error::InvalidGraph("missing pass input".into()))
        };
        let head = U::default();
        Ok(match op {
            PassOp::Fill { color } => (
                PipeId::Fill,
                head.u4([size[0], size[1], 0, 0]).f4(*color),
                vec![],
                vec![],
            ),
            PassOp::Shape {
                corner_radius,
                color,
            } => (
                PipeId::Shape,
                head.u4([size[0], size[1], 0, 0])
                    .f4(*color)
                    .f4([*corner_radius, 0.0, 0.0, 0.0]),
                vec![],
                vec![],
            ),
            PassOp::Dissolve { progress } => (
                PipeId::Dissolve,
                head.u4([size[0], size[1], 0, 0])
                    .f4([*progress, 0.0, 0.0, 0.0]),
                vec![(1, input(0)?), (2, input(1)?)],
                vec![],
            ),
            PassOp::Blur { horizontal, radius } => {
                let wts = blur_weights(*radius);
                let mut packed = [0.0f32; 16];
                packed[..wts.len()].copy_from_slice(&wts);
                let mut u = head
                    .u4([
                        size[0],
                        size[1],
                        u32::from(*horizontal),
                        (wts.len() - 1) as u32,
                    ])
                    .f4([0.0; 4])
                    .f4([0.0; 4])
                    .f4([0.0; 4]);
                for c in packed.as_chunks::<4>().0 {
                    u = u.f4(*c);
                }
                (PipeId::Blur, u, vec![(1, input(0)?)], vec![])
            }
            PassOp::Lut { lut, space, mix } => {
                let l = &graph.luts[*lut];
                let lt = luts
                    .get(*lut)
                    .ok_or_else(|| Error::InvalidGraph("lut".into()))?;
                let (dmin, dmax) = (l.domain_min.map(f), l.domain_max.map(f));
                (
                    PipeId::Lut,
                    head.u4([size[0], size[1], l.size as u32, space.shader_id()])
                        .f4([dmin[0], dmin[1], dmin[2], *mix])
                        .f4([dmax[0], dmax[1], dmax[2], 0.0]),
                    vec![(1, input(0)?), (4, view(lt))],
                    vec![],
                )
            }
            PassOp::Composite {
                inverse: m,
                opacity,
                blend,
                layer_size,
            } => (
                PipeId::Composite,
                head.f4([m[0], m[1], m[2], 0.0])
                    .f4([m[3], m[4], m[5], 0.0])
                    .f4([layer_size[0], layer_size[1], *opacity, 0.0])
                    .u4([blend.shader_id(), size[0], size[1], 0]),
                vec![(1, input(0)?), (2, input(1)?)],
                vec![],
            ),
            PassOp::Decode {
                source,
                tags,
                tone_map,
            } => {
                let src = &graph.sources[*source];
                let prim = primaries_conversion(tags.primaries, Primaries::Bt709);
                let (kr, kb) = kr_kb(tags.matrix).unwrap_or((0.2126, 0.0722));
                let scale = if tags.transfer == Transfer::Pq {
                    10_000.0
                } else {
                    1.0
                };
                let eetf = tone_map.map(|t| Eetf::new(&t));
                let tm = eetf.unwrap_or(Eetf {
                    src_min_pq: 0.0,
                    src_max_pq: 1.0,
                    min_lum: 0.0,
                    max_lum: 1.0,
                    ks: 1.0,
                    sdr_white_nits: 1.0,
                });
                let (cw, ch) = src.chroma_size();
                let (pipe, bits, bindings, uploads) = match &src.pixels {
                    PixelData::Yuv420 {
                        bit_depth,
                        y,
                        cb,
                        cr,
                    } => {
                        let fmt = wgpu::TextureFormat::R16Uint;
                        let ty =
                            self.upload(src.width, src.height, fmt, 2, bytemuck::cast_slice(y));
                        let tcb = self.upload(cw, ch, fmt, 2, bytemuck::cast_slice(cb));
                        let tcr = self.upload(cw, ch, fmt, 2, bytemuck::cast_slice(cr));
                        let b = vec![(1, view(&ty)), (2, view(&tcb)), (3, view(&tcr))];
                        (PipeId::DecodeYuv, *bit_depth, b, vec![ty, tcb, tcr])
                    }
                    PixelData::Rgba8(data) => {
                        let fmt = wgpu::TextureFormat::Rgba8Unorm;
                        let t = self.upload(src.width, src.height, fmt, 4, data);
                        (PipeId::DecodeRgba, 8, vec![(5, view(&t))], vec![t])
                    }
                };
                let rp = range_params(tags.range, bits);
                let u = head
                    .u4([src.width, src.height, cw, ch])
                    .u4([tags.transfer.shader_id(), u32::from(eetf.is_some()), 0, 0])
                    .f4([f(rp.y_off), f(rp.y_scale), f(rp.c_off), f(rp.c_scale)])
                    .f4([f(kr), f(kb), f(1.0 - kr - kb), scale])
                    .f4([f(prim[0][0]), f(prim[0][1]), f(prim[0][2]), 0.0])
                    .f4([f(prim[1][0]), f(prim[1][1]), f(prim[1][2]), 0.0])
                    .f4([f(prim[2][0]), f(prim[2][1]), f(prim[2][2]), 0.0])
                    .f4([
                        f(tm.src_min_pq),
                        f(tm.src_max_pq),
                        f(tm.min_lum),
                        f(tm.max_lum),
                    ])
                    .f4([f(tm.ks), f(tm.sdr_white_nits), 0.0, 0.0]);
                (pipe, u, bindings, uploads)
            }
            PassOp::Output {
                transfer,
                bit_depth,
            } => (
                PipeId::Output,
                head.u4([
                    size[0],
                    size[1],
                    transfer.shader_id(),
                    u32::from(*bit_depth),
                ]),
                vec![(1, input(0)?)],
                vec![],
            ),
            PassOp::Text { .. } => {
                return Err(Error::Unsupported(
                    "text rendering lands in WP-0F.02".into(),
                ));
            }
        })
    }

    fn pipe(&self, id: PipeId) -> &Pipe {
        match id {
            PipeId::DecodeYuv => &self.pipes.decode_yuv,
            PipeId::DecodeRgba => &self.pipes.decode_rgba,
            PipeId::Composite => &self.pipes.composite,
            PipeId::Fill => &self.pipes.fill,
            PipeId::Shape => &self.pipes.shape,
            PipeId::Dissolve => &self.pipes.dissolve,
            PipeId::Blur => &self.pipes.blur,
            PipeId::Lut => &self.pipes.lut,
            PipeId::Output => &self.pipes.output,
        }
    }

    /// Executes `graph` and reads the output back.
    pub fn render(&mut self, graph: &RenderGraph) -> Result<Frame> {
        if self.lost.load(Ordering::SeqCst) {
            return Err(Error::DeviceLost(self.summary.name.clone()));
        }
        let t0 = Instant::now();
        let order = graph.topo_order()?;
        let last = graph.last_uses(&order);
        for p in &graph.passes {
            if matches!(p.op, PassOp::Text { .. }) && !self.cache.contains_key(&p.key) {
                return Err(Error::Unsupported(format!(
                    "`{}`: text rendering lands in WP-0F.02 (MSDF / Vello)",
                    p.label
                )));
            }
        }
        let (w, h) = (graph.width, graph.height);
        let bits = graph
            .passes
            .iter()
            .find_map(|p| match p.op {
                PassOp::Output { bit_depth, .. } if p.output == graph.output => Some(bit_depth),
                _ => None,
            })
            .ok_or_else(|| Error::InvalidGraph("graph output is not an output pass".into()))?;
        let out_bytes = u64::from(w) * u64::from(h) * if bits == 16 { 8 } else { 4 };
        self.ensure_buffers(graph.passes.len(), out_bytes);
        let luts: Vec<wgpu::Texture> = graph.luts.iter().map(|l| self.lut_texture(l)).collect();

        let mut stats = FrameStats::default();
        let mut tex: Vec<Option<wgpu::Texture>> = vec![None; graph.resources.len()];
        let mut transient = vec![false; graph.resources.len()];
        let mut used = HashSet::new();
        let mut timed: Vec<String> = Vec::new();
        let mut keep_alive: Vec<wgpu::Texture> = Vec::new();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });

        for (pos, &pi) in order.iter().enumerate() {
            let pass = &graph.passes[pi];
            if pass.cacheable
                && let Some(t) = self.cache.get(&pass.key)
            {
                tex[pass.output] = Some(t.clone());
                used.insert(pass.key);
                stats.cache_hits += 1;
            } else {
                let res = graph.resources[pass.output];
                let is_output = matches!(pass.op, PassOp::Output { .. });
                let inputs = pass
                    .inputs
                    .iter()
                    .map(|&r| {
                        tex[r]
                            .as_ref()
                            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
                            .ok_or_else(|| Error::InvalidGraph(format!("input r{r} not ready")))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let (pipe_id, u, bindings, uploads) =
                    self.prepare_pass(graph, &pass.op, [res.width, res.height], &inputs, &luts)?;
                keep_alive.extend(uploads);

                // Output texture: cached ones are owned by the cache, others come from the pool.
                let out_tex = if is_output {
                    None
                } else if pass.cacheable {
                    stats.textures_created += 1;
                    Some(self.texture(res.width, res.height, "cached"))
                } else {
                    transient[pass.output] = true;
                    let i = self
                        .pool
                        .iter()
                        .position(|(pw, ph, _)| *pw == res.width && *ph == res.height);
                    Some(match i {
                        Some(i) => self.pool.swap_remove(i).2,
                        None => {
                            stats.textures_created += 1;
                            self.texture(res.width, res.height, "transient")
                        }
                    })
                };

                let uniform = &self.uniforms[pos];
                self.queue.write_buffer(uniform, 0, u.bytes());
                let out_view = out_tex
                    .as_ref()
                    .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
                let mut entries = vec![wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                }];
                for (b, v) in &bindings {
                    entries.push(wgpu::BindGroupEntry {
                        binding: *b,
                        resource: wgpu::BindingResource::TextureView(v),
                    });
                }
                let rb = self
                    .readback
                    .as_ref()
                    .ok_or_else(|| Error::Gpu("no readback".into()))?;
                if is_output {
                    entries.push(wgpu::BindGroupEntry {
                        binding: 2,
                        resource: rb.storage.as_entire_binding(),
                    });
                } else if let Some(v) = &out_view {
                    let b = if matches!(pass.op, PassOp::Decode { .. }) {
                        4
                    } else {
                        3
                    };
                    entries.push(wgpu::BindGroupEntry {
                        binding: b,
                        resource: wgpu::BindingResource::TextureView(v),
                    });
                }
                let pipe = self.pipe(pipe_id);
                let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&pass.label),
                    layout: &pipe.layout,
                    entries: &entries,
                });
                let (gw, gh) = if let PassOp::Decode { source, .. } = pass.op {
                    (graph.sources[source].width, graph.sources[source].height)
                } else {
                    (res.width, res.height)
                };
                let ts_index = timed.len() as u32;
                {
                    let timestamp_writes =
                        self.queries
                            .as_ref()
                            .map(|q| wgpu::ComputePassTimestampWrites {
                                query_set: &q.set,
                                beginning_of_pass_write_index: Some(2 * ts_index),
                                end_of_pass_write_index: Some(2 * ts_index + 1),
                            });
                    let mut cp = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some(&pass.label),
                        timestamp_writes,
                    });
                    cp.set_pipeline(&pipe.pipeline);
                    cp.set_bind_group(0, &bind, &[]);
                    cp.dispatch_workgroups(gw.div_ceil(8), gh.div_ceil(8), 1);
                }
                timed.push(pass.label.clone());
                stats.passes_run += 1;
                if let Some(t) = out_tex {
                    if pass.cacheable {
                        self.cache.insert(pass.key, t.clone());
                        used.insert(pass.key);
                    }
                    tex[pass.output] = Some(t);
                }
            }
            // Recycle transient inputs after their last reader.
            for &r in &pass.inputs {
                if last[r] == Some(pos)
                    && transient[r]
                    && let Some(t) = tex[r].take()
                {
                    let res = graph.resources[r];
                    self.pool.push((res.width, res.height, t));
                }
            }
        }
        for (r, slot) in tex.iter_mut().enumerate() {
            if transient[r]
                && let Some(t) = slot.take()
            {
                let res = graph.resources[r];
                self.pool.push((res.width, res.height, t));
            }
        }
        // Keep only what this frame used (an LRU of one frame).
        self.cache.retain(|k, _| used.contains(k));

        let rb = self
            .readback
            .as_ref()
            .ok_or_else(|| Error::Gpu("no readback buffer".into()))?;
        encoder.copy_buffer_to_buffer(&rb.storage, 0, &rb.map, 0, out_bytes);
        let n_ts = 2 * timed.len() as u32;
        if let Some(q) = &self.queries {
            encoder.resolve_query_set(&q.set, 0..n_ts, &q.resolve, 0);
            encoder.copy_buffer_to_buffer(&q.resolve, 0, &q.read, 0, u64::from(n_ts) * 8);
        }
        self.queue.submit([encoder.finish()]);
        drop(keep_alive);

        let (tx, rx) = mpsc::channel();
        rb.map.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let ts_bytes = u64::from(n_ts) * 8;
        let ts_rx = self.queries.as_ref().map(|q| {
            let (tx, rx) = mpsc::channel();
            q.read
                .slice(..ts_bytes)
                .map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
            rx
        });
        let poll = self.device.poll(wgpu::PollType::wait_indefinitely());
        if self.lost.load(Ordering::SeqCst) {
            return Err(Error::DeviceLost(self.summary.name.clone()));
        }
        poll.map_err(|e| Error::Gpu(format!("poll: {e}")))?;
        if let Some(e) = self.take_errors() {
            return Err(Error::Gpu(e));
        }
        rx.recv()
            .map_err(|e| Error::Gpu(e.to_string()))?
            .map_err(|e| Error::Gpu(format!("map: {e}")))?;
        let image = {
            let view = rb
                .map
                .get_mapped_range(..)
                .map_err(|e| Error::Gpu(format!("{e:?}")))?;
            let data: Vec<u16> = if bits == 16 {
                view.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect()
            } else {
                view.iter().map(|&b| u16::from(b)).collect()
            };
            Image {
                width: w,
                height: h,
                bit_depth: bits,
                data,
            }
        };
        rb.map.unmap();

        if let (Some(q), Some(rx)) = (&self.queries, ts_rx) {
            rx.recv()
                .map_err(|e| Error::Gpu(e.to_string()))?
                .map_err(|e| Error::Gpu(format!("map timestamps: {e}")))?;
            {
                let view = q
                    .read
                    .get_mapped_range(..ts_bytes)
                    .map_err(|e| Error::Gpu(format!("{e:?}")))?;
                let ticks: Vec<u64> = view
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| u64::from_le_bytes(*c))
                    .collect();
                let period = f64::from(self.queue.get_timestamp_period());
                let ns = |t: u64| (t as f64 * period) as u64;
                let (mut first, mut end) = (u64::MAX, 0u64);
                for (i, label) in timed.iter().enumerate() {
                    let (b, e) = (ticks[2 * i], ticks[2 * i + 1]);
                    first = first.min(b);
                    end = end.max(e);
                    stats.pass_ns.push((label.clone(), ns(e.saturating_sub(b))));
                }
                if end >= first {
                    stats.gpu_ns = Some(ns(end - first));
                }
            }
            q.read.unmap();
        }
        stats.cpu_ms = t0.elapsed().as_secs_f64() * 1e3;
        Ok(Frame { image, stats })
    }
}
