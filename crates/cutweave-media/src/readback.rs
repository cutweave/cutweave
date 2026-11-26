// SPDX-License-Identifier: GPL-3.0-or-later
//! [`ReadBack`]: the frames of a long-GOP stream, served backwards (WP-2D.01b; arch §7 "Decode",
//! §19).
//!
//! A frame of a long-GOP stream decodes only forward from its keyframe, so playing or scrubbing
//! backwards through [`VideoDecoder::seek_exact`] alone decodes up to a whole GOP for every frame
//! it shows. A read-back decodes forward once and keeps the frames, so the ones before come out
//! of memory:
//!
//! - **The window.** Asked to [`fill`](ReadBack::fill) up to frame `n` (presentation order), it
//!   decodes the frames from the start of `n`'s GOP (its keyframe), or from `n + 1 − capacity`
//!   when the GOP is longer than it holds, up to `n` ([`ReadBack::window`]). The first comes from
//!   `seek_exact` (which skips the non-reference pictures before it), the rest from `next_frame`
//!   in presentation order, so each is the decoder's own frame, bit-identical to a seek to it.
//!   Going backwards, the frames before `n` are lookups until the window is used up; the frame
//!   before it starts the next window: the GOP before, or the earlier part of a long GOP, which
//!   costs the decode from its keyframe again (about `G² / 2C` decoded frames for a GOP of `G`
//!   frames at capacity `C`, against `G² / 2` without a read-back).
//! - **Bounded; no allocation once warm.** It holds at most `capacity` frames (callers size it
//!   from a byte budget: [`ReadBack::capacity_for`]). Its slots are allocated with it; a lookup
//!   ([`get`](ReadBack::get)) takes no lock and allocates nothing, and a fill reuses the slots
//!   (the frames are the decoder's).
//! - **What it keeps** is the caller's: the decoder's [`VideoFrame`], or what the caller makes of
//!   it (the preview keeps the compositor's picture, so a hit is a reference-count bump).
//! - **Damage** follows the seek rule: a frame the decoder skips is shown as the next frame it
//!   produces, and a stream that ends early leaves the window short.
//! - **One owner** (the preview's render thread holds one per media item); a fill that fails or
//!   is cancelled keeps the frames it decoded.

use std::ops::RangeInclusive;

use crate::{FrameIndex, MediaError, VideoDecoder, VideoFrame};

/// Frames of one stream decoded forward and served in any order (see the module docs).
#[derive(Debug)]
pub struct ReadBack<T> {
    capacity: usize,
    /// Presentation index of `slots[0]`.
    first: usize,
    /// The window: frame `first + i` is `slots[i]`.
    slots: Vec<T>,
    fills: u64,
}

impl<T: Clone> ReadBack<T> {
    /// A read-back holding at most `capacity` frames (at least 1). Allocates its slots.
    #[must_use]
    pub fn new(capacity: usize) -> ReadBack<T> {
        let capacity = capacity.max(1);
        ReadBack {
            capacity,
            first: 0,
            slots: Vec::with_capacity(capacity),
            fills: 0,
        }
    }

    /// How many frames of `frame_bytes` each fit in `budget_bytes` (at least 1).
    #[must_use]
    pub fn capacity_for(budget_bytes: usize, frame_bytes: usize) -> usize {
        (budget_bytes / frame_bytes.max(1)).max(1)
    }

    /// The most frames it holds.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Changes the most frames it holds (at least 1): drops the window and allocates the slots
    /// when it grows. Frames it holds beyond a smaller capacity are dropped.
    pub fn set_capacity(&mut self, capacity: usize) {
        let capacity = capacity.max(1);
        if capacity == self.capacity {
            return;
        }
        self.slots.truncate(capacity);
        if capacity > self.slots.capacity() {
            self.slots.reserve_exact(capacity - self.slots.len());
        }
        self.capacity = capacity;
    }

    /// Frames held now.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether it holds no frame.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The presentation indices of the frames held, if any.
    #[must_use]
    pub fn held(&self) -> Option<RangeInclusive<usize>> {
        let last = self.slots.len().checked_sub(1)?;
        Some(self.first..=self.first + last)
    }

    /// Windows decoded so far ([`fill`](Self::fill) calls that started decoding).
    #[must_use]
    pub fn fills(&self) -> u64 {
        self.fills
    }

    /// Frame `n` (presentation order), if it is held. No lock, no allocation.
    #[must_use]
    pub fn get(&self, n: usize) -> Option<&T> {
        self.slots.get(n.checked_sub(self.first)?)
    }

    /// Drops every frame held.
    pub fn clear(&mut self) {
        self.slots.clear();
    }

    /// The frames a fill up to frame `n` of `index` decodes with `capacity` slots: from the last
    /// keyframe at or before `n` (presentation order), but no further back than
    /// `n + 1 − capacity`, to `n`. `None` when `index` has no frame `n`.
    #[must_use]
    pub fn window(index: &FrameIndex, n: usize, capacity: usize) -> Option<RangeInclusive<usize>> {
        let frames = index.frames();
        frames.get(n)?;
        let entries = index.entries();
        let lowest = (n + 1).saturating_sub(capacity.max(1));
        let is_key = |i: usize| entries.get(frames[i].decode_idx).is_some_and(|e| e.is_key);
        let mut start = n;
        while start > lowest && !is_key(start) {
            start -= 1;
        }
        Some(start..=n)
    }

    /// Decodes the window that ends at frame `n` of `index` with `decoder` (which decodes the
    /// stream `index` describes), replacing the frames held, and keeps what `keep` makes of
    /// each frame. Returns what it keeps for frame `n`, or `None` when the stream ended before
    /// it. A decoder error (a cancel, damage it cannot get past) or an error of `keep` ends the
    /// fill; the frames decoded until then stay.
    pub fn fill<E: From<MediaError>>(
        &mut self,
        decoder: &mut dyn VideoDecoder,
        index: &FrameIndex,
        n: usize,
        mut keep: impl FnMut(VideoFrame) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        let window = Self::window(index, n, self.capacity).ok_or_else(|| MediaError::Invalid {
            message: format!("frame {n} is not in the frame index"),
        })?;
        let (lo, hi) = (*window.start(), *window.end());
        let at = index.time_of(lo).ok_or_else(|| MediaError::Invalid {
            message: format!("frame {lo} has no time"),
        })?;
        self.slots.clear();
        self.first = lo;
        self.fills += 1;
        let mut frame = decoder.seek_exact(at)?;
        loop {
            let shown = index.frame_at(frame.pts).min(hi);
            let next = self.first + self.slots.len();
            if shown >= next {
                let kept = keep(frame)?;
                // Frames the decoder did not produce show the next one, as a seek to them does.
                for _ in next..shown {
                    self.slots.push(kept.clone());
                }
                self.slots.push(kept);
            }
            if self.first + self.slots.len() > hi {
                break;
            }
            match decoder.next_frame()? {
                Some(f) => frame = f,
                None => break,
            }
        }
        Ok(self.get(n).cloned())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::Path;
    use std::sync::Arc;

    use cutweave_model::FrameRate;

    use super::*;
    use crate::testing::{FakeBackend, FakeMedia, FakeVideo, fake_frame_number};
    use crate::{DecodeOpts, MediaBackend};

    const PATH: &str = "readback.mp4";

    fn backend(frames: u64, gop: u64) -> FakeBackend {
        FakeBackend::new().with_media(
            PATH,
            FakeMedia::video(FakeVideo::new(64, 36, FrameRate::FPS_30, frames).with_gop(gop)),
        )
    }

    fn open(backend: &FakeBackend) -> (Box<dyn VideoDecoder>, Arc<FrameIndex>) {
        let index = Arc::new(backend.index_media(Path::new(PATH)).unwrap());
        let decoder = backend
            .open_decoder(
                Path::new(PATH),
                &DecodeOpts::default().with_index(Arc::clone(&index)),
            )
            .unwrap();
        (decoder, index)
    }

    /// Plays `frames` backwards through `rb`, returning each frame's number as decoded.
    fn reverse(
        rb: &mut ReadBack<Arc<VideoFrame>>,
        decoder: &mut dyn VideoDecoder,
        index: &FrameIndex,
    ) -> Vec<u32> {
        (0..index.frame_count())
            .rev()
            .map(|n| {
                let frame = match rb.get(n) {
                    Some(f) => Arc::clone(f),
                    None => rb
                        .fill(decoder, index, n, |f| Ok::<_, MediaError>(Arc::new(f)))
                        .unwrap()
                        .unwrap(),
                };
                fake_frame_number(&frame).unwrap()
            })
            .collect()
    }

    #[test]
    fn windows_start_at_the_gop_or_the_capacity() {
        let b = backend(60, 12);
        let (_, index) = open(&b);
        let w = |n, c| ReadBack::<()>::window(&index, n, c).unwrap();
        assert_eq!(w(59, 100), 48..=59);
        assert_eq!(w(48, 100), 48..=48);
        assert_eq!(w(47, 100), 36..=47);
        assert_eq!(w(59, 5), 55..=59);
        assert_eq!(w(50, 5), 48..=50);
        assert_eq!(w(0, 1), 0..=0);
        assert_eq!(ReadBack::<()>::window(&index, 60, 4), None);
    }

    #[test]
    fn reverse_playback_decodes_each_frame_once_when_the_gop_fits() {
        let b = backend(60, 12);
        let (mut decoder, index) = open(&b);
        let mut rb = ReadBack::new(ReadBack::<()>::capacity_for(
            64 * 36 * 3 / 2 * 20,
            64 * 36 * 3 / 2,
        ));
        assert_eq!(rb.capacity(), 20);
        let before = b.decoded_frames();
        let shown = reverse(&mut rb, decoder.as_mut(), &index);
        assert_eq!(shown, (0..60).rev().collect::<Vec<u32>>());
        // One window per GOP, every frame decoded once.
        assert_eq!(rb.fills(), 5);
        assert_eq!(b.decoded_frames() - before, 60);
        assert_eq!(rb.held(), Some(0..=11));
    }

    #[test]
    fn a_gop_longer_than_the_capacity_is_read_back_in_parts() {
        let b = backend(48, 24);
        let (mut decoder, index) = open(&b);
        let mut rb = ReadBack::new(8);
        let shown = reverse(&mut rb, decoder.as_mut(), &index);
        assert_eq!(shown, (0..48).rev().collect::<Vec<u32>>());
        // 24 frames in windows of 8: three per GOP.
        assert_eq!(rb.fills(), 6);
        assert!(rb.len() <= 8);
    }

    #[test]
    fn read_back_frames_equal_the_frames_a_seek_returns() {
        let b = backend(40, 12);
        let (mut decoder, index) = open(&b);
        let (mut seeker, _) = open(&b);
        let mut rb = ReadBack::new(7);
        for n in (0..40).rev() {
            if rb.get(n).is_none() {
                rb.fill(decoder.as_mut(), &index, n, Ok::<_, MediaError>)
                    .unwrap();
            }
            let sought = seeker.seek_exact(index.time_of(n).unwrap()).unwrap();
            assert_eq!(rb.get(n), Some(&sought), "frame {n}");
        }
    }

    #[test]
    fn lookups_outside_the_window_miss_and_capacity_changes_keep_it_bounded() {
        let b = backend(30, 12);
        let (mut decoder, index) = open(&b);
        let mut rb: ReadBack<u32> = ReadBack::new(4);
        let got = rb
            .fill(decoder.as_mut(), &index, 22, |f| {
                Ok::<_, MediaError>(fake_frame_number(&f).unwrap())
            })
            .unwrap();
        assert_eq!(got, Some(22));
        assert_eq!(rb.held(), Some(19..=22));
        assert_eq!(rb.get(18), None);
        assert_eq!(rb.get(23), None);
        assert_eq!(rb.get(19), Some(&19));
        rb.set_capacity(2);
        assert_eq!(rb.len(), 2);
        rb.set_capacity(16);
        rb.fill(decoder.as_mut(), &index, 22, |f| {
            Ok::<_, MediaError>(fake_frame_number(&f).unwrap())
        })
        .unwrap();
        assert_eq!(rb.held(), Some(12..=22));
        rb.clear();
        assert!(rb.is_empty());
        assert_eq!(rb.held(), None);
        // An error of the keeper ends the fill and keeps what was decoded.
        let err = rb.fill(decoder.as_mut(), &index, 22, |f| {
            let n = fake_frame_number(&f).unwrap();
            if n < 15 {
                Ok(n)
            } else {
                Err(MediaError::Cancelled)
            }
        });
        assert!(matches!(err, Err(MediaError::Cancelled)));
        assert_eq!(rb.held(), Some(12..=14));
    }
}
