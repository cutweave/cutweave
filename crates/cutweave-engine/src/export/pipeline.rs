// SPDX-License-Identifier: GPL-3.0-or-later
//! The export pipeline (WP-1E.03; arch §7, §8, §9): render every output frame of an
//! [`ExportPlan`], mix the range down, and encode both into the output file.
//!
//! ```text
//! decode thread:  for frame i: evaluate(snapshot, t_i) → render graph (decodes its pictures)
//!                 ──▶ bounded queue (PIPELINE_DEPTH frames)
//! render thread:  GPU (lock) → RGBA8 readback ──▶ bounded queue (PIPELINE_DEPTH frames)
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
//! - **Decode-ahead.** Decoding and GPU rendering run on threads of their own, each up to
//!   [`PIPELINE_DEPTH`] frames ahead of the next stage, so an export runs at the speed of its
//!   slowest stage rather than their sum; clips decode sequentially
//!   ([`DecoderFrameSource`](super::DecoderFrameSource)).
//! - **Sound** is the offline mixdown ([`mixdown`], the real-time mixer's lane and bus code) of
//!   the exported range at [`EXPORT_SAMPLE_RATE`] stereo, gapless from sample 0, pushed in step
//!   with the pictures (up to the end of each frame).
//! - **Timestamps.** Frame `i` shows sequence time `start + i / fps` and gets pts `i / fps` in
//!   the file (output time from 0); flicks become codec time bases only inside the encoder.
//! - **Encoder** (WP-1E.02): the plan's `hw` becomes the encode spec's [`HwEncode`] (`auto`:
//!   the first hardware H.264 encoder that opens, else x264; `off`: x264; `force`: that encoder
//!   or fail), and its quality one [`RateControl::Crf`] value that `cutweave-media` maps onto
//!   each encoder's constant-quality scale ([`rate_control`]). The whole export runs inside
//!   [`encode_with_fallback`]: when a hardware encoder fails mid-stream under `auto`, its
//!   partial file is removed and the export **restarts from frame 0 in software** (fresh
//!   decoders, mixdown and progress). The job's closing message names the encoder used.
//! - **Cancellation** is polled per frame by both threads and, through the encoder's
//!   [`CancelCheck`], on every push, muxed packet and 32 KiB written (so `finish` stops too).
//!   A cancelled or failed export removes its partial file and leaves an existing file alone;
//!   a cancel is never retried.
//! - **Determinism.** Output depends only on the snapshot, the plan, the adapter and the
//!   encoder settings; in software (`hw: "off"`) with one encoder thread
//!   ([`ExportServices::with_encoder_threads`]) two exports of the same snapshot decode to
//!   identical frames. The only clock here measures the speed shown in progress messages.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use cutweave_audio::{Mixdown, mixdown};
use cutweave_jobs::{CancelToken, JobError};
use cutweave_media::{
    AudioChunk, AudioEncodeSpec, CancelCheck, EncodeSpec, EncodeSummary, Encoder, HwEncode,
    MediaBackend, MediaError, PixelFormat, Plane, ProgressSink, RateControl, SpeedPreset,
    VideoEncodeSpec, VideoFrame, encode_with_fallback,
};
use cutweave_model::{ColorTags, Project, Rounding, SampleRate, SequenceId, Time};
use cutweave_ops::export::presets::PlanRate;
use cutweave_ops::export::{ExportHw, ExportPlan, ExportQuality};
use cutweave_render::graph::{GraphOptions, RenderGraph, build};
use cutweave_render::query::SharedBackend;
use cutweave_render::{
    FrameSource, Image, Quality, SceneDescription, Target, evaluate, evaluate_audio,
};

use super::sources::ExportSources;

/// Frames rendered ahead of the encoder.
pub const PIPELINE_DEPTH: usize = 3;
/// The sample rate of exported audio (AAC, stereo).
pub const EXPORT_SAMPLE_RATE: SampleRate = SampleRate::HZ_48_000;
/// Channels of exported audio.
pub const EXPORT_CHANNELS: u16 = 2;

/// The rate control of each quality: one value on the x264 CRF scale (`high` is the encoder
/// default, 20). `cutweave-media` maps it onto every encoder's own constant-quality mode with
/// offsets measured to give x264's PSNR at the same CRF (NVENC `cq = c + 6`, AMF
/// `qp = c + 1`, …; docs/reference/export-quality.md), so a quality looks the same whichever
/// encoder `hw` picks.
#[must_use]
pub const fn rate_control(quality: ExportQuality) -> RateControl {
    RateControl::Crf(match quality {
        ExportQuality::Low => 28,
        ExportQuality::Medium => 23,
        ExportQuality::High => RateControl::DEFAULT_CRF,
    })
}

/// The encoder policy of an export's `hw`.
#[must_use]
pub fn hw_encode(hw: &ExportHw) -> HwEncode {
    match hw {
        ExportHw::Auto => HwEncode::Auto,
        ExportHw::Off => HwEncode::Off,
        ExportHw::Force(name) => HwEncode::Force(name.clone()),
    }
}

/// Sees every frame the pipeline hands to the encoder: its index, its sequence time and its
/// pixels (tests compare them with `render.frame`). After a hardware failure the frames come
/// again from index 0.
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
    /// Encoder speed against compression (default `medium`; hardware encoders map it to their
    /// own presets).
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

/// Why one encoding attempt stopped: an encoder error (which [`encode_with_fallback`] may
/// answer by retrying in software) or a failure of the pipeline itself.
enum Stop {
    Media(MediaError),
    Export(ExportError),
}

impl From<ExportError> for Stop {
    fn from(e: ExportError) -> Stop {
        Stop::Export(e)
    }
}

/// What an export wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportSummary {
    /// What the encoder wrote.
    pub encoded: EncodeSummary,
    /// The video encoder that wrote the file (`h264_nvenc`, `libx264`, …), when the backend
    /// names it.
    pub video_encoder: Option<String>,
    /// The hardware encoder that failed during the export, after which it was encoded again
    /// in software.
    pub failed_encoder: Option<String>,
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
        if let Some(encoder) = &self.video_encoder {
            let _ = write!(msg, " with {encoder}");
        }
        if let Some(broken) = &self.failed_encoder {
            let _ = write!(
                msg,
                " ({broken} failed during the export, so it was encoded again in software)"
            );
        }
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

/// The first half of rendering a frame, the part that decodes: the scene of `seq` at `t` at
/// export quality and its render graph for an sRGB RGBA8 target of `size` (frames resolved
/// through `source`).
pub fn prepare_frame(
    doc: &Project,
    seq: SequenceId,
    t: Time,
    size: (u32, u32),
    source: &dyn FrameSource,
    options: &GraphOptions,
) -> Result<(SceneDescription, RenderGraph), ExportError> {
    let mut scene =
        evaluate(doc, seq, t).map_err(|e| failed(format!("cannot evaluate {t}: {e}")))?;
    Quality::EXPORT.apply(&mut scene);
    let graph = build(&scene, Target::rgba8(size.0, size.1), source, options)
        .map_err(|e| failed(format!("cannot compile the frame at {t}: {e}")))?;
    Ok((scene, graph))
}

/// The second half: runs a prepared graph on the shared renderer (holding its lock for this
/// frame only) and reads the pixels back: `(pixels, layers whose media was unavailable)`.
pub fn rasterize_frame(
    backend: &SharedBackend,
    scene: &SceneDescription,
    graph: &RenderGraph,
) -> Result<(Image, usize), ExportError> {
    let t = scene.time;
    let size = graph.target.size();
    let image = backend
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .rasterize(scene, graph)
        .map_err(|e| failed(format!("rendering the frame at {t} failed: {e}")))?;
    if (image.width, image.height) != size || !image.is_valid() {
        return Err(failed(format!(
            "the renderer returned a {}x{} image for a {}x{} export",
            image.width, image.height, size.0, size.1
        )));
    }
    Ok((image, graph.missing.len()))
}

/// Renders the frame of `seq` at `t` as `render.frame` does at full quality
/// ([`prepare_frame`], then [`rasterize_frame`]).
pub fn render_frame(
    doc: &Project,
    seq: SequenceId,
    t: Time,
    size: (u32, u32),
    backend: &SharedBackend,
    source: &dyn FrameSource,
    options: &GraphOptions,
) -> Result<(Image, usize), ExportError> {
    let (scene, graph) = prepare_frame(doc, seq, t, size, source, options)?;
    rasterize_frame(backend, &scene, &graph)
}

/// A frame whose source pictures are decoded, on its way to the GPU.
struct Prepared {
    index: u32,
    scene: SceneDescription,
    graph: RenderGraph,
}

/// One rendered frame on its way to the encoder.
struct Picture {
    index: u32,
    time: Time,
    image: Image,
    missing: usize,
}

/// What the decode and render threads work with.
struct RenderInputs<'a> {
    plan: &'a ExportPlan,
    backend: &'a SharedBackend,
    source: &'a (dyn FrameSource + Send + Sync),
    options: GraphOptions,
    cancel: &'a CancelToken,
}

impl RenderInputs<'_> {
    /// The decode thread: evaluates and compiles every frame in order (which decodes its
    /// source pictures) into `out`, until done, cancelled, failed or the GPU side hangs up.
    fn prepare_all(&self, out: &SyncSender<Result<Prepared, ExportError>>) {
        let plan = self.plan;
        for index in 0..plan.frames {
            if self.cancel.is_cancelled() {
                return;
            }
            let prepared = plan
                .time_of(index)
                .map_err(|e| failed(e.message()))
                .and_then(|time| {
                    prepare_frame(
                        &plan.snapshot.doc,
                        plan.sequence,
                        time,
                        (plan.width, plan.height),
                        self.source,
                        &self.options,
                    )
                })
                .map(|(scene, graph)| Prepared {
                    index,
                    scene,
                    graph,
                });
            let stop = prepared.is_err();
            if out.send(prepared).is_err() || stop {
                return;
            }
        }
    }

    /// The render thread: runs each prepared frame on the GPU and passes its pixels on to
    /// `out`, until the decode side ends, the export is cancelled or the encoder hangs up.
    fn rasterize_all(
        &self,
        input: &Receiver<Result<Prepared, ExportError>>,
        out: &SyncSender<Result<Picture, ExportError>>,
    ) {
        while let Ok(prepared) = input.recv() {
            if self.cancel.is_cancelled() {
                return;
            }
            let picture = prepared.and_then(|p| {
                rasterize_frame(self.backend, &p.scene, &p.graph).map(|(image, missing)| Picture {
                    index: p.index,
                    time: p.scene.time,
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
    fn push_until(&mut self, target: i64, encoder: &mut dyn Encoder) -> Result<(), Stop> {
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
                .map_err(Stop::Media)?;
            self.head += take;
            self.pushed += i64::try_from(take).unwrap_or(i64::MAX);
        }
        Ok(())
    }
}

/// The encode spec of `plan`: H.264 from RGBA8 frames + AAC with the plan's encoder policy and
/// quality, cancelled by `cancel`, reporting the encoded length into `written` (flicks).
fn encode_spec(
    plan: &ExportPlan,
    services: &ExportServices,
    cancel: &CancelToken,
    written: &Arc<AtomicI64>,
) -> EncodeSpec {
    let mut video =
        VideoEncodeSpec::h264(plan.width, plan.height, plan.frame_rate, PixelFormat::Rgba8);
    video.rate_control = match plan.rate {
        // A preset's own rate control; an override's quality leaves `rate` empty.
        Some(PlanRate::Crf(crf)) => RateControl::Crf(crf.min(RateControl::MAX_CRF)),
        Some(PlanRate::BitrateKbps(kbps)) => RateControl::Bitrate(kbps.saturating_mul(1000)),
        None => rate_control(plan.quality),
    };
    video.preset = services.speed;
    video.threads = services.encoder_threads;
    let token = cancel.clone();
    let sink = Arc::clone(written);
    EncodeSpec::new(&plan.path)
        .with_hw(hw_encode(&plan.hw))
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

/// The progress fraction and status line of one attempt while frames are pushed.
struct Progress<'a> {
    report: &'a dyn Fn(f64, &str),
    started: Instant,
    total: u32,
    duration: f64,
    written: Arc<AtomicI64>,
    last: f64,
    /// Appended to every status line (the retry after a hardware failure).
    note: String,
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
            "frame {pushed} of {}, {fps:.1} fps, about {} left{}",
            self.total,
            clock(eta),
            self.note
        );
        (self.report)(fraction, &msg);
    }
}

/// The encoder side of one attempt.
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
    ) -> Result<u32, Stop> {
        let plan = self.plan;
        let at = |i: i64| {
            plan.frame_rate
                .frame_to_time(i)
                .map_err(|e| failed(format!("frame {i}: {e}")))
        };
        let mut missing = 0u32;
        for expected in 0..plan.frames {
            let Ok(picture) = pictures.recv() else {
                return Err(Stop::Export(if self.cancel.is_cancelled() {
                    ExportError::Cancelled
                } else {
                    failed("the render thread stopped")
                }));
            };
            let picture = picture?;
            if picture.index != expected {
                return Err(failed("frames arrived out of order").into());
            }
            if self.cancel.is_cancelled() {
                return Err(ExportError::Cancelled.into());
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
                // Rendered, not decoded: nothing here can be damaged.
                damaged: false,
            })
            .map_err(Stop::Media)?;
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

/// What one attempt pushed.
struct Pushed {
    missing: u32,
    audio_source_errors: u64,
}

/// One complete pass: renders and mixes everything from frame 0 into `sink` (fresh decoders,
/// mixdown and progress), up to `finish`, which the caller does.
fn push_everything(
    plan: &ExportPlan,
    services: &ExportServices,
    cancel: &CancelToken,
    report: &dyn Fn(f64, &str),
    written: &Arc<AtomicI64>,
    note: &str,
    sink: &mut dyn Encoder,
) -> Result<Pushed, Stop> {
    written.store(0, Ordering::Relaxed);
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
    #[allow(clippy::cast_precision_loss)]
    let mut side = EncodeSide {
        plan,
        services,
        cancel,
        audio: AudioFeed::new(plan, services)?,
        progress: Progress {
            report,
            started: Instant::now(),
            total: plan.frames,
            duration: (plan.duration.flicks() as f64).max(1.0),
            written: Arc::clone(written),
            last: -1.0,
            note: note.to_owned(),
        },
    };
    report(0.0, &format!("frame 0 of {}{note}", plan.frames));
    let missing = std::thread::scope(|scope| {
        let (prepared_tx, prepared_rx) = sync_channel(PIPELINE_DEPTH);
        let (picture_tx, picture_rx) = sync_channel(PIPELINE_DEPTH);
        let inputs = &inputs;
        let cannot = |name: &str, e: std::io::Error| {
            failed(format!("cannot start the export's {name} thread: {e}"))
        };
        std::thread::Builder::new()
            .name("cutweave-export-decode".into())
            .spawn_scoped(scope, move || inputs.prepare_all(&prepared_tx))
            .map_err(|e| cannot("decode", e))?;
        std::thread::Builder::new()
            .name("cutweave-export-render".into())
            .spawn_scoped(scope, move || {
                inputs.rasterize_all(&prepared_rx, &picture_tx);
            })
            .map_err(|e| cannot("render", e))?;
        // Returning drops `picture_rx`; the render thread then stops, dropping `prepared_rx`,
        // which stops the decode thread: neither stays blocked on a full queue.
        side.run(&picture_rx, sink)
    })?;
    side.audio.push_until(i64::MAX, sink)?;
    report(0.99, &format!("finishing the file{note}"));
    Ok(Pushed {
        missing,
        audio_source_errors: side.audio.mix.source_errors(),
    })
}

/// Runs the export of `plan` (see the module docs): renders, mixes and encodes it into
/// `plan.path`, reporting `(fraction, status line)` to `report`, until done or `cancel`.
/// After a mid-stream hardware-encoder failure (`hw: auto`) it starts over in software from
/// frame 0, reporting from zero again.
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
    let written = Arc::new(AtomicI64::new(0));
    let spec = encode_spec(plan, services, cancel, &written);
    tracing::info!(
        path = %plan.path.display(),
        frames = plan.frames,
        width = plan.width,
        height = plan.height,
        fps = %plan.frame_rate,
        hw = ?plan.hw,
        "export started"
    );
    // The video encoder of each attempt, the pipeline's own failure, the last attempt's result.
    let mut encoders: Vec<Option<String>> = Vec::new();
    let mut failure: Option<ExportError> = None;
    let mut pushed: Option<Pushed> = None;
    let result = encode_with_fallback(&*services.encoder, &spec, |sink| {
        let encoder = sink.video_encoder().map(str::to_owned);
        let note = match encoders.last() {
            None => String::new(),
            Some(broken) => {
                let broken = broken.as_deref().unwrap_or("the hardware encoder");
                tracing::warn!(
                    path = %plan.path.display(),
                    failed = broken,
                    now = encoder.as_deref().unwrap_or("software"),
                    "the hardware encoder failed during the export; exporting again from frame 0"
                );
                format!(" (again in software: {broken} failed)")
            }
        };
        encoders.push(encoder);
        match push_everything(plan, services, cancel, report, &written, &note, sink) {
            Ok(p) => {
                pushed = Some(p);
                Ok(())
            }
            Err(Stop::Media(e)) => Err(e),
            Err(Stop::Export(ExportError::Cancelled)) => Err(MediaError::Cancelled),
            Err(Stop::Export(e)) => {
                failure = Some(e);
                Err(MediaError::Invalid {
                    message: "the export pipeline stopped".into(),
                })
            }
        }
    });
    let encoded = match result {
        Ok(encoded) => encoded,
        Err(MediaError::Cancelled) => return Err(ExportError::Cancelled),
        Err(e) => {
            return Err(failure.unwrap_or_else(|| failed(format!("encoding failed: {e}"))));
        }
    };
    let pushed = pushed.ok_or_else(|| failed("the encoder finished without frames"))?;
    let failed_encoder = (encoders.len() > 1).then(|| {
        encoders
            .first()
            .cloned()
            .flatten()
            .unwrap_or_else(|| "the hardware encoder".to_owned())
    });
    let summary = ExportSummary {
        encoded,
        video_encoder: encoders.last().cloned().flatten(),
        failed_encoder,
        frames_missing_media: pushed.missing,
        audio_source_errors: pushed.audio_source_errors,
    };
    let elapsed = started.elapsed().as_secs_f64();
    tracing::info!(
        path = %plan.path.display(),
        frames = summary.encoded.video_frames,
        encoder = summary.video_encoder.as_deref().unwrap_or("unknown"),
        seconds = elapsed,
        realtime = summary.encoded.duration.as_secs_f64() / elapsed.max(f64::EPSILON),
        "export finished"
    );
    Ok(summary)
}
