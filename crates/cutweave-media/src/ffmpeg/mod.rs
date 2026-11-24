// SPDX-License-Identifier: GPL-3.0-or-later
//! [`FfmpegBackend`]: FFmpeg 8.1 through `ffmpeg-next` 9.1 (ADR-0003). Feature `ffmpeg`.
//!
//! Start-up ([`FfmpegBackend::new`], once per process):
//!
//! 1. **Version lock.** The libav* libraries the process loaded must be exactly the versions
//!    of the headers the engine was built against, or `new` fails with
//!    [`MediaError::RuntimeMismatch`] before any call that converts FFmpeg enums (S3 F2.3).
//! 2. **Logging.** `av_log` goes to `tracing` (target `ffmpeg`), rate limited.
//! 3. **Banner.** The `ffmpeg -version` banner of the loaded libraries is logged at `info`
//!    (target `cutweave_media`), with a warning when it is not the pinned release series.
//!
//! Paths are validated before FFmpeg sees them (ffmpeg-next panics on non-UTF-8 paths and
//! interior NULs, S3 F2.2), and open failures map to typed errors. Probe/index (WP-1D.02),
//! decode (WP-1D.03) and hardware decode (WP-1D.04) build on [`input`]; the encoder (WP-1E.01)
//! is in `encode`.

mod encode;
mod error;
mod index;
mod log;
mod probe;
// Justification: FFmpeg FFI that ffmpeg-next does not wrap (version getters, the C-variadic log
// callback, codec enumeration), ADR-0003 and AGENTS.md ("no unsafe outside the crates that need
// it for FFI"). The rest of the crate stays `unsafe`-free; every block in `sys` has a
// `// SAFETY:` comment.
#[allow(unsafe_code)]
mod sys;

use std::fmt;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use cutweave_model::ProbeInfo;

use crate::backend::{AudioDecoder, DecodeOpts, EncodeSpec, Encoder, MediaBackend, VideoDecoder};
use crate::version::{self, LinkedLibrary, Release};
use crate::{Capabilities, FrameIndex, MediaError};

const BACKEND: &str = "ffmpeg";

/// What FFmpeg the process loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeInfo {
    /// The version banner (`av_version_info()`, the word after `ffmpeg version`), e.g.
    /// `8.1.3` for the CI bundle or `n8.1.3-39-g660b5aa7db` for a git release-branch build.
    pub version: String,
    /// The release parsed from [`version`](Self::version) (`None` for master-branch builds).
    pub release: Option<Release>,
    /// Header and loaded version of each linked library (all equal once `new` succeeded).
    pub libraries: Vec<LinkedLibrary>,
    /// libavcodec's license (`GPL version 3 or later` for the GPL bundle).
    pub license: String,
    /// The configure line.
    pub configuration: String,
}

impl RuntimeInfo {
    fn read() -> RuntimeInfo {
        let version = sys::version_info();
        RuntimeInfo {
            release: Release::parse(&version),
            version,
            libraries: sys::linked_libraries(),
            license: sys::license(),
            configuration: sys::configuration(),
        }
    }

    /// Whether the loaded release is in the pinned series ([`crate::PINNED_FFMPEG`]).
    #[must_use]
    pub fn is_pinned_series(&self) -> bool {
        self.release
            .is_some_and(|r| r.same_series(Release::pinned()))
    }
}

impl fmt::Display for RuntimeInfo {
    /// `FFmpeg 8.1.3 (libavutil 60.26.103, libavcodec 62.28.103, …), GPL version 3 or later`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FFmpeg {} (", self.version)?;
        for (i, lib) in self.libraries.iter().enumerate() {
            let sep = if i == 0 { "" } else { ", " };
            write!(f, "{sep}{} {}", lib.name, lib.loaded)?;
        }
        write!(f, "), {}", self.license)
    }
}

/// The outcome of the process-wide start-up.
static INIT: OnceLock<Result<Arc<RuntimeInfo>, MediaError>> = OnceLock::new();

/// Once per process: version lock, log routing, banner.
fn init() -> Result<Arc<RuntimeInfo>, MediaError> {
    INIT.get_or_init(|| {
        let info = RuntimeInfo::read();
        if let Err(e) = version::check_linked(&info.libraries) {
            tracing::error!(target: "cutweave_media", "refusing to use {info}: {e}");
            return Err(e);
        }
        sys::install_log_callback();
        tracing::info!(target: "cutweave_media", "media backend: {info}");
        if !info.is_pinned_series() {
            tracing::warn!(
                target: "cutweave_media",
                "FFmpeg {} is not the pinned {} series; only the pinned bundle is supported \
                 (docs/dev/ffmpeg.md)",
                info.version,
                crate::PINNED_FFMPEG
            );
        }
        Ok(Arc::new(info))
    })
    .clone()
}

/// The FFmpeg-backed [`MediaBackend`]. Cheap to clone; all clones share one start-up.
#[derive(Clone, Debug)]
pub struct FfmpegBackend {
    runtime: Arc<RuntimeInfo>,
    caps: Arc<Capabilities>,
}

impl FfmpegBackend {
    /// Checks the loaded FFmpeg against the build (see the module docs) and returns the backend.
    ///
    /// Errors: [`MediaError::RuntimeMismatch`] if the loaded libav* versions differ from the
    /// headers this crate was built against. (Missing libraries fail earlier, at process load:
    /// see docs/dev/ffmpeg.md.)
    pub fn new() -> Result<FfmpegBackend, MediaError> {
        static CAPS: OnceLock<Arc<Capabilities>> = OnceLock::new();
        let runtime = init()?;
        let caps = CAPS
            .get_or_init(|| Arc::new(Capabilities::with_codecs(sys::software_codecs())))
            .clone();
        Ok(FfmpegBackend { runtime, caps })
    }

    /// The FFmpeg the process loaded.
    #[must_use]
    pub fn runtime(&self) -> &RuntimeInfo {
        &self.runtime
    }
}

/// Opening an input: the seam WP-1D.02/1D.03 continue from.
pub(crate) mod input {
    use std::path::Path;

    use ffmpeg_next::format::context::Input;

    use super::error::media_error;
    use crate::MediaError;

    /// Opens `path` for demuxing (container header and stream info read).
    pub(crate) fn open(path: &Path) -> Result<Input, MediaError> {
        let checked = crate::path::check_file(path)?;
        ffmpeg_next::format::input(checked).map_err(|e| media_error(path, e))
    }
}

impl MediaBackend for FfmpegBackend {
    fn probe(&self, path: &Path) -> Result<ProbeInfo, MediaError> {
        probe::probe(path)
    }

    fn index_media(&self, path: &Path) -> Result<FrameIndex, MediaError> {
        index::index_media(path)
    }

    fn open_decoder(
        &self,
        path: &Path,
        _opts: &DecodeOpts,
    ) -> Result<Box<dyn VideoDecoder>, MediaError> {
        let _input = input::open(path)?;
        Err(MediaError::not_implemented(
            BACKEND,
            "open_decoder (WP-1D.03)",
        ))
    }

    fn open_audio_decoder(
        &self,
        path: &Path,
        _stream: Option<u32>,
    ) -> Result<Box<dyn AudioDecoder>, MediaError> {
        let _input = input::open(path)?;
        Err(MediaError::not_implemented(
            BACKEND,
            "open_audio_decoder (WP-1D.03)",
        ))
    }

    fn open_encoder(&self, spec: &EncodeSpec) -> Result<Box<dyn Encoder>, MediaError> {
        spec.validate()?;
        encode::open(spec)
    }

    fn capabilities(&self) -> Capabilities {
        (*self.caps).clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::*;
    use crate::version::PINNED_FFMPEG;

    /// Records `(target, level, message)` of every event (no tracing-subscriber dependency).
    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<(String, Level, String)>>,
    }

    struct Message(String);

    impl Visit for Message {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }

    impl Subscriber for &'static Recorder {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut m = Message(String::new());
            event.record(&mut m);
            let meta = event.metadata();
            self.events
                .lock()
                .unwrap()
                .push((meta.target().to_owned(), *meta.level(), m.0));
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    fn recorded(f: impl FnOnce()) -> Vec<(String, Level, String)> {
        let recorder: &'static Recorder = Box::leak(Box::default());
        tracing::subscriber::with_default(recorder, f);
        recorder.events.lock().unwrap().clone()
    }

    #[test]
    fn loaded_libraries_equal_the_headers_and_the_pin() {
        let backend = FfmpegBackend::new().expect("FFmpeg runtime matches the build");
        let rt = backend.runtime();
        assert_eq!(rt.libraries.len(), 5);
        for lib in &rt.libraries {
            assert_eq!(lib.built, lib.loaded, "{}", lib.name);
        }
        // The acceptance criterion: the bundled libraries are the pinned release.
        assert_eq!(
            rt.release.map(|r| r.to_string()).as_deref(),
            Some(PINNED_FFMPEG),
            "loaded FFmpeg banner `{}`",
            rt.version
        );
        assert!(rt.is_pinned_series());
        assert!(rt.license.contains("GPL"), "license `{}`", rt.license);
        assert!(rt.to_string().starts_with("FFmpeg "), "{rt}");
    }

    #[test]
    fn log_lines_reach_tracing_rate_limited() {
        FfmpegBackend::new().expect("backend");
        let line = "cutweave-media test: mmco: unref short failure";
        let events = recorded(|| {
            for _ in 0..1000 {
                sys::test_support::av_log(ffmpeg_next::ffi::AV_LOG_ERROR, &format!("{line}\n"));
            }
            // Pieces of one line are joined; a debug line is below FFmpeg's default threshold.
            sys::test_support::av_log(
                ffmpeg_next::ffi::AV_LOG_WARNING,
                "cutweave-media test: two ",
            );
            sys::test_support::av_log(ffmpeg_next::ffi::AV_LOG_WARNING, "pieces\n");
            sys::test_support::av_log(
                ffmpeg_next::ffi::AV_LOG_DEBUG,
                "cutweave-media test: hidden\n",
            );
        });
        let ours: Vec<_> = events
            .iter()
            .filter(|(target, _, m)| target == "ffmpeg" && m.contains("cutweave-media test"))
            .collect();
        let mmco: Vec<&str> = ours
            .iter()
            .filter(|(_, _, m)| m.contains("mmco"))
            .map(|(_, _, m)| m.as_str())
            .collect();
        // Occurrences 1, 2, 3, then 4, 8, …, 512: 11 lines instead of 1000.
        assert_eq!(mmco.len(), 11, "{mmco:#?}");
        assert_eq!(mmco[0], line);
        assert_eq!(mmco[3], format!("{line} (repeated 4 times)"));
        assert_eq!(mmco[10], format!("{line} (repeated 512 times)"));
        assert!(ours.iter().all(|(_, level, _)| *level == Level::WARN));
        assert!(
            ours.iter()
                .any(|(_, _, m)| m == "cutweave-media test: two pieces"),
            "{ours:#?}"
        );
        assert!(!ours.iter().any(|(_, _, m)| m.contains("hidden")));
    }

    #[test]
    fn start_up_logs_the_pinned_banner_once() {
        // Start-up runs once per process. nextest runs each test in its own process, so here it
        // is normally this test's; under `cargo test` another test may have run it first.
        let first = INIT.get().is_none();
        let events = recorded(|| {
            FfmpegBackend::new().expect("backend");
            FfmpegBackend::new().expect("backend");
        });
        let banners: Vec<&str> = events
            .iter()
            .filter(|(target, level, m)| {
                target == "cutweave_media"
                    && *level == Level::INFO
                    && m.starts_with("media backend: ")
            })
            .map(|(_, _, m)| m.as_str())
            .collect();
        if !first {
            assert!(banners.is_empty(), "{banners:?}");
            return;
        }
        assert_eq!(banners.len(), 1, "{events:#?}");
        let banner = banners[0];
        // `media backend: FFmpeg <version> (libavutil …, libavcodec …), GPL …`
        let version = banner
            .strip_prefix("media backend: FFmpeg ")
            .and_then(|rest| rest.split_whitespace().next())
            .expect("version word");
        assert_eq!(
            Release::parse(version).map(|r| r.to_string()).as_deref(),
            Some(PINNED_FFMPEG),
            "{banner}"
        );
        assert!(banner.contains("libavcodec 62."), "{banner}");
        assert!(banner.contains("GPL"), "{banner}");
        assert!(
            !events.iter().any(|(_, level, _)| *level == Level::WARN),
            "the pinned bundle logs no warning: {events:#?}"
        );
    }

    #[test]
    fn capabilities_list_the_software_codecs_of_the_bundle() {
        let caps = FfmpegBackend::new().expect("backend").capabilities();
        for codec in ["h264", "hevc", "prores", "aac", "mjpeg", "pcm_s16le"] {
            assert!(caps.can_decode(codec), "no {codec} decoder");
        }
        // The GPL bundle's encoders (scripts/ffmpeg/README.md).
        for codec in ["h264", "hevc", "prores", "aac", "mjpeg"] {
            assert!(caps.can_encode(codec), "no {codec} encoder");
        }
        let h264 = caps.codec("h264").expect("h264");
        assert!(h264.encoders.iter().any(|e| e == "libx264"), "{h264:?}");
        // Hardware wrappers are not software codecs (WP-1D.04/1E.02 list them separately).
        let all: Vec<&String> = caps
            .codecs
            .iter()
            .flat_map(|c| c.decoders.iter().chain(&c.encoders))
            .collect();
        for hw in [
            "h264_cuvid",
            "h264_nvenc",
            "h264_qsv",
            "h264_amf",
            "h264_mf",
            "h264_videotoolbox",
            "h264_vaapi",
        ] {
            assert!(!all.iter().any(|n| *n == hw), "{hw} listed as software");
        }
        assert!(caps.hw_decode.is_empty() && caps.hw_encode.is_empty());
    }
}
