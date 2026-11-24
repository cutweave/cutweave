// SPDX-License-Identifier: GPL-3.0-or-later
//! The export pipeline (WP-1E.03; arch §7, §8, §9): render every output frame of an
//! [`ExportPlan`], mix the range down, and encode both into the output file.
//!
//! ```text
//! render thread:  for frame i: evaluate(snapshot, t_i) → render graph (decodes) → GPU (lock)
//!                 → RGBA8 readback ──▶ bounded queue (PIPELINE_DEPTH frames)
//! job thread:     frame i ──▶ encoder.push_video(pts = i / fps)
//!                 mixdown samples up to the end of frame i ──▶ encoder.push_audio
//!                 … encoder.finish() (flush, faststart) → the file appears
//! ```
//!
//! - **Pictures** are rendered exactly like `render.frame` at full quality: the same
//!   [`evaluate`] → [`Quality::EXPORT`] → [`build`] for an sRGB RGBA8 target → the backend's
//!   `rasterize`, with the backend's graph options. On the same adapter the frame handed to the
//!   encoder is bit-identical to `render.frame` at that time (the "preview matches export"
//!   guarantee). The [`SharedBackend`] lock is the GPU budget shared with preview and agent
//!   frames: it is held for one frame at a time.
//! - **Decode-ahead.** Decoding and rendering run on their own thread, up to
//!   [`PIPELINE_DEPTH`] frames ahead of the encoder; clips decode sequentially
//!   ([`DecoderFrameSource`](super::DecoderFrameSource)).
//! - **Sound** is the offline mixdown ([`mixdown`], the real-time mixer's lane and bus code) of
//!   the exported range at [`EXPORT_SAMPLE_RATE`] stereo, gapless from sample 0, pushed in step
//!   with the pictures (up to the end of each frame).
//! - **Timestamps.** Frame `i` shows sequence time `start + i / fps` and gets pts `i / fps` in
//!   the file (output time from 0); flicks become codec time bases only inside the encoder.
//! - **Cancellation** is polled per frame by both threads and, through the encoder's
//!   [`CancelCheck`], on every push, muxed packet and 32 KiB written (so `finish` stops too).
//!   A cancelled or failed export removes its partial file and leaves an existing file alone.
//! - **Determinism.** Output depends only on the snapshot, the plan, the adapter and the
//!   encoder settings; with one encoder thread ([`ExportServices::with_encoder_threads`]) two
//!   exports of the same snapshot decode to identical frames. The only clock here measures the
//!   speed shown in progress messages.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use cutweave_audio::{Mixdown, mixdown};
use cutweave_jobs::{CancelToken, JobError};
use cutweave_media::{
    AudioChunk, AudioEncodeSpec, CancelCheck, EncodeSpec, EncodeSummary, Encoder, MediaBackend,
    MediaError, PixelFormat, Plane, ProgressSink, RateControl, SpeedPreset, VideoEncodeSpec,
    VideoFrame,
};
use cutweave_model::{ColorTags, Project, Rounding, SampleRate, SequenceId, Time};
use cutweave_ops::export::{ExportPlan, ExportQuality};
use cutweave_render::graph::{GraphOptions, build};
use cutweave_render::query::SharedBackend;
use cutweave_render::{FrameSource, Image, Quality, Target, evaluate, evaluate_audio};

use super::sources::ExportSources;

/// Frames rendered ahead of the encoder.
pub const PIPELINE_DEPTH: usize = 3;
/// The sample rate of exported audio (AAC, stereo).
pub const EXPORT_SAMPLE_RATE: SampleRate = SampleRate::HZ_48_000;
/// Channels of exported audio.
pub const EXPORT_CHANNELS: u16 = 2;

/// The x264 CRF each quality maps to (`high` is the encoder default, the reference point of
/// the hardware-encoder quality table of WP-1E.02).
#[must_use]
pub const fn crf(quality: ExportQuality) -> u8 {
    match quality {
        ExportQuality::Low => 28,
        ExportQuality::Medium => 23,
        ExportQuality::High => RateControl::DEFAULT_CRF,
    }
}

/// Sees every frame the pipeline hands to the encoder: its index, its sequence time and its
/// pixels (tests compare them with `render.frame`).
pub type FrameTap = Arc<dyn Fn(u32, Time, &Image) + Send + Sync>;

/// What exports render, decode and encode with: the media, render and audio subsystems'
/// share of the export (WP-1I.05 provides it through
/// [`Subsystems::export_services`](crate::Subsystems::export_services)).
#[derive(Clone)]
pub struct ExportServices {
    /// Opens the output encoder (H.264 + AAC in MP4).
    pub encoder: Arc<dyn MediaBackend>,
    /// Clip pictures and sound.
    pub sources: Arc<dyn ExportSources>,
    /// The renderer shared with `render.frame` and preview; its lock is the GPU budget.
    pub renderer: SharedBackend,
    /// Encoder threads (0: the encoder decides). One thread makes x264 output identical run to
    /// run (the determinism test).
    pub encoder_threads: u16,
    /// Encoder speed against compression (default x264 `medium`).
    pub speed: SpeedPreset,
    /// Sees every frame before it is encoded (tests).
    pub frame_tap: Option<FrameTap>,
}

impl std::fmt::Debug for ExportServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportServices")
            .field("encoder_threads", &self.encoder_threads)
            .field("speed", &self.speed)
            .field("frame_tap", &self.frame_tap.is_some())
            .finish_non_exhaustive()
    }
}

impl ExportServices {
    /// Services with automatic encoder threads, the `medium` preset and no tap.
    #[must_use]
    pub fn new(
        encoder: Arc<dyn MediaBackend>,
        sources: Arc<dyn ExportSources>,
        renderer: SharedBackend,
    ) -> ExportServices {
        ExportServices {
            encoder,
            sources,
            renderer,
            encoder_threads: 0,
            speed: SpeedPreset::default(),
            frame_tap: None,
        }
    }

    /// The same services with `threads` encoder threads.
    #[must_use]
    pub fn with_encoder_threads(mut self, threads: u16) -> ExportServices {
        self.encoder_threads = threads;
        self
    }

    /// The same services with another encoder speed.
    #[must_use]
    pub fn with_speed(mut self, speed: SpeedPreset) -> ExportServices {
        self.speed = speed;
        self
    }

    /// The same services, showing every frame to `tap` before it is encoded.
    #[must_use]
    pub fn with_frame_tap(mut self, tap: FrameTap) -> ExportServices {
        self.frame_tap = Some(tap);
        self
    }
}

/// Why an export stopped.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExportError {
    /// It was cancelled; the partial file is gone.
    #[error("cancelled")]
    Cancelled,
    /// It failed; the partial file is gone.
    #[error("{0}")]
    Failed(String),
}

impl From<ExportError> for JobError {
    fn from(e: ExportError) -> JobError {
        match e {
            ExportError::Cancelled => JobError::Cancelled,
            ExportError::Failed(msg) => JobError::Failed(msg),
        }
    }
}

fn failed(msg: impl Into<String>) -> ExportError {
    ExportError::Failed(msg.into())
}

/// Encoder errors as export errors: `Cancelled` stays a cancellation.
fn media(context: &str, e: MediaError) -> ExportError {
    match e {
        MediaError::Cancelled => ExportError::Cancelled,
        other => failed(format!("{context}: {other}")),
    }
}

/// What an export wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportSummary {
    /// What the encoder wrote.
    pub encoded: EncodeSummary,
    /// Frames on which some layer's media was unavailable (drawn without it).
    pub frames_missing_media: u32,
    /// Audio clips whose source failed (silent in the mix).
    pub audio_source_errors: u64,
}

impl ExportSummary {
    /// The job's closing status line.
    #[must_use]
    pub fn message(&self) -> String {
        let mut msg = format!(
            "exported {} frames ({:.2} s)",
            self.encoded.video_frames,
            self.encoded.duration.as_secs_f64()
        );
        if self.frames_missing_media > 0 {
            let _ = write!(
                msg,
                "; {} frames were drawn without unavailable media",
                self.frames_missing_media
            );
        }
        if self.audio_source_errors > 0 {
            let _ = write!(
                msg,
                "; {} audio clips could not be read and are silent",
                self.audio_source_errors
            );
        }
        msg
    }
}

/// Renders the frame of `seq` at `t` as `render.frame` does at full quality: `(pixels,
/// layers whose media was unavailable)`.
pub fn render_frame(
    doc: &Project,
    seq: SequenceId,
    t: Time,
    size: (u32, u32),
    backend: &SharedBackend,
    source: &dyn FrameSource,
    options: &GraphOptions,
) -> Result<(Image, usize), ExportError> {
    let mut scene =
        evaluate(doc, seq, t).map_err(|e| failed(format!("cannot evaluate {t}: {e}")))?;
    Quality::EXPORT.apply(&mut scene);
    let graph = build(&scene, Target::rgba8(size.0, size.1), source, options)
        .map_err(|e| failed(format!("cannot compile the frame at {t}: {e}")))?;
    let image = backend
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .rasterize(&scene, &graph)
        .map_err(|e| failed(format!("rendering the frame at {t} failed: {e}")))?;
    if image.width != size.0 || image.height != size.1 || !image.is_valid() {
        return Err(failed(format!(
            "the renderer returned a {}x{} image for a {}x{} export",
            image.width, image.height, size.0, size.1
        )));
    }
    Ok((image, graph.missing.len()))
}

/// One rendered frame on its way to the encoder.
struct Picture {
    index: u32,
    time: Time,
    image: Image,
    missing: usize,
}

/// What the render thread works with.
struct RenderInputs<'a> {
    plan: &'a ExportPlan,
    backend: &'a SharedBackend,
    source: &'a (dyn FrameSource + Send + Sync),
    options: GraphOptions,
    cancel: &'a CancelToken,
}

impl RenderInputs<'_> {
    /// Renders every frame in order into `out` until done, cancelled, failed or the encoder
    /// side hangs up.
    fn run(&self, out: &SyncSender<Result<Picture, ExportError>>) {
        let plan = self.plan;
        for index in 0..plan.frames {
            if self.cancel.is_cancelled() {
                return;
            }
            let picture = plan
                .time_of(index)
                .map_err(|e| failed(e.message()))
                .and_then(|time| {
                    render_frame(
                        &plan.snapshot.doc,
                        plan.sequence,
                        time,
                        (plan.width, plan.height),
                        self.backend,
                        self.source,
                        &self.options,
                    )
                    .map(|(image, missing)| Picture {
                        index,
                        time,
                        image,
                        missing,
                    })
                });
            let stop = picture.is_err();
            if out.send(picture).is_err() || stop {
                return;
            }
        }
    }
}

/// Feeds the mixdown to the encoder in step with the pictures.
struct AudioFeed {
    mix: Mixdown,
    /// The current mixdown chunk (interleaved stereo) and how many of its frames were pushed.
    chunk: Vec<f32>,
    head: usize,
    /// Samples pushed so far (output sample index).
    pushed: i64,
}

impl AudioFeed {
    /// The mixdown of `plan`'s range: exactly the pictures' length from the range start.
    fn new(plan: &ExportPlan, services: &ExportServices) -> Result<AudioFeed, ExportError> {
        let doc = &plan.snapshot.doc;
        let end = plan
            .start
            .checked_add(plan.duration)
            .map_err(|e| failed(format!("the export range: {e}")))?;
        let segments = evaluate_audio(doc, plan.sequence, plan.start, end)
            .map_err(|e| failed(format!("cannot evaluate the audio: {e}")))?;
        let mix = mixdown(
            &services.sources.audio(doc),
            &segments,
            plan.start..end,
            EXPORT_SAMPLE_RATE,
        )
        .map_err(|e| failed(format!("cannot mix the audio down: {e}")))?;
        Ok(AudioFeed {
            mix,
            chunk: Vec::new(),
            head: 0,
            pushed: 0,
        })
    }

    /// Pushes samples until `target` (exclusive) or the end of the mix.
    fn push_until(&mut self, target: i64, encoder: &mut dyn Encoder) -> Result<(), ExportError> {
        let channels = usize::from(EXPORT_CHANNELS);
        while self.pushed < target {
            let left = self.chunk.len() / channels - self.head;
            if left == 0 {
                let Some(next) = self.mix.next() else {
                    return Ok(());
                };
                self.chunk = next.samples;
                self.head = 0;
                continue;
            }
            let take = usize::try_from(target - self.pushed).map_or(left, |n| n.min(left));
            let span = &self.chunk[self.head * channels..(self.head + take) * channels];
            let mut planes = vec![Vec::with_capacity(take); channels];
            for frame in span.chunks_exact(channels) {
                for (plane, &s) in planes.iter_mut().zip(frame) {
                    plane.push(s);
                }
            }
            let pts = EXPORT_SAMPLE_RATE
                .samples_to_time(self.pushed)
                .map_err(|e| failed(format!("audio position: {e}")))?;
            encoder
                .push_audio(&AudioChunk {
                    pts,
                    rate: EXPORT_SAMPLE_RATE,
                    channels: EXPORT_CHANNELS,
                    planes,
                })
                .map_err(|e| media("encoding audio failed", e))?;
            self.head += take;
            self.pushed += i64::try_from(take).unwrap_or(i64::MAX);
        }
        Ok(())
    }
}

/// The encode spec of `plan`: H.264 from RGBA8 frames + AAC, cancelled by `cancel`, reporting
/// the encoded length into `written` (flicks).
fn encode_spec(
    plan: &ExportPlan,
    services: &ExportServices,
    cancel: &CancelToken,
    written: &Arc<AtomicI64>,
) -> EncodeSpec {
    let mut video =
        VideoEncodeSpec::h264(plan.width, plan.height, plan.frame_rate, PixelFormat::Rgba8);
    video.rate_control = RateControl::Crf(crf(plan.quality));
    video.preset = services.speed;
    video.threads = services.encoder_threads;
    let token = cancel.clone();
    let sink = Arc::clone(written);
    EncodeSpec::new(&plan.path)
        .with_video(video)
        .with_audio(AudioEncodeSpec::aac(EXPORT_SAMPLE_RATE, EXPORT_CHANNELS))
        .with_cancel(CancelCheck::new(move || token.is_cancelled()))
        .with_progress(ProgressSink::new(move |p| {
            sink.store(p.encoded.flicks(), Ordering::Relaxed);
        }))
}

/// `m:ss` for status lines.
fn clock(secs: f64) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The progress fraction and status line while frames are pushed.
struct Progress<'a> {
    report: &'a dyn Fn(f64, &str),
    started: Instant,
    total: u32,
    duration: f64,
    written: Arc<AtomicI64>,
    last: f64,
}

impl Progress<'_> {
    /// Reports after frame `pushed` when the encoded fraction moved by at least 1 %.
    #[allow(clippy::cast_precision_loss)]
    fn frame(&mut self, pushed: u32) {
        let done = self.written.load(Ordering::Relaxed) as f64 / self.duration;
        let fraction = done.clamp(0.0, 0.99);
        if fraction - self.last < 0.01 && pushed != self.total {
            return;
        }
        self.last = fraction;
        let elapsed = self.started.elapsed().as_secs_f64();
        let fps = if elapsed > 0.0 {
            f64::from(pushed) / elapsed
        } else {
            0.0
        };
        let eta = if fps > 0.0 {
            f64::from(self.total - pushed) / fps
        } else {
            0.0
        };
        let msg = format!(
            "frame {pushed} of {}, {fps:.1} fps, about {} left",
            self.total,
            clock(eta)
        );
        (self.report)(fraction, &msg);
    }
}

/// The encoder side of the pipeline.
struct EncodeSide<'a> {
    plan: &'a ExportPlan,
    services: &'a ExportServices,
    cancel: &'a CancelToken,
    audio: AudioFeed,
    progress: Progress<'a>,
}

impl EncodeSide<'_> {
    /// Takes the pictures in order and pushes each with the sound up to its end. Returns the
    /// number of frames drawn with unavailable media.
    fn run(
        &mut self,
        pictures: &Receiver<Result<Picture, ExportError>>,
        sink: &mut dyn Encoder,
    ) -> Result<u32, ExportError> {
        let plan = self.plan;
        let at = |i: i64| {
            plan.frame_rate
                .frame_to_time(i)
                .map_err(|e| failed(format!("frame {i}: {e}")))
        };
        let mut missing = 0u32;
        for expected in 0..plan.frames {
            let Ok(picture) = pictures.recv() else {
                return Err(if self.cancel.is_cancelled() {
                    ExportError::Cancelled
                } else {
                    failed("the render thread stopped")
                });
            };
            let picture = picture?;
            if picture.index != expected {
                return Err(failed("frames arrived out of order"));
            }
            if self.cancel.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            if let Some(tap) = &self.services.frame_tap {
                tap(picture.index, picture.time, &picture.image);
            }
            if picture.missing > 0 {
                missing += 1;
            }
            let Image {
                width,
                height,
                data,
            } = picture.image;
            sink.push_video(&VideoFrame {
                pts: at(i64::from(expected))?,
                width,
                height,
                format: PixelFormat::Rgba8,
                planes: vec![Plane {
                    data,
                    stride: width * 4,
                }],
                color: ColorTags::default(),
            })
            .map_err(|e| media("encoding video failed", e))?;
            let frame_end = at(i64::from(expected) + 1)?;
            self.audio.push_until(
                EXPORT_SAMPLE_RATE.time_to_samples(frame_end, Rounding::Nearest),
                sink,
            )?;
            self.progress.frame(expected + 1);
        }
        Ok(missing)
    }
}

/// Runs the export of `plan` (see the module docs): renders, mixes and encodes it into
/// `plan.path`, reporting `(fraction, status line)` to `report`, until done or `cancel`.
pub fn run_export(
    plan: &ExportPlan,
    services: &ExportServices,
    cancel: &CancelToken,
    report: &dyn Fn(f64, &str),
) -> Result<ExportSummary, ExportError> {
    let started = Instant::now();
    if cancel.is_cancelled() {
        return Err(ExportError::Cancelled);
    }
    if !plan.overwrite && plan.path.exists() {
        return Err(failed(format!(
            "`{}` was created after the export was requested; not replacing it (export again \
             with overwrite: true)",
            plan.path.display()
        )));
    }
    let source = services.sources.video(&plan.snapshot.doc);
    let inputs = RenderInputs {
        plan,
        backend: &services.renderer,
        source: &*source,
        options: services
            .renderer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .graph_options(),
        cancel,
    };
    let written = Arc::new(AtomicI64::new(0));
    #[allow(clippy::cast_precision_loss)]
    let mut side = EncodeSide {
        plan,
        services,
        cancel,
        audio: AudioFeed::new(plan, services)?,
        progress: Progress {
            report,
            started,
            total: plan.frames,
            duration: (plan.duration.flicks() as f64).max(1.0),
            written: Arc::clone(&written),
            last: -1.0,
        },
    };
    tracing::info!(
        path = %plan.path.display(),
        frames = plan.frames,
        width = plan.width,
        height = plan.height,
        fps = %plan.frame_rate,
        hw = ?plan.hw,
        "export started"
    );
    let mut sink = services
        .encoder
        .open_encoder(&encode_spec(plan, services, cancel, &written))
        .map_err(|e| media("cannot open the encoder", e))?;
    report(0.0, &format!("frame 0 of {}", plan.frames));
    let missing = std::thread::scope(|scope| {
        let (tx, rx) = sync_channel(PIPELINE_DEPTH);
        let inputs = &inputs;
        std::thread::Builder::new()
            .name("cutweave-export-render".into())
            .spawn_scoped(scope, move || inputs.run(&tx))
            .map_err(|e| failed(format!("cannot start the render thread: {e}")))?;
        // Returning drops `rx`, so a render thread blocked on a full queue stops too.
        side.run(&rx, &mut *sink)
    })?;
    side.audio.push_until(i64::MAX, &mut *sink)?;
    report(0.99, "finishing the file");
    let encoded = sink
        .finish()
        .map_err(|e| media("finishing the file failed", e))?;
    let summary = ExportSummary {
        encoded,
        frames_missing_media: missing,
        audio_source_errors: side.audio.mix.source_errors(),
    };
    let elapsed = started.elapsed().as_secs_f64();
    tracing::info!(
        path = %plan.path.display(),
        frames = summary.encoded.video_frames,
        seconds = elapsed,
        realtime = summary.encoded.duration.as_secs_f64() / elapsed.max(f64::EPSILON),
        "export finished"
    );
    Ok(summary)
}
