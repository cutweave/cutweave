// SPDX-License-Identifier: GPL-3.0-or-later
//! `RING_LAYOUT_V1`: the byte layout of the preview frame ring.
//!
//! This module is the Rust half of a binary contract. The normative description is
//! `crates/cutweave-playback/RING.md`; `include/cutweave_preview_ring_v1.h` is the C mirror
//! (checked against these constants by a unit test). Change all three together, and bump
//! [`LAYOUT_VERSION`] for any incompatible change.
//!
//! ```text
//! 0                 RingHeader          1024 B
//! 1024              SlotDesc[N]         128 B each (N = slot_count)
//! payload_offset    payload[0]          slot_stride bytes   (16 KiB aligned)
//!                   payload[1] .. payload[N-1]
//! mapping_size    = payload_offset + N * slot_stride
//! ```
//!
//! Every field either side touches is accessed atomically; all integers are little-endian
//! (the only byte order Cutweave targets: x86-64 and arm64).

use std::mem::{offset_of, size_of};
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU32, AtomicU64};

use thiserror::Error;

/// `"CWPR"` (Cutweave Preview Ring) read as a little-endian `u32`.
pub const MAGIC: u32 = u32::from_le_bytes(*b"CWPR");
/// Layout version. Bump on any change an older reader would misinterpret.
pub const LAYOUT_VERSION: u32 = 1;
/// `required_features` bits a v1 reader understands (none). A reader must refuse a ring that
/// sets any bit it does not know.
pub const KNOWN_REQUIRED_FEATURES: u32 = 0;

/// Size of [`RingHeader`]; also the offset of the slot table.
pub const HEADER_SIZE: usize = 1024;
/// Offset of the slot descriptor table.
pub const SLOT_TABLE_OFFSET: usize = HEADER_SIZE;
/// Size of one [`SlotDesc`].
pub const SLOT_DESC_SIZE: usize = 128;
/// Fewest slots a v1 writer creates (arch §10: 4–6).
pub const MIN_SLOTS: u32 = 4;
/// Most slots a v1 writer creates (arch §10: 4–6).
pub const MAX_SLOTS: u32 = 6;
/// Slot count the publisher uses unless told otherwise (S1 ran 0-drop 60 s runs with 6).
pub const DEFAULT_SLOTS: u32 = 6;
/// Alignment of `payload_offset` and `slot_stride`. 16 KiB is the arm64 macOS page size, so
/// every payload is page aligned on every OS (room for a future zero-copy host-memory import).
pub const PAYLOAD_ALIGN: usize = 16 * 1024;
/// Offset of payload 0 written by v1 writers. Readers use the header value, not this constant.
pub const PAYLOAD_OFFSET: usize = PAYLOAD_ALIGN;
/// Bytes reserved for a ring name in the header (NUL padded).
pub const NAME_FIELD_LEN: usize = 32;
/// Largest frame width or height a v1 ring carries (8K).
pub const MAX_DIMENSION: u32 = 8192;
/// Flicks per second (arch §4.1); same value as `cutweave_model::FLICKS_PER_SECOND`.
pub const FLICKS_PER_SECOND: i64 = 705_600_000;
/// A reader should treat the writer as hung or dead when `heartbeat_ns` is older than this.
pub const HEARTBEAT_TIMEOUT_NS: u64 = 500_000_000;
/// `clock_rate_milli` for normal-speed forward playback.
pub const RATE_MILLI_1X: i32 = 1000;

/// `SlotDesc::flags` bit: reduced-resolution scrub preview. A full-resolution frame of the same
/// `pts` follows unless a newer seek supersedes it (arch §10 step 5). Other bits are 0 in v1.
pub const FLAG_LOWRES: u32 = 1;
/// All `SlotDesc::flags` bits defined in v1.
pub const KNOWN_FLAGS: u32 = FLAG_LOWRES;

/// Byte offsets of every header and slot field, shared with the C mirror and `RING.md`.
pub mod off {
    #![allow(missing_docs)] // Each constant is named after the field it locates.
    use super::{RingHeader, SlotDesc, offset_of};

    pub const MAGIC: usize = offset_of!(RingHeader, magic);
    pub const LAYOUT_VERSION: usize = offset_of!(RingHeader, layout_version);
    pub const REQUIRED_FEATURES: usize = offset_of!(RingHeader, required_features);
    pub const HEADER_SIZE: usize = offset_of!(RingHeader, header_size);
    pub const SLOT_DESC_SIZE: usize = offset_of!(RingHeader, slot_desc_size);
    pub const SLOT_COUNT: usize = offset_of!(RingHeader, slot_count);
    pub const PAYLOAD_OFFSET: usize = offset_of!(RingHeader, payload_offset);
    pub const SLOT_STRIDE: usize = offset_of!(RingHeader, slot_stride);
    pub const SLOT_CAPACITY: usize = offset_of!(RingHeader, slot_capacity);
    pub const MAPPING_SIZE: usize = offset_of!(RingHeader, mapping_size);
    pub const RING_EPOCH: usize = offset_of!(RingHeader, ring_epoch);
    pub const WRITER_PID: usize = offset_of!(RingHeader, writer_pid);
    pub const HOST_CLOCK: usize = offset_of!(RingHeader, host_clock);
    pub const WRITE_INDEX: usize = offset_of!(RingHeader, write_index);
    pub const WRITER_STATE: usize = offset_of!(RingHeader, writer_state);
    pub const HEARTBEAT_NS: usize = offset_of!(RingHeader, heartbeat_ns);
    pub const FRAMES_DROPPED: usize = offset_of!(RingHeader, frames_dropped);
    pub const CLOCK_LOCK: usize = offset_of!(RingHeader, clock_lock);
    pub const CLOCK_FLICKS: usize = offset_of!(RingHeader, clock_flicks);
    pub const CLOCK_HOST_NS: usize = offset_of!(RingHeader, clock_host_ns);
    pub const CLOCK_RATE_MILLI: usize = offset_of!(RingHeader, clock_rate_milli);
    pub const CLOCK_STATE: usize = offset_of!(RingHeader, clock_state);
    pub const CONFIG_LOCK: usize = offset_of!(RingHeader, config_lock);
    pub const CONFIG_GEN: usize = offset_of!(RingHeader, config_gen);
    pub const PREVIEW_WIDTH: usize = offset_of!(RingHeader, preview_width);
    pub const PREVIEW_HEIGHT: usize = offset_of!(RingHeader, preview_height);
    pub const PREVIEW_FORMAT: usize = offset_of!(RingHeader, preview_format);
    pub const SUCCESSOR_EPOCH: usize = offset_of!(RingHeader, successor_epoch);
    pub const SUCCESSOR_NAME: usize = offset_of!(RingHeader, successor_name);

    pub const SLOT_LOCK: usize = offset_of!(SlotDesc, lock);
    pub const SLOT_SEQ: usize = offset_of!(SlotDesc, seq);
    pub const SLOT_PTS: usize = offset_of!(SlotDesc, pts);
    pub const SLOT_DURATION: usize = offset_of!(SlotDesc, duration);
    pub const SLOT_REV: usize = offset_of!(SlotDesc, rev);
    pub const SLOT_WIDTH: usize = offset_of!(SlotDesc, width);
    pub const SLOT_HEIGHT: usize = offset_of!(SlotDesc, height);
    pub const SLOT_FORMAT: usize = offset_of!(SlotDesc, format);
    pub const SLOT_FLAGS: usize = offset_of!(SlotDesc, flags);
    pub const SLOT_CONFIG_GEN: usize = offset_of!(SlotDesc, config_gen);
    pub const SLOT_PAYLOAD_LEN: usize = offset_of!(SlotDesc, payload_len);
    pub const SLOT_PLANE0_OFFSET: usize = offset_of!(SlotDesc, plane0_offset);
    pub const SLOT_PLANE1_OFFSET: usize = offset_of!(SlotDesc, plane1_offset);
    pub const SLOT_PLANE0_STRIDE: usize = offset_of!(SlotDesc, plane0_stride);
    pub const SLOT_PLANE1_STRIDE: usize = offset_of!(SlotDesc, plane1_stride);
    pub const SLOT_RENDER_NS: usize = offset_of!(SlotDesc, render_ns);
    pub const SLOT_PUBLISH_NS: usize = offset_of!(SlotDesc, publish_ns);
}

/// Ring header at offset 0, in five 128-byte blocks so the hot words of different writers
/// (frame publisher, clock writer) never share a cache line (64 B on x86, 128 B on Apple).
///
/// Field semantics are in `RING.md`. Reserved bytes are zero (fresh shared memory is zero
/// filled) and are never read or written.
#[repr(C)]
pub struct RingHeader {
    // --- Block A (0..128): identity and geometry, written once before `magic`. ---
    /// [`MAGIC`]; stored last at initialisation with `Release`.
    pub magic: AtomicU32,
    /// [`LAYOUT_VERSION`].
    pub layout_version: AtomicU32,
    /// Feature bits a reader must understand; 0 in v1.
    pub required_features: AtomicU32,
    /// Size of this header and offset of the slot table ([`HEADER_SIZE`]).
    pub header_size: AtomicU32,
    /// Size of one slot descriptor ([`SLOT_DESC_SIZE`]).
    pub slot_desc_size: AtomicU32,
    /// Number of slots.
    pub slot_count: AtomicU32,
    /// Offset of payload 0 from the start of the mapping.
    pub payload_offset: AtomicU64,
    /// Distance between consecutive payloads.
    pub slot_stride: AtomicU64,
    /// Largest `payload_len` a slot can hold (≤ `slot_stride`).
    pub slot_capacity: AtomicU64,
    /// Total bytes of the shared-memory object the writer created.
    pub mapping_size: AtomicU64,
    /// Random per-ring nonce; the engine announces `{name, epoch}` in its handshake.
    pub ring_epoch: AtomicU64,
    /// Process id of the writer.
    pub writer_pid: AtomicU32,
    /// [`HostClock`] of every `*_ns` field.
    pub host_clock: AtomicU32,
    _reserved_a: [u64; 7],

    // --- Block B (128..256): writer progress and liveness. ---
    /// Frames committed so far (also the `seq` of the newest frame); the newest frame is in
    /// slot `(write_index - 1) % slot_count`. 0 = none yet. Stored with `Release` after the
    /// slot is complete.
    pub write_index: AtomicU64,
    /// [`WriterState`].
    pub writer_state: AtomicU32,
    _reserved_b0: u32,
    /// Host time of the writer's last publish or heartbeat.
    pub heartbeat_ns: AtomicU64,
    /// Frames the producer skipped (rendered too late, no readback buffer), for diagnostics.
    pub frames_dropped: AtomicU64,
    _reserved_b: [u64; 12],

    // --- Block C (256..384): audio clock, guarded by the `clock_lock` seqlock. ---
    /// Seqlock word: odd while a clock sample is being written.
    pub clock_lock: AtomicU64,
    /// Audio clock position (timeline flicks) at `clock_host_ns`.
    pub clock_flicks: AtomicI64,
    /// Host time at which `clock_flicks` was sampled.
    pub clock_host_ns: AtomicU64,
    /// Playback rate in thousandths (1000 = 1x, -2000 = 2x reverse, 0 = still).
    pub clock_rate_milli: AtomicI32,
    /// [`ClockState`].
    pub clock_state: AtomicU32,
    _reserved_c: [u64; 12],

    // --- Block D (384..512): negotiated preview configuration, `config_lock` seqlock. ---
    /// Seqlock word: odd while the configuration is being written.
    pub config_lock: AtomicU64,
    /// Configuration generation: bumped by every renegotiation (0 = none negotiated yet).
    pub config_gen: AtomicU32,
    /// Negotiated preview width in pixels (the canvas pixel size).
    pub preview_width: AtomicU32,
    /// Negotiated preview height in pixels.
    pub preview_height: AtomicU32,
    /// Negotiated [`PixelFormat`].
    pub preview_format: AtomicU32,
    _reserved_d: [u64; 13],

    // --- Block E (512..1024): successor ring, valid once `writer_state == Retired`. ---
    /// `ring_epoch` of the ring that replaced this one.
    pub successor_epoch: AtomicU64,
    /// Name of the ring that replaced this one: ASCII, NUL padded, packed little-endian.
    pub successor_name: [AtomicU64; NAME_FIELD_LEN / 8],
    _reserved_e: [u64; 59],
}

/// Per-slot descriptor, guarded by the slot's `lock` seqlock.
#[repr(C)]
pub struct SlotDesc {
    /// Seqlock word: odd while the writer fills the slot, even when stable.
    pub lock: AtomicU64,
    /// Frame sequence number (= `write_index` right after this frame was committed; strictly
    /// increasing across slots and across successor rings). 0 = slot empty.
    pub seq: AtomicU64,
    /// Presentation timestamp in timeline flicks.
    pub pts: AtomicI64,
    /// Frame duration in flicks (0 = unknown).
    pub duration: AtomicI64,
    /// Project revision the frame was rendered from.
    pub rev: AtomicU64,
    /// Frame width in pixels.
    pub width: AtomicU32,
    /// Frame height in pixels.
    pub height: AtomicU32,
    /// [`PixelFormat`].
    pub format: AtomicU32,
    /// Flag bits ([`FLAG_LOWRES`]).
    pub flags: AtomicU32,
    /// `config_gen` the frame was rendered for.
    pub config_gen: AtomicU32,
    _reserved0: u32,
    /// Bytes of the payload in use.
    pub payload_len: AtomicU64,
    /// Offset of plane 0 inside the payload.
    pub plane0_offset: AtomicU64,
    /// Offset of plane 1 inside the payload (NV12 interleaved UV; 0 for RGBA8).
    pub plane1_offset: AtomicU64,
    /// Bytes per row of plane 0.
    pub plane0_stride: AtomicU32,
    /// Bytes per row of plane 1 (0 for RGBA8).
    pub plane1_stride: AtomicU32,
    /// Host time the frame's GPU work was submitted (0 = unknown).
    pub render_ns: AtomicU64,
    /// Host time the slot was committed.
    pub publish_ns: AtomicU64,
    _reserved1: [u64; 2],
}

const _: () = assert!(size_of::<RingHeader>() == HEADER_SIZE);
const _: () = assert!(size_of::<SlotDesc>() == SLOT_DESC_SIZE);
const _: () = assert!(off::WRITE_INDEX == 128);
const _: () = assert!(off::CLOCK_LOCK == 256);
const _: () = assert!(off::CONFIG_LOCK == 384);
const _: () = assert!(off::SUCCESSOR_EPOCH == 512);
const _: () = assert!(off::SUCCESSOR_NAME == 520);
const _: () = assert!(SLOT_TABLE_OFFSET + SLOT_DESC_SIZE * MAX_SLOTS as usize <= PAYLOAD_OFFSET);
const _: () = assert!(PAYLOAD_OFFSET.is_multiple_of(PAYLOAD_ALIGN));

/// Lifecycle of the writer, as seen by readers (`RingHeader::writer_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WriterState {
    /// Header not complete yet (never visible once `magic` is set).
    Initializing = 0,
    /// Publishing; check `heartbeat_ns` for liveness.
    Live = 1,
    /// Replaced by a larger ring: open `successor_name` and verify `successor_epoch`.
    Retired = 2,
    /// The engine shut down cleanly; no more frames will come.
    Closed = 3,
}

impl WriterState {
    /// Decode the raw state word.
    #[must_use]
    pub const fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::Initializing),
            1 => Some(Self::Live),
            2 => Some(Self::Retired),
            3 => Some(Self::Closed),
            _ => None,
        }
    }
}

/// Transport state published with the audio clock (`RingHeader::clock_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum ClockState {
    /// Not advancing (stopped or paused). Readers use `clock_flicks` as is.
    #[default]
    Stopped = 0,
    /// Advancing at `clock_rate_milli`; readers extrapolate from `clock_host_ns`.
    Playing = 1,
    /// Scrubbing: `clock_flicks` is the scrub target; readers do not extrapolate.
    Scrubbing = 2,
}

impl ClockState {
    /// Decode the raw state word.
    #[must_use]
    pub const fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::Stopped),
            1 => Some(Self::Playing),
            2 => Some(Self::Scrubbing),
            _ => None,
        }
    }
}

/// The OS clock behind every `*_ns` field (`RingHeader::host_clock`). Readers must sample the
/// same clock to compare against `heartbeat_ns` or to extrapolate the audio clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum HostClock {
    /// Windows `QueryPerformanceCounter`, converted as `ticks * 10^9 / QueryPerformanceFrequency`
    /// (128-bit intermediate, truncating).
    WindowsQpc = 1,
    /// Linux `clock_gettime(CLOCK_MONOTONIC)`.
    LinuxMonotonic = 2,
    /// macOS `clock_gettime(CLOCK_UPTIME_RAW)` (= `mach_absolute_time` in ns).
    MacUptimeRaw = 3,
}

impl HostClock {
    /// Decode the raw clock word.
    #[must_use]
    pub const fn from_u32(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::WindowsQpc),
            2 => Some(Self::LinuxMonotonic),
            3 => Some(Self::MacUptimeRaw),
            _ => None,
        }
    }
}

/// Pixel format of a slot payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum PixelFormat {
    /// One plane, 4 bytes per pixel in R, G, B, A order; sRGB-encoded, full range, opaque.
    Rgba8 = 1,
    /// Plane 0: Y, 1 byte per pixel. Plane 1: interleaved U,V at half resolution in both
    /// directions (`height / 2` rows of `width` bytes). BT.709, limited range. Width and height
    /// are even.
    Nv12 = 2,
}

impl PixelFormat {
    /// Decode the raw format word.
    #[must_use]
    pub const fn from_u32(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::Rgba8),
            2 => Some(Self::Nv12),
            _ => None,
        }
    }

    /// Number of planes.
    #[must_use]
    pub const fn plane_count(self) -> usize {
        match self {
            Self::Rgba8 => 1,
            Self::Nv12 => 2,
        }
    }

    /// Minimum bytes per row of `plane` for a frame `width` pixels wide.
    #[must_use]
    pub const fn row_bytes(self, plane: usize, width: u32) -> u64 {
        match (self, plane) {
            (Self::Rgba8, 0) => width as u64 * 4,
            (Self::Nv12, 0 | 1) => width as u64,
            _ => 0,
        }
    }

    /// Number of rows of `plane` for a frame `height` pixels high.
    #[must_use]
    pub const fn rows(self, plane: usize, height: u32) -> u64 {
        match (self, plane) {
            (Self::Rgba8 | Self::Nv12, 0) => height as u64,
            (Self::Nv12, 1) => height as u64 / 2,
            _ => 0,
        }
    }

    /// True if frames of this format need even dimensions.
    #[must_use]
    pub const fn needs_even_size(self) -> bool {
        matches!(self, Self::Nv12)
    }
}

/// Where one plane lives inside a slot payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlaneLayout {
    /// Byte offset of the plane's first row inside the payload.
    pub offset: usize,
    /// Bytes per row (≥ the format's minimum row size).
    pub stride: u32,
}

/// Why a [`FrameLayout`] is invalid.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LayoutError {
    /// Width or height is 0 or above [`MAX_DIMENSION`].
    #[error("frame size {width}x{height} outside 1..={MAX_DIMENSION}")]
    Size {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// NV12 needs even width and height.
    #[error("NV12 frame size {width}x{height} must be even")]
    OddSize {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// A plane's stride is smaller than one row of pixels.
    #[error("plane {plane} stride {stride} is below the {min} bytes of one row")]
    Stride {
        /// Plane index.
        plane: usize,
        /// Given stride.
        stride: u32,
        /// Minimum stride.
        min: u64,
    },
    /// Two planes overlap.
    #[error("planes 0 and 1 overlap")]
    Overlap,
    /// Row alignment is 0 or not a power of two.
    #[error("row alignment {0} is not a power of two")]
    RowAlign(u32),
    /// The layout does not fit in addressable memory.
    #[error("frame layout overflows")]
    Overflow,
}

/// Geometry of one frame inside a slot: the format, the size and the plane offsets and
/// strides. The publisher copies a source buffer with the same layout in one `memcpy`, so a
/// GPU readback buffer (rows padded to 256 bytes by wgpu) needs no repacking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLayout {
    width: u32,
    height: u32,
    format: PixelFormat,
    planes: [PlaneLayout; 2],
    len: usize,
}

impl FrameLayout {
    /// Tightly packed layout (no row padding); NV12's UV plane follows the Y plane directly.
    pub fn packed(width: u32, height: u32, format: PixelFormat) -> Result<Self, LayoutError> {
        Self::aligned(width, height, format, 1)
    }

    /// Layout whose strides and plane offsets are rounded up to `row_align` bytes (a power of
    /// two): 256 matches wgpu's `COPY_BYTES_PER_ROW_ALIGNMENT` for texture-to-buffer copies.
    pub fn aligned(
        width: u32,
        height: u32,
        format: PixelFormat,
        row_align: u32,
    ) -> Result<Self, LayoutError> {
        if !row_align.is_power_of_two() {
            return Err(LayoutError::RowAlign(row_align));
        }
        check_size(width, height, format)?;
        let align = u64::from(row_align);
        let stride0 = format.row_bytes(0, width).next_multiple_of(align);
        let stride0 = u32::try_from(stride0).map_err(|_| LayoutError::Overflow)?;
        let mut planes = [
            PlaneLayout {
                offset: 0,
                stride: stride0,
            },
            PlaneLayout::default(),
        ];
        if format.plane_count() == 2 {
            let plane0_len = u64::from(stride0) * format.rows(0, height);
            let stride1 = format.row_bytes(1, width).next_multiple_of(align);
            planes[1] = PlaneLayout {
                offset: usize::try_from(plane0_len.next_multiple_of(align))
                    .map_err(|_| LayoutError::Overflow)?,
                stride: u32::try_from(stride1).map_err(|_| LayoutError::Overflow)?,
            };
        }
        Self::with_planes(width, height, format, planes)
    }

    /// Layout with explicit plane offsets and strides (validated). Plane 1 is ignored for
    /// single-plane formats.
    pub fn with_planes(
        width: u32,
        height: u32,
        format: PixelFormat,
        mut planes: [PlaneLayout; 2],
    ) -> Result<Self, LayoutError> {
        check_size(width, height, format)?;
        if format.plane_count() == 1 {
            planes[1] = PlaneLayout::default();
        }
        let mut len = 0u64;
        let mut ranges = [(0u64, 0u64); 2];
        for (i, p) in planes.iter().take(format.plane_count()).enumerate() {
            let min = format.row_bytes(i, width);
            if u64::from(p.stride) < min {
                return Err(LayoutError::Stride {
                    plane: i,
                    stride: p.stride,
                    min,
                });
            }
            let start = u64::try_from(p.offset).map_err(|_| LayoutError::Overflow)?;
            let end = u64::from(p.stride)
                .checked_mul(format.rows(i, height))
                .and_then(|n| n.checked_add(start))
                .ok_or(LayoutError::Overflow)?;
            ranges[i] = (start, end);
            len = len.max(end);
        }
        if format.plane_count() == 2 {
            let [(s0, e0), (s1, e1)] = ranges;
            if s0 < e1 && s1 < e0 {
                return Err(LayoutError::Overlap);
            }
        }
        Ok(Self {
            width,
            height,
            format,
            planes,
            len: usize::try_from(len).map_err(|_| LayoutError::Overflow)?,
        })
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Pixel format.
    #[must_use]
    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    /// Plane 0 and plane 1 (plane 1 is all zero for single-plane formats).
    #[must_use]
    pub const fn planes(&self) -> [PlaneLayout; 2] {
        self.planes
    }

    /// Bytes the frame occupies (end of the last plane).
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Always false: a valid layout has at least one pixel.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

fn check_size(width: u32, height: u32, format: PixelFormat) -> Result<(), LayoutError> {
    if !(1..=MAX_DIMENSION).contains(&width) || !(1..=MAX_DIMENSION).contains(&height) {
        return Err(LayoutError::Size { width, height });
    }
    if format.needs_even_size() && (!width.is_multiple_of(2) || !height.is_multiple_of(2)) {
        return Err(LayoutError::OddSize { width, height });
    }
    Ok(())
}

/// Round `v` up to a multiple of `align` (a power of two), or `None` on overflow.
#[must_use]
pub const fn align_up(v: usize, align: usize) -> Option<usize> {
    match v.checked_add(align - 1) {
        Some(x) => Some(x & !(align - 1)),
        None => None,
    }
}

/// `slot_stride` for slots of `slot_capacity` bytes.
#[must_use]
pub const fn slot_stride(slot_capacity: usize) -> Option<usize> {
    align_up(slot_capacity, PAYLOAD_ALIGN)
}

/// Total mapping size for `slot_count` slots of `slot_capacity` bytes.
#[must_use]
pub fn mapping_size(slot_count: u32, slot_capacity: usize) -> Option<usize> {
    slot_stride(slot_capacity)?
        .checked_mul(usize::try_from(slot_count).ok()?)?
        .checked_add(PAYLOAD_OFFSET)
}

/// Pack a ring name into the `successor_name` words (ASCII, NUL padded, little-endian).
#[must_use]
pub fn pack_name(name: &str) -> [u64; NAME_FIELD_LEN / 8] {
    let mut bytes = [0u8; NAME_FIELD_LEN];
    let n = name.len().min(NAME_FIELD_LEN - 1);
    bytes[..n].copy_from_slice(&name.as_bytes()[..n]);
    let mut words = [0u64; NAME_FIELD_LEN / 8];
    let (chunks, _) = bytes.as_chunks::<8>();
    for (w, chunk) in words.iter_mut().zip(chunks) {
        *w = u64::from_le_bytes(*chunk);
    }
    words
}

/// Inverse of [`pack_name`]; `None` if the bytes are not NUL-terminated printable ASCII.
#[must_use]
pub fn unpack_name(words: &[u64; NAME_FIELD_LEN / 8]) -> Option<String> {
    let mut bytes = Vec::with_capacity(NAME_FIELD_LEN);
    for w in words {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    let end = bytes.iter().position(|&b| b == 0)?;
    let name = &bytes[..end];
    (!name.is_empty() && name.iter().all(u8::is_ascii_graphic))
        .then(|| String::from_utf8_lossy(name).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_spells_cwpr() {
        assert_eq!(MAGIC.to_le_bytes(), *b"CWPR");
        assert_eq!(MAGIC, 0x5250_5743);
    }

    #[test]
    fn packed_layouts() {
        let rgba = FrameLayout::packed(1920, 1080, PixelFormat::Rgba8).unwrap();
        assert_eq!(rgba.len(), 1920 * 1080 * 4);
        assert_eq!(
            rgba.planes()[0],
            PlaneLayout {
                offset: 0,
                stride: 7680
            }
        );
        assert_eq!(rgba.planes()[1], PlaneLayout::default());

        let nv12 = FrameLayout::packed(1920, 1080, PixelFormat::Nv12).unwrap();
        assert_eq!(nv12.len(), 1920 * 1080 * 3 / 2);
        assert_eq!(
            nv12.planes()[1],
            PlaneLayout {
                offset: 1920 * 1080,
                stride: 1920
            }
        );
    }

    #[test]
    fn gpu_aligned_layouts_pad_rows_and_planes() {
        // 1366 px: RGBA rows of 5464 B pad to 5632; NV12 rows of 1366 B pad to 1536.
        let rgba = FrameLayout::aligned(1366, 768, PixelFormat::Rgba8, 256).unwrap();
        assert_eq!(rgba.planes()[0].stride, 5632);
        assert_eq!(rgba.len(), 5632 * 768);
        let nv12 = FrameLayout::aligned(1366, 768, PixelFormat::Nv12, 256).unwrap();
        assert_eq!(nv12.planes()[0].stride, 1536);
        assert_eq!(nv12.planes()[1].offset, 1536 * 768);
        assert_eq!(nv12.len(), 1536 * 768 + 1536 * 384);
        assert_eq!(
            FrameLayout::aligned(16, 16, PixelFormat::Rgba8, 3),
            Err(LayoutError::RowAlign(3))
        );
    }

    #[test]
    fn invalid_layouts_are_rejected() {
        assert!(matches!(
            FrameLayout::packed(0, 10, PixelFormat::Rgba8),
            Err(LayoutError::Size { .. })
        ));
        assert!(matches!(
            FrameLayout::packed(MAX_DIMENSION + 1, 10, PixelFormat::Rgba8),
            Err(LayoutError::Size { .. })
        ));
        assert!(matches!(
            FrameLayout::packed(15, 10, PixelFormat::Nv12),
            Err(LayoutError::OddSize { .. })
        ));
        let tight = PlaneLayout {
            offset: 0,
            stride: 63,
        };
        assert!(matches!(
            FrameLayout::with_planes(16, 4, PixelFormat::Rgba8, [tight, PlaneLayout::default()]),
            Err(LayoutError::Stride { plane: 0, .. })
        ));
        let y = PlaneLayout {
            offset: 0,
            stride: 16,
        };
        let uv = PlaneLayout {
            offset: 32,
            stride: 16,
        };
        assert_eq!(
            FrameLayout::with_planes(16, 4, PixelFormat::Nv12, [y, uv]),
            Err(LayoutError::Overlap)
        );
        // UV first, then Y, is a valid custom layout.
        let uv_first = PlaneLayout {
            offset: 0,
            stride: 16,
        };
        let y_after = PlaneLayout {
            offset: 32,
            stride: 16,
        };
        let l = FrameLayout::with_planes(16, 4, PixelFormat::Nv12, [y_after, uv_first]).unwrap();
        assert_eq!(l.len(), 96);
    }

    #[test]
    fn mapping_geometry() {
        assert_eq!(slot_stride(1), Some(PAYLOAD_ALIGN));
        assert_eq!(slot_stride(PAYLOAD_ALIGN), Some(PAYLOAD_ALIGN));
        assert_eq!(
            mapping_size(4, 8_294_400),
            Some(PAYLOAD_OFFSET + 4 * 8_306_688)
        );
        assert_eq!(mapping_size(4, usize::MAX), None);
    }

    #[test]
    fn names_round_trip_through_the_header_field() {
        let words = pack_name("cwp-0000abcd-0123456789abcdef");
        assert_eq!(
            unpack_name(&words).as_deref(),
            Some("cwp-0000abcd-0123456789abcdef")
        );
        assert_eq!(unpack_name(&[0; 4]), None);
        assert_eq!(unpack_name(&[u64::MAX; 4]), None);
    }

    #[test]
    fn enums_decode() {
        for s in [
            WriterState::Initializing,
            WriterState::Live,
            WriterState::Retired,
            WriterState::Closed,
        ] {
            assert_eq!(WriterState::from_u32(s as u32), Some(s));
        }
        for s in [
            ClockState::Stopped,
            ClockState::Playing,
            ClockState::Scrubbing,
        ] {
            assert_eq!(ClockState::from_u32(s as u32), Some(s));
        }
        for f in [PixelFormat::Rgba8, PixelFormat::Nv12] {
            assert_eq!(PixelFormat::from_u32(f as u32), Some(f));
        }
        for c in [
            HostClock::WindowsQpc,
            HostClock::LinuxMonotonic,
            HostClock::MacUptimeRaw,
        ] {
            assert_eq!(HostClock::from_u32(c as u32), Some(c));
        }
        assert_eq!(PixelFormat::from_u32(0), None);
        assert_eq!(WriterState::from_u32(9), None);
    }
}
