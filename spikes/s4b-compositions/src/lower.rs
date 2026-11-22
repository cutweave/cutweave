// SPDX-License-Identifier: GPL-3.0-or-later
//! Composition nodes → S4 scene graph.
//!
//! * Every node becomes an S4 [`Layer`]: `x, y` = where the node's anchor lands in the parent,
//!   `anchor` (normalised, default top-left), `scale`, `rotation` (clockwise degrees), `opacity`,
//!   `blend`, `blur` (S4 blur-lite effect).
//! * `rect` → S4 `Shape`; `ellipse` → `Shape` with `corner_radius = min(w, h) / 2` (exact for
//!   circles, a stadium otherwise: S4 has no ellipse primitive yet).
//! * `group` → S4 `Group` whose intermediate is sized to the bounding box of its children
//!   (rotations included), clipped to the part that can reach the canvas, and pivoting on the
//!   group's local origin. Group opacity is true group opacity (applied to the flattened result).
//! * `text` → S4 `Text` (box estimated from the font size; WP-0F.02 renders it) or, when the
//!   renderer has no text pass yet, a "greeked" placeholder: one rounded bar per word.
//! * `image` → S4 `Media` resolved by asset id, scaled to `w × h`.

use s4::scene::{
    Affine, BlendMode, Effect, GroupNode, Layer, MediaNode, Node, Scene, ShapeNode, TextNode,
    Transform, apply_affine, compose_affine, invert_affine,
};
use s4::source::SourceProvider;

use crate::diagnostics::{ComposeError, Diagnostic, ErrorKind, Phase, codes};
use crate::scene::{FrameOutput, RawNode};

/// How text nodes are emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextMode {
    /// S4 `Text` nodes (requires the WP-0F.02 text pass).
    Native,
    /// Greeked placeholder bars (S4 shapes).
    Placeholder,
}

/// Lowering parameters.
#[derive(Clone, Debug)]
pub struct LowerOptions {
    pub width: u32,
    pub height: u32,
    /// Linear-light, premultiplied background.
    pub background: [f32; 4],
    /// Optional media source composited below the composition (e.g. `camera709`).
    pub under: Option<String>,
    /// Source frame for `under`.
    pub under_frame: i64,
    pub text: TextMode,
    /// Output encoding (S4 default: BT.709 OETF, 8-bit; sRGB makes hex fills round-trip).
    pub output: s4::scene::OutputTransform,
}

impl Default for LowerOptions {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            background: [0.012, 0.014, 0.018, 1.0],
            under: None,
            under_frame: 0,
            text: TextMode::Placeholder,
            output: s4::scene::OutputTransform::default(),
        }
    }
}

const LINE_HEIGHT: f64 = 1.25;

/// Estimated advance per character (no font metrics in the sandbox or here; WP-0F.02 owns
/// shaping). Bold faces are a little wider.
fn advance(font: &str, size: f64) -> f64 {
    let bold = ["600", "700", "800", "900", "bold", "Bold"]
        .iter()
        .any(|w| font.contains(w));
    size * if bold { 0.58 } else { 0.54 }
}

/// Estimated text box (w, h).
fn text_box(text: &str, font: &str, size: f64) -> [f64; 2] {
    let adv = advance(font, size);
    let lines = text.split('\n');
    let (mut max_chars, mut n) = (0usize, 0usize);
    for l in lines {
        max_chars = max_chars.max(l.chars().count());
        n += 1;
    }
    [
        (max_chars as f64 * adv).ceil().max(1.0),
        (n as f64 * LINE_HEIGHT * size).ceil().max(1.0),
    ]
}

/// Parses `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` → sRGB straight alpha in [0, 1].
pub fn parse_color(s: &str) -> Option<[f32; 4]> {
    let h = s.strip_prefix('#')?;
    if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let nib = |i: usize| u8::from_str_radix(&h[i..=i], 16).ok().map(|v| v * 17);
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    let v = match h.len() {
        3 => [nib(0)?, nib(1)?, nib(2)?, 255],
        4 => [nib(0)?, nib(1)?, nib(2)?, nib(3)?],
        6 => [byte(0)?, byte(2)?, byte(4)?, 255],
        8 => [byte(0)?, byte(2)?, byte(4)?, byte(6)?],
        _ => return None,
    };
    Some(v.map(|c| f32::from(c) / 255.0))
}

fn parse_blend(s: &str) -> Option<BlendMode> {
    Some(match s {
        "normal" => BlendMode::Normal,
        "multiply" => BlendMode::Multiply,
        "screen" => BlendMode::Screen,
        "overlay" => BlendMode::Overlay,
        "darken" => BlendMode::Darken,
        "lighten" => BlendMode::Lighten,
        "add" => BlendMode::Add,
        "soft_light" | "soft-light" | "softLight" => BlendMode::SoftLight,
        _ => return None,
    })
}

struct Lowerer<'a> {
    opts: &'a LowerOptions,
    provider: &'a dyn SourceProvider,
    /// Index path of the node being lowered (for error messages and default ids).
    path: Vec<usize>,
}

/// A lowered node: the layer, its intrinsic size and the margin (px) its effects may bleed.
struct Lowered {
    layer: Layer,
    size: [f64; 2],
}

impl Lowerer<'_> {
    fn path_str(&self, kind: &str) -> String {
        let p: Vec<String> = self.path.iter().map(ToString::to_string).collect();
        format!("nodes/{} (<{kind}>)", p.join("/children/"))
    }

    fn err(&self, kind: &str, msg: impl Into<String>) -> ComposeError {
        ComposeError::new(
            ErrorKind::Scene,
            Diagnostic::error(codes::SCENE_INVALID, Phase::Scene, msg)
                .with_path(self.path_str(kind)),
        )
    }

    fn finite(
        &self,
        kind: &str,
        name: &str,
        v: Option<f64>,
        default: f64,
    ) -> Result<f64, ComposeError> {
        match v {
            None => Ok(default),
            Some(x) if x.is_finite() => Ok(x),
            Some(_) => Err(self.err(
                kind,
                format!("`{name}` must be a finite number (got NaN/Infinity)"),
            )),
        }
    }

    fn pair(
        &self,
        kind: &str,
        name: &str,
        v: Option<[f64; 2]>,
        default: [f64; 2],
    ) -> Result<[f64; 2], ComposeError> {
        match v {
            None => Ok(default),
            Some([a, b]) if a.is_finite() && b.is_finite() => Ok([a, b]),
            Some(_) => Err(self.err(kind, format!("`{name}` must be finite numbers"))),
        }
    }

    fn fill(&self, kind: &str, v: Option<&str>) -> Result<[f32; 4], ComposeError> {
        match v {
            None => Ok([1.0; 4]),
            Some(s) => parse_color(s).ok_or_else(|| {
                self.err(
                    kind,
                    format!("`fill` must be a hex colour like \"#7c5cff\", got \"{s}\""),
                )
            }),
        }
    }

    fn id(&self, n: &RawNode) -> String {
        n.id.clone().unwrap_or_else(|| {
            let p: Vec<String> = self.path.iter().map(ToString::to_string).collect();
            format!("n{}", p.join("."))
        })
    }

    /// Lowers the children of a group (or the root) in a space whose local→canvas map is
    /// `to_canvas`.
    fn children(
        &mut self,
        nodes: &[RawNode],
        to_canvas: &Affine,
    ) -> Result<Vec<Lowered>, ComposeError> {
        let mut out = Vec::with_capacity(nodes.len());
        for (i, n) in nodes.iter().enumerate() {
            self.path.push(i);
            let l = self.node(n, to_canvas)?;
            self.path.pop();
            if let Some(l) = l {
                out.push(l);
            }
        }
        Ok(out)
    }

    fn node(&mut self, n: &RawNode, to_canvas: &Affine) -> Result<Option<Lowered>, ComposeError> {
        let kind = n.kind.as_str();
        let x = self.finite(kind, "x", n.x, 0.0)?;
        let y = self.finite(kind, "y", n.y, 0.0)?;
        let opacity = self
            .finite(kind, "opacity", n.opacity, 1.0)?
            .clamp(0.0, 1.0);
        let rotation = self.finite(kind, "rotation", n.rotation, 0.0)?;
        let scale = self.pair(kind, "scale", n.scale, [1.0, 1.0])?;
        let anchor = self.pair(kind, "anchor", n.anchor, [0.0, 0.0])?;
        let blur = self.finite(kind, "blur", n.blur, 0.0)?.max(0.0);
        let blend = match n.blend.as_deref() {
            None => BlendMode::Normal,
            Some(b) => {
                parse_blend(b).ok_or_else(|| self.err(kind, format!("unknown blend mode `{b}`")))?
            }
        };
        let mut transform = Transform {
            position: [x as f32, y as f32],
            anchor: [anchor[0] as f32, anchor[1] as f32],
            scale: [scale[0] as f32, scale[1] as f32],
            rotation_deg: rotation as f32,
        };
        let (content, size) = match kind {
            "rect" | "ellipse" => {
                let w = self.finite(kind, "w", n.w, f64::NAN)?;
                let h = self.finite(kind, "h", n.h, f64::NAN)?;
                if n.w.is_none() || n.h.is_none() {
                    return Err(self.err(kind, "`w` and `h` are required"));
                }
                if w <= 0.0 || h <= 0.0 {
                    return Ok(None);
                }
                let radius = if kind == "ellipse" {
                    w.min(h) / 2.0
                } else {
                    self.finite(kind, "radius", n.radius, 0.0)?
                        .clamp(0.0, w.min(h) / 2.0)
                };
                let fill = self.fill(kind, n.fill.as_deref())?;
                (
                    Node::Shape(ShapeNode {
                        width: w as f32,
                        height: h as f32,
                        corner_radius: radius as f32,
                        fill_srgb: fill,
                    }),
                    [w, h],
                )
            }
            "text" => {
                let text = n.text.clone().unwrap_or_default();
                let size = self.finite(kind, "size", n.size, 32.0)?;
                if text.trim().is_empty() || size <= 0.0 {
                    return Ok(None);
                }
                let font = n.font.clone().unwrap_or_else(|| "Inter 400".into());
                let fill = self.fill(kind, n.fill.as_deref())?;
                let bx = text_box(&text, &font, size);
                let content = match self.opts.text {
                    TextMode::Native => native_text(&text, &font, size, fill, bx)
                        .map_err(|e| self.err(kind, format!("text node: {e}")))?,
                    TextMode::Placeholder => placeholder_text(&text, &font, size, fill, bx),
                };
                (content, bx)
            }
            "image" => {
                let Some(src) = n.src.clone() else {
                    return Err(self.err(kind, "`src` (asset id) is required"));
                };
                let frame = self.finite(kind, "frame", n.frame, 0.0)?.floor() as i64;
                let f = self
                    .provider
                    .frame(&src, frame)
                    .map_err(|e| self.err(kind, format!("asset `{src}` is not available: {e}")))?;
                let (sw, sh) = (f64::from(f.width), f64::from(f.height));
                let w = self.finite(kind, "w", n.w, sw)?;
                let h = self.finite(kind, "h", n.h, sh)?;
                if w <= 0.0 || h <= 0.0 {
                    return Ok(None);
                }
                transform.scale = [(scale[0] * w / sw) as f32, (scale[1] * h / sh) as f32];
                (
                    Node::Media(MediaNode {
                        source: src,
                        frame,
                        tags: None,
                        tone_map: None,
                    }),
                    [sw, sh],
                )
            }
            "group" => {
                // Local → parent: translate(x, y) · R · S (pivot = local origin).
                let local = Transform {
                    position: [x as f32, y as f32],
                    anchor: [0.0, 0.0],
                    scale: transform.scale,
                    rotation_deg: transform.rotation_deg,
                }
                .forward([1.0, 1.0]);
                let child_to_canvas = compose_affine(to_canvas, &local);
                let kids = self.children(&n.children, &child_to_canvas)?;
                if kids.is_empty() {
                    return Ok(None);
                }
                let Some(bbox) = self.group_bounds(&kids, &child_to_canvas, blur) else {
                    return Ok(None);
                };
                let [x0, y0, x1, y1] = bbox;
                let (w, h) = (x1 - x0, y1 - y0);
                let layers = kids
                    .into_iter()
                    .map(|mut k| {
                        k.layer.transform.position[0] -= x0 as f32;
                        k.layer.transform.position[1] -= y0 as f32;
                        k.layer
                    })
                    .collect();
                transform.anchor = [(-x0 / w) as f32, (-y0 / h) as f32];
                (
                    Node::Group(GroupNode {
                        width: w as u32,
                        height: h as u32,
                        layers,
                    }),
                    [w, h],
                )
            }
            other => {
                return Err(self.err(
                    other,
                    format!(
                        "unknown node type `{other}` (expected group, rect, ellipse, text, image)"
                    ),
                ));
            }
        };
        let mut layer = Layer::new(self.id(n), content);
        layer.transform = transform;
        layer.opacity = opacity as f32;
        layer.blend = blend;
        if blur > 0.0 {
            layer.effects.push(Effect::BlurLite {
                radius: blur as f32,
            });
        }
        Ok(Some(Lowered { layer, size }))
    }

    /// Integer bounds `[x0, y0, x1, y1]` of the children in group-local pixels, padded for
    /// anti-aliasing and clipped to what can reach the canvas. `None` if nothing is visible.
    fn group_bounds(&self, kids: &[Lowered], to_canvas: &Affine, blur: f64) -> Option<[f64; 4]> {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for k in kids {
            let m = k.layer.transform.forward(k.size);
            for c in [[0.0, 0.0], [k.size[0], 0.0], [0.0, k.size[1]], k.size] {
                let p = apply_affine(&m, c);
                x0 = x0.min(p[0]);
                y0 = y0.min(p[1]);
                x1 = x1.max(p[0]);
                y1 = y1.max(p[1]);
            }
        }
        let pad = 1.0 + 2.0 * blur;
        // Canvas rectangle mapped into group-local space (conservative under rotation).
        if let Some(inv) = invert_affine(to_canvas) {
            let (cw, ch) = (f64::from(self.opts.width), f64::from(self.opts.height));
            let (mut vx0, mut vy0, mut vx1, mut vy1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for c in [[0.0, 0.0], [cw, 0.0], [0.0, ch], [cw, ch]] {
                let p = apply_affine(&inv, c);
                vx0 = vx0.min(p[0]);
                vy0 = vy0.min(p[1]);
                vx1 = vx1.max(p[0]);
                vy1 = vy1.max(p[1]);
            }
            x0 = x0.max(vx0 - pad);
            y0 = y0.max(vy0 - pad);
            x1 = x1.min(vx1 + pad);
            y1 = y1.min(vy1 + pad);
        }
        let (x0, y0) = ((x0 - pad).floor(), (y0 - pad).floor());
        let (x1, y1) = ((x1 + pad).ceil(), (y1 + pad).ceil());
        if !(x1 - x0 >= 1.0 && y1 - y0 >= 1.0) {
            return None;
        }
        // S4 canvases (and so group intermediates) are limited to 8192².
        Some([x0, y0, x1.min(x0 + 8192.0), y1.min(y0 + 8192.0)])
    }
}

/// S4 `Text` node, built through serde so fields that WP-0F.02 adds with defaults still parse.
fn native_text(
    text: &str,
    font: &str,
    size: f64,
    fill: [f32; 4],
    bx: [f64; 2],
) -> Result<Node, String> {
    let v = serde_json::json!({
        "text": text,
        "font": font,
        "size_px": size,
        "fill_srgb": fill,
        "box_size": bx,
    });
    serde_json::from_value::<TextNode>(v)
        .map(Node::Text)
        .map_err(|e| e.to_string())
}

/// One rounded bar per word, laid out with the same metrics as the native box estimate.
fn placeholder_text(text: &str, font: &str, size: f64, fill: [f32; 4], bx: [f64; 2]) -> Node {
    let adv = advance(font, size);
    let mut layers = Vec::new();
    for (li, line) in text.split('\n').enumerate() {
        let top = li as f64 * LINE_HEIGHT * size;
        let mut col = 0usize;
        for word in line.split(' ') {
            let n = word.chars().count();
            if n > 0 {
                let mut l = Layer::new(
                    format!("w{li}.{col}"),
                    Node::Shape(ShapeNode {
                        width: (n as f64 * adv - 0.08 * size).max(1.0) as f32,
                        height: (0.62 * size) as f32,
                        corner_radius: (0.14 * size) as f32,
                        fill_srgb: fill,
                    }),
                );
                l.transform.position = [(col as f64 * adv) as f32, (top + 0.34 * size) as f32];
                layers.push(l);
            }
            col += n + 1;
        }
    }
    Node::Group(GroupNode {
        width: bx[0] as u32,
        height: bx[1] as u32,
        layers,
    })
}

/// Lowers one frame's nodes to an S4 scene.
pub fn lower(
    out: &FrameOutput,
    opts: &LowerOptions,
    provider: &dyn SourceProvider,
) -> Result<Scene, ComposeError> {
    let mut l = Lowerer {
        opts,
        provider,
        path: Vec::new(),
    };
    let identity: Affine = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let mut layers = Vec::with_capacity(out.nodes.len() + 1);
    if let Some(src) = &opts.under {
        let f = provider
            .frame(src, opts.under_frame)
            .map_err(|e| ComposeError::engine(format!("background media `{src}`: {e}")))?;
        let mut under = Layer::new(
            "under",
            Node::Media(MediaNode {
                source: src.clone(),
                frame: opts.under_frame,
                tags: None,
                tone_map: None,
            }),
        );
        under.transform.scale = [
            opts.width as f32 / f.width as f32,
            opts.height as f32 / f.height as f32,
        ];
        layers.push(under);
    }
    layers.extend(
        l.children(&out.nodes, &identity)?
            .into_iter()
            .map(|k| k.layer),
    );
    Ok(Scene {
        width: opts.width,
        height: opts.height,
        background: opts.background,
        layers,
        effects: Vec::new(),
        output: opts.output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::AssetSources;
    use crate::runtime::parse_frame;

    fn lower_json(json: &str, text: TextMode) -> Result<Scene, ComposeError> {
        let out = parse_frame(json)?;
        let opts = LowerOptions {
            width: 640,
            height: 360,
            text,
            ..LowerOptions::default()
        };
        let assets = AssetSources::new(640, 360).expect("assets");
        lower(&out, &opts, &assets)
    }

    #[test]
    fn colors_parse() {
        assert_eq!(parse_color("#fff"), Some([1.0; 4]));
        assert_eq!(
            parse_color("#00000080").map(|c| (c[3] * 255.0).round()),
            Some(128.0)
        );
        assert_eq!(
            parse_color("#7c5cff").map(|c| (c[0] * 255.0).round()),
            Some(124.0)
        );
        assert_eq!(parse_color("7c5cff"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#gggggg"), None);
    }

    #[test]
    fn group_is_sized_to_children_and_pivots_on_origin() {
        let s = lower_json(
            r##"{"nodes":[{"type":"group","x":100,"y":50,"children":[
                {"type":"rect","x":10,"y":20,"w":30,"h":40,"fill":"#ff0000"},
                {"type":"ellipse","x":-5,"y":0,"w":10,"h":10}]}]}"##,
            TextMode::Placeholder,
        )
        .expect("lower");
        let g = &s.layers[0];
        let Node::Group(gn) = &g.content else {
            panic!("group")
        };
        // Children span x ∈ [-5, 40], y ∈ [0, 60]; padded by 1 px.
        assert_eq!((gn.width, gn.height), (47, 62));
        // The group's local origin lands on (100, 50).
        let m = g
            .transform
            .forward([f64::from(gn.width), f64::from(gn.height)]);
        let local_origin_in_intermediate = [6.0, 1.0];
        let p = apply_affine(&m, local_origin_in_intermediate);
        assert!(
            (p[0] - 100.0).abs() < 1e-4 && (p[1] - 50.0).abs() < 1e-4,
            "{p:?}"
        );
        let Node::Shape(e) = &gn.layers[1].content else {
            panic!("shape")
        };
        assert_eq!(e.corner_radius, 5.0);
    }

    #[test]
    fn offscreen_group_is_culled_and_partially_visible_group_clipped() {
        let s = lower_json(
            r##"{"nodes":[
                {"type":"group","x":-5000,"children":[{"type":"rect","w":100,"h":100}]},
                {"type":"group","x":-1000,"children":[{"type":"rect","w":1100,"h":10}]}]}"##,
            TextMode::Placeholder,
        )
        .expect("lower");
        assert_eq!(s.layers.len(), 1, "fully off-canvas group culled");
        let Node::Group(g) = &s.layers[0].content else {
            panic!("group")
        };
        assert!(
            (100..=104).contains(&g.width),
            "clipped to the visible 100 px: {}",
            g.width
        );
    }

    #[test]
    fn scene_errors_carry_node_paths() {
        let e = lower_json(
            r#"{"nodes":[{"type":"group","children":[{"type":"rect","w":10,"h":null}]}]}"#,
            TextMode::Placeholder,
        )
        .expect_err("NaN");
        assert_eq!(e.kind, ErrorKind::Scene);
        assert_eq!(
            e.diagnostics[0].path.as_deref(),
            Some("nodes/0/children/0 (<rect>)")
        );
        let e = lower_json(r#"{"nodes":[{"type":"circle"}]}"#, TextMode::Placeholder)
            .expect_err("type");
        assert!(e.diagnostics[0].message.contains("unknown node type"));
        let e = lower_json(
            r#"{"nodes":[{"type":"image","src":"nope"}]}"#,
            TextMode::Placeholder,
        )
        .expect_err("asset");
        assert!(e.diagnostics[0].message.contains("asset `nope`"));
    }

    #[test]
    fn text_lowers_to_native_or_placeholder() {
        let json =
            r##"{"nodes":[{"type":"text","text":"Ada Lovelace","size":40,"fill":"#ffffff"}]}"##;
        let s = lower_json(json, TextMode::Native).expect("native");
        let Node::Text(t) = &s.layers[0].content else {
            panic!("text")
        };
        assert_eq!(t.text, "Ada Lovelace");
        assert_eq!(t.box_size, [260.0, 50.0]);
        let s = lower_json(json, TextMode::Placeholder).expect("placeholder");
        let Node::Group(g) = &s.layers[0].content else {
            panic!("group")
        };
        assert_eq!(g.layers.len(), 2, "one bar per word");
    }

    #[test]
    fn image_is_scaled_to_requested_size() {
        let s = lower_json(
            r#"{"nodes":[{"type":"image","src":"logo","w":64,"h":32}]}"#,
            TextMode::Placeholder,
        )
        .expect("lower");
        assert_eq!(s.layers[0].transform.scale, [0.25, 0.125]);
    }
}
