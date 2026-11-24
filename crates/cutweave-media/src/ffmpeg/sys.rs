// SPDX-License-Identifier: GPL-3.0-or-later
//! The FFmpeg calls `ffmpeg-next` 9.1 does not wrap: library versions, build strings, the log
//! callback and codec enumeration. This is the only `unsafe` in the crate (ADR-0003); every
//! block carries a `// SAFETY:` comment. WP-1D.02 appended the codec-parameter, seek-index and
//! packet-flag reads (its section is delimited at the end); WP-1D.03 adds per-stream seek and
//! pixel-format descriptors here (S3 finding F2.1); WP-1E.02 the GPU-surface pool for hardware
//! encoders (its own delimited section); WP-1D.04 hardware decode devices, `get_format` and the
//! copy of decoded surfaces to system memory (its own delimited section).

use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use cutweave_model::StreamKind;
use ffmpeg_next::ffi;

use crate::CodecSupport;
use crate::version::{LibVersion, LinkedLibrary};

/// Owned copy of a C string from FFmpeg (empty for NULL).
fn owned(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    // SAFETY: FFmpeg returns NUL-terminated strings that live at least as long as the library
    // (version, license and configuration strings are static; codec names are in static AVCodec
    // tables). The bytes are copied before returning.
    unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
}

fn header_version(major: c_int, minor: c_int, micro: c_int) -> LibVersion {
    LibVersion::new(
        major.cast_unsigned(),
        minor.cast_unsigned(),
        micro.cast_unsigned(),
    )
}

/// Every linked library with its header (build-time) and loaded (run-time) version.
///
/// Only calls `*_version()` getters, which take no arguments and return a plain integer: safe to
/// call whatever library version was loaded, so this runs before anything that converts FFmpeg
/// enums.
pub(super) fn linked_libraries() -> Vec<LinkedLibrary> {
    // SAFETY (all five calls): argument-less getters returning `unsigned`; no memory is touched.
    let loaded = unsafe {
        [
            ffi::avutil_version(),
            ffi::avcodec_version(),
            ffi::avformat_version(),
            ffi::swscale_version(),
            ffi::swresample_version(),
        ]
    };
    let built = [
        header_version(
            ffi::LIBAVUTIL_VERSION_MAJOR,
            ffi::LIBAVUTIL_VERSION_MINOR,
            ffi::LIBAVUTIL_VERSION_MICRO,
        ),
        header_version(
            ffi::LIBAVCODEC_VERSION_MAJOR,
            ffi::LIBAVCODEC_VERSION_MINOR,
            ffi::LIBAVCODEC_VERSION_MICRO,
        ),
        header_version(
            ffi::LIBAVFORMAT_VERSION_MAJOR,
            ffi::LIBAVFORMAT_VERSION_MINOR,
            ffi::LIBAVFORMAT_VERSION_MICRO,
        ),
        header_version(
            ffi::LIBSWSCALE_VERSION_MAJOR,
            ffi::LIBSWSCALE_VERSION_MINOR,
            ffi::LIBSWSCALE_VERSION_MICRO,
        ),
        header_version(
            ffi::LIBSWRESAMPLE_VERSION_MAJOR,
            ffi::LIBSWRESAMPLE_VERSION_MINOR,
            ffi::LIBSWRESAMPLE_VERSION_MICRO,
        ),
    ];
    let names = [
        "libavutil",
        "libavcodec",
        "libavformat",
        "libswscale",
        "libswresample",
    ];
    names
        .iter()
        .zip(built)
        .zip(loaded)
        .map(|((&name, built), loaded)| LinkedLibrary {
            name,
            built,
            loaded: LibVersion::from_int(loaded),
        })
        .collect()
}

/// The FFmpeg version banner (`av_version_info()`, as in `ffmpeg -version`).
pub(super) fn version_info() -> String {
    // SAFETY: argument-less getter returning a static string.
    owned(unsafe { ffi::av_version_info() })
}

/// libavcodec's license line (`GPL version 3 or later`).
pub(super) fn license() -> String {
    // SAFETY: argument-less getter returning a static string.
    owned(unsafe { ffi::avcodec_license() })
}

/// The configure line the libraries were built with.
pub(super) fn configuration() -> String {
    // SAFETY: argument-less getter returning a static string.
    owned(unsafe { ffi::avcodec_configuration() })
}

/// Software codec implementations (not hardware or hybrid wrappers, not experimental), one entry
/// per implementation; `Capabilities::with_codecs` merges them by codec.
pub(super) fn software_codecs() -> Vec<CodecSupport> {
    const SKIP: u32 =
        ffi::AV_CODEC_CAP_HARDWARE | ffi::AV_CODEC_CAP_HYBRID | ffi::AV_CODEC_CAP_EXPERIMENTAL;
    let mut out = Vec::new();
    let mut opaque: *mut c_void = ptr::null_mut();
    loop {
        // SAFETY: `opaque` is the iteration cookie av_codec_iterate documents: NULL to start,
        // then only written by FFmpeg. The result is NULL at the end or points into libavcodec's
        // static codec table.
        let codec = unsafe { ffi::av_codec_iterate(&raw mut opaque) };
        // SAFETY: a non-NULL result points to a static, immutable AVCodec valid for the process
        // lifetime. Its enum fields are in range because the start-up check guarantees these
        // bindings' headers match the loaded library (version.rs).
        let Some(c) = (unsafe { codec.as_ref() }) else {
            break;
        };
        if c.capabilities.cast_unsigned() & SKIP != 0 {
            continue;
        }
        let kind = match c.type_ {
            ffi::AVMediaType::AVMEDIA_TYPE_VIDEO => StreamKind::Video,
            ffi::AVMediaType::AVMEDIA_TYPE_AUDIO => StreamKind::Audio,
            ffi::AVMediaType::AVMEDIA_TYPE_SUBTITLE => StreamKind::Subtitle,
            ffi::AVMediaType::AVMEDIA_TYPE_DATA => StreamKind::Data,
            ffi::AVMediaType::AVMEDIA_TYPE_ATTACHMENT => StreamKind::Attachment,
            _ => continue,
        };
        // SAFETY: `c.id` comes from a registered codec; avcodec_get_name returns a static string
        // for every id (or "unknown_codec").
        let id_name = owned(unsafe { ffi::avcodec_get_name(c.id) });
        let implementation = owned(c.name);
        if id_name.is_empty() || implementation.is_empty() {
            continue;
        }
        // SAFETY: `codec` is the valid AVCodec pointer from above; the call only reads it.
        let encoder = unsafe { ffi::av_codec_is_encoder(codec) } != 0;
        let (decoders, encoders) = if encoder {
            (Vec::new(), vec![implementation])
        } else {
            (vec![implementation], Vec::new())
        };
        out.push(CodecSupport {
            name: id_name,
            kind,
            decoders,
            encoders,
        });
    }
    out
}

/// The `va_list` parameter type bindgen gives the log callback and `av_log_format_line2`. On
/// x86_64 System V (Linux, macOS Intel) a `va_list` parameter decays to a pointer to
/// `__va_list_tag`; elsewhere (Windows, Apple arm64: `char *`; Linux arm64: a struct passed as C
/// passes it) it is `va_list` itself. A wrong guess fails to compile against
/// `av_log_set_callback`, never at run time.
#[cfg(all(target_arch = "x86_64", not(windows)))]
type VaListArg = *mut ffi::__va_list_tag;
#[cfg(not(all(target_arch = "x86_64", not(windows))))]
type VaListArg = ffi::va_list;

/// Longest formatted line kept (FFmpeg's default callback uses 1024 too); longer lines are cut.
const LINE: usize = 1024;

thread_local! {
    /// `av_log_format_line2`'s `print_prefix` state: whether the next fragment starts a line.
    static PRINT_PREFIX: Cell<c_int> = const { Cell::new(1) };
}

/// The `av_log` callback: formats the line exactly like FFmpeg's default callback and hands it
/// to [`super::log`]. Runs on whichever thread FFmpeg logs from (decoder threads included).
unsafe extern "C" fn log_callback(
    avcl: *mut c_void,
    level: c_int,
    fmt: *const c_char,
    vl: VaListArg,
) {
    // SAFETY: argument-less getter of FFmpeg's global log level.
    if level > unsafe { ffi::av_log_get_level() } || !super::log::enabled(level) {
        return;
    }
    let mut buf: [c_char; LINE] = [0; LINE];
    let mut prefix = PRINT_PREFIX.try_with(Cell::get).unwrap_or(1);
    // SAFETY: `avcl`, `fmt` and `vl` are the arguments FFmpeg passed to this callback, forwarded
    // unchanged and only once to the library's own formatter (the same contract as FFmpeg's
    // default callback). `buf` is LINE bytes and LINE is passed as its size, so the formatter
    // writes at most LINE - 1 bytes plus a NUL. `prefix` is a live local int.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)] // LINE = 1024
    let n = unsafe {
        ffi::av_log_format_line2(
            avcl,
            level,
            fmt,
            vl,
            buf.as_mut_ptr(),
            LINE as c_int,
            &raw mut prefix,
        )
    };
    let _ = PRINT_PREFIX.try_with(|p| p.set(prefix));
    if n < 0 {
        return;
    }
    // SAFETY: av_log_format_line2 NUL-terminates the buffer (n >= 0, size > 0).
    let text = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    // A panic must not unwind into C (it would abort the process): log routing is best effort.
    let _ = catch_unwind(AssertUnwindSafe(|| super::log::on_fragment(level, &text)));
}

/// Routes FFmpeg's log through [`log_callback`]. Idempotent.
pub(super) fn install_log_callback() {
    // SAFETY: `log_callback` has exactly the signature av_log_set_callback expects (checked by
    // the compiler against the bindings), is thread-safe (thread-local and mutex-guarded state
    // only) and never unwinds.
    unsafe { ffi::av_log_set_callback(Some(log_callback)) };
}

// ===== WP-1D.02: probe and frame index (everything below is this work package's) =============
//
// Reads of `AVCodecParameters` and of the demuxer's seek index, which `ffmpeg-next` 9.1 does not
// expose. Values that FFmpeg stores in C enums (pixel format, color code points, codec id) are
// read as plain integers and looked up through the matching `*_name` functions, declared below
// with `c_int` parameters (an enum is an `int` in the C ABI): the bindings' enums are Rust enums,
// and a value outside them would be undefined behaviour to even materialize, which a hostile
// file's header could provoke. The functions themselves range-check their argument.

use cutweave_model::ColorRange;
use ffmpeg_next::Packet;
use ffmpeg_next::codec::Parameters;
use ffmpeg_next::format::stream::Stream;
use ffmpeg_next::packet::Ref;

#[allow(clashing_extern_declarations)] // same symbols as the bindings', `int` for the enum
unsafe extern "C" {
    fn av_get_pix_fmt_name(pix_fmt: c_int) -> *const c_char;
    fn av_color_primaries_name(primaries: c_int) -> *const c_char;
    fn av_color_transfer_name(transfer: c_int) -> *const c_char;
    fn av_color_space_name(space: c_int) -> *const c_char;
    fn avcodec_get_name(id: c_int) -> *const c_char;
    fn avcodec_profile_name(id: c_int, profile: c_int) -> *const c_char;
}

/// `AVCOL_PRI_UNSPECIFIED`, `AVCOL_TRC_UNSPECIFIED` and `AVCOL_SPC_UNSPECIFIED` are all 2.
const COLOR_UNSPECIFIED: c_int = 2;

/// What the demuxer knows about one stream's codec (`AVCodecParameters`), as plain values.
#[derive(Clone, Debug, Default)]
pub(super) struct StreamParams {
    /// Codec name (`h264`), `None` for an unknown codec.
    pub codec: Option<String>,
    /// Codec profile name (`High`, `Main 10`), `None` when not reported.
    pub profile: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Pixel format name (`yuv420p10le`); video only.
    pub pixel_format: Option<String>,
    pub sample_rate: u32,
    pub channels: u32,
    /// Color tags by FFmpeg name (video only); `None` when unspecified.
    pub primaries: Option<String>,
    pub transfer: Option<String>,
    pub matrix: Option<String>,
    pub range: Option<ColorRange>,
}

/// `Some(name)` for a non-NULL C string.
fn name_of(s: *const c_char) -> Option<String> {
    (!s.is_null()).then(|| owned(s))
}

/// `name(v)` for a color code point, `None` when unspecified or unknown.
fn color_name(v: c_int, name: unsafe extern "C" fn(c_int) -> *const c_char) -> Option<String> {
    if v == COLOR_UNSPECIFIED {
        return None;
    }
    // SAFETY: the `*_name` functions range-check their argument and return a static string or
    // NULL.
    name_of(unsafe { name(v) })
}

/// The codec parameters of a stream as plain values.
pub(super) fn stream_params(par: &Parameters) -> StreamParams {
    // SAFETY: `as_ptr` returns the AVCodecParameters the stream owns; for a stream of an open
    // input it is non-NULL (checked below) and stays valid while `par` is borrowed.
    let p = unsafe { par.as_ptr() };
    if p.is_null() {
        return StreamParams::default();
    }
    let is_video = matches!(par.medium(), ffmpeg_next::media::Type::Video);
    // SAFETY (all raw reads below): `p` is a valid, aligned AVCodecParameters (see above). Plain
    // `c_int` fields are read as such. Enum-typed fields (`codec_id`, `color_*`) are read through
    // a `*const c_int` cast of the field's address, never as the bindings' enum types (see the
    // section comment): sound because those C enums are `int`-sized.
    unsafe {
        let int_at = |field: *const c_int| field.read();
        let codec_id = int_at(ptr::addr_of!((*p).codec_id).cast());
        let primaries = int_at(ptr::addr_of!((*p).color_primaries).cast());
        let transfer = int_at(ptr::addr_of!((*p).color_trc).cast());
        let matrix = int_at(ptr::addr_of!((*p).color_space).cast());
        let range = int_at(ptr::addr_of!((*p).color_range).cast());
        let format = (*p).format;
        StreamParams {
            codec: name_of(avcodec_get_name(codec_id)).filter(|n| n != "none"),
            profile: name_of(avcodec_profile_name(codec_id, (*p).profile)),
            width: (*p).width.max(0).cast_unsigned(),
            height: (*p).height.max(0).cast_unsigned(),
            // Only video streams store a pixel format here; audio streams store a sample format.
            pixel_format: if is_video && format >= 0 {
                name_of(av_get_pix_fmt_name(format))
            } else {
                None
            },
            sample_rate: (*p).sample_rate.max(0).cast_unsigned(),
            channels: (*p).ch_layout.nb_channels.max(0).cast_unsigned(),
            primaries: if is_video {
                color_name(primaries, av_color_primaries_name)
            } else {
                None
            },
            transfer: if is_video {
                color_name(transfer, av_color_transfer_name)
            } else {
                None
            },
            matrix: if is_video {
                color_name(matrix, av_color_space_name)
            } else {
                None
            },
            // AVCOL_RANGE_MPEG = 1 (limited), AVCOL_RANGE_JPEG = 2 (full).
            range: match range {
                1 if is_video => Some(ColorRange::Limited),
                2 if is_video => Some(ColorRange::Full),
                _ => None,
            },
        }
    }
}

/// The timestamps (stream time base) of the demuxer's seek index for `stream`, in index order.
/// MP4/MOV, AVI and similar demuxers fill it for every sample while reading the header; Matroska
/// keeps only sparse cue points and MPEG-TS none. `None` when the index is empty.
pub(super) fn seek_index_timestamps(stream: &Stream<'_>) -> Option<Vec<i64>> {
    // SAFETY: `as_ptr` is the stream of an open input, valid while `stream` is borrowed.
    let st = unsafe { stream.as_ptr() };
    // SAFETY: the argument is that valid AVStream; the getter only reads it.
    let count = unsafe { ffi::avformat_index_get_entries_count(st) };
    if count <= 0 {
        return None;
    }
    let mut out = Vec::with_capacity(count.cast_unsigned() as usize);
    for i in 0..count {
        // SAFETY: `i` is below the entry count just read; the function takes a non-const
        // AVStream* for historical reasons but does not modify it. The returned entry is read
        // immediately, before any other call that takes the stream.
        let entry = unsafe { ffi::avformat_index_get_entry(st.cast_mut(), i) };
        // SAFETY: a non-NULL result points to a valid AVIndexEntry (see above).
        let Some(entry) = (unsafe { entry.as_ref() }) else {
            break;
        };
        out.push(entry.timestamp);
    }
    Some(out)
}

/// `AV_PKT_FLAG_DISCARD`: the demuxer wants the decoder to decode this packet but drop its frame
/// (MP4/MOV edit lists). `ffmpeg-next`'s `packet::Flags` only knows KEY and CORRUPT and
/// truncates the rest, so the raw flag word is read.
pub(super) fn packet_is_discard(packet: &Packet) -> bool {
    // SAFETY: `as_ptr` points at the AVPacket owned by `packet`, which is borrowed for the read.
    let flags = unsafe { (*packet.as_ptr()).flags };
    flags & ffi::AV_PKT_FLAG_DISCARD != 0
}

// ===== end of WP-1D.02 section ==============================================================

// ---------------------------------------------------------------------------------------------
// Encode (WP-1E.01): swscale with an explicit YUV matrix and range, movable between threads.
// `ffmpeg-next` has no wrapper for `sws_setColorspaceDetails`, and its scaling context is not
// `Send`, which an `Encoder` must be. WP-1E.02 made it slice-threaded: an RGBA 1080p frame took
// about 14 ms on one thread, which alone held a hardware export under 2× realtime.
// ---------------------------------------------------------------------------------------------

/// A swscale context converting `src` to `dst` at one size, with the YUV side in an explicit
/// matrix (an `SWS_CS_*` value) and range. swscale's default is BT.601, so the encoder always
/// sets it. Large frames are converted in slices on swscale's own worker threads.
pub(super) struct Scaler(ptr::NonNull<ffi::SwsContext>);

// SAFETY: an SwsContext is heap state with no thread affinity: its slice-thread pool (if any) is
// owned by the context and synchronises internally, and nothing is thread-local. FFmpeg only
// forbids using one context from two threads at once, and `Scaler` is used through `&mut` by
// the one encoder that owns it.
unsafe impl Send for Scaler {}

/// Frames this many pixels or larger are converted on several threads (720p and up).
const THREADED_PIXELS: u64 = 1280 * 720;

impl Scaler {
    /// A `width × height` converter from `src` to `dst`. `colorspace` is the `SWS_CS_*` matrix
    /// of the YUV side(s); `src_full` / `dst_full` say whether each side is full range (RGB
    /// always is).
    pub(super) fn new(
        src: ffmpeg_next::format::Pixel,
        dst: ffmpeg_next::format::Pixel,
        (width, height): (u32, u32),
        colorspace: c_int,
        (src_full, dst_full): (bool, bool),
    ) -> Result<Scaler, ffmpeg_next::Error> {
        let (Ok(w), Ok(h)) = (c_int::try_from(width), c_int::try_from(height)) else {
            return Err(ffmpeg_next::Error::Other {
                errno: ffmpeg_next::error::EINVAL,
            });
        };
        // 1 for small frames; 0 is automatic, one slice per core.
        let threads = c_int::from(u64::from(width) * u64::from(height) < THREADED_PIXELS);
        // SAFETY: allocates a context with default fields, or returns NULL.
        let ctx = ptr::NonNull::new(unsafe { ffi::sws_alloc_context() }).ok_or(
            ffmpeg_next::Error::Other {
                errno: ffmpeg_next::error::ENOMEM,
            },
        )?;
        // From here on, dropping `scaler` frees the context.
        let scaler = Scaler(ctx);
        // SAFETY: `ctx` is the live, not yet initialised context allocated above and not shared.
        // Its public fields are set as documented for sws_init_context (the pixel formats as the
        // `int` values of the bindings' own enum). sws_getCoefficients returns a pointer into
        // libswscale's static, read-only coefficient table for any argument (unknown values fall
        // back to the default matrix); sws_setColorspaceDetails only reads it (it copies the
        // coefficients, into the slice contexts too). Brightness 0, contrast and saturation 1.0
        // in 16.16 fixed point are the identity.
        let ret = unsafe {
            let c = ctx.as_ptr();
            (*c).flags = ffi::SwsFlags::SWS_BILINEAR as std::ffi::c_uint
                | ffi::SwsFlags::SWS_ACCURATE_RND as std::ffi::c_uint;
            (*c).threads = threads;
            (*c).src_w = w;
            (*c).src_h = h;
            (*c).dst_w = w;
            (*c).dst_h = h;
            (*c).src_format = ffi::AVPixelFormat::from(src) as c_int;
            (*c).dst_format = ffi::AVPixelFormat::from(dst) as c_int;
            (*c).src_range = c_int::from(src_full);
            (*c).dst_range = c_int::from(dst_full);
            let ret = ffi::sws_init_context(c, ptr::null_mut(), ptr::null_mut());
            if ret < 0 {
                ret
            } else {
                let table = ffi::sws_getCoefficients(colorspace);
                ffi::sws_setColorspaceDetails(
                    c,
                    table,
                    c_int::from(src_full),
                    table,
                    c_int::from(dst_full),
                    0,
                    1 << 16,
                    1 << 16,
                )
            }
        };
        if ret < 0 {
            return Err(ffmpeg_next::Error::from(ret));
        }
        Ok(scaler)
    }

    /// Converts `input` into `output` (both allocated, of the context's size and formats).
    pub(super) fn run(
        &mut self,
        input: &ffmpeg_next::frame::Video,
        output: &mut ffmpeg_next::frame::Video,
    ) -> Result<(), ffmpeg_next::Error> {
        // SAFETY: the context is initialised and used by this one call; `output` and `input` are
        // live frames with allocated buffers in the formats and size the context was made for
        // (the encoder allocates both to match), so sws_scale_frame only writes `output`'s
        // buffers and only reads `input`.
        let ret =
            unsafe { ffi::sws_scale_frame(self.0.as_ptr(), output.as_mut_ptr(), input.as_ptr()) };
        if ret < 0 {
            return Err(ffmpeg_next::Error::from(ret));
        }
        Ok(())
    }
}

impl Drop for Scaler {
    fn drop(&mut self) {
        // SAFETY: frees the context this struct owns (and its slice threads); nothing else holds
        // it.
        unsafe { ffi::sws_freeContext(self.0.as_ptr()) };
    }
}

// ===== WP-1E.02: GPU surfaces for hardware encoders (this section is this work package's) ======
//
// VA-API (and D3D12VA, which the tests use on Windows) encoders take only GPU surfaces. The
// encoder gets a frames pool on a new device of that type (`hw_frames_ctx`), and every converted
// NV12 frame is uploaded into a surface from it. `ffmpeg-next` 9.1 wraps none of the `hwcontext`
// API. The device is the type's default one (VA-API: the first render node), never an adapter
// index (S3 finding H7).

/// A pool of NV12-backed GPU surfaces on a device of its own, for one encoder.
pub(super) struct HwFrames {
    /// Our reference to the `AVHWFramesContext` (which holds one to its device).
    frames: ptr::NonNull<ffi::AVBufferRef>,
}

// SAFETY: a frames context is reference-counted heap state with no thread affinity (FFmpeg's
// hwcontext API may be called from any thread, not concurrently on one context). `HwFrames` is
// owned by one encoder, used through it from one thread at a time.
unsafe impl Send for HwFrames {}

/// `AVERROR(errno)` as an `ffmpeg-next` error.
fn errno(code: c_int) -> ffmpeg_next::Error {
    ffmpeg_next::Error::Other { errno: code }
}

impl HwFrames {
    /// A pool of `format` surfaces (`AV_PIX_FMT_VAAPI`, `AV_PIX_FMT_D3D12`) of `width × height`
    /// NV12 on a new device of type `device` (`vaapi`, `d3d12va`).
    pub(super) fn new(
        device: &CStr,
        format: ffmpeg_next::format::Pixel,
        (width, height): (u32, u32),
    ) -> Result<HwFrames, ffmpeg_next::Error> {
        let (Ok(width), Ok(height)) = (c_int::try_from(width), c_int::try_from(height)) else {
            return Err(errno(ffmpeg_next::error::EINVAL));
        };
        // SAFETY: `device` is a NUL-terminated string; the call only reads it and returns
        // AV_HWDEVICE_TYPE_NONE for an unknown name (a value of the bindings' enum: the start-up
        // version lock ties these headers to the loaded library).
        let kind = unsafe { ffi::av_hwdevice_find_type_by_name(device.as_ptr()) };
        if kind == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            return Err(errno(ffmpeg_next::error::ENOSYS));
        }
        let mut device_ref: *mut ffi::AVBufferRef = ptr::null_mut();
        // SAFETY: `device_ref` is a valid out-pointer; a NULL device string and NULL options
        // open the type's default device. On success it holds a new reference we own.
        let ret = unsafe {
            ffi::av_hwdevice_ctx_create(&raw mut device_ref, kind, ptr::null(), ptr::null_mut(), 0)
        };
        if ret < 0 {
            return Err(ffmpeg_next::Error::from(ret));
        }
        // SAFETY: `device_ref` is the live device reference from above; av_hwframe_ctx_alloc
        // takes a reference of its own (NULL on allocation failure).
        let frames = unsafe { ffi::av_hwframe_ctx_alloc(device_ref) };
        // SAFETY: releases our device reference (the frames context keeps its own) and sets the
        // pointer to NULL.
        unsafe { ffi::av_buffer_unref(&raw mut device_ref) };
        let frames = ptr::NonNull::new(frames).ok_or(errno(ffmpeg_next::error::ENOMEM))?;
        // From here on, dropping `pool` releases the frames context.
        let pool = HwFrames { frames };
        // SAFETY: the `data` of a reference from av_hwframe_ctx_alloc is its AVHWFramesContext
        // (allocated by av_mallocz, so suitably aligned), which the caller fills in before
        // av_hwframe_ctx_init (documented usage); nothing else holds it yet. `format` comes from
        // the bindings' own enum.
        #[allow(clippy::cast_ptr_alignment)] // see above: FFmpeg's allocation is aligned
        let ret = unsafe {
            let ctx = (*frames.as_ptr()).data.cast::<ffi::AVHWFramesContext>();
            (*ctx).format = format.into();
            (*ctx).sw_format = ffi::AVPixelFormat::AV_PIX_FMT_NV12;
            (*ctx).width = width;
            (*ctx).height = height;
            ffi::av_hwframe_ctx_init(frames.as_ptr())
        };
        if ret < 0 {
            return Err(ffmpeg_next::Error::from(ret));
        }
        Ok(pool)
    }

    /// Gives the not yet opened encoder context `ctx` a reference to the pool
    /// (`hw_frames_ctx`), which the encoder needs to open.
    pub(super) fn attach(
        &self,
        ctx: &mut ffmpeg_next::codec::context::Context,
    ) -> Result<(), ffmpeg_next::Error> {
        // SAFETY: `frames` is our live reference; av_buffer_ref returns a new one or NULL.
        let reference = unsafe { ffi::av_buffer_ref(self.frames.as_ptr()) };
        if reference.is_null() {
            return Err(errno(ffmpeg_next::error::ENOMEM));
        }
        // SAFETY: `as_mut_ptr` is the live codec context `ctx` owns, not yet opened and not
        // shared. Any previous `hw_frames_ctx` is released first; the context then owns the new
        // reference and releases it when it is freed.
        unsafe {
            let c = ctx.as_mut_ptr();
            ffi::av_buffer_unref(&raw mut (*c).hw_frames_ctx);
            (*c).hw_frames_ctx = reference;
        }
        Ok(())
    }

    /// A new surface from the pool holding `frame` (NV12 at the pool's size).
    pub(super) fn upload(
        &self,
        frame: &ffmpeg_next::frame::Video,
    ) -> Result<ffmpeg_next::frame::Video, ffmpeg_next::Error> {
        let mut surface = ffmpeg_next::frame::Video::empty();
        // SAFETY: `surface` owns a fresh, empty AVFrame and the pool is initialised; on success
        // the frame holds a reference to a pool surface, released when `surface` drops.
        let ret =
            unsafe { ffi::av_hwframe_get_buffer(self.frames.as_ptr(), surface.as_mut_ptr(), 0) };
        if ret < 0 {
            return Err(ffmpeg_next::Error::from(ret));
        }
        // SAFETY: `surface` holds a pool surface and `frame` an allocated NV12 frame of the same
        // size (the encoder converts every input to that before uploading); the call copies
        // `frame` into the surface and only reads `frame`.
        let ret = unsafe { ffi::av_hwframe_transfer_data(surface.as_mut_ptr(), frame.as_ptr(), 0) };
        if ret < 0 {
            return Err(ffmpeg_next::Error::from(ret));
        }
        Ok(surface)
    }
}

impl Drop for HwFrames {
    fn drop(&mut self) {
        let mut reference = self.frames.as_ptr();
        // SAFETY: releases our one reference to the frames context (the encoder context and
        // surfaces in flight keep theirs).
        unsafe { ffi::av_buffer_unref(&raw mut reference) };
    }
}

// ===== end of WP-1E.02 section ===============================================================

// ===== WP-1D.03: decode (everything below up to the test module is this work package's) =====
//
// Per-stream seek (S3 finding F2.1), stream discard flags, pixel-format descriptors and the
// color fields of decoded frames, which `ffmpeg-next` 9.1 does not expose (or exposes only by
// transmuting C enums). As in the WP-1D.02 section, enum-typed fields a file can influence
// (frame color tags) are read as plain integers.

use ffmpeg_next::format::Pixel;
use ffmpeg_next::format::context::Input;

/// `avformat_seek_file` on one stream, in that stream's time base, landing at or before `ts`.
///
/// `Input::seek` in ffmpeg-next always passes stream index -1, so FFmpeg reads the timestamp in
/// `AV_TIME_BASE` (microseconds); rescaling a 1/90000 or 1/30720 timestamp to microseconds and
/// back can round across a frame boundary (S3 F2.1).
pub(super) fn seek_stream(
    input: &mut Input,
    stream: usize,
    ts: i64,
) -> Result<(), ffmpeg_next::Error> {
    let index = c_int::try_from(stream).map_err(|_| ffmpeg_next::Error::StreamNotFound)?;
    if stream >= input.nb_streams() as usize {
        return Err(ffmpeg_next::Error::StreamNotFound);
    }
    // SAFETY: `input` owns a valid, open AVFormatContext for the duration of the call (from
    // `avformat_open_input`, freed only by `Input`'s Drop) and `index` was checked against
    // `nb_streams` above. `avformat_seek_file` does not retain the pointer.
    let ret = unsafe { ffi::avformat_seek_file(input.as_mut_ptr(), index, i64::MIN, ts, ts, 0) };
    if ret < 0 {
        Err(ffmpeg_next::Error::from(ret))
    } else {
        Ok(())
    }
}

/// Marks every stream but `keep` `AVDISCARD_ALL`, so the demuxer skips their packets (MP4/MOV do
/// not even read them): a decoder reads only its own stream.
pub(super) fn discard_other_streams(input: &mut Input, keep: usize) {
    let n = input.nb_streams() as usize;
    for i in (0..n).filter(|&i| i != keep) {
        // SAFETY: `input` owns a valid AVFormatContext whose `streams` array holds `nb_streams`
        // valid AVStream pointers; `i < nb_streams`. Writing a valid `AVDiscard` constant into
        // `discard` is what `-map`/`-vn` do in the CLI; nothing else holds a reference to the
        // stream during the write.
        unsafe {
            let ctx = input.as_mut_ptr();
            let st = *(*ctx).streams.add(i);
            if let Some(st) = st.as_mut() {
                st.discard = ffi::AVDiscard::AVDISCARD_ALL;
            }
        }
    }
}

/// The layout facts of a pixel format that `ffmpeg-next`'s descriptor does not expose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PixDesc {
    /// Bits per sample of the first component.
    pub depth: u8,
    /// Horizontal chroma subsampling as a shift (1 = half width).
    pub log2_chroma_w: u8,
    /// Vertical chroma subsampling as a shift (1 = half height).
    pub log2_chroma_h: u8,
    /// Number of components (1 gray, 2 gray + alpha, 3 colour, 4 colour + alpha).
    pub components: u8,
    /// RGB rather than Y'CbCr (or gray).
    pub rgb: bool,
    /// Has an alpha channel.
    pub alpha: bool,
    /// A hardware surface or a bitstream format, not samples in memory.
    pub opaque: bool,
}

/// The descriptor of `format`, `None` for `Pixel::None`.
pub(super) fn pix_desc(format: Pixel) -> Option<PixDesc> {
    // SAFETY: av_pix_fmt_desc_get range-checks its argument and returns NULL or a pointer into
    // libavutil's static, immutable descriptor table (valid for the process lifetime). The
    // `AVPixelFormat` comes from ffmpeg-next's `Pixel`, so it is a valid enum value.
    let desc = unsafe { ffi::av_pix_fmt_desc_get(format.into()).as_ref() }?;
    let flags = desc.flags;
    let flag = |f: c_int| flags & u64::from(f.cast_unsigned()) != 0;
    Some(PixDesc {
        depth: u8::try_from(desc.comp[0].depth).unwrap_or(u8::MAX),
        log2_chroma_w: desc.log2_chroma_w,
        log2_chroma_h: desc.log2_chroma_h,
        components: desc.nb_components,
        rgb: flag(ffi::AV_PIX_FMT_FLAG_RGB),
        alpha: flag(ffi::AV_PIX_FMT_FLAG_ALPHA),
        opaque: flag(ffi::AV_PIX_FMT_FLAG_HWACCEL) || flag(ffi::AV_PIX_FMT_FLAG_BITSTREAM),
    })
}

/// The color tags a decoded frame carries, by FFmpeg name (`None` when unspecified).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FrameColor {
    pub primaries: Option<String>,
    pub transfer: Option<String>,
    pub matrix: Option<String>,
    pub range: Option<ColorRange>,
}

/// The color tags of `frame`.
pub(super) fn frame_color(frame: &ffmpeg_next::frame::Video) -> FrameColor {
    // SAFETY: `as_ptr` is the AVFrame `frame` owns, valid while it is borrowed. The four
    // enum-typed fields are read through `*const c_int` casts of their addresses (C enums are
    // `int`-sized), never as the bindings' Rust enums, so an out-of-range value from a stream
    // header cannot be materialized as an invalid enum (see the WP-1D.02 section).
    let (primaries, transfer, matrix, range) = unsafe {
        let f = frame.as_ptr();
        let int_at = |field: *const c_int| field.read();
        (
            int_at(ptr::addr_of!((*f).color_primaries).cast()),
            int_at(ptr::addr_of!((*f).color_trc).cast()),
            int_at(ptr::addr_of!((*f).colorspace).cast()),
            int_at(ptr::addr_of!((*f).color_range).cast()),
        )
    };
    FrameColor {
        primaries: color_name(primaries, av_color_primaries_name),
        transfer: color_name(transfer, av_color_transfer_name),
        matrix: color_name(matrix, av_color_space_name),
        // AVCOL_RANGE_MPEG = 1 (limited), AVCOL_RANGE_JPEG = 2 (full).
        range: match range {
            1 => Some(ColorRange::Limited),
            2 => Some(ColorRange::Full),
            _ => None,
        },
    }
}

/// Copies `rows` rows of `row_bytes` bytes of plane `plane` of a decoded `frame` into a tight
/// buffer. Works with negative line sizes (bottom-up pictures, which `ffmpeg-next`'s `data()`
/// cannot describe). `None` when the plane does not exist or is smaller than asked for.
pub(super) fn copy_plane(
    frame: &ffmpeg_next::frame::Video,
    plane: usize,
    row_bytes: usize,
    rows: usize,
) -> Option<Vec<u8>> {
    // The plane's height for the frame's own layout (av_image_fill_plane_sizes' rule: planes 1
    // and 2 are chroma).
    let desc = pix_desc(frame.format())?;
    let height = frame.height() as usize;
    let plane_rows = if plane == 1 || plane == 2 {
        height.div_ceil(1 << desc.log2_chroma_h)
    } else {
        height
    };
    if plane >= 8 || rows > plane_rows {
        return None;
    }
    // SAFETY: `as_ptr` is the AVFrame `frame` owns, valid while it is borrowed; `plane` is below
    // AV_NUM_DATA_POINTERS (8). Plain pointer and int fields are read.
    let (data, linesize) = unsafe {
        let f = frame.as_ptr();
        ((*f).data[plane], (*f).linesize[plane])
    };
    let step = isize::try_from(linesize).ok()?;
    if data.is_null() || (linesize.unsigned_abs() as usize) < row_bytes {
        return None;
    }
    let mut out = Vec::with_capacity(row_bytes.checked_mul(rows)?);
    for r in 0..rows {
        let offset = isize::try_from(r).ok()?.checked_mul(step)?;
        // SAFETY: a decoded frame's plane `plane` holds `plane_rows` rows of `|linesize|` bytes,
        // row `r` starting at `data + r × linesize` (for a negative linesize `data` points at
        // the bottom row in memory and rows go backwards); FFmpeg allocates the buffers that
        // way for every frame it returns. `r < rows <= plane_rows` and
        // `row_bytes <= |linesize|`, so the slice lies inside the plane. The frame is borrowed
        // for the whole copy, so its buffers stay alive.
        let row =
            unsafe { std::slice::from_raw_parts(data.offset(offset).cast_const(), row_bytes) };
        out.extend_from_slice(row);
    }
    Some(out)
}

/// `AVCodecParameters.seek_preroll`: samples an audio decoder must decode after a seek before
/// its output is exact (Opus: 80 ms); 0 when unknown.
pub(super) fn seek_preroll(par: &Parameters) -> u32 {
    // SAFETY: `as_ptr` returns the AVCodecParameters `par` refers to; it is checked for NULL and
    // only the plain `c_int` field is read, while `par` is borrowed.
    unsafe {
        let p = par.as_ptr();
        if p.is_null() {
            0
        } else {
            (*p).seek_preroll.max(0).cast_unsigned()
        }
    }
}

// ===== end of WP-1D.03 section ==============================================================

// ===== WP-1D.04: hardware decode (this section is this work package's) ======================
//
// Path (a) of Spike S3 (`spikes/s3-decode/src/hw.rs`, findings H1-H10): libavcodec's own H.264
// and HEVC decoders with a hardware device attached (`hw_device_ctx`) and a `get_format`
// callback that picks the device's surface format when libavcodec offers it. Frames come out as
// GPU surfaces; only the frame a request returns is copied to system memory (NV12 / P010) with
// `av_hwframe_transfer_data`. `ffmpeg-next` 9.1 wraps none of this. Pixel formats are handled
// as C ints (see the WP-1D.02 section), so no value from FFmpeg is materialized as a Rust enum.

use std::sync::atomic::{AtomicU32, Ordering};

/// A hardware device context (`AVHWDeviceContext`), shared by every decoder that uses it.
pub(super) struct HwDevice {
    /// Our reference to the device context.
    buf: ptr::NonNull<ffi::AVBufferRef>,
    kind: ffi::AVHWDeviceType,
}

// SAFETY: a device context is reference-counted heap state that is immutable once
// `av_hwdevice_ctx_create` has initialised it. FFmpeg shares one device between decoders on
// different threads (the CLI does): each decoder takes its own reference (atomic refcount), and
// the device types used here serialise access to the GPU context internally (D3D11VA's device
// lock, CUDA context push/pop, VA-API's display lock). We only take references and read it.
unsafe impl Send for HwDevice {}
// SAFETY: as above; `&HwDevice` only hands out new references and reads immutable fields.
unsafe impl Sync for HwDevice {}

impl HwDevice {
    /// Opens a device of FFmpeg type `kind` (`cuda`, `d3d11va`, `vaapi`, `videotoolbox`):
    /// `device` is the type's device string (a DRM node for VA-API), `vendor` a PCI vendor id
    /// for D3D11VA's `vendor_id` option (never an adapter index: S3 finding H7).
    pub(super) fn create(
        kind: &CStr,
        device: Option<&CStr>,
        vendor: Option<u16>,
    ) -> Result<HwDevice, ffmpeg_next::Error> {
        // SAFETY: `kind` is a NUL-terminated string; the call only reads it and returns
        // AV_HWDEVICE_TYPE_NONE for a name this build does not have.
        let ty = unsafe { ffi::av_hwdevice_find_type_by_name(kind.as_ptr()) };
        if ty == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            return Err(errno(ffmpeg_next::error::ENOSYS));
        }
        let mut opts: *mut ffi::AVDictionary = ptr::null_mut();
        if let Some(v) = vendor {
            let value = std::ffi::CString::new(format!("0x{v:04x}"))
                .map_err(|_| errno(ffmpeg_next::error::EINVAL))?;
            // SAFETY: `opts` starts NULL (av_dict_set allocates it); both strings are valid
            // NUL-terminated C strings, copied by FFmpeg. Freed below.
            unsafe { ffi::av_dict_set(&raw mut opts, c"vendor_id".as_ptr(), value.as_ptr(), 0) };
        }
        let mut buf: *mut ffi::AVBufferRef = ptr::null_mut();
        // SAFETY: `buf` is a valid out-pointer; `device` (if any) outlives the call; `opts` is
        // NULL or the dictionary built above, which FFmpeg only reads and we free right after.
        // On success `buf` holds one new reference we own (released in Drop).
        let ret = unsafe {
            let r = ffi::av_hwdevice_ctx_create(
                &raw mut buf,
                ty,
                device.map_or(ptr::null(), CStr::as_ptr),
                opts,
                0,
            );
            ffi::av_dict_free(&raw mut opts);
            r
        };
        if ret < 0 {
            return Err(ffmpeg_next::Error::from(ret));
        }
        let buf = ptr::NonNull::new(buf).ok_or(errno(ffmpeg_next::error::ENOMEM))?;
        Ok(HwDevice { buf, kind: ty })
    }

    /// The surface format (as a C int) libavcodec's `codec` decodes to on this device type, if
    /// that decoder has a hwaccel for it that takes a device context. ProRes, MJPEG and most
    /// intra codecs have none.
    pub(super) fn surface_format(&self, codec: &ffmpeg_next::Codec) -> Option<c_int> {
        for i in 0.. {
            // SAFETY: `codec` wraps a static AVCodec of the loaded library; the call returns
            // NULL past the last entry and a pointer into static data otherwise.
            let cfg = unsafe { ffi::avcodec_get_hw_config(codec.as_ptr(), i) };
            // SAFETY: NULL or a pointer to a static AVCodecHWConfig. Its `pix_fmt` is read
            // through a `*const c_int` cast (an `int`-sized C enum), never as the Rust enum.
            let (pix_fmt, methods, device_type) = unsafe {
                let cfg = cfg.as_ref()?;
                (
                    ptr::addr_of!(cfg.pix_fmt).cast::<c_int>().read(),
                    cfg.methods,
                    cfg.device_type,
                )
            };
            // A bindgen enum of flag values (1, 2, 4, 8): the cast is the C value.
            let by_device = methods & (ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as c_int) != 0;
            if device_type == self.kind && by_device {
                return Some(pix_fmt);
            }
        }
        None
    }
}

impl Drop for HwDevice {
    fn drop(&mut self) {
        let mut reference = self.buf.as_ptr();
        // SAFETY: releases our one reference (decoders keep theirs) and NULLs the local.
        unsafe { ffi::av_buffer_unref(&raw mut reference) };
    }
}

/// The `get_format` state of one hardware decoder (`AVCodecContext.opaque`).
struct Negotiation {
    /// The surface format to pick.
    want: c_int,
    /// `get_format` calls that picked it.
    hw_picks: AtomicU32,
    /// `get_format` calls where it was not offered (the stream's profile or bit depth has no
    /// hwaccel) or was withdrawn because the hwaccel failed to initialise, so a software format
    /// was picked.
    sw_picks: AtomicU32,
}

/// The hardware side of one decoder: owns the state its `get_format` callback reads. It must
/// outlive the codec context it was attached to, so the decoder is declared (and dropped)
/// before it.
pub(super) struct HwDecode {
    negotiation: Box<Negotiation>,
}

/// `AVCodecContext.get_format`: the wanted surface format if offered, else the first software
/// format (libavcodec then decodes this stream in software).
unsafe extern "C" fn get_format(
    ctx: *mut ffi::AVCodecContext,
    fmts: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    // SAFETY: libavcodec passes the context it decodes with; `opaque` was set by
    // `HwDecode::attach` to a `Negotiation` that outlives the context (see `HwDecode`).
    let neg = unsafe { (*ctx).opaque.cast::<Negotiation>().cast_const().as_ref() };
    let Some(neg) = neg else {
        return ffi::AVPixelFormat::AV_PIX_FMT_NONE;
    };
    let ints = fmts.cast::<c_int>();
    let none = ffi::AVPixelFormat::AV_PIX_FMT_NONE as c_int;
    let mut first_sw = None;
    for i in 0..1024 {
        // SAFETY: `fmts` is an AV_PIX_FMT_NONE-terminated array from libavcodec; we stop at the
        // terminator. Elements are read as C ints (an `int`-sized enum).
        let v = unsafe { ints.add(i).read() };
        if v == none {
            break;
        }
        if v == neg.want {
            neg.hw_picks.fetch_add(1, Ordering::Relaxed);
            // SAFETY: the same element read as the enum: libavcodec of the version-locked build
            // listed it, so it is one of the enum's values.
            return unsafe { fmts.add(i).read() };
        }
        // SAFETY: `av_pix_fmt_desc_get` range-checks its argument and returns NULL or a pointer
        // into a static table. The element is passed as the enum it came as.
        let desc = unsafe { ffi::av_pix_fmt_desc_get(fmts.add(i).read()).as_ref() };
        let hwaccel = u64::from(ffi::AV_PIX_FMT_FLAG_HWACCEL.cast_unsigned());
        if first_sw.is_none() && desc.is_some_and(|d| d.flags & hwaccel == 0) {
            first_sw = Some(i);
        }
    }
    neg.sw_picks.fetch_add(1, Ordering::Relaxed);
    match first_sw {
        // SAFETY: an element before the terminator, as above.
        Some(i) => unsafe { fmts.add(i).read() },
        None => ffi::AVPixelFormat::AV_PIX_FMT_NONE,
    }
}

impl HwDecode {
    /// Attaches `device` to the not yet opened codec context `ctx`, with a `get_format` that
    /// picks surface format `want` (from [`HwDevice::surface_format`]). The surface pools keep
    /// FFmpeg's default slack for frames the caller holds (D3D11VA: 4 surfaces); a decoder here
    /// holds at most two.
    pub(super) fn attach(
        ctx: &mut ffmpeg_next::codec::Context,
        device: &HwDevice,
        want: c_int,
    ) -> Result<HwDecode, ffmpeg_next::Error> {
        let negotiation = Box::new(Negotiation {
            want,
            hw_picks: AtomicU32::new(0),
            sw_picks: AtomicU32::new(0),
        });
        // SAFETY: `device.buf` is a live device reference; av_buffer_ref returns a new one or
        // NULL.
        let reference = unsafe { ffi::av_buffer_ref(device.buf.as_ptr()) };
        if reference.is_null() {
            return Err(errno(ffmpeg_next::error::ENOMEM));
        }
        // SAFETY: `as_mut_ptr` is the live codec context `ctx` owns, not opened yet and not
        // shared. The context takes the device reference (freed by avcodec_free_context; a
        // previous one is released first). `opaque` points into the boxed `Negotiation`, whose
        // heap address never moves and which the caller keeps alive past the context (see
        // `HwDecode`); only `get_format` reads it.
        unsafe {
            let c = ctx.as_mut_ptr();
            ffi::av_buffer_unref(&raw mut (*c).hw_device_ctx);
            (*c).hw_device_ctx = reference;
            (*c).opaque = ptr::from_ref::<Negotiation>(&negotiation)
                .cast_mut()
                .cast::<c_void>();
            (*c).get_format = Some(get_format);
        }
        Ok(HwDecode { negotiation })
    }

    /// Whether `get_format` ever picked a software format: the hardware is not decoding (all of)
    /// this stream.
    pub(super) fn fell_back(&self) -> bool {
        self.negotiation.sw_picks.load(Ordering::Relaxed) > 0
    }

    /// Whether `get_format` picked the surface format at least once (the hwaccel was offered).
    pub(super) fn was_offered(&self) -> bool {
        self.negotiation.hw_picks.load(Ordering::Relaxed) > 0
    }

    /// Whether `frame` is a hardware surface of this decoder.
    pub(super) fn is_surface(&self, frame: &ffmpeg_next::frame::Video) -> bool {
        // SAFETY: reads the plain int `format` field of the AVFrame `frame` owns.
        unsafe { (*frame.as_ptr()).format == self.negotiation.want }
    }
}

/// Copies hardware surface `surface` into system memory in the device's software layout (NV12
/// for 8-bit, P010 for 10-bit 4:2:0). Only the planes are copied: timestamps and colour stay on
/// `surface`. `out` keeps its buffers from the previous copy when it has the same layout and
/// size, so sequential decoding does not allocate a frame per copy.
pub(super) fn transfer_surface(
    surface: &ffmpeg_next::frame::Video,
    out: &mut ffmpeg_next::frame::Video,
) -> Result<(), ffmpeg_next::Error> {
    // SAFETY: both frames are valid AVFrames owned by the caller, `surface` a hardware frame
    // (the caller checked `is_surface`), whose `hw_frames_ctx` data is its AVHWFramesContext
    // (allocated by av_mallocz, so aligned). `out` is either unreferenced first, so
    // av_hwframe_transfer_data allocates fresh buffers in the frames context's software format,
    // or reused only when it already holds writable buffers of exactly that format and size
    // (from the previous transfer of this decoder). Plain int fields are read.
    #[allow(clippy::cast_ptr_alignment)] // see above: FFmpeg's allocation is aligned
    unsafe {
        let src = surface.as_ptr();
        let dst = out.as_mut_ptr();
        let frames = (*src).hw_frames_ctx;
        let sw_format = if frames.is_null() {
            None
        } else {
            let ctx = (*frames).data.cast::<ffi::AVHWFramesContext>();
            Some(ptr::addr_of!((*ctx).sw_format).cast::<c_int>().read())
        };
        let reuse = !(*dst).buf[0].is_null()
            && Some((*dst).format) == sw_format
            && (*dst).width == (*src).width
            && (*dst).height == (*src).height
            && ffi::av_frame_is_writable(dst) > 0;
        if reuse && ffi::av_hwframe_transfer_data(dst, src, 0) >= 0 {
            return Ok(());
        }
        ffi::av_frame_unref(dst);
        let ret = ffi::av_hwframe_transfer_data(dst, src, 0);
        if ret < 0 {
            return Err(ffmpeg_next::Error::from(ret));
        }
    }
    Ok(())
}

/// The NAL length-prefix size of an H.264 stream stored as `avcC` (MP4, MOV, Matroska): 1, 2
/// or 4 bytes, from its extradata. `None` for Annex B streams (MPEG-TS, raw), whose extradata, if
/// any, does not start with `avcC` version 1.
pub(super) fn avc_nal_length_size(par: &Parameters) -> Option<usize> {
    // SAFETY: `as_ptr` returns the AVCodecParameters `par` refers to, valid while it is borrowed
    // (checked for NULL). `extradata` is NULL or points to `extradata_size` bytes owned by the
    // parameters (plus padding), which are only read here, while `par` is borrowed.
    let header = unsafe {
        let p = par.as_ptr();
        if p.is_null() || (*p).extradata.is_null() {
            return None;
        }
        let size = usize::try_from((*p).extradata_size).ok()?;
        std::slice::from_raw_parts((*p).extradata.cast_const(), size.min(5))
    };
    match header {
        [1, _, _, _, last, ..] => Some(usize::from(last & 3) + 1),
        _ => None,
    }
}

// ===== end of WP-1D.04 section ==============================================================

#[cfg(test)]
pub(super) mod test_support {
    //! Test-only access to `av_log` (C-variadic, so it cannot be wrapped safely in general).
    use super::{c_int, ffi};
    use std::ffi::CString;

    /// Logs `message` through `av_log` at `level` with no context, as a library would.
    pub(crate) fn av_log(level: c_int, message: &str) {
        let Ok(text) = CString::new(message) else {
            return;
        };
        // SAFETY: "%s" consumes exactly the one `const char *` argument passed, which is a valid
        // NUL-terminated string kept alive by `text` for the call.
        unsafe { ffi::av_log(std::ptr::null_mut(), level, c"%s".as_ptr(), text.as_ptr()) };
    }
}
