//! Scene graph: what timeline evaluation (or a composition, WP-0G.01) produces for one frame.
//!
//! A [`Scene`] is a stack of [`Layer`]s. Every layer has *content* (a [`Node`]), a 2D
//! [`Transform`], opacity, a [`BlendMode`] and a list of [`Effect`]s. The contract every content
//! node fulfils is the same: it renders to a **premultiplied, linear-light, Rec.709-primaries
//! `Rgba16Float` texture of its intrinsic size**. Effects then run on that texture and the
//! compositor places it with the layer transform. That contract is the plug-in point for new node
//! kinds: [`Node::Text`] (WP-0F.02) only needs a pass that rasterises glyphs into such a texture.
//!
//! The whole scene is plain data (`serde`), so a QuickJS composition can emit it as JSON
//! (WP-0G.01) and `s4 render --scene-json` draws it. Pixel data never lives in the scene: media is
//! referenced by id and resolved through a [`crate::source::SourceProvider`].

use serde::{Deserialize, Serialize};

/// Engine time unit (§4.1): 1/705,600,000 s.
pub type Flicks = i64;
/// Flicks per second.
pub const FLICKS_PER_SECOND: Flicks = 705_600_000;

/// Transfer characteristic (signal ↔ linear light).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transfer {
    /// ITU-R BT.709 OETF (and its inverse on input). Scene-linear working space.
    #[default]
    Bt709,
    /// IEC 61966-2-1 sRGB.
    Srgb,
    /// ITU-R BT.1886 display EOTF (pure 2.4 gamma, black at 0).
    Bt1886,
    /// No transfer (already linear).
    Linear,
    /// SMPTE ST 2084 (PQ), absolute luminance up to 10,000 cd/m².
    Pq,
}

impl Transfer {
    /// Id shared with the WGSL `to_linear` / `from_linear` switches.
    #[must_use]
    pub fn shader_id(self) -> u32 {
        match self {
            Self::Linear => 0,
            Self::Srgb => 1,
            Self::Bt709 => 2,
            Self::Bt1886 => 3,
            Self::Pq => 4,
        }
    }

    /// ITU-T H.273 TransferCharacteristics code point (for PNG `cICP`).
    #[must_use]
    pub fn h273(self) -> u8 {
        match self {
            Self::Bt709 | Self::Bt1886 => 1,
            Self::Linear => 8,
            Self::Srgb => 13,
            Self::Pq => 16,
        }
    }
}

/// Colour primaries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Primaries {
    #[default]
    Bt709,
    Bt2020,
}

impl Primaries {
    /// ITU-T H.273 ColourPrimaries code point.
    #[must_use]
    pub fn h273(self) -> u8 {
        match self {
            Self::Bt709 => 1,
            Self::Bt2020 => 9,
        }
    }
}

/// Y'CbCr matrix coefficients (`Rgb` = the samples are already R'G'B').
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Matrix {
    Rgb,
    Bt601,
    #[default]
    Bt709,
    Bt2020Ncl,
}

/// Quantisation range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Range {
    /// "TV"/narrow range: Y' 16–235, C 16–240 (8-bit).
    #[default]
    Limited,
    /// "PC"/full range.
    Full,
}

/// Colour tags of a source, as a media probe reports them (or a user override).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ColorTags {
    pub matrix: Matrix,
    pub range: Range,
    pub transfer: Transfer,
    pub primaries: Primaries,
}

/// PQ → SDR tone-map parameters (ITU-R BT.2390 EETF applied to max(R,G,B)).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToneMap {
    /// Mastering display / content peak (cd/m²); the EETF maps this to `target_peak_nits`.
    pub source_peak_nits: f32,
    /// Target (SDR) display peak (cd/m²).
    pub target_peak_nits: f32,
    /// Luminance that becomes linear 1.0 in the SDR working space.
    pub sdr_white_nits: f32,
}

impl Default for ToneMap {
    fn default() -> Self {
        Self {
            source_peak_nits: 1000.0,
            target_peak_nits: 100.0,
            sdr_white_nits: 100.0,
        }
    }
}

/// Core blend modes (§8). All blending happens in linear light on premultiplied colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    Add,
    SoftLight,
}

impl BlendMode {
    /// All core modes, in shader-id order.
    pub const ALL: [Self; 8] = [
        Self::Normal,
        Self::Multiply,
        Self::Screen,
        Self::Overlay,
        Self::Darken,
        Self::Lighten,
        Self::Add,
        Self::SoftLight,
    ];

    /// Id shared with the WGSL `blend_fn` switch.
    #[must_use]
    pub fn shader_id(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Multiply => 1,
            Self::Screen => 2,
            Self::Overlay => 3,
            Self::Darken => 4,
            Self::Lighten => 5,
            Self::Add => 6,
            Self::SoftLight => 7,
        }
    }
}

/// 2D layer transform: `canvas = position + R(rotation) · S(scale) · (p − anchor·size)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    /// Where the anchor lands, in canvas pixels.
    pub position: [f32; 2],
    /// Anchor in normalised layer coordinates (0,0 = top-left, 1,1 = bottom-right).
    pub anchor: [f32; 2],
    pub scale: [f32; 2],
    /// Clockwise degrees (y points down).
    pub rotation_deg: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0],
            anchor: [0.0, 0.0],
            scale: [1.0, 1.0],
            rotation_deg: 0.0,
        }
    }
}

/// Affine map `[a, b, c, d, e, f]`: `x' = a·x + b·y + c`, `y' = d·x + e·y + f`.
pub type Affine = [f64; 6];

impl Transform {
    /// Layer pixels → canvas pixels for a layer of `size` (w, h).
    #[must_use]
    pub fn forward(&self, size: [f64; 2]) -> Affine {
        let (s, c) = f64::from(self.rotation_deg).to_radians().sin_cos();
        let (sx, sy) = (f64::from(self.scale[0]), f64::from(self.scale[1]));
        let ax = f64::from(self.anchor[0]) * size[0];
        let ay = f64::from(self.anchor[1]) * size[1];
        let (a, b, d, e) = (c * sx, -s * sy, s * sx, c * sy);
        let px = f64::from(self.position[0]);
        let py = f64::from(self.position[1]);
        [a, b, px - a * ax - b * ay, d, e, py - d * ax - e * ay]
    }

    /// Canvas pixels → layer pixels; `None` for a degenerate (zero-scale) transform.
    #[must_use]
    pub fn inverse(&self, size: [f64; 2]) -> Option<Affine> {
        invert_affine(&self.forward(size))
    }
}

/// Inverts an affine map; `None` if singular.
#[must_use]
pub fn invert_affine(m: &Affine) -> Option<Affine> {
    let [a, b, c, d, e, f] = *m;
    let det = a * e - b * d;
    if det.abs() < 1e-12 {
        return None;
    }
    let (ia, ib, id, ie) = (e / det, -b / det, -d / det, a / det);
    Some([ia, ib, -(ia * c + ib * f), id, ie, -(id * c + ie * f)])
}

/// `outer ∘ inner` (apply `inner` first).
#[must_use]
pub fn compose_affine(outer: &Affine, inner: &Affine) -> Affine {
    let [a, b, c, d, e, f] = *outer;
    let [a2, b2, c2, d2, e2, f2] = *inner;
    [
        a * a2 + b * d2,
        a * b2 + b * e2,
        a * c2 + b * f2 + c,
        d * a2 + e * d2,
        d * b2 + e * e2,
        d * c2 + e * f2 + f,
    ]
}

/// Applies an affine map to a point.
#[must_use]
pub fn apply_affine(m: &Affine, p: [f64; 2]) -> [f64; 2] {
    [
        m[0] * p[0] + m[1] * p[1] + m[2],
        m[3] * p[0] + m[4] * p[1] + m[5],
    ]
}

/// Per-layer (or sequence-level) effect. Effects are pure functions of their inputs: no
/// persistent state across frames (§8); temporal effects would declare history taps instead.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    /// Separable Gaussian blur, σ = radius / 2, up to 15 taps each side.
    BlurLite { radius: f32 },
    /// `.cube` 3D LUT applied in `space` (the LUT's expected input encoding).
    Lut3d {
        lut: String,
        #[serde(default)]
        space: Transfer,
        #[serde(default = "one")]
        mix: f32,
    },
}

fn one() -> f32 {
    1.0
}

/// One entry of the layer stack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: String,
    pub content: Node,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default)]
    pub blend: BlendMode,
    #[serde(default)]
    pub effects: Vec<Effect>,
}

impl Layer {
    /// A full-opacity, normal-blend layer with an identity transform.
    #[must_use]
    pub fn new(id: impl Into<String>, content: Node) -> Self {
        Self {
            id: id.into(),
            content,
            transform: Transform::default(),
            opacity: 1.0,
            blend: BlendMode::Normal,
            effects: Vec::new(),
        }
    }
}

/// Layer content. Each variant renders to a premultiplied linear texture of its intrinsic size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    /// A decoded media frame (synthetic here) with its probe colour tags.
    Media(MediaNode),
    /// Vector shape (rect / rounded rect), as used by compositions (§12, WP-0G.01).
    Shape(ShapeNode),
    /// Nested layers composited into an intermediate of the group's size.
    Group(GroupNode),
    /// Two-input transition between clips on one track, evaluated in the enclosing canvas.
    Transition(TransitionNode),
    /// Styled, animatable text (WP-0F.02): shaped with cosmic-text, drawn by the MSDF atlas
    /// pass or by Vello. Rasterised under the layer transform (see `graph`); the box is the clip.
    Text(TextNode),
}

/// Media reference, resolved through a `SourceProvider`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaNode {
    pub source: String,
    /// Source frame index (the timeline maps sequence time → source time).
    #[serde(default)]
    pub frame: i64,
    /// Overrides the probed colour tags ("interpret footage as").
    #[serde(default)]
    pub tags: Option<ColorTags>,
    /// Tone-map parameters for PQ input; PQ sources get `ToneMap::default()` when absent.
    #[serde(default)]
    pub tone_map: Option<ToneMap>,
}

/// Analytic (anti-aliased) rounded rectangle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapeNode {
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub corner_radius: f32,
    /// sRGB-encoded, straight alpha (what a designer types as `#7c5cff`).
    pub fill_srgb: [f32; 4],
}

/// Group of layers rendered into a `width × height` intermediate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupNode {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
}

/// Transition kinds (only the cross-dissolve in this spike).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    #[default]
    CrossDissolve,
}

/// Transition between two clips. Each side is a full layer (own transform/effects) rendered
/// into the enclosing canvas; the result is blended in linear light by `progress` (0 → `from`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransitionNode {
    #[serde(default)]
    pub kind: TransitionKind,
    pub progress: f32,
    pub from: Box<Layer>,
    pub to: Box<Layer>,
}

/// Text node (WP-0F.02). The first five fields are the WP-0F.01 placeholder contract; the rest
/// are optional (serde defaults), so older scene JSON still parses.
///
/// Animation is *not* part of the node: an animator (a caption style or a composition, §11/§12)
/// evaluates its curves for the frame and emits [`TextSpan`]s with the resolved per-word or
/// per-glyph transform, opacity, colour and karaoke progress. The node is therefore a pure
/// description of one frame, like every other node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextNode {
    pub text: String,
    /// Family, optionally followed by a weight: `"Bricolage Grotesque 800"`. Unknown families
    /// fall back to the bundled UI sans (`text::fonts::DEFAULT_FAMILY`).
    pub font: String,
    pub size_px: f32,
    /// sRGB-encoded, straight alpha.
    pub fill_srgb: [f32; 4],
    /// Layout box (w, h) in pixels: wrap width, alignment frame and clip.
    pub box_size: [f32; 2],
    #[serde(default)]
    pub align: TextAlign,
    /// Line advance as a multiple of `size_px`.
    #[serde(default = "default_line_height")]
    pub line_height: f32,
    #[serde(default)]
    pub outline: Option<TextOutline>,
    #[serde(default)]
    pub shadow: Option<TextShadow>,
    /// Per-range overrides for this frame (animator output). Non-overlapping byte ranges.
    #[serde(default)]
    pub spans: Vec<TextSpan>,
    #[serde(default)]
    pub renderer: TextRenderer,
}

fn default_line_height() -> f32 {
    1.2
}

fn one2() -> [f32; 2] {
    [1.0, 1.0]
}

impl TextNode {
    /// A plain node with the WP-0F.01 fields and default styling (MSDF, left-aligned).
    #[must_use]
    pub fn new(
        text: impl Into<String>,
        font: impl Into<String>,
        size_px: f32,
        fill_srgb: [f32; 4],
        box_size: [f32; 2],
    ) -> Self {
        Self {
            text: text.into(),
            font: font.into(),
            size_px,
            fill_srgb,
            box_size,
            align: TextAlign::default(),
            line_height: default_line_height(),
            outline: None,
            shadow: None,
            spans: Vec::new(),
            renderer: TextRenderer::default(),
        }
    }
}

/// Horizontal alignment of each line inside the box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Which glyph renderer draws a text node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextRenderer {
    /// MTSDF atlas + instanced quads (the default).
    #[default]
    Msdf,
    /// Vello 0.12 compute rasteriser into an RGBA8 target, resolved into the linear graph.
    Vello,
}

/// Outline drawn under the fill (round joins).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextOutline {
    pub width_px: f32,
    pub color_srgb: [f32; 4],
}

/// Drop shadow of the (outlined) glyphs.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextShadow {
    pub offset_px: [f32; 2],
    /// Softness (width of the edge ramp) in pixels; 0 = hard.
    #[serde(default)]
    pub blur_px: f32,
    pub color_srgb: [f32; 4],
}

/// Karaoke highlight: the part of a span before `progress` (in reading direction) uses `color`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextHighlight {
    pub color_srgb: [f32; 4],
    /// 0 = nothing highlighted, 1 = whole span.
    pub progress: f32,
}

/// Resolved per-frame style of a byte range of the text (one word, one glyph, …).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextSpan {
    /// UTF-8 byte range `[start, end)` of `TextNode::text`.
    pub range: [u32; 2],
    /// Pivot at each glyph's centre instead of the span's centre.
    #[serde(default)]
    pub per_glyph: bool,
    #[serde(default)]
    pub offset_px: [f32; 2],
    #[serde(default = "one2")]
    pub scale: [f32; 2],
    #[serde(default)]
    pub rotation_deg: f32,
    #[serde(default = "one")]
    pub opacity: f32,
    /// Overrides the node fill.
    #[serde(default)]
    pub fill_srgb: Option<[f32; 4]>,
    #[serde(default)]
    pub highlight: Option<TextHighlight>,
}

impl TextSpan {
    /// Identity span over `range`.
    #[must_use]
    pub fn new(range: [u32; 2]) -> Self {
        Self {
            range,
            per_glyph: false,
            offset_px: [0.0; 2],
            scale: [1.0; 2],
            rotation_deg: 0.0,
            opacity: 1.0,
            fill_srgb: None,
            highlight: None,
        }
    }
}

/// Output (export) transform from the linear working space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputTransform {
    pub transfer: Transfer,
    #[serde(default)]
    pub primaries: Primaries,
    /// 8 or 16 bits per channel.
    pub bit_depth: u8,
}

impl Default for OutputTransform {
    fn default() -> Self {
        Self {
            transfer: Transfer::Bt709,
            primaries: Primaries::Bt709,
            bit_depth: 8,
        }
    }
}

/// One frame's scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    /// Linear-light, premultiplied background colour.
    pub background: [f32; 4],
    /// Bottom to top.
    pub layers: Vec<Layer>,
    /// Sequence-level adjustment effects applied to the composite (e.g. a grading LUT).
    #[serde(default)]
    pub effects: Vec<Effect>,
    #[serde(default)]
    pub output: OutputTransform,
}

impl Scene {
    /// Parses a scene from JSON (the WP-0G.01 composition runtime's output format).
    pub fn from_json(s: &str) -> crate::Result<Self> {
        serde_json::from_str(s).map_err(|e| crate::Error::InvalidScene(e.to_string()))
    }

    /// Pretty JSON.
    pub fn to_json(&self) -> crate::Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| crate::Error::InvalidScene(e.to_string()))
    }

    /// Basic validation shared by both renderers.
    pub fn validate(&self) -> crate::Result<()> {
        if self.width == 0 || self.height == 0 || self.width > 8192 || self.height > 8192 {
            return Err(crate::Error::InvalidScene(format!(
                "canvas {}x{} out of range 1..=8192",
                self.width, self.height
            )));
        }
        if self.output.bit_depth != 8 && self.output.bit_depth != 16 {
            return Err(crate::Error::InvalidScene(
                "output bit_depth must be 8 or 16".into(),
            ));
        }
        if self.output.primaries != Primaries::Bt709 {
            return Err(crate::Error::Unsupported(
                "output primaries other than BT.709 (HDR/wide-gamut output is v2)".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_inverse_round_trips() {
        let t = Transform {
            position: [300.0, 200.0],
            anchor: [0.5, 0.5],
            scale: [0.8, 0.6],
            rotation_deg: 17.0,
        };
        let size = [640.0, 360.0];
        let fwd = t.forward(size);
        let inv = t.inverse(size).expect("invertible");
        for p in [[0.0, 0.0], [640.0, 360.0], [12.5, 300.25]] {
            let q = apply_affine(&inv, apply_affine(&fwd, p));
            assert!((q[0] - p[0]).abs() < 1e-9 && (q[1] - p[1]).abs() < 1e-9);
        }
        // Anchor lands on position.
        let c = apply_affine(&fwd, [320.0, 180.0]);
        assert!((c[0] - 300.0).abs() < 1e-9 && (c[1] - 200.0).abs() < 1e-9);
    }

    #[test]
    fn zero_scale_is_not_invertible() {
        let t = Transform {
            scale: [0.0, 1.0],
            ..Transform::default()
        };
        assert!(t.inverse([10.0, 10.0]).is_none());
    }

    #[test]
    fn scene_json_round_trip_and_defaults() {
        let json = r#"{
            "width": 64, "height": 36, "background": [0,0,0,1],
            "layers": [
              {"id": "a", "content": {"type": "shape", "width": 10, "height": 5, "fill_srgb": [1,0,0,1]}},
              {"id": "t", "content": {"type": "text", "text": "Hi", "font": "Inter 700",
                "size_px": 42, "fill_srgb": [1,1,1,1], "box_size": [100, 50]},
               "blend": "screen", "effects": [{"type": "blur_lite", "radius": 2}]}
            ]
        }"#;
        let s = Scene::from_json(json).expect("parse");
        assert_eq!(s.layers[0].opacity, 1.0);
        assert_eq!(s.layers[1].blend, BlendMode::Screen);
        assert_eq!(s.output, OutputTransform::default());
        let back = Scene::from_json(&s.to_json().expect("ser")).expect("reparse");
        assert_eq!(back, s);
    }
}
