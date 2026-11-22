//! GPU side of the text pass (WP-0F.02): the MTSDF atlas texture, the instanced glyph render
//! pipeline, instance packing, and the Vello variant's renderer and RGBA8 targets. Driven by
//! `gpu::Renderer` for every `PassOp::Text`.

use std::num::NonZeroUsize;

use bytemuck::{Pod, Zeroable};

use crate::text::ResolvedText;
use crate::text::TextEngine;
use crate::text::atlas::GlyphAtlas;
use crate::text::vello_scene::VelloFonts;
use crate::{Error, Result};

const TEXT_WGSL: &str = include_str!("shaders/text.wgsl");

/// One glyph quad (layout in `shaders/text.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuGlyph {
    pub anim0: [f32; 4],
    pub anim1: [f32; 4],
    pub quad: [f32; 4],
    pub cell: [f32; 4],
    pub color: [f32; 4],
    pub hl: [f32; 4],
    pub params: [f32; 4],
}

const KIND_FILL: f32 = 0.0;
const KIND_OUTLINE: f32 = 1.0;
const KIND_SHADOW: f32 = 2.0;

fn scaled(c: [f32; 4], k: f32) -> [f32; 4] {
    c.map(|v| v * k)
}

/// Packs `r` as shadow, outline and fill layers (in that order, so neighbouring glyphs'
/// outlines never cover a fill). Blank glyphs (spaces) and glyphs missing from the atlas are
/// skipped.
pub fn pack_instances(r: &ResolvedText, atlas: &GlyphAtlas, out: &mut Vec<GpuGlyph>) {
    let em = atlas.config.em_px as f32;
    let outline_w = r.outline.map_or(0.0, |o| o.0);
    let mut layers: Vec<(f32, [f32; 2])> = Vec::with_capacity(3);
    if let Some(s) = r.shadow {
        layers.push((KIND_SHADOW, s.offset));
    }
    if r.outline.is_some() {
        layers.push((KIND_OUTLINE, [0.0; 2]));
    }
    layers.push((KIND_FILL, [0.0; 2]));
    out.reserve(r.glyphs.len() * layers.len());
    for (kind, offset) in layers {
        for g in &r.glyphs {
            let Some(e) = atlas.get(&g.key) else {
                continue;
            };
            if e.is_blank() {
                continue;
            }
            let [l, b, rr, t] = e.plane;
            let quad = [
                g.pen[0] + l * g.size,
                g.pen[1] - t * g.size,
                g.pen[0] + rr * g.size,
                g.pen[1] - b * g.size,
            ];
            let [x, y, w, h] = e.rect.map(|v| v as f32);
            let mut anim = g.anim;
            anim[2] += offset[0];
            anim[5] += offset[1];
            let (color, hl, sweep) = if kind == KIND_FILL {
                match g.highlight {
                    Some(h) => (
                        scaled(g.fill, g.opacity),
                        scaled(h.color, g.opacity),
                        [h.sweep_x, h.dir],
                    ),
                    None => (scaled(g.fill, g.opacity), [0.0; 4], [0.0; 2]),
                }
            } else if kind == KIND_OUTLINE {
                let c = r.outline.map_or([0.0; 4], |o| o.1);
                (scaled(c, g.opacity), [0.0; 4], [0.0; 2])
            } else {
                let c = r.shadow.map_or([0.0; 4], |s| s.color);
                (scaled(c, g.opacity), [0.0; 4], [0.0; 2])
            };
            out.push(GpuGlyph {
                anim0: [anim[0], anim[1], anim[2], kind],
                anim1: [anim[3], anim[4], anim[5], g.size / em],
                quad,
                cell: [x, y, x + w, y + h],
                color,
                hl,
                params: [
                    outline_w,
                    r.shadow.map_or(0.0, |s| s.blur),
                    sweep[0],
                    sweep[1],
                ],
            });
        }
    }
}

/// Uniform block of the text pass.
#[must_use]
pub fn text_params(
    transform: &[f32; 6],
    size: [u32; 2],
    r: &ResolvedText,
    atlas: &GlyphAtlas,
    opacity: f32,
) -> [f32; 16] {
    let t = transform;
    [
        t[0],
        t[1],
        t[2],
        0.0,
        t[3],
        t[4],
        t[5],
        0.0,
        size[0] as f32,
        size[1] as f32,
        r.box_size[0],
        r.box_size[1],
        atlas.config.range_px as f32,
        atlas.config.em_px as f32,
        opacity,
        0.0,
    ]
}

/// The text renderer's GPU state (one per device).
pub struct TextGpu {
    pub engine: TextEngine,
    pub pipeline: wgpu::RenderPipeline,
    pub layout: wgpu::BindGroupLayout,
    atlas: Option<(wgpu::Texture, u32, u32, u64)>,
    instance_bufs: Vec<wgpu::Buffer>,
    pub scratch: Vec<GpuGlyph>,
    pub vello: Option<vello::Renderer>,
    pub vello_fonts: VelloFonts,
    vello_targets: Vec<(u32, u32, wgpu::Texture)>,
}

impl TextGpu {
    /// Fonts + pipeline (call with the process-wide pipeline-creation lock held).
    pub fn new(device: &wgpu::Device) -> Result<Self> {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("text"),
            source: wgpu::ShaderSource::Wgsl(TEXT_WGSL.into()),
        });
        let vf = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("text"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: vf,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: vf,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("text"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("text"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(Error::Gpu(format!("text pipeline: {e}")));
        }
        Ok(Self {
            engine: TextEngine::new()?,
            pipeline,
            layout,
            atlas: None,
            instance_bufs: Vec::new(),
            scratch: Vec::new(),
            vello: None,
            vello_fonts: VelloFonts::default(),
            vello_targets: Vec::new(),
        })
    }

    /// Creates the Vello renderer on first use (area AA only; single-threaded shader init).
    ///
    /// Finding: Vello 0.12's GPU pipeline crashes the WARP software adapter (access violation
    /// while WARP executes Vello's pre-fine compute stages; every standalone run, with FXC or
    /// DXC). With `use_cpu` (all stages up to fine on the CPU) WARP survives but takes ≈ 10 s
    /// per frame. So on software adapters Vello is `Unsupported` unless `S4_VELLO_CPU=1`.
    pub fn ensure_vello(&mut self, device: &wgpu::Device, software: bool) -> Result<()> {
        let cpu_stages = std::env::var("S4_VELLO_CPU").is_ok_and(|v| v == "1");
        if software && !cpu_stages {
            return Err(Error::Unsupported(
                "Vello on a software adapter (crashes WARP; set S4_VELLO_CPU=1 for the slow CPU-stage mode)"
                    .into(),
            ));
        }
        if self.vello.is_none() {
            let _serialised = crate::gpu::PIPELINE_CREATION.lock();
            let r = vello::Renderer::new(
                device,
                vello::RendererOptions {
                    use_cpu: cpu_stages,
                    antialiasing_support: vello::AaSupport::area_only(),
                    num_init_threads: NonZeroUsize::new(1),
                    pipeline_cache: None,
                },
            )
            .map_err(|e| Error::Gpu(format!("vello: {e}")))?;
            self.vello = Some(r);
        }
        Ok(())
    }

    /// The atlas texture, (re)uploaded when the CPU atlas changed.
    pub fn atlas_view(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
        let a = &self.engine.atlas;
        let stale = self
            .atlas
            .as_ref()
            .is_none_or(|(_, w, h, rev)| *w != a.width || *h != a.height || *rev != a.revision());
        if stale {
            let size = wgpu::Extent3d {
                width: a.width,
                height: a.height,
                depth_or_array_layers: 1,
            };
            let reuse = self
                .atlas
                .take()
                .filter(|(_, w, h, _)| *w == a.width && *h == a.height)
                .map(|(t, ..)| t);
            let tex = reuse.unwrap_or_else(|| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("msdf-atlas"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            });
            queue.write_texture(
                tex.as_image_copy(),
                a.pixels(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(a.width * 4),
                    rows_per_image: Some(a.height),
                },
                size,
            );
            self.atlas = Some((tex, a.width, a.height, a.revision()));
        }
        self.atlas
            .as_ref()
            .map(|(t, ..)| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .unwrap_or_else(|| unreachable_view(device))
    }

    /// Instance storage buffer `slot` with at least `bytes` capacity.
    pub fn instance_buffer(
        &mut self,
        device: &wgpu::Device,
        slot: usize,
        bytes: u64,
    ) -> wgpu::Buffer {
        let bytes = bytes.max(256);
        while self.instance_bufs.len() <= slot {
            self.instance_bufs.push(new_instance_buffer(device, 256));
        }
        if self.instance_bufs[slot].size() < bytes {
            self.instance_bufs[slot] = new_instance_buffer(device, bytes.next_power_of_two());
        }
        self.instance_bufs[slot].clone()
    }

    /// An RGBA8 storage target for Vello (pooled by size).
    pub fn vello_target(&mut self, device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
        if let Some(i) = self
            .vello_targets
            .iter()
            .position(|(tw, th, _)| *tw == w && *th == h)
        {
            return self.vello_targets.swap_remove(i).2;
        }
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vello-target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    pub fn recycle_vello_target(&mut self, t: wgpu::Texture) {
        if self.vello_targets.len() < 8 {
            self.vello_targets.push((t.width(), t.height(), t));
        }
    }

    /// Encodes `r` into a Vello scene with `transform` (box → target px).
    pub fn vello_scene(&mut self, r: &ResolvedText, transform: &[f32; 6]) -> vello::Scene {
        let mut scene = vello::Scene::new();
        self.vello_fonts
            .encode(&self.engine.fonts, r, transform, &mut scene);
        scene
    }

    /// Bytes held by the atlas texture.
    #[must_use]
    pub fn atlas_bytes(&self) -> u64 {
        self.atlas
            .as_ref()
            .map_or(0, |(_, w, h, _)| u64::from(*w) * u64::from(*h) * 4)
    }
}

fn new_instance_buffer(device: &wgpu::Device, bytes: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("glyph-instances"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn unreachable_view(device: &wgpu::Device) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("atlas-placeholder"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}
