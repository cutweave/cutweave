// SPDX-License-Identifier: GPL-3.0-or-later
//! The backend contract: behaviour every [`MediaBackend`] must share, so callers never branch on
//! which backend they have (ADR-0003). [`backend_contract!`](crate::backend_contract) turns each
//! case into a `#[test]` for a given backend; `cutweave-media` runs it against `FakeBackend` and,
//! with the `ffmpeg` feature, `FfmpegBackend`.
//!
//! The cases need no media files: the fixtures are a missing path, an empty file, a file of
//! garbage and a directory, created in a fresh temporary directory per test. Probe, decode and
//! encode cases on real media join with WP-1D.02/1D.03/1E.01, which generate or fetch it.
//!
//! These are test-support functions: they panic with a message when the backend breaks the
//! contract.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cutweave_model::FrameRate;

use crate::backend::{AudioEncodeSpec, DecodeOpts, EncodeSpec, MediaBackend, VideoEncodeSpec};
use crate::{MediaError, PixelFormat};

/// A temporary directory removed on drop (no extra dependency for test support).
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// A new empty directory under the OS temp dir, unique per process and call.
    #[must_use]
    pub fn new(tag: &str) -> TempDir {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("cutweave-media-{tag}-{}-{n}", std::process::id()));
        // A leftover from a crashed run with the same pid is replaced.
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path)
            .unwrap_or_else(|e| panic!("cannot create {}: {e}", path.display()));
        TempDir { path }
    }

    /// The directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Paths the contract cases use.
#[derive(Debug)]
pub struct Fixtures {
    /// Does not exist.
    pub missing: PathBuf,
    /// An existing empty file.
    pub empty: PathBuf,
    /// 64 KiB of deterministic pseudo-random bytes with a media-like extension.
    pub garbage: PathBuf,
    /// An existing directory.
    pub directory: PathBuf,
    dir: TempDir,
}

impl Fixtures {
    /// Creates the fixtures in a fresh temporary directory.
    #[must_use]
    pub fn new() -> Fixtures {
        let dir = TempDir::new("contract");
        let missing = dir.path().join("missing.mp4");
        let empty = dir.path().join("empty.mp4");
        let garbage = dir.path().join("garbage.mp4");
        let directory = dir.path().join("folder.mov");
        let write = |p: &Path, bytes: &[u8]| {
            fs::write(p, bytes).unwrap_or_else(|e| panic!("cannot write {}: {e}", p.display()));
        };
        write(&empty, &[]);
        // xorshift64: no RNG dependency, same bytes every run.
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let bytes: Vec<u8> = (0..64 * 1024)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            })
            .collect();
        write(&garbage, &bytes);
        fs::create_dir(&directory)
            .unwrap_or_else(|e| panic!("cannot create {}: {e}", directory.display()));
        Fixtures {
            missing,
            empty,
            garbage,
            directory,
            dir,
        }
    }

    /// The temporary directory holding the fixtures.
    #[must_use]
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }
}

impl Default for Fixtures {
    fn default() -> Self {
        Fixtures::new()
    }
}

/// The error of each opening call on `path`: probe, frame index, video decoder, audio decoder.
fn open_errors(b: &dyn MediaBackend, path: &Path) -> [(&'static str, Option<MediaError>); 4] {
    [
        ("probe", b.probe(path).err()),
        ("index_media", b.index_media(path).err()),
        (
            "open_decoder",
            b.open_decoder(path, &DecodeOpts::default()).err(),
        ),
        ("open_audio_decoder", b.open_audio_decoder(path, None).err()),
    ]
}

/// Two calls agree; codecs are sorted by name without duplicates; every implementation list is
/// sorted; every hardware entry names a known codec.
pub fn capabilities_are_stable_and_sorted(b: &dyn MediaBackend) {
    let caps = b.capabilities();
    assert_eq!(
        caps,
        b.capabilities(),
        "capabilities() changed between calls"
    );
    for pair in caps.codecs.windows(2) {
        assert!(
            pair[0].name < pair[1].name,
            "codecs not sorted/unique: `{}` before `{}`",
            pair[0].name,
            pair[1].name
        );
    }
    for c in &caps.codecs {
        assert!(!c.name.is_empty(), "a codec has an empty name");
        assert!(c.decoders.is_sorted(), "{}: decoders not sorted", c.name);
        assert!(c.encoders.is_sorted(), "{}: encoders not sorted", c.name);
        assert!(
            !(c.decoders.is_empty() && c.encoders.is_empty()),
            "{}: listed without a decoder or an encoder",
            c.name
        );
    }
    for hw in caps.hw_decode.iter().chain(&caps.hw_encode) {
        assert!(
            caps.codec(&hw.codec).is_some(),
            "hardware entry {hw:?} names a codec missing from `codecs`"
        );
    }
}

/// A path that does not exist is [`MediaError::NotFound`] for every opening call.
pub fn missing_files_are_not_found(b: &dyn MediaBackend, fx: &Fixtures) {
    for (call, err) in open_errors(b, &fx.missing) {
        assert!(
            matches!(err, Some(MediaError::NotFound { .. })),
            "{call}(missing) = {err:?}, want NotFound"
        );
    }
}

/// A path with a NUL character is [`MediaError::InvalidPath`], not a panic.
pub fn nul_paths_are_invalid(b: &dyn MediaBackend, fx: &Fixtures) {
    let bad = fx.dir().join("bad\0name.mp4");
    for (call, err) in open_errors(b, &bad) {
        assert!(
            matches!(err, Some(MediaError::InvalidPath { .. })),
            "{call}(NUL path) = {err:?}, want InvalidPath"
        );
    }
}

/// A directory is [`MediaError::InvalidPath`].
pub fn directories_are_invalid(b: &dyn MediaBackend, fx: &Fixtures) {
    for (call, err) in open_errors(b, &fx.directory) {
        assert!(
            matches!(err, Some(MediaError::InvalidPath { .. })),
            "{call}(directory) = {err:?}, want InvalidPath"
        );
    }
}

/// Random bytes are [`MediaError::Corrupt`].
pub fn garbage_is_corrupt(b: &dyn MediaBackend, fx: &Fixtures) {
    for (call, err) in open_errors(b, &fx.garbage) {
        assert!(
            matches!(err, Some(MediaError::Corrupt { .. })),
            "{call}(garbage) = {err:?}, want Corrupt"
        );
    }
}

/// An empty file is [`MediaError::Corrupt`].
pub fn empty_files_are_corrupt(b: &dyn MediaBackend, fx: &Fixtures) {
    for (call, err) in open_errors(b, &fx.empty) {
        assert!(
            matches!(err, Some(MediaError::Corrupt { .. })),
            "{call}(empty) = {err:?}, want Corrupt"
        );
    }
}

/// Hostile inputs (container headers with nothing behind them, zeros, text, absurd box sizes)
/// make `probe` and `index_media` fail with a media fault (damaged, unsupported or without
/// streams), never panic or hang, and never succeed with an empty answer.
pub fn hostile_files_are_media_faults(b: &dyn MediaBackend, fx: &Fixtures) {
    let mut ftyp = Vec::new();
    ftyp.extend_from_slice(&24u32.to_be_bytes());
    ftyp.extend_from_slice(b"ftypisom\0\0\x02\0isomiso2");
    let mut huge_moov = ftyp.clone();
    huge_moov.extend_from_slice(&u32::MAX.to_be_bytes());
    huge_moov.extend_from_slice(b"moov");
    let cases: [(&str, Vec<u8>); 5] = [
        ("ftyp-only.mp4", ftyp),
        ("huge-moov.mp4", huge_moov),
        ("zeros.mov", vec![0; 1 << 20]),
        ("text.mkv", b"this is not a video\n".repeat(500)),
        ("riff-only.wav", b"RIFF\0\0\0\0WAVE".to_vec()),
    ];
    for (name, bytes) in cases {
        let path = fx.dir().join(name);
        fs::write(&path, bytes).unwrap_or_else(|e| panic!("cannot write {name}: {e}"));
        for (call, err) in [
            ("probe", b.probe(&path).err()),
            ("index_media", b.index_media(&path).err()),
        ] {
            assert!(
                err.as_ref().is_some_and(MediaError::is_media_fault),
                "{call}({name}) = {err:?}, want a media fault"
            );
        }
    }
}

/// Specs without streams, with empty codec names or sizes, or with a NUL in the path are
/// [`MediaError::Invalid`] / [`MediaError::InvalidPath`], and create no file.
pub fn invalid_encode_specs_are_rejected(b: &dyn MediaBackend, fx: &Fixtures) {
    let path = fx.dir().join("out.mp4");
    let video = VideoEncodeSpec {
        codec: "h264".to_owned(),
        width: 64,
        height: 36,
        rate: FrameRate::FPS_30,
        input_format: PixelFormat::Yuv420p,
    };
    let audio = AudioEncodeSpec {
        codec: "aac".to_owned(),
        rate: cutweave_model::SampleRate::HZ_48_000,
        channels: 2,
    };
    let base = EncodeSpec {
        path: path.clone(),
        container: None,
        video: None,
        audio: None,
    };
    let cases = [
        ("no streams", base.clone()),
        (
            "empty video size",
            EncodeSpec {
                video: Some(VideoEncodeSpec {
                    width: 0,
                    ..video.clone()
                }),
                ..base.clone()
            },
        ),
        (
            "empty codec name",
            EncodeSpec {
                video: Some(VideoEncodeSpec {
                    codec: String::new(),
                    ..video
                }),
                ..base.clone()
            },
        ),
        (
            "no audio channels",
            EncodeSpec {
                audio: Some(AudioEncodeSpec {
                    channels: 0,
                    ..audio
                }),
                ..base.clone()
            },
        ),
    ];
    for (what, spec) in cases {
        let err = b.open_encoder(&spec).err();
        assert!(
            matches!(err, Some(MediaError::Invalid { .. })),
            "open_encoder({what}) = {err:?}, want Invalid"
        );
    }
    let nul = EncodeSpec {
        path: fx.dir().join("bad\0out.mp4"),
        ..base
    };
    let err = b.open_encoder(&nul).err();
    assert!(
        matches!(err, Some(MediaError::InvalidPath { .. })),
        "open_encoder(NUL path) = {err:?}, want InvalidPath"
    );
    assert!(!path.exists(), "a rejected spec created {}", path.display());
}

/// The backend can be shared across threads: concurrent calls agree with serial ones.
pub fn backend_is_shareable_across_threads(b: &Arc<dyn MediaBackend>, fx: &Fixtures) {
    let want_caps = b.capabilities();
    let want_err = b.probe(&fx.garbage).err();
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let b = Arc::clone(b);
            let garbage = fx.garbage.clone();
            std::thread::spawn(move || (b.capabilities(), b.probe(&garbage).err()))
        })
        .collect();
    for t in threads {
        let (caps, err) = t
            .join()
            .unwrap_or_else(|_| panic!("a backend call panicked on another thread"));
        assert_eq!(caps, want_caps);
        assert_eq!(
            std::mem::discriminant(&err),
            std::mem::discriminant(&want_err)
        );
    }
}

/// Runs the backend contract (see [`crate::testing::contract`]) as one `#[test]` per case in a
/// module named `$name`. `$make` builds the backend (any expression of a type implementing
/// [`MediaBackend`](crate::MediaBackend)); it runs once per case.
///
/// ```ignore
/// use cutweave_media::{FakeBackend, backend_contract};
/// backend_contract!(fake, FakeBackend::new());
/// ```
#[macro_export]
macro_rules! backend_contract {
    ($name:ident, $make:expr) => {
        mod $name {
            #[allow(unused_imports)]
            use super::*;
            use $crate::testing::contract as c;

            fn backend() -> ::std::sync::Arc<dyn $crate::MediaBackend> {
                ::std::sync::Arc::new($make)
            }

            #[test]
            fn capabilities_are_stable_and_sorted() {
                c::capabilities_are_stable_and_sorted(&*backend());
            }

            #[test]
            fn missing_files_are_not_found() {
                c::missing_files_are_not_found(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn nul_paths_are_invalid() {
                c::nul_paths_are_invalid(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn directories_are_invalid() {
                c::directories_are_invalid(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn garbage_is_corrupt() {
                c::garbage_is_corrupt(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn empty_files_are_corrupt() {
                c::empty_files_are_corrupt(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn hostile_files_are_media_faults() {
                c::hostile_files_are_media_faults(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn invalid_encode_specs_are_rejected() {
                c::invalid_encode_specs_are_rejected(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn backend_is_shareable_across_threads() {
                c::backend_is_shareable_across_threads(&backend(), &c::Fixtures::new());
            }
        }
    };
}
