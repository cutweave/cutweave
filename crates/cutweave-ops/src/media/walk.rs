// SPDX-License-Identifier: GPL-3.0-or-later
//! Walking a folder for `media.import` (WP-2F.09): which files a directory path stands for.
//!
//! The walk is depth-first in name order, so the same tree always imports in the same order.
//! It never follows a link (a symlinked folder or file, a Windows junction), skips hidden and
//! system entries, enters at most [`MAX_WALK_DEPTH`] levels below the folder it was given and
//! stops counting at a hard cap of files. Which files are media is the prober's call, not the
//! walk's.

use std::{
    fs,
    path::{Path, PathBuf},
};

use cutweave_model::{MAX_FOLDER_NAME_CHARS, folder_name_problem};

/// Deepest level of sub-folders below the given folder that the walk enters.
pub const MAX_WALK_DEPTH: usize = 16;
/// Most files one `media.import` call takes, counting those inside folders.
pub const MAX_IMPORT_FILES: usize = 2_000;
/// The walk stops collecting here (a tree this big is refused anyway).
const COUNT_CAP: usize = 100_000;

/// A file the walk found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    /// Absolute path.
    pub path: String,
    /// The names of the folders from the given folder down to the file's: the given folder's
    /// own name first. Already usable as bin folder names.
    pub dirs: Vec<String>,
}

/// What walking one folder gave.
#[derive(Debug, Default)]
pub(crate) struct Walked {
    /// The files, in walk order (at most the hard cap).
    pub files: Vec<Found>,
    /// Folders below [`MAX_WALK_DEPTH`] that were not entered.
    pub too_deep: u32,
    /// Folders or entries that could not be read.
    pub unreadable: u32,
    /// The walk stopped at the cap: there are more files than `files` holds.
    pub truncated: bool,
}

/// Whether `path` is a folder (a link to one counts: the person chose it).
pub(crate) fn is_folder(path: &str) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// A bin folder name for an OS entry name: no `/` or control characters, trimmed, at most 255
/// characters; "Folder" when nothing is left.
pub(crate) fn bin_folder_name(os_name: &str) -> String {
    let cleaned: String = os_name
        .chars()
        .map(|c| if c == '/' || c.is_control() { '_' } else { c })
        .take(MAX_FOLDER_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim().to_owned();
    if folder_name_problem(&cleaned).is_some() {
        "Folder".to_owned()
    } else {
        cleaned
    }
}

#[cfg(windows)]
fn is_hidden(name: &str, meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    name.starts_with('.') || meta.file_attributes() & (HIDDEN | SYSTEM) != 0
}

#[cfg(not(windows))]
fn is_hidden(name: &str, _meta: &fs::Metadata) -> bool {
    name.starts_with('.')
}

/// The files below `root`, each with the folder names that lead to it.
pub(crate) fn walk(root: &Path) -> Walked {
    let mut out = Walked::default();
    let name = root
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut dirs = vec![bin_folder_name(&name)];
    visit(root, 0, &mut dirs, &mut out);
    out
}

fn visit(dir: &Path, depth: usize, dirs: &mut Vec<String>, out: &mut Walked) {
    let Ok(read) = fs::read_dir(dir) else {
        out.unreadable += 1;
        return;
    };
    let mut entries: Vec<(String, PathBuf, fs::FileType)> = Vec::new();
    for entry in read {
        let Ok(entry) = entry else {
            out.unreadable += 1;
            continue;
        };
        let (Ok(kind), Ok(meta)) = (entry.file_type(), entry.metadata()) else {
            out.unreadable += 1;
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        // Links are never followed: a link could lead out of the folder the person chose (or
        // out of the allowed roots), or round in a circle.
        if kind.is_symlink() || is_hidden(&name, &meta) {
            continue;
        }
        entries.push((name, entry.path(), kind));
    }
    entries.sort_by_cached_key(|(name, ..)| (name.to_lowercase(), name.clone()));
    for (name, path, kind) in entries {
        if out.truncated {
            return;
        }
        if kind.is_dir() {
            if depth >= MAX_WALK_DEPTH {
                out.too_deep += 1;
                continue;
            }
            dirs.push(bin_folder_name(&name));
            visit(&path, depth + 1, dirs, out);
            dirs.pop();
        } else if kind.is_file() {
            let Some(path) = path.to_str() else {
                out.unreadable += 1;
                continue;
            };
            if out.files.len() >= COUNT_CAP {
                out.truncated = true;
                return;
            }
            out.files.push(Found {
                path: path.to_owned(),
                dirs: dirs.clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cutweave-walk-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap_or_default();
        dir
    }

    #[test]
    fn names_are_made_into_folder_names() {
        assert_eq!(bin_folder_name("Day 1"), "Day 1");
        assert_eq!(bin_folder_name("a/b\u{7}c"), "a_b_c");
        assert_eq!(bin_folder_name("  "), "Folder");
        assert_eq!(bin_folder_name(""), "Folder");
        assert_eq!(bin_folder_name(&"x".repeat(300)).chars().count(), 255);
    }

    #[test]
    fn the_walk_is_ordered_skips_hidden_files_and_counts_deep_folders() {
        let base = temp("order");
        let root = base.join("Shoot");
        fs::create_dir_all(root.join("b")).unwrap_or_default();
        fs::create_dir_all(root.join("A")).unwrap_or_default();
        for file in ["z.mp4", ".hidden.mp4", "A/one.mp4", "b/two.mp4"] {
            fs::write(root.join(file), b"x").unwrap_or_default();
        }
        let mut deep = root.clone();
        for i in 0..=MAX_WALK_DEPTH {
            deep = deep.join(format!("d{i}"));
        }
        fs::create_dir_all(&deep).unwrap_or_default();
        fs::write(deep.join("never.mp4"), b"x").unwrap_or_default();
        let walked = walk(&root);
        let seen: Vec<_> = walked
            .files
            .iter()
            .map(|f| {
                let file = f.path.rsplit(['/', '\\']).next().unwrap_or("").to_owned();
                (f.dirs.join("/"), file)
            })
            .collect();
        assert_eq!(seen[0], ("Shoot/A".to_owned(), "one.mp4".to_owned()));
        assert_eq!(seen[1], ("Shoot/b".to_owned(), "two.mp4".to_owned()));
        assert_eq!(seen[2].1, "z.mp4");
        // d0..d15 are entered (depth 16 below Shoot); d16 is not, so its file is never seen.
        assert_eq!(
            seen.len(),
            3,
            "the hidden file and the too deep one are left out"
        );
        assert_eq!(walked.too_deep, 1);
        assert!(is_folder(&root.to_string_lossy()));
        assert!(!is_folder(&root.join("z.mp4").to_string_lossy()));
        let _ = fs::remove_dir_all(&base);
    }
}
