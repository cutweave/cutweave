// SPDX-License-Identifier: GPL-3.0-or-later
//! Path (a) on the GPU side (WP-0E.02): a decoded frame in system memory (NV12/P010 from a
//! hwaccel transfer, or planar yuv420p/yuv420p10le from software decode) is uploaded as integer
//! textures with `Queue::write_texture` and converted to R'G'B' (`Rgba8Unorm`) by a compute
//! shader. "Texture ready" is the moment the GPU has finished that conversion
//! (`Device::poll(Wait)` returns).
//!
//! Evidence that the frame really rendered: [`Gpu::read_rgba`] reads the RGB result back, and the
//! harness compares it with [`cpu_reference`] (the same maths on the CPU) and reads the burned-in
//! frame barcode off the green channel.

use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

use ffmpeg::format::Pixel;
use ffmpeg::util::color;
use ffmpeg_next as ffmpeg;

use crate::frame_check::Luma;

const COMMON_WGSL: &str = include_str!("shaders/yuv_common.wgsl");
const UPLOAD_WGSL: &str = include_str!("shaders/yuv_upload.wgsl");

/// Blocks on a future. wgpu's native futures (adapter/device requests) are ready on first poll;
/// this avoids a `pollster` dependency.
pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// Plane layouts the upload path accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Nv12,
    P010,
    Yuv420p,
    Yuv420p10,
}

impl Layout {
    pub fn of(f: Pixel) -> Option<Self> {
        Some(match f {
            Pixel::NV12 => Layout::Nv12,
            Pixel::P010LE => Layout::P010,
            Pixel::YUV420P | Pixel::YUVJ420P => Layout::Yuv420p,
            Pixel::YUV420P10LE => Layout::Yuv420p10,
            _ => return None,
        })
    }

    fn semi_planar(self) -> bool {
        matches!(self, Layout::Nv12 | Layout::P010)
    }

    fn wide(self) -> bool {
        matches!(self, Layout::P010 | Layout::Yuv420p10)
    }

    /// Right shift that LSB-aligns a sample.
    pub fn shift(self) -> u32 {
        if self == Layout::P010 { 6 } else { 0 }
    }

    pub fn depth(self) -> u32 {
        if self.wide() { 10 } else { 8 }
    }

    fn formats(self) -> (wgpu::TextureFormat, wgpu::TextureFormat) {
        use wgpu::TextureFormat as F;
        match (self.wide(), self.semi_planar()) {
            (false, true) => (F::R8Uint, F::Rg8Uint),
            (false, false) => (F::R8Uint, F::R8Uint),
            (true, true) => (F::R16Uint, F::Rg16Uint),
            (true, false) => (F::R16Uint, F::R16Uint),
        }
    }
}

/// Colour parameters for the shader (and the CPU reference).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorParams {
    pub y_off: f32,
    pub y_scale: f32,
    pub c_off: f32,
    pub c_scale: f32,
    pub kr: f32,
    pub kb: f32,
}

impl ColorParams {
    /// From the frame's range and matrix tags (BT.709 when unspecified and >= 720 lines, else
    /// BT.601), for code values of `depth` bits.
    pub fn of(frame: &ffmpeg::frame::Video, depth: u32) -> Self {
        let full = frame.color_range() == color::Range::JPEG;
        let (kr, kb) = match frame.color_space() {
            color::Space::BT709 => (0.2126, 0.0722),
            color::Space::BT470BG | color::Space::SMPTE170M => (0.299, 0.114),
            color::Space::BT2020NCL | color::Space::BT2020CL => (0.2627, 0.0593),
            _ if frame.height() >= 720 => (0.2126, 0.0722),
            _ => (0.299, 0.114),
        };
        let k = (1u32 << (depth - 8)) as f32;
        let max = ((1u32 << depth) - 1) as f32;
        let (y_off, y_scale, c_off, c_scale) = if full {
            (0.0, max, (1u32 << (depth - 1)) as f32, max)
        } else {
            (16.0 * k, 219.0 * k, 128.0 * k, 224.0 * k)
        };
        ColorParams {
            y_off,
            y_scale,
            c_off,
            c_scale,
            kr,
            kb,
        }
    }

    /// The shader's conversion of one sample (Y' and interpolated Cb, Cr code values) to RGBA8.
    pub fn rgb8(&self, y: f32, cb: f32, cr: f32) -> [u8; 4] {
        let yn = (y - self.y_off) / self.y_scale;
        let cb = (cb - self.c_off) / self.c_scale;
        let cr = (cr - self.c_off) / self.c_scale;
        let r = yn + 2.0 * (1.0 - self.kr) * cr;
        let b = yn + 2.0 * (1.0 - self.kb) * cb;
        let g = (yn - self.kr * r - self.kb * b) / (1.0 - self.kr - self.kb);
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [q(r), q(g), q(b), 255]
    }

    /// The 16-float uniform block (`Params` in yuv_common.wgsl).
    pub fn uniform(&self, w: u32, h: u32, layout_id: u32, shift: u32, max_code: u32) -> Vec<u8> {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut b = Vec::with_capacity(64);
        for v in [w, h, cw, ch, layout_id, shift, max_code, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let kg = 1.0 - self.kr - self.kb;
        for v in [
            self.y_off,
            self.y_scale,
            self.c_off,
            self.c_scale,
            self.kr,
            self.kb,
            kg,
            0.0,
        ] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }
}

/// Timing of one upload + conversion.
#[derive(Clone, Copy, Debug, Default)]
pub struct Upload {
    /// CPU time in `write_texture` (copy into wgpu's staging memory).
    pub write_ms: f64,
    /// From the start of the upload to the GPU finishing the conversion.
    pub total_ms: f64,
}

struct Targets {
    key: (u32, u32, Layout),
    y: wgpu::Texture,
    c0: wgpu::Texture,
    c1: wgpu::Texture,
    out: wgpu::Texture,
    readback: wgpu::Buffer,
    padded_row: u32,
    bind: wgpu::BindGroup,
}

/// A wgpu device with the upload + conversion pipeline.
pub struct Gpu {
    pub info: wgpu::AdapterInfo,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    targets: Option<Targets>,
    errors: Arc<Mutex<Vec<String>>>,
}

fn instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
    desc.flags = wgpu::InstanceFlags::empty().with_env();
    wgpu::Instance::new(desc)
}

/// `name (backend, type)` of every adapter.
pub fn adapters() -> Vec<String> {
    block_on(instance().enumerate_adapters(wgpu::Backends::all()))
        .iter()
        .map(|a| {
            let i = a.get_info();
            format!("{} ({:?}, {:?})", i.name, i.backend, i.device_type)
        })
        .collect()
}

/// Picks an adapter: `""` = the first discrete GPU (DX12 preferred on Windows), else a
/// case-insensitive name substring (`nvidia`, `radeon`), optionally with `@vulkan` / `@dx12`.
fn pick(choice: &str) -> Result<wgpu::Adapter, String> {
    let all = block_on(instance().enumerate_adapters(wgpu::Backends::all()));
    let (name, backend) = choice
        .split_once('@')
        .map_or((choice, None), |(n, b)| (n, Some(b.to_lowercase())));
    let name = name.to_lowercase();
    let backend_rank = |b: wgpu::Backend| match b {
        wgpu::Backend::Dx12 => 0,
        wgpu::Backend::Metal => 0,
        wgpu::Backend::Vulkan => 1,
        _ => 3,
    };
    let mut cands: Vec<wgpu::Adapter> = all
        .into_iter()
        .filter(|a| {
            let i = a.get_info();
            i.backend != wgpu::Backend::Gl
                && (name.is_empty() || i.name.to_lowercase().contains(&name))
                && backend
                    .as_deref()
                    .is_none_or(|b| format!("{:?}", i.backend).to_lowercase() == b)
        })
        .collect();
    cands.sort_by_key(|a| {
        let i = a.get_info();
        let ty = match i.device_type {
            wgpu::DeviceType::DiscreteGpu => 0,
            wgpu::DeviceType::IntegratedGpu => 1,
            wgpu::DeviceType::Cpu => 3,
            _ => 2,
        };
        (ty, backend_rank(i.backend))
    });
    cands
        .into_iter()
        .next()
        .ok_or_else(|| format!("no GPU adapter matches '{choice}': {:?}", adapters()))
}

impl Gpu {
    /// Opens a device on the adapter `choice` picks (see [`pick`]). NV12/P010 texture features
    /// are requested when the adapter has them (the zero-copy experiment needs them).
    pub fn new(choice: &str) -> Result<Self, String> {
        let adapter = pick(choice)?;
        let info = adapter.get_info();
        let wanted = wgpu::Features::TEXTURE_FORMAT_NV12
            | wgpu::Features::TEXTURE_FORMAT_P010
            | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
        let features = adapter.features() & wanted;
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("s3"),
            required_features: features,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .map_err(|e| format!("request_device: {e}"))?;
        let errors = Arc::new(Mutex::new(Vec::new()));
        let errors_cb = Arc::clone(&errors);
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            if let Ok(mut v) = errors_cb.lock() {
                v.push(e.to_string());
            }
        }));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv_upload"),
            source: wgpu::ShaderSource::Wgsl(format!("{COMMON_WGSL}\n{UPLOAD_WGSL}").into()),
        });
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Uint,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv_upload"),
            entries: &[uniform_entry(0), tex(1), tex(2), tex(3), storage_entry(4)],
        });
        let pipeline = compute_pipeline(&device, &layout, &module, "convert");
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Gpu {
            info,
            adapter,
            device,
            queue,
            pipeline,
            layout,
            params,
            targets: None,
            errors,
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    /// `name (backend)`.
    pub fn label(&self) -> String {
        format!("{} ({:?})", self.info.name, self.info.backend)
    }

    /// Validation errors reported since the last call.
    pub fn take_errors(&self) -> Option<String> {
        let mut v = self.errors.lock().ok()?;
        if v.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut *v).join("; "))
        }
    }

    fn ensure_targets(&mut self, w: u32, h: u32, layout: Layout) {
        if self
            .targets
            .as_ref()
            .is_some_and(|t| t.key == (w, h, layout))
        {
            return;
        }
        let (fy, fc) = layout.formats();
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mk = |label, w, h, format, usage| {
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
                format,
                usage,
                view_formats: &[],
            })
        };
        let src = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let y = mk("y", w, h, fy, src);
        let c0 = mk("c0", cw, ch, fc, src);
        let c1 = if layout.semi_planar() {
            mk("c1", 1, 1, fc, src)
        } else {
            mk("c1", cw, ch, fc, src)
        };
        let out = mk(
            "rgb",
            w,
            h,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let padded_row = (w * 4).div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded_row) * u64::from(h),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let v = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuv_upload"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&v(&y)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&v(&c0)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&v(&c1)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&v(&out)),
                },
            ],
        });
        self.targets = Some(Targets {
            key: (w, h, layout),
            y,
            c0,
            c1,
            out,
            readback,
            padded_row,
            bind,
        });
    }

    /// Uploads `frame`'s planes, converts them to RGB and waits for the GPU (texture ready).
    pub fn upload(&mut self, frame: &ffmpeg::frame::Video) -> Result<Upload, String> {
        let t0 = Instant::now();
        let layout = Layout::of(frame.format())
            .ok_or_else(|| format!("upload: unsupported pixel format {:?}", frame.format()))?;
        let (w, h) = (frame.width(), frame.height());
        self.ensure_targets(w, h, layout);
        let params = ColorParams::of(frame, layout.depth());
        let max_code = if layout.wide() { 65535 } else { 255 };
        self.queue.write_buffer(
            &self.params,
            0,
            &params.uniform(
                w,
                h,
                u32::from(!layout.semi_planar()),
                layout.shift(),
                max_code,
            ),
        );
        let t = self.targets.as_ref().ok_or("no targets")?;
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let write = |tex: &wgpu::Texture, data: &[u8], stride: usize, pw: u32, ph: u32| {
            self.queue.write_texture(
                tex.as_image_copy(),
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride as u32),
                    rows_per_image: Some(ph),
                },
                wgpu::Extent3d {
                    width: pw,
                    height: ph,
                    depth_or_array_layers: 1,
                },
            );
        };
        let tw = Instant::now();
        write(&t.y, frame.data(0), frame.stride(0), w, h);
        write(&t.c0, frame.data(1), frame.stride(1), cw, ch);
        if !layout.semi_planar() {
            write(&t.c1, frame.data(2), frame.stride(2), cw, ch);
        }
        let write_ms = tw.elapsed().as_secs_f64() * 1e3;
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("yuv") });
        {
            let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("convert"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.pipeline);
            cp.set_bind_group(0, &t.bind, &[]);
            cp.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
        self.queue.submit([enc.finish()]);
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("poll: {e}"))?;
        if let Some(e) = self.take_errors() {
            return Err(e);
        }
        Ok(Upload {
            write_ms,
            total_ms: t0.elapsed().as_secs_f64() * 1e3,
        })
    }

    /// Reads back the last converted frame as tightly packed RGBA8.
    pub fn read_rgba(&mut self) -> Result<Vec<u8>, String> {
        let t = self.targets.as_ref().ok_or("nothing converted yet")?;
        let (w, h, _) = t.key;
        read_texture(
            &self.device,
            &self.queue,
            &t.out,
            &t.readback,
            t.padded_row,
            w,
            h,
        )
    }
}

pub(crate) fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub(crate) fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: wgpu::TextureFormat::Rgba8Unorm,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

pub(crate) fn compute_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    module: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(entry),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: Some(&pl),
        module,
        entry_point: Some(entry),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    })
}

/// Copies an `Rgba8Unorm` texture to `buf` and returns tightly packed RGBA8.
pub(crate) fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    buf: &wgpu::Buffer,
    padded_row: u32,
    w: u32,
    h: u32,
) -> Result<Vec<u8>, String> {
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("readback"),
    });
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    let (tx, rx) = mpsc::channel();
    buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| format!("poll: {e}"))?;
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("map: {e}"))?;
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    {
        let view = buf
            .slice(..)
            .get_mapped_range()
            .map_err(|e| format!("{e:?}"))?;
        for r in 0..h as usize {
            let s = r * padded_row as usize;
            out.extend_from_slice(&view[s..s + (w * 4) as usize]);
        }
    }
    buf.unmap();
    Ok(out)
}

/// The shader's conversion on the CPU (for the readback check). `None` for unsupported layouts.
pub fn cpu_reference(frame: &ffmpeg::frame::Video) -> Option<Vec<u8>> {
    let layout = Layout::of(frame.format())?;
    let p = crate::planes::Planes::of(frame)?;
    let params = ColorParams::of(frame, layout.depth());
    let (w, h) = (p.width, p.height);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let c = |plane: &[u16], x: i64, y: i64| {
        let x = x.clamp(0, cw as i64 - 1) as usize;
        let y = y.clamp(0, ch as i64 - 1) as usize;
        f32::from(plane[y * cw + x])
    };
    let bil = |plane: &[u16], px: f32, py: f32| {
        let (fx, fy) = (px.floor(), py.floor());
        let (rx, ry) = (px - fx, py - fy);
        let (x0, y0) = (fx as i64, fy as i64);
        let top = c(plane, x0, y0) * (1.0 - rx) + c(plane, x0 + 1, y0) * rx;
        let bottom = c(plane, x0, y0 + 1) * (1.0 - rx) + c(plane, x0 + 1, y0 + 1) * rx;
        top * (1.0 - ry) + bottom * ry
    };
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let py = (y as f32 - 0.5) * 0.5;
        for x in 0..w {
            let px = x as f32 * 0.5;
            let yc = f32::from(p.y[y * w + x]);
            out.extend_from_slice(&params.rgb8(yc, bil(&p.cb, px, py), bil(&p.cr, px, py)));
        }
    }
    Some(out)
}

/// Largest per-channel difference and the share of channels that differ by more than 1.
pub fn compare_rgba(a: &[u8], b: &[u8]) -> (u8, f64) {
    let mut max = 0u8;
    let mut off = 0usize;
    for (x, y) in a.iter().zip(b) {
        let d = x.abs_diff(*y);
        max = max.max(d);
        off += usize::from(d > 1);
    }
    (max, off as f64 / a.len().max(1) as f64)
}

/// Reads the frame barcode off the green channel of an RGBA8 image (white/black blocks).
pub fn barcode_from_rgba(rgba: &[u8], w: usize, h: usize) -> Option<u32> {
    let g: Vec<u8> = rgba.as_chunks::<4>().0.iter().map(|p| p[1]).collect();
    Luma {
        data: &g,
        stride: w,
        width: w,
        height: h,
        bytes: 1,
        depth: 8,
        shift: 0,
    }
    .barcode()
}

/// Writes RGBA8 as a binary PPM (no dependency), for eyeballing a rendered frame.
pub fn write_ppm(path: &std::path::Path, rgba: &[u8], w: usize, h: usize) -> std::io::Result<()> {
    let mut data = format!("P6\n{w} {h}\n255\n").into_bytes();
    data.extend(
        rgba.as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]]),
    );
    std::fs::write(path, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_range_extremes_map_to_black_and_white() {
        let p = ColorParams {
            y_off: 16.0,
            y_scale: 219.0,
            c_off: 128.0,
            c_scale: 224.0,
            kr: 0.2126,
            kb: 0.0722,
        };
        assert_eq!(p.rgb8(16.0, 128.0, 128.0), [0, 0, 0, 255]);
        assert_eq!(p.rgb8(235.0, 128.0, 128.0), [255, 255, 255, 255]);
        // 10-bit limited white (940) with the 10-bit offsets.
        let p10 = ColorParams {
            y_off: 64.0,
            y_scale: 876.0,
            c_off: 512.0,
            c_scale: 896.0,
            ..p
        };
        assert_eq!(p10.rgb8(940.0, 512.0, 512.0), [255, 255, 255, 255]);
        // Pure Cr pushes red up and green down.
        let [r, g, _, _] = p.rgb8(126.0, 128.0, 240.0);
        assert!(r > 200 && g < 100, "{r} {g}");
    }

    #[test]
    fn uniform_block_is_64_bytes() {
        let p = ColorParams {
            y_off: 16.0,
            y_scale: 219.0,
            c_off: 128.0,
            c_scale: 224.0,
            kr: 0.2126,
            kb: 0.0722,
        };
        let u = p.uniform(3840, 2160, 0, 6, 65535);
        assert_eq!(u.len(), 64);
        // size = (w, h, chroma w, chroma h), then mode = (layout, shift, max code, -).
        assert_eq!(&u[8..12], &1920u32.to_le_bytes());
        assert_eq!(&u[12..16], &1080u32.to_le_bytes());
        assert_eq!(&u[20..24], &6u32.to_le_bytes());
    }

    #[test]
    fn rgba_compare_and_barcode() {
        assert_eq!(compare_rgba(&[1, 2, 3, 4], &[1, 4, 3, 4]), (2, 0.25));
        // Barcode 5 (bits 0 and 2) drawn as RGBA blocks.
        let (w, h) = (640usize, 360usize);
        let (bw, bh) = (w / 16, h / 18);
        let mut img = vec![0u8; w * h * 4];
        for y in 0..2 * bh {
            for x in 0..w {
                let k = x / bw;
                let bit = (5u32 >> k) & 1 == 1;
                let white = (y < bh) == bit;
                let v = if white { 255 } else { 0 };
                img[(y * w + x) * 4..][..4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        assert_eq!(barcode_from_rgba(&img, w, h), Some(5));
    }
}
