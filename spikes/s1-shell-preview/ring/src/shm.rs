//! Named shared memory, one implementation per OS behind the same API.
//!
//! - Windows: pagefile-backed file mapping in the per-session `Local\` namespace. The kernel
//!   object dies with its last handle, so a crashed engine leaks nothing once every reader
//!   has closed its view (the addon closes on `engine-down`).
//! - Linux/macOS: stub in this spike (see `posix` below for the intended implementation).

use thiserror::Error;

/// Shared-memory errors.
#[derive(Debug, Error)]
pub enum ShmError {
    /// The name is empty, too long or contains a path separator.
    #[error("invalid shared-memory name {0:?}")]
    InvalidName(String),
    /// `create` found an existing object with that name (possible squatting).
    #[error("shared-memory object {0:?} already exists")]
    AlreadyExists(String),
    /// `open` found no object with that name.
    #[error("shared-memory object {0:?} not found")]
    NotFound(String),
    /// Any other OS error.
    #[error("{op} failed for {name:?}: os error {code}")]
    Os {
        /// Failing call.
        op: &'static str,
        /// Object name.
        name: String,
        /// Raw OS error code.
        code: u32,
    },
    /// Not implemented on this OS in the spike.
    #[error("named shared memory is not implemented on this OS in spike S1 (see shm.rs)")]
    Unsupported,
}

/// Something that exposes a stable, writable, 8-byte-aligned memory region.
///
/// # Safety
///
/// Implementors must return a pointer that stays valid (and at the same address) for `len()`
/// bytes until the value is dropped, aligned to at least 8 bytes.
pub unsafe trait Mapping: Send + Sync {
    /// Base pointer of the region.
    fn as_ptr(&self) -> *mut u8;
    /// Length of the region in bytes.
    fn len(&self) -> usize;
    /// True if the region is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// SAFETY: forwarding to the inner mapping, which upholds the contract.
unsafe impl<M: Mapping> Mapping for std::sync::Arc<M> {
    fn as_ptr(&self) -> *mut u8 {
        (**self).as_ptr()
    }
    fn len(&self) -> usize {
        (**self).len()
    }
}

/// Plain heap memory with the [`Mapping`] contract, for unit tests and in-process use.
pub struct HeapMapping {
    ptr: *mut u64,
    words: usize,
}

impl HeapMapping {
    /// Zeroed region of at least `len` bytes.
    #[must_use]
    pub fn new(len: usize) -> Self {
        let words = len.div_ceil(8).max(1);
        let boxed: Box<[u64]> = vec![0u64; words].into_boxed_slice();
        let ptr = Box::into_raw(boxed).cast::<u64>();
        Self { ptr, words }
    }
}

impl Drop for HeapMapping {
    fn drop(&mut self) {
        // SAFETY: `ptr`/`words` came from `Box::into_raw` of a `Box<[u64]>` of that length.
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                self.ptr, self.words,
            )))
        };
    }
}

// SAFETY: the allocation is only freed on drop; concurrent access goes through atomics or the
// seqlock protocol, exactly as with a real cross-process mapping.
unsafe impl Send for HeapMapping {}
// SAFETY: see above.
unsafe impl Sync for HeapMapping {}

// SAFETY: heap allocation of u64s: 8-byte aligned, fixed address until drop.
unsafe impl Mapping for HeapMapping {
    fn as_ptr(&self) -> *mut u8 {
        self.ptr.cast()
    }
    fn len(&self) -> usize {
        self.words * 8
    }
}

fn validate_name(name: &str) -> Result<(), ShmError> {
    // macOS limits POSIX shm names to 31 bytes including the leading '/'; keep the same budget
    // on every OS so names are portable.
    let ok = !name.is_empty()
        && name.len() <= 30
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(ShmError::InvalidName(name.to_owned()))
    }
}

/// A named shared-memory object mapped into this process.
pub struct SharedMemory {
    name: String,
    ptr: *mut u8,
    len: usize,
    owner: bool,
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: the mapping is process-global memory; the raw handle/pointer are not thread-bound.
unsafe impl Send for SharedMemory {}
// SAFETY: shared access is coordinated through atomics / the seqlock protocol.
unsafe impl Sync for SharedMemory {}

// SAFETY: views are page aligned and stay mapped until drop.
unsafe impl Mapping for SharedMemory {
    fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }
    fn len(&self) -> usize {
        self.len
    }
}

impl SharedMemory {
    /// Create a new object of `len` bytes. Fails if the name already exists.
    pub fn create(name: &str, len: usize) -> Result<Self, ShmError> {
        validate_name(name)?;
        imp::create(name, len)
    }

    /// Open an existing object, mapping all of it. `writable` maps it read-write.
    pub fn open(name: &str, writable: bool) -> Result<Self, ShmError> {
        validate_name(name)?;
        imp::open(name, writable)
    }

    /// True if an object with this name currently exists (i.e. some process still holds it).
    #[must_use]
    pub fn exists(name: &str) -> bool {
        validate_name(name).is_ok() && imp::exists(name)
    }

    /// The portable name (without the OS namespace prefix).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// True if this handle created the object.
    #[must_use]
    pub fn is_owner(&self) -> bool {
        self.owner
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        imp::close(self);
    }
}

#[cfg(windows)]
mod imp {
    use super::{SharedMemory, ShmError};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_BASIC_INFORMATION,
        MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE,
        UnmapViewOfFile, VirtualQuery,
    };

    fn wide(name: &str) -> Vec<u16> {
        format!("Local\\{name}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    fn os_err(op: &'static str, name: &str) -> ShmError {
        // SAFETY: trivially safe FFI call.
        let code = unsafe { GetLastError() };
        ShmError::Os {
            op,
            name: name.to_owned(),
            code,
        }
    }

    pub(super) fn create(name: &str, len: usize) -> Result<SharedMemory, ShmError> {
        let w = wide(name);
        let len64 = len as u64;
        // Splitting a u64 into the high/low dwords the API wants.
        #[allow(clippy::cast_possible_truncation)]
        let (hi, lo) = ((len64 >> 32) as u32, len64 as u32);
        // SAFETY: valid NUL-terminated wide string; null security attributes = default DACL
        // (creator's token). See README "Findings" for the hardening note.
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                hi,
                lo,
                w.as_ptr(),
            )
        };
        if handle.is_null() {
            return Err(os_err("CreateFileMappingW", name));
        }
        // SAFETY: trivially safe FFI call, read right after CreateFileMappingW.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: we own this handle.
            unsafe { CloseHandle(handle) };
            return Err(ShmError::AlreadyExists(name.to_owned()));
        }
        // SAFETY: valid mapping handle; mapping the whole object.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, len) };
        if view.Value.is_null() {
            let e = os_err("MapViewOfFile", name);
            // SAFETY: we own this handle.
            unsafe { CloseHandle(handle) };
            return Err(e);
        }
        Ok(SharedMemory {
            name: name.to_owned(),
            ptr: view.Value.cast(),
            len,
            owner: true,
            handle,
        })
    }

    pub(super) fn open(name: &str, writable: bool) -> Result<SharedMemory, ShmError> {
        let w = wide(name);
        let access = if writable {
            FILE_MAP_READ | FILE_MAP_WRITE
        } else {
            FILE_MAP_READ
        };
        // SAFETY: valid NUL-terminated wide string.
        let handle = unsafe { OpenFileMappingW(access, 0, w.as_ptr()) };
        if handle.is_null() {
            // SAFETY: trivially safe FFI call.
            return Err(if unsafe { GetLastError() } == ERROR_FILE_NOT_FOUND {
                ShmError::NotFound(name.to_owned())
            } else {
                os_err("OpenFileMappingW", name)
            });
        }
        // SAFETY: valid mapping handle; size 0 maps the whole object.
        let view = unsafe { MapViewOfFile(handle, access, 0, 0, 0) };
        if view.Value.is_null() {
            let e = os_err("MapViewOfFile", name);
            // SAFETY: we own this handle.
            unsafe { CloseHandle(handle) };
            return Err(e);
        }
        // SAFETY: zeroed POD out-parameter; `view.Value` is a live view base address.
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let got = unsafe {
            VirtualQuery(
                view.Value,
                &raw mut mbi,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if got == 0 {
            let e = os_err("VirtualQuery", name);
            // SAFETY: we own the view and the handle.
            unsafe {
                UnmapViewOfFile(view);
                CloseHandle(handle);
            }
            return Err(e);
        }
        Ok(SharedMemory {
            name: name.to_owned(),
            ptr: view.Value.cast(),
            len: mbi.RegionSize,
            owner: false,
            handle,
        })
    }

    pub(super) fn exists(name: &str) -> bool {
        let w = wide(name);
        // SAFETY: valid NUL-terminated wide string.
        let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, w.as_ptr()) };
        if handle.is_null() {
            return false;
        }
        // SAFETY: we own this handle.
        unsafe { CloseHandle(handle) };
        true
    }

    pub(super) fn close(shm: &mut SharedMemory) {
        // SAFETY: `ptr` is the base of a view we mapped and `handle` is ours; both are released
        // exactly once (Drop).
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: shm.ptr.cast(),
            });
            CloseHandle(shm.handle);
        }
    }
}

/// Intended POSIX implementation (Linux + macOS), stubbed in the spike because it cannot be
/// exercised on the Windows dev machine. Plan for WP-1H.02:
///
/// - create: `shm_open("/<name>", O_CREAT | O_EXCL | O_RDWR, 0o600)`, `ftruncate(len)`,
///   `mmap(PROT_READ | PROT_WRITE, MAP_SHARED)`; the creator `shm_unlink`s on drop.
/// - open: `shm_open("/<name>", O_RDONLY)`, `fstat` for the size, `mmap(PROT_READ, MAP_SHARED)`.
/// - crash cleanup: the engine cannot unlink after `kill -9`, so Electron main (which chose
///   the name) calls `shm_unlink` when it observes the child exit. Readers keep their mapping
///   alive until they unmap, but the name is gone, so nothing leaks past the last unmap.
/// - Linux alternative: `memfd_create` + fd passing over the handshake socket avoids names
///   entirely (no squatting, no leak), at the cost of an fd-passing channel to the preload.
#[cfg(not(windows))]
mod imp {
    use super::{SharedMemory, ShmError};

    pub(super) fn create(_name: &str, _len: usize) -> Result<SharedMemory, ShmError> {
        Err(ShmError::Unsupported)
    }

    pub(super) fn open(_name: &str, _writable: bool) -> Result<SharedMemory, ShmError> {
        Err(ShmError::Unsupported)
    }

    pub(super) fn exists(_name: &str) -> bool {
        false
    }

    pub(super) fn close(_shm: &mut SharedMemory) {}
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn create_open_drop_releases_the_object() {
        let name = format!("cw-s1-test-{}", std::process::id());
        let a = SharedMemory::create(&name, 8192).expect("create");
        assert!(SharedMemory::exists(&name));
        assert!(matches!(
            SharedMemory::create(&name, 8192),
            Err(ShmError::AlreadyExists(_))
        ));
        let b = SharedMemory::open(&name, false).expect("open");
        assert!(b.len() >= 8192);
        // SAFETY: both views map the same object; a is writable.
        unsafe {
            a.as_ptr().write(0xAB);
            assert_eq!(b.as_ptr().read(), 0xAB);
        }
        drop(a);
        assert!(SharedMemory::exists(&name), "reader still holds a handle");
        drop(b);
        assert!(
            !SharedMemory::exists(&name),
            "last handle closed => object gone"
        );
    }

    #[test]
    fn rejects_bad_names() {
        assert!(matches!(
            SharedMemory::create("a/b", 4096),
            Err(ShmError::InvalidName(_))
        ));
        assert!(matches!(
            SharedMemory::open("", false),
            Err(ShmError::InvalidName(_))
        ));
    }
}
