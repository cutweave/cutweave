// SPDX-License-Identifier: GPL-3.0-or-later
//! The single-writer project lock (arch §17).
//!
//! Two files in `.cutweave/`:
//!
//! - `lock.guard` carries an OS advisory lock (`File::try_lock`: `flock` / `LockFileEx`) held for
//!   the life of the store. The OS drops it when the process exits *or is killed*, so a crashed
//!   editor can never leave the project permanently locked, and a live holder can never be
//!   stolen from. This is the stale-lock detector: "pid dead" is exactly "guard acquirable".
//! - `lock` is the human/tool-readable holder record `{pid, host, started}`. It is separate from
//!   the guard because Windows byte-range locks also block other handles from *reading* the
//!   locked file, which would hide who holds the project.
//!
//! A leftover `lock` record whose guard is free is a stale lock from a crash: it is taken over and
//! reported. A record from a different host is never taken over silently (a synced or network
//! folder cannot prove the other machine is gone); the caller must opt in.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::ProjectIoError;
use super::atomic::write_atomic;
use super::paths::ProjectLayout;

/// Who holds (or held) a project lock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockInfo {
    /// Process id of the holder.
    pub pid: u32,
    /// Host name of the holder.
    pub host: String,
    /// When the lock was taken, seconds since the Unix epoch.
    pub started: u64,
}

impl LockInfo {
    /// The record for this process, now.
    #[must_use]
    pub fn current() -> LockInfo {
        LockInfo {
            pid: std::process::id(),
            host: host_name(),
            started: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        }
    }
}

/// Why the project could not be locked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockedReason {
    /// A live process holds the OS lock (this machine, or a lock-coherent network share).
    HeldByLiveProcess,
    /// A leftover record from another host: its liveness cannot be checked from here.
    ForeignHost,
}

/// Best-effort host name without extra dependencies.
#[must_use]
pub fn host_name() -> String {
    for var in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(v) = std::env::var(var)
            && !v.trim().is_empty()
        {
            return v.trim().to_owned();
        }
    }
    if let Ok(v) = fs::read_to_string("/etc/hostname")
        && !v.trim().is_empty()
    {
        return v.trim().to_owned();
    }
    "unknown".to_owned()
}

/// A held lock. Released by [`ProjectLock::release`] or on drop.
#[derive(Debug)]
pub(crate) struct ProjectLock {
    guard: Option<File>,
    layout: ProjectLayout,
    info: LockInfo,
}

/// A successful acquisition.
#[derive(Debug)]
pub(crate) struct Acquired {
    pub lock: ProjectLock,
    /// The record of a crashed previous holder that was taken over, if any.
    pub stale: Option<LockInfo>,
}

fn read_record(layout: &ProjectLayout) -> Option<LockInfo> {
    let text = fs::read_to_string(layout.lock_file()).ok()?;
    serde_json::from_str(&text).ok()
}

impl ProjectLock {
    /// Acquires the lock for `layout`, creating `.cutweave/{journal,backups}` as needed.
    /// `steal_foreign`: take over a record left by another host.
    pub(crate) fn acquire(
        layout: &ProjectLayout,
        steal_foreign: bool,
    ) -> Result<Acquired, ProjectIoError> {
        let io_err = |op, path: &std::path::Path, e| ProjectIoError::io(op, path, e);
        for dir in [layout.journal_dir(), layout.backups_dir()] {
            layout.ensure_inside(&dir)?;
            fs::create_dir_all(&dir).map_err(|e| io_err("create directory", &dir, e))?;
        }
        let guard_path = layout.meta_dir().join("lock.guard");
        let guard = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&guard_path)
            .map_err(|e| io_err("open lock", &guard_path, e))?;
        match guard.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(ProjectIoError::Locked {
                    holder: read_record(layout),
                    reason: LockedReason::HeldByLiveProcess,
                });
            }
            Err(TryLockError::Error(e)) => return Err(io_err("lock", &guard_path, e)),
        }
        let me = LockInfo::current();
        let previous = read_record(layout);
        if let Some(prev) = &previous
            && prev.host != me.host
            && !steal_foreign
        {
            return Err(ProjectIoError::Locked {
                holder: previous,
                reason: LockedReason::ForeignHost,
            });
        }
        let text = serde_json::to_string_pretty(&me).map_err(ProjectIoError::Serialize)?;
        write_atomic(&layout.lock_file(), text.as_bytes())
            .map_err(|e| io_err("write lock", &layout.lock_file(), e))?;
        Ok(Acquired {
            lock: ProjectLock {
                guard: Some(guard),
                layout: layout.clone(),
                info: me,
            },
            stale: previous,
        })
    }

    pub(crate) fn info(&self) -> &LockInfo {
        &self.info
    }

    /// Releases the lock: removes the record, then drops the OS lock.
    pub(crate) fn release(&mut self) -> io::Result<()> {
        let Some(guard) = self.guard.take() else {
            return Ok(());
        };
        let removed = match fs::remove_file(self.layout.lock_file()) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
        let _ = guard.unlock();
        removed
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        let _ = self.release();
    }
}
