// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows backend: open the engine's pagefile-backed named file mapping read-only.

use std::ptr::NonNull;

use cutweave_playback::shm::RingName;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE,
};
use windows_sys::Win32::System::Memory::{
    FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, UnmapViewOfFile, VirtualQuery,
};

use crate::error::BridgeError;

/// The mapping handle of one open object.
pub(super) struct Object {
    handle: HANDLE,
}

fn last_error() -> u32 {
    // SAFETY: no preconditions; reads this thread's last-error value.
    unsafe { GetLastError() }
}

fn os_err(op: &'static str, name: &RingName, code: u32) -> BridgeError {
    BridgeError::Os {
        op,
        name: name.to_string(),
        code: i64::from(code),
    }
}

fn close_handle(handle: HANDLE) {
    // SAFETY: the handle came from a successful `OpenFileMappingW` and is closed exactly once.
    unsafe { CloseHandle(handle) };
}

pub(super) fn open_read_only(name: &RingName) -> Result<(Object, NonNull<u8>, usize), BridgeError> {
    let wide: Vec<u16> = format!("Local\\{name}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is NUL terminated; read access only, handle not inheritable.
    let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, wide.as_ptr()) };
    if handle.is_null() {
        return Err(match last_error() {
            ERROR_FILE_NOT_FOUND => BridgeError::NotFound(name.to_string()),
            ERROR_ACCESS_DENIED => BridgeError::AccessDenied(name.to_string()),
            code => os_err("OpenFileMappingW", name, code),
        });
    }
    // SAFETY: valid section handle opened for reading; size 0 maps the whole object.
    let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0) };
    let Some(ptr) = NonNull::new(view.Value.cast::<u8>()) else {
        let e = os_err("MapViewOfFile", name, last_error());
        close_handle(handle);
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
        let e = os_err("VirtualQuery", name, last_error());
        let mut object = Object { handle };
        close(&mut object, ptr, 0);
        return Err(e);
    }
    Ok((Object { handle }, ptr, info.RegionSize))
}

pub(super) fn close(object: &mut Object, ptr: NonNull<u8>, _len: usize) {
    // SAFETY: `ptr` is the base of a view this process mapped from `object`; the view and the
    // handle are released exactly once, here.
    unsafe {
        UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
            Value: ptr.as_ptr().cast(),
        });
    }
    close_handle(object.handle);
    object.handle = std::ptr::null_mut();
}
