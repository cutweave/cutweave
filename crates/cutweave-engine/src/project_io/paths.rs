// SPDX-License-Identifier: GPL-3.0-or-later
//! Project folder layout, name validation and relative-path math.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf, Prefix};

use super::ProjectIoError;

/// File extension of the project document.
pub const DOCUMENT_EXTENSION: &str = "cutweave";

/// The fixed locations inside a project folder (arch §17). Every write the store performs goes
/// through one of these, so nothing is ever written outside `root`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectLayout {
    /// The project folder.
    pub root: PathBuf,
    /// The `.cutweave` document.
    pub document: PathBuf,
}

impl ProjectLayout {
    /// Layout for a new/known project folder: `<root>/<name>.cutweave`, `name` = folder name.
    pub fn for_folder(root: &Path) -> Result<ProjectLayout, ProjectIoError> {
        // Validate the name as given: `absolute` normalizes away trailing dots and rewrites
        // device names on Windows, which would hide exactly the names we must reject.
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| ProjectIoError::invalid_name("", "folder has no usable name"))?
            .to_owned();
        validate_name(&name)?;
        let root = std::path::absolute(root).map_err(|e| ProjectIoError::io("resolve", root, e))?;
        let document = root.join(format!("{name}.{DOCUMENT_EXTENSION}"));
        Ok(ProjectLayout { root, document })
    }

    /// Layout for an existing document file.
    pub fn for_document(document: &Path) -> Result<ProjectLayout, ProjectIoError> {
        let document = std::path::absolute(document)
            .map_err(|e| ProjectIoError::io("resolve", document, e))?;
        let ext_ok = document
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(DOCUMENT_EXTENSION));
        let root = document.parent().map(Path::to_path_buf);
        match (ext_ok, root) {
            (true, Some(root)) => Ok(ProjectLayout { root, document }),
            _ => Err(ProjectIoError::invalid_name(
                &document.to_string_lossy(),
                "not a .cutweave document",
            )),
        }
    }

    /// `<root>/.cutweave`.
    #[must_use]
    pub fn meta_dir(&self) -> PathBuf {
        self.root.join(".cutweave")
    }
    /// `<root>/.cutweave/journal` (WP-1C.02).
    #[must_use]
    pub fn journal_dir(&self) -> PathBuf {
        self.meta_dir().join("journal")
    }
    /// `<root>/.cutweave/backups` (WP-1C.03).
    #[must_use]
    pub fn backups_dir(&self) -> PathBuf {
        self.meta_dir().join("backups")
    }
    /// `<root>/.cutweave/lock`: the lock holder record.
    #[must_use]
    pub fn lock_file(&self) -> PathBuf {
        self.meta_dir().join("lock")
    }
    /// `<root>/.cutweave/ui-state.json` (WP-1C.04).
    #[must_use]
    pub fn ui_state_file(&self) -> PathBuf {
        self.meta_dir().join("ui-state.json")
    }
    /// `<root>/generated`.
    #[must_use]
    pub fn generated_dir(&self) -> PathBuf {
        self.root.join("generated")
    }
    /// `<root>/compositions`.
    #[must_use]
    pub fn compositions_dir(&self) -> PathBuf {
        self.root.join("compositions")
    }

    /// Fails unless `path` is lexically inside the project folder (no `..` escapes).
    pub fn ensure_inside(&self, path: &Path) -> Result<(), ProjectIoError> {
        let escapes = path.components().any(|c| matches!(c, Component::ParentDir));
        if escapes || !path.starts_with(&self.root) {
            return Err(ProjectIoError::OutsideProject(path.to_path_buf()));
        }
        Ok(())
    }
}

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A project name becomes a folder and file name on every OS, so it must be valid on all of them.
pub fn validate_name(name: &str) -> Result<(), ProjectIoError> {
    let bad = |why: &str| Err(ProjectIoError::invalid_name(name, why));
    if name.is_empty() || name == "." || name == ".." {
        return bad("empty or a dot name");
    }
    if name.chars().count() > 200 {
        return bad("longer than 200 characters");
    }
    if name
        .chars()
        .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return bad("contains a character not allowed in file names");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return bad("ends with a dot or space");
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return bad("is a reserved device name on Windows");
    }
    Ok(())
}

fn key(component: Component<'_>) -> Option<OsString> {
    Some(match component {
        Component::Prefix(p) => match p.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
                OsString::from(format!("{}:", char::from(d.to_ascii_uppercase())))
            }
            _ => p.as_os_str().to_owned(),
        },
        Component::RootDir => OsString::from("/"),
        Component::Normal(s) => s.to_owned(),
        Component::CurDir | Component::ParentDir => return None,
    })
}

fn normalize(path: &Path) -> Option<Vec<OsString>> {
    let mut out: Vec<OsString> = Vec::new();
    let mut anchor = 0;
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if out.len() <= anchor {
                    return None;
                }
                out.pop();
            }
            other => {
                let is_anchor = matches!(other, Component::Prefix(_) | Component::RootDir);
                out.push(key(other)?);
                if is_anchor {
                    anchor = out.len();
                }
            }
        }
    }
    Some(out)
}

fn same(a: &OsString, b: &OsString) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// `target` relative to the directory `base`, with `/` separators, or `None` when either is not
/// absolute or they share no root (different drives). Lexical: no filesystem access.
#[must_use]
pub fn relative_to(base: &Path, target: &Path) -> Option<String> {
    if !base.is_absolute() || !target.is_absolute() {
        return None;
    }
    let b = normalize(base)?;
    let t = normalize(target)?;
    let common = b.iter().zip(&t).take_while(|(x, y)| same(x, y)).count();
    // The anchor (drive / root) must match, otherwise there is no relative path.
    let anchor_len = base
        .components()
        .take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
        .count();
    if common < anchor_len {
        return None;
    }
    let mut parts: Vec<String> = vec!["..".to_owned(); b.len() - common];
    parts.extend(t[common..].iter().map(|s| s.to_string_lossy().into_owned()));
    if parts.is_empty() {
        return Some(".".to_owned());
    }
    Some(parts.join("/"))
}

/// Resolves a stored `path_rel` against the project folder. Rejects absolute paths and
/// drive-relative forms; `..` is allowed (media may live beside the project folder).
#[must_use]
pub fn resolve_relative(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.starts_with(['/', '\\']) {
        return None;
    }
    let mut out = root.to_path_buf();
    for part in rel.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => out.push(".."),
            p if p.contains(':') => return None,
            p => out.push(p),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["My Video", "Vidéo été 日本語", "a.b", "COM10", "x"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", ".", "..", "a/b", "a\\b", "con", "NUL.txt", "a.", "a ", "a:b", "q?", "t\u{7}",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn relative_windows() {
        let base = Path::new(r"C:\Users\me\My Video");
        assert_eq!(
            relative_to(base, Path::new(r"c:\users\me\My Video\generated\a.wav")).as_deref(),
            Some("generated/a.wav")
        );
        assert_eq!(
            relative_to(base, Path::new(r"C:\Users\me\Footage\a.mp4")).as_deref(),
            Some("../Footage/a.mp4")
        );
        assert_eq!(relative_to(base, Path::new(r"D:\a.mp4")), None);
        assert_eq!(relative_to(base, Path::new("a.mp4")), None);
        assert_eq!(
            relative_to(base, Path::new(r"\\?\C:\Users\me\My Video\x.mov")).as_deref(),
            Some("x.mov")
        );
    }

    #[cfg(unix)]
    #[test]
    fn relative_unix() {
        let base = Path::new("/home/me/My Video");
        assert_eq!(
            relative_to(base, Path::new("/home/me/My Video/generated/a.wav")).as_deref(),
            Some("generated/a.wav")
        );
        assert_eq!(
            relative_to(base, Path::new("/home/me/Footage/./x/../a.mp4")).as_deref(),
            Some("../Footage/a.mp4")
        );
        assert_eq!(relative_to(base, base).as_deref(), Some("."));
    }

    #[test]
    fn resolve_rejects_absolute_forms() {
        let root = Path::new("/p");
        assert!(resolve_relative(root, "/etc/passwd").is_none());
        assert!(resolve_relative(root, "\\evil").is_none());
        assert!(resolve_relative(root, "C:evil").is_none());
        assert!(resolve_relative(root, "").is_none());
        assert_eq!(
            resolve_relative(root, "a/../b"),
            Some(PathBuf::from("/p/a/../b"))
        );
    }
}
