// SPDX-License-Identifier: GPL-3.0-or-later
//! What the crash harness does to, and checks in, the project folder: the copy the reference
//! engine works on, the torn journal tail, and the files a killed engine may leave behind.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::BenchError;

/// Copies the folder `from` to `to` (which must not exist), files and subfolders.
pub fn copy_dir(from: &Path, to: &Path) -> Result<(), BenchError> {
    fs::create_dir_all(to).map_err(|e| BenchError::io("create", to, e))?;
    let entries = fs::read_dir(from).map_err(|e| BenchError::io("read", from, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| BenchError::io("read", from, e))?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if source.is_dir() {
            copy_dir(&source, &target)?;
        } else {
            fs::copy(&source, &target).map_err(|e| BenchError::io("copy", &source, e))?;
        }
    }
    Ok(())
}

/// The project's document: the one `.cutweave` file in `folder`.
#[must_use]
pub fn document(folder: &Path) -> Option<PathBuf> {
    fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "cutweave"))
}

/// The journal segments of the project in `folder`, oldest first (`<base revision>.jnl`).
#[must_use]
pub fn segments(folder: &Path) -> Vec<PathBuf> {
    let dir = folder.join(".cutweave").join("journal");
    let mut found: Vec<(u64, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "jnl"))
        .filter_map(|p| {
            let base = p.file_stem()?.to_str()?.parse().ok()?;
            Some((base, p))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, p)| p).collect()
}

/// Cuts `bytes` off the end of the newest journal segment, as a write that the crash tore
/// would leave it. Returns the segment and its new length; `None`: there is no segment long
/// enough.
pub fn tear_tail(folder: &Path, bytes: u64) -> Result<Option<(PathBuf, u64)>, BenchError> {
    let Some(segment) = segments(folder).pop() else {
        return Ok(None);
    };
    let len = fs::metadata(&segment)
        .map_err(|e| BenchError::io("read", &segment, e))?
        .len();
    if len <= bytes {
        return Ok(None);
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&segment)
        .map_err(|e| BenchError::io("open", &segment, e))?;
    file.set_len(len - bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| BenchError::io("truncate", &segment, e))?;
    Ok(Some((segment, len - bytes)))
}

/// Files in `dir` (not below it) whose name satisfies `is`.
fn files_named(dir: &Path, is: impl Fn(&str) -> bool) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| is(name))
        .collect();
    found.sort();
    found
}

/// Checks the project folder after a recovery: the document on disk is a readable project
/// file, the journal and the export queue are there, and, once the engine has `saved` again,
/// no temporary file of an interrupted save is left.
pub fn check(folder: &Path, saved: bool) -> Result<(), String> {
    let document = document(folder).ok_or("the project folder has no document")?;
    let text =
        fs::read_to_string(&document).map_err(|e| format!("the document cannot be read: {e}"))?;
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|e| format!("the document on disk is not valid JSON: {e}"))?;
    if !parsed["meta"]["schema_version"].is_u64() || !parsed["sequences"].is_object() {
        return Err("the document on disk is not a project file".into());
    }
    if segments(folder).is_empty() {
        return Err("the project has no journal segment".into());
    }
    if !folder.join(".cutweave").join("exports.sqlite").is_file() {
        return Err("the project has no export queue file".into());
    }
    if saved {
        let debris = files_named(folder, |name| name.ends_with(".tmp"));
        if !debris.is_empty() {
            return Err(format!(
                "a save left temporary files behind: {}",
                debris.join(", ")
            ));
        }
    }
    Ok(())
}

/// Partial export files (`.<name>.<pid>-<n>.partial`) in `dir`.
#[must_use]
pub fn partials(dir: &Path) -> Vec<String> {
    files_named(dir, |name| name.ends_with(".partial"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("target")
            .join("tmp")
            .join(format!("crash-disk-{tag}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn project(dir: &Path) -> PathBuf {
        let folder = dir.join("P");
        fs::create_dir_all(folder.join(".cutweave").join("journal")).unwrap();
        fs::write(
            folder.join("P.cutweave"),
            r#"{"meta":{"schema_version":3},"sequences":{}}"#,
        )
        .unwrap();
        fs::write(folder.join(".cutweave").join("exports.sqlite"), b"").unwrap();
        fs::write(folder.join(".cutweave/journal/0.jnl"), [1u8; 40]).unwrap();
        fs::write(folder.join(".cutweave/journal/12.jnl"), [2u8; 100]).unwrap();
        fs::write(folder.join(".cutweave/journal/7.jnl"), [3u8; 10]).unwrap();
        folder
    }

    #[test]
    fn the_newest_segment_is_torn_and_the_folder_is_checked() {
        let dir = scratch("tear");
        let folder = project(&dir);
        let names: Vec<String> = segments(&folder)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["0.jnl", "7.jnl", "12.jnl"]);
        let (segment, len) = tear_tail(&folder, 30).unwrap().unwrap();
        assert!(segment.ends_with("12.jnl"));
        assert_eq!((len, fs::metadata(&segment).unwrap().len()), (70, 70));
        assert_eq!(tear_tail(&folder, 500).unwrap(), None);

        assert_eq!(check(&folder, true), Ok(()));
        fs::write(folder.join(".P.cutweave.tmp"), b"half").unwrap();
        assert_eq!(check(&folder, false), Ok(()));
        assert!(
            check(&folder, true)
                .unwrap_err()
                .contains(".P.cutweave.tmp")
        );
        fs::write(folder.join("P.cutweave"), b"{\"meta\":").unwrap();
        assert!(
            check(&folder, false)
                .unwrap_err()
                .contains("not valid JSON")
        );
    }

    #[test]
    fn a_folder_is_copied_with_its_subfolders() {
        let dir = scratch("copy");
        let folder = project(&dir);
        let copy = dir.join("copy").join("P");
        copy_dir(&folder, &copy).unwrap();
        assert_eq!(segments(&copy).len(), 3);
        assert_eq!(document(&copy), Some(copy.join("P.cutweave")));
        fs::write(copy.join(".out.mp4.123-1.partial"), b"x").unwrap();
        assert_eq!(partials(&copy), [".out.mp4.123-1.partial"]);
    }
}
