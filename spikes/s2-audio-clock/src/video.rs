// SPDX-License-Identifier: GPL-3.0-or-later
//! The "video" side: presents frames whose timing follows the audio clock (§10: show the newest
//! frame with `pts <= audio clock`). Frames are synthesised on demand (frame number, timecode,
//! moving bar, HUD); on the frame of each click the whole frame flashes white so WP-0D.02 can
//! measure the A/V offset with a camera + mic, a photodiode + loopback, or the software estimate in
//! `av_events.csv`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::clock::{AudioClock, MonotonicReader};
use crate::engine::CallbackStats;
use crate::font::{Pen, draw_text};
use crate::meter::LoudnessStats;
use crate::mixer::MixerStats;
use crate::util::NANOS;

/// Presenter settings.
#[derive(Clone, Debug)]
pub struct VideoConfig {
    /// Timeline frame rate.
    pub fps: u32,
    /// Frame buffer width (pixels).
    pub width: usize,
    /// Frame buffer height (pixels).
    pub height: usize,
    /// Shift of video relative to the audio clock in ms (positive = show frames earlier, to
    /// compensate display latency). WP-0D.02 derives the value; 0 by default.
    pub av_offset_ms: f64,
    /// Seconds between clicks / flashes.
    pub click_interval: f64,
    /// Open a window (otherwise terminal-only: frames are scheduled and counted, not drawn).
    pub window: bool,
}

/// Presenter counters shared with the logger.
#[derive(Debug)]
pub struct VideoStats {
    /// Frames presented.
    pub presented: AtomicU64,
    /// Frame indices skipped (clock advanced by more than one frame between presents).
    pub skipped: AtomicU64,
    /// Index of the last presented frame (-1 before the first).
    pub last_index: AtomicI64,
    /// Flash frames presented.
    pub flashes: AtomicU64,
    /// Flashes shown on a later frame than scheduled (their frame was skipped).
    pub late_flashes: AtomicU64,
    /// Interval max duration of a present call (ns).
    pub present_ns_max: AtomicU64,
}

impl Default for VideoStats {
    fn default() -> Self {
        Self {
            presented: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            last_index: AtomicI64::new(-1),
            flashes: AtomicU64::new(0),
            late_flashes: AtomicU64::new(0),
            present_ns_max: AtomicU64::new(0),
        }
    }
}

/// Read-only engine state shown in the HUD.
#[derive(Debug)]
pub struct Hud<'a> {
    /// Master clock.
    pub clock: &'a AudioClock,
    /// Callback health.
    pub cb: &'a CallbackStats,
    /// Mixer health.
    pub mix: &'a MixerStats,
    /// Loudness.
    pub loud: &'a LoudnessStats,
    /// Static description (device, rate, buffer).
    pub info: String,
}

/// Software A/V offset estimates collected while presenting.
#[derive(Debug, Default)]
pub struct VideoSummary {
    /// Present time minus click-audible time, per flash (ms).
    pub offsets_ms: Vec<f64>,
}

/// Index of the newest frame whose pts is at or before `t` seconds.
pub fn frame_at(t: f64, fps: u32) -> i64 {
    // 1e-6 frame of slack absorbs float noise exactly at a frame boundary.
    (t * f64::from(fps) + 1e-6).floor() as i64
}

/// Frame on which click `k` (k >= 1) flashes.
pub fn flash_frame(k: i64, interval: f64, fps: u32) -> i64 {
    (k as f64 * interval * f64::from(fps)).round() as i64
}

/// The latest click whose flash frame lies in `(prev, cur]`, if any.
pub fn flash_in(prev: i64, cur: i64, interval: f64, fps: u32) -> Option<i64> {
    let per = interval * f64::from(fps);
    let mut k = ((cur as f64 + 0.5) / per).floor() as i64;
    while k >= 1 && flash_frame(k, interval, fps) > cur {
        k -= 1;
    }
    while flash_frame(k + 1, interval, fps) <= cur {
        k += 1;
    }
    (k >= 1 && flash_frame(k, interval, fps) > prev).then_some(k)
}

trait Surface {
    /// Shows `frame`; returns false when the user closed the window.
    fn present(&mut self, frame: &[u32], w: usize, h: usize) -> bool;
    /// Processes window events without presenting; false when closed.
    fn pump(&mut self) -> bool;
    /// Whether frames need to be drawn at all.
    fn draws(&self) -> bool;
}

struct Headless;

impl Surface for Headless {
    fn present(&mut self, _: &[u32], _: usize, _: usize) -> bool {
        true
    }
    fn pump(&mut self) -> bool {
        true
    }
    fn draws(&self) -> bool {
        false
    }
}

#[cfg(feature = "window")]
struct WindowSurface(minifb::Window);

#[cfg(feature = "window")]
impl Surface for WindowSurface {
    fn present(&mut self, frame: &[u32], w: usize, h: usize) -> bool {
        if self.0.update_with_buffer(frame, w, h).is_err() {
            return false;
        }
        self.0.is_open() && !self.0.is_key_down(minifb::Key::Escape)
    }
    fn pump(&mut self) -> bool {
        self.0.update();
        self.0.is_open() && !self.0.is_key_down(minifb::Key::Escape)
    }
    fn draws(&self) -> bool {
        true
    }
}

fn open_surface(cfg: &VideoConfig) -> Result<Box<dyn Surface>, String> {
    if !cfg.window {
        return Ok(Box::new(Headless));
    }
    #[cfg(feature = "window")]
    {
        let opts = minifb::WindowOptions {
            scale: minifb::Scale::X2,
            ..minifb::WindowOptions::default()
        };
        let mut w = minifb::Window::new(
            "Cutweave S2 — video follows the audio clock (Esc quits)",
            cfg.width,
            cfg.height,
            opts,
        )
        .map_err(|e| format!("open window: {e}"))?;
        w.set_target_fps(0); // we pace on the audio clock ourselves
        Ok(Box::new(WindowSurface(w)))
    }
    #[cfg(not(feature = "window"))]
    Err("built without the `window` feature; use --headless".into())
}

const BG: u32 = 0x0018_1A20;
const FG: u32 = 0x00E8_E8E8;
const ACCENT: u32 = 0x00F2_B43A;
const WHITE: u32 = 0x00FF_FFFF;
const BLACK: u32 = 0x0000_0000;

/// Frame buffer plus a reusable text line.
struct Canvas {
    buf: Vec<u32>,
    w: usize,
    h: usize,
    line: String,
}

impl Canvas {
    fn text(&mut self, x: usize, y: usize, scale: usize, color: u32) {
        let pen = Pen { x, y, scale, color };
        draw_text(&mut self.buf, self.w, self.h, pen, &self.line);
    }

    fn render(&mut self, idx: i64, fps: u32, flash: bool, hud: &Hud<'_>) {
        use std::fmt::Write as _;
        let (bg, fg) = if flash { (WHITE, BLACK) } else { (BG, FG) };
        self.buf.fill(bg);
        let secs = idx as f64 / f64::from(fps);
        self.line.clear();
        let _ = write!(self.line, "F {idx:06}");
        self.text(16, 14, 6, fg);
        self.line.clear();
        let _ = write!(
            self.line,
            "T {:02}:{:06.3}   FPS {fps}",
            (secs / 60.0).floor() as u64,
            secs % 60.0
        );
        self.text(16, 70, 2, fg);
        if !flash {
            let cb = hud.cb;
            let lat = cb.latency_ns_last.load(Ordering::Relaxed) as f64 / 1e6;
            let frames = cb.frames_max_all.load(Ordering::Relaxed);
            self.line.clear();
            let _ = write!(self.line, "LAT {lat:.1} MS  CB {frames} F");
            self.text(16, 100, 2, fg);
            self.line.clear();
            let _ = write!(
                self.line,
                "UNDERRUN {}  GLITCH {}  XRUN {}",
                hud.mix.underrun_events.load(Ordering::Relaxed),
                cb.glitches.load(Ordering::Relaxed),
                cb.xruns.load(Ordering::Relaxed)
            );
            self.text(16, 124, 2, fg);
            self.line.clear();
            let _ = write!(
                self.line,
                "LUFS M {:.1}  I {:.1}",
                hud.loud.momentary.load(),
                hud.loud.integrated.load()
            );
            self.text(16, 148, 2, fg);
            self.line.clear();
            let drift = hud.clock.drift_ns().unwrap_or(0.0) / 1e6;
            let _ = write!(self.line, "DRIFT {drift:+.3} MS");
            self.text(16, 172, 2, fg);
            self.line.clear();
            self.line.push_str(&hud.info);
            self.text(16, 200, 1, fg);
        }
        // Moving bar: one position per frame within each second, so skipped frames are visible.
        let (w, h) = (self.w, self.h);
        let pos = idx.rem_euclid(i64::from(fps)) as usize;
        let bar_w = 8;
        let span = w.saturating_sub(32 + bar_w);
        let x0 = 16 + pos * span / (fps as usize - 1).max(1);
        let color = if flash { BLACK } else { ACCENT };
        for y in h.saturating_sub(70)..h.saturating_sub(20) {
            for x in x0..(x0 + bar_w).min(w) {
                self.buf[y * w + x] = color;
            }
        }
    }
}

/// Which presenter log a line belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFile {
    /// `av_events.csv`: one row per flash/click pair.
    Av,
    /// `frames.csv`: one row per presented frame (`--frame-log`).
    Frames,
}

/// Presenter log sink. Lines go to a writer thread, so file I/O (and antivirus scanning of the
/// files being written) can never stall presentation.
#[derive(Debug)]
pub struct PresenterLogs {
    /// Channel to the writer thread.
    pub tx: Sender<(LogFile, String)>,
    /// Emit per-frame rows.
    pub frames: bool,
}

/// Spawns the log writer for `av` (and optionally `frames`); returns its sender and handle.
pub fn spawn_log_writer(
    av: File,
    frames: Option<File>,
) -> (Sender<(LogFile, String)>, JoinHandle<()>) {
    let (tx, rx) = channel::<(LogFile, String)>();
    let handle = std::thread::Builder::new()
        .name("s2-video-log".into())
        .spawn(move || {
            let mut av = BufWriter::new(av);
            let mut frames = frames.map(BufWriter::new);
            for (file, line) in rx {
                match file {
                    LogFile::Av => {
                        let _ = writeln!(av, "{line}");
                        let _ = av.flush();
                    }
                    LogFile::Frames => {
                        if let Some(f) = frames.as_mut() {
                            let _ = writeln!(f, "{line}");
                        }
                    }
                }
            }
            let _ = av.flush();
            if let Some(f) = frames.as_mut() {
                let _ = f.flush();
            }
        })
        .expect("spawn video log writer");
    (tx, handle)
}

/// Runs the presenter on the current thread until `stop`, `deadline` or the window closes.
pub fn run_presenter(
    cfg: &VideoConfig,
    hud: &Hud<'_>,
    stats: &VideoStats,
    logs: &PresenterLogs,
    stop: &AtomicBool,
    deadline: Option<Instant>,
) -> Result<VideoSummary, String> {
    let send = |file: LogFile, line: String| {
        let _ = logs.tx.send((file, line));
    };
    let mut surface = open_surface(cfg)?;
    let clock = hud.clock;
    let rate = f64::from(clock.sample_rate());
    let fps = cfg.fps;
    let offset_ns = (cfg.av_offset_ms * 1e6) as i64;
    let mut canvas = Canvas {
        buf: vec![BG; cfg.width * cfg.height],
        w: cfg.width,
        h: cfg.height,
        line: String::with_capacity(128),
    };
    let mut reader = MonotonicReader::default();
    let mut last: i64 = -1;
    let mut last_pump = Instant::now();
    let mut summary = VideoSummary::default();
    send(
        LogFile::Av,
        "click,click_timeline_s,flash_frame,presented_frame,frames_late,present_wall_s,\
         click_audible_wall_s,sw_offset_ms,av_offset_setting_ms"
            .into(),
    );
    if logs.frames {
        send(
            LogFile::Frames,
            "frame,skipped_before,present_wall_s,clock_s,clock_raw_s,late_ms,present_us,\
             anchor_frame,anchor_wall_s,flash"
                .into(),
        );
    }

    loop {
        if stop.load(Ordering::Relaxed) || deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        let now = clock.now_ns();
        let t_video = now.saturating_add_signed(offset_ns);
        let Some(pos) = reader.seconds_at(clock, t_video) else {
            if !surface.pump() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
            continue;
        };
        let idx = frame_at(pos, fps);
        if idx > last {
            let flash_k = flash_in(last, idx, cfg.click_interval, fps);
            if surface.draws() {
                canvas.render(idx, fps, flash_k.is_some(), hud);
            }
            let t0 = Instant::now();
            let open = surface.present(&canvas.buf, cfg.width, cfg.height);
            let present_ns = clock.now_ns();
            let took = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
            stats.present_ns_max.fetch_max(took, Ordering::Relaxed);
            last_pump = Instant::now();
            if last >= 0 && idx > last + 1 {
                stats
                    .skipped
                    .fetch_add((idx - last - 1) as u64, Ordering::Relaxed);
            }
            stats.presented.fetch_add(1, Ordering::Relaxed);
            stats.last_index.store(idx, Ordering::Relaxed);
            if logs.frames {
                let raw = clock.position_frames_at(t_video).unwrap_or(0.0) / rate;
                let crossed = clock.wall_ns_of_frame(idx as f64 / f64::from(fps) * rate);
                let late_ms = crossed.map_or(f64::NAN, |c| {
                    (present_ns as f64 + offset_ns as f64 - c) / 1e6
                });
                let a = clock.anchor().unwrap_or(crate::clock::Anchor {
                    frame: 0,
                    wall_ns: 0,
                    span_frames: 0,
                });
                send(
                    LogFile::Frames,
                    format!(
                        "{idx},{},{:.6},{pos:.6},{raw:.6},{late_ms:.3},{:.1},{},{:.6},{}",
                        if last >= 0 { idx - last - 1 } else { 0 },
                        present_ns as f64 / NANOS,
                        took as f64 / 1e3,
                        a.frame,
                        a.wall_ns as f64 / NANOS,
                        u8::from(flash_k.is_some())
                    ),
                );
            }
            if let Some(k) = flash_k {
                let ff = flash_frame(k, cfg.click_interval, fps);
                stats.flashes.fetch_add(1, Ordering::Relaxed);
                if idx > ff {
                    stats.late_flashes.fetch_add(1, Ordering::Relaxed);
                }
                let click_s = k as f64 * cfg.click_interval;
                if let Some(audible_ns) = clock.wall_ns_of_frame(click_s * rate) {
                    let off_ms = (present_ns as f64 - audible_ns) / 1e6;
                    summary.offsets_ms.push(off_ms);
                    send(
                        LogFile::Av,
                        format!(
                            "{k},{click_s:.6},{ff},{idx},{},{:.6},{:.6},{off_ms:.3},{:.3}",
                            idx - ff,
                            present_ns as f64 / NANOS,
                            audible_ns / NANOS,
                            cfg.av_offset_ms
                        ),
                    );
                }
            }
            last = idx;
            if !open {
                break;
            }
        } else if last_pump.elapsed() > Duration::from_millis(8) {
            if !surface.pump() {
                break;
            }
            last_pump = Instant::now();
        }
        // Pace on the audio clock: sleep until ~1 ms before the next frame boundary, then yield.
        let next_pts = (idx + 1) as f64 / f64::from(fps);
        let remaining = next_pts - pos;
        if remaining > 0.002 {
            std::thread::sleep(Duration::from_secs_f64(remaining - 0.001));
        } else {
            std::thread::yield_now();
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_frame_with_pts_not_after_clock() {
        assert_eq!(frame_at(0.0, 60), 0);
        assert_eq!(frame_at(1.0, 60), 60);
        assert_eq!(frame_at(1.0 - 1e-6, 60), 59);
        assert_eq!(frame_at(0.999_999_999_9, 60), 60); // float noise at a boundary
    }

    /// Minimal PNG (stored deflate) so a rendered frame can be inspected by eye.
    fn write_png(path: &std::path::Path, buf: &[u32], w: usize, h: usize) {
        fn crc32(data: &[u8]) -> u32 {
            let mut c = 0xFFFF_FFFFu32;
            for b in data {
                c ^= u32::from(*b);
                for _ in 0..8 {
                    c = if c & 1 == 1 {
                        0xEDB8_8320 ^ (c >> 1)
                    } else {
                        c >> 1
                    };
                }
            }
            !c
        }
        let mut raw = Vec::new();
        for y in 0..h {
            raw.push(0u8);
            for x in 0..w {
                let p = buf[y * w + x];
                raw.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
        }
        let mut z = vec![0x78, 0x01];
        for (i, block) in raw.chunks(65_535).enumerate() {
            let last = u8::from((i + 1) * 65_535 >= raw.len());
            let len = block.len() as u16;
            z.push(last);
            z.extend_from_slice(&len.to_le_bytes());
            z.extend_from_slice(&(!len).to_le_bytes());
            z.extend_from_slice(block);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for v in &raw {
            a = (a + u32::from(*v)) % 65_521;
            b = (b + a) % 65_521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |ty: &[u8], data: &[u8]| {
            png.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut c = ty.to_vec();
            c.extend_from_slice(data);
            png.extend_from_slice(&c);
            png.extend_from_slice(&crc32(&c).to_be_bytes());
        };
        let mut ihdr = (w as u32).to_be_bytes().to_vec();
        ihdr.extend_from_slice(&(h as u32).to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(b"IHDR", &ihdr);
        chunk(b"IDAT", &z);
        chunk(b"IEND", &[]);
        std::fs::write(path, png).unwrap();
    }

    #[test]
    fn renders_hud_and_flash_frames() {
        let clock = AudioClock::new(48_000, Instant::now());
        let (cb, mix, loud) = (
            CallbackStats::default(),
            MixerStats::default(),
            LoudnessStats::default(),
        );
        let hud = Hud {
            clock: &clock,
            cb: &cb,
            mix: &mix,
            loud: &loud,
            info: "WASAPI 48000 HZ BUF FIXED 256 TRACKS 8".into(),
        };
        let mut c = Canvas {
            buf: vec![0; 640 * 360],
            w: 640,
            h: 360,
            line: String::new(),
        };
        c.render(90, 60, false, &hud);
        assert_eq!(c.buf[0], BG);
        assert!(c.buf.contains(&ACCENT), "moving bar drawn");
        let snapshot = std::env::var_os("S2_SNAPSHOT_DIR");
        if let Some(dir) = &snapshot {
            write_png(
                &std::path::Path::new(dir).join("frame.png"),
                &c.buf,
                640,
                360,
            );
        }
        c.render(120, 60, true, &hud);
        let white = c.buf.iter().filter(|p| **p == WHITE).count();
        assert!(white > 640 * 360 * 9 / 10, "flash frame is (mostly) white");
        if let Some(dir) = &snapshot {
            write_png(
                &std::path::Path::new(dir).join("flash.png"),
                &c.buf,
                640,
                360,
            );
        }
    }

    #[test]
    fn flash_frames_are_found_even_if_skipped() {
        // 1 s interval at 60 fps: flash frames 60, 120, ...
        assert_eq!(flash_in(58, 59, 1.0, 60), None);
        assert_eq!(flash_in(59, 60, 1.0, 60), Some(1));
        assert_eq!(flash_in(60, 61, 1.0, 60), None);
        // Frame 120 skipped (119 → 121): flash shown late on 121.
        assert_eq!(flash_in(119, 121, 1.0, 60), Some(2));
        assert_eq!(flash_in(-1, 30, 1.0, 60), None);
        // Non-integer interval * fps rounds to the nearest frame.
        assert_eq!(flash_frame(1, 0.5, 25), 13);
    }
}
