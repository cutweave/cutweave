//! WP-0F.02 measurements shared by the CLI and the tests: 1000-glyph cost, 4K crispness vs the
//! CPU outline reference, and shimmer under sub-pixel motion. Informal (no clock pinning).

use std::path::Path;
use std::time::Instant;

use crate::Result;
use crate::color::to_linear;
use crate::cpu_ref;
use crate::gpu::Renderer;
use crate::image::{Image, write_png};
use crate::metrics::{psnr, ssim};
use crate::scene::{Node, Scene, TextRenderer, Transfer};
use crate::source::SyntheticSources;
use crate::text::caption::{glyph_storm, storm_text};
use crate::timeline::{crisp_scene, glyph_storm_scene, shimmer_scene};

/// Nearest-rank percentile (sorts `v`).
#[must_use]
pub fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

/// Result of [`glyph_bench`].
#[derive(Clone, Debug)]
pub struct GlyphBench {
    pub renderer: TextRenderer,
    pub glyphs: usize,
    pub instances: usize,
    /// Animator (spans for every glyph) per frame.
    pub animate_ms_p50: f64,
    /// Span resolution + instance buffer (MSDF) / scene encoding (Vello), layout cached.
    pub build_ms_p50: f64,
    pub build_ms_p95: f64,
    /// Render-graph build per frame (hashes the 1000-span node).
    pub graph_ms_p50: f64,
    /// First frame: shaping and MTSDF generation.
    pub layout_cold_ms: f64,
    pub atlas_cold_ms: f64,
    /// GPU time of the text pass(es) per frame.
    pub text_gpu_p50: f64,
    pub text_gpu_p95: f64,
    /// Whole frame GPU (text + composite + output).
    pub frame_gpu_p50: f64,
    pub wall_p50: f64,
    /// Allocator delta (bytes) vs the same frame without text, after the run (text target,
    /// atlas, instance buffers, Vello's buffers and targets).
    pub mem_delta: Option<i64>,
    pub atlas_bytes: u64,
}

/// Times `frames` frames of the `n`-glyph storm on `r`.
pub fn glyph_bench(
    r: &mut Renderer,
    size: (u32, u32),
    n: usize,
    frames: i64,
    renderer: TextRenderer,
    styled: bool,
) -> Result<GlyphBench> {
    let provider = SyntheticSources::new(16, 16)?;
    let mut scene = glyph_storm_scene(size.0, size.1, 0, n, renderer, styled);
    // Baseline: the same frame without the text layer (canvas, readback buffers allocated).
    let mut bare = scene.clone();
    bare.layers.clear();
    r.render_scene(&bare, &provider)?;
    let mem0 = r.gpu_memory();
    let text = storm_text(n);
    let first = r.render_scene(&scene, &provider)?;
    let (layout_cold_ms, atlas_cold_ms) = (first.stats.text.layout_ms, first.stats.text.atlas_ms);
    let (mut anim, mut build, mut graph, mut tgpu, mut fgpu, mut wall) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let mut instances = 0;
    let mut glyphs = 0;
    for f in 1..=frames {
        let t0 = Instant::now();
        let spans = glyph_storm(&text, f);
        anim.push(t0.elapsed().as_secs_f64() * 1e3);
        if let Node::Text(t) = &mut scene.layers[0].content {
            t.spans = spans;
        }
        let t1 = Instant::now();
        let g = crate::graph::build(&scene, &provider)?;
        graph.push(t1.elapsed().as_secs_f64() * 1e3);
        let st = r.render(&g)?.stats;
        build.push(st.text.build_ms);
        wall.push(st.cpu_ms);
        if let Some(ns) = st.text_gpu_ns {
            tgpu.push(ns as f64 / 1e6);
        }
        if let Some(ns) = st.gpu_ns {
            fgpu.push(ns as f64 / 1e6);
        }
        instances = st.text.instances;
        glyphs = st.text.glyphs;
    }
    let mem_delta = match (mem0, r.gpu_memory()) {
        (Some(a), Some(b)) => Some(b.0 as i64 - a.0 as i64),
        _ => None,
    };
    Ok(GlyphBench {
        renderer,
        glyphs,
        instances,
        animate_ms_p50: percentile(&mut anim, 0.5),
        build_ms_p50: percentile(&mut build.clone(), 0.5),
        build_ms_p95: percentile(&mut build, 0.95),
        graph_ms_p50: percentile(&mut graph, 0.5),
        layout_cold_ms,
        atlas_cold_ms,
        text_gpu_p50: percentile(&mut tgpu.clone(), 0.5),
        text_gpu_p95: percentile(&mut tgpu, 0.95),
        frame_gpu_p50: percentile(&mut fgpu, 0.5),
        wall_p50: percentile(&mut wall, 0.5),
        mem_delta,
        atlas_bytes: r.text_atlas_info().map_or(0, |a| a.1),
    })
}

/// Crop of an image.
#[must_use]
pub fn crop(img: &Image, x: u32, y: u32, w: u32, h: u32) -> Image {
    let (w, h) = (w.min(img.width - x), h.min(img.height - y));
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for j in y..y + h {
        let s = ((j * img.width + x) * 4) as usize;
        data.extend_from_slice(&img.data[s..s + (w * 4) as usize]);
    }
    Image {
        width: w,
        height: h,
        bit_depth: img.bit_depth,
        data,
    }
}

/// Nearest-neighbour enlargement (for zoom crops).
#[must_use]
pub fn zoom(img: &Image, k: u32) -> Image {
    let (w, h) = (img.width * k, img.height * k);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&img.data[(((y / k) * img.width + x / k) * 4) as usize..][..4]);
        }
    }
    Image {
        width: w,
        height: h,
        bit_depth: img.bit_depth,
        data,
    }
}

/// One crispness comparison.
#[derive(Clone, Debug)]
pub struct Crisp {
    pub renderer: TextRenderer,
    pub ssim_full: f64,
    /// SSIM over the text box only (the number that matters).
    pub ssim_text: f64,
    pub psnr_text: f64,
    pub max_abs: u16,
    pub gpu_text_ms: f64,
}

/// Text box rect of [`crisp_scene`] in pixels.
#[must_use]
pub fn crisp_rect(w: u32, h: u32, size: f32) -> [u32; 4] {
    let bw = (w as f32 * 0.94) as u32;
    let bh = (size * 2.6) as u32;
    let x = (w as f32 * 0.03) as u32;
    let y = ((h as f32 - size * 2.6) * 0.5) as u32;
    [x, y, bw.min(w - x), bh.min(h - y)]
}

/// GPU (`renderer`) vs the CPU 8×8 outline reference on [`crisp_scene`].
pub fn crispness(
    r: &mut Renderer,
    size: (u32, u32),
    glyph_px: f32,
    renderer: TextRenderer,
    reference: &Image,
    diff_dir: Option<&Path>,
) -> Result<(Crisp, Image)> {
    let provider = SyntheticSources::new(16, 16)?;
    let scene = crisp_scene(size.0, size.1, glyph_px, [0.0; 2], renderer);
    // Warm-up (atlas generation, Vello pipelines), then a timed frame with the pass cache
    // cleared so the (static) text pass really runs.
    let _warm = r.render_scene(&scene, &provider)?;
    r.clear_cache();
    let f = r.render_scene(&scene, &provider)?;
    let img = f.image;
    let [x, y, w, h] = crisp_rect(size.0, size.1, glyph_px);
    let (gc, rc) = (crop(&img, x, y, w, h), crop(reference, x, y, w, h));
    let c = Crisp {
        renderer,
        ssim_full: ssim(&img, reference),
        ssim_text: ssim(&gc, &rc),
        psnr_text: psnr(&gc, &rc),
        max_abs: gc.max_abs_diff(&rc),
        gpu_text_ms: f.stats.text_gpu_ns.map_or(f64::NAN, |n| n as f64 / 1e6),
    };
    if let Some(d) = diff_dir {
        let tag = match renderer {
            TextRenderer::Msdf => "msdf",
            TextRenderer::Vello => "vello",
        };
        let stem = format!("crisp_{}p_{tag}", size.1);
        write_png(&d.join(format!("{stem}.png")), &img, None)?;
        write_png(
            &d.join(format!("{stem}_diff8.png")),
            &img.diff_image(reference, 8),
            None,
        )?;
        // Zoom on the first glyphs ("Ag") ×4.
        let zw = (glyph_px * 1.6) as u32;
        let zc = |im: &Image| zoom(&crop(im, x, y, zw, (glyph_px * 1.3) as u32), 4);
        write_png(&d.join(format!("{stem}_zoom4.png")), &zc(&img), None)?;
    }
    Ok((c, img))
}

/// CPU reference for [`crisp_scene`] (renderer-independent).
pub fn crisp_reference(size: (u32, u32), glyph_px: f32) -> Result<Image> {
    let provider = SyntheticSources::new(16, 16)?;
    cpu_ref::render(
        &crisp_scene(size.0, size.1, glyph_px, [0.0; 2], TextRenderer::Msdf),
        &provider,
    )
}

/// Shimmer statistics over 16 sub-pixel phases.
#[derive(Clone, Debug)]
pub struct Shimmer {
    pub label: String,
    /// (max − min) / mean of the frame's total linear luminance ("ink"); 0 = perfectly stable.
    pub ink_range: f64,
    /// Worst deviation of the luminance centroid from the commanded offset (px).
    pub centroid_err: f64,
    /// PSNR vs the CPU reference at the same phase: mean and spread (max − min).
    pub psnr_mean: f64,
    pub psnr_spread: f64,
}

fn ink_and_centroid(img: &Image) -> (f64, f64) {
    let (mut ink, mut mx) = (0.0, 0.0);
    let max = f64::from(img.max_value());
    for y in 0..img.height {
        for x in 0..img.width {
            let p = img.pixel(x, y);
            let l: f64 = 0.2126 * to_linear(f64::from(p[0]) / max, Transfer::Bt709)
                + 0.7152 * to_linear(f64::from(p[1]) / max, Transfer::Bt709)
                + 0.0722 * to_linear(f64::from(p[2]) / max, Transfer::Bt709);
            ink += l;
            mx += l * (f64::from(x) + 0.5);
        }
    }
    (ink, if ink > 0.0 { mx / ink } else { 0.0 })
}

/// Renders the shimmer line at phases 0, 1/16, … 15/16 px. `renderer = None` uses the CPU
/// reference itself (the baseline).
pub fn shimmer(
    r: Option<&mut Renderer>,
    size: (u32, u32),
    renderer: TextRenderer,
    resampled: bool,
    references: &[Image],
) -> Result<Shimmer> {
    let provider = SyntheticSources::new(16, 16)?;
    let mut rr = r;
    let (mut inks, mut cents, mut ps) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..16 {
        let off = i as f32 / 16.0;
        let scene: Scene = shimmer_scene(size.0, size.1, off, renderer, resampled);
        let img = match rr.as_deref_mut() {
            Some(r) => r.render_scene(&scene, &provider)?.image,
            None => cpu_ref::render(&scene, &provider)?,
        };
        let (ink, c) = ink_and_centroid(&img);
        inks.push(ink);
        cents.push(c - f64::from(off));
        if let Some(reference) = references.get(i) {
            ps.push(psnr(&img, reference));
        }
    }
    let mean = inks.iter().sum::<f64>() / inks.len() as f64;
    let (mn, mx) = inks
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let c0 = cents[0];
    let centroid_err = cents.iter().map(|c| (c - c0).abs()).fold(0.0, f64::max);
    let finite: Vec<f64> = ps.iter().copied().filter(|p| p.is_finite()).collect();
    let pm = if finite.is_empty() {
        f64::INFINITY
    } else {
        finite.iter().sum::<f64>() / finite.len() as f64
    };
    let spread = finite
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    Ok(Shimmer {
        label: String::new(),
        ink_range: (mx - mn) / mean,
        centroid_err,
        psnr_mean: pm,
        psnr_spread: if finite.is_empty() {
            0.0
        } else {
            spread.1 - spread.0
        },
    })
}

/// CPU reference frames of the shimmer line at the 16 phases.
pub fn shimmer_references(size: (u32, u32)) -> Result<Vec<Image>> {
    let provider = SyntheticSources::new(16, 16)?;
    (0..16)
        .map(|i| {
            let s = shimmer_scene(size.0, size.1, i as f32 / 16.0, TextRenderer::Msdf, false);
            cpu_ref::render(&s, &provider)
        })
        .collect()
}
