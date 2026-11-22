// SPDX-License-Identifier: GPL-3.0-or-later
//! GPU path: lower-third frames 0/60/119 through the S4 compositor to PNG, and determinism of
//! the PNG bytes across independent runs. Uses the software adapter by default (WARP / lavapipe,
//! like S4's tests); `S4_ADAPTER` overrides.

use s4::gpu::AdapterChoice;
use s4b::lower::TextMode;
use s4b::render::{FrameRenderer, TextChoice};
use s4b::runtime::{BindOptions, Limits, load_source};
use s4b::stats::fnv64;
use s4b::{LOWER_THIRD_PROPS, LOWER_THIRD_TSX, STRESS_TSX};

const W: u32 = 960;
const H: u32 = 540;

fn adapter() -> AdapterChoice {
    AdapterChoice::from_env_or(AdapterChoice::Software)
}

/// Renders `frames` of a composition in a fresh runtime and a fresh GPU device.
fn render_frames(
    src: &str,
    props: &str,
    frames: &[f64],
) -> Vec<(Vec<u8>, s4::image::Image, TextMode)> {
    let comp = load_source("c.tsx", src, Limits::default()).expect("load");
    let opts = BindOptions {
        width: W,
        height: H,
        ..BindOptions::default()
    };
    let ev = comp.bind(props, &opts).expect("bind");
    let mut r = FrameRenderer::new(&adapter(), W, H, TextChoice::Auto).expect("renderer");
    // sRGB output so designer hex colours round-trip (S4's default is the BT.709 OETF).
    r.options.output.transfer = s4::scene::Transfer::Srgb;
    frames
        .iter()
        .map(|&f| {
            let out = ev.eval(f).expect("eval");
            let fr = r.render(&out, f as i64).expect("render");
            (fr.png, fr.image, fr.text)
        })
        .collect()
}

fn px(img: &s4::image::Image, x: u32, y: u32) -> [u16; 4] {
    img.pixel(x, y)
}

/// RGB within ±1 code value (Rgba16Float intermediates).
fn near(p: [u16; 4], rgb: [u16; 3]) -> bool {
    (0..3).all(|c| p[c].abs_diff(rgb[c]) <= 1)
}

#[test]
fn lower_third_renders_frames_0_60_119() {
    let frames = render_frames(LOWER_THIRD_TSX, LOWER_THIRD_PROPS, &[0.0, 60.0, 119.0]);
    let dir = std::env::temp_dir().join("s4b-render-test");
    std::fs::create_dir_all(&dir).expect("tmp");
    for ((png, img, _), f) in frames.iter().zip([0, 60, 119]) {
        assert_eq!((img.width, img.height), (W, H));
        assert!(png.starts_with(b"\x89PNG"), "frame {f} is a PNG");
        std::fs::write(dir.join(format!("lower_third_f{f:04}.png")), png).expect("write");
    }
    let bg = px(&frames[1].1, 5, 5);
    // Frame 60: settled at x = 80, y = H - 220; the accent card (#7c5cff) is opaque at its
    // centre-right (text placeholders/glyphs are on the left half).
    let card = px(&frames[1].1, 80 + 480, H - 220 + 55);
    assert!(
        near(card, [124, 92, 255]),
        "accent card at frame 60: {card:?}"
    );
    // Frame 0: x = -400, so only the card's right 120 px are on screen.
    let edge = px(&frames[0].1, 60, H - 220 + 55);
    assert!(
        near(edge, [124, 92, 255]),
        "card edge visible at frame 0: {edge:?}"
    );
    assert_eq!(
        px(&frames[0].1, 200, H - 220 + 55),
        bg,
        "rest of frame 0 is background"
    );
    // Frame 119: opacity 0.05 → close to the background.
    let faded = px(&frames[2].1, 80 + 480, H - 220 + 55);
    for c in 0..3 {
        assert!(
            faded[c].abs_diff(bg[c]) < 40,
            "faded {faded:?} vs bg {bg:?}"
        );
    }
    assert!(
        !near(faded, [124, 92, 255]),
        "faded differs from the opaque card"
    );
    // Text (glyphs or placeholder bars) is white-ish over the card at frame 60.
    let name_row = H - 220 + 18 + 30;
    let brightest = (80 + 24..80 + 200)
        .map(|x| px(&frames[1].1, x, name_row)[1])
        .max()
        .expect("row");
    assert!(brightest > 200, "name text visible ({brightest})");
}

#[test]
fn png_bytes_are_identical_across_three_independent_runs() {
    let frames = [0.0, 60.0, 119.0];
    let runs: Vec<Vec<u64>> = (0..3)
        .map(|_| {
            render_frames(LOWER_THIRD_TSX, LOWER_THIRD_PROPS, &frames)
                .iter()
                .map(|(png, _, _)| fnv64(png))
                .collect()
        })
        .collect();
    assert_eq!(runs[0], runs[1]);
    assert_eq!(runs[0], runs[2]);
}

#[test]
fn stress_scene_renders() {
    let frames = render_frames(STRESS_TSX, "{}", &[120.0]);
    let (_, img, _) = &frames[0];
    // Background rect #0b1020 covers the canvas.
    assert!(
        near(px(img, W / 2, H - 4), [11, 16, 32]),
        "{:?}",
        px(img, W / 2, H - 4)
    );
}
