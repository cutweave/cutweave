// SPDX-License-Identifier: GPL-3.0-or-later
//! Unit tests: failpoint crash-safety, migration backup, read-only, save-as, moved projects.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use cutweave_model::{IdGen, MediaAsset, MediaKind, ProbeInfo, SeededGen};

use super::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cutweave-unit-{}-{}",
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

fn project_with_media(name: &str, abs: &Path) -> Project {
    let mut p = Project::new(name);
    let id: MediaId = SeededGen::new(1).mint();
    p.media.insert(
        id,
        MediaAsset::file(
            id,
            MediaKind::Video,
            abs.to_string_lossy(),
            ProbeInfo::default(),
        ),
    );
    p
}

#[test]
fn create_makes_the_layout() {
    let t = TempDir::new();
    let folder = t.0.join("MyVideo");
    let store = ProjectStore::create(&folder, &Project::new("MyVideo")).expect("create");
    assert!(folder.join("MyVideo.cutweave").is_file());
    for d in [
        "generated",
        "compositions",
        ".cutweave/journal",
        ".cutweave/backups",
    ] {
        assert!(folder.join(d).is_dir(), "{d}");
    }
    assert!(folder.join(".cutweave/lock").is_file());
    drop(store);
    assert!(!folder.join(".cutweave/lock").exists(), "released on drop");
    assert!(matches!(
        ProjectStore::create(&folder, &Project::new("again")),
        Err(ProjectIoError::AlreadyExists(_))
    ));
}

/// Aborting after each of the four steps leaves the previous document intact and loadable.
#[test]
fn failpoint_after_each_step_keeps_a_loadable_document() {
    for (fail_at, new_is_visible) in [
        (Step::TempWritten, false),
        (Step::TempSynced, false),
        (Step::Renamed, true),
        (Step::DirSynced, true),
    ] {
        let t = TempDir::new();
        let folder = t.0.join("Crash Test");
        let old = Project::new("old");
        let mut store = ProjectStore::create(&folder, &old).expect("create");
        let before = fs::read(store.document_path()).expect("read");

        let new = Project::new("new");
        let err = store
            .save_inner(&new, &mut |step| {
                if step == fail_at {
                    Err(io::Error::other("simulated kill"))
                } else {
                    Ok(())
                }
            })
            .expect_err("failpoint fires");
        assert!(matches!(err, ProjectIoError::Io { .. }), "{fail_at:?}");
        // The process "died": no cleanup ran. Drop only releases the lock.
        drop(store);

        let after = fs::read(folder.join("Crash Test.cutweave")).expect("read");
        let opened = ProjectStore::open(&folder).expect("loadable after crash");
        if new_is_visible {
            assert_eq!(opened.project, new, "{fail_at:?}");
        } else {
            assert_eq!(after, before, "previous file byte-identical at {fail_at:?}");
            assert_eq!(opened.project, old, "{fail_at:?}");
        }
        // The next open cleaned the leftover temp.
        assert!(!atomic::temp_path_for(&folder.join("Crash Test.cutweave")).exists());
    }
}

#[test]
fn real_failure_removes_the_temp_file() {
    let t = TempDir::new();
    let target = t.0.join("missing-dir").join("x.cutweave");
    assert!(write_atomic(&target, b"{}").is_err());
    let target = t.0.join("x.cutweave");
    write_atomic(&target, b"1").expect("write");
    write_atomic(&target, b"22").expect("replace");
    assert_eq!(fs::read(&target).expect("read"), b"22");
    assert!(!atomic::temp_path_for(&target).exists());
}

#[test]
fn migrated_document_is_backed_up_before_first_save() {
    let t = TempDir::new();
    let folder = t.0.join("Old");
    fs::create_dir_all(&folder).expect("mkdir");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../cutweave-model/tests/fixtures/v0/three-clips.cutweave");
    let original = fs::read(&fixture).expect("fixture");
    fs::write(folder.join("Old.cutweave"), &original).expect("copy");

    let mut o = ProjectStore::open(&folder).expect("open");
    assert!(o.report.migration.is_some());
    o.store.save(&o.project).expect("save");
    let backup = folder.join(".cutweave/backups/Old.cutweave.pre-migration-v0");
    assert_eq!(fs::read(&backup).expect("backup"), original);
    assert_ne!(
        fs::read(folder.join("Old.cutweave")).expect("doc"),
        original
    );
    // A second save does not touch the backup.
    o.store.save(&o.project).expect("save");
    assert_eq!(fs::read(&backup).expect("backup"), original);
}

#[test]
fn newer_schema_is_read_only_and_untouched() {
    let t = TempDir::new();
    let folder = t.0.join("Future");
    fs::create_dir_all(&folder).expect("mkdir");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../cutweave-model/tests/fixtures/newer/v999.cutweave");
    let original = fs::read(&fixture).expect("fixture");
    fs::write(folder.join("Future.cutweave"), &original).expect("copy");

    let mut o = ProjectStore::open(&folder).expect("open");
    assert!(matches!(
        o.report.read_only,
        Some(ReadOnlyReason::NewerSchema {
            found_version: 999,
            ..
        })
    ));
    assert!(o.store.lock_info().is_none(), "no lock taken");
    assert!(matches!(
        o.store.save(&o.project),
        Err(ProjectIoError::ReadOnly(_))
    ));
    assert!(matches!(
        o.store.save_as(&t.0.join("Copy"), &o.project),
        Err(ProjectIoError::ReadOnly(_))
    ));
    assert_eq!(
        fs::read(folder.join("Future.cutweave")).expect("doc"),
        original
    );
    assert!(!folder.join(".cutweave").exists(), "folder not touched");
}

#[test]
fn save_as_recomputes_relative_paths_and_moves_the_lock() {
    let t = TempDir::new();
    let a = t.0.join("A");
    let footage = t.0.join("Footage");
    fs::create_dir_all(&footage).expect("mkdir");
    let clip = footage.join("clip one.mp4");
    fs::write(&clip, b"x").expect("clip");
    let project = project_with_media("A", &clip);

    let mut store = ProjectStore::create(&a, &project).expect("create");
    let rel_a = fs::read_to_string(a.join("A.cutweave")).expect("doc");
    assert!(
        rel_a.contains("\"path_rel\": \"../Footage/clip one.mp4\""),
        "{rel_a}"
    );

    let b = t.0.join("deep").join("B");
    store.save_as(&b, &project).expect("save_as");
    let rel_b = fs::read_to_string(b.join("B.cutweave")).expect("doc");
    assert!(
        rel_b.contains("\"path_rel\": \"../../Footage/clip one.mp4\""),
        "{rel_b}"
    );
    assert_eq!(store.root(), std::path::absolute(&b).expect("abs"));
    assert!(!a.join(".cutweave/lock").exists(), "old lock released");
    assert!(b.join(".cutweave/lock").exists(), "new lock held");
    assert!(matches!(
        store.save_as(&b, &project),
        Err(ProjectIoError::AlreadyExists(_))
    ));
    // Old project can be opened (lock free) and is unchanged.
    assert!(ProjectStore::open(&a).is_ok());
}

#[test]
fn moved_project_finds_media_by_relative_path() {
    let t = TempDir::new();
    let a = t.0.join("A");
    fs::create_dir_all(a.join("generated")).expect("mkdir");
    let clip = a.join("generated").join("voice.wav");
    fs::write(&clip, b"x").expect("clip");
    let project = project_with_media("A", &clip);
    ProjectStore::create(&a, &project)
        .expect("create")
        .close()
        .expect("close");

    let b = t.0.join("Moved");
    fs::rename(&a, &b).expect("move folder");
    fs::rename(b.join("A.cutweave"), b.join("Moved.cutweave")).expect("rename doc");

    let mut o = ProjectStore::open(&b).expect("open");
    assert_eq!(o.report.missing_media.len(), 0);
    assert_eq!(o.report.relinked_media.len(), 1);
    let new_abs = o
        .project
        .media
        .values()
        .next()
        .and_then(|m| m.path_abs.clone());
    assert_eq!(
        PathBuf::from(new_abs.expect("abs")),
        std::path::absolute(&b)
            .expect("abs")
            .join("generated/voice.wav")
    );
    // Saving persists the corrected absolute path.
    o.store.save(&o.project).expect("save");
    let again = ProjectStore::open_with(
        &b,
        &OpenOptions {
            read_only: true,
            ..OpenOptions::default()
        },
    )
    .expect("reopen");
    assert_eq!(again.report.relinked_media.len(), 0);
}

#[test]
fn missing_media_is_reported() {
    let t = TempDir::new();
    let folder = t.0.join("M");
    let project = project_with_media("M", &t.0.join("gone.mp4"));
    ProjectStore::create(&folder, &project)
        .expect("create")
        .close()
        .expect("close");
    let o = ProjectStore::open(&folder).expect("open");
    assert_eq!(o.report.missing_media.len(), 1);
}

/// A project `<t>/S` with one image sequence `<dir>/f_0001.png` to `f_0004.png` (number 3 had
/// no file at import), closed again; returns the project folder.
fn project_with_run(t: &TempDir, dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).expect("mkdir");
    for n in [1, 2, 4] {
        fs::write(dir.join(format!("f_{n:04}.png")), b"x").expect("picture");
    }
    let run = cutweave_model::ImageSequence {
        pattern: "f_%04d.png".to_owned(),
        first: 1,
        count: 4,
        fps: cutweave_model::FrameRate::FPS_30,
        missing: vec![3],
        extra: cutweave_model::Extra::default(),
    };
    let mut p = Project::new("S");
    let id: MediaId = SeededGen::new(1).mint();
    let first = dir.join("f_0001.png");
    p.media.insert(
        id,
        MediaAsset::image_sequence(id, first.to_string_lossy(), run, ProbeInfo::default()),
    );
    let folder = t.0.join("S");
    ProjectStore::create(&folder, &p)
        .expect("create")
        .close()
        .expect("close");
    folder
}

#[test]
fn a_whole_image_sequence_is_not_reported() {
    let t = TempDir::new();
    // Number 3 never had a file: it is not looked for.
    let folder = project_with_run(&t, &t.0.join("run"));
    let o = ProjectStore::open(&folder).expect("open");
    assert_eq!(o.report.missing_media, Vec::new());
}

#[test]
fn an_image_sequence_with_a_picture_gone_is_reported() {
    let t = TempDir::new();
    let dir = t.0.join("run");
    let folder = project_with_run(&t, &dir);
    // The first picture is still there; one in the middle of the run is not.
    fs::remove_file(dir.join("f_0004.png")).expect("remove");
    let o = ProjectStore::open(&folder).expect("open");
    assert_eq!(o.report.missing_media.len(), 1);
    let first = dir.join("f_0001.png");
    assert_eq!(
        o.report.missing_media[0].path_abs.as_deref(),
        Some(&*first.to_string_lossy())
    );
}

#[test]
fn a_moved_image_sequence_with_a_picture_gone_is_relinked_and_reported() {
    let t = TempDir::new();
    let a = t.0.join("S");
    fs::create_dir_all(&a).expect("mkdir");
    // The run is inside the project folder, which then moves.
    let folder = project_with_run(&t, &a.join("media").join("run"));
    assert_eq!(folder, a);
    let b = t.0.join("T");
    fs::rename(&a, &b).expect("move folder");
    fs::rename(b.join("S.cutweave"), b.join("T.cutweave")).expect("rename doc");
    let whole = ProjectStore::open(&b).expect("open");
    assert_eq!(whole.report.relinked_media.len(), 1);
    assert_eq!(whole.report.missing_media, Vec::new());
    whole.store.close().expect("close");

    fs::remove_file(b.join("media").join("run").join("f_0002.png")).expect("remove");
    let o = ProjectStore::open(&b).expect("open");
    assert_eq!(o.report.relinked_media.len(), 1);
    assert_eq!(o.report.missing_media.len(), 1);
}

#[test]
fn writes_never_leave_the_project_folder() {
    let layout = ProjectLayout::for_folder(Path::new("/p/Proj")).expect("layout");
    assert!(layout.ensure_inside(&layout.lock_file()).is_ok());
    assert!(layout.ensure_inside(&layout.root.join("../evil")).is_err());
    assert!(layout.ensure_inside(Path::new("/elsewhere/x")).is_err());
    assert!(ProjectLayout::for_folder(Path::new("/p/a:b")).is_err());
    assert!(ProjectLayout::for_document(Path::new("/p/x.txt")).is_err());
}
