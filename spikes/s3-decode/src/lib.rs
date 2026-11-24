// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike S3 (WP-0E.01): FFmpeg decode through `ffmpeg-next`, a packet-scan frame index and
//! frame-exact seek, verified against ffprobe/ffmpeg ground truth. Throwaway; see README.md.

pub mod bench;
pub mod error;
pub mod ffi;
pub mod frame_check;
pub mod index;
pub mod manifest;
pub mod probe;
pub mod robust;
pub mod source;
pub mod truth;
pub mod util;
pub mod verify;

/// Initialises FFmpeg once and silences its log below errors (corrupt-file tests are noisy).
pub fn init(quiet: bool) {
    let _ = ffmpeg_next::init();
    ffmpeg_next::util::log::set_level(if quiet {
        ffmpeg_next::util::log::Level::Fatal
    } else {
        ffmpeg_next::util::log::Level::Error
    });
}
