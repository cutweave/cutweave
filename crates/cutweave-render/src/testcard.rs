// SPDX-License-Identifier: GPL-3.0-or-later
//! Synthetic test cards encoded exactly like decoder output, and a [`FrameSource`] serving them.
//!
//! FFmpeg decode (WP-1D.03) is not wired yet, and golden tests must not depend on the fetched
//! corpus anyway, so the compositor's tests (and later the quality-ladder and `render.frame`
//! tests) use generated pictures: a [`TestCard`] describes an R'G'B' *signal* pattern and the
//! target layout (NV12 / P010 / Yuv420p / RGBA8 with colour tags and chroma siting), and
//! [`TestCard::frame`] quantises it into planes the way an encoder + decoder round trip would.
//!
//! Chroma is subsampled consistently with the declared siting (co-sited samples take the
//! co-sited luma site, interstitial ones average the two neighbours), so a correct 4:2:0
//! upsampler reproduces flat areas exactly.

// Pixel and matrix math reads best with the conventional single-letter names.
#![allow(clippy::many_single_char_names)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cutweave_model::{ColorTags, FLICKS_PER_SECOND, MediaId};

use crate::color::{InputColor, range_params, rgb_to_ycbcr};
use crate::frame::{
    ChromaSiting, FrameError, FrameId, FrameSource, PixelFormat, Plane, VideoFrame,
};
use crate::scene::FrameRequest;

/// 75% colour bars: white, yellow, cyan, green, magenta, red, blue (R'G'B' signal).
pub const BARS_75: [[f64; 3]; 7] = [
    [0.75, 0.75, 0.75],
    [0.75, 0.75, 0.0],
    [0.0, 0.75, 0.75],
    [0.0, 0.75, 0.0],
    [0.75, 0.0, 0.75],
    [0.75, 0.0, 0.0],
    [0.0, 0.0, 0.75],
];

/// What a test card shows (as an R'G'B' signal in `[0, 1]`, in the card's own encoding).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pattern {
    /// SMPTE-style card: 75% bars (top 60%), a horizontal luma ramp (15%) and 11 grey steps
    /// (bottom 25%). When animated, a white bar sweeps across once every 100 frames.
    ColorBars,
    /// One flat colour.
    Solid([f64; 3]),
    /// Hue across, value down, with fine diagonal detail: exercises chroma upsampling and
    /// resampling.
    Sweep,
}

/// Alpha of an RGBA8 card.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Alpha {
    /// 255 everywhere.
    #[default]
    Opaque,
    /// One constant value in `[0, 1]`.
    Constant(f64),
    /// 0 at the left edge to 1 at the right edge.
    RampX,
}

/// A generated picture in a decoder layout.
#[derive(Clone, Debug, PartialEq)]
pub struct TestCard {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The picture.
    pub pattern: Pattern,
    /// Sample layout.
    pub format: PixelFormat,
    /// Colour tags written into the frame (and used to encode it; unspecified fields take the
    /// [`InputColor::resolve`] defaults).
    pub color: ColorTags,
    /// Chroma siting of 4:2:0 layouts.
    pub siting: ChromaSiting,
    /// Alpha of RGBA8 layouts.
    pub alpha: Alpha,
    /// Whether the picture changes with the frame index.
    pub animated: bool,
}

impl TestCard {
    /// A static card with default siting and opaque alpha.
    #[must_use]
    pub fn new(
        width: u32,
        height: u32,
        pattern: Pattern,
        format: PixelFormat,
        color: ColorTags,
    ) -> TestCard {
        TestCard {
            width,
            height,
            pattern,
            format,
            color,
            siting: ChromaSiting::Left,
            alpha: Alpha::Opaque,
            animated: false,
        }
    }

    /// The R'G'B' signal at pixel `(x, y)` of frame `index`.
    #[must_use]
    pub fn signal(&self, x: u32, y: u32, index: i64) -> [f64; 3] {
        let u = (f64::from(x) + 0.5) / f64::from(self.width);
        let v = (f64::from(y) + 0.5) / f64::from(self.height);
        match self.pattern {
            Pattern::Solid(c) => c.map(|c| c.clamp(0.0, 1.0)),
            Pattern::ColorBars => {
                if self.animated {
                    let sweep = (index.rem_euclid(100) as f64 + 0.5) / 100.0;
                    // At least about a pixel wide, so small cards change too.
                    if (u - sweep).abs() < 0.0125f64.max(0.75 / f64::from(self.width)) {
                        return [1.0; 3];
                    }
                }
                if v < 0.6 {
                    BARS_75[((u * 7.0) as usize).min(6)]
                } else if v < 0.75 {
                    [u; 3]
                } else {
                    let s = ((u * 11.0).floor() / 10.0).min(1.0);
                    [s; 3]
                }
            }
            Pattern::Sweep => {
                let detail = 0.08 * (f64::from(x + y) * 0.9).sin();
                let value = (1.0 - v * 0.9 + detail).clamp(0.0, 1.0);
                hsv(u + index as f64 * 0.01, 0.85, value)
            }
        }
    }

    fn alpha_at(&self, x: u32) -> f64 {
        match self.alpha {
            Alpha::Opaque => 1.0,
            Alpha::Constant(a) => a.clamp(0.0, 1.0),
            Alpha::RampX => f64::from(x) / f64::from((self.width - 1).max(1)),
        }
    }

    /// Encodes frame `index` into planes.
    #[must_use]
    pub fn frame(&self, index: i64) -> VideoFrame {
        let (w, h) = (self.width.max(1), self.height.max(1));
        let planes = if self.format.is_yuv() {
            self.yuv_planes(w, h, index)
        } else {
            vec![self.rgba_plane(w, h, index)]
        };
        let id = FrameId::of_content(self.format, w, h, &planes);
        VideoFrame {
            id,
            width: w,
            height: h,
            format: self.format,
            planes,
            color: self.color.clone(),
            chroma_siting: self.siting,
        }
    }

    fn rgba_plane(&self, w: u32, h: u32, index: i64) -> Plane {
        let color = InputColor::resolve(&self.color, false, h);
        let rp = range_params(color.range, 8);
        let q = |v: f64| (rp.y_off + rp.y_scale * v).round().clamp(0.0, 255.0) as u8;
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let s = self.signal(x, y, index);
                let a = (self.alpha_at(x) * 255.0).round() as u8;
                data.extend_from_slice(&[q(s[0]), q(s[1]), q(s[2]), a]);
            }
        }
        Plane {
            data,
            stride: w * 4,
        }
    }

    fn yuv_planes(&self, w: u32, h: u32, index: i64) -> Vec<Plane> {
        let color = InputColor::resolve(&self.color, true, h);
        let (kr, kb) = color
            .matrix
            .map_or((0.2126, 0.0722), crate::color::YcbcrMatrix::kr_kb);
        let bits = self.format.bit_depth();
        let rp = range_params(color.range, bits);
        let max = f64::from((1u32 << bits) - 1);
        let q = |v: f64| v.round().clamp(0.0, max) as u16;
        let ycc =
            |x: u32, y: u32| rgb_to_ycbcr(self.signal(x.min(w - 1), y.min(h - 1), index), kr, kb);

        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut luma = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                luma.push(q(rp.y_off + rp.y_scale * ycc(x, y)[0]));
            }
        }
        let (mut cb, mut cr) = (Vec::new(), Vec::new());
        for j in 0..ch {
            for i in 0..cw {
                let sites: &[(u32, u32)] = match self.siting {
                    ChromaSiting::TopLeft => &[(2 * i, 2 * j)],
                    ChromaSiting::Left => &[(2 * i, 2 * j), (2 * i, 2 * j + 1)],
                    ChromaSiting::Center => &[
                        (2 * i, 2 * j),
                        (2 * i + 1, 2 * j),
                        (2 * i, 2 * j + 1),
                        (2 * i + 1, 2 * j + 1),
                    ],
                };
                let n = sites.len() as f64;
                let (sb, sr) = sites.iter().fold((0.0, 0.0), |(b, r), &(x, y)| {
                    let c = ycc(x, y);
                    (b + c[1], r + c[2])
                });
                cb.push(q(rp.c_off + rp.c_scale * sb / n));
                cr.push(q(rp.c_off + rp.c_scale * sr / n));
            }
        }

        let bytes8 = |v: &[u16]| v.iter().map(|&s| s as u8).collect::<Vec<u8>>();
        let words = |v: &[u16]| {
            v.iter()
                .flat_map(|&s| (s << 6).to_le_bytes())
                .collect::<Vec<u8>>()
        };
        let interleave = |a: &[u16], b: &[u16]| {
            a.iter()
                .zip(b)
                .flat_map(|(&x, &y)| [x, y])
                .collect::<Vec<u16>>()
        };
        match self.format {
            PixelFormat::Nv12 => vec![
                Plane {
                    data: bytes8(&luma),
                    stride: w,
                },
                Plane {
                    data: bytes8(&interleave(&cb, &cr)),
                    stride: cw * 2,
                },
            ],
            PixelFormat::P010 => vec![
                Plane {
                    data: words(&luma),
                    stride: w * 2,
                },
                Plane {
                    data: words(&interleave(&cb, &cr)),
                    stride: cw * 4,
                },
            ],
            _ => vec![
                Plane {
                    data: bytes8(&luma),
                    stride: w,
                },
                Plane {
                    data: bytes8(&cb),
                    stride: cw,
                },
                Plane {
                    data: bytes8(&cr),
                    stride: cw,
                },
            ],
        }
    }
}

fn hsv(h: f64, s: f64, v: f64) -> [f64; 3] {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let i = h6.floor();
    let f = h6 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as u32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// A [`FrameSource`] serving test cards by media id. Animated cards advance one frame per
/// 1/25 s of source time. Generated frames are memoised.
#[derive(Debug, Default)]
pub struct SyntheticSource {
    cards: HashMap<MediaId, TestCard>,
    frames: Mutex<HashMap<(MediaId, i64), Arc<VideoFrame>>>,
}

impl SyntheticSource {
    /// An empty source (every request is unavailable).
    #[must_use]
    pub fn new() -> SyntheticSource {
        SyntheticSource::default()
    }

    /// Serves `card` for `media`.
    #[must_use]
    pub fn with(mut self, media: MediaId, card: TestCard) -> SyntheticSource {
        self.insert(media, card);
        self
    }

    /// Serves `card` for `media`.
    pub fn insert(&mut self, media: MediaId, card: TestCard) {
        self.cards.insert(media, card);
        if let Ok(mut f) = self.frames.lock() {
            f.retain(|(m, _), _| *m != media);
        }
    }
}

impl FrameSource for SyntheticSource {
    fn frame(&self, request: &FrameRequest) -> Result<Arc<VideoFrame>, FrameError> {
        let card = self.cards.get(&request.media).ok_or_else(|| {
            FrameError::Unavailable(format!("no test card for {}", request.media))
        })?;
        let index = if card.animated {
            request
                .source_time
                .flicks()
                .div_euclid(FLICKS_PER_SECOND / 25)
        } else {
            0
        };
        let key = (request.media, index);
        let mut frames = self
            .frames
            .lock()
            .map_err(|_| FrameError::Unavailable("test card cache poisoned".into()))?;
        Ok(Arc::clone(
            frames
                .entry(key)
                .or_insert_with(|| Arc::new(card.frame(index))),
        ))
    }
}

#[cfg(test)]
mod tests {
    use cutweave_model::{ColorRange, MatrixCoefficients, Time};

    use super::*;
    use crate::color::{InputTransform, ToneMap};

    fn tags(matrix: MatrixCoefficients, range: ColorRange) -> ColorTags {
        ColorTags {
            matrix: Some(matrix),
            range: Some(range),
            ..ColorTags::default()
        }
    }

    #[test]
    fn bars_encode_to_published_bt709_limited_codes() {
        let card = TestCard::new(
            70,
            10,
            Pattern::ColorBars,
            PixelFormat::Yuv420p,
            tags(MatrixCoefficients::Bt709, ColorRange::Limited),
        );
        let f = card.frame(0);
        // 75% white: Y' = 16 + 219·0.75 = 180.25 → 180, Cb = Cr = 128.
        assert_eq!(f.code(0, 0, 0, 0), 180);
        assert_eq!((f.code(1, 0, 0, 0), f.code(2, 0, 0, 0)), (128, 128));
        // 75% yellow (BT.709): Y' 168, Cb 44, Cr 136 (published 75% bar values).
        assert_eq!(f.code(0, 0, 12, 0), 168);
        assert_eq!((f.code(1, 0, 6, 0), f.code(2, 0, 6, 0)), (44, 136));
    }

    #[test]
    fn every_layout_decodes_back_to_the_signal() {
        for (format, matrix, range) in [
            (
                PixelFormat::Nv12,
                MatrixCoefficients::Bt709,
                ColorRange::Limited,
            ),
            (
                PixelFormat::Yuv420p,
                MatrixCoefficients::Smpte170M,
                ColorRange::Limited,
            ),
            (
                PixelFormat::P010,
                MatrixCoefficients::Bt709,
                ColorRange::Full,
            ),
        ] {
            let card = TestCard::new(
                8,
                4,
                Pattern::Solid([0.2, 0.6, 0.4]),
                format,
                tags(matrix, range),
            );
            let f = card.frame(0);
            let it = InputTransform::new(
                &InputColor::resolve(&f.color, true, f.height),
                format.bit_depth(),
                &ToneMap::default(),
            );
            let (y, cb, cr) = (
                f.code(0, 0, 3, 2),
                f.code(1, 0, 1, 1),
                f.code(
                    if format == PixelFormat::Yuv420p { 2 } else { 1 },
                    usize::from(format != PixelFormat::Yuv420p),
                    1,
                    1,
                ),
            );
            let s = it.codes_to_signal(f64::from(y), f64::from(cb), f64::from(cr));
            let tol = if format == PixelFormat::P010 {
                3e-3
            } else {
                1.2e-2
            };
            for (got, want) in s.iter().zip([0.2, 0.6, 0.4]) {
                assert!((got - want).abs() < tol, "{format:?}: {s:?}");
            }
        }
        let rgba = TestCard {
            alpha: Alpha::RampX,
            ..TestCard::new(
                5,
                1,
                Pattern::Solid([1.0, 0.5, 0.0]),
                PixelFormat::Rgba8,
                ColorTags::default(),
            )
        }
        .frame(0);
        assert_eq!(&rgba.planes[0].data[..4], &[255, 128, 0, 0]);
        assert_eq!(&rgba.planes[0].data[16..], &[255, 128, 0, 255]);
    }

    #[test]
    fn synthetic_source_is_deterministic_and_reports_missing_media() {
        let media = MediaId::from_u128(7);
        let card = TestCard {
            animated: true,
            ..TestCard::new(
                16,
                8,
                Pattern::ColorBars,
                PixelFormat::Nv12,
                ColorTags::default(),
            )
        };
        let src = SyntheticSource::new().with(media, card);
        let mut req = FrameRequest {
            media,
            stream: Some(0),
            source_time: Time::ZERO,
            source_size: Some((16, 8)),
            rotation: 0,
            color: ColorTags::default(),
            still: false,
        };
        let a = src.frame(&req).unwrap();
        let b = src.frame(&req).unwrap();
        assert_eq!(a.id, b.id);
        req.source_time = Time::from_secs(1).unwrap();
        assert_ne!(src.frame(&req).unwrap().id, a.id);
        req.media = MediaId::from_u128(8);
        assert!(matches!(src.frame(&req), Err(FrameError::Unavailable(_))));
    }
}
