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
//! decode (WP-1D.03: `decode` for video, `audio`) and hardware decode (WP-1D.04: devices,
//! eligible streams and the probe in `hwdec`) build on [`input`]; the encoder (WP-1E.01) is in
//! `encode`, its hardware encoders and their probe (WP-1E.02) in `hwenc`.

mod audio;
mod concat;
mod decode;
mod encode;
mod error;
mod hwdec;
mod hwenc;
mod index;
mod log;
mod probe;
mod sequence;
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

use crate::backend::{
    AudioDecoder, ConcatSpec, DecodeOpts, EncodeSpec, EncodeSummary, Encoder, MediaBackend,
    VideoDecoder,
};
use crate::version::{self, LinkedLibrary, Release};
use crate::{Capabilities, FrameIndex, MediaError, SequenceSource};

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
    /// Which hardware decoders `HwMode::Auto` decoders may use.
    hw: Arc<hwdec::Policy>,
}

impl FfmpegBackend {
    /// Checks the loaded FFmpeg against the build (see the module docs) and returns the backend.
    /// Hardware decode follows `CUTWEAVE_HW` ([`with_hw_decoders`](Self::with_hw_decoders)
    /// syntax; unset is `auto`, a value that does not parse is logged and means `off`).
    ///
    /// Errors: [`MediaError::RuntimeMismatch`] if the loaded libav* versions differ from the
    /// headers this crate was built against. (Missing libraries fail earlier, at process load:
    /// see docs/dev/ffmpeg.md.)
    pub fn new() -> Result<FfmpegBackend, MediaError> {
        static CAPS: OnceLock<Arc<Capabilities>> = OnceLock::new();
        static HW: OnceLock<Arc<hwdec::Policy>> = OnceLock::new();
        let runtime = init()?;
        let caps = CAPS
            .get_or_init(|| Arc::new(Capabilities::with_codecs(sys::software_codecs())))
            .clone();
        let hw = HW
            .get_or_init(|| Arc::new(hwdec::Policy::from_env()))
            .clone();
        Ok(FfmpegBackend { runtime, caps, hw })
    }

    /// The same backend with hardware decoders restricted by `spec` (WP-1D.04), the syntax of
    /// `CUTWEAVE_HW`, case-insensitive:
    ///
    /// - `off` (also `none`, `sw`, `0`): every decoder decodes in software;
    /// - `auto` (also empty, `on`, `1`): the OS's devices in order: Windows `cuda` (NVDEC),
    ///   then `d3d11va` on an NVIDIA, AMD or Intel GPU; macOS `videotoolbox`; Linux `cuda`,
    ///   then `vaapi`;
    /// - a comma-separated list of devices, tried in that order: `cuda` (or `nvdec`),
    ///   `d3d11va` (every vendor), `d3d11va:nvidia`, `d3d11va:amd`, `d3d11va:intel`,
    ///   `d3d11va:vendor=0xNNNN`, `vaapi`, `vaapi:/dev/dri/renderDN`, `videotoolbox`. GPUs are
    ///   named by vendor, never by adapter index (adapter order changes inside a process).
    ///
    /// Only devices that pass the probe are used (listed in
    /// [`Capabilities::hw_decode`](crate::Capabilities::hw_decode)); decoders opened with
    /// [`HwMode::Off`](crate::HwMode::Off) stay in software whatever the policy.
    ///
    /// Errors: [`MediaError::Invalid`] for a spec that does not parse.
    pub fn with_hw_decoders(mut self, spec: &str) -> Result<FfmpegBackend, MediaError> {
        let policy = hwdec::Policy::parse(spec).map_err(|why| MediaError::Invalid {
            message: format!("hardware decoders `{spec}`: {why}"),
        })?;
        self.hw = Arc::new(policy);
        Ok(self)
    }

    /// The same backend with a pseudo hardware decoder failing as `faults` says (tests).
    #[cfg(test)]
    fn with_pseudo_hw(mut self, faults: hwdec::Faults) -> FfmpegBackend {
        self.hw = Arc::new(hwdec::Policy::Pseudo(faults));
        self
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

    use cutweave_model::FrameRate;
    use ffmpeg_next::Dictionary;
    use ffmpeg_next::format::context::Input;

    use super::error::media_error;
    use crate::MediaError;

    /// Opens the numbered pictures at `pattern` (`shot_%04d.png` in its directory) from number
    /// `start` as one stream of `fps` pictures a second, with FFmpeg's `image2` demuxer
    /// (ADR-0035). `first_file` is the file of `start`: it must exist. The stream ends at the
    /// first number without a file; packet `n` has pts `n` in a time base of one frame.
    pub(crate) fn open_sequence(
        pattern: &Path,
        first_file: &Path,
        start: u32,
        fps: FrameRate,
    ) -> Result<Input, MediaError> {
        crate::path::check_file(first_file)?;
        let checked = crate::path::check_syntax(pattern)?;
        let mut options = Dictionary::new();
        // The counter is the only thing `%` means in the pattern: no glob characters.
        options.set("pattern_type", "sequence");
        options.set("start_number", &start.to_string());
        // Exactly `start`: never a nearby number.
        options.set("start_number_range", "1");
        options.set("framerate", &format!("{}/{}", fps.num(), fps.den()));
        // The rate is given: no packets are read to estimate it.
        options.set("fpsprobesize", "0");
        ffmpeg_next::format::input_with_dictionary(checked, options)
            .map_err(|e| media_error(first_file, e))
    }

    /// Opens `path` for demuxing (container header and stream info read).
    pub(crate) fn open(path: &Path) -> Result<Input, MediaError> {
        let checked = crate::path::check_file(path)?;
        ffmpeg_next::format::input(checked).map_err(|e| media_error(path, e))
    }
}

impl MediaBackend for FfmpegBackend {
    fn probe(&self, path: &Path) -> Result<ProbeInfo, MediaError> {
        if let Some(run) = SequenceSource::parse(path) {
            // An image sequence (ADR-0035): the first file only.
            return Ok(run.probe(probe::probe(&run.first_file())?));
        }
        probe::probe(path)
    }

    fn index_media(&self, path: &Path) -> Result<FrameIndex, MediaError> {
        if let Some(run) = SequenceSource::parse(path) {
            // Computed (frame `n` is file `first + n`): nothing is scanned.
            crate::path::check_file(&run.first_file())?;
            return run.index();
        }
        index::index_media(path)
    }

    fn open_decoder(
        &self,
        path: &Path,
        opts: &DecodeOpts,
    ) -> Result<Box<dyn VideoDecoder>, MediaError> {
        if let Some(run) = SequenceSource::parse(path) {
            return sequence::open(run, opts, Arc::clone(&self.hw));
        }
        decode::open(path, opts, &self.hw)
    }

    fn open_audio_decoder(
        &self,
        path: &Path,
        stream: Option<u32>,
    ) -> Result<Box<dyn AudioDecoder>, MediaError> {
        if SequenceSource::parse(path).is_some() {
            // An image sequence has no sound.
            return Err(MediaError::NoStream {
                path: crate::path::display(path),
                kind: cutweave_model::StreamKind::Audio,
                index: stream,
            });
        }
        audio::open(path, stream)
    }

    fn open_encoder(&self, spec: &EncodeSpec) -> Result<Box<dyn Encoder>, MediaError> {
        spec.validate()?;
        encode::open(spec)
    }

    fn concat(&self, spec: &ConcatSpec) -> Result<EncodeSummary, MediaError> {
        concat::concat(spec)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // Probed on first use (WP-1D.04, WP-1E.02), then cached for the process.
            hw_decode: hwdec::capabilities(&self.hw),
            hw_encode: hwenc::capabilities(),
            ..(*self.caps).clone()
        }
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

    /// `(target, level, message)` of every event `f` emits on this thread.
    pub(super) fn recorded(f: impl FnOnce()) -> Vec<(String, Level, String)> {
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
        // Hardware decoders (WP-1D.04): whatever this machine verified, H.264 and HEVC only;
        // none with hardware decode off.
        for hw in &caps.hw_decode {
            assert!(["h264", "hevc"].contains(&hw.codec.as_str()), "{hw:?}");
            assert!(hw.implementation.starts_with(&hw.codec), "{hw:?}");
        }
        let off = FfmpegBackend::new()
            .and_then(|b| b.with_hw_decoders("off"))
            .expect("backend");
        assert_eq!(off.capabilities().hw_decode, Vec::new());
    }
}
