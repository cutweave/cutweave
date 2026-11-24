// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared fixtures of the WP-1E.03 export tests: an engine whose subsystems provide only the
//! export services, `FakeBackend` media placed on the timeline through the engine's commands,
//! a decoder wrapper that counts seeks, and test renderers (a GPU-free probe renderer and a
//! frame source bound once the document exists).

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration as StdDuration, Instant};

use cutweave_api::Credential;
use cutweave_engine::export::{DecoderFrameSource, ExportServices, ExportSources};
use cutweave_engine::{Call, Connection, Engine, EngineConfig, EngineMode, Subsystems};
use cutweave_jobs::{JobInfo, JobState};
use cutweave_media::testing::{FakeAudio, FakeBackend, FakeMedia, FakeVideo};
use cutweave_media::{
    AudioDecoder, Capabilities, CodecSupport, DecodeOpts, EncodeSpec, Encoder, FrameIndex, HwCodec,
    HwEncode, MediaBackend, MediaError, PixelFormat, VideoDecoder,
};
use cutweave_model::{
    Duration, FrameRate, MediaId, ProbeInfo, Project, SampleRate, StreamKind, Time,
};
use cutweave_render::graph::RenderGraph;
use cutweave_render::query::{FrameBackend, SharedBackend};
use cutweave_render::{FrameError, FrameRequest, FrameSource, Image, SceneDescription, VideoFrame};
use serde_json::{Value, json};

/// Flicks per second.
pub const SEC: i64 = 705_600_000;

/// A temp directory removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cutweave-export-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn file(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }

    /// The names of the files in the directory.
    pub fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ----- media -----------------------------------------------------------------------------------

/// A clip of `secs` seconds: `width × height` pictures in `format` at `rate`, plus a stereo tone
/// at `tone_hz` (48 kHz unless `audio_rate` says otherwise).
pub fn clip(
    secs: i64,
    (width, height): (u32, u32),
    rate: FrameRate,
    format: PixelFormat,
    tone_hz: f64,
    audio_rate: SampleRate,
) -> FakeMedia {
    let d = Duration::from_secs(secs).unwrap();
    let mut audio = FakeAudio::tone(audio_rate, 2, d);
    audio.frequency_hz = tone_hz;
    FakeMedia {
        video: Some(FakeVideo::lasting(width, height, rate, d).with_format(format)),
        audio: Some(audio),
    }
}

/// `FakeBackend` codecs plus the export's `h264` and `aac` encoders.
pub fn export_capabilities() -> Capabilities {
    let codec = |name: &str, kind| CodecSupport {
        name: name.to_owned(),
        kind,
        decoders: vec![name.to_owned()],
        encoders: vec![name.to_owned()],
    };
    Capabilities::with_codecs([
        codec("rawvideo", StreamKind::Video),
        codec("pcm_f32le", StreamKind::Audio),
        codec("h264", StreamKind::Video),
        codec("aac", StreamKind::Audio),
    ])
}

/// Registers the fake file at `path` with `media.register` (probed by `fake`).
pub fn register(conn: &Connection, fake: &FakeBackend, path: &str) -> MediaId {
    let probe: ProbeInfo = fake.probe(Path::new(path)).unwrap();
    let out = must(
        conn,
        "media.register",
        json!({ "path": path, "probe": probe, "content_hash": format!("fake:{path}") }),
    );
    serde_json::from_value(out["media_id"].clone()).unwrap()
}

/// Inserts `media` on `track` at `at`, `duration` long from `source_in` (linked audio follows).
pub fn insert(
    conn: &Connection,
    sequence: &Value,
    track: &Value,
    media: MediaId,
    at: &str,
    duration: &str,
    source_in: &str,
) -> Value {
    must(
        conn,
        "timeline.insert",
        json!({
            "sequence": sequence, "track": track, "media": media, "at": at,
            "duration": duration, "source_in": source_in,
        }),
    )
}

/// Calls `name` and unwraps the output.
pub fn must(conn: &Connection, name: &str, params: Value) -> Value {
    match conn.call(Call::new(name, params)) {
        Ok(reply) => reply.output,
        Err(e) => panic!("{name} failed: {e}"),
    }
}

// ----- the engine ------------------------------------------------------------------------------

/// Subsystems that provide only the export services.
pub struct ExportOnly(pub ExportServices);

impl Subsystems for ExportOnly {
    fn export_services(&self) -> Option<ExportServices> {
        Some(self.0.clone())
    }
}

/// A oneshot engine whose agents may write in `dir`, with `services` (or the core only).
pub fn engine(dir: &Path, services: Option<ExportServices>) -> Engine {
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.allowed_roots = vec![dir.to_path_buf()];
    config.subsystems = services.map(|s| Box::new(ExportOnly(s)) as Box<dyn Subsystems>);
    Engine::start(config).unwrap()
}

/// An agent connection.
pub fn agent(engine: &Engine) -> Connection {
    engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap()
}

/// `project.new`: `(sequence, video track, audio track)`.
pub fn new_project(conn: &Connection) -> (Value, Value, Value) {
    let out = must(conn, "project.new", json!({ "name": "Export" }));
    (
        out["sequence"].clone(),
        out["video_track"].clone(),
        out["audio_track"].clone(),
    )
}

/// The job's state, parsed.
pub fn job(conn: &Connection, name: &str, params: Value) -> JobInfo {
    serde_json::from_value(must(conn, name, params)).unwrap()
}

/// Waits (up to `timeout`) for the job to finish.
pub fn wait(conn: &Connection, job_id: &Value, timeout: StdDuration) -> JobInfo {
    let deadline = Instant::now() + timeout;
    loop {
        let info = job(
            conn,
            "job.wait",
            json!({ "job_id": job_id, "timeout_ms": 1000 }),
        );
        if info.state.is_terminal() || Instant::now() > deadline {
            return info;
        }
    }
}

/// Polls `job.status` until `pred` holds (or panics after `timeout`).
pub fn wait_until(
    conn: &Connection,
    job_id: &Value,
    timeout: StdDuration,
    pred: impl Fn(&JobInfo) -> bool,
) -> JobInfo {
    let deadline = Instant::now() + timeout;
    loop {
        let info = job(conn, "job.status", json!({ "id": job_id }));
        if pred(&info) {
            return info;
        }
        assert!(
            !info.state.is_terminal() && Instant::now() < deadline,
            "the job never got there: {info:?}"
        );
        std::thread::sleep(StdDuration::from_millis(5));
    }
}

/// Every event queued now.
pub fn drain(rx: &mut cutweave_engine::EventReceiver) -> Vec<cutweave_engine::EngineEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

/// Asserts the job succeeded.
#[track_caller]
pub fn assert_succeeded(info: &JobInfo) {
    assert_eq!(info.state, JobState::Succeeded, "{info:?}");
}

// ----- decoders that count seeks ---------------------------------------------------------------

/// A backend delegating to `FakeBackend`, counting `seek_exact` calls.
#[derive(Clone)]
pub struct Counting {
    pub inner: FakeBackend,
    pub seeks: Arc<AtomicU64>,
}

impl Counting {
    pub fn new(inner: FakeBackend) -> Counting {
        Counting {
            inner,
            seeks: Arc::default(),
        }
    }

    pub fn seeks(&self) -> u64 {
        self.seeks.load(Ordering::Relaxed)
    }
}

struct CountingDecoder {
    inner: Box<dyn VideoDecoder>,
    seeks: Arc<AtomicU64>,
}

impl VideoDecoder for CountingDecoder {
    fn next_frame(&mut self) -> Result<Option<cutweave_media::VideoFrame>, MediaError> {
        self.inner.next_frame()
    }

    fn seek_exact(&mut self, t: Time) -> Result<cutweave_media::VideoFrame, MediaError> {
        self.seeks.fetch_add(1, Ordering::Relaxed);
        self.inner.seek_exact(t)
    }
    fn index(&self) -> std::sync::Arc<cutweave_media::FrameIndex> {
        self.inner.index()
    }

    fn set_cancel(&mut self, cancel: Option<cutweave_media::CancelToken>) {
        self.inner.set_cancel(cancel);
    }
}

impl MediaBackend for Counting {
    fn probe(&self, path: &Path) -> Result<ProbeInfo, MediaError> {
        self.inner.probe(path)
    }

    fn index_media(&self, path: &Path) -> Result<FrameIndex, MediaError> {
        self.inner.index_media(path)
    }

    fn open_decoder(
        &self,
        path: &Path,
        opts: &DecodeOpts,
    ) -> Result<Box<dyn VideoDecoder>, MediaError> {
        Ok(Box::new(CountingDecoder {
            inner: self.inner.open_decoder(path, opts)?,
            seeks: Arc::clone(&self.seeks),
        }))
    }

    fn open_audio_decoder(
        &self,
        path: &Path,
        stream: Option<u32>,
    ) -> Result<Box<dyn AudioDecoder>, MediaError> {
        self.inner.open_audio_decoder(path, stream)
    }

    fn open_encoder(&self, spec: &EncodeSpec) -> Result<Box<dyn Encoder>, MediaError> {
        self.inner.open_encoder(spec)
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
}

// ----- renderers -------------------------------------------------------------------------------

/// A frame source bound once the document exists (the engine starts before the test builds
/// its project): a `DecoderFrameSource` over the bound document.
#[derive(Default)]
pub struct LateSource(Mutex<Option<Arc<dyn FrameSource + Send + Sync>>>);

impl LateSource {
    pub fn bind(&self, backend: Arc<dyn MediaBackend>, doc: &Project) {
        *self.0.lock().unwrap() = Some(Arc::new(DecoderFrameSource::new(backend, doc)));
    }
}

impl FrameSource for LateSource {
    fn frame(&self, request: &FrameRequest) -> Result<Arc<VideoFrame>, FrameError> {
        let inner = self.0.lock().unwrap().clone();
        inner
            .ok_or_else(|| FrameError::Unavailable("the test source is not bound yet".into()))?
            .frame(request)
    }
}

/// Holds renders until opened.
#[derive(Clone, Default)]
pub struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    /// A closed gate.
    pub fn closed() -> Gate {
        Gate::default()
    }

    pub fn open(&self) {
        let (lock, cv) = &*self.0;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }

    fn pass(&self) {
        let (lock, cv) = &*self.0;
        let mut open = lock.lock().unwrap_or_else(PoisonError::into_inner);
        while !*open {
            open = cv.wait(open).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// A GPU-free renderer for pipeline tests: every pixel is `[n, n >> 8, layers, 255]` where `n`
/// is the `FakeBackend` frame number of the bottom layer's frame (`0xffff` without layers), so
/// a test can tell from the pixels which source frame was shown.
pub struct ProbeRenderer {
    source: Arc<dyn FrameSource + Send + Sync>,
    delay: StdDuration,
    gate: Option<Gate>,
}

impl ProbeRenderer {
    pub fn new(source: Arc<dyn FrameSource + Send + Sync>) -> ProbeRenderer {
        ProbeRenderer {
            source,
            delay: StdDuration::ZERO,
            gate: None,
        }
    }

    /// Each frame takes at least `delay`.
    pub fn slow(mut self, delay: StdDuration) -> ProbeRenderer {
        self.delay = delay;
        self
    }

    /// Frames wait for `gate`.
    pub fn gated(mut self, gate: Gate) -> ProbeRenderer {
        self.gate = Some(gate);
        self
    }

    pub fn shared(self) -> SharedBackend {
        Arc::new(Mutex::new(self))
    }

    /// The frame number and layer count a probe image carries.
    pub fn read(image: &Image) -> (u32, u8) {
        let p = image.pixel(0, 0);
        (u32::from(p[0]) | (u32::from(p[1]) << 8), p[2])
    }
}

impl FrameBackend for ProbeRenderer {
    fn frame_source(&self) -> &dyn FrameSource {
        &*self.source
    }

    fn rasterize(
        &mut self,
        scene: &SceneDescription,
        graph: &RenderGraph,
    ) -> Result<Image, String> {
        if let Some(gate) = &self.gate {
            gate.pass();
        }
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        let (w, h) = graph.target.size();
        let n = graph.frames.first().map_or(0xffff, |f| {
            let d = &f.planes[0].data;
            u32::from_le_bytes([d[0], d[1], d[2], d[3]]) & 0xffff
        });
        let layers = u8::try_from(scene.layers.len()).unwrap_or(u8::MAX);
        #[allow(clippy::cast_possible_truncation)]
        Ok(Image::filled(w, h, [n as u8, (n >> 8) as u8, layers, 255]))
    }
}

// ----- a pretend hardware encoder ----------------------------------------------------------------

/// The pretend hardware encoder's name.
pub const PSEUDO_HW: &str = "test_hw";

/// `inner` with a pretend hardware H.264 encoder [`PSEUDO_HW`] (listed in `hw_encode`): under
/// `HwEncode::Auto` and `Force("test_hw")` the video goes to `inner`'s software encoder under
/// that name and fails with `HwUnavailable` once `fail_after` frames were pushed (dropping the
/// encoder removes its partial output, as a real one does); `Off` is `inner` unchanged. Every
/// open's policy is recorded. `cutweave-media`'s own pseudo encoder is private to its tests.
#[derive(Clone)]
pub struct PseudoHw<B> {
    pub inner: B,
    pub fail_after: Option<u64>,
    pub opened: Arc<Mutex<Vec<HwEncode>>>,
}

impl<B> PseudoHw<B> {
    pub fn new(inner: B, fail_after: Option<u64>) -> PseudoHw<B> {
        PseudoHw {
            inner,
            fail_after,
            opened: Arc::default(),
        }
    }

    pub fn opened(&self) -> Vec<HwEncode> {
        self.opened.lock().unwrap().clone()
    }
}

struct PseudoEncoder {
    inner: Box<dyn Encoder>,
    fail_after: Option<u64>,
    pushed: u64,
}

impl Encoder for PseudoEncoder {
    fn push_video(&mut self, frame: &cutweave_media::VideoFrame) -> Result<(), MediaError> {
        if self.fail_after.is_some_and(|n| self.pushed >= n) {
            return Err(MediaError::HwUnavailable {
                reason: format!("{PSEUDO_HW}: device lost (simulated)"),
            });
        }
        self.inner.push_video(frame)?;
        self.pushed += 1;
        Ok(())
    }

    fn push_audio(&mut self, chunk: &cutweave_media::AudioChunk) -> Result<(), MediaError> {
        self.inner.push_audio(chunk)
    }

    fn video_encoder(&self) -> Option<&str> {
        Some(PSEUDO_HW)
    }

    fn finish(self: Box<Self>) -> Result<cutweave_media::EncodeSummary, MediaError> {
        self.inner.finish()
    }
}

impl<B: MediaBackend> MediaBackend for PseudoHw<B> {
    fn probe(&self, path: &Path) -> Result<ProbeInfo, MediaError> {
        self.inner.probe(path)
    }

    fn index_media(&self, path: &Path) -> Result<FrameIndex, MediaError> {
        self.inner.index_media(path)
    }

    fn open_decoder(
        &self,
        path: &Path,
        opts: &DecodeOpts,
    ) -> Result<Box<dyn VideoDecoder>, MediaError> {
        self.inner.open_decoder(path, opts)
    }

    fn open_audio_decoder(
        &self,
        path: &Path,
        stream: Option<u32>,
    ) -> Result<Box<dyn AudioDecoder>, MediaError> {
        self.inner.open_audio_decoder(path, stream)
    }

    fn open_encoder(&self, spec: &EncodeSpec) -> Result<Box<dyn Encoder>, MediaError> {
        self.opened.lock().unwrap().push(spec.hw.clone());
        let hardware = match &spec.hw {
            HwEncode::Auto => true,
            HwEncode::Force(name) if name == PSEUDO_HW => true,
            HwEncode::Force(name) => {
                return Err(MediaError::HwUnavailable {
                    reason: format!("no hardware encoder `{name}`"),
                });
            }
            _ => false,
        };
        let software = spec.clone().with_hw(HwEncode::Off);
        if !hardware {
            return self.inner.open_encoder(&software);
        }
        Ok(Box::new(PseudoEncoder {
            inner: self.inner.open_encoder(&software)?,
            fail_after: self.fail_after,
            pushed: 0,
        }))
    }

    fn capabilities(&self) -> Capabilities {
        let mut caps = self.inner.capabilities();
        caps.hw_encode = vec![HwCodec {
            device: "test".into(),
            codec: "h264".into(),
            implementation: PSEUDO_HW.into(),
        }];
        caps
    }
}

/// Export services over `media` (decode and encode) rendering with `renderer`.
pub fn services(media: Arc<dyn MediaBackend>, renderer: SharedBackend) -> ExportServices {
    let sources: Arc<dyn ExportSources> =
        Arc::new(cutweave_engine::DecoderSources::new(Arc::clone(&media)));
    ExportServices::new(media, sources, renderer)
}
