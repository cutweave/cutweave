// SPDX-License-Identifier: GPL-3.0-or-later
//! Crash-safe file replacement: temp file in the same directory, `fsync`, rename over the target,
//! `fsync` the directory (arch §17). A reader (or a crash) sees either the complete old file or the
//! complete new one, never a partial file.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

/// The four durable steps of an atomic write. Failpoints fire after each one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The temp file is written (not yet synced).
    TempWritten,
    /// The temp file is `fsync`ed.
    TempSynced,
    /// The temp file replaced the target.
    Renamed,
    /// The directory entry is `fsync`ed.
    DirSynced,
}

/// Name of the temp file used for `target`: `.<name>.tmp` next to it.
pub(crate) fn temp_path_for(target: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(target.file_name().unwrap_or_default());
    name.push(".tmp");
    target.with_file_name(name)
}

/// Atomically replaces (or creates) `target` with `bytes`. On failure the temp file is removed and
/// the previous `target` is untouched.
pub(crate) fn write_atomic(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let result = write_atomic_with(target, bytes, &mut |_| Ok(()));
    if result.is_err() {
        let _ = fs::remove_file(temp_path_for(target));
    }
    result
}

/// [`write_atomic`] with a hook called after each [`Step`]. A hook error returns immediately with
/// no cleanup, which models the process being killed at that point (used by the failpoint tests).
pub(crate) fn write_atomic_with(
    target: &Path,
    bytes: &[u8],
    hook: &mut dyn FnMut(Step) -> io::Result<()>,
) -> io::Result<()> {
    let temp = temp_path_for(target);
    // We hold the project lock, so a leftover temp from a crash is ours to overwrite.
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp)?;
    file.write_all(bytes)?;
    hook(Step::TempWritten)?;
    file.sync_all()?;
    drop(file);
    hook(Step::TempSynced)?;
    rename_replace(&temp, target)?;
    hook(Step::Renamed)?;
    sync_parent_dir(target);
    hook(Step::DirSynced)?;
    Ok(())
}

/// Renames over an existing file. `std::fs::rename` maps to `MoveFileExW(REPLACE_EXISTING)` on
/// Windows, which can transiently fail with a sharing violation while a virus scanner or indexer
/// holds the target open, so that case is retried with a short backoff.
fn rename_replace(from: &Path, to: &Path) -> io::Result<()> {
    let attempts: u64 = if cfg!(windows) { 12 } else { 1 };
    let mut last = None;
    for attempt in 0..attempts {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if cfg!(windows) && is_transient_windows(&e) => {
                last = Some(e);
                thread::sleep(Duration::from_millis(15 * (attempt + 1)));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("rename failed")))
}

fn is_transient_windows(e: &io::Error) -> bool {
    // 5 = ERROR_ACCESS_DENIED, 32 = ERROR_SHARING_VIOLATION, 33 = ERROR_LOCK_VIOLATION.
    e.kind() == io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// Makes the rename itself durable. POSIX: `fsync` the directory. Windows cannot flush a directory
/// through std without FFI (this crate forbids `unsafe`; NTFS journals the metadata), so it is
/// best effort there. Failure never fails the save: the contents are already durable and complete.
fn sync_parent_dir(target: &Path) {
    let Some(dir) = target.parent() else { return };
    #[cfg(unix)]
    {
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        if let Ok(d) = OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(dir)
        {
            let _ = d.sync_all();
        }
    }
    #[cfg(not(any(unix, windows)))]
    let _ = dir;
}
