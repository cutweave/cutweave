// SPDX-License-Identifier: GPL-3.0-or-later
//! `s4b`: CLI for Spike S4b (WP-0G.01). See README.md.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use s4::gpu::AdapterChoice;
use s4b::adversarial;
use s4b::lower::{LowerOptions, TextMode, lower};
use s4b::render::{FrameRenderer, TextChoice};
use s4b::runtime::{BindOptions, Composition, Limits, parse_frame, sandbox_globals};
use s4b::stats::{Summary, fnv64};
use s4b::{ComposeError, transpile};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "s4b",
    about = "Spike S4b: TS/JSX compositions -> oxc -> QuickJS -> S4 compositor"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Args, Clone)]
struct Input {
    /// Composition source (.tsx/.ts/.jsx/.js).
    file: PathBuf,
    /// Props as JSON, or @path/to/props.json (default: <stem>.props.json next to the file, else {}).
    #[arg(long)]
    props: Option<String>,
    #[arg(long, default_value_t = 1920)]
    width: u32,
    #[arg(long, default_value_t = 1080)]
    height: u32,
    /// Override the composition's fps.
    #[arg(long)]
    fps: Option<f64>,
    /// Seed for Math.random (the timeline item id in the product).
    #[arg(long, default_value = "item-0")]
    seed: String,
    /// Per-frame time limit in ms.
    #[arg(long, default_value_t = 50)]
    frame_ms: u64,
    /// Deterministic CPU budget per frame in interrupt polls (default: wall clock only).
    #[arg(long)]
    fuel: Option<u64>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Transpile and print the JS (or diagnostics as JSON).
    Transpile {
        file: PathBuf,
        /// Print {js, warnings, timings} as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print the composition metadata and props JSON Schema (MCP `composition.create` result).
    Describe { file: PathBuf },
    /// Print the scene of one frame (composition nodes, or the lowered S4 scene with --s4).
    Scene {
        #[command(flatten)]
        input: Input,
        #[arg(long, default_value_t = 0.0)]
        frame: f64,
        #[arg(long)]
        s4: bool,
    },
    /// Render frames to PNG through the S4 compositor.
    Render {
        #[command(flatten)]
        input: Input,
        /// Comma-separated frames.
        #[arg(long, default_value = "0")]
        frames: String,
        #[arg(long, default_value = "out")]
        out_dir: PathBuf,
        /// auto | software | <index> | key:<key> | <name substring>
        #[arg(long, default_value = "software")]
        adapter: String,
        /// Media source composited under the composition (e.g. camera709).
        #[arg(long)]
        under: Option<String>,
        /// auto | native | placeholder
        #[arg(long, default_value = "auto")]
        text: String,
        /// Output transfer: bt709 (S4 default, video) or srgb (hex colours round-trip).
        #[arg(long, default_value = "bt709")]
        transfer: String,
    },
    /// Transpile/load/eval timings (and optional GPU render timings).
    Bench {
        #[command(flatten)]
        input: Input,
        /// Frames evaluated for the eval statistics.
        #[arg(long, default_value_t = 10_000)]
        frames: usize,
        /// Warm transpile/load iterations.
        #[arg(long, default_value_t = 100)]
        iters: usize,
        /// Also render this many frames on the GPU.
        #[arg(long, default_value_t = 0)]
        render: usize,
        #[arg(long, default_value = "auto")]
        adapter: String,
        #[arg(long)]
        json: bool,
    },
    /// Hash the scene JSON of frames [0, n) and the PNG bytes of selected frames (determinism).
    Hash {
        #[command(flatten)]
        input: Input,
        #[arg(long, default_value_t = 240)]
        frames: usize,
        #[arg(long, default_value = "0,60,119")]
        png: String,
        #[arg(long, default_value = "software")]
        adapter: String,
        /// Evaluate frames in reverse order (order-independence check).
        #[arg(long)]
        reverse: bool,
    },
    /// Run `hash` in N separate processes (and once in reverse order) and compare.
    Determinism {
        #[command(flatten)]
        input: Input,
        #[arg(long, default_value_t = 3)]
        runs: usize,
        #[arg(long, default_value_t = 240)]
        frames: usize,
        #[arg(long, default_value = "0,60,119")]
        png: String,
        #[arg(long, default_value = "software")]
        adapter: String,
    },
    /// Run the adversarial sandbox suite.
    Sandbox {
        #[arg(long)]
        json: bool,
    },
    /// List the globals a composition can see.
    Globals,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Compositions run on a worker thread with an explicit native stack: QuickJS's JS stack
    // limit is checked against the native stack, and the Windows main thread has only 1 MiB.
    let worker = std::thread::Builder::new()
        .name("compose".into())
        .stack_size(16 << 20)
        .spawn(move || run(cli));
    let result = match worker.map(std::thread::JoinHandle::join) {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err(anyhow!("worker thread panicked")),
        Err(e) => Err(anyhow!("spawning the worker thread: {e}")),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            if let Some(ce) = e.downcast_ref::<ComposeError>() {
                eprintln!("{}", ce.to_json());
            } else {
                eprintln!("error: {e:#}");
            }
            ExitCode::FAILURE
        }
    }
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || "composition.tsx".into(),
        |s| s.to_string_lossy().into_owned(),
    )
}

fn props_json(input: &Input) -> Result<String> {
    if let Some(p) = &input.props {
        return match p.strip_prefix('@') {
            Some(path) => read(Path::new(path)),
            None => Ok(p.clone()),
        };
    }
    let stem = input
        .file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let sibling = input.file.with_file_name(format!("{stem}.props.json"));
    if sibling.exists() {
        read(&sibling)
    } else {
        Ok("{}".into())
    }
}

fn limits(input: &Input) -> Limits {
    Limits {
        frame_time: std::time::Duration::from_millis(input.frame_ms),
        fuel_polls: input.fuel,
        ..Limits::default()
    }
}

fn bind_options(input: &Input) -> BindOptions {
    BindOptions {
        width: input.width,
        height: input.height,
        fps: input.fps,
        item_seed: input.seed.clone(),
    }
}

fn load(input: &Input) -> Result<Composition> {
    let src = read(&input.file)?;
    let compiled = transpile(&file_name(&input.file), &src)?;
    for w in &compiled.warnings {
        eprintln!("{}", w.render());
    }
    Ok(Composition::load(&compiled, limits(input))?)
}

fn parse_frames(s: &str) -> Result<Vec<f64>> {
    s.split(',')
        .filter(|t| !t.trim().is_empty())
        .map(|t| {
            t.trim()
                .parse::<f64>()
                .map_err(|e| anyhow!("bad frame `{t}`: {e}"))
        })
        .collect()
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.cmd {
        Cmd::Transpile { file, json } => {
            let src = read(&file)?;
            let c = transpile(&file_name(&file), &src)?;
            if json {
                #[derive(Serialize)]
                struct Out<'a> {
                    js: &'a str,
                    warnings: &'a [s4b::Diagnostic],
                    timings: s4b::transpile::TranspileTimings,
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&Out {
                        js: &c.js,
                        warnings: &c.warnings,
                        timings: c.timings
                    })?
                );
            } else {
                for w in &c.warnings {
                    eprintln!("{}", w.render());
                }
                print!("{}", c.js);
            }
        }
        Cmd::Describe { file } => {
            let src = read(&file)?;
            let c = transpile(&file_name(&file), &src)?;
            let comp = Composition::load(&c, Limits::default())?;
            #[derive(Serialize)]
            #[serde(rename_all = "camelCase")]
            struct Out<'a> {
                #[serde(flatten)]
                info: &'a s4b::runtime::CompositionInfo,
                warnings: &'a [s4b::Diagnostic],
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&Out {
                    info: comp.info(),
                    warnings: &c.warnings
                })?
            );
        }
        Cmd::Scene { input, frame, s4 } => {
            let comp = load(&input)?;
            let ev = comp.bind(&props_json(&input)?, &bind_options(&input))?;
            let json = ev.eval_json(frame)?;
            if s4 {
                let out = parse_frame(&json)?;
                let assets = s4b::assets::AssetSources::new(input.width, input.height)
                    .map_err(|e| anyhow!("{e}"))?;
                let opts = LowerOptions {
                    width: input.width,
                    height: input.height,
                    text: TextMode::Native,
                    ..LowerOptions::default()
                };
                let scene = lower(&out, &opts, &assets)?;
                println!("{}", scene.to_json().map_err(|e| anyhow!("{e}"))?);
            } else {
                let v: serde_json::Value = serde_json::from_str(&json)?;
                println!("{}", serde_json::to_string_pretty(&v)?);
            }
        }
        Cmd::Render {
            input,
            frames,
            out_dir,
            adapter,
            under,
            text,
            transfer,
        } => {
            let comp = load(&input)?;
            let ev = comp.bind(&props_json(&input)?, &bind_options(&input))?;
            let text = TextChoice::parse(&text)
                .ok_or_else(|| anyhow!("--text: auto|native|placeholder"))?;
            let mut r = FrameRenderer::new(
                &AdapterChoice::parse(&adapter),
                input.width,
                input.height,
                text,
            )?;
            r.options.under = under;
            r.options.output.transfer = match transfer.as_str() {
                "bt709" => s4::scene::Transfer::Bt709,
                "srgb" => s4::scene::Transfer::Srgb,
                other => bail!("--transfer: bt709|srgb, got {other}"),
            };
            eprintln!("adapter: {}", r.adapter());
            std::fs::create_dir_all(&out_dir)?;
            let stem = input
                .file
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            for f in parse_frames(&frames)? {
                let t = Instant::now();
                let out = ev.eval(f)?;
                let eval_ms = t.elapsed().as_secs_f64() * 1e3;
                for l in &out.logs {
                    eprintln!("[{} frame {f}] {}", l.level, l.message);
                }
                let frame = r.render(&out, f as i64)?;
                let path = out_dir.join(format!("{stem}_f{:04}.png", f as i64));
                std::fs::write(&path, &frame.png)?;
                println!(
                    "{} ({} nodes, eval {eval_ms:.3} ms, lower {:.3} ms, gpu {} , text {:?}, png fnv64 {:016x})",
                    path.display(),
                    out.node_count(),
                    frame.lower_us / 1e3,
                    frame
                        .gpu
                        .gpu_ns
                        .map_or("n/a".into(), |ns| format!("{:.3} ms", ns as f64 / 1e6)),
                    frame.text,
                    fnv64(&frame.png)
                );
            }
        }
        Cmd::Bench {
            input,
            frames,
            iters,
            render,
            adapter,
            json,
        } => {
            let report = bench(&input, frames, iters, render, &adapter)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_bench(&report);
            }
        }
        Cmd::Hash {
            input,
            frames,
            png,
            adapter,
            reverse,
        } => {
            let h = hash_run(&input, frames, &parse_frames(&png)?, &adapter, reverse)?;
            println!("{}", serde_json::to_string(&h)?);
        }
        Cmd::Determinism {
            input,
            runs,
            frames,
            png,
            adapter,
        } => {
            return determinism(&input, runs, frames, &png, &adapter);
        }
        Cmd::Sandbox { json } => {
            let outcomes: Vec<_> = adversarial::cases()
                .iter()
                .map(|c| adversarial::run(c, Limits::default()))
                .collect();
            let passed = outcomes.iter().filter(|o| o.passed).count();
            if json {
                println!("{}", serde_json::to_string_pretty(&outcomes)?);
            } else {
                println!("| case | attack | result | time (ms) | contained |");
                println!("|---|---|---|---|---|");
                for o in &outcomes {
                    println!(
                        "| `{}` | {} | {} | {:.1} | {} |",
                        o.name,
                        o.what,
                        o.actual,
                        o.elapsed_ms,
                        if o.passed { "yes" } else { "**NO**" }
                    );
                }
                println!("\n{passed}/{} contained", outcomes.len());
                for o in outcomes.iter().filter(|o| !o.passed) {
                    println!("FAILED {}: {} {}", o.name, o.actual, o.detail);
                }
            }
            if passed != outcomes.len() {
                return Ok(ExitCode::FAILURE);
            }
        }
        Cmd::Globals => {
            let comp = s4b::runtime::load_source("g.tsx", s4b::LOWER_THIRD_TSX, Limits::default())?;
            println!("{}", sandbox_globals(&comp)?.join(" "));
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Serialize)]
struct BenchReport {
    file: String,
    source_bytes: usize,
    js_bytes: usize,
    transpile_cold_ms: f64,
    transpile_warm_ms: Summary,
    transpile_stage_ms_p50: BTreeMap<&'static str, f64>,
    load_cold_ms: f64,
    load_warm_ms: Summary,
    load_stage_ms_cold: BTreeMap<&'static str, f64>,
    bind_ms: f64,
    nodes_per_frame: usize,
    scene_json_bytes: usize,
    frames: usize,
    /// Fast path: QuickJS render() + direct read of the node objects into Rust.
    eval_direct_ms: Summary,
    /// Lowering to the S4 scene graph.
    lower_ms: Summary,
    /// direct + lower: the per-frame cost the 1 ms budget applies to.
    eval_total_ms: Summary,
    /// JSON path: QuickJS render() + JSON.stringify.
    eval_json_ms: Summary,
    /// JSON path: serde parse of the scene JSON.
    parse_ms: Summary,
    interrupt_polls_per_frame: f64,
    heap_bytes_after: i64,
    gpu: Option<GpuBench>,
}

#[derive(Serialize)]
struct GpuBench {
    adapter: String,
    frames: usize,
    gpu_ms: Summary,
    wall_ms: Summary,
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn bench(
    input: &Input,
    frames: usize,
    iters: usize,
    render: usize,
    adapter: &str,
) -> Result<BenchReport> {
    let src = read(&input.file)?;
    let name = file_name(&input.file);

    let t = Instant::now();
    let compiled = transpile(&name, &src)?;
    let transpile_cold_ms = ms(t);
    let mut warm = Vec::with_capacity(iters);
    let mut stages: [Vec<f64>; 4] = Default::default();
    for _ in 0..iters {
        let t = Instant::now();
        let c = transpile(&name, &src)?;
        warm.push(ms(t));
        stages[0].push(c.timings.parse_us / 1e3);
        stages[1].push(c.timings.semantic_us / 1e3);
        stages[2].push(c.timings.transform_us / 1e3);
        stages[3].push(c.timings.codegen_us / 1e3);
    }
    let transpile_stage_ms_p50 = ["parse", "semantic+policy", "transform", "codegen"]
        .into_iter()
        .zip(stages.iter().map(|s| Summary::of(s).p50))
        .collect();

    let lim = limits(input);
    let t = Instant::now();
    let comp = Composition::load(&compiled, lim)?;
    let load_cold_ms = ms(t);
    let lt = comp.timings();
    let load_stage_ms_cold = [
        ("runtime+context", lt.runtime_us / 1e3),
        ("prelude+hardening", lt.prelude_us / 1e3),
        ("module+describe", lt.module_us / 1e3),
    ]
    .into_iter()
    .collect();
    let mut load_warm = Vec::new();
    for _ in 0..iters.min(50) {
        let t = Instant::now();
        let c = Composition::load(&compiled, lim)?;
        load_warm.push(ms(t));
        drop(c);
    }

    let props = props_json(input)?;
    let t = Instant::now();
    let ev = comp.bind(&props, &bind_options(input))?;
    let bind_ms = ms(t);
    let duration = comp.info().duration_in_frames.max(1) as usize;
    let assets =
        s4b::assets::AssetSources::new(input.width, input.height).map_err(|e| anyhow!("{e}"))?;
    let lopts = LowerOptions {
        width: input.width,
        height: input.height,
        text: TextMode::Native,
        ..LowerOptions::default()
    };
    // Warm-up.
    for f in 0..100.min(frames) {
        let _ = ev.eval((f % duration) as f64)?;
        let _ = ev.eval_json((f % duration) as f64)?;
    }
    let d = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1e3;
    // Fast path: render() + direct read of the node objects, then lowering.
    let polls0 = comp.polls();
    let (mut direct, mut low, mut total) = (Vec::new(), Vec::new(), Vec::new());
    let mut nodes = 0;
    for i in 0..frames {
        let f = (i % duration) as f64;
        let t0 = Instant::now();
        let out = ev.eval(f)?;
        let t1 = Instant::now();
        let scene = lower(&out, &lopts, &assets)?;
        let t2 = Instant::now();
        std::hint::black_box(&scene);
        direct.push(d(t0, t1));
        low.push(d(t1, t2));
        total.push(d(t0, t2));
        nodes = nodes.max(out.node_count());
    }
    let polls = (comp.polls() - polls0) as f64 / frames.max(1) as f64;
    // JSON path (canonical bytes): render() + JSON.stringify in QuickJS, serde parse in Rust.
    let (mut js, mut parse) = (Vec::new(), Vec::new());
    let mut bytes = 0;
    for i in 0..frames {
        let f = (i % duration) as f64;
        let t0 = Instant::now();
        let json = ev.eval_json(f)?;
        let t1 = Instant::now();
        let out = parse_frame(&json)?;
        let t2 = Instant::now();
        std::hint::black_box(&out);
        js.push(d(t0, t1));
        parse.push(d(t1, t2));
        bytes = bytes.max(json.len());
    }

    let gpu = if render > 0 {
        let mut r = FrameRenderer::new(
            &AdapterChoice::parse(adapter),
            input.width,
            input.height,
            TextChoice::Auto,
        )?;
        let (mut g, mut w) = (Vec::new(), Vec::new());
        for i in 0..render + 5 {
            let f = (i % duration) as f64;
            let out = ev.eval(f)?;
            let t = Instant::now();
            let fr = r.render(&out, f as i64)?;
            if i >= 5 {
                w.push(ms(t));
                if let Some(ns) = fr.gpu.gpu_ns {
                    g.push(ns as f64 / 1e6);
                }
            }
        }
        Some(GpuBench {
            adapter: r.adapter(),
            frames: render,
            gpu_ms: Summary::of(&g),
            wall_ms: Summary::of(&w),
        })
    } else {
        None
    };

    Ok(BenchReport {
        file: name,
        source_bytes: src.len(),
        js_bytes: compiled.js.len(),
        transpile_cold_ms,
        transpile_warm_ms: Summary::of(&warm),
        transpile_stage_ms_p50,
        load_cold_ms,
        load_warm_ms: Summary::of(&load_warm),
        load_stage_ms_cold,
        bind_ms,
        nodes_per_frame: nodes,
        scene_json_bytes: bytes,
        frames,
        eval_direct_ms: Summary::of(&direct),
        lower_ms: Summary::of(&low),
        eval_total_ms: Summary::of(&total),
        eval_json_ms: Summary::of(&js),
        parse_ms: Summary::of(&parse),
        interrupt_polls_per_frame: polls,
        heap_bytes_after: comp.memory_used(),
        gpu,
    })
}

fn print_bench(r: &BenchReport) {
    let row = |name: &str, s: &Summary| {
        println!(
            "{name:<34} p50 {:>8.4}  p95 {:>8.4}  p99 {:>8.4}  max {:>8.4}  (n={})",
            s.p50, s.p95, s.p99, s.max, s.n
        );
    };
    println!(
        "{}: {} B TSX -> {} B JS",
        r.file, r.source_bytes, r.js_bytes
    );
    println!(
        "transpile cold                     {:.4} ms",
        r.transpile_cold_ms
    );
    row("transpile warm (ms)", &r.transpile_warm_ms);
    for (k, v) in &r.transpile_stage_ms_p50 {
        println!("  {k:<32} p50 {v:.4} ms");
    }
    println!(
        "load cold (runtime+prelude+module) {:.4} ms",
        r.load_cold_ms
    );
    for (k, v) in &r.load_stage_ms_cold {
        println!("  {k:<32} {v:.4} ms");
    }
    row("load warm (ms)", &r.load_warm_ms);
    println!("bind (props validation)            {:.4} ms", r.bind_ms);
    println!(
        "frame: {} nodes, {} B scene JSON, {:.2} interrupt polls/frame, heap {} KiB",
        r.nodes_per_frame,
        r.scene_json_bytes,
        r.interrupt_polls_per_frame,
        r.heap_bytes_after / 1024
    );
    row("eval: render + direct read", &r.eval_direct_ms);
    row("eval: lower to S4 scene", &r.lower_ms);
    row("eval: TOTAL (direct + lower)", &r.eval_total_ms);
    row("json path: render + stringify", &r.eval_json_ms);
    row("json path: serde parse", &r.parse_ms);
    if let Some(g) = &r.gpu {
        println!("GPU ({}) over {} frames:", g.adapter, g.frames);
        row("  gpu (timestamp queries)", &g.gpu_ms);
        row("  wall (lower+render+readback+png)", &g.wall_ms);
    }
}

#[derive(Serialize, serde::Deserialize, PartialEq, Debug)]
struct HashReport {
    frames: usize,
    /// fnv64 over the per-frame scene-JSON hashes, in frame order.
    scene_hash: String,
    per_frame: Vec<String>,
    png: BTreeMap<String, String>,
}

fn hash_run(
    input: &Input,
    frames: usize,
    png: &[f64],
    adapter: &str,
    reverse: bool,
) -> Result<HashReport> {
    let comp = load(input)?;
    let ev = comp.bind(&props_json(input)?, &bind_options(input))?;
    let mut per = vec![String::new(); frames];
    let order: Vec<usize> = if reverse {
        (0..frames).rev().collect()
    } else {
        (0..frames).collect()
    };
    for f in order {
        per[f] = format!("{:016x}", fnv64(ev.eval_json(f as f64)?.as_bytes()));
    }
    let scene_hash = format!("{:016x}", fnv64(per.join("").as_bytes()));
    let mut pngs = BTreeMap::new();
    if !png.is_empty() {
        let mut r = FrameRenderer::new(
            &AdapterChoice::parse(adapter),
            input.width,
            input.height,
            TextChoice::Auto,
        )?;
        for &f in png {
            let out = ev.eval(f)?;
            let fr = r.render(&out, f as i64)?;
            pngs.insert(format!("{f}"), format!("{:016x}", fnv64(&fr.png)));
        }
    }
    Ok(HashReport {
        frames,
        scene_hash,
        per_frame: per,
        png: pngs,
    })
}

fn determinism(
    input: &Input,
    runs: usize,
    frames: usize,
    png: &str,
    adapter: &str,
) -> Result<ExitCode> {
    let exe = std::env::current_exe()?;
    let mut reports = Vec::new();
    let mut args: Vec<String> = vec![
        "hash".into(),
        input.file.display().to_string(),
        "--width".into(),
        input.width.to_string(),
        "--height".into(),
        input.height.to_string(),
        "--seed".into(),
        input.seed.clone(),
        "--frame-ms".into(),
        input.frame_ms.to_string(),
        "--frames".into(),
        frames.to_string(),
        "--png".into(),
        png.to_string(),
        "--adapter".into(),
        adapter.to_string(),
        "--props".into(),
        props_json(input)?,
    ];
    if let Some(fps) = input.fps {
        args.extend(["--fps".into(), fps.to_string()]);
    }
    for run in 0..=runs {
        let mut a = args.clone();
        let label = if run == runs {
            a.push("--reverse".into());
            "reverse order (in one process)".to_string()
        } else {
            format!("process {}", run + 1)
        };
        let t = Instant::now();
        let out = std::process::Command::new(&exe).args(&a).output()?;
        if !out.status.success() {
            bail!("{label} failed: {}", String::from_utf8_lossy(&out.stderr));
        }
        let rep: HashReport = serde_json::from_slice(&out.stdout)
            .with_context(|| format!("{label}: {}", String::from_utf8_lossy(&out.stdout)))?;
        println!(
            "{label:<32} scene[0..{frames}) {}  png {:?}  ({:.1} s)",
            rep.scene_hash,
            rep.png,
            t.elapsed().as_secs_f64()
        );
        reports.push(rep);
    }
    let first = &reports[0];
    let same = reports.iter().all(|r| r == first);
    println!(
        "{}",
        if same {
            format!(
                "DETERMINISTIC: {} runs + reverse order byte-identical (scene JSON and PNG)",
                runs
            )
        } else {
            "NOT DETERMINISTIC".to_string()
        }
    );
    Ok(if same {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
