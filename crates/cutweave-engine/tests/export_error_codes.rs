// SPDX-License-Identifier: GPL-3.0-or-later
//! ADR-0038 through the engine, without FFmpeg or a GPU: a failed export says what kind of
//! failure it was. Each failure site of the export pipeline gives its code (`output_exists`,
//! `disk_full`, `output_unwritable`, `encoder_failed`, `render_failed`, `engine_unavailable`;
//! `media_offline` is in `export_offline.rs`), on `job.status`, `job.wait`, the `job.progress`
//! event and the `export.queue` row; the code is kept in the project's `exports.sqlite` across
//! an engine restart, and a retry clears it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod export_support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use cutweave_engine::export::ExportServices;
use cutweave_engine::project_io::ProjectStore;
use cutweave_engine::{Connection, Engine, EngineConfig, EngineEvent, EngineMode, Subsystems};
use cutweave_jobs::{JobInfo, JobState};
use cutweave_media::testing::FakeBackend;
use cutweave_media::{
    AudioDecoder, Capabilities, DecodeOpts, EncodeSpec, Encoder, FrameIndex, MediaBackend,
    MediaError, PixelFormat, VideoDecoder,
};
use cutweave_model::{FrameRate, ProbeInfo, Project, SampleRate};
use cutweave_render::graph::RenderGraph;
use cutweave_render::query::FrameBackend;
use cutweave_render::{FrameSource, Image, SceneDescription};
use export_support::{
    LateSource, ProbeRenderer, TempDir, agent, assert_succeeded, clip, drain, export_capabilities,
    insert, job, must, new_project, register, services, wait,
};
use serde_json::{Value, json};

const WAIT: StdDuration = StdDuration::from_secs(60);

/// One 3 s clip (64x36, 30 fps) at an absolute path inside `dir`.
fn media(dir: &Path) -> (FakeBackend, String) {
    let path = dir.join("a.mp4").to_string_lossy().into_owned();
    let fake = FakeBackend::new()
        .with_capabilities(export_capabilities())
        .with_media(
            &path,
            clip(
                3,
                (64, 36),
                FrameRate::FPS_30,
                PixelFormat::Yuv420p,
                440.0,
                SampleRate::HZ_48_000,
            ),
        );
    (fake, path)
}

/// A 2 s timeline of the clip.
fn timeline(conn: &Connection, fake: &FakeBackend, path: &str) {
    let (seq, video, _) = new_project(conn);
    let id = register(conn, fake, path);
    insert(conn, &seq, &video, id, "0s", "2s", "0s");
}

/// The small export the tests queue.
fn small(path: &str) -> Value {
    json!({ "path": path, "preset": {"custom": {"width": 64, "height": 36}} })
}

/// Export services over `media` with a probe renderer.
fn svc(media: Arc<dyn MediaBackend>) -> ExportServices {
    let late = Arc::new(LateSource::default());
    services(media, ProbeRenderer::new(late as _).shared())
}

/// Subsystems whose export services can go away while the engine runs.
struct Switchable {
    services: ExportServices,
    gone: Arc<AtomicBool>,
}

impl Subsystems for Switchable {
    fn export_services(&self) -> Option<ExportServices> {
        (!self.gone.load(Ordering::SeqCst)).then(|| self.services.clone())
    }
}

/// A oneshot engine on `project` (a folder) or untitled, whose agents may write in `dir`.
fn engine_on(
    dir: &Path,
    project: Option<&Path>,
    services: ExportServices,
    gone: &Arc<AtomicBool>,
) -> Engine {
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.allowed_roots = vec![dir.to_path_buf()];
    config.project = project.map(Path::to_path_buf);
    config.subsystems = Some(Box::new(Switchable {
        services,
        gone: Arc::clone(gone),
    }) as Box<dyn Subsystems>);
    Engine::start(config).unwrap()
}

/// An empty project folder `name` in `dir`.
fn project_folder(dir: &Path, name: &str) -> PathBuf {
    let folder = dir.join(name);
    ProjectStore::create(&folder, &Project::new(name))
        .unwrap()
        .close()
        .unwrap();
    folder
}

/// `FakeBackend` whose encoder fails as the test says: on open, or (`None`) not at all.
#[derive(Clone)]
struct BrokenEncoder {
    inner: FakeBackend,
    error: Arc<Mutex<Option<MediaError>>>,
}

impl BrokenEncoder {
    fn new(inner: FakeBackend) -> BrokenEncoder {
        BrokenEncoder {
            inner,
            error: Arc::default(),
        }
    }

    fn fail_with(&self, error: Option<MediaError>) {
        *self.error.lock().unwrap() = error;
    }
}

impl MediaBackend for BrokenEncoder {
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
        match self.error.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => self.inner.open_encoder(spec),
        }
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
}

/// A renderer that cannot draw.
struct DeadRenderer(Arc<dyn FrameSource + Send + Sync>);

impl FrameBackend for DeadRenderer {
    fn frame_source(&self) -> &dyn FrameSource {
        &*self.0
    }

    fn rasterize(&mut self, _: &SceneDescription, _: &RenderGraph) -> Result<Image, String> {
        Err("the GPU device was lost".into())
    }
}

/// Starts the small export to `name` and waits for it to fail: the job as `job.wait` reports it.
fn failed_export(conn: &Connection, dir: &TempDir, name: &str) -> JobInfo {
    let started = must(conn, "export.start", small(&dir.file(name)));
    let info = wait(conn, &started["job_id"], WAIT);
    assert_eq!(info.state, JobState::Failed, "{info:?}");
    info
}

#[test]
fn encoder_failures_are_told_apart_by_their_code() {
    let dir = TempDir::new("codes-encoder");
    let (fake, path) = media(dir.path());
    let broken = BrokenEncoder::new(fake.clone());
    let gone = Arc::new(AtomicBool::new(false));
    let e = engine_on(dir.path(), None, svc(Arc::new(broken.clone())), &gone);
    let conn = agent(&e);
    timeline(&conn, &fake, &path);
    let out = dir.file("out.mp4");
    for (error, code) in [
        (
            MediaError::DiskFull {
                path: out.clone(),
                message: "writing the output failed: no space left on device".into(),
            },
            "disk_full",
        ),
        (
            MediaError::Io {
                path: out.clone(),
                message: "cannot create the file: access is denied".into(),
            },
            "output_unwritable",
        ),
        (
            MediaError::Backend {
                message: "video: the encoder returned an error".into(),
            },
            "encoder_failed",
        ),
        (
            MediaError::Unsupported {
                message: "no encoder for `h264`".into(),
            },
            "encoder_failed",
        ),
    ] {
        broken.fail_with(Some(error.clone()));
        let info = failed_export(&conn, &dir, "out.mp4");
        assert_eq!(info.error_code.as_deref(), Some(code), "{info:?}");
        // The message is the one people read, unchanged by the code.
        assert_eq!(
            info.error.as_deref(),
            Some(format!("encoding failed: {error}").as_str())
        );
        must(&conn, "export.remove", json!({ "id": info.id }));
    }
    // The same engine exports once the encoder works: a job that succeeds has no code.
    broken.fail_with(None);
    let started = must(&conn, "export.start", small(&out));
    let info = wait(&conn, &started["job_id"], WAIT);
    assert_succeeded(&info);
    assert_eq!(info.error_code, None);
    drop(conn);
    e.shutdown();
}

#[test]
fn a_frame_that_cannot_be_drawn_is_render_failed() {
    let dir = TempDir::new("codes-render");
    let (fake, path) = media(dir.path());
    let late = Arc::new(LateSource::default());
    let renderer: cutweave_render::query::SharedBackend =
        Arc::new(Mutex::new(DeadRenderer(late as _)));
    let gone = Arc::new(AtomicBool::new(false));
    let e = engine_on(
        dir.path(),
        None,
        services(Arc::new(fake.clone()), renderer),
        &gone,
    );
    let conn = agent(&e);
    timeline(&conn, &fake, &path);
    let info = failed_export(&conn, &dir, "out.mp4");
    assert_eq!(
        info.error_code.as_deref(),
        Some("render_failed"),
        "{info:?}"
    );
    let error = info.error.unwrap_or_default();
    assert!(error.contains("the GPU device was lost"), "{error}");
    drop(conn);
    e.shutdown();
}

#[test]
fn an_engine_that_lost_its_export_services_is_engine_unavailable() {
    let dir = TempDir::new("codes-engine");
    let (fake, path) = media(dir.path());
    let gone = Arc::new(AtomicBool::new(false));
    let e = engine_on(dir.path(), None, svc(Arc::new(fake.clone())), &gone);
    let conn = agent(&e);
    timeline(&conn, &fake, &path);
    // Queued while the engine could export; its media backend is gone when its turn comes.
    must(&conn, "export.pause_queue", json!({ "paused": true }));
    let queued = must(&conn, "export.enqueue", small(&dir.file("out.mp4")));
    gone.store(true, Ordering::SeqCst);
    must(&conn, "export.pause_queue", json!({ "paused": false }));
    let info = wait(&conn, &queued["job_id"], WAIT);
    assert_eq!(info.state, JobState::Failed, "{info:?}");
    assert_eq!(info.error_code.as_deref(), Some("engine_unavailable"));
    drop(conn);
    e.shutdown();
}

#[test]
fn the_code_is_on_every_surface_survives_a_restart_and_a_retry_clears_it() {
    let dir = TempDir::new("codes-restart");
    let folder = project_folder(dir.path(), "Codes");
    let (fake, path) = media(dir.path());
    let out = dir.file("out.mp4");
    let gone = Arc::new(AtomicBool::new(false));
    let id;
    {
        let e = engine_on(
            dir.path(),
            Some(&folder),
            svc(Arc::new(fake.clone())),
            &gone,
        );
        let conn = agent(&e);
        let mut events = e.handle().subscribe();
        timeline(&conn, &fake, &path);
        must(&conn, "project.save", json!({}));
        // Queued while the file is not there; it appears before the export's turn.
        must(&conn, "export.pause_queue", json!({ "paused": true }));
        let queued = must(&conn, "export.enqueue", small(&out));
        id = queued["job_id"].clone();
        std::fs::write(&out, b"someone else's file").unwrap();
        must(&conn, "export.pause_queue", json!({ "paused": false }));

        // job.wait and job.status.
        let info = wait(&conn, &id, WAIT);
        assert_eq!(info.state, JobState::Failed, "{info:?}");
        assert_eq!(info.error_code.as_deref(), Some("output_exists"));
        let error = info.error.clone().unwrap_or_default();
        assert!(error.contains("was created after the export"), "{error}");
        assert_eq!(job(&conn, "job.status", json!({ "id": id })), info);
        // job.list.
        let listed = must(&conn, "job.list", json!({ "state": "failed" }));
        assert_eq!(listed["jobs"][0]["error_code"], json!("output_exists"));
        // The export.queue row.
        let row = must(&conn, "export.queue", json!({}))["jobs"][0].clone();
        assert_eq!(row["error_code"], json!("output_exists"));
        assert_eq!(row["error"], json!(error));
        // The job.progress event of the failure; the ones before it carry no code.
        let jobs: Vec<JobInfo> = drain(&mut events)
            .into_iter()
            .filter_map(|event| match event {
                EngineEvent::Job(j) if j.job().id == info.id => Some(j.job().clone()),
                _ => None,
            })
            .collect();
        let last = jobs.last().expect("job.progress events");
        assert_eq!(last.error_code.as_deref(), Some("output_exists"));
        assert!(
            jobs.iter()
                .all(|j| j.state == JobState::Failed || j.error_code.is_none()),
            "{jobs:?}"
        );
        // The file that was in the way is untouched.
        assert_eq!(std::fs::read(&out).unwrap(), b"someone else's file");
        drop(conn);
        e.shutdown();
    }
    // The next engine reads the failure, code included, from the project's queue.
    let e = engine_on(
        dir.path(),
        Some(&folder),
        svc(Arc::new(fake.clone())),
        &gone,
    );
    let conn = agent(&e);
    let row = must(&conn, "export.queue", json!({}))["jobs"][0].clone();
    assert_eq!(row["id"], id);
    assert_eq!(row["state"], json!("failed"));
    assert_eq!(row["error_code"], json!("output_exists"));
    let info = job(&conn, "job.status", json!({ "id": id }));
    assert_eq!(info.error_code.as_deref(), Some("output_exists"));
    // With the file out of the way a retry runs it; the failure and its code are gone.
    std::fs::remove_file(&out).unwrap();
    must(&conn, "export.retry", json!({ "id": id }));
    let info = wait(&conn, &id, WAIT);
    assert_succeeded(&info);
    assert_eq!((info.error, info.error_code), (None, None));
    let row = must(&conn, "export.queue", json!({}))["jobs"][0].clone();
    assert_eq!(row["error_code"], Value::Null);
    drop(conn);
    e.shutdown();
}
