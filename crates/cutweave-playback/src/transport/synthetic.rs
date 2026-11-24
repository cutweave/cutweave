// SPDX-License-Identifier: GPL-3.0-or-later
//! [`SyntheticSource`]: a deterministic test pattern whose pixels encode the frame number, for
//! tests and demos until FFmpeg decode lands (R31).
//!
//! Every frame is a gradient with a white bar that sweeps across the picture once every 60
//! frames, and starts (payload bytes 0..16) with a marker: the frame number as a little-endian
//! `i64` followed by its bitwise complement. [`decode_frame_number`] reads it back from any
//! delivered frame, at any size and in either pixel format, so tests can check exactly which
//! frame reached the sink.

use crate::shm::{FrameLayout, PixelFormat};
use crate::transport::cancel::CancelToken;
use crate::transport::source::{FrameSource, RenderRequest, Rendered, SequenceTiming, SourceError};

/// Bytes of the frame-number marker at the start of the payload.
pub const MARKER_LEN: usize = 16;
/// Frames per sweep of the moving bar.
const SWEEP_FRAMES: i64 = 60;

/// A [`FrameSource`] that paints [`SyntheticSource::paint`]'s test pattern.
#[derive(Clone, Debug)]
pub struct SyntheticSource {
    timing: SequenceTiming,
    rev: u64,
    renders: u64,
}

impl SyntheticSource {
    /// A source with `timing` showing document revision 0.
    #[must_use]
    pub fn new(timing: SequenceTiming) -> SyntheticSource {
        SyntheticSource {
            timing,
            rev: 0,
            renders: 0,
        }
    }

    /// The same source reporting document revision `rev`.
    #[must_use]
    pub fn with_rev(mut self, rev: u64) -> SyntheticSource {
        self.rev = rev;
        self
    }

    /// Frames rendered so far.
    #[must_use]
    pub fn renders(&self) -> u64 {
        self.renders
    }

    /// Paints frame `frame` into `out` laid out as `layout` (`out` holds at least
    /// `layout.len()` bytes; shorter buffers get as much as fits).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // pattern bytes
    pub fn paint(frame: i64, layout: &FrameLayout, out: &mut [u8]) {
        let (w, h) = (layout.width().max(1), layout.height().max(1));
        let bar_w = (w / 30).max(1);
        let bar_x =
            (frame.rem_euclid(SWEEP_FRAMES) as u64 * u64::from(w) / SWEEP_FRAMES as u64) as u32;
        let in_bar = |x: u32| x >= bar_x && x < bar_x.saturating_add(bar_w);
        let shade = (frame.rem_euclid(256)) as u8;
        let [p0, p1] = layout.planes();
        match layout.format() {
            PixelFormat::Rgba8 => {
                for y in 0..h {
                    let row = p0.offset + y as usize * p0.stride as usize;
                    let Some(row) = out.get_mut(row..row + w as usize * 4) else {
                        break;
                    };
                    let b = (u64::from(y) * 255 / u64::from(h)) as u8;
                    for (x, px) in (0..w).zip(row.as_chunks_mut::<4>().0) {
                        *px = if in_bar(x) {
                            [255, 255, 255, 255]
                        } else {
                            [shade, (u64::from(x) * 255 / u64::from(w)) as u8, b, 255]
                        };
                    }
                }
            }
            PixelFormat::Nv12 => {
                for y in 0..h {
                    let row = p0.offset + y as usize * p0.stride as usize;
                    let Some(row) = out.get_mut(row..row + w as usize) else {
                        break;
                    };
                    for (x, px) in (0..w).zip(row.iter_mut()) {
                        *px = if in_bar(x) {
                            235
                        } else {
                            16 + (u64::from(x) * 200 / u64::from(w)) as u8
                        };
                    }
                }
                for y in 0..h / 2 {
                    let row = p1.offset + y as usize * p1.stride as usize;
                    let Some(row) = out.get_mut(row..row + w as usize) else {
                        break;
                    };
                    for uv in row.as_chunks_mut::<2>().0 {
                        *uv = [128, 128u8.wrapping_add(shade / 4)];
                    }
                }
            }
        }
        let start = p0.offset.min(out.len());
        if let Some(marker) = out.get_mut(start..start + MARKER_LEN) {
            marker[..8].copy_from_slice(&frame.to_le_bytes());
            marker[8..].copy_from_slice(&(!frame).to_le_bytes());
        }
    }
}

/// The frame number [`SyntheticSource`] painted into `pixels` (laid out as `layout`), or `None`
/// if the marker is missing or corrupt.
#[must_use]
pub fn decode_frame_number(layout: &FrameLayout, pixels: &[u8]) -> Option<i64> {
    let start = layout.planes()[0].offset;
    let marker = pixels.get(start..start + MARKER_LEN)?;
    let frame = i64::from_le_bytes(marker[..8].try_into().ok()?);
    let check = i64::from_le_bytes(marker[8..].try_into().ok()?);
    (check == !frame).then_some(frame)
}

impl FrameSource for SyntheticSource {
    fn timing(&self) -> SequenceTiming {
        self.timing
    }

    fn render(
        &mut self,
        request: &RenderRequest,
        cancel: &CancelToken,
        out: &mut [u8],
    ) -> Result<Rendered, SourceError> {
        if cancel.is_cancelled() {
            return Err(SourceError::Cancelled);
        }
        if out.len() < request.layout.len() {
            return Err(SourceError::Failed(format!(
                "output buffer of {} bytes, the layout needs {}",
                out.len(),
                request.layout.len()
            )));
        }
        SyntheticSource::paint(request.frame, &request.layout, out);
        self.renders += 1;
        Ok(Rendered { rev: self.rev })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::{FrameRate, Time};
    use cutweave_render::Quality;

    use super::*;
    use crate::transport::source::RenderPurpose;

    fn request(frame: i64, layout: FrameLayout) -> RenderRequest {
        RenderRequest {
            frame,
            t: Time::ZERO,
            quality: Quality::FULL,
            layout,
            purpose: RenderPurpose::Playback,
        }
    }

    #[test]
    fn the_frame_number_survives_every_layout() {
        let layouts = [
            FrameLayout::packed(64, 36, PixelFormat::Rgba8).unwrap(),
            FrameLayout::aligned(64, 36, PixelFormat::Rgba8, 256).unwrap(),
            FrameLayout::packed(16, 10, PixelFormat::Nv12).unwrap(),
            FrameLayout::aligned(160, 90, PixelFormat::Nv12, 256).unwrap(),
            FrameLayout::packed(4, 4, PixelFormat::Rgba8).unwrap(),
        ];
        let timing = SequenceTiming::new(FrameRate::FPS_30, Time::from_secs(1).unwrap());
        let mut source = SyntheticSource::new(timing).with_rev(7);
        for layout in layouts {
            let mut buf = vec![0u8; layout.len()];
            for frame in [0, 1, 59, 60, 299, 1_000_000, -1] {
                let r = source
                    .render(&request(frame, layout), &CancelToken::never(), &mut buf)
                    .unwrap();
                assert_eq!(r.rev, 7);
                assert_eq!(
                    decode_frame_number(&layout, &buf),
                    Some(frame),
                    "{layout:?}"
                );
            }
        }
        assert_eq!(source.renders(), 35);
    }

    #[test]
    fn corrupt_or_short_frames_have_no_number() {
        let layout = FrameLayout::packed(8, 2, PixelFormat::Rgba8).unwrap();
        let mut buf = vec![0u8; layout.len()];
        SyntheticSource::paint(42, &layout, &mut buf);
        assert_eq!(decode_frame_number(&layout, &buf), Some(42));
        buf[3] ^= 1;
        assert_eq!(decode_frame_number(&layout, &buf), None);
        assert_eq!(decode_frame_number(&layout, &buf[..10]), None);
    }

    #[test]
    fn a_cancelled_token_or_short_buffer_renders_nothing() {
        let timing = SequenceTiming::new(FrameRate::FPS_30, Time::from_secs(1).unwrap());
        let mut source = SyntheticSource::new(timing);
        let layout = FrameLayout::packed(8, 8, PixelFormat::Rgba8).unwrap();
        let gate = std::sync::Arc::new(crate::transport::EpochGate::new());
        let token = gate.token(0);
        gate.advance();
        let mut buf = vec![0u8; layout.len()];
        assert_eq!(
            source.render(&request(1, layout), &token, &mut buf),
            Err(SourceError::Cancelled)
        );
        let mut short = vec![0u8; 8];
        assert!(matches!(
            source.render(&request(1, layout), &CancelToken::never(), &mut short),
            Err(SourceError::Failed(_))
        ));
        assert_eq!(source.renders(), 0);
    }
}
