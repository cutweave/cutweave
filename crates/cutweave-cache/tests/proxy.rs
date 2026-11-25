// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.01 without FFmpeg: the proxy store (states, the size-capped LRU, clearing) and the
//! generator over `FakeBackend` (frame mapping, resuming after a cancel, the job).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as Wall;

use cutweave_cache::proxy::{
    GenerateOptions, ProxyCodec, ProxyGenerator, ProxyProfile, ProxySource, ProxyState, ProxyStore,
    plan_eviction, submit_proxy,
};
use cutweave_cache::{CacheDb, HashMode};
use cutweave_jobs::{JobConfig, JobKind, JobManager, JobState};
use cutweave_media::testing::contract::TempDir;
use cutweave_media::testing::{FakeAudio, FakeMedia, FakeVideo, fake_frame_number};
use cutweave_media::{
    CancelToken, Capabilities, CodecSupport, DecodeOpts, FakeBackend, MediaBackend,
};
use cutweave_model::{FrameRate, SampleRate, StreamKind};

const GB: u64 = 1 << 30;
const MB: u64 = 1 << 20;

fn codec(name: &str, kind: StreamKind) -> CodecSupport {
    CodecSupport {
        name: name.to_owned(),
        kind,
        decoders: vec![name.to_owned()],
        encoders: vec![name.to_owned()],
    }
}

/// A fake backend that "encodes" proxies (ProRes and PCM) into files.
fn backend() -> FakeBackend {
    FakeBackend::new()
        .with_capabilities(Capabilities::with_codecs([
            codec("prores", StreamKind::Video),
            codec("pcm_s16le", StreamKind::Audio),
            codec("rawvideo", StreamKind::Video),
        ]))
        .with_output_files()
}

struct Fixture {
    dir: TempDir,
    backend: FakeBackend,
    db: Arc<CacheDb>,
    store: Arc<ProxyStore>,
    source: ProxySource,
}

/// A 1280×720 clip of `frames` frames at 30 fps with a stereo tone, on disk (for its hash).
fn fixture(tag: &str, frames: u64) -> Fixture {
    let dir = TempDir::new(tag);
    let path = dir.path().join("original.mp4");
    std::fs::write(&path, format!("original {tag}")).unwrap();
    let hash = cutweave_media::content_hash(&path, HashMode::Fast).unwrap();
    let video = FakeVideo::new(1280, 720, FrameRate::FPS_30, frames);
    let tone = FakeAudio::tone(SampleRate::HZ_48_000, 2, video.duration().unwrap());
    let media = FakeMedia {
        video: Some(video),
        audio: Some(tone),
    };
    let probe = media.probe().unwrap();
    let backend = backend().with_media(&path, media);
    let db = Arc::new(CacheDb::open_in_memory().unwrap());
    let store = Arc::new(ProxyStore::new(
        Arc::clone(&db),
        dir.path().join("proxies"),
        ProxyStore::DEFAULT_MAX_BYTES,
    ));
    Fixture {
        source: ProxySource { path, hash, probe },
        dir,
        backend,
        db,
        store,
    }
}

fn generator(f: &Fixture, part_frames: usize) -> ProxyGenerator {
    ProxyGenerator::new(
        Arc::new(f.backend.clone()),
        Arc::clone(&f.db),
        Arc::clone(&f.store),
        GenerateOptions {
            part_frames,
            ..GenerateOptions::default()
        },
    )
}

fn profile() -> ProxyProfile {
    ProxyProfile::new(ProxyCodec::ProRes)
}

#[test]
fn the_lru_plan_evicts_the_least_recently_used_until_the_cap_holds() {
    // Acceptance: a 1 GB cap with three 600 MB proxies keeps only the newest.
    let rows = [("a", 600 * MB, 1), ("b", 600 * MB, 2), ("c", 600 * MB, 3)];
    assert_eq!(plan_eviction(&rows, GB), ["a", "b"]);
    // Touching `a` makes `b` the oldest.
    let touched = [("a", 600 * MB, 4), ("b", 600 * MB, 2), ("c", 600 * MB, 3)];
    assert_eq!(plan_eviction(&touched, 1300 * MB), ["b"]);
    assert_eq!(plan_eviction(&rows, 2 * GB), [] as [&str; 0]);
    assert_eq!(plan_eviction(&rows, 0), ["a", "b", "c"]);
}

#[test]
fn a_proxy_maps_every_frame_to_the_original_frame_with_its_sound() {
    let f = fixture("proxy-gen", 100);
    let report = generator(&f, 30)
        .generate(&f.source, &profile(), &CancelToken::new(), &mut |_| {})
        .unwrap();
    assert_eq!(report.encoded_parts, 4);
    assert_eq!(report.reused_parts, 0);
    assert_eq!(report.proxy.state, ProxyState::Ready);
    assert_eq!(report.proxy.frames, 100);
    assert_eq!(report.proxy.size, (960, 540));
    assert!(report.proxy.path.is_file());
    assert!(report.proxy.bytes > 0);
    // The parts are gone; only the joined file is left.
    let dir = f.store.dir(&f.source.hash, &profile().key());
    assert!(!dir.join("parts").exists());

    let ready = f
        .store
        .ready(&f.source.hash, &profile().key())
        .unwrap()
        .unwrap();
    assert_eq!(ready.index.frame_count(), 100);
    assert_eq!(ready.source_index.frame_count(), 100);
    assert_ne!(ready.identity, f.source.hash);
    let probe = f.backend.probe(&ready.record.path).unwrap();
    assert_eq!(probe.audio.unwrap().channels, 2);
    // Proxy frame n is original frame n (the fake's pixels carry the frame number).
    let mut d = f
        .backend
        .open_decoder(&ready.record.path, &DecodeOpts::default())
        .unwrap();
    for n in [0_usize, 1, 29, 30, 31, 59, 60, 99] {
        let t = ready.source_index.time_of(n).unwrap();
        let (k, shown) = ready.frame_at(t).unwrap();
        assert_eq!((k, shown), (n, t));
        let frame = d.seek_exact(ready.index.time_of(k).unwrap()).unwrap();
        assert_eq!(fake_frame_number(&frame), u32::try_from(n).ok());
    }
    // A second generation finds it ready.
    let again = generator(&f, 30)
        .generate(&f.source, &profile(), &CancelToken::new(), &mut |_| {})
        .unwrap();
    assert_eq!((again.encoded_parts, again.reused_parts), (0, 0));
}

#[test]
fn a_cancelled_generation_keeps_its_parts_and_the_next_one_resumes() {
    let f = fixture("proxy-resume", 120);
    let cancel = CancelToken::new();
    let token = cancel.clone();
    let err = generator(&f, 20)
        .generate(&f.source, &profile(), &cancel, &mut |p| {
            if p >= 0.5 {
                token.cancel();
            }
        })
        .unwrap_err();
    assert!(
        matches!(
            err,
            cutweave_cache::CacheError::Media(cutweave_media::MediaError::Cancelled)
        ),
        "{err:?}"
    );
    let row = f
        .store
        .get(&f.source.hash, &profile().key())
        .unwrap()
        .unwrap();
    assert_eq!(row.state, ProxyState::Building);
    let parts: Vec<PathBuf> =
        std::fs::read_dir(f.store.dir(&f.source.hash, &profile().key()).join("parts"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
    assert!(
        parts.len() >= 3 && parts.len() < 6,
        "finished parts are kept: {parts:?}"
    );
    assert!(
        parts
            .iter()
            .all(|p| p.extension().is_some_and(|e| e == "mov")),
        "no partial files: {parts:?}"
    );
    // A new backend (a restarted engine) continues after the finished parts.
    let resumed = generator(&f, 20)
        .generate(&f.source, &profile(), &CancelToken::new(), &mut |_| {})
        .unwrap();
    assert_eq!(resumed.reused_parts, parts.len());
    assert_eq!(resumed.encoded_parts, 6 - parts.len());
    assert_eq!(resumed.proxy.frames, 120);
    let ready = f
        .store
        .ready(&f.source.hash, &profile().key())
        .unwrap()
        .unwrap();
    let mut d = FakeBackend::new()
        .open_decoder(&ready.record.path, &DecodeOpts::default())
        .unwrap();
    for n in [0_usize, 59, 60, 61, 119] {
        let frame = d.seek_exact(ready.index.time_of(n).unwrap()).unwrap();
        assert_eq!(fake_frame_number(&frame), u32::try_from(n).ok());
    }
}

#[test]
fn the_cap_evicts_the_least_recently_used_proxy_which_then_reads_as_evicted() {
    let a = fixture("proxy-cap-a", 30);
    // Three originals sharing one store; a cap that holds two proxies.
    let make = |tag: &str| {
        let path = a.dir.path().join(format!("{tag}.mp4"));
        std::fs::write(&path, tag).unwrap();
        let hash = cutweave_media::content_hash(&path, HashMode::Fast).unwrap();
        (path, hash)
    };
    let mut backend = a.backend.clone();
    let mut sources = vec![a.source.clone()];
    for tag in ["b", "c"] {
        let (path, hash) = make(tag);
        let media = FakeMedia::video(FakeVideo::new(1280, 720, FrameRate::FPS_30, 30));
        let probe = media.probe().unwrap();
        backend = backend.with_media(&path, media);
        sources.push(ProxySource { path, hash, probe });
    }
    let generate = |s: &ProxySource| {
        ProxyGenerator::new(
            Arc::new(backend.clone()),
            Arc::clone(&a.db),
            Arc::clone(&a.store),
            GenerateOptions::default(),
        )
        .generate(s, &profile(), &CancelToken::new(), &mut |_| {})
        .unwrap()
    };
    let first = generate(&sources[0]);
    let size = first.proxy.bytes;
    // The fake's proxies differ in size (one has sound); the cap fits any two.
    a.store.shrink_to(u64::MAX).unwrap();
    let second = generate(&sources[1]);
    let cap = first.proxy.bytes + second.proxy.bytes;
    a.store.shrink_to(cap).unwrap();
    assert_eq!(a.store.usage().unwrap().entries, 2);
    let generation = a.store.generation();
    // Using the first makes the second the least recently used.
    assert!(
        a.store
            .ready(&sources[0].hash, &profile().key())
            .unwrap()
            .is_some()
    );
    let third = generate(&sources[2]);
    assert_eq!(third.evicted.len(), 1);
    assert_eq!(third.evicted[0].hash, sources[1].hash);
    assert!(a.store.generation() > generation);
    let evicted = a
        .store
        .get(&sources[1].hash, &profile().key())
        .unwrap()
        .unwrap();
    assert_eq!(evicted.state, ProxyState::Evicted);
    assert!(!evicted.path.exists());
    assert!(
        a.store
            .ready(&sources[1].hash, &profile().key())
            .unwrap()
            .is_none()
    );
    assert!(
        a.store
            .ready(&sources[0].hash, &profile().key())
            .unwrap()
            .is_some()
    );
    assert!(a.store.usage().unwrap().bytes <= cap);
    // Lowering the cap below one proxy evicts everything; clearing removes the rest.
    let shrunk = a.store.shrink_to(size / 2).unwrap();
    assert_eq!(shrunk.removed, 2);
    assert_eq!(a.store.usage().unwrap().entries, 0);
    // A generation brings an evicted proxy back.
    a.store.shrink_to(u64::MAX).unwrap();
    assert_eq!(generate(&sources[1]).proxy.state, ProxyState::Ready);
    let cleared = a.store.clear().unwrap();
    assert_eq!(cleared.removed, 1);
    assert!(cleared.freed > 0);
    assert_eq!(a.store.usage().unwrap().entries, 0);
    assert_eq!(
        a.store.list(ProxyState::Evicted).unwrap().len(),
        3,
        "rows stay, as evicted"
    );
}

#[test]
fn a_ready_proxy_whose_file_is_gone_reads_as_evicted() {
    let f = fixture("proxy-gone", 30);
    let report = generator(&f, 600)
        .generate(&f.source, &profile(), &CancelToken::new(), &mut |_| {})
        .unwrap();
    std::fs::remove_file(&report.proxy.path).unwrap();
    assert!(
        f.store
            .ready(&f.source.hash, &profile().key())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.store
            .get(&f.source.hash, &profile().key())
            .unwrap()
            .unwrap()
            .state,
        ProxyState::Evicted
    );
}

#[test]
fn proxy_jobs_report_progress_and_their_outcome() {
    let f = fixture("proxy-job", 90);
    let jobs = JobManager::new(JobConfig::default()).unwrap();
    let outcome = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&outcome);
    let handle = submit_proxy(
        &jobs,
        Arc::new(generator(&f, 30)),
        f.source.clone(),
        profile(),
        None,
        Vec::new(),
        Box::new(move |r| *seen.lock().unwrap() = Some(r.is_ok())),
    );
    let info = handle.wait(Wall::from_secs(30)).unwrap();
    assert_eq!(info.state, JobState::Succeeded, "{info:?}");
    assert_eq!(info.kind, JobKind::Proxy);
    assert_eq!(info.label, "proxy");
    assert_eq!(*outcome.lock().unwrap(), Some(true));

    // A cancelled job ends cancelled and keeps its parts.
    let g = fixture("proxy-job-cancel", 300);
    let slow = FakeBackend::new()
        .with_capabilities(backend().capabilities())
        .with_output_files()
        .with_frame_cost(Wall::from_millis(5))
        .with_media(
            &g.source.path,
            FakeMedia::video(FakeVideo::new(1280, 720, FrameRate::FPS_30, 300)),
        );
    let started = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&started);
    let gen2 = ProxyGenerator::new(
        Arc::new(slow),
        Arc::clone(&g.db),
        Arc::clone(&g.store),
        GenerateOptions {
            part_frames: 30,
            ..GenerateOptions::default()
        },
    );
    let handle = submit_proxy(
        &jobs,
        Arc::new(gen2),
        g.source.clone(),
        profile(),
        None,
        Vec::new(),
        Box::new(move |_| flag.store(true, Ordering::SeqCst)),
    );
    while jobs.status(handle.id()).unwrap().progress.fraction < 0.2 {
        std::thread::sleep(Wall::from_millis(5));
    }
    handle.cancel();
    let info = handle.wait(Wall::from_secs(30)).unwrap();
    assert_eq!(info.state, JobState::Cancelled);
    assert!(started.load(Ordering::SeqCst));
    assert_eq!(
        g.store
            .get(&g.source.hash, &profile().key())
            .unwrap()
            .unwrap()
            .state,
        ProxyState::Building
    );
}

/// A pacer that holds generations while `busy` is set, counting how often they asked.
struct Hold {
    busy: AtomicBool,
    asked: std::sync::atomic::AtomicUsize,
}

impl cutweave_cache::proxy::Pacer for Hold {
    fn wait(&self, cancel: &CancelToken) {
        self.asked.fetch_add(1, Ordering::SeqCst);
        while self.busy.load(Ordering::SeqCst) && !cancel.is_cancelled() {
            std::thread::yield_now();
        }
    }
}

#[test]
fn a_generation_takes_no_frame_while_the_pacer_holds_it_and_finishes_after() {
    let f = fixture("proxy-pace", 60);
    let hold = Arc::new(Hold {
        busy: AtomicBool::new(true),
        asked: std::sync::atomic::AtomicUsize::new(0),
    });
    let generator = Arc::new(generator(&f, 30).with_pacer(hold.clone()));
    let cancel = CancelToken::new();
    let done = Arc::new(Mutex::new(Vec::<f64>::new()));
    let worker = {
        let (generator, source, cancel, done) = (
            Arc::clone(&generator),
            f.source.clone(),
            cancel.clone(),
            Arc::clone(&done),
        );
        std::thread::spawn(move || {
            generator.generate(&source, &profile(), &cancel, &mut |p| {
                done.lock().unwrap().push(p);
            })
        })
    };
    // It reached the first frame and waits there: no progress is reported.
    while hold.asked.load(Ordering::SeqCst) == 0 {
        std::thread::yield_now();
    }
    std::thread::sleep(Wall::from_millis(50));
    assert!(!worker.is_finished());
    assert_eq!(hold.asked.load(Ordering::SeqCst), 1, "no second frame");
    assert!(done.lock().unwrap().iter().all(|p| *p == 0.0));
    // Idle: it finishes, with nothing lost.
    hold.busy.store(false, Ordering::SeqCst);
    let report = worker.join().unwrap().unwrap();
    assert_eq!(report.proxy.state, ProxyState::Ready);
    assert_eq!(report.proxy.frames, 60);
    assert!(hold.asked.load(Ordering::SeqCst) >= 60);
}

#[test]
fn a_held_generation_stops_on_cancel_and_keeps_its_finished_parts() {
    let f = fixture("proxy-pace-cancel", 60);
    // The first part runs free; the pacer holds once it is done.
    let hold = Arc::new(Hold {
        busy: AtomicBool::new(false),
        asked: std::sync::atomic::AtomicUsize::new(0),
    });
    let generator = Arc::new(generator(&f, 30).with_pacer(hold.clone()));
    let cancel = CancelToken::new();
    let worker = {
        let (generator, source, cancel, hold) = (
            Arc::clone(&generator),
            f.source.clone(),
            cancel.clone(),
            Arc::clone(&hold),
        );
        std::thread::spawn(move || {
            generator.generate(&source, &profile(), &cancel, &mut |p| {
                if p > 0.48 {
                    hold.busy.store(true, Ordering::SeqCst);
                }
            })
        })
    };
    while hold.asked.load(Ordering::SeqCst) < 31 {
        std::thread::yield_now();
    }
    std::thread::sleep(Wall::from_millis(30));
    assert!(!worker.is_finished());
    cancel.cancel();
    assert!(worker.join().unwrap().is_err());
    let part = f.store.part(&f.source.hash, &profile().key(), 0);
    assert!(part.is_file(), "the finished part is kept");
}
