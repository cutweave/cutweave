// SPDX-License-Identifier: GPL-3.0-or-later
//! CPU% and GPU% per benchmark phase (WP-0E.02 metrics).
//!
//! - **Process CPU** comes from the process CPU time (`GetProcessTimes`) over each phase's wall
//!   time, reported as cores used and as % of the machine.
//! - **Machine CPU and GPU engines** come from `monitor.ps1` (Windows performance counters, the
//!   numbers Task Manager shows), sampled once a second in a separate process. A sample counts
//!   for a phase when the middle of its interval falls inside the phase. The machine CPU minus the
//!   process CPU is the background load from other jobs on the shared machine.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ffi;

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn cores() -> f64 {
    std::thread::available_parallelism().map_or(1.0, |n| n.get() as f64)
}

/// A measured phase (wall time, process CPU time, wall-clock window).
#[derive(Clone, Debug, Default)]
pub struct Phase {
    pub name: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub wall: Duration,
    pub cpu: Option<Duration>,
}

impl Phase {
    /// Process CPU time / wall time (1.0 = one core busy).
    pub fn cores_used(&self) -> Option<f64> {
        let w = self.wall.as_secs_f64();
        self.cpu.filter(|_| w > 0.0).map(|c| c.as_secs_f64() / w)
    }

    /// Process CPU as % of all logical cores.
    pub fn machine_pct(&self) -> Option<f64> {
        self.cores_used().map(|c| 100.0 * c / cores())
    }
}

/// Starts a phase; `stop` it to get the [`Phase`].
pub struct PhaseClock {
    name: String,
    t0: Instant,
    start_ms: u64,
    cpu0: Option<Duration>,
}

impl PhaseClock {
    pub fn start(name: &str) -> Self {
        PhaseClock {
            name: name.to_string(),
            t0: Instant::now(),
            start_ms: unix_ms(),
            cpu0: ffi::process_cpu_time(),
        }
    }

    pub fn stop(self) -> Phase {
        let cpu = match (self.cpu0, ffi::process_cpu_time()) {
            (Some(a), Some(b)) => Some(b.saturating_sub(a)),
            _ => None,
        };
        Phase {
            name: self.name,
            start_ms: self.start_ms,
            end_ms: unix_ms(),
            wall: self.t0.elapsed(),
            cpu,
        }
    }
}

/// One `monitor.ps1` row.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub unix_ms: u64,
    pub cpu_total: f64,
    /// This process: decode, 3d, copy, compute engine %.
    pub proc: [f64; 4],
    /// All processes: decode, 3d, copy, compute engine % (busiest engine per class).
    pub all: [f64; 4],
}

/// Phase averages of the monitor samples.
#[derive(Clone, Copy, Debug, Default)]
pub struct Utilisation {
    pub samples: usize,
    pub cpu_total: f64,
    pub proc: [f64; 4],
    pub all: [f64; 4],
}

/// The running `monitor.ps1`.
pub struct Monitor {
    child: Child,
    csv: PathBuf,
    interval_ms: u64,
}

impl Monitor {
    /// Starts sampling this process (Windows only; `None` elsewhere or if PowerShell fails).
    pub fn start(out_dir: &Path, tag: &str) -> Option<Self> {
        if !cfg!(windows) {
            return None;
        }
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("monitor.ps1");
        let _ = std::fs::create_dir_all(out_dir);
        let csv = out_dir.join(format!("monitor-{tag}.csv"));
        let _ = std::fs::remove_file(&csv);
        let child = Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .arg("-ProcessId")
            .arg(std::process::id().to_string())
            .arg("-Out")
            .arg(&csv)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let m = Monitor {
            child,
            csv,
            interval_ms: 1000,
        };
        // Wait for the first sample (PowerShell start-up plus one interval).
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            if !m.samples().is_empty() {
                return Some(m);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        eprintln!("monitor: no sample after 20 s, continuing without GPU/CPU counters");
        Some(m)
    }

    fn samples(&self) -> Vec<Sample> {
        let Ok(text) = std::fs::read_to_string(&self.csv) else {
            return vec![];
        };
        text.lines().skip(1).filter_map(parse_row).collect()
    }

    /// Averages the samples whose interval midpoint lies inside `phase`.
    pub fn utilisation(&self, phase: &Phase) -> Utilisation {
        summarize(&self.samples(), phase, self.interval_ms)
    }

    /// Stops the sampler (after one more interval, so the last phase gets its samples).
    pub fn stop(mut self) -> Vec<Sample> {
        std::thread::sleep(Duration::from_millis(self.interval_ms + 300));
        let s = self.samples();
        let _ = self.child.kill();
        let _ = self.child.wait();
        s
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn parse_row(line: &str) -> Option<Sample> {
    let v: Vec<f64> = line
        .split(',')
        .map(|x| x.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    if v.len() < 10 {
        return None;
    }
    Some(Sample {
        unix_ms: v[0] as u64,
        cpu_total: v[1],
        proc: [v[2], v[3], v[4], v[5]],
        all: [v[6], v[7], v[8], v[9]],
    })
}

/// Averages `samples` over `phase` (a sample covers the `interval_ms` before its timestamp).
pub fn summarize(samples: &[Sample], phase: &Phase, interval_ms: u64) -> Utilisation {
    let inside: Vec<&Sample> = samples
        .iter()
        .filter(|s| {
            let mid = s.unix_ms.saturating_sub(interval_ms / 2);
            mid >= phase.start_ms && mid <= phase.end_ms
        })
        .collect();
    let n = inside.len();
    let mut u = Utilisation {
        samples: n,
        ..Utilisation::default()
    };
    if n == 0 {
        return u;
    }
    for s in &inside {
        u.cpu_total += s.cpu_total;
        for i in 0..4 {
            u.proc[i] += s.proc[i];
            u.all[i] += s.all[i];
        }
    }
    let k = n as f64;
    u.cpu_total /= k;
    for i in 0..4 {
        u.proc[i] /= k;
        u.all[i] /= k;
    }
    u
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_attributed_by_interval_midpoint() {
        let s = |ms, cpu| Sample {
            unix_ms: ms,
            cpu_total: cpu,
            proc: [cpu; 4],
            all: [cpu; 4],
        };
        let samples = [s(1000, 10.0), s(2000, 20.0), s(3000, 40.0), s(4000, 80.0)];
        let phase = Phase {
            start_ms: 1200,
            end_ms: 2800,
            ..Phase::default()
        };
        // Midpoints 500, 1500, 2500, 3500: the second and third count.
        let u = summarize(&samples, &phase, 1000);
        assert_eq!(u.samples, 2);
        assert!((u.cpu_total - 30.0).abs() < 1e-9);
        assert!(parse_row("1,2,3,4,5,6,7,8,9,10").is_some());
        assert!(parse_row("unix_ms,cpu_total").is_none());
    }
}
