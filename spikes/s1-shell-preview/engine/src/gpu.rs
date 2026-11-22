//! wgpu frame producer: render pass -> (optional NV12 compute) -> async readback -> ring.
//!
//! WP-0C.02: the output size follows the preview canvas (`resize`, arch §10 step 1). Readback
//! buffers are sized for the ring's maximum frame once, so a resize only re-creates the render
//! target and the NV12 buffer; frames already in flight carry their own size in `FrameMeta`.
//! Scrub previews can be rendered at a reduced size first (`Quality::Low`, arch §10 step 5).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use s1_ring::{FrameMeta, Mapping, PixelFormat, RingWriter};

/// Which adapter to ask wgpu for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuPref {
    /// `PowerPreference::HighPerformance` (dGPU on hybrid laptops).
    High,
    /// `PowerPreference::LowPower` (iGPU on hybrid laptops).
    Low,
    /// Software / fallback adapter (WARP on Windows).
    Software,
}

/// Output resolution of one render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// The negotiated preview size.
    Full,
    /// Each dimension divided by the given factor (scrub preview, `FLAG_LOWRES`).
    Low(u32),
}

/// Smallest frame the engine renders (keeps the NV12 packing and the glyph readable).
const MIN_W: u32 = 64;
const MIN_H: u32 = 36;

/// Clamp a requested size into `[MIN, max]` keeping the aspect ratio, then round down to the
/// NV12 packing constraints (width % 4 == 0, even height).
#[must_use]
pub fn fit_size(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let s = f64::min(1.0, f64::min(f64::from(max_w) / w, f64::from(max_h) / h));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (w, h) = ((w * s).floor() as u32, (h * s).floor() as u32);
    (w.clamp(MIN_W, max_w) & !3, h.clamp(MIN_H, max_h) & !1)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    frame: u32,
    width: u32,
    height: u32,
    pts_ms: u32,
}

const FREE: u8 = 0;
const PENDING: u8 = 1;
const MAPPED: u8 = 2;
const FAILED: u8 = 3;

/// Readback buffers in flight. 3 lets the GPU run up to 2 frames ahead of the CPU copy.
const STAGING_BUFFERS: usize = 3;

struct Staging {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

struct InFlight {
    staging: usize,
    submission: wgpu::SubmissionIndex,
    meta: FrameMeta,
    padded_bpr: u32,
}

/// Size-dependent GPU resources: render target, NV12 buffer and its bind group.
struct Targets {
    width: u32,
    height: u32,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    nv12_buf: wgpu::Buffer,
    nv12_bg: wgpu::BindGroup,
}

impl Targets {
    fn new(
        device: &wgpu::Device,
        nv12_pipeline: &wgpu::ComputePipeline,
        uniforms: &wgpu::Buffer,
        width: u32,
        height: u32,
    ) -> Self {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let nv12_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nv12"),
            size: PixelFormat::Nv12.frame_len(width, height) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let nv12_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nv12-bg"),
            layout: &nv12_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&target_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: nv12_buf.as_entire_binding(),
                },
            ],
        });
        Self {
            width,
            height,
            target,
            target_view,
            nv12_buf,
            nv12_bg,
        }
    }
}

/// Counters for the once-per-second engine log line.
#[derive(Default, Debug, Clone, Copy)]
pub struct GpuStats {
    /// Frames published.
    pub published: u64,
    /// Sum of (publish - render submit) in µs.
    pub submit_to_publish_us: u64,
    /// Max of (publish - render submit) in µs.
    pub max_submit_to_publish_us: u64,
    /// Sum of ring copy time (memcpy + stamp + checksum) in µs.
    pub copy_us: u64,
}

/// GPU state: pipelines, the negotiated output size and the readback buffers.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Adapter description for the handshake / README.
    pub info: wgpu::AdapterInfo,
    render_pipeline: wgpu::RenderPipeline,
    nv12_pipeline: wgpu::ComputePipeline,
    uniforms: wgpu::Buffer,
    render_bg: wgpu::BindGroup,
    /// Negotiated preview size.
    full: Targets,
    /// Reduced-size targets for scrub previews (created on first use).
    low: Option<Targets>,
    max_w: u32,
    max_h: u32,
    staging: Vec<Staging>,
    in_flight: VecDeque<InFlight>,
    /// Running counters (reset by the caller).
    pub stats: GpuStats,
}

impl Gpu {
    /// Pick an adapter, create the device and all resources for `width` x `height`. Readback
    /// buffers are sized for frames up to `max_w` x `max_h` (the ring's slot capacity).
    pub fn new(pref: GpuPref, width: u32, height: u32, max_w: u32, max_h: u32) -> Result<Self> {
        if !width.is_multiple_of(4) || !height.is_multiple_of(2) {
            return Err(anyhow!("NV12 path needs width % 4 == 0 and even height"));
        }
        if width > max_w || height > max_h {
            return Err(anyhow!("initial size exceeds the maximum frame size"));
        }
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: match pref {
                GpuPref::Low => wgpu::PowerPreference::LowPower,
                GpuPref::High | GpuPref::Software => wgpu::PowerPreference::HighPerformance,
            },
            force_fallback_adapter: pref == GpuPref::Software,
            compatible_surface: None,
            ..Default::default()
        }))
        .context("no wgpu adapter")?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("s1-engine"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            ..Default::default()
        }))
        .context("request_device")?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frame.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("frame.wgsl").into()),
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("frame"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let nv12_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("nv12"),
            layout: None,
            module: &shader,
            entry_point: Some("cs_nv12"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let render_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame-bg"),
            layout: &render_pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let full = Targets::new(&device, &nv12_pipeline, &uniforms, width, height);
        let staging_len = u64::from(padded_bpr(max_w)) * u64::from(max_h);
        let staging_len = staging_len.max(PixelFormat::Nv12.frame_len(max_w, max_h) as u64);
        let staging = (0..STAGING_BUFFERS)
            .map(|i| Staging {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("staging-{i}")),
                    size: staging_len,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
            })
            .collect();
        Ok(Self {
            device,
            queue,
            info,
            render_pipeline,
            nv12_pipeline,
            uniforms,
            render_bg,
            full,
            low: None,
            max_w,
            max_h,
            staging,
            in_flight: VecDeque::new(),
            stats: GpuStats::default(),
        })
    }

    /// Frames submitted but not yet published.
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// Current (negotiated) output size.
    pub fn size(&self) -> (u32, u32) {
        (self.full.width, self.full.height)
    }

    /// Change the output size (clamped to the maximum, rounded for NV12). Frames already in
    /// flight are unaffected. Returns the size actually used.
    pub fn resize(&mut self, width: u32, height: u32) -> (u32, u32) {
        let (w, h) = fit_size(width, height, self.max_w, self.max_h);
        if (w, h) != self.size() {
            self.full = Targets::new(&self.device, &self.nv12_pipeline, &self.uniforms, w, h);
            self.low = None;
        }
        (w, h)
    }

    /// Render frame `frame_no` at `pts` and start its async readback. Returns `false` (frame
    /// dropped by the producer) if every readback buffer is still busy.
    pub fn render(
        &mut self,
        frame_no: u64,
        pts: i64,
        format: PixelFormat,
        rev: u64,
        quality: Quality,
    ) -> bool {
        let Some(idx) = self
            .staging
            .iter()
            .position(|s| s.state.load(Ordering::Acquire) == FREE)
        else {
            return false;
        };
        let flags = match quality {
            Quality::Full => 0,
            Quality::Low(div) => {
                let (fw, fh) = self.size();
                let div = div.max(1);
                let (w, h) = fit_size(fw / div, fh / div, fw, fh);
                if self
                    .low
                    .as_ref()
                    .is_none_or(|t| (t.width, t.height) != (w, h))
                {
                    self.low = Some(Targets::new(
                        &self.device,
                        &self.nv12_pipeline,
                        &self.uniforms,
                        w,
                        h,
                    ));
                }
                s1_ring::FLAG_LOWRES
            }
        };
        let t = match (quality, &self.low) {
            (Quality::Low(_), Some(low)) => low,
            _ => &self.full,
        };
        let (width, height) = (t.width, t.height);
        let pts_ms = pts / (s1_ring::FLICKS_PER_SECOND / 1000);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let u = Uniforms {
            frame: frame_no as u32,
            width,
            height,
            pts_ms: pts_ms.max(0) as u32,
        };
        self.queue
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.render_pipeline);
            pass.set_bind_group(0, &self.render_bg, &[]);
            pass.draw(0..3, 0..1);
        }
        let staging = &self.staging[idx];
        let bpr = padded_bpr(width);
        match format {
            PixelFormat::Rgba8 => enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &t.target,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging.buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bpr),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            ),
            PixelFormat::Nv12 => {
                {
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("nv12"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&self.nv12_pipeline);
                    pass.set_bind_group(0, &t.nv12_bg, &[]);
                    pass.dispatch_workgroups((width / 4).div_ceil(64), height * 3 / 2, 1);
                }
                let len = PixelFormat::Nv12.frame_len(width, height) as u64;
                enc.copy_buffer_to_buffer(&t.nv12_buf, 0, &staging.buffer, 0, len);
            }
        }
        let t_render_us = s1_ring::clock::now_us();
        let submission = self.queue.submit([enc.finish()]);
        staging.state.store(PENDING, Ordering::Release);
        let state = Arc::clone(&staging.state);
        staging.buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            state.store(if r.is_ok() { MAPPED } else { FAILED }, Ordering::Release);
        });
        self.in_flight.push_back(InFlight {
            staging: idx,
            submission,
            meta: FrameMeta {
                pts,
                width,
                height,
                format,
                rev,
                flags,
                t_render_us,
            },
            padded_bpr: if format == PixelFormat::Rgba8 {
                bpr
            } else {
                width
            },
        });
        true
    }

    /// Publish every completed readback, oldest first, waiting for the oldest outstanding one
    /// until `deadline`. Returns the number of frames published.
    pub fn publish_ready<M: Mapping>(
        &mut self,
        ring: &mut RingWriter<M>,
        deadline: Instant,
    ) -> Result<usize> {
        let mut published = 0;
        while let Some(front) = self.in_flight.front() {
            let state = self.staging[front.staging].state.load(Ordering::Acquire);
            match state {
                MAPPED => {
                    let f = self
                        .in_flight
                        .pop_front()
                        .context("in_flight front vanished")?;
                    self.publish_one(ring, &f)?;
                    published += 1;
                }
                FAILED => {
                    let f = self
                        .in_flight
                        .pop_front()
                        .context("in_flight front vanished")?;
                    self.staging[f.staging].state.store(FREE, Ordering::Release);
                    tracing::warn!("readback map failed; frame dropped");
                }
                _ => {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    let timeout = deadline - now;
                    match self.device.poll(wgpu::PollType::Wait {
                        submission_index: Some(front.submission.clone()),
                        timeout: Some(timeout.max(Duration::from_micros(100))),
                    }) {
                        Ok(_) | Err(wgpu::PollError::Timeout) => {}
                        Err(e) => return Err(anyhow!("device poll: {e}")),
                    }
                    // After a timeout the state is re-checked; if still pending we leave.
                    if self.staging[front.staging].state.load(Ordering::Acquire) == PENDING
                        && Instant::now() >= deadline
                    {
                        break;
                    }
                }
            }
        }
        Ok(published)
    }

    fn publish_one<M: Mapping>(&mut self, ring: &mut RingWriter<M>, f: &InFlight) -> Result<()> {
        let t0 = Instant::now();
        let staging = &self.staging[f.staging];
        {
            let view = staging
                .buffer
                .get_mapped_range(..)
                .context("get_mapped_range")?;
            let src: &[u8] = &view;
            let meta = f.meta;
            let row_len = meta.format.stride(meta.width) as usize;
            let len = meta.format.frame_len(meta.width, meta.height);
            let padded = f.padded_bpr as usize;
            ring.publish(&meta, |dst| {
                if padded == row_len {
                    dst[..len].copy_from_slice(&src[..len]);
                } else {
                    for (r, row) in dst[..len].chunks_exact_mut(row_len).enumerate() {
                        row.copy_from_slice(&src[r * padded..r * padded + row_len]);
                    }
                }
                len
            })?;
        }
        staging.buffer.unmap();
        staging.state.store(FREE, Ordering::Release);
        let now = s1_ring::clock::now_us();
        let lat = now.saturating_sub(f.meta.t_render_us);
        self.stats.published += 1;
        self.stats.submit_to_publish_us += lat;
        self.stats.max_submit_to_publish_us = self.stats.max_submit_to_publish_us.max(lat);
        self.stats.copy_us += u64::try_from(t0.elapsed().as_micros()).unwrap_or(u64::MAX);
        Ok(())
    }
}

/// `bytes_per_row` for texture -> buffer copies must be a multiple of 256.
fn padded_bpr(width: u32) -> u32 {
    (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
}

#[cfg(test)]
mod tests {
    use super::fit_size;

    #[test]
    fn fit_size_rounds_for_nv12_and_keeps_aspect() {
        assert_eq!(fit_size(1920, 1080, 2560, 1440), (1920, 1080));
        assert_eq!(fit_size(1283, 721, 2560, 1440), (1280, 720));
        // Larger than the ring allows: scaled into the bounds, aspect kept.
        assert_eq!(fit_size(3840, 2160, 2560, 1440), (2560, 1440));
        assert_eq!(fit_size(5000, 2000, 2560, 1440), (2560, 1024));
        // Tiny: clamped to the minimum.
        assert_eq!(fit_size(10, 10, 2560, 1440), (64, 36));
        let (w, h) = fit_size(961, 541, 2560, 1440);
        assert!(w % 4 == 0 && h % 2 == 0 && w <= 961 && h <= 541);
    }
}
