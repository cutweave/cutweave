// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-cache`: frame cache, thumbnails, waveform pyramids, proxy manager and the SQLite
//! index (arch §7).
//!
//! WP-1D.02 ships the part everything else keys on:
//!
//! - [`CacheDb`]: `cache.sqlite`, opened with a versioned schema and ordered migrations, holding
//!   the [`FrameIndex`] of every analysed media file (table `frame_index`, keyed by content hash
//!   and stream) and accounting for its size with a least-recently-used eviction
//!   ([`CacheDb::evict_lru`]). Thumbnails, filmstrips and waveforms (WP-1D.05/1D.06) add their
//!   tables as further migrations and share the accounting.
//! - [`FrameIndex`] (from `cutweave-media`, re-exported): the packet-scan index that makes seeks
//!   frame-exact and maps VFR time to frames.
//!
//! WP-1D.05 adds the RAM frame cache ([`FrameCache`]), JPEG thumbnails and filmstrips
//! ([`ThumbCache`]) kept in a size-capped disk cache ([`DiskCache`], table `disk_entries`) and a
//! prefetch job ([`submit_prefetch`]).
//!
//! WP-1D.06 adds waveform pyramids ([`WaveformCache`], [`Waveform`], [`PeakData`]): min/max peaks
//! at 256, 1024, 4096, ... samples per bin, stored as one file per audio stream (not in SQLite;
//! format: docs/reference/waveform-format.md).
//!
//! The cache is disposable: everything in it can be rebuilt from the media, so a damaged or
//! too-new database is moved aside ([`CacheDb::open_or_reset`]) rather than repaired.

mod db;
mod disk;
mod error;
mod frame_cache;
mod thumbs;
mod waveform;
mod waveform_cache;

pub use cutweave_media::{ContentHash, FrameIndex, HashMode};
pub use db::{CacheDb, CacheUsage, Eviction, SCHEMA_VERSION};
pub use disk::{DiskCache, DiskEntry, DiskKind};
pub use error::CacheError;
pub use frame_cache::{FrameCache, FrameCacheStats, FrameKey};
pub use thumbs::{
    Filmstrip, FilmstripManifest, FilmstripSpec, MediaRef, PrefetchSpec, Thumb, ThumbCache,
    ThumbConfig, submit_prefetch,
};
pub use waveform::{
    BASE_SAMPLES_PER_BIN, LEVEL_FACTOR, MAX_LEVELS, Peak, PeakData, Track, Waveform,
    WaveformBuilder, WaveformOptions, WaveformStyle, render_png,
};
pub use waveform_cache::{WaveformCache, WaveformRequest};
