// SPDX-License-Identifier: GPL-3.0-or-later
//! Named shared memory, one backend per OS behind the same crate-internal API.
//!
//! | | Windows | Linux / macOS |
//! |---|---|---|
//! | Object | pagefile-backed file mapping `Local\<name>` (per-session namespace) | POSIX `shm_open("/<name>")` |
//! | Create | exclusive; DACL: current user and OWNER RIGHTS get `GENERIC_READ` only; Medium mandatory label with no-read-up / no-write-up | `O_CREAT \| O_EXCL`, mode `0600`, owner and mode verified with `fstat` |
//! | Open (readers) | `FILE_MAP_READ`, read-only view | `O_RDONLY` + `PROT_READ`, owner and mode verified first |
//! | Lifetime | kernel object freed with its last handle (crash-safe) | name lives until `shm_unlink`; a crashed engine's ring is reaped at the next start |
//!
//! See `RING.md` ("Security posture") for what each OS can and cannot guarantee.

use std::ptr::NonNull;

use thiserror::Error;

use crate::shm::name::RingName;

#[cfg(unix)]
#[path = "posix.rs"]
mod imp;
#[cfg(windows)]
#[path = "windows.rs"]
mod imp;

pub use imp::HOST_CLOCK;
pub(crate) use imp::host_clock_ns;

/// Shared-memory errors.
#[derive(Debug, Error)]
pub enum ShmError {
    /// The name is not a valid ring name.
    #[error("invalid ring name {0:?}")]
    InvalidName(String),
    /// An object with this name already exists (possible squatting); pick another name.
    #[error("shared-memory object {0:?} already exists")]
    AlreadyExists(String),
    /// No object with this name exists.
    #[error("shared-memory object {0:?} not found")]
    NotFound(String),
    /// The object exists but this process may not open it this way.
    #[error("access to shared-memory object {0:?} denied")]
    AccessDenied(String),
    /// The object is owned by another user or is accessible to other users.
    #[error("shared-memory object {name:?} is not private to this user: {reason}")]
    Insecure {
        /// Object name.
        name: String,
        /// What is wrong.
        reason: String,
    },
    /// The requested size does not fit the platform's limits.
    #[error("shared-memory size {0} is not supported")]
    Size(usize),
    /// The OS random number generator failed.
    #[error("OS random number generator failed: {0}")]
    Entropy(String),
    /// Any other OS error.
    #[error("{op} failed for {name:?}: os error {code}")]
    Os {
        /// Failing call.
        op: &'static str,
        /// Object name.
        name: String,
        /// Raw OS error code (`GetLastError` / `errno`).
        code: i64,
    },
}

/// A named shared-memory object mapped into this process.
///
/// The creator maps it read-write; every other open maps it read-only. Dropping the creator
/// unmaps it and removes the name (POSIX) or closes the handle (Windows).
pub(crate) struct SharedMemory {
    name: RingName,
    ptr: NonNull<u8>,
    len: usize,
    writable: bool,
    object: imp::Object,
}

// SAFETY: the mapping is process-wide memory and the OS handle is not tied to a thread. All
// concurrent access to the mapped bytes goes through atomics or the seqlock protocol.
unsafe impl Send for SharedMemory {}
// SAFETY: as above; `&SharedMemory` only hands out the base pointer and length.
unsafe impl Sync for SharedMemory {}

impl SharedMemory {
    /// Create a new object of `len` bytes, private to the current user, and map it read-write.
    /// Fails with [`ShmError::AlreadyExists`] if the name is taken.
    pub(crate) fn create(name: &RingName, len: usize) -> Result<Self, ShmError> {
        let (object, ptr, mapped) = imp::create(name, len)?;
        Ok(Self {
            name: name.clone(),
            ptr,
            len: mapped,
            writable: true,
            object,
        })
    }

    /// Open an existing object read-only (readers; tests in this crate).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn open_read_only(name: &RingName) -> Result<Self, ShmError> {
        let (object, ptr, len) = imp::open_read_only(name)?;
        Ok(Self {
            name: name.clone(),
            ptr,
            len,
            writable: false,
            object,
        })
    }

    /// Base address of the mapping (page aligned).
    pub(crate) fn as_ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    /// Mapped length in bytes.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// The object's name.
    pub(crate) fn name(&self) -> &RingName {
        &self.name
    }

    /// The object's effective access control as the OS reports it: the SDDL string on
    /// Windows, `mode=0600 uid=…` on POSIX.
    pub(crate) fn access_description(&self) -> Result<String, ShmError> {
        imp::access_description(&self.object, &self.name)
    }

    /// True if some process still holds an object with this name.
    #[cfg(test)]
    pub(crate) fn exists(name: &RingName) -> bool {
        // Through `Self` so the probe's own view and handle are released on drop.
        Self::open_read_only(name).is_ok()
    }

    /// Try to open the object for writing from outside the creator (must be refused on
    /// Windows; allowed on POSIX for the same user, where mode bits are all there is).
    #[cfg(test)]
    pub(crate) fn try_open_writable(name: &RingName) -> Result<(), ShmError> {
        imp::try_open_writable(name)
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        imp::close(&mut self.object, self.ptr, self.len, self.writable);
    }
}

/// Remove rings left behind by engines that died without cleaning up (`kill -9`, power loss).
/// Call once at engine start, before creating this launch's ring. Returns the removed names.
///
/// - Linux: scans `/dev/shm` for this user's `cwp-<pid>-…` objects whose pid is gone.
/// - macOS: POSIX shared memory cannot be enumerated, so this finds nothing; pass the names
///   recorded in stale run-files (WP-1I.01) to [`remove_stale_ring`] instead.
/// - Windows: nothing to do; a file mapping dies with its last handle, so a dead engine's ring
///   disappears as soon as its readers close their views.
pub fn reap_stale_rings() -> Result<Vec<RingName>, ShmError> {
    imp::reap_stale()
}

/// Remove the ring `name` if the process that created it is gone. Returns true if a ring was
/// removed. Never removes a ring whose creator is alive (or is this process).
pub fn remove_stale_ring(name: &RingName) -> Result<bool, ShmError> {
    imp::remove_stale(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_open_share_bytes_and_release() {
        let name = RingName::generate().unwrap();
        let a = SharedMemory::create(&name, 64 * 1024).unwrap();
        assert!(a.len() >= 64 * 1024);
        assert!(SharedMemory::exists(&name));
        assert!(matches!(
            SharedMemory::create(&name, 64 * 1024),
            Err(ShmError::AlreadyExists(_))
        ));
        let b = SharedMemory::open_read_only(&name).unwrap();
        assert!(b.len() >= 64 * 1024);
        // SAFETY: test-only; `a` is the writable view of the object `b` maps read-only, both
        // at least 64 KiB, and no other thread touches either.
        unsafe {
            a.as_ptr().add(4097).write_volatile(0xAB);
            assert_eq!(b.as_ptr().add(4097).read_volatile(), 0xAB);
        }
        drop(a);
        drop(b);
        assert!(!SharedMemory::exists(&name), "creator and last reader gone");
        assert!(matches!(
            SharedMemory::open_read_only(&name),
            Err(ShmError::NotFound(_))
        ));
    }

    #[test]
    fn missing_object_is_not_found() {
        let name = RingName::generate().unwrap();
        assert!(matches!(
            SharedMemory::open_read_only(&name),
            Err(ShmError::NotFound(_))
        ));
        assert!(
            !remove_stale_ring(&name).unwrap(),
            "own pid is never reaped"
        );
    }

    #[test]
    fn host_clock_is_monotonic_and_in_nanoseconds() {
        let a = host_clock_ns();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = host_clock_ns();
        let ms = (b - a) / 1_000_000;
        assert!(
            (15..1000).contains(&ms),
            "slept 20 ms, clock advanced {ms} ms"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_object_is_private_and_read_only_for_everyone_but_the_creator() {
        let name = RingName::generate().unwrap();
        let shm = SharedMemory::create(&name, 64 * 1024).unwrap();
        let sddl = shm.access_description().unwrap();
        // SDDL prints well-known SIDs as aliases (e.g. `LA` for the built-in Administrator).
        let user = imp::sddl_trustee(&imp::current_user_sid().unwrap()).unwrap();
        // Protected DACL, exactly two ACEs (current user + OWNER RIGHTS), read access only.
        let dacl = sddl
            .split("D:")
            .nth(1)
            .and_then(|d| d.split("S:").next())
            .unwrap_or_default()
            .to_owned();
        assert!(dacl.starts_with('P'), "DACL must not inherit: {sddl}");
        let aces: Vec<&str> = dacl
            .trim_start_matches(['P', 'A', 'I'])
            .split(")(")
            .collect();
        assert_eq!(aces.len(), 2, "{sddl}");
        assert!(dacl.contains(&format!(";;;{user})")), "{sddl}");
        assert!(dacl.contains(";;;OW)"), "{sddl}");
        assert!(!dacl.contains("WD") && !dacl.contains(";BU)") && !dacl.contains(";AU)"));
        for ace in &aces {
            // GENERIC_READ for a section is mapped to READ_CONTROL (RC) | SECTION_MAP_READ (LC)
            // | SECTION_QUERY (CC): no write, no WRITE_DAC, no WRITE_OWNER.
            assert!(
                ace.contains("A;;CCLCRC;;;"),
                "ACE grants more than read: {ace} in {sddl}"
            );
        }
        // Medium mandatory label with no-write-up and no-read-up.
        assert!(sddl.contains("(ML;;NWNR;;;ME)"), "{sddl}");
        // Readers can map it read-only but not writable; only the creator handle writes.
        assert!(SharedMemory::open_read_only(&name).is_ok());
        assert!(matches!(
            SharedMemory::try_open_writable(&name),
            Err(ShmError::AccessDenied(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn posix_object_is_private_and_read_only_after_creation() {
        let name = RingName::generate().unwrap();
        let shm = SharedMemory::create(&name, 64 * 1024).unwrap();
        let desc = shm.access_description().unwrap();
        // SAFETY: no preconditions.
        let uid = unsafe { libc::geteuid() };
        assert!(desc.ends_with(&format!(" uid={uid}")), "{desc}");
        #[cfg(target_os = "linux")]
        {
            assert!(desc.starts_with("mode=0400 "), "{desc}");
            // Root bypasses mode bits; everyone else gets read-only opens.
            if uid != 0 {
                assert!(matches!(
                    SharedMemory::try_open_writable(&name),
                    Err(ShmError::AccessDenied(_))
                ));
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert!(
                desc.starts_with("mode=0400 ") || desc.starts_with("mode=0600 "),
                "{desc}"
            );
            let _ = SharedMemory::try_open_writable(&name);
        }
        // The creator's own mapping stays writable.
        // SAFETY: test-only; `shm` is the creator's 64 KiB read-write view.
        unsafe { shm.as_ptr().write_volatile(1) };
    }
}
