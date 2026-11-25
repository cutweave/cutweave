// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared fixtures of the WP-1I.04 engine tests: temp dirs, fixture projects, event waits, the
//! process's thread count and its sockets (for the no-telemetry check).

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use cutweave_engine::project_io::ProjectStore;
use cutweave_engine::{
    Call, Connection, EngineConfig, EngineEvent, EngineMode, EventReceiver, LogConfig,
};
use cutweave_model::Project;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::TryRecvError;

/// A temp directory removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cutweave-eng-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Creates the project folder `<dir>/<name>` holding an empty document (and releases it).
pub fn empty_project(dir: &Path, name: &str) -> PathBuf {
    let folder = dir.join(name);
    ProjectStore::create(&folder, &Project::new(name))
        .unwrap()
        .close()
        .unwrap();
    folder
}

/// A headless engine configuration with a server on a free port and its run-file in `dir`;
/// agents may use paths in `dir`.
pub fn headless(dir: &Path) -> EngineConfig {
    let mut config = EngineConfig::new(EngineMode::Headless);
    config.port = Some(0);
    config.api.run_dir = Some(dir.join("run"));
    config.log = LogConfig::disabled();
    config.allowed_roots = vec![dir.to_path_buf()];
    config
}

/// A oneshot engine configuration whose agents may use paths in `dir`.
pub fn oneshot(dir: &Path) -> EngineConfig {
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.allowed_roots = vec![dir.to_path_buf()];
    config
}

/// Calls `name` and unwraps the output.
pub fn must(conn: &Connection, name: &str, params: Value) -> Value {
    match conn.call(Call::new(name, params)) {
        Ok(reply) => reply.output,
        Err(e) => panic!("{name} failed: {e}"),
    }
}

/// Initializes the document with one sequence and returns `(sequence, video track)`.
pub fn new_document(conn: &Connection, name: &str) -> (Value, Value) {
    let out = must(conn, "project.new", json!({ "name": name }));
    (out["sequence"].clone(), out["video_track"].clone())
}

/// Receives events until `pred` matches one (or panics after `timeout`).
pub fn wait_for(
    rx: &mut EventReceiver,
    timeout: Duration,
    mut pred: impl FnMut(&EngineEvent) -> bool,
) -> EngineEvent {
    let deadline = Instant::now() + timeout;
    loop {
        match rx.try_recv() {
            Ok(event) => {
                if pred(&event) {
                    return event;
                }
            }
            Err(TryRecvError::Empty) => {
                assert!(
                    Instant::now() < deadline,
                    "no matching event within {timeout:?}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(TryRecvError::Lagged(n)) => panic!("the test subscriber lagged by {n}"),
            Err(TryRecvError::Closed) => panic!("the event fan-out closed"),
        }
    }
}

/// Every event queued now.
pub fn drain(rx: &mut EventReceiver) -> Vec<EngineEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

// ----- threads ---------------------------------------------------------------------------------

/// The number of threads of this process.
pub fn thread_count() -> usize {
    thread_count_of(std::process::id())
}

#[cfg(target_os = "linux")]
pub fn thread_count_of(pid: u32) -> usize {
    fs::read_dir(format!("/proc/{pid}/task")).unwrap().count()
}

#[cfg(target_os = "macos")]
pub fn thread_count_of(pid: u32) -> usize {
    let out = std::process::Command::new("ps")
        .args(["-M", "-p", &pid.to_string()])
        .output()
        .unwrap();
    // One header line, then one line per thread.
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .count()
        .saturating_sub(1)
}

#[cfg(windows)]
pub fn thread_count_of(pid: u32) -> usize {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("(Get-Process -Id {pid}).Threads.Count"),
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

/// Waits (up to 5 s) for this process's thread count to settle within `tolerance` of
/// `baseline`, returning the last count seen.
pub fn settle_threads(baseline: usize, tolerance: usize) -> usize {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now = thread_count();
        if now <= baseline + tolerance || Instant::now() > deadline {
            return now;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ----- sockets ---------------------------------------------------------------------------------

/// One socket owned by a process: protocol, local and remote address (`ip:port`) and state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Socket {
    pub proto: String,
    pub local: String,
    pub remote: String,
    pub state: String,
}

impl Socket {
    /// Whether this socket could talk to another machine: anything but loopback or an
    /// unconnected listener.
    pub fn is_outbound(&self) -> bool {
        let loopback =
            |a: &str| a.starts_with("127.") || a.starts_with("[::1]") || a.starts_with("::1");
        let unconnected = |a: &str| {
            a == "*:*"
                || a == "0.0.0.0:0"
                || a == "[::]:0"
                || a == "*"
                || a.ends_with(":*")
                || a.is_empty()
        };
        let local_ok = loopback(&self.local);
        let remote_ok = unconnected(&self.remote) || loopback(&self.remote);
        !(local_ok && remote_ok)
    }
}

/// The sockets of process `pid`, or `None` when this platform's tool is unavailable.
#[cfg(windows)]
pub fn sockets_of(pid: u32) -> Option<Vec<Socket>> {
    let out = std::process::Command::new("netstat")
        .args(["-ano"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pid = pid.to_string();
    Some(
        text.lines()
            .filter_map(|line| {
                let cols: Vec<&str> = line.split_whitespace().collect();
                match cols.as_slice() {
                    [proto @ ("TCP" | "TCPv6"), local, remote, state, owner] if *owner == pid => {
                        Some(Socket {
                            proto: (*proto).to_owned(),
                            local: (*local).to_owned(),
                            remote: (*remote).to_owned(),
                            state: (*state).to_owned(),
                        })
                    }
                    [proto @ ("UDP" | "UDPv6"), local, remote, owner] if *owner == pid => {
                        Some(Socket {
                            proto: (*proto).to_owned(),
                            local: (*local).to_owned(),
                            remote: (*remote).to_owned(),
                            state: String::new(),
                        })
                    }
                    _ => None,
                }
            })
            .collect(),
    )
}

/// The sockets of process `pid`, or `None` when this platform's tool is unavailable.
#[cfg(not(windows))]
pub fn sockets_of(pid: u32) -> Option<Vec<Socket>> {
    let out = std::process::Command::new("lsof")
        .args(["-nP", "-a", "-p", &pid.to_string(), "-i"])
        .output()
        .ok()?;
    if !out.status.success() && out.stdout.is_empty() {
        // lsof exits 1 when the process has no sockets at all.
        return if out.status.code() == Some(1) {
            Some(Vec::new())
        } else {
            None
        };
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Some(
        text.lines()
            .skip(1)
            .filter_map(|line| {
                let cols: Vec<&str> = line.split_whitespace().collect();
                // COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME [(STATE)]
                let proto = (*cols.get(7)?).to_owned();
                let name = *cols.get(8)?;
                let state = cols.get(9).map(|s| s.trim_matches(['(', ')']).to_owned());
                let (local, remote) = match name.split_once("->") {
                    Some((l, r)) => (l.to_owned(), r.to_owned()),
                    None => (name.to_owned(), "*:*".to_owned()),
                };
                Some(Socket {
                    proto,
                    local,
                    remote,
                    state: state.unwrap_or_default(),
                })
            })
            .collect(),
    )
}

/// Runs `attempt` (which asserts its own correctness every time and returns how long the timed
/// thing took) up to `attempts` times, stopping at the first within `budget`, and returns the
/// best time with the number of attempts made. For budgets that contention only inflates (a
/// descheduled thread, Defender scanning a fresh binary): a real regression is over the budget
/// on every attempt, so the best of a few still catches it, and the first attempt (the truly
/// cold one) is what a quiet machine reports.
pub fn best_within(
    budget: Duration,
    attempts: u32,
    mut attempt: impl FnMut() -> Duration,
) -> (Duration, u32) {
    let mut best = Duration::MAX;
    let mut made = 0;
    while made < attempts && best >= budget {
        best = best.min(attempt());
        made += 1;
    }
    (best, made)
}
