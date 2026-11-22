// SPDX-License-Identifier: GPL-3.0-or-later
//! Contact sheets: frames of a time range as one labelled grid image (arch §14
//! `render_contact_sheet`: one image is far cheaper for an agent than N images).
//!
//! * **Sampling** ([`sample_times`]): `count` frames evenly spaced from `start` (inclusive) to
//!   `end` (exclusive), `t_i = start + i·(end − start)/count`, or one frame every `interval`;
//!   each time is moved to the start of the frame containing it, and repeats (ranges shorter
//!   than `count` frames) are dropped. At most [`MAX_CELLS`] cells.
//! * **Layout** ([`SheetLayout`]): `cols` columns (default `ceil(√n)`), cells with the sequence
//!   aspect, [`GAP`] px gutters, the whole sheet within `max_size` on its longer side, cells
//!   never larger than the sequence frame.
//! * **Labels**: the cell's timecode is burned in on a strip under each cell with the built-in
//!   bitmap font ([`crate::font`]: digits and punctuation, integer scales 1–3 chosen from the
//!   cell width). That keeps sheets self-explanatory for a vision model without a text engine;
//!   the same timecodes, and the items visible in each cell, are also returned as metadata.
//!   Caption rendering on frames arrives with the P3 text engine.

use cutweave_model::{Duration, FrameRate, Rounding, Time};

use crate::font;
use crate::image::Image;
use crate::inspect::PixelRect;

/// Most cells a sheet may have.
pub const MAX_CELLS: u32 = 64;
/// Gutter between cells and around the sheet, in pixels.
pub const GAP: u32 = 4;
/// Smallest cell width in pixels.
pub const MIN_CELL: u32 = 16;
/// Sheet background (straight-alpha sRGB).
pub const BACKGROUND: [u8; 4] = [20, 20, 20, 255];
/// Label colour.
pub const LABEL: [u8; 4] = [235, 235, 235, 255];
/// The widest label the layout plans for (a timecode).
const LABEL_TEMPLATE: &str = "00:00:00:00";

/// How sample times are chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sampling {
    /// This many frames, evenly spaced.
    Count(u32),
    /// One frame every this long.
    Interval(Duration),
}

/// Why a sheet cannot be made.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SheetError {
    /// The range is empty.
    #[error("the range is empty: end must be after start")]
    EmptyRange,
    /// Zero frames or a zero interval.
    #[error("count and interval must be positive")]
    NothingToSample,
    /// More cells than [`MAX_CELLS`].
    #[error("{0} frames exceed the limit of {MAX_CELLS}; use a larger interval or a smaller range")]
    TooMany(u64),
    /// The cells would be smaller than [`MIN_CELL`] pixels.
    #[error("max_size {max_size} is too small for {cols}x{rows} cells")]
    TooSmall {
        /// The requested bound.
        max_size: u32,
        /// Columns.
        cols: u32,
        /// Rows.
        rows: u32,
    },
}

/// The frame start times of a sheet (see the module docs).
pub fn sample_times(
    start: Time,
    end: Time,
    sampling: Sampling,
    rate: FrameRate,
) -> Result<Vec<Time>, SheetError> {
    if end <= start {
        return Err(SheetError::EmptyRange);
    }
    let span = i128::from(end.flicks()) - i128::from(start.flicks());
    let raw: Vec<i128> = match sampling {
        Sampling::Count(0) => return Err(SheetError::NothingToSample),
        Sampling::Count(n) if n > MAX_CELLS => return Err(SheetError::TooMany(u64::from(n))),
        Sampling::Count(n) => (0..i128::from(n))
            .map(|i| i128::from(start.flicks()) + span * i / i128::from(n))
            .collect(),
        Sampling::Interval(d) if d.is_zero() => return Err(SheetError::NothingToSample),
        Sampling::Interval(d) => {
            let n = (span + i128::from(d.flicks()) - 1) / i128::from(d.flicks());
            if n > i128::from(MAX_CELLS) {
                return Err(SheetError::TooMany(u64::try_from(n).unwrap_or(u64::MAX)));
            }
            (0..n)
                .map(|i| i128::from(start.flicks()) + i128::from(d.flicks()) * i)
                .collect()
        }
    };
    let mut out: Vec<Time> = Vec::with_capacity(raw.len());
    for flicks in raw {
        // Inside [start, end), so always a valid time.
        let t = i64::try_from(flicks)
            .ok()
            .and_then(|f| Time::from_flicks(f).ok())
            .unwrap_or(start);
        let t = rate.snap_to_frame(t, Rounding::Floor).unwrap_or(t);
        if out.last() != Some(&t) {
            out.push(t);
        }
    }
    Ok(out)
}

/// Where everything goes on a sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SheetLayout {
    /// Columns.
    pub cols: u32,
    /// Rows.
    pub rows: u32,
    /// Cells.
    pub count: u32,
    /// Picture width of a cell.
    pub cell_width: u32,
    /// Picture height of a cell.
    pub cell_height: u32,
    /// Height of the label strip under each cell (0 without labels).
    pub label_height: u32,
    /// Font scale of labels (0 without labels).
    pub label_scale: u32,
    /// Sheet width.
    pub width: u32,
    /// Sheet height.
    pub height: u32,
}

fn label_height(scale: u32) -> u32 {
    if scale == 0 {
        0
    } else {
        (font::HEIGHT + 4) * scale
    }
}

impl SheetLayout {
    /// Lays out `count` cells with the aspect of `canvas` in `cols` columns (clamped to
    /// `1..=count`; `None` = `ceil(√count)`), the sheet's longer side at most `max_size`.
    pub fn new(
        count: u32,
        cols: Option<u32>,
        canvas: (u32, u32),
        max_size: u32,
        labels: bool,
    ) -> Result<SheetLayout, SheetError> {
        let count = count.clamp(1, MAX_CELLS);
        let cols = cols
            .unwrap_or_else(|| count.isqrt() + u32::from(count.isqrt().pow(2) < count))
            .clamp(1, count);
        let rows = count.div_ceil(cols);
        let (cw, ch) = (u64::from(canvas.0.max(1)), u64::from(canvas.1.max(1)));
        let max = u64::from(max_size);
        let gaps = |n: u32| u64::from(GAP) * (u64::from(n) + 1);
        // The largest cell width that fits both ways with a `label`-pixel strip.
        let fit = |label: u32| -> u64 {
            let by_width = max.saturating_sub(gaps(cols)) / u64::from(cols);
            let per_row = max.saturating_sub(gaps(rows)) / u64::from(rows);
            let by_height = per_row.saturating_sub(u64::from(label)) * cw / ch;
            by_width.min(by_height).min(cw)
        };
        let mut scale = 0;
        if labels {
            // The largest font scale whose timecode spans at most ¾ of the cell, else 1 if it
            // fits at all.
            let estimate = fit(label_height(1));
            let width = |s: u32| u64::from(font::text_width(LABEL_TEMPLATE, s));
            scale = (1..=3)
                .rev()
                .find(|&s| width(s) * 4 <= estimate * 3)
                .unwrap_or(u32::from(width(1) <= estimate));
        }
        let label = label_height(scale);
        let mut cell_w = fit(label);
        loop {
            if cell_w < u64::from(MIN_CELL) {
                return Err(SheetError::TooSmall {
                    max_size,
                    cols,
                    rows,
                });
            }
            let cell_h = ((cell_w * ch + cw / 2) / cw).max(1);
            let width = u64::from(cols) * cell_w + gaps(cols);
            let height = u64::from(rows) * (cell_h + u64::from(label)) + gaps(rows);
            if width <= max && height <= max {
                let as_u32 = |v: u64| u32::try_from(v).unwrap_or(u32::MAX);
                return Ok(SheetLayout {
                    cols,
                    rows,
                    count,
                    cell_width: as_u32(cell_w),
                    cell_height: as_u32(cell_h),
                    label_height: label,
                    label_scale: scale,
                    width: as_u32(width),
                    height: as_u32(height),
                });
            }
            cell_w -= 1;
        }
    }

    /// The picture rectangle of cell `index` (row-major).
    #[must_use]
    pub fn cell(&self, index: u32) -> PixelRect {
        let (col, row) = (index % self.cols, index / self.cols);
        PixelRect {
            x: GAP + col * (self.cell_width + GAP),
            y: GAP + row * (self.cell_height + self.label_height + GAP),
            width: self.cell_width,
            height: self.cell_height,
        }
    }

    /// An empty sheet.
    #[must_use]
    pub fn canvas(&self) -> Image {
        Image::filled(self.width, self.height, BACKGROUND)
    }

    /// Draws `tile` (straight alpha, composited over the sheet) into cell `index` and its
    /// `label` (centred under it; shortened to drop a `00:` hour field, or left out, when it
    /// does not fit).
    pub fn place(&self, sheet: &mut Image, index: u32, tile: &Image, label: &str) {
        let r = self.cell(index);
        blit_over(sheet, tile, r.x, r.y);
        if self.label_scale == 0 {
            return;
        }
        let short = label.strip_prefix("00:").unwrap_or(label);
        for text in [label, short] {
            for scale in (1..=self.label_scale).rev() {
                let w = font::text_width(text, scale);
                if w <= r.width && font::HEIGHT * scale <= self.label_height {
                    let x = r.x + (r.width - w) / 2;
                    let y = r.y + r.height + (self.label_height - font::HEIGHT * scale) / 2;
                    font::draw_text(sheet, x, y, text, scale, LABEL);
                    return;
                }
            }
        }
    }
}

/// Composites a straight-alpha `tile` over `dst` with its top-left corner at `(left, top)`
/// (clipped), in 8-bit sRGB.
#[allow(clippy::cast_possible_truncation)]
pub fn blit_over(dst: &mut Image, tile: &Image, left: u32, top: u32) {
    for ty in 0..tile.height {
        let dy = top + ty;
        if dy >= dst.height {
            break;
        }
        for tx in 0..tile.width {
            let dx = left + tx;
            if dx >= dst.width {
                break;
            }
            let src = tile.pixel(tx, ty);
            let out = if src[3] == 255 {
                src
            } else {
                let under = dst.pixel(dx, dy);
                let alpha = u32::from(src[3]);
                let mix = |over: u8, back: u8| {
                    ((u32::from(over) * alpha + u32::from(back) * (255 - alpha) + 127) / 255) as u8
                };
                [
                    mix(src[0], under[0]),
                    mix(src[1], under[1]),
                    mix(src[2], under[2]),
                    (alpha + u32::from(under[3]) * (255 - alpha) / 255) as u8,
                ]
            };
            dst.set_pixel(dx, dy, out);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::FLICKS_PER_SECOND;

    use super::*;

    fn secs(s: f64) -> Time {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        Time::from_flicks((s * FLICKS_PER_SECOND as f64).round() as i64).unwrap()
    }

    #[test]
    fn count_sampling_is_even_start_inclusive_end_exclusive() {
        let t = sample_times(secs(0.0), secs(9.0), Sampling::Count(3), FrameRate::FPS_24).unwrap();
        assert_eq!(t, [secs(0.0), secs(3.0), secs(6.0)]);
        // Times snap to the start of the frame containing them.
        let t = sample_times(secs(0.01), secs(1.0), Sampling::Count(2), FrameRate::FPS_25).unwrap();
        assert_eq!(t, [secs(0.0), secs(0.48)]);
        // A range shorter than `count` frames repeats frames: repeats are dropped.
        let t = sample_times(secs(0.0), secs(0.1), Sampling::Count(10), FrameRate::FPS_30).unwrap();
        assert_eq!(t, [secs(0.0), secs(1.0 / 30.0), secs(2.0 / 30.0)]);
    }

    #[test]
    fn interval_sampling_and_limits() {
        let two = Duration::from_secs(2).unwrap();
        let t = sample_times(
            secs(1.0),
            secs(7.0),
            Sampling::Interval(two),
            FrameRate::FPS_24,
        )
        .unwrap();
        assert_eq!(t, [secs(1.0), secs(3.0), secs(5.0)]);
        let t = sample_times(
            secs(1.0),
            secs(7.5),
            Sampling::Interval(two),
            FrameRate::FPS_24,
        )
        .unwrap();
        assert_eq!(t.len(), 4);
        let ms = Duration::from_flicks(FLICKS_PER_SECOND / 1000).unwrap();
        assert_eq!(
            sample_times(
                secs(0.0),
                secs(1.0),
                Sampling::Interval(ms),
                FrameRate::FPS_24
            ),
            Err(SheetError::TooMany(1000))
        );
        assert_eq!(
            sample_times(secs(0.0), secs(1.0), Sampling::Count(65), FrameRate::FPS_24),
            Err(SheetError::TooMany(65))
        );
        assert_eq!(
            sample_times(secs(2.0), secs(2.0), Sampling::Count(4), FrameRate::FPS_24),
            Err(SheetError::EmptyRange)
        );
        assert_eq!(
            sample_times(secs(0.0), secs(2.0), Sampling::Count(0), FrameRate::FPS_24),
            Err(SheetError::NothingToSample)
        );
        assert_eq!(
            sample_times(
                secs(0.0),
                secs(2.0),
                Sampling::Interval(Duration::ZERO),
                FrameRate::FPS_24
            ),
            Err(SheetError::NothingToSample)
        );
    }

    #[test]
    fn layout_fits_max_size_and_keeps_the_aspect() {
        let l = SheetLayout::new(9, None, (1920, 1080), 768, true).unwrap();
        assert_eq!((l.cols, l.rows), (3, 3));
        assert!(l.width <= 768 && l.height <= 768);
        // Width-bound for 16:9 cells in a square grid.
        assert_eq!(l.cell_width, (768 - 4 * GAP) / 3);
        assert_eq!(l.width, 3 * l.cell_width + 4 * GAP);
        assert_eq!(l.cell_height, (l.cell_width * 1080 + 960) / 1920);
        assert_eq!(l.label_scale, 2);
        // Portrait cells in one column are height-bound.
        let p = SheetLayout::new(4, Some(1), (1080, 1920), 768, false).unwrap();
        assert_eq!((p.cols, p.rows, p.label_height), (1, 4, 0));
        assert!(p.height <= 768 && p.height > 760 && p.width < 768);
        // Cells are never larger than the sequence frame.
        let small = SheetLayout::new(1, None, (320, 180), 2048, true).unwrap();
        assert_eq!((small.cell_width, small.cell_height), (320, 180));
        // Columns default to ceil(sqrt(n)) and clamp to the count.
        assert_eq!(
            SheetLayout::new(10, None, (16, 9), 768, true).unwrap().cols,
            4
        );
        assert_eq!(
            SheetLayout::new(2, Some(9), (16, 9), 768, true)
                .unwrap()
                .cols,
            2
        );
        assert!(matches!(
            SheetLayout::new(64, Some(64), (1920, 1080), 200, true),
            Err(SheetError::TooSmall { .. })
        ));
        // Cell rectangles do not overlap and stay inside the sheet.
        for i in 0..9 {
            let r = l.cell(i);
            assert!(r.x + r.width <= l.width && r.y + r.height + l.label_height <= l.height);
        }
        assert_eq!(l.cell(4).x, l.cell(1).x);
        assert_eq!(l.cell(4).y, l.cell(3).y);
    }

    #[test]
    fn placing_tiles_and_labels() {
        let l = SheetLayout::new(2, Some(2), (160, 90), 200, true).unwrap();
        assert_eq!(l.label_scale, 1);
        let mut sheet = l.canvas();
        let red = Image::filled(l.cell_width, l.cell_height, [255, 0, 0, 255]);
        let clear = Image::filled(l.cell_width, l.cell_height, [0, 0, 255, 0]);
        l.place(&mut sheet, 0, &red, "00:00:01:02");
        l.place(&mut sheet, 1, &clear, "00:00:03:04");
        let (a, b) = (l.cell(0), l.cell(1));
        assert_eq!(sheet.pixel(a.x, a.y), [255, 0, 0, 255]);
        assert_eq!(
            sheet.pixel(b.x, b.y),
            BACKGROUND,
            "transparent tile shows the sheet"
        );
        assert_eq!(sheet.pixel(0, 0), BACKGROUND);
        let label_px = (a.y + a.height..a.y + a.height + l.label_height)
            .flat_map(|y| (a.x..a.x + a.width).map(move |x| (x, y)))
            .filter(|&(x, y)| sheet.pixel(x, y) == LABEL)
            .count();
        assert!(label_px > 20, "label drawn ({label_px} px)");
        // Half-transparent over opaque.
        let mut dst = Image::filled(1, 1, [0, 0, 0, 255]);
        blit_over(&mut dst, &Image::filled(1, 1, [255, 255, 255, 128]), 0, 0);
        assert_eq!(dst.pixel(0, 0), [128, 128, 128, 255]);
    }
}
