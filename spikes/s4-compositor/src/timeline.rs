//! A hard-coded "timeline evaluation": frame number → [`Scene`]. Deterministic (no clock, no
//! randomness), so `render(scene(t))` is a pure function of `t` (§8).

use crate::scene::{
    BlendMode, Effect, FLICKS_PER_SECOND, Flicks, GroupNode, Layer, MediaNode, Node,
    OutputTransform, Scene, ShapeNode, TextAlign, TextHighlight, TextNode, TextOutline,
    TextRenderer, TextShadow, TextSpan, Transform, TransitionKind, TransitionNode,
};
use crate::text::caption::{glyph_storm, pop_karaoke, storm_text, word_timings};

/// Sequence frame rate.
pub const FPS: i64 = 60;
/// Demo length in frames (5 s at 60 fps).
pub const DEMO_FRAMES: i64 = 300;
/// First frame of the cross-dissolve on the picture-in-picture track.
pub const DISSOLVE_START: i64 = 120;
/// Dissolve length in frames.
pub const DISSOLVE_FRAMES: i64 = 60;

/// Sequence frame → flicks.
#[must_use]
pub fn frame_to_flicks(frame: i64) -> Flicks {
    frame * FLICKS_PER_SECOND / FPS
}

fn media(source: &str, frame: i64) -> Node {
    Node::Media(MediaNode {
        source: source.into(),
        frame,
        tags: None,
        tone_map: None,
    })
}

/// The demo sequence (3 layers, 2 blend modes, a dissolve, blur-lite + LUT):
///
/// 1. `bg` — `camera709` (BT.709 limited Y'CbCr), full frame, **blur-lite**, normal.
/// 2. `pip` — picture-in-picture: `hdr10` (PQ, tone-mapped) **cross-dissolving** to `phone601`
///    (BT.601 full range) over frames 120–180; rotating slowly; normal at 95% opacity.
/// 3. `chart` — colour checker card drifting right, **multiply** at 85%.
///
/// Sequence-level: the `warm_contrast` **`.cube` LUT** in BT.709 encoding; BT.709 8-bit output.
#[must_use]
pub fn demo_scene(width: u32, height: u32, frame: i64) -> Scene {
    let (wf, hf) = (width as f32, height as f32);
    let k = wf / 1920.0;
    let t = frame as f32 / (DEMO_FRAMES - 1) as f32;

    let mut bg = Layer::new("bg", media("camera709", frame));
    bg.effects.push(Effect::BlurLite { radius: 3.0 * k });

    let pip_transform = Transform {
        position: [0.72 * wf, 0.32 * hf],
        anchor: [0.5, 0.5],
        scale: [0.8, 0.8],
        rotation_deg: -4.0 + 8.0 * t,
    };
    let pip_side = |id: &str, source: &str| Layer {
        transform: pip_transform,
        ..Layer::new(id, media(source, frame))
    };
    let pip = if frame < DISSOLVE_START {
        Layer {
            opacity: 0.95,
            ..pip_side("pip", "hdr10")
        }
    } else if frame >= DISSOLVE_START + DISSOLVE_FRAMES {
        Layer {
            opacity: 0.95,
            ..pip_side("pip", "phone601")
        }
    } else {
        let progress = (frame - DISSOLVE_START) as f32 / DISSOLVE_FRAMES as f32;
        Layer {
            opacity: 0.95,
            ..Layer::new(
                "pip",
                Node::Transition(TransitionNode {
                    kind: TransitionKind::CrossDissolve,
                    progress,
                    from: Box::new(pip_side("pip.from", "hdr10")),
                    to: Box::new(pip_side("pip.to", "phone601")),
                }),
            )
        }
    };

    let chart = Layer {
        transform: Transform {
            position: [(0.04 + 0.10 * t) * wf, 0.55 * hf],
            ..Transform::default()
        },
        opacity: 0.85,
        blend: BlendMode::Multiply,
        ..Layer::new("chart", media("chart", frame))
    };

    Scene {
        width,
        height,
        background: [0.0, 0.0, 0.0, 1.0],
        layers: vec![bg, pip, chart],
        effects: vec![Effect::Lut3d {
            lut: "warm_contrast".into(),
            space: crate::scene::Transfer::Bt709,
            mix: 1.0,
        }],
        output: OutputTransform::default(),
    }
}

fn shape(w: f32, h: f32, r: f32, fill_srgb: [f32; 4]) -> Node {
    Node::Shape(ShapeNode {
        width: w,
        height: h,
        corner_radius: r,
        fill_srgb,
    })
}

/// Named scenes for the reference comparison and golden tests.
#[must_use]
pub fn test_scenes(width: u32, height: u32) -> Vec<(String, Scene)> {
    let (wf, hf) = (width as f32, height as f32);
    let base = |layers: Vec<Layer>, effects: Vec<Effect>| Scene {
        width,
        height,
        background: [0.0, 0.0, 0.0, 1.0],
        layers,
        effects,
        output: OutputTransform::default(),
    };

    // Input → output round trip of a BT.709 source (should be ~identity).
    let identity = base(vec![Layer::new("cam", media("camera709", 0))], vec![]);

    // PQ → SDR tone-map, scaled ×2 (the HDR source is half canvas size).
    let hdr = base(
        vec![Layer {
            transform: Transform {
                scale: [2.0, 2.0],
                ..Transform::default()
            },
            ..Layer::new("hdr", media("hdr10", 0))
        }],
        vec![],
    );

    // All eight core blend modes over the bars, plus a rotated group with its own opacity.
    let mut layers = vec![Layer::new("cam", media("camera709", 0))];
    let cell = wf / 9.0;
    for (i, mode) in BlendMode::ALL.iter().enumerate() {
        layers.push(Layer {
            transform: Transform {
                position: [(i as f32 + 0.75) * cell, 0.15 * hf],
                ..Transform::default()
            },
            opacity: 0.9,
            blend: *mode,
            ..Layer::new(
                format!("blend.{mode:?}"),
                shape(cell * 0.8, 0.5 * hf, cell * 0.1, [0.9, 0.45, 0.2, 0.85]),
            )
        });
    }
    let (gw, gh) = ((wf / 3.0).round() as u32, (hf / 4.0).round() as u32);
    layers.push(Layer {
        transform: Transform {
            position: [0.75 * wf, 0.8 * hf],
            anchor: [0.5, 0.5],
            rotation_deg: 10.0,
            ..Transform::default()
        },
        opacity: 0.7,
        ..Layer::new(
            "group",
            Node::Group(GroupNode {
                width: gw,
                height: gh,
                layers: vec![
                    Layer::new(
                        "g.a",
                        shape(gw as f32 * 0.6, gh as f32, 12.0, [0.1, 0.3, 0.9, 1.0]),
                    ),
                    Layer {
                        transform: Transform {
                            position: [gw as f32 * 0.4, 0.0],
                            ..Transform::default()
                        },
                        blend: BlendMode::Screen,
                        ..Layer::new(
                            "g.b",
                            shape(gw as f32 * 0.6, gh as f32, 12.0, [0.9, 0.2, 0.1, 1.0]),
                        )
                    },
                ],
            }),
        )
    });
    let blend_modes = base(layers, vec![]);

    // Layer-level LUT on the chart over an 18% grey card.
    let lut_chart = base(
        vec![
            Layer::new("grey", shape(wf, hf, 0.0, [0.46, 0.46, 0.46, 1.0])),
            Layer {
                transform: Transform {
                    position: [0.5 * wf, 0.5 * hf],
                    anchor: [0.5, 0.5],
                    scale: [1.5, 1.5],
                    ..Transform::default()
                },
                effects: vec![Effect::Lut3d {
                    lut: "warm_contrast".into(),
                    space: crate::scene::Transfer::Bt709,
                    mix: 1.0,
                }],
                ..Layer::new("chart", media("chart", 0))
            },
        ],
        vec![],
    );

    vec![
        ("demo_f0000".into(), demo_scene(width, height, 0)),
        ("demo_f0150".into(), demo_scene(width, height, 150)),
        ("demo_f0299".into(), demo_scene(width, height, 299)),
        ("identity709".into(), identity),
        ("hdr_tonemap".into(), hdr),
        ("blend_modes".into(), blend_modes),
        ("lut_chart".into(), lut_chart),
    ]
}

/// The animated caption demo's text (10 words).
pub const CAPTION_TEXT: &str = "Captions that pop, word by word, right on the beat";
/// Latin + RTL (Arabic) + CJK + emoji sample.
pub const MULTISCRIPT_TEXT: &str = "Cutweave مرحبا بالعالم 字幕很好看 🎬✨🔥";
/// Karaoke colour (the design system's marigold, sRGB).
pub const MARIGOLD: [f32; 4] = [1.0, 0.784, 0.239, 1.0];
/// First word's start frame and word spacing (frames) of the caption demo.
pub const CAPTION_FIRST: i64 = 10;
pub const CAPTION_STEP: i64 = 18;

fn caption_style(t: &mut TextNode, k: f32) {
    t.align = TextAlign::Center;
    t.line_height = 1.15;
    t.outline = Some(TextOutline {
        width_px: 6.0 * k,
        color_srgb: [0.05, 0.04, 0.08, 1.0],
    });
    t.shadow = Some(TextShadow {
        offset_px: [0.0, 5.0 * k],
        blur_px: 6.0 * k,
        color_srgb: [0.0, 0.0, 0.0, 0.55],
    });
}

/// The caption layer of the demo at `frame`: words pop in, the active word gets a karaoke
/// sweep (`text::caption::pop_karaoke`), white with a dark outline and a soft drop shadow.
#[must_use]
pub fn caption_layer(width: u32, height: u32, frame: i64, renderer: TextRenderer) -> Layer {
    let k = height as f32 / 1080.0;
    let size = 72.0 * k;
    let mut t = TextNode::new(
        CAPTION_TEXT,
        "Bricolage Grotesque 800",
        size,
        [1.0, 1.0, 1.0, 1.0],
        [1600.0 * k, 220.0 * k],
    );
    caption_style(&mut t, k);
    let words = word_timings(CAPTION_TEXT, CAPTION_FIRST, CAPTION_STEP);
    t.spans = pop_karaoke(&words, frame, size, MARIGOLD);
    t.renderer = renderer;
    Layer {
        transform: Transform {
            position: [(width as f32 * 0.5).round(), (height as f32 * 0.80).round()],
            anchor: [0.5, 0.5],
            ..Transform::default()
        },
        ..Layer::new("caption", Node::Text(t))
    }
}

/// The 3-layer demo with the animated caption on top (the §19 "3 video layers + 2 effects +
/// animated captions" budget scene).
#[must_use]
pub fn caption_scene(width: u32, height: u32, frame: i64, renderer: TextRenderer) -> Scene {
    let mut s = demo_scene(width, height, frame);
    s.layers.push(caption_layer(width, height, frame, renderer));
    s
}

fn flat_scene(width: u32, height: u32, srgb: [f32; 3]) -> Scene {
    let c = crate::color::srgb_fill_to_linear_premul([srgb[0], srgb[1], srgb[2], 1.0]);
    Scene {
        width,
        height,
        background: c,
        layers: Vec::new(),
        effects: Vec::new(),
        output: OutputTransform::default(),
    }
}

/// Latin + Arabic + CJK + emoji with the caption style, over a flat background.
#[must_use]
pub fn multiscript_scene(width: u32, height: u32, renderer: TextRenderer) -> Scene {
    let k = height as f32 / 1080.0;
    let mut s = flat_scene(width, height, [0.16, 0.18, 0.24]);
    let mut t = TextNode::new(
        MULTISCRIPT_TEXT,
        "Bricolage Grotesque 700",
        84.0 * k,
        [1.0, 1.0, 1.0, 1.0],
        [1800.0 * k, 360.0 * k],
    );
    caption_style(&mut t, k);
    // Karaoke across the Arabic phrase (right-to-left sweep) and a pop on the CJK word.
    let ar = MULTISCRIPT_TEXT.find('م').unwrap_or(0) as u32;
    let ar_end = MULTISCRIPT_TEXT.find(" 字").unwrap_or(0) as u32;
    let mut arabic = TextSpan::new([ar, ar_end]);
    arabic.highlight = Some(TextHighlight {
        color_srgb: MARIGOLD,
        progress: 0.6,
    });
    let cjk = MULTISCRIPT_TEXT.find('字').unwrap_or(0) as u32;
    let mut pop = TextSpan::new([cjk, cjk + "字幕很好看".len() as u32]);
    pop.scale = [1.12, 1.12];
    pop.rotation_deg = -3.0;
    t.spans = vec![arabic, pop];
    t.renderer = renderer;
    s.layers.push(Layer {
        transform: Transform {
            position: [(width as f32 * 0.5).round(), (height as f32 * 0.5).round()],
            anchor: [0.5, 0.5],
            ..Transform::default()
        },
        ..Layer::new("multiscript", Node::Text(t))
    });
    s
}

/// `n` animated glyphs (per-glyph transform, colour and opacity) at 1080p-relative size, over a
/// flat background. `styled` adds the caption outline + shadow (3 quads per glyph for MSDF).
#[must_use]
pub fn glyph_storm_scene(
    width: u32,
    height: u32,
    frame: i64,
    n: usize,
    renderer: TextRenderer,
    styled: bool,
) -> Scene {
    let k = height as f32 / 1080.0;
    let mut s = flat_scene(width, height, [0.08, 0.09, 0.12]);
    let text = storm_text(n);
    let mut t = TextNode::new(
        text.clone(),
        "Bricolage Grotesque 700",
        34.0 * k,
        [1.0; 4],
        [width as f32 - 80.0 * k, height as f32 - 60.0 * k],
    );
    if styled {
        t.outline = Some(TextOutline {
            width_px: 3.0 * k,
            color_srgb: [0.05, 0.04, 0.08, 1.0],
        });
        t.shadow = Some(TextShadow {
            offset_px: [0.0, 3.0 * k],
            blur_px: 3.0 * k,
            color_srgb: [0.0, 0.0, 0.0, 0.55],
        });
    }
    t.spans = glyph_storm(&text, frame);
    t.renderer = renderer;
    s.layers.push(Layer {
        transform: Transform {
            position: [40.0 * k, 30.0 * k],
            ..Transform::default()
        },
        ..Layer::new("storm", Node::Text(t))
    });
    s
}

/// Fill-only text (no outline / shadow, so the CPU reference can render it) at `size_px`,
/// white on dark grey: the crispness scene. At 3840×2160 with 200 px this is the WBS 4K scene.
#[must_use]
pub fn crisp_scene(
    width: u32,
    height: u32,
    size_px: f32,
    offset_px: [f32; 2],
    renderer: TextRenderer,
) -> Scene {
    let mut s = flat_scene(width, height, [0.12, 0.12, 0.14]);
    let mut t = TextNode::new(
        "Ag@% Cutweave\n字幕 مرحبا 🎬",
        "Bricolage Grotesque 800",
        size_px,
        [1.0; 4],
        [width as f32 * 0.94, size_px * 2.6],
    );
    t.line_height = 1.25;
    t.renderer = renderer;
    s.layers.push(Layer {
        transform: Transform {
            position: [
                width as f32 * 0.03 + offset_px[0],
                (height as f32 - size_px * 2.6) * 0.5 + offset_px[1],
            ],
            ..Transform::default()
        },
        ..Layer::new("crisp", Node::Text(t))
    });
    s
}

/// A text line moving horizontally by `offset_px` (sub-pixel motion). `resampled` renders it
/// the generic way instead (at intrinsic size inside a group, then bilinear-composited with the
/// fractional offset) to show why text is rasterised in canvas space.
#[must_use]
pub fn shimmer_scene(
    width: u32,
    height: u32,
    offset_px: f32,
    renderer: TextRenderer,
    resampled: bool,
) -> Scene {
    let k = height as f32 / 1080.0;
    let mut s = flat_scene(width, height, [0.0, 0.0, 0.0]);
    let size = 40.0 * k;
    let mut t = TextNode::new(
        "Hamburgefonstiv minimum 0123 Il1| sub-pixel motion",
        "Instrument Sans 500",
        size,
        [1.0; 4],
        [width as f32 - 200.0 * k, size * 1.6],
    );
    t.renderer = renderer;
    let box_size = t.box_size;
    let pos = [100.0 * k + offset_px, (height as f32 * 0.5).round()];
    let layer = if resampled {
        Layer {
            transform: Transform {
                position: pos,
                ..Transform::default()
            },
            ..Layer::new(
                "shimmer",
                Node::Group(GroupNode {
                    width: box_size[0].ceil() as u32,
                    height: box_size[1].ceil() as u32,
                    layers: vec![Layer::new("shimmer.text", Node::Text(t))],
                }),
            )
        }
    } else {
        Layer {
            transform: Transform {
                position: pos,
                ..Transform::default()
            },
            ..Layer::new("shimmer", Node::Text(t))
        }
    };
    s.layers.push(layer);
    s
}

/// Named text scenes rendered into the per-adapter golden sets (both renderers).
#[must_use]
pub fn text_golden_scenes(width: u32, height: u32) -> Vec<(String, Scene)> {
    let mut v = Vec::new();
    for (tag, r) in [("msdf", TextRenderer::Msdf), ("vello", TextRenderer::Vello)] {
        v.push((
            format!("text_caption_{tag}_f0064"),
            caption_scene(width, height, 64, r),
        ));
        v.push((
            format!("text_multiscript_{tag}"),
            multiscript_scene(width, height, r),
        ));
    }
    v
}

/// A scene with a text layer: proves the node plugs into the graph (WP-0F.02 renders it).
#[must_use]
pub fn text_scene(width: u32, height: u32) -> Scene {
    let mut s = demo_scene(width, height, 0);
    s.layers.push(Layer {
        transform: Transform {
            position: [80.0, height as f32 - 220.0],
            ..Transform::default()
        },
        ..Layer::new(
            "caption",
            Node::Text(TextNode::new(
                "Cutweave",
                "Inter 700",
                42.0,
                [1.0, 1.0, 1.0, 1.0],
                [520.0, 110.0],
            )),
        )
    });
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flicks_conversion() {
        assert_eq!(frame_to_flicks(60), FLICKS_PER_SECOND);
        assert_eq!(frame_to_flicks(1), 11_760_000);
    }

    #[test]
    fn demo_is_deterministic_and_has_dissolve_window() {
        assert_eq!(demo_scene(320, 180, 42), demo_scene(320, 180, 42));
        let is_transition = |f| {
            matches!(
                demo_scene(320, 180, f).layers[1].content,
                Node::Transition(_)
            )
        };
        assert!(!is_transition(119));
        assert!(is_transition(120));
        assert!(is_transition(179));
        assert!(!is_transition(180));
    }
}
