//! A hard-coded "timeline evaluation": frame number → [`Scene`]. Deterministic (no clock, no
//! randomness), so `render(scene(t))` is a pure function of `t` (§8).

use crate::scene::{
    BlendMode, Effect, FLICKS_PER_SECOND, Flicks, GroupNode, Layer, MediaNode, Node,
    OutputTransform, Scene, ShapeNode, TextNode, Transform, TransitionKind, TransitionNode,
};

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
            Node::Text(TextNode {
                text: "Cutweave".into(),
                font: "Inter 700".into(),
                size_px: 42.0,
                fill_srgb: [1.0, 1.0, 1.0, 1.0],
                box_size: [520.0, 110.0],
            }),
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
