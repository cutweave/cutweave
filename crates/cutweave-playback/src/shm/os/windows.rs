// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows backend: a pagefile-backed named file mapping in the per-session `Local\` namespace.
//!
//! Security descriptor (SDDL) of every ring:
//!
//! ```text
//! D:P(A;;GR;;;<current user SID>)(A;;GR;;;OW)S:(ML;;NWNR;;;ME)
//! ```
//!
//! - `D:P`: protected DACL, nothing inherited; no ACE for other users, groups or SYSTEM.
//! - The current user gets `GENERIC_READ` only (map-read, query, read-control). The creator's
//!   own handle keeps the full access it was created with, so only the engine can write.
//! - `OW` (OWNER RIGHTS) limits the owner to read as well, removing the owner's implicit
//!   `WRITE_DAC`: another process of the same user cannot loosen the DACL to gain write access.
//! - `ML;;NWNR;;;ME`: Medium integrity label with no-write-up and no-read-up, so Low-integrity
//!   and AppContainer processes of the same user (browser sandboxes) cannot read frames.
//!   If the OS refuses the label (an engine running below Medium), the ring is created with
//!   the DACL alone.

use std::ffi::c_void;
use std::ptr::{NonNull, null_mut};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND,
    ERROR_INVALID_LABEL, ERROR_PRIVILEGE_NOT_HELD, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetTokenInformation, LABEL_SECURITY_INFORMATION,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_BASIC_INFORMATION,
    MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE, SEC_COMMIT,
    UnmapViewOfFile, VirtualQuery,
};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::ShmError;
use crate::shm::layout::HostClock;
use crate::shm::name::RingName;

/// The clock behind [`host_clock_ns`].
pub const HOST_CLOCK: HostClock = HostClock::WindowsQpc;

/// The mapping handle of one open object.
pub(super) struct Object {
    handle: HANDLE,
}

/// A `HANDLE` closed on drop (for handles that do not outlive a function).
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful Win32 call and is closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Memory returned by an API that documents `LocalFree` as its deallocator.
struct LocalBox(*mut c_void);

impl Drop for LocalBox {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: allocated by the Win32 call that filled this box; freed exactly once.
            unsafe { LocalFree(self.0) };
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn object_name(name: &RingName) -> Vec<u16> {
    wide(&format!("Local\\{name}"))
}

fn last_error() -> u32 {
    // SAFETY: no preconditions; reads this thread's last-error value.
    unsafe { GetLastError() }
}

fn os_err(op: &'static str, name: &str, code: u32) -> ShmError {
    ShmError::Os {
        op,
        name: name.to_owned(),
        code: i64::from(code),
    }
}

/// Copy a NUL-terminated UTF-16 string owned by `LocalFree`d memory.
fn take_local_wide(p: *mut u16) -> String {
    let owner = LocalBox(p.cast());
    let mut len = 0;
    // SAFETY: `p` is a NUL-terminated string returned by a successful Win32 call; we stop at
    // the terminator and it stays allocated until `owner` drops at the end of this function.
    let s = unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    };
    drop(owner);
    s
}

/// The current process user's SID in string form (`S-1-5-21-…`).
pub(super) fn current_user_sid() -> Result<String, ShmError> {
    let mut token: HANDLE = null_mut();
    // SAFETY: `GetCurrentProcess` is a pseudo-handle; `token` is a valid out-pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(os_err("OpenProcessToken", "", last_error()));
    }
    let token = OwnedHandle(token);
    let mut len = 0u32;
    // SAFETY: size query with an empty buffer; `len` is a valid out-pointer. The call fails
    // with ERROR_INSUFFICIENT_BUFFER by design.
    unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &raw mut len) };
    // u64 storage keeps the buffer 8-byte aligned for TOKEN_USER.
    let mut buf = vec![0u64; usize::try_from(len).unwrap_or(0).div_ceil(8).max(1)];
    let cap = u32::try_from(buf.len() * 8).unwrap_or(u32::MAX);
    // SAFETY: `buf` provides `cap` writable, suitably aligned bytes.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buf.as_mut_ptr().cast(),
            cap,
            &raw mut len,
        )
    } == 0
    {
        return Err(os_err("GetTokenInformation", "", last_error()));
    }
    // SAFETY: the call above wrote a TOKEN_USER at the start of `buf`; its SID points into
    // `buf`, which outlives this use.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut s: *mut u16 = null_mut();
    // SAFETY: `sid` is a valid SID; `s` is an out-pointer released by `take_local_wide`.
    if unsafe { ConvertSidToStringSidW(sid, &raw mut s) } == 0 {
        return Err(os_err("ConvertSidToStringSidW", "", last_error()));
    }
    Ok(take_local_wide(s))
}

/// Build a self-relative security descriptor from SDDL.
fn security_descriptor(sddl: &str) -> Result<LocalBox, ShmError> {
    let w = wide(sddl);
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: `w` is NUL terminated; `sd` is an out-pointer released by `LocalBox`.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w.as_ptr(),
            SDDL_REVISION_1,
            &raw mut sd,
            null_mut(),
        )
    };
    if ok == 0 {
        return Err(os_err(
            "ConvertStringSecurityDescriptorToSecurityDescriptorW",
            sddl,
            last_error(),
        ));
    }
    Ok(LocalBox(sd))
}

pub(super) fn create(
    name: &RingName,
    len: usize,
) -> Result<(Object, NonNull<u8>, usize), ShmError> {
    let dacl = format!("D:P(A;;GR;;;{})(A;;GR;;;OW)", current_user_sid()?);
    match create_with(name, len, &format!("{dacl}S:(ML;;NWNR;;;ME)")) {
        Err(ShmError::Os { code, .. })
            if code == i64::from(ERROR_INVALID_LABEL)
                || code == i64::from(ERROR_PRIVILEGE_NOT_HELD) =>
        {
            create_with(name, len, &dacl)
        }
        other => other,
    }
}

fn create_with(
    name: &RingName,
    len: usize,
    sddl: &str,
) -> Result<(Object, NonNull<u8>, usize), ShmError> {
    let sd = security_descriptor(sddl)?;
    let sa = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    let len64 = u64::try_from(len).map_err(|_| ShmError::Size(len))?;
    let hi = u32::try_from(len64 >> 32).map_err(|_| ShmError::Size(len))?;
    let lo = u32::try_from(len64 & 0xFFFF_FFFF).map_err(|_| ShmError::Size(len))?;
    let w = object_name(name);
    // SAFETY: `sa` points at a live security descriptor, `w` is NUL terminated, and
    // INVALID_HANDLE_VALUE asks for a pagefile-backed section.
    let handle = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            &raw const sa,
            PAGE_READWRITE | SEC_COMMIT,
            hi,
            lo,
            w.as_ptr(),
        )
    };
    let err = last_error();
    if handle.is_null() {
        return Err(match err {
            // The name exists and is not ours to open: someone else's object, treat as taken.
            ERROR_ACCESS_DENIED | ERROR_ALREADY_EXISTS => ShmError::AlreadyExists(name.to_string()),
            code => os_err("CreateFileMappingW", name.as_str(), code),
        });
    }
    let object = Object { handle };
    if err == ERROR_ALREADY_EXISTS {
        // An existing object was opened instead of created (anti-squatting).
        close_handle(&object);
        return Err(ShmError::AlreadyExists(name.to_string()));
    }
    // SAFETY: valid section handle with the full access a creator gets; maps all `len` bytes.
    let view = unsafe { MapViewOfFile(object.handle, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, len) };
    let Some(ptr) = NonNull::new(view.Value.cast::<u8>()) else {
        let e = os_err("MapViewOfFile", name.as_str(), last_error());
        close_handle(&object);
        return Err(e);
    };
    Ok((object, ptr, len))
}

pub(super) fn open_read_only(name: &RingName) -> Result<(Object, NonNull<u8>, usize), ShmError> {
    let w = object_name(name);
    // SAFETY: `w` is NUL terminated.
    let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, w.as_ptr()) };
    if handle.is_null() {
        return Err(match last_error() {
            ERROR_FILE_NOT_FOUND => ShmError::NotFound(name.to_string()),
            ERROR_ACCESS_DENIED => ShmError::AccessDenied(name.to_string()),
            code => os_err("OpenFileMappingW", name.as_str(), code),
        });
    }
    let object = Object { handle };
    // SAFETY: valid section handle opened for reading; size 0 maps the whole object.
    let view = unsafe { MapViewOfFile(object.handle, FILE_MAP_READ, 0, 0, 0) };
    let Some(ptr) = NonNull::new(view.Value.cast::<u8>()) else {
        let e = os_err("MapViewOfFile", name.as_str(), last_error());
        close_handle(&object);
        return Err(e);
    };
    let mut info = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: `ptr` is the base of the live view; `info` is a correctly sized out-buffer.
    let got = unsafe {
        VirtualQuery(
            ptr.as_ptr().cast_const().cast(),
            &raw mut info,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if got == 0 {
        let e = os_err("VirtualQuery", name.as_str(), last_error());
        let mut object = object;
        close(&mut object, ptr, 0, false);
        return Err(e);
    }
    Ok((object, ptr, info.RegionSize))
}

#[cfg(test)]
pub(super) fn try_open_writable(name: &RingName) -> Result<(), ShmError> {
    let w = object_name(name);
    // SAFETY: `w` is NUL terminated.
    let handle = unsafe { OpenFileMappingW(FILE_MAP_WRITE, 0, w.as_ptr()) };
    if handle.is_null() {
        return Err(match last_error() {
            ERROR_FILE_NOT_FOUND => ShmError::NotFound(name.to_string()),
            ERROR_ACCESS_DENIED => ShmError::AccessDenied(name.to_string()),
            code => os_err("OpenFileMappingW", name.as_str(), code),
        });
    }
    drop(OwnedHandle(handle));
    Ok(())
}

/// How SDDL spells `sid` (its alias, e.g. `LA`, or the `S-1-…` string itself).
#[cfg(test)]
pub(super) fn sddl_trustee(sid: &str) -> Result<String, ShmError> {
    let sd = security_descriptor(&format!("O:{sid}"))?;
    let s = sd_to_string(&sd, OWNER_SECURITY_INFORMATION, sid)?;
    Ok(s.trim_start_matches("O:").to_owned())
}

fn sd_to_string(sd: &LocalBox, what: u32, name: &str) -> Result<String, ShmError> {
    let mut s: *mut u16 = null_mut();
    // SAFETY: `sd.0` is a valid descriptor; `s` is released by `take_local_wide`.
    let ok = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd.0,
            SDDL_REVISION_1,
            what,
            &raw mut s,
            null_mut(),
        )
    };
    if ok == 0 {
        return Err(os_err(
            "ConvertSecurityDescriptorToStringSecurityDescriptorW",
            name,
            last_error(),
        ));
    }
    Ok(take_local_wide(s))
}

pub(super) fn access_description(object: &Object, name: &RingName) -> Result<String, ShmError> {
    let what = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION;
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: our handle has READ_CONTROL (creator handle); the out-pointers are valid and the
    // descriptor is released by `LocalBox`.
    let err = unsafe {
        GetSecurityInfo(
            object.handle,
            SE_KERNEL_OBJECT,
            what,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &raw mut sd,
        )
    };
    if err != 0 {
        return Err(os_err("GetSecurityInfo", name.as_str(), err));
    }
    sd_to_string(&LocalBox(sd), what, name.as_str())
}

fn close_handle(object: &Object) {
    // SAFETY: the handle is valid and closed exactly once (callers drop `object` right after).
    unsafe { CloseHandle(object.handle) };
}

pub(super) fn close(object: &mut Object, ptr: NonNull<u8>, _len: usize, _writable: bool) {
    // SAFETY: `ptr` is the base of a view this process mapped from `object`; both the view and
    // the handle are released exactly once, here.
    unsafe {
        UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
            Value: ptr.as_ptr().cast(),
        });
    }
    close_handle(object);
    object.handle = null_mut();
}

/// Windows needs no reaping: a file mapping is a kernel object freed with its last handle, so
/// a killed engine's ring is gone once its readers unmap it.
#[allow(clippy::unnecessary_wraps)] // Same signature as the POSIX backend.
pub(super) fn reap_stale() -> Result<Vec<RingName>, ShmError> {
    Ok(Vec::new())
}

/// See [`reap_stale`]: there is never a stale name to remove on Windows.
#[allow(clippy::unnecessary_wraps)] // Same signature as the POSIX backend.
pub(super) fn remove_stale(_name: &RingName) -> Result<bool, ShmError> {
    Ok(false)
}

/// Host monotonic time in ns: `QueryPerformanceCounter` scaled by its frequency.
pub(crate) fn host_clock_ns() -> u64 {
    static FREQ: OnceLock<u64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: valid out-pointer; cannot fail on Windows XP and later.
        unsafe { QueryPerformanceFrequency(&raw mut f) };
        u64::try_from(f).unwrap_or(0).max(1)
    });
    let mut t = 0i64;
    // SAFETY: valid out-pointer; cannot fail on Windows XP and later.
    unsafe { QueryPerformanceCounter(&raw mut t) };
    let ticks = u128::from(u64::try_from(t).unwrap_or(0));
    u64::try_from(ticks * 1_000_000_000 / u128::from(freq)).unwrap_or(u64::MAX)
}
