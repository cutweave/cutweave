// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.04: `project.collect` through the engine. A project with media in several places is
//! collected into one folder; with the originals gone, the collected project opens with no
//! missing media, and the original project was never written to. The media are built at test
//! time (the dev stub prober reads files by extension).

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod engine_support;

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cutweave_api::Credential;
use cutweave_engine::{Call, Connection, Engine};
use engine_support::{TempDir, must, new_document, oneshot};
use serde_json::{Value, json};

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn agent() -> Credential {
    Credential::Agent("mcp".to_owned())
}

fn start(t: &TempDir) -> Engine {
    let mut config = oneshot(t.path());
    config.subsystems = Some(Box::new(cutweave_engine::DevStubMedia));
    Engine::start(config).unwrap()
}

/// Every file below `dir` with its modification time. The engine's own bookkeeping folder
/// (`.cutweave`: the lock, the journal, the export queue's database) is left out: the engine
/// writes there while a project is open, whatever a collect does.
fn mtimes(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == ".cutweave") {
            continue;
        }
        if path.is_dir() {
            out.extend(mtimes(&path));
        } else {
            out.push((path.clone(), entry.metadata().unwrap().modified().unwrap()));
        }
    }
    out.sort();
    out
}

/// A saved project `<t>/Promo` with six assets from two folders outside it and one inside it.
fn project(t: &TempDir, conn: &Connection) -> PathBuf {
    let mut paths = Vec::new();
    for (dir, n) in [("Footage/day1", 3), ("Footage/day2", 3)] {
        let folder = dir
            .split('/')
            .fold(t.path().to_path_buf(), |p, part| p.join(part));
        std::fs::create_dir_all(&folder).unwrap();
        for i in 0..n {
            let file = folder.join(format!("clip{i}.mp4"));
            std::fs::write(&file, format!("{dir} clip {i}")).unwrap();
            paths.push(path_str(&file));
        }
    }
    new_document(conn, "Promo");
    let folder = t.path().join("Promo");
    must(
        conn,
        "project.save_as",
        json!({ "path": path_str(&folder) }),
    );
    let inside = folder.join("assets");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::write(inside.join("logo.png"), "logo").unwrap();
    paths.push(path_str(&inside.join("logo.png")));
    let out = must(conn, "media.import", json!({ "paths": paths }));
    assert_eq!(out["media_ids"].as_array().unwrap().len(), 7, "{out}");
    must(conn, "project.save", json!({}));
    folder
}

fn wait(conn: &Connection, job: &Value) -> Value {
    must(
        conn,
        "job.wait",
        json!({ "job_id": job, "timeout_ms": 30_000 }),
    )
}

#[test]
fn a_collected_project_opens_with_no_missing_media_after_the_originals_are_gone() {
    let t = TempDir::new("collect");
    let engine = start(&t);
    let conn = engine.handle().connect(&agent()).unwrap();
    let original = project(&t, &conn);
    let before = mtimes(&original);
    let footage = mtimes(&t.path().join("Footage"));

    let dest = t.path().join("Handover").join("Promo copy");
    let estimate = must(
        &conn,
        "project.collect_estimate",
        json!({ "dest": path_str(&dest) }),
    );
    assert_eq!(estimate["files"], 7, "{estimate}");
    assert_eq!(estimate["enough_space"], true, "{estimate}");
    assert!(!dest.exists(), "an estimate writes nothing");

    let out = must(&conn, "project.collect", json!({ "dest": path_str(&dest) }));
    let done = wait(&conn, &out["job_id"]);
    assert_eq!(done["state"], "succeeded", "{done}");
    assert!(dest.join("Promo copy.cutweave").exists());
    assert!(dest.join("media").join("day1").join("clip0.mp4").exists());
    assert!(dest.join("media").join("assets").join("logo.png").exists());

    // The original project and its media were only read.
    assert_eq!(mtimes(&original), before);
    assert_eq!(mtimes(&t.path().join("Footage")), footage);
    assert_eq!(
        must(&conn, "media.find_missing", json!({}))["missing"],
        json!([])
    );

    // "Another machine": the originals are gone, the copy is moved, and it opens complete.
    must(&conn, "project.close", json!({}));
    std::fs::remove_dir_all(t.path().join("Footage")).unwrap();
    std::fs::remove_dir_all(&original).unwrap();
    let moved = t.path().join("Elsewhere");
    std::fs::rename(&dest, &moved).unwrap();
    let opened = must(&conn, "project.open", json!({ "path": path_str(&moved) }));
    assert_eq!(opened["missing_media"], json!([]), "{opened}");
    let health = must(&conn, "media.find_missing", json!({}));
    assert_eq!(health["missing"], json!([]));
    assert_eq!(health["checked"], 7);
}

#[test]
fn collecting_again_into_a_finished_copy_is_refused_and_agents_stay_in_their_folders() {
    let t = TempDir::new("collect-twice");
    let outside = TempDir::new("collect-outside");
    let engine = start(&t);
    let conn = engine.handle().connect(&agent()).unwrap();
    project(&t, &conn);
    let dest = t.path().join("Copy");
    let out = must(&conn, "project.collect", json!({ "dest": path_str(&dest) }));
    assert_eq!(wait(&conn, &out["job_id"])["state"], "succeeded");
    for folder in [dest.as_path(), outside.path()] {
        let err = conn
            .call(Call::new(
                "project.collect",
                json!({ "dest": path_str(folder) }),
            ))
            .unwrap_err();
        assert_eq!(err.code(), "invalid", "{err}");
    }
}

/// #208: an image sequence is one asset made of many files. Collecting copies every picture,
/// the copy opens complete with the originals gone, and a run that moved is found again whole.
#[test]
fn a_collected_image_sequence_opens_complete_and_a_moved_run_is_found_again() {
    let t = TempDir::new("collect-seq");
    let engine = start(&t);
    let conn = engine.handle().connect(&agent()).unwrap();
    let renders = t.path().join("Renders").join("shotA");
    std::fs::create_dir_all(&renders).unwrap();
    // `f_0001.png` … `f_0006.png`; number 4 has no file.
    let numbers = [1, 2, 3, 5, 6];
    let name = |n: i32| format!("f_{n:04}.png");
    for n in numbers {
        std::fs::write(renders.join(name(n)), format!("picture {n}")).unwrap();
    }
    new_document(&conn, "Promo");
    let original = t.path().join("Promo");
    must(
        &conn,
        "project.save_as",
        json!({ "path": path_str(&original) }),
    );
    let imported = must(
        &conn,
        "media.import_sequence",
        json!({ "path": path_str(&renders) }),
    );
    assert_eq!(imported["image_sequence"]["count"], 6, "{imported}");
    assert_eq!(imported["image_sequence"]["missing"], json!([4]));
    must(&conn, "project.save", json!({}));

    let dest = t.path().join("Handover");
    let estimate = must(
        &conn,
        "project.collect_estimate",
        json!({ "dest": path_str(&dest) }),
    );
    assert_eq!(estimate["files"], 5, "{estimate}");
    assert_eq!(estimate["missing"], json!([]));
    let out = must(&conn, "project.collect", json!({ "dest": path_str(&dest) }));
    assert_eq!(wait(&conn, &out["job_id"])["state"], "succeeded");

    // "Another machine": the originals are gone, the copy is moved, and it opens complete.
    must(&conn, "project.close", json!({}));
    std::fs::remove_dir_all(t.path().join("Renders")).unwrap();
    std::fs::remove_dir_all(&original).unwrap();
    let moved = t.path().join("Elsewhere");
    std::fs::rename(&dest, &moved).unwrap();
    let opened = must(&conn, "project.open", json!({ "path": path_str(&moved) }));
    assert_eq!(opened["missing_media"], json!([]), "{opened}");
    let health = must(&conn, "media.find_missing", json!({ "verify": true }));
    assert_eq!(health["missing"], json!([]), "{health}");
    assert_eq!(health["checked"], 1);
    let copied = moved.join("media").join("shotA");
    for n in numbers {
        assert_eq!(
            std::fs::read(copied.join(name(n))).unwrap(),
            format!("picture {n}").into_bytes()
        );
    }
    assert!(!copied.join(name(4)).exists());

    // The run moves: the asset is missing, and one search of a folder above finds it by content.
    let away = t.path().join("Moved").join("deep").join("shotA");
    std::fs::create_dir_all(away.parent().unwrap()).unwrap();
    std::fs::rename(&copied, &away).unwrap();
    let health = must(&conn, "media.find_missing", json!({}));
    assert_eq!(health["missing"][0]["status"], "missing", "{health}");
    assert_eq!(health["missing"][0]["missing_frames"], 5);
    let found = must(
        &conn,
        "media.relink_folder",
        json!({ "folder": path_str(&t.path().join("Moved")) }),
    );
    assert_eq!(found["still_missing"], json!([]), "{found}");
    assert_eq!(found["relinked"][0]["matched_by"], "hash", "{found}");
    assert_eq!(
        must(&conn, "media.find_missing", json!({}))["missing"],
        json!([])
    );

    // One picture goes: the sequence is incomplete, and a collect refuses it.
    std::fs::remove_file(away.join(name(5))).unwrap();
    let health = must(&conn, "media.find_missing", json!({}));
    assert_eq!(health["missing"][0]["status"], "incomplete", "{health}");
    assert_eq!(health["missing"][0]["missing_frames"], 1);
    let err = conn
        .call(Call::new(
            "project.collect",
            json!({ "dest": path_str(&t.path().join("Again")) }),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "invalid", "{err}");
    assert!(err.to_string().contains("1 picture"), "{err}");
}
