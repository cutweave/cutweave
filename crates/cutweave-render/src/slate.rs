// SPDX-License-Identifier: GPL-3.0-or-later
//! The **"media offline" slate** (WP-2F.03c): the picture a clip shows while its media file is
//! not on this computer, in the preview, `render.frame` and exports alike.
//!
//! A [`FrameSource`](crate::FrameSource) that knows a clip's file is gone answers
//! [`FrameError::Offline`](crate::FrameError::Offline); the render graph then composites this
//! frame in the layer's place (and still lists the item in
//! [`RenderGraph::missing`](crate::graph::RenderGraph::missing)). Layout uses the probed size
//! of the missing media ([`FrameRequest::source_size`](crate::FrameRequest::source_size)), so
//! the slate fills exactly the rectangle the clip's picture would, with the clip's transform,
//! crop, flip, opacity and blend mode: like a proxy, the slate's own size only enters the
//! display → texel map.
//!
//! **The picture** ([`image`]): 320 × 180, opaque. Diagonal bands 20 px wide, alternating a dark
//! red ([`BAND`]) and near-black ([`FIELD`]), with a darker plate ([`PLATE`]) across the middle
//! carrying [`TEXT`] in the built-in bitmap [`font`](crate::font) (no shaping, no font files).
//! It is tagged full-range sRGB with BT.709 primaries whatever the missing media's tags were.
//!
//! **Cost.** The frame is built once per process ([`frame`]) and shared: showing it allocates
//! nothing, and its fixed [`FrameId`] makes the compositor upload it once. Nothing about the
//! slate is saved in a project.

use std::sync::{Arc, OnceLock};

use cutweave_model::{
    ColorPrimaries, ColorRange, ColorTags, MatrixCoefficients, TransferCharacteristics,
};

use crate::font;
use crate::frame::{ChromaSiting, FrameId, PixelFormat, Plane, VideoFrame};
use crate::hash::Fnv;
use crate::image::Image;

/// Width of the slate in pixels.
pub const WIDTH: u32 = 320;
/// Height of the slate in pixels.
pub const HEIGHT: u32 = 180;
/// Width of one diagonal band, measured along a row.
pub const BAND_WIDTH: u32 = 20;
/// The red bands (sRGB, opaque).
pub const BAND: [u8; 4] = [150, 28, 28, 255];
/// The dark bands between them.
pub const FIELD: [u8; 4] = [30, 30, 32, 255];
/// The plate behind the words.
pub const PLATE: [u8; 4] = [16, 16, 18, 255];
/// The words.
pub const INK: [u8; 4] = [236, 236, 236, 255];
/// What the slate says.
pub const TEXT: &str = "MEDIA OFFLINE";
/// Size of a font pixel of the words.
pub const TEXT_SCALE: u32 = 3;
/// Space between the words and the plate's edge, left and right.
const PLATE_PAD_X: u32 = 12;
/// Space between the words and the plate's edge, above and below.
const PLATE_PAD_Y: u32 = 10;

/// The colour of the bands at `(x, y)` (before the plate and the words are drawn over them).
#[must_use]
pub const fn band_at(x: u32, y: u32) -> [u8; 4] {
    if ((x + y) / BAND_WIDTH).is_multiple_of(2) {
        BAND
    } else {
        FIELD
    }
}

/// The plate's rectangle `[x0, y0, x1, y1)` and the top-left corner of the words.
#[must_use]
pub fn layout() -> ([u32; 4], [u32; 2]) {
    let text_w = font::text_width(TEXT, TEXT_SCALE);
    let text_h = font::HEIGHT * TEXT_SCALE;
    let (tx, ty) = ((WIDTH - text_w) / 2, (HEIGHT - text_h) / 2);
    (
        [
            tx - PLATE_PAD_X,
            ty - PLATE_PAD_Y,
            tx + text_w + PLATE_PAD_X,
            ty + text_h + PLATE_PAD_Y,
        ],
        [tx, ty],
    )
}

/// The slate's pixels (see the module docs). Allocates; [`frame`] keeps the one copy renders
/// use.
#[must_use]
pub fn image() -> Image {
    let mut image = Image::filled(WIDTH, HEIGHT, FIELD);
    let ([x0, y0, x1, y1], [tx, ty]) = layout();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let plate = (x0..x1).contains(&x) && (y0..y1).contains(&y);
            image.set_pixel(x, y, if plate { PLATE } else { band_at(x, y) });
        }
    }
    font::draw_text(&mut image, tx, ty, TEXT, TEXT_SCALE, INK);
    image
}

/// The slate's colour tags: full-range sRGB, every field set so that none falls back to the
/// tags of the media it stands in for.
#[must_use]
pub fn color_tags() -> ColorTags {
    ColorTags {
        primaries: Some(ColorPrimaries::Bt709),
        transfer: Some(TransferCharacteristics::Srgb),
        matrix: Some(MatrixCoefficients::Rgb),
        range: Some(ColorRange::Full),
    }
}

/// The identity of the slate's pixels (the same in every process and on every machine).
#[must_use]
pub fn frame_id() -> FrameId {
    let mut h = Fnv::new();
    h.str("cutweave.slate.media-offline.v1");
    FrameId(h.finish())
}

/// The slate as a source frame. Built on first use, then shared: no allocation and no lock
/// afterwards (one atomic load).
#[must_use]
pub fn frame() -> Arc<VideoFrame> {
    static SLATE: OnceLock<Arc<VideoFrame>> = OnceLock::new();
    Arc::clone(SLATE.get_or_init(|| {
        let image = image();
        Arc::new(VideoFrame {
            id: frame_id(),
            width: WIDTH,
            height: HEIGHT,
            format: PixelFormat::Rgba8,
            planes: vec![Plane {
                data: image.data,
                stride: WIDTH * 4,
            }],
            color: color_tags(),
            chroma_siting: ChromaSiting::default(),
        })
    }))
}

/// Whether `frame` is the slate.
#[must_use]
pub fn is_slate(frame: &VideoFrame) -> bool {
    frame.id == frame_id()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_slate_is_a_valid_opaque_srgb_frame_built_once() {
        let a = frame();
        let b = frame();
        assert!(Arc::ptr_eq(&a, &b), "one shared frame");
        a.validate().unwrap();
        assert_eq!(
            (a.width, a.height, a.format),
            (320, 180, PixelFormat::Rgba8)
        );
        assert!(is_slate(&a));
        assert_eq!(a.planes[0].data, image().data);
        assert!(
            a.planes[0]
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[3] == 255)
        );
        // No tag is left for the missing media's probe to fill in.
        let tags = &a.color;
        assert!(
            tags.primaries.is_some()
                && tags.transfer.is_some()
                && tags.matrix.is_some()
                && tags.range == Some(ColorRange::Full)
        );
    }

    /// Pins every pixel of the slate: the bands, the plate, and the words through a content
    /// hash. A change here changes what users see in previews and exports of offline media.
    #[test]
    fn the_pattern_is_pinned_pixel_for_pixel() {
        let img = image();
        assert!(img.is_valid());
        // Bands: 20 px along a row, running down to the left.
        assert_eq!(img.pixel(0, 0), BAND);
        assert_eq!(img.pixel(19, 0), BAND);
        assert_eq!(img.pixel(20, 0), FIELD);
        assert_eq!(img.pixel(39, 0), FIELD);
        assert_eq!(img.pixel(40, 0), BAND);
        assert_eq!(img.pixel(0, 20), FIELD);
        assert_eq!(img.pixel(19, 1), FIELD);
        assert_eq!(img.pixel(319, 179), band_at(319, 179));
        assert_eq!(band_at(319, 179), BAND);
        // The plate and the words: "MEDIA OFFLINE" is 13 glyphs, 231 × 21 px at scale 3.
        let (plate, text) = layout();
        assert_eq!(plate, [32, 69, 287, 110]);
        assert_eq!(text, [44, 79]);
        assert_eq!(img.pixel(32, 69), PLATE);
        assert_eq!(img.pixel(286, 109), PLATE);
        assert_eq!(img.pixel(31, 69), band_at(31, 69));
        assert_eq!(img.pixel(287, 110), band_at(287, 110));
        // The left stem of the "M" and the gap beside it.
        assert_eq!(img.pixel(44, 79), INK);
        assert_eq!(img.pixel(46, 99), INK);
        assert_eq!(img.pixel(47, 85), PLATE);
        // Only the four colours occur.
        let mut counts = [0usize; 4];
        for p in img.data.as_chunks::<4>().0 {
            let i = [BAND, FIELD, PLATE, INK]
                .iter()
                .position(|c| c == p)
                .expect("a slate colour");
            counts[i] += 1;
        }
        assert_eq!(counts.iter().sum::<usize>(), 320 * 180);
        assert_eq!(counts, [23_580, 23_565, 8_799, 1_656], "{counts:?}");
        let mut h = Fnv::new();
        h.bytes(&img.data);
        assert_eq!(
            h.finish(),
            0x94fc_188f_1409_b09f,
            "content hash {:#018x}",
            h.finish()
        );
    }
}
