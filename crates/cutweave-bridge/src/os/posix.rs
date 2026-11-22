// SPDX-License-Identifier: GPL-3.0-or-later
//! POSIX backend (Linux, macOS): `shm_open(O_RDONLY)` + `mmap(PROT_READ)`, refusing objects
//! that another user owns or that group/other can access.

use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::ptr::{NonNull, null_mut};

use cutweave_playback::shm::RingName;

use crate::error::BridgeError;

/// Nothing to keep: the descriptor is closed right after `mmap`.
pub(super) struct Object;

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn os_err(op: &'static str, name: &RingName, code: i32) -> BridgeError {
    BridgeError::Os {
        op,
        name: name.to_string(),
        code: i64::from(code),
    }
}

/// `shm_open` with the platform's calling convention (variadic on macOS).
fn shm_open_read_only(path: &CString) -> libc::c_int {
    // SAFETY: `path` is NUL terminated; read-only open, the mode argument is unused.
    unsafe {
        #[cfg(target_vendor = "apple")]
        {
            libc::shm_open(path.as_ptr(), libc::O_RDONLY, libc::c_uint::from(0u16))
        }
        #[cfg(not(target_vendor = "apple"))]
        {
            libc::shm_open(path.as_ptr(), libc::O_RDONLY, 0)
        }
    }
}

pub(super) fn open_read_only(name: &RingName) -> Result<(Object, NonNull<u8>, usize), BridgeError> {
    // Ring names are validated hex/ASCII without NUL bytes.
    let path = CString::new(format!("/{name}")).unwrap_or_default();
    let fd = shm_open_read_only(&path);
    if fd < 0 {
        return Err(match errno() {
            libc::ENOENT => BridgeError::NotFound(name.to_string()),
            libc::EACCES | libc::EPERM => BridgeError::AccessDenied(name.to_string()),
            code => os_err("shm_open", name, code),
        });
    }
    // SAFETY: `fd` is a fresh descriptor this function owns.
    let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    let meta = file
        .metadata()
        .map_err(|e| os_err("fstat", name, e.raw_os_error().unwrap_or(0)))?;
    // SAFETY: no preconditions; always succeeds.
    let euid = unsafe { libc::geteuid() };
    let mode = meta.mode() & 0o777;
    if meta.uid() != euid {
        return Err(BridgeError::Insecure {
            name: name.to_string(),
            reason: format!("owned by uid {}, not {euid}", meta.uid()),
        });
    }
    if mode & 0o077 != 0 {
        return Err(BridgeError::Insecure {
            name: name.to_string(),
            reason: format!("mode {mode:04o} grants group/other access"),
        });
    }
    let len = usize::try_from(meta.len()).map_err(|_| BridgeError::Invalid("size".into()))?;
    if len == 0 {
        return Err(BridgeError::Invalid("shared-memory object is empty".into()));
    }
    // SAFETY: `file` is an open shared-memory descriptor of `len` bytes; the kernel picks the
    // address and the read-only mapping outlives the descriptor by design.
    let p = unsafe {
        libc::mmap(
            null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if p == libc::MAP_FAILED {
        return Err(os_err("mmap", name, errno()));
    }
    let ptr = NonNull::new(p.cast::<u8>()).ok_or_else(|| os_err("mmap", name, 0))?;
    Ok((Object, ptr, len))
}

pub(super) fn close(_object: &mut Object, ptr: NonNull<u8>, len: usize) {
    // SAFETY: `ptr`/`len` describe a mapping this process created with `mmap`; it is unmapped
    // exactly once, here. Readers never unlink: the name belongs to the engine.
    unsafe { libc::munmap(ptr.as_ptr().cast(), len) };
}
