// SPDX-License-Identifier: GPL-3.0-or-later
//! Test writer for the Node harness and the Playwright e2e of WP-1H.03: a stand-in for the
//! engine that publishes a recognisable frame stream into a real `FramePublisher` ring.
//!
//! ```text
//! ring_fixture [--width W] [--height H] [--fps N] [--format rgba8|nv12] [--slots N]
//!              [--row-align N] [--frames N]
//! ```
//!
//! Prints one JSON line `{"event":"ready","name":...,"epoch":"0x...",...}` to stdout, then
//! publishes frames paced by its own audio clock (frame N is published when the clock enters
//! N, as the engine does), until `--frames` frames (default: until stdin closes). Commands, one
//! per line on stdin:
//!
//! | Command | Effect |
//! |---|---|
//! | `resize W H` | `FramePublisher::configure`; prints `{"event":"moved"|"in-place",...}` |
//! | `idle` | stop publishing but keep beating the heart (an idle engine) |
//! | `hang` | stop publishing and heartbeating (a hung engine) |
//! | `resume` | publish again |
//! | `quit` | close the ring cleanly and exit |
//!
//! Frame content, for verification: bytes `0..4` of the payload are the frame's `seq` (u32 LE);
//! every other byte `i` is `(seq * 3 + i / stride0) as u8`, `stride0` being plane 0's row
//! stride. A torn frame mixes two sequences and fails that check.

// A test tool, not product code: short names and truncating casts are deliberate.
#![allow(
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::expect_used,
    clippy::unchecked_time_subtraction
)]

use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::{Duration as StdDuration, Instant};

use cutweave_model::{Duration, Time};
use cutweave_playback::shm::layout::FLICKS_PER_SECOND;
use cutweave_playback::shm::{
    ClockPump, ClockSample, ClockState, DEFAULT_CLOCK_PERIOD, FrameMeta, FramePublisher,
    PixelFormat, PreviewConfig, Reconfigured, RingConfig,
};

struct Args {
    width: u32,
    height: u32,
    fps: u32,
    format: PixelFormat,
    slots: u32,
    row_align: u32,
    frames: Option<u64>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        width: 1920,
        height: 1080,
        fps: 60,
        format: PixelFormat::Rgba8,
        slots: 6,
        row_align: 1,
        frames: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        let num = |v: String| v.parse::<u32>().map_err(|e| format!("{flag}: {e}"));
        match flag.as_str() {
            "--width" => a.width = num(value()?)?,
            "--height" => a.height = num(value()?)?,
            "--fps" => a.fps = num(value()?)?.max(1),
            "--slots" => a.slots = num(value()?)?,
            "--row-align" => a.row_align = num(value()?)?,
            "--frames" => a.frames = Some(u64::from(num(value()?)?)),
            "--format" => {
                a.format = match value()?.as_str() {
                    "rgba8" => PixelFormat::Rgba8,
                    "nv12" => PixelFormat::Nv12,
                    other => return Err(format!("unknown format {other}")),
                }
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

fn format_name(f: PixelFormat) -> &'static str {
    match f {
        PixelFormat::Rgba8 => "rgba8",
        PixelFormat::Nv12 => "nv12",
    }
}

fn emit(event: &str, p: &FramePublisher, extra: &str) {
    let (cfg, gen_) = p.preview();
    println!(
        "{{\"event\":\"{event}\",\"name\":\"{}\",\"epoch\":\"{:#018x}\",\"pid\":{},\"width\":{},\"height\":{},\"format\":\"{}\",\"configGen\":{gen_},\"slotCapacity\":{}{extra}}}",
        p.name(),
        p.epoch(),
        std::process::id(),
        cfg.width,
        cfg.height,
        format_name(cfg.format),
        p.slot_capacity(),
    );
    let _ = std::io::stdout().flush();
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Publish,
    Idle,
    Hang,
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ring_fixture: {e}");
            std::process::exit(2);
        }
    };
    let mut cfg = RingConfig::new(args.width, args.height, args.format);
    cfg.slot_count = args.slots;
    cfg.row_align = args.row_align;
    let mut p = match FramePublisher::create(&cfg) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ring_fixture: cannot create the ring: {e}");
            std::process::exit(1);
        }
    };
    let start = Instant::now();
    // The audio clock: 1x, position = time since start, sampled every millisecond.
    let _pump = ClockPump::spawn(p.clock_writer(), DEFAULT_CLOCK_PERIOD, move || {
        let flicks = start.elapsed().as_nanos().cast_signed() * i128::from(FLICKS_PER_SECOND)
            / 1_000_000_000;
        let pos = Time::from_flicks(i64::try_from(flicks).unwrap_or(0)).unwrap_or(Time::ZERO);
        ClockSample::now(pos, ClockState::Playing, 1000)
    })
    .expect("clock pump");
    emit("ready", &p, &format!(",\"fps\":{}", args.fps));

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
        // EOF: the harness went away.
        let _ = tx.send("quit".into());
    });

    let period = StdDuration::from_nanos(1_000_000_000 / u64::from(args.fps));
    let flicks_per_frame = FLICKS_PER_SECOND / i64::from(args.fps);
    let mut mode = Mode::Publish;
    let (mut n, mut next) = (0u64, start);
    let mut last_beat = Instant::now();
    loop {
        while let Ok(cmd) = rx.try_recv() {
            let words: Vec<&str> = cmd.split_whitespace().collect();
            match words.as_slice() {
                ["quit"] => {
                    p.close();
                    return;
                }
                ["idle"] => mode = Mode::Idle,
                ["hang"] => mode = Mode::Hang,
                ["resume"] => {
                    mode = Mode::Publish;
                    next = Instant::now();
                }
                ["resize", w, h] => {
                    let (Ok(w), Ok(h)) = (w.parse(), h.parse()) else {
                        eprintln!("ring_fixture: bad resize {cmd:?}");
                        continue;
                    };
                    match p.configure(PreviewConfig {
                        width: w,
                        height: h,
                        format: args.format,
                    }) {
                        Ok(Reconfigured::Moved { .. }) => emit("moved", &p, ""),
                        Ok(Reconfigured::InPlace { .. }) => emit("in-place", &p, ""),
                        Err(e) => eprintln!("ring_fixture: configure failed: {e}"),
                    }
                }
                _ => eprintln!("ring_fixture: unknown command {cmd:?}"),
            }
        }
        if args.frames.is_some_and(|max| n >= max) {
            // Done publishing: stay alive and beating so the harness can read what is left.
            mode = Mode::Idle;
        }
        match mode {
            Mode::Hang => std::thread::sleep(StdDuration::from_millis(5)),
            Mode::Idle => {
                if last_beat.elapsed() >= StdDuration::from_millis(50) {
                    p.heartbeat();
                    last_beat = Instant::now();
                }
                std::thread::sleep(StdDuration::from_millis(5));
            }
            Mode::Publish => {
                let now = Instant::now();
                if now < next {
                    let wait = next - now;
                    // Sleep most of the wait, spin the last 200 us.
                    if wait > StdDuration::from_micros(500) {
                        std::thread::sleep(wait - StdDuration::from_micros(300));
                    } else {
                        std::hint::spin_loop();
                    }
                    continue;
                }
                let Ok(layout) = p.preview_layout() else {
                    continue;
                };
                let meta = FrameMeta {
                    pts: Time::from_flicks(i64::try_from(n).unwrap_or(0) * flicks_per_frame)
                        .unwrap_or(Time::ZERO),
                    duration: Duration::from_flicks(flicks_per_frame).unwrap_or(Duration::ZERO),
                    rev: 1,
                    flags: 0,
                    render_ns: cutweave_playback::shm::host_clock_ns(),
                };
                let stride0 = layout.planes()[0].stride as usize;
                match p.begin_frame(&meta, &layout) {
                    Ok(mut slot) => {
                        let seq = slot.seq();
                        let payload = slot.payload_mut();
                        // One memset per row: fast even in a debug build.
                        #[allow(clippy::cast_possible_truncation)]
                        for (row, chunk) in payload.chunks_mut(stride0).enumerate() {
                            chunk.fill((seq.wrapping_mul(3) as usize).wrapping_add(row) as u8);
                        }
                        #[allow(clippy::cast_possible_truncation)]
                        payload[..4].copy_from_slice(&(seq as u32).to_le_bytes());
                        slot.commit();
                    }
                    Err(e) => eprintln!("ring_fixture: publish failed: {e}"),
                }
                n += 1;
                next += period;
                if now.saturating_duration_since(next) > period * 4 {
                    next = now; // after a stall, restart the schedule instead of bursting
                }
            }
        }
    }
}
