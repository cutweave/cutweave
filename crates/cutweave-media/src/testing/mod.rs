// SPDX-License-Identifier: GPL-3.0-or-later
//! Test support for crates that use the media layer without FFmpeg.
//!
//! - [`FakeBackend`]: a [`MediaBackend`](crate::MediaBackend) over registered in-memory media
//!   ([`FakeMedia`]: synthetic pictures with exact pts and sine tones).
//! - [`contract`]: the behaviour every backend shares, run per backend with
//!   [`backend_contract!`](crate::backend_contract).
//! - [`stills`]: still-image files (PNG, JPEG, TIFF, BMP, WebP) written in memory, with EXIF.
//! - [`SpyBackend`]: notes every file another backend is asked to read (WP-2F.02).
//!
//! Always compiled (no feature): it is small, pure Rust and needs no FFmpeg.

pub mod contract;
mod fake;
mod spy;
pub mod stills;

pub use fake::{
    FAKE_AUDIO_CODEC, FAKE_GOP_FRAMES, FAKE_VIDEO_CODEC, FakeAudio, FakeBackend, FakeMedia,
    FakeVideo, fake_frame_number,
};
pub use spy::{FileOpen, OpenKind, OpenLog, SpyBackend};
