// SPDX-License-Identifier: GPL-3.0-or-later
//! FFmpeg's log in `tracing` (target `ffmpeg`), rate limited ([`crate::log_filter`]).
//!
//! Levels: `AV_LOG_PANIC`/`FATAL` → `error`; `ERROR`/`WARNING` → `warn` (FFmpeg reports
//! recoverable stream damage as errors; real failures reach callers as `MediaError`);
//! `INFO` → `info`; `VERBOSE` → `debug`; `DEBUG`/`TRACE` → `trace`. FFmpeg's own threshold
//! (`av_log_set_level`, default `INFO`) still applies first, and nothing is formatted unless a
//! subscriber wants the level.

use std::cell::RefCell;
use std::ffi::c_int;
use std::sync::{Mutex, PoisonError};

use ffmpeg_next::ffi;
use tracing::Level;

use crate::log_filter::{RepeatFilter, Verdict};

/// The `tracing` target of FFmpeg's log lines.
pub(crate) const TARGET: &str = "ffmpeg";

static FILTER: Mutex<RepeatFilter> = Mutex::new(RepeatFilter::new());

thread_local! {
    /// A line FFmpeg is still writing in pieces (fragments without a trailing newline).
    static PARTIAL: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The `tracing` level for an FFmpeg level, or `None` for `AV_LOG_QUIET`.
pub(super) fn tracing_level(level: c_int) -> Option<Level> {
    Some(match level {
        l if l < ffi::AV_LOG_PANIC => return None,
        l if l <= ffi::AV_LOG_FATAL => Level::ERROR,
        l if l <= ffi::AV_LOG_WARNING => Level::WARN,
        l if l <= ffi::AV_LOG_INFO => Level::INFO,
        l if l <= ffi::AV_LOG_VERBOSE => Level::DEBUG,
        _ => Level::TRACE,
    })
}

/// Whether a subscriber wants FFmpeg lines at `level` (checked before formatting).
pub(super) fn enabled(level: c_int) -> bool {
    match tracing_level(level) {
        Some(Level::ERROR) => tracing::enabled!(target: TARGET, Level::ERROR),
        Some(Level::WARN) => tracing::enabled!(target: TARGET, Level::WARN),
        Some(Level::INFO) => tracing::enabled!(target: TARGET, Level::INFO),
        Some(Level::DEBUG) => tracing::enabled!(target: TARGET, Level::DEBUG),
        Some(_) => tracing::enabled!(target: TARGET, Level::TRACE),
        None => false,
    }
}

/// One formatted piece of an FFmpeg log line. Pieces are joined per thread until a newline.
pub(super) fn on_fragment(level: c_int, fragment: &str) {
    let line = PARTIAL.try_with(|partial| {
        let mut partial = partial.try_borrow_mut().ok()?;
        partial.push_str(fragment);
        partial
            .ends_with('\n')
            .then(|| std::mem::take(&mut *partial))
    });
    if let Ok(Some(line)) = line {
        emit(level, line.trim_end());
    }
}

fn emit(level: c_int, line: &str) {
    let Some(level) = tracing_level(level) else {
        return;
    };
    if line.is_empty() {
        return;
    }
    let verdict = FILTER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .check(line);
    match verdict {
        Verdict::Emit => event(level, line),
        Verdict::EmitRepeated(n) => event(level, &format!("{line} (repeated {n} times)")),
        Verdict::Suppress => {}
    }
}

fn event(level: Level, line: &str) {
    match level {
        Level::ERROR => tracing::error!(target: TARGET, "{line}"),
        Level::WARN => tracing::warn!(target: TARGET, "{line}"),
        Level::INFO => tracing::info!(target: TARGET, "{line}"),
        Level::DEBUG => tracing::debug!(target: TARGET, "{line}"),
        _ => tracing::trace!(target: TARGET, "{line}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_map_to_tracing() {
        assert_eq!(tracing_level(ffi::AV_LOG_QUIET), None);
        assert_eq!(tracing_level(ffi::AV_LOG_PANIC), Some(Level::ERROR));
        assert_eq!(tracing_level(ffi::AV_LOG_FATAL), Some(Level::ERROR));
        assert_eq!(tracing_level(ffi::AV_LOG_ERROR), Some(Level::WARN));
        assert_eq!(tracing_level(ffi::AV_LOG_WARNING), Some(Level::WARN));
        assert_eq!(tracing_level(ffi::AV_LOG_INFO), Some(Level::INFO));
        assert_eq!(tracing_level(ffi::AV_LOG_VERBOSE), Some(Level::DEBUG));
        assert_eq!(tracing_level(ffi::AV_LOG_DEBUG), Some(Level::TRACE));
        assert_eq!(tracing_level(ffi::AV_LOG_TRACE), Some(Level::TRACE));
        // Context-specific offsets can push levels between the named ones.
        assert_eq!(tracing_level(ffi::AV_LOG_ERROR + 1), Some(Level::WARN));
    }
}
