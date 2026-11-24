// SPDX-License-Identifier: GPL-3.0-or-later
//! `s3`: the Spike S3 harness. See README.md for the commands and the findings.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use s3::hw::{self, HwSpec};
use s3::manifest::{self, Entry};
use s3::monitor::{Monitor, Utilisation};
use s3::robust::{self, Outcome};
use s3::source::{Options, Threads, VideoSource};
use s3::{bench, probe, verify};

const USAGE: &str = "\
s3: Spike S3 harness (WP-0E.01, WP-0E.02). Generate media first: python gen_media.py

USAGE: s3 <command> [options] [files...]   (no files: every matching entry of media/manifest.tsv)

COMMANDS
  info       probe: codec, pixel format, rotation, color tags, VFR, GOP; checks manifest expectations
  verify     frame-exactness vs ffprobe/ffmpeg truth: sequential, N random seeks, edges, scrub
             (--hw X: hardware decode, compared with the software frame: bit-exact or PSNR >= 50 dB)
  bench      random-seek / scrub / step latency (p50/p95) and decode fps per configuration
  gpu        decode + upload to a wgpu texture + NV12/P010->RGB shader: texture-ready latency for
             random seeks and sequential playback, plus a readback check of the rendered frame
  hwinfo     hwaccel device types in this FFmpeg build, which devices open, wgpu adapters
  zerocopy   (feature zero-copy, Windows) D3D11VA texture -> shared handle -> D3D12 -> wgpu
  audio      decode audio streams, compare sample counts with ffprobe
  robust     broken files: no panic / crash / hang (--isolate: one child process per file)
  fuzz       --source FILE: deterministic corruptions of a valid file, each run isolated
  all        info + verify + audio + robust + fuzz (bench is separate: it is slow)
  seek       FILE N [N...]: cold seeks to presentation indices (S3_TRACE=1 logs packets/frames)

OPTIONS
  --media DIR        media folder (default: media)
  --out DIR          output folder for truth cache and CSVs (default: s3-out)
  --seeks N          random seeks per file (verify: 500, bench: 200, gpu: 100)
  --seed N           RNG seed (default 0x5EED)
  --threads T        frame | frame:N | slice | slice:N | single   (default frame)
  --no-skip-nonref   decode non-reference frames before the target (default: skip them)
  --hw H             hardware decode: d3d11va[:ADAPTER] | d3d12va[:N] | cuda[:N] | dxva2 | vulkan[:N]
                     | qsv | vaapi[:/dev/dri/renderD128] | videotoolbox | off  (default off)
  --configs LIST     bench/gpu configurations [HW/]THREADS[+skip], e.g.
                     frame+skip,cuda/single+skip,d3d11va:1/frame+skip
  --gpu NAME         wgpu adapter for gpu/zerocopy: name substring (nvidia, radeon), optional
                     @dx12 / @vulkan (default: first discrete GPU, DX12 first)
  --monitor          bench/gpu: sample machine CPU and GPU engine % (monitor.ps1, Windows)
  --dump             gpu: write the rendered middle frame as a PPM to the output folder
  --cases N          fuzz cases per source (default 60)
  --isolate          robust: run each file in a child process
  --quiet            silence FFmpeg's log entirely
";

struct Args {
    cmd: String,
    files: Vec<PathBuf>,
    media: PathBuf,
    out: PathBuf,
    seeks: Option<usize>,
    seed: u64,
    threads: Threads,
    skip_nonref: bool,
    hw: Option<HwSpec>,
    configs: Option<String>,
    gpu: String,
    monitor: bool,
    dump: bool,
    cases: usize,
    isolate: bool,
    quiet: bool,
    source: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().ok_or_else(|| USAGE.to_string())?;
    let mut a = Args {
        cmd,
        files: vec![],
        media: PathBuf::from("media"),
        out: PathBuf::from("s3-out"),
        seeks: None,
        seed: 0x5EED,
        threads: Threads::Frame(0),
        skip_nonref: true,
        hw: None,
        configs: None,
        gpu: String::new(),
        monitor: false,
        dump: false,
        cases: 60,
        isolate: false,
        quiet: false,
        source: None,
    };
    while let Some(arg) = it.next() {
        let mut val = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--media" => a.media = PathBuf::from(val("--media")?),
            "--out" => a.out = PathBuf::from(val("--out")?),
            "--seeks" => {
                a.seeks = Some(
                    val("--seeks")?
                        .parse()
                        .map_err(|e| format!("--seeks: {e}"))?,
                )
            }
            "--seed" => a.seed = parse_u64(&val("--seed")?)?,
            "--threads" => {
                let v = val("--threads")?;
                a.threads = Threads::parse(&v).ok_or(format!("--threads: unknown {v}"))?;
            }
            "--no-skip-nonref" => a.skip_nonref = false,
            "--hw" => a.hw = HwSpec::parse(&val("--hw")?)?,
            "--gpu" => a.gpu = val("--gpu")?,
            "--monitor" => a.monitor = true,
            "--dump" => a.dump = true,
            "--configs" => a.configs = Some(val("--configs")?),
            "--cases" => {
                a.cases = val("--cases")?
                    .parse()
                    .map_err(|e| format!("--cases: {e}"))?
            }
            "--isolate" => a.isolate = true,
            "--quiet" => a.quiet = true,
            "--source" => a.source = Some(PathBuf::from(val("--source")?)),
            "-h" | "--help" => return Err(USAGE.to_string()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}\n\n{USAGE}")),
            s => a.files.push(PathBuf::from(s)),
        }
    }
    Ok(a)
}

fn parse_u64(s: &str) -> Result<u64, String> {
    let r = if let Some(h) = s.strip_prefix("0x") {
        u64::from_str_radix(h, 16)
    } else {
        s.parse()
    };
    r.map_err(|e| format!("bad number {s}: {e}"))
}

/// Files to work on: explicit ones, else manifest entries of the given kinds.
fn targets(a: &Args, kinds: &[&str]) -> Result<Vec<Entry>, String> {
    if !a.files.is_empty() {
        let m = manifest::load(&a.media).unwrap_or_default();
        return Ok(a
            .files
            .iter()
            .map(|f| {
                let name = f
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                m.iter().find(|e| e.name == name).cloned().map_or_else(
                    || Entry {
                        path: f.clone(),
                        name,
                        kind: "video".into(),
                        expect: Default::default(),
                    },
                    |e| Entry {
                        path: f.clone(),
                        ..e
                    },
                )
            })
            .collect());
    }
    let m = manifest::load(&a.media).map_err(|e| {
        format!(
            "cannot read {}/manifest.tsv ({e}); run gen_media.py",
            a.media.display()
        )
    })?;
    Ok(m.into_iter()
        .filter(|e| kinds.contains(&e.kind.as_str()))
        .collect())
}

fn write_csv(out: &Path, name: &str, header: &str, rows: &[String]) {
    let _ = fs::create_dir_all(out);
    let mut s = String::from(header);
    s.push('\n');
    for r in rows {
        s.push_str(r);
        s.push('\n');
    }
    let path = out.join(name);
    match fs::write(&path, s) {
        Ok(()) => println!("wrote {}", path.display()),
        Err(e) => eprintln!("cannot write {}: {e}", path.display()),
    }
}

fn main() -> ExitCode {
    let a = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };
    s3::init(a.quiet || matches!(a.cmd.as_str(), "robust" | "robust-one" | "fuzz"));
    let report = |r: Result<bool, String>| {
        r.unwrap_or_else(|e| {
            eprintln!("error: {e}");
            false
        })
    };
    let ok = match a.cmd.as_str() {
        "info" => report(cmd_info(&a)),
        "verify" => report(cmd_verify(&a)),
        "bench" => report(cmd_bench(&a)),
        "hwinfo" => report(cmd_hwinfo(&a)),
        "gpu" => report(cmd_gpu(&a)),
        "zerocopy" => report(cmd_zerocopy(&a)),
        "audio" => report(cmd_audio(&a)),
        "robust" => report(cmd_robust(&a)),
        "robust-one" => report(cmd_robust_one(&a)),
        "fuzz" => report(cmd_fuzz(&a)),
        "seek" => report(cmd_seek(&a)),
        "all" => {
            let results = [
                report(cmd_info(&a)),
                report(cmd_verify(&a)),
                report(cmd_audio(&a)),
                report(cmd_robust(&Args {
                    isolate: true,
                    ..clone_args(&a)
                })),
                report(cmd_fuzz_default(&a)),
            ];
            println!(
                "all: info {} verify {} audio {} robust {} fuzz {}",
                results[0], results[1], results[2], results[3], results[4]
            );
            results.iter().all(|r| *r)
        }
        other => {
            eprintln!("unknown command {other}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn clone_args(a: &Args) -> Args {
    Args {
        cmd: a.cmd.clone(),
        files: a.files.clone(),
        media: a.media.clone(),
        out: a.out.clone(),
        seeks: a.seeks,
        seed: a.seed,
        threads: a.threads,
        skip_nonref: a.skip_nonref,
        hw: a.hw,
        configs: a.configs.clone(),
        gpu: a.gpu.clone(),
        monitor: a.monitor,
        dump: a.dump,
        cases: a.cases,
        isolate: a.isolate,
        quiet: a.quiet,
        source: a.source.clone(),
    }
}

fn cmd_info(a: &Args) -> Result<bool, String> {
    let mut rows = vec![];
    let mut all_ok = true;
    println!(
        "{:<36} {:<10} {:<12} {:>9} {:>3} {:>5} {:>4} {:<10} {:<10} {:>4} {:>14} {:>8}",
        "file",
        "codec",
        "pix_fmt",
        "size",
        "bit",
        "rot",
        "vfr",
        "primaries",
        "transfer",
        "gop",
        "frame dt",
        "index ms"
    );
    for e in targets(a, &["video", "proxy"])? {
        match probe::info(&e.path) {
            Ok(i) => {
                let mut notes = vec![];
                if let Some(r) = e.get("rotation")
                    && r.parse::<f64>().ok() != Some(i.rotation)
                {
                    notes.push(format!("rotation {} != {r}", i.rotation));
                }
                if e.flag("vfr") != i.vfr {
                    notes.push(format!("vfr {} != expected {}", i.vfr, e.flag("vfr")));
                }
                if let Some(t) = e.get("trc")
                    && !i.transfer.to_lowercase().contains(&t.to_lowercase())
                {
                    notes.push(format!("transfer {} != {t}", i.transfer));
                }
                if let Some(b) = e.get("bitdepth")
                    && b.parse::<u32>().ok() != Some(i.bit_depth)
                {
                    notes.push(format!("bit depth {} != {b}", i.bit_depth));
                }
                // x264 open GOP: the I-frame sits after its leading B-frames, so one interval in
                // presentation order is one frame longer (61 for keyint 60).
                let slack = usize::from(e.flag("opengop"));
                if let Some(g) = e.get("gop")
                    && g.parse::<usize>()
                        .ok()
                        .is_none_or(|g| i.gop.2.abs_diff(g) > slack)
                {
                    notes.push(format!("max GOP {} != {g}", i.gop.2));
                }
                all_ok &= notes.is_empty();
                println!(
                    "{:<36} {:<10} {:<12} {:>9} {:>3} {:>5} {:>4} {:<10} {:<10} {:>4} {:>14} {:>8.1} {}",
                    e.name,
                    i.codec,
                    i.pix_fmt,
                    format!("{}x{}", i.width, i.height),
                    i.bit_depth,
                    i.rotation,
                    if i.vfr { "yes" } else { "no" },
                    i.primaries,
                    i.transfer,
                    i.gop.2,
                    format!(
                        "{}..{} /{}",
                        i.delta_min_max.0, i.delta_min_max.1, i.time_base.1
                    ),
                    i.index_ms,
                    if notes.is_empty() {
                        "ok".to_string()
                    } else {
                        notes.join("; ")
                    }
                );
                rows.push(format!(
                    "{},{},{},{},{},{},{},{},{},{},{},{},{},{}/{},{},{},{},{:.2},{:.1},{}",
                    e.name,
                    i.container.replace(',', ";"),
                    i.codec,
                    i.profile,
                    i.pix_fmt,
                    i.width,
                    i.height,
                    i.bit_depth,
                    i.rotation,
                    i.vfr,
                    i.primaries,
                    i.transfer,
                    i.matrix,
                    i.time_base.0,
                    i.time_base.1,
                    i.start_pts,
                    i.frames,
                    i.gop.2,
                    i.gop.3,
                    i.index_ms,
                    if notes.is_empty() {
                        "ok".to_string()
                    } else {
                        notes.join("; ").replace(',', ";")
                    }
                ));
            }
            Err(err) => {
                all_ok = false;
                println!("{:<36} ERROR {err}", e.name);
                rows.push(format!(
                    "{},ERROR {}",
                    e.name,
                    err.to_string().replace(',', ";")
                ));
            }
        }
    }
    write_csv(
        &a.out,
        "info.csv",
        "file,container,codec,profile,pix_fmt,width,height,bit_depth,rotation_ccw,vfr,primaries,transfer,matrix,time_base,start_pts,frames,gop_max,gop_mean,index_ms,check",
        &rows,
    );
    Ok(all_ok)
}

fn cmd_verify(a: &Args) -> Result<bool, String> {
    let mut rows = vec![];
    let mut all_ok = true;
    let opts = Options {
        threads: a.threads,
        skip_nonref: a.skip_nonref,
        forward_continue: false,
        hw: a.hw,
    };
    println!(
        "verify: config={} seeds={:#x}{}",
        opts.label(),
        a.seed,
        if a.hw.is_some() {
            " (hw: pts + barcode exact, planes bit-exact or PSNR >= 50 dB vs software)"
        } else {
            ""
        }
    );
    for e in targets(a, &["video", "proxy"])? {
        let t = Instant::now();
        let cfg = verify::Config {
            seeks: a.seeks.unwrap_or(500),
            seed: a.seed,
            opts,
            barcode: e.flag("barcode"),
        };
        match verify::verify(&a.out, &e.path, cfg) {
            Ok(r) => {
                all_ok &= r.exact();
                println!(
                    "{:<36} frame-exact: {:<8} | seq {:<9} edges {:<9} scrub {:<9} select {} | index pkts {} pts {} | frames {} gop<={} fwd<={} leading {} vfr {} | {:.1}s",
                    r.file,
                    r.random.show(),
                    r.sequential.show(),
                    r.edges.show(),
                    r.scrub.show(),
                    r.select_spot.show(),
                    if r.packets_match.all() {
                        "=ffprobe".to_string()
                    } else {
                        r.packets_match.show()
                    },
                    if r.frames_match.all() {
                        "=ffprobe".to_string()
                    } else {
                        r.frames_match.show()
                    },
                    r.frames,
                    r.gop_max,
                    r.max_decode_distance,
                    r.open_gop_leading,
                    r.vfr,
                    t.elapsed().as_secs_f64()
                );
                if a.hw.is_some() {
                    println!(
                        "    hw: {} | bit-exact vs sw {} | PSNR min {} over {} non-exact frames",
                        r.hw,
                        r.hw_bit_exact.show(),
                        show_db(r.hw_psnr_min),
                        r.hw_psnr_frames
                    );
                }
                for f in &r.failures {
                    println!("    ! {f}");
                }
                let s = r.stats;
                rows.push(format!(
                    "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                    r.file, r.codec, r.pix_fmt, r.width, r.height, r.frames, r.packets, r.keyframes, r.gop_max, r.max_decode_distance,
                    r.vfr, r.open_gop_leading, r.barcode_checked, r.packets_match.show(), r.frames_match.show(), r.crc_rows_aligned.show(),
                    r.select_spot.show(),
                    r.sequential.show(), r.random.show(), r.edges.show(), r.scrub.show(), s.seeks, s.landing_retries, s.reopens,
                    s.index_mismatches, s.decode_errors, s.corrupt_frames, s.packets_skipped_nonref,
                    r.config, r.hw.replace(',', ";"), s.hw_frames, s.sw_frames, r.hw_bit_exact.show(), show_db(r.hw_psnr_min)
                ));
            }
            Err(err) => {
                all_ok = false;
                println!("{:<36} ERROR {err}", e.name);
                rows.push(format!(
                    "{},ERROR {}",
                    e.name,
                    err.to_string().replace(',', ";")
                ));
            }
        }
    }
    write_csv(
        &a.out,
        "verify.csv",
        "file,codec,pix_fmt,width,height,frames,packets,keyframes,gop_max,max_decode_forward,vfr,open_gop_leading,barcode,index_packets_vs_ffprobe,index_pts_vs_ffprobe,crc_rows_aligned,select_spot_checks,sequential,random_seeks,edge_seeks,scrub,seeks_issued,landing_retries,reopens,index_mismatches,decode_errors,corrupt_frames,packets_skipped_nonref,config,hw,hw_frames,sw_frames,hw_bit_exact_vs_sw,hw_psnr_min_db",
        &rows,
    );
    Ok(all_ok)
}

fn show_db(db: f64) -> String {
    if db.is_infinite() {
        "inf".into()
    } else {
        format!("{db:.2}")
    }
}

/// Monitor columns for one phase: machine CPU, process GPU decode/3d/copy, all-process decode.
fn util_cols(u: &Utilisation) -> String {
    if u.samples == 0 {
        return ",,,,,".into();
    }
    format!(
        "{:.0},{:.0},{:.0},{:.0},{:.0},{}",
        u.cpu_total, u.proc[0], u.proc[1], u.proc[2], u.all[0], u.samples
    )
}

fn util_show(u: &Utilisation) -> String {
    if u.samples == 0 {
        return "no samples".into();
    }
    format!(
        "machine CPU {:.0}% | GPU (this process) decode {:.0}% 3d {:.0}% copy {:.0}% | GPU decode all {:.0}% ({} s)",
        u.cpu_total, u.proc[0], u.proc[1], u.proc[2], u.all[0], u.samples
    )
}

fn cmd_bench(a: &Args) -> Result<bool, String> {
    let configs = a
        .configs
        .clone()
        .unwrap_or_else(|| "frame+skip,frame,slice+skip".into());
    let mut rows = vec![];
    let monitor = if a.monitor {
        Monitor::start(&a.out, "bench")
    } else {
        None
    };
    println!(
        "{:<34} {:<24} {:>5} {:>7} | {:>8} {:>8} {:>8} {:>6} {:>5} {:>5} {:>5} | {:>7} {:>7} | {:>6} {:>6} | {:>7} | {:>5} {:>5}",
        "file",
        "config",
        "gop",
        "open ms",
        "seek p50",
        "p95",
        "max",
        "pkts",
        "delay",
        "out",
        "xfer",
        "scrub50",
        "p95",
        "step50",
        "p95",
        "dec fps",
        "cores",
        "seq"
    );
    for e in targets(a, &["video", "proxy"])? {
        for c in configs.split(',') {
            let opts = Options::parse(c)?;
            match bench::run(&e.path, opts, a.seeks.unwrap_or(200), a.seed, "") {
                Ok(r) => {
                    let cores = |n: &str| r.phase(n).and_then(|p| p.cores_used()).unwrap_or(0.0);
                    println!(
                        "{:<34} {:<24} {:>5} {:>7.1} | {:>8.1} {:>8.1} {:>8.1} {:>6.1} {:>5.1} {:>5.1} {:>5.1} | {:>7.1} {:>7.1} | {:>6.1} {:>6.1} | {:>7.1} | {:>5.2} {:>5.2}",
                        r.file,
                        r.config,
                        r.gop_max,
                        r.open_ms,
                        r.seek.p50,
                        r.seek.p95,
                        r.seek.max,
                        r.packets_per_seek,
                        r.delay_packets,
                        r.frames_out_per_seek,
                        r.transfer_ms,
                        r.scrub.p50,
                        r.scrub.p95,
                        r.step.p50,
                        r.step.p95,
                        r.decode_fps,
                        cores("seek"),
                        cores("sequential"),
                    );
                    if !r.hw.is_empty() {
                        println!("    hw: {}", r.hw);
                    }
                    let mut util = String::new();
                    for name in ["seek", "sequential"] {
                        let u = match (&monitor, r.phase(name)) {
                            (Some(m), Some(p)) => m.utilisation(p),
                            _ => Utilisation::default(),
                        };
                        if monitor.is_some() {
                            println!("    {name:<10} {}", util_show(&u));
                        }
                        util.push(',');
                        util.push_str(&util_cols(&u));
                    }
                    rows.push(format!(
                        "{},{},{},{},{},{},{:.1},{:.1},{},{:.2},{:.2},{:.2},{:.2},{:.1},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.1},{:.2},{:.2},{:.2},{}{}",
                        r.file, r.config, r.width, r.height, r.frames, r.gop_max, r.open_ms, r.index_ms, r.seek.n, r.seek.p50, r.seek.p95,
                        r.seek.max, r.seek.mean, r.packets_per_seek, r.delay_packets, r.frames_out_per_seek, r.transfer_ms,
                        r.scrub.p50, r.scrub.p95, r.scrub.max, r.step.p50, r.step.p95, r.decode_fps, cores("seek"), cores("sequential"),
                        r.hw.replace(',', ";"), util
                    ));
                }
                Err(err) => {
                    println!("{:<34} {:<14} ERROR {err}", e.name, c);
                    rows.push(format!(
                        "{},{},ERROR {}",
                        e.name,
                        c,
                        err.to_string().replace(',', ";")
                    ));
                }
            }
        }
    }
    write_csv(
        &a.out,
        "bench.csv",
        "file,config,width,height,frames,gop_max,open_ms,index_ms,seeks,seek_p50_ms,seek_p95_ms,seek_max_ms,seek_mean_ms,packets_per_seek,delay_packets_per_seek,frames_out_per_seek,transfer_ms_per_seek,scrub_p50_ms,scrub_p95_ms,scrub_max_ms,step_p50_ms,step_p95_ms,decode_fps,cpu_cores_seek,cpu_cores_sequential,hw,seek_machine_cpu_pct,seek_gpu_decode_pct,seek_gpu_3d_pct,seek_gpu_copy_pct,seek_gpu_decode_all_pct,seek_samples,seq_machine_cpu_pct,seq_gpu_decode_pct,seq_gpu_3d_pct,seq_gpu_copy_pct,seq_gpu_decode_all_pct,seq_samples",
        &rows,
    );
    if let Some(m) = monitor {
        m.stop();
    }
    Ok(true)
}

/// `s3 hwinfo`: hwaccel device types in this FFmpeg build, which devices open, wgpu adapters.
fn cmd_hwinfo(_a: &Args) -> Result<bool, String> {
    println!(
        "FFmpeg hwaccel device types: {}",
        hw::compiled_device_types().join(" ")
    );
    let tries: Vec<String> = if cfg!(windows) {
        let mut v = vec![];
        for k in ["d3d11va", "d3d12va", "dxva2", "vulkan"] {
            for i in 0..3 {
                v.push(format!("{k}:{i}"));
            }
        }
        v.extend(["cuda:0", "qsv", "vaapi"].map(String::from));
        v
    } else if cfg!(target_os = "macos") {
        vec!["videotoolbox".into(), "vulkan".into()]
    } else {
        [
            "vaapi",
            "vaapi:/dev/dri/renderD128",
            "vaapi:/dev/dri/renderD129",
            "cuda",
            "vulkan",
        ]
        .map(String::from)
        .to_vec()
    };
    for t in tries {
        let Ok(Some(spec)) = HwSpec::parse(&t) else {
            continue;
        };
        let t0 = Instant::now();
        match hw::Device::create(spec) {
            Ok(_) => println!(
                "  {t:<28} opens ({:.0} ms)",
                t0.elapsed().as_secs_f64() * 1e3
            ),
            Err(e) => println!("  {t:<28} fails: {e}"),
        }
    }
    #[cfg(feature = "gpu")]
    {
        println!("wgpu adapters:");
        for (i, name) in s3::gpu::adapters().iter().enumerate() {
            println!("  {i}: {name}");
        }
    }
    Ok(true)
}

/// `s3 gpu`: path (a) end to end. Per file and configuration:
/// 1. readback check: three frames decoded, uploaded, converted, read back and compared with the
///    CPU conversion (max channel difference) and their barcode read off the rendered RGB;
/// 2. random cold seeks: decode (incl. hw transfer) + upload + convert, texture-ready latency;
/// 3. sequential playback: the same per frame, p50/p95 and fps.
#[cfg(feature = "gpu")]
fn cmd_gpu(a: &Args) -> Result<bool, String> {
    use s3::gpu::{self, Gpu};
    use s3::util::{Rng, percentile};
    let configs = a.configs.clone().unwrap_or_else(|| {
        Options {
            threads: a.threads,
            skip_nonref: a.skip_nonref,
            forward_continue: false,
            hw: a.hw,
        }
        .label()
    });
    let mut g = Gpu::new(&a.gpu)?;
    println!("gpu: {}", g.label());
    let monitor = if a.monitor {
        Monitor::start(&a.out, "gpu")
    } else {
        None
    };
    let mut rows = vec![];
    let mut all_ok = true;
    let p = |v: &[f64], q: f64| percentile(v, q).unwrap_or(0.0);
    for e in targets(a, &["video", "proxy"])? {
        for c in configs.split(',') {
            let opts = Options::parse(c)?;
            let mut src =
                VideoSource::open(&e.path, opts).map_err(|err| format!("{}: {err}", e.name))?;
            let frames = src.index.frames.len();
            let barcode = e.flag("barcode");
            // 1. Readback check.
            let mut check = String::new();
            let mut ok = true;
            let mut rng = Rng::new(a.seed ^ 0x6170);
            for (k, pi) in [0, frames / 2, rng.below(frames)].into_iter().enumerate() {
                src.seek_frame(pi)
                    .map_err(|err| format!("{}: seek {pi}: {err}", e.name))?;
                g.upload(src.frame())?;
                let rgba = g.read_rgba()?;
                let (w, h) = (src.frame().width() as usize, src.frame().height() as usize);
                let (max, share) = gpu::cpu_reference(src.frame())
                    .map_or((255, 1.0), |r| gpu::compare_rgba(&rgba, &r));
                let code = gpu::barcode_from_rgba(&rgba, w, h);
                let good = max <= 2 && (!barcode || code == Some(pi as u32));
                ok &= good;
                let _ = write!(
                    check,
                    "{}f{pi}: max diff {max} ({:.4}% > 1), barcode {code:?}",
                    if k > 0 { "; " } else { "" },
                    share * 100.0
                );
                if a.dump && k == 1 {
                    let name = format!(
                        "gpu-{}-{}.ppm",
                        e.name.replace('.', "_"),
                        c.replace([':', '/', '+'], "_")
                    );
                    let _ = gpu::write_ppm(&a.out.join(name), &rgba, w, h);
                }
            }
            all_ok &= ok;
            let pix = format!("{:?}", src.frame().format());
            // 2. Random cold seeks to texture-ready.
            let n = a.seeks.unwrap_or(100);
            let (mut dec, mut up, mut tot) = (vec![], vec![], vec![]);
            let mut rng = Rng::new(a.seed);
            let seek_clock = s3::monitor::PhaseClock::start("seek");
            for _ in 0..n {
                let pi = rng.below(frames);
                let t = Instant::now();
                src.seek_frame(pi)
                    .map_err(|err| format!("{}: seek {pi}: {err}", e.name))?;
                let d = t.elapsed().as_secs_f64() * 1e3;
                let u = g.upload(src.frame())?;
                dec.push(d);
                up.push(u.total_ms);
                tot.push(t.elapsed().as_secs_f64() * 1e3);
            }
            let seek_phase = seek_clock.stop();
            // 3. Sequential playback to texture-ready.
            src.rewind().map_err(|err| err.to_string())?;
            let (mut sdec, mut sup, mut stot) = (vec![], vec![], vec![]);
            let seq_clock = s3::monitor::PhaseClock::start("sequential");
            let t_all = Instant::now();
            loop {
                let t = Instant::now();
                if !src.next_frame().map_err(|err| err.to_string())? {
                    break;
                }
                let d = t.elapsed().as_secs_f64() * 1e3;
                let u = g.upload(src.frame())?;
                sdec.push(d);
                sup.push(u.total_ms);
                stot.push(t.elapsed().as_secs_f64() * 1e3);
            }
            let fps = sdec.len() as f64 / t_all.elapsed().as_secs_f64();
            let seq_phase = seq_clock.stop();
            println!(
                "{:<34} {:<24} {:<10} seek: decode p50 {:>6.1} p95 {:>6.1} | upload+convert p50 {:>5.1} p95 {:>5.1} | texture-ready p50 {:>6.1} p95 {:>6.1} ms",
                e.name,
                c,
                pix,
                p(&dec, 50.0),
                p(&dec, 95.0),
                p(&up, 50.0),
                p(&up, 95.0),
                p(&tot, 50.0),
                p(&tot, 95.0)
            );
            println!(
                "{:<34} {:<24} {:<10} play: decode p50 {:>6.1} p95 {:>6.1} | upload+convert p50 {:>5.1} p95 {:>5.1} | per frame p50 {:>6.1} p95 {:>6.1} ms | {:.1} fps | CPU cores seek {:.2} play {:.2}",
                "",
                "",
                "",
                p(&sdec, 50.0),
                p(&sdec, 95.0),
                p(&sup, 50.0),
                p(&sup, 95.0),
                p(&stot, 50.0),
                p(&stot, 95.0),
                fps,
                seek_phase.cores_used().unwrap_or(0.0),
                seq_phase.cores_used().unwrap_or(0.0)
            );
            println!(
                "    render check: {check} -> {}",
                if ok { "ok" } else { "MISMATCH" }
            );
            if opts.hw.is_some() {
                println!("    hw: {}", src.hw_summary());
            }
            let mut util = String::new();
            for ph in [&seek_phase, &seq_phase] {
                let u = monitor
                    .as_ref()
                    .map_or_else(Utilisation::default, |m| m.utilisation(ph));
                if monitor.is_some() {
                    println!("    {:<10} {}", ph.name, util_show(&u));
                }
                util.push(',');
                util.push_str(&util_cols(&u));
            }
            rows.push(format!(
                "{},{},{},{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.1},{:.2},{:.2},{},{}{}",
                e.name, c, g.label(), pix, n, p(&dec, 50.0), p(&dec, 95.0), p(&up, 50.0), p(&up, 95.0), p(&tot, 50.0), p(&tot, 95.0),
                p(&sdec, 50.0), p(&sdec, 95.0), p(&sup, 50.0), p(&sup, 95.0), p(&stot, 50.0), p(&stot, 95.0), fps,
                seek_phase.cores_used().unwrap_or(0.0), seq_phase.cores_used().unwrap_or(0.0), ok,
                src.hw_summary().replace(',', ";"), util
            ));
        }
    }
    write_csv(
        &a.out,
        "gpu.csv",
        "file,config,adapter,pix_fmt,seeks,seek_decode_p50_ms,seek_decode_p95_ms,seek_upload_p50_ms,seek_upload_p95_ms,seek_texture_ready_p50_ms,seek_texture_ready_p95_ms,play_decode_p50_ms,play_decode_p95_ms,play_upload_p50_ms,play_upload_p95_ms,play_frame_p50_ms,play_frame_p95_ms,play_fps,cpu_cores_seek,cpu_cores_play,render_check,hw,seek_machine_cpu_pct,seek_gpu_decode_pct,seek_gpu_3d_pct,seek_gpu_copy_pct,seek_gpu_decode_all_pct,seek_samples,play_machine_cpu_pct,play_gpu_decode_pct,play_gpu_3d_pct,play_gpu_copy_pct,play_gpu_decode_all_pct,play_samples",
        &rows,
    );
    if let Some(m) = monitor {
        m.stop();
    }
    Ok(all_ok)
}

#[cfg(not(feature = "gpu"))]
fn cmd_gpu(_a: &Args) -> Result<bool, String> {
    Err("built without the `gpu` feature".into())
}

#[cfg(all(feature = "zero-copy", windows))]
fn cmd_zerocopy(a: &Args) -> Result<bool, String> {
    let files: Vec<Entry> = targets(a, &["video"])?;
    s3::zerocopy::run(
        &files.iter().map(|e| e.path.clone()).collect::<Vec<_>>(),
        &a.gpu,
        a.hw,
        a.seeks.unwrap_or(100),
        a.seed,
        &a.out,
    )
}

#[cfg(not(all(feature = "zero-copy", windows)))]
fn cmd_zerocopy(_a: &Args) -> Result<bool, String> {
    Err("built without the `zero-copy` feature (Windows only): cargo build --release --features zero-copy".into())
}

fn cmd_audio(a: &Args) -> Result<bool, String> {
    let mut rows = vec![];
    let mut all_ok = true;
    let mut list = targets(a, &["audio"])?;
    if a.files.is_empty() {
        // Video files with an audio track.
        list.extend(
            targets(a, &["video"])?
                .into_iter()
                .filter(|e| e.name.contains("phone") || e.name.contains("2160p30_gop30")),
        );
    }
    for e in list {
        match probe::audio(&e.path) {
            Ok(r) => {
                let mut ok = r.samples == r.ffprobe_samples
                    && u64::from(r.rate) == r.ffprobe_rate
                    && u64::from(r.channels) == r.ffprobe_channels;
                if let Some(rate) = e.get("rate") {
                    ok &= rate.parse::<u32>().ok() == Some(r.rate);
                }
                all_ok &= ok;
                println!(
                    "{:<36} {:<10} {:>6} Hz {} ch {:<6} samples {:>9} (ffprobe {:>9}) errors {} {}",
                    e.name,
                    r.codec,
                    r.rate,
                    r.channels,
                    r.sample_fmt,
                    r.samples,
                    r.ffprobe_samples,
                    r.decode_errors,
                    if ok { "ok" } else { "MISMATCH" }
                );
                rows.push(format!(
                    "{},{},{},{},{},{},{},{},{}",
                    e.name,
                    r.codec,
                    r.rate,
                    r.channels,
                    r.sample_fmt,
                    r.samples,
                    r.ffprobe_samples,
                    r.decode_errors,
                    ok
                ));
            }
            Err(err) => {
                all_ok = false;
                println!("{:<36} ERROR {err}", e.name);
            }
        }
    }
    write_csv(
        &a.out,
        "audio.csv",
        "file,codec,rate,channels,sample_fmt,samples,ffprobe_samples,decode_errors,match",
        &rows,
    );
    Ok(all_ok)
}

/// Runs `s3 robust-one FILE` in a child process with a timeout.
fn isolated(path: &Path, timeout: Duration) -> Outcome {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            return Outcome {
                file: name,
                error: Some(format!("current_exe: {e}")),
                ..Outcome::default()
            };
        }
    };
    let child = Command::new(exe)
        .arg("robust-one")
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return Outcome {
                file: name,
                error: Some(format!("spawn: {e}")),
                ..Outcome::default()
            };
        }
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(mut so) = child.stdout.take() {
                    let _ = std::io::Read::read_to_string(&mut so, &mut out);
                }
                if let Some(line) = out.lines().find_map(|l| l.strip_prefix("ROBUST,")) {
                    return parse_outcome(line);
                }
                return Outcome {
                    file: name,
                    stage: "crash".into(),
                    panicked: Some(format!("child exited without a result: {status}")),
                    ms: started.elapsed().as_secs_f64() * 1e3,
                    ..Outcome::default()
                };
            }
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Outcome {
                    file: name,
                    stage: "hang".into(),
                    panicked: Some(format!("killed after {timeout:?}")),
                    ..Outcome::default()
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                return Outcome {
                    file: name,
                    error: Some(format!("wait: {e}")),
                    ..Outcome::default()
                };
            }
        }
    }
}

fn parse_outcome(line: &str) -> Outcome {
    let c: Vec<&str> = line.split(',').collect();
    let g = |i: usize| c.get(i).copied().unwrap_or("");
    let n = |i: usize| g(i).parse().unwrap_or(0);
    Outcome {
        file: g(0).into(),
        bytes: n(1) as u64,
        stage: g(2).into(),
        error: Some(g(3).to_string()).filter(|s| !s.is_empty()),
        frames_indexed: n(4),
        frames_decoded: n(5),
        seeks_ok: n(6),
        seeks_err: n(7),
        decode_errors: n(8) as u64,
        demux_errors: n(9) as u64,
        panicked: Some(g(10).to_string()).filter(|s| !s.is_empty()),
        ms: g(11).parse().unwrap_or(0.0),
        corrupt_frames: n(12) as u64,
    }
}

fn print_outcome(o: &Outcome) {
    let verdict = if o.panicked.is_some() { "FAIL" } else { "ok" };
    println!(
        "{:<44} {:>10} B  stage {:<6} indexed {:>5} decoded {:>5} seeks {:>2}/{:<2} dec-err {:>4} concealed {:>4} demux-err {:>4} {:>7.0} ms  {verdict}  {}",
        o.file,
        o.bytes,
        o.stage,
        o.frames_indexed,
        o.frames_decoded,
        o.seeks_ok,
        o.seeks_ok + o.seeks_err,
        o.decode_errors,
        o.corrupt_frames,
        o.demux_errors,
        o.ms,
        o.panicked.as_deref().or(o.error.as_deref()).unwrap_or("")
    );
}

fn cmd_robust(a: &Args) -> Result<bool, String> {
    let mut rows = vec![];
    let mut all_ok = true;
    for e in targets(a, &["broken"])? {
        let o = if a.isolate {
            isolated(&e.path, Duration::from_secs(120))
        } else {
            robust::check(&e.path)
        };
        all_ok &= o.panicked.is_none();
        print_outcome(&o);
        rows.push(o.line());
    }
    write_csv(&a.out, "robust.csv", Outcome::HEADER, &rows);
    Ok(all_ok)
}

/// `s3 seek FILE N [N...]`: cold seeks to presentation indices (debugging; `S3_TRACE=1` logs
/// every packet and frame).
fn cmd_seek(a: &Args) -> Result<bool, String> {
    let (file, targets) = a.files.split_first().ok_or("seek needs FILE N...")?;
    let opts = Options {
        threads: a.threads,
        skip_nonref: a.skip_nonref,
        forward_continue: false,
        hw: a.hw,
    };
    let mut src = VideoSource::open(file, opts).map_err(|e| e.to_string())?;
    let mut ok = true;
    if a.hw.is_some() {
        eprintln!("hw: {}", src.hw_status().show());
    }
    for t in targets {
        let pi: usize = t
            .to_string_lossy()
            .parse()
            .map_err(|e| format!("{t:?}: {e}"))?;
        let f = src
            .index
            .frames
            .get(pi)
            .copied()
            .ok_or(format!("no frame {pi}"))?;
        eprintln!(
            "seek {pi}: pts {} start packet {} (decode idx {})",
            f.pts, f.start_idx, f.decode_idx
        );
        match src.seek_frame(pi) {
            Ok(()) => println!(
                "{pi}: ok pts {:?} {:?}",
                src.frame().pts(),
                src.frame().format()
            ),
            Err(e) => {
                ok = false;
                println!("{pi}: {e}");
            }
        }
    }
    if a.hw.is_some() {
        eprintln!("hw: {}", src.hw_summary());
    }
    Ok(ok)
}

fn cmd_robust_one(a: &Args) -> Result<bool, String> {
    let f = a.files.first().ok_or("robust-one needs a file")?;
    let o = robust::check(f);
    println!("ROBUST,{}", o.line());
    Ok(o.panicked.is_none())
}

fn cmd_fuzz_default(a: &Args) -> Result<bool, String> {
    let mut ok = true;
    for src in [
        "h264_1080p23976_gop48.mp4",
        "h264_1080p25_gop50.mkv",
        "hevc_main10_pq_1080p24.mp4",
        "h264_1080p25_gop25.ts",
    ] {
        let mut b = clone_args(a);
        b.source = Some(a.media.join(src));
        b.cases = a.cases.min(40);
        ok &= cmd_fuzz(&b)?;
    }
    Ok(ok)
}

fn cmd_fuzz(a: &Args) -> Result<bool, String> {
    let src = a.source.clone().ok_or("fuzz needs --source FILE")?;
    let data = fs::read(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    let dir = a.out.join("fuzz");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stem = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut rows = vec![];
    let (mut fails, mut errors, mut clean) = (0, 0, 0);
    let mut summary = String::new();
    for (i, (label, bytes)) in robust::mutate(&data, a.cases, a.seed).enumerate() {
        let path = dir.join(format!("{i:03}_{stem}"));
        fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        let mut o = isolated(&path, Duration::from_secs(120));
        o.file = format!("{i:03} {label}");
        if o.panicked.is_some() {
            fails += 1;
            print_outcome(&o);
        } else {
            let _ = fs::remove_file(&path);
            if o.error.is_some() {
                errors += 1
            } else {
                clean += 1
            }
        }
        rows.push(format!("{stem},{}", o.line()));
    }
    let _ = writeln!(
        summary,
        "fuzz {stem}: {} cases, {clean} decoded cleanly, {errors} ended in a typed error, {fails} panics/crashes/hangs",
        a.cases
    );
    print!("{summary}");
    let name = format!("fuzz_{}.csv", stem.replace('.', "_"));
    write_csv(&a.out, &name, &format!("source,{}", Outcome::HEADER), &rows);
    Ok(fails == 0)
}
