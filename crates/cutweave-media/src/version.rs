// SPDX-License-Identifier: GPL-3.0-or-later
//! FFmpeg version bookkeeping that needs no FFmpeg: library version triples, the start-up rule
//! that the loaded libraries equal the headers (ADR-0003, S3 finding F2.3), and parsing of the
//! FFmpeg release banner against the pinned release.
//!
//! The FFI half (reading `avcodec_version()` and friends) lives in the `ffmpeg` module; keeping
//! the rules here lets the default, FFmpeg-free test run cover them.

use std::fmt;

#[cfg(any(feature = "ffmpeg", test))]
use crate::MediaError;

/// The FFmpeg release this tree is pinned to (`scripts/ffmpeg/versions.toml`, `[ffmpeg] version`;
/// a test keeps the two equal). The FFmpeg-backed tests check the loaded libraries against it.
pub const PINNED_FFMPEG: &str = "8.1.3";

/// A libav* library version (`AV_VERSION_INT` triple), e.g. libavcodec `62.28.103`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LibVersion {
    /// Major version (ABI epoch).
    pub major: u32,
    /// Minor version.
    pub minor: u32,
    /// Micro version.
    pub micro: u32,
}

impl LibVersion {
    /// `major.minor.micro`.
    #[must_use]
    pub const fn new(major: u32, minor: u32, micro: u32) -> LibVersion {
        LibVersion {
            major,
            minor,
            micro,
        }
    }

    /// Decodes an `AV_VERSION_INT` (`major << 16 | minor << 8 | micro`), as returned by
    /// `avcodec_version()`.
    #[must_use]
    pub const fn from_int(v: u32) -> LibVersion {
        LibVersion::new(v >> 16, (v >> 8) & 0xff, v & 0xff)
    }
}

impl fmt::Display for LibVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.micro)
    }
}

/// One linked FFmpeg library: the version of its headers at build time and of the library the
/// process loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkedLibrary {
    /// `libavcodec`, `libavformat`, `libavutil`, `libswscale` or `libswresample`.
    pub name: &'static str,
    /// From the headers (`LIBAVCODEC_VERSION_MAJOR/MINOR/MICRO`).
    pub built: LibVersion,
    /// From the loaded library (`avcodec_version()`).
    pub loaded: LibVersion,
}

/// The start-up rule: every loaded library must be exactly the version its headers declared.
///
/// Exact, not "same major": the bindings convert C enums into Rust enums, and a newer minor
/// release can add enum values (a pixel format, a codec id) that would be undefined behaviour
/// to receive. The headers and the libraries must come from the same bundle.
#[cfg(any(feature = "ffmpeg", test))]
pub(crate) fn check_linked(libs: &[LinkedLibrary]) -> Result<(), MediaError> {
    match libs.iter().find(|l| l.built != l.loaded) {
        Some(l) => Err(MediaError::RuntimeMismatch {
            library: l.name,
            built: l.built,
            loaded: l.loaded,
        }),
        None => Ok(()),
    }
}

/// An FFmpeg release number (`8.1.3`), parsed from the version banner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Release {
    /// Major.
    pub major: u32,
    /// Minor.
    pub minor: u32,
    /// Patch (0 when the banner has none, as in `8.1`).
    pub patch: u32,
}

impl Release {
    /// Parses the release from an FFmpeg version banner (`av_version_info()`, the
    /// `ffmpeg -version` word): `8.1.3` (release tarball, the CI bundle), `n8.1.3-39-g660b5aa7db`
    /// (git release branch, e.g. BtbN builds), `8.1.3-1ubuntu1` (distro). Builds from the master
    /// branch (`N-121234-g…`) carry no release and give `None`.
    #[must_use]
    pub fn parse(banner: &str) -> Option<Release> {
        let s = banner.trim();
        let s = s.strip_prefix('n').unwrap_or(s);
        let end = s
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(s.len());
        let mut parts = s[..end].split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = match parts.next() {
            Some(p) => p.parse().ok()?,
            None => 0,
        };
        Some(Release {
            major,
            minor,
            patch,
        })
    }

    /// The pinned release ([`PINNED_FFMPEG`]).
    #[must_use]
    pub fn pinned() -> Release {
        // PINNED_FFMPEG is a literal checked by the tests below.
        Release::parse(PINNED_FFMPEG).unwrap_or(Release {
            major: 0,
            minor: 0,
            patch: 0,
        })
    }

    /// Whether `self` is in the same release series (major.minor) as `other`. ADR-0003 pins the
    /// 8.1.x series; patch releases are ABI compatible.
    #[must_use]
    pub fn same_series(self, other: Release) -> bool {
        (self.major, self.minor) == (other.major, other.minor)
    }
}

impl fmt::Display for Release {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(name: &'static str, built: (u32, u32, u32), loaded: (u32, u32, u32)) -> LinkedLibrary {
        LinkedLibrary {
            name,
            built: LibVersion::new(built.0, built.1, built.2),
            loaded: LibVersion::new(loaded.0, loaded.1, loaded.2),
        }
    }

    #[test]
    fn version_ints_decode_like_av_version_int() {
        // AV_VERSION_INT(62, 28, 103) = 62 << 16 | 28 << 8 | 103
        let v = LibVersion::from_int(0x003E_1C67);
        assert_eq!(v, LibVersion::new(62, 28, 103));
        assert_eq!(v.to_string(), "62.28.103");
    }

    #[test]
    fn equal_libraries_pass() {
        let libs = [
            lib("libavcodec", (62, 28, 103), (62, 28, 103)),
            lib("libavutil", (60, 26, 101), (60, 26, 101)),
        ];
        assert_eq!(check_linked(&libs), Ok(()));
    }

    #[test]
    fn any_difference_is_a_typed_error_naming_the_first_library() {
        for loaded in [(62, 28, 104), (62, 29, 103), (63, 0, 100), (62, 28, 102)] {
            let libs = [
                lib("libavutil", (60, 26, 101), (60, 26, 101)),
                lib("libavcodec", (62, 28, 103), loaded),
                lib("libavformat", (62, 12, 100), (61, 0, 0)),
            ];
            assert_eq!(
                check_linked(&libs),
                Err(MediaError::RuntimeMismatch {
                    library: "libavcodec",
                    built: LibVersion::new(62, 28, 103),
                    loaded: LibVersion::new(loaded.0, loaded.1, loaded.2),
                }),
                "loaded {loaded:?}"
            );
        }
    }

    #[test]
    fn release_banners_parse() {
        let r = |a, b, c| {
            Some(Release {
                major: a,
                minor: b,
                patch: c,
            })
        };
        assert_eq!(Release::parse("8.1.3"), r(8, 1, 3));
        assert_eq!(Release::parse("n8.1.3-39-g660b5aa7db"), r(8, 1, 3));
        assert_eq!(Release::parse("8.1.3-1ubuntu1"), r(8, 1, 3));
        assert_eq!(Release::parse("7.1"), r(7, 1, 0));
        assert_eq!(Release::parse("n7.1-static"), r(7, 1, 0));
        assert_eq!(Release::parse(" 8.1.3\n"), r(8, 1, 3));
        assert_eq!(Release::parse("N-121234-gabcdef0123"), None);
        assert_eq!(Release::parse(""), None);
        assert_eq!(Release::parse("8"), None);
    }

    #[test]
    fn series_compare_ignores_the_patch() {
        let pinned = Release::pinned();
        assert_eq!(pinned.to_string(), PINNED_FFMPEG);
        assert!(Release::parse("n8.1.4").is_some_and(|r| r.same_series(pinned)));
        assert!(!Release::parse("8.0.1").is_some_and(|r| r.same_series(pinned)));
    }

    /// The constant must follow the bundle pin, or the FFmpeg-backed banner test checks the
    /// wrong thing.
    #[test]
    fn pin_matches_the_bundle_pin() {
        let toml = include_str!("../../../scripts/ffmpeg/versions.toml");
        let mut in_ffmpeg = false;
        let mut version = None;
        for line in toml.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_ffmpeg = line == "[ffmpeg]";
            } else if in_ffmpeg && let Some(rest) = line.strip_prefix("version") {
                version = rest
                    .trim_start()
                    .strip_prefix('=')
                    .map(|v| v.trim().trim_matches('"').to_owned());
                break;
            }
        }
        assert_eq!(version.as_deref(), Some(PINNED_FFMPEG));
    }
}
