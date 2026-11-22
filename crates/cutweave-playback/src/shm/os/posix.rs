// SPDX-License-Identifier: GPL-3.0-or-later
//! POSIX backend (Linux, macOS): `shm_open("/<name>")` + `mmap`.
//!
//! - Create: `O_CREAT | O_EXCL | O_RDWR` with mode `0600` (exclusive, so a squatted name
//!   fails); on Linux an explicit `fchmod(0600)` makes the mode independent of the umask; the
//!   owner and mode are then verified with `fstat` before anything is written. Once mapped,
//!   the mode drops to `0400`: the engine's mapping stays writable, while any later open,
//!   even by the same user, gets read-only access unless it first changes the mode back.
//! - Open (readers): `O_RDONLY`, refused unless the object is owned by the effective user and
//!   has no group/other bits, then mapped `PROT_READ`.
//! - The descriptor is closed right after `mmap` (the mapping keeps the object alive), so no
//!   fd leaks into child processes.
//! - Lifetime: the creator `shm_unlink`s on drop. After a crash the name survives until
//!   [`reap_stale`] (Linux) or [`remove_stale`] (by name; macOS cannot enumerate) removes it.

use std::ffi::CString;
use std::fs::File;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::ptr::{NonNull, null_mut};

use super::ShmError;
use crate::shm::layout::HostClock;
use crate::shm::name::RingName;

/// The clock behind [`host_clock_ns`].
#[cfg(target_vendor = "apple")]
pub const HOST_CLOCK: HostClock = HostClock::MacUptimeRaw;
/// The clock behind [`host_clock_ns`].
#[cfg(not(target_vendor = "apple"))]
pub const HOST_CLOCK: HostClock = HostClock::LinuxMonotonic;

/// Bookkeeping for one mapped object.
pub(super) struct Object {
    /// `/<name>`, kept for `shm_unlink`.
    path: CString,
    /// True for the creator, which unlinks the name on drop.
    owner: bool,
}

fn object_path(name: &RingName) -> CString {
    // Ring names are validated hex/ASCII without NUL bytes.
    CString::new(format!("/{name}")).unwrap_or_default()
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn os_err(op: &'static str, name: &RingName, code: i32) -> ShmError {
    ShmError::Os {
        op,
        name: name.to_string(),
        code: i64::from(code),
    }
}

fn map_open_error(op: &'static str, name: &RingName, code: i32) -> ShmError {
    match code {
        libc::EEXIST => ShmError::AlreadyExists(name.to_string()),
        libc::ENOENT => ShmError::NotFound(name.to_string()),
        libc::EACCES | libc::EPERM => ShmError::AccessDenied(name.to_string()),
        _ => os_err(op, name, code),
    }
}

/// `shm_open` with the platform's calling convention (variadic on macOS).
fn shm_open(path: &CString, oflag: libc::c_int, mode: libc::mode_t) -> libc::c_int {
    // SAFETY: `path` is NUL terminated; flags and mode are plain integers. macOS declares
    // `shm_open` variadic, where the mode is passed promoted to `c_uint`.
    unsafe {
        #[cfg(target_vendor = "apple")]
        {
            libc::shm_open(path.as_ptr(), oflag, libc::c_uint::from(mode))
        }
        #[cfg(not(target_vendor = "apple"))]
        {
            libc::shm_open(path.as_ptr(), oflag, mode)
        }
    }
}

fn shm_unlink(path: &CString) -> i32 {
    // SAFETY: `path` is NUL terminated.
    unsafe { libc::shm_unlink(path.as_ptr()) }
}

fn euid() -> u32 {
    // SAFETY: no preconditions; always succeeds.
    unsafe { libc::geteuid() }
}

/// Refuse objects that another user owns or that other users can access.
fn check_private(file: &File, name: &RingName) -> Result<u64, ShmError> {
    let meta = file
        .metadata()
        .map_err(|e| os_err("fstat", name, e.raw_os_error().unwrap_or(0)))?;
    let mode = meta.mode() & 0o777;
    if meta.uid() != euid() {
        return Err(ShmError::Insecure {
            name: name.to_string(),
            reason: format!("owned by uid {}, not {}", meta.uid(), euid()),
        });
    }
    if mode & 0o077 != 0 {
        return Err(ShmError::Insecure {
            name: name.to_string(),
            reason: format!("mode {mode:04o} grants group/other access"),
        });
    }
    Ok(meta.len())
}

fn mmap(
    file: &File,
    len: usize,
    prot: libc::c_int,
    name: &RingName,
) -> Result<NonNull<u8>, ShmError> {
    use std::os::fd::AsRawFd;
    // SAFETY: `file` is an open shared-memory descriptor of at least `len` bytes; we ask the
    // kernel to choose the address. The mapping outlives the descriptor by design.
    let p = unsafe { libc::mmap(null_mut(), len, prot, libc::MAP_SHARED, file.as_raw_fd(), 0) };
    if p == libc::MAP_FAILED {
        return Err(os_err("mmap", name, errno()));
    }
    NonNull::new(p.cast::<u8>()).ok_or_else(|| os_err("mmap", name, 0))
}

pub(super) fn create(
    name: &RingName,
    len: usize,
) -> Result<(Object, NonNull<u8>, usize), ShmError> {
    let size = libc::off_t::try_from(len).map_err(|_| ShmError::Size(len))?;
    let path = object_path(name);
    let fd = shm_open(&path, libc::O_CREAT | libc::O_EXCL | libc::O_RDWR, 0o600);
    if fd < 0 {
        return Err(map_open_error("shm_open", name, errno()));
    }
    // SAFETY: `fd` is a fresh descriptor this function owns.
    let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    let result = (|| {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| os_err("fchmod", name, e.raw_os_error().unwrap_or(0)))?;
        }
        check_private(&file, name)?;
        file.set_len(u64::try_from(size).unwrap_or(0))
            .map_err(|e| os_err("ftruncate", name, e.raw_os_error().unwrap_or(0)))?;
        let ptr = mmap(&file, len, libc::PROT_READ | libc::PROT_WRITE, name)?;
        // Read-only from now on for every open except the mapping above (Linux; best effort on
        // macOS, whose shm objects may reject fchmod, leaving 0600).
        let read_only = {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o400))
        };
        if cfg!(target_os = "linux")
            && let Err(e) = read_only
        {
            // SAFETY: unmaps the mapping created just above, which nothing else references.
            unsafe { libc::munmap(ptr.as_ptr().cast(), len) };
            return Err(os_err("fchmod", name, e.raw_os_error().unwrap_or(0)));
        }
        Ok(ptr)
    })();
    drop(file);
    match result {
        Ok(ptr) => Ok((Object { path, owner: true }, ptr, len)),
        Err(e) => {
            shm_unlink(&path);
            Err(e)
        }
    }
}

pub(super) fn open_read_only(name: &RingName) -> Result<(Object, NonNull<u8>, usize), ShmError> {
    let path = object_path(name);
    let fd = shm_open(&path, libc::O_RDONLY, 0);
    if fd < 0 {
        return Err(map_open_error("shm_open", name, errno()));
    }
    // SAFETY: `fd` is a fresh descriptor this function owns.
    let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    let size = check_private(&file, name)?;
    let len = usize::try_from(size).map_err(|_| ShmError::Size(usize::MAX))?;
    if len == 0 {
        return Err(ShmError::Size(0));
    }
    let ptr = mmap(&file, len, libc::PROT_READ, name)?;
    Ok((Object { path, owner: false }, ptr, len))
}

#[cfg(test)]
pub(super) fn try_open_writable(name: &RingName) -> Result<(), ShmError> {
    let fd = shm_open(&object_path(name), libc::O_RDWR, 0);
    if fd < 0 {
        return Err(map_open_error("shm_open", name, errno()));
    }
    // SAFETY: `fd` is a fresh descriptor this function owns; dropping it closes it.
    drop(unsafe { OwnedFd::from_raw_fd(fd) });
    Ok(())
}

pub(super) fn access_description(object: &Object, name: &RingName) -> Result<String, ShmError> {
    let fd = shm_open(&object.path, libc::O_RDONLY, 0);
    if fd < 0 {
        return Err(map_open_error("shm_open", name, errno()));
    }
    // SAFETY: `fd` is a fresh descriptor this function owns.
    let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    let meta = file
        .metadata()
        .map_err(|e| os_err("fstat", name, e.raw_os_error().unwrap_or(0)))?;
    Ok(format!(
        "mode={:04o} uid={}",
        meta.mode() & 0o7777,
        meta.uid()
    ))
}

pub(super) fn close(object: &mut Object, ptr: NonNull<u8>, len: usize, _writable: bool) {
    // SAFETY: `ptr`/`len` describe a mapping this process created with `mmap`; it is unmapped
    // exactly once, here.
    unsafe { libc::munmap(ptr.as_ptr().cast(), len) };
    if object.owner {
        // ENOENT (already reaped by someone else) is fine.
        shm_unlink(&object.path);
        object.owner = false;
    }
}

/// True if a process with this pid exists (possibly owned by another user).
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs only the existence and permission checks.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || errno() == libc::EPERM
}

pub(super) fn remove_stale(name: &RingName) -> Result<bool, ShmError> {
    let pid = name.pid();
    if pid == std::process::id() || process_alive(pid) {
        return Ok(false);
    }
    if shm_unlink(&object_path(name)) == 0 {
        return Ok(true);
    }
    match errno() {
        libc::ENOENT => Ok(false),
        code => Err(map_open_error("shm_unlink", name, code)),
    }
}

#[cfg(target_os = "linux")]
pub(super) fn reap_stale() -> Result<Vec<RingName>, ShmError> {
    let dir = match std::fs::read_dir("/dev/shm") {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(ShmError::Os {
                op: "read_dir(/dev/shm)",
                name: String::new(),
                code: i64::from(e.raw_os_error().unwrap_or(0)),
            });
        }
    };
    let mut removed = Vec::new();
    for entry in dir.flatten() {
        let Some(name) = entry
            .file_name()
            .to_str()
            .and_then(|n| RingName::parse(n).ok())
        else {
            continue;
        };
        // Only this user's objects; another user's rings are theirs to reap.
        if entry.metadata().map(|m| m.uid()).ok() != Some(euid()) {
            continue;
        }
        if remove_stale(&name)? {
            removed.push(name);
        }
    }
    Ok(removed)
}

/// macOS cannot enumerate POSIX shared memory; reap by name with [`remove_stale`].
#[cfg(not(target_os = "linux"))]
#[allow(clippy::unnecessary_wraps)] // Same signature as the Linux and Windows backends.
pub(super) fn reap_stale() -> Result<Vec<RingName>, ShmError> {
    Ok(Vec::new())
}

/// Host monotonic time in ns: `CLOCK_UPTIME_RAW` on macOS, `CLOCK_MONOTONIC` elsewhere.
pub(crate) fn host_clock_ns() -> u64 {
    #[cfg(target_vendor = "apple")]
    let id = libc::CLOCK_UPTIME_RAW;
    #[cfg(not(target_vendor = "apple"))]
    let id = libc::CLOCK_MONOTONIC;
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid out-pointer and a clock id this OS provides.
    unsafe { libc::clock_gettime(id, &raw mut ts) };
    let secs = u64::try_from(ts.tv_sec).unwrap_or(0);
    let nanos = u64::try_from(ts.tv_nsec).unwrap_or(0);
    secs.saturating_mul(1_000_000_000).saturating_add(nanos)
}
