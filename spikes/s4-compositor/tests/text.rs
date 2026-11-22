//! WP-0F.02 text tests on the GPU: per-adapter goldens for both renderers, same-adapter
//! determinism (cold vs warm, repeat), crispness vs the CPU outline reference, the 1000-glyph
//! scene and sub-pixel motion. Like `tests/gpu.rs`, they skip when no adapter exists.
//!
//! * `S4_ADAPTER` selects the adapter for the single-adapter tests (default: software). Vello is
//!   unsupported on software adapters (it crashes WARP), so its single-adapter tests use the
//!   best hardware adapter instead, and skip when there is none.
//! * `S4_UPDATE_GOLDENS=1` rewrites the text golden set of every adapter present.

use s4::golden;
use s4::gpu::{AdapterChoice, Renderer, list_adapters};
use s4::image::Image;
use s4::scene::{Node, TextRenderer};
use s4::source::SyntheticSources;
use s4::text_bench;
use s4::timeline::{caption_scene, glyph_storm_scene, text_scene};

const FALLBACK_GOLDEN_SET: &str = "dx12-microsoft-basic-render-driver";
const BOTH: [TextRenderer; 2] = [TextRenderer::Msdf, TextRenderer::Vello];

/// Finding (WP-0F.02): WARP devices used concurrently from several test threads crash the
/// process (STATUS_ACCESS_VIOLATION) even with pipeline creation serialised, so the text tests
/// run one at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The adapter for `rend`'s single-adapter tests.
fn renderer_for(rend: TextRenderer) -> Option<Renderer> {
    let choice = AdapterChoice::from_env_or(AdapterChoice::Software);
    let r = Renderer::new(&choice)
        .or_else(|_| Renderer::new(&AdapterChoice::Auto))
        .ok();
    match (rend, r) {
        (TextRenderer::Vello, Some(r)) if r.adapter().software => {
            match Renderer::new(&AdapterChoice::Auto) {
                Ok(hw) if !hw.adapter().software => Some(hw),
                _ => {
                    eprintln!("SKIP {rend:?}: no hardware adapter");
                    None
                }
            }
        }
        (_, None) => {
            eprintln!("SKIP {rend:?}: no usable GPU adapter");
            None
        }
        (_, r) => r,
    }
}

#[test]
fn text_golden_frames_match_per_adapter_references() {
    let _gpu = serial();
    let adapters = list_adapters();
    if adapters.is_empty() {
        eprintln!("SKIP: no adapters");
        return;
    }
    let root = golden::default_root();
    let update = std::env::var("S4_UPDATE_GOLDENS").is_ok_and(|v| v == "1");
    let mut failures = Vec::new();
    for a in adapters {
        let mut r = match Renderer::new(&AdapterChoice::Key(a.key.clone())) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skip {}: {e}", a.key);
                continue;
            }
        };
        if update {
            let dir = golden::write_text_set(&mut r, &root).expect("write text goldens");
            eprintln!("updated {}", dir.display());
            continue;
        }
        for row in golden::check_text(&mut r, &root, FALLBACK_GOLDEN_SET).expect("text golden") {
            eprintln!(
                "{:<44} {:<28} vs {:<36} {:<5} PSNR {:>7.2} dB SSIM {:.6} bit-identical {}",
                a.key,
                row.scene,
                row.reference_set,
                if row.same_adapter { "same" } else { "cross" },
                row.cmp.psnr_db,
                row.cmp.ssim,
                row.cmp.bit_identical
            );
            // Same adapter: bit-identical (§20 preview == export); cross-adapter: §20 tolerance.
            let ok = if row.same_adapter {
                row.cmp.bit_identical
            } else {
                row.cmp.passes_golden()
            };
            if !ok {
                failures.push(format!("{} {}: {:?}", a.key, row.scene, row.cmp));
            }
        }
    }
    assert!(failures.is_empty(), "text goldens failed: {failures:#?}");
}

#[test]
fn text_is_bit_identical_cold_warm_and_repeated_on_one_adapter() {
    let _gpu = serial();
    let (w, h) = (480, 270);
    let provider = SyntheticSources::new(w, h).expect("sources");
    let target = 70;
    for rend in BOTH {
        let Some(mut cold) = renderer_for(rend) else {
            continue;
        };
        let key = cold.adapter().key.clone();
        let a = cold
            .render_scene(&caption_scene(w, h, target, rend), &provider)
            .expect("cold")
            .image;
        let mut warm = Renderer::new(&AdapterChoice::Key(key.clone())).expect("warm renderer");
        let mut last: Option<Image> = None;
        for f in 40..=target {
            last = Some(
                warm.render_scene(&caption_scene(w, h, f, rend), &provider)
                    .expect("warm")
                    .image,
            );
        }
        let b = last.expect("rendered");
        assert_eq!(
            a.diff_count(&b),
            0,
            "{rend:?} on {key}: cold vs warm differ"
        );
        for _ in 0..3 {
            let c = warm
                .render_scene(&caption_scene(w, h, target, rend), &provider)
                .expect("repeat")
                .image;
            assert_eq!(
                a.diff_count(&c),
                0,
                "{rend:?} on {key}: repeated render differs"
            );
        }
        eprintln!("{rend:?} on {key}: cold == warm == repeated");
    }
}

#[test]
fn text_layers_render_on_both_paths_and_animate() {
    let _gpu = serial();
    let provider = SyntheticSources::new(320, 180).expect("sources");
    for rend in BOTH {
        let Some(mut r) = renderer_for(rend) else {
            continue;
        };
        let mut bare = text_scene(320, 180);
        bare.layers.truncate(3);
        let without = r.render_scene(&bare, &provider).expect("no text").image;
        let mut s = text_scene(320, 180);
        if let Node::Text(t) = &mut s.layers[3].content {
            t.renderer = rend;
        }
        let with = r.render_scene(&s, &provider).expect("text").image;
        assert!(with.diff_count(&without) > 100, "{rend:?} drew glyphs");
        // The caption changes between frames (pop + karaoke) and the text pass reports work.
        let f1 = r
            .render_scene(&caption_scene(320, 180, 30, rend), &provider)
            .expect("f30");
        let f2 = r
            .render_scene(&caption_scene(320, 180, 31, rend), &provider)
            .expect("f31");
        assert!(f1.image.diff_count(&f2.image) > 0);
        assert!(f2.stats.text.glyphs > 0);
        if r.adapter().timestamps {
            assert!(f2.stats.text_gpu_ns.is_some());
        }
    }
}

#[test]
fn vello_is_reported_unsupported_on_software_adapters() {
    let _gpu = serial();
    let Ok(mut r) = Renderer::new(&AdapterChoice::Software) else {
        eprintln!("SKIP: no software adapter");
        return;
    };
    if std::env::var("S4_VELLO_CPU").is_ok() {
        return;
    }
    let provider = SyntheticSources::new(320, 180).expect("sources");
    assert!(matches!(
        r.render_scene(&caption_scene(320, 180, 30, TextRenderer::Vello), &provider),
        Err(s4::Error::Unsupported(_))
    ));
}

#[test]
fn thousand_glyph_storm_builds_one_instance_per_glyph_and_layer() {
    let _gpu = serial();
    let Some(mut r) = renderer_for(TextRenderer::Msdf) else {
        return;
    };
    let provider = SyntheticSources::new(16, 16).expect("sources");
    let s = glyph_storm_scene(960, 540, 5, 1000, TextRenderer::Msdf, true);
    let f = r.render_scene(&s, &provider).expect("storm");
    assert_eq!(
        f.stats.text.instances, 3000,
        "shadow + outline + fill per glyph"
    );
    let b = text_bench::glyph_bench(&mut r, (960, 540), 1000, 5, TextRenderer::Msdf, false)
        .expect("bench");
    assert_eq!(b.instances, 1000);
    eprintln!(
        "{}: build p50 {:.3} ms, animator {:.3} ms, text GPU p50 {:.3} ms",
        r.adapter().name,
        b.build_ms_p50,
        b.animate_ms_p50,
        b.text_gpu_p50
    );
}

#[test]
fn text_is_crisp_against_the_supersampled_outline_reference() {
    // WBS criterion: SSIM ≥ 0.98 on the 4K scene with 200 px glyphs for the chosen path.
    let _gpu = serial();
    let (dims, size) = ((3840, 2160), 200.0);
    let reference = text_bench::crisp_reference(dims, size).expect("reference");
    for rend in BOTH {
        let Some(mut r) = renderer_for(rend) else {
            continue;
        };
        let (c, _) =
            text_bench::crispness(&mut r, dims, size, rend, &reference, None).expect("crispness");
        eprintln!(
            "{rend:?} on {}: SSIM text {:.5} full {:.5} PSNR text {:.2} dB max|Δ| {}",
            r.adapter().key,
            c.ssim_text,
            c.ssim_full,
            c.psnr_text,
            c.max_abs
        );
        assert!(c.ssim_text >= 0.98, "{rend:?}: {c:?}");
    }
}

#[test]
fn canvas_space_text_tracks_sub_pixel_motion() {
    let _gpu = serial();
    let dims = (1920, 1080);
    let refs = text_bench::shimmer_references(dims).expect("refs");
    for rend in BOTH {
        let Some(mut r) = renderer_for(rend) else {
            continue;
        };
        let resampled =
            text_bench::shimmer(Some(&mut r), dims, TextRenderer::Msdf, true, &refs).expect("rs");
        let s = text_bench::shimmer(Some(&mut r), dims, rend, false, &refs).expect("shimmer");
        eprintln!("{rend:?}: {s:?}\nresampled: {resampled:?}");
        // Stable per-phase quality (no sharpness pumping) and position tracking.
        assert!(s.psnr_mean > 45.0, "{rend:?}: {s:?}");
        assert!(s.psnr_spread < 3.0, "{rend:?}: {s:?}");
        assert!(s.centroid_err < 0.25, "{rend:?}: {s:?}");
        assert!(s.ink_range < 0.005, "{rend:?}: {s:?}");
        assert!(
            s.psnr_spread < resampled.psnr_spread,
            "canvas space beats resampling"
        );
    }
}
