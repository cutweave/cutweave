//! Scrub scheduling for seeks while paused (WP-0C.02, arch §10 step 5).
//!
//! A seek renders a reduced-size preview at once (when enabled) and asks the full-resolution
//! "decoder" for the same frame. The synthetic engine renders full frames in about a
//! millisecond, so a real decoder's cost (long-GOP decode, effects) is emulated with `cost`.
//! The decoder runs one request at a time and is not preempted; while it is busy, only the
//! newest request waits (latest-wins: older waiting requests are cancelled, never rendered).

use std::time::{Duration, Instant};

/// One-at-a-time full-resolution decoder with a latest-wins queue of one.
#[derive(Debug, Default)]
pub struct FullDecoder {
    /// Emulated decode time per full-resolution frame.
    pub cost: Duration,
    running: Option<(i64, Instant)>,
    waiting: Option<i64>,
}

impl FullDecoder {
    /// A decoder whose full-resolution frames take `cost` each.
    #[must_use]
    pub fn new(cost: Duration) -> Self {
        Self {
            cost,
            ..Self::default()
        }
    }

    /// Ask for `frame` at full resolution. Replaces a waiting (not yet started) request.
    pub fn request(&mut self, frame: i64, now: Instant) {
        match self.running {
            None => self.running = Some((frame, now + self.cost)),
            Some((f, _)) if f == frame => self.waiting = None,
            Some(_) => self.waiting = Some(frame),
        }
    }

    /// The frame whose decode finished by `now` (render it), starting the waiting one.
    pub fn poll(&mut self, now: Instant) -> Option<i64> {
        let (frame, done_at) = self.running?;
        if now < done_at {
            return None;
        }
        self.running = self
            .waiting
            .take()
            .filter(|w| *w != frame)
            .map(|w| (w, now + self.cost));
        Some(frame)
    }

    /// Put `frame` back as done (rendering it failed; try again next iteration).
    pub fn retry(&mut self, frame: i64, now: Instant) {
        if let Some((next, _)) = self.running.replace((frame, now)) {
            self.waiting = Some(next);
        }
    }

    /// When the running decode completes.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        self.running.map(|(_, at)| at)
    }

    /// Drop everything (playback resumed).
    pub fn clear(&mut self) {
        self.running = None;
        self.waiting = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn zero_cost_completes_immediately() {
        let t = Instant::now();
        let mut d = FullDecoder::new(Duration::ZERO);
        d.request(7, t);
        assert_eq!(d.poll(t), Some(7));
        assert_eq!(d.poll(t), None);
    }

    #[test]
    fn busy_decoder_keeps_only_the_newest_waiting_request() {
        let t = Instant::now();
        let mut d = FullDecoder::new(50 * MS);
        d.request(1, t);
        d.request(2, t + MS);
        d.request(3, t + 2 * MS); // replaces 2 (latest-wins)
        assert_eq!(d.poll(t + 49 * MS), None, "running decode is not done yet");
        assert_eq!(
            d.poll(t + 50 * MS),
            Some(1),
            "running decode is not preempted"
        );
        assert_eq!(
            d.deadline(),
            Some(t + 100 * MS),
            "newest waiting request starts next"
        );
        assert_eq!(d.poll(t + 100 * MS), Some(3));
        assert_eq!(d.poll(t + 200 * MS), None, "2 was cancelled");
    }

    #[test]
    fn same_frame_is_not_decoded_twice() {
        let t = Instant::now();
        let mut d = FullDecoder::new(10 * MS);
        d.request(4, t);
        d.request(5, t);
        d.request(4, t); // back to the running frame: nothing waits
        assert_eq!(d.poll(t + 10 * MS), Some(4));
        assert_eq!(d.deadline(), None);
    }

    #[test]
    fn retry_and_clear() {
        let t = Instant::now();
        let mut d = FullDecoder::new(10 * MS);
        d.request(1, t);
        d.request(2, t);
        assert_eq!(d.poll(t + 10 * MS), Some(1));
        d.retry(1, t + 10 * MS);
        assert_eq!(
            d.poll(t + 10 * MS),
            Some(1),
            "retried frame comes back first"
        );
        assert_eq!(d.poll(t + 20 * MS), Some(2));
        d.request(3, t);
        d.clear();
        assert_eq!(d.deadline(), None);
    }
}
