// SPDX-License-Identifier: GPL-3.0-or-later
//! Timestamped autosave backups in `.cutweave/backups/` (WP-1C.03).
//!
//! Files are named `autosave-<YYYYMMDDTHHMMSS.mmm>Z.cutweave` (UTC, fixed width), so a plain
//! name sort is chronological. Only files with that shape are ever listed or deleted; other
//! residents of the folder (pre-migration originals, retired journal segments) are left alone.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cutweave_commands::WallTime;

use crate::project_io::atomic::write_atomic;

const PREFIX: &str = "autosave-";
const SUFFIX: &str = "Z.cutweave";
/// `YYYYMMDDTHHMMSS.mmm`
const STAMP_LEN: usize = 19;

/// Formats `t` as `YYYYMMDDTHHMMSS.mmm` (UTC).
#[must_use]
#[allow(clippy::many_single_char_names)]
pub(crate) fn stamp(t: WallTime) -> String {
    let ms = t.as_millis();
    let (secs, milli) = (ms / 1000, ms % 1000);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    let (y, mo, d) = civil_from_days(i64::try_from(days).unwrap_or(0));
    format!("{y:04}{mo:02}{d:02}T{h:02}{m:02}{s:02}.{milli:03}")
}

/// Days since 1970-01-01 to (year, month, day) (Hinnant's algorithm).
#[allow(clippy::many_single_char_names)]
pub(crate) fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn is_backup_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(PREFIX) else {
        return false;
    };
    let Some(stamp) = rest.strip_suffix(SUFFIX) else {
        return false;
    };
    stamp.len() == STAMP_LEN
        && stamp.bytes().enumerate().all(|(i, b)| match i {
            8 => b == b'T',
            15 => b == b'.',
            _ => b.is_ascii_digit(),
        })
}

/// The autosave backups in `dir`, oldest first.
pub(crate) fn list(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_str().is_some_and(is_backup_name))
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// Writes `bytes` as a new backup stamped `now` (nudged forward a millisecond at a time if
/// that name is taken), then deletes the oldest backups beyond `keep`. Returns the new file.
pub(crate) fn add(dir: &Path, now: WallTime, bytes: &[u8], keep: usize) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let mut t = now;
    let path = loop {
        let candidate = dir.join(format!("{PREFIX}{}{SUFFIX}", stamp(t)));
        if !candidate.exists() {
            break candidate;
        }
        t = WallTime::from_millis(t.as_millis() + 1);
    };
    write_atomic(&path, bytes)?;
    let all = list(dir);
    let excess = all.len().saturating_sub(keep.max(1));
    for old in all.into_iter().take(excess) {
        if let Err(e) = fs::remove_file(&old)
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(path = %old.display(), error = %e, "could not delete an old backup");
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_utc_and_sortable() {
        assert_eq!(stamp(WallTime::UNIX_EPOCH), "19700101T000000.000");
        // 2026-05-23T10:15:30.123Z
        assert_eq!(
            stamp(WallTime::from_millis(1_779_531_330_123)),
            "20260523T101530.123"
        );
        assert!(is_backup_name("autosave-19700101T000000.000Z.cutweave"));
        assert!(!is_backup_name("a.cutweave.pre-migration-v1"));
        assert!(!is_backup_name("autosave-x.cutweave"));
    }
}
