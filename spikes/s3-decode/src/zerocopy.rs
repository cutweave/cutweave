// SPDX-License-Identifier: GPL-3.0-or-later
//! Zero-copy experiment, Windows (WP-0E.02 path b, feature `zero-copy`).
//!
//! The D3D11VA decoder writes into a texture array that FFmpeg owns. Per frame:
//!
//! 1. `CopySubresourceRegion` (GPU, same adapter) of the decoded array slice into one NV12/P010
//!    texture created with `D3D11_RESOURCE_MISC_SHARED | SHARED_NTHANDLE`. The decoder's own
//!    array is not shared: FFmpeg creates it with `D3D11_BIND_DECODER` only.
//! 2. Created once: an NT handle for that texture (`IDXGIResource1::CreateSharedHandle`), opened
//!    on wgpu's `ID3D12Device` (`OpenSharedHandle`), wrapped with
//!    `wgpu::hal::dx12::Device::texture_from_raw` + `create_texture_from_hal` as a
//!    `TextureFormat::NV12` / `P010` texture, read through its Plane0/Plane1 views.
//! 3. Sync on the GPU: a shared `ID3D11Fence` is signalled after the copy and waited on by wgpu's
//!    D3D12 queue (`wgpu::hal::dx12::Queue::add_wait_fence`) before the conversion pass.
//!    `S3_ZC_SYNC=cpu` waits on the CPU instead (D3D11 event query).
//!
//! No frame data crosses the CPU. The decoder and wgpu must be on the same adapter (LUIDs are
//! compared). The readback check compares the zero-copy RGB with the CPU conversion of the same
//! frame copied to system memory (path a), and reads the barcode off the result.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ffmpeg::ffi;
use ffmpeg_next as ffmpeg;
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE, LUID};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_FENCE_FLAG_SHARED, D3D11_QUERY_DESC,
    D3D11_QUERY_EVENT, D3D11_RESOURCE_MISC_SHARED, D3D11_RESOURCE_MISC_SHARED_NTHANDLE,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11Device5, ID3D11DeviceContext,
    ID3D11DeviceContext4, ID3D11Fence, ID3D11Query, ID3D11Texture2D,
};
use windows::Win32::Graphics::Direct3D12::{ID3D12Device, ID3D12Fence, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_FORMAT_P010, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIDevice,
    IDXGIFactory1, IDXGIResource1,
};
use windows::core::{Interface, PCWSTR};

use crate::gpu::{self, ColorParams, Gpu};
use crate::hw::{self, HwKind, HwSpec};
use crate::source::{Options, Threads, VideoSource};
use crate::util::{Rng, percentile};

type Dx12 = wgpu::hal::api::Dx12;

const COMMON_WGSL: &str = include_str!("shaders/yuv_common.wgsl");
const ZEROCOPY_WGSL: &str = include_str!("shaders/yuv_zerocopy.wgsl");

fn werr(op: &str) -> impl FnOnce(windows::core::Error) -> String + '_ {
    move |e| format!("{op}: {e}")
}

/// The D3D11 device FFmpeg decodes on, borrowed from its `AVD3D11VADeviceContext`.
struct D3d11 {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    context4: ID3D11DeviceContext4,
    lock: Option<unsafe extern "C" fn(*mut c_void)>,
    unlock: Option<unsafe extern "C" fn(*mut c_void)>,
    lock_ctx: *mut c_void,
}

impl D3d11 {
    fn from_ffmpeg(dev: &hw::Device) -> Result<Self, String> {
        if dev.spec.kind != HwKind::D3d11va {
            return Err("zero-copy needs --hw d3d11va[:N]".into());
        }
        // SAFETY: `dev` owns a live AVBufferRef whose data is an AVHWDeviceContext of type
        // D3D11VA (checked above), so `hwctx` is an AVD3D11VADeviceContext. The COM pointers in
        // it are valid while the device context lives; `from_raw_borrowed` + `clone` AddRef them.
        unsafe {
            let hwdev = (*dev.as_ptr()).data as *const ffi::AVHWDeviceContext;
            let d3d = (*hwdev).hwctx as *const ffi::AVD3D11VADeviceContext;
            let dev_raw = (*d3d).device as *mut c_void;
            let ctx_raw = (*d3d).device_context as *mut c_void;
            let device = ID3D11Device::from_raw_borrowed(&dev_raw)
                .ok_or("no ID3D11Device")?
                .clone();
            let context = ID3D11DeviceContext::from_raw_borrowed(&ctx_raw)
                .ok_or("no ID3D11DeviceContext")?
                .clone();
            let context4 = context
                .cast::<ID3D11DeviceContext4>()
                .map_err(werr("ID3D11DeviceContext4"))?;
            Ok(D3d11 {
                device,
                context,
                context4,
                lock: (*d3d).lock,
                unlock: (*d3d).unlock,
                lock_ctx: (*d3d).lock_ctx,
            })
        }
    }

    fn luid(&self) -> Result<LUID, String> {
        // SAFETY: plain COM calls on a live device.
        unsafe {
            let dxgi: IDXGIDevice = self.device.cast().map_err(werr("IDXGIDevice"))?;
            let adapter = dxgi.GetAdapter().map_err(werr("GetAdapter"))?;
            Ok(adapter.GetDesc().map_err(werr("GetDesc"))?.AdapterLuid)
        }
    }

    /// Runs `f` under FFmpeg's device lock (the decoder uses the same immediate context).
    fn locked<R>(&self, f: impl FnOnce() -> R) -> R {
        // SAFETY: FFmpeg's lock/unlock callbacks with their own context pointer, as documented
        // in hwcontext_d3d11va.h; always paired.
        unsafe {
            if let Some(l) = self.lock {
                l(self.lock_ctx);
            }
            let r = f();
            if let Some(u) = self.unlock {
                u(self.lock_ctx);
            }
            r
        }
    }
}

/// DXGI adapter index (FFmpeg's `d3d11va:N`) of the adapter with `luid`.
fn dxgi_index(luid: LUID) -> Option<u8> {
    // SAFETY: plain DXGI enumeration.
    unsafe {
        let f: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        for i in 0..16u32 {
            let a = f.EnumAdapters1(i).ok()?;
            if a.GetDesc1().ok()?.AdapterLuid == luid {
                return u8::try_from(i).ok();
            }
        }
    }
    None
}

fn wgpu_luid(g: &Gpu) -> Result<LUID, String> {
    // SAFETY: the hal adapter is only used for a GetDesc1 call while the guard lives.
    unsafe {
        let a = g
            .adapter()
            .as_hal::<Dx12>()
            .ok_or("the wgpu adapter is not DX12 (use --gpu <name>@dx12)")?;
        Ok(a.raw_adapter()
            .GetDesc1()
            .map_err(werr("GetDesc1"))?
            .AdapterLuid)
    }
}

fn wgpu_d3d12(g: &Gpu) -> Result<ID3D12Device, String> {
    // SAFETY: the raw device is cloned (AddRef) while the guard lives.
    unsafe {
        Ok(g.device()
            .as_hal::<Dx12>()
            .ok_or("the wgpu device is not DX12")?
            .raw_device()
            .clone())
    }
}

/// The shared texture and its wgpu import, per frame size and format.
struct Bridge {
    key: (u32, u32, i32),
    shared: ID3D11Texture2D,
    bind: wgpu::BindGroup,
    out: wgpu::Texture,
    readback: wgpu::Buffer,
    padded_row: u32,
    p010: bool,
}

/// Timing of one import (copy + sync + convert, until the GPU is done).
#[derive(Clone, Copy, Debug, Default)]
pub struct Import {
    pub total_ms: f64,
}

pub struct ZeroCopy {
    d3d11: D3d11,
    d3d12: ID3D12Device,
    fence11: ID3D11Fence,
    fence12: ID3D12Fence,
    value: u64,
    cpu_sync: bool,
    query: ID3D11Query,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    bridge: Option<Bridge>,
}

impl ZeroCopy {
    /// Sets up the shared fence and the conversion pipeline. `dev` must be the D3D11VA device the
    /// source decodes on, on the same adapter as `g` (DX12).
    pub fn new(g: &Gpu, dev: &hw::Device) -> Result<Self, String> {
        let d3d11 = D3d11::from_ffmpeg(dev)?;
        let (a, b) = (d3d11.luid()?, wgpu_luid(g)?);
        if a != b {
            return Err(format!(
                "decoder adapter LUID {:?} != wgpu adapter LUID {:?}: pick the same GPU",
                (a.HighPart, a.LowPart),
                (b.HighPart, b.LowPart)
            ));
        }
        let d3d12 = wgpu_d3d12(g)?;
        // SAFETY: COM calls on live devices; the NT handle is closed after the D3D12 side has
        // opened it (the fence keeps its own reference).
        let (fence11, fence12, query) = unsafe {
            let dev5: ID3D11Device5 = d3d11.device.cast().map_err(werr("ID3D11Device5"))?;
            let mut f11: Option<ID3D11Fence> = None;
            dev5.CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut f11)
                .map_err(werr("CreateFence"))?;
            let f11 = f11.ok_or("CreateFence returned nothing")?;
            let h = f11
                .CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null())
                .map_err(werr("fence CreateSharedHandle"))?;
            let mut f12: Option<ID3D12Fence> = None;
            let r = d3d12.OpenSharedHandle(h, &mut f12);
            let _ = CloseHandle(h);
            r.map_err(werr("fence OpenSharedHandle"))?;
            let mut q: Option<ID3D11Query> = None;
            d3d11
                .device
                .CreateQuery(
                    &D3D11_QUERY_DESC {
                        Query: D3D11_QUERY_EVENT,
                        MiscFlags: 0,
                    },
                    Some(&mut q),
                )
                .map_err(werr("CreateQuery"))?;
            (
                f11,
                f12.ok_or("OpenSharedHandle(fence) returned nothing")?,
                q.ok_or("CreateQuery returned nothing")?,
            )
        };
        let device = g.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv_zerocopy"),
            source: wgpu::ShaderSource::Wgsl(format!("{COMMON_WGSL}\n{ZEROCOPY_WGSL}").into()),
        });
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv_zerocopy"),
            entries: &[gpu::uniform_entry(0), tex(1), tex(2), gpu::storage_entry(4)],
        });
        let pipeline = gpu::compute_pipeline(device, &layout, &module, "convert_planes");
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("zc params"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(ZeroCopy {
            d3d11,
            d3d12,
            fence11,
            fence12,
            value: 0,
            cpu_sync: std::env::var("S3_ZC_SYNC").is_ok_and(|v| v == "cpu"),
            query,
            pipeline,
            layout,
            params,
            bridge: None,
        })
    }

    /// Creates the shared texture and its wgpu import for `(w, h, format)`.
    fn ensure_bridge(
        &mut self,
        g: &Gpu,
        w: u32,
        h: u32,
        format: DXGI_FORMAT,
    ) -> Result<(), String> {
        if self
            .bridge
            .as_ref()
            .is_some_and(|b| b.key == (w, h, format.0))
        {
            return Ok(());
        }
        let p010 = format == DXGI_FORMAT_P010;
        if !p010 && format != DXGI_FORMAT_NV12 {
            return Err(format!(
                "decoder texture format {format:?} is neither NV12 nor P010"
            ));
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0)
                as u32,
        };
        // SAFETY: COM calls on live devices. The NT handle is closed once D3D12 has opened the
        // resource (which holds its own reference). `texture_from_raw` takes ownership of the
        // `ID3D12Resource`; the wgpu texture keeps it alive.
        let (shared, resource) = unsafe {
            let mut t: Option<ID3D11Texture2D> = None;
            self.d3d11
                .device
                .CreateTexture2D(&desc, None, Some(&mut t))
                .map_err(werr("CreateTexture2D(shared NV12/P010)"))?;
            let t = t.ok_or("CreateTexture2D returned nothing")?;
            let r1: IDXGIResource1 = t.cast().map_err(werr("IDXGIResource1"))?;
            let h: HANDLE = r1
                .CreateSharedHandle(
                    None,
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                    PCWSTR::null(),
                )
                .map_err(werr("texture CreateSharedHandle"))?;
            let mut res: Option<ID3D12Resource> = None;
            let r = self.d3d12.OpenSharedHandle(h, &mut res);
            let _ = CloseHandle(h);
            r.map_err(werr("texture OpenSharedHandle"))?;
            (t, res.ok_or("OpenSharedHandle(texture) returned nothing")?)
        };
        let format = if p010 {
            wgpu::TextureFormat::P010
        } else {
            wgpu::TextureFormat::NV12
        };
        if !g.device().features().contains(if p010 {
            wgpu::Features::TEXTURE_FORMAT_P010
        } else {
            wgpu::Features::TEXTURE_FORMAT_NV12
        }) {
            return Err(format!("wgpu device lacks the {format:?} texture feature"));
        }
        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        let texture = unsafe {
            // SAFETY: `resource` is a live NV12/P010 2D texture of `size` on this device's
            // ID3D12Device (opened from the shared handle), 1 mip, 1 sample, in the COMMON state
            // (`UNINITIALIZED` maps to COMMON in wgpu-hal dx12).
            let hal_tex = wgpu::hal::dx12::Device::texture_from_raw(
                resource,
                format,
                wgpu::TextureDimension::D2,
                size,
                1,
                1,
            );
            g.device().create_texture_from_hal::<Dx12>(
                hal_tex,
                &wgpu::TextureDescriptor {
                    label: Some("zero-copy NV12/P010"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::TextureUses::UNINITIALIZED,
            )
        };
        let (fy, fc) = if p010 {
            (
                wgpu::TextureFormat::R16Unorm,
                wgpu::TextureFormat::Rg16Unorm,
            )
        } else {
            (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm)
        };
        let plane = |format, aspect| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("plane"),
                format: Some(format),
                aspect,
                ..Default::default()
            })
        };
        let vy = plane(fy, wgpu::TextureAspect::Plane0);
        let vc = plane(fc, wgpu::TextureAspect::Plane1);
        let out = g.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("zc rgb"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let vo = out.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = g.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuv_zerocopy"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&vy),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&vc),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&vo),
                },
            ],
        });
        if let Some(e) = g.take_errors() {
            return Err(format!("wgpu import: {e}"));
        }
        let padded_row = (w * 4).div_ceil(256) * 256;
        let readback = g.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("zc readback"),
            size: u64::from(padded_row) * u64::from(h),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.bridge = Some(Bridge {
            key: (w, h, desc.Format.0),
            shared,
            bind,
            out,
            readback,
            padded_row,
            p010,
        });
        Ok(())
    }

    /// Copies the decoded D3D11 frame into the shared texture, converts it in wgpu and waits for
    /// the GPU (texture ready). `frame` must be an `AV_PIX_FMT_D3D11` frame from this device.
    pub fn import(&mut self, g: &Gpu, frame: &ffmpeg::frame::Video) -> Result<Import, String> {
        let t0 = Instant::now();
        // SAFETY: an AV_PIX_FMT_D3D11 frame carries an ID3D11Texture2D* in data[0] and the array
        // index in data[1] (hwcontext_d3d11va.h); the frame keeps the texture alive.
        let (src_raw, index) = unsafe {
            let f = frame.as_ptr();
            if (*f).format != ffi::AVPixelFormat::AV_PIX_FMT_D3D11 as i32 {
                return Err(format!("not a D3D11 frame (format {})", (*f).format));
            }
            ((*f).data[0] as *mut c_void, (*f).data[1] as usize)
        };
        // SAFETY: borrowed COM pointer from the frame (valid while `frame` is).
        let src = unsafe { ID3D11Texture2D::from_raw_borrowed(&src_raw) }.ok_or("null texture")?;
        let mut sdesc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: plain COM call.
        unsafe { src.GetDesc(&mut sdesc) };
        let (w, h) = (frame.width(), frame.height());
        self.ensure_bridge(g, w, h, sdesc.Format)?;
        let b = self.bridge.as_ref().ok_or("no bridge")?;
        self.value += 1;
        let value = self.value;
        let region = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: w,
            bottom: h,
            back: 1,
        };
        let cpu_sync = self.cpu_sync;
        let d = &self.d3d11;
        let r: Result<(), String> = d.locked(|| {
            // SAFETY: COM calls on the decoder's immediate context under FFmpeg's lock; the
            // source slice index comes from the frame, the box is inside both textures.
            unsafe {
                d.context.CopySubresourceRegion(
                    &b.shared,
                    0,
                    0,
                    0,
                    0,
                    src,
                    u32::try_from(index).map_err(|e| e.to_string())?,
                    Some(&region),
                );
                if cpu_sync {
                    d.context.End(&self.query);
                } else {
                    d.context4
                        .Signal(&self.fence11, value)
                        .map_err(werr("ID3D11DeviceContext4::Signal"))?;
                }
                d.context.Flush();
            }
            Ok(())
        });
        r?;
        if cpu_sync {
            // An event query writes TRUE once the GPU has passed it; until then GetData returns
            // S_FALSE (a success code) and leaves the BOOL alone.
            let mut done: i32 = 0;
            while done == 0 {
                // SAFETY: `done` is a valid 4-byte BOOL out buffer for an event query.
                let _ = d.locked(|| unsafe {
                    d.context.GetData(
                        &self.query,
                        Some(&mut done as *mut i32 as *mut c_void),
                        4,
                        0,
                    )
                });
                std::thread::yield_now();
            }
        } else {
            // SAFETY: the hal queue is only used to stage a wait; the guard is dropped before
            // `submit`, which consumes the wait.
            unsafe {
                let q = g
                    .queue()
                    .as_hal::<Dx12>()
                    .ok_or("the wgpu queue is not DX12")?;
                q.add_wait_fence(self.fence12.clone(), value);
            }
        }
        let params = ColorParams::of(frame, if b.p010 { 10 } else { 8 });
        let (max_code, shift) = if b.p010 { (65535, 6) } else { (255, 0) };
        g.queue()
            .write_buffer(&self.params, 0, &params.uniform(w, h, 0, shift, max_code));
        let mut enc = g
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("zc") });
        {
            let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("zc convert"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.pipeline);
            cp.set_bind_group(0, &b.bind, &[]);
            cp.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
        g.queue().submit([enc.finish()]);
        g.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("poll: {e}"))?;
        if let Some(e) = g.take_errors() {
            return Err(e);
        }
        Ok(Import {
            total_ms: t0.elapsed().as_secs_f64() * 1e3,
        })
    }

    /// Reads back the last converted frame (RGBA8).
    pub fn read_rgba(&self, g: &Gpu) -> Result<Vec<u8>, String> {
        let b = self.bridge.as_ref().ok_or("nothing imported yet")?;
        let (w, h, _) = b.key;
        gpu::read_texture(
            g.device(),
            g.queue(),
            &b.out,
            &b.readback,
            b.padded_row,
            w,
            h,
        )
    }
}

/// `s3 zerocopy`: per file, readback check + random seeks + sequential playback to texture-ready.
pub fn run(
    files: &[PathBuf],
    gpu_choice: &str,
    hw: Option<HwSpec>,
    seeks: usize,
    seed: u64,
    out: &Path,
) -> Result<bool, String> {
    let choice = if gpu_choice.is_empty() {
        "@dx12"
    } else {
        gpu_choice
    };
    let g = Gpu::new(choice)?;
    if g.info.backend != wgpu::Backend::Dx12 {
        return Err(format!("{} is not DX12; use --gpu <name>@dx12", g.label()));
    }
    let spec = match hw {
        Some(s) => s,
        None => HwSpec {
            kind: HwKind::D3d11va,
            device: dxgi_index(wgpu_luid(&g)?),
            render_node: None,
        },
    };
    println!(
        "zerocopy: wgpu {} | decoder {} | sync {}",
        g.label(),
        spec.label(),
        if std::env::var("S3_ZC_SYNC").is_ok_and(|v| v == "cpu") {
            "CPU event query"
        } else {
            "shared D3D11/D3D12 fence (GPU wait)"
        }
    );
    let p = |v: &[f64], q: f64| percentile(v, q).unwrap_or(0.0);
    let mut rows = vec![];
    let mut all_ok = true;
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let opts = Options {
            threads: Threads::Single,
            skip_nonref: true,
            forward_continue: false,
            hw: Some(spec),
        };
        let mut src = VideoSource::open(path, opts).map_err(|e| format!("{name}: {e}"))?;
        let dev = src
            .hw_device()
            .ok_or_else(|| format!("{name}: {}", src.hw_status().show()))?;
        let mut zc = ZeroCopy::new(&g, dev)?;
        src.set_transfer(false);
        let frames = src.index.frames.len();
        // Readback check against the CPU conversion of the same frame copied to system memory.
        let mut ok = true;
        let mut check = String::new();
        let mut rng = Rng::new(seed ^ 0x7A7A);
        let mut sysmem = ffmpeg::frame::Video::empty();
        for pi in [0, frames / 2, rng.below(frames)] {
            src.seek_frame(pi)
                .map_err(|e| format!("{name}: seek {pi}: {e}"))?;
            zc.import(&g, src.decoded_frame())
                .map_err(|e| format!("{name}: frame {pi}: {e}"))?;
            let rgba = zc.read_rgba(&g)?;
            hw::transfer(src.decoded_frame(), &mut sysmem).map_err(|e| e.to_string())?;
            let (w, h) = (sysmem.width() as usize, sysmem.height() as usize);
            let (max, _) =
                gpu::cpu_reference(&sysmem).map_or((255, 1.0), |r| gpu::compare_rgba(&rgba, &r));
            let code = gpu::barcode_from_rgba(&rgba, w, h);
            let good = max <= 2 && code == Some(pi as u32);
            ok &= good;
            check.push_str(&format!("f{pi}: max diff {max} barcode {code:?}; "));
        }
        all_ok &= ok;
        // Random cold seeks to texture-ready.
        let (mut dec, mut imp, mut tot) = (vec![], vec![], vec![]);
        let mut rng = Rng::new(seed);
        for _ in 0..seeks {
            let pi = rng.below(frames);
            let t = Instant::now();
            src.seek_frame(pi)
                .map_err(|e| format!("{name}: seek {pi}: {e}"))?;
            let d = t.elapsed().as_secs_f64() * 1e3;
            let i = zc.import(&g, src.decoded_frame())?;
            dec.push(d);
            imp.push(i.total_ms);
            tot.push(t.elapsed().as_secs_f64() * 1e3);
        }
        // Sequential playback to texture-ready, with a pixel check every 50 frames.
        src.rewind().map_err(|e| e.to_string())?;
        let (mut sdec, mut simp, mut stot) = (vec![], vec![], vec![]);
        let t_all = Instant::now();
        let mut n = 0usize;
        let mut seq_bad = 0usize;
        loop {
            let t = Instant::now();
            if !src.next_frame().map_err(|e| e.to_string())? {
                break;
            }
            let d = t.elapsed().as_secs_f64() * 1e3;
            let i = zc.import(&g, src.decoded_frame())?;
            sdec.push(d);
            simp.push(i.total_ms);
            stot.push(t.elapsed().as_secs_f64() * 1e3);
            if n.is_multiple_of(50) {
                let rgba = zc.read_rgba(&g)?;
                let (w, h) = (
                    src.decoded_frame().width() as usize,
                    src.decoded_frame().height() as usize,
                );
                if gpu::barcode_from_rgba(&rgba, w, h) != Some(n as u32) {
                    seq_bad += 1;
                }
            }
            n += 1;
        }
        let fps = n as f64 / t_all.elapsed().as_secs_f64();
        ok &= seq_bad == 0;
        all_ok &= ok;
        println!(
            "{name:<34} seek: decode p50 {:>6.1} p95 {:>6.1} | copy+sync+convert p50 {:>5.2} p95 {:>5.2} | texture-ready p50 {:>6.1} p95 {:>6.1} ms",
            p(&dec, 50.0),
            p(&dec, 95.0),
            p(&imp, 50.0),
            p(&imp, 95.0),
            p(&tot, 50.0),
            p(&tot, 95.0)
        );
        println!(
            "{:<34} play: decode p50 {:>6.1} p95 {:>6.1} | copy+sync+convert p50 {:>5.2} p95 {:>5.2} | per frame p50 {:>6.1} p95 {:>6.1} ms | {fps:.1} fps",
            "",
            p(&sdec, 50.0),
            p(&sdec, 95.0),
            p(&simp, 50.0),
            p(&simp, 95.0),
            p(&stot, 50.0),
            p(&stot, 95.0)
        );
        println!(
            "    check: {check}sequential barcode misses {seq_bad} -> {} | {}",
            if ok { "ok" } else { "MISMATCH" },
            src.hw_summary()
        );
        rows.push(format!(
            "{name},{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{fps:.1},{ok}",
            g.label(),
            spec.label(),
            p(&dec, 50.0), p(&dec, 95.0), p(&imp, 50.0), p(&imp, 95.0), p(&tot, 50.0), p(&tot, 95.0),
            p(&sdec, 50.0), p(&sdec, 95.0), p(&simp, 50.0), p(&simp, 95.0), p(&stot, 50.0), p(&stot, 95.0)
        ));
    }
    let _ = std::fs::create_dir_all(out);
    let csv = out.join("zerocopy.csv");
    let mut s = String::from(
        "file,adapter,decoder,seek_decode_p50_ms,seek_decode_p95_ms,seek_import_p50_ms,seek_import_p95_ms,seek_texture_ready_p50_ms,seek_texture_ready_p95_ms,play_decode_p50_ms,play_decode_p95_ms,play_import_p50_ms,play_import_p95_ms,play_frame_p50_ms,play_frame_p95_ms,play_fps,check\n",
    );
    for r in rows {
        s.push_str(&r);
        s.push('\n');
    }
    let _ = std::fs::write(&csv, s);
    println!("wrote {}", csv.display());
    Ok(all_ok)
}
