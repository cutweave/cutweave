// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-bridge`: napi-rs addon for the Electron preload (WP-1H.03, arch §10).
//!
//! Reads the engine's preview frame ring (`RING_LAYOUT_V1`, `crates/cutweave-playback/RING.md`)
//! from the main editor window's non-sandboxed preload (ADR-0001 transport (a)):
//!
//! - [`reader`]: one read-only mapping, validated at open (magic, version, geometry, **epoch**),
//!   seqlock-guarded frame copies, the audio clock, configuration and writer liveness;
//! - [`session`]: follows `Retired` -> successor rings on reconfigure, treats the writer as dead
//!   after 500 ms without heartbeat, releases the mapping when it is gone;
//! - `node`: the Node-API surface (`openRing`, `Ring`), pooled one-copy `ArrayBuffer`s (Electron's
//!   V8 memory cage forbids external buffers over shared memory).
//!
//! The reader never writes to the ring. Everything but `node` is plain Rust and is tested
//! against the real `FramePublisher`.

// Justification: this crate maps OS shared memory (Win32 file mappings, POSIX shm_open/mmap)
// and copies from memory another process writes (docs/01-architecture.md §3, AGENTS.md "No
// unsafe outside FFI/SHM/GPU crates"). Every `unsafe` block carries a `// SAFETY:` comment.
#![allow(unsafe_code)]
// napi signatures take owned values, and exported items are documented for the generated typings.
#![allow(clippy::needless_pass_by_value)]

mod error;
// napi-derive registers the exports with the addon loader only in non-test builds; in test builds
// the (unregistered) items look unused.
#[cfg_attr(test, allow(dead_code))]
mod node;
mod os;
pub mod reader;
pub mod session;

pub use error::BridgeError;
