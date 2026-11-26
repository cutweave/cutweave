// SPDX-License-Identifier: GPL-3.0-or-later
//! Content hashes of media files (arch §7): the key of every cache entry, and what P2's relink
//! uses to find a moved file.
//!
//! Two modes, both BLAKE3:
//!
//! - **Fast** ([`HashMode::Fast`]): hashes the domain tag `cutweave/fast-hash/v1`, the file size
//!   as 8 little-endian bytes, the first 1 MiB and the last 1 MiB. A file of at most 2 MiB is
//!   hashed whole; above that the head and the tail do not overlap. It reads at most 2 MiB, so
//!   it is fine to run on import for every file. Two different files that agree on size, head
//!   and tail collide: rare for camera and screen-recording footage (the tail carries the
//!   container index), but possible, which is why [`Full`](HashMode::Full) exists.
//! - **Full** ([`HashMode::Full`]): hashes the domain tag `cutweave/full-hash/v1` and every byte.
//!   Reads the whole file in 1 MiB chunks: run it on a worker thread, not on import.
//!
//! The text form ([`ContentHash`]'s `Display`) carries the mode, so the two never compare equal:
//! `blake3-fast:<64 hex digits>` and `blake3-full:<64 hex digits>`.

use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::str::FromStr;

use crate::MediaError;
use crate::path;

/// Bytes hashed from each end by [`HashMode::Fast`].
pub const FAST_HASH_WINDOW: u64 = 1 << 20;

const FAST_DOMAIN: &[u8] = b"cutweave/fast-hash/v1";
const FULL_DOMAIN: &[u8] = b"cutweave/full-hash/v1";
const SEQUENCE_DOMAIN: &[u8] = b"cutweave/image-sequence-hash/v1";
const CHUNK: usize = 1 << 20;

/// How much of the file a hash covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HashMode {
    /// Size + first 1 MiB + last 1 MiB.
    Fast,
    /// Every byte.
    Full,
}

/// A BLAKE3 content hash and the mode that made it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash {
    mode: HashMode,
    digest: [u8; 32],
}

impl ContentHash {
    /// How the hash was made.
    #[must_use]
    pub fn mode(&self) -> HashMode {
        self.mode
    }

    /// The 32 digest bytes.
    #[must_use]
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

impl fmt::Display for ContentHash {
    /// `blake3-fast:<hex>` or `blake3-full:<hex>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = match self.mode {
            HashMode::Fast => "blake3-fast",
            HashMode::Full => "blake3-full",
        };
        write!(
            f,
            "{prefix}:{}",
            blake3::Hash::from_bytes(self.digest).to_hex()
        )
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

/// Why a string is not a [`ContentHash`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a content hash (want `blake3-fast:<hex>` or `blake3-full:<hex>`)")]
pub struct ParseHashError;

impl FromStr for ContentHash {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<ContentHash, ParseHashError> {
        let (mode, hex) = if let Some(h) = s.strip_prefix("blake3-fast:") {
            (HashMode::Fast, h)
        } else if let Some(h) = s.strip_prefix("blake3-full:") {
            (HashMode::Full, h)
        } else {
            return Err(ParseHashError);
        };
        let hash = blake3::Hash::from_hex(hex).map_err(|_| ParseHashError)?;
        Ok(ContentHash {
            mode,
            digest: *hash.as_bytes(),
        })
    }
}

/// The content hash of the file at `path` in `mode`. For the locator of an image sequence
/// ([`SequenceSource`](crate::SequenceSource)) it is the hash of the first file mixed with the
/// run's pattern, first number, count and frame rate.
///
/// Errors: [`MediaError::NotFound`], [`MediaError::InvalidPath`] (not a regular file),
/// [`MediaError::Io`]. A file that changes while it is hashed gives an unspecified hash, never an
/// error.
pub fn content_hash(path: &Path, mode: HashMode) -> Result<ContentHash, MediaError> {
    if let Some(sequence) = crate::SequenceSource::parse(path) {
        // An image sequence (ADR-0035): the first file's hash mixed with the run, so the
        // sequence and a still of its first picture have different cache keys, and importing
        // reads one file.
        let first = content_hash(&sequence.first_file(), mode)?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(SEQUENCE_DOMAIN);
        hasher.update(first.digest());
        hasher.update(sequence.hash_salt().as_bytes());
        return Ok(ContentHash {
            mode,
            digest: *hasher.finalize().as_bytes(),
        });
    }
    path::check_file(path)?;
    let io = |e: std::io::Error| MediaError::Io {
        path: path::display(path),
        message: e.to_string(),
    };
    let mut file = File::open(path).map_err(io)?;
    let size = file.metadata().map_err(io)?.len();
    let mut hasher = blake3::Hasher::new();
    match mode {
        HashMode::Full => {
            hasher.update(FULL_DOMAIN);
            feed(&mut file, &mut hasher, u64::MAX).map_err(io)?;
        }
        HashMode::Fast => {
            hasher.update(FAST_DOMAIN);
            hasher.update(&size.to_le_bytes());
            feed(&mut file, &mut hasher, FAST_HASH_WINDOW).map_err(io)?;
            if size > FAST_HASH_WINDOW {
                // The tail starts where the head ended at the earliest, so a file of at most
                // 2 MiB is hashed whole and nothing is hashed twice.
                let tail_start = size.saturating_sub(FAST_HASH_WINDOW).max(FAST_HASH_WINDOW);
                file.seek(SeekFrom::Start(tail_start)).map_err(io)?;
                feed(&mut file, &mut hasher, FAST_HASH_WINDOW).map_err(io)?;
            }
        }
    }
    Ok(ContentHash {
        mode,
        digest: *hasher.finalize().as_bytes(),
    })
}

/// Feeds up to `limit` bytes of `reader` to `hasher`.
fn feed(reader: &mut impl Read, hasher: &mut blake3::Hasher, limit: u64) -> std::io::Result<()> {
    let mut buf = vec![0u8; CHUNK];
    let mut left = limit;
    while left > 0 {
        let want = usize::try_from(left).map_or(CHUNK, |l| l.min(CHUNK));
        let n = match reader.read(&mut buf[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buf[..n]);
        left -= n as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testing::contract::TempDir;

    fn file(dir: &TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.path().join(name);
        fs::write(&p, bytes).expect("write");
        p
    }

    /// Deterministic bytes that differ everywhere.
    fn data(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13).to_le_bytes()[0])
            .collect()
    }

    #[test]
    fn fast_hash_covers_size_head_and_tail_only() {
        let dir = TempDir::new("hash");
        let mib = 1usize << 20;
        let base = data(5 * mib);
        let a = content_hash(&file(&dir, "a.bin", &base), HashMode::Fast).expect("hash");

        // The middle is not read: changing it keeps the hash.
        let mut mid = base.clone();
        mid[2 * mib + 17] ^= 0xff;
        assert_eq!(
            content_hash(&file(&dir, "mid.bin", &mid), HashMode::Fast).expect("hash"),
            a
        );
        // The first and last bytes are.
        for at in [0, mib - 1, 4 * mib, 5 * mib - 1] {
            let mut changed = base.clone();
            changed[at] ^= 1;
            let p = file(&dir, "changed.bin", &changed);
            assert_ne!(
                content_hash(&p, HashMode::Fast).expect("hash"),
                a,
                "byte {at}"
            );
        }
        // So is the size, even with the same head and tail windows.
        let mut longer = base.clone();
        longer.insert(2 * mib, 0);
        assert_ne!(
            content_hash(&file(&dir, "longer.bin", &longer), HashMode::Fast).expect("hash"),
            a
        );
        // The full hash sees the middle.
        let full = |p: &str| content_hash(&dir.path().join(p), HashMode::Full).expect("hash");
        assert_ne!(full("a.bin"), full("mid.bin"));
    }

    #[test]
    fn small_files_are_hashed_whole() {
        let dir = TempDir::new("hash-small");
        let mib = 1usize << 20;
        // At the 2 MiB boundary and below, every byte counts even in fast mode.
        for len in [0, 1, mib - 1, mib, mib + 1, 2 * mib] {
            let bytes = data(len);
            let a = content_hash(&file(&dir, "a.bin", &bytes), HashMode::Fast).expect("hash");
            if len > 0 {
                for at in [0, len / 2, len - 1] {
                    let mut c = bytes.clone();
                    c[at] ^= 1;
                    let b = content_hash(&file(&dir, "b.bin", &c), HashMode::Fast).expect("hash");
                    assert_ne!(a, b, "len {len}, byte {at}");
                }
            }
        }
        // One byte past 2 MiB is the first size with a gap: the byte at index 1 MiB lies after
        // the head window and before the tail window, so it is the first one not hashed.
        let bytes = data(2 * mib + 1);
        let a = content_hash(&file(&dir, "c.bin", &bytes), HashMode::Fast).expect("hash");
        let mut c = bytes.clone();
        c[mib] ^= 1;
        let b = content_hash(&file(&dir, "d.bin", &c), HashMode::Fast).expect("hash");
        assert_eq!(a, b);
    }

    #[test]
    fn modes_never_compare_equal_and_the_text_form_round_trips() {
        let dir = TempDir::new("hash-text");
        let p = file(&dir, "a.bin", b"hello");
        let fast = content_hash(&p, HashMode::Fast).expect("hash");
        let full = content_hash(&p, HashMode::Full).expect("hash");
        assert_ne!(fast, full);
        assert_ne!(fast.digest(), full.digest());
        let text = fast.to_string();
        assert!(
            text.starts_with("blake3-fast:") && text.len() == 12 + 64,
            "{text}"
        );
        assert!(full.to_string().starts_with("blake3-full:"));
        assert_eq!(text.parse::<ContentHash>(), Ok(fast));
        assert_eq!(full.to_string().parse::<ContentHash>(), Ok(full));
        for bad in [
            "",
            "blake3:00",
            "blake3-fast:xyz",
            "blake3-fast:",
            "sha256:00",
        ] {
            assert_eq!(bad.parse::<ContentHash>(), Err(ParseHashError), "{bad}");
        }
        // Stable across runs and platforms: the empty file's fast hash is fixed by the format.
        let empty = file(&dir, "empty.bin", b"");
        let mut h = blake3::Hasher::new();
        h.update(b"cutweave/fast-hash/v1");
        h.update(&0u64.to_le_bytes());
        assert_eq!(
            content_hash(&empty, HashMode::Fast).expect("hash").digest(),
            h.finalize().as_bytes()
        );
    }

    #[test]
    fn full_hash_is_blake3_of_the_domain_and_the_bytes() {
        let dir = TempDir::new("hash-full");
        let bytes = data(3_000_000);
        let p = file(&dir, "a.bin", &bytes);
        let mut h = blake3::Hasher::new();
        h.update(b"cutweave/full-hash/v1");
        h.update(&bytes);
        assert_eq!(
            content_hash(&p, HashMode::Full).expect("hash").digest(),
            h.finalize().as_bytes()
        );
    }

    #[test]
    fn bad_paths_are_typed_errors() {
        let dir = TempDir::new("hash-err");
        assert!(matches!(
            content_hash(&dir.path().join("none.mp4"), HashMode::Fast),
            Err(MediaError::NotFound { .. })
        ));
        assert!(matches!(
            content_hash(dir.path(), HashMode::Full),
            Err(MediaError::InvalidPath { .. })
        ));
        assert!(matches!(
            content_hash(Path::new("a\0b"), HashMode::Fast),
            Err(MediaError::InvalidPath { .. })
        ));
    }
}
