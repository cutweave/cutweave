// SPDX-License-Identifier: GPL-3.0-or-later
//! Small FFmpeg calls that `ffmpeg-next` 9.1 does not expose safely. Each one is a finding for the
//! `MediaBackend` FFmpeg module (WP-1D.01). WP-0E.02 adds the hwaccel FFI in `hw.rs`, a process
//! CPU-time probe here, and the zero-copy experiment in `zerocopy.rs`.

use ffmpeg::codec::packet::Ref as _;
use ffmpeg::ffi;
use ffmpeg::format::context::Input;
use ffmpeg_next as ffmpeg;

/// `avformat_seek_file` on one stream, in that stream's time base, landing at or before `ts`.
///
/// `Input::seek` in ffmpeg-next always passes stream index -1, which makes FFmpeg interpret the
/// timestamp in `AV_TIME_BASE` (microseconds). Rescaling a 1/90000 or 1/24000 timestamp to
/// microseconds and back can round across a frame boundary, so the spike seeks per stream.
pub fn seek_stream(input: &mut Input, stream: usize, ts: i64) -> Result<(), ffmpeg::Error> {
    let stream = i32::try_from(stream).map_err(|_| ffmpeg::Error::StreamNotFound)?;
    if stream >= i32::try_from(input.nb_streams()).unwrap_or(i32::MAX) {
        return Err(ffmpeg::Error::StreamNotFound);
    }
    // SAFETY: `input` owns a valid, open AVFormatContext for the duration of the call (the
    // pointer comes from `avformat_open_input` and is only freed by `Input`'s Drop), and
    // `stream` was checked against `nb_streams` above. `avformat_seek_file` does not retain
    // the pointer.
    let ret = unsafe { ffi::avformat_seek_file(input.as_mut_ptr(), stream, i64::MIN, ts, ts, 0) };
    if ret < 0 {
        Err(ffmpeg::Error::from(ret))
    } else {
        Ok(())
    }
}

/// `AV_PKT_FLAG_DISCARD`: the demuxer wants the decoder to decode this packet but drop its frame
/// (MP4/MOV edit lists). ffmpeg-next's `packet::Flags` only knows KEY and CORRUPT and truncates
/// the rest, so the raw flag word is read.
pub fn packet_is_discard(packet: &ffmpeg::Packet) -> bool {
    // SAFETY: `as_ptr` points at the AVPacket owned by `packet`, which is borrowed for the read.
    let flags = unsafe { (*packet.as_ptr()).flags };
    flags & ffi::AV_PKT_FLAG_DISCARD != 0
}

/// Luma layout of a pixel format: `(bit depth, bytes per sample, is planar luma, shift)`.
///
/// ffmpeg-next's `Descriptor` exposes the component count and chroma shifts but not the
/// per-component depth/step/shift, which the Y-plane CRC and barcode reader need. `shift` is
/// non-zero for MSB-aligned formats such as P010 (hardware decode output, WP-0E.02).
pub fn luma_layout(format: ffmpeg::format::Pixel) -> Option<(u32, usize, bool, u32)> {
    let desc = format.descriptor()?;
    // SAFETY: `descriptor()` returned a non-null pointer into FFmpeg's static, immutable
    // pixel-format descriptor table (`av_pix_fmt_desc_get`), valid for the process lifetime.
    let comp = unsafe { (*desc.as_ptr()).comp[0] };
    let depth = u32::try_from(comp.depth).ok()?;
    let shift = u32::try_from(comp.shift).ok()?;
    let bytes = if depth + shift > 8 { 2 } else { 1 };
    let planar = comp.plane == 0 && comp.offset == 0 && usize::try_from(comp.step).ok()? == bytes;
    Some((depth, bytes, planar, shift))
}

/// Process CPU time (user + kernel) so far, for CPU% per benchmark phase. Windows only: other
/// platforms return `None` (wrap the run in `/usr/bin/time -v` there, WP-0E.03).
pub fn process_cpu_time() -> Option<std::time::Duration> {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn GetProcessTimes(
                process: *mut std::ffi::c_void,
                creation: *mut u64,
                exit: *mut u64,
                kernel: *mut u64,
                user: *mut u64,
            ) -> i32;
        }
        let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
        // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no closing. The four
        // out-pointers are valid, writable u64s; a FILETIME is two little-endian u32 halves, the
        // same 8 bytes as a u64 counting 100 ns units.
        let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
        (ok != 0).then(|| std::time::Duration::from_nanos((k + u) * 100))
    }
    #[cfg(not(windows))]
    {
        None
    }
}
