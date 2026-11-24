// SPDX-License-Identifier: GPL-3.0-or-later
//! The FFmpeg calls `ffmpeg-next` 9.1 does not wrap: library versions, build strings, the log
//! callback and codec enumeration. This is the only `unsafe` in the crate (ADR-0003); every
//! block carries a `// SAFETY:` comment. WP-1D.02/1D.03 add per-stream seek, raw packet flags and
//! pixel-format descriptors here (S3 finding F2.1).

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
