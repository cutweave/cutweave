//! Per-adapter golden references (§8 "golden references are stored per GPU adapter class",
//! §20 tolerances). Layout: `tests/golden/<adapter-key>/<scene>.png`.

use std::path::{Path, PathBuf};

use crate::Result;
use crate::gpu::Renderer;
use crate::image::{Image, cicp_for, read_png, write_png};
use crate::metrics::{Comparison, compare};
use crate::scene::Transfer;
use crate::source::SyntheticSources;
use crate::timeline::test_scenes;

/// Golden frame size (small on purpose: references live in git without LFS for now).
pub const GOLDEN_SIZE: (u32, u32) = (384, 216);
/// Scenes rendered into every golden set.
pub const GOLDEN_SCENES: [&str; 4] = ["demo_f0000", "demo_f0150", "demo_f0299", "blend_modes"];

/// Default golden root inside the crate.
#[must_use]
pub fn default_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

/// Renders the golden scenes on `renderer`.
pub fn render_set(renderer: &mut Renderer) -> Result<Vec<(String, Image)>> {
    let (w, h) = GOLDEN_SIZE;
    let provider = SyntheticSources::new(w, h)?;
    let mut out = Vec::new();
    for (name, scene) in test_scenes(w, h) {
        if GOLDEN_SCENES.contains(&name.as_str()) {
            let frame = renderer.render_scene(&scene, &provider)?;
            out.push((name, frame.image));
        }
    }
    Ok(out)
}

/// Writes the golden set for `renderer`'s adapter; returns the directory.
pub fn write_set(renderer: &mut Renderer, root: &Path) -> Result<PathBuf> {
    let dir = root.join(&renderer.adapter().key);
    let cicp = cicp_for(&crate::scene::OutputTransform::default());
    for (name, img) in render_set(renderer)? {
        write_png(&dir.join(format!("{name}.png")), &img, Some(cicp))?;
    }
    Ok(dir)
}

/// Text golden frame size (WP-0F.02; caption glyphs ≈ 19 px).
pub const TEXT_GOLDEN_SIZE: (u32, u32) = (512, 288);

/// Renders the text golden scenes (caption over the demo + multi-script, MSDF and Vello; Vello
/// is skipped on software adapters, where it is unsupported — see `text_gpu::ensure_vello`).
pub fn render_text_set(renderer: &mut Renderer) -> Result<Vec<(String, Image)>> {
    let (w, h) = TEXT_GOLDEN_SIZE;
    let provider = SyntheticSources::new(w, h)?;
    let mut out = Vec::new();
    let software = renderer.adapter().software;
    for (name, scene) in crate::timeline::text_golden_scenes(w, h) {
        if software && name.contains("_vello") {
            continue;
        }
        let frame = renderer.render_scene(&scene, &provider)?;
        out.push((name, frame.image));
    }
    Ok(out)
}

/// Writes the text golden set for `renderer`'s adapter.
pub fn write_text_set(renderer: &mut Renderer, root: &Path) -> Result<PathBuf> {
    let dir = root.join(&renderer.adapter().key);
    let cicp = cicp_for(&crate::scene::OutputTransform::default());
    for (name, img) in render_text_set(renderer)? {
        write_png(&dir.join(format!("{name}.png")), &img, Some(cicp))?;
    }
    Ok(dir)
}

/// Like [`check`] for the text set; an adapter whose directory lacks the text frames is
/// compared cross-adapter against `fallback_set`.
pub fn check_text(
    renderer: &mut Renderer,
    root: &Path,
    fallback_set: &str,
) -> Result<Vec<GoldenRow>> {
    let own = renderer.adapter().key.clone();
    let has_own = root.join(&own).join("text_multiscript_msdf.png").exists();
    let (set, same) = if has_own {
        (own, true)
    } else {
        (fallback_set.to_string(), false)
    };
    let dir = root.join(&set);
    let mut rows = Vec::new();
    for (name, img) in render_text_set(renderer)? {
        let reference = read_png(&dir.join(format!("{name}.png")))?;
        rows.push(GoldenRow {
            scene: name,
            reference_set: set.clone(),
            same_adapter: same,
            cmp: compare(&img, &reference, Transfer::Bt709),
        });
    }
    Ok(rows)
}

/// Adapter keys that have a golden set under `root`.
#[must_use]
pub fn available_sets(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(root)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// One golden comparison.
#[derive(Clone, Debug)]
pub struct GoldenRow {
    pub scene: String,
    pub reference_set: String,
    pub same_adapter: bool,
    pub cmp: Comparison,
}

/// Compares `renderer`'s output against its own golden set, or — if this adapter has none —
/// against `fallback_set` (cross-adapter, tolerance only).
pub fn check(renderer: &mut Renderer, root: &Path, fallback_set: &str) -> Result<Vec<GoldenRow>> {
    let own = renderer.adapter().key.clone();
    let sets = available_sets(root);
    let (set, same) = if sets.contains(&own) {
        (own, true)
    } else {
        (fallback_set.to_string(), false)
    };
    let dir = root.join(&set);
    let mut rows = Vec::new();
    for (name, img) in render_set(renderer)? {
        let reference = read_png(&dir.join(format!("{name}.png")))?;
        rows.push(GoldenRow {
            scene: name,
            reference_set: set.clone(),
            same_adapter: same,
            cmp: compare(&img, &reference, Transfer::Bt709),
        });
    }
    Ok(rows)
}
