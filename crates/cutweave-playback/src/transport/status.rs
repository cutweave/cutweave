// SPDX-License-Identifier: GPL-3.0-or-later
//! [`StatusPump`]: `playback.state` events at most [`MAX_STATE_HZ`] times a second, and the
//! stop at the end (or, in reverse, the start) of the content.

use crate::transport::control::Transport;
use crate::transport::state::{PlaybackState, TransportMode};

/// The most `playback.state` events per second.
pub const MAX_STATE_HZ: u32 = 30;
/// The `playback.state` event name (arch §5.1 events).
pub const PLAYBACK_STATE_EVENT: &str = "playback.state";
/// Minimum spacing of two events: `1 s / MAX_STATE_HZ`, rounded up.
const MIN_INTERVAL_NS: u64 = 1_000_000_000_u64.div_ceil(MAX_STATE_HZ as u64);
/// How often an unchanged, stopped transport is looked at (changes the render thread makes,
/// such as the end of a scrub refine, are found this way; control changes wake the pump).
const IDLE_POLL_NS: u64 = 250_000_000;

/// Receives `playback.state` events (the engine publishes them to every client).
pub trait StateSink: Send {
    /// One event.
    fn publish(&mut self, state: &PlaybackState);
}

impl<F: FnMut(&PlaybackState) + Send> StateSink for F {
    fn publish(&mut self, state: &PlaybackState) {
        self(state);
    }
}

/// What a [`StatusPump::poll`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusPoll {
    /// The event to publish now, if any.
    pub event: Option<PlaybackState>,
    /// When to poll next (host clock), unless the transport changes first.
    pub wake_at_ns: u64,
}

/// Throttles `playback.state` to [`MAX_STATE_HZ`] (every period while playing or scrubbing,
/// otherwise only on change) and stops playback at the content boundary. Runs on the status
/// thread of [`PlaybackEngine`](crate::transport::PlaybackEngine); tests poll it by hand.
#[derive(Debug)]
pub struct StatusPump {
    transport: Transport,
    last_sent_ns: Option<u64>,
    last: Option<PlaybackState>,
}

impl StatusPump {
    pub(crate) fn new(transport: Transport) -> StatusPump {
        StatusPump {
            transport,
            last_sent_ns: None,
            last: None,
        }
    }

    /// Stops playback that reached the end (or start), then returns the event due now.
    pub fn poll(&mut self) -> StatusPoll {
        let shared = &self.transport.shared;
        let now = shared.host.now_ns();
        let snap = shared.snapshot.load();
        if snap.rate_milli != 0 && snap.at_boundary(snap.position(&shared.clock.read(), now)) {
            self.transport.stop_at_boundary(snap.epoch);
        }
        if let Some(sent) = self.last_sent_ns {
            let due = sent.saturating_add(MIN_INTERVAL_NS);
            if now < due {
                return StatusPoll {
                    event: None,
                    wake_at_ns: due,
                };
            }
        }
        let state = self.transport.state();
        let period = if state.state == TransportMode::Stopped {
            IDLE_POLL_NS
        } else {
            MIN_INTERVAL_NS
        };
        if self.last.as_ref() == Some(&state) {
            return StatusPoll {
                event: None,
                wake_at_ns: now.saturating_add(period),
            };
        }
        self.last = Some(state.clone());
        self.last_sent_ns = Some(now);
        StatusPoll {
            event: Some(state),
            wake_at_ns: now.saturating_add(period.max(MIN_INTERVAL_NS)),
        }
    }
}
