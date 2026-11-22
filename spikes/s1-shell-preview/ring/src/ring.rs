//! Single-producer, multi-consumer latest-wins frame ring over a [`Mapping`].
//!
//! Writer protocol per frame (slot `s = write_index % N`):
//! 1. `lock = lock + 1` (odd = writing), `Release` fence.
//! 2. Fill payload, stamp rows, compute checksum, store descriptor fields (`Relaxed`).
//! 3. `lock = lock + 1` (even) with `Release`.
//! 4. `write_index += 1` with `Release`.
//!
//! Reader protocol (seqlock): read `lock` (`Acquire`, must be even), copy descriptor and
//! payload, `Acquire` fence, re-read `lock`; if it changed the copy is torn and is retried.

use std::sync::Arc;
use std::sync::atomic::{Ordering, fence};

use thiserror::Error;

use crate::checksum::{sampled_fnv1a, stamp_rows};
use crate::clock::now_us;
use crate::layout::{
    EngineState, HEADER_SIZE, MAGIC, MAX_SLOTS, MIN_SLOTS, PAYLOAD_ALIGN, PixelFormat, RingHeader,
    SLOT_DESC_SIZE, SLOT_TABLE_OFFSET, SlotDesc, VERSION, align_up, mapping_len,
};
use crate::shm::Mapping;

/// Ring setup / attach errors.
#[derive(Debug, Error)]
pub enum RingError {
    /// Slot count outside 4..=6.
    #[error("slot count {0} outside {MIN_SLOTS}..={MAX_SLOTS}")]
    SlotCount(u32),
    /// The mapping is smaller than the layout requires.
    #[error("mapping too small: need {need} bytes, have {have}")]
    TooSmall {
        /// Required size.
        need: usize,
        /// Actual size.
        have: usize,
    },
    /// Header magic/version mismatch.
    #[error("not a CWS1 v{VERSION} ring (magic {magic:#x}, version {version})")]
    BadHeader {
        /// Found magic.
        magic: u32,
        /// Found version.
        version: u32,
    },
    /// Payload larger than the slot capacity.
    #[error("payload of {len} bytes exceeds slot capacity {capacity}")]
    PayloadTooLarge {
        /// Requested length.
        len: usize,
        /// Slot capacity.
        capacity: usize,
    },
}

/// Per-read errors (all recoverable).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReadError {
    /// No frame has been published yet.
    #[error("ring is empty")]
    Empty,
    /// The writer overwrote the slot while we were copying it (after all retries).
    #[error("slot {0} was overwritten during the read")]
    Torn(u32),
    /// Destination buffer too small.
    #[error("destination holds {have} bytes, frame needs {need}")]
    BufferTooSmall {
        /// Required size.
        need: usize,
        /// Provided size.
        have: usize,
    },
    /// Descriptor is inconsistent (payload length beyond capacity).
    #[error("slot {0} descriptor is corrupt")]
    Corrupt(u32),
    /// Slot index out of range.
    #[error("slot {0} out of range")]
    NoSuchSlot(u32),
}

/// Metadata of one frame as published by the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameMeta {
    /// Presentation timestamp (flicks).
    pub pts: i64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: PixelFormat,
    /// Render revision.
    pub rev: u64,
    /// Flag bits (`FLAG_LOWRES`).
    pub flags: u32,
    /// Host µs at which GPU work for the frame was submitted.
    pub t_render_us: u64,
}

/// A consistent snapshot of one slot descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotMeta {
    /// Slot index.
    pub slot: u32,
    /// Seqlock value the snapshot was taken at (even).
    pub lock: u64,
    /// Frame sequence number (0 = slot never written).
    pub seq: u64,
    /// pts in flicks.
    pub pts: i64,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Raw format word (see [`PixelFormat`]).
    pub format: u32,
    /// Bytes per row of plane 0.
    pub stride: u32,
    /// Render revision.
    pub rev: u64,
    /// Payload bytes used.
    pub payload_len: u64,
    /// Writer-side checksum.
    pub checksum: u32,
    /// Flag bits (`FLAG_LOWRES`).
    pub flags: u32,
    /// Host µs at GPU submit.
    pub t_render_us: u64,
    /// Host µs at publish.
    pub t_publish_us: u64,
}

/// Snapshot of the ring header plus every slot descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingSnapshot {
    /// Slot count.
    pub slot_count: u32,
    /// Slot capacity in bytes.
    pub slot_capacity: u64,
    /// Frames published so far.
    pub write_index: u64,
    /// Fake audio clock (flicks).
    pub audio_clock_flicks: i64,
    /// Host µs at which the audio clock was sampled.
    pub audio_clock_host_us: u64,
    /// Engine heartbeat (host µs).
    pub heartbeat_us: u64,
    /// Frame duration (flicks).
    pub frame_duration_flicks: i64,
    /// Engine PID.
    pub engine_pid: u32,
    /// Raw engine state word.
    pub engine_state: u32,
    /// Producer-side dropped frames.
    pub engine_dropped: u64,
    /// Per-slot descriptors (`None` if the slot was mid-write for every retry).
    pub slots: Vec<Option<SlotMeta>>,
}

/// Shared geometry + raw accessors used by both ends.
struct Layout<M: Mapping> {
    mem: Arc<M>,
    slot_count: u32,
    capacity: usize,
    stride: usize,
}

impl<M: Mapping> Layout<M> {
    fn header(&self) -> &RingHeader {
        // SAFETY: the mapping is at least HEADER_SIZE bytes, 8-byte aligned and lives as long
        // as `self.mem`; RingHeader is all atomics (valid for any bit pattern).
        unsafe { &*self.mem.as_ptr().cast::<RingHeader>() }
    }

    fn desc(&self, slot: u32) -> &SlotDesc {
        debug_assert!(slot < self.slot_count);
        let off = SLOT_TABLE_OFFSET + slot as usize * SLOT_DESC_SIZE;
        // SAFETY: as above; the slot table fits in the header page (const-asserted).
        unsafe { &*self.mem.as_ptr().add(off).cast::<SlotDesc>() }
    }

    fn payload_ptr(&self, slot: u32) -> *mut u8 {
        // SAFETY: offset within the mapping (checked against mapping_len at init/attach).
        unsafe {
            self.mem
                .as_ptr()
                .add(HEADER_SIZE + slot as usize * self.stride)
        }
    }
}

/// The single writer (engine side).
pub struct RingWriter<M: Mapping> {
    l: Layout<M>,
}

/// Cheap handle that updates the clock/heartbeat/state words from any thread.
pub struct RingClock<M: Mapping> {
    mem: Arc<M>,
}

impl<M: Mapping> Clone for RingClock<M> {
    fn clone(&self) -> Self {
        Self {
            mem: Arc::clone(&self.mem),
        }
    }
}

impl<M: Mapping> RingClock<M> {
    fn header(&self) -> &RingHeader {
        // SAFETY: see Layout::header.
        unsafe { &*self.mem.as_ptr().cast::<RingHeader>() }
    }

    /// Publish the audio clock position.
    pub fn set_audio_clock(&self, flicks: i64, host_us: u64) {
        let h = self.header();
        h.audio_clock_host_us.store(host_us, Ordering::Relaxed);
        h.audio_clock_flicks.store(flicks, Ordering::Release);
    }

    /// Refresh the heartbeat word.
    pub fn heartbeat(&self) {
        self.header()
            .heartbeat_us
            .store(now_us(), Ordering::Release);
    }

    /// Set the engine state word.
    pub fn set_state(&self, state: EngineState) {
        self.header()
            .engine_state
            .store(state as u32, Ordering::Release);
    }

    /// Count producer-side dropped frames.
    pub fn add_dropped(&self, n: u64) {
        self.header().engine_dropped.fetch_add(n, Ordering::Relaxed);
    }
}

impl<M: Mapping> RingWriter<M> {
    /// Initialise a fresh ring in `mem`.
    pub fn init(
        mem: M,
        slot_count: u32,
        slot_capacity: usize,
        frame_duration_flicks: i64,
    ) -> Result<Self, RingError> {
        if !(MIN_SLOTS..=MAX_SLOTS).contains(&slot_count) {
            return Err(RingError::SlotCount(slot_count));
        }
        let need = mapping_len(slot_count, slot_capacity);
        if mem.len() < need {
            return Err(RingError::TooSmall {
                need,
                have: mem.len(),
            });
        }
        let l = Layout {
            mem: Arc::new(mem),
            slot_count,
            capacity: slot_capacity,
            stride: align_up(slot_capacity, PAYLOAD_ALIGN),
        };
        let h = l.header();
        h.version.store(VERSION, Ordering::Relaxed);
        h.header_size.store(HEADER_SIZE as u32, Ordering::Relaxed);
        h.slot_count.store(slot_count, Ordering::Relaxed);
        h.slot_capacity
            .store(slot_capacity as u64, Ordering::Relaxed);
        h.payload_offset
            .store(HEADER_SIZE as u64, Ordering::Relaxed);
        h.slot_stride.store(l.stride as u64, Ordering::Relaxed);
        h.write_index.store(0, Ordering::Relaxed);
        h.frame_duration_flicks
            .store(frame_duration_flicks, Ordering::Relaxed);
        h.engine_pid.store(std::process::id(), Ordering::Relaxed);
        h.engine_state
            .store(EngineState::Starting as u32, Ordering::Relaxed);
        h.engine_dropped.store(0, Ordering::Relaxed);
        h.heartbeat_us.store(now_us(), Ordering::Relaxed);
        for s in 0..slot_count {
            l.desc(s).lock.store(0, Ordering::Relaxed);
            l.desc(s).seq.store(0, Ordering::Relaxed);
        }
        // Magic last: a reader that sees it sees a fully initialised header.
        h.magic.store(MAGIC, Ordering::Release);
        Ok(Self { l })
    }

    /// Clock/heartbeat handle for other threads.
    #[must_use]
    pub fn clock(&self) -> RingClock<M> {
        RingClock {
            mem: Arc::clone(&self.l.mem),
        }
    }

    /// Slot capacity in bytes.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.l.capacity
    }

    /// Frames published so far.
    #[must_use]
    pub fn write_index(&self) -> u64 {
        self.l.header().write_index.load(Ordering::Relaxed)
    }

    /// Publish one frame. `fill` receives the slot's payload buffer (capacity bytes) and
    /// returns the number of bytes written. Rows are then stamped with the new `seq` and the
    /// checksum is computed over the final bytes. Returns the frame's `seq`.
    ///
    /// Takes `&mut self`: there must be exactly one writer.
    pub fn publish(
        &mut self,
        meta: &FrameMeta,
        fill: impl FnOnce(&mut [u8]) -> usize,
    ) -> Result<u64, RingError> {
        let h = self.l.header();
        let wi = h.write_index.load(Ordering::Relaxed);
        let seq = wi + 1;
        let slot = (wi % u64::from(self.l.slot_count)) as u32;
        let d = self.l.desc(slot);

        let lock = d.lock.load(Ordering::Relaxed);
        d.lock.store(lock | 1, Ordering::Relaxed);
        fence(Ordering::Release);

        // SAFETY: payload region of `capacity` bytes inside the mapping; we are the only writer.
        let buf =
            unsafe { std::slice::from_raw_parts_mut(self.l.payload_ptr(slot), self.l.capacity) };
        let len = fill(buf);
        if len > self.l.capacity {
            // Leave the slot marked as being written so readers never trust it.
            return Err(RingError::PayloadTooLarge {
                len,
                capacity: self.l.capacity,
            });
        }
        let stride = meta.format.stride(meta.width);
        stamp_rows(&mut buf[..len], stride as usize, meta.height as usize, seq);
        let checksum = sampled_fnv1a(&buf[..len]);

        d.seq.store(seq, Ordering::Relaxed);
        d.pts.store(meta.pts, Ordering::Relaxed);
        d.width.store(meta.width, Ordering::Relaxed);
        d.height.store(meta.height, Ordering::Relaxed);
        d.format.store(meta.format as u32, Ordering::Relaxed);
        d.stride.store(stride, Ordering::Relaxed);
        d.rev.store(meta.rev, Ordering::Relaxed);
        d.payload_len.store(len as u64, Ordering::Relaxed);
        d.checksum.store(checksum, Ordering::Relaxed);
        d.flags.store(meta.flags, Ordering::Relaxed);
        d.t_render_us.store(meta.t_render_us, Ordering::Relaxed);
        d.t_publish_us.store(now_us(), Ordering::Relaxed);

        d.lock.store((lock | 1) + 1, Ordering::Release);
        h.write_index.store(seq, Ordering::Release);
        Ok(seq)
    }

    /// Test hook: mark `slot` as being written (odd lock) without finishing, to simulate a
    /// writer that is mid-copy or died mid-copy.
    pub fn begin_write_for_test(&self, slot: u32) {
        let d = self.l.desc(slot);
        let lock = d.lock.load(Ordering::Relaxed);
        d.lock.store(lock | 1, Ordering::Release);
    }

    /// Test hook: finish a write started by [`Self::begin_write_for_test`].
    pub fn end_write_for_test(&self, slot: u32) {
        let d = self.l.desc(slot);
        let lock = d.lock.load(Ordering::Relaxed);
        d.lock.store((lock | 1) + 1, Ordering::Release);
    }

    /// Test hook: scribble over a published payload *without* touching the seqlock, i.e. a
    /// protocol violation the renderer-side checksum must catch.
    pub fn corrupt_payload_for_test(&self, slot: u32, offset: usize) {
        if offset < self.l.capacity {
            // SAFETY: in-bounds byte of the slot payload.
            unsafe {
                let p = self.l.payload_ptr(slot).add(offset);
                p.write(p.read() ^ 0xFF);
            }
        }
    }
}

/// A reader (addon side). Any number may exist, in any process.
pub struct RingReader<M: Mapping> {
    l: Layout<M>,
}

const READ_RETRIES: u32 = 3;

impl<M: Mapping> RingReader<M> {
    /// Attach to an initialised ring.
    pub fn attach(mem: M) -> Result<Self, RingError> {
        if mem.len() < HEADER_SIZE {
            return Err(RingError::TooSmall {
                need: HEADER_SIZE,
                have: mem.len(),
            });
        }
        // SAFETY: see Layout::header.
        let h = unsafe { &*mem.as_ptr().cast::<RingHeader>() };
        let magic = h.magic.load(Ordering::Acquire);
        let version = h.version.load(Ordering::Relaxed);
        if magic != MAGIC || version != VERSION {
            return Err(RingError::BadHeader { magic, version });
        }
        let slot_count = h.slot_count.load(Ordering::Relaxed);
        if !(MIN_SLOTS..=MAX_SLOTS).contains(&slot_count) {
            return Err(RingError::SlotCount(slot_count));
        }
        let capacity =
            usize::try_from(h.slot_capacity.load(Ordering::Relaxed)).unwrap_or(usize::MAX);
        let stride = usize::try_from(h.slot_stride.load(Ordering::Relaxed)).unwrap_or(usize::MAX);
        let need = mapping_len(slot_count, capacity);
        if stride != align_up(capacity, PAYLOAD_ALIGN) || mem.len() < need {
            return Err(RingError::TooSmall {
                need,
                have: mem.len(),
            });
        }
        Ok(Self {
            l: Layout {
                mem: Arc::new(mem),
                slot_count,
                capacity,
                stride,
            },
        })
    }

    /// Slot count.
    #[must_use]
    pub fn slot_count(&self) -> u32 {
        self.l.slot_count
    }

    /// Slot capacity in bytes.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.l.capacity
    }

    /// Frames published so far (`Acquire`).
    #[must_use]
    pub fn write_index(&self) -> u64 {
        self.l.header().write_index.load(Ordering::Acquire)
    }

    /// Slot holding the newest frame, if any.
    #[must_use]
    pub fn latest_slot(&self) -> Option<u32> {
        let wi = self.write_index();
        (wi > 0).then(|| ((wi - 1) % u64::from(self.l.slot_count)) as u32)
    }

    fn try_meta(&self, slot: u32) -> Option<SlotMeta> {
        let d = self.l.desc(slot);
        let lock = d.lock.load(Ordering::Acquire);
        if lock & 1 == 1 {
            return None;
        }
        let m = SlotMeta {
            slot,
            lock,
            seq: d.seq.load(Ordering::Relaxed),
            pts: d.pts.load(Ordering::Relaxed),
            width: d.width.load(Ordering::Relaxed),
            height: d.height.load(Ordering::Relaxed),
            format: d.format.load(Ordering::Relaxed),
            stride: d.stride.load(Ordering::Relaxed),
            rev: d.rev.load(Ordering::Relaxed),
            payload_len: d.payload_len.load(Ordering::Relaxed),
            checksum: d.checksum.load(Ordering::Relaxed),
            flags: d.flags.load(Ordering::Relaxed),
            t_render_us: d.t_render_us.load(Ordering::Relaxed),
            t_publish_us: d.t_publish_us.load(Ordering::Relaxed),
        };
        fence(Ordering::Acquire);
        (d.lock.load(Ordering::Relaxed) == lock).then_some(m)
    }

    /// Consistent descriptor of `slot` (retries a few times while it is being written).
    pub fn slot_meta(&self, slot: u32) -> Result<SlotMeta, ReadError> {
        if slot >= self.l.slot_count {
            return Err(ReadError::NoSuchSlot(slot));
        }
        (0..READ_RETRIES)
            .find_map(|_| self.try_meta(slot))
            .ok_or(ReadError::Torn(slot))
    }

    /// True if `slot` still holds the frame described by `meta` (lock unchanged). Use after
    /// consuming a zero-copy view of the payload to validate it was not overwritten.
    #[must_use]
    pub fn still_valid(&self, meta: &SlotMeta) -> bool {
        fence(Ordering::Acquire);
        self.l.desc(meta.slot).lock.load(Ordering::Relaxed) == meta.lock
    }

    /// Raw pointer + capacity of a slot payload (for zero-copy views).
    #[must_use]
    pub fn payload_ptr(&self, slot: u32) -> *const u8 {
        self.l.payload_ptr(slot)
    }

    /// Copy `slot` into `dst` under the seqlock. Returns the descriptor of the copied frame.
    pub fn read_slot_into(&self, slot: u32, dst: &mut [u8]) -> Result<SlotMeta, ReadError> {
        if slot >= self.l.slot_count {
            return Err(ReadError::NoSuchSlot(slot));
        }
        for _ in 0..READ_RETRIES {
            let Some(m) = self.try_meta(slot) else {
                continue;
            };
            let len = usize::try_from(m.payload_len).unwrap_or(usize::MAX);
            if len > self.l.capacity {
                return Err(ReadError::Corrupt(slot));
            }
            if dst.len() < len {
                return Err(ReadError::BufferTooSmall {
                    need: len,
                    have: dst.len(),
                });
            }
            // SAFETY: `len` bytes inside the slot payload; the writer may be modifying them
            // concurrently, which the seqlock re-check below detects (the bytes are then
            // discarded). Plain byte copies of shared memory are the standard seqlock pattern.
            unsafe {
                std::ptr::copy_nonoverlapping(self.l.payload_ptr(slot), dst.as_mut_ptr(), len)
            };
            if self.still_valid(&m) {
                return Ok(m);
            }
        }
        Err(ReadError::Torn(slot))
    }

    /// Copy the newest frame into `dst` if its `seq` is greater than `newer_than`.
    /// Returns `Ok(None)` when there is nothing new.
    pub fn read_latest_into(
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
            let slot = ((wi - 1) % u64::from(self.l.slot_count)) as u32;
            match self.read_slot_into(slot, dst) {
                Ok(m) => return Ok(Some(m)),
                Err(ReadError::Torn(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Err(ReadError::Torn(self.latest_slot().unwrap_or(0)))
    }

    /// Header + all slot descriptors.
    #[must_use]
    pub fn snapshot(&self) -> RingSnapshot {
        let h = self.l.header();
        RingSnapshot {
            slot_count: self.l.slot_count,
            slot_capacity: self.l.capacity as u64,
            write_index: h.write_index.load(Ordering::Acquire),
            audio_clock_flicks: h.audio_clock_flicks.load(Ordering::Acquire),
            audio_clock_host_us: h.audio_clock_host_us.load(Ordering::Relaxed),
            heartbeat_us: h.heartbeat_us.load(Ordering::Acquire),
            frame_duration_flicks: h.frame_duration_flicks.load(Ordering::Relaxed),
            engine_pid: h.engine_pid.load(Ordering::Relaxed),
            engine_state: h.engine_state.load(Ordering::Acquire),
            engine_dropped: h.engine_dropped.load(Ordering::Relaxed),
            slots: (0..self.l.slot_count)
                .map(|s| self.slot_meta(s).ok())
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checksum::bad_row_stamps;
    use crate::shm::HeapMapping;

    const W: u32 = 64;
    const H: u32 = 8;

    fn ring(slots: u32) -> (RingWriter<Arc<HeapMapping>>, RingReader<Arc<HeapMapping>>) {
        let cap = PixelFormat::Rgba8.frame_len(W, H);
        let mem = Arc::new(HeapMapping::new(mapping_len(slots, cap)));
        let w = RingWriter::init(Arc::clone(&mem), slots, cap, 11_760_000).expect("init");
        let r = RingReader::attach(mem).expect("attach");
        (w, r)
    }

    fn meta(n: u64) -> FrameMeta {
        FrameMeta {
            pts: i64::try_from(n).expect("small") * 11_760_000,
            width: W,
            height: H,
            format: PixelFormat::Rgba8,
            rev: 7,
            flags: u32::from(n % 2 == 1),
            t_render_us: 1,
        }
    }

    fn publish(w: &mut RingWriter<Arc<HeapMapping>>, n: u64) -> u64 {
        let len = PixelFormat::Rgba8.frame_len(W, H);
        w.publish(&meta(n), |buf| {
            buf[..len].fill((n % 251) as u8);
            len
        })
        .expect("publish")
    }

    #[test]
    fn rejects_bad_slot_counts_and_small_mappings() {
        let mem = HeapMapping::new(1 << 20);
        assert!(matches!(
            RingWriter::init(mem, 3, 1024, 1),
            Err(RingError::SlotCount(3))
        ));
        let mem = HeapMapping::new(8192);
        assert!(matches!(
            RingWriter::init(mem, 4, 1 << 20, 1),
            Err(RingError::TooSmall { .. })
        ));
        let mem = HeapMapping::new(8192);
        assert!(matches!(
            RingReader::attach(mem),
            Err(RingError::BadHeader { .. })
        ));
    }

    #[test]
    fn empty_ring_reads_empty() {
        let (_w, r) = ring(4);
        let mut dst = vec![0u8; r.capacity()];
        assert_eq!(r.read_latest_into(&mut dst, 0), Err(ReadError::Empty));
        assert_eq!(r.latest_slot(), None);
    }

    #[test]
    fn wrap_around_keeps_latest_and_monotonic_seq() {
        for slots in MIN_SLOTS..=MAX_SLOTS {
            let (mut w, r) = ring(slots);
            let mut dst = vec![0u8; r.capacity()];
            let mut last_seq = 0;
            // Several full laps around the ring.
            for n in 0..u64::from(slots) * 5 + 3 {
                let seq = publish(&mut w, n);
                assert_eq!(seq, n + 1);
                let m = r
                    .read_latest_into(&mut dst, last_seq)
                    .expect("read")
                    .expect("new frame");
                assert_eq!(m.seq, seq);
                assert!(m.seq > last_seq, "seq must increase");
                assert_eq!(m.slot, (n % u64::from(slots)) as u32, "slot index wraps");
                assert_eq!(m.pts, meta(n).pts);
                assert_eq!(m.rev, 7);
                assert_eq!(m.flags, meta(n).flags, "flags round-trip");
                let len = usize::try_from(m.payload_len).expect("len");
                assert_eq!(sampled_fnv1a(&dst[..len]), m.checksum);
                assert_eq!(
                    bad_row_stamps(&dst[..len], m.stride as usize, H as usize, m.seq),
                    0
                );
                assert_eq!(dst[ROW_PIXEL], (n % 251) as u8);
                last_seq = m.seq;
                // Nothing newer until the next publish.
                assert_eq!(r.read_latest_into(&mut dst, last_seq), Ok(None));
            }
            let snap = r.snapshot();
            assert_eq!(snap.write_index, last_seq);
            // After wrapping, every slot holds one of the last `slots` frames.
            let mut seqs: Vec<u64> = snap.slots.iter().map(|s| s.expect("stable").seq).collect();
            seqs.sort_unstable();
            let expect: Vec<u64> = (last_seq + 1 - u64::from(slots)..=last_seq).collect();
            assert_eq!(seqs, expect);
        }
    }

    // First byte after the row stamp of row 0.
    const ROW_PIXEL: usize = crate::checksum::ROW_STAMP_LEN;

    #[test]
    fn slow_reader_skips_to_latest() {
        let (mut w, r) = ring(4);
        let mut dst = vec![0u8; r.capacity()];
        for n in 0..10 {
            publish(&mut w, n);
        }
        let m = r
            .read_latest_into(&mut dst, 2)
            .expect("read")
            .expect("frame");
        assert_eq!(
            m.seq, 10,
            "latest wins; frames 3..=9 are dropped by the reader"
        );
    }

    #[test]
    fn slot_being_written_is_reported_torn() {
        let (mut w, r) = ring(4);
        let mut dst = vec![0u8; r.capacity()];
        publish(&mut w, 0);
        w.begin_write_for_test(0);
        assert_eq!(r.read_slot_into(0, &mut dst), Err(ReadError::Torn(0)));
        assert_eq!(r.read_latest_into(&mut dst, 0), Err(ReadError::Torn(0)));
        assert!(r.snapshot().slots[0].is_none());
        w.end_write_for_test(0);
        assert!(r.read_slot_into(0, &mut dst).is_ok());
    }

    #[test]
    fn zero_copy_view_validation_detects_overwrite() {
        let (mut w, r) = ring(4);
        publish(&mut w, 0);
        let m = r.slot_meta(0).expect("meta");
        assert!(r.still_valid(&m));
        // Writer laps the ring and reuses slot 0.
        for n in 1..=4 {
            publish(&mut w, n);
        }
        assert!(!r.still_valid(&m));
    }

    #[test]
    fn checksum_catches_unsynchronised_corruption() {
        let (mut w, r) = ring(4);
        let mut dst = vec![0u8; r.capacity()];
        publish(&mut w, 0);
        w.corrupt_payload_for_test(0, 0);
        let m = r
            .read_latest_into(&mut dst, 0)
            .expect("read")
            .expect("frame");
        let len = usize::try_from(m.payload_len).expect("len");
        // Byte 0 is sampled by the checksum and is part of the row-0 stamp.
        assert_ne!(sampled_fnv1a(&dst[..len]), m.checksum);
        assert_eq!(
            bad_row_stamps(&dst[..len], m.stride as usize, H as usize, m.seq),
            1
        );
    }

    #[test]
    fn buffer_too_small_is_an_error() {
        let (mut w, r) = ring(4);
        publish(&mut w, 0);
        let mut dst = vec![0u8; 16];
        assert!(matches!(
            r.read_latest_into(&mut dst, 0),
            Err(ReadError::BufferTooSmall { .. })
        ));
    }

    #[test]
    fn concurrent_writer_and_reader_never_yield_torn_data() {
        let (mut w, r) = ring(4);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = Arc::clone(&stop);
        let writer = std::thread::spawn(move || {
            let mut n = 0u64;
            while !stop2.load(Ordering::Relaxed) {
                publish(&mut w, n);
                n += 1;
            }
            n
        });
        let mut dst = vec![0u8; r.capacity()];
        let (mut good, mut last) = (0u32, 0u64);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        while std::time::Instant::now() < deadline {
            match r.read_latest_into(&mut dst, last) {
                Ok(Some(m)) => {
                    let len = usize::try_from(m.payload_len).expect("len");
                    assert_eq!(
                        sampled_fnv1a(&dst[..len]),
                        m.checksum,
                        "torn copy escaped seqlock"
                    );
                    assert!(m.seq > last);
                    last = m.seq;
                    good += 1;
                }
                Ok(None) | Err(ReadError::Torn(_) | ReadError::Empty) => {}
                Err(e) => panic!("{e}"),
            }
        }
        stop.store(true, Ordering::Relaxed);
        let written = writer.join().expect("writer");
        assert!(good > 10 && written > 10, "good={good} written={written}");
    }
}
