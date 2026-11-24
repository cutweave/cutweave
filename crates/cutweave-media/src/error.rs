// SPDX-License-Identifier: GPL-3.0-or-later
//! [`MediaError`]: every way a [`MediaBackend`](crate::MediaBackend) call can fail.
//!
//! Backends map their library errors onto these variants (the FFmpeg mapping is in the crate's
//! `ffmpeg` module), so callers branch on the kind of failure and never on backend-specific codes.
//! Damaged media is reported as [`MediaError::Corrupt`]; nothing here is ever a panic.

use cutweave_model::{StreamKind, Time};

use crate::version::LibVersion;

/// Why a media operation failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MediaError {
    /// There is no file at the path.
    #[error("no media file at `{path}`")]
    NotFound {
        /// The path as given.
        path: String,
    },
    /// The path cannot name a media file: it is not valid UTF-8, contains a NUL character, or is
    /// not a regular file.
    #[error("`{path}` is not a usable media path: {reason}")]
    InvalidPath {
        /// The path (lossily converted for display).
        path: String,
        /// What is wrong with it.
        reason: String,
    },
    /// Reading the file failed (permissions, device errors).
    #[error("could not read `{path}`: {message}")]
    Io {
        /// The path.
        path: String,
        /// The failure.
        message: String,
    },
    /// The data is damaged or is not a media file this backend can read (truncated container,
    /// garbage, a missing header).
    #[error("`{path}` is damaged or not a media file: {message}")]
    Corrupt {
        /// The path.
        path: String,
        /// What the backend found.
        message: String,
    },
    /// The file is valid but uses a container, codec or feature this backend cannot handle.
    #[error("unsupported: {message}")]
    Unsupported {
        /// What is unsupported.
        message: String,
    },
    /// The file has no stream of the requested kind (or no stream with the requested index).
    #[error("`{path}` has no {kind:?} stream{}", .index.map_or_default(|i| format!(" #{i}")))]
    NoStream {
        /// The path.
        path: String,
        /// The kind that was asked for.
        kind: StreamKind,
        /// The container stream index that was asked for, if any.
        index: Option<u32>,
    },
    /// A seek or read asked for a time outside the stream.
    #[error("{t} is outside the stream")]
    OutOfRange {
        /// The requested media time.
        t: Time,
    },
    /// The caller passed something the operation cannot accept (an encoder spec without
    /// streams, a frame that does not match the encoder, timestamps going backwards).
    #[error("invalid input: {message}")]
    Invalid {
        /// What is wrong.
        message: String,
    },
    /// Hardware acceleration was requested but cannot be used (WP-1D.04 falls back to software
    /// for `HwMode::Auto`; this surfaces only where no fallback exists).
    #[error("hardware acceleration unavailable: {reason}")]
    HwUnavailable {
        /// Why.
        reason: String,
    },
    /// The operation exists in the trait but this backend does not implement it yet.
    #[error("{operation} is not implemented by the {backend} backend yet")]
    NotImplemented {
        /// The backend (`ffmpeg`, `fake`).
        backend: &'static str,
        /// The operation, with the work package that adds it.
        operation: &'static str,
    },
    /// The FFmpeg libraries loaded at run time are not the ones the engine was built against
    /// (ADR-0003, S3 finding F2.3). Running on would be undefined behaviour in the bindings'
    /// enums, so the backend refuses to start.
    #[error(
        "{library} {loaded} is loaded but the engine was built against {built}: \
         install the FFmpeg bundle this build was made with (docs/dev/ffmpeg.md)"
    )]
    RuntimeMismatch {
        /// The library (`libavcodec`, ...).
        library: &'static str,
        /// The version of the headers at build time.
        built: LibVersion,
        /// The version of the library loaded at run time.
        loaded: LibVersion,
    },
    /// Any other failure inside the media library.
    #[error("media backend error: {message}")]
    Backend {
        /// The library's message.
        message: String,
    },
}

impl MediaError {
    /// [`MediaError::NotImplemented`] for `backend`.
    #[must_use]
    pub const fn not_implemented(backend: &'static str, operation: &'static str) -> MediaError {
        MediaError::NotImplemented { backend, operation }
    }

    /// Whether the failure is a property of the file (missing, damaged, unsupported) rather than
    /// of the environment or the caller. Import reports these per file (`failed[]`, arch §7).
    #[must_use]
    pub fn is_media_fault(&self) -> bool {
        matches!(
            self,
            MediaError::NotFound { .. }
                | MediaError::InvalidPath { .. }
                | MediaError::Corrupt { .. }
                | MediaError::Unsupported { .. }
                | MediaError::NoStream { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_name_the_path_and_the_cause() {
        let e = MediaError::Corrupt {
            path: "a.mp4".into(),
            message: "moov atom not found".into(),
        };
        assert_eq!(
            e.to_string(),
            "`a.mp4` is damaged or not a media file: moov atom not found"
        );
        let e = MediaError::NoStream {
            path: "a.wav".into(),
            kind: StreamKind::Video,
            index: None,
        };
        assert_eq!(e.to_string(), "`a.wav` has no Video stream");
        let e = MediaError::NoStream {
            path: "a.mp4".into(),
            kind: StreamKind::Audio,
            index: Some(3),
        };
        assert_eq!(e.to_string(), "`a.mp4` has no Audio stream #3");
    }

    #[test]
    fn runtime_mismatch_says_what_to_do() {
        let e = MediaError::RuntimeMismatch {
            library: "libavcodec",
            built: LibVersion::new(62, 28, 103),
            loaded: LibVersion::new(62, 29, 100),
        };
        let text = e.to_string();
        assert!(text.contains("libavcodec 62.29.100 is loaded"), "{text}");
        assert!(text.contains("built against 62.28.103"), "{text}");
        assert!(text.contains("docs/dev/ffmpeg.md"), "{text}");
    }

    #[test]
    fn media_faults_are_per_file() {
        assert!(MediaError::NotFound { path: "x".into() }.is_media_fault());
        assert!(
            !MediaError::RuntimeMismatch {
                library: "libavutil",
                built: LibVersion::new(60, 0, 0),
                loaded: LibVersion::new(61, 0, 0),
            }
            .is_media_fault()
        );
        assert!(!MediaError::not_implemented("ffmpeg", "probe (WP-1D.02)").is_media_fault());
    }
}
