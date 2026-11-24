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

/// Which kind of engine wrote a run-file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    /// A desktop app's engine (it has a window, so it has a "last focused" time).
    Gui,
    /// A script's or service's engine (no UI credential).
    #[default]
    Headless,
}

impl RunMode {
    /// `gui` or `headless`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RunMode::Gui => "gui",
            RunMode::Headless => "headless",
        }
    }
}

/// The project an engine has open, as the run-file records it (WP-1J.04).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFileProject {
    /// The project's name (from its document).
    pub name: String,
    /// The project folder.
    pub path: String,
}

impl RunFileProject {
    /// A project known only by its path; the name is the folder's name (the path of a
    /// `.cutweave` document stands for its folder).
    #[must_use]
    pub fn from_path(path: &str) -> RunFileProject {
        let p = Path::new(path);
        let folder = if p.extension().is_some_and(|e| e == "cutweave") {
            p.parent().unwrap_or(p)
        } else {
            p
        };
        let name = folder
            .file_name()
            .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned());
        RunFileProject {
            name,
            path: folder.to_string_lossy().into_owned(),
        }
    }

    /// Whether `wanted` names this project: its name or its folder's name (case-insensitive),
    /// or its folder or its document, given as a path.
    #[must_use]
    pub fn matches(&self, wanted: &str) -> bool {
        let wanted = wanted.trim();
        if wanted.is_empty() {
            return false;
        }
        let mine = Path::new(&self.path);
        if self.name.eq_ignore_ascii_case(wanted)
            || mine
                .file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(wanted))
        {
            return true;
        }
        let wanted = Path::new(wanted);
        let same = |a: &Path, b: &Path| match (fs::canonicalize(a), fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => same_text(a, b),
        };
        same(wanted, mine) || wanted.parent().is_some_and(|dir| same(dir, mine))
    }
}

/// Paths compared as text when they cannot be resolved (separators and case folded: the
/// filesystems of Windows and macOS ignore case).
fn same_text(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// Reads `project` as an object, or (run-files before WP-1J.04) as a bare path.
fn project_field<'de, D>(d: D) -> Result<Option<RunFileProject>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Field {
        Full(RunFileProject),
        Path(String),
    }
    Ok(Option::<Field>::deserialize(d)?.map(|f| match f {
        Field::Full(p) => p,
        Field::Path(path) => RunFileProject::from_path(&path),
    }))
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
    /// The open project, if any (kept current as projects are opened and closed).
    #[serde(default, deserialize_with = "project_field")]
    pub project: Option<RunFileProject>,
    /// `gui` for a desktop app's engine (it has a UI credential), else `headless`.
    #[serde(default)]
    pub mode: RunMode,
    /// GUI engines: when the window last gained focus, milliseconds since the Unix epoch.
    /// Absent until it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_at: Option<u64>,
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
    /// The write is atomic: the content goes to a private `<pid>.tmp` first and is renamed over
    /// the run-file, so a reader sees the old file or the new one, never none or half of one
    /// (engines rewrite their run-file as the focus and the project change).
    ///
    /// Fails closed: if the permissions cannot be restricted, the file is removed and an
    /// error returned, so a token is never left readable by other users.
    pub fn write(dir: &Path, contents: &RunFileContents) -> io::Result<RunFile> {
        create_private_dir(dir)?;
        let path = dir.join(format!("{}.json", contents.pid));
        let tmp = dir.join(format!("{}.tmp", contents.pid));
        let _ = fs::remove_file(&tmp);
        let mut file = create_private_file(&tmp)?;
        let result = serde_json::to_vec_pretty(contents)
            .map_err(io::Error::other)
            .and_then(|bytes| file.write_all(&bytes))
            .and_then(|()| file.sync_all());
        drop(file);
        if let Err(e) = result.and_then(|()| replace_file(&tmp, &path)) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(RunFile { path })
    }

    /// Reads the run-file at `path`, applies `change` and writes it back atomically. A
    /// run-file that is gone (the engine is stopping) is not recreated; `change` returns
    /// whether anything changed (nothing is written otherwise).
    pub fn update(
        path: &Path,
        change: impl FnOnce(&mut RunFileContents) -> bool,
    ) -> io::Result<()> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let mut contents: RunFileContents =
            serde_json::from_str(&text).map_err(io::Error::other)?;
        if !change(&mut contents) {
            return Ok(());
        }
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::other("the run-file has no folder"))?;
        RunFile::write(dir, &contents).map(|_| ())
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

/// Renames `from` over `to`. On Windows a reader that has the target open can make the first
/// attempts fail; they are retried for a moment.
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    let mut last = None;
    for attempt in 0..20 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(5 * (attempt + 1)));
    }
    Err(last.unwrap_or_else(|| io::Error::other("cannot replace the run-file")))
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
    // The user's SID does not change while the process runs; a rewrite per focus change should
    // not start `whoami` every time.
    static SID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let sid = if let Some(sid) = SID.get() {
        sid.clone()
    } else {
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
        SID.get_or_init(|| sid).clone()
    };
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
            mode: RunMode::Headless,
            focused_at: None,
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
    fn a_run_file_from_before_the_project_object_still_reads() {
        let old = r#"{"pid":5,"port":1,"agent_tokens":[],"project":"/work/film.cutweave","started":1,"version":"0"}"#;
        let c: RunFileContents = serde_json::from_str(old).unwrap();
        let p = c.project.unwrap();
        assert_eq!((p.name.as_str(), p.path.as_str()), ("work", "/work"));
        assert_eq!(c.mode, RunMode::Headless);
        assert_eq!(c.focused_at, None);
    }

    #[test]
    fn a_project_matches_by_name_folder_or_document() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("My Film");
        fs::create_dir_all(&folder).unwrap();
        let p = RunFileProject {
            name: "Final Cut".into(),
            path: folder.to_string_lossy().into_owned(),
        };
        assert!(p.matches("final cut") && p.matches("MY FILM"));
        assert!(p.matches(folder.to_str().unwrap()));
        assert!(p.matches(folder.join("project.cutweave").to_str().unwrap()));
        assert!(!p.matches("film") && !p.matches("") && !p.matches(dir.path().to_str().unwrap()));
    }

    #[test]
    fn update_rewrites_in_place_and_never_recreates_a_removed_file() {
        let dir = tempfile::tempdir().unwrap();
        let rf = RunFile::write(dir.path(), &sample(std::process::id())).unwrap();
        RunFile::update(rf.path(), |c| {
            c.focused_at = Some(42);
            true
        })
        .unwrap();
        assert_eq!(RunFile::discover(dir.path())[0].focused_at, Some(42));
        // Nothing but the run-file is left in the folder (no temp file).
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        // No change, no write.
        let before = fs::metadata(rf.path()).unwrap().modified().unwrap();
        RunFile::update(rf.path(), |_| false).unwrap();
        assert_eq!(fs::metadata(rf.path()).unwrap().modified().unwrap(), before);
        rf.remove();
        RunFile::update(rf.path(), |_| true).unwrap();
        assert!(!rf.path().exists());
    }

    #[test]
    fn readers_never_see_a_missing_or_half_written_file_during_rewrites() {
        let dir = tempfile::tempdir().unwrap();
        let rf = RunFile::write(dir.path(), &sample(std::process::id())).unwrap();
        let path = rf.path().to_path_buf();
        let writer = std::thread::spawn(move || {
            for i in 0..25 {
                RunFile::update(&path, |c| {
                    c.focused_at = Some(i);
                    true
                })
                .unwrap();
            }
        });
        while !writer.is_finished() {
            let found = RunFile::discover(dir.path());
            assert_eq!(found.len(), 1, "the run-file vanished during a rewrite");
        }
        writer.join().unwrap();
    }

    #[test]
    fn debug_redacts_tokens() {
        let s = format!("{:?}", sample(1));
        assert!(!s.contains("tttt"));
    }
}
