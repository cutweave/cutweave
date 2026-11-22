// SPDX-License-Identifier: GPL-3.0-or-later
//! Reference reader, compiled for tests only. It follows `RING.md` ("Reader protocol") to
//! the letter, through its own read-only mapping, exactly as the napi addon (WP-1H.03) will;
//! the concurrency tests run it against the real publisher.

use std::cell::Cell;
use std::sync::atomic::{Ordering, fence};

use crate::shm::clock::ClockSample;
use crate::shm::layout::{
    FrameLayout, KNOWN_REQUIRED_FEATURES, LAYOUT_VERSION, MAGIC, PAYLOAD_ALIGN, PixelFormat,
    PlaneLayout, RingHeader, SLOT_DESC_SIZE, SlotDesc, WriterState, unpack_name,
};
use crate::shm::name::RingName;
use crate::shm::os::SharedMemory;

/// Copy attempts per slot before reporting it busy.
const READ_RETRIES: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReadError {
    /// No frame published yet.
    Empty,
    /// The slot was being written for every attempt.
    Busy,
    /// Destination too small.
    BufferTooSmall,
    /// A stable descriptor that violates the layout rules (must never happen).
    Corrupt(String),
}

/// A consistent snapshot of one slot descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlotMeta {
    pub slot: u32,
    pub lock: u64,
    pub seq: u64,
    pub pts: i64,
    pub duration: i64,
    pub rev: u64,
    pub flags: u32,
    pub config_gen: u32,
    pub layout: FrameLayout,
    pub render_ns: u64,
    pub publish_ns: u64,
}

/// A consistent snapshot of the negotiated configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConfigSnapshot {
    pub config_gen: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
}

pub(crate) struct TestReader {
    shm: SharedMemory,
    epoch: u64,
    slot_count: u32,
    slot_capacity: usize,
    slot_stride: usize,
    payload_offset: usize,
    /// Copies discarded because the seqlock moved (the torn reads the protocol caught).
    pub torn_retries: Cell<u64>,
}

impl TestReader {
    /// Open and validate a ring. `expected_epoch` is the announced epoch, if known.
    #[allow(clippy::cast_ptr_alignment)] // Page-aligned mapping base.
    pub(crate) fn open(name: &RingName, expected_epoch: Option<u64>) -> Result<Self, String> {
        let shm = SharedMemory::open_read_only(name).map_err(|e| e.to_string())?;
        if shm.len() < PAYLOAD_ALIGN {
            return Err(format!("mapping of {} bytes is too small", shm.len()));
        }
        // SAFETY: test reader; the mapping holds at least a header (checked above), is page
        // aligned and outlives the reference; the header is all atomics.
        let h = unsafe { &*shm.as_ptr().cast::<RingHeader>() };
        if h.magic.load(Ordering::Acquire) != MAGIC {
            return Err("bad magic (not a ring, or not initialised yet)".into());
        }
        let version = h.layout_version.load(Ordering::Relaxed);
        if version != LAYOUT_VERSION {
            return Err(format!("layout version {version}"));
        }
        let features = h.required_features.load(Ordering::Relaxed);
        if features & !KNOWN_REQUIRED_FEATURES != 0 {
            return Err(format!("unknown required features {features:#x}"));
        }
        let to_usize = |v: u64| usize::try_from(v).map_err(|_| "field overflows usize".to_owned());
        let header_size = h.header_size.load(Ordering::Relaxed) as usize;
        let desc_size = h.slot_desc_size.load(Ordering::Relaxed) as usize;
        let slot_count = h.slot_count.load(Ordering::Relaxed);
        let payload_offset = to_usize(h.payload_offset.load(Ordering::Relaxed))?;
        let slot_stride = to_usize(h.slot_stride.load(Ordering::Relaxed))?;
        let slot_capacity = to_usize(h.slot_capacity.load(Ordering::Relaxed))?;
        let mapping_size = to_usize(h.mapping_size.load(Ordering::Relaxed))?;
        let table_end = header_size + slot_count as usize * desc_size;
        let payload_end = slot_stride
            .checked_mul(slot_count as usize)
            .and_then(|n| n.checked_add(payload_offset));
        let ok = desc_size == SLOT_DESC_SIZE
            && slot_count >= 1
            && table_end <= payload_offset
            && payload_offset % 4096 == 0
            && slot_capacity <= slot_stride
            && payload_end == Some(mapping_size)
            && mapping_size <= shm.len();
        if !ok {
            return Err("inconsistent geometry".into());
        }
        let epoch = h.ring_epoch.load(Ordering::Relaxed);
        if expected_epoch.is_some_and(|e| e != epoch) {
            return Err(format!("epoch {epoch:#x} is not the announced one"));
        }
        Ok(Self {
            shm,
            epoch,
            slot_count,
            slot_capacity,
            slot_stride,
            payload_offset,
            torn_retries: Cell::new(0),
        })
    }

    pub(crate) fn name(&self) -> &RingName {
        self.shm.name()
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(crate) fn slot_count(&self) -> u32 {
        self.slot_count
    }

    pub(crate) fn slot_capacity(&self) -> usize {
        self.slot_capacity
    }

    #[allow(clippy::cast_ptr_alignment)] // Page-aligned mapping base.
    pub(crate) fn header(&self) -> &RingHeader {
        // SAFETY: validated in `open`; see there.
        unsafe { &*self.shm.as_ptr().cast::<RingHeader>() }
    }

    #[allow(clippy::cast_ptr_alignment)] // Page-aligned base + multiple of 8 (validated).
    fn desc(&self, slot: u32) -> &SlotDesc {
        assert!(slot < self.slot_count);
        let header_size = self.header().header_size.load(Ordering::Relaxed) as usize;
        // SAFETY: the slot table lies inside the mapping (validated in `open`).
        unsafe {
            &*self
                .shm
                .as_ptr()
                .add(header_size + slot as usize * SLOT_DESC_SIZE)
                .cast::<SlotDesc>()
        }
    }

    pub(crate) fn write_index(&self) -> u64 {
        self.header().write_index.load(Ordering::Acquire)
    }

    pub(crate) fn writer_state(&self) -> Option<WriterState> {
        WriterState::from_u32(self.header().writer_state.load(Ordering::Acquire))
    }

    pub(crate) fn heartbeat_ns(&self) -> u64 {
        self.header().heartbeat_ns.load(Ordering::Acquire)
    }

    /// True while the writer is live and its heartbeat is fresher than `timeout_ns`.
    pub(crate) fn writer_alive(&self, now_ns: u64, timeout_ns: u64) -> bool {
        self.writer_state() == Some(WriterState::Live)
            && now_ns.saturating_sub(self.heartbeat_ns()) <= timeout_ns
    }

    /// The audio clock under its seqlock.
    pub(crate) fn clock(&self) -> Option<ClockSample> {
        let h = self.header();
        for _ in 0..1000 {
            let l1 = h.clock_lock.load(Ordering::Acquire);
            if l1 & 1 == 0 {
                let s = ClockSample::from_raw(
                    h.clock_flicks.load(Ordering::Relaxed),
                    h.clock_host_ns.load(Ordering::Relaxed),
                    h.clock_rate_milli.load(Ordering::Relaxed),
                    h.clock_state.load(Ordering::Relaxed),
                );
                fence(Ordering::Acquire);
                if h.clock_lock.load(Ordering::Relaxed) == l1 {
                    return Some(s);
                }
            }
            std::hint::spin_loop();
        }
        None
    }

    /// The negotiated configuration under its seqlock.
    pub(crate) fn config(&self) -> Option<ConfigSnapshot> {
        let h = self.header();
        for _ in 0..1000 {
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

    /// The successor ring, once this one is retired.
    pub(crate) fn successor(&self) -> Option<(RingName, u64)> {
        let h = self.header();
        if self.writer_state() != Some(WriterState::Retired) {
            return None;
        }
        let words = std::array::from_fn(|i| h.successor_name[i].load(Ordering::Relaxed));
        let name = RingName::parse(&unpack_name(&words)?).ok()?;
        Some((name, h.successor_epoch.load(Ordering::Relaxed)))
    }

    /// Open the successor ring (verifying its epoch), if this one is retired.
    pub(crate) fn follow(&self) -> Option<Result<Self, String>> {
        let (name, epoch) = self.successor()?;
        Some(Self::open(&name, Some(epoch)))
    }

    /// Validate a stable descriptor against the layout rules.
    fn validate(&self, d: &SlotDesc, slot: u32, lock: u64) -> Result<Option<SlotMeta>, ReadError> {
        let relaxed = Ordering::Relaxed;
        let seq = d.seq.load(relaxed);
        if seq == 0 {
            return Ok(None);
        }
        let corrupt = |what: &str| ReadError::Corrupt(format!("slot {slot} seq {seq}: {what}"));
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
        if payload_len != layout.len() || payload_len > self.slot_capacity {
            return Err(corrupt("payload_len"));
        }
        Ok(Some(SlotMeta {
            slot,
            lock,
            seq,
            pts: d.pts.load(relaxed),
            duration: d.duration.load(relaxed),
            rev: d.rev.load(relaxed),
            flags: d.flags.load(relaxed),
            config_gen: d.config_gen.load(relaxed),
            layout,
            render_ns: d.render_ns.load(relaxed),
            publish_ns: d.publish_ns.load(relaxed),
        }))
    }

    /// One attempt at a consistent descriptor. `Err(Busy)` if the slot is being written or
    /// changed underneath; `Ok(None)` for an empty slot.
    pub(crate) fn try_meta(&self, slot: u32) -> Result<Option<SlotMeta>, ReadError> {
        let d = self.desc(slot);
        let lock = d.lock.load(Ordering::Acquire);
        if lock & 1 == 1 {
            return Err(ReadError::Busy);
        }
        let meta = self.validate(d, slot, lock);
        fence(Ordering::Acquire);
        if d.lock.load(Ordering::Relaxed) != lock {
            self.torn_retries.set(self.torn_retries.get() + 1);
            return Err(ReadError::Busy);
        }
        meta
    }

    /// True if the slot still holds the frame `meta` was taken from.
    pub(crate) fn still_valid(&self, meta: &SlotMeta) -> bool {
        fence(Ordering::Acquire);
        self.desc(meta.slot).lock.load(Ordering::Relaxed) == meta.lock
    }

    /// Copy slot `slot` into `dst` under its seqlock.
    pub(crate) fn read_slot_into(
        &self,
        slot: u32,
        dst: &mut [u8],
    ) -> Result<Option<SlotMeta>, ReadError> {
        for _ in 0..READ_RETRIES {
            let meta = match self.try_meta(slot) {
                Ok(Some(m)) => m,
                Ok(None) => return Ok(None),
                Err(ReadError::Busy) => continue,
                Err(e) => return Err(e),
            };
            let len = meta.layout.len();
            if dst.len() < len {
                return Err(ReadError::BufferTooSmall);
            }
            let src = self.payload_offset + slot as usize * self.slot_stride;
            // SAFETY: `src + len` lies inside this reader's own read-only mapping (len <=
            // capacity <= stride, validated). The writer may be changing these bytes in another
            // mapping right now; that is the seqlock's job to detect: the copy is discarded
            // unless the lock word is unchanged afterwards.
            unsafe {
                std::ptr::copy_nonoverlapping(self.shm.as_ptr().add(src), dst.as_mut_ptr(), len);
            }
            if self.still_valid(&meta) {
                return Ok(Some(meta));
            }
            self.torn_retries.set(self.torn_retries.get() + 1);
        }
        Err(ReadError::Busy)
    }

    /// Copy the newest frame if its `seq` is above `newer_than` (latest wins).
    pub(crate) fn read_latest_into(
        &self,
        dst: &mut [u8],
        newer_than: u64,
    ) -> Result<Option<SlotMeta>, ReadError> {
        for _ in 0..READ_RETRIES {
            let wi = self.write_index();
            if wi == 0 {
                return Err(ReadError::Empty);
            }
            if wi <= newer_than {
                return Ok(None);
            }
            let slot = u32::try_from((wi - 1) % u64::from(self.slot_count)).unwrap_or(0);
            match self.read_slot_into(slot, dst) {
                // The writer may have lapped us: any consistent frame newer than the caller's
                // last one is fine, the newest wins next time.
                Ok(Some(m)) if m.seq > newer_than => return Ok(Some(m)),
                // Newest slot empty: a successor ring before its first frame.
                Ok(None) => return Ok(None),
                Ok(Some(_)) | Err(ReadError::Busy) => {}
                Err(e) => return Err(e),
            }
        }
        Err(ReadError::Busy)
    }

    /// Clock-scheduled choice (arch §10 step 3): copy the newest frame with
    /// `pts <= clock_flicks`, if it is newer than `newer_than`.
    pub(crate) fn read_at_clock_into(
        &self,
        dst: &mut [u8],
        clock_flicks: i64,
        newer_than: u64,
    ) -> Result<Option<SlotMeta>, ReadError> {
        for _ in 0..READ_RETRIES {
            let best = (0..self.slot_count)
                .filter_map(|s| self.try_meta(s).ok().flatten())
                .filter(|m| m.pts <= clock_flicks && m.seq > newer_than)
                .max_by_key(|m| m.seq);
            let Some(best) = best else {
                return Ok(None);
            };
            match self.read_slot_into(best.slot, dst) {
                Ok(Some(m)) if m.seq == best.seq => return Ok(Some(m)),
                Ok(_) | Err(ReadError::Busy) => {}
                Err(e) => return Err(e),
            }
        }
        Err(ReadError::Busy)
    }
}
