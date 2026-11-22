//! GPU integration tests. They need a wgpu adapter; on machines without one (no WARP, no
//! lavapipe) they print a notice and return early.
//!
//! * `S4_ADAPTER` selects the adapter for the single-adapter tests (default: software).
//! * `S4_UPDATE_GOLDENS=1` rewrites the golden set of every adapter present.

use std::path::PathBuf;

use s4::Error;
use s4::cpu_ref;
use s4::export::{export_png_sequence, frame_path};
use s4::golden;
use s4::gpu::{AdapterChoice, Renderer, list_adapters};
use s4::image::read_png;
use s4::metrics::compare;
use s4::scene::Transfer;
use s4::source::SyntheticSources;
use s4::timeline::{demo_scene, test_scenes};

/// Golden set used when an adapter has none of its own (the CI software adapter on Windows).
const FALLBACK_GOLDEN_SET: &str = "dx12-microsoft-basic-render-driver";

fn test_renderer() -> Option<Renderer> {
    let choice = AdapterChoice::from_env_or(AdapterChoice::Software);
    match Renderer::new(&choice).or_else(|_| Renderer::new(&AdapterChoice::Auto)) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("SKIP: no usable GPU adapter ({e})");
            None
        }
    }
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("s4-test-{}-{name}", std::process::id()))
}

#[test]
fn golden_frames_match_per_adapter_references() {
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
            let dir = golden::write_set(&mut r, &root).expect("write goldens");
            eprintln!("updated {}", dir.display());
            continue;
        }
        let fallback = if golden::available_sets(&root)
            .iter()
            .any(|s| s == FALLBACK_GOLDEN_SET)
        {
            FALLBACK_GOLDEN_SET.to_string()
        } else {
            golden::available_sets(&root)
                .into_iter()
                .next()
                .expect("at least one golden set")
        };
        for row in golden::check(&mut r, &root, &fallback).expect("golden check") {
            eprintln!(
                "{:<44} {:<12} vs {:<36} {:<5} PSNR {:>7.2} dB SSIM {:.6} bit-identical {}",
                a.key,
                row.scene,
                row.reference_set,
                if row.same_adapter { "same" } else { "cross" },
                row.cmp.psnr_db,
                row.cmp.ssim,
                row.cmp.bit_identical
            );
            if !row.cmp.passes_golden() {
                failures.push(format!("{} {}: {:?}", a.key, row.scene, row.cmp));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "§20 golden tolerance (SSIM ≥ 0.995, PSNR ≥ 40 dB) failed: {failures:#?}"
    );
}

#[test]
fn gpu_matches_independent_cpu_reference() {
    let Some(mut r) = test_renderer() else { return };
    let (w, h) = (480, 270);
    let provider = SyntheticSources::new(w, h).expect("sources");
    let mut failures = Vec::new();
    for (name, scene) in test_scenes(w, h) {
        let gpu = r.render_scene(&scene, &provider).expect("gpu").image;
        let reference = cpu_ref::render(&scene, &provider).expect("cpu");
        let c = compare(&gpu, &reference, scene.output.transfer);
        eprintln!(
            "{name:<12} PSNR {:>6.2} dB  SSIM {:.5}  ΔE2000 mean {:.4} p95 {:.4} max {:.4}  max|Δ| {}",
            c.psnr_db, c.ssim, c.delta_e.mean, c.delta_e.p95, c.delta_e.max, c.max_abs_diff
        );
        if !c.passes_reference() {
            failures.push(name);
        }
    }
    assert!(
        failures.is_empty(),
        "PSNR > 45 dB / ΔE < 2 failed for {failures:?}"
    );
}

#[test]
fn sixteen_bit_output_matches_reference() {
    let Some(mut r) = test_renderer() else { return };
    let provider = SyntheticSources::new(320, 180).expect("sources");
    let mut scene = demo_scene(320, 180, 150);
    scene.output.bit_depth = 16;
    scene.output.transfer = Transfer::Srgb;
    let gpu = r.render_scene(&scene, &provider).expect("gpu").image;
    let reference = cpu_ref::render(&scene, &provider).expect("cpu");
    assert_eq!(gpu.bit_depth, 16);
    let c = compare(&gpu, &reference, Transfer::Srgb);
    eprintln!(
        "16-bit sRGB: PSNR {:.2} dB, max|Δ| {}",
        c.psnr_db, c.max_abs_diff
    );
    assert!(c.passes_reference(), "{c:?}");
}

#[test]
fn same_adapter_frames_are_bit_identical_cold_and_warm() {
    // "Preview vs export on the same adapter: bit-identical" (§20): a frame rendered cold by a
    // fresh renderer must equal the same frame rendered mid-sequence with warm caches and
    // recycled textures.
    let Some(mut cold) = test_renderer() else {
        return;
    };
    let key = cold.adapter().key.clone();
    let provider = SyntheticSources::new(320, 180).expect("sources");
    let target = 150;
    let a = cold
        .render_scene(&demo_scene(320, 180, target), &provider)
        .expect("cold")
        .image;

    let mut warm = Renderer::new(&AdapterChoice::Key(key)).expect("second renderer");
    let mut last = None;
    for f in 140..=target {
        let frame = warm
            .render_scene(&demo_scene(320, 180, f), &provider)
            .expect("warm");
        if f > 140 {
            assert!(
                frame.stats.cache_hits >= 2,
                "static sources hit the cache: {:?}",
                frame.stats
            );
        }
        last = Some(frame.image);
    }
    let b = last.expect("rendered");
    assert_eq!(a.diff_count(&b), 0, "cold vs warm render differ");
    // And rendering is repeatable on one renderer.
    let c = warm
        .render_scene(&demo_scene(320, 180, target), &provider)
        .expect("again")
        .image;
    assert_eq!(a, c);
}

#[test]
fn png_sequence_export_round_trips() {
    let Some(mut r) = test_renderer() else { return };
    let provider = SyntheticSources::new(160, 90).expect("sources");
    let dir = scratch("export");
    let stats = export_png_sequence(&mut r, &provider, (160, 90), 118..123, &dir).expect("export");
    assert_eq!(stats.frames, 5);
    for f in 118..123 {
        let on_disk = read_png(&frame_path(&dir, f)).expect("png");
        let fresh = r
            .render_scene(&demo_scene(160, 90, f), &provider)
            .expect("render")
            .image;
        assert_eq!(on_disk, fresh, "frame {f}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn device_lost_is_reported_and_recovered() {
    let Some(mut r) = test_renderer() else { return };
    let provider = SyntheticSources::new(160, 90).expect("sources");
    let scene = demo_scene(160, 90, 30);
    let before = r.render_scene(&scene, &provider).expect("before").image;
    r.simulate_device_loss();
    match r.render_scene(&scene, &provider) {
        Err(Error::DeviceLost(_)) => {}
        other => panic!("expected DeviceLost, got {:?}", other.map(|f| f.stats)),
    }
    r.recover().expect("recover");
    let after = r.render_scene(&scene, &provider).expect("after").image;
    assert_eq!(
        before, after,
        "recovered device renders the same frame bit-identically"
    );
}

#[test]
fn timestamps_are_reported_when_supported() {
    let Some(mut r) = test_renderer() else { return };
    let provider = SyntheticSources::new(320, 180).expect("sources");
    let frame = r
        .render_scene(&demo_scene(320, 180, 150), &provider)
        .expect("render");
    if r.adapter().timestamps {
        let gpu = frame.stats.gpu_ns.expect("gpu time");
        assert!(gpu > 0);
        assert_eq!(frame.stats.pass_ns.len(), frame.stats.passes_run);
    }
}
