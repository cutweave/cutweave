// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine's log file and crash marker (WP-1I.04; arch §18).
//!
//! - **JSON lines** in `~/.cutweave/logs/engine-<pid>.log` (configurable directory): one object
//!   per `tracing` event with `ts` (RFC 3339 UTC), `level`, `target`, `msg`, `fields`, the
//!   enclosing `spans` (name and fields) and the `thread`. Nothing is formatted for terminals
//!   and nothing leaves the machine: **no telemetry**. Code never logs secrets (tokens are
//!   redacted by their `Debug`; `#[secret]` params never reach logs).
//! - **Rotation:** the active file is rotated at [`LogConfig::max_bytes`] (10 MB) to
//!   `engine-<pid>.<n>.log`, and the directory keeps the newest [`LogConfig::keep`] (5) engine
//!   log files: at most 5 × 10 MB.
//! - **Crash marker:** a panic hook writes `crash-<pid>.json` (`{event: "crash", pid, version,
//!   ts, thread, message, location}`) next to the log, then runs the previous hook. A clean exit
//!   removes it ([`remove_crash_marker`]), so after the process is gone a marker means it
//!   crashed (WP-1L.03 reads it).
//!
//! The subscriber is process-global: the first [`init`] installs it (if no other global
//! subscriber exists) and later ones in the same process redirect it to their file.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard, Once, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use tracing::field::{Field, Visit};
use tracing::span;
use tracing::subscriber::Interest;
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// Rotate the active log file at this size.
pub const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Engine log files kept in the directory.
pub const DEFAULT_KEEP: usize = 5;
/// Longest panic message kept in a crash marker.
const MAX_CRASH_MESSAGE: usize = 2048;
/// Crash markers kept in the directory.
const KEEP_CRASH_MARKERS: usize = 10;

/// Where and how much the engine logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogConfig {
    /// The directory (`~/.cutweave/logs` when `None` and [`enabled`](Self::enabled)).
    pub dir: Option<PathBuf>,
    /// Whether to log to a file at all.
    pub on: bool,
    /// The most verbose level written.
    pub level: Level,
    /// Rotation size.
    pub max_bytes: u64,
    /// Engine log files kept.
    pub keep: usize,
}

impl Default for LogConfig {
    /// The engine process default: `~/.cutweave/logs`, `INFO`, 5 × 10 MB.
    fn default() -> LogConfig {
        LogConfig {
            dir: None,
            on: true,
            level: Level::INFO,
            max_bytes: DEFAULT_MAX_BYTES,
            keep: DEFAULT_KEEP,
        }
    }
}

impl LogConfig {
    /// No log file (in-process engines: tests, embedding).
    #[must_use]
    pub fn disabled() -> LogConfig {
        LogConfig {
            on: false,
            ..LogConfig::default()
        }
    }

    /// Logging into `dir`.
    #[must_use]
    pub fn in_dir(dir: impl Into<PathBuf>) -> LogConfig {
        LogConfig {
            dir: Some(dir.into()),
            ..LogConfig::default()
        }
    }

    /// Whether a log file is written.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.on
    }

    /// The directory used: the configured one or `~/.cutweave/logs`.
    #[must_use]
    pub fn resolved_dir(&self) -> Option<PathBuf> {
        self.dir.clone().or_else(default_dir)
    }
}

/// `~/.cutweave/logs`, or `None` when no home directory is known.
#[must_use]
pub fn default_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()))?;
    Some(PathBuf::from(home).join(".cutweave").join("logs"))
}

/// The crash marker of process `pid` in `dir`.
#[must_use]
pub fn crash_marker_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("crash-{pid}.json"))
}

/// Removes this process's crash marker (a clean exit).
pub fn remove_crash_marker(dir: &Path) {
    let _ = fs::remove_file(crash_marker_path(dir, std::process::id()));
}

/// Whether the file name `name` has the extension `ext` (any case).
fn has_ext(name: &str, ext: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

// ----- the rotating file -----------------------------------------------------------------------

/// An append-only log file rotated by size, with the directory pruned to a number of files.
#[derive(Debug)]
pub(crate) struct RotatingFile {
    dir: PathBuf,
    stem: String,
    file: Option<File>,
    size: u64,
    max_bytes: u64,
    keep: usize,
    rotations: u32,
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

impl RotatingFile {
    /// Opens (appending) `<dir>/<stem>.log`.
    pub(crate) fn open(
        dir: &Path,
        stem: &str,
        max_bytes: u64,
        keep: usize,
    ) -> io::Result<RotatingFile> {
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("{stem}.log"));
        let file = open_append(&path)?;
        let size = file.metadata().map_or(0, |m| m.len());
        let rf = RotatingFile {
            dir: dir.to_path_buf(),
            stem: stem.to_owned(),
            file: Some(file),
            size,
            max_bytes: max_bytes.max(1),
            keep: keep.max(1),
            rotations: 0,
        };
        rf.prune();
        Ok(rf)
    }

    /// The active file.
    pub(crate) fn path(&self) -> PathBuf {
        self.dir.join(format!("{}.log", self.stem))
    }

    /// Appends `line` and a newline (one write), rotating first if it would not fit.
    pub(crate) fn write_line(&mut self, line: &str) -> io::Result<()> {
        let len = u64::try_from(line.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if self.size > 0 && self.size.saturating_add(len) > self.max_bytes {
            self.rotate()?;
        }
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        let file = match &mut self.file {
            Some(f) => f,
            None => self.file.insert(open_append(&self.path())?),
        };
        file.write_all(&buf)?;
        self.size = self.size.saturating_add(len);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        drop(self.file.take());
        let active = self.path();
        // A name not used yet (older rotations of the same pid may exist from a reused pid).
        let archived = loop {
            self.rotations += 1;
            let candidate = self
                .dir
                .join(format!("{}.{}.log", self.stem, self.rotations));
            if !candidate.exists() {
                break candidate;
            }
        };
        fs::rename(&active, &archived)?;
        self.file = Some(open_append(&active)?);
        self.size = 0;
        self.prune();
        Ok(())
    }

    /// Deletes the oldest engine log files beyond `keep` (never the active one).
    fn prune(&self) {
        let active = self.path();
        let mut files: Vec<(SystemTime, PathBuf)> = fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("engine-") && has_ext(n, "log"))
            })
            .map(|e| {
                let modified = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(UNIX_EPOCH);
                (modified, e.path())
            })
            .filter(|(_, p)| *p != active)
            .collect();
        // Newest first; the active file is one of the `keep`.
        files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        for (_, old) in files.into_iter().skip(self.keep.saturating_sub(1)) {
            let _ = fs::remove_file(old);
        }
    }
}

// ----- the global sink -------------------------------------------------------------------------

struct Sink {
    file: Option<RotatingFile>,
    dir: Option<PathBuf>,
    generation: u64,
}

static SINK: Mutex<Sink> = Mutex::new(Sink {
    file: None,
    dir: None,
    generation: 0,
});

/// The most verbose level written (`Level` as 1 = ERROR … 5 = TRACE).
static LEVEL: AtomicU8 = AtomicU8::new(3);

fn sink() -> MutexGuard<'static, Sink> {
    SINK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn level_code(level: Level) -> u8 {
    if level == Level::ERROR {
        1
    } else if level == Level::WARN {
        2
    } else if level == Level::INFO {
        3
    } else if level == Level::DEBUG {
        4
    } else {
        5
    }
}

fn write_line(line: &str) {
    let mut sink = sink();
    if let Some(file) = &mut sink.file {
        let _ = file.write_line(line);
    }
}

/// Keeps the log file open; [`finish`](Self::finish) (or drop) detaches it.
#[derive(Debug)]
pub struct LogGuard {
    path: PathBuf,
    dir: PathBuf,
    generation: u64,
}

impl LogGuard {
    /// The active log file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The log directory (where the crash marker goes).
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Detaches the file (later events of this process are not written).
    pub fn finish(self) {}
}

impl Drop for LogGuard {
    fn drop(&mut self) {
        let mut sink = sink();
        if sink.generation == self.generation {
            sink.file = None;
            sink.dir = None;
        }
    }
}

static INSTALL: Once = Once::new();
static INSTALLED: OnceLock<bool> = OnceLock::new();

/// Starts logging to `config`'s directory and installs the crash-marker panic hook. Returns
/// whether this process's global subscriber is ours in [`LogGuard`]'s lifetime.
pub fn init(config: &LogConfig) -> io::Result<LogGuard> {
    let dir = config
        .resolved_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no log directory (no HOME)"))?;
    let pid = std::process::id();
    let file = RotatingFile::open(
        &dir,
        &format!("engine-{pid}"),
        config.max_bytes,
        config.keep,
    )?;
    prune_crash_markers(&dir);
    let path = file.path();
    LEVEL.store(level_code(config.level), Ordering::Relaxed);
    let generation = {
        let mut sink = sink();
        sink.generation += 1;
        sink.file = Some(file);
        sink.dir = Some(dir.clone());
        sink.generation
    };
    INSTALL.call_once(|| {
        let subscriber = tracing_subscriber::registry().with(JsonLayer);
        let ok = tracing::subscriber::set_global_default(subscriber).is_ok();
        let _ = INSTALLED.set(ok);
        install_panic_hook();
    });
    if INSTALLED.get() != Some(&true) {
        // Someone else's subscriber is global (an embedding host): ours never sees events.
        eprintln!("cutweave: another tracing subscriber is installed; the engine log is empty");
    }
    Ok(LogGuard {
        path,
        dir,
        generation,
    })
}

// ----- the JSON layer --------------------------------------------------------------------------

/// Fields of a span, kept in its extensions.
struct SpanFields(Map<String, Value>);

struct JsonVisitor<'a>(&'a mut Map<String, Value>);

impl Visit for JsonVisitor<'_> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.0.insert(field.name().into(), json!(value.to_string()));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().into(), json!(format!("{value:?}")));
    }
}

/// Writes every enabled event as one JSON line into the global sink.
struct JsonLayer;

impl<S> Layer<S> for JsonLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        // The level can change with the next `init`: decide per event.
        Interest::sometimes()
    }

    fn enabled(&self, metadata: &Metadata<'_>, _: Context<'_, S>) -> bool {
        level_code(*metadata.level()) <= LEVEL.load(Ordering::Relaxed)
    }

    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut fields = Map::new();
        attrs.record(&mut JsonVisitor(&mut fields));
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(SpanFields(fields));
        }
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(SpanFields(fields)) = span.extensions_mut().get_mut::<SpanFields>()
        {
            values.record(&mut JsonVisitor(fields));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = Map::new();
        event.record(&mut JsonVisitor(&mut fields));
        let meta = event.metadata();
        let mut line = Map::new();
        line.insert("ts".into(), Value::String(rfc3339(SystemTime::now())));
        line.insert("level".into(), Value::String(meta.level().as_str().into()));
        line.insert("target".into(), Value::String(meta.target().into()));
        if let Some(message) = fields.remove("message") {
            line.insert("msg".into(), message);
        }
        if !fields.is_empty() {
            line.insert("fields".into(), Value::Object(fields));
        }
        if let Some(scope) = ctx.event_scope(event) {
            let spans: Vec<Value> = scope
                .from_root()
                .map(|span| {
                    let mut obj = Map::new();
                    obj.insert("name".into(), Value::String(span.name().into()));
                    if let Some(SpanFields(f)) = span.extensions().get::<SpanFields>() {
                        obj.extend(f.clone());
                    }
                    Value::Object(obj)
                })
                .collect();
            if !spans.is_empty() {
                line.insert("spans".into(), Value::Array(spans));
            }
        }
        if let Some(name) = std::thread::current().name() {
            line.insert("thread".into(), Value::String(name.into()));
        }
        write_line(&Value::Object(line).to_string());
    }
}

/// `t` as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn rfc3339(t: SystemTime) -> String {
    let ms = t
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    let (secs, milli) = (ms / 1000, ms % 1000);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    let (y, mo, d) = crate::autosave::civil_from_days(i64::try_from(days).unwrap_or(0));
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{milli:03}Z")
}

// ----- the crash marker ------------------------------------------------------------------------

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write_crash_marker(info);
        previous(info);
    }));
}

fn write_crash_marker(info: &std::panic::PanicHookInfo<'_>) {
    let message: String = info
        .payload_as_str()
        .unwrap_or("(non-string panic payload)")
        .chars()
        .take(MAX_CRASH_MESSAGE)
        .collect();
    let location = info
        .location()
        .map(|l| format!("{}:{}", l.file(), l.line()));
    let thread = std::thread::current().name().map(str::to_owned);
    let now = rfc3339(SystemTime::now());
    // `try_lock`: the panic may have happened while this thread held the sink.
    let Ok(mut sink) = SINK.try_lock() else {
        return;
    };
    let Some(dir) = sink.dir.clone() else {
        return;
    };
    let pid = std::process::id();
    let marker = json!({
        "event": "crash",
        "pid": pid,
        "version": crate::engine::VERSION,
        "ts": now,
        "thread": thread,
        "message": message,
        "location": location,
    });
    let line = json!({
        "ts": now,
        "level": "ERROR",
        "target": "cutweave_engine::panic",
        "msg": "panic",
        "fields": {"message": message, "location": location},
        "thread": thread,
    });
    if let Some(file) = &mut sink.file {
        let _ = file.write_line(&line.to_string());
    }
    drop(sink);
    let _ = fs::write(crash_marker_path(&dir, pid), marker.to_string());
}

/// Keeps the newest crash markers only.
fn prune_crash_markers(dir: &Path) {
    let mut markers: Vec<(SystemTime, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("crash-") && has_ext(n, "json"))
        })
        .map(|e| {
            let modified = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            (modified, e.path())
        })
        .collect();
    markers.sort_by_key(|m| std::cmp::Reverse(m.0));
    for (_, old) in markers.into_iter().skip(KEEP_CRASH_MARKERS) {
        let _ = fs::remove_file(old);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cutweave-log-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rotates_by_size_and_keeps_five_files() {
        let dir = temp("rotate");
        let mut rf = RotatingFile::open(&dir, "engine-7", 100, 5).unwrap();
        let line = "x".repeat(40);
        for _ in 0..40 {
            rf.write_line(&line).unwrap();
        }
        let mut files: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files.len(), 5, "{files:?}");
        assert!(files.contains(&"engine-7.log".to_owned()));
        for f in &files {
            let len = fs::metadata(dir.join(f)).unwrap().len();
            assert!(len <= 100, "{f} is {len} bytes");
        }
        // Every archived line is whole.
        for f in &files {
            let text = fs::read_to_string(dir.join(f)).unwrap();
            assert!(text.lines().all(|l| l == line), "{f}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_spares_the_active_file_and_other_files() {
        let dir = temp("prune");
        for i in 0..8 {
            fs::write(dir.join(format!("engine-{i}.log")), "old\n").unwrap();
        }
        fs::write(dir.join("crash-1.json"), "{}").unwrap();
        let rf = RotatingFile::open(&dir, "engine-99", 1000, 5).unwrap();
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"engine-99.log".to_owned()));
        assert!(names.contains(&"crash-1.json".to_owned()));
        assert_eq!(names.iter().filter(|n| n.starts_with("engine-")).count(), 5);
        drop(rf);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        let t = UNIX_EPOCH + std::time::Duration::from_millis(1_779_531_330_123);
        assert_eq!(rfc3339(t), "2026-05-23T10:15:30.123Z");
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    }
}
