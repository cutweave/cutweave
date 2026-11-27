// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.02 on a full engine over `FakeBackend`, behind a file-open spy (`SpyBackend` notes
//! every file the engine asks the media backend to read):
//!
//! - **An export of three clips opens no proxy file**, although all three have a ready proxy
//!   and the preview was just reading one: 0 reads under the proxy cache, and every original is
//!   opened for its pictures and its sound.
//! - **The preview's toggle** (`view.set_preview_quality{use_proxies}`, the app's: an agent is
//!   refused): on, the frame on screen is decoded from the proxy and `playback.state` says so
//!   (`proxy`); off, the same frame is decoded from the original.
//! - **The resolution rule:** a preview that draws the clip larger than its proxy shows the
//!   original.
//! - **The choice outlives the project:** the playback of the next project opens with it.
//! - **Hot swap and fallback:** a proxy that becomes ready while its file is open is used from
//!   the next seek; a deleted (or evicted) proxy falls back to the original at the next seek.
//!
//! The same export through real FFmpeg, with frame hashes of the exported files, is
//! `proxy_sources_ffmpeg.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod engine_support;
mod full_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cutweave_api::{Credential, Token};
use cutweave_engine::full::BackendProbe;
use cutweave_engine::{Connection, Engine, EngineMode, FullHandle, FullSubsystems};
use cutweave_media::testing::{
    FakeAudio, FakeBackend, FakeMedia, FakeVideo, OpenKind, OpenLog, SpyBackend,
};
use cutweave_media::{Capabilities, CodecSupport, MediaBackend};
use cutweave_model::{Duration, FrameRate, SampleRate, StreamKind};
use cutweave_ops::media::{MediaProbe, ProbeError, Probed};
use engine_support::TempDir;
use full_support::{Clocks, SEC, call, engine_config, eventually, full_config};
use serde_json::{Value, json};

/// Probes with the backend and reports the picture stream as H.264, like a real long-GOP file.
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
    handle: FullHandle,
    clocks: Clocks,
    ui: Connection,
    agent: Connection,
    clips: Vec<PathBuf>,
    log: OpenLog,
    project: Value,
}

/// A GUI engine whose backend makes proxies and exports, spied on, with `clips` 1 s clips of
/// 1024 × 576 on disk (small, so the software GPU of the tests exports them quickly; their
/// proxies are 960 × 540) in a project that makes a proxy of every video.
fn rig(tag: &str, clips: usize) -> Rig {
    let dir = TempDir::new(tag);
    let d = Duration::from_secs(1).unwrap();
    let mut fake = FakeBackend::new()
        .with_capabilities(Capabilities::with_codecs([
            codec("rawvideo", StreamKind::Video),
            codec("pcm_f32le", StreamKind::Audio),
            codec("prores", StreamKind::Video),
            codec("pcm_s16le", StreamKind::Audio),
            codec("h264", StreamKind::Video),
            codec("aac", StreamKind::Audio),
        ]))
        .with_output_files();
    let mut paths = Vec::new();
    for n in 0..clips {
        let path = dir.path().join(format!("clip-{n}.mp4"));
        // Different bytes: three files, three content hashes, three proxies.
        std::fs::write(&path, format!("fake media {tag} {n}")).unwrap();
        fake = fake.with_media(
            &path,
            FakeMedia {
                video: Some(FakeVideo::lasting(1024, 576, FrameRate::FPS_30, d)),
                audio: Some(FakeAudio::tone(SampleRate::HZ_48_000, 2, d)),
            },
        );
        paths.push(path);
    }
    let spy = SpyBackend::new(Arc::new(fake));
    let log = spy.log();
    let backend: Arc<dyn MediaBackend> = Arc::new(spy);
    let clocks = Clocks::new();
    let mut config = full_config(dir.path(), clocks.mode());
    config.media = Some(Arc::clone(&backend));
    config.probe = Some(Arc::new(H264Probe(BackendProbe::new(backend))));
    let full = FullSubsystems::new(config);
    let handle = full.handle();
    let mut config = engine_config(dir.path(), EngineMode::Gui, Box::new(full));
    config.ui_token = Some(Token::parse(&"u".repeat(48)).unwrap());
    let engine = Engine::start(config).unwrap();
    eventually("the GPU", || handle.gpu_settled().then_some(()));
    let ui = engine.handle().connect(&Credential::Ui).unwrap();
    let agent = engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap();
    let project = call(&ui, "project.new", json!({ "name": "Proxy sources" })).unwrap();
    call(&ui, "media.set_proxy_policy", json!({ "policy": "all" })).unwrap();
    // A preview canvas smaller than a proxy (960 x 540), as a docked preview is.
    call(
        &ui,
        "view.set_preview_size",
        json!({ "width": 640, "height": 360 }),
    )
    .unwrap();
    Rig {
        dir,
        engine,
        handle,
        clocks,
        ui,
        agent,
        clips: paths,
        log,
        project,
    }
}

impl Rig {
    /// Where the cache keeps proxies.
    fn proxies_dir(&self) -> PathBuf {
        self.dir.path().join("cache").join("proxies")
    }

    /// Imports every clip and waits for its analysis; the media ids.
    fn import(&self) -> Vec<Value> {
        let paths: Vec<String> = self
            .clips
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let imported = call(&self.ui, "media.import", json!({ "paths": paths })).unwrap();
        assert_eq!(imported["failed"], json!([]), "{imported}");
        imported["media_ids"].as_array().unwrap().clone()
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

    fn wait_proxy(&self, media: &Value, want: &str) -> Value {
        eventually(&format!("proxy {want}"), || {
            let s = self.status(media);
            (s["status"] == want).then_some(s)
        })
    }

    /// Places each media item (1 s) one after the other on the video track.
    fn place(&self, media: &[Value]) {
        for (n, id) in media.iter().enumerate() {
            call(
                &self.ui,
                "timeline.insert",
                json!({
                    "sequence": self.project["sequence"],
                    "track": self.project["video_track"],
                    "media": id,
                    "at": i64::try_from(n).unwrap() * SEC,
                    "duration": "1s",
                }),
            )
            .unwrap();
        }
    }

    /// The playback state (the proxy command without a change answers with it).
    fn state(&self) -> Value {
        call(&self.ui, "view.set_preview_quality", json!({})).unwrap()
    }

    /// Seeks to `t` and waits until the full-quality frame there is on screen; the state then.
    fn show(&self, t: i64) -> Value {
        let before = full_support::delivered(&self.handle);
        call(&self.ui, "view.seek", json!({ "t": t })).unwrap();
        eventually(&format!("the frame after the seek to {t}"), || {
            // The clocks are the test's: let the scrub's idle time pass, so it refines.
            self.clocks.advance(SEC / 5);
            let s = self.state();
            (s["state"] == "stopped"
                && s["t"] == json!(t)
                && full_support::delivered(&self.handle) > before)
                .then_some(s)
        })
    }

    /// The decoder opens of pictures so far, as (is a proxy file, file name).
    fn video_opens(&self) -> Vec<(bool, String)> {
        let proxies = self.proxies_dir();
        self.log
            .opens()
            .into_iter()
            .filter(|o| o.kind == OpenKind::Video)
            .map(|o| {
                (
                    o.path.starts_with(&proxies),
                    o.path.file_name().unwrap().to_string_lossy().into_owned(),
                )
            })
            .collect()
    }
}

fn is_original(path: &Path, clips: &[PathBuf]) -> bool {
    clips.iter().any(|c| c == path)
}

#[test]
fn an_export_of_three_clips_opens_no_proxy_file() {
    let r = rig("proxy-sources-export", 3);
    let media = r.import();
    assert_eq!(media.len(), 3);
    let mut proxy_files = Vec::new();
    for id in &media {
        let ready = r.wait_proxy(id, "ready");
        assert_eq!(ready["size"], json!([960, 540]), "{ready}");
        proxy_files.push(PathBuf::from(ready["path"].as_str().unwrap()));
    }
    assert!(proxy_files.iter().all(|p| p.starts_with(r.proxies_dir())));
    r.place(&media);

    // The preview reads proxies: a frame of the second clip is on screen from its proxy (the
    // preview is 640 x 360, smaller than the proxy), and the spy saw that open.
    let shown = r.show(SEC + SEC / 2);
    assert_eq!(shown["use_proxies"], json!(true), "{shown}");
    assert_eq!(shown["proxy"], json!(true), "{shown}");
    assert!(
        r.video_opens().iter().any(|(proxy, _)| *proxy),
        "the spy sees the preview's proxy: {:?}",
        r.video_opens()
    );

    // The export: every clip, pictures and sound.
    r.log.clear();
    let out = r.dir.path().join("out.mp4");
    let export = call(
        &r.agent,
        "export.start",
        json!({
            "path": out.to_string_lossy(),
            "preset": { "custom": { "width": 320, "height": 180 } },
        }),
    )
    .unwrap();
    // The software GPU is slow on a busy machine: wait in turns, up to five minutes.
    let mut done = Value::Null;
    for _ in 0..5 {
        done = call(
            &r.agent,
            "job.wait",
            json!({ "job_id": export["job_id"], "timeout_ms": 60_000 }),
        )
        .unwrap();
        if done["state"] != "running" && done["state"] != "queued" {
            break;
        }
    }
    assert_eq!(done["state"], "succeeded", "{done}");

    let opens = r.log.opens();
    let under_proxies = r.log.under(&r.proxies_dir());
    assert_eq!(
        under_proxies,
        Vec::new(),
        "an export reads no file of the proxy cache"
    );
    assert!(
        opens.iter().all(|o| !proxy_files.contains(&o.path)),
        "{opens:?}"
    );
    // It did read: each original was opened for its pictures and for its sound, nothing else.
    let decoders = r.log.decoder_opens();
    assert!(
        decoders.iter().all(|o| is_original(&o.path, &r.clips)),
        "{decoders:?}"
    );
    for clip in &r.clips {
        for kind in [OpenKind::Video, OpenKind::Audio] {
            assert!(
                decoders.iter().any(|o| o.kind == kind && &o.path == clip),
                "{kind:?} of {} was not read: {decoders:?}",
                clip.display()
            );
        }
    }
    // The proxies are still there and still the preview's.
    for id in &media {
        assert_eq!(r.status(id)["status"], "ready");
    }
    let shown = r.show(2 * SEC + SEC / 2);
    assert_eq!(shown["proxy"], json!(true), "{shown}");
    drop((r.ui, r.agent));
    r.engine.shutdown();
}

#[test]
fn the_toggle_turns_the_previews_proxies_off_and_a_new_or_deleted_proxy_swaps_at_the_next_seek() {
    let r = rig("proxy-sources-toggle", 1);
    // No proxy at first: the project makes none, so the clip opens on its original.
    call(&r.ui, "media.set_proxy_policy", json!({ "policy": "off" })).unwrap();
    let media = r.import();
    let id = &media[0];
    r.place(&media);
    let shown = r.show(SEC / 10);
    assert_eq!(
        (shown["use_proxies"].clone(), shown["proxy"].clone()),
        (json!(true), json!(false)),
        "{shown}"
    );
    assert!(r.video_opens().iter().all(|(proxy, _)| !*proxy));

    // Hot swap: the proxy finishes while the file is open; the next seek uses it.
    call(&r.agent, "media.generate_proxy", json!({ "media": [id] })).unwrap();
    r.wait_proxy(id, "ready");
    let shown = r.show(SEC / 2);
    assert_eq!(shown["proxy"], json!(true), "{shown}");
    assert!(r.video_opens().iter().any(|(proxy, _)| *proxy));

    // The toggle: off re-renders the frame on screen from the original, on from the proxy.
    // It is this computer's preference: the app's, never an agent's.
    let err = call(
        &r.agent,
        "view.set_preview_quality",
        json!({ "use_proxies": false }),
    )
    .unwrap_err();
    assert_eq!(err.code(), "forbidden", "{err}");
    let off = call(
        &r.ui,
        "view.set_preview_quality",
        json!({ "use_proxies": false }),
    )
    .unwrap();
    assert_eq!(off["use_proxies"], json!(false), "{off}");
    let state = eventually("the original on screen", || {
        let s = r.state();
        (s["proxy"] == json!(false)).then_some(s)
    });
    assert_eq!(
        state["t"],
        json!(SEC / 2),
        "the playhead did not move: {state}"
    );
    // Seeks and playback stay on the original while it is off.
    let shown = r.show(SEC / 2 + SEC / 10);
    assert_eq!(shown["proxy"], json!(false), "{shown}");
    call(
        &r.ui,
        "view.set_preview_quality",
        json!({ "use_proxies": true }),
    )
    .unwrap();
    eventually("the proxy on screen again", || {
        (r.state()["proxy"] == json!(true)).then_some(())
    });

    // The resolution rule: a preview that draws the clip larger than its proxy (960 x 540)
    // shows the original; back at a size the proxy covers, the proxy.
    let size = |width: u32, height: u32| {
        call(
            &r.ui,
            "view.set_preview_size",
            json!({ "width": width, "height": height }),
        )
        .unwrap();
    };
    size(1920, 1080);
    eventually("the original at full resolution", || {
        r.clocks.advance(SEC / 5);
        (r.state()["proxy"] == json!(false)).then_some(())
    });
    assert_eq!(r.show(SEC / 5)["proxy"], json!(false));
    size(960, 540);
    eventually("the proxy at its own size", || {
        r.clocks.advance(SEC / 5);
        (r.state()["proxy"] == json!(true)).then_some(())
    });

    // Fallback: the proxy is deleted (an eviction does the same); the next seek shows the
    // original, with no error.
    let deleted = call(&r.agent, "media.delete_proxy", json!({ "media": [id] })).unwrap();
    assert_eq!(deleted["proxies"][0]["status"], "evicted", "{deleted}");
    let shown = r.show(SEC / 3);
    assert_eq!(shown["proxy"], json!(false), "{shown}");
    assert_eq!(shown["use_proxies"], json!(true));

    // The choice is this computer's, not the project's: the next project (the untitled one
    // that follows a close, with its own playback) opens with it.
    call(
        &r.ui,
        "view.set_preview_quality",
        json!({ "use_proxies": false }),
    )
    .unwrap();
    let closed = call(&r.ui, "project.close", json!({ "save": false })).unwrap();
    assert_eq!(
        closed["project"]["rev"],
        json!(0),
        "a new document: {closed}"
    );
    let state = eventually("the new project's playback", || {
        call(&r.ui, "view.set_preview_quality", json!({})).ok()
    });
    assert_eq!(state["use_proxies"], json!(false), "{state}");
    drop((r.ui, r.agent));
    r.engine.shutdown();
}
