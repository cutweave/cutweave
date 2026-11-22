//! `s4` — headless CLI for spike S4 (WP-0F.01). Every command is headless (no window/surface).

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use s4::gpu::{AdapterChoice, FrameStats, Renderer, list_adapters};
use s4::image::{cicp_for, read_png, write_png};
use s4::lut::Lut3d;
use s4::metrics::{self, compare};
use s4::scene::{Scene, Transfer};
use s4::source::SyntheticSources;
use s4::timeline::{self, demo_scene, test_scenes};
use s4::{cpu_ref, export, golden, graph};

#[derive(Parser)]
#[command(
    name = "s4",
    about = "Spike S4 compositor (WP-0F.01). All commands run headless."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Common {
    #[arg(long, default_value_t = 1920)]
    width: u32,
    #[arg(long, default_value_t = 1080)]
    height: u32,
    /// `auto`, `software` (WARP/lavapipe), an index from `s4 adapters`, `key:<key>` or a name
    /// substring.
    #[arg(long, default_value = "auto")]
    adapter: String,
}

#[derive(Subcommand)]
enum Cmd {
    /// List GPU adapters.
    Adapters,
    /// Render one frame to PNG.
    Render {
        /// Demo timeline frame (0..300 at 60 fps).
        #[arg(long, default_value_t = 0)]
        frame: i64,
        #[arg(long)]
        out: PathBuf,
        #[command(flatten)]
        common: Common,
        /// Render a named test scene instead of the demo timeline (see `s4 reference`).
        #[arg(long)]
        scene: Option<String>,
        /// Render a scene JSON file (WP-0G.01 interchange format).
        #[arg(long)]
        scene_json: Option<PathBuf>,
        /// Use the CPU reference renderer instead of the GPU.
        #[arg(long)]
        cpu: bool,
        /// Print the render graph.
        #[arg(long)]
        graph: bool,
        /// Output transfer: bt709 | srgb | bt1886 | linear.
        #[arg(long)]
        transfer: Option<String>,
        /// Output bits per channel (8 or 16).
        #[arg(long)]
        bits: Option<u8>,
    },
    /// Export the demo timeline as a PNG frame sequence (H.264/MP4 via FFmpeg is pending).
    Export {
        #[arg(long)]
        out_dir: PathBuf,
        #[arg(long, default_value_t = 0)]
        start: i64,
        #[arg(long, default_value_t = timeline::DEMO_FRAMES)]
        count: i64,
        #[command(flatten)]
        common: Common,
    },
    /// GPU vs the independent CPU reference for every test scene (PSNR / SSIM / ΔE2000).
    Reference {
        #[command(flatten)]
        common: Common,
        /// Write GPU, reference and ×32 diff PNGs here.
        #[arg(long)]
        diff_dir: Option<PathBuf>,
    },
    /// GPU timing table (timestamp queries) for the 1080p demo scene.
    Bench {
        #[command(flatten)]
        common: Common,
        /// Frames to time (steady state, starting at the dissolve).
        #[arg(long, default_value_t = 120)]
        frames: i64,
        /// Time every adapter instead of `--adapter`.
        #[arg(long)]
        all_adapters: bool,
        /// Print the per-pass breakdown.
        #[arg(long)]
        passes: bool,
    },
    /// Compare two PNGs with the §20 metrics.
    Compare {
        a: PathBuf,
        b: PathBuf,
        #[arg(long, default_value = "bt709")]
        transfer: String,
    },
    /// Print the scene JSON for a demo frame.
    Scene {
        #[arg(long, default_value_t = 0)]
        frame: i64,
        #[arg(long, default_value_t = 1920)]
        width: u32,
        #[arg(long, default_value_t = 1080)]
        height: u32,
    },
    /// Write the generated test LUT (`assets/warm_contrast_9.cube`).
    GenLut {
        #[arg(long, default_value_t = 9)]
        size: usize,
        #[arg(long)]
        out: PathBuf,
    },
    /// (Re)write the golden set for an adapter, or check all adapters against the goldens.
    Golden {
        #[arg(long, default_value = "auto")]
        adapter: String,
        /// Write instead of check.
        #[arg(long)]
        update: bool,
        /// Check every adapter.
        #[arg(long)]
        all_adapters: bool,
    },
}

fn parse_transfer(s: &str) -> Result<Transfer> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "bt709" | "709" => Transfer::Bt709,
        "srgb" => Transfer::Srgb,
        "bt1886" | "gamma24" => Transfer::Bt1886,
        "linear" => Transfer::Linear,
        other => bail!("unknown transfer `{other}`"),
    })
}

fn renderer(adapter: &str) -> Result<Renderer> {
    let r = Renderer::new(&AdapterChoice::parse(adapter))?;
    let a = r.adapter();
    eprintln!(
        "adapter: {} ({}, {}, {}) key={}",
        a.name, a.backend, a.device_type, a.driver, a.key
    );
    Ok(r)
}

fn ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Adapters => {
            println!(
                "{:<3} {:<8} {:<14} {:<5} {:<48} key",
                "#", "backend", "type", "ts", "name"
            );
            for a in list_adapters() {
                println!(
                    "{:<3} {:<8} {:<14} {:<5} {:<48} {}",
                    a.index, a.backend, a.device_type, a.timestamps, a.name, a.key
                );
            }
        }
        Cmd::Render {
            frame,
            out,
            common,
            scene,
            scene_json,
            cpu,
            graph: show_graph,
            transfer,
            bits,
        } => {
            let mut s: Scene = if let Some(p) = scene_json {
                Scene::from_json(
                    &std::fs::read_to_string(&p).with_context(|| p.display().to_string())?,
                )?
            } else if let Some(name) = scene {
                test_scenes(common.width, common.height)
                    .into_iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, s)| s)
                    .with_context(|| format!("unknown scene `{name}`"))?
            } else {
                demo_scene(common.width, common.height, frame)
            };
            if let Some(t) = transfer {
                s.output.transfer = parse_transfer(&t)?;
            }
            if let Some(b) = bits {
                s.output.bit_depth = b;
            }
            let provider = SyntheticSources::new(s.width, s.height)?;
            let t0 = Instant::now();
            let g = graph::build(&s, &provider)?;
            let build_ms = t0.elapsed().as_secs_f64() * 1e3;
            if show_graph {
                print!("{}", g.describe());
            }
            let img = if cpu {
                let t = Instant::now();
                let img = cpu_ref::render(&s, &provider)?;
                eprintln!("cpu reference: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
                img
            } else {
                let mut r = renderer(&common.adapter)?;
                let f = r.render(&g)?;
                eprintln!(
                    "graph build {build_ms:.2} ms, {} passes, GPU {}, wall {:.1} ms",
                    f.stats.passes_run,
                    f.stats
                        .gpu_ns
                        .map_or("n/a".into(), |n| format!("{:.3} ms", ms(n))),
                    f.stats.cpu_ms
                );
                f.image
            };
            write_png(&out, &img, Some(cicp_for(&s.output)))?;
            println!("wrote {}", out.display());
        }
        Cmd::Export {
            out_dir,
            start,
            count,
            common,
        } => {
            let mut r = renderer(&common.adapter)?;
            let provider = SyntheticSources::new(common.width, common.height)?;
            let mut st = export::export_png_sequence(
                &mut r,
                &provider,
                (common.width, common.height),
                start..start + count,
                &out_dir,
            )?;
            println!(
                "exported {} frames {}x{} to {} in {:.2} s ({:.1} fps incl. scene eval + PNG encode; render+readback {:.1} ms/frame; GPU median {:.3} ms)",
                st.frames,
                common.width,
                common.height,
                out_dir.display(),
                st.seconds,
                st.frames as f64 / st.seconds,
                st.render_ms_mean,
                percentile(&mut st.gpu_ms, 0.5)
            );
            println!(
                "H.264/MP4: pending — FFmpeg is not available on this machine (bundle arrives in WP-0A.06)."
            );
            println!(
                "  when it is: ffmpeg -framerate {} -i {}/frame_%05d.png -c:v libx264 -pix_fmt yuv420p \\\n    -colorspace bt709 -color_primaries bt709 -color_trc bt709 -color_range tv out.mp4",
                timeline::FPS,
                out_dir.display()
            );
        }
        Cmd::Reference { common, diff_dir } => {
            let mut r = renderer(&common.adapter)?;
            let provider = SyntheticSources::new(common.width, common.height)?;
            println!(
                "{:<12} {:>9} {:>8} {:>8} {:>8} {:>8} {:>6} {:>6}",
                "scene", "PSNR dB", "SSIM", "ΔE mean", "ΔE p95", "ΔE max", "max|Δ|", "pass"
            );
            let mut all = true;
            for (name, scene) in test_scenes(common.width, common.height) {
                let gpu = r.render_scene(&scene, &provider)?.image;
                let reference = cpu_ref::render(&scene, &provider)?;
                let c = compare(&gpu, &reference, scene.output.transfer);
                all &= c.passes_reference();
                println!(
                    "{:<12} {:>9.2} {:>8.5} {:>8.4} {:>8.4} {:>8.4} {:>6} {:>6}",
                    name,
                    c.psnr_db,
                    c.ssim,
                    c.delta_e.mean,
                    c.delta_e.p95,
                    c.delta_e.max,
                    c.max_abs_diff,
                    if c.passes_reference() { "ok" } else { "FAIL" }
                );
                if let Some(d) = &diff_dir {
                    let cicp = Some(cicp_for(&scene.output));
                    write_png(&d.join(format!("{name}_gpu.png")), &gpu, cicp)?;
                    write_png(&d.join(format!("{name}_ref.png")), &reference, cicp)?;
                    write_png(
                        &d.join(format!("{name}_diff32.png")),
                        &gpu.diff_image(&reference, 32),
                        None,
                    )?;
                }
            }
            println!(
                "criteria: PSNR > {} dB and mean ΔE2000 < {} vs the independent CPU f64 reference → {}",
                metrics::REFERENCE_MIN_PSNR_DB,
                metrics::REFERENCE_MAX_DELTA_E,
                if all { "PASS" } else { "FAIL" }
            );
        }
        Cmd::Bench {
            common,
            frames,
            all_adapters,
            passes,
        } => {
            let choices: Vec<AdapterChoice> = if all_adapters {
                list_adapters()
                    .into_iter()
                    .map(|a| AdapterChoice::Key(a.key))
                    .collect()
            } else {
                vec![AdapterChoice::parse(&common.adapter)]
            };
            println!(
                "scene: demo {}x{}, 3 layers (2 blend modes) + dissolve + blur-lite + LUT, frames {}..{} (INFORMAL)",
                common.width,
                common.height,
                timeline::DISSOLVE_START,
                timeline::DISSOLVE_START + frames
            );
            println!(
                "{:<46} {:<7} {:>10} {:>9} {:>9} {:>10} {:>11} {:>7}",
                "adapter",
                "backend",
                "cold GPU",
                "GPU p50",
                "GPU p95",
                "wall p50",
                "1st frame",
                "passes"
            );
            for choice in choices {
                let t_init = Instant::now();
                let mut r = match Renderer::new(&choice) {
                    Ok(r) => r,
                    Err(e) => {
                        println!("{choice:?}: {e}");
                        continue;
                    }
                };
                let provider = SyntheticSources::new(common.width, common.height)?;
                let first = demo_scene(common.width, common.height, timeline::DISSOLVE_START + 30);
                let f0 = r.render_scene(&first, &provider)?;
                let first_ms = t_init.elapsed().as_secs_f64() * 1e3;
                let _ = f0;
                r.clear_cache();
                let cold = r.render_scene(&first, &provider)?.stats;
                let mut gpu = Vec::new();
                let mut wall = Vec::new();
                let mut per_pass: Vec<(String, f64)> = Vec::new();
                let mut last_stats = FrameStats::default();
                for f in timeline::DISSOLVE_START..timeline::DISSOLVE_START + frames {
                    let s = demo_scene(common.width, common.height, f);
                    let st = r.render_scene(&s, &provider)?.stats;
                    if let Some(n) = st.gpu_ns {
                        gpu.push(ms(n));
                    }
                    wall.push(st.cpu_ms);
                    for (label, n) in &st.pass_ns {
                        let key = label.split_whitespace().next().unwrap_or("").to_string();
                        match per_pass.iter_mut().find(|(k, _)| *k == key) {
                            Some(e) => e.1 += ms(*n),
                            None => per_pass.push((key, ms(*n))),
                        }
                    }
                    last_stats = st;
                }
                let a = r.adapter();
                let fmt = |v: f64| {
                    if v.is_nan() {
                        "n/a".to_string()
                    } else {
                        format!("{v:.3} ms")
                    }
                };
                println!(
                    "{:<46} {:<7} {:>10} {:>9} {:>9} {:>10} {:>11} {:>7}",
                    format!("{} ({})", a.name, a.device_type),
                    a.backend,
                    cold.gpu_ns
                        .map_or("n/a".into(), |n| format!("{:.3} ms", ms(n))),
                    fmt(percentile(&mut gpu, 0.5)),
                    fmt(percentile(&mut gpu, 0.95)),
                    format!("{:.2} ms", percentile(&mut wall, 0.5)),
                    format!("{first_ms:.0} ms"),
                    format!(
                        "{}/{}",
                        last_stats.passes_run,
                        last_stats.passes_run + last_stats.cache_hits
                    ),
                );
                if passes {
                    for (k, total) in &per_pass {
                        println!("    {:<12} {:>8.3} ms/frame", k, total / frames as f64);
                    }
                }
            }
            println!(
                "cold = first frame after clearing the pass cache; GPU = first pass start → last pass end (timestamp queries);"
            );
            println!(
                "wall = encode + uploads + submit + wait + readback of the frame; 1st frame includes device + pipeline creation."
            );
        }
        Cmd::Compare { a, b, transfer } => {
            let (ia, ib) = (read_png(&a)?, read_png(&b)?);
            let c = compare(&ia, &ib, parse_transfer(&transfer)?);
            println!(
                "PSNR {:.2} dB, SSIM {:.6}, ΔE2000 mean {:.4} p95 {:.4} max {:.4}, max|Δ| {}, bit-identical {}",
                c.psnr_db,
                c.ssim,
                c.delta_e.mean,
                c.delta_e.p95,
                c.delta_e.max,
                c.max_abs_diff,
                c.bit_identical
            );
            println!(
                "§20 golden (SSIM ≥ {} and PSNR ≥ {} dB): {}",
                metrics::GOLDEN_MIN_SSIM,
                metrics::GOLDEN_MIN_PSNR_DB,
                if c.passes_golden() { "PASS" } else { "FAIL" }
            );
        }
        Cmd::Scene {
            frame,
            width,
            height,
        } => {
            println!("{}", demo_scene(width, height, frame).to_json()?);
        }
        Cmd::GenLut { size, out } => {
            std::fs::write(&out, Lut3d::warm_contrast(size).to_cube())?;
            println!("wrote {}", out.display());
        }
        Cmd::Golden {
            adapter,
            update,
            all_adapters,
        } => {
            let root = golden::default_root();
            let choices: Vec<AdapterChoice> = if all_adapters {
                list_adapters()
                    .into_iter()
                    .map(|a| AdapterChoice::Key(a.key))
                    .collect()
            } else {
                vec![AdapterChoice::parse(&adapter)]
            };
            for choice in choices {
                let mut r = Renderer::new(&choice)?;
                if update {
                    let dir = golden::write_set(&mut r, &root)?;
                    println!("wrote {}", dir.display());
                    continue;
                }
                let fallback = golden::available_sets(&root)
                    .into_iter()
                    .next()
                    .context("no golden sets; run `s4 golden --update` first")?;
                for row in golden::check(&mut r, &root, &fallback)? {
                    println!(
                        "{:<44} {:<12} vs {:<44} {:<5} PSNR {:>7.2} SSIM {:.6} bit-identical {:<5} {}",
                        r.adapter().key,
                        row.scene,
                        row.reference_set,
                        if row.same_adapter { "same" } else { "cross" },
                        row.cmp.psnr_db,
                        row.cmp.ssim,
                        row.cmp.bit_identical,
                        if row.cmp.passes_golden() {
                            "ok"
                        } else {
                            "FAIL"
                        }
                    );
                }
            }
        }
    }
    Ok(())
}
