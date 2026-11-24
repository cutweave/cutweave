// SPDX-License-Identifier: GPL-3.0-or-later
//! Decoded source frames and the [`FrameSource`] the compositor pulls them from.
//!
//! A [`VideoFrame`] is what a decoder hands over (arch §7, path (a): CPU planes uploaded to the
//! GPU and converted in a shader): the raw planes in the decoder's layout, the colour tags and
//! the chroma siting. Frames are always in **coded orientation**; the display rotation
//! ([`FrameRequest::rotation`]) is applied by the compositor.
//!
//! The media layer (FFmpeg decode, WP-1D.03) and the frame cache implement [`FrameSource`];
//! until then [`crate::testcard::SyntheticSource`] serves generated test cards.

use std::sync::Arc;

use cutweave_model::ColorTags;

use crate::hash::Fnv;
use crate::scene::FrameRequest;

/// Pixel layout of a decoded frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PixelFormat {
    /// 8-bit 4:2:0, two planes: Y, then interleaved Cb Cr (hardware decoders).
    Nv12,
    /// 10-bit 4:2:0 in 16-bit little-endian words, value in the high 10 bits; two planes: Y,
    /// then interleaved Cb Cr (hardware decoders, HDR).
    P010,
    /// 8-bit 4:2:0, three planes: Y, Cb, Cr (software decoders).
    Yuv420p,
    /// 8-bit interleaved R'G'B'A, straight alpha (stills, generated pictures).
    Rgba8,
}

impl PixelFormat {
    /// Number of planes.
    #[must_use]
    pub const fn plane_count(self) -> usize {
        match self {
            PixelFormat::Nv12 | PixelFormat::P010 => 2,
            PixelFormat::Yuv420p => 3,
            PixelFormat::Rgba8 => 1,
        }
    }

    /// Whether the samples are Y'CbCr.
    #[must_use]
    pub const fn is_yuv(self) -> bool {
        !matches!(self, PixelFormat::Rgba8)
    }

    /// Significant bits per sample.
    #[must_use]
    pub const fn bit_depth(self) -> u8 {
        match self {
            PixelFormat::P010 => 10,
            _ => 8,
        }
    }

    /// Size in samples of plane `index` of a `w × h` frame, and bytes per texel of that plane.
    #[must_use]
    pub const fn plane_layout(self, index: usize, w: u32, h: u32) -> (u32, u32, u32) {
        let chroma = (w.div_ceil(2), h.div_ceil(2));
        match (self, index) {
            (PixelFormat::Nv12 | PixelFormat::Yuv420p, 0) => (w, h, 1),
            (PixelFormat::Nv12, _) => (chroma.0, chroma.1, 2),
            (PixelFormat::P010, 0) => (w, h, 2),
            (PixelFormat::P010, _) => (chroma.0, chroma.1, 4),
            (PixelFormat::Yuv420p, _) => (chroma.0, chroma.1, 1),
            (PixelFormat::Rgba8, _) => (w, h, 4),
        }
    }

    /// Whether every pixel is opaque by construction (no alpha channel).
    #[must_use]
    pub const fn is_opaque(self) -> bool {
        self.is_yuv()
    }

    /// The variant's name, as `Debug` prints it (hashed into cache keys without formatting).
    #[must_use]
    pub const fn debug_name(self) -> &'static str {
        match self {
            PixelFormat::Nv12 => "Nv12",
            PixelFormat::P010 => "P010",
            PixelFormat::Yuv420p => "Yuv420p",
            PixelFormat::Rgba8 => "Rgba8",
        }
    }
}

/// Where 4:2:0 chroma samples sit relative to luma (H.273 `chroma_sample_loc_type`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ChromaSiting {
    /// Co-sited with even luma columns, between rows (MPEG-2, H.264/HEVC default; type 0).
    #[default]
    Left,
    /// Between luma columns and rows (MPEG-1, JPEG; type 1).
    Center,
    /// Co-sited with even luma columns and rows (common for HDR / BT.2020; type 2).
    TopLeft,
}

impl ChromaSiting {
    /// Position of chroma sample `(0, 0)` in luma sample units (chroma sample `(i, j)` sits at
    /// luma `(2i + x, 2j + y)`).
    #[must_use]
    pub const fn offset(self) -> [f64; 2] {
        match self {
            ChromaSiting::Left => [0.0, 0.5],
            ChromaSiting::Center => [0.5, 0.5],
            ChromaSiting::TopLeft => [0.0, 0.0],
        }
    }
}

/// One plane of samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plane {
    /// The bytes, `stride` per row.
    pub data: Vec<u8>,
    /// Bytes per row (at least the plane width × bytes per texel).
    pub stride: u32,
}

/// Identity of a frame's pixel content: two frames with the same id must have identical planes
/// and tags (the decoder derives it from the media and the frame position). It keys the GPU
/// decode cache, so stills and paused frames are uploaded once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameId(pub u64);

impl FrameId {
    /// An id hashed from the frame content (for sources without a natural identity).
    #[must_use]
    pub fn of_content(format: PixelFormat, width: u32, height: u32, planes: &[Plane]) -> FrameId {
        let mut h = Fnv::new();
        h.str(&format!("{format:?}")).u32(width).u32(height);
        for p in planes {
            h.u32(p.stride).bytes(&p.data);
        }
        FrameId(h.finish())
    }
}

/// A decoded picture ready for upload.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoFrame {
    /// Content identity (see [`FrameId`]).
    pub id: FrameId,
    /// Width in pixels (luma samples).
    pub width: u32,
    /// Height in pixels (luma samples).
    pub height: u32,
    /// Sample layout.
    pub format: PixelFormat,
    /// The planes, in [`PixelFormat`] order.
    pub planes: Vec<Plane>,
    /// The decoder's colour tags; unspecified fields fall back to the probe's
    /// ([`FrameRequest::color`]), then to [`crate::color::InputColor::resolve`] defaults.
    pub color: ColorTags,
    /// Chroma siting of 4:2:0 formats.
    pub chroma_siting: ChromaSiting,
}

impl VideoFrame {
    /// Validates the plane layout and builds a frame.
    pub fn new(
        id: FrameId,
        width: u32,
        height: u32,
        format: PixelFormat,
        planes: Vec<Plane>,
        color: ColorTags,
    ) -> Result<VideoFrame, FrameError> {
        let frame = VideoFrame {
            id,
            width,
            height,
            format,
            planes,
            color,
            chroma_siting: ChromaSiting::default(),
        };
        frame.validate()?;
        Ok(frame)
    }

    /// Checks that the planes match the format and size: plane count, strides (at least a row,
    /// whole texels) and data lengths.
    pub fn validate(&self) -> Result<(), FrameError> {
        let (width, height, format) = (self.width, self.height, self.format);
        if width == 0 || height == 0 {
            return Err(FrameError::Invalid(format!("empty {width}x{height} frame")));
        }
        if self.planes.len() != format.plane_count() {
            return Err(FrameError::Invalid(format!(
                "{format:?} needs {} planes, got {}",
                format.plane_count(),
                self.planes.len()
            )));
        }
        for (i, p) in self.planes.iter().enumerate() {
            let (w, h, bpt) = format.plane_layout(i, width, height);
            let row = u64::from(w) * u64::from(bpt);
            let need = u64::from(p.stride) * u64::from(h - 1) + row;
            if u64::from(p.stride) < row || p.stride % bpt != 0 || (p.data.len() as u64) < need {
                return Err(FrameError::Invalid(format!(
                    "{format:?} plane {i}: stride {} / {} bytes do not fit {w}x{h}",
                    p.stride,
                    p.data.len()
                )));
            }
        }
        Ok(())
    }

    /// The same frame with another chroma siting.
    #[must_use]
    pub fn with_chroma_siting(mut self, siting: ChromaSiting) -> VideoFrame {
        self.chroma_siting = siting;
        self
    }

    /// Code value of plane `plane`, component `comp` at sample `(x, y)` (clamped to the plane).
    /// P010 values are returned as 10-bit codes.
    #[must_use]
    #[allow(clippy::many_single_char_names)]
    pub fn code(&self, plane: usize, comp: usize, x: i64, y: i64) -> u32 {
        let (w, h, bpt) = self.format.plane_layout(plane, self.width, self.height);
        let cx = u64::try_from(x.clamp(0, i64::from(w) - 1)).unwrap_or(0);
        let cy = u64::try_from(y.clamp(0, i64::from(h) - 1)).unwrap_or(0);
        let p = &self.planes[plane];
        let at = usize::try_from(cy * u64::from(p.stride) + cx * u64::from(bpt)).unwrap_or(0);
        match self.format {
            PixelFormat::P010 => {
                let i = at + comp * 2;
                u32::from(u16::from_le_bytes([p.data[i], p.data[i + 1]])) >> 6
            }
            _ => u32::from(p.data[at + comp]),
        }
    }
}

/// Why a frame could not be produced.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// The media or frame is not available (offline media, decode failure).
    #[error("frame unavailable: {0}")]
    Unavailable(String),
    /// The frame data is malformed.
    #[error("invalid frame: {0}")]
    Invalid(String),
}

/// Resolves frame requests to decoded frames (decoder pool / frame cache).
///
/// Implementations must be deterministic: the same request yields a frame with the same
/// [`FrameId`] and pixels (arch §8, render is a pure function of its inputs).
pub trait FrameSource {
    /// The frame for `request` (its media, stream and source time).
    fn frame(&self, request: &FrameRequest) -> Result<Arc<VideoFrame>, FrameError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_layouts() {
        assert_eq!(PixelFormat::Nv12.plane_layout(1, 1919, 1081), (960, 541, 2));
        assert_eq!(PixelFormat::P010.plane_layout(0, 8, 8), (8, 8, 2));
        assert_eq!(PixelFormat::P010.plane_layout(1, 8, 8), (4, 4, 4));
        assert_eq!(PixelFormat::Yuv420p.plane_layout(2, 7, 5), (4, 3, 1));
        assert_eq!(PixelFormat::Rgba8.plane_layout(0, 3, 2), (3, 2, 4));
    }

    #[test]
    fn frames_validate_their_planes() {
        let ok = VideoFrame::new(
            FrameId(1),
            4,
            2,
            PixelFormat::Nv12,
            vec![
                Plane {
                    data: vec![0; 8],
                    stride: 4,
                },
                Plane {
                    data: vec![0; 4],
                    stride: 4,
                },
            ],
            ColorTags::default(),
        );
        assert!(ok.is_ok());
        let short = VideoFrame::new(
            FrameId(1),
            4,
            2,
            PixelFormat::Rgba8,
            vec![Plane {
                data: vec![0; 31],
                stride: 16,
            }],
            ColorTags::default(),
        );
        assert!(matches!(short, Err(FrameError::Invalid(_))));
        let planes = VideoFrame::new(
            FrameId(1),
            4,
            2,
            PixelFormat::Yuv420p,
            vec![],
            ColorTags::default(),
        );
        assert!(matches!(planes, Err(FrameError::Invalid(_))));
    }

    #[test]
    fn p010_codes_are_the_high_ten_bits() {
        let word = (700u16 << 6).to_le_bytes();
        let f = VideoFrame::new(
            FrameId(1),
            2,
            2,
            PixelFormat::P010,
            vec![
                Plane {
                    data: [word; 4].concat(),
                    stride: 4,
                },
                Plane {
                    data: [word; 2].concat(),
                    stride: 4,
                },
            ],
            ColorTags::default(),
        )
        .unwrap();
        assert_eq!(f.code(0, 0, 1, 1), 700);
        assert_eq!(f.code(1, 1, 5, 5), 700);
    }
}
