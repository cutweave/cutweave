// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike S3 (WP-0E.01, WP-0E.02): FFmpeg decode through `ffmpeg-next`, a packet-scan frame index
//! and frame-exact seek, verified against ffprobe/ffmpeg ground truth; hardware decode through
//! FFmpeg hwaccels, GPU upload and a zero-copy experiment. Throwaway; see README.md.

pub mod bench;
pub mod error;
pub mod ffi;
pub mod frame_check;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod hw;
pub mod index;
pub mod manifest;
pub mod monitor;
pub mod planes;
pub mod probe;
pub mod robust;
pub mod source;
pub mod truth;
pub mod util;
pub mod verify;
#[cfg(all(feature = "zero-copy", windows))]
pub mod zerocopy;

/// Initialises FFmpeg once and silences its log below errors (corrupt-file tests are noisy).
pub fn init(quiet: bool) {
    let _ = ffmpeg_next::init();
    ffmpeg_next::util::log::set_level(if quiet {
        ffmpeg_next::util::log::Level::Fatal
    } else {
        ffmpeg_next::util::log::Level::Error
    });
}
