// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared fixtures of the WP-1I.04 engine tests: temp dirs, fixture projects, event waits, the
//! process's threads and its sockets (for the no-telemetry check).

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;
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

/// The number of this process's threads a leak check counts (see [`ThreadCensus::counted`]).
pub fn thread_count() -> usize {
    thread_count_of(std::process::id())
}

/// The number of threads of process `pid` a leak check counts (see [`ThreadCensus::counted`]).
pub fn thread_count_of(pid: u32) -> usize {
    thread_census_of(pid).counted()
}

/// This process's threads (see [`ThreadCensus`]).
pub fn thread_census() -> ThreadCensus {
    thread_census_of(std::process::id())
}

/// The threads of a process: how many, and on Windows the module each one started in.
#[derive(Clone, Debug, Default)]
pub struct ThreadCensus {
    pub total: usize,
    /// Threads by the module of their start address, lowercase (Windows; empty elsewhere).
    pub by_start: BTreeMap<String, usize>,
}

impl ThreadCensus {
    /// Workers of the Windows thread pool: they start in `ntdll.dll` (`TppWorkerThread`).
    pub fn os_pool(&self) -> usize {
        self.by_start.get("ntdll.dll").copied().unwrap_or(0)
    }

    /// The threads a leak check counts: all but the Windows thread pool's workers. That pool is
    /// the operating system's, one per process: the software rasterizer (WARP), the GPU drivers
    /// and the loader queue work on it, and Windows adds workers under load and retires idle
    /// ones on its own schedule (after identical engine runs it held 15 to 24, and grew while
    /// the process was idle). No engine code starts those threads or could join them; every
    /// thread the engine starts (std, tokio, FFmpeg's) starts elsewhere and counts.
    pub fn counted(&self) -> usize {
        self.total - self.os_pool()
    }
}

impl std::fmt::Display for ThreadCensus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} threads", self.total)?;
        let mut by_count: Vec<_> = self.by_start.iter().collect();
        by_count.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (i, (module, n)) in by_count.into_iter().enumerate() {
            let pool = if module == "ntdll.dll" {
                " (OS thread pool)"
            } else {
                ""
            };
            write!(f, "{} {n} {module}{pool}", if i == 0 { ":" } else { "," })?;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub fn thread_census_of(pid: u32) -> ThreadCensus {
    ThreadCensus {
        total: fs::read_dir(format!("/proc/{pid}/task")).unwrap().count(),
        by_start: BTreeMap::new(),
    }
}

#[cfg(target_os = "macos")]
pub fn thread_census_of(pid: u32) -> ThreadCensus {
    let out = std::process::Command::new("ps")
        .args(["-M", "-p", &pid.to_string()])
        .output()
        .unwrap();
    // One header line, then one line per thread.
    let total = String::from_utf8_lossy(&out.stdout)
        .lines()
        .count()
        .saturating_sub(1);
    ThreadCensus {
        total,
        by_start: BTreeMap::new(),
    }
}

/// The start address of a thread (`ThreadQuerySetWin32StartAddress`; 0 once it is gone), for
/// PowerShell's `Add-Type` (passed in an environment variable: no command-line quoting).
#[cfg(windows)]
const THREAD_START_CS: &str = r#"
using System;
using System.Runtime.InteropServices;
public static class CutweaveThreadStart {
    [DllImport("kernel32.dll")] static extern IntPtr OpenThread(int access, bool inherit, int id);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("ntdll.dll")] static extern int NtQueryInformationThread(
        IntPtr thread, int infoClass, out IntPtr info, int length, IntPtr returned);
    public static long Of(int id) {
        IntPtr thread = OpenThread(0x0040, false, id); // THREAD_QUERY_INFORMATION
        if (thread == IntPtr.Zero) return 0;
        try {
            IntPtr start;
            return NtQueryInformationThread(thread, 9, out start, IntPtr.Size, IntPtr.Zero) == 0
                ? start.ToInt64() : 0;
        } finally { CloseHandle(thread); }
    }
}
"#;

#[cfg(windows)]
pub fn thread_census_of(pid: u32) -> ThreadCensus {
    // One line per live thread: the module its start address is in ("?" outside any).
    let script = format!(
        "Add-Type -TypeDefinition $env:CUTWEAVE_THREAD_START_CS; \
         $p = Get-Process -Id {pid}; \
         $m = @($p.Modules | ForEach-Object {{ ,@($_.ModuleName, $_.BaseAddress.ToInt64(), \
           ($_.BaseAddress.ToInt64() + $_.ModuleMemorySize)) }}); \
         foreach ($t in $p.Threads) {{ $s = [CutweaveThreadStart]::Of($t.Id); \
           if ($s -ne 0) {{ $n = '?'; \
             foreach ($x in $m) {{ if ($s -ge $x[1] -and $s -lt $x[2]) {{ $n = $x[0]; break }} }}; \
             $n }} }}"
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .env("CUTWEAVE_THREAD_START_CS", THREAD_START_CS)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && !stdout.trim().is_empty(),
        "cannot list the threads of {pid}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut census = ThreadCensus::default();
    for module in stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
        census.total += 1;
        *census
            .by_start
            .entry(module.to_ascii_lowercase())
            .or_default() += 1;
    }
    census
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
