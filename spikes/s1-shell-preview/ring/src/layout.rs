//! Byte layout of the ring (mirrors `RING.md`; keep both in sync).
//!
//! Everything is little-endian and lives in one named shared-memory object:
//!
//! ```text
//! 0      RingHeader   (256 B)
//! 256    SlotDesc[N]  (128 B each, N = slot_count, 4..=6)
//! 4096   payload 0    (slot_stride bytes, 4 KiB aligned)
//! ...    payload N-1
//! ```

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64};

/// `"CWS1"` read as a little-endian `u32`.
pub const MAGIC: u32 = u32::from_le_bytes(*b"CWS1");
/// Layout version. Bump on any incompatible change.
pub const VERSION: u32 = 1;
/// Size of the header page (ring header + slot table).
pub const HEADER_SIZE: usize = 4096;
/// Offset of the slot descriptor table.
pub const SLOT_TABLE_OFFSET: usize = 256;
/// Size of one slot descriptor.
pub const SLOT_DESC_SIZE: usize = 128;
/// Minimum number of slots (arch §10: 4–6).
pub const MIN_SLOTS: u32 = 4;
/// Maximum number of slots (arch §10: 4–6).
pub const MAX_SLOTS: u32 = 6;
/// Alignment of every payload region.
pub const PAYLOAD_ALIGN: usize = 4096;
/// Flicks per second (arch §4.1).
pub const FLICKS_PER_SECOND: i64 = 705_600_000;

/// Engine state word (`RingHeader::engine_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum EngineState {
    /// Ring initialised, no frame yet.
    Starting = 0,
    /// Producing frames at the target rate.
    Playing = 1,
    /// Paused; frames are only produced on seek.
    Paused = 2,
    /// Engine is shutting down cleanly.
    Stopped = 3,
}

impl EngineState {
    /// Decode the raw state word.
    #[must_use]
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::Starting),
            1 => Some(Self::Playing),
            2 => Some(Self::Paused),
            3 => Some(Self::Stopped),
            _ => None,
        }
    }
}

/// Pixel format of a slot payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PixelFormat {
    /// 4 bytes per pixel, R G B A, `stride` bytes per row.
    Rgba8 = 1,
    /// Y plane (`stride` bytes x `height` rows) followed by an interleaved UV plane
    /// (`stride` bytes x `height / 2` rows). BT.709 limited range.
    Nv12 = 2,
}

impl PixelFormat {
    /// Decode the raw format word.
    #[must_use]
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::Rgba8),
            2 => Some(Self::Nv12),
            _ => None,
        }
    }

    /// Payload size in bytes for a tightly packed frame.
    #[must_use]
    pub fn frame_len(self, width: u32, height: u32) -> usize {
        let (w, h) = (width as usize, height as usize);
        match self {
            Self::Rgba8 => w * h * 4,
            Self::Nv12 => w * h + w * (h / 2),
        }
    }

    /// Bytes per row of plane 0 for a tightly packed frame.
    #[must_use]
    pub fn stride(self, width: u32) -> u32 {
        match self {
            Self::Rgba8 => width * 4,
            Self::Nv12 => width,
        }
    }
}

/// Ring header at offset 0. All fields are atomics because they are shared across processes.
#[repr(C)]
pub struct RingHeader {
    /// [`MAGIC`].
    pub magic: AtomicU32,
    /// [`VERSION`].
    pub version: AtomicU32,
    /// [`HEADER_SIZE`].
    pub header_size: AtomicU32,
    /// Number of slots (4..=6).
    pub slot_count: AtomicU32,
    /// Maximum payload bytes per slot.
    pub slot_capacity: AtomicU64,
    /// Offset of payload 0 from the start of the mapping.
    pub payload_offset: AtomicU64,
    /// Distance between consecutive payloads (capacity rounded up to [`PAYLOAD_ALIGN`]).
    pub slot_stride: AtomicU64,
    /// Number of frames published so far. The newest frame is in slot `(write_index - 1) % N`.
    /// 0 means "no frame yet". Stored with `Release` after the slot is complete.
    pub write_index: AtomicU64,
    /// Fake audio clock in flicks, updated continuously (~1 kHz) by the engine.
    pub audio_clock_flicks: AtomicI64,
    /// Host time (µs since the Unix epoch) at which `audio_clock_flicks` was sampled.
    pub audio_clock_host_us: AtomicU64,
    /// Host time (µs since the Unix epoch) of the engine's last loop iteration.
    pub heartbeat_us: AtomicU64,
    /// Nominal frame duration in flicks (11_760_000 at 60 fps).
    pub frame_duration_flicks: AtomicI64,
    /// PID of the writer process.
    pub engine_pid: AtomicU32,
    /// [`EngineState`] as `u32`.
    pub engine_state: AtomicU32,
    /// Frames the engine skipped because no readback buffer was free (producer-side drops).
    pub engine_dropped: AtomicU64,
    _reserved: [u64; 20],
}

/// Per-slot descriptor in the slot table. Guarded by a seqlock (`lock`).
#[repr(C)]
pub struct SlotDesc {
    /// Seqlock word: odd while the writer is filling the slot, even when stable.
    pub lock: AtomicU64,
    /// Frame sequence number: equals `write_index` right after this frame was published
    /// (1-based, strictly increasing across slots).
    pub seq: AtomicU64,
    /// Presentation timestamp in flicks.
    pub pts: AtomicI64,
    /// Frame width in pixels.
    pub width: AtomicU32,
    /// Frame height in pixels.
    pub height: AtomicU32,
    /// [`PixelFormat`] as `u32`.
    pub format: AtomicU32,
    /// Bytes per row of plane 0.
    pub stride: AtomicU32,
    /// Render revision (project revision the frame was rendered from).
    pub rev: AtomicU64,
    /// Bytes of payload actually used.
    pub payload_len: AtomicU64,
    /// [`crate::checksum::sampled_fnv1a`] of the payload, computed after row stamping.
    pub checksum: AtomicU32,
    /// Reserved flag bits (0).
    pub flags: AtomicU32,
    /// Host time (µs since the Unix epoch) at which the frame's GPU work was submitted.
    pub t_render_us: AtomicU64,
    /// Host time (µs since the Unix epoch) at which the slot was published.
    pub t_publish_us: AtomicU64,
    _reserved: [u64; 6],
}

const _: () = assert!(size_of::<RingHeader>() == SLOT_TABLE_OFFSET);
const _: () = assert!(size_of::<SlotDesc>() == SLOT_DESC_SIZE);
const _: () = assert!(SLOT_TABLE_OFFSET + SLOT_DESC_SIZE * MAX_SLOTS as usize <= HEADER_SIZE);

/// Round `v` up to a multiple of `align` (a power of two).
#[must_use]
pub const fn align_up(v: usize, align: usize) -> usize {
    (v + align - 1) & !(align - 1)
}

/// Total mapping size for `slot_count` slots of `slot_capacity` payload bytes.
#[must_use]
pub const fn mapping_len(slot_count: u32, slot_capacity: usize) -> usize {
    HEADER_SIZE + slot_count as usize * align_up(slot_capacity, PAYLOAD_ALIGN)
}
