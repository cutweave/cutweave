// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1E.03 through the engine, without FFmpeg or a GPU: `export.start` runs as an `export`
//! job over the subsystems' export services (here `FakeBackend` decoding and encoding and a
//! probe renderer), every frame and sample reaches the encoder, clips decode sequentially, the
//! export renders the snapshot it was started on, a cancel stops it within a second, one
//! output path is written by one export at a time, and the core engine (no media subsystem)
//! refuses exports with `Unsupported`. With a pretend hardware encoder (WP-1E.02): `hw` reaches
//! the encoder, a mid-export hardware failure starts the export over in software from frame 0,
//! and a forced encoder never falls back. The MP4 output itself is checked in
//! `export_ffmpeg.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod export_support;

use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use cutweave_commands::CmdError;
use cutweave_engine::Call;
use cutweave_engine::export::FrameTap;
use cutweave_jobs::{JobKind, JobState};
use cutweave_media::testing::FakeBackend;
use cutweave_media::{HwEncode, MediaBackend, PixelFormat};
use cutweave_model::{FrameRate, SampleRate, Time};
use cutweave_render::Image;
use export_support::{
    Counting, Gate, LateSource, PSEUDO_HW, ProbeRenderer, PseudoHw, SEC, TempDir, agent,
    assert_succeeded, clip, engine, export_capabilities, insert, job, must, new_project, register,
    services, wait, wait_until,
};
use serde_json::{Value, json};

const WAIT: StdDuration = StdDuration::from_secs(30);

/// The small custom output every test here uses (the probe renderer fills it).
fn small(path: &str) -> Value {
    json!({ "path": path, "preset": {"custom": {"width": 64, "height": 36}} })
}

/// What the tap saw: `(index, time, frame number, layers)` per frame.
type Seen = Arc<Mutex<Vec<(u32, Time, u32, u8)>>>;

fn tap(seen: &Seen) -> FrameTap {
    let seen = Arc::clone(seen);
    Arc::new(move |index, time, image: &Image| {
        let (n, layers) = ProbeRenderer::read(image);
        seen.lock().unwrap().push((index, time, n, layers));
    })
}

/// The media of the tests, at absolute paths inside `dir` (the files need not exist).
fn media(dir: &TempDir) -> (FakeBackend, [String; 3]) {
    let paths = [dir.file("a.mp4"), dir.file("b.mp4"), dir.file("c.mp4")];
    let fake = FakeBackend::new()
        .with_capabilities(export_capabilities())
        .with_media(
            &paths[0],
            clip(
                4,
                (64, 36),
                FrameRate::FPS_30,
                PixelFormat::Yuv420p,
                440.0,
                SampleRate::HZ_48_000,
            ),
        )
        .with_media(
            &paths[1],
            clip(
                4,
                (64, 36),
                FrameRate::FPS_30,
                PixelFormat::Nv12,
                660.0,
                SampleRate::HZ_48_000,
            ),
        )
        .with_media(
            &paths[2],
            clip(
                2,
                (64, 36),
                FrameRate::FPS_25,
                PixelFormat::Yuv420p,
                880.0,
                SampleRate::HZ_44_100,
            ),
        );
    (fake, paths)
}

/// Fills the document: A [0, 2 s) from its start, B [2 s, 4 s) from 1 s in, C (25 fps)
/// [4 s, 5 s), each with its linked audio. Returns the clip ids `(video, audio)` per clip.
fn timeline(
    conn: &cutweave_engine::Connection,
    fake: &FakeBackend,
    paths: &[String; 3],
) -> Vec<(Value, Value)> {
    let (seq, video, _) = new_project(conn);
    let ids: Vec<_> = paths.iter().map(|p| register(conn, fake, p)).collect();
    let placed = [
        insert(conn, &seq, &video, ids[0], "0s", "2s", "0s"),
        insert(conn, &seq, &video, ids[1], "2s", "2s", "1s"),
        insert(conn, &seq, &video, ids[2], "4s", "1s", "0s"),
    ];
    placed
        .iter()
        .map(|o| (o["clips"][0].clone(), o["clips"][1].clone()))
        .collect()
}

#[test]
fn an_export_job_encodes_every_frame_and_sample_and_decodes_clips_sequentially() {
    let dir = TempDir::new("job");
    let (fake, paths) = media(&dir);
    let counting = Counting::new(fake.clone());
    let late = Arc::new(LateSource::default());
    let seen: Seen = Arc::default();
    let media_backend: Arc<dyn MediaBackend> = Arc::new(counting.clone());
    let svc = services(
        Arc::clone(&media_backend),
        ProbeRenderer::new(Arc::clone(&late) as _).shared(),
    )
    .with_frame_tap(tap(&seen));
    let e = engine(dir.path(), Some(svc));
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    let out_path = dir.file("out.mp4");

    let started = must(&conn, "export.start", small(&out_path));
    assert_eq!(started["frames"], json!(150));
    assert_eq!(started["duration"], json!(5 * SEC));
    assert_eq!(
        (started["width"].clone(), started["height"].clone()),
        (json!(64), json!(36))
    );
    let info = wait(&conn, &started["job_id"], WAIT);
    assert_succeeded(&info);
    assert_eq!(info.kind, JobKind::Export);
    assert_eq!(info.label, "export");
    assert_eq!(info.progress.fraction, 1.0);
    assert_eq!(
        info.progress.message.as_deref(),
        Some("exported 150 frames (5.00 s)")
    );

    // The encoder got every frame and 5 s of 48 kHz audio, in order (FakeEncoder checks pts,
    // sizes and gapless audio like the real one).
    let encodes = fake.encodes();
    assert_eq!(encodes.len(), 1);
    let (path, summary) = &encodes[0];
    assert_eq!(path.to_string_lossy(), out_path);
    assert_eq!(summary.video_frames, 150);
    assert_eq!(summary.audio_samples, 5 * 48_000);
    assert_eq!(summary.duration.flicks(), 5 * SEC);

    // Frame i shows sequence time i/30 s and the right source frame of each clip.
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 150);
    for (i, &(index, time, n, layers)) in seen.iter().enumerate() {
        let i = u32::try_from(i).unwrap();
        assert_eq!(index, i);
        assert_eq!(time, FrameRate::FPS_30.frame_to_time(i64::from(i)).unwrap());
        let expected = match i {
            0..60 => i,               // A from its start
            60..120 => i - 60 + 30,   // B from 1 s in
            _ => (i - 120) * 25 / 30, // C at 25 fps: the frame on screen at (i-120)/30 s
        };
        assert_eq!((n, layers), (expected, 1), "frame {i}");
    }

    // Sequential decode: one seek per clip, then forward (plus one frame of look-ahead each).
    assert_eq!(counting.seeks(), 3);
    assert!(
        fake.decoded_frames() <= 150 + 3,
        "{}",
        fake.decoded_frames()
    );
    drop(conn);
    e.shutdown();
    drop(late);
}

#[test]
fn an_export_with_a_preset_id_uses_its_size_rate_and_warns_without_failing() {
    let dir = TempDir::new("preset");
    let (fake, paths) = media(&dir);
    let late = Arc::new(LateSource::default());
    let backend: Arc<dyn MediaBackend> = Arc::new(fake.clone());
    let svc = services(backend, ProbeRenderer::new(Arc::clone(&late) as _).shared());
    let e = engine(dir.path(), Some(svc));
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);

    // The commands answer through the engine; TikTok on a 16:9 sequence is a warning.
    let list = must(&conn, "export.list_presets", json!({}));
    assert!(list["presets"].as_array().unwrap().len() >= 20);
    let resolved = must(&conn, "export.resolve_preset", json!({"preset": "tiktok"}));
    assert_eq!(resolved["warnings"][0]["code"], json!("AspectMismatch"));

    // A preset export with small overrides keeps the preset's frame-rate rule (the
    // sequence's 30 fps is within TikTok's maximum) and reports the warning.
    let out_path = dir.file("tiktok.mp4");
    let started = must(
        &conn,
        "export.start",
        json!({"path": out_path, "preset": {"custom": {"preset": "tiktok",
               "width": 64, "height": 36}}}),
    );
    assert_eq!(started["warnings"][0]["code"], json!("AspectMismatch"));
    assert_eq!(
        (started["width"].clone(), started["height"].clone()),
        (json!(64), json!(36))
    );
    assert_eq!(started["frame_rate"], json!({"num": 30, "den": 1}));
    assert_succeeded(&wait(&conn, &started["job_id"], WAIT));

    // User presets save and delete through the engine; built-ins are protected.
    let preset = json!({
        "schema_version": 1, "id": "mine", "name": "Mine", "container": "mp4",
        "video": {"codec": "h264", "rate_control": {"mode": "crf", "crf": 24}, "hw_preferred": false},
        "audio": {"codec": "aac", "sample_rate": 48000, "channels": 2},
        "size": {"kind": "fit", "width": 64, "height": 36},
        "fps": {"kind": "follow_sequence"}
    });
    must(&conn, "export.save_preset", json!({"preset": preset}));
    let started = must(
        &conn,
        "export.start",
        json!({"path": dir.file("mine.mp4"), "preset": "mine"}),
    );
    assert_eq!(started["warnings"], json!([]));
    assert_succeeded(&wait(&conn, &started["job_id"], WAIT));
    must(&conn, "export.delete_preset", json!({"id": "mine"}));
    assert!(
        conn.call(Call::new("export.delete_preset", json!({"id": "tiktok"})))
            .is_err()
    );
    drop(conn);
    e.shutdown();
    drop(late);
}

#[test]
fn an_export_renders_the_snapshot_it_was_started_on() {
    let dir = TempDir::new("snapshot");
    let (fake, paths) = media(&dir);
    let gate = Gate::closed();
    let late = Arc::new(LateSource::default());
    let seen: Seen = Arc::default();
    let svc = services(
        Arc::new(fake.clone()),
        ProbeRenderer::new(Arc::clone(&late) as _)
            .gated(gate.clone())
            .shared(),
    )
    .with_frame_tap(tap(&seen));
    let e = engine(dir.path(), Some(svc));
    let conn = agent(&e);
    let clips = timeline(&conn, &fake, &paths);
    let started = must(&conn, "export.start", small(&dir.file("out.mp4")));
    let rev = started["rev"].clone();

    // The export waits at its first frame; meanwhile the last clip goes away.
    let (last_video, last_audio) = clips[2].clone();
    let deleted = conn
        .call(Call::new(
            "item.delete",
            json!({ "items": [last_video, last_audio] }),
        ))
        .unwrap();
    assert!(deleted.rev.0 > rev.as_u64().unwrap());
    gate.open();
    let info = wait(&conn, &started["job_id"], WAIT);
    assert_succeeded(&info);
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen.len(),
        150,
        "the export kept the 5 s sequence it was started on"
    );
    assert_eq!((seen[149].2, seen[149].3), (24, 1), "clip C is still in it");
    assert_eq!(fake.encodes()[0].1.audio_samples, 5 * 48_000);

    // A new export sees the edit.
    let again = must(
        &conn,
        "export.start",
        json!({ "path": dir.file("again.mp4"), "preset": {"custom": {"width": 64, "height": 36}} }),
    );
    assert_eq!(again["frames"], json!(120));
    assert_succeeded(&wait(&conn, &again["job_id"], WAIT));
    drop(conn);
    e.shutdown();
}

#[test]
fn cancelling_stops_an_export_within_a_second() {
    let dir = TempDir::new("cancel");
    let (fake, paths) = media(&dir);
    let late = Arc::new(LateSource::default());
    let svc = services(
        Arc::new(fake.clone()),
        ProbeRenderer::new(Arc::clone(&late) as _)
            .slow(StdDuration::from_millis(10))
            .shared(),
    );
    let e = engine(dir.path(), Some(svc));
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    let started = must(&conn, "export.start", small(&dir.file("out.mp4")));
    let id = started["job_id"].clone();
    let half = wait_until(&conn, &id, WAIT, |j| j.progress.fraction >= 0.5);
    assert_eq!(half.state, JobState::Running);
    assert!(
        half.progress
            .message
            .as_deref()
            .is_some_and(|m| m.contains(" of 150") && m.contains("fps")),
        "{half:?}"
    );
    let asked = Instant::now();
    let cancelling = job(&conn, "job.cancel", json!({ "id": id }));
    assert!(cancelling.cancel_requested);
    let done = wait(&conn, &id, WAIT);
    let took = asked.elapsed();
    assert_eq!(done.state, JobState::Cancelled, "{done:?}");
    assert!(
        took < StdDuration::from_secs(1),
        "the export took {took:?} to stop"
    );
    println!(
        "cancelled at {:.0}% in {took:?}",
        half.progress.fraction * 100.0
    );
    assert!(
        fake.encodes().is_empty(),
        "a cancelled export never finishes its file"
    );
    assert_eq!(dir.entries(), Vec::<String>::new());
    drop(conn);
    e.shutdown();
}

#[test]
fn one_export_writes_a_file_at_a_time_and_exports_queue() {
    let dir = TempDir::new("queue");
    let (fake, paths) = media(&dir);
    let gate = Gate::closed();
    let late = Arc::new(LateSource::default());
    let svc = services(
        Arc::new(fake.clone()),
        ProbeRenderer::new(Arc::clone(&late) as _)
            .gated(gate.clone())
            .shared(),
    );
    let e = engine(dir.path(), Some(svc));
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    let out = dir.file("out.mp4");
    let first = must(&conn, "export.start", small(&out));
    // The same file again: refused while the first export holds it.
    match conn.call(Call::new("export.start", small(&out))) {
        Err(CmdError::Invalid { path, .. }) => assert_eq!(path, "/path"),
        other => panic!("expected Invalid at /path, got {other:?}"),
    }
    // Another file: queued behind it (one export at a time).
    let second = must(&conn, "export.start", small(&dir.file("other.mp4")));
    let queued = job(&conn, "job.status", json!({ "id": second["job_id"] }));
    assert_eq!(queued.state, JobState::Queued);
    gate.open();
    assert_succeeded(&wait(&conn, &first["job_id"], WAIT));
    assert_succeeded(&wait(&conn, &second["job_id"], WAIT));
    // Released: the first path can be exported again (it exists now only for real encoders).
    let third = must(&conn, "export.start", small(&out));
    assert_succeeded(&wait(&conn, &third["job_id"], WAIT));
    assert_eq!(fake.encodes().len(), 3);
    drop(conn);
    e.shutdown();
}

#[test]
fn agents_export_only_inside_allowed_folders() {
    let dir = TempDir::new("roots");
    let elsewhere = TempDir::new("elsewhere");
    let (fake, paths) = media(&dir);
    let late = Arc::new(LateSource::default());
    let svc = services(
        Arc::new(fake.clone()),
        ProbeRenderer::new(Arc::clone(&late) as _).shared(),
    );
    let e = engine(dir.path(), Some(svc));
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    match conn.call(Call::new("export.start", small(&elsewhere.file("out.mp4")))) {
        Err(CmdError::Invalid { path, msg }) => {
            assert_eq!(path, "/path");
            assert!(msg.contains("outside the folders"), "{msg}");
        }
        other => panic!("expected Invalid at /path, got {other:?}"),
    }
    // A source file of the project is never an output.
    match conn.call(Call::new(
        "export.start",
        json!({ "path": paths[0], "overwrite": true }),
    )) {
        Err(CmdError::Invalid { path, msg }) => {
            assert_eq!(path, "/path");
            assert!(msg.contains("source file"), "{msg}");
        }
        other => panic!("expected Invalid at /path, got {other:?}"),
    }
    assert_eq!(fake.encodes().len(), 0);
    drop(conn);
    e.shutdown();
}

#[test]
fn engines_without_an_encoder_refuse_exports_after_validating_them() {
    let dir = TempDir::new("core");
    let (fake, paths) = media(&dir);
    // The core engine: no subsystems.
    let core = engine(dir.path(), None);
    let conn = agent(&core);
    timeline(&conn, &fake, &paths);
    match conn.call(Call::new(
        "export.start",
        json!({ "path": dir.file("out.mov") }),
    )) {
        Err(CmdError::Invalid { path, .. }) => assert_eq!(path, "/path"),
        other => panic!("expected Invalid at /path, got {other:?}"),
    }
    match conn.call(Call::new(
        "export.start",
        json!({ "path": dir.file("out.mp4") }),
    )) {
        Err(CmdError::Unsupported { msg }) => assert!(msg.contains("media backend"), "{msg}"),
        other => panic!("expected Unsupported, got {other:?}"),
    }
    let jobs = must(&conn, "job.list", json!({}));
    assert_eq!(jobs["jobs"], json!([]));
    drop(conn);
    core.shutdown();

    // A media backend without H.264: Unsupported too (it has the files: missing media would
    // be refused first, WP-2F.03c).
    let plain = fake
        .clone()
        .with_capabilities(FakeBackend::new().capabilities());
    let late = Arc::new(LateSource::default());
    let e = engine(
        dir.path(),
        Some(services(
            Arc::new(plain),
            ProbeRenderer::new(Arc::clone(&late) as _).shared(),
        )),
    );
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    match conn.call(Call::new(
        "export.start",
        json!({ "path": dir.file("out.mp4") }),
    )) {
        Err(CmdError::Unsupported { msg }) => assert!(msg.contains("h264"), "{msg}"),
        other => panic!("expected Unsupported, got {other:?}"),
    }
    assert_eq!(dir.entries(), Vec::<String>::new());
    drop(conn);
    e.shutdown();
}

// ----- hardware encoders (WP-1E.02) ----------------------------------------------------------

/// An engine exporting with `backend` (a `PseudoHw` over `FakeBackend`), the probe renderer and
/// a tap.
fn hw_engine(
    dir: &TempDir,
    backend: PseudoHw<FakeBackend>,
    seen: &Seen,
) -> cutweave_engine::Engine {
    let late = Arc::new(LateSource::default());
    let svc = services(Arc::new(backend), ProbeRenderer::new(late as _).shared())
        .with_frame_tap(tap(seen));
    engine(dir.path(), Some(svc))
}

#[test]
fn the_hw_param_picks_the_video_encoder() {
    let dir = TempDir::new("hw-policy");
    let (fake, paths) = media(&dir);
    let pseudo = PseudoHw::new(fake.clone(), None);
    let seen: Seen = Arc::default();
    let e = hw_engine(&dir, pseudo.clone(), &seen);
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    let mut messages = Vec::new();
    for (i, hw) in [
        Value::Null,
        json!("auto"),
        json!("off"),
        json!({ "force": PSEUDO_HW }),
    ]
    .into_iter()
    .enumerate()
    {
        let mut params = small(&dir.file(&format!("out{i}.mp4")));
        params["hw"] = hw;
        let started = must(&conn, "export.start", params);
        let info = wait(&conn, &started["job_id"], WAIT);
        assert_succeeded(&info);
        messages.push(info.progress.message.unwrap_or_default());
    }
    assert_eq!(
        pseudo.opened(),
        [
            HwEncode::Auto,
            HwEncode::Auto,
            HwEncode::Off,
            HwEncode::Force(PSEUDO_HW.into())
        ]
    );
    // The closing message names the encoder (`FakeBackend`'s own does not name itself).
    assert_eq!(messages[0], "exported 150 frames (5.00 s) with test_hw");
    assert_eq!(messages[2], "exported 150 frames (5.00 s)");
    assert_eq!(messages[3], "exported 150 frames (5.00 s) with test_hw");

    // A forced encoder must be one this machine has.
    let mut params = small(&dir.file("qsv.mp4"));
    params["hw"] = json!({ "force": "h264_qsv" });
    match conn.call(Call::new("export.start", params)) {
        Err(CmdError::Invalid { path, msg }) => {
            assert_eq!(path, "/hw/force");
            assert!(msg.contains("test_hw"), "{msg}");
        }
        other => panic!("expected Invalid at /hw/force, got {other:?}"),
    }
    drop(conn);
    e.shutdown();

    // Without hardware encoders there is nothing to force.
    let plain = engine(
        dir.path(),
        Some(services(
            Arc::new(fake.clone()),
            ProbeRenderer::new(Arc::new(LateSource::default()) as _).shared(),
        )),
    );
    let conn = agent(&plain);
    timeline(&conn, &fake, &paths);
    let mut params = small(&dir.file("nvenc.mp4"));
    params["hw"] = json!({ "force": "h264_nvenc" });
    match conn.call(Call::new("export.start", params)) {
        Err(CmdError::Invalid { path, msg }) => {
            assert_eq!(path, "/hw/force");
            assert!(msg.contains("no hardware"), "{msg}");
        }
        other => panic!("expected Invalid at /hw/force, got {other:?}"),
    }
    drop(conn);
    plain.shutdown();
}

#[test]
fn a_hardware_failure_mid_export_starts_over_in_software() {
    let dir = TempDir::new("hw-fallback");
    let (fake, paths) = media(&dir);
    let pseudo = PseudoHw::new(fake.clone(), Some(60));
    let seen: Seen = Arc::default();
    let e = hw_engine(&dir, pseudo.clone(), &seen);
    let mut events = e.handle().subscribe();
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    let started = must(&conn, "export.start", small(&dir.file("out.mp4")));
    let info = wait(&conn, &started["job_id"], WAIT);
    assert_succeeded(&info);
    assert_eq!(
        info.progress.message.as_deref(),
        Some(
            "exported 150 frames (5.00 s) (test_hw failed during the export, so it was encoded \
             again in software)"
        )
    );
    assert_eq!(pseudo.opened(), [HwEncode::Auto, HwEncode::Off]);

    // Rendered again from frame 0: the hardware attempt saw frames 0..=60 (it failed pushing
    // frame 60), the software one all 150.
    let indices: Vec<u32> = seen.lock().unwrap().iter().map(|s| s.0).collect();
    let restart = indices
        .windows(2)
        .position(|w| w[1] <= w[0])
        .expect("the frames start over")
        + 1;
    assert_eq!(indices[..restart], (0..=60).collect::<Vec<u32>>()[..]);
    assert_eq!(indices[restart..], (0..150).collect::<Vec<u32>>()[..]);
    // Only the software attempt finished a file, with everything in it.
    let encodes = fake.encodes();
    assert_eq!(encodes.len(), 1);
    assert_eq!(encodes[0].1.video_frames, 150);
    assert_eq!(encodes[0].1.audio_samples, 5 * 48_000);

    // Progress went back to the start, saying why.
    let id = started["job_id"].as_str().unwrap().to_owned();
    let progress: Vec<(f64, String)> = export_support::drain(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            cutweave_engine::EngineEvent::Job(j) if j.job().id.to_string() == id => {
                let p = &j.job().progress;
                Some((p.fraction, p.message.clone().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    let again = progress
        .iter()
        .position(|(_, m)| m.contains("again in software: test_hw failed"))
        .expect("a retry status line");
    assert!(
        progress[..again].iter().any(|(f, _)| *f >= 0.3),
        "{progress:?}"
    );
    assert!(progress[again].0 < 0.05, "{progress:?}");
    drop(conn);
    e.shutdown();
}

#[test]
fn a_forced_hardware_encoder_that_fails_does_not_fall_back() {
    let dir = TempDir::new("hw-forced");
    let (fake, paths) = media(&dir);
    let pseudo = PseudoHw::new(fake.clone(), Some(30));
    let seen: Seen = Arc::default();
    let e = hw_engine(&dir, pseudo.clone(), &seen);
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    let mut params = small(&dir.file("out.mp4"));
    params["hw"] = json!({ "force": PSEUDO_HW });
    let started = must(&conn, "export.start", params);
    let info = wait(&conn, &started["job_id"], WAIT);
    assert_eq!(info.state, JobState::Failed, "{info:?}");
    assert_eq!(info.error_code.as_deref(), Some("encoder_failed"));
    let error = info.error.unwrap_or_default();
    assert!(error.contains("device lost"), "{error}");
    assert_eq!(pseudo.opened(), [HwEncode::Force(PSEUDO_HW.into())]);
    assert_eq!(fake.encodes().len(), 0);
    drop(conn);
    e.shutdown();
}
