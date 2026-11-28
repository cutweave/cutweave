// SPDX-License-Identifier: GPL-3.0-or-later
//! Text (arch §4.2 `Text`, §8, §11; WP-3A.01): the content and look of a title, caption-sized
//! text or any other words on the picture. This module is data only. Fonts, shaping, layout and
//! drawing are `cutweave-text` and `cutweave-render` (WP-3A.02 to 3A.05).
//!
//! * [`RichText`]: runs of text, each with an optional [`RunStyle`] override.
//! * [`TextStyle`]: the look of the whole item (font, size, fill, strokes, shadows, glow,
//!   background box, spacing, alignment, auto-fit, direction). A [`TextStyleRef`] either names
//!   a built-in style ([`builtin_styles`]) or holds one inline.
//! * [`TextBox`]: the layout rectangle the text wraps and aligns in.
//!
//! **Units.** Sizes, widths, offsets, blur and padding are *reference pixels*: pixels of a
//! canvas 1080 lines high, scaled by `sequence height / 1080` at render time, so a style looks
//! the same in a 4K and a 720p sequence. `letter_spacing` is in em, `line_height` a multiple
//! of the font size. Box sizes are canvas units like `transform.position` (1.0 = the canvas
//! width horizontally, the canvas height vertically). Text positions come from the item's
//! `visual.transform`.
//!
//! **Animation.** `size`, `letter_spacing`, `line_height`, a solid fill's colour and the item's
//! visual properties are [`Animatable`]; the keyframe commands target the first three by
//! [`PropId`](crate::PropId). Run styles, strokes, shadows and the rest are static.
//!
//! Everything validates with a JSON Pointer path ([`TextError`]) relative to the value
//! checked, so commands and `Project::validate` report the same problem the same way.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::{Animatable, Color, Extra, PropId, Vec2};

/// Most characters of one text item (all runs together).
pub const MAX_TEXT_CHARS: usize = 20_000;
/// Most runs of one text item.
pub const MAX_RUNS: usize = 2_000;
/// Most strokes (and, separately, shadows) of one style.
pub const MAX_LAYERS: usize = 8;
/// Most stops of a gradient.
pub const MAX_GRADIENT_STOPS: usize = 16;
/// Most variable-font axes of a font.
pub const MAX_FONT_AXES: usize = 8;
/// Longest font family name, in characters.
pub const MAX_FAMILY_CHARS: usize = 200;
/// Largest value of a length in reference pixels (size, stroke width, blur, offsets, padding).
pub const MAX_LENGTH: f64 = 4000.0;

/// What is wrong with a text value and where, relative to the value checked.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {msg}")]
pub struct TextError {
    /// JSON Pointer from the checked value (`/size`, `/strokes/0/width`); empty for the value
    /// itself.
    pub path: String,
    /// What is wrong.
    pub msg: String,
}

impl TextError {
    /// An error at `path`.
    pub fn new(path: impl Into<String>, msg: impl Into<String>) -> TextError {
        TextError {
            path: path.into(),
            msg: msg.into(),
        }
    }

    /// The error with `prefix` put before its path.
    #[must_use]
    pub fn under(mut self, prefix: &str) -> TextError {
        self.path = format!("{prefix}{}", self.path);
        self
    }
}

fn length(path: &str, v: f64, min: f64, max: f64, what: &str) -> Result<(), TextError> {
    if v.is_finite() && v >= min && v <= max {
        Ok(())
    } else {
        Err(TextError::new(
            path,
            format!("{what} must be a number from {min} to {max}"),
        ))
    }
}

// ----- the font ----------------------------------------------------------------------------------

/// One variable-font axis setting (`wght`, `wdth`, `opsz`, ...).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct FontAxis {
    /// The four-letter axis tag.
    pub tag: String,
    /// Its value.
    #[specta(type = specta_typescript::Number)]
    pub value: f64,
}

/// A font as the document names it: a family, weight and slant, plus variable-font axes. The
/// font manager (WP-3A.02) resolves it to a file, falling back deterministically.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct FontSpec {
    /// Family name, e.g. `"Inter"`.
    pub family: String,
    /// Weight, 1 to 1000 (400 regular, 700 bold).
    pub weight: u16,
    /// Italic or oblique.
    pub italic: bool,
    /// Variable-font axes, sorted by tag.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub axes: Vec<FontAxis>,
}

impl Default for FontSpec {
    fn default() -> FontSpec {
        FontSpec {
            family: "Inter".to_owned(),
            weight: 700,
            italic: false,
            axes: Vec::new(),
        }
    }
}

impl FontSpec {
    /// Checks the family, weight and axes.
    pub fn check(&self) -> Result<(), TextError> {
        let n = self.family.chars().count();
        if n == 0 || n > MAX_FAMILY_CHARS || self.family.chars().any(char::is_control) {
            return Err(TextError::new(
                "/family",
                format!(
                    "the family must be 1 to {MAX_FAMILY_CHARS} characters, no control characters"
                ),
            ));
        }
        if !(1..=1000).contains(&self.weight) {
            return Err(TextError::new(
                "/weight",
                "the weight must be from 1 to 1000",
            ));
        }
        if self.axes.len() > MAX_FONT_AXES {
            return Err(TextError::new(
                "/axes",
                format!("at most {MAX_FONT_AXES} axes"),
            ));
        }
        for (i, axis) in self.axes.iter().enumerate() {
            if axis.tag.len() != 4 || !axis.tag.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(TextError::new(
                    format!("/axes/{i}/tag"),
                    "an axis tag is four visible ASCII characters",
                ));
            }
            if !axis.value.is_finite() {
                return Err(TextError::new(format!("/axes/{i}/value"), "must be finite"));
            }
            if i > 0 && self.axes[i - 1].tag >= axis.tag {
                return Err(TextError::new(
                    format!("/axes/{i}/tag"),
                    "axes are sorted by tag, each once",
                ));
            }
        }
        Ok(())
    }
}

// ----- paint -------------------------------------------------------------------------------------

/// One stop of a gradient.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct GradientStop {
    /// Position along the gradient, 0.0 to 1.0.
    #[specta(type = specta_typescript::Number)]
    pub at: f64,
    /// The colour there.
    pub color: Color,
}

/// How the glyphs are filled. Gradients run over the text box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fill {
    /// One colour (animatable).
    Solid {
        /// The colour, `#rrggbb` or `#rrggbbaa`.
        color: Animatable<Color>,
    },
    /// A straight gradient.
    Linear {
        /// Direction in degrees, clockwise from "left to right" (0) .
        #[specta(type = specta_typescript::Number)]
        angle: f64,
        /// Two to sixteen stops, ascending by `at`.
        stops: Vec<GradientStop>,
    },
    /// A circular gradient.
    Radial {
        /// Centre in box units (0.5, 0.5 is the middle).
        center: Vec2,
        /// Radius as a fraction of the box's larger side.
        #[specta(type = specta_typescript::Number)]
        radius: f64,
        /// Two to sixteen stops, ascending by `at`.
        stops: Vec<GradientStop>,
    },
}

impl Fill {
    /// Opaque or translucent solid `color`.
    #[must_use]
    pub fn solid(color: Color) -> Fill {
        Fill::Solid {
            color: Animatable::Static(color),
        }
    }

    fn check_stops(stops: &[GradientStop]) -> Result<(), TextError> {
        if !(2..=MAX_GRADIENT_STOPS).contains(&stops.len()) {
            return Err(TextError::new(
                "/stops",
                format!("a gradient has 2 to {MAX_GRADIENT_STOPS} stops"),
            ));
        }
        let mut last = 0.0;
        for (i, s) in stops.iter().enumerate() {
            length(&format!("/stops/{i}/at"), s.at, 0.0, 1.0, "a stop position")?;
            if s.at < last {
                return Err(TextError::new(
                    format!("/stops/{i}/at"),
                    "stops go in ascending order",
                ));
            }
            last = s.at;
        }
        Ok(())
    }

    /// Checks the gradient's numbers and a solid colour's keys.
    pub fn check(&self) -> Result<(), TextError> {
        match self {
            Fill::Solid { color } => color
                .check_range(crate::ValueRange::ANY)
                .map_err(|e| TextError::new("/color", e.to_string())),
            Fill::Linear { angle, stops } => {
                length("/angle", *angle, -36_000.0, 36_000.0, "the angle")?;
                Fill::check_stops(stops)
            }
            Fill::Radial {
                center,
                radius,
                stops,
            } => {
                if !center.is_finite() {
                    return Err(TextError::new("/center", "must be finite"));
                }
                length("/radius", *radius, 0.0, 10.0, "the radius")?;
                Fill::check_stops(stops)
            }
        }
    }
}

/// How an outline's corners are drawn.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum StrokeJoin {
    /// Sharp corners.
    #[default]
    Miter,
    /// Rounded corners.
    Round,
    /// Cut-off corners.
    Bevel,
}

/// Whether an outline is drawn behind the fill or over it.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum StrokeOrder {
    /// Under the fill: the outline grows outward only.
    #[default]
    Behind,
    /// Over the fill: it also eats into the letter.
    Over,
}

/// One outline. A style may stack several; they are drawn in list order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct Stroke {
    /// Width in reference pixels, 0 to 4000.
    #[specta(type = specta_typescript::Number)]
    pub width: f64,
    /// Corner style.
    pub join: StrokeJoin,
    /// Colour.
    pub color: Color,
    /// Behind or over the fill.
    pub order: StrokeOrder,
}

impl Stroke {
    fn check(&self) -> Result<(), TextError> {
        length("/width", self.width, 0.0, MAX_LENGTH, "the stroke width")
    }
}

/// A drop shadow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct Shadow {
    /// Offset in reference pixels (+y is down).
    pub offset: Vec2,
    /// Blur radius in reference pixels, 0 to 4000.
    #[specta(type = specta_typescript::Number)]
    pub blur: f64,
    /// Colour (alpha is the strength).
    pub color: Color,
}

impl Shadow {
    fn check(&self) -> Result<(), TextError> {
        if !self.offset.is_finite()
            || self.offset.x.abs() > MAX_LENGTH
            || self.offset.y.abs() > MAX_LENGTH
        {
            return Err(TextError::new(
                "/offset",
                format!("each offset is within -{MAX_LENGTH} to {MAX_LENGTH}"),
            ));
        }
        length("/blur", self.blur, 0.0, MAX_LENGTH, "the blur")
    }
}

/// A soft halo around the glyphs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct Glow {
    /// Radius in reference pixels, 0 to 4000.
    #[specta(type = specta_typescript::Number)]
    pub radius: f64,
    /// Colour.
    pub color: Color,
    /// Strength, 0 to 4 (1 is a plain halo).
    #[specta(type = specta_typescript::Number)]
    pub intensity: f64,
}

impl Glow {
    fn check(&self) -> Result<(), TextError> {
        length("/radius", self.radius, 0.0, MAX_LENGTH, "the radius")?;
        length("/intensity", self.intensity, 0.0, 4.0, "the intensity")
    }
}

/// Whether a background box wraps the whole text or each line.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundMode {
    /// One rectangle behind all lines.
    #[default]
    Block,
    /// One rectangle per line (a "pill" behind each).
    PerLine,
}

/// A coloured box behind the text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct Background {
    /// Colour.
    pub color: Color,
    /// Space between text and box edge, in reference pixels, 0 to 4000.
    #[specta(type = specta_typescript::Number)]
    pub padding: f64,
    /// Corner radius in reference pixels, 0 to 4000.
    #[specta(type = specta_typescript::Number)]
    pub radius: f64,
    /// One box or one per line.
    pub mode: BackgroundMode,
}

impl Background {
    fn check(&self) -> Result<(), TextError> {
        length("/padding", self.padding, 0.0, MAX_LENGTH, "the padding")?;
        length("/radius", self.radius, 0.0, MAX_LENGTH, "the radius")
    }
}

// ----- layout enums ------------------------------------------------------------------------------

/// Horizontal alignment of lines in the box.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    /// Flush left (flush start in right-to-left text is `start` in a later version).
    Left,
    /// Centred.
    #[default]
    Center,
    /// Flush right.
    Right,
    /// Stretched to both edges (the last line stays flush left).
    Justify,
}

/// Vertical alignment of the text block in the box.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum VerticalAlign {
    /// Against the top edge.
    Top,
    /// In the middle.
    #[default]
    Middle,
    /// Against the bottom edge.
    Bottom,
}

/// Letter case applied when laying out (the stored text is unchanged).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum CaseTransform {
    /// As typed.
    #[default]
    None,
    /// ALL CAPS.
    Upper,
    /// all lower.
    Lower,
    /// Each Word Capitalised.
    Title,
}

/// What happens when the text does not fit the box.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum AutoFit {
    /// Nothing: the text may overflow.
    #[default]
    None,
    /// The font size shrinks until it fits.
    Shrink,
    /// The box grows to hold the text.
    GrowBox,
}

/// The base direction of the text.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum TextDirection {
    /// From the first strong character.
    #[default]
    Auto,
    /// Left to right.
    Ltr,
    /// Right to left.
    Rtl,
}

// ----- the style ---------------------------------------------------------------------------------

/// Range of `size` in reference pixels.
pub const SIZE_RANGE: (f64, f64) = (1.0, MAX_LENGTH);
/// Range of `letter_spacing` in em.
pub const LETTER_SPACING_RANGE: (f64, f64) = (-1.0, 10.0);
/// Range of `line_height`, a multiple of the font size.
pub const LINE_HEIGHT_RANGE: (f64, f64) = (0.1, 10.0);

/// The look of a text item (WP-3A.01).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct TextStyle {
    /// The font.
    pub font: FontSpec,
    /// Font size in reference pixels, 1 to 4000 (animatable: `text.size`).
    #[specta(type = Animatable<specta_typescript::Number>)]
    pub size: Animatable<f64>,
    /// The glyph fill.
    pub fill: Fill,
    /// Outlines, drawn in order (at most 8).
    #[serde(default)]
    pub strokes: Vec<Stroke>,
    /// Drop shadows (at most 8).
    #[serde(default)]
    pub shadows: Vec<Shadow>,
    /// A halo, if any.
    #[serde(default)]
    pub glow: Option<Glow>,
    /// A box behind the text, if any.
    #[serde(default)]
    pub background: Option<Background>,
    /// Extra space between letters in em, -1 to 10 (animatable: `text.letter_spacing`).
    #[specta(type = Animatable<specta_typescript::Number>)]
    pub letter_spacing: Animatable<f64>,
    /// Line height as a multiple of the size, 0.1 to 10 (animatable: `text.line_height`).
    #[specta(type = Animatable<specta_typescript::Number>)]
    pub line_height: Animatable<f64>,
    /// Horizontal alignment.
    pub align: TextAlign,
    /// Vertical alignment in the box.
    pub v_align: VerticalAlign,
    /// Case applied at layout.
    pub case_transform: CaseTransform,
    /// What to do when the text does not fit.
    pub auto_fit: AutoFit,
    /// Base direction.
    pub direction: TextDirection,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

impl Default for TextStyle {
    /// A bold white 72 px Inter with a soft shadow, centred.
    fn default() -> TextStyle {
        TextStyle {
            font: FontSpec::default(),
            size: 72.0.into(),
            fill: Fill::solid(Color::rgb(255, 255, 255)),
            strokes: Vec::new(),
            shadows: vec![Shadow {
                offset: Vec2::new(0.0, 2.0),
                blur: 8.0,
                color: Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 153,
                },
            }],
            glow: None,
            background: None,
            letter_spacing: 0.0.into(),
            line_height: 1.2.into(),
            align: TextAlign::Center,
            v_align: VerticalAlign::Middle,
            case_transform: CaseTransform::None,
            auto_fit: AutoFit::None,
            direction: TextDirection::Auto,
            extra: Extra::new(),
        }
    }
}

fn check_all<T>(
    items: &[T],
    max: usize,
    base: &str,
    check: impl Fn(&T) -> Result<(), TextError>,
) -> Result<(), TextError> {
    if items.len() > max {
        return Err(TextError::new(base, format!("at most {max} entries")));
    }
    for (i, item) in items.iter().enumerate() {
        check(item).map_err(|e| e.under(&format!("{base}/{i}")))?;
    }
    Ok(())
}

impl TextStyle {
    /// Checks every value against its range. The first problem is returned, with its path
    /// relative to the style (`/size`, `/strokes/0/width`).
    pub fn check(&self) -> Result<(), TextError> {
        self.font.check().map_err(|e| e.under("/font"))?;
        for (path, a, (min, max), what) in [
            ("/size", &self.size, SIZE_RANGE, "the font size"),
            (
                "/letter_spacing",
                &self.letter_spacing,
                LETTER_SPACING_RANGE,
                "the letter spacing",
            ),
            (
                "/line_height",
                &self.line_height,
                LINE_HEIGHT_RANGE,
                "the line height",
            ),
        ] {
            a.check_range(crate::ValueRange::new(min, max))
                .map_err(|_| {
                    TextError::new(path, format!("{what} must be a number from {min} to {max}"))
                })?;
        }
        self.fill.check().map_err(|e| e.under("/fill"))?;
        check_all(&self.strokes, MAX_LAYERS, "/strokes", Stroke::check)?;
        check_all(&self.shadows, MAX_LAYERS, "/shadows", Shadow::check)?;
        if let Some(g) = &self.glow {
            g.check().map_err(|e| e.under("/glow"))?;
        }
        if let Some(b) = &self.background {
            b.check().map_err(|e| e.under("/background"))?;
        }
        Ok(())
    }
}

/// A style override for a run: only the fields given replace the item's style for those
/// characters. Static values only (no keys).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct RunStyle {
    /// Font.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<FontSpec>,
    /// Size in reference pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<specta_typescript::Number>)]
    pub size: Option<f64>,
    /// Fill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Fill>,
    /// Outlines (replace the style's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strokes: Option<Vec<Stroke>>,
    /// Shadows (replace the style's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadows: Option<Vec<Shadow>>,
    /// Letter spacing in em.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<specta_typescript::Number>)]
    pub letter_spacing: Option<f64>,
    /// Case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_transform: Option<CaseTransform>,
}

impl RunStyle {
    /// Whether no field is set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == RunStyle::default()
    }

    /// Checks the fields that are set.
    pub fn check(&self) -> Result<(), TextError> {
        if let Some(f) = &self.font {
            f.check().map_err(|e| e.under("/font"))?;
        }
        if let Some(s) = self.size {
            length("/size", s, SIZE_RANGE.0, SIZE_RANGE.1, "the font size")?;
        }
        if let Some(f) = &self.fill {
            if matches!(f, Fill::Solid { color } if color.is_animated()) {
                return Err(TextError::new("/fill", "a run's fill cannot be keyed"));
            }
            f.check().map_err(|e| e.under("/fill"))?;
        }
        if let Some(s) = &self.strokes {
            check_all(s, MAX_LAYERS, "/strokes", Stroke::check)?;
        }
        if let Some(s) = &self.shadows {
            check_all(s, MAX_LAYERS, "/shadows", Shadow::check)?;
        }
        if let Some(l) = self.letter_spacing {
            length(
                "/letter_spacing",
                l,
                LETTER_SPACING_RANGE.0,
                LETTER_SPACING_RANGE.1,
                "the letter spacing",
            )?;
        }
        Ok(())
    }
}

// ----- the content -------------------------------------------------------------------------------

/// A stretch of text with one style override.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct Run {
    /// The characters (may hold `\n` line breaks).
    pub text: String,
    /// Overrides of the item's style for this run; absent: the item's style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<RunStyle>,
}

impl Run {
    /// A run in the item's style.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Run {
        Run {
            text: text.into(),
            style: None,
        }
    }
}

/// The words of a text item: runs, in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct RichText {
    /// The runs. Adjacent runs with equal overrides are kept merged.
    pub runs: Vec<Run>,
}

impl RichText {
    /// Plain text in one run (none for the empty string).
    #[must_use]
    pub fn plain(text: impl Into<String>) -> RichText {
        let text = text.into();
        RichText {
            runs: if text.is_empty() {
                Vec::new()
            } else {
                vec![Run::plain(text)]
            },
        }
    }

    /// All characters, runs joined.
    #[must_use]
    pub fn to_plain(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    /// Number of characters (Unicode scalar values).
    #[must_use]
    pub fn char_count(&self) -> usize {
        self.runs.iter().map(|r| r.text.chars().count()).sum()
    }

    /// Checks sizes and run styles.
    pub fn check(&self) -> Result<(), TextError> {
        if self.runs.len() > MAX_RUNS {
            return Err(TextError::new(
                "/runs",
                format!("at most {MAX_RUNS} runs; merge runs with the same style"),
            ));
        }
        if self.char_count() > MAX_TEXT_CHARS {
            return Err(TextError::new(
                "/runs",
                format!("at most {MAX_TEXT_CHARS} characters"),
            ));
        }
        for (i, run) in self.runs.iter().enumerate() {
            if run.text.is_empty() {
                return Err(TextError::new(
                    format!("/runs/{i}/text"),
                    "a run is not empty",
                ));
            }
            if let Some(s) = &run.style {
                s.check()
                    .map_err(|e| e.under(&format!("/runs/{i}/style")))?;
            }
        }
        Ok(())
    }

    /// Drops empty runs and merges neighbours with equal overrides.
    pub fn normalize(&mut self) {
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        for run in self.runs.drain(..) {
            if run.text.is_empty() {
                continue;
            }
            let style = run.style.filter(|s| !s.is_empty());
            match out.last_mut() {
                Some(last) if last.style == style => last.text.push_str(&run.text),
                _ => out.push(Run {
                    text: run.text,
                    style,
                }),
            }
        }
        self.runs = out;
    }

    /// Splits runs so that `[start, end)` (character offsets) begins and ends on run
    /// boundaries, then calls `f` on the runs inside it. Offsets past the end are clamped.
    pub fn edit_range(&mut self, start: usize, end: usize, mut f: impl FnMut(&mut Run)) {
        let split_at = |runs: &mut Vec<Run>, at: usize| {
            let mut pos = 0;
            for i in 0..runs.len() {
                let n = runs[i].text.chars().count();
                if at > pos && at < pos + n {
                    let byte = runs[i]
                        .text
                        .char_indices()
                        .nth(at - pos)
                        .map_or(runs[i].text.len(), |(b, _)| b);
                    let tail = runs[i].text.split_off(byte);
                    let style = runs[i].style.clone();
                    runs.insert(i + 1, Run { text: tail, style });
                    return;
                }
                pos += n;
            }
        };
        split_at(&mut self.runs, start);
        split_at(&mut self.runs, end);
        let mut pos = 0;
        for run in &mut self.runs {
            let n = run.text.chars().count();
            if pos >= start && pos + n <= end {
                f(run);
            }
            pos += n;
        }
    }

    /// Inserts `text` at character offset `at` (clamped to the end), in the style of the run
    /// it lands in (the previous run at a boundary).
    pub fn insert(&mut self, at: usize, text: &str) {
        let mut pos = 0;
        for run in &mut self.runs {
            let n = run.text.chars().count();
            if at <= pos + n {
                let byte = run
                    .text
                    .char_indices()
                    .nth(at - pos)
                    .map_or(run.text.len(), |(b, _)| b);
                run.text.insert_str(byte, text);
                return;
            }
            pos += n;
        }
        match self.runs.last_mut() {
            Some(last) => last.text.push_str(text),
            None => self.runs.push(Run::plain(text)),
        }
    }
}

// ----- box, references ---------------------------------------------------------------------------

/// The rectangle text lays out in: it wraps at `width` and aligns inside `width` x `height`.
/// Sizes are canvas units; `None` is automatic (as wide or high as the text).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(deny_unknown_fields)]
pub struct TextBox {
    /// Width in canvas widths, above 0 and at most 4; `None`: no wrapping.
    #[serde(default)]
    #[specta(type = Option<specta_typescript::Number>)]
    pub width: Option<f64>,
    /// Height in canvas heights, above 0 and at most 4; `None`: as high as the text.
    #[serde(default)]
    #[specta(type = Option<specta_typescript::Number>)]
    pub height: Option<f64>,
}

impl TextBox {
    /// Checks the sizes.
    pub fn check(&self) -> Result<(), TextError> {
        for (path, v) in [("/width", self.width), ("/height", self.height)] {
            if let Some(v) = v
                && !(v.is_finite() && v > 0.0 && v <= 4.0)
            {
                return Err(TextError::new(
                    path,
                    "a box side must be above 0 and at most 4 canvas units; leave it out for automatic",
                ));
            }
        }
        Ok(())
    }
}

/// Where an item's style comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum TextStyleRef {
    /// A built-in style by name (see [`builtin_styles`]).
    Named {
        /// `title`, `subtitle` or `body`.
        name: String,
    },
    /// A style of its own.
    Inline {
        /// The style.
        style: Box<TextStyle>,
    },
}

impl TextStyleRef {
    /// The style, resolving a name; `None` for an unknown name.
    #[must_use]
    pub fn resolve(&self) -> Option<Cow<'_, TextStyle>> {
        match self {
            TextStyleRef::Inline { style } => Some(Cow::Borrowed(style)),
            TextStyleRef::Named { name } => builtin_style(name).map(Cow::Owned),
        }
    }
}

/// A reference to a text animation (3C fills this in; always `None` until then).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct AnimRef {
    /// The animation's ID.
    pub id: String,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// In, out and loop animations of a text item. WP-3A.01 never sets one; the field is `None`
/// until WP-3C.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct TextAnimation {
    /// Plays as the item appears.
    #[serde(rename = "in", default)]
    pub in_: Option<AnimRef>,
    /// Plays as the item leaves.
    #[serde(default)]
    pub out: Option<AnimRef>,
    /// Plays all the while.
    #[serde(rename = "loop", default)]
    pub loop_: Option<AnimRef>,
}

// ----- built-in styles ---------------------------------------------------------------------------

/// A built-in style as `text.styles.list` shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct NamedStyle {
    /// The name used in a [`TextStyleRef::Named`].
    pub name: String,
    /// What it is for.
    pub description: String,
    /// The style.
    pub style: TextStyle,
}

/// The built-in styles: `title`, `subtitle` and `body`.
#[must_use]
pub fn builtin_styles() -> Vec<NamedStyle> {
    let mut title = TextStyle::default();
    title.font.weight = 800;
    title.size = 96.0.into();
    let mut subtitle = TextStyle::default();
    subtitle.font.weight = 600;
    subtitle.size = 56.0.into();
    let mut body = TextStyle::default();
    body.font.weight = 400;
    body.size = 40.0.into();
    body.line_height = 1.3.into();
    vec![
        NamedStyle {
            name: "title".to_owned(),
            description: "A large bold title, centred, with a soft shadow.".to_owned(),
            style: title,
        },
        NamedStyle {
            name: "subtitle".to_owned(),
            description: "A medium semi-bold line under a title.".to_owned(),
            style: subtitle,
        },
        NamedStyle {
            name: "body".to_owned(),
            description: "Regular running text.".to_owned(),
            style: body,
        },
    ]
}

/// The built-in style called `name`.
#[must_use]
pub fn builtin_style(name: &str) -> Option<TextStyle> {
    builtin_styles()
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| s.style)
}

// ----- the item ----------------------------------------------------------------------------------

/// A text item on a text track (arch §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct TextItem {
    /// The item ID (also its key in the track's `items`).
    pub id: crate::ItemId,
    /// Display name; `None` shows the start of the text.
    pub name: Option<String>,
    /// Timeline position of the first frame.
    pub start: crate::Time,
    /// Length on the timeline (positive).
    pub duration: crate::Duration,
    /// Disabled items are kept but not rendered.
    pub enabled: bool,
    /// Items with the same group move, trim and delete together.
    pub linked_group: Option<crate::GroupId>,
    /// The item this one follows when it moves.
    pub attached_to: Option<crate::ItemId>,
    /// Markers in item-relative time.
    #[schemars(with = "std::collections::BTreeMap<crate::MarkerId, crate::Marker>")]
    #[specta(type = std::collections::BTreeMap<crate::MarkerId, crate::Marker>)]
    pub markers: crate::Markers,
    /// The color label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_label: Option<Color>,
    /// The words.
    pub content: RichText,
    /// The look: a built-in style by name, or one of its own.
    pub style: TextStyleRef,
    /// The layout rectangle.
    #[serde(rename = "box")]
    pub text_box: TextBox,
    /// In, out and loop animations; `None` until WP-3C.
    #[serde(default)]
    pub animation: Option<TextAnimation>,
    /// Placement and opacity (the same properties a clip has).
    pub visual: crate::VisualProps,
    /// Plugin data.
    pub custom: crate::Custom,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

impl TextItem {
    /// An enabled item with `content` in the built-in `title` style, a box as wide as the
    /// text and default visual properties.
    #[must_use]
    pub fn new(
        id: crate::ItemId,
        start: crate::Time,
        duration: crate::Duration,
        content: RichText,
    ) -> TextItem {
        TextItem {
            id,
            name: None,
            start,
            duration,
            enabled: true,
            linked_group: None,
            attached_to: None,
            markers: crate::Markers::new(),
            color_label: None,
            content,
            style: TextStyleRef::Named {
                name: "title".to_owned(),
            },
            text_box: TextBox::default(),
            animation: None,
            visual: crate::VisualProps::default(),
            custom: crate::Custom::new(),
            extra: Extra::new(),
        }
    }

    /// The style in effect: the named built-in, or the inline one. An unknown name (a
    /// document from a build with more styles) reads as the default style.
    #[must_use]
    pub fn resolved_style(&self) -> Cow<'_, TextStyle> {
        self.style
            .resolve()
            .unwrap_or_else(|| Cow::Owned(TextStyle::default()))
    }

    /// Makes the style inline (a copy of the built-in one it names), so its animatable
    /// properties can be keyed. Does nothing for an inline style.
    pub fn detach_style(&mut self) {
        if let TextStyleRef::Named { .. } = &self.style {
            self.style = TextStyleRef::Inline {
                style: Box::new(self.resolved_style().into_owned()),
            };
        }
    }

    /// Checks the content, style and box.
    pub fn check(&self) -> Result<(), TextError> {
        self.content.check().map_err(|e| e.under("/content"))?;
        match &self.style {
            TextStyleRef::Inline { style } => style.check().map_err(|e| e.under("/style/style"))?,
            TextStyleRef::Named { name } => {
                if builtin_style(name).is_none() {
                    return Err(TextError::new(
                        "/style/name",
                        format!("unknown style `{name}`; use title, subtitle or body"),
                    ));
                }
            }
        }
        self.text_box.check().map_err(|e| e.under("/box"))
    }

    /// The animatable property `id` of this item, if it has it.
    #[must_use]
    pub fn prop(&self, id: PropId) -> Option<crate::PropRef<'_>> {
        use crate::PropRef;
        let v = &self.visual;
        Some(match id {
            PropId::TransformPosition => PropRef::Vec2(&v.transform.position),
            PropId::TransformScale => PropRef::Vec2(&v.transform.scale),
            PropId::TransformRotation => PropRef::Scalar(&v.transform.rotation),
            PropId::TransformAnchor => PropRef::Vec2(&v.transform.anchor),
            PropId::Opacity => PropRef::Scalar(&v.opacity),
            PropId::TextSize | PropId::TextLetterSpacing | PropId::TextLineHeight => {
                let TextStyleRef::Inline { style } = &self.style else {
                    return None;
                };
                match id {
                    PropId::TextSize => PropRef::Scalar(&style.size),
                    PropId::TextLetterSpacing => PropRef::Scalar(&style.letter_spacing),
                    _ => PropRef::Scalar(&style.line_height),
                }
            }
            _ => return None,
        })
    }

    /// The mutable animatable property `id`, if it has it.
    pub fn prop_mut(&mut self, id: PropId) -> Option<crate::PropMut<'_>> {
        use crate::PropMut;
        let v = &mut self.visual;
        Some(match id {
            PropId::TransformPosition => PropMut::Vec2(&mut v.transform.position),
            PropId::TransformScale => PropMut::Vec2(&mut v.transform.scale),
            PropId::TransformRotation => PropMut::Scalar(&mut v.transform.rotation),
            PropId::TransformAnchor => PropMut::Vec2(&mut v.transform.anchor),
            PropId::Opacity => PropMut::Scalar(&mut v.opacity),
            PropId::TextSize | PropId::TextLetterSpacing | PropId::TextLineHeight => {
                let TextStyleRef::Inline { style } = &mut self.style else {
                    return None;
                };
                match id {
                    PropId::TextSize => PropMut::Scalar(&mut style.size),
                    PropId::TextLetterSpacing => PropMut::Scalar(&mut style.letter_spacing),
                    _ => PropMut::Scalar(&mut style.line_height),
                }
            }
            _ => return None,
        })
    }
}

/// One flat schema for the style names is not needed; this keeps the unused-import lint quiet
/// for `Schema` helpers used by the macro expansions on some toolchains.
#[allow(dead_code)]
fn _schema_helpers(_: &mut SchemaGenerator) -> Schema {
    json_schema!({})
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use serde_json::json;

    use super::*;

    #[test]
    fn default_style_is_valid_and_round_trips() {
        let style = TextStyle::default();
        style.check().unwrap();
        let v = serde_json::to_value(&style).unwrap();
        assert_eq!(v["size"], 72.0);
        assert_eq!(v["fill"]["kind"], "solid");
        assert_eq!(v["fill"]["color"], "#ffffff");
        assert_eq!(serde_json::from_value::<TextStyle>(v).unwrap(), style);
    }

    #[test]
    fn builtin_styles_are_valid() {
        for s in builtin_styles() {
            s.style.check().unwrap();
        }
        assert!(builtin_style("nope").is_none());
    }

    #[test]
    fn checks_name_the_path() {
        let s = TextStyle {
            size: 0.0.into(),
            ..TextStyle::default()
        };
        assert_eq!(s.check().unwrap_err().path, "/size");
        let mut s = TextStyle::default();
        s.strokes.push(Stroke {
            width: -1.0,
            join: StrokeJoin::Round,
            color: Color::BLACK,
            order: StrokeOrder::Behind,
        });
        assert_eq!(s.check().unwrap_err().path, "/strokes/0/width");
        let mut s = TextStyle::default();
        s.font.weight = 0;
        assert_eq!(s.check().unwrap_err().path, "/font/weight");
        let s = TextStyle {
            fill: Fill::Linear {
                angle: 0.0,
                stops: vec![GradientStop {
                    at: 0.0,
                    color: Color::BLACK,
                }],
            },
            ..TextStyle::default()
        };
        assert_eq!(s.check().unwrap_err().path, "/fill/stops");
    }

    #[test]
    fn rich_text_edits() {
        let mut t = RichText::plain("héllo world");
        t.edit_range(1, 4, |r| {
            r.style = Some(RunStyle {
                size: Some(10.0),
                ..RunStyle::default()
            });
        });
        assert_eq!(
            t.runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
            ["h", "éll", "o world"]
        );
        t.edit_range(1, 4, |r| r.style = None);
        t.normalize();
        assert_eq!(t.runs.len(), 1);
        t.insert(5, ",");
        assert_eq!(t.to_plain(), "héllo, world");
        t.insert(999, "!");
        assert_eq!(t.to_plain(), "héllo, world!");
        assert_eq!(t.char_count(), 13);
    }

    #[test]
    fn item_wire_form_uses_box_and_style_source() {
        let item = TextItem::new(
            crate::ItemId::from_u128(1),
            crate::Time::ZERO,
            crate::Duration::from_secs(5).unwrap(),
            RichText::plain("Hi"),
        );
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["box"], json!({"width": null, "height": null}));
        assert_eq!(v["style"], json!({"source": "named", "name": "title"}));
        assert_eq!(v["animation"], json!(null));
        assert_eq!(serde_json::from_value::<TextItem>(v).unwrap(), item);
        item.check().unwrap();
    }
}
