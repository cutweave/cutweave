// SPDX-License-Identifier: GPL-3.0-or-later
//! The Node-API surface (`napi-rs` v3): `openRing`, `Ring`, `perfCounters`. A thin layer over
//! [`RingSession`]; the protocol logic lives in `reader.rs` and `session.rs`.
//!
//! Electron's V8 memory cage forbids external `ArrayBuffer`s over shared memory, so every frame
//! is copied once, straight from the mapping into a V8 `ArrayBuffer`. Those buffers are pooled
//! (two per frame size, alternating): the previous frame's bytes stay valid while the next
//! frame is read, and steady-state playback allocates nothing.

// JS numbers are f64: flicks, ns and sequence numbers stay far below 2^53, so the casts are exact.
#![allow(clippy::cast_precision_loss)]
// `Env` is a pointer-sized handle that napi-rs methods take by reference.
#![allow(clippy::trivially_copy_pass_by_ref)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use napi::bindgen_prelude::{ArrayBuffer, AsyncTask, BigInt, Either, FromNapiValue, ObjectRef};
use napi::{Env, Error, JsValue, Result, Status, Task};
use napi_derive::napi;

use cutweave_playback::shm::layout::{ClockState, FLICKS_PER_SECOND, LAYOUT_VERSION, WriterState};

use crate::error::BridgeError;
use crate::reader::{FrameBuffer, RingReader, SlotMeta, now_ns};
use crate::session::{EndReason, RingSession, SessionState};

/// Distinct frame sizes whose buffers stay pooled (full size, low-res scrub size, the size
/// before a resize, one spare).
const MAX_POOLED_SIZES: usize = 4;
/// Longest `waitForFrame` timeout.
const MAX_WAIT_MS: u32 = 60_000;
/// Poll period of the wait task: a sub-millisecond sleep (Rust's `thread::sleep` uses a
/// high-resolution waitable timer on Windows; a JS `setTimeout` wakes every ~15 ms there).
const WAIT_POLL: Duration = Duration::from_micros(250);

static FRAMES_READ: AtomicU64 = AtomicU64::new(0);
static BYTES_COPIED: AtomicU64 = AtomicU64::new(0);
static SUCCESSOR_SWITCHES: AtomicU64 = AtomicU64::new(0);
static RINGS_OPENED: AtomicU64 = AtomicU64::new(0);
static TORN_RETRIES: AtomicU64 = AtomicU64::new(0);

fn js_err(e: &BridgeError) -> Error {
    // The code leads the message so callers can also match on `message` (`ERR_RING_EPOCH: ...`).
    Error::new(Status::GenericFailure, format!("{}: {e}", e.code()))
}

fn n(v: u64) -> f64 {
    v as f64
}

/// Parse an epoch given as a `bigint` or as a decimal / `0x` hex string.
fn parse_epoch(v: Either<BigInt, String>) -> Result<u64> {
    let bad = |what: &str| Error::new(Status::InvalidArg, format!("ERR_RING_EPOCH: {what}"));
    match v {
        Either::A(big) => {
            let (signed, value, lossless) = big.get_u64();
            if signed || !lossless {
                return Err(bad("epoch must fit in an unsigned 64-bit integer"));
            }
            Ok(value)
        }
        Either::B(s) => {
            let s = s.trim();
            let parsed = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                Some(hex) => u64::from_str_radix(hex, 16),
                None => s.parse::<u64>(),
            };
            parsed.map_err(|_| bad("epoch must be a decimal or 0x-prefixed hex u64"))
        }
    }
}

/// Static facts about an opened ring.
#[napi(object)]
pub struct RingInfo {
    /// Ring name (without OS prefix).
    pub name: String,
    /// Ring epoch, `0x` + 16 hex digits.
    pub epoch: String,
    /// Engine process id.
    pub writer_pid: u32,
    /// `layout_version` (1).
    pub layout_version: u32,
    /// Number of slots.
    pub slot_count: u32,
    /// Largest frame payload in bytes.
    pub slot_capacity: f64,
    /// Bytes mapped.
    pub mapping_size: f64,
}

/// Liveness and progress of the ring.
#[napi(object)]
pub struct RingStatus {
    /// `live`, `starting` (writer not live yet) or `ended` (ring released; see `reason`).
    pub state: String,
    /// True while `state` is `live`.
    pub alive: bool,
    /// Why the ring ended: `writer-closed`, `heartbeat-stale` (no heartbeat for 500 ms),
    /// `successor-gone` (ask the engine for the current ring), `successor-rejected`, `closed`.
    pub reason: Option<String>,
    /// `ERR_RING_*` code of a rejected successor.
    pub reason_code: Option<String>,
    /// Current ring name, while there is one.
    pub name: Option<String>,
    /// Milliseconds since the writer's last heartbeat.
    pub heartbeat_age_ms: Option<f64>,
    /// Newest `seq` the writer has committed.
    pub write_index: f64,
    /// Newest `seq` handed to JS.
    pub last_seq: f64,
    /// Times the ring moved to a successor.
    pub switches: u32,
    /// Slot copies discarded because the writer reused the slot meanwhile.
    pub torn_retries: f64,
    /// Producer-side frame drops (diagnostics).
    pub frames_dropped: f64,
    /// Configuration generation, preview size and format the engine negotiated.
    pub config_gen: u32,
    /// Negotiated preview width in device pixels.
    pub preview_width: u32,
    /// Negotiated preview height.
    pub preview_height: u32,
}

/// One preview frame. `bytes` is the slot payload exactly as published (rows may be padded:
/// use the strides) in a pooled buffer that stays valid until the second `latestFrame` /
/// `frameAtClock` call after this one.
#[napi(object)]
pub struct PreviewFrame<'env> {
    /// Presentation timestamp (flicks).
    pub pts: f64,
    /// Frame duration (flicks), 0 = unknown.
    pub duration: f64,
    /// Sequence number: strictly increasing, also across successor rings.
    pub seq: f64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `rgba8` or `nv12`.
    pub format: String,
    /// The pixels (`byteLength` = payload length).
    pub bytes: ArrayBuffer<'env>,
    /// Bytes per row of plane 0.
    pub stride: u32,
    /// Offset of plane 1 (NV12 UV), 0 for RGBA8.
    pub plane1_offset: u32,
    /// Bytes per row of plane 1, 0 for RGBA8.
    pub plane1_stride: u32,
    /// Reduced-size scrub preview; a full-size frame of the same `pts` follows.
    pub lowres: bool,
    /// Project revision the frame was rendered from.
    pub rev: f64,
    /// Configuration generation the frame was rendered for.
    pub config_gen: u32,
    /// Milliseconds from the engine's GPU submit to now (0 = unknown).
    pub render_age_ms: f64,
    /// Milliseconds from the frame's commit in the ring to now.
    pub publish_age_ms: f64,
}

/// The audio clock, extrapolated to now.
#[napi(object)]
pub struct AudioClock {
    /// Timeline position now, in flicks (advanced from the last sample while playing).
    pub pts_flicks: f64,
    /// `stopped`, `playing` or `scrubbing`.
    pub state: String,
    /// Playback rate in thousandths (1000 = 1x, negative = reverse).
    pub rate_milli: i32,
    /// Milliseconds since the engine's last clock sample.
    pub age_ms: f64,
}

/// Process-wide counters (stub for the perf overlay; WP-1M later).
#[napi(object)]
pub struct PerfCounters {
    /// Rings opened.
    pub rings_opened: f64,
    /// Frames copied out of shared memory.
    pub frames_read: f64,
    /// Bytes copied out of shared memory.
    pub bytes_copied: f64,
    /// Slot copies discarded because the writer reused the slot meanwhile.
    pub torn_retries: f64,
    /// Moves to a successor ring.
    pub successor_switches: f64,
}

/// Pooled V8 buffers of one frame size.
struct SizeClass {
    len: usize,
    bufs: [Option<ObjectRef<false>>; 2],
    next: usize,
}

/// Buffer pool: up to `MAX_POOLED_SIZES` sizes, two alternating buffers each.
#[derive(Default)]
struct Pool {
    classes: Vec<SizeClass>,
}

impl Pool {
    /// The next buffer for frames of `len` bytes, creating it on first use.
    fn acquire(&mut self, env: &Env, len: usize) -> Result<ArrayBuffer<'static>> {
        let idx = if let Some(i) = self.classes.iter().position(|c| c.len == len) {
            i
        } else {
            if self.classes.len() >= MAX_POOLED_SIZES {
                let mut old = self.classes.remove(0);
                old.release(env);
            }
            self.classes.push(SizeClass {
                len,
                bufs: [None, None],
                next: 0,
            });
            self.classes.len() - 1
        };
        let class = &mut self.classes[idx];
        let slot = class.next;
        if class.bufs[slot].is_none() {
            let fresh = ArrayBuffer::copy_from(env, vec![0u8; len])?;
            // SAFETY: `fresh` is a live napi value of this env; the reference keeps the buffer
            // alive until `release` deletes it.
            class.bufs[slot] =
                Some(unsafe { ObjectRef::<false>::from_napi_value(env.raw(), fresh.raw())? });
        }
        let Some(r) = &class.bufs[slot] else {
            return Err(Error::new(Status::GenericFailure, "buffer pool empty"));
        };
        let object = r.get_value(env)?;
        // SAFETY: the referenced value is the `ArrayBuffer` created above; the `'static` is
        // sound because the pool's reference keeps the buffer alive for as long as the pool
        // (and every `Ring` call that uses the value) exists.
        unsafe { ArrayBuffer::<'static>::from_napi_value(env.raw(), object.raw()) }
    }

    /// Flip to the other buffer of `len`'s class (after a frame was delivered).
    fn flip(&mut self, len: usize) {
        if let Some(c) = self.classes.iter_mut().find(|c| c.len == len) {
            c.next ^= 1;
        }
    }

    fn release_all(&mut self, env: &Env) {
        for c in &mut self.classes {
            c.release(env);
        }
        self.classes.clear();
    }
}

impl SizeClass {
    fn release(&mut self, env: &Env) {
        for b in &mut self.bufs {
            if let Some(r) = b.take() {
                // Failing to unref only leaks one small buffer; nothing to recover.
                let _ = r.unref(env);
            }
        }
    }
}

/// [`FrameBuffer`] over the pool: hands out the next pooled `ArrayBuffer` of the right size.
struct PoolBuffer<'a> {
    env: &'a Env,
    pool: &'a mut Pool,
    current: Option<ArrayBuffer<'static>>,
    error: Option<Error>,
}

impl FrameBuffer for PoolBuffer<'_> {
    fn buffer(&mut self, len: usize) -> std::result::Result<&mut [u8], BridgeError> {
        match self.pool.acquire(self.env, len) {
            Ok(ab) => {
                let slot = self.current.insert(ab);
                // SAFETY: JS is not running while this native call executes, and the pooled
                // buffer is not the one the previous frame object handed out (alternating
                // pair), so nothing else reads or writes these bytes meanwhile.
                Ok(unsafe { slot.as_mut() })
            }
            Err(e) => {
                let msg = e.to_string();
                self.error = Some(e);
                Err(BridgeError::Buffer(msg))
            }
        }
    }
}

fn state_name(c: ClockState) -> &'static str {
    match c {
        ClockState::Stopped => "stopped",
        ClockState::Playing => "playing",
        ClockState::Scrubbing => "scrubbing",
    }
}

fn format_name(f: cutweave_playback::shm::layout::PixelFormat) -> &'static str {
    use cutweave_playback::shm::layout::PixelFormat;
    match f {
        PixelFormat::Rgba8 => "rgba8",
        PixelFormat::Nv12 => "nv12",
    }
}

/// Handle to the preview ring: follows reconfiguration, tracks the writer's liveness and hands
/// out frames, newest first. All methods are cheap and non-blocking except `waitForFrame`,
/// which is asynchronous.
#[napi]
pub struct Ring {
    session: RingSession,
    pool: Pool,
    torn_reported: u64,
    switches_reported: u32,
}

#[napi]
impl Ring {
    /// Static facts about the ring currently mapped.
    #[napi]
    pub fn header(&self) -> Result<RingInfo> {
        let r = self
            .session
            .reader()
            .ok_or_else(|| js_err(&BridgeError::Closed))?;
        Ok(RingInfo {
            name: r.name().to_string(),
            epoch: format!("{:#018x}", r.epoch()),
            writer_pid: r.writer_pid(),
            layout_version: LAYOUT_VERSION,
            slot_count: r.slot_count(),
            slot_capacity: n(r.slot_capacity() as u64),
            mapping_size: n(r.mapping_size() as u64),
        })
    }

    /// Liveness verdict and progress. Also follows a retired ring to its successor and
    /// releases the mapping once the writer is gone (closed, or no heartbeat for 500 ms).
    #[napi]
    pub fn status(&mut self) -> RingStatus {
        let s = self.session.status();
        self.sync_counters();
        let (state, reason, reason_code) = match &s.state {
            SessionState::Live => ("live", None, None),
            SessionState::Starting => ("starting", None, None),
            SessionState::Ended(r) => (
                "ended",
                Some(r.as_str().to_owned()),
                match r {
                    EndReason::SuccessorRejected(code) => Some((*code).to_owned()),
                    _ => None,
                },
            ),
        };
        let reader = self.session.reader();
        let cfg = reader.and_then(|r| r.config());
        RingStatus {
            state: state.to_owned(),
            alive: s.state == SessionState::Live,
            reason,
            reason_code,
            name: s.name,
            heartbeat_age_ms: s.heartbeat_age_ms,
            write_index: n(s.write_index),
            last_seq: n(s.last_seq),
            switches: s.switches,
            torn_retries: n(reader.map_or(0, |r| r.torn_retries())),
            frames_dropped: n(reader.map_or(0, |r| r.frames_dropped())),
            config_gen: cfg.map_or(0, |c| c.config_gen),
            preview_width: cfg.map_or(0, |c| c.width),
            preview_height: cfg.map_or(0, |c| c.height),
        }
    }

    /// The newest frame not returned yet, or `null`. With `maxPtsFlicks`, only frames whose
    /// `pts` is at or before it are considered (the newest of those wins).
    #[napi]
    pub fn latest_frame<'env>(
        &mut self,
        env: &'env Env,
        max_pts_flicks: Option<f64>,
    ) -> Result<Option<PreviewFrame<'env>>> {
        #[allow(clippy::cast_possible_truncation)]
        let max_pts = max_pts_flicks.filter(|p| p.is_finite()).map(|p| p as i64);
        let mut buf = PoolBuffer {
            env,
            pool: &mut self.pool,
            current: None,
            error: None,
        };
        let got = self.session.read(max_pts, &mut buf);
        Self::finish(env, got, buf)
    }

    /// The clock-scheduled frame (arch §10 step 3): the newest not-yet-returned frame with
    /// `pts <= audio clock now - latencyMs`. Without a clock sample it is `latestFrame()`.
    /// Presenting this choice instead of "newest wins" gave 60.00 fps with no gaps in Spike S1.
    #[napi]
    pub fn frame_at_clock<'env>(
        &mut self,
        env: &'env Env,
        latency_ms: Option<f64>,
    ) -> Result<Option<PreviewFrame<'env>>> {
        let latency = latency_ms.filter(|l| l.is_finite()).unwrap_or(0.0).max(0.0);
        #[allow(clippy::cast_possible_truncation)]
        let latency_flicks = (latency * FLICKS_PER_SECOND as f64 / 1000.0) as i64;
        let mut buf = PoolBuffer {
            env,
            pool: &mut self.pool,
            current: None,
            error: None,
        };
        let got = self.session.read_at_clock(latency_flicks, &mut buf);
        Self::finish(env, got, buf)
    }

    fn finish<'env>(
        env: &'env Env,
        got: std::result::Result<Option<(SlotMeta, std::sync::Arc<RingReader>)>, BridgeError>,
        mut buf: PoolBuffer<'_>,
    ) -> Result<Option<PreviewFrame<'env>>> {
        let (meta, _) = match got {
            Ok(Some(x)) => x,
            Ok(None) => return Ok(None),
            Err(e) => return Err(buf.error.take().unwrap_or_else(|| js_err(&e))),
        };
        let Some(current) = buf.current else {
            return Err(Error::new(Status::GenericFailure, "frame buffer missing"));
        };
        let len = meta.layout.len();
        buf.pool.flip(len);
        FRAMES_READ.fetch_add(1, Ordering::Relaxed);
        BYTES_COPIED.fetch_add(len as u64, Ordering::Relaxed);
        let now = now_ns();
        let age_ms = |t: u64| {
            if t == 0 {
                0.0
            } else {
                now.saturating_sub(t) as f64 / 1e6
            }
        };
        let [p0, p1] = meta.layout.planes();
        let to_u32 = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
        // `'static` -> `'env`: the pooled buffer is also alive through the pool's reference.
        let bytes: ArrayBuffer<'env> = current;
        let _ = env;
        Ok(Some(PreviewFrame {
            pts: meta.pts as f64,
            duration: meta.duration as f64,
            seq: n(meta.seq),
            width: meta.layout.width(),
            height: meta.layout.height(),
            format: format_name(meta.layout.format()).to_owned(),
            bytes,
            stride: p0.stride,
            plane1_offset: to_u32(p1.offset),
            plane1_stride: p1.stride,
            lowres: meta.flags & cutweave_playback::shm::FLAG_LOWRES != 0,
            rev: n(meta.rev),
            config_gen: meta.config_gen,
            render_age_ms: age_ms(meta.render_ns),
            publish_age_ms: age_ms(meta.publish_ns),
        }))
    }

    /// Resolve when something worth reacting to happens, after at most `timeoutMs`:
    /// `"frame"` (a frame newer than the last one returned is published), `"changed"` (the
    /// writer retired this ring or its heartbeat went stale: call `status()`), `"ended"`
    /// (the ring is already released) or `"timeout"`. Polls on a worker thread, so it never
    /// blocks the JS thread.
    #[napi(ts_return_type = "Promise<'frame' | 'changed' | 'ended' | 'timeout'>")]
    pub fn wait_for_frame(&self, timeout_ms: u32) -> AsyncTask<WaitTask> {
        AsyncTask::new(WaitTask {
            reader: self.session.reader().cloned(),
            after: self.session.last_seq(),
            timeout: Duration::from_millis(u64::from(timeout_ms.min(MAX_WAIT_MS))),
        })
    }

    /// The audio clock extrapolated to now, or `null` before the engine's first sample (null
    /// clock) or once the ring is gone.
    #[napi]
    pub fn audio_clock(&self) -> Option<AudioClock> {
        let c = self.session.clock().filter(|c| c.host_ns != 0)?;
        let now = now_ns();
        Some(AudioClock {
            pts_flicks: c.extrapolate(now) as f64,
            state: state_name(c.state).to_owned(),
            rate_milli: c.rate_milli,
            age_ms: now.saturating_sub(c.host_ns) as f64 / 1e6,
        })
    }

    /// Release the mapping and the pooled buffers. Safe to call twice.
    #[napi]
    pub fn close(&mut self, env: &Env) {
        self.session.close();
        self.pool.release_all(env);
    }
}

impl Ring {
    /// Fold this ring's torn-copy and successor-switch counts into the process counters.
    fn sync_counters(&mut self) {
        let torn = self.session.reader().map_or(0, |r| r.torn_retries());
        if torn > self.torn_reported {
            TORN_RETRIES.fetch_add(torn - self.torn_reported, Ordering::Relaxed);
            self.torn_reported = torn;
        }
        let switches = self.session.switches();
        if switches > self.switches_reported {
            SUCCESSOR_SWITCHES.fetch_add(
                u64::from(switches - self.switches_reported),
                Ordering::Relaxed,
            );
            self.switches_reported = switches;
        }
    }
}

/// Background poll for [`Ring::wait_for_frame`].
pub struct WaitTask {
    reader: Option<std::sync::Arc<RingReader>>,
    after: u64,
    timeout: Duration,
}

impl Task for WaitTask {
    type Output = &'static str;
    type JsValue = String;

    fn compute(&mut self) -> Result<&'static str> {
        let Some(r) = &self.reader else {
            return Ok("ended");
        };
        let deadline = Instant::now() + self.timeout;
        loop {
            if r.write_index() > self.after {
                return Ok("frame");
            }
            if r.writer_state() != Some(WriterState::Live) || r.heartbeat_stale(now_ns()) {
                return Ok("changed");
            }
            if Instant::now() >= deadline {
                return Ok("timeout");
            }
            std::thread::sleep(WAIT_POLL);
        }
    }

    fn resolve(&mut self, _env: Env, output: &'static str) -> Result<String> {
        Ok(output.to_owned())
    }
}

/// Open the preview ring `name` read-only. `epoch` is the `ring_epoch` the engine announced
/// (a `bigint`, or a decimal / `0x` hex string); a ring with another epoch is rejected.
/// Throws errors with `ERR_RING_*` codes at the start of the message: `ERR_RING_NOT_FOUND`,
/// `ERR_RING_EPOCH`, `ERR_RING_VERSION`, `ERR_RING_INVALID`, `ERR_RING_ACCESS_DENIED`, ...
#[napi]
pub fn open_ring(name: String, epoch: Either<BigInt, String>) -> Result<Ring> {
    let epoch = parse_epoch(epoch)?;
    let session = RingSession::open(&name, epoch).map_err(|e| js_err(&e))?;
    RINGS_OPENED.fetch_add(1, Ordering::Relaxed);
    Ok(Ring {
        session,
        pool: Pool::default(),
        torn_reported: 0,
        switches_reported: 0,
    })
}

/// Process-wide counters.
#[napi]
pub fn perf_counters() -> PerfCounters {
    PerfCounters {
        rings_opened: n(RINGS_OPENED.load(Ordering::Relaxed)),
        frames_read: n(FRAMES_READ.load(Ordering::Relaxed)),
        bytes_copied: n(BYTES_COPIED.load(Ordering::Relaxed)),
        torn_retries: n(TORN_RETRIES.load(Ordering::Relaxed)),
        successor_switches: n(SUCCESSOR_SWITCHES.load(Ordering::Relaxed)),
    }
}
