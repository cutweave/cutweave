// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1C.03 tests: autosave timing on a fake clock, backup rotation, the committed-only rule,
//! dirty tracking and events, the recovery surface, save-as copying `generated/`, and a coarse
//! "autosave does not delay command acks" check (the precise numbers are in
//! `benches/autosave.rs`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names,
    missing_docs
)]

mod journal_support;

use std::fs;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cutweave_commands::{Clock, DocActor, DocConfig, DocHandle, ManualClock, Rev, Submit, TxKind};
use cutweave_engine::autosave::{
    AutosaveConfig, AutosavePreference, AutosaveService, AutosaveSettings, PROJECT_EVENTS,
    ProjectEvent, TickOutcome, list_backups,
};
use cutweave_engine::project_io::ProjectStore;
use journal_support::{
    Session, SessionOptions, TempDir, append, create_project, fixture, open_session, registry,
    tracks, ui,
};
use serde_json::json;

const SEC: Duration = Duration::from_secs(1);

struct Rig {
    svc: AutosaveService,
    handle: DocHandle,
    clock: Arc<ManualClock>,
    store: Arc<Mutex<ProjectStore>>,
    events: Receiver<ProjectEvent>,
    folder: std::path::PathBuf,
    _actor: DocActor,
}

fn rig(dir: &TempDir, name: &str) -> Rig {
    let folder = create_project(&dir.0, name, &fixture(1));
    let Session {
        store,
        actor,
        handle,
        clock,
        ..
    } = open_session(&folder, &SessionOptions::default());
    let store = Arc::new(Mutex::new(store));
    let svc = AutosaveService::new(
        handle.clone(),
        Arc::clone(&store),
        Arc::clone(&clock) as Arc<dyn Clock>,
        AutosaveConfig::default(),
        Rev::ZERO,
    );
    let events = svc.subscribe();
    Rig {
        svc,
        handle,
        clock,
        store,
        events,
        folder,
        _actor: actor,
    }
}

impl Rig {
    fn rename(&self, name: &str) {
        self.handle
            .submit(Submit::new(ui(), "project.rename", json!({ "name": name })))
            .wait()
            .unwrap();
    }

    fn tick(&self) -> TickOutcome {
        self.svc.tick(self.clock.now())
    }

    fn backups(&self) -> Vec<std::path::PathBuf> {
        list_backups(&self.folder.join(".cutweave").join("backups"))
    }

    fn drain(&self) -> Vec<ProjectEvent> {
        self.events.try_iter().collect()
    }
}

fn is_saved(o: TickOutcome) -> bool {
    matches!(o, TickOutcome::Saved { .. })
}

#[test]
fn saves_after_two_seconds_idle_and_not_before() {
    let t = TempDir::new("idle");
    let r = rig(&t, "Idle");
    assert_eq!(r.tick(), TickOutcome::Clean);
    assert!(!r.svc.is_dirty());

    r.rename("A");
    assert!(r.svc.is_dirty());
    assert_eq!(r.tick(), TickOutcome::Waiting);
    r.clock.advance(Duration::from_millis(1_900));
    assert_eq!(r.tick(), TickOutcome::Waiting);
    r.clock.advance(Duration::from_millis(100));
    let saved = r.tick();
    assert!(is_saved(saved), "{saved:?}");
    assert!(!r.svc.is_dirty());
    assert_eq!(r.svc.saved_rev(), Rev(1));
    assert_eq!(r.tick(), TickOutcome::Clean);

    let events = r.drain();
    assert!(matches!(
        events.as_slice(),
        [
            ProjectEvent::Dirty(d1),
            ProjectEvent::Saved(s),
            ProjectEvent::Dirty(d2),
        ] if d1.dirty && s.rev == Rev(1) && s.path.ends_with("Idle.cutweave") && !d2.dirty
    ));
    let on_disk = fs::read_to_string(r.folder.join("Idle.cutweave")).unwrap();
    assert!(on_disk.contains("\"A\""));
}

#[test]
fn continuous_edits_save_every_thirty_seconds() {
    let t = TempDir::new("continuous");
    let r = rig(&t, "Cont");
    // An edit every second: never idle for 2 s, so only the 30 s rule can fire.
    r.rename("n0");
    assert_eq!(r.tick(), TickOutcome::Waiting);
    for i in 1..30 {
        r.clock.advance(SEC);
        r.rename(&format!("n{i}"));
        assert_eq!(r.tick(), TickOutcome::Waiting, "at {i} s");
    }
    r.clock.advance(SEC);
    r.rename("n30");
    let out = r.tick();
    assert!(is_saved(out), "{out:?}");
    // The clock restarts with the next dirty period.
    r.clock.advance(SEC);
    r.rename("n31");
    assert_eq!(r.tick(), TickOutcome::Waiting);
}

#[test]
fn settings_drive_the_interval_and_can_disable() {
    let t = TempDir::new("settings");
    let r = rig(&t, "Set");
    let mut cfg = AutosaveConfig::from(AutosaveSettings {
        enabled: true,
        interval_s: 10,
    });
    assert_eq!(cfg.interval, Duration::from_secs(10));
    assert_eq!(AutosaveSettings::default().interval_s, 30);
    cfg.enabled = false;
    r.svc.set_config(cfg);
    r.rename("A");
    r.tick();
    r.clock.advance(Duration::from_secs(60));
    assert_eq!(r.tick(), TickOutcome::Disabled);
    assert!(r.svc.is_dirty());
    cfg.enabled = true;
    r.svc.set_config(cfg);
    assert!(is_saved(r.tick()));
}

#[test]
fn twelve_changed_autosaves_leave_exactly_ten_backups() {
    let t = TempDir::new("backups");
    let r = rig(&t, "Bk");
    for i in 0..12 {
        r.rename(&format!("v{i}"));
        r.tick();
        r.clock.advance(Duration::from_secs(3));
        assert_eq!(
            r.tick(),
            TickOutcome::Saved {
                rev: Rev(i + 1),
                backed_up: true
            }
        );
    }
    let backups = r.backups();
    assert_eq!(backups.len(), 10);
    // Oldest two are gone: the survivors hold v2..v11, in name (time) order.
    let contents: Vec<String> = backups
        .iter()
        .map(|p| fs::read_to_string(p).unwrap())
        .collect();
    assert!(contents[0].contains("\"v2\""));
    assert!(contents[9].contains("\"v11\""));
    let name = backups[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with("autosave-") && name.ends_with("Z.cutweave"),
        "{name}"
    );
}

#[test]
fn unchanged_content_does_not_rotate() {
    let t = TempDir::new("unchanged");
    let r = rig(&t, "Same");
    r.rename("X");
    r.tick();
    r.clock.advance(Duration::from_secs(3));
    assert_eq!(
        r.tick(),
        TickOutcome::Saved {
            rev: Rev(1),
            backed_up: true
        }
    );
    assert_eq!(r.backups().len(), 1);

    // Two commits that end at the same content: a new revision, an identical document.
    r.rename("Y");
    r.rename("X");
    r.tick();
    r.clock.advance(Duration::from_secs(3));
    assert_eq!(
        r.tick(),
        TickOutcome::Saved {
            rev: Rev(3),
            backed_up: false
        }
    );
    assert_eq!(r.backups().len(), 1);

    // An explicit save never rotates.
    r.rename("Z");
    r.svc.save_now().unwrap();
    assert_eq!(r.backups().len(), 1);
}

#[test]
fn a_restart_does_not_duplicate_the_newest_backup() {
    let t = TempDir::new("restart");
    let r = rig(&t, "Rs");
    r.rename("X");
    r.tick();
    r.clock.advance(Duration::from_secs(3));
    r.tick();
    let folder = r.folder.clone();
    drop(r);

    let Session {
        store,
        actor,
        handle,
        clock,
        ..
    } = open_session(&folder, &SessionOptions::default());
    let svc = AutosaveService::new(
        handle,
        Arc::new(Mutex::new(store)),
        Arc::clone(&clock) as Arc<dyn Clock>,
        AutosaveConfig::default(),
        Rev::ZERO,
    );
    // Dirty again at the same content as the backup (revision counter restarted).
    svc.tick(clock.now());
    clock.advance(Duration::from_secs(3));
    let _keep = actor;
    // Nothing changed on disk since the backup: no second copy.
    let before = list_backups(&folder.join(".cutweave").join("backups")).len();
    svc.tick(clock.now());
    assert_eq!(
        list_backups(&folder.join(".cutweave").join("backups")).len(),
        before
    );
}

#[test]
fn never_saves_an_uncommitted_gesture() {
    let t = TempDir::new("gesture");
    let r = rig(&t, "Ges");
    r.rename("committed");
    let g = r
        .handle
        .begin_tx(&ui(), TxKind::Gesture, "drag")
        .wait()
        .unwrap();
    r.handle
        .submit(Submit::new(ui(), "project.rename", json!({"name": "in-flight"})).in_tx(g.id))
        .wait()
        .unwrap();
    let live = r.handle.snapshot();
    assert!(live.rev > live.committed_rev, "the gesture is open");
    assert_eq!(live.doc.meta.name, "in-flight");

    r.tick();
    r.clock.advance(Duration::from_secs(3));
    let out = r.tick();
    assert_eq!(
        out,
        TickOutcome::Saved {
            rev: Rev(1),
            backed_up: true
        }
    );
    let disk = fs::read_to_string(r.folder.join("Ges.cutweave")).unwrap();
    assert!(disk.contains("committed") && !disk.contains("in-flight"));
    for b in r.backups() {
        assert!(!fs::read_to_string(b).unwrap().contains("in-flight"));
    }

    // A gesture alone is not dirty at all.
    assert!(!r.svc.is_dirty());
    r.handle.cancel_tx(&ui(), g.id).wait().unwrap();
    assert_eq!(r.tick(), TickOutcome::Clean);
}

#[test]
fn recovery_pending_blocks_autosave_and_is_described() {
    let t = TempDir::new("recovery");
    let folder = {
        let r = rig(&t, "Rec");
        r.rename("unsaved-1");
        r.rename("unsaved-2");
        r.folder.clone()
    }; // dropped: a "crash" with two journaled, unsaved transactions

    let opened = ProjectStore::open(&folder).unwrap();
    let expected = opened.report.recovery.clone().expect("recovery available");
    let actor = DocActor::spawn(DocConfig::new(registry(), opened.project.clone())).unwrap();
    let clock = Arc::new(ManualClock::new(cutweave_commands::WallTime::UNIX_EPOCH));
    let store = Arc::new(Mutex::new(opened.store));
    let svc = AutosaveService::new(
        actor.handle(),
        Arc::clone(&store),
        Arc::clone(&clock) as Arc<dyn Clock>,
        AutosaveConfig::default(),
        Rev::ZERO,
    );
    let events = svc.subscribe();

    let info = svc.recovery_available().expect("available");
    assert_eq!(info.records, 2);
    assert_eq!(info.records, expected.records);
    assert_eq!(info.last_rev, Rev(2));
    assert!(svc.announce_recovery());
    let ev = events.try_recv().unwrap();
    assert_eq!(ev.name(), "project.recovery_available");
    assert_eq!(
        serde_json::to_value(&ev).unwrap()["data"]["records"],
        json!(2)
    );

    // Even when the doc is dirty, nothing may be written until the user decides.
    let _ = actor
        .handle()
        .submit(Submit::new(ui(), "edit.checkpoint", json!({"name": "c"})))
        .wait();
    clock.advance(Duration::from_secs(60));
    let out = svc.tick(clock.now());
    assert!(matches!(out, TickOutcome::Clean | TickOutcome::Unavailable));

    // Discarding clears the offer.
    let base = opened.project;
    let opts = cutweave_engine::journal::RecoverOptions::at(clock.now());
    store
        .lock()
        .unwrap()
        .discard_recovery(&base, &opts)
        .unwrap();
    assert!(svc.recovery_available().is_none());
    assert!(!svc.announce_recovery());
}

#[test]
fn event_catalog_lists_every_event() {
    let names: Vec<&str> = PROJECT_EVENTS.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "project.dirty",
            "project.saved",
            "project.recovery_available",
            "project.autosave_failed"
        ]
    );
}

#[test]
fn failed_saves_are_reported_and_retried_after_the_backoff() {
    let t = TempDir::new("fail");
    let r = rig(&t, "Fail");
    // Block the document path with a directory so the atomic replace fails.
    r.rename("A");
    r.tick();
    r.clock.advance(Duration::from_secs(3));
    // Temp file path is `.Fail.cutweave.tmp`: occupy it with a directory.
    let tmp = r.folder.join(".Fail.cutweave.tmp");
    fs::create_dir_all(&tmp).unwrap();
    assert_eq!(r.tick(), TickOutcome::Failed);
    assert!(
        r.drain()
            .iter()
            .any(|e| e.name() == "project.autosave_failed")
    );
    assert!(r.svc.is_dirty());
    // Within the backoff nothing is retried.
    r.clock.advance(Duration::from_secs(1));
    assert_eq!(r.tick(), TickOutcome::Waiting);
    fs::remove_dir_all(&tmp).unwrap();
    r.clock.advance(Duration::from_secs(5));
    assert!(is_saved(r.tick()));
    drop(r.store);
}

// ----- save-as and generated/ ---------------------------------------------------------------

fn seed_generated(root: &Path) {
    let gen_dir = root.join("generated");
    fs::create_dir_all(gen_dir.join("ai").join("deep")).unwrap();
    fs::write(gen_dir.join("take1.wav"), b"audio-bytes").unwrap();
    fs::write(gen_dir.join("ai").join("deep").join("img.png"), b"png").unwrap();
}

fn assert_generated_copied(from: &Path, to: &Path) {
    assert_eq!(
        fs::read(to.join("generated").join("take1.wav")).unwrap(),
        b"audio-bytes"
    );
    assert_eq!(
        fs::read(to.join("generated/ai/deep/img.png")).unwrap(),
        b"png"
    );
    // Copy, not move.
    assert_eq!(
        fs::read(from.join("generated").join("take1.wav")).unwrap(),
        b"audio-bytes"
    );
    assert!(from.join("generated/ai/deep/img.png").exists());
    assert!(!to.join("generated.copying").exists());
}

#[test]
fn save_as_copies_generated_files() {
    let t = TempDir::new("saveas");
    let from = t.0.join("Orig");
    let project = fixture(1);
    let mut store = ProjectStore::create(&from, &project).unwrap();
    seed_generated(&from);
    let to = t.0.join("Copy");
    store.save_as(&to, &project).unwrap();
    assert_generated_copied(&from, &to);
    assert_eq!(store.root(), to.as_path());
}

#[test]
fn save_as_snapshot_copies_generated_files_with_a_journal() {
    let t = TempDir::new("saveas-jnl");
    let folder = create_project(&t.0, "Orig", &fixture(1));
    seed_generated(&folder);
    let mut s = open_session(&folder, &SessionOptions::default());
    let tr = tracks(&s.handle.snapshot().doc);
    append(&s.handle, tr[0], 5);
    let to = t.0.join("Copy");
    let snap = s.handle.committed_snapshot();
    s.store.save_as_snapshot(&to, &snap).unwrap();
    assert_generated_copied(&folder, &to);
}

#[test]
fn save_as_without_generated_is_fine() {
    let t = TempDir::new("saveas-none");
    let from = t.0.join("Orig");
    let project = fixture(1);
    let mut store = ProjectStore::create(&from, &project).unwrap();
    fs::remove_dir_all(from.join("generated")).unwrap();
    let to = t.0.join("Copy");
    store.save_as(&to, &project).unwrap();
    assert!(to.join("Copy.cutweave").exists());
}

#[test]
fn a_failed_generated_copy_leaves_the_original_intact_and_no_project() {
    let t = TempDir::new("saveas-fail");
    let from = t.0.join("Orig");
    let project = fixture(1);
    let mut store = ProjectStore::create(&from, &project).unwrap();
    seed_generated(&from);
    // The staging name is taken by a file: the copy cannot be staged.
    let to = t.0.join("Copy");
    fs::create_dir_all(&to).unwrap();
    fs::write(to.join("generated.copying"), b"in the way").unwrap();
    let err = store.save_as(&to, &project);
    assert!(err.is_err(), "the copy must fail");
    assert_eq!(store.root(), from.as_path(), "still the original project");
    assert_eq!(
        fs::read(from.join("generated").join("take1.wav")).unwrap(),
        b"audio-bytes"
    );
    assert!(!to.join("Copy.cutweave").exists());
}

// ----- ack latency --------------------------------------------------------------------------

#[test]
fn autosave_during_a_command_burst_does_not_stall_acks() {
    let t = TempDir::new("burst");
    let folder = create_project(&t.0, "Burst", &fixture(4));
    let Session {
        store,
        actor,
        handle,
        ..
    } = open_session(&folder, &SessionOptions::default());
    let tr = tracks(&handle.snapshot().doc);
    for i in 0..200 {
        append(&handle, tr[i % tr.len()], 30);
    }
    let svc = AutosaveService::new(
        handle.clone(),
        Arc::new(Mutex::new(store)),
        Arc::new(cutweave_commands::SystemClock),
        AutosaveConfig {
            interval: Duration::from_millis(5),
            idle: Duration::from_millis(1),
            poll: Duration::from_millis(1),
            ..AutosaveConfig::default()
        },
        Rev::ZERO,
    );
    let events = svc.subscribe();
    let task = svc.spawn().unwrap();
    let mut lat = Vec::new();
    for i in 0..500 {
        let t0 = Instant::now();
        handle
            .submit(Submit::new(
                ui(),
                "project.rename",
                json!({"name": format!("p{i}")}),
            ))
            .wait()
            .unwrap();
        lat.push(t0.elapsed());
    }
    task.stop();
    let saves = events
        .try_iter()
        .filter(|e| e.name() == "project.saved")
        .count();
    lat.sort_unstable();
    let p95 = lat[lat.len() * 95 / 100];
    println!(
        "burst: {saves} autosaves, p95 ack {p95:?}, max {:?}",
        lat[499]
    );
    assert!(saves >= 1, "autosave ran during the burst");
    // The bench asserts the 5 ms budget; this is a coarse bound that survives a loaded machine.
    assert!(p95 < Duration::from_millis(50), "p95 {p95:?}");
    drop(actor);
}

// ----- the app's preference (WP-1M.10) ---------------------------------------------------------

fn pref(enabled: bool, interval_s: u32, keep_backups: u32) -> AutosavePreference {
    AutosavePreference {
        enabled,
        interval_s,
        keep_backups,
    }
}

#[test]
fn a_new_interval_applies_to_the_current_dirty_period() {
    let t = TempDir::new("pref-interval");
    let r = rig(&t, "PrefInterval");
    // Edits every second at the default 30 s: nothing is saved while they continue.
    r.rename("n0");
    assert_eq!(r.tick(), TickOutcome::Waiting);
    for i in 1..=5 {
        r.clock.advance(SEC);
        r.rename(&format!("n{i}"));
        assert_eq!(r.tick(), TickOutcome::Waiting, "at {i} s");
    }
    // The user picks 5 s: the dirty period already is 5 s old, so it is due at once.
    r.svc.apply_preference(pref(true, 5, 10));
    assert_eq!(r.svc.config().interval, Duration::from_secs(5));
    assert_eq!(r.svc.config().idle, Duration::from_secs(2), "idle stays");
    assert!(is_saved(r.tick()));
}

#[test]
fn disabling_through_the_preference_stops_autosaves() {
    let t = TempDir::new("pref-off");
    let r = rig(&t, "PrefOff");
    r.svc.apply_preference(pref(false, 30, 10));
    r.rename("A");
    r.tick();
    r.clock.advance(Duration::from_secs(120));
    assert_eq!(r.tick(), TickOutcome::Disabled);
    assert!(r.svc.is_dirty());
    assert_eq!(r.backups().len(), 0);
    r.svc.apply_preference(pref(true, 30, 10));
    assert!(is_saved(r.tick()));
}

#[test]
fn the_preference_sets_the_number_of_backups_kept() {
    let t = TempDir::new("pref-backups");
    let r = rig(&t, "PrefBackups");
    r.svc.apply_preference(pref(true, 30, 5));
    for i in 0..8 {
        r.rename(&format!("v{i}"));
        r.tick();
        r.clock.advance(Duration::from_secs(3));
        assert!(is_saved(r.tick()));
    }
    assert_eq!(r.backups().len(), 5);
    // Raising it keeps more from then on.
    r.svc.apply_preference(pref(true, 30, 20));
    for i in 8..12 {
        r.rename(&format!("v{i}"));
        r.tick();
        r.clock.advance(Duration::from_secs(3));
        assert!(is_saved(r.tick()));
    }
    assert_eq!(r.backups().len(), 9);
}
