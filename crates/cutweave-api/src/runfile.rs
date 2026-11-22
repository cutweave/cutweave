// SPDX-License-Identifier: GPL-3.0-or-later
//! The run-file `~/.cutweave/run/<pid>.json` (arch §6, WP-1I.01).
//!
//! It tells local clients (CLI, MCP proxy) how to attach: port, pid and the **agent** tokens.
//! It never contains the UI credential. It is created with user-only permissions (POSIX
//! `0600`; on Windows the DACL is reduced to the current user), removed on exit, and stale
//! files left by a crashed engine are ignored by [`RunFile::discover`] and reaped at startup.

use std::{
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// One agent token entry.
#[derive(Clone, Serialize, Deserialize)]
pub struct RunFileToken {
    /// The label (`cli`, `mcp`).
    pub label: String,
    /// The secret.
    pub token: String,
}

impl fmt::Debug for RunFileToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunFileToken")
            .field("label", &self.label)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// The run-file's JSON content.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunFileContents {
    /// Engine process id.
    pub pid: u32,
    /// The loopback port the server listens on.
    pub port: u16,
    /// Agent tokens (never the UI credential).
    pub agent_tokens: Vec<RunFileToken>,
    /// The open project, if any.
    pub project: Option<String>,
    /// Start time, seconds since the Unix epoch.
    pub started: u64,
    /// Engine version.
    pub version: String,
    /// Name of the shared-memory frame ring (arrives with WP-1I.05).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shm: Option<String>,
}

/// A written run-file; remove it with [`RunFile::remove`].
#[derive(Debug)]
pub struct RunFile {
    path: PathBuf,
}

impl RunFile {
    /// `~/.cutweave/run`, or `None` when no home directory is known.
    #[must_use]
    pub fn default_dir() -> Option<PathBuf> {
        let home = std::env::var_os("USERPROFILE")
            .filter(|v| !v.is_empty())
            .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()))?;
        Some(PathBuf::from(home).join(".cutweave").join("run"))
    }

    /// Writes `<dir>/<pid>.json` with user-only permissions.
    ///
    /// Fails closed: if the permissions cannot be restricted, the file is removed and an
    /// error returned, so a token is never left readable by other users.
    pub fn write(dir: &Path, contents: &RunFileContents) -> io::Result<RunFile> {
        create_private_dir(dir)?;
        let path = dir.join(format!("{}.json", contents.pid));
        let _ = fs::remove_file(&path);
        let mut file = create_private_file(&path)?;
        let result = serde_json::to_vec_pretty(contents)
            .map_err(io::Error::other)
            .and_then(|bytes| file.write_all(&bytes))
            .and_then(|()| file.sync_all());
        if let Err(e) = result {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(e);
        }
        Ok(RunFile { path })
    }

    /// The file's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Deletes the file (idempotent).
    pub fn remove(&self) {
        let _ = fs::remove_file(&self.path);
    }

    /// Run-files in `dir` whose engine is still alive (a `kill -9` leaves a stale file whose
    /// pid is dead; those are ignored). Unreadable or malformed files are ignored too.
    #[must_use]
    pub fn discover(dir: &Path) -> Vec<RunFileContents> {
        let mut found: Vec<RunFileContents> = entries(dir)
            .into_iter()
            .filter_map(|(_, pid)| {
                let text = fs::read_to_string(dir.join(format!("{pid}.json"))).ok()?;
                let c: RunFileContents = serde_json::from_str(&text).ok()?;
                (c.pid == pid && process_alive(pid)).then_some(c)
            })
            .collect();
        found.sort_by_key(|c| c.pid);
        found
    }

    /// Removes run-files whose engine is dead; returns how many were removed.
    #[must_use]
    pub fn reap_stale(dir: &Path) -> usize {
        entries(dir)
            .into_iter()
            .filter(|(_, pid)| *pid != std::process::id() && !process_alive(*pid))
            .filter(|(path, _)| fs::remove_file(path).is_ok())
            .count()
    }
}

/// `<pid>.json` files in `dir` as `(path, pid)`.
fn entries(dir: &Path) -> Vec<(PathBuf, u32)> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    read.filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "json" {
                return None;
            }
            let pid = path.file_stem()?.to_str()?.parse::<u32>().ok()?;
            Some((path, pid))
        })
        .collect()
}

/// Whether a process with `pid` exists.
#[must_use]
pub fn process_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    alive_impl(pid)
}

#[cfg(unix)]
fn alive_impl(pid: u32) -> bool {
    if Path::new("/proc/self").exists() {
        return Path::new(&format!("/proc/{pid}")).exists();
    }
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(windows)]
fn alive_impl(pid: u32) -> bool {
    let Ok(out) = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
    else {
        // Cannot tell: treat as alive so a live engine's file is never reaped.
        return true;
    };
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(windows)]
fn create_private_file(path: &Path) -> io::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    // The file is still empty here, so nothing secret is exposed while the DACL is replaced.
    if let Err(e) = restrict_to_current_user(path) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(e);
    }
    Ok(file)
}

/// Replaces the DACL with a single full-control entry for the current user (no inheritance).
/// Uses `whoami` and `icacls` so no `unsafe` Win32 calls are needed.
#[cfg(windows)]
fn restrict_to_current_user(path: &Path) -> io::Result<()> {
    let who = std::process::Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()?;
    let text = String::from_utf8_lossy(&who.stdout);
    let sid = text
        .trim()
        .rsplit(',')
        .next()
        .map(|s| s.trim().trim_matches('"').to_owned())
        .filter(|s| s.starts_with("S-1-"))
        .ok_or_else(|| io::Error::other("cannot determine the current user SID"))?;
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("*{sid}:F")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("icacls failed to restrict the run-file"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(pid: u32) -> RunFileContents {
        RunFileContents {
            pid,
            port: 1234,
            agent_tokens: vec![RunFileToken {
                label: "cli".into(),
                token: "t".repeat(64),
            }],
            project: None,
            started: 1,
            version: "0".into(),
            shm: None,
        }
    }

    #[test]
    fn write_discover_remove_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let rf = RunFile::write(dir.path(), &sample(std::process::id())).unwrap();
        let found = RunFile::discover(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].port, 1234);
        rf.remove();
        assert!(RunFile::discover(dir.path()).is_empty());
    }

    #[test]
    fn stale_file_is_ignored_and_reaped() {
        let dir = tempfile::tempdir().unwrap();
        // A pid that cannot belong to a live process.
        let dead = u32::MAX - 1;
        fs::write(
            dir.path().join(format!("{dead}.json")),
            serde_json::to_vec(&sample(dead)).unwrap(),
        )
        .unwrap();
        fs::write(dir.path().join("garbage.json"), b"{").unwrap();
        assert!(RunFile::discover(dir.path()).is_empty());
        assert_eq!(RunFile::reap_stale(dir.path()), 1);
        assert!(!dir.path().join(format!("{dead}.json")).exists());
    }

    #[test]
    fn debug_redacts_tokens() {
        let s = format!("{:?}", sample(1));
        assert!(!s.contains("tttt"));
    }
}
