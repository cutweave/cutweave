//! `s1-engine`: spike S1 sidecar (WP-0C.01).
//!
//! Renders synthetic animated frames with wgpu at a fixed rate, reads them back asynchronously
//! and publishes them into a named shared-memory ring (arch §10). A WebSocket control channel
//! accepts play/pause/seek/format. Startup handshake (arch §6): exactly one JSON line on stdout
//! carrying the bound port and a fresh 256-bit UI token; the token is never written to disk.
//! The engine exits when stdin closes (parent died), so it never outlives Electron main.

mod control;
mod gpu;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use s1_ring::layout::mapping_len;
use s1_ring::{EngineState, FLICKS_PER_SECOND, PixelFormat, RingWriter, SharedMemory};
use serde::Serialize;

use crate::control::{Command, ControlConfig};
use crate::gpu::{Gpu, GpuPref};

const USAGE: &str = "usage: s1-engine --ring <name> [--port <p>] [--width 1920] [--height 1080] \
[--fps 60] [--slots 6] [--format rgba8|nv12] [--gpu high|low|software] [--frames N] \
[--paused] [--allow-origin app://s1] [--no-stdin-watch]";

struct Args {
    ring: String,
    port: u16,
    width: u32,
    height: u32,
    fps: u32,
    slots: u32,
    format: PixelFormat,
    gpu: GpuPref,
    frames: Option<u64>,
    paused: bool,
    allow_origin: String,
    watch_stdin: bool,
}

fn parse_args() -> Result<Args> {
    let mut a = Args {
        ring: String::new(),
        port: 0,
        width: 1920,
        height: 1080,
        fps: 60,
        slots: 6,
        format: PixelFormat::Rgba8,
        gpu: GpuPref::High,
        frames: None,
        paused: false,
        allow_origin: "app://s1".into(),
        watch_stdin: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| anyhow!("{flag} needs a value\n{USAGE}"))
        };
        match flag.as_str() {
            "--ring" => a.ring = val()?,
            "--port" => a.port = val()?.parse()?,
            "--width" => a.width = val()?.parse()?,
            "--height" => a.height = val()?.parse()?,
            "--fps" => a.fps = val()?.parse()?,
            "--slots" => a.slots = val()?.parse()?,
            "--format" => {
                a.format = match val()?.as_str() {
                    "rgba8" => PixelFormat::Rgba8,
                    "nv12" => PixelFormat::Nv12,
                    f => bail!("unknown format {f}"),
                }
            }
            "--gpu" => {
                a.gpu = match val()?.as_str() {
                    "high" => GpuPref::High,
                    "low" => GpuPref::Low,
                    "software" => GpuPref::Software,
                    g => bail!("unknown gpu preference {g}"),
                }
            }
            "--frames" => a.frames = Some(val()?.parse()?),
            "--paused" => a.paused = true,
            "--allow-origin" => a.allow_origin = val()?,
            "--no-stdin-watch" => a.watch_stdin = false,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => bail!("unknown argument {other}\n{USAGE}"),
        }
    }
    if a.ring.is_empty() {
        bail!("--ring is required\n{USAGE}");
    }
    if a.fps == 0 {
        bail!("--fps must be > 0");
    }
    Ok(a)
}

#[derive(Serialize)]
struct Ready<'a> {
    s1: &'static str,
    v: u32,
    pid: u32,
    port: u16,
    token: &'a str,
    ring: &'a str,
    width: u32,
    height: u32,
    fps: u32,
    slots: u32,
    adapter: String,
    backend: String,
    device_type: String,
}

/// Playback position shared with the audio-clock thread. Only the render loop mutates it.
struct Transport {
    playing: AtomicBool,
    anchor_us: AtomicU64,
    anchor_flicks: AtomicI64,
}

impl Transport {
    fn position(&self, now_us: u64) -> i64 {
        let base = self.anchor_flicks.load(Ordering::Acquire);
        if !self.playing.load(Ordering::Acquire) {
            return base;
        }
        let dt = i64::try_from(now_us.saturating_sub(self.anchor_us.load(Ordering::Acquire)))
            .unwrap_or(i64::MAX);
        // 705.6 flicks per µs.
        base + dt * (FLICKS_PER_SECOND / 1000) / 1000
    }

    fn reanchor(&self, flicks: i64, playing: bool) {
        self.anchor_us
            .store(s1_ring::clock::now_us(), Ordering::Release);
        self.anchor_flicks.store(flicks, Ordering::Release);
        self.playing.store(playing, Ordering::Release);
    }
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("getrandom: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("S1_LOG").unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn")
            }),
        )
        .with_ansi(false)
        .init();
    if let Err(e) = run() {
        tracing::error!("{e:#}");
        eprintln!("s1-engine: {e:#}");
        std::process::exit(1);
    }
}

#[allow(clippy::too_many_lines)]
fn run() -> Result<()> {
    let args = parse_args()?;
    let frame_dur_flicks = FLICKS_PER_SECOND / i64::from(args.fps);

    // 1. Ring. Capacity = largest format at this size (RGBA8).
    let capacity = PixelFormat::Rgba8.frame_len(args.width, args.height);
    let shm = SharedMemory::create(&args.ring, mapping_len(args.slots, capacity))
        .with_context(|| format!("create ring {}", args.ring))?;
    let mut ring = RingWriter::init(shm, args.slots, capacity, frame_dur_flicks)?;
    let clock = ring.clock();

    // 2. GPU.
    let mut gpu = Gpu::new(args.gpu, args.width, args.height)?;
    tracing::info!(adapter = %gpu.info.name, backend = ?gpu.info.backend, "gpu ready");

    // 3. Control channel.
    let listener = TcpListener::bind(("127.0.0.1", args.port)).context("bind control port")?;
    let port = listener.local_addr()?.port();
    let token = random_token()?;
    let (tx, rx) = mpsc::channel::<Command>();
    control::spawn(
        listener,
        ControlConfig {
            token: token.clone(),
            port,
            allow_origin: args.allow_origin.clone(),
        },
        tx,
    );

    // 4. Handshake: one line on stdout, then stdout is never used again.
    let ready = Ready {
        s1: "ready",
        v: 1,
        pid: std::process::id(),
        port,
        token: &token,
        ring: &args.ring,
        width: args.width,
        height: args.height,
        fps: args.fps,
        slots: args.slots,
        adapter: gpu.info.name.clone(),
        backend: format!("{:?}", gpu.info.backend),
        device_type: format!("{:?}", gpu.info.device_type),
    };
    {
        let mut out = std::io::stdout().lock();
        serde_json::to_writer(&mut out, &ready)?;
        out.write_all(b"\n")?;
        out.flush()?;
    }
    drop(token);

    // 5. Lifetime: exit when the parent closes our stdin.
    let shutdown = Arc::new(AtomicBool::new(false));
    if args.watch_stdin {
        let shutdown = Arc::clone(&shutdown);
        std::thread::Builder::new()
            .name("stdin-watch".into())
            .spawn(move || {
                let mut buf = [0u8; 64];
                let mut stdin = std::io::stdin();
                while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
                tracing::info!("stdin closed; shutting down");
                shutdown.store(true, Ordering::Release);
            })?;
    }

    // 6. Fake audio clock, ~1 kHz.
    let transport = Arc::new(Transport {
        playing: AtomicBool::new(false),
        anchor_us: AtomicU64::new(s1_ring::clock::now_us()),
        anchor_flicks: AtomicI64::new(0),
    });
    {
        let (transport, clock, shutdown) =
            (Arc::clone(&transport), clock.clone(), Arc::clone(&shutdown));
        std::thread::Builder::new()
            .name("audio-clock".into())
            .spawn(move || {
                while !shutdown.load(Ordering::Acquire) {
                    let now = s1_ring::clock::now_us();
                    clock.set_audio_clock(transport.position(now), now);
                    std::thread::sleep(Duration::from_millis(1));
                }
            })?;
    }

    // 7. Render loop.
    let mut format = args.format;
    let mut last_frame: Option<i64> = None;
    let mut needs_frame = true; // always produce frame 0, even when starting paused
    transport.reanchor(0, !args.paused);
    clock.set_state(if args.paused {
        EngineState::Paused
    } else {
        EngineState::Playing
    });
    let rev = 1u64;
    let mut log_at = Instant::now() + Duration::from_secs(1);
    let (mut dropped_s, mut late_s) = (0u64, 0u64);

    while !shutdown.load(Ordering::Acquire) {
        // Latest-wins command handling: apply in order, but only the last seek renders.
        let mut seek: Option<i64> = None;
        while let Ok(cmd) = rx.try_recv() {
            let now = s1_ring::clock::now_us();
            match cmd {
                Command::Play => {
                    transport.reanchor(transport.position(now), true);
                    clock.set_state(EngineState::Playing);
                }
                Command::Pause => {
                    transport.reanchor(transport.position(now), false);
                    clock.set_state(EngineState::Paused);
                }
                Command::Seek(pts) => seek = Some(pts.max(0)),
                Command::Format(f) => {
                    format = f;
                    needs_frame = true;
                }
            }
        }
        if let Some(pts) = seek {
            let playing = transport.playing.load(Ordering::Acquire);
            transport.reanchor(pts, playing);
            last_frame = None;
            needs_frame = true;
        }

        // Clock-driven production: frame N (pts = N * dur) is rendered as soon as the audio
        // clock enters [N * dur, (N + 1) * dur), so a frame never leads the clock and a late
        // loop iteration cannot make the engine drift ahead (an earlier tick-accumulator
        // version did, by ~35 ms under load).
        let playing = transport.playing.load(Ordering::Acquire);
        let pos = transport.position(s1_ring::clock::now_us());
        let frame = if playing {
            pos.div_euclid(frame_dur_flicks)
        } else {
            // Paused: the frame nearest the position (exact for frame-aligned seeks).
            (pos + frame_dur_flicks / 2).div_euclid(frame_dur_flicks)
        };
        let due = match last_frame {
            _ if needs_frame => true,
            Some(last) => playing && frame > last,
            None => playing,
        };
        if due {
            if let Some(last) = last_frame.filter(|_| playing) {
                let skipped = u64::try_from(frame - last - 1).unwrap_or(0);
                if skipped > 0 {
                    clock.add_dropped(skipped);
                    dropped_s += skipped;
                    late_s += 1;
                }
            }
            let frame_u = u64::try_from(frame).unwrap_or(0);
            if gpu.render(frame_u, frame * frame_dur_flicks, format, rev) {
                last_frame = Some(frame);
                needs_frame = false;
            } else {
                clock.add_dropped(1);
                dropped_s += 1;
            }
        }

        // Next wake-up: the next frame boundary on the audio clock (or one frame when paused).
        let wait_flicks = if playing {
            (last_frame.unwrap_or(frame) + 1) * frame_dur_flicks - pos
        } else {
            frame_dur_flicks
        };
        let wait_us = u64::try_from(
            wait_flicks.clamp(0, frame_dur_flicks * 2) * 1000 / (FLICKS_PER_SECOND / 1000),
        )
        .unwrap_or(0);
        let next_wake = Instant::now() + Duration::from_micros(wait_us);
        // Publish finished readbacks as soon as the GPU is done (low latency), up to the wake-up.
        gpu.publish_ready(&mut ring, next_wake)?;
        clock.heartbeat();

        if args.frames.is_some_and(|limit| ring.write_index() >= limit) {
            break;
        }
        let now = Instant::now();
        if next_wake > now {
            std::thread::sleep(next_wake - now);
        }

        if Instant::now() >= log_at {
            let s = gpu.stats;
            if s.published > 0 {
                tracing::info!(
                    fps = s.published,
                    submit_to_publish_ms = format!(
                        "{:.2}",
                        s.submit_to_publish_us as f64 / s.published as f64 / 1000.0
                    ),
                    max_ms = format!("{:.2}", s.max_submit_to_publish_us as f64 / 1000.0),
                    copy_ms = format!("{:.2}", s.copy_us as f64 / s.published as f64 / 1000.0),
                    dropped = dropped_s,
                    late_ticks = late_s,
                    in_flight = gpu.in_flight(),
                    "engine"
                );
            }
            gpu.stats = gpu::GpuStats::default();
            dropped_s = 0;
            late_s = 0;
            log_at += Duration::from_secs(1);
        }
    }

    // Drain outstanding readbacks so the last frames land, then mark stopped.
    let _ = gpu.publish_ready(&mut ring, Instant::now() + Duration::from_millis(200));
    clock.set_state(EngineState::Stopped);
    tracing::info!(frames = ring.write_index(), "engine stopped");
    Ok(())
}
