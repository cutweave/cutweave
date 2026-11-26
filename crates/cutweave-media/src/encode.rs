// SPDX-License-Identifier: GPL-3.0-or-later
//! Encoder settings, hooks and the input rules every [`Encoder`](crate::Encoder) enforces.
//!
//! - [`RateControl`] and [`SpeedPreset`]: how a video encoder spends bits and time.
//! - [`CancelCheck`] and [`ProgressSink`]: hooks set on [`EncodeSpec`](crate::EncodeSpec). An
//!   encoder polls the first before each frame or chunk and while it writes (so `finish` stops
//!   too), and reports [`EncodeProgress`] to the second.
//! - `InputCheck` (crate-private): the input rules of the [`Encoder`](crate::Encoder) trait,
//!   shared by `FakeBackend` and `FfmpegBackend` so that both reject the same input the same
//!   way. A rejected frame or chunk is [`MediaError::Invalid`] and changes nothing.
//! - [`encode_with_fallback`]: runs an encode and, when a hardware encoder fails mid-stream,
//!   runs it again from the start in software (WP-1E.02).

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cutweave_model::{
    ColorPrimaries, ColorRange, ColorTags, Duration, MatrixCoefficients, Time,
    TransferCharacteristics,
};

use crate::backend::{
    AudioEncodeSpec, EncodeSpec, EncodeSummary, Encoder, HwEncode, MediaBackend, VideoEncodeSpec,
};
use crate::{AudioChunk, MediaError, PixelFormat, VideoFrame};

/// How a video encoder spends bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RateControl {
    /// Constant quality on the H.264/HEVC CRF scale, `0..=51`: lower is better and larger
    /// (18 is close to visually lossless, x264's own default is 23).
    Crf(u8),
    /// Average bitrate in bits per second.
    Bitrate(u32),
    /// Constant bitrate in bits per second: the peak equals the average and the encoder pads
    /// when the picture needs fewer bits (x264 and x265, and the hardware encoders that have
    /// the mode; SVT-AV1 does not).
    Cbr(u32),
}

impl RateControl {
    /// The CRF exports use unless told otherwise (the reference point of WP-1E.02's
    /// hardware-encoder quality table).
    pub const DEFAULT_CRF: u8 = 20;
    /// The largest CRF (the H.264/HEVC scale).
    pub const MAX_CRF: u8 = 51;
}

impl Default for RateControl {
    fn default() -> Self {
        RateControl::Crf(RateControl::DEFAULT_CRF)
    }
}

/// Encoder speed against compression, named after x264's presets (hardware encoders map them to
/// their own levels, WP-1E.02). Slower presets make smaller files at the same quality.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum SpeedPreset {
    /// Fastest, largest files.
    Ultrafast,
    /// x264 `superfast`.
    Superfast,
    /// x264 `veryfast`.
    Veryfast,
    /// x264 `faster`.
    Faster,
    /// x264 `fast`.
    Fast,
    /// x264 `medium`: the default.
    #[default]
    Medium,
    /// x264 `slow`.
    Slow,
    /// x264 `slower`.
    Slower,
    /// x264 `veryslow`.
    Veryslow,
}

impl SpeedPreset {
    /// The preset called `name` (`slow`), `None` for any other word.
    #[must_use]
    pub fn from_name(name: &str) -> Option<SpeedPreset> {
        [
            SpeedPreset::Ultrafast,
            SpeedPreset::Superfast,
            SpeedPreset::Veryfast,
            SpeedPreset::Faster,
            SpeedPreset::Fast,
            SpeedPreset::Medium,
            SpeedPreset::Slow,
            SpeedPreset::Slower,
            SpeedPreset::Veryslow,
        ]
        .into_iter()
        .find(|p| p.name() == name)
    }

    /// The x264 preset name (`medium`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            SpeedPreset::Ultrafast => "ultrafast",
            SpeedPreset::Superfast => "superfast",
            SpeedPreset::Veryfast => "veryfast",
            SpeedPreset::Faster => "faster",
            SpeedPreset::Fast => "fast",
            SpeedPreset::Medium => "medium",
            SpeedPreset::Slow => "slow",
            SpeedPreset::Slower => "slower",
            SpeedPreset::Veryslow => "veryslow",
        }
    }
}

/// BT.709 primaries, transfer and matrix in limited range: the color of every export in v1
/// (WP-1E.01). Output color fields left unset default to these values.
#[must_use]
pub fn bt709_tags() -> ColorTags {
    ColorTags {
        primaries: Some(ColorPrimaries::Bt709),
        transfer: Some(TransferCharacteristics::Bt709),
        matrix: Some(MatrixCoefficients::Bt709),
        range: Some(ColorRange::Limited),
    }
}

/// `tags` with every unset field taken from [`bt709_tags`].
pub(crate) fn output_color(tags: &ColorTags) -> ColorTags {
    let d = bt709_tags();
    ColorTags {
        primaries: tags.primaries.clone().or(d.primaries),
        transfer: tags.transfer.clone().or(d.transfer),
        matrix: tags.matrix.clone().or(d.matrix),
        range: tags.range.or(d.range),
    }
}

/// Asks whether to stop encoding. Cloning shares the check.
///
/// The encoder polls it before each frame or chunk and while it writes the file, so a cancel
/// also stops a `finish` that is flushing or moving the index to the front. Once it answers
/// `true` the encoder returns [`MediaError::Cancelled`] and removes its partial output.
#[derive(Clone)]
pub struct CancelCheck(Arc<dyn Fn() -> bool + Send + Sync>);

impl CancelCheck {
    /// Cancels once `check` returns `true`. It is called from the encoding thread, often (every
    /// frame and every 32 KiB written), so it must be cheap and must not block. Bridge a job's
    /// token with `CancelCheck::new(move || token.is_cancelled())`.
    pub fn new(check: impl Fn() -> bool + Send + Sync + 'static) -> CancelCheck {
        CancelCheck(Arc::new(check))
    }

    /// Cancels once `flag` is set.
    #[must_use]
    pub fn from_flag(flag: Arc<AtomicBool>) -> CancelCheck {
        CancelCheck::new(move || flag.load(Ordering::Relaxed))
    }

    /// Whether cancellation was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        (self.0)()
    }
}

impl fmt::Debug for CancelCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CancelCheck(..)")
    }
}

/// Equal when both are clones of the same check.
impl PartialEq for CancelCheck {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for CancelCheck {}

/// What an encoder has done so far. Every field only grows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EncodeProgress {
    /// Video frames accepted.
    pub video_frames: u64,
    /// Audio samples accepted per channel.
    pub audio_samples: u64,
    /// Output media time the encoders have produced. It trails the accepted input by the
    /// encoder's look-ahead while frames are pushed, catches up during `finish`, and equals the
    /// summary's duration in the last report.
    pub encoded: Duration,
    /// Size of the output written so far, in bytes (0 for backends that write no file).
    pub bytes: u64,
}

/// Receives [`EncodeProgress`]: after every accepted frame or chunk, while `finish` flushes,
/// and once more when `finish` succeeds. Called on the encoding thread; keep it short.
#[derive(Clone)]
pub struct ProgressSink(Arc<dyn Fn(&EncodeProgress) + Send + Sync>);

impl ProgressSink {
    /// Reports to `sink`.
    pub fn new(sink: impl Fn(&EncodeProgress) + Send + Sync + 'static) -> ProgressSink {
        ProgressSink(Arc::new(sink))
    }

    /// Delivers one report.
    pub fn report(&self, progress: &EncodeProgress) {
        (self.0)(progress);
    }
}

impl fmt::Debug for ProgressSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProgressSink(..)")
    }
}

/// Equal when both are clones of the same sink.
impl PartialEq for ProgressSink {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ProgressSink {}

/// The input rules (see the module docs) and the counts they track.
#[derive(Debug)]
pub(crate) struct InputCheck {
    video: Option<VideoEncodeSpec>,
    /// The output's color with defaults filled in (only matrix and range are compared).
    color: ColorTags,
    audio: Option<AudioEncodeSpec>,
    frames: u64,
    samples: u64,
}

fn invalid<T>(message: String) -> Result<T, MediaError> {
    Err(MediaError::Invalid { message })
}

impl InputCheck {
    pub(crate) fn new(spec: &EncodeSpec) -> InputCheck {
        InputCheck {
            color: output_color(
                &spec
                    .video
                    .as_ref()
                    .map_or_else(ColorTags::default, |v| v.color.clone()),
            ),
            video: spec.video.clone(),
            audio: spec.audio.clone(),
            frames: 0,
            samples: 0,
        }
    }

    /// Checks that `frame` is the next frame of the video stream; returns its index.
    pub(crate) fn video(&self, frame: &VideoFrame) -> Result<i64, MediaError> {
        let Some(spec) = &self.video else {
            return invalid("this output has no video stream".to_owned());
        };
        frame.validate()?;
        if (frame.width, frame.height, frame.format) != (spec.width, spec.height, spec.input_format)
        {
            return invalid(format!(
                "frame is {}x{} {:?}, the stream takes {}x{} {:?}",
                frame.width, frame.height, frame.format, spec.width, spec.height, spec.input_format
            ));
        }
        if frame.format != PixelFormat::Rgba8 {
            let (matrix, range) = (&frame.color.matrix, frame.color.range);
            if matrix.is_some() && *matrix != self.color.matrix
                || range.is_some() && range != self.color.range
            {
                return invalid(format!(
                    "frame is tagged {:?} {:?} but the output is {:?} {:?}: convert it first",
                    matrix, range, self.color.matrix, self.color.range
                ));
            }
        }
        let index = i64::try_from(self.frames).unwrap_or(i64::MAX);
        let expected = spec.rate.frame_to_time(index).ok();
        if expected != Some(frame.pts) {
            return invalid(format!(
                "frame pts {} is not the start of output frame {index} at {} fps{}: frames start \
                 at 0 and follow each other on the frame grid",
                frame.pts,
                spec.rate,
                expected.map_or_else(String::new, |t| format!(" ({t})")),
            ));
        }
        Ok(index)
    }

    /// Counts the frame [`video`](Self::video) accepted.
    pub(crate) fn add_video(&mut self) {
        self.frames += 1;
    }

    /// Checks that `chunk` continues the audio stream; returns the index of its first sample.
    pub(crate) fn audio(&self, chunk: &AudioChunk) -> Result<i64, MediaError> {
        let Some(spec) = &self.audio else {
            return invalid("this output has no audio stream".to_owned());
        };
        chunk.validate()?;
        if (chunk.rate, chunk.channels) != (spec.rate, spec.channels) {
            return invalid(format!(
                "chunk is {} x{}, the stream takes {} x{}",
                chunk.rate, chunk.channels, spec.rate, spec.channels
            ));
        }
        let index = i64::try_from(self.samples).unwrap_or(i64::MAX);
        if spec.rate.samples_to_time(index).ok() != Some(chunk.pts) {
            return invalid(format!(
                "chunk at {} does not continue the stream at sample {index}",
                chunk.pts
            ));
        }
        Ok(index)
    }

    /// Counts the samples of a chunk [`audio`](Self::audio) accepted.
    pub(crate) fn add_audio(&mut self, samples: usize) {
        self.samples += samples as u64;
    }

    /// End of the accepted video (the start of the next frame), or zero.
    pub(crate) fn video_end(&self) -> Duration {
        let end = self.video.as_ref().and_then(|v| {
            i64::try_from(self.frames)
                .ok()
                .and_then(|n| v.rate.frame_to_time(n).ok())
        });
        to_duration(end)
    }

    /// End of the accepted audio, or zero.
    pub(crate) fn audio_end(&self) -> Duration {
        let end = self.audio.as_ref().and_then(|a| {
            i64::try_from(self.samples)
                .ok()
                .and_then(|n| a.rate.samples_to_time(n).ok())
        });
        to_duration(end)
    }

    /// What was accepted: counts and the end of the longer stream.
    pub(crate) fn summary(&self) -> EncodeSummary {
        EncodeSummary {
            video_frames: self.frames,
            audio_samples: self.samples,
            duration: self.video_end().max(self.audio_end()),
        }
    }

    /// A progress report with the accepted counts.
    pub(crate) fn progress(&self, encoded: Duration, bytes: u64) -> EncodeProgress {
        EncodeProgress {
            video_frames: self.frames,
            audio_samples: self.samples,
            encoded,
            bytes,
        }
    }
}

fn to_duration(t: Option<Time>) -> Duration {
    t.and_then(|t| Duration::try_from(t).ok())
        .unwrap_or(Duration::ZERO)
}

/// Encodes `spec` on `backend` and finishes the output, with `produce` pushing the input; when a
/// hardware encoder fails mid-stream, encodes everything again in software (WP-1E.02).
///
/// `produce` pushes every frame and chunk into the encoder it is given, propagating push errors,
/// and returns; this function then calls [`Encoder::finish`]. Under [`HwEncode::Auto`], a
/// [`MediaError::HwUnavailable`] from opening, `produce` or `finish` (a hardware encoder that
/// failed after it opened; its partial output is already removed) is logged, and the whole encode
/// runs once more with [`HwEncode::Off`]: `produce` is called again and must push the same input
/// from the start (progress reports restart at zero, and
/// [`Encoder::video_encoder`] tells the attempts apart). Every other error, a cancel, and any
/// failure under `Off` or `Force` are returned as they are.
///
/// # Errors
///
/// What opening, `produce` or `finish` returned on the last attempt.
pub fn encode_with_fallback(
    backend: &dyn MediaBackend,
    spec: &EncodeSpec,
    mut produce: impl FnMut(&mut dyn Encoder) -> Result<(), MediaError>,
) -> Result<EncodeSummary, MediaError> {
    let attempt =
        |spec: &EncodeSpec, produce: &mut dyn FnMut(&mut dyn Encoder) -> Result<(), MediaError>| {
            let mut enc = backend.open_encoder(spec)?;
            // On an error the encoder is dropped here, which removes its partial output.
            produce(enc.as_mut())?;
            enc.finish()
        };
    match attempt(spec, &mut produce) {
        Err(MediaError::HwUnavailable { reason }) if spec.hw == HwEncode::Auto => {
            tracing::warn!(
                target: "cutweave_media",
                "hardware video encoding of `{}` failed ({reason}); encoding it again from the \
                 start in software",
                crate::path::display(&spec.path)
            );
            attempt(&spec.clone().with_hw(HwEncode::Off), &mut produce)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use cutweave_model::{FrameRate, SampleRate};

    use super::*;
    use crate::Plane;

    fn spec() -> EncodeSpec {
        EncodeSpec::new("out.mp4")
            .with_video(VideoEncodeSpec::h264(
                4,
                2,
                FrameRate::FPS_25,
                PixelFormat::Nv12,
            ))
            .with_audio(AudioEncodeSpec::aac(SampleRate::HZ_48_000, 1))
    }

    fn frame(n: i64) -> VideoFrame {
        VideoFrame {
            pts: FrameRate::FPS_25.frame_to_time(n).unwrap(),
            width: 4,
            height: 2,
            format: PixelFormat::Nv12,
            planes: vec![
                Plane {
                    data: vec![0; 8],
                    stride: 4,
                },
                Plane {
                    data: vec![0; 4],
                    stride: 4,
                },
            ],
            color: ColorTags::default(),
            damaged: false,
        }
    }

    fn chunk(first: i64, n: usize) -> AudioChunk {
        AudioChunk {
            pts: SampleRate::HZ_48_000.samples_to_time(first).unwrap(),
            rate: SampleRate::HZ_48_000,
            channels: 1,
            planes: vec![vec![0.0; n]],
        }
    }

    #[test]
    fn frames_follow_the_grid_from_zero() {
        let mut check = InputCheck::new(&spec());
        assert!(check.video(&frame(1)).is_err(), "must start at 0");
        assert_eq!(check.video(&frame(0)), Ok(0));
        check.add_video();
        assert!(check.video(&frame(0)).is_err(), "repeat");
        assert!(check.video(&frame(2)).is_err(), "gap");
        let mut off_grid = frame(1);
        off_grid.pts = Time::from_flicks(off_grid.pts.flicks() + 1).unwrap();
        assert!(check.video(&off_grid).is_err(), "off the grid");
        assert_eq!(check.video(&frame(1)), Ok(1));
        check.add_video();
        assert_eq!(
            check.video_end(),
            Duration::from_flicks(2 * 28_224_000).unwrap()
        );
    }

    #[test]
    fn yuv_frames_must_not_contradict_the_output_color() {
        let check = InputCheck::new(&spec());
        let mut f = frame(0);
        f.color.matrix = Some(MatrixCoefficients::Bt709);
        f.color.range = Some(ColorRange::Limited);
        assert_eq!(check.video(&f), Ok(0));
        f.color.matrix = Some(MatrixCoefficients::Smpte170M);
        assert!(check.video(&f).is_err());
        f.color.matrix = None;
        f.color.range = Some(ColorRange::Full);
        assert!(check.video(&f).is_err());
    }

    #[test]
    fn audio_continues_without_gaps() {
        let mut check = InputCheck::new(&spec());
        assert!(check.audio(&chunk(1, 10)).is_err());
        assert_eq!(check.audio(&chunk(0, 10)), Ok(0));
        check.add_audio(10);
        assert!(check.audio(&chunk(0, 10)).is_err());
        assert_eq!(check.audio(&chunk(10, 0)), Ok(10));
        let mut stereo = chunk(10, 4);
        stereo.channels = 2;
        stereo.planes.push(vec![0.0; 4]);
        assert!(check.audio(&stereo).is_err());
        let summary = check.summary();
        assert_eq!((summary.video_frames, summary.audio_samples), (0, 10));
    }

    #[test]
    fn missing_streams_reject_their_input() {
        let video_only = EncodeSpec {
            audio: None,
            ..spec()
        };
        assert!(InputCheck::new(&video_only).audio(&chunk(0, 1)).is_err());
        let audio_only = EncodeSpec {
            video: None,
            ..spec()
        };
        assert!(InputCheck::new(&audio_only).video(&frame(0)).is_err());
    }

    #[test]
    fn hooks_compare_by_identity_and_report() {
        let flag = Arc::new(AtomicBool::new(false));
        let cancel = CancelCheck::from_flag(Arc::clone(&flag));
        assert_eq!(cancel, cancel.clone());
        assert_ne!(cancel, CancelCheck::from_flag(Arc::clone(&flag)));
        assert!(!cancel.is_cancelled());
        flag.store(true, Ordering::Relaxed);
        assert!(cancel.is_cancelled());
        assert_eq!(format!("{cancel:?}"), "CancelCheck(..)");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = {
            let seen = Arc::clone(&seen);
            ProgressSink::new(move |p| seen.lock().unwrap().push(*p))
        };
        sink.report(&EncodeProgress {
            video_frames: 3,
            ..EncodeProgress::default()
        });
        assert_eq!(seen.lock().unwrap()[0].video_frames, 3);
        assert_eq!(sink, sink.clone());
    }

    /// Where [`Flaky`]'s non-`Off` encoders fail with `HwUnavailable`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FailAt {
        Open,
        Frame(u64),
        Finish,
        /// A different error at this frame (no fallback expected).
        Cancel(u64),
    }

    /// A fake backend whose "hardware" (any `hw` but `Off`) fails at `fail_at`; records the `hw`
    /// of every open.
    struct Flaky {
        inner: crate::FakeBackend,
        fail_at: FailAt,
        opened: Mutex<Vec<HwEncode>>,
    }

    struct FlakyEncoder {
        inner: Box<dyn Encoder>,
        fail_at: FailAt,
        frames: u64,
    }

    fn hw_failed() -> MediaError {
        MediaError::HwUnavailable {
            reason: "test_hw: device lost (simulated)".to_owned(),
        }
    }

    impl Encoder for FlakyEncoder {
        fn push_video(&mut self, frame: &VideoFrame) -> Result<(), MediaError> {
            match self.fail_at {
                FailAt::Frame(n) if n == self.frames => return Err(hw_failed()),
                FailAt::Cancel(n) if n == self.frames => return Err(MediaError::Cancelled),
                _ => {}
            }
            self.frames += 1;
            self.inner.push_video(frame)
        }
        fn push_audio(&mut self, chunk: &AudioChunk) -> Result<(), MediaError> {
            self.inner.push_audio(chunk)
        }
        fn finish(self: Box<Self>) -> Result<EncodeSummary, MediaError> {
            if self.fail_at == FailAt::Finish {
                return Err(hw_failed());
            }
            self.inner.finish()
        }
        fn video_encoder(&self) -> Option<&str> {
            Some("test_hw")
        }
    }

    impl MediaBackend for Flaky {
        fn probe(&self, path: &std::path::Path) -> Result<cutweave_model::ProbeInfo, MediaError> {
            self.inner.probe(path)
        }
        fn index_media(&self, path: &std::path::Path) -> Result<crate::FrameIndex, MediaError> {
            self.inner.index_media(path)
        }
        fn open_decoder(
            &self,
            path: &std::path::Path,
            opts: &crate::DecodeOpts,
        ) -> Result<Box<dyn crate::VideoDecoder>, MediaError> {
            self.inner.open_decoder(path, opts)
        }
        fn open_audio_decoder(
            &self,
            path: &std::path::Path,
            stream: Option<u32>,
        ) -> Result<Box<dyn crate::AudioDecoder>, MediaError> {
            self.inner.open_audio_decoder(path, stream)
        }
        fn open_encoder(&self, spec: &EncodeSpec) -> Result<Box<dyn Encoder>, MediaError> {
            self.opened.lock().unwrap().push(spec.hw.clone());
            if spec.hw == HwEncode::Off {
                return self.inner.open_encoder(spec);
            }
            if self.fail_at == FailAt::Open {
                return Err(hw_failed());
            }
            Ok(Box::new(FlakyEncoder {
                inner: self
                    .inner
                    .open_encoder(&spec.clone().with_hw(HwEncode::Off))?,
                fail_at: self.fail_at,
                frames: 0,
            }))
        }
        fn capabilities(&self) -> crate::Capabilities {
            self.inner.capabilities()
        }
    }

    const FRAMES: i64 = 6;

    /// Runs `encode_with_fallback` on a [`Flaky`] backend with `hw`, pushing six frames; returns
    /// the result, the `hw` of every open, and the encoder name each `produce` call saw.
    fn fallback_run(
        fail_at: FailAt,
        hw: HwEncode,
    ) -> (
        Result<EncodeSummary, MediaError>,
        Vec<HwEncode>,
        Vec<String>,
    ) {
        let backend = Flaky {
            inner: crate::FakeBackend::new(),
            fail_at,
            opened: Mutex::new(Vec::new()),
        };
        let spec = EncodeSpec::new("out.mp4")
            .with_video(VideoEncodeSpec {
                codec: crate::testing::FAKE_VIDEO_CODEC.to_owned(),
                ..VideoEncodeSpec::h264(4, 2, FrameRate::FPS_25, PixelFormat::Nv12)
            })
            .with_hw(hw);
        let mut seen = Vec::new();
        let result = encode_with_fallback(&backend, &spec, |enc| {
            seen.push(enc.video_encoder().unwrap_or("software").to_owned());
            for n in 0..FRAMES {
                enc.push_video(&frame(n))?;
            }
            Ok(())
        });
        (result, backend.opened.into_inner().unwrap(), seen)
    }

    #[test]
    fn a_mid_stream_hardware_failure_re_encodes_everything_in_software() {
        for fail_at in [FailAt::Frame(3), FailAt::Finish, FailAt::Open] {
            let (result, opened, seen) = fallback_run(fail_at, HwEncode::Auto);
            assert_eq!(
                result.map(|s| s.video_frames),
                Ok(6),
                "{fail_at:?}: the software attempt pushes all frames from the start"
            );
            assert_eq!(opened, [HwEncode::Auto, HwEncode::Off], "{fail_at:?}");
            let want: &[&str] = if fail_at == FailAt::Open {
                &["software"]
            } else {
                &["test_hw", "software"]
            };
            assert_eq!(seen, want, "{fail_at:?}");
        }
    }

    #[test]
    fn only_auto_falls_back_and_only_for_hardware_failures() {
        // A forced encoder or `Off` returns the failure as it is.
        let forced = HwEncode::Force("test_hw".into());
        let (result, opened, _) = fallback_run(FailAt::Frame(2), forced.clone());
        assert_eq!(result, Err(hw_failed()));
        assert_eq!(opened, [forced]);
        // A cancel (or any other error) is not retried.
        let (result, opened, seen) = fallback_run(FailAt::Cancel(2), HwEncode::Auto);
        assert_eq!(result, Err(MediaError::Cancelled));
        assert_eq!((opened.len(), seen.len()), (1, 1));
        // Success needs one attempt.
        let (result, opened, _) = fallback_run(FailAt::Frame(99), HwEncode::Auto);
        assert_eq!(result.map(|s| s.video_frames), Ok(6));
        assert_eq!(opened, [HwEncode::Auto]);
    }

    #[test]
    fn hw_policy_defaults_to_auto_and_a_forced_name_must_not_be_empty() {
        assert_eq!(HwEncode::default(), HwEncode::Auto);
        assert_eq!(EncodeSpec::new("a.mp4").hw, HwEncode::Auto);
        assert_eq!(
            spec().with_hw(HwEncode::Off).hw,
            HwEncode::Off,
            "with_hw sets it"
        );
        assert!(
            spec()
                .with_hw(HwEncode::Force("h264_nvenc".into()))
                .validate()
                .is_ok()
        );
        for empty in ["", "  "] {
            assert!(matches!(
                spec().with_hw(HwEncode::Force(empty.into())).validate(),
                Err(MediaError::Invalid { .. })
            ));
        }
        // Without a video stream the policy is irrelevant.
        let audio_only = EncodeSpec {
            video: None,
            ..spec().with_hw(HwEncode::Force(String::new()))
        };
        assert!(audio_only.validate().is_ok());
    }

    #[test]
    fn the_fake_backend_has_no_hardware_encoder() {
        let fake = crate::FakeBackend::new();
        assert_eq!(fake.capabilities().hw_encode, Vec::new());
        let spec = EncodeSpec::new("out.mp4").with_video(VideoEncodeSpec {
            codec: crate::testing::FAKE_VIDEO_CODEC.to_owned(),
            ..VideoEncodeSpec::h264(4, 2, FrameRate::FPS_25, PixelFormat::Nv12)
        });
        assert!(fake.open_encoder(&spec).is_ok(), "Auto encodes in software");
        assert!(matches!(
            fake.open_encoder(&spec.with_hw(HwEncode::Force("h264_nvenc".into())))
                .err(),
            Some(MediaError::HwUnavailable { .. })
        ));
    }

    #[test]
    fn defaults_are_crf_20_medium_and_bt709_limited() {
        assert_eq!(RateControl::default(), RateControl::Crf(20));
        assert_eq!(SpeedPreset::default().name(), "medium");
        let tags = output_color(&ColorTags::default());
        assert_eq!(tags, bt709_tags());
        let full = output_color(&ColorTags {
            range: Some(ColorRange::Full),
            ..ColorTags::default()
        });
        assert_eq!(full.range, Some(ColorRange::Full));
        assert_eq!(full.matrix, Some(MatrixCoefficients::Bt709));
    }
}
