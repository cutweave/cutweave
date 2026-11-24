// SPDX-License-Identifier: GPL-3.0-or-later
//! The backend contract: behaviour every [`MediaBackend`] must share, so callers never branch on
//! which backend they have (ADR-0003). [`backend_contract!`](crate::backend_contract) turns each
//! case into a `#[test]` for a given backend; `cutweave-media` runs it against `FakeBackend` and,
//! with the `ffmpeg` feature, `FfmpegBackend`.
//!
//! The cases need no media files: the fixtures are a missing path, an empty file, a file of
//! garbage and a directory, created in a fresh temporary directory per test. The encode cases
//! (WP-1E.01) push small synthetic pictures and tones ([`small_encode_spec`]) and check the
//! summary, progress, the input rules, cancellation and that nothing is left behind. The decode
//! cases (WP-1D.03) encode a 1 s clip with the backend's own encoder and decode it back: frame
//! count and exact pts, seeks equal to sequential decode, cancellation, audio ranges that join
//! and seek exactly, missing streams. Corpus checks (frame-exactness against the FFmpeg CLI)
//! live in the FFmpeg-only tests.
//!
//! These are test-support functions: they panic with a message when the backend breaks the
//! contract.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_model::{Duration, FrameRate, SampleRate, StreamKind, Time};

use crate::backend::{
    AudioEncodeSpec, DecodeOpts, EncodeSpec, Encoder, HwEncode, MediaBackend, VideoDecoder,
    VideoEncodeSpec,
};
use crate::testing::{FAKE_AUDIO_CODEC, FAKE_VIDEO_CODEC, FakeAudio, FakeVideo};
use crate::{
    AudioChunk, CancelCheck, CancelToken, EncodeProgress, MediaError, PixelFormat, ProgressSink,
    RateControl, VideoFrame,
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

// ---------------------------------------------------------------------------------------------
// Decode (WP-1D.03). The clips are written by the backend's own encoder (the fake's encoder
// registers its outputs, which then read back), so the cases need no media files.
// ---------------------------------------------------------------------------------------------

/// Frames of the decode cases' clip (1 s at 30 fps).
const CLIP_FRAMES: u64 = 30;

/// How far a sample read after a seek may differ from the same sample read sequentially:
/// −80 dBFS (see [`audio_ranges_join_and_seek_exactly`]).
pub const AUDIO_SEEK_TOLERANCE: f32 = 1e-4;

/// Encodes a 1 s 64×36 30 fps clip with 48 kHz stereo audio into `dir/name` with the backend's
/// own encoder; `video` / `audio` false leave that stream out.
fn encoded_clip(b: &dyn MediaBackend, dir: &Path, name: &str, video: bool, audio: bool) -> PathBuf {
    let path = dir.join(name);
    let mut spec = small_encode_spec(b, &path);
    if !video {
        spec.video = None;
    }
    if !audio {
        spec.audio = None;
    }
    let mut enc = open(b, &spec);
    for n in 0..CLIP_FRAMES {
        if video {
            enc.push_video(&picture(n))
                .unwrap_or_else(|e| panic!("push_video({n}): {e}"));
        }
        if audio {
            enc.push_audio(&sound(n * SAMPLES_PER_FRAME, SAMPLES_PER_FRAME))
                .unwrap_or_else(|e| panic!("push_audio({n}): {e}"));
        }
    }
    enc.finish()
        .unwrap_or_else(|e| panic!("finish {}: {e}", path.display()));
    path
}

fn open_video(b: &dyn MediaBackend, path: &Path, opts: &DecodeOpts) -> Box<dyn VideoDecoder> {
    b.open_decoder(path, opts)
        .unwrap_or_else(|e| panic!("open_decoder({}): {e}", path.display()))
}

/// Every frame of `path`, decoded in order.
fn decode_all(b: &dyn MediaBackend, path: &Path) -> Vec<VideoFrame> {
    let mut d = open_video(b, path, &DecodeOpts::default());
    let mut frames = Vec::new();
    while let Some(f) = d
        .next_frame()
        .unwrap_or_else(|e| panic!("next_frame {}: {e}", frames.len()))
    {
        frames.push(f);
        assert!(
            frames.len() as u64 <= 10 * CLIP_FRAMES,
            "next_frame never ends"
        );
    }
    frames
}

fn frame_time(n: usize) -> Time {
    ENCODE_RATE
        .frame_to_time(i64::try_from(n).unwrap_or(i64::MAX))
        .unwrap_or_else(|e| panic!("frame {n}: {e}"))
}

/// The backend decodes what it encoded: every frame once, in order, with pts `n / 30 s` exactly,
/// at the encoded size in a 4:2:0 layout, undamaged; the end stays the end; the index has the
/// same frames. With [`HwMode::Off`](crate::HwMode::Off) the decoder is software.
pub fn encoded_clips_decode_back(b: &dyn MediaBackend, fx: &Fixtures) {
    let dir = output_dir(fx, "decode");
    let clip = encoded_clip(b, &dir, "clip.mp4", true, true);
    let opts = DecodeOpts::default().with_hw(crate::HwMode::Off);
    let mut d = open_video(b, &clip, &opts);
    assert!(!d.is_hw() && !d.hw_failed(), "software decode");
    let index = d.index();
    assert_eq!(index.frame_count() as u64, CLIP_FRAMES, "index frames");
    let mut n = 0usize;
    while let Some(f) = d
        .next_frame()
        .unwrap_or_else(|e| panic!("next_frame {n}: {e}"))
    {
        assert_eq!(f.pts, frame_time(n), "pts of frame {n}");
        assert_eq!(index.time_of(n), Some(f.pts), "index time of frame {n}");
        assert_eq!((f.width, f.height), ENCODE_SIZE, "size of frame {n}");
        assert_eq!(f.format, PixelFormat::Yuv420p, "layout of frame {n}");
        assert!(!f.damaged, "frame {n} flagged damaged");
        f.validate()
            .unwrap_or_else(|e| panic!("frame {n} planes: {e}"));
        n += 1;
        assert!(n as u64 <= CLIP_FRAMES, "more frames than encoded");
    }
    assert_eq!(n as u64, CLIP_FRAMES, "frames decoded");
    assert!(matches!(d.next_frame(), Ok(None)), "the end stays the end");
}

/// `seek_exact` returns exactly the frame sequential decoding gives for every on-screen time
/// (on a frame, between frames, before the first), in any order, forward and backward;
/// `next_frame` continues after it; times at or past the end are out of range. A decoder opened
/// with the first one's index agrees, and an RGBA target keeps the size.
pub fn seeks_equal_sequential_decode(b: &dyn MediaBackend, fx: &Fixtures) {
    let dir = output_dir(fx, "seek");
    let clip = encoded_clip(b, &dir, "clip.mp4", true, false);
    let all = decode_all(b, &clip);
    assert_eq!(all.len() as u64, CLIP_FRAMES);
    let mut d = open_video(b, &clip, &DecodeOpts::default());
    let half = frame_time(1).flicks() / 2;
    let order: [usize; 17] = [0, 29, 15, 14, 16, 3, 28, 1, 0, 29, 10, 11, 12, 20, 5, 5, 6];
    for (i, &n) in order.iter().enumerate() {
        let on = frame_time(n);
        let between = Time::from_flicks(on.flicks() + half).unwrap_or(on);
        let t = if i % 2 == 0 { on } else { between };
        let f = d
            .seek_exact(t)
            .unwrap_or_else(|e| panic!("seek_exact({t:?}) for frame {n}: {e}"));
        assert!(
            f == all[n],
            "seek to frame {n} ({t:?}) differs from sequential decode"
        );
    }
    let before = Time::from_flicks(-1).unwrap_or(Time::ZERO);
    assert!(
        d.seek_exact(before).ok().as_ref() == Some(&all[0]),
        "a time before the first frame shows the first frame"
    );
    // next_frame continues after the sought frame.
    d.seek_exact(frame_time(7))
        .unwrap_or_else(|e| panic!("seek 7: {e}"));
    for (n, want) in all.iter().enumerate().take(12).skip(8) {
        let f = d
            .next_frame()
            .unwrap_or_else(|e| panic!("next_frame after seek: {e}"));
        assert!(
            f.as_ref() == Some(want),
            "next_frame after seek 7 is not frame {n}"
        );
    }
    // The last frame lasts until the end; nothing is on screen from there on.
    let end = frame_time(all.len());
    let last = Time::from_flicks(end.flicks() - 1).unwrap_or(Time::ZERO);
    assert!(
        d.seek_exact(last).ok().as_ref() == Some(&all[29]),
        "the end of the last frame"
    );
    for t in [end, frame_time(all.len() + 100)] {
        assert_eq!(
            d.seek_exact(t).err(),
            Some(MediaError::OutOfRange { t }),
            "seek_exact({t:?}) past the end"
        );
    }
    // A decoder opened with a known index agrees.
    let opts = DecodeOpts::default().with_index(d.index());
    let mut e = open_video(b, &clip, &opts);
    assert!(
        e.seek_exact(frame_time(17)).ok().as_ref() == Some(&all[17]),
        "with an index"
    );
    // The RGBA thumbnail path.
    let mut rgba = open_video(
        b,
        &clip,
        &DecodeOpts::default().with_target(PixelFormat::Rgba8),
    );
    let f = rgba
        .seek_exact(frame_time(4))
        .unwrap_or_else(|e| panic!("RGBA seek: {e}"));
    assert_eq!(
        (f.format, f.width, f.height, f.pts),
        (PixelFormat::Rgba8, 64, 36, frame_time(4))
    );
    f.validate().unwrap_or_else(|e| panic!("RGBA planes: {e}"));
}

/// A cancelled token makes every decoder call return [`MediaError::Cancelled`]; without it the
/// decoder works on, still exact.
pub fn cancelled_decoders_return_cancelled(b: &dyn MediaBackend, fx: &Fixtures) {
    let dir = output_dir(fx, "cancel-decode");
    let clip = encoded_clip(b, &dir, "clip.mp4", true, true);
    let all = decode_all(b, &clip);
    let token = CancelToken::new();
    token.cancel();
    let mut d = open_video(b, &clip, &DecodeOpts::default());
    d.set_cancel(Some(token.clone()));
    assert_eq!(
        d.seek_exact(frame_time(9)).err(),
        Some(MediaError::Cancelled),
        "seek"
    );
    assert_eq!(
        d.next_frame().err(),
        Some(MediaError::Cancelled),
        "next_frame"
    );
    d.set_cancel(None);
    assert!(
        d.seek_exact(frame_time(9)).ok().as_ref() == Some(&all[9]),
        "after the cancel"
    );
    let mut a = b
        .open_audio_decoder(&clip, None)
        .unwrap_or_else(|e| panic!("open_audio_decoder: {e}"));
    a.set_cancel(Some(token));
    let tenth = Duration::from_flicks(ENCODE_RATE.frame_to_time(3).map_or(1, Time::flicks))
        .unwrap_or(Duration::ZERO);
    assert_eq!(
        a.decode_range(Time::ZERO, tenth).err(),
        Some(MediaError::Cancelled),
        "decode_range"
    );
}

/// The audio stream decodes at its native format; consecutive ranges join into exactly the
/// whole; a range read after a seek (forward or back) has exactly the same placement as in the
/// whole and the same samples within [`AUDIO_SEEK_TOLERANCE`]; ranges past the end are empty;
/// every chunk's pts is its first sample's time.
#[allow(clippy::cast_possible_truncation)] // sample counts of a 1 s clip
pub fn audio_ranges_join_and_seek_exactly(b: &dyn MediaBackend, fx: &Fixtures) {
    let dir = output_dir(fx, "audio");
    let clip = encoded_clip(b, &dir, "clip.mp4", true, true);
    let open_audio = || {
        b.open_audio_decoder(&clip, None)
            .unwrap_or_else(|e| panic!("open_audio_decoder: {e}"))
    };
    let rate = ENCODE_SAMPLE_RATE;
    let at = |n: usize| {
        rate.samples_to_time(i64::try_from(n).unwrap_or(i64::MAX))
            .unwrap_or_else(|e| panic!("sample {n}: {e}"))
    };
    let span = |n: usize| Duration::try_from(at(n)).unwrap_or(Duration::ZERO);
    let mut a = open_audio();
    assert_eq!(a.format(), crate::AudioFormat { rate, channels: 2 });
    let whole = a
        .decode_range(Time::ZERO, span(3 * 48_000))
        .unwrap_or_else(|e| panic!("whole: {e}"));
    whole.validate().unwrap_or_else(|e| panic!("whole: {e}"));
    assert_eq!(whole.pts, Time::ZERO);
    let pushed = (CLIP_FRAMES * SAMPLES_PER_FRAME) as usize;
    assert!(
        (pushed..pushed + 4096).contains(&whole.frames()),
        "{} samples decoded for {pushed} encoded",
        whole.frames()
    );
    let slice = |from: usize, to: usize| -> Vec<Vec<f32>> {
        whole
            .planes
            .iter()
            .map(|p| p[from.min(p.len())..to.min(p.len())].to_vec())
            .collect()
    };
    // 10 ms pieces join without gaps or overlaps.
    let mut a = open_audio();
    let mut joined: Vec<Vec<f32>> = vec![Vec::new(); 2];
    for k in 0..120 {
        let chunk = a
            .decode_range(at(k * 480), span(480))
            .unwrap_or_else(|e| panic!("piece {k}: {e}"));
        if chunk.frames() > 0 {
            assert_eq!(chunk.pts, at(k * 480), "pts of piece {k}");
        }
        for (j, p) in joined.iter_mut().zip(&chunk.planes) {
            j.extend_from_slice(p);
        }
    }
    assert!(
        joined == whole.planes,
        "10 ms pieces differ from one whole read"
    );
    // Random access: forward, then back, then forward again. Placement is exact; the samples
    // match to the noise floor (lossy codecs with noise substitution, such as AAC PNS, draw
    // their noise from decoder state that a seek resets: about −100 dBFS here).
    let mut a = open_audio();
    for (first, len) in [(24_000, 4_800), (12_000, 2_400), (40_000, 1_000), (0, 100)] {
        let chunk = a
            .decode_range(at(first), span(len))
            .unwrap_or_else(|e| panic!("range at {first}: {e}"));
        assert_eq!(chunk.pts, at(first), "pts of the range at {first}");
        let want = slice(first, first + len);
        assert_eq!(
            chunk.frames(),
            want[0].len(),
            "length of the range at {first}"
        );
        let worst = chunk
            .planes
            .iter()
            .zip(&want)
            .flat_map(|(got, want)| got.iter().zip(want).map(|(g, w)| (g - w).abs()))
            .fold(0.0_f32, f32::max);
        assert!(
            worst <= AUDIO_SEEK_TOLERANCE,
            "samples {first}..{} after a seek differ from the whole read by up to {worst}",
            first + len
        );
    }
    let past = a
        .decode_range(at(5 * 48_000), span(48_000))
        .unwrap_or_else(|e| panic!("past the end: {e}"));
    assert_eq!(past.frames(), 0, "a range past the end is empty");
}

/// A file without pictures has no video stream to decode and one without sound no audio
/// stream ([`MediaError::NoStream`]); asking for a stream index that is not audio is the same.
pub fn missing_streams_are_no_stream(b: &dyn MediaBackend, fx: &Fixtures) {
    let dir = output_dir(fx, "streams");
    let video_only = encoded_clip(b, &dir, "video.mp4", true, false);
    let audio_only = encoded_clip(b, &dir, "audio.mp4", false, true);
    let err = b.open_audio_decoder(&video_only, None).err();
    assert!(
        matches!(
            err,
            Some(MediaError::NoStream {
                kind: StreamKind::Audio,
                ..
            })
        ),
        "audio decoder on a video-only file: {err:?}"
    );
    let err = b.open_decoder(&audio_only, &DecodeOpts::default()).err();
    assert!(
        matches!(
            err,
            Some(MediaError::NoStream {
                kind: StreamKind::Video,
                ..
            })
        ),
        "video decoder on an audio-only file: {err:?}"
    );
    let both = encoded_clip(b, &dir, "both.mp4", true, true);
    for index in [Some(0), Some(7)] {
        let err = b.open_audio_decoder(&both, index).err();
        assert!(
            matches!(err, Some(MediaError::NoStream { kind: StreamKind::Audio, index: i, .. }) if i == index),
            "audio decoder on stream {index:?}: {err:?}"
        );
    }
}

/// Forcing a hardware encoder the backend does not have is [`MediaError::HwUnavailable`] (no
/// software fallback) and creates no file; an empty forced name is [`MediaError::Invalid`]
/// (WP-1E.02).
pub fn forced_missing_hw_encoders_are_unavailable(b: &dyn MediaBackend, fx: &Fixtures) {
    let dir = output_dir(fx, "forced-hw");
    let spec = small_encode_spec(b, &dir.join("out.mp4"));
    let err = b
        .open_encoder(
            &spec
                .clone()
                .with_hw(HwEncode::Force("no-such-hw-encoder".into())),
        )
        .err();
    assert!(
        matches!(err, Some(MediaError::HwUnavailable { .. })),
        "open_encoder(Force(no-such-hw-encoder)) = {err:?}, want HwUnavailable"
    );
    let err = b
        .open_encoder(&spec.with_hw(HwEncode::Force(String::new())))
        .err();
    assert!(
        matches!(err, Some(MediaError::Invalid { .. })),
        "open_encoder(Force(\"\")) = {err:?}, want Invalid"
    );
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
            fn hostile_files_are_media_faults() {
                c::hostile_files_are_media_faults(&*backend(), &c::Fixtures::new());
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
            fn forced_missing_hw_encoders_are_unavailable() {
                c::forced_missing_hw_encoders_are_unavailable(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn backend_is_shareable_across_threads() {
                c::backend_is_shareable_across_threads(&backend(), &c::Fixtures::new());
            }

            #[test]
            fn encoded_clips_decode_back() {
                c::encoded_clips_decode_back(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn seeks_equal_sequential_decode() {
                c::seeks_equal_sequential_decode(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn cancelled_decoders_return_cancelled() {
                c::cancelled_decoders_return_cancelled(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn audio_ranges_join_and_seek_exactly() {
                c::audio_ranges_join_and_seek_exactly(&*backend(), &c::Fixtures::new());
            }

            #[test]
            fn missing_streams_are_no_stream() {
                c::missing_streams_are_no_stream(&*backend(), &c::Fixtures::new());
            }
        }
    };
}
