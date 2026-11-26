// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.01 on a full engine over `FakeBackend` (whose "encodes" write small files): the proxy
//! commands and how proxies are made and kept.
//!
//! - `media.import` of a long-GOP H.264 clip larger than 1920 pixels queues a proxy job after
//!   the asset's `index` job (`proxy: "queued"`); the proxy becomes ready at 960 × 540 and
//!   shows in `media.proxy_status` and in `media.cache_status` (kind `proxies`).
//! - `media.delete_proxy` evicts it (and it is not made again automatically);
//!   `media.generate_proxy` makes it again; `media.clear_cache{proxies}` and a lower
//!   `limits.proxies` evict too. Agents may read and make proxies; clearing is the UI's.
//! - An interrupted generation (cancelled) resumes when the project is opened again.
//! - WP-2D.01b: reversing a clip of a 1080p long-GOP file (which `auto` gives no proxy) queues
//!   one, announced by `media.reverse_slow` and reported with its reason by
//!   `media.proxy_status`. (A file whose GOPs fit the preview's read-back gets none: the unit
//!   tests of `full::proxy`.)

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod engine_support;
mod full_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use cutweave_api::{Credential, Token};
use cutweave_engine::full::BackendProbe;
use cutweave_engine::{Connection, Engine, EngineMode, FullSubsystems};
use cutweave_media::testing::{FakeAudio, FakeBackend, FakeMedia, FakeVideo};
use cutweave_media::{Capabilities, CodecSupport};
use cutweave_model::{Duration, FrameRate, SampleRate, StreamKind};
use cutweave_ops::media::{MediaProbe, ProbeError, Probed};
use engine_support::TempDir;
use full_support::{Clocks, call, drain, engine_config, eventually, full_config};
use serde_json::{Value, json};

/// Probes with the fake backend and reports the picture stream as H.264 (the fake's own codec
/// name would not qualify for `auto`).
struct H264Probe(BackendProbe);

impl MediaProbe for H264Probe {
    fn probe(&self, path: &str) -> Result<Probed, ProbeError> {
        let mut probed = self.0.probe(path)?;
        for s in &mut probed.probe.streams {
            if s.kind == StreamKind::Video {
                s.codec = Some("h264".into());
            }
        }
        Ok(probed)
    }
}

fn codec(name: &str, kind: StreamKind) -> CodecSupport {
    CodecSupport {
        name: name.to_owned(),
        kind,
        decoders: vec![name.to_owned()],
        encoders: vec![name.to_owned()],
    }
}

struct Rig {
    dir: TempDir,
    engine: Engine,
    ui: Connection,
    agent: Connection,
    clip: PathBuf,
}

/// An engine whose backend makes proxies, with a 2560×1440 clip of `secs` seconds (frames cost
/// `cost` each to decode).
fn rig(tag: &str, secs: i64, cost: StdDuration) -> Rig {
    let d = Duration::from_secs(secs).unwrap();
    rig_with(
        tag,
        FakeVideo::lasting(2560, 1440, FrameRate::FPS_30, d),
        cost,
    )
}

/// An engine whose backend makes proxies, with a clip of `video` (frames cost `cost` each).
fn rig_with(tag: &str, video: FakeVideo, cost: StdDuration) -> Rig {
    let dir = TempDir::new(tag);
    let clip = dir.path().join("wide.mp4");
    std::fs::write(&clip, format!("fake media {tag}")).unwrap();
    let d = video.duration().unwrap();
    let fake = FakeBackend::new()
        .with_capabilities(Capabilities::with_codecs([
            codec("rawvideo", StreamKind::Video),
            codec("pcm_f32le", StreamKind::Audio),
            codec("prores", StreamKind::Video),
            codec("pcm_s16le", StreamKind::Audio),
        ]))
        .with_output_files()
        .with_frame_cost(cost)
        .with_media(
            &clip,
            FakeMedia {
                video: Some(video),
                audio: Some(FakeAudio::tone(SampleRate::HZ_48_000, 2, d)),
            },
        );
    let backend: Arc<FakeBackend> = Arc::new(fake);
    let clocks = Clocks::new();
    let mut config = full_config(dir.path(), clocks.mode()).with_media(backend.clone());
    config.probe = Some(Arc::new(H264Probe(BackendProbe::new(backend))));
    let full = FullSubsystems::new(config);
    let mut config = engine_config(dir.path(), EngineMode::Gui, Box::new(full));
    config.ui_token = Some(Token::parse(&"u".repeat(48)).unwrap());
    let engine = Engine::start(config).unwrap();
    let ui = engine.handle().connect(&Credential::Ui).unwrap();
    let agent = engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap();
    Rig {
        dir,
        engine,
        ui,
        agent,
        clip,
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

impl Rig {
    fn import(&self) -> Value {
        call(&self.ui, "project.new", json!({ "name": "Proxies" })).unwrap();
        call(
            &self.ui,
            "media.import",
            json!({ "paths": [path_str(&self.clip)] }),
        )
        .unwrap()
    }

    fn status(&self, media: &Value) -> Value {
        call(
            &self.agent,
            "media.proxy_status",
            json!({ "media": [media] }),
        )
        .unwrap()["proxies"][0]
            .clone()
    }

    fn wait_status(&self, media: &Value, want: &str) -> Value {
        eventually(&format!("proxy {want}"), || {
            let s = self.status(media);
            (s["status"] == want).then_some(s)
        })
    }

    fn proxy_job(&self, media: &Value) -> Value {
        let jobs = call(
            &self.agent,
            "job.list",
            json!({ "media_id": media, "kind": "proxy" }),
        )
        .unwrap();
        jobs["jobs"][0].clone()
    }

    fn proxies_kind(&self) -> Value {
        let status = call(&self.agent, "media.cache_status", json!({})).unwrap();
        status["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["kind"] == "proxies")
            .cloned()
            .unwrap_or(Value::Null)
    }
}

#[test]
fn an_imported_4k_class_clip_gets_a_proxy_that_the_commands_manage() {
    let r = rig("proxies-commands", 3, StdDuration::ZERO);
    let imported = r.import();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    assert_eq!(imported["media"][0]["proxy"], "queued", "{imported}");
    let media = imported["media_ids"][0].clone();
    let job = r.proxy_job(&media);
    assert_eq!(job["label"], "proxy");
    let done = call(
        &r.ui,
        "job.wait",
        json!({ "job_id": job["id"], "timeout_ms": 30_000 }),
    )
    .unwrap();
    assert_eq!(done["state"], "succeeded", "{done}");
    let ready = r.status(&media);
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["wanted"], true);
    assert_eq!(ready["profile"], "prores_proxy-960");
    assert_eq!(ready["size"], json!([960, 540]));
    assert!(Path::new(ready["path"].as_str().unwrap()).is_file());
    let kind = r.proxies_kind();
    assert_eq!(kind["entries"], 1, "{kind}");
    assert_eq!(kind["limit_bytes"], 50_u64 << 30);
    // Every video asset when none is named.
    let all = call(&r.agent, "media.proxy_status", json!({})).unwrap();
    assert_eq!(all["proxies"].as_array().unwrap().len(), 1);
    assert_eq!(all["policy"], "auto");

    // Deleting: the original is used again, and nothing makes it again on its own.
    let deleted = call(&r.agent, "media.delete_proxy", json!({ "media": [media] })).unwrap();
    assert_eq!(deleted["proxies"][0]["status"], "evicted", "{deleted}");
    assert_eq!(r.proxies_kind()["entries"], 0);
    // Making it again, on request.
    let generating = call(
        &r.agent,
        "media.generate_proxy",
        json!({ "media": [media] }),
    )
    .unwrap();
    let status = &generating["proxies"][0]["status"];
    assert!(
        status == "queued" || status == "generating" || status == "ready",
        "{generating}"
    );
    r.wait_status(&media, "ready");
    // A lower limit evicts at once; clearing is the UI's.
    call(
        &r.ui,
        "media.set_cache_config",
        json!({ "limits": { "proxies": 0 } }),
    )
    .unwrap();
    assert_eq!(r.status(&media)["status"], "evicted");
    call(
        &r.ui,
        "media.set_cache_config",
        json!({ "limits": { "proxies": 50_u64 << 30 } }),
    )
    .unwrap();
    call(
        &r.agent,
        "media.generate_proxy",
        json!({ "media": [media] }),
    )
    .unwrap();
    r.wait_status(&media, "ready");
    assert!(
        call(
            &r.agent,
            "media.clear_cache",
            json!({ "kinds": ["proxies"] })
        )
        .is_err(),
        "agents may not clear caches"
    );
    let cleared = call(&r.ui, "media.clear_cache", json!({ "kinds": ["proxies"] })).unwrap();
    assert_eq!(cleared["cleared"][0]["kind"], "proxies", "{cleared}");
    assert_eq!(cleared["cleared"][0]["removed"], 1);
    assert_eq!(r.status(&media)["status"], "evicted");

    // Bad requests.
    let err = call(&r.agent, "media.generate_proxy", json!({ "media": [] })).unwrap_err();
    assert!(err.to_string().contains("1..=256"), "{err}");
    let twice = call(
        &r.agent,
        "media.generate_proxy",
        json!({ "media": [media, media] }),
    )
    .unwrap_err();
    assert!(twice.to_string().contains("twice"), "{twice}");
    let _ = r.dir.path();
}

#[test]
fn an_interrupted_proxy_resumes_when_the_project_opens_again() {
    // Slow frames so the generation can be stopped part-way.
    let r = rig("proxies-resume", 10, StdDuration::from_millis(4));
    let imported = r.import();
    let media = imported["media_ids"][0].clone();
    assert_eq!(imported["media"][0]["proxy"], "queued");
    let job = eventually("the proxy job runs", || {
        let job = r.proxy_job(&media);
        (job["state"] == "running" && job["progress"]["fraction"].as_f64() > Some(0.1))
            .then_some(job)
    });
    call(&r.ui, "job.cancel", json!({ "id": job["id"] })).unwrap();
    let cancelled = call(
        &r.ui,
        "job.wait",
        json!({ "job_id": job["id"], "timeout_ms": 30_000 }),
    )
    .unwrap();
    assert_eq!(cancelled["state"], "cancelled", "{cancelled}");
    assert_eq!(r.status(&media)["status"], "incomplete");
    // Save, close and open the project: the proxy is queued again and completes.
    let folder = r.dir.path().join("Proxies");
    call(
        &r.ui,
        "project.save_as",
        json!({ "path": path_str(&folder) }),
    )
    .unwrap();
    // (Saving into the folder swaps the document, which already queues it again; closing the
    // project cancels every job.)
    call(&r.ui, "project.close", json!({})).unwrap();
    let before = r.proxy_job(&media)["id"].clone();
    call(&r.ui, "project.open", json!({ "path": path_str(&folder) })).unwrap();
    let resumed = eventually("the resumed proxy job", || {
        let job = r.proxy_job(&media);
        (job["id"] != before && job["id"] != cancelled["id"] && !job.is_null()).then_some(job)
    });
    let done = call(
        &r.ui,
        "job.wait",
        json!({ "job_id": resumed["id"], "timeout_ms": 60_000 }),
    )
    .unwrap();
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(r.status(&media)["status"], "ready");
}

#[test]
fn reversing_a_long_gop_clip_queues_its_proxy_and_warns_while_it_builds() {
    // 1080p30 H.264 (to the probe) with a keyframe every 4 s: `auto` makes no proxy, and the
    // preview's read-back holds 64 frames of 1080p, fewer than a GOP.
    let video = FakeVideo::lasting(
        1920,
        1080,
        FrameRate::FPS_30,
        Duration::from_secs(6).unwrap(),
    )
    .with_gop(120);
    let r = rig_with("proxies-reverse", video, StdDuration::ZERO);
    let mut rx = r.engine.handle().subscribe();
    let project = call(&r.ui, "project.new", json!({ "name": "Reverse" })).unwrap();
    let imported = call(
        &r.ui,
        "media.import",
        json!({ "paths": [path_str(&r.clip)] }),
    )
    .unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    assert_eq!(imported["media"][0]["proxy"], "none", "{imported}");
    for job in imported["job_ids"].as_array().unwrap() {
        call(
            &r.ui,
            "job.wait",
            json!({ "job_id": job, "timeout_ms": 30_000 }),
        )
        .unwrap();
    }
    let media = imported["media_ids"][0].clone();
    let inserted = call(
        &r.agent,
        "timeline.insert",
        json!({
            "sequence": project["sequence"], "track": project["video_track"],
            "media": media, "at": 0, "duration": "2s",
        }),
    )
    .unwrap();
    assert_eq!(r.status(&media)["wanted"], false);
    let item = inserted["clips"][0].clone();
    call(&r.agent, "clip.reverse", json!({ "item": item })).unwrap();

    let warned = eventually("media.reverse_slow", || {
        drain(&mut rx)
            .into_iter()
            .find(|e| e.name() == "media.reverse_slow")
            .map(|e| e.payload())
    });
    assert_eq!(warned["media_id"], media, "{warned}");
    assert!(
        warned["reason"]
            .as_str()
            .unwrap()
            .contains("up to 120 frames apart"),
        "{warned}"
    );
    let job = warned["job_id"].clone();
    assert!(job.is_string(), "a proxy job was queued: {warned}");
    let done = call(
        &r.ui,
        "job.wait",
        json!({ "job_id": job, "timeout_ms": 30_000 }),
    )
    .unwrap();
    assert_eq!(done["state"], "succeeded", "{done}");
    let ready = r.status(&media);
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["wanted"], true, "{ready}");
    assert!(
        ready["reason"].as_str().unwrap().contains("reverse"),
        "{ready}"
    );
}
