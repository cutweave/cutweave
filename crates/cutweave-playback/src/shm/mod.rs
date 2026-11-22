// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared-memory preview ring publisher (WP-1H.02, arch §10, ADR-0001 transport (a)).
//!
//! The engine renders preview frames at the canvas pixel size, reads them back from the GPU
//! and publishes them into a named shared-memory ring that the napi-rs addon in the main
//! editor window's preload maps read-only (`cutweave-bridge`, WP-1H.03). The binary contract
//! is `RING_LAYOUT_V1`: normative text in `crates/cutweave-playback/RING.md`, Rust constants
//! in [`layout`], C mirror in `crates/cutweave-playback/include/cutweave_preview_ring_v1.h`.
//!
//! ```no_run
//! use cutweave_model::Time;
//! use cutweave_playback::shm::{
//!     ClockPump, ClockSample, ClockState, FrameMeta, FramePublisher, PixelFormat,
//!     PreviewConfig, RingConfig, DEFAULT_CLOCK_PERIOD,
//! };
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! cutweave_playback::shm::reap_stale_rings()?; // rings of engines that were killed
//! let mut publisher = FramePublisher::create(&RingConfig::new(1920, 1080, PixelFormat::Nv12))?;
//! let announce = publisher.announcement(); // -> run-file / handshake: name + epoch
//! let _pump = ClockPump::spawn(publisher.clock_writer(), DEFAULT_CLOCK_PERIOD, || {
//!     ClockSample::now(Time::ZERO, ClockState::Playing, 1000) // read AudioClock here
//! })?;
//! let layout = publisher.preview_layout()?; // render + read back with this layout
//! let readback = vec![0u8; layout.len()];
//! publisher.publish(&FrameMeta::default(), &layout, &readback)?;
//! publisher.configure(PreviewConfig { width: 1280, height: 720, format: PixelFormat::Nv12 })?;
//! # drop(announce);
//! # Ok(())
//! # }
//! ```

mod clock;
pub mod layout;
mod name;
mod os;
mod publisher;
mod ring;

#[cfg(test)]
mod reader;
#[cfg(test)]
mod tests;

pub use clock::{
    ClockPump, ClockSample, ClockWriter, DEFAULT_CLOCK_PERIOD, MAX_CLOCK_EXTRAPOLATION_NS,
};
pub use layout::{
    ClockState, FLAG_LOWRES, FrameLayout, HEARTBEAT_TIMEOUT_NS, HostClock, LAYOUT_VERSION,
    LayoutError, PixelFormat, PlaneLayout, WriterState,
};
pub use name::{RING_NAME_LEN, RING_NAME_PREFIX, RingName};
pub use os::{HOST_CLOCK, ShmError, reap_stale_rings, remove_stale_ring};
pub use publisher::{
    FrameMeta, FramePublisher, PreviewConfig, PublishError, Reconfigured, RingAnnouncement,
    RingConfig, RingError, SlotWriter,
};

/// Host monotonic time in nanoseconds: the clock of every `*_ns` ring field ([`HOST_CLOCK`]).
/// Readers compare `heartbeat_ns` and extrapolate the audio clock against this clock.
#[must_use]
pub fn host_clock_ns() -> u64 {
    os::host_clock_ns()
}
