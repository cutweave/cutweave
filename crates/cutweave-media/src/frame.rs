// SPDX-License-Identifier: GPL-3.0-or-later
//! Decoded media: [`VideoFrame`] (CPU planes in the decoder's layout, arch §7 path (a)) and
//! [`AudioChunk`] (planar `f32`).
//!
//! Timestamps are media time in flicks (ADR-0004): a frame's `pts` is its presentation time in
//! the source, from the stream's own timestamps (never `n / fps`, ADR-0003). Frames are in coded
//! orientation; the display rotation is in the probe. The plane layout of the formats the
//! compositor shares (`Nv12`, `P010`, `Yuv420p`, `Rgba8`) matches `cutweave_render::frame`
//! exactly, so handing such a frame to the compositor moves the planes without copying. Software
//! decoders also produce planar 10-bit and 4:2:2 layouts (WP-1D.03), which the GPU upload path
//! converts (WP-1F.02).

use cutweave_model::{ColorTags, SampleRate, Time};

use crate::MediaError;

/// Pixel layout of a decoded frame. `Nv12`, `P010`, `Yuv420p` and `Rgba8` are the compositor's
/// `cutweave_render::PixelFormat` (same plane layout); the planar 10-bit and 4:2:2 layouts are
/// what software decoders produce (WP-1D.03).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PixelFormat {
    /// 8-bit 4:2:0, two planes: Y, then interleaved Cb Cr (hardware decoders).
    Nv12,
    /// 10-bit 4:2:0 in 16-bit little-endian words, value in the high 10 bits; two planes: Y,
    /// then interleaved Cb Cr (hardware decoders, HDR).
    P010,
    /// 8-bit 4:2:0, three planes: Y, Cb, Cr (software decoders: H.264, HEVC Main, VP9, AV1).
    Yuv420p,
    /// 10-bit 4:2:0 in 16-bit little-endian words, value in the **low** 10 bits; three planes:
    /// Y, Cb, Cr (software decoders: HEVC Main 10, 10-bit VP9/AV1/H.264). FFmpeg's
    /// `yuv420p10le`.
    Yuv420p10,
    /// 8-bit 4:2:2, three planes: Y, then Cb and Cr at half width and full height (DNxHD,
    /// MJPEG, 4:2:2 H.264).
    Yuv422p,
    /// 10-bit 4:2:2 in 16-bit little-endian words, value in the low 10 bits; three planes
    /// (ProRes 422, DNxHR, 10-bit 4:2:2 H.264/HEVC). FFmpeg's `yuv422p10le`.
    Yuv422p10,
    /// 8-bit interleaved R'G'B'A, straight alpha (thumbnails, stills).
    Rgba8,
}

impl PixelFormat {
    /// Every layout, for tests and tables.
    pub const ALL: [PixelFormat; 7] = [
        PixelFormat::Nv12,
        PixelFormat::P010,
        PixelFormat::Yuv420p,
        PixelFormat::Yuv420p10,
        PixelFormat::Yuv422p,
        PixelFormat::Yuv422p10,
        PixelFormat::Rgba8,
    ];

    /// Number of planes.
    #[must_use]
    pub const fn plane_count(self) -> usize {
        match self {
            PixelFormat::Nv12 | PixelFormat::P010 => 2,
            PixelFormat::Yuv420p
            | PixelFormat::Yuv420p10
            | PixelFormat::Yuv422p
            | PixelFormat::Yuv422p10 => 3,
            PixelFormat::Rgba8 => 1,
        }
    }

    /// Size in samples of plane `index` of a `w × h` frame, and bytes per texel of that plane
    /// (`(width, height, bytes)`). Subsampled chroma rounds up, so odd sizes keep their last
    /// column and row.
    #[must_use]
    pub const fn plane_layout(self, index: usize, w: u32, h: u32) -> (u32, u32, u32) {
        let c420 = (w.div_ceil(2), h.div_ceil(2));
        let c422 = (w.div_ceil(2), h);
        match (self, index) {
            (PixelFormat::Nv12 | PixelFormat::Yuv420p | PixelFormat::Yuv422p, 0) => (w, h, 1),
            (PixelFormat::P010 | PixelFormat::Yuv420p10 | PixelFormat::Yuv422p10, 0) => (w, h, 2),
            // NV12's interleaved CbCr pair and a 10-bit sample are both 2 bytes.
            (PixelFormat::Nv12 | PixelFormat::Yuv420p10, _) => (c420.0, c420.1, 2),
            (PixelFormat::P010, _) => (c420.0, c420.1, 4),
            (PixelFormat::Yuv420p, _) => (c420.0, c420.1, 1),
            (PixelFormat::Yuv422p, _) => (c422.0, c422.1, 1),
            (PixelFormat::Yuv422p10, _) => (c422.0, c422.1, 2),
            (PixelFormat::Rgba8, _) => (w, h, 4),
        }
    }

    /// Significant bits per sample.
    #[must_use]
    pub const fn bit_depth(self) -> u8 {
        match self {
            PixelFormat::P010 | PixelFormat::Yuv420p10 | PixelFormat::Yuv422p10 => 10,
            _ => 8,
        }
    }

    /// Whether the samples are Y'CbCr (everything but `Rgba8`).
    #[must_use]
    pub const fn is_yuv(self) -> bool {
        !matches!(self, PixelFormat::Rgba8)
    }

    /// FFmpeg's name for the layout (`nv12`, `p010le`, `yuv420p`, `yuv420p10le`, `yuv422p`,
    /// `yuv422p10le`, `rgba`).
    #[must_use]
    pub const fn ffmpeg_name(self) -> &'static str {
        match self {
            PixelFormat::Nv12 => "nv12",
            PixelFormat::P010 => "p010le",
            PixelFormat::Yuv420p => "yuv420p",
            PixelFormat::Yuv420p10 => "yuv420p10le",
            PixelFormat::Yuv422p => "yuv422p",
            PixelFormat::Yuv422p10 => "yuv422p10le",
            PixelFormat::Rgba8 => "rgba",
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

/// A decoded picture in CPU memory.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoFrame {
    /// Presentation time in the source (media time, flicks).
    pub pts: Time,
    /// Width in pixels (luma samples).
    pub width: u32,
    /// Height in pixels (luma samples).
    pub height: u32,
    /// Sample layout.
    pub format: PixelFormat,
    /// The planes, in [`PixelFormat`] order.
    pub planes: Vec<Plane>,
    /// The decoder's color tags (unspecified fields fall back to the probe's).
    pub color: ColorTags,
    /// The media is damaged here: the decoder concealed errors in this picture, or returned it
    /// in place of the requested one, which it could not decode (the nearest following frame,
    /// else the nearest preceding one; ADR-0003). Playback and scrubbing go on; the asset can be
    /// flagged.
    pub damaged: bool,
}

impl VideoFrame {
    /// Checks that the planes match the format and size: plane count, strides (at least a row,
    /// whole texels) and data lengths.
    pub fn validate(&self) -> Result<(), MediaError> {
        let (width, height, format) = (self.width, self.height, self.format);
        let invalid = |message: String| Err(MediaError::Invalid { message });
        if width == 0 || height == 0 {
            return invalid(format!("empty {width}x{height} frame"));
        }
        if self.planes.len() != format.plane_count() {
            return invalid(format!(
                "{format:?} needs {} planes, got {}",
                format.plane_count(),
                self.planes.len()
            ));
        }
        for (i, p) in self.planes.iter().enumerate() {
            let (w, h, bpt) = format.plane_layout(i, width, height);
            let row = u64::from(w) * u64::from(bpt);
            let need = u64::from(p.stride) * u64::from(h - 1) + row;
            if u64::from(p.stride) < row || p.stride % bpt != 0 || (p.data.len() as u64) < need {
                return invalid(format!(
                    "{format:?} plane {i}: stride {} / {} bytes do not fit {w}x{h}",
                    p.stride,
                    p.data.len()
                ));
            }
        }
        Ok(())
    }

    /// Row `y` of plane `index` without stride padding, or `None` if out of bounds.
    #[must_use]
    pub fn row(&self, index: usize, y: u32) -> Option<&[u8]> {
        let plane = self.planes.get(index)?;
        let (w, h, bpt) = self.format.plane_layout(index, self.width, self.height);
        if y >= h {
            return None;
        }
        let start = usize::try_from(u64::from(plane.stride) * u64::from(y)).ok()?;
        let len = usize::try_from(u64::from(w) * u64::from(bpt)).ok()?;
        plane.data.get(start..start.checked_add(len)?)
    }
}

/// The sample layout of an audio stream as decoded: its native rate and channel count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioFormat {
    /// Samples per second per channel.
    pub rate: SampleRate,
    /// Channel count (≥ 1).
    pub channels: u16,
}

/// Decoded audio: planar `f32` samples at the stream's native rate (resampling is the mixer's
/// job, WP-1G).
#[derive(Clone, Debug, PartialEq)]
pub struct AudioChunk {
    /// Media time of the first sample (sample `n` of the stream starts at
    /// `rate.samples_to_time(n)`).
    pub pts: Time,
    /// Samples per second per channel.
    pub rate: SampleRate,
    /// Channel count; equals `planes.len()`.
    pub channels: u16,
    /// One plane per channel, all the same length, nominally in `[-1, 1]`.
    pub planes: Vec<Vec<f32>>,
}

impl AudioChunk {
    /// Samples per channel.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.planes.first().map_or(0, Vec::len)
    }

    /// The format of the chunk.
    #[must_use]
    pub fn format(&self) -> AudioFormat {
        AudioFormat {
            rate: self.rate,
            channels: self.channels,
        }
    }

    /// Checks `channels == planes.len()` (and ≥ 1) and that every plane has the same length.
    pub fn validate(&self) -> Result<(), MediaError> {
        let n = self.frames();
        if self.channels == 0 || usize::from(self.channels) != self.planes.len() {
            return Err(MediaError::Invalid {
                message: format!(
                    "audio chunk declares {} channels but has {} planes",
                    self.channels,
                    self.planes.len()
                ),
            });
        }
        if self.planes.iter().any(|p| p.len() != n) {
            return Err(MediaError::Invalid {
                message: "audio chunk planes differ in length".to_owned(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(format: PixelFormat, w: u32, h: u32) -> VideoFrame {
        let planes = (0..format.plane_count())
            .map(|i| {
                let (pw, ph, bpt) = format.plane_layout(i, w, h);
                Plane {
                    data: vec![0; (pw * bpt * ph) as usize],
                    stride: pw * bpt,
                }
            })
            .collect();
        VideoFrame {
            pts: Time::ZERO,
            width: w,
            height: h,
            format,
            planes,
            color: ColorTags::default(),
            damaged: false,
        }
    }

    #[test]
    fn layouts_match_the_compositor() {
        // The same table as cutweave_render::frame::tests::plane_layouts.
        assert_eq!(PixelFormat::Nv12.plane_layout(1, 1919, 1081), (960, 541, 2));
        assert_eq!(PixelFormat::P010.plane_layout(0, 8, 8), (8, 8, 2));
        assert_eq!(PixelFormat::P010.plane_layout(1, 8, 8), (4, 4, 4));
        assert_eq!(PixelFormat::Yuv420p.plane_layout(2, 7, 5), (4, 3, 1));
        assert_eq!(PixelFormat::Rgba8.plane_layout(0, 3, 2), (3, 2, 4));
    }

    #[test]
    fn software_layouts_are_planar_with_rounded_up_chroma() {
        assert_eq!(PixelFormat::Yuv420p10.plane_layout(0, 7, 5), (7, 5, 2));
        assert_eq!(PixelFormat::Yuv420p10.plane_layout(2, 7, 5), (4, 3, 2));
        assert_eq!(PixelFormat::Yuv422p.plane_layout(1, 7, 5), (4, 5, 1));
        assert_eq!(
            PixelFormat::Yuv422p10.plane_layout(1, 1920, 1080),
            (960, 1080, 2)
        );
        assert_eq!(PixelFormat::Yuv422p10.bit_depth(), 10);
        assert_eq!(PixelFormat::Yuv422p.bit_depth(), 8);
        for f in PixelFormat::ALL {
            assert_eq!(f.is_yuv(), f != PixelFormat::Rgba8, "{f:?}");
            assert_eq!(
                f.plane_count(),
                if f == PixelFormat::Rgba8 {
                    1
                } else if matches!(f, PixelFormat::Nv12 | PixelFormat::P010) {
                    2
                } else {
                    3
                }
            );
        }
    }

    #[test]
    fn tight_frames_validate_and_short_planes_do_not() {
        for format in PixelFormat::ALL {
            let f = frame(format, 7, 5);
            assert_eq!(f.validate(), Ok(()), "{format:?}");
            let mut short = f.clone();
            short.planes[0].data.pop();
            assert!(short.validate().is_err(), "{format:?}");
            let mut missing = f;
            missing.planes.pop();
            assert!(missing.validate().is_err(), "{format:?}");
        }
        assert!(frame(PixelFormat::Nv12, 0, 2).validate().is_err());
    }

    #[test]
    fn rows_skip_stride_padding() {
        let mut f = frame(PixelFormat::Yuv420p, 3, 2);
        f.planes[0] = Plane {
            data: vec![1, 2, 3, 0, 4, 5, 6, 0],
            stride: 4,
        };
        assert_eq!(f.row(0, 1), Some(&[4, 5, 6][..]));
        assert_eq!(f.row(0, 2), None);
        assert_eq!(f.row(3, 0), None);
    }

    #[test]
    fn chunks_validate_their_planes() {
        let ok = AudioChunk {
            pts: Time::ZERO,
            rate: SampleRate::HZ_48_000,
            channels: 2,
            planes: vec![vec![0.0; 4], vec![0.0; 4]],
        };
        assert_eq!(ok.validate(), Ok(()));
        assert_eq!(ok.frames(), 4);
        let mut ragged = ok.clone();
        ragged.planes[1].pop();
        assert!(ragged.validate().is_err());
        let mut wrong = ok;
        wrong.channels = 1;
        assert!(wrong.validate().is_err());
    }
}
