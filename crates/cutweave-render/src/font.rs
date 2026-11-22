// SPDX-License-Identifier: GPL-3.0-or-later
//! A built-in 5×7 bitmap font for burned-in labels (contact-sheet timecodes).
//!
//! Deliberately tiny: digits and timecode punctuation only, drawn at integer scales with no
//! antialiasing, so labels are deterministic and need no font files, shaping or GPU text. Real
//! text (captions, titles) arrives with the P3 text engine (`cutweave-text`, MSDF, arch §8); this
//! is not a substitute for it. Unsupported characters draw as an empty box.

use crate::image::Image;

/// Glyph cell width in font pixels (5 columns + 1 spacing).
pub const ADVANCE: u32 = 6;
/// Glyph height in font pixels.
pub const HEIGHT: u32 = 7;

/// Rows top to bottom, 5 bits each (bit 4 = leftmost column).
fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        ';' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x04, 0x08],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '/' => [0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x00],
        ' ' => [0; 7],
        _ => [0x1F, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1F],
    }
}

/// Width in pixels of `text` drawn at `scale` (no trailing spacing).
#[must_use]
pub fn text_width(text: &str, scale: u32) -> u32 {
    let n = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    n.saturating_mul(ADVANCE)
        .saturating_sub(1)
        .saturating_mul(scale)
}

/// Draws `text` with its top-left corner at `(x, y)`, each font pixel a `scale × scale` square
/// of `rgba`. Pixels outside the image are clipped.
pub fn draw_text(image: &mut Image, x: u32, y: u32, text: &str, scale: u32, rgba: [u8; 4]) {
    let scale = scale.max(1);
    let mut pen = x;
    for c in text.chars() {
        let rows = glyph(c);
        for (gy, bits) in (0u32..).zip(rows) {
            for gx in 0..5u32 {
                if bits & (0x10 >> gx) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = pen + gx * scale + sx;
                        let py = y + gy * scale + sy;
                        if px < image.width && py < image.height {
                            image.set_pixel(px, py, rgba);
                        }
                    }
                }
            }
        }
        pen = pen.saturating_add(ADVANCE * scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(img: &Image) -> Vec<(u32, u32)> {
        let mut v = Vec::new();
        for y in 0..img.height {
            for x in 0..img.width {
                if img.pixel(x, y)[0] == 255 {
                    v.push((x, y));
                }
            }
        }
        v
    }

    #[test]
    fn digits_are_distinct_and_fit_the_cell() {
        let all: Vec<[u8; 7]> = "0123456789:;.-/ ".chars().map(glyph).collect();
        for (i, a) in all.iter().enumerate() {
            assert!(a.iter().all(|r| *r < 0x20), "5 columns");
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(glyph('?'), glyph('#'), "unknown characters share the box");
    }

    #[test]
    fn draws_scaled_and_clipped() {
        assert_eq!(text_width("00:00", 1), 29);
        assert_eq!(text_width("00:00", 2), 58);
        assert_eq!(text_width("", 3), 0);
        let mut img = Image::filled(8, 9, [0, 0, 0, 255]);
        draw_text(&mut img, 1, 1, "1", 1, [255; 4]);
        // '1': the stem is column 2 of the glyph, rows 0..7; the base row spans columns 1..=3.
        let px = lit(&img);
        assert!(px.contains(&(3, 1)) && px.contains(&(3, 7)));
        assert!(px.contains(&(2, 7)) && px.contains(&(4, 7)));
        assert_eq!(px.len(), 10);
        let mut big = Image::filled(12, 14, [0, 0, 0, 255]);
        draw_text(&mut big, 0, 0, "1", 2, [255; 4]);
        assert_eq!(lit(&big).len(), 40);
        // Off the edge: clipped, no panic.
        draw_text(&mut big, 10, 12, "88", 3, [255; 4]);
    }
}
