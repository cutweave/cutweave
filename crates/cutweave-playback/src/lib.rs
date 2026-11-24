// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-playback`: transport, A/V sync, prefetch, scrub scheduler and shared-memory frame
//! publisher (docs/01-architecture.md §10).
//!
//! - [`shm`]: the preview frame ring the engine publishes into and the Electron preload reads
//!   (WP-1H.02, `RING_LAYOUT_V1`).
//! - `transport` (feature `transport`): the transport state machine, JKL, A/V sync against the
//!   audio clock, the render loop with its latest-wins scrub scheduler, `playback.state` and
//!   the `view.*` transport commands (WP-1H.01). It renders through the `FrameSource` seam and
//!   publishes into [`shm::FramePublisher`]. The feature is off by default so the Electron
//!   addon, which only needs the ring layout, does not build the audio, render and command
//!   crates.

// Justification: the shared-memory ring maps OS objects through FFI (Win32 file mappings,
// POSIX shm_open/mmap) and copies frames into memory shared with another process
// (docs/01-architecture.md §3, AGENTS.md "No unsafe outside FFI/SHM/GPU crates"). Every
// `unsafe` block carries a `// SAFETY:` comment; they live in `shm::os` (FFI), `shm::ring` and
// `shm::publisher` (typed views of the mapping). The transport has none.
#![allow(unsafe_code)]

pub mod shm;
#[cfg(feature = "transport")]
#[deny(unsafe_code)]
pub mod transport;
