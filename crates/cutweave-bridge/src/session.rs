// SPDX-License-Identifier: GPL-3.0-or-later
//! [`RingSession`]: what the preload holds. One logical preview ring over time: it follows
//! `Retired` -> successor moves, watches the writer's heartbeat, releases the mapping when the
//! writer is gone, and remembers the last delivered `seq` (latest wins, never goes back).

use std::sync::Arc;

use cutweave_playback::shm::layout::WriterState;

use crate::error::BridgeError;
use crate::reader::{ClockReading, FrameBuffer, RingReader, SlotMeta, now_ns};

/// Successor hops followed in one call. A new hop needs a window resize beyond the current
/// slot capacity, so more than a couple in a row means something is wrong.
const MAX_HOPS: u32 = 8;

/// Why a session no longer has a ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// The engine closed the ring cleanly (`writer_state == 3`).
    WriterClosed,
    /// No heartbeat for 500 ms: the engine is hung or dead.
    HeartbeatStale,
    /// The ring was retired but its successor is already gone (two moves in quick
    /// succession): ask the engine for the current ring and open it again.
    SuccessorGone,
    /// The retired ring named a successor this reader refuses (bad name, epoch or version);
    /// the payload is the `ERR_RING_*` code.
    SuccessorRejected(&'static str),
    /// [`RingSession::close`] was called.
    ClosedByCaller,
}

impl EndReason {
    /// Stable string for JS.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::WriterClosed => "writer-closed",
            Self::HeartbeatStale => "heartbeat-stale",
            Self::SuccessorGone => "successor-gone",
            Self::SuccessorRejected(_) => "successor-rejected",
            Self::ClosedByCaller => "closed",
        }
    }
}

/// State of the session as seen right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// The writer is publishing and its heartbeat is fresh.
    Live,
    /// The header exists but the writer has not gone live yet.
    Starting,
    /// The ring is gone; reopen with a fresh announcement from the engine.
    Ended(EndReason),
}

/// A point-in-time description of the session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionStatus {
    /// Liveness verdict.
    pub state: SessionState,
    /// Current ring name, while there is one.
    pub name: Option<String>,
    /// Its epoch.
    pub epoch: Option<u64>,
    /// Engine process id.
    pub writer_pid: Option<u32>,
    /// Milliseconds since the writer's last heartbeat (live rings only).
    pub heartbeat_age_ms: Option<f64>,
    /// Newest `seq` the writer has committed.
    pub write_index: u64,
    /// Newest `seq` delivered to the caller.
    pub last_seq: u64,
    /// Times the session moved to a successor ring.
    pub switches: u32,
}

/// See the module docs.
pub struct RingSession {
    reader: Option<Arc<RingReader>>,
    end: Option<EndReason>,
    last_seq: u64,
    switches: u32,
}

impl RingSession {
    /// Open the ring `name`, which must carry `expected_epoch`.
    pub fn open(name: &str, expected_epoch: u64) -> Result<Self, BridgeError> {
        Ok(Self::with_reader(RingReader::open(name, expected_epoch)?))
    }

    /// Wrap an already opened reader.
    #[must_use]
    pub fn with_reader(reader: RingReader) -> Self {
        Self {
            reader: Some(Arc::new(reader)),
            end: None,
            last_seq: 0,
            switches: 0,
        }
    }

    /// The current ring's reader, if the session is not ended.
    #[must_use]
    pub fn reader(&self) -> Option<&Arc<RingReader>> {
        self.reader.as_ref()
    }

    /// Newest `seq` delivered so far.
    #[must_use]
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// Times the session moved to a successor ring.
    #[must_use]
    pub fn switches(&self) -> u32 {
        self.switches
    }

    /// Why the session ended, if it did.
    #[must_use]
    pub fn end_reason(&self) -> Option<&EndReason> {
        self.end.as_ref()
    }

    fn end(&mut self, reason: EndReason) {
        // Dropping the reader unmaps the ring: on Windows a dead engine's ring lives exactly
        // as long as a reader maps it (`RING.md`, "Notes for the reader").
        self.reader = None;
        self.end = Some(reason);
    }

    /// Release the ring. Further reads fail with [`BridgeError::Closed`].
    pub fn close(&mut self) {
        self.end(EndReason::ClosedByCaller);
    }

    /// Follow the retired ring to its successor, once. Ends the session if that is
    /// impossible.
    fn follow(&mut self) {
        let Some(old) = self.reader.clone() else {
            return;
        };
        let Some((name, epoch)) = old.successor() else {
            self.end(EndReason::SuccessorRejected("ERR_RING_INVALID"));
            return;
        };
        match RingReader::open(name.as_str(), epoch) {
            Ok(new) => {
                self.reader = Some(Arc::new(new));
                self.switches += 1;
            }
            Err(BridgeError::NotFound(_)) => self.end(EndReason::SuccessorGone),
            Err(e) => self.end(EndReason::SuccessorRejected(e.code())),
        }
    }

    /// Apply what the header says about the writer at `now`: end the session on a closed ring
    /// or a stale heartbeat, follow a retired ring to its successor once every frame it holds
    /// has been delivered. Returns the resulting state.
    pub fn refresh(&mut self, now: u64) -> SessionState {
        for _ in 0..MAX_HOPS {
            let Some(r) = self.reader.clone() else {
                break;
            };
            match r.writer_state() {
                Some(WriterState::Closed) => self.end(EndReason::WriterClosed),
                Some(WriterState::Live) => {
                    if r.heartbeat_stale(now) {
                        self.end(EndReason::HeartbeatStale);
                    }
                    break;
                }
                Some(WriterState::Retired) => {
                    if r.write_index() > self.last_seq && !self.drained_unreadable(&r) {
                        // Frames committed before the move are still unread: `read` delivers
                        // them first, then follows.
                        break;
                    }
                    self.follow();
                    continue;
                }
                Some(WriterState::Initializing) | None => break,
            }
            break;
        }
        self.state(now)
    }

    /// True if the retired ring's unread frames cannot be delivered (every slot already
    /// holds an older frame or is corrupt), so waiting for them would never end.
    fn drained_unreadable(&self, r: &RingReader) -> bool {
        r.select(self.last_seq, None).ok().flatten().is_none()
    }

    fn state(&self, now: u64) -> SessionState {
        if let Some(reason) = &self.end {
            return SessionState::Ended(reason.clone());
        }
        match self.reader.as_ref().and_then(|r| r.writer_state()) {
            Some(WriterState::Live) => {
                if self.reader.as_ref().is_some_and(|r| r.heartbeat_stale(now)) {
                    SessionState::Ended(EndReason::HeartbeatStale)
                } else {
                    SessionState::Live
                }
            }
            // A retired ring we have not left yet still has frames to hand out.
            Some(WriterState::Retired) => SessionState::Live,
            _ => SessionState::Starting,
        }
    }

    /// Describe the session (and apply [`refresh`](Self::refresh)).
    pub fn status(&mut self) -> SessionStatus {
        let now = now_ns();
        let state = self.refresh(now);
        let r = self.reader.as_ref();
        SessionStatus {
            state,
            name: r.map(|r| r.name().to_string()),
            epoch: r.map(|r| r.epoch()),
            writer_pid: r.map(|r| r.writer_pid()),
            #[allow(clippy::cast_precision_loss)] // ns -> ms for display
            heartbeat_age_ms: r.map(|r| r.heartbeat_age_ns(now) as f64 / 1e6),
            write_index: r.map_or(self.last_seq, |r| r.write_index()),
            last_seq: self.last_seq,
            switches: self.switches,
        }
    }

    /// Deliver the newest frame not delivered yet, optionally only frames with `pts <=
    /// max_pts`. Follows successor rings and ends the session when the writer is gone;
    /// `Ok(None)` means nothing new (or the session ended: see [`status`](Self::status)).
    pub fn read(
        &mut self,
        max_pts: Option<i64>,
        buffer: &mut impl FrameBuffer,
    ) -> Result<Option<(SlotMeta, Arc<RingReader>)>, BridgeError> {
        if self.end == Some(EndReason::ClosedByCaller) {
            return Err(BridgeError::Closed);
        }
        for _ in 0..MAX_HOPS {
            let state = self.refresh(now_ns());
            if matches!(state, SessionState::Ended(_)) {
                return Ok(None);
            }
            let Some(r) = self.reader.clone() else {
                return Ok(None);
            };
            if let Some(meta) = r.read(self.last_seq, max_pts, buffer)? {
                self.last_seq = meta.seq;
                return Ok(Some((meta, r)));
            }
            if r.writer_state() == Some(WriterState::Retired) {
                // Nothing deliverable left in the old ring: move on to the successor.
                self.follow();
                continue;
            }
            return Ok(None);
        }
        Ok(None)
    }

    /// Like [`read`](Self::read), restricted to `pts <= clock(now) - latency_flicks`: the
    /// clock-scheduled choice of arch §10 step 3. The latency only applies while the clock
    /// plays: stopped or scrubbing, the frame for the playhead itself (`pts <= clock`) is due,
    /// so a pause, a step or a seek to a frame's start shows that frame (WP-1I.05). Without an
    /// audio clock sample (null clock) it reads the newest frame.
    pub fn read_at_clock(
        &mut self,
        latency_flicks: i64,
        buffer: &mut impl FrameBuffer,
    ) -> Result<Option<(SlotMeta, Arc<RingReader>)>, BridgeError> {
        let now = now_ns();
        let target = self
            .reader
            .as_ref()
            .and_then(|r| r.clock())
            .filter(|c| c.host_ns != 0)
            .map(|c| {
                let at = c.extrapolate(now);
                if c.state == cutweave_playback::shm::ClockState::Playing {
                    at.saturating_sub(latency_flicks)
                } else {
                    at
                }
            });
        self.read(target, buffer)
    }

    /// The audio clock sample of the current ring.
    #[must_use]
    pub fn clock(&self) -> Option<ClockReading> {
        self.reader.as_ref().and_then(|r| r.clock())
    }
}
