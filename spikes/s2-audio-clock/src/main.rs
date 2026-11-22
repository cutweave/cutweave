// SPDX-License-Identifier: GPL-3.0-or-later
//! `s2-audio-clock`: run the Spike S2 harness (see README.md).

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use s2_audio_clock::engine::{Engine, EngineConfig, list_devices};
use s2_audio_clock::mixer::TrackParams;
use s2_audio_clock::scene::{Scene, tracks};
use s2_audio_clock::soak::{LoggerCtx, RunTotals, spawn_logger};
use s2_audio_clock::util::parse_duration;
use s2_audio_clock::video::{
    Hud, PresenterLogs, VideoConfig, VideoStats, VideoSummary, run_presenter, spawn_log_writer,
};

const USAGE: &str = "\
s2-audio-clock — Cutweave Spike S2: cpal mixer + stretch + loudness, video follows the audio clock

USAGE: s2-audio-clock [OPTIONS]

  --scene <mix|sync|ref-tone>  mix: 8 tracks incl. click (default); sync: loud click only;
                               ref-tone: -23 LUFS 1 kHz reference through the whole chain
  --tracks <n>                 limit the mix scene to the first n tracks (default 8)
  --buffer <frames|default>    requested frames per callback (default 256)
  --duration <dur>             stop after dur (e.g. 10s, 2m); default: until Esc / 'q'+Enter
  --soak <dur>                 soak run of dur (e.g. 30m): headless unless --window
  --headless                   terminal only (no window); --window forces a window
  --fps <n>                    video timeline frame rate (default 60)
  --click-interval <dur>       seconds between click/flash pairs (default 1s)
  --av-offset-ms <ms>          show video this much earlier (+) / later (-) than the audio clock
  --no-latency-comp            ignore cpal's reported output latency in the clock
  --ring-ms <ms>               per-track ring capacity (default 250)
  --workers <n>                decode/stretch worker threads (default 2)
  --limiter-ceiling <dBFS>     master limiter ceiling (default -1.0)
  --cheaper-stretch            signalsmith 'cheaper' preset
  --no-automation              keep gain/pan static (mix scene automates a few params)
  --device <substring>         output device by name (see --list-devices)
  --list-devices               list output devices and exit
  --out <dir>                  output directory (default s2-out/<scene>-<unix time>)
  --log-interval <dur>         CSV/console row interval (default 1s)
  --frame-log                  also write <out>/frames.csv (one row per presented frame)
  -h, --help                   this help

Outputs: <out>/soak.csv (one row per interval), <out>/av_events.csv (one row per flash/click),
<out>/summary.json. Quit with Esc in the window or 'q' + Enter in the terminal.";

#[derive(Debug)]
struct Args {
    scene: Scene,
    tracks: usize,
    buffer: Option<u32>,
    duration: Option<Duration>,
    soak: bool,
    window: Option<bool>,
    fps: u32,
    click_interval: f64,
    av_offset_ms: f64,
    latency_comp: bool,
    ring_ms: u32,
    workers: usize,
    limiter_ceiling: f32,
    cheaper: bool,
    automation: bool,
    device: Option<String>,
    list_devices: bool,
    out: Option<PathBuf>,
    log_interval: Duration,
    frame_log: bool,
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut a = Args {
        scene: Scene::Mix,
        tracks: 8,
        buffer: Some(256),
        duration: None,
        soak: false,
        window: None,
        fps: 60,
        click_interval: 1.0,
        av_offset_ms: 0.0,
        latency_comp: true,
        ring_ms: 250,
        workers: 2,
        limiter_ceiling: -1.0,
        cheaper: false,
        automation: true,
        device: None,
        list_devices: false,
        out: None,
        log_interval: Duration::from_secs(1),
        frame_log: false,
    };
    fn val(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
        it.next().ok_or_else(|| format!("{flag} needs a value"))
    }
    fn num<T: std::str::FromStr>(s: &str, flag: &str) -> Result<T, String> {
        s.parse()
            .map_err(|_| format!("{flag}: invalid value `{s}`"))
    }
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--scene" => a.scene = Scene::parse(&val(&mut it, &arg)?)?,
            "--tracks" => a.tracks = num(&val(&mut it, &arg)?, &arg)?,
            "--buffer" => {
                let v = val(&mut it, &arg)?;
                a.buffer = if v == "default" {
                    None
                } else {
                    Some(num(&v, &arg)?)
                };
            }
            "--duration" => a.duration = Some(parse_duration(&val(&mut it, &arg)?)?),
            "--soak" => {
                a.duration = Some(parse_duration(&val(&mut it, &arg)?)?);
                a.soak = true;
            }
            "--headless" => a.window = Some(false),
            "--window" => a.window = Some(true),
            "--fps" => a.fps = num(&val(&mut it, &arg)?, &arg)?,
            "--click-interval" => {
                a.click_interval = parse_duration(&val(&mut it, &arg)?)?.as_secs_f64();
            }
            "--av-offset-ms" => a.av_offset_ms = num(&val(&mut it, &arg)?, &arg)?,
            "--no-latency-comp" => a.latency_comp = false,
            "--ring-ms" => a.ring_ms = num(&val(&mut it, &arg)?, &arg)?,
            "--workers" => a.workers = num(&val(&mut it, &arg)?, &arg)?,
            "--limiter-ceiling" => a.limiter_ceiling = num(&val(&mut it, &arg)?, &arg)?,
            "--cheaper-stretch" => a.cheaper = true,
            "--no-automation" => a.automation = false,
            "--device" => a.device = Some(val(&mut it, &arg)?),
            "--list-devices" => a.list_devices = true,
            "--out" => a.out = Some(PathBuf::from(val(&mut it, &arg)?)),
            "--log-interval" => a.log_interval = parse_duration(&val(&mut it, &arg)?)?,
            "--frame-log" => a.frame_log = true,
            other => return Err(format!("unknown option `{other}` (see --help)")),
        }
    }
    if a.fps == 0 || a.click_interval <= 0.0 || a.ring_ms < 20 || a.log_interval.is_zero() {
        return Err("--fps, --click-interval, --log-interval must be > 0; --ring-ms >= 20".into());
    }
    Ok(Some(a))
}

/// Slow LFOs on a few mix parameters so the lock-free parameter path is exercised.
fn spawn_automation(params: Vec<Arc<TrackParams>>, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("s2-automation".into())
        .spawn(move || {
            let t0 = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                let t = t0.elapsed().as_secs_f32();
                if let Some(p) = params.get(1) {
                    p.pan.store(-0.5 + 0.4 * (t * 0.7).sin());
                }
                if let Some(p) = params.get(2) {
                    p.pan.store(0.5 * (t * 0.3).cos());
                }
                if let Some(p) = params.get(4) {
                    p.gain_db.store(-24.0 + 3.0 * (t * 0.5).sin());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        })
        .expect("spawn automation thread");
}

/// Stops the run when the user types `q` + Enter.
fn spawn_stdin_quit(stop: Arc<AtomicBool>) {
    let _ = std::thread::Builder::new()
        .name("s2-stdin".into())
        .spawn(move || {
            let mut line = String::new();
            while std::io::stdin().read_line(&mut line).is_ok_and(|n| n > 0) {
                if line.trim().eq_ignore_ascii_case("q") {
                    stop.store(true, Ordering::Relaxed);
                    return;
                }
                line.clear();
            }
        });
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn json_num(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.4}")
    } else {
        "null".into()
    }
}

#[allow(clippy::too_many_lines)]
fn summarize(
    args: &Args,
    engine: &Engine,
    vstats: &VideoStats,
    video: &VideoSummary,
    totals: &RunTotals,
) -> (String, Vec<String>) {
    let cb = &engine.cb_stats;
    let mix = &engine.mix_stats;
    let info = &engine.info;
    let ld = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
    // Drift rate from a least-squares fit over all rows (end-point ratios are jitter-dominated).
    let ppm = if totals.rows >= 2 {
        totals.drift_ppm_fit
    } else {
        f64::NAN
    };
    let startup = |arr: &[std::sync::atomic::AtomicU64], scale: f64| {
        let v: Vec<String> = arr
            .iter()
            .take(usize::try_from(ld(&cb.callbacks)).unwrap_or(usize::MAX))
            .map(|a| json_num(a.load(Ordering::Relaxed) as f64 / scale))
            .collect();
        format!("[{}]", v.join(", "))
    };
    let frames_per_30m = ppm * 1e-6 * 1800.0 * f64::from(args.fps);
    let offs = &video.offsets_ms;
    let (off_mean, off_min, off_max) = if offs.is_empty() {
        (f64::NAN, f64::NAN, f64::NAN)
    } else {
        (
            offs.iter().sum::<f64>() / offs.len() as f64,
            offs.iter().copied().fold(f64::INFINITY, f64::min),
            offs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        )
    };
    let lufs_i = f64::from(engine.loudness.integrated.load());
    let lat_min = ld(&cb.latency_ns_min_all);
    let lat_min_ms = if lat_min == u64::MAX {
        f64::NAN
    } else {
        lat_min as f64 / 1e6
    };
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    let mut f: Vec<(&str, String)> = vec![
        ("spike", json_str("S2 audio clock (WP-0D.01)")),
        ("informal", "true".into()),
        ("unix_time", unix.to_string()),
        ("os", json_str(std::env::consts::OS)),
        (
            "build",
            json_str(if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }),
        ),
        ("scene", json_str(args.scene.name())),
        ("tracks", engine.params.len().to_string()),
        ("host", json_str(&info.host)),
        ("device", json_str(&info.device)),
        ("sample_rate", info.sample_rate.to_string()),
        ("channels", info.channels.to_string()),
        ("sample_format", json_str(&info.format)),
        ("buffer_requested", json_str(&info.buffer_requested)),
        ("buffer_used", json_str(&info.buffer_used)),
        ("supported_buffer", json_str(&info.supported_buffer)),
        (
            "cpal_buffer_size",
            info.cpal_buffer_size
                .map_or("null".into(), |b| b.to_string()),
        ),
        ("callback_frames_min", ld(&cb.frames_min_all).to_string()),
        ("callback_frames_max", ld(&cb.frames_max_all).to_string()),
        ("callbacks", ld(&cb.callbacks).to_string()),
        ("duration_s", json_num(totals.wall_s)),
        ("latency_comp", args.latency_comp.to_string()),
        ("latency_ms_min", json_num(lat_min_ms)),
        ("latency_ms_mean", json_num(totals.latency_ms_mean)),
        (
            "latency_ms_max",
            json_num(ld(&cb.latency_ns_max_all) as f64 / 1e6),
        ),
        ("startup_callback_frames", startup(&cb.startup_frames, 1.0)),
        ("startup_latency_ms", startup(&cb.startup_latency_ns, 1e6)),
        (
            "warmup_ts_err_ms_max",
            json_num(ld(&cb.warmup_ts_err_ns_max) as f64 / 1e6),
        ),
        ("drift_ms_end", json_num(totals.drift_ms_end)),
        ("drift_ms_fit_end", json_num(totals.drift_ms_fit_end)),
        ("drift_ms_max_abs", json_num(totals.drift_ms_max_abs)),
        ("drift_jitter_ms_max", json_num(totals.drift_jitter_ms_max)),
        ("drift_ppm_fit", json_num(ppm)),
        (
            "drift_frames_per_30min_extrapolated",
            json_num(frames_per_30m),
        ),
        (
            "video_minus_audio_frames_max_abs",
            totals.video_minus_audio_max_abs.to_string(),
        ),
        (
            "track_underrun_events",
            ld(&mix.underrun_events).to_string(),
        ),
        (
            "track_underrun_frames",
            ld(&mix.underrun_frames).to_string(),
        ),
        ("device_glitches", ld(&cb.glitches).to_string()),
        ("late_callbacks", ld(&cb.late_callbacks).to_string()),
        ("cpal_xruns", ld(&cb.xruns).to_string()),
        ("cpal_errors", ld(&cb.errors).to_string()),
        ("rt_allocs", ld(&cb.rt_allocs).to_string()),
        (
            "callback_dur_us_max",
            json_num(ld(&cb.dur_ns_max_all) as f64 / 1e3),
        ),
        (
            "callback_load_max_pct",
            json_num(ld(&cb.load_permille_max_all) as f64 / 10.0),
        ),
        (
            "worker_load_mean_pct",
            json_num(totals.worker_load_mean_pct),
        ),
        ("worker_load_max_pct", json_num(totals.worker_load_max_pct)),
        ("ring_min_ms", json_num(totals.ring_min_ms)),
        ("limiter_gr_db_max", json_num(totals.limiter_gr_db_max)),
        ("lufs_integrated", json_num(lufs_i)),
        (
            "true_peak_dbtp",
            json_num(f64::from(engine.loudness.true_peak_dbtp.load())),
        ),
        ("video_fps", args.fps.to_string()),
        ("video_presented", ld(&vstats.presented).to_string()),
        ("video_skipped", ld(&vstats.skipped).to_string()),
        ("flashes", ld(&vstats.flashes).to_string()),
        ("late_flashes", ld(&vstats.late_flashes).to_string()),
        ("av_offset_setting_ms", json_num(args.av_offset_ms)),
        ("sw_av_offset_ms_mean", json_num(off_mean)),
        ("sw_av_offset_ms_min", json_num(off_min)),
        ("sw_av_offset_ms_max", json_num(off_max)),
    ];
    if args.scene == Scene::RefTone {
        let pass = (lufs_i + 23.0).abs() <= 0.1;
        f.push(("ref_tone_pass", pass.to_string()));
    }
    let mut json = String::from("{\n");
    for (i, (k, v)) in f.iter().enumerate() {
        let sep = if i + 1 == f.len() { "" } else { "," };
        let _ = writeln!(json, "  \"{k}\": {v}{sep}");
    }
    json.push_str("}\n");

    let mut lines = vec![
        format!(
            "device: {} / {} @ {} Hz, {} ch, {} | buffer requested {} → used {}, cpal reports {:?}, callbacks {}..={} frames",
            info.host,
            info.device,
            info.sample_rate,
            info.channels,
            info.format,
            info.buffer_requested,
            info.buffer_used,
            info.cpal_buffer_size,
            ld(&cb.frames_min_all),
            ld(&cb.frames_max_all)
        ),
        format!(
            "duration {:.1} s | drift (audio - wall) end {:+.3} ms, fit {:+.2} ppm ≈ {:+.3} frames/30 min @ {} fps (extrapolated) | max |{:.3}| ms, anchor jitter ≤ {:.3} ms",
            totals.wall_s,
            totals.drift_ms_end,
            ppm,
            frames_per_30m,
            args.fps,
            totals.drift_ms_max_abs,
            totals.drift_jitter_ms_max
        ),
        format!(
            "start-up: first callbacks {} frames, latency {} ms; warm-up timestamp jump {:.2} ms (not counted)",
            startup(&cb.startup_frames, 1.0),
            startup(&cb.startup_latency_ns, 1e6),
            ld(&cb.warmup_ts_err_ns_max) as f64 / 1e6
        ),
        format!(
            "underruns: track {} events / {} frames | device glitches {} | late callbacks {} | cpal xruns {} errors {} | rt allocs {}",
            ld(&mix.underrun_events),
            ld(&mix.underrun_frames),
            ld(&cb.glitches),
            ld(&cb.late_callbacks),
            ld(&cb.xruns),
            ld(&cb.errors),
            ld(&cb.rt_allocs)
        ),
        format!(
            "latency (cpal playback-callback) {:.2}..{:.2} ms, mean {:.2} ms | callback max {:.0} us ({:.1}% of buffer) | workers mean {:.1}% max {:.1}% | ring min {:.0} ms",
            lat_min_ms,
            ld(&cb.latency_ns_max_all) as f64 / 1e6,
            totals.latency_ms_mean,
            ld(&cb.dur_ns_max_all) as f64 / 1e3,
            ld(&cb.load_permille_max_all) as f64 / 10.0,
            totals.worker_load_mean_pct,
            totals.worker_load_max_pct,
            totals.ring_min_ms
        ),
        format!(
            "video: {} presented, {} skipped, max |video-audio| {} frames | flashes {} (late {}) | software A/V offset (present - audible) mean {:.2} ms, {:.2}..{:.2} ms",
            ld(&vstats.presented),
            ld(&vstats.skipped),
            totals.video_minus_audio_max_abs,
            ld(&vstats.flashes),
            ld(&vstats.late_flashes),
            off_mean,
            off_min,
            off_max
        ),
        format!(
            "loudness: integrated {:.2} LUFS, true peak {:.2} dBTP, limiter max GR {:.2} dB",
            lufs_i,
            engine.loudness.true_peak_dbtp.load(),
            totals.limiter_gr_db_max
        ),
    ];
    if args.scene == Scene::RefTone {
        let pass = (lufs_i + 23.0).abs() <= 0.1;
        lines.push(format!(
            "ref-tone: {lufs_i:.2} LUFS vs -23.0 ±0.1 → {}",
            if pass { "PASS" } else { "FAIL" }
        ));
    }
    (json, lines)
}

fn run(args: &Args) -> Result<(), String> {
    let epoch = Instant::now();
    let out = args.out.clone().unwrap_or_else(|| {
        let unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        PathBuf::from(format!("s2-out/{}-{unix}", args.scene.name()))
    });
    fs::create_dir_all(&out).map_err(|e| format!("create {}: {e}", out.display()))?;

    let specs = tracks(args.scene, args.click_interval, args.tracks);
    let cfg = EngineConfig {
        specs: specs.clone(),
        buffer: args.buffer,
        ring_ms: args.ring_ms,
        workers: args.workers,
        latency_comp: args.latency_comp,
        limiter_ceiling_db: args.limiter_ceiling,
        device: args.device.clone(),
        cheaper_stretch: args.cheaper,
    };
    let engine = Engine::start(&cfg, epoch)?;
    let info = &engine.info;
    println!(
        "S2 audio clock | {} / {} | {} Hz {} ch {} | buffer requested {} → {} (device range {}, cpal buffer_size {:?})",
        info.host,
        info.device,
        info.sample_rate,
        info.channels,
        info.format,
        info.buffer_requested,
        info.buffer_used,
        info.supported_buffer,
        info.cpal_buffer_size
    );
    for (s, (name, lat)) in specs.iter().zip(&info.track_latency) {
        println!(
            "  track {name:6} src {:6} Hz speed {:?} stretch {} | resampler delay {} f, stretch latency in {} / out {} f",
            s.src_rate, s.speed, s.stretch, lat.resampler_delay, lat.stretch_in, lat.stretch_out
        );
    }
    println!(
        "  limiter look-ahead {} f | output: {}",
        info.limiter_latency,
        out.display()
    );

    let stop = Arc::new(AtomicBool::new(false));
    let vstats = Arc::new(VideoStats::default());
    let logger = spawn_logger(
        LoggerCtx {
            clock: engine.clock.clone(),
            cb: engine.cb_stats.clone(),
            mix: engine.mix_stats.clone(),
            loud: engine.loudness.clone(),
            video: vstats.clone(),
            workers: engine.worker_stats.clone(),
            fps: args.fps,
            interval: args.log_interval,
            csv: out.join("soak.csv"),
        },
        stop.clone(),
    );
    if args.automation && args.scene == Scene::Mix {
        spawn_automation(engine.params.clone(), stop.clone());
    }
    spawn_stdin_quit(stop.clone());

    let window = args.window.unwrap_or(!args.soak);
    let vcfg = VideoConfig {
        fps: args.fps,
        width: 640,
        height: 360,
        av_offset_ms: args.av_offset_ms,
        click_interval: args.click_interval,
        window,
    };
    let hud = Hud {
        clock: &engine.clock,
        cb: &engine.cb_stats,
        mix: &engine.mix_stats,
        loud: &engine.loudness,
        info: format!(
            "{} {} HZ BUF {} TRACKS {}",
            info.host,
            info.sample_rate,
            info.buffer_used,
            specs.len()
        ),
    };
    let create = |name: &str| {
        let p = out.join(name);
        fs::File::create(&p).map_err(|e| format!("create {}: {e}", p.display()))
    };
    let av = create("av_events.csv")?;
    let frames = if args.frame_log {
        Some(create("frames.csv")?)
    } else {
        None
    };
    let (tx, log_writer) = spawn_log_writer(av, frames);
    let logs = PresenterLogs {
        tx,
        frames: args.frame_log,
    };
    let deadline = args.duration.map(|d| Instant::now() + d);
    let video = run_presenter(&vcfg, &hud, &vstats, &logs, &stop, deadline);
    drop(logs);
    let _ = log_writer.join();
    stop.store(true, Ordering::Relaxed);
    let totals = logger
        .join()
        .map_err(|_| "logger thread panicked".to_string())??;
    let video = video?;

    let (json, lines) = summarize(args, &engine, &vstats, &video, &totals);
    engine.shutdown();
    fs::write(out.join("summary.json"), json).map_err(|e| format!("write summary: {e}"))?;
    println!("\n=== S2 summary (informal; formal measurement is WP-0D.02) ===");
    for l in lines {
        println!("{l}");
    }
    println!("files: {}", out.display());
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(a)) => a,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    if args.list_devices {
        return match list_devices() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
