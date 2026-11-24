// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardware decode through FFmpeg hwaccels (WP-0E.02, path a).
//!
//! The decoder stays libavcodec's own H.264/HEVC decoder (parsing, reference handling, RASL
//! skipping, `skip_frame`); only the slice decoding runs on the GPU. Setup per decoder:
//!
//! 1. `av_hwdevice_ctx_create(type, device)` once per source (`d3d11va:1` = DXGI adapter 1,
//!    `cuda:0` = CUDA device 0). Failure here is fallback case 1: the source decodes in software.
//! 2. `avcodec_get_hw_config`: the decoder must have a config for that device type with
//!    `AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX`. No config (ProRes, MJPEG) is fallback case 2.
//! 3. `hw_device_ctx` + a `get_format` callback that picks the hw pixel format when libavcodec
//!    offers it and the first software format otherwise. libavcodec calls it again without the hw
//!    format when hwaccel initialisation fails (unsupported profile, level or size), so cases 3 and
//!    4 (format not offered / init failed) fall back per stream, counted in [`Negotiation`].
//! 4. Frames come out as GPU surfaces (`AV_PIX_FMT_D3D11`, `CUDA`, `D3D12`, ...). Only the frame a
//!    request returns is copied to system memory with `av_hwframe_transfer_data` (NV12 for 8-bit,
//!    P010 for 10-bit); frames dropped on the way to a seek target are never transferred.
//!
//! The `unsafe` here is FFI only, like `ffi.rs`: each block has a `// SAFETY:` comment.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use ffmpeg::ffi;
use ffmpeg_next as ffmpeg;

/// FFmpeg hwaccel device types the harness knows by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwKind {
    D3d11va,
    D3d12va,
    Dxva2,
    Cuda,
    Vulkan,
    Qsv,
    Vaapi,
    VideoToolbox,
}

impl HwKind {
    /// FFmpeg's device type name (`av_hwdevice_find_type_by_name`).
    pub fn name(self) -> &'static str {
        match self {
            HwKind::D3d11va => "d3d11va",
            HwKind::D3d12va => "d3d12va",
            HwKind::Dxva2 => "dxva2",
            HwKind::Cuda => "cuda",
            HwKind::Vulkan => "vulkan",
            HwKind::Qsv => "qsv",
            HwKind::Vaapi => "vaapi",
            HwKind::VideoToolbox => "videotoolbox",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "d3d11va" | "d3d11" => HwKind::D3d11va,
            "d3d12va" | "d3d12" => HwKind::D3d12va,
            "dxva2" => HwKind::Dxva2,
            "cuda" | "nvdec" => HwKind::Cuda,
            "vulkan" => HwKind::Vulkan,
            "qsv" => HwKind::Qsv,
            "vaapi" => HwKind::Vaapi,
            "videotoolbox" | "vt" => HwKind::VideoToolbox,
            _ => return None,
        })
    }
}

/// A requested hwaccel: device type plus an optional device (adapter index, a GPU vendor, or a
/// DRM node for VA-API such as `vaapi:/dev/dri/renderD128`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HwSpec {
    pub kind: HwKind,
    /// Adapter / device index; `None` lets FFmpeg pick its default.
    pub device: Option<u8>,
    /// VA-API render node (`/dev/dri/renderD128` = 128); ignored by other types.
    pub render_node: Option<u8>,
    /// PCI vendor id (`d3d11va:nvidia` = 0x10de): FFmpeg's `vendor_id` device option. DXGI
    /// adapter indexes are not stable on hybrid laptops (finding H7), vendors are.
    pub vendor: Option<u16>,
}

fn vendor_id(name: &str) -> Option<u16> {
    match name {
        "nvidia" => Some(0x10de),
        "amd" => Some(0x1002),
        "intel" => Some(0x8086),
        _ => name
            .strip_prefix("vendor=0x")
            .and_then(|h| u16::from_str_radix(h, 16).ok()),
    }
}

fn vendor_name(id: u16) -> String {
    match id {
        0x10de => "nvidia".into(),
        0x1002 => "amd".into(),
        0x8086 => "intel".into(),
        _ => format!("vendor=0x{id:04x}"),
    }
}

impl HwSpec {
    /// Parses `d3d11va`, `d3d11va:1`, `d3d11va:nvidia`, `cuda:0`, `vaapi:/dev/dri/renderD129`, ...;
    /// `off`, `none` and `sw` mean "software decode" (`Ok(None)`).
    pub fn parse(s: &str) -> Result<Option<Self>, String> {
        if matches!(s, "off" | "none" | "sw") {
            return Ok(None);
        }
        let (kind, dev) = s.split_once(':').map_or((s, None), |(k, d)| (k, Some(d)));
        let kind = HwKind::parse(kind).ok_or_else(|| format!("unknown hwaccel {kind}"))?;
        let mut spec = HwSpec {
            kind,
            device: None,
            render_node: None,
            vendor: None,
        };
        match dev {
            None => {}
            Some(d) if d.starts_with("/dev/dri/renderD") => {
                spec.render_node = d["/dev/dri/renderD".len()..].parse().ok();
                if spec.render_node.is_none() {
                    return Err(format!("bad render node {d}"));
                }
            }
            Some(d) if vendor_id(d).is_some() => spec.vendor = vendor_id(d),
            Some(d) => {
                spec.device = Some(d.parse().map_err(|_| format!("bad device index {d}"))?);
            }
        }
        Ok(Some(spec))
    }

    /// `d3d11va`, `d3d11va:1`, `d3d11va:nvidia`, `vaapi:/dev/dri/renderD128`.
    pub fn label(self) -> String {
        match (self.device, self.render_node, self.vendor) {
            (_, Some(n), _) => format!("{}:/dev/dri/renderD{n}", self.kind.name()),
            (Some(d), None, _) => format!("{}:{d}", self.kind.name()),
            (None, None, Some(v)) => format!("{}:{}", self.kind.name(), vendor_name(v)),
            (None, None, None) => self.kind.name().to_string(),
        }
    }

    fn device_arg(self) -> Option<String> {
        match (self.device, self.render_node) {
            (_, Some(n)) => Some(format!("/dev/dri/renderD{n}")),
            (Some(d), None) => Some(d.to_string()),
            (None, None) => None,
        }
    }
}

/// Outcome of the hwaccel setup for one source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HwStatus {
    /// Software decode by request (`--hw off` or no `--hw`).
    Off,
    /// Hardware device opened and the decoder has a config for it. Whether frames actually come
    /// out as hw surfaces is decided per stream by `get_format` (see [`Negotiation`]).
    Attached { spec: HwSpec, pix_fmt: String },
    /// Fallback 1: `av_hwdevice_ctx_create` failed (no such adapter, driver, or API).
    NoDevice { spec: HwSpec, reason: String },
    /// Fallback 2: the decoder has no hwaccel for this device type (ProRes, MJPEG, ...).
    NoHwConfig { spec: HwSpec, codec: String },
}

impl HwStatus {
    /// One-line description.
    pub fn show(&self) -> String {
        match self {
            HwStatus::Off => "sw (hw off)".into(),
            HwStatus::Attached { spec, pix_fmt } => {
                format!("{} attached ({pix_fmt})", spec.label())
            }
            HwStatus::NoDevice { spec, reason } => {
                format!("sw fallback: {} device failed ({reason})", spec.label())
            }
            HwStatus::NoHwConfig { spec, codec } => {
                format!("sw fallback: {codec} has no {} hwaccel", spec.label())
            }
        }
    }
}

/// A hardware device context (`AVBufferRef` around an `AVHWDeviceContext`).
pub struct Device {
    buf: *mut ffi::AVBufferRef,
    ty: ffi::AVHWDeviceType,
    pub spec: HwSpec,
}

impl Device {
    /// Opens the device. Errors are fallback case 1.
    pub fn create(spec: HwSpec) -> Result<Self, String> {
        let name = CString::new(spec.kind.name()).map_err(|e| e.to_string())?;
        // SAFETY: `name` is a valid NUL-terminated string for the duration of the call.
        let ty = unsafe { ffi::av_hwdevice_find_type_by_name(name.as_ptr()) };
        if ty == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            return Err(format!(
                "{} not supported by this FFmpeg build",
                spec.kind.name()
            ));
        }
        let dev = spec
            .device_arg()
            .map(CString::new)
            .transpose()
            .map_err(|e| e.to_string())?;
        let mut buf: *mut ffi::AVBufferRef = ptr::null_mut();
        let mut opts: *mut ffi::AVDictionary = ptr::null_mut();
        // FFmpeg's d3d11va reads `vendor_id` only when no device string is given. (An index that
        // does not exist is not an error there: it silently opens the default adapter, H6.)
        if let (Some(v), None) = (spec.vendor, spec.device_arg()) {
            let (k, val) = (
                c"vendor_id",
                CString::new(format!("0x{v:04x}")).map_err(|e| e.to_string())?,
            );
            // SAFETY: `opts` starts NULL (av_dict_set allocates it); both strings are valid
            // NUL-terminated C strings copied by FFmpeg.
            unsafe { ffi::av_dict_set(&mut opts, k.as_ptr(), val.as_ptr(), 0) };
        }
        // SAFETY: `buf` is a valid out-pointer; `dev` (if any) outlives the call; `opts` is NULL
        // or a dictionary we own and free below. On success FFmpeg hands us one reference,
        // released in Drop.
        let ret = unsafe {
            let r = ffi::av_hwdevice_ctx_create(
                &mut buf,
                ty,
                dev.as_ref().map_or(ptr::null(), |d| d.as_ptr()),
                opts,
                0,
            );
            ffi::av_dict_free(&mut opts);
            r
        };
        if ret < 0 || buf.is_null() {
            return Err(ffmpeg::Error::from(ret).to_string());
        }
        Ok(Device { buf, ty, spec })
    }

    /// The device context buffer (borrowed; FFmpeg's `AVHWDeviceContext`).
    pub fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.buf
    }

    /// The hw pixel format `codec` decodes to on this device type, if it has such a hwaccel.
    pub fn pix_fmt_for(&self, codec: &ffmpeg::Codec) -> Option<ffi::AVPixelFormat> {
        for i in 0.. {
            // SAFETY: `codec` wraps a valid static AVCodec; `avcodec_get_hw_config` returns NULL
            // past the last entry and pointers into static data otherwise.
            let cfg = unsafe { ffi::avcodec_get_hw_config(codec.as_ptr(), i) };
            if cfg.is_null() {
                return None;
            }
            // SAFETY: non-null pointer to a static AVCodecHWConfig.
            let cfg = unsafe { &*cfg };
            if cfg.device_type == self.ty
                && cfg.methods & ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
            {
                return Some(cfg.pix_fmt);
            }
        }
        None
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: `buf` is the reference we own (decoders hold their own references).
        unsafe { ffi::av_buffer_unref(&mut self.buf) };
    }
}

/// State shared with the `get_format` callback through `AVCodecContext.opaque`. Boxed by the
/// caller so its address is stable, and kept alive at least as long as the decoder.
#[derive(Debug)]
pub struct Negotiation {
    /// The hw pixel format to pick (as the C int value).
    want: i32,
    /// `get_format` calls that picked the hw format.
    pub hw_picks: AtomicU32,
    /// `get_format` calls where the hw format was not offered (unsupported profile/bit depth, or
    /// a retry after hwaccel initialisation failed) and a software format was picked.
    pub sw_picks: AtomicU32,
}

impl Negotiation {
    pub fn new(want: ffi::AVPixelFormat) -> Self {
        Negotiation {
            want: want as i32,
            hw_picks: AtomicU32::new(0),
            sw_picks: AtomicU32::new(0),
        }
    }

    /// The hw pixel format as a C int (compare with `AVFrame.format`).
    pub fn hw_format(&self) -> i32 {
        self.want
    }
}

/// `AVCodecContext.get_format`: the hw format if offered, else the first software format.
unsafe extern "C" fn get_format(
    ctx: *mut ffi::AVCodecContext,
    fmts: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    // SAFETY: libavcodec passes the context it is decoding with; `opaque` was set by `attach` to
    // a `Negotiation` that outlives the decoder.
    let neg = unsafe { ((*ctx).opaque as *const Negotiation).as_ref() };
    let Some(neg) = neg else {
        return ffi::AVPixelFormat::AV_PIX_FMT_NONE;
    };
    let mut first_sw = None;
    for i in 0.. {
        // SAFETY: `fmts` is an AV_PIX_FMT_NONE-terminated array; we stop at the terminator. The
        // values are read as C ints so an unknown value can never form an invalid Rust enum.
        let v = unsafe { *(fmts as *const i32).add(i) };
        if v == ffi::AVPixelFormat::AV_PIX_FMT_NONE as i32 {
            break;
        }
        if v == neg.want {
            neg.hw_picks.fetch_add(1, Ordering::Relaxed);
            // SAFETY: same element, now read as the enum: it equals `want`, a valid variant.
            return unsafe { *fmts.add(i) };
        }
        // SAFETY: as above; elements before the terminator are valid pixel formats of the same
        // FFmpeg build (the headers match the DLLs, F2.3).
        let f = unsafe { *fmts.add(i) };
        // SAFETY: `av_pix_fmt_desc_get` returns NULL or a pointer into a static table.
        let desc = unsafe { ffi::av_pix_fmt_desc_get(f).as_ref() };
        let is_hw = desc.is_some_and(|d| d.flags & ffi::AV_PIX_FMT_FLAG_HWACCEL as u64 != 0);
        if !is_hw && first_sw.is_none() {
            first_sw = Some(f);
        }
    }
    neg.sw_picks.fetch_add(1, Ordering::Relaxed);
    first_sw.unwrap_or(ffi::AVPixelFormat::AV_PIX_FMT_NONE)
}

/// Attaches `device` and the `get_format` callback to an unopened codec context.
///
/// `neg` must stay at the same address and outlive the decoder (the caller boxes it and drops it
/// after the decoder).
pub fn attach(ctx: &mut ffmpeg::codec::Context, device: &Device, neg: &Negotiation) {
    // SAFETY: `ctx` owns a valid, not yet opened AVCodecContext. `av_buffer_ref` gives the
    // context its own reference to the device (freed by `avcodec_free_context`). `opaque` is
    // only read by `get_format` above, and the caller guarantees `neg` outlives the decoder.
    unsafe {
        let c = ctx.as_mut_ptr();
        (*c).hw_device_ctx = ffi::av_buffer_ref(device.as_ptr());
        (*c).opaque = neg as *const Negotiation as *mut std::ffi::c_void;
        (*c).get_format = Some(get_format);
    }
}

/// True when `frame` holds a hardware surface (its format is `hw_format`).
pub fn is_hw_frame(frame: &ffmpeg::frame::Video, hw_format: i32) -> bool {
    // SAFETY: reads the plain int `format` field of the AVFrame `frame` owns.
    unsafe { (*frame.as_ptr()).format == hw_format }
}

/// Copies a hardware frame to system memory (NV12 / P010 for 4:2:0 8 / 10-bit), with its
/// timestamps and colour properties.
pub fn transfer(
    hw: &ffmpeg::frame::Video,
    out: &mut ffmpeg::frame::Video,
) -> Result<(), ffmpeg::Error> {
    // SAFETY: both frames are valid AVFrames owned by the caller. `out` is unreferenced first so
    // `av_hwframe_transfer_data` allocates fresh buffers in the default software format;
    // `av_frame_copy_props` copies pts, colour tags and side data.
    unsafe {
        ffi::av_frame_unref(out.as_mut_ptr());
        let r = ffi::av_hwframe_transfer_data(out.as_mut_ptr(), hw.as_ptr(), 0);
        if r < 0 {
            return Err(ffmpeg::Error::from(r));
        }
        let r = ffi::av_frame_copy_props(out.as_mut_ptr(), hw.as_ptr());
        if r < 0 {
            return Err(ffmpeg::Error::from(r));
        }
    }
    Ok(())
}

/// Name of a pixel format.
pub fn pix_fmt_name(f: ffi::AVPixelFormat) -> String {
    // SAFETY: returns NULL or a pointer to a static NUL-terminated string.
    let p = unsafe { ffi::av_get_pix_fmt_name(f) };
    if p.is_null() {
        return format!("{f:?}");
    }
    // SAFETY: non-null static C string.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Which hwaccel device types this FFmpeg build was compiled with.
pub fn compiled_device_types() -> Vec<String> {
    let mut out = vec![];
    let mut t = ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE;
    loop {
        // SAFETY: iterates FFmpeg's static list; returns NONE at the end.
        t = unsafe { ffi::av_hwdevice_iterate_types(t) };
        if t == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            return out;
        }
        // SAFETY: static C string or NULL.
        let p = unsafe { ffi::av_hwdevice_get_type_name(t) };
        if !p.is_null() {
            // SAFETY: non-null static C string.
            out.push(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_specs() {
        assert_eq!(HwSpec::parse("off"), Ok(None));
        assert_eq!(HwSpec::parse("sw"), Ok(None));
        let s = HwSpec::parse("d3d11va:1").ok().flatten();
        assert_eq!(
            s.map(|s| (s.kind, s.device)),
            Some((HwKind::D3d11va, Some(1)))
        );
        assert_eq!(s.map(HwSpec::label).as_deref(), Some("d3d11va:1"));
        let s = HwSpec::parse("nvdec").ok().flatten();
        assert_eq!(s.map(|s| (s.kind, s.device)), Some((HwKind::Cuda, None)));
        let s = HwSpec::parse("vaapi:/dev/dri/renderD129").ok().flatten();
        assert_eq!(s.and_then(|s| s.render_node), Some(129));
        assert_eq!(
            s.map(HwSpec::label).as_deref(),
            Some("vaapi:/dev/dri/renderD129")
        );
        let s = HwSpec::parse("d3d11va:nvidia").ok().flatten();
        assert_eq!(s.and_then(|s| s.vendor), Some(0x10de));
        assert_eq!(s.map(HwSpec::label).as_deref(), Some("d3d11va:nvidia"));
        let s = HwSpec::parse("d3d11va:vendor=0x1af4").ok().flatten();
        assert_eq!(
            s.map(HwSpec::label).as_deref(),
            Some("d3d11va:vendor=0x1af4")
        );
        assert!(HwSpec::parse("bogus").is_err());
        assert!(HwSpec::parse("cuda:x").is_err());
    }
}
