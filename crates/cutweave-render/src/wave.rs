// SPDX-License-Identifier: GPL-3.0-or-later
//! Waveform pictures (WP-2I.02; arch §14 `audio_waveform`): peaks drawn as one lane per signal
//! with a time ruler underneath, so an agent can see where sound is loud, quiet or missing.
//!
//! * **Lanes.** Each lane is one signal (the mix, or one channel), `width` columns wide. A
//!   column is the lowest and the highest sample of its slice of time, `-1..=1`, drawn as a
//!   vertical line from one to the other (at least one pixel). Lanes share the height left
//!   above the ruler, one pixel apart.
//! * **Rows** ([`row_of`]). Linear by default: `+1` is the lane's top row, `0` its middle,
//!   `-1` its bottom row. With a dB scale of `floor` dB the distance from the middle is the
//!   level between `-floor` dBFS (the middle) and 0 dBFS (the edge), so quiet sound is visible.
//! * **Ruler** ([`ticks`]). Ticks at a round step (1, 2 or 5 times a power of ten seconds, or
//!   whole minutes) chosen so labels never touch, labelled `M:SS` (with tenths or hundredths
//!   under a second) in the built-in bitmap font ([`crate::font`]).
//!
//! Pure and deterministic: the same columns give the same pixels on every machine.

use cutweave_model::{FLICKS_PER_SECOND, Time};

use crate::font;
use crate::image::Image;
use crate::inspect::PixelRect;

/// Picture background (straight-alpha sRGB).
pub const BACKGROUND: [u8; 4] = [20, 20, 20, 255];
/// The waveform.
pub const WAVE: [u8; 4] = [110, 190, 255, 255];
/// The zero line of a lane.
pub const ZERO_LINE: [u8; 4] = [70, 70, 70, 255];
/// Ruler ticks and labels.
pub const RULER: [u8; 4] = [235, 235, 235, 255];
/// Height of the ruler strip in pixels (a tick row, the label, padding).
pub const RULER_HEIGHT: u32 = font::HEIGHT + 7;
/// Smallest lane height in pixels.
pub const MIN_LANE_HEIGHT: u32 = 16;
/// Smallest distance between two ruler ticks, in label widths.
const TICK_SPACING: u32 = 2;

/// The extremes of one pixel column of a lane: lowest and highest sample, `-1..=1`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Column {
    /// The lowest sample.
    pub min: f32,
    /// The highest sample.
    pub max: f32,
}

impl Column {
    /// A column that saw no sample yet (any sample replaces both extremes).
    pub const EMPTY: Column = Column {
        min: f32::INFINITY,
        max: f32::NEG_INFINITY,
    };

    /// Widens the column to include `sample`.
    pub fn add(&mut self, sample: f32) {
        self.min = self.min.min(sample);
        self.max = self.max.max(sample);
    }

    /// The column, or silence if it saw no sample.
    #[must_use]
    pub fn or_silent(self) -> Column {
        if self.min <= self.max {
            self
        } else {
            Column::default()
        }
    }
}

/// Why a waveform picture cannot be laid out.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WaveError {
    /// No lane, or lanes of different widths, or a zero width.
    #[error("a waveform needs at least one lane and every lane as wide as the picture")]
    Lanes,
    /// The picture is too low for its lanes and the ruler.
    #[error("height {height} is too small for {lanes} lane(s); at least {needed}")]
    TooLow {
        /// The requested height.
        height: u32,
        /// Lanes to draw.
        lanes: u32,
        /// The smallest height that works.
        needed: u32,
    },
}

/// Where the parts of a waveform picture are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveLayout {
    /// Picture width.
    pub width: u32,
    /// Picture height.
    pub height: u32,
    /// The lanes, top to bottom.
    pub lanes: Vec<PixelRect>,
    /// The ruler strip under the lanes.
    pub ruler: PixelRect,
}

impl WaveLayout {
    /// The smallest picture height for `lanes` lanes.
    #[must_use]
    pub const fn min_height(lanes: u32) -> u32 {
        lanes * MIN_LANE_HEIGHT + lanes.saturating_sub(1) + RULER_HEIGHT
    }

    /// Lays out `lanes` lanes and the ruler in a `width × height` picture.
    pub fn new(width: u32, height: u32, lanes: u32) -> Result<WaveLayout, WaveError> {
        if lanes == 0 || width == 0 {
            return Err(WaveError::Lanes);
        }
        let needed = WaveLayout::min_height(lanes);
        if height < needed {
            return Err(WaveError::TooLow {
                height,
                lanes,
                needed,
            });
        }
        let room = height - RULER_HEIGHT - (lanes - 1);
        let (each, extra) = (room / lanes, room % lanes);
        let mut y = 0;
        let rects = (0..lanes)
            .map(|i| {
                // The first lanes take the pixels a division leaves over.
                let h = each + u32::from(i < extra);
                let rect = PixelRect {
                    x: 0,
                    y,
                    width,
                    height: h,
                };
                y += h + 1;
                rect
            })
            .collect();
        Ok(WaveLayout {
            width,
            height,
            lanes: rects,
            ruler: PixelRect {
                x: 0,
                y: height - RULER_HEIGHT,
                width,
                height: RULER_HEIGHT,
            },
        })
    }
}

/// The row (0 = top) of `value` (`-1..=1`) in a lane `height` pixels high: linear, or on a dB
/// scale that puts `-floor` dBFS at the middle and 0 dBFS at the edges (see the module docs).
#[must_use]
pub fn row_of(value: f32, height: u32, scale_db: Option<f64>) -> u32 {
    let v = f64::from(value).clamp(-1.0, 1.0);
    let v = match scale_db {
        Some(floor) if floor > 0.0 => {
            let magnitude = v.abs();
            let level = if magnitude > 0.0 {
                (20.0 * magnitude.log10()).max(-floor)
            } else {
                -floor
            };
            (1.0 + level / floor).copysign(v)
        }
        _ => v,
    };
    let last = f64::from(height.saturating_sub(1));
    // `v` is within -1..=1, so the product is within 0..=last.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let row = ((1.0 - v) / 2.0 * last).round() as u32;
    row.min(height.saturating_sub(1))
}

/// A ruler tick: its column and its label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tick {
    /// The pixel column.
    pub x: u32,
    /// The time it marks.
    pub t: Time,
    /// The label (`M:SS`, with a fraction when ticks are under a second apart).
    pub label: String,
}

/// Tick steps in milliseconds: 1, 2 and 5 times powers of ten, then minutes.
const STEPS_MS: [i64; 22] = [
    10, 20, 50, 100, 200, 500, 1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000,
    300_000, 600_000, 900_000, 1_800_000, 3_600_000, 7_200_000, 18_000_000, 36_000_000,
];

fn label_of(ms: i64, step_ms: i64) -> String {
    let (minutes, rest) = (ms / 60_000, ms % 60_000);
    let (seconds, fraction) = (rest / 1_000, rest % 1_000);
    if step_ms >= 1_000 {
        format!("{minutes}:{seconds:02}")
    } else if step_ms >= 100 {
        format!("{minutes}:{seconds:02}.{}", fraction / 100)
    } else {
        format!("{minutes}:{seconds:02}.{:02}", fraction / 10)
    }
}

/// The ruler ticks of the range `start..end` drawn over `width` columns: at multiples of the
/// smallest round step whose labels stay [`TICK_SPACING`] label widths apart. Times before zero
/// get no tick.
#[must_use]
pub fn ticks(start: Time, end: Time, width: u32) -> Vec<Tick> {
    let span = i128::from(end.flicks()) - i128::from(start.flicks());
    if span <= 0 || width == 0 {
        return Vec::new();
    }
    let per_ms = i128::from(FLICKS_PER_SECOND / 1_000);
    let x_of = |ms: i64| -> Option<u32> {
        let offset = i128::from(ms) * per_ms - i128::from(start.flicks());
        if offset < 0 || offset >= span {
            return None;
        }
        u32::try_from(offset * i128::from(width) / span).ok()
    };
    let last_ms = i64::try_from(i128::from(end.flicks()) / per_ms).unwrap_or(i64::MAX);
    let first_ms = i64::try_from(i128::from(start.flicks().max(0)) / per_ms).unwrap_or(0);
    for step in STEPS_MS {
        let widest = font::text_width(&label_of(last_ms, step), 1);
        let apart = i128::from(step) * per_ms * i128::from(width) / span;
        if apart < i128::from(widest * TICK_SPACING) {
            continue;
        }
        let mut out = Vec::new();
        let mut ms = first_ms.div_euclid(step) * step;
        while ms <= last_ms {
            if let (Some(x), Ok(t)) = (
                x_of(ms),
                Time::from_flicks(ms.saturating_mul(FLICKS_PER_SECOND / 1_000)),
            ) {
                out.push(Tick {
                    x,
                    t,
                    label: label_of(ms, step),
                });
            }
            ms += step;
        }
        return out;
    }
    Vec::new()
}

fn vertical(image: &mut Image, x: u32, rows: std::ops::RangeInclusive<u32>, rgba: [u8; 4]) {
    for y in rows {
        if x < image.width && y < image.height {
            image.set_pixel(x, y, rgba);
        }
    }
}

/// Draws `lanes` (each `width` columns) over the time range `start..end` into a
/// `width × height` picture: the lanes top to bottom, then the ruler.
pub fn draw(
    lanes: &[Vec<Column>],
    (start, end): (Time, Time),
    (width, height): (u32, u32),
    scale_db: Option<f64>,
) -> Result<(Image, WaveLayout), WaveError> {
    if lanes.iter().any(|lane| lane.len() != width as usize) {
        return Err(WaveError::Lanes);
    }
    let count = u32::try_from(lanes.len()).map_err(|_| WaveError::Lanes)?;
    let layout = WaveLayout::new(width, height, count)?;
    let mut image = Image::filled(width, height, BACKGROUND);
    for (lane, rect) in lanes.iter().zip(&layout.lanes) {
        let zero = rect.y + row_of(0.0, rect.height, scale_db);
        for x in 0..width {
            image.set_pixel(x, zero, ZERO_LINE);
        }
        for (x, column) in (0u32..).zip(lane) {
            let column = column.or_silent();
            if column.min == 0.0 && column.max == 0.0 {
                continue;
            }
            let top = row_of(column.max, rect.height, scale_db);
            let bottom = row_of(column.min, rect.height, scale_db);
            vertical(
                &mut image,
                x,
                rect.y + top.min(bottom)..=rect.y + top.max(bottom),
                WAVE,
            );
        }
    }
    for tick in ticks(start, end, width) {
        vertical(
            &mut image,
            tick.x,
            layout.ruler.y..=layout.ruler.y + 2,
            RULER,
        );
        let label_x = tick.x + 2;
        if label_x + font::text_width(&tick.label, 1) <= width {
            font::draw_text(
                &mut image,
                label_x,
                layout.ruler.y + 4,
                &tick.label,
                1,
                RULER,
            );
        }
    }
    Ok((image, layout))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn secs(s: f64) -> Time {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        Time::from_flicks((s * FLICKS_PER_SECOND as f64).round() as i64).unwrap()
    }

    #[test]
    fn rows_are_linear_or_decibels() {
        // 101 rows: +1 at the top, 0 in the middle, -1 at the bottom.
        assert_eq!(row_of(1.0, 101, None), 0);
        assert_eq!(row_of(0.0, 101, None), 50);
        assert_eq!(row_of(-1.0, 101, None), 100);
        assert_eq!(row_of(0.5, 101, None), 25);
        assert_eq!(row_of(-0.5, 101, None), 75);
        assert_eq!(row_of(7.0, 101, None), 0, "clamped");
        // dB: -60 dBFS is the middle, 0 dBFS the edge, -30 dBFS half way.
        let db = Some(60.0);
        assert_eq!(row_of(1.0, 101, db), 0);
        assert_eq!(row_of(0.0, 101, db), 50);
        assert_eq!(row_of(0.001, 101, db), 50);
        assert_eq!(row_of(0.031_622_78, 101, db), 25);
        assert_eq!(row_of(-0.031_622_78, 101, db), 75);
        // -6 dBFS is 90% of the way out on a 60 dB scale.
        assert_eq!(row_of(0.5, 101, db), 5);
        assert_eq!(row_of(0.5, 1, None), 0);
    }

    #[test]
    fn layouts_share_the_height_and_refuse_too_little() {
        let one = WaveLayout::new(300, 100, 1).unwrap();
        assert_eq!(one.lanes.len(), 1);
        assert_eq!(one.lanes[0].height, 100 - RULER_HEIGHT);
        assert_eq!(one.ruler.y, 100 - RULER_HEIGHT);
        let two = WaveLayout::new(300, 100, 2).unwrap();
        let (a, b) = (two.lanes[0], two.lanes[1]);
        assert_eq!(a.height + b.height + 1, 100 - RULER_HEIGHT);
        assert_eq!(b.y, a.y + a.height + 1);
        assert_eq!(b.y + b.height, two.ruler.y);
        assert_eq!(
            WaveLayout::new(300, 20, 1),
            Err(WaveError::TooLow {
                height: 20,
                lanes: 1,
                needed: WaveLayout::min_height(1)
            })
        );
        assert_eq!(WaveLayout::new(0, 100, 1), Err(WaveError::Lanes));
        assert_eq!(WaveLayout::new(300, 100, 0), Err(WaveError::Lanes));
    }

    #[test]
    fn ticks_use_round_steps_that_fit() {
        // 10 s over 600 px: 1 s is 60 px, a label ("0:10") is 23 px wide.
        let t = ticks(secs(0.0), secs(10.0), 600);
        let labels: Vec<&str> = t.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "0:00", "0:01", "0:02", "0:03", "0:04", "0:05", "0:06", "0:07", "0:08", "0:09"
            ]
        );
        assert_eq!(t[3].x, 180);
        assert_eq!(t[3].t, secs(3.0));
        // A range that does not start on a tick; minutes.
        let t = ticks(secs(95.0), secs(215.0), 400);
        let labels: Vec<&str> = t.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "1:45", "2:00", "2:15", "2:30", "2:45", "3:00", "3:15", "3:30"
            ]
        );
        // Under a second: tenths.
        let t = ticks(secs(1.0), secs(1.5), 500);
        assert_eq!(t[0].label, "0:01.0");
        assert_eq!(t[1].label, "0:01.1");
        assert_eq!(ticks(secs(2.0), secs(2.0), 100), []);
        // Labels never touch.
        for width in [64, 100, 333, 2048] {
            let t = ticks(secs(0.0), secs(3600.0), width);
            for pair in t.windows(2) {
                assert!(pair[1].x >= pair[0].x + font::text_width(&pair[0].label, 1) + 2);
            }
        }
    }

    #[test]
    fn a_half_scale_column_spans_the_quarter_rows() {
        let width = 200;
        let lane = vec![
            Column {
                min: -0.5,
                max: 0.5
            };
            width as usize
        ];
        let (image, layout) = draw(
            std::slice::from_ref(&lane),
            (secs(0.0), secs(4.0)),
            (width, 120),
            None,
        )
        .unwrap();
        let rect = layout.lanes[0];
        for x in [0, 57, width - 1] {
            let lit: Vec<u32> = (rect.y..rect.y + rect.height)
                .filter(|&y| image.pixel(x, y) == WAVE)
                .collect();
            assert_eq!(lit[0], rect.y + row_of(0.5, rect.height, None));
            assert_eq!(
                *lit.last().unwrap(),
                rect.y + row_of(-0.5, rect.height, None)
            );
        }
        // Silence leaves only the zero line; the ruler carries labels.
        let (quiet, layout) = draw(
            &[vec![Column::default(); width as usize], lane],
            (secs(0.0), secs(4.0)),
            (width, 120),
            Some(60.0),
        )
        .unwrap();
        let top = layout.lanes[0];
        assert!((top.y..top.y + top.height).all(|y| quiet.pixel(5, y) != WAVE));
        assert_eq!(
            quiet.pixel(5, top.y + row_of(0.0, top.height, Some(60.0))),
            ZERO_LINE
        );
        let ruler = layout.ruler;
        let labelled = (ruler.y..ruler.y + ruler.height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|&(x, y)| quiet.pixel(x, y) == RULER)
            .count();
        assert!(labelled > 40, "{labelled} ruler pixels");
        assert_eq!(
            draw(
                &[vec![Column::default(); 3]],
                (secs(0.0), secs(1.0)),
                (4, 100),
                None
            ),
            Err(WaveError::Lanes)
        );
        // A column that saw no sample is silent.
        assert_eq!(Column::EMPTY.or_silent(), Column::default());
        let mut c = Column::EMPTY;
        c.add(0.25);
        c.add(-0.5);
        assert_eq!(
            c.or_silent(),
            Column {
                min: -0.5,
                max: 0.25
            }
        );
    }
}
