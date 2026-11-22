//! `s1-bridge`: napi-rs v3 addon that maps the S1 frame ring (arch §10 step 2, transport (a)).
//!
//! Loaded by the main window's non-sandboxed preload (and by Node tests). Exposes:
//! - `RingReader.open(name)`: map the ring read-only;
//! - `readHeader()`: header + every slot descriptor;
//! - `readLatestInto(dst, newerThan)`: seqlock-validated copy of the newest frame into a
//!   caller-owned `Uint8Array` (the path that works in Electron);
//! - `latestView()`: an `ArrayBuffer` over the mapped slot itself. Zero-copy in Node; Electron's
//!   V8 memory cage forbids external buffers, so napi-rs silently copies there and the result
//!   says `zeroCopy: false` (spike finding);
//! - `waitForFrame(after, timeoutMs)`: promise that resolves when a newer frame is published
//!   (transport (b), WP-0C.06: the addon then runs in an Electron utility process);
//! - `TestRingWriter`: a writer for hermetic Node tests (no GPU needed).

#![allow(clippy::needless_pass_by_value)] // napi signatures take owned values

mod wait;

use std::sync::Arc;
use std::time::{Duration, Instant};

use napi::bindgen_prelude::{ArrayBuffer, AsyncTask, Uint8ArraySlice};
use napi::{Env, Error, Result, Status};
use napi_derive::napi;
use s1_ring::{
    FrameMeta, PixelFormat, ReadError, RingReader as Reader, RingWriter, SharedMemory, SlotMeta,
};

fn err(e: impl std::fmt::Display) -> Error {
    Error::new(Status::GenericFailure, e.to_string())
}

#[allow(clippy::cast_precision_loss)]
fn n(v: u64) -> f64 {
    v as f64
}

/// Descriptor of one frame (all 64-bit values as JS numbers; exact below 2^53).
#[napi(object)]
pub struct FrameHeader {
    pub slot: u32,
    pub seq: f64,
    /// pts in flicks.
    pub pts: f64,
    pub width: u32,
    pub height: u32,
    /// 1 = RGBA8, 2 = NV12.
    pub format: u32,
    pub stride: u32,
    pub rev: f64,
    pub payload_len: f64,
    pub checksum: u32,
    /// Host µs (Unix epoch) at GPU submit in the engine.
    pub t_render_us: f64,
    /// Host µs at slot publish.
    pub t_publish_us: f64,
    /// Seqlock value at read time.
    pub lock: f64,
    /// Time spent in the addon copying this frame (µs), 0 for header-only reads.
    pub read_us: f64,
}

fn header(m: &SlotMeta, read_us: f64) -> FrameHeader {
    FrameHeader {
        slot: m.slot,
        seq: n(m.seq),
        #[allow(clippy::cast_precision_loss)]
        pts: m.pts as f64,
        width: m.width,
        height: m.height,
        format: m.format,
        stride: m.stride,
        rev: n(m.rev),
        payload_len: n(m.payload_len),
        checksum: m.checksum,
        t_render_us: n(m.t_render_us),
        t_publish_us: n(m.t_publish_us),
        lock: n(m.lock),
        read_us,
    }
}

/// Ring header snapshot.
#[napi(object)]
pub struct RingHeader {
    pub slot_count: u32,
    pub slot_capacity: f64,
    pub write_index: f64,
    pub audio_clock_flicks: f64,
    pub audio_clock_host_us: f64,
    pub heartbeat_us: f64,
    pub frame_duration_flicks: f64,
    pub engine_pid: u32,
    /// 0 starting, 1 playing, 2 paused, 3 stopped.
    pub engine_state: u32,
    pub engine_dropped: f64,
    /// `null` for a slot that was mid-write.
    pub slots: Vec<Option<FrameHeader>>,
}

/// A frame exposed as an `ArrayBuffer` over (ideally) the shared memory itself.
#[napi(object)]
pub struct FrameView<'env> {
    pub header: FrameHeader,
    pub buffer: ArrayBuffer<'env>,
    /// True if `buffer` aliases the shared memory; false if the runtime forced a copy.
    pub zero_copy: bool,
}

/// Reader counters.
#[napi(object)]
pub struct ReaderStats {
    pub frames: f64,
    pub torn: f64,
    pub nothing_new: f64,
}

/// Read-only mapping of a frame ring.
#[napi]
pub struct RingReader {
    inner: Option<(Arc<SharedMemory>, Reader<Arc<SharedMemory>>)>,
    frames: u64,
    torn: u64,
    nothing_new: u64,
}

#[napi]
impl RingReader {
    /// Map an existing ring by name (read-only).
    #[napi(factory)]
    pub fn open(name: String) -> Result<Self> {
        let shm = Arc::new(SharedMemory::open(&name, false).map_err(err)?);
        let reader = Reader::attach(Arc::clone(&shm)).map_err(err)?;
        Ok(Self {
            inner: Some((shm, reader)),
            frames: 0,
            torn: 0,
            nothing_new: 0,
        })
    }

    fn reader(&self) -> Result<&Reader<Arc<SharedMemory>>> {
        self.inner
            .as_ref()
            .map(|(_, r)| r)
            .ok_or_else(|| err("ring reader is closed"))
    }

    #[napi(getter)]
    pub fn closed(&self) -> bool {
        self.inner.is_none()
    }

    #[napi(getter)]
    pub fn slot_count(&self) -> Result<u32> {
        Ok(self.reader()?.slot_count())
    }

    #[napi(getter)]
    pub fn capacity(&self) -> Result<f64> {
        Ok(n(self.reader()?.capacity() as u64))
    }

    /// Header and all slot descriptors.
    #[napi]
    pub fn read_header(&self) -> Result<RingHeader> {
        let s = self.reader()?.snapshot();
        Ok(RingHeader {
            slot_count: s.slot_count,
            slot_capacity: n(s.slot_capacity),
            write_index: n(s.write_index),
            #[allow(clippy::cast_precision_loss)]
            audio_clock_flicks: s.audio_clock_flicks as f64,
            audio_clock_host_us: n(s.audio_clock_host_us),
            heartbeat_us: n(s.heartbeat_us),
            #[allow(clippy::cast_precision_loss)]
            frame_duration_flicks: s.frame_duration_flicks as f64,
            engine_pid: s.engine_pid,
            engine_state: s.engine_state,
            engine_dropped: n(s.engine_dropped),
            slots: s
                .slots
                .iter()
                .map(|m| m.as_ref().map(|m| header(m, 0.0)))
                .collect(),
        })
    }

    /// Descriptor of the newest frame without copying it (`null` if none).
    #[napi]
    pub fn latest_header(&self) -> Result<Option<FrameHeader>> {
        let r = self.reader()?;
        Ok(r.latest_slot()
            .and_then(|s| r.slot_meta(s).ok())
            .map(|m| header(&m, 0.0)))
    }

    /// Copy the newest frame into `dst` if its seq is greater than `newer_than`.
    /// Returns `null` if there is nothing new (or the copy was torn after retries; counted).
    #[napi]
    pub fn read_latest_into(
        &mut self,
        mut dst: Uint8ArraySlice,
        newer_than: f64,
    ) -> Result<Option<FrameHeader>> {
        let t0 = Instant::now();
        // SAFETY: we write into the JS-owned typed array on the JS thread; JS cannot run
        // concurrently with this call.
        let dst = unsafe { dst.as_mut() };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let newer = newer_than.max(0.0) as u64;
        let res = self.reader()?.read_latest_into(dst, newer);
        match res {
            Ok(Some(m)) => {
                self.frames += 1;
                Ok(Some(header(&m, t0.elapsed().as_secs_f64() * 1e6)))
            }
            Ok(None) | Err(ReadError::Empty) => {
                self.nothing_new += 1;
                Ok(None)
            }
            Err(ReadError::Torn(_)) => {
                self.torn += 1;
                Ok(None)
            }
            Err(e) => Err(err(e)),
        }
    }

    /// Copy slot `slot` into `dst` (seqlock-validated). Throws if torn.
    #[napi]
    pub fn read_slot_into(&mut self, slot: u32, mut dst: Uint8ArraySlice) -> Result<FrameHeader> {
        let t0 = Instant::now();
        // SAFETY: as in read_latest_into.
        let dst = unsafe { dst.as_mut() };
        let m = self.reader()?.read_slot_into(slot, dst).map_err(err)?;
        self.frames += 1;
        Ok(header(&m, t0.elapsed().as_secs_f64() * 1e6))
    }

    /// The newest frame as an `ArrayBuffer` aliasing the mapping when the runtime allows it.
    /// The view is READ-ONLY (writing to it faults) and only trustworthy while
    /// `isValid(view.header)` is true; it keeps the mapping alive until garbage-collected.
    #[napi]
    pub fn latest_view<'env>(&self, env: &'env Env) -> Result<Option<FrameView<'env>>> {
        let (shm, r) = self
            .inner
            .as_ref()
            .ok_or_else(|| err("ring reader is closed"))?;
        let Some(slot) = r.latest_slot() else {
            return Ok(None);
        };
        let m = r.slot_meta(slot).map_err(err)?;
        let len = usize::try_from(m.payload_len).map_err(err)?;
        if len == 0 {
            return Ok(None);
        }
        let ptr = r.payload_ptr(slot).cast_mut();
        let keep_alive = Arc::clone(shm);
        // SAFETY: `ptr..ptr+len` lies inside the mapping, which `keep_alive` keeps mapped until
        // the finalizer runs. JS must treat it as read-only (documented).
        let buffer = unsafe {
            ArrayBuffer::from_external(env, ptr, len, keep_alive, |_env, hint| drop(hint))
        }?;
        let zero_copy = std::ptr::eq(buffer.as_ptr(), ptr);
        Ok(Some(FrameView {
            header: header(&m, 0.0),
            buffer,
            zero_copy,
        }))
    }

    /// True if the frame described by `h` is still in its slot (seqlock unchanged).
    #[napi]
    pub fn is_valid(&self, h: FrameHeader) -> Result<bool> {
        let r = self.reader()?;
        let meta = r.slot_meta(h.slot).map_err(err)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(meta.lock == h.lock as u64 && r.still_valid(&meta))
    }

    /// Transport (b): resolve with the write index once it exceeds `after_write_index`, or
    /// after `timeout_ms` (resolves with the unchanged index). Polls every `poll_us` µs
    /// (default 250) on a libuv worker thread; see `wait.rs`. The pending task keeps the
    /// mapping alive until it resolves, so callers should await it before reporting the ring
    /// closed.
    #[napi(ts_return_type = "Promise<number>")]
    pub fn wait_for_frame(
        &self,
        after_write_index: f64,
        timeout_ms: u32,
        poll_us: Option<u32>,
    ) -> Result<AsyncTask<wait::WaitTask>> {
        let (shm, _) = self
            .inner
            .as_ref()
            .ok_or_else(|| err("ring reader is closed"))?;
        let reader = Reader::attach(Arc::clone(shm)).map_err(err)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let after = after_write_index.max(0.0) as u64;
        Ok(AsyncTask::new(wait::WaitTask::new(
            reader,
            after,
            Duration::from_millis(u64::from(timeout_ms)),
            Duration::from_micros(u64::from(poll_us.unwrap_or(250).max(50))),
        )))
    }

    #[napi]
    pub fn stats(&self) -> ReaderStats {
        ReaderStats {
            frames: n(self.frames),
            torn: n(self.torn),
            nothing_new: n(self.nothing_new),
        }
    }

    /// Unmap and close the handle (views created by `latestView` keep it alive until GC).
    #[napi]
    pub fn close(&mut self) {
        self.inner = None;
    }
}

/// True if a ring with this name is still held open by some process.
#[napi]
pub fn ring_exists(name: String) -> bool {
    SharedMemory::exists(&name)
}

/// Host clock shared with the engine: µs since the Unix epoch.
#[napi]
pub fn now_us() -> f64 {
    n(s1_ring::clock::now_us())
}

/// Rust reference implementation of the sampled checksum, for cross-checking the TS mirror.
#[napi]
pub fn checksum(data: Uint8ArraySlice, len: u32) -> u32 {
    let d: &[u8] = &data;
    s1_ring::checksum::sampled_fnv1a(&d[..(len as usize).min(d.len())])
}

/// Ring writer for hermetic tests (no engine, no GPU).
#[napi]
pub struct TestRingWriter {
    inner: Option<RingWriter<SharedMemory>>,
    width: u32,
    height: u32,
}

#[napi]
impl TestRingWriter {
    /// Create a ring sized for `width` x `height` RGBA8 frames.
    #[napi(factory)]
    pub fn create(name: String, slots: u32, width: u32, height: u32) -> Result<Self> {
        let cap = PixelFormat::Rgba8.frame_len(width, height);
        let shm =
            SharedMemory::create(&name, s1_ring::layout::mapping_len(slots, cap)).map_err(err)?;
        let w = RingWriter::init(shm, slots, cap, s1_ring::FLICKS_PER_SECOND / 60).map_err(err)?;
        Ok(Self {
            inner: Some(w),
            width,
            height,
        })
    }

    fn w(&mut self) -> Result<&mut RingWriter<SharedMemory>> {
        self.inner.as_mut().ok_or_else(|| err("writer closed"))
    }

    /// Publish a frame whose pixels are all `fill` (before row stamping). Returns its seq.
    #[napi]
    pub fn publish(&mut self, format: u32, pts: f64, fill: u32) -> Result<f64> {
        let format = PixelFormat::from_u32(format).ok_or_else(|| err("bad format"))?;
        let (width, height) = (self.width, self.height);
        let len = format.frame_len(width, height);
        #[allow(clippy::cast_possible_truncation)]
        let meta = FrameMeta {
            pts: pts as i64,
            width,
            height,
            format,
            rev: 1,
            t_render_us: s1_ring::clock::now_us(),
        };
        #[allow(clippy::cast_possible_truncation)]
        let byte = fill as u8;
        let seq = self
            .w()?
            .publish(&meta, |buf| {
                buf[..len].fill(byte);
                len
            })
            .map_err(err)?;
        Ok(n(seq))
    }

    /// Leave `slot` marked as being written.
    #[napi]
    pub fn begin_write(&mut self, slot: u32) -> Result<()> {
        self.w()?.begin_write_for_test(slot);
        Ok(())
    }

    /// Finish a `beginWrite`.
    #[napi]
    pub fn end_write(&mut self, slot: u32) -> Result<()> {
        self.w()?.end_write_for_test(slot);
        Ok(())
    }

    /// Flip one payload byte without touching the seqlock.
    #[napi]
    pub fn corrupt(&mut self, slot: u32, offset: u32) -> Result<()> {
        self.w()?.corrupt_payload_for_test(slot, offset as usize);
        Ok(())
    }

    #[napi]
    pub fn close(&mut self) {
        self.inner = None;
    }
}
