// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed errors of the ring reader. Each variant has a stable `ERR_RING_*` code that the
//! addon puts on the thrown JS error (`error.code`), so the preload and UI can branch on it.

use thiserror::Error;

/// Why a ring could not be opened or read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BridgeError {
    /// No ring with this name exists (engine not started, ring already released, or a stale
    /// name).
    #[error("preview ring {0:?} not found")]
    NotFound(String),
    /// The OS refused read access to the ring.
    #[error("access to preview ring {0:?} denied")]
    AccessDenied(String),
    /// The ring is owned by another user or open to other users (POSIX owner/mode check).
    #[error("preview ring {name:?} is not private to this user: {reason}")]
    Insecure {
        /// Ring name.
        name: String,
        /// What is wrong.
        reason: String,
    },
    /// Not a ring this reader understands: bad magic, malformed name or inconsistent geometry.
    #[error("not a valid preview ring: {0}")]
    Invalid(String),
    /// `layout_version` other than 1, or `required_features` bits this reader does not know.
    #[error("unsupported preview ring: {0}")]
    Version(String),
    /// The ring's `ring_epoch` is not the announced one (stale or foreign object).
    #[error("preview ring epoch {found:#018x} is not the announced epoch {expected:#018x}")]
    EpochMismatch {
        /// Epoch the engine announced.
        expected: u64,
        /// Epoch found in the header.
        found: u64,
    },
    /// The ring's timestamps use a different host clock than this reader.
    #[error("preview ring uses host clock {found}, this reader uses {expected}")]
    HostClock {
        /// Clock id of this reader.
        expected: u32,
        /// Clock id in the header.
        found: u32,
    },
    /// A stable slot descriptor violates the layout rules (a writer bug).
    #[error("corrupt preview frame descriptor: {0}")]
    Corrupt(String),
    /// The ring handle was closed, or released because the writer is gone.
    #[error("preview ring is closed")]
    Closed,
    /// The destination buffer could not be provided.
    #[error("cannot provide a frame buffer: {0}")]
    Buffer(String),
    /// Any other OS error.
    #[error("{op} failed for {name:?}: os error {code}")]
    Os {
        /// Failing call.
        op: &'static str,
        /// Ring name.
        name: String,
        /// Raw OS error code (`GetLastError` / `errno`).
        code: i64,
    },
}

impl BridgeError {
    /// Stable machine-readable code (`error.code` in JS).
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "ERR_RING_NOT_FOUND",
            Self::AccessDenied(_) => "ERR_RING_ACCESS_DENIED",
            Self::Insecure { .. } => "ERR_RING_INSECURE",
            Self::Invalid(_) => "ERR_RING_INVALID",
            Self::Version(_) => "ERR_RING_VERSION",
            Self::EpochMismatch { .. } => "ERR_RING_EPOCH",
            Self::HostClock { .. } => "ERR_RING_HOST_CLOCK",
            Self::Corrupt(_) => "ERR_RING_CORRUPT",
            Self::Closed => "ERR_RING_CLOSED",
            Self::Buffer(_) => "ERR_RING_BUFFER",
            Self::Os { .. } => "ERR_RING_OS",
        }
    }
}
