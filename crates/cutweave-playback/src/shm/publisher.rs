// SPDX-License-Identifier: GPL-3.0-or-later
//! [`FramePublisher`]: the engine side of the preview ring (WP-1H.02, arch §10).

use std::ptr::{NonNull, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering, fence};

use cutweave_model::{Duration, Time};
use thiserror::Error;

use crate::shm::clock::{ClockSample, ClockWriter};
use crate::shm::layout::{
    DEFAULT_SLOTS, FrameLayout, KNOWN_FLAGS, LAYOUT_VERSION, LayoutError, MAX_DIMENSION, MAX_SLOTS,
    MIN_SLOTS, PixelFormat,
};
use crate::shm::name::RingName;
use crate::shm::os::{ShmError, host_clock_ns};
use crate::shm::ring::{Ring, RingInit};

/// Errors creating or reconfiguring a ring.
#[derive(Debug, Error)]
pub enum RingError {
    /// Shared-memory failure.
    #[error(transparent)]
    Shm(#[from] ShmError),
    /// Slot count outside [`MIN_SLOTS`]..=[`MAX_SLOTS`].
    #[error("slot count {0} outside {MIN_SLOTS}..={MAX_SLOTS}")]
    SlotCount(u32),
    /// The planned frame layout is invalid.
    #[error(transparent)]
    Layout(#[from] LayoutError),
    /// The ring would not fit in memory.
    #[error("ring of {0} bytes per slot is too large")]
    Size(usize),
}

/// Errors publishing one frame. The ring is unchanged when one is returned.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PublishError {
    /// The frame needs more bytes than a slot holds; renegotiate with
    /// [`FramePublisher::configure`] first.
    #[error("frame of {len} bytes exceeds the slot capacity of {capacity} bytes")]
    TooLarge {
        /// Bytes the frame needs.
        len: usize,
        /// Bytes a slot holds.
        capacity: usize,
    },
    /// The source buffer is shorter than its layout says.
    #[error("source buffer holds {have} bytes, the layout needs {need}")]
    SourceTooShort {
        /// Bytes the layout needs.
        need: usize,
        /// Bytes provided.
        have: usize,
    },
    /// Flag bits that v1 does not define.
    #[error("unknown frame flags {0:#x}")]
    UnknownFlags(u32),
}

/// How to size a new ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingConfig {
    /// Number of slots, [`MIN_SLOTS`]..=[`MAX_SLOTS`] (default [`DEFAULT_SLOTS`]).
    pub slot_count: u32,
    /// Initial preview configuration; slots are sized for it (its GPU-aligned layout).
    pub preview: PreviewConfig,
    /// Row alignment of the frames the engine publishes (`FrameLayout::aligned`): 256 for
    /// wgpu readback buffers, 1 for tightly packed frames.
    pub row_align: u32,
    /// Touch every payload page at creation, so the first lap does not page-fault.
    pub prefault: bool,
}

impl RingConfig {
    /// Defaults for frames up to `max_width` x `max_height` in `format`: 6 slots, 256-byte
    /// row alignment, prefaulted. Larger frames later trigger a move to a bigger ring.
    #[must_use]
    pub fn new(max_width: u32, max_height: u32, format: PixelFormat) -> Self {
        Self {
            slot_count: DEFAULT_SLOTS,
            preview: PreviewConfig {
                width: max_width,
                height: max_height,
                format,
            },
            row_align: 256,
            prefault: true,
        }
    }
}

/// The preview frame size and format the UI asked for (`view.set_preview_size`): the canvas
/// size in device pixels (arch §10 step 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PreviewConfig {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: PixelFormat,
}

impl PreviewConfig {
    /// The configuration actually rendered: clamped to [`MAX_DIMENSION`] keeping the aspect
    /// ratio, at least 1 pixel (2 for NV12), and rounded down to even sizes for NV12.
    #[must_use]
    pub fn fitted(self) -> Self {
        let (mut w, mut h) = (u64::from(self.width.max(1)), u64::from(self.height.max(1)));
        let max = u64::from(MAX_DIMENSION);
        if w > max || h > max {
            if w >= h {
                h = (h * max / w).max(1);
                w = max;
            } else {
                w = (w * max / h).max(1);
                h = max;
            }
        }
        let (mut w, mut h) = (
            u32::try_from(w).unwrap_or(MAX_DIMENSION),
            u32::try_from(h).unwrap_or(MAX_DIMENSION),
        );
        if self.format.needs_even_size() {
            w = (w & !1).max(2);
            h = (h & !1).max(2);
        }
        Self {
            width: w,
            height: h,
            format: self.format,
        }
    }
}

/// Metadata of one published frame (geometry comes from its [`FrameLayout`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameMeta {
    /// Presentation timestamp on the timeline.
    pub pts: Time,
    /// Frame duration (zero = unknown).
    pub duration: Duration,
    /// Project revision the frame was rendered from.
    pub rev: u64,
    /// Flag bits ([`crate::shm::FLAG_LOWRES`]).
    pub flags: u32,
    /// Host time ([`crate::shm::host_clock_ns`]) the GPU work was submitted; 0 = unknown.
    pub render_ns: u64,
}

/// What the engine announces so the reader can find and verify the ring (run-file `shm`
/// field of WP-1I.01, `rpc.hello`, the spawn handshake).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingAnnouncement {
    /// Ring name (without OS prefix).
    pub name: RingName,
    /// `ring_epoch` the reader must find in the header.
    pub epoch: u64,
    /// [`LAYOUT_VERSION`].
    pub layout_version: u32,
    /// Bytes of the shared-memory object.
    pub mapping_size: u64,
}

/// Outcome of [`FramePublisher::configure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconfigured {
    /// The new size fits the current slots; only the configuration generation changed (or
    /// nothing, if the configuration was already current).
    InPlace {
        /// The configuration in effect.
        config: PreviewConfig,
        /// Its generation.
        config_gen: u32,
    },
    /// The new size needed bigger slots: frames now go to a new ring. The old ring points
    /// attached readers at it; announce the new ring to everyone else.
    Moved {
        /// The configuration in effect.
        config: PreviewConfig,
        /// Its generation.
        config_gen: u32,
        /// The new ring.
        ring: RingAnnouncement,
    },
}

/// State shared with [`ClockWriter`]s: which ring is current, and how many clock writes are
/// in flight (so a replaced ring is freed only once no clock writer can still touch it).
pub(crate) struct Shared {
    current: AtomicPtr<Ring>,
    active: AtomicUsize,
}

struct ActiveGuard<'a>(&'a AtomicUsize);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Shared {
    /// Run `f` on the current ring, or return `None` once the publisher is gone.
    pub(crate) fn with_current<R>(&self, f: impl FnOnce(&Ring) -> R) -> Option<R> {
        self.active.fetch_add(1, Ordering::SeqCst);
        let _guard = ActiveGuard(&self.active);
        let ring = self.current.load(Ordering::SeqCst);
        // SAFETY: `ring` is null or the publisher's live ring. The publisher frees a ring only
        // after swapping `current` away from it and then observing `active == 0`; because we
        // incremented `active` (SeqCst) before loading `current` (SeqCst), it cannot see 0
        // until `_guard` drops after `f` returns.
        unsafe { ring.as_ref() }.map(f)
    }

    /// Point clock writers at `next` and wait until none can still be using the old ring.
    fn replace(&self, next: *mut Ring) {
        self.current.store(next, Ordering::SeqCst);
        let mut spins = 0u32;
        while self.active.load(Ordering::SeqCst) != 0 {
            spins += 1;
            if spins < 64 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
    }
}

/// Publishes rendered preview frames into a named shared-memory ring for the Electron preload
/// (ADR-0001 transport (a)). One writer, any number of read-only readers, latest wins: the
/// writer never waits for a reader.
///
/// Layout and protocol: `RING_LAYOUT_V1` (`crates/cutweave-playback/RING.md`).
///
/// - Frames: [`publish`](Self::publish) copies a source buffer (e.g. a mapped GPU readback
///   buffer) into the next slot with one `memcpy`; [`begin_frame`](Self::begin_frame) hands
///   out the slot memory to fill directly. No allocation per frame.
/// - Audio clock: [`clock_writer`](Self::clock_writer) (any thread, ≥ 200 Hz, see
///   [`crate::shm::ClockPump`]).
/// - Liveness: every publish and [`heartbeat`](Self::heartbeat) refreshes `heartbeat_ns`;
///   call `heartbeat` at ≥ 10 Hz while idle.
/// - Size changes: [`configure`](Self::configure) bumps the configuration generation and, if
///   the slots are too small, moves to a bigger ring that the old one points to.
/// - Teardown: dropping the publisher marks the ring `Closed` and releases it (POSIX:
///   `shm_unlink`; Windows: the kernel object dies with the last handle). After a crash,
///   [`crate::shm::reap_stale_rings`] cleans up at the next start.
pub struct FramePublisher {
    ring: NonNull<Ring>,
    shared: Arc<Shared>,
    slot_count: u32,
    row_align: u32,
    prefault: bool,
    preview: PreviewConfig,
    config_gen: u32,
}

// SAFETY: the publisher exclusively owns its ring allocation (from `Box::leak`); the ring
// itself is `Send + Sync` (shared memory accessed through atomics and the seqlock protocol).
unsafe impl Send for FramePublisher {}
// SAFETY: `&self` methods only perform atomic operations on the ring or read plain fields.
unsafe impl Sync for FramePublisher {}

/// Attempts at finding a free random name before giving up.
const NAME_ATTEMPTS: u32 = 8;
/// Geometric growth factor (numerator / 2) when frames outgrow the slots.
const GROWTH_HALVES: usize = 3;

impl FramePublisher {
    /// Create a ring with a fresh random name (`cwp-<pid>-<64 random bits>`).
    pub fn create(config: &RingConfig) -> Result<Self, RingError> {
        let (slot_count, capacity, preview) = Self::plan(config)?;
        let init = RingInit {
            start_index: 0,
            frames_dropped: 0,
            config: (1, preview),
            clock: ClockSample::stopped(Time::ZERO),
            prefault: config.prefault,
        };
        let ring = create_unique(slot_count, capacity, &init)?;
        Ok(Self::from_ring(ring, config, preview))
    }

    /// Create a ring under a name chosen elsewhere (e.g. by Electron main, which then knows
    /// the name even if the engine is killed). Fails with [`ShmError::AlreadyExists`] if the
    /// name is taken.
    pub fn create_named(name: &RingName, config: &RingConfig) -> Result<Self, RingError> {
        let (slot_count, capacity, preview) = Self::plan(config)?;
        let init = RingInit {
            start_index: 0,
            frames_dropped: 0,
            config: (1, preview),
            clock: ClockSample::stopped(Time::ZERO),
            prefault: config.prefault,
        };
        let ring = Ring::create(name, slot_count, capacity, &init)?;
        Ok(Self::from_ring(ring, config, preview))
    }

    fn plan(config: &RingConfig) -> Result<(u32, usize, PreviewConfig), RingError> {
        if !(MIN_SLOTS..=MAX_SLOTS).contains(&config.slot_count) {
            return Err(RingError::SlotCount(config.slot_count));
        }
        let preview = config.preview.fitted();
        let layout = FrameLayout::aligned(
            preview.width,
            preview.height,
            preview.format,
            config.row_align,
        )?;
        Ok((config.slot_count, layout.len(), preview))
    }

    fn from_ring(ring: Ring, config: &RingConfig, preview: PreviewConfig) -> Self {
        let ring = NonNull::from(Box::leak(Box::new(ring)));
        Self {
            shared: Arc::new(Shared {
                current: AtomicPtr::new(ring.as_ptr()),
                active: AtomicUsize::new(0),
            }),
            ring,
            slot_count: config.slot_count,
            row_align: config.row_align,
            prefault: config.prefault,
            preview,
            config_gen: 1,
        }
    }

    fn ring(&self) -> &Ring {
        // SAFETY: `self.ring` comes from `Box::leak` and is freed only by `self` (in `move_to`
        // after replacing it, or in `drop`), so it is live for the duration of `&self`.
        unsafe { self.ring.as_ref() }
    }

    /// The current ring's name.
    #[must_use]
    pub fn name(&self) -> &RingName {
        self.ring().name()
    }

    /// The current ring's random epoch.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.ring().epoch()
    }

    /// Name, epoch and version to announce to readers.
    #[must_use]
    pub fn announcement(&self) -> RingAnnouncement {
        RingAnnouncement {
            name: self.name().clone(),
            epoch: self.epoch(),
            layout_version: LAYOUT_VERSION,
            mapping_size: self.ring().mapping_size() as u64,
        }
    }

    /// Number of slots.
    #[must_use]
    pub fn slot_count(&self) -> u32 {
        self.slot_count
    }

    /// Bytes one slot holds.
    #[must_use]
    pub fn slot_capacity(&self) -> usize {
        self.ring().slot_capacity()
    }

    /// The configuration in effect and its generation.
    #[must_use]
    pub fn preview(&self) -> (PreviewConfig, u32) {
        (self.preview, self.config_gen)
    }

    /// The layout the engine should render and read back for the current configuration
    /// (rows aligned to [`RingConfig::row_align`]).
    pub fn preview_layout(&self) -> Result<FrameLayout, LayoutError> {
        FrameLayout::aligned(
            self.preview.width,
            self.preview.height,
            self.preview.format,
            self.row_align,
        )
    }

    /// Frames committed so far (the `seq` of the newest frame).
    #[must_use]
    pub fn write_index(&self) -> u64 {
        self.ring().header().write_index.load(Ordering::Relaxed)
    }

    /// The OS access control of the ring object as the OS reports it (Windows SDDL, POSIX
    /// mode and owner), for logs and diagnostics.
    pub fn access_control(&self) -> Result<String, ShmError> {
        self.ring().access_description()
    }

    /// A handle that publishes the audio clock into the current ring from any thread.
    #[must_use]
    pub fn clock_writer(&self) -> ClockWriter {
        ClockWriter::new(Arc::clone(&self.shared))
    }

    /// Refresh `heartbeat_ns`. Publishing does this too; call it at ≥ 10 Hz while no frames
    /// are published so readers can tell an idle engine from a hung one.
    pub fn heartbeat(&self) {
        self.ring()
            .header()
            .heartbeat_ns
            .fetch_max(host_clock_ns(), Ordering::Relaxed);
    }

    /// Count frames the producer skipped (late renders, no free readback buffer).
    pub fn add_dropped(&self, frames: u64) {
        self.ring()
            .header()
            .frames_dropped
            .fetch_add(frames, Ordering::Relaxed);
    }

    /// Renegotiate the preview size or format (`view.set_preview_size`). The request is
    /// [`fitted`](PreviewConfig::fitted). If its aligned layout fits the current slots, only
    /// the generation changes; otherwise frames move to a new, larger ring (slots grow by at
    /// least 1.5x so a window drag does not move on every step).
    pub fn configure(&mut self, request: PreviewConfig) -> Result<Reconfigured, RingError> {
        let config = request.fitted();
        if config == self.preview {
            return Ok(Reconfigured::InPlace {
                config,
                config_gen: self.config_gen,
            });
        }
        let layout =
            FrameLayout::aligned(config.width, config.height, config.format, self.row_align)?;
        let config_gen = self.config_gen.wrapping_add(1).max(1);
        if layout.len() <= self.slot_capacity() {
            self.ring().write_config(config_gen, config);
            self.preview = config;
            self.config_gen = config_gen;
            return Ok(Reconfigured::InPlace { config, config_gen });
        }
        let grown = self.slot_capacity().saturating_mul(GROWTH_HALVES) / 2;
        self.move_to(layout.len().max(grown), config_gen, config)?;
        self.preview = config;
        self.config_gen = config_gen;
        Ok(Reconfigured::Moved {
            config,
            config_gen,
            ring: self.announcement(),
        })
    }

    /// Replace the current ring with a new one of `capacity` bytes per slot. Readers attached
    /// to the old ring find the new name in its header; `seq` and the clock carry over.
    fn move_to(
        &mut self,
        capacity: usize,
        config_gen: u32,
        config: PreviewConfig,
    ) -> Result<(), RingError> {
        let old = self.ring();
        let h = old.header();
        let init = RingInit {
            start_index: h.write_index.load(Ordering::Relaxed),
            frames_dropped: h.frames_dropped.load(Ordering::Relaxed),
            config: (config_gen, config),
            clock: old.read_clock(),
            prefault: self.prefault,
        };
        let new = create_unique(self.slot_count, capacity, &init)?;
        old.retire(new.name(), new.epoch());
        let new = NonNull::from(Box::leak(Box::new(new)));
        self.shared.replace(new.as_ptr());
        let old = std::mem::replace(&mut self.ring, new);
        // SAFETY: `old` came from `Box::leak`, is no longer reachable through `self.ring`, and
        // `replace` returned only after every clock writer left it. Dropping it unmaps the old
        // view and releases its name (attached readers keep their own mappings).
        drop(unsafe { Box::from_raw(old.as_ptr()) });
        Ok(())
    }

    /// Start writing the next frame directly into its slot. Fill
    /// [`SlotWriter::payload_mut`] according to `layout`, then [`SlotWriter::commit`].
    /// Dropping the writer without committing leaves that slot empty.
    pub fn begin_frame(
        &mut self,
        meta: &FrameMeta,
        layout: &FrameLayout,
    ) -> Result<SlotWriter<'_>, PublishError> {
        if meta.flags & !KNOWN_FLAGS != 0 {
            return Err(PublishError::UnknownFlags(meta.flags & !KNOWN_FLAGS));
        }
        let ring = self.ring();
        if layout.len() > ring.slot_capacity() {
            return Err(PublishError::TooLarge {
                len: layout.len(),
                capacity: ring.slot_capacity(),
            });
        }
        let write_index = ring.header().write_index.load(Ordering::Relaxed);
        let seq = write_index + 1;
        let slot = u32::try_from(write_index % u64::from(self.slot_count)).unwrap_or(0);
        let d = ring.desc(slot);
        // `| 1` keeps the word odd even if an earlier SlotWriter was leaked mid-write.
        let odd = d.lock.load(Ordering::Relaxed) | 1;
        d.lock.store(odd, Ordering::Relaxed);
        // Orders the odd lock word before every payload and descriptor store that follows.
        fence(Ordering::Release);
        Ok(SlotWriter {
            ring,
            slot,
            odd,
            seq,
            layout: *layout,
            meta: *meta,
            config_gen: self.config_gen,
            committed: false,
        })
    }

    /// Publish one frame whose bytes are in `src` (laid out as `layout`, e.g. a mapped GPU
    /// readback buffer). One `memcpy` of `layout.len()` bytes. Returns the frame's `seq`.
    pub fn publish(
        &mut self,
        meta: &FrameMeta,
        layout: &FrameLayout,
        src: &[u8],
    ) -> Result<u64, PublishError> {
        let Some(src) = src.get(..layout.len()) else {
            return Err(PublishError::SourceTooShort {
                need: layout.len(),
                have: src.len(),
            });
        };
        let mut slot = self.begin_frame(meta, layout)?;
        slot.payload_mut().copy_from_slice(src);
        Ok(slot.commit())
    }

    /// Mark the ring closed and release it (same as dropping the publisher).
    pub fn close(self) {
        drop(self);
    }
}

impl Drop for FramePublisher {
    fn drop(&mut self) {
        self.ring().mark_closed();
        self.shared.replace(null_mut());
        // SAFETY: `self.ring` came from `Box::leak` and is freed exactly once, here, after
        // `replace` guaranteed no clock writer still uses it.
        drop(unsafe { Box::from_raw(self.ring.as_ptr()) });
    }
}

fn create_unique(slot_count: u32, capacity: usize, init: &RingInit) -> Result<Ring, RingError> {
    let mut attempt = 0;
    loop {
        let name = RingName::generate()?;
        match Ring::create(&name, slot_count, capacity, init) {
            Err(RingError::Shm(ShmError::AlreadyExists(_))) if attempt + 1 < NAME_ATTEMPTS => {
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// A slot being written; see [`FramePublisher::begin_frame`].
pub struct SlotWriter<'a> {
    ring: &'a Ring,
    slot: u32,
    odd: u64,
    seq: u64,
    layout: FrameLayout,
    meta: FrameMeta,
    config_gen: u32,
    committed: bool,
}

impl SlotWriter<'_> {
    /// The frame's payload bytes (`layout.len()` of them); plane `i` starts at
    /// `layout.planes()[i].offset`. Previous contents are unspecified.
    pub fn payload_mut(&mut self) -> &mut [u8] {
        // SAFETY: `layout.len() <= slot_capacity` (checked in `begin_frame`), so the slice is
        // inside slot `self.slot`'s payload. This process holds the only writable view and
        // `begin_frame` borrowed the publisher mutably, so nothing else here aliases it.
        // Readers live in other mappings (other processes) and detect concurrent writes with
        // the slot seqlock, which is odd until `commit`.
        unsafe {
            std::slice::from_raw_parts_mut(self.ring.payload_ptr(self.slot), self.layout.len())
        }
    }

    /// The `seq` this frame gets on commit.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Make the frame visible to readers. Returns its `seq`.
    #[allow(clippy::must_use_candidate)] // Committing is the point; the seq is optional.
    pub fn commit(mut self) -> u64 {
        let d = self.ring.desc(self.slot);
        let l = &self.layout;
        let [p0, p1] = l.planes();
        let relaxed = Ordering::Relaxed;
        d.seq.store(self.seq, relaxed);
        d.pts.store(self.meta.pts.flicks(), relaxed);
        d.duration.store(self.meta.duration.flicks(), relaxed);
        d.rev.store(self.meta.rev, relaxed);
        d.width.store(l.width(), relaxed);
        d.height.store(l.height(), relaxed);
        d.format.store(l.format() as u32, relaxed);
        d.flags.store(self.meta.flags, relaxed);
        d.config_gen.store(self.config_gen, relaxed);
        d.payload_len.store(l.len() as u64, relaxed);
        d.plane0_offset.store(p0.offset as u64, relaxed);
        d.plane1_offset.store(p1.offset as u64, relaxed);
        d.plane0_stride.store(p0.stride, relaxed);
        d.plane1_stride.store(p1.stride, relaxed);
        d.render_ns.store(self.meta.render_ns, relaxed);
        let now = host_clock_ns();
        d.publish_ns.store(now, relaxed);
        // Slot stable (Release: every store above happens-before a reader's Acquire of it) ...
        d.lock.store(self.odd + 1, Ordering::Release);
        // ... then advertised as the newest frame.
        let h = self.ring.header();
        h.write_index.store(self.seq, Ordering::Release);
        h.heartbeat_ns.fetch_max(now, relaxed);
        self.committed = true;
        self.seq
    }
}

impl Drop for SlotWriter<'_> {
    fn drop(&mut self) {
        if !self.committed {
            // The old frame in this slot was partly overwritten: leave the slot empty.
            let d = self.ring.desc(self.slot);
            d.seq.store(0, Ordering::Relaxed);
            d.payload_len.store(0, Ordering::Relaxed);
            d.lock.store(self.odd + 1, Ordering::Release);
        }
    }
}
