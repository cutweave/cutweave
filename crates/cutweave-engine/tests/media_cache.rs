// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1D.08: the cache and decoding commands behind the settings pages "Cache and storage" and
//! "Performance and hardware" (arch §7, §16; ADR-0016), on a full engine over `FakeBackend`.
//!
//! - A lower thumbnail limit shrinks the disk cache at once; clearing empties it and the next
//!   thumbnail request regenerates the image; clearing leaves projects and generated media.
//! - `media.set_cache_config`, `media.clear_cache` and `media.set_decode_policy` need the UI
//!   credential (an agent gets `forbidden`); `media.cache_status` answers agents.
//! - With hardware decoding off, decoders opened afterwards are software ones (a decoder-kind
//!   probe in `FakeBackend`), and the open pools reopen on their next seek.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod engine_support;
mod full_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use cutweave_api::{Credential, Token};
use cutweave_cache::{DiskKind, MediaRef};
use cutweave_engine::{Connection, Engine, EngineMode, FullSubsystems};
use cutweave_media::CancelToken;
use cutweave_media::testing::{FakeAudio, FakeBackend, FakeMedia, FakeVideo};
use cutweave_model::{Duration, FrameRate, SampleRate, Time};
use engine_support::TempDir;
use full_support::{Clocks, call, delivered, engine_config, eventually, full_config};
use serde_json::{Value, json};

const MB: u64 = 1024 * 1024;

/// A 3 s 30 fps 320x180 clip with a stereo tone, as a file (only hashed) and in `FakeBackend`.
fn fake_clip(dir: &Path, fake: FakeBackend) -> (FakeBackend, PathBuf) {
    let path = dir.join("a.mp4");
    std::fs::write(&path, "fake media a.mp4").unwrap();
    let d = Duration::from_secs(3).unwrap();
    let media = FakeMedia {
        video: Some(FakeVideo::lasting(320, 180, FrameRate::FPS_30, d)),
        audio: Some(FakeAudio::tone(SampleRate::HZ_48_000, 2, d)),
    };
    (fake.with_media(&path, media), path)
}

struct Rig {
    _dir: TempDir,
    engine: Engine,
    ui: Connection,
    agent: Connection,
    fake: FakeBackend,
    clip: PathBuf,
    handle: cutweave_engine::FullHandle,
    clocks: Clocks,
}

fn rig(tag: &str, pseudo_hw: bool) -> Rig {
    let dir = TempDir::new(tag);
    let mut fake = FakeBackend::new();
    if pseudo_hw {
        fake = fake.with_pseudo_hw_decode();
    }
    let (fake, clip) = fake_clip(dir.path(), fake);
    let clocks = Clocks::new();
    let config = full_config(dir.path(), clocks.mode()).with_media(Arc::new(fake.clone()));
    let full = FullSubsystems::new(config);
    let handle = full.handle();
    let mut config = engine_config(dir.path(), EngineMode::Gui, Box::new(full));
    config.ui_token = Some(Token::parse(&"u".repeat(48)).unwrap());
    let engine = Engine::start(config).unwrap();
    let ui = engine.handle().connect(&Credential::Ui).unwrap();
    let agent = engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap();
    Rig {
        _dir: dir,
        engine,
        ui,
        agent,
        fake,
        clip,
        handle,
        clocks,
    }
}

impl Rig {
    /// A project with the clip imported and inserted; waits for the analysis jobs.
    fn build_timeline(&self) -> Value {
        let project = call(&self.ui, "project.new", json!({ "name": "Cache" })).unwrap();
        let imported = call(
            &self.ui,
            "media.import",
            json!({ "paths": [self.clip.to_string_lossy()] }),
        )
        .unwrap();
        assert_eq!(imported["failed"], json!([]), "{imported}");
        for job in imported["job_ids"].as_array().unwrap() {
            let info = call(
                &self.ui,
                "job.wait",
                json!({ "job_id": job, "timeout_ms": 30_000 }),
            )
            .unwrap();
            assert_eq!(info["state"], "succeeded", "{info}");
        }
        call(
            &self.ui,
            "timeline.insert",
            json!({
                "sequence": project["sequence"], "track": project["video_track"],
                "media": imported["media_ids"][0], "at": 0, "duration": "2s",
            }),
        )
        .unwrap();
        imported
    }

    fn status(&self) -> Value {
        call(&self.agent, "media.cache_status", json!({})).unwrap()
    }

    fn shutdown(self) {
        drop(self.ui);
        drop(self.agent);
        self.engine.shutdown();
    }
}

fn kind<'a>(status: &'a Value, name: &str) -> &'a Value {
    status["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == name)
        .unwrap_or_else(|| panic!("no {name} in {status}"))
}

#[test]
fn the_status_names_the_folder_each_cache_and_the_policy() {
    let r = rig("status", false);
    let status = r.status();
    let folder = status["folder"].as_str().unwrap();
    assert!(folder.ends_with("cache"), "{status}");
    assert_eq!(status["decode_hw"], "auto");
    assert!(status["frame_cache_bytes"].as_u64().unwrap() > 0);
    let thumbs = kind(&status, "thumbnails");
    assert_eq!(thumbs["limit_bytes"], json!(2048 * MB));
    assert_eq!(thumbs["entries"], 0);
    assert_eq!(kind(&status, "waveforms")["limit_bytes"], json!(512 * MB));
    assert_eq!(status["kinds"].as_array().unwrap().len(), 2);
    r.shutdown();
}

/// Acceptance 1: a 50 MB thumbnail limit shrinks the disk cache below it at once; clearing
/// empties it and the next thumbnail request regenerates the image.
#[test]
fn a_lower_limit_shrinks_the_cache_and_clearing_empties_it() {
    let r = rig("shrink", false);
    let imported = r.build_timeline();
    let media_id = imported["media_ids"][0].clone();
    // The poster and filmstrip of the import are in.
    let after_import = r.status();
    assert!(
        kind(&after_import, "thumbnails")["bytes"].as_u64().unwrap() > 0,
        "{after_import}"
    );
    assert!(kind(&after_import, "waveforms")["bytes"].as_u64().unwrap() > 0);

    // Fill the thumbnail cache with 60 MB (twelve 5 MB entries).
    let state = r.engine.handle().media_state().unwrap();
    let thumbs = state.thumbs.clone().unwrap();
    let media = state
        .media(serde_json::from_value(media_id).unwrap())
        .unwrap();
    let reference = MediaRef::from_path(&media.path).unwrap();
    let block = vec![7u8; usize::try_from(5 * MB).unwrap()];
    for i in 0..12 {
        thumbs
            .disk()
            .put(
                DiskKind::Thumb,
                &reference.hash,
                &format!("filler-{i}.jpg"),
                "",
                &block,
            )
            .unwrap();
    }
    let full = kind(&r.status(), "thumbnails")["bytes"].as_u64().unwrap();
    assert!(full > 50 * MB, "{full}");

    let began = Instant::now();
    let after = call(
        &r.ui,
        "media.set_cache_config",
        json!({ "limits": { "thumbnails": 50 * MB } }),
    )
    .unwrap();
    assert!(began.elapsed() < StdDuration::from_secs(1));
    let now = kind(&after, "thumbnails");
    assert_eq!(now["limit_bytes"], json!(50 * MB));
    assert!(now["bytes"].as_u64().unwrap() <= 50 * MB, "{after}");
    assert_eq!(kind(&r.status(), "thumbnails")["bytes"], now["bytes"]);
    // The other cache kept its limit.
    assert_eq!(kind(&after, "waveforms")["limit_bytes"], json!(512 * MB));

    let cleared = call(
        &r.ui,
        "media.clear_cache",
        json!({ "kinds": ["thumbnails"] }),
    )
    .unwrap();
    assert_eq!(cleared["cleared"][0]["kind"], "thumbnails", "{cleared}");
    assert!(cleared["cleared"][0]["removed"].as_u64().unwrap() > 0);
    let status = r.status();
    assert_eq!(kind(&status, "thumbnails")["entries"], 0, "{status}");
    assert_eq!(kind(&status, "thumbnails")["bytes"], 0);
    // Waveforms were not asked for.
    assert!(kind(&status, "waveforms")["bytes"].as_u64().unwrap() > 0);
    // The project is untouched: not dirty, not changed by clearing or limits.
    let info = call(&r.ui, "project.info", json!({})).unwrap();
    assert_ne!(info["dirty"], json!(true), "{info}");

    // The next thumbnail request regenerates the image.
    let thumb = thumbs
        .thumb(
            &reference,
            Time::from_secs(1).unwrap(),
            96,
            &CancelToken::new(),
        )
        .unwrap();
    assert_ne!(thumb.jpeg.len(), 0);
    assert_eq!(kind(&r.status(), "thumbnails")["entries"], 1);

    // Clear everything: both kinds.
    let all = call(
        &r.ui,
        "media.clear_cache",
        json!({ "kinds": ["thumbnails", "waveforms", "thumbnails"] }),
    )
    .unwrap();
    assert_eq!(all["cleared"].as_array().unwrap().len(), 2, "{all}");
    let status = r.status();
    assert_eq!(kind(&status, "waveforms")["entries"], 0, "{status}");
    r.shutdown();
}

#[test]
fn the_waveform_limit_and_frame_memory_apply_at_once() {
    let r = rig("waveforms", false);
    r.build_timeline();
    let before = kind(&r.status(), "waveforms")["bytes"].as_u64().unwrap();
    assert!(before > 0);
    let after = call(
        &r.ui,
        "media.set_cache_config",
        json!({ "limits": { "waveforms": 0 }, "frame_cache_bytes": 512 * MB }),
    )
    .unwrap();
    assert_eq!(kind(&after, "waveforms")["bytes"], 0, "{after}");
    assert_eq!(kind(&after, "waveforms")["limit_bytes"], 0);
    assert_eq!(after["frame_cache_bytes"], json!(512 * MB));
    // 0 is Auto: the smaller of 1 GiB and a quarter of RAM.
    let auto = call(
        &r.ui,
        "media.set_cache_config",
        json!({ "frame_cache_bytes": 0 }),
    )
    .unwrap();
    let auto = auto["frame_cache_bytes"].as_u64().unwrap();
    assert!(auto > 0 && auto <= 1024 * MB, "{auto}");
    // Out-of-range values are refused.
    for bad in [
        json!({ "frame_cache_bytes": 1024 }),
        json!({ "frame_cache_bytes": 1u64 << 40 }),
    ] {
        let err = call(&r.ui, "media.set_cache_config", bad).unwrap_err();
        assert_eq!(err.code(), "invalid", "{err:?}");
    }
    r.shutdown();
}

/// Acceptance 2 (engine level; the RPC, MCP and CLI routes are in `cutweave-api`'s parity test
/// and `cutweave-cli`'s tests): an agent credential is refused the three UI-only commands, and
/// the status stays readable.
#[test]
fn an_agent_cannot_change_the_caches() {
    let r = rig("forbidden", false);
    for (name, params) in [
        ("media.set_cache_config", json!({ "frame_cache_bytes": 0 })),
        ("media.clear_cache", json!({ "kinds": ["thumbnails"] })),
        ("media.set_decode_policy", json!({ "hw": "off" })),
    ] {
        let err = call(&r.agent, name, params).unwrap_err();
        assert_eq!(err.code(), "forbidden", "{name}: {err:?}");
    }
    assert_eq!(r.status()["decode_hw"], "auto");
    r.shutdown();
}

/// Acceptance 3: with hardware decoding off, a decoder opened afterwards is the software one
/// (the fake backend's decoder-kind probe), and the open pool reopens on its next seek.
#[test]
fn hardware_decoding_off_opens_software_decoders() {
    let r = rig("hw", true);
    r.build_timeline();
    let seek = |t: &str| {
        let before = delivered(&r.handle);
        call(&r.ui, "view.seek", json!({ "t": t })).unwrap();
        eventually("the frame of the seek", || {
            (delivered(&r.handle) > before).then_some(())
        });
    };
    seek("0.5s");
    let hw_before = r.fake.hw_decoders_opened();
    assert!(hw_before >= 1, "the preview decodes in hardware by default");
    let _ = &r.clocks;

    let set = call(&r.ui, "media.set_decode_policy", json!({ "hw": "off" })).unwrap();
    assert_eq!(set, json!({ "hw": "off" }));
    assert_eq!(r.status()["decode_hw"], "off");
    assert_eq!(r.fake.hw_decoders_opened(), hw_before);

    let opened = r.fake.decoders_opened();
    seek("1.5s");
    eventually("a decoder opened under the new policy", || {
        (r.fake.decoders_opened() > opened).then_some(())
    });
    assert_eq!(
        r.fake.hw_decoders_opened(),
        hw_before,
        "no hardware decoder is opened while hardware decoding is off"
    );

    // The thumbnail generator follows the policy too.
    let state = r.engine.handle().media_state().unwrap();
    let thumbs = state.thumbs.clone().unwrap();
    let reference = MediaRef::from_path(&r.clip).unwrap();
    let thumb_at = |s: i64, w: u32| {
        thumbs
            .thumb(
                &reference,
                Time::from_secs(s).unwrap(),
                w,
                &CancelToken::new(),
            )
            .unwrap();
    };
    thumb_at(2, 61);
    assert_eq!(r.fake.hw_decoders_opened(), hw_before);

    // And back on: the pool reopens a hardware decoder at its next seek.
    call(&r.ui, "media.set_decode_policy", json!({ "hw": "auto" })).unwrap();
    seek("2.5s");
    eventually("a hardware decoder again", || {
        (r.fake.hw_decoders_opened() > hw_before).then_some(())
    });
    let with_seek = r.fake.hw_decoders_opened();
    thumb_at(1, 63);
    assert!(r.fake.hw_decoders_opened() > with_seek);
    r.shutdown();
}

#[test]
fn media_capabilities_names_the_gpu_in_use() {
    let r = rig("gpu", false);
    eventually("the GPU to settle", || r.handle.gpu_settled().then_some(()));
    let caps = call(&r.agent, "media.capabilities", json!({})).unwrap();
    if r.handle.has_gpu() {
        assert!(
            caps["gpu"]["name"].as_str().is_some_and(|n| !n.is_empty()),
            "{caps}"
        );
        assert!(caps["gpu"]["software"].is_boolean());
    } else {
        // The CPU renderer: no adapter to name.
        assert!(caps.get("gpu").is_none(), "{caps}");
    }
    r.shutdown();
}
