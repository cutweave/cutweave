// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1C.04 integration: `view.*` state survives close/reopen through `ui-state.json`, never
//! touches the document or journal, tolerates a damaged file, and debounces writes.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use cutweave_commands::{Access, Registry};
use cutweave_engine::project_io::{ProjectLayout, ProjectStore};
use cutweave_engine::ui_state::{DEBOUNCE, MAX_WAIT, UiStateStore};
use cutweave_model::testing::arb_project;
use cutweave_model::{Clip, Duration as Dur, IdGen, MediaAsset, MediaKind, ProbeInfo, Project};
use cutweave_model::{
    FrameRate, SampleRate, SeededGen, Sequence, SequenceFormat, Time, Track, TrackKind,
};
use cutweave_ops::view::{PreviewQuality, SelectionRef, UiState, ViewCtx};
use proptest::prelude::*;
use serde_json::{Value, json};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cutweave-ui-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn project_with_clip() -> (Project, cutweave_model::ItemId) {
    let mut ids = SeededGen::new(3);
    let mut project = Project::new("UiState");
    let media = MediaAsset::file(
        ids.mint(),
        MediaKind::Video,
        "/m.mp4",
        ProbeInfo {
            duration: Some(Dur::from_secs(10).unwrap()),
            ..ProbeInfo::default()
        },
    );
    let format = SequenceFormat::new(1280, 720, FrameRate::FPS_30, SampleRate::HZ_48_000);
    let mut sequence = Sequence::new(ids.mint(), "Main", format);
    let mut track = Track::new(ids.mint(), TrackKind::Video, "V1");
    let clip = Clip::new(ids.mint(), media.id, Time::ZERO, Dur::from_secs(4).unwrap());
    let id = clip.id;
    track.items.insert(clip.into());
    sequence.insert_track(0, track);
    project.media.insert(media.id, media);
    project.sequences.insert(sequence.id, sequence);
    (project, id)
}

fn registry() -> Registry {
    let mut r = Registry::new();
    cutweave_ops::register(&mut r).unwrap();
    r
}

fn run(store: &mut UiStateStore, now: Instant, name: &str, params: Value) -> Value {
    let registry = registry();
    store
        .with_ctx(now, |ctx| {
            registry.invoke_engine_json(name, params, ctx, Access::UI)
        })
        .unwrap()
}

#[test]
fn playhead_and_selection_survive_close_and_reopen() {
    let tmp = TempDir::new();
    let folder = tmp.0.join("UiState");
    let (project, clip) = project_with_clip();
    let store = ProjectStore::create(&folder, &project).unwrap();
    let doc_before = fs::read(store.document_path()).unwrap();

    let mut ui = UiStateStore::open_for(&store, &project);
    assert!(
        ui.take_warnings().is_empty(),
        "no file yet is not a problem"
    );
    let now = Instant::now();
    run(
        &mut ui,
        now,
        "view.set",
        json!({"patch": {
            "playhead": 705_600_000, "zoom": 321.5, "preview_quality": "half",
            "selection": [{"kind": "item", "id": clip}],
        }}),
    );
    ui.close().unwrap();

    // Not undoable, not journaled, no doc change: the document is byte-identical and the only
    // file view state added to .cutweave is ui-state.json.
    assert_eq!(fs::read(store.document_path()).unwrap(), doc_before);
    let mut names: Vec<_> = fs::read_dir(store.layout().meta_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(names.contains(&"ui-state.json".to_owned()), "{names:?}");
    assert!(
        !names.iter().any(|n| std::path::Path::new(n)
            .extension()
            .is_some_and(|e| e == "tmp")),
        "{names:?}"
    );
    let journal = store.layout().journal_dir();
    assert!(!journal.exists() || fs::read_dir(&journal).unwrap().next().is_none());
    store.close().unwrap();

    let opened = ProjectStore::open(&folder).unwrap();
    let ui = UiStateStore::open_for(&opened.store, &opened.project);
    let s = ui.ctx().state();
    assert_eq!(s.playhead, Time::from_secs(1).unwrap());
    assert_eq!(s.selection, vec![SelectionRef::Item { id: clip }]);
    assert_eq!(s.zoom, 321.5);
    assert_eq!(s.preview_quality, PreviewQuality::Half);
}

#[test]
fn stale_selection_ids_are_dropped_on_load() {
    let tmp = TempDir::new();
    let folder = tmp.0.join("UiState");
    let (project, clip) = project_with_clip();
    let store = ProjectStore::create(&folder, &project).unwrap();
    let mut ui = UiStateStore::open_for(&store, &project);
    run(
        &mut ui,
        Instant::now(),
        "view.select",
        json!({"ids": [{"kind": "item", "id": clip}]}),
    );
    ui.close().unwrap();

    let empty = Project::new("Empty");
    let ui = UiStateStore::open_for(&store, &empty);
    assert_eq!(ui.ctx().state().selection, vec![]);
}

#[test]
fn corrupt_missing_and_old_files_never_block_open() {
    let tmp = TempDir::new();
    let folder = tmp.0.join("UiState");
    let (project, _) = project_with_clip();
    let store = ProjectStore::create(&folder, &project).unwrap();
    let file = store.layout().ui_state_file();

    let cases: &[(&[u8], bool)] = &[
        (b"{ not json", true),
        (b"", true),
        (&[0xff, 0xfe, 0x00], true),
        (b"[1,2,3]", true),
        (b"{\"zoom\": \"wide\"}", true),
        (b"{}", false),                        // an old file without a version
        (b"{\"playhead\": 705600000}", false), // version 0: still read
    ];
    for (bytes, warns) in cases {
        fs::write(&file, bytes).unwrap();
        let mut ui = UiStateStore::open_for(&store, &project);
        assert_eq!(!ui.take_warnings().is_empty(), *warns, "{bytes:?}");
        assert!(ui.ctx().state().zoom > 0.0);
        drop(ui); // clean, so the damaged file stays for the next case
    }
    // A directory in place of the file is an I/O error, still just a warning.
    fs::remove_file(&file).unwrap();
    fs::create_dir(&file).unwrap();
    let mut ui = UiStateStore::open_for(&store, &project);
    assert_eq!(ui.take_warnings().len(), 1);
    assert_eq!(ui.ctx().state(), &UiState::initial());
    drop(ui);
    // The real project still opens.
    drop(store);
    ProjectStore::open(&folder).unwrap();
}

#[test]
fn damaged_file_is_replaced_by_the_next_good_write() {
    let tmp = TempDir::new();
    let folder = tmp.0.join("UiState");
    let (project, _) = project_with_clip();
    let store = ProjectStore::create(&folder, &project).unwrap();
    fs::write(store.layout().ui_state_file(), b"garbage").unwrap();
    let mut ui = UiStateStore::open_for(&store, &project);
    assert_eq!(ui.take_warnings().len(), 1);
    assert!(ui.take_warnings().is_empty(), "warnings are delivered once");
    run(
        &mut ui,
        Instant::now(),
        "view.set_zoom",
        json!({"zoom": 7.0}),
    );
    ui.close().unwrap();
    let v: Value =
        serde_json::from_slice(&fs::read(store.layout().ui_state_file()).unwrap()).unwrap();
    assert_eq!(v["zoom"], 7.0);
    assert_eq!(v["version"], 1);
}

#[test]
fn unknown_keys_survive_a_rewrite() {
    let tmp = TempDir::new();
    let folder = tmp.0.join("UiState");
    let (project, _) = project_with_clip();
    let store = ProjectStore::create(&folder, &project).unwrap();
    fs::write(
        store.layout().ui_state_file(),
        br#"{"version":1,"zoom":50.0,"from_the_future":{"a":[1,2]}}"#,
    )
    .unwrap();
    let mut ui = UiStateStore::open_for(&store, &project);
    run(
        &mut ui,
        Instant::now(),
        "view.set_zoom",
        json!({"zoom": 60.0}),
    );
    ui.close().unwrap();
    let v: Value =
        serde_json::from_slice(&fs::read(store.layout().ui_state_file()).unwrap()).unwrap();
    assert_eq!(v["from_the_future"], json!({"a": [1, 2]}));
    assert_eq!(v["zoom"], 60.0);
}

#[test]
fn debounce_coalesces_bursts() {
    let tmp = TempDir::new();
    let folder = tmp.0.join("UiState");
    let (project, _) = project_with_clip();
    let store = ProjectStore::create(&folder, &project).unwrap();
    let mut ui = UiStateStore::open_for(&store, &project);
    let t0 = Instant::now();
    assert!(ui.deadline().is_none());
    assert!(!ui.poll(t0).unwrap(), "nothing to write");

    // 30 scrub updates, 10 ms apart: nothing is written until the burst is over.
    for i in 0..30u32 {
        let t = t0 + Duration::from_millis(10 * u64::from(i));
        run(
            &mut ui,
            t,
            "view.set_playhead",
            json!({"at": i64::from(i) * 1000}),
        );
        assert!(!ui.poll(t).unwrap());
    }
    let last = t0 + Duration::from_millis(290);
    assert_eq!(ui.writes(), 0);
    assert!(!store.layout().ui_state_file().exists());
    assert_eq!(ui.deadline(), Some(last + DEBOUNCE));
    assert!(!ui.poll(last + DEBOUNCE / 2).unwrap());
    assert!(ui.poll(last + DEBOUNCE).unwrap());
    assert_eq!(ui.writes(), 1, "one write for the whole burst");
    assert!(!ui.is_dirty() && ui.deadline().is_none());
    assert!(!ui.poll(last + DEBOUNCE * 4).unwrap());
    let v: Value =
        serde_json::from_slice(&fs::read(store.layout().ui_state_file()).unwrap()).unwrap();
    assert_eq!(v["playhead"], 29_000);

    // A no-op call does not schedule anything.
    run(
        &mut ui,
        last + Duration::from_secs(1),
        "view.set_playhead",
        json!({"at": 29_000}),
    );
    assert!(ui.deadline().is_none());
}

#[test]
fn endless_burst_is_still_written_every_max_wait() {
    let tmp = TempDir::new();
    let (project, _) = project_with_clip();
    let store = ProjectStore::create(&tmp.0.join("UiState"), &project).unwrap();
    let mut ui = UiStateStore::open_for(&store, &project);
    let t0 = Instant::now();
    // A change every 100 ms never lets the 500 ms quiet period elapse.
    let mut now = t0;
    for i in 1..=60i64 {
        now = t0 + Duration::from_millis(100 * u64::try_from(i).unwrap());
        run(&mut ui, now, "view.set_playhead", json!({"at": i}));
        ui.poll(now).unwrap();
    }
    // 6 s of continuous changes with MAX_WAIT 2 s: 2 or 3 writes, not 0 and not 60.
    assert!((2..=3).contains(&ui.writes()), "{}", ui.writes());
    assert!(MAX_WAIT > DEBOUNCE);
    let _ = now;
}

#[test]
fn flush_on_save_and_drop_and_read_only() {
    let tmp = TempDir::new();
    let folder = tmp.0.join("UiState");
    let (project, _) = project_with_clip();
    let store = ProjectStore::create(&folder, &project).unwrap();
    let mut ui = UiStateStore::open_for(&store, &project);
    run(
        &mut ui,
        Instant::now(),
        "view.set_zoom",
        json!({"zoom": 9.0}),
    );
    assert!(ui.flush().unwrap());
    assert!(!ui.flush().unwrap(), "clean");
    run(
        &mut ui,
        Instant::now(),
        "view.set_zoom",
        json!({"zoom": 10.0}),
    );
    drop(ui); // closing without an explicit flush still persists
    let ui = UiStateStore::open_for(&store, &project);
    assert_eq!(ui.ctx().state().zoom, 10.0);
    drop(ui);

    let layout = ProjectLayout::for_folder(&folder).unwrap();
    let mut ro = UiStateStore::open(&layout, &project, false);
    run(
        &mut ro,
        Instant::now(),
        "view.set_zoom",
        json!({"zoom": 11.0}),
    );
    assert!(!ro.is_dirty() && !ro.flush().unwrap());
    drop(ro);
    let again = UiStateStore::open(&layout, &project, false);
    assert_eq!(
        again.ctx().state().zoom,
        10.0,
        "read-only stores never write"
    );
}

#[test]
fn doc_changes_prune_selection_and_schedule_a_write() {
    let tmp = TempDir::new();
    let (project, clip) = project_with_clip();
    let store = ProjectStore::create(&tmp.0.join("UiState"), &project).unwrap();
    let mut ui = UiStateStore::open_for(&store, &project);
    let t = Instant::now();
    run(
        &mut ui,
        t,
        "view.select",
        json!({"ids": [{"kind": "item", "id": clip}]}),
    );
    ui.flush().unwrap();
    assert!(!ui.set_doc(t, &project));
    assert!(ui.deadline().is_none());
    assert!(ui.set_doc(t, &Project::new("Without the clip")));
    assert!(ui.is_dirty());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    /// Whatever a project and a file contain, loading never panics and the written file reloads
    /// to the same state with no warnings.
    #[test]
    fn load_then_write_is_stable(doc in arb_project(), junk in "[ -~]{0,64}") {
        let tmp = TempDir::new();
        let folder = tmp.0.join("P");
        let store = ProjectStore::create(&folder, &doc).unwrap();
        fs::write(store.layout().ui_state_file(), junk.as_bytes()).unwrap();
        let mut ui = UiStateStore::open_for(&store, &doc);
        let _ = ui.take_warnings();
        run(&mut ui, Instant::now(), "view.set_zoom", json!({"zoom": 42.0}));
        ui.flush().unwrap();
        let state = ui.ctx().state().clone();
        drop(ui);
        let mut again = UiStateStore::open_for(&store, &doc);
        prop_assert!(again.take_warnings().is_empty());
        prop_assert_eq!(again.ctx().state(), &state);
        let _: &ViewCtx = again.ctx();
    }
}
