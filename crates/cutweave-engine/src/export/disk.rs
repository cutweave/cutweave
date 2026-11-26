// SPDX-License-Identifier: GPL-3.0-or-later
//! Free disk space for the exports' disk check (WP-2G.03b; arch §8): the bytes a normal user
//! may still write on the drive holding a folder. The standard library has no such call, so this
//! is the one FFI call per OS (`GetDiskFreeSpaceExW`, `statvfs`); the planner reads it through
//! [`FreeSpaceProbe`](super::FreeSpaceProbe) so tests inject a figure instead of filling a disk.
#![allow(unsafe_code)]

use std::path::Path;

/// Bytes available to the current user on the drive holding `folder`; `None` when the system
/// cannot tell (the disk check is then skipped, never a failure).
#[must_use]
pub fn free_space(folder: &Path) -> Option<u64> {
    imp::free_space(folder)
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    pub(super) fn free_space(folder: &Path) -> Option<u64> {
        let wide: Vec<u16> = folder
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut available: u64 = 0;
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call, and
        // `available` is a valid `u64` the call writes; the two optional outputs are null,
        // which the API allows.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &raw mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(available)
    }
}

#[cfg(unix)]
mod imp {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub(super) fn free_space(folder: &Path) -> Option<u64> {
        let path = CString::new(folder.as_os_str().as_bytes()).ok()?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
        // SAFETY: `path` is a NUL-terminated C string that outlives the call and `stat` is
        // writable memory of the right type; it is read only after `statvfs` reported success,
        // which means it filled it in.
        let ok = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) };
        if ok != 0 {
            return None;
        }
        // SAFETY: see above: `statvfs` succeeded.
        let stat = unsafe { stat.assume_init() };
        let blocks = u64::from(stat.f_bavail);
        let size = u64::from(stat.f_frsize);
        Some(blocks.saturating_mul(size))
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    use std::path::Path;

    pub(super) fn free_space(_folder: &Path) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_temp_folder_has_some_free_space_and_a_missing_folder_has_none() {
        let free = free_space(&std::env::temp_dir()).expect("the OS reports free space");
        assert!(free > 0);
        assert_eq!(
            free_space(&std::env::temp_dir().join("cutweave-no-such-folder-2g03b")),
            None
        );
    }
}
