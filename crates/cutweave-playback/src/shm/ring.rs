// SPDX-License-Identifier: GPL-3.0-or-later
//! One mapped ring (writer side): typed access to the header and slot table, and the writer
//! halves of the seqlock protocols. [`crate::shm::FramePublisher`] owns the current ring.

use std::sync::atomic::{AtomicU64, Ordering, fence};

use crate::shm::clock::ClockSample;
use crate::shm::layout::{
    HEADER_SIZE, KNOWN_REQUIRED_FEATURES, LAYOUT_VERSION, MAGIC, PAYLOAD_OFFSET, RingHeader,
    SLOT_DESC_SIZE, SLOT_TABLE_OFFSET, SlotDesc, WriterState, mapping_size, pack_name, slot_stride,
};
use crate::shm::name::{RingName, random_u64};
use crate::shm::os::{HOST_CLOCK, SharedMemory, ShmError, host_clock_ns};
use crate::shm::publisher::{PreviewConfig, RingError};

/// Values a new ring starts with, written before it becomes visible (`magic`).
pub(crate) struct RingInit {
    /// `write_index` to start from (seq continuity across successor rings).
    pub start_index: u64,
    /// Producer drops carried over from a predecessor.
    pub frames_dropped: u64,
    /// Negotiated configuration and its generation.
    pub config: (u32, PreviewConfig),
    /// Initial audio clock.
    pub clock: ClockSample,
    /// Touch every payload page now so the first lap does not page-fault on the render thread.
    pub prefault: bool,
}

#[allow(clippy::cast_possible_truncation)] // 1024 and 128 fit in u32 (const-evaluated).
const HEADER_SIZE_U32: u32 = HEADER_SIZE as u32;
#[allow(clippy::cast_possible_truncation)]
const SLOT_DESC_SIZE_U32: u32 = SLOT_DESC_SIZE as u32;

/// A mapped ring this process created and writes.
pub(crate) struct Ring {
    shm: SharedMemory,
    epoch: u64,
    slot_count: u32,
    slot_capacity: usize,
    slot_stride: usize,
}

impl Ring {
    /// Create and initialise a ring under `name`.
    pub(crate) fn create(
        name: &RingName,
        slot_count: u32,
        slot_capacity: usize,
        init: &RingInit,
    ) -> Result<Self, RingError> {
        let stride = slot_stride(slot_capacity).ok_or(RingError::Size(slot_capacity))?;
        let size = mapping_size(slot_count, slot_capacity).ok_or(RingError::Size(slot_capacity))?;
        let shm = SharedMemory::create(name, size)?;
        // Mappings start on a page boundary (4 KiB or more on every supported OS), which the
        // header atomics and the 16 KiB payload offsets rely on.
        if shm.len() < size || shm.as_ptr().align_offset(4096) != 0 {
            return Err(RingError::Size(size));
        }
        let ring = Self {
            shm,
            epoch: random_u64()?,
            slot_count,
            slot_capacity,
            slot_stride: stride,
        };
        ring.initialise(size, init);
        Ok(ring)
    }

    fn initialise(&self, size: usize, init: &RingInit) {
        let h = self.header();
        let relaxed = Ordering::Relaxed;
        h.layout_version.store(LAYOUT_VERSION, relaxed);
        h.required_features.store(KNOWN_REQUIRED_FEATURES, relaxed);
        h.header_size.store(HEADER_SIZE_U32, relaxed);
        h.slot_desc_size.store(SLOT_DESC_SIZE_U32, relaxed);
        h.slot_count.store(self.slot_count, relaxed);
        h.payload_offset.store(PAYLOAD_OFFSET as u64, relaxed);
        h.slot_stride.store(self.slot_stride as u64, relaxed);
        h.slot_capacity.store(self.slot_capacity as u64, relaxed);
        h.mapping_size.store(size as u64, relaxed);
        h.ring_epoch.store(self.epoch, relaxed);
        h.writer_pid.store(std::process::id(), relaxed);
        h.host_clock.store(HOST_CLOCK as u32, relaxed);
        h.write_index.store(init.start_index, relaxed);
        h.frames_dropped.store(init.frames_dropped, relaxed);
        h.heartbeat_ns.store(host_clock_ns(), relaxed);
        let (generation, config) = init.config;
        self.write_config(generation, config);
        self.write_clock(&init.clock);
        if init.prefault {
            self.prefault();
        }
        h.writer_state.store(WriterState::Live as u32, relaxed);
        // Magic last: a reader that sees it (Acquire) sees a fully initialised header.
        h.magic.store(MAGIC, Ordering::Release);
    }

    /// Write one byte per 4 KiB page of the payload area so physical pages exist before the
    /// render thread's first lap. Fresh shared memory is zero, so this changes no content.
    fn prefault(&self) {
        let base = self.shm.as_ptr();
        let end = PAYLOAD_OFFSET + self.slot_count as usize * self.slot_stride;
        for off in (PAYLOAD_OFFSET..end).step_by(4096) {
            // SAFETY: `off` < mapping size (checked in `create`); no reader can be looking at
            // the ring yet because `magic` is not set, and the value written is the existing 0.
            unsafe { base.add(off).write_volatile(0) };
        }
    }

    /// The ring's name.
    pub(crate) fn name(&self) -> &RingName {
        self.shm.name()
    }

    /// The ring's random epoch.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Usable bytes per slot.
    pub(crate) fn slot_capacity(&self) -> usize {
        self.slot_capacity
    }

    /// Total bytes mapped.
    pub(crate) fn mapping_size(&self) -> usize {
        self.shm.len()
    }

    /// OS access control of the object (diagnostics).
    pub(crate) fn access_description(&self) -> Result<String, ShmError> {
        self.shm.access_description()
    }

    // The mapping base is page aligned, so the cast is aligned (checked in `create`).
    #[allow(clippy::cast_ptr_alignment)]
    pub(crate) fn header(&self) -> &RingHeader {
        // SAFETY: the mapping is at least HEADER_SIZE bytes, page aligned, and lives as long as
        // `self`; `RingHeader` is `repr(C)` and only atomics (plus never-touched reserved
        // bytes), so any bit pattern is valid and shared mutation goes through atomics.
        unsafe { &*self.shm.as_ptr().cast::<RingHeader>() }
    }

    // Page-aligned base plus a multiple of 128: aligned for `SlotDesc`.
    #[allow(clippy::cast_ptr_alignment)]
    pub(crate) fn desc(&self, slot: u32) -> &SlotDesc {
        assert!(slot < self.slot_count, "slot {slot} out of range");
        let off = SLOT_TABLE_OFFSET + slot as usize * SLOT_DESC_SIZE;
        // SAFETY: as for `header`; the slot table ends below PAYLOAD_OFFSET (const-asserted in
        // `layout`) and `off` is 8-byte aligned.
        unsafe { &*self.shm.as_ptr().add(off).cast::<SlotDesc>() }
    }

    /// Base of slot `slot`'s payload (`slot_capacity` bytes).
    pub(crate) fn payload_ptr(&self, slot: u32) -> *mut u8 {
        assert!(slot < self.slot_count, "slot {slot} out of range");
        // SAFETY: PAYLOAD_OFFSET + slot * stride + capacity <= mapping size (checked in
        // `create`), so the offset stays inside the mapping.
        unsafe {
            self.shm
                .as_ptr()
                .add(PAYLOAD_OFFSET + slot as usize * self.slot_stride)
        }
    }

    /// Store a clock sample under the clock seqlock. Safe to call from several threads.
    pub(crate) fn write_clock(&self, s: &ClockSample) {
        let h = self.header();
        seqlock_write(&h.clock_lock, || {
            h.clock_flicks.store(s.position.flicks(), Ordering::Relaxed);
            h.clock_host_ns.store(s.host_ns, Ordering::Relaxed);
            h.clock_rate_milli.store(s.rate_milli, Ordering::Relaxed);
            h.clock_state.store(s.state as u32, Ordering::Relaxed);
        });
    }

    /// The clock sample last stored (writer side; used to seed a successor ring).
    pub(crate) fn read_clock(&self) -> ClockSample {
        let h = self.header();
        loop {
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
                    return s;
                }
            }
            std::hint::spin_loop();
        }
    }

    /// Store the negotiated preview configuration under the config seqlock.
    pub(crate) fn write_config(&self, generation: u32, c: PreviewConfig) {
        let h = self.header();
        seqlock_write(&h.config_lock, || {
            h.config_gen.store(generation, Ordering::Relaxed);
            h.preview_width.store(c.width, Ordering::Relaxed);
            h.preview_height.store(c.height, Ordering::Relaxed);
            h.preview_format.store(c.format as u32, Ordering::Relaxed);
        });
    }

    /// Record the successor ring, then mark this one retired (readers follow the name).
    pub(crate) fn retire(&self, successor: &RingName, successor_epoch: u64) {
        let h = self.header();
        h.successor_epoch.store(successor_epoch, Ordering::Relaxed);
        for (word, value) in h.successor_name.iter().zip(pack_name(successor.as_str())) {
            word.store(value, Ordering::Relaxed);
        }
        // Release: a reader that sees `Retired` (Acquire) sees the successor fields.
        h.writer_state
            .store(WriterState::Retired as u32, Ordering::Release);
    }

    /// Mark the ring closed (clean shutdown).
    pub(crate) fn mark_closed(&self) {
        let h = self.header();
        h.heartbeat_ns.fetch_max(host_clock_ns(), Ordering::Relaxed);
        h.writer_state
            .store(WriterState::Closed as u32, Ordering::Release);
    }
}

/// Writer half of a seqlock: make `lock` odd, run `write`, make it even again. Writers are
/// serialised with a compare-exchange, so several threads may call this for the same lock.
pub(crate) fn seqlock_write(lock: &AtomicU64, write: impl FnOnce()) {
    let mut spins = 0u32;
    let odd = loop {
        let cur = lock.load(Ordering::Relaxed);
        if cur & 1 == 0
            && lock
                .compare_exchange_weak(cur, cur + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        {
            break cur + 1;
        }
        spins += 1;
        if spins < 64 {
            std::hint::spin_loop();
        } else {
            std::thread::yield_now();
        }
    };
    // Orders the odd lock word before every data store that follows.
    fence(Ordering::Release);
    write();
    lock.store(odd + 1, Ordering::Release);
}
