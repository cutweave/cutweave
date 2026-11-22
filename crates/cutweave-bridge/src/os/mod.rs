// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only mapping of a named ring, one backend per OS.
//!
//! Mirrors the reader half of the engine's backend (`RING.md`, "Security posture"):
//!
//! | | Windows | Linux / macOS |
//! |---|---|---|
//! | Open | `OpenFileMappingW(FILE_MAP_READ)` on `Local\<name>`, `MapViewOfFile(FILE_MAP_READ)` | `shm_open("/<name>", O_RDONLY)`, `mmap(PROT_READ)` |
//! | Checks | the DACL already restricts the object to the current user | refuse objects not owned by the effective user or with group/other mode bits |
//!
//! The reader never requests write access, so even a bug here cannot modify the ring.

use std::ptr::NonNull;

use cutweave_playback::shm::RingName;

use crate::error::BridgeError;

#[cfg(unix)]
#[path = "posix.rs"]
mod imp;
#[cfg(windows)]
#[path = "windows.rs"]
mod imp;

/// A named shared-memory object mapped read-only into this process. Dropping it unmaps the
/// view and closes the handle.
pub(crate) struct Mapping {
    ptr: NonNull<u8>,
    len: usize,
    object: imp::Object,
}

// SAFETY: the mapping is process-wide memory and the OS handle is not tied to a thread. The
// view is read-only; every access to it goes through atomics or the seqlock protocol.
unsafe impl Send for Mapping {}
// SAFETY: as above; `&Mapping` only hands out the base pointer and length.
unsafe impl Sync for Mapping {}

impl Mapping {
    /// Open an existing ring object read-only and map all of it.
    pub(crate) fn open(name: &RingName) -> Result<Self, BridgeError> {
        let (object, ptr, len) = imp::open_read_only(name)?;
        Ok(Self { ptr, len, object })
    }

    /// Base address (page aligned).
    pub(crate) fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr().cast_const()
    }

    /// Mapped length in bytes.
    pub(crate) fn len(&self) -> usize {
        self.len
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        imp::close(&mut self.object, self.ptr, self.len);
    }
}
