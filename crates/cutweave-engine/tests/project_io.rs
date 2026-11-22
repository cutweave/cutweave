// SPDX-License-Identifier: GPL-3.0-or-later
//! Integration tests for `project_io`: locking (in-process and multi-process), stale takeover,
//! Windows path edge cases and fixture round-trips.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use cutweave_engine::project_io::{
    LockInfo, LockedReason, OnLocked, OpenOptions, ProjectIoError, ProjectStore, ReadOnlyReason,
    host_name,
};
use cutweave_model::Project;
use cutweave_model::testing::arb_project;
use proptest::prelude::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cutweave-it-{}-{}",
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

fn fixtures(dir: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cutweave-model/tests/fixtures");
    let mut v: Vec<PathBuf> = fs::read_dir(root.join(dir))
        .expect("fixtures")
        .map(|e| e.expect("entry").path())
        .collect();
    v.sort();
    v
}

#[test]
fn second_open_in_same_process_is_locked() {
    let t = TempDir::new();
    let folder = t.0.join("Lock Me");
    let store = ProjectStore::create(&folder, &Project::new("Lock Me")).expect("create");

    let err = ProjectStore::open(&folder).expect_err("locked");
    match err {
        ProjectIoError::Locked { holder, reason } => {
            assert_eq!(reason, LockedReason::HeldByLiveProcess);
            let holder = holder.expect("holder record readable while locked");
            assert_eq!(holder.pid, std::process::id());
            assert_eq!(holder.host, host_name());
        }
        other => panic!("expected Locked, got {other:?}"),
    }
    assert!(err_message_is_clear(&folder));

    // Read-only fallback: no lock, cannot save, does not disturb the writer.
    let mut ro = ProjectStore::open_with(
        &folder,
        &OpenOptions {
            on_locked: OnLocked::ReadOnly,
            ..OpenOptions::default()
        },
    )
    .expect("read-only fallback");
    assert!(matches!(
        ro.report.read_only,
        Some(ReadOnlyReason::Locked(Some(_)))
    ));
    assert!(matches!(
        ro.store.save(&ro.project),
        Err(ProjectIoError::ReadOnly(_))
    ));
    drop(ro);
    assert!(
        folder.join(".cutweave/lock").exists(),
        "writer's lock survives"
    );

    store.close().expect("close");
    assert!(
        ProjectStore::open(&folder).is_ok(),
        "free after clean close"
    );
}

fn err_message_is_clear(folder: &Path) -> bool {
    ProjectStore::open(folder).err().is_some_and(|e| {
        e.to_string()
            .contains("already open in another Cutweave process")
    })
}

#[test]
fn stale_lock_record_is_taken_over_and_reported() {
    let t = TempDir::new();
    let folder = t.0.join("Stale");
    ProjectStore::create(&folder, &Project::new("Stale"))
        .expect("create")
        .close()
        .expect("close");
    // A crashed holder: record present, OS lock gone.
    let dead = LockInfo {
        pid: 4_000_000,
        host: host_name(),
        started: 1,
    };
    fs::write(
        folder.join(".cutweave/lock"),
        serde_json::to_string(&dead).expect("json"),
    )
    .expect("write");

    let o = ProjectStore::open(&folder).expect("stale lock is reclaimed");
    assert_eq!(o.report.stale_lock_taken, Some(dead));
    assert_eq!(o.store.lock_info().map(|l| l.pid), Some(std::process::id()));
}

#[test]
fn foreign_host_record_needs_an_explicit_takeover() {
    let t = TempDir::new();
    let folder = t.0.join("Synced");
    ProjectStore::create(&folder, &Project::new("Synced"))
        .expect("create")
        .close()
        .expect("close");
    let other = LockInfo {
        pid: 1,
        host: format!("{}-other-machine", host_name()),
        started: 1,
    };
    fs::write(
        folder.join(".cutweave/lock"),
        serde_json::to_string(&other).expect("json"),
    )
    .expect("write");

    assert!(matches!(
        ProjectStore::open(&folder),
        Err(ProjectIoError::Locked {
            reason: LockedReason::ForeignHost,
            ..
        })
    ));
    let o = ProjectStore::open_with(
        &folder,
        &OpenOptions {
            steal_foreign_lock: true,
            ..OpenOptions::default()
        },
    )
    .expect("explicit takeover");
    assert_eq!(o.report.stale_lock_taken, Some(other));
}

/// Child half of the multi-process test: holds a project open until killed.
#[test]
fn child_lock_holder() {
    let Ok(folder) = std::env::var("CUTWEAVE_TEST_HOLD_PROJECT") else {
        return; // running as a normal test: nothing to do
    };
    let _store = ProjectStore::open(Path::new(&folder)).expect("child open");
    println!("READY");
    // Parent kills us; never return normally.
    std::thread::sleep(std::time::Duration::from_secs(120));
}

#[test]
fn second_process_is_locked_and_kill_9_leaves_a_reclaimable_lock() {
    let t = TempDir::new();
    let folder = t.0.join("Two Procs");
    ProjectStore::create(&folder, &Project::new("Two Procs"))
        .expect("create")
        .close()
        .expect("close");

    let mut child = Command::new(std::env::current_exe().expect("exe"))
        .args([
            "--exact",
            "child_lock_holder",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("CUTWEAVE_TEST_HOLD_PROJECT", &folder)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child");
    let stdout = child.stdout.take().expect("stdout");
    let ready = BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .any(|l| l.contains("READY"));
    assert!(ready, "child never reported READY");

    match ProjectStore::open(&folder) {
        Err(ProjectIoError::Locked { holder, reason }) => {
            assert_eq!(reason, LockedReason::HeldByLiveProcess);
            assert_eq!(holder.map(|h| h.pid), Some(child.id()));
        }
        other => panic!("expected Locked while the child is alive, got {other:?}"),
    }

    child.kill().expect("kill"); // SIGKILL / TerminateProcess: no cleanup runs
    child.wait().expect("wait");

    let o = ProjectStore::open(&folder).expect("stale lock reclaimed after kill");
    assert_eq!(o.report.stale_lock_taken.map(|l| l.pid), Some(child.id()));
}

fn round_trip(folder: &Path, project: &Project) {
    let store = ProjectStore::create(folder, project).expect("create");
    let expected = store.persisted_form(project);
    store.close().expect("close");
    let o = ProjectStore::open(folder).expect("open");
    assert_eq!(o.project, expected);
    assert!(o.report.migration.is_none());
    // Saving the loaded document again is byte-stable.
    let mut store = o.store;
    let first = fs::read(store.document_path()).expect("read");
    store.save(&o.project).expect("resave");
    assert_eq!(fs::read(store.document_path()).expect("read"), first);
}

#[test]
fn unicode_and_spaces_round_trip() {
    let t = TempDir::new();
    for name in [
        "My Video",
        "Проект ✓ 日本語",
        "  inner  spaces",
        "emoji 🎬 cut",
    ] {
        let folder = t.0.join("dir with spaces").join(name);
        let mut project = Project::new(name);
        project.meta.app_version = Some("тест".to_owned());
        round_trip(&folder, &project);
    }
}

#[test]
fn long_paths_beyond_260_chars_work() {
    let t = TempDir::new();
    let mut parent = t.0.clone();
    while parent.as_os_str().len() < 280 {
        parent.push("segment-with-a-long-name-0123456789");
    }
    let folder = parent.join("Long Project");
    round_trip(&folder, &Project::new("Long Project"));
    // And through the document path, as the OS shell would pass it.
    let o = ProjectStore::open(&folder.join("Long Project.cutweave")).expect("open by file");
    assert_eq!(o.project.meta.name, "Long Project");
}

#[test]
fn invalid_names_are_rejected_before_touching_disk() {
    let t = TempDir::new();
    for bad in ["con", "a.", "x?y"] {
        let folder = t.0.join(bad);
        assert!(matches!(
            ProjectStore::create(&folder, &Project::new("x")),
            Err(ProjectIoError::InvalidName { .. })
        ));
        assert!(!folder.exists(), "{bad}");
    }
}

#[test]
fn open_accepts_folder_or_document_and_reports_missing() {
    let t = TempDir::new();
    assert!(matches!(
        ProjectStore::open(&t.0.join("nope")),
        Err(ProjectIoError::NotFound(_))
    ));
    let empty = t.0.join("Empty");
    fs::create_dir_all(&empty).expect("mkdir");
    assert!(matches!(
        ProjectStore::open(&empty),
        Err(ProjectIoError::NotFound(_))
    ));
    let folder = t.0.join("P");
    ProjectStore::create(&folder, &Project::new("P")).expect("create");
    // Renamed document: still found as the single .cutweave file.
    fs::rename(folder.join("P.cutweave"), folder.join("other.cutweave")).expect("rename");
    // (the first store was dropped at the end of the statement above, releasing the lock)
    let o = ProjectStore::open(&folder).expect("open by folder");
    assert!(o.store.document_path().ends_with("other.cutweave"));
}

#[test]
fn model_fixtures_round_trip() {
    let t = TempDir::new();
    let mut n = 0;
    for fixture in fixtures("v1").into_iter().chain(fixtures("v0")) {
        n += 1;
        let folder = t.0.join(format!("Fixture {n}"));
        fs::create_dir_all(&folder).expect("mkdir");
        let doc = folder.join(format!("Fixture {n}.cutweave"));
        fs::copy(&fixture, &doc).expect("copy");
        let text = fs::read_to_string(&fixture).expect("fixture text");
        let model_view = match cutweave_model::open_str(&text).expect("model open") {
            cutweave_model::OpenOutcome::Editable { project, .. } => project,
            cutweave_model::OpenOutcome::ReadOnly { .. } => panic!("fixture is not newer"),
        };

        let mut o = ProjectStore::open(&folder).expect("open fixture");
        assert_eq!(
            o.project.item_count(),
            model_view.item_count(),
            "{fixture:?}"
        );
        o.store.save(&o.project).expect("save");
        let expected = o.store.persisted_form(&o.project);
        o.store.close().expect("close");
        let again = ProjectStore::open(&folder).expect("reopen");
        assert_eq!(again.project, expected, "{fixture:?}");
        assert!(again.report.migration.is_none(), "saved in current format");
    }
    assert!(n >= 4);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]
    #[test]
    fn generated_projects_round_trip(project in arb_project()) {
        let t = TempDir::new();
        round_trip(&t.0.join("Generated Project"), &project);
    }
}
