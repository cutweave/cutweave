// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.03c through the engine, without FFmpeg or a GPU: a sequence with one clip whose media
//! file is gone. `export.start` refuses it with `Invalid` at `/media/<id>` and a message that
//! names the file; with `allow_offline: true` the export runs and that clip's frames are the
//! "media offline" slate, pixel for pixel (the CPU reference renderer over `FakeBackend`
//! decoders), with its sound silent and the other clip untouched. A queued export whose media
//! goes missing while it waits fails instead of exporting a slate nobody asked for.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod export_support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use cutweave_commands::CmdError;
use cutweave_engine::export::FrameTap;
use cutweave_engine::{Call, Engine};
use cutweave_jobs::JobState;
use cutweave_media::testing::FakeBackend;
use cutweave_media::{
    AudioDecoder, Capabilities, DecodeOpts, EncodeSpec, Encoder, FrameIndex, MediaBackend,
    MediaError, PixelFormat, VideoDecoder,
};
use cutweave_model::{FrameRate, MediaId, ProbeInfo, Project, SampleRate};
use cutweave_render::query::ReferenceBackend;
use cutweave_render::{Image, slate};
use export_support::{
    LateSource, SEC, TempDir, agent, assert_succeeded, clip, engine, export_capabilities, insert,
    must, new_project, register, services, wait,
};
use serde_json::{Value, json};

const WAIT: StdDuration = StdDuration::from_secs(60);

/// The two fake files: `a` (2 s) and `b` (2 s), slate-sized so an export of the slate's size
/// shows it texel for pixel.
fn media(dir: &TempDir) -> (FakeBackend, [String; 2]) {
    let paths = [dir.file("a.mp4"), dir.file("b.mp4")];
    let mut fake = FakeBackend::new().with_capabilities(export_capabilities());
    for (path, tone) in paths.iter().zip([440.0, 660.0]) {
        fake = fake.with_media(
            path,
            clip(
                2,
                (slate::WIDTH, slate::HEIGHT),
                FrameRate::FPS_30,
                PixelFormat::Yuv420p,
                tone,
                SampleRate::HZ_48_000,
            ),
        );
    }
    (fake, paths)
}

/// `inner` on a computer where the files in `gone` are not on the disk (any call that opens
/// one answers `NotFound`, as every backend does for a path without a file).
#[derive(Clone)]
struct Vanishing {
    inner: FakeBackend,
    gone: Arc<Mutex<Vec<PathBuf>>>,
}

impl Vanishing {
    fn new(inner: FakeBackend) -> Vanishing {
        Vanishing {
            inner,
            gone: Arc::default(),
        }
    }

    fn lose(&self, path: &str) {
        self.gone.lock().unwrap().push(PathBuf::from(path));
    }

    fn check(&self, path: &Path) -> Result<(), MediaError> {
        if self.gone.lock().unwrap().iter().any(|p| p == path) {
            return Err(MediaError::NotFound {
                path: path.to_string_lossy().into_owned(),
            });
        }
        Ok(())
    }
}

impl MediaBackend for Vanishing {
    fn probe(&self, path: &Path) -> Result<ProbeInfo, MediaError> {
        self.check(path)?;
        self.inner.probe(path)
    }

    fn index_media(&self, path: &Path) -> Result<FrameIndex, MediaError> {
        self.check(path)?;
        self.inner.index_media(path)
    }

    fn open_decoder(
        &self,
        path: &Path,
        opts: &DecodeOpts,
    ) -> Result<Box<dyn VideoDecoder>, MediaError> {
        self.check(path)?;
        self.inner.open_decoder(path, opts)
    }

    fn open_audio_decoder(
        &self,
        path: &Path,
        stream: Option<u32>,
    ) -> Result<Box<dyn AudioDecoder>, MediaError> {
        self.check(path)?;
        self.inner.open_audio_decoder(path, stream)
    }

    fn open_encoder(&self, spec: &EncodeSpec) -> Result<Box<dyn Encoder>, MediaError> {
        self.inner.open_encoder(spec)
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
}

fn doc_of(engine: &Engine) -> Project {
    engine
        .handle()
        .doc()
        .unwrap()
        .committed_snapshot()
        .doc
        .clone()
}

type Seen = Arc<Mutex<Vec<(u32, Image)>>>;

fn tap(seen: &Seen) -> FrameTap {
    let seen = Arc::clone(seen);
    Arc::new(move |index, _time, image: &Image| {
        seen.lock().unwrap().push((index, image.clone()));
    })
}

/// A 0.2 s range across the cut at 2 s, at the slate's size: 3 frames of `a`, 3 of `b`.
fn across_the_cut(path: &str, allow_offline: Option<bool>) -> Value {
    let mut params = json!({
        "path": path,
        "range": {"kind": "custom", "in": "1.9s", "out": "2.1s"},
        "preset": {"custom": {"width": slate::WIDTH, "height": slate::HEIGHT}},
    });
    if let Some(allow) = allow_offline {
        params["allow_offline"] = json!(allow);
    }
    params
}

struct Rig {
    dir: TempDir,
    engine: Engine,
    conn: cutweave_engine::Connection,
    /// The media layer (the encoder's log is here).
    fake: FakeBackend,
    /// The disk: `lose` takes a file away.
    disk: Vanishing,
    paths: [String; 2],
    seen: Seen,
    b: MediaId,
    _late: Arc<LateSource>,
}

/// A project with `a` at [0, 2 s) and `b` at [2 s, 4 s), both files on the disk.
fn rig(tag: &str) -> Rig {
    let dir = TempDir::new(tag);
    let (imported, paths) = media(&dir);
    let disk = Vanishing::new(imported.clone());
    let late = Arc::new(LateSource::default());
    let seen: Seen = Arc::default();
    let backend: Arc<dyn MediaBackend> = Arc::new(disk.clone());
    let svc = services(
        Arc::clone(&backend),
        ReferenceBackend::new(Arc::clone(&late) as _).shared(),
    )
    .with_frame_tap(tap(&seen));
    let engine = engine(dir.path(), Some(svc));
    let conn = agent(&engine);
    let (seq, video, _) = new_project(&conn);
    let a = register(&conn, &imported, &paths[0]);
    let b = register(&conn, &imported, &paths[1]);
    insert(&conn, &seq, &video, a, "0s", "2s", "0s");
    insert(&conn, &seq, &video, b, "2s", "2s", "0s");
    late.bind(backend, &doc_of(&engine));
    Rig {
        dir,
        engine,
        conn,
        fake: imported,
        disk,
        paths,
        seen,
        b,
        _late: late,
    }
}

#[test]
fn missing_media_refuses_the_export_and_allow_offline_exports_the_slate() {
    let rig = rig("off");
    rig.disk.lose(&rig.paths[1]);
    let out_path = rig.dir.file("out.mp4");

    // Refused, naming the asset; nothing is queued and nothing is written.
    for params in [
        across_the_cut(&out_path, None),
        across_the_cut(&out_path, Some(false)),
    ] {
        match rig.conn.call(Call::new("export.start", params)) {
            Err(CmdError::Invalid { path, msg }) => {
                assert_eq!(path, format!("/media/{}", rig.b));
                assert!(
                    msg.starts_with("1 media file is missing: `b.mp4`; "),
                    "{msg}"
                );
                assert!(msg.contains("allow_offline: true"), "{msg}");
            }
            other => panic!("expected Invalid at /media/<id>, got {other:?}"),
        }
    }
    let queue = must(&rig.conn, "export.queue", json!({}));
    assert_eq!(queue["jobs"], json!([]), "{queue}");
    assert!(rig.fake.encodes().is_empty());
    assert!(!std::path::Path::new(&out_path).exists());

    // A range that only plays `a` exports as ever.
    let head = rig.dir.file("head.mp4");
    let mut params = across_the_cut(&head, None);
    params["range"] = json!({"kind": "custom", "in": "1s", "out": "1.1s"});
    let started = must(&rig.conn, "export.start", params);
    let info = wait(&rig.conn, &started["job_id"], WAIT);
    assert_succeeded(&info);
    assert_eq!(
        info.progress.message.as_deref(),
        Some("exported 3 frames (0.10 s)")
    );
    rig.seen.lock().unwrap().clear();

    // Allowed: the export runs, `a`'s frames are its pictures and `b`'s are the slate.
    let started = must(
        &rig.conn,
        "export.start",
        across_the_cut(&out_path, Some(true)),
    );
    assert_eq!(started["frames"], json!(6));
    let info = wait(&rig.conn, &started["job_id"], WAIT);
    assert_succeeded(&info);
    let message = info.progress.message.clone().unwrap_or_default();
    assert!(
        message.starts_with("exported 6 frames (0.20 s)"),
        "{message}"
    );
    assert!(
        message.contains("3 frames have offline or unreadable media"),
        "{message}"
    );
    assert!(
        message.contains("1 audio clips could not be read and are silent"),
        "{message}"
    );
    let seen = rig.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 6);
    let slate = slate::image();
    for (index, image) in &seen {
        assert_eq!(
            (image.width, image.height),
            (slate::WIDTH, slate::HEIGHT),
            "frame {index}"
        );
        if *index < 3 {
            assert_ne!(image, &slate, "frame {index} shows clip a");
        } else {
            assert_eq!(image, &slate, "frame {index} is the slate, exactly");
        }
    }
    // Every frame and the whole range's sound reached the encoder (the missing clip's part is
    // silence: a source that cannot be opened is never mixed).
    let encodes = rig.fake.encodes();
    let (path, summary) = encodes.last().unwrap();
    assert_eq!(path.to_string_lossy(), out_path);
    assert_eq!(summary.video_frames, 6);
    assert_eq!(summary.audio_samples, 48_000 / 5);
    assert_eq!(summary.duration.flicks(), SEC / 5);

    drop(rig.conn);
    rig.engine.shutdown();
}

#[test]
fn a_queued_export_fails_when_its_media_is_gone_by_its_turn() {
    // Queued while both files are there (the queue is paused); `b` is gone when it resumes.
    let rig = rig("offq");
    let (conn, dir) = (&rig.conn, &rig.dir);
    must(conn, "export.pause_queue", json!({ "paused": true }));
    let strict = must(
        conn,
        "export.enqueue",
        across_the_cut(&dir.file("strict.mp4"), None),
    );
    let lenient = must(
        conn,
        "export.enqueue",
        across_the_cut(&dir.file("lenient.mp4"), Some(true)),
    );
    rig.disk.lose(&rig.paths[1]);
    must(conn, "export.pause_queue", json!({ "paused": false }));

    let info = wait(conn, &strict["job_id"], WAIT);
    assert_eq!(info.state, JobState::Failed, "{info:?}");
    let error = format!("{info:?}");
    assert!(
        error.contains("1 media file is missing: `b.mp4`"),
        "{error}"
    );
    assert!(!std::path::Path::new(&dir.file("strict.mp4")).exists());
    // The one that allowed it exports the slate.
    let info = wait(conn, &lenient["job_id"], WAIT);
    assert_succeeded(&info);
    assert_eq!(rig.seen.lock().unwrap().len(), 6);

    drop(rig.conn);
    rig.engine.shutdown();
}
