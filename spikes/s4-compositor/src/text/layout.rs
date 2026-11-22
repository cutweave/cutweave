//! Shaping and line layout with `cosmic-text` (harfrust shaping, bidi, line breaking, font
//! fallback across the bundled faces). Output positions are in box pixels, y down, pen at the
//! baseline. Layouts are cached by the engine: captions animate spans, not text.

use cosmic_text::{Align, Attrs, Buffer, Family, Metrics, Shaping, Weight, Wrap, fontdb};

use super::atlas::GlyphKey;
use super::fonts::{FontLibrary, parse_font_spec};
use crate::hash::Fnv;
use crate::scene::{TextAlign, TextNode};

/// One positioned glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaidGlyph {
    pub face: fontdb::ID,
    pub key: GlyphKey,
    /// Pen position on the baseline (box px, y down).
    pub pen: [f32; 2],
    pub size: f32,
    pub advance: f32,
    /// UTF-8 byte range of the cluster in the node text.
    pub range: [u32; 2],
    pub rtl: bool,
    pub line: u32,
}

/// One laid-out line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineBox {
    pub top: f32,
    pub baseline: f32,
    pub height: f32,
    pub width: f32,
}

/// Shaped and laid-out text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextLayout {
    pub glyphs: Vec<LaidGlyph>,
    pub lines: Vec<LineBox>,
}

/// Cache key of a layout (everything shaping and line breaking depend on).
#[must_use]
pub fn layout_key(node: &TextNode) -> u64 {
    let mut h = Fnv::new();
    h.str(&node.text)
        .str(&node.font)
        .f32(node.size_px)
        .f32s(&node.box_size)
        .u32(node.align as u32)
        .f32(node.line_height);
    h.key().0
}

/// Shapes and lays out `node` (spans and colours are ignored here).
pub fn layout(lib: &mut FontLibrary, node: &TextNode) -> TextLayout {
    let (family, weight) = parse_font_spec(&node.font);
    let family = lib.resolve_family(&family).to_string();
    let size = node.size_px.max(1.0);
    let mut buffer = Buffer::new(
        &mut lib.system,
        Metrics::new(size, size * node.line_height.max(0.1)),
    );
    buffer.set_wrap(Wrap::WordOrGlyph);
    buffer.set_size(Some(node.box_size[0].max(1.0)), None);
    let attrs = Attrs::new()
        .family(Family::Name(&family))
        .weight(Weight(weight));
    let align = match node.align {
        TextAlign::Left => Align::Left,
        TextAlign::Center => Align::Center,
        TextAlign::Right => Align::Right,
    };
    buffer.set_text(&node.text, &attrs, Shaping::Advanced, Some(align));
    buffer.shape_until_scroll(&mut lib.system, false);

    // Byte offset of each paragraph (cosmic-text splits on line endings).
    let mut para_start = vec![0u32];
    for (i, c) in node.text.char_indices() {
        if c == '\n' {
            para_start.push((i + 1) as u32);
        }
    }

    let mut out = TextLayout::default();
    for (line_no, run) in buffer.layout_runs().enumerate() {
        out.lines.push(LineBox {
            top: run.line_top,
            baseline: run.line_y,
            height: run.line_height,
            width: run.line_w,
        });
        let base = para_start.get(run.line_i).copied().unwrap_or(0);
        for g in run.glyphs {
            let Some(face) = lib.face(g.font_id) else {
                continue;
            };
            out.glyphs.push(LaidGlyph {
                face: g.font_id,
                key: GlyphKey {
                    face_hash: face.content_hash,
                    glyph: g.glyph_id,
                    weight: g.font_weight.0,
                },
                pen: [
                    g.x + g.font_size * g.x_offset,
                    run.line_y + g.y - g.font_size * g.y_offset,
                ],
                size: g.font_size,
                advance: g.w,
                range: [base + g.start as u32, base + g.end as u32],
                rtl: g.level.is_rtl(),
                line: line_no as u32,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(text: &str, font: &str) -> TextNode {
        TextNode::new(text, font, 64.0, [1.0; 4], [1600.0, 400.0])
    }

    #[test]
    fn shapes_latin_rtl_cjk_and_emoji_with_fallback() {
        let mut lib = FontLibrary::bundled().expect("fonts");
        let text = "Cut مرحبا 字幕 🎬";
        let l = layout(&mut lib, &node(text, "Bricolage Grotesque 800"));
        assert!(!l.glyphs.is_empty());
        let family_of = |needle: &str| {
            let start = text.find(needle).expect("needle") as u32;
            let g = l
                .glyphs
                .iter()
                .find(|g| g.range[0] == start)
                .unwrap_or_else(|| panic!("no glyph at {needle}"));
            (lib.face(g.face).expect("face").family.clone(), g.rtl)
        };
        assert_eq!(family_of("Cut"), ("Bricolage Grotesque".into(), false));
        let (fam, rtl) = family_of("م");
        assert_eq!(fam, "Noto Sans Arabic");
        assert!(rtl, "Arabic is right-to-left");
        assert_eq!(family_of("字").0, "Noto Sans SC");
        assert_eq!(family_of("🎬").0, "Noto Emoji");
        // Arabic is joined: fewer glyphs than code points is not required, but the shaper must
        // pick contextual forms, i.e. the initial م differs from an isolated one.
        let iso = layout(&mut lib, &node("م", "Bricolage Grotesque 800"));
        let start = text.find('م').expect("meem") as u32;
        let joined = l.glyphs.iter().find(|g| g.range[0] == start).expect("meem");
        assert_ne!(joined.key.glyph, iso.glyphs[0].key.glyph, "contextual form");
    }

    #[test]
    fn variable_weight_reaches_the_glyph_key_and_layout_is_deterministic() {
        let mut lib = FontLibrary::bundled().expect("fonts");
        let a = layout(&mut lib, &node("Pop", "Bricolage Grotesque 800"));
        let b = layout(&mut lib, &node("Pop", "Bricolage Grotesque 400"));
        assert_eq!(a.glyphs[0].key.weight, 800);
        assert_eq!(b.glyphs[0].key.weight, 400);
        assert!(a.glyphs[2].pen[0] > b.glyphs[2].pen[0], "bolder is wider");
        let again = layout(&mut lib, &node("Pop", "Bricolage Grotesque 800"));
        assert_eq!(a, again);
    }

    #[test]
    fn wraps_and_aligns_in_the_box() {
        let mut lib = FontLibrary::bundled().expect("fonts");
        let mut n = node("one two three four five six seven", "Instrument Sans 600");
        n.box_size = [300.0, 400.0];
        n.align = TextAlign::Center;
        let l = layout(&mut lib, &n);
        assert!(l.lines.len() >= 2);
        for g in &l.glyphs {
            assert!(g.pen[0] >= -1.0 && g.pen[0] + g.advance <= 301.0);
        }
    }
}
