//! Spike S1 (WP-0C.01): named shared-memory preview frame ring, arch §10.
//!
//! One writer (the engine sidecar) publishes frames into 4–6 slots; any number of readers (the
//! napi addon) copy the newest one, latest-wins. Byte layout: `../RING.md` and [`layout`].
//!
//! Throwaway spike code: the product version is `cutweave-bridge` / `cutweave-playback`
//! (WP-1H.02), which should take the layout, not necessarily the code.

pub mod checksum;
pub mod layout;
mod ring;
pub mod shm;

pub use layout::{EngineState, FLICKS_PER_SECOND, PixelFormat};
pub use ring::{
    FrameMeta, ReadError, RingClock, RingError, RingReader, RingSnapshot, RingWriter, SlotMeta,
};
pub use shm::{HeapMapping, Mapping, SharedMemory, ShmError};

/// Host clock shared by the engine and the addon so latencies can be compared across
/// processes: microseconds since the Unix epoch (`GetSystemTimePreciseAsFileTime` on Windows).
pub mod clock {
    /// Microseconds since the Unix epoch.
    #[must_use]
    pub fn now_us() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX))
    }
}
