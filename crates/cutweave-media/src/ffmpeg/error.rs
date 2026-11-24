// SPDX-License-Identifier: GPL-3.0-or-later
//! FFmpeg errors as [`MediaError`]s (S3: open failures on broken files are `AVERROR_INVALIDDATA`
//! or `AVERROR_EOF`; I/O failures are wrapped POSIX codes).

use std::path::Path;

use ffmpeg_next::Error;
use ffmpeg_next::error::{EISDIR, ENOENT, ENOMEM, ENOTDIR};

use crate::MediaError;
use crate::path::display;

/// Maps a failure to open or read `path`.
pub(super) fn media_error(path: &Path, e: Error) -> MediaError {
    let path = display(path);
    let message = e.to_string();
    match e {
        // Garbage, truncated headers, missing moov: damaged or not media at all.
        Error::InvalidData | Error::Eof => MediaError::Corrupt { path, message },
        Error::Other { errno } if errno == ENOENT => MediaError::NotFound { path },
        Error::Other { errno } if errno == EISDIR || errno == ENOTDIR => MediaError::InvalidPath {
            path,
            reason: "not a regular file".to_owned(),
        },
        Error::Other { errno } if errno == ENOMEM => MediaError::Backend { message },
        Error::Other { .. } => MediaError::Io { path, message },
        Error::DemuxerNotFound
        | Error::DecoderNotFound
        | Error::ProtocolNotFound
        | Error::StreamNotFound
        | Error::PatchWelcome
        | Error::Experimental => MediaError::Unsupported {
            message: format!("`{path}`: {message}"),
        },
        _ => MediaError::Backend {
            message: format!("`{path}`: {message}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_failures_map_to_typed_errors() {
        let p = Path::new("clip.mp4");
        assert!(matches!(
            media_error(p, Error::InvalidData),
            MediaError::Corrupt { .. }
        ));
        assert!(matches!(
            media_error(p, Error::Eof),
            MediaError::Corrupt { .. }
        ));
        assert!(matches!(
            media_error(p, Error::Other { errno: ENOENT }),
            MediaError::NotFound { .. }
        ));
        assert!(matches!(
            media_error(p, Error::Other { errno: EISDIR }),
            MediaError::InvalidPath { .. }
        ));
        assert!(matches!(
            media_error(
                p,
                Error::Other {
                    errno: ffmpeg_next::error::EACCES
                }
            ),
            MediaError::Io { .. }
        ));
        assert!(matches!(
            media_error(p, Error::DecoderNotFound),
            MediaError::Unsupported { .. }
        ));
        assert!(matches!(
            media_error(p, Error::Bug),
            MediaError::Backend { .. }
        ));
    }
}
