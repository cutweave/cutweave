// SPDX-License-Identifier: GPL-3.0-or-later
//! [`RingReader`]: one read-only mapping of a `RING_LAYOUT_V1` ring, implementing the reader
//! protocol of `crates/cutweave-playback/RING.md` ("Reader protocol") to the letter.
//!
//! Everything here is plain Rust (no Node-API), so it is tested directly against the engine's
//! `FramePublisher`. The reader never writes to the mapping.

use std::sync::atomic::{AtomicU64, Ordering, fence};

use cutweave_playback::shm::layout::{
    ClockState, FLICKS_PER_SECOND, FrameLayout, HEARTBEAT_TIMEOUT_NS, KNOWN_FLAGS,
    KNOWN_REQUIRED_FEATURES, LAYOUT_VERSION, MAGIC, PAYLOAD_ALIGN, PixelFormat, PlaneLayout,
    RATE_MILLI_1X, RingHeader, SLOT_DESC_SIZE, SlotDesc, WriterState, unpack_name,
};
use cutweave_playback::shm::{HOST_CLOCK, MAX_CLOCK_EXTRAPOLATION_NS, RingName, host_clock_ns};

use crate::error::BridgeError;
use crate::os::Mapping;

/// Seqlock attempts for one slot copy before the read is reported busy.
const COPY_RETRIES: u32 = 4;
/// Spins while waiting for a clock or configuration seqlock to become even and stable.
const SEQLOCK_SPINS: u32 = 1000;

/// A consistent snapshot of one slot descriptor, validated against the layout rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotMeta {
    /// Slot index.
    pub slot: u32,
    /// Seqlock value the snapshot was taken under (re-checked after the payload copy).
    pub lock: u64,
    /// Frame sequence number (> 0).
    pub seq: u64,
    /// Presentation timestamp (flicks).
    pub pts: i64,
    /// Frame duration (flicks), 0 = unknown.
    pub duration: i64,
    /// Project revision the frame was rendered from.
    pub rev: u64,
    /// Frame flags (`FLAG_LOWRES`).
    pub flags: u32,
    /// Configuration generation the frame was rendered for.
    pub config_gen: u32,
    /// Geometry of the payload.
    pub layout: FrameLayout,
    /// Host time the GPU work was submitted (0 = unknown).
    pub render_ns: u64,
    /// Host time the slot was committed.
    pub publish_ns: u64,
}

/// A consistent snapshot of the negotiated configuration (block D).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigSnapshot {
    /// Generation, bumped by every renegotiation.
    pub config_gen: u32,
    /// Preview width in device pixels.
    pub width: u32,
    /// Preview height.
    pub height: u32,
    /// Raw pixel format word (1 RGBA8, 2 NV12).
    pub format: u32,
}

/// A consistent sample of the audio clock (block C).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockReading {
    /// Timeline position at `host_ns` (flicks).
    pub flicks: i64,
    /// Host time of the sample; 0 if the engine never wrote one.
    pub host_ns: u64,
    /// Rate in thousandths (1000 = 1x, negative = reverse).
    pub rate_milli: i32,
    /// Transport state.
    pub state: ClockState,
}

impl ClockReading {
    /// Clock position at host time `now_ns`: the sample itself unless playing, else advanced by
    /// `(now - host_ns) * rate` with the elapsed time capped at `MAX_CLOCK_EXTRAPOLATION_NS`
    /// (`RING.md`, "Reader protocol").
    #[must_use]
    pub fn extrapolate(&self, now_ns: u64) -> i64 {
        if self.state != ClockState::Playing {
            return self.flicks;
        }
        let elapsed = now_ns
            .saturating_sub(self.host_ns)
            .min(MAX_CLOCK_EXTRAPOLATION_NS);
        let delta =
            i128::from(elapsed) * i128::from(FLICKS_PER_SECOND) * i128::from(self.rate_milli)
                / (1_000_000_000 * i128::from(RATE_MILLI_1X));
        i64::try_from(i128::from(self.flicks) + delta).unwrap_or(self.flicks)
    }
}

/// Outcome of a seqlock-guarded payload copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Copied {
    /// The payload is consistent with the descriptor it was chosen by.
    Valid,
    /// The writer reused the slot during the copy; the bytes in the destination are garbage.
    Torn,
}

/// Immutable ring geometry, read once at open (`RING.md`, "Reader protocol" steps 1-5).
#[derive(Debug, Clone, Copy)]
struct Geometry {
    slot_count: u32,
    header_size: usize,
    payload_offset: usize,
    slot_stride: usize,
    slot_capacity: usize,
    epoch: u64,
    writer_pid: u32,
}

/// A read-only, validated mapping of one ring. `Send + Sync`: every method takes `&self`, so
/// a wait task on a worker thread can share it with the JS thread.
pub struct RingReader {
    map: Mapping,
    name: RingName,
    geom: Geometry,
    /// Slot copies discarded because the seqlock moved (the torn reads the protocol caught).
    torn_retries: AtomicU64,
}

impl RingReader {
    /// Open and validate the ring `name`. `expected_epoch` is the epoch the engine announced
    /// (run-file, handshake or the old ring's `successor_epoch`); a ring with another epoch is
    /// rejected, which catches a stale name that the OS reused and a foreign object.
    #[allow(clippy::cast_ptr_alignment)] // Page-aligned mapping base.
    pub fn open(name: &str, expected_epoch: u64) -> Result<Self, BridgeError> {
        let name = RingName::parse(name).map_err(|e| BridgeError::Invalid(e.to_string()))?;
        let map = Mapping::open(&name)?;
        if map.len() < PAYLOAD_ALIGN {
            return Err(BridgeError::Invalid(format!(
                "mapping of {} bytes is too small",
                map.len()
            )));
        }
        // SAFETY: the mapping holds at least a header (checked above), is page aligned and
        // outlives the reference (it is owned by the reader the reference is derived from);
        // every header field is an atomic.
        let h = unsafe { &*map.as_ptr().cast::<RingHeader>() };
        if h.magic.load(Ordering::Acquire) != MAGIC {
            return Err(BridgeError::Invalid(
                "bad magic (not a ring, or not initialised yet)".into(),
            ));
        }
        let version = h.layout_version.load(Ordering::Relaxed);
        if version != LAYOUT_VERSION {
            return Err(BridgeError::Version(format!(
                "layout version {version}, this reader speaks {LAYOUT_VERSION}"
            )));
        }
        let features = h.required_features.load(Ordering::Relaxed);
        if features & !KNOWN_REQUIRED_FEATURES != 0 {
            return Err(BridgeError::Version(format!(
                "unknown required features {features:#x}"
            )));
        }
        let to_usize = |v: u64| {
            usize::try_from(v).map_err(|_| BridgeError::Invalid("field overflows usize".into()))
        };
        let header_size = h.header_size.load(Ordering::Relaxed) as usize;
        let desc_size = h.slot_desc_size.load(Ordering::Relaxed) as usize;
        let slot_count = h.slot_count.load(Ordering::Relaxed);
        let payload_offset = to_usize(h.payload_offset.load(Ordering::Relaxed))?;
        let slot_stride = to_usize(h.slot_stride.load(Ordering::Relaxed))?;
        let slot_capacity = to_usize(h.slot_capacity.load(Ordering::Relaxed))?;
        let mapping_size = to_usize(h.mapping_size.load(Ordering::Relaxed))?;
        let table_end = (slot_count as usize)
            .checked_mul(desc_size)
            .and_then(|n| n.checked_add(header_size));
        let payload_end = slot_stride
            .checked_mul(slot_count as usize)
            .and_then(|n| n.checked_add(payload_offset));
        let ok = desc_size == SLOT_DESC_SIZE
            && header_size >= std::mem::size_of::<RingHeader>()
            && header_size.is_multiple_of(8)
            && slot_count >= 1
            && table_end.is_some_and(|end| end <= payload_offset)
            && payload_offset.is_multiple_of(4096)
            && slot_capacity <= slot_stride
            && payload_end == Some(mapping_size)
            && mapping_size <= map.len();
        if !ok {
            return Err(BridgeError::Invalid("inconsistent ring geometry".into()));
        }
        let epoch = h.ring_epoch.load(Ordering::Relaxed);
        if epoch != expected_epoch {
            return Err(BridgeError::EpochMismatch {
                expected: expected_epoch,
                found: epoch,
            });
        }
        let host_clock = h.host_clock.load(Ordering::Relaxed);
        if host_clock != HOST_CLOCK as u32 {
            return Err(BridgeError::HostClock {
                expected: HOST_CLOCK as u32,
                found: host_clock,
            });
        }
        Ok(Self {
            geom: Geometry {
                slot_count,
                header_size,
                payload_offset,
                slot_stride,
                slot_capacity,
                epoch,
                writer_pid: h.writer_pid.load(Ordering::Relaxed),
            },
            map,
            name,
            torn_retries: AtomicU64::new(0),
        })
    }

    /// The ring's name.
    #[must_use]
    pub fn name(&self) -> &RingName {
        &self.name
    }

    /// The ring's epoch (equal to the announced one).
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.geom.epoch
    }

    /// Number of slots.
    #[must_use]
    pub fn slot_count(&self) -> u32 {
        self.geom.slot_count
    }

    /// Largest frame payload the ring holds, in bytes.
    #[must_use]
    pub fn slot_capacity(&self) -> usize {
        self.geom.slot_capacity
    }

    /// Engine process id.
    #[must_use]
    pub fn writer_pid(&self) -> u32 {
        self.geom.writer_pid
    }

    /// Bytes mapped.
    #[must_use]
    pub fn mapping_size(&self) -> usize {
        self.map.len()
    }

    /// Slot copies discarded because the writer reused the slot meanwhile.
    #[must_use]
    pub fn torn_retries(&self) -> u64 {
        self.torn_retries.load(Ordering::Relaxed)
    }

    #[allow(clippy::cast_ptr_alignment)] // Page-aligned mapping base.
    fn header(&self) -> &RingHeader {
        // SAFETY: validated in `open`; the mapping lives as long as `self`.
        unsafe { &*self.map.as_ptr().cast::<RingHeader>() }
    }

    #[allow(clippy::cast_ptr_alignment)] // Page-aligned base + multiple of 8 (validated).
    fn desc(&self, slot: u32) -> &SlotDesc {
        debug_assert!(slot < self.geom.slot_count);
        let off = self.geom.header_size + slot as usize * SLOT_DESC_SIZE;
        // SAFETY: the slot table lies inside the mapping (validated in `open`).
        unsafe { &*self.map.as_ptr().add(off).cast::<SlotDesc>() }
    }

    /// Frames committed so far (`seq` of the newest frame), Acquire.
    #[must_use]
    pub fn write_index(&self) -> u64 {
        self.header().write_index.load(Ordering::Acquire)
    }

    /// The writer's lifecycle state; `None` for a value this reader does not know.
    #[must_use]
    pub fn writer_state(&self) -> Option<WriterState> {
        WriterState::from_u32(self.header().writer_state.load(Ordering::Acquire))
    }

    /// Host time of the last publish or heartbeat.
    #[must_use]
    pub fn heartbeat_ns(&self) -> u64 {
        self.header().heartbeat_ns.load(Ordering::Acquire)
    }

    /// Nanoseconds since the last heartbeat, as of `now_ns`.
    #[must_use]
    pub fn heartbeat_age_ns(&self, now_ns: u64) -> u64 {
        now_ns.saturating_sub(self.heartbeat_ns())
    }

    /// True if the writer is `Live` but has not published or beaten its heart for more than
    /// `HEARTBEAT_TIMEOUT_NS` (500 ms): hung or dead.
    #[must_use]
    pub fn heartbeat_stale(&self, now_ns: u64) -> bool {
        self.writer_state() == Some(WriterState::Live)
            && self.heartbeat_age_ns(now_ns) > HEARTBEAT_TIMEOUT_NS
    }

    /// Frames the producer skipped (diagnostics).
    #[must_use]
    pub fn frames_dropped(&self) -> u64 {
        self.header().frames_dropped.load(Ordering::Relaxed)
    }

    /// The audio clock under its seqlock; `None` if the engine never wrote a sample (null
    /// clock) or the lock did not settle.
    #[must_use]
    pub fn clock(&self) -> Option<ClockReading> {
        let h = self.header();
        for _ in 0..SEQLOCK_SPINS {
            let l1 = h.clock_lock.load(Ordering::Acquire);
            if l1 & 1 == 0 {
                let flicks = h.clock_flicks.load(Ordering::Relaxed);
                let host_ns = h.clock_host_ns.load(Ordering::Relaxed);
                let rate_milli = h.clock_rate_milli.load(Ordering::Relaxed);
                let state = h.clock_state.load(Ordering::Relaxed);
                fence(Ordering::Acquire);
                if h.clock_lock.load(Ordering::Relaxed) == l1 {
                    return Some(ClockReading {
                        flicks,
                        host_ns,
                        rate_milli,
                        state: ClockState::from_u32(state).unwrap_or_default(),
                    });
                }
            }
            std::hint::spin_loop();
        }
        None
    }

    /// The audio clock extrapolated to `now_ns`, in flicks. `None` before the first sample.
    #[must_use]
    pub fn clock_at(&self, now_ns: u64) -> Option<i64> {
        self.clock()
            .filter(|c| c.host_ns != 0)
            .map(|c| c.extrapolate(now_ns))
    }

    /// The negotiated configuration under its seqlock.
    #[must_use]
    pub fn config(&self) -> Option<ConfigSnapshot> {
        let h = self.header();
        for _ in 0..SEQLOCK_SPINS {
            let l1 = h.config_lock.load(Ordering::Acquire);
            if l1 & 1 == 0 {
                let c = ConfigSnapshot {
                    config_gen: h.config_gen.load(Ordering::Relaxed),
                    width: h.preview_width.load(Ordering::Relaxed),
                    height: h.preview_height.load(Ordering::Relaxed),
                    format: h.preview_format.load(Ordering::Relaxed),
                };
                fence(Ordering::Acquire);
                if h.config_lock.load(Ordering::Relaxed) == l1 {
                    return Some(c);
                }
            }
            std::hint::spin_loop();
        }
        None
    }

    /// The successor ring `(name, epoch)` once this one is retired; `None` while it is not, or
    /// if the name field is malformed.
    #[must_use]
    pub fn successor(&self) -> Option<(RingName, u64)> {
        if self.writer_state() != Some(WriterState::Retired) {
            return None;
        }
        let h = self.header();
        let words = std::array::from_fn(|i| h.successor_name[i].load(Ordering::Relaxed));
        let name = RingName::parse(&unpack_name(&words)?).ok()?;
        Some((name, h.successor_epoch.load(Ordering::Relaxed)))
    }

    /// Validate a stable descriptor against the layout rules.
    fn validate(
        &self,
        d: &SlotDesc,
        slot: u32,
        lock: u64,
    ) -> Result<Option<SlotMeta>, BridgeError> {
        let relaxed = Ordering::Relaxed;
        let seq = d.seq.load(relaxed);
        if seq == 0 {
            return Ok(None);
        }
        let corrupt = |what: &str| BridgeError::Corrupt(format!("slot {slot} seq {seq}: {what}"));
        let format =
            PixelFormat::from_u32(d.format.load(relaxed)).ok_or_else(|| corrupt("format"))?;
        let offset = |v: u64| usize::try_from(v).map_err(|_| corrupt("offset"));
        let planes = [
            PlaneLayout {
                offset: offset(d.plane0_offset.load(relaxed))?,
                stride: d.plane0_stride.load(relaxed),
            },
            PlaneLayout {
                offset: offset(d.plane1_offset.load(relaxed))?,
                stride: d.plane1_stride.load(relaxed),
            },
        ];
        let layout = FrameLayout::with_planes(
            d.width.load(relaxed),
            d.height.load(relaxed),
            format,
            planes,
        )
        .map_err(|e| corrupt(&e.to_string()))?;
        let payload_len = offset(d.payload_len.load(relaxed))?;
        if payload_len != layout.len() || payload_len > self.geom.slot_capacity {
            return Err(corrupt("payload_len"));
        }
        // Unknown flag bits are ignorable additions (reserved bits are zero in v1).
        let flags = d.flags.load(relaxed) & KNOWN_FLAGS;
        Ok(Some(Self::meta(d, slot, lock, seq, layout, flags)))
    }

    fn meta(
        d: &SlotDesc,
        slot: u32,
        lock: u64,
        seq: u64,
        layout: FrameLayout,
        flags: u32,
    ) -> SlotMeta {
        let relaxed = Ordering::Relaxed;
        SlotMeta {
            slot,
            lock,
            seq,
            pts: d.pts.load(relaxed),
            duration: d.duration.load(relaxed),
            rev: d.rev.load(relaxed),
            flags,
            config_gen: d.config_gen.load(relaxed),
            layout,
            render_ns: d.render_ns.load(relaxed),
            publish_ns: d.publish_ns.load(relaxed),
        }
    }

    /// One attempt at a consistent descriptor of `slot`: `Ok(None)` for an empty slot or one
    /// the writer is filling right now.
    fn try_meta(&self, slot: u32) -> Result<Option<SlotMeta>, BridgeError> {
        let d = self.desc(slot);
        let lock = d.lock.load(Ordering::Acquire);
        if lock & 1 == 1 {
            return Ok(None);
        }
        let meta = self.validate(d, slot, lock);
        fence(Ordering::Acquire);
        if d.lock.load(Ordering::Relaxed) != lock {
            self.torn_retries.fetch_add(1, Ordering::Relaxed);
            return Ok(None);
        }
        meta
    }

    /// Descriptors of every consistent, non-empty slot, in slot order. Corrupt descriptors
    /// are reported only if no slot is usable.
    pub fn snapshot(&self) -> Result<Vec<SlotMeta>, BridgeError> {
        let mut out = Vec::with_capacity(self.geom.slot_count as usize);
        let mut first_error = None;
        for slot in 0..self.geom.slot_count {
            match self.try_meta(slot) {
                Ok(Some(m)) => out.push(m),
                Ok(None) => {}
                Err(e) => first_error = first_error.or(Some(e)),
            }
        }
        match first_error {
            Some(e) if out.is_empty() => Err(e),
            _ => Ok(out),
        }
    }

    /// Choose the frame to present: the highest `seq` above `newer_than` among the slots,
    /// optionally restricted to `pts <= max_pts` (clock-scheduled choice, arch §10 step 3).
    /// Latest wins; nothing is copied yet.
    pub fn select(
        &self,
        newer_than: u64,
        max_pts: Option<i64>,
    ) -> Result<Option<SlotMeta>, BridgeError> {
        // Cheap exit for the common idle case of the unrestricted read.
        if max_pts.is_none() && self.write_index() <= newer_than {
            return Ok(None);
        }
        Ok(self
            .snapshot()?
            .into_iter()
            .filter(|m| m.seq > newer_than && max_pts.is_none_or(|p| m.pts <= p))
            .max_by_key(|m| m.seq))
    }

    /// Copy the payload of the frame `meta` describes into `dst` (at least `meta.layout.len()`
    /// bytes) and check the slot's seqlock afterwards. `Torn` means the writer lapped this
    /// reader during the copy and `dst` must be discarded (`RING.md`: with N slots a reader has
    /// about N - 1 frame periods to finish a copy).
    ///
    /// # Panics
    ///
    /// If `dst` is shorter than the payload: callers size it from `meta`.
    pub fn copy_payload(&self, meta: &SlotMeta, dst: &mut [u8]) -> Copied {
        let len = meta.layout.len();
        assert!(dst.len() >= len, "destination shorter than the payload");
        let src = self.geom.payload_offset + meta.slot as usize * self.geom.slot_stride;
        // SAFETY: `src + len` lies inside this reader's own read-only mapping (`len` <=
        // `slot_capacity` <= `slot_stride`, validated in `open` and `validate`). The writer may
        // be changing these bytes through its own mapping right now: that is exactly what the
        // seqlock re-check below detects, and the copy is discarded unless the lock word is
        // unchanged. The destination is a distinct allocation of at least `len` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(self.map.as_ptr().add(src), dst.as_mut_ptr(), len);
        }
        fence(Ordering::Acquire);
        if self.desc(meta.slot).lock.load(Ordering::Relaxed) == meta.lock {
            Copied::Valid
        } else {
            self.torn_retries.fetch_add(1, Ordering::Relaxed);
            Copied::Torn
        }
    }

    /// Select (see [`select`](Self::select)) and copy a frame into a buffer obtained from
    /// `buffer(len)`, retrying a few times if the writer laps the copy. `Ok(None)`: nothing
    /// newer. A frame is only returned if it is newer than `newer_than`, even when the writer
    /// lapped the reader during the copy.
    pub fn read(
        &self,
        newer_than: u64,
        max_pts: Option<i64>,
        buffer: &mut impl FrameBuffer,
    ) -> Result<Option<SlotMeta>, BridgeError> {
        for _ in 0..COPY_RETRIES {
            let Some(meta) = self.select(newer_than, max_pts)? else {
                return Ok(None);
            };
            let dst = buffer.buffer(meta.layout.len())?;
            if self.copy_payload(&meta, dst) == Copied::Valid {
                return Ok(Some(meta));
            }
        }
        // The writer kept lapping us: report "nothing yet"; the next call tries again.
        Ok(None)
    }
}

/// Where a frame is copied to. The addon implements it over pooled JS `ArrayBuffer`s; tests
/// over a `Vec`.
pub trait FrameBuffer {
    /// A writable buffer of at least `len` bytes (the frame payload size). May be called again
    /// with a different `len` if the first attempt was torn; only the last buffer matters.
    fn buffer(&mut self, len: usize) -> Result<&mut [u8], BridgeError>;
}

impl FrameBuffer for Vec<u8> {
    fn buffer(&mut self, len: usize) -> Result<&mut [u8], BridgeError> {
        if self.len() < len {
            self.resize(len, 0);
        }
        Ok(&mut self[..len])
    }
}

/// Current host time on the clock the ring's `*_ns` fields use.
#[must_use]
pub fn now_ns() -> u64 {
    host_clock_ns()
}
