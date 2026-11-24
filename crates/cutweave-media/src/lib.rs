// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-media`: the media backend boundary (docs/01-architecture.md §7, ADR-0003).
//!
//! - [`MediaBackend`]: probe, open video/audio decoders and encoders, report [`Capabilities`].
//!   Synchronous, `Send + Sync`; shared as `Arc<dyn MediaBackend>`.
//! - [`VideoFrame`] / [`AudioChunk`]: decoded media with exact pts in flicks (ADR-0004).
//! - [`MediaError`]: typed failures; bad or damaged files never panic.
//! - [`testing`]: [`FakeBackend`] (synthetic pictures and tones, no FFmpeg) and the backend
//!   contract ([`backend_contract!`]) every backend passes.
//! - `FfmpegBackend` (feature `ffmpeg`): FFmpeg 8.1 through `ffmpeg-next` 9.1. Refuses to start
//!   unless the loaded libav* libraries equal the headers it was built against, and routes
//!   FFmpeg's log into `tracing` (target `ffmpeg`). Build and run setup: docs/dev/ffmpeg.md.
//!
//! WP-1D.01 ships the boundary. Probe and the frame index arrive with WP-1D.02, decode with
//! WP-1D.03, hardware decode with WP-1D.04 and encode with WP-1E.01; until then the FFmpeg
//! backend opens and validates files and answers [`MediaError::NotImplemented`] past that point.
//!
//! **Dependency rule** (arch §3, `cargo xtask check-deps`): `cutweave-model` never depends on
//! this crate, and `cutweave-render` / `cutweave-audio` may use only its trait half (the default
//! features): never the `ffmpeg` feature, never `ffmpeg-next` directly.

mod backend;
mod caps;
mod error;
mod frame;
#[cfg(any(feature = "ffmpeg", test))]
mod log_filter;
mod path;
pub mod testing;
mod version;

#[cfg(feature = "ffmpeg")]
mod ffmpeg;

pub use backend::{
    AudioDecoder, AudioEncodeSpec, DecodeOpts, EncodeSpec, EncodeSummary, Encoder, HwMode,
    MediaBackend, VideoDecoder, VideoEncodeSpec,
};
pub use caps::{Capabilities, CodecSupport, HwCodec};
pub use error::MediaError;
pub use frame::{AudioChunk, AudioFormat, PixelFormat, Plane, VideoFrame};
pub use testing::FakeBackend;
pub use version::{LibVersion, LinkedLibrary, PINNED_FFMPEG, Release};

#[cfg(feature = "ffmpeg")]
pub use ffmpeg::{FfmpegBackend, RuntimeInfo};
