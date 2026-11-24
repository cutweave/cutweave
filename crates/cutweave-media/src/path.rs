// SPDX-License-Identifier: GPL-3.0-or-later
//! Path pre-flight shared by every backend, so they agree on how a bad path fails (the backend
//! contract checks it).
//!
//! FFmpeg takes paths as C strings, and `ffmpeg-next` panics on a non-UTF-8 path or an interior
//! NUL (S3 finding F2.2), so these are rejected before any library call.

use std::path::Path;

use crate::MediaError;

/// The path for messages.
pub(crate) fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The path as UTF-8 without NUL characters, or [`MediaError::InvalidPath`].
pub(crate) fn check_syntax(path: &Path) -> Result<&str, MediaError> {
    let invalid = |reason: &str| MediaError::InvalidPath {
        path: display(path),
        reason: reason.to_owned(),
    };
    let s = path.to_str().ok_or_else(|| invalid("not valid UTF-8"))?;
    if s.is_empty() {
        return Err(invalid("empty path"));
    }
    if s.contains('\0') {
        return Err(invalid("contains a NUL character"));
    }
    Ok(s)
}

/// [`check_syntax`], then the path must name an existing regular file.
pub(crate) fn check_file(path: &Path) -> Result<&str, MediaError> {
    let s = check_syntax(path)?;
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => Ok(s),
        Ok(_) => Err(MediaError::InvalidPath {
            path: display(path),
            reason: "not a regular file".to_owned(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(MediaError::NotFound {
            path: display(path),
        }),
        Err(e) => Err(MediaError::Io {
            path: display(path),
            message: e.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_and_empty_paths_are_rejected_before_touching_the_disk() {
        assert!(matches!(
            check_syntax(Path::new("a\0b.mp4")),
            Err(MediaError::InvalidPath { reason, .. }) if reason.contains("NUL")
        ));
        assert!(matches!(
            check_syntax(Path::new("")),
            Err(MediaError::InvalidPath { .. })
        ));
        assert_eq!(check_syntax(Path::new("clip.mp4")), Ok("clip.mp4"));
    }

    #[test]
    fn missing_files_and_directories_fail_differently() {
        let missing = std::env::temp_dir().join("cutweave-media-path-test-does-not-exist.mp4");
        assert!(matches!(
            check_file(&missing),
            Err(MediaError::NotFound { .. })
        ));
        assert!(matches!(
            check_file(&std::env::temp_dir()),
            Err(MediaError::InvalidPath { reason, .. }) if reason == "not a regular file"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_rejected() {
        use std::os::unix::ffi::OsStrExt;
        let p = Path::new(std::ffi::OsStr::from_bytes(b"clip\xff.mp4"));
        assert!(matches!(
            check_syntax(p),
            Err(MediaError::InvalidPath { reason, .. }) if reason.contains("UTF-8")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn non_utf8_paths_are_rejected() {
        use std::os::windows::ffi::OsStringExt;
        // An unpaired surrogate: a valid Windows file name that is not Unicode.
        let name = std::ffi::OsString::from_wide(&[0x63, 0xD800, 0x2E, 0x6D]);
        assert!(matches!(
            check_syntax(Path::new(&name)),
            Err(MediaError::InvalidPath { reason, .. }) if reason.contains("UTF-8")
        ));
    }
}
