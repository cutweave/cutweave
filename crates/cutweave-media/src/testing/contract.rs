// SPDX-License-Identifier: GPL-3.0-or-later
//! The backend contract: behaviour every [`MediaBackend`] must share, so callers never branch on
//! which backend they have (ADR-0003). [`backend_contract!`](crate::backend_contract) turns each
//! case into a `#[test]` for a given backend; `cutweave-media` runs it against `FakeBackend` and,
//! with the `ffmpeg` feature, `FfmpegBackend`.
//!
//! The cases need no media files: the fixtures are a missing path, an empty file, a file of
//! garbage and a directory, created in a fresh temporary directory per test. The encode cases
//! (WP-1E.01) push small synthetic pictures and tones ([`small_encode_spec`]) and check the
//! summary, progress, the input rules, cancellation and that nothing is left behind. Probe and
//! decode cases on real media join with WP-1D.02/1D.03, which generate or fetch it.
//!
//! These are test-support functions: they panic with a message when the backend breaks the
//! contract.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_model::{Duration, FrameRate, SampleRate, Time};

use crate::backend::{
    AudioEncodeSpec, DecodeOpts, EncodeSpec, Encoder, MediaBackend, VideoEncodeSpec,
};
use crate::testing::{FAKE_AUDIO_CODEC, FAKE_VIDEO_CODEC, FakeAudio, FakeVideo};
use crate::{
    AudioChunk, CancelCheck, EncodeProgress, MediaError, PixelFormat, ProgressSink, RateControl,
    VideoFrame,
};

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

/// The error of each opening call on `path`: probe, video decoder, audio decoder.
fn open_errors(b: &dyn MediaBackend, path: &Path) -> [(&'static str, Option<MediaError>); 3] {
    [
        ("probe", b.probe(path).err()),
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

/// Specs without streams, with empty codec names or sizes, a CRF above 51, zero bitrates or a NUL
/// in the path are [`MediaError::Invalid`] / [`MediaError::InvalidPath`], and create no file.
pub fn invalid_encode_specs_are_rejected(b: &dyn MediaBackend, fx: &Fixtures) {
    let path = fx.dir().join("out.mp4");
    let video = VideoEncodeSpec::h264(64, 36, FrameRate::FPS_30, PixelFormat::Yuv420p);
    let audio = AudioEncodeSpec::aac(SampleRate::HZ_48_000, 2);
    let base = EncodeSpec::new(&path);
    let with_video = |v: VideoEncodeSpec| base.clone().with_video(v);
    let with_audio = |a: AudioEncodeSpec| base.clone().with_audio(a);
    let cases = [
        ("no streams", base.clone()),
        (
            "empty video size",
            with_video(VideoEncodeSpec {
                width: 0,
                ..video.clone()
            }),
        ),
        (
            "empty codec name",
            with_video(VideoEncodeSpec {
                codec: String::new(),
                ..video.clone()
            }),
        ),
        (
            "CRF 52",
            with_video(VideoEncodeSpec {
                rate_control: RateControl::Crf(52),
                ..video.clone()
            }),
        ),
        (
            "zero video bitrate",
            with_video(VideoEncodeSpec {
                rate_control: RateControl::Bitrate(0),
                ..video
            }),
        ),
        (
            "no audio channels",
            with_audio(AudioEncodeSpec {
                channels: 0,
                ..audio.clone()
            }),
        ),
        (
            "zero audio bitrate",
            with_audio(AudioEncodeSpec {
                bitrate: 0,
                ..audio
            }),
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

/// Width and height of the encode cases' pictures.
const ENCODE_SIZE: (u32, u32) = (64, 36);
/// Frame rate of the encode cases.
const ENCODE_RATE: FrameRate = FrameRate::FPS_30;
/// Sample rate of the encode cases.
const ENCODE_SAMPLE_RATE: SampleRate = SampleRate::HZ_48_000;
/// Audio samples per video frame (48 kHz at 30 fps).
const SAMPLES_PER_FRAME: u64 = 1600;

/// A 64×36 RGBA 30 fps + 48 kHz stereo output at `path` that `b` can write: H.264 + AAC when it
/// has both encoders, the fake's raw codecs otherwise.
#[must_use]
pub fn small_encode_spec(b: &dyn MediaBackend, path: &Path) -> EncodeSpec {
    let caps = b.capabilities();
    let (video, audio) = if caps.can_encode("h264") && caps.can_encode("aac") {
        ("h264", "aac")
    } else {
        (FAKE_VIDEO_CODEC, FAKE_AUDIO_CODEC)
    };
    let (w, h) = ENCODE_SIZE;
    EncodeSpec::new(path)
        .with_video(VideoEncodeSpec {
            codec: video.to_owned(),
            ..VideoEncodeSpec::h264(w, h, ENCODE_RATE, PixelFormat::Rgba8)
        })
        .with_audio(AudioEncodeSpec {
            codec: audio.to_owned(),
            ..AudioEncodeSpec::aac(ENCODE_SAMPLE_RATE, 2)
        })
}

/// Frame `n` of the encode cases (a [`FakeVideo`] picture, RGBA).
fn picture(n: u64) -> VideoFrame {
    let (w, h) = ENCODE_SIZE;
    FakeVideo::new(w, h, ENCODE_RATE, n + 1)
        .frame(n, PixelFormat::Rgba8)
        .unwrap_or_else(|e| panic!("fake frame {n}: {e}"))
}

/// Samples `first..first + n` of a 440 Hz stereo tone.
fn sound(first: u64, n: u64) -> AudioChunk {
    let tone = FakeAudio {
        rate: ENCODE_SAMPLE_RATE,
        channels: 2,
        samples: first + n,
        frequency_hz: 440.0,
        amplitude: 0.5,
    };
    let at = i64::try_from(first).unwrap_or(i64::MAX);
    AudioChunk {
        pts: ENCODE_SAMPLE_RATE
            .samples_to_time(at)
            .unwrap_or_else(|e| panic!("sample {first}: {e}")),
        rate: ENCODE_SAMPLE_RATE,
        channels: 2,
        planes: (0..2)
            .map(|c| (first..first + n).map(|i| tone.sample(c, i)).collect())
            .collect(),
    }
}

/// Pushes video frames `frames` and their audio, one frame's worth at a time.
fn push_av(enc: &mut dyn Encoder, frames: std::ops::Range<u64>) {
    for n in frames {
        enc.push_video(&picture(n))
            .unwrap_or_else(|e| panic!("push_video({n}): {e}"));
        enc.push_audio(&sound(n * SAMPLES_PER_FRAME, SAMPLES_PER_FRAME))
            .unwrap_or_else(|e| panic!("push_audio({n}): {e}"));
    }
}

/// A fresh empty directory under the fixtures for one case's outputs.
fn output_dir(fx: &Fixtures, name: &str) -> PathBuf {
    let dir = fx.dir().join(name);
    fs::create_dir(&dir).unwrap_or_else(|e| panic!("cannot create {}: {e}", dir.display()));
    dir
}

/// The names in `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn open(b: &dyn MediaBackend, spec: &EncodeSpec) -> Box<dyn Encoder> {
    b.open_encoder(spec)
        .unwrap_or_else(|e| panic!("open_encoder({}): {e}", spec.path.display()))
}

/// Ten frames and their audio come back in the summary (counts and the exact duration), and
/// progress arrives after every push and from `finish`, only grows, and ends at the summary.
pub fn encoders_report_what_they_wrote(b: &dyn MediaBackend, fx: &Fixtures) {
    let dir = output_dir(fx, "report");
    let reports = Arc::new(Mutex::new(Vec::<EncodeProgress>::new()));
    let sink = {
        let reports = Arc::clone(&reports);
        ProgressSink::new(move |p| {
            reports
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(*p);
        })
    };
    let spec = small_encode_spec(b, &dir.join("out.mp4")).with_progress(sink);
    let mut enc = open(b, &spec);
    push_av(&mut *enc, 0..10);
    let summary = enc.finish().unwrap_or_else(|e| panic!("finish: {e}"));
    let third = ENCODE_RATE
        .frame_to_time(10)
        .ok()
        .and_then(|t| Duration::try_from(t).ok());
    assert_eq!(
        (
            summary.video_frames,
            summary.audio_samples,
            Some(summary.duration)
        ),
        (10, 10 * SAMPLES_PER_FRAME, third),
        "summary"
    );
    let reports = reports
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(
        reports.len() > 20,
        "{} reports for 20 pushes and a finish",
        reports.len()
    );
    for pair in reports.windows(2) {
        let (a, z) = (pair[0], pair[1]);
        assert!(
            z.video_frames >= a.video_frames
                && z.audio_samples >= a.audio_samples
                && z.encoded >= a.encoded
                && z.bytes >= a.bytes,
            "progress went back: {a:?} then {z:?}"
        );
    }
    let last = reports[reports.len() - 1];
    assert_eq!(
        (last.video_frames, last.audio_samples, last.encoded),
        (
            summary.video_frames,
            summary.audio_samples,
            summary.duration
        ),
        "the last report is the summary"
    );
}

/// Frames of the wrong size or layout or off the frame grid, and audio of the wrong format or
/// with a gap, are [`MediaError::Invalid`]; the encoder then goes on accepting valid input.
pub fn encoders_check_their_input(b: &dyn MediaBackend, fx: &Fixtures) {
    let dir = output_dir(fx, "input");
    let spec = small_encode_spec(b, &dir.join("out.mp4"));
    let mut enc = open(b, &spec);
    let invalid = |what: &str, r: Result<(), MediaError>| {
        assert!(
            matches!(r, Err(MediaError::Invalid { .. })),
            "{what}: {r:?}, want Invalid"
        );
    };
    let (w, h) = ENCODE_SIZE;
    let frame = |w, h, format| {
        FakeVideo::new(w, h, ENCODE_RATE, 1)
            .frame(0, format)
            .unwrap_or_else(|e| panic!("fake frame: {e}"))
    };
    invalid(
        "half size",
        enc.push_video(&frame(w / 2, h / 2, PixelFormat::Rgba8)),
    );
    invalid(
        "NV12 into RGBA",
        enc.push_video(&frame(w, h, PixelFormat::Nv12)),
    );
    invalid("frame 1 first", enc.push_video(&picture(1)));
    enc.push_video(&picture(0))
        .unwrap_or_else(|e| panic!("frame 0: {e}"));
    invalid("frame 0 again", enc.push_video(&picture(0)));
    invalid("frame 2 after 0", enc.push_video(&picture(2)));
    let mut off_grid = picture(1);
    off_grid.pts = Time::from_flicks(off_grid.pts.flicks() + 1).unwrap_or(off_grid.pts);
    invalid("frame 1 one flick late", enc.push_video(&off_grid));
    enc.push_video(&picture(1))
        .unwrap_or_else(|e| panic!("frame 1: {e}"));

    invalid("audio from sample 1", enc.push_audio(&sound(1, 100)));
    let mut mono = sound(0, 100);
    mono.channels = 1;
    mono.planes.pop();
    invalid("mono into stereo", enc.push_audio(&mono));
    let mut other_rate = sound(0, 100);
    other_rate.rate = SampleRate::HZ_44_100;
    invalid("44.1 kHz into 48 kHz", enc.push_audio(&other_rate));
    enc.push_audio(&sound(0, 2 * SAMPLES_PER_FRAME))
        .unwrap_or_else(|e| panic!("audio: {e}"));
    invalid(
        "audio with a gap",
        enc.push_audio(&sound(2 * SAMPLES_PER_FRAME + 100, 100)),
    );
    let summary = enc
        .finish()
        .unwrap_or_else(|e| panic!("finish after rejected input: {e}"));
    assert_eq!(
        (summary.video_frames, summary.audio_samples),
        (2, 2 * SAMPLES_PER_FRAME)
    );

    // Streams the output does not have.
    let video_only = EncodeSpec {
        audio: None,
        ..small_encode_spec(b, &dir.join("video.mp4"))
    };
    invalid(
        "audio into a video-only output",
        open(b, &video_only).push_audio(&sound(0, 100)),
    );
    let audio_only = EncodeSpec {
        video: None,
        ..small_encode_spec(b, &dir.join("audio.mp4"))
    };
    invalid(
        "video into an audio-only output",
        open(b, &audio_only).push_video(&picture(0)),
    );
}

/// Once the cancel check answers `true`, pushes and `finish` return [`MediaError::Cancelled`]
/// and no output is left; dropping an encoder before `finish` leaves none either, and a file
/// already at the path survives both untouched.
pub fn cancelled_encodes_leave_no_file(b: &dyn MediaBackend, fx: &Fixtures) {
    let dir = output_dir(fx, "cancel");
    let cancelled = |r: Result<(), MediaError>, what: &str| {
        assert_eq!(r, Err(MediaError::Cancelled), "{what}");
    };

    // Mid-encode.
    let flag = Arc::new(AtomicBool::new(false));
    let spec = small_encode_spec(b, &dir.join("mid.mp4"))
        .with_cancel(CancelCheck::from_flag(Arc::clone(&flag)));
    let mut enc = open(b, &spec);
    push_av(&mut *enc, 0..5);
    flag.store(true, Ordering::Relaxed);
    cancelled(enc.push_video(&picture(5)), "push_video after cancel");
    cancelled(
        enc.push_audio(&sound(5 * SAMPLES_PER_FRAME, SAMPLES_PER_FRAME)),
        "push_audio after cancel",
    );
    assert_eq!(
        enc.finish().err(),
        Some(MediaError::Cancelled),
        "finish after cancel"
    );
    assert_eq!(
        entries(&dir),
        Vec::<String>::new(),
        "after a cancel mid-encode"
    );

    // Right before finish, over an existing file.
    let keep = dir.join("keep.mp4");
    fs::write(&keep, b"original")
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", keep.display()));
    let flag = Arc::new(AtomicBool::new(false));
    let spec = small_encode_spec(b, &keep).with_cancel(CancelCheck::from_flag(Arc::clone(&flag)));
    let mut enc = open(b, &spec);
    push_av(&mut *enc, 0..5);
    flag.store(true, Ordering::Relaxed);
    assert_eq!(enc.finish().err(), Some(MediaError::Cancelled), "finish");
    assert_eq!(entries(&dir), ["keep.mp4"], "after a cancelled finish");
    assert_eq!(
        fs::read(&keep).ok().as_deref(),
        Some(&b"original"[..]),
        "a cancelled encode replaced the existing file"
    );

    // Dropped before finish.
    let mut enc = open(b, &small_encode_spec(b, &dir.join("dropped.mp4")));
    push_av(&mut *enc, 0..5);
    drop(enc);
    assert_eq!(entries(&dir), ["keep.mp4"], "after dropping an encoder");
}

/// A codec the backend has no encoder for is [`MediaError::Unsupported`] and creates no file.
pub fn unknown_codecs_are_unsupported(b: &dyn MediaBackend, fx: &Fixtures) {
    let dir = output_dir(fx, "unsupported");
    let base = small_encode_spec(b, &dir.join("out.mp4"));
    let video = EncodeSpec {
        video: base.video.clone().map(|v| VideoEncodeSpec {
            codec: "no-such-codec".to_owned(),
            ..v
        }),
        ..base.clone()
    };
    let audio = EncodeSpec {
        audio: base.audio.clone().map(|a| AudioEncodeSpec {
            codec: "no-such-codec".to_owned(),
            ..a
        }),
        ..base
    };
    for (what, spec) in [("video", video), ("audio", audio)] {
        let err = b.open_encoder(&spec).err();
        assert!(
            matches!(err, Some(MediaError::Unsupported { .. })),
            "open_encoder(unknown {what} codec) = {err:?}, want Unsupported"
        );
    }
    assert_eq!(entries(&dir), Vec::<String>::new());
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
            fn invalid_encode_specs_are_rejected() {
                c::invalid_encode_specs_are_rejected(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn encoders_report_what_they_wrote() {
                c::encoders_report_what_they_wrote(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn encoders_check_their_input() {
                c::encoders_check_their_input(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn cancelled_encodes_leave_no_file() {
                c::cancelled_encodes_leave_no_file(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn unknown_codecs_are_unsupported() {
                c::unknown_codecs_are_unsupported(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn backend_is_shareable_across_threads() {
                c::backend_is_shareable_across_threads(&backend(), &c::Fixtures::new());
            }
        }
    };
}
