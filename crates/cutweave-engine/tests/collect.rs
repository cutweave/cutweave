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

/// Every file below `dir` with its modification time.
fn mtimes(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
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
        let folder = dir.split('/').fold(t.path().to_path_buf(), |p, part| p.join(part));
        std::fs::create_dir_all(&folder).unwrap();
        for i in 0..n {
            let file = folder.join(format!("clip{i}.mp4"));
            std::fs::write(&file, format!("{dir} clip {i}")).unwrap();
            paths.push(path_str(&file));
        }
    }
    new_document(conn, "Promo");
    let folder = t.path().join("Promo");
    must(conn, "project.save_as", json!({ "path": path_str(&folder) }));
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
    must(conn, "job.wait", json!({ "job_id": job, "timeout_ms": 30_000 }))
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
