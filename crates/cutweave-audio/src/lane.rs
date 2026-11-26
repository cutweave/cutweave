// SPDX-License-Identifier: GPL-3.0-or-later
//! The per-track lane (worker side): renders a bus's input, the sum of its clips at their clip
//! gains on the output grid, block after block. The same renderer feeds the real-time rings and
//! the offline mixdown, which is what makes their output identical.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cutweave_model::SampleRate;

use crate::dsp::CHANNELS;
use crate::plan::ClipPlan;
use crate::source::SourceFactory;
use crate::voice::ClipVoice;

/// An open clip: its index in the lane, and its chain (`None` once the source failed: the clip
/// then plays silence until its end and is not reopened).
struct Active {
    clip: usize,
    voice: Option<ClipVoice>,
}

/// Renders one track's clips in output order from a start position.
pub(crate) struct LaneRenderer {
    clips: Arc<[ClipPlan]>,
    sources: Arc<dyn SourceFactory>,
    rate: SampleRate,
    pos: i64,
    /// Every clip before this index ends at or before `pos`.
    cursor: usize,
    active: Vec<Active>,
    errors: Arc<AtomicU64>,
}

impl LaneRenderer {
    /// A renderer whose first frame is output sample `pos`. Source failures are counted in
    /// `errors`.
    pub fn new(
        clips: Arc<[ClipPlan]>,
        sources: Arc<dyn SourceFactory>,
        rate: SampleRate,
        pos: i64,
        errors: Arc<AtomicU64>,
    ) -> LaneRenderer {
        LaneRenderer {
            clips,
            sources,
            rate,
            pos,
            cursor: 0,
            active: Vec::new(),
            errors,
        }
    }

    /// The output sample the next [`render`](Self::render) starts at.
    pub fn pos(&self) -> i64 {
        self.pos
    }

    fn fail(&self, clip: &ClipPlan, error: &dyn std::fmt::Display) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(item = %clip.item, media = %clip.media, %error, "clip audio is silent");
    }

    /// Overwrites `out` with the next `out.len() / 2` frames and advances.
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let frames = i64::try_from(out.len() / CHANNELS).unwrap_or(i64::MAX);
        let (from, to) = (self.pos, self.pos.saturating_add(frames));
        while self.cursor < self.clips.len() && self.clips[self.cursor].end <= from {
            self.cursor += 1;
        }
        for index in self.cursor..self.clips.len() {
            let clip = &self.clips[index];
            if clip.start >= to {
                break;
            }
            if clip.end <= from {
                continue;
            }
            let a = clip.start.max(from);
            let b = clip.end.min(to);
            let slot = if let Some(slot) = self.active.iter().position(|v| v.clip == index) {
                slot
            } else {
                let voice = ClipVoice::open(&*self.sources, clip, self.rate, a)
                    .map_err(|e| self.fail(clip, &e))
                    .ok();
                self.active.push(Active { clip: index, voice });
                self.active.len() - 1
            };
            let range = offset(a - from)..offset(b - from);
            if let Some(voice) = self.active[slot].voice.as_mut()
                && let Err(e) = voice.render_add(&mut out[range])
            {
                self.active[slot].voice = None;
                self.fail(clip, &e);
            }
        }
        let clips = &self.clips;
        self.active.retain(|v| clips[v.clip].end > to);
        self.pos = to;
    }
}

/// Sample offset of a frame offset within a block (blocks are far below `usize::MAX` frames).
fn offset(frames: i64) -> usize {
    usize::try_from(frames).unwrap_or(0) * CHANNELS
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp, clippy::unwrap_used, clippy::cast_sign_loss)]

    use cutweave_model::{Duration, ItemId, MediaId, Time};

    use super::*;
    use crate::automation::Envelope;
    use crate::source::PcmFormat;
    use crate::tone::{Tone, ToneFactory, ToneSpec};

    fn clip(item: u128, media: u128, start: i64, end: i64, gain: f32) -> ClipPlan {
        ClipPlan {
            item: ItemId::from_u128(item),
            media: MediaId::from_u128(media),
            stream: None,
            start,
            end,
            src_start: Time::ZERO,
            src_end: Time::ZERO
                .checked_add(Duration::from_secs(10).unwrap())
                .unwrap(),
            speed: cutweave_model::Speed::ONE,
            reverse: false,
            env: Envelope::flat(gain, start, end - start),
        }
    }

    fn impulses() -> ToneSpec {
        ToneSpec::new(
            Tone::Impulse {
                amplitude: 1.0,
                period: 0,
            },
            PcmFormat::new(SampleRate::HZ_48_000, 2),
        )
    }

    #[test]
    fn clips_start_on_their_sample_whatever_the_block_size() {
        let sources: Arc<dyn SourceFactory> = Arc::new(
            ToneFactory::new()
                .with(MediaId::from_u128(1), impulses())
                .with(
                    MediaId::from_u128(2),
                    ToneSpec::new(
                        Tone::Sine {
                            freq_hz: 1000.0,
                            amplitude: 0.5,
                            phase: 0.0,
                        },
                        PcmFormat::new(SampleRate::HZ_44_100, 2),
                    ),
                ),
        );
        let clips: Arc<[ClipPlan]> = vec![
            clip(1, 1, 100, 200, 0.5),
            clip(2, 1, 200, 300, 1.0),
            clip(3, 2, 250, 5000, 1.0),
            clip(4, 1, 777, 900, 0.25), // overlapping clips still sum
        ]
        .into();
        let render = |block: usize| {
            let mut lane = LaneRenderer::new(
                clips.clone(),
                sources.clone(),
                SampleRate::HZ_48_000,
                0,
                Arc::default(),
            );
            let mut out = vec![0.0; 2 * 6000];
            for piece in out.chunks_mut(2 * block) {
                lane.render(piece);
            }
            assert_eq!(lane.pos(), 6000);
            out
        };
        let a = render(256);
        for block in [1, 17, 1000, 6000] {
            assert_eq!(a, render(block), "block {block}");
        }
        let left: Vec<f32> = a.iter().step_by(2).copied().collect();
        assert_eq!(left[99], 0.0);
        assert_eq!(left[100], 0.5);
        assert_eq!(left[101], 0.0);
        assert_eq!(left[200], 1.0);
        assert!(left[5000..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn unknown_media_is_silent_and_counted() {
        let sources: Arc<dyn SourceFactory> = Arc::new(ToneFactory::new());
        let errors = Arc::new(AtomicU64::new(0));
        let mut lane = LaneRenderer::new(
            vec![clip(1, 9, 0, 1000, 1.0)].into(),
            sources,
            SampleRate::HZ_48_000,
            0,
            errors.clone(),
        );
        let mut out = vec![1.0; 2 * 400];
        lane.render(&mut out);
        lane.render(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));
        assert_eq!(errors.load(Ordering::Relaxed), 1, "logged once per clip");
    }
}
