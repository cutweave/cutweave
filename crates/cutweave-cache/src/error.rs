// SPDX-License-Identifier: GPL-3.0-or-later
//! [`CacheError`]: every way a cache operation can fail.

use cutweave_media::MediaError;

/// Why a cache operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CacheError {
    /// SQLite reported an error.
    #[error("cache database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The cache file or its directory could not be read or written.
    #[error("cache file `{path}`: {message}")]
    Io {
        /// The path.
        path: String,
        /// The failure.
        message: String,
    },
    /// The file is not a SQLite database or is damaged. [`CacheDb::open_or_reset`] recovers.
    ///
    /// [`CacheDb::open_or_reset`]: crate::CacheDb::open_or_reset
    #[error("cache database `{path}` is damaged")]
    Corrupt {
        /// The path.
        path: String,
    },
    /// The database was written by a newer Cutweave (a downgrade). [`CacheDb::open_or_reset`]
    /// moves it aside.
    ///
    /// [`CacheDb::open_or_reset`]: crate::CacheDb::open_or_reset
    #[error("cache database schema {found} is newer than this build supports ({supported})")]
    SchemaTooNew {
        /// The version in the file.
        found: u32,
        /// The newest version this build knows.
        supported: u32,
    },
    /// Building a missing entry failed in the media backend.
    #[error(transparent)]
    Media(#[from] MediaError),
    /// A request parameter is out of bounds (zero width, too many tiles).
    #[error("invalid request: {0}")]
    InvalidParams(String),
    /// Encoding a generated image failed.
    #[error("image encoding failed: {0}")]
    Encode(String),
}
