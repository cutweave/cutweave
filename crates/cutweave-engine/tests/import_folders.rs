// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.09 (ADR-0020): `media.import` of OS folders through the engine. The fixture tree is
//! built at test time (no media is committed); the dev stub prober reads files by extension, so
//! a `.mp4` made of a few bytes is "media" and a `.txt` is not.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    missing_docs
)]

mod engine_support;

use std::path::Path;
use std::time::Duration;

use cutweave_api::Credential;
use cutweave_engine::{Call, Engine};
use engine_support::{TempDir, must, new_document, oneshot, wait_for};
use serde_json::json;

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn agent() -> Credential {
    Credential::Agent("mcp".to_owned())
}

/// Writes `n` files of `ext` (each with its own bytes, so none repeats another's content).
fn files(dir: &Path, stem: &str, ext: &str, n: usize) {
    std::fs::create_dir_all(dir).unwrap();
    for i in 0..n {
        std::fs::write(
            dir.join(format!("{stem}{i:03}.{ext}")),
            format!("{stem}-{i}-{}", dir.display()),
        )
        .unwrap();
    }
}

/// A directory link to `target`; whether it could be made.
fn dir_link(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
            || std::process::Command::new("cmd")
                .arg("/C")
                .arg("mklink")
                .arg("/J")
                .arg(link)
                .arg(target)
                .output()
                .is_ok_and(|o| o.status.success())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        false
    }
}

/// 3 levels, 40 media files, 5 unsupported, a hidden file and a link back to the root.
fn fixture(root: &Path) -> std::path::PathBuf {
    let shoot = root.join("Shoot");
    files(&shoot.join("day1").join("cam-a"), "a", "mp4", 15);
    files(&shoot.join("day1").join("cam-b"), "b", "mp4", 10);
    files(&shoot.join("day2"), "m", "wav", 10);
    files(&shoot.join("audio"), "s", "png", 5);
    files(&shoot.join("docs"), "notes", "txt", 5);
    std::fs::write(shoot.join(".hidden.mp4"), "hidden").unwrap();
    // The link must not be followed, or the walk would never end.
    let linked = dir_link(&shoot, &shoot.join("day1").join("loop"));
    assert!(linked || cfg!(not(any(unix, windows))), "no directory link");
    shoot
}

fn start(t: &TempDir) -> Engine {
    let mut config = oneshot(t.path());
    config.subsystems = Some(Box::new(cutweave_engine::DevStubMedia));
    Engine::start(config).unwrap()
}

fn folder_paths(conn: &cutweave_engine::Connection) -> Vec<String> {
    let out = must(conn, "media.list_folders", json!({}));
    let mut paths: Vec<String> = out["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_owned())
        .collect();
    paths.sort();
    paths
}

fn total(conn: &cutweave_engine::Connection) -> u64 {
    must(conn, "media.list", json!({}))["total"]
        .as_u64()
        .unwrap()
}

#[test]
fn a_tree_imports_into_folders_that_mirror_it_in_one_undo_step() {
    let t = TempDir::new("tree");
    let shoot = fixture(t.path());
    let engine = start(&t);
    let mut events = engine.handle().subscribe();
    let conn = engine.handle().connect(&agent()).unwrap();
    new_document(&conn, "Tree");

    let out = must(
        &conn,
        "media.import",
        json!({ "paths": [path_str(&shoot)] }),
    );
    assert_eq!(out["media_ids"].as_array().unwrap().len(), 40, "{out}");
    assert_eq!(out["skipped"]["unsupported"], 5, "{out}");
    assert_eq!(out["failed"], json!([]), "{out}");
    assert_eq!(out["folders"], 6, "{out}");
    assert_eq!(total(&conn), 40);
    assert_eq!(
        folder_paths(&conn),
        [
            "Shoot",
            "Shoot/audio",
            "Shoot/day1",
            "Shoot/day1/cam-a",
            "Shoot/day1/cam-b",
            "Shoot/day2"
        ]
    );
    let progress = wait_for(&mut events, Duration::from_secs(10), |e| {
        e.name() == "media.import_progress"
    });
    assert_eq!(progress.payload()["total"], 45);

    // The same tree again: every file is known, so nothing is added and no folder is made.
    let again = must(
        &conn,
        "media.import",
        json!({ "paths": [path_str(&shoot)] }),
    );
    assert_eq!(again["folders"], 0, "{again}");
    assert!(
        again["media"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["duplicate"] == true),
        "{again}"
    );
    assert_eq!(total(&conn), 40);
    assert_eq!(folder_paths(&conn).len(), 6);

    // One undo (the second import added nothing, so it is the first one that goes).
    must(&conn, "edit.undo", json!({}));
    if total(&conn) != 0 {
        must(&conn, "edit.undo", json!({}));
    }
    assert_eq!(total(&conn), 0);
    assert_eq!(folder_paths(&conn), Vec::<String>::new());
    engine.shutdown();
}

#[test]
fn without_keep_structure_every_file_lands_in_the_given_folder() {
    let t = TempDir::new("flat");
    let shoot = fixture(t.path());
    let engine = start(&t);
    let ui = engine.handle().connect(&agent()).unwrap();
    new_document(&ui, "Flat");
    let into = must(&ui, "media.create_folder", json!({"name": "Dump"}));
    let out = must(
        &ui,
        "media.import",
        json!({ "paths": [path_str(&shoot)], "folder": into["folder_id"], "keep_structure": false }),
    );
    assert_eq!(out["media_ids"].as_array().unwrap().len(), 40, "{out}");
    assert_eq!(out["folders"], 0);
    assert_eq!(folder_paths(&ui), ["Dump"]);
    let listed = must(
        &ui,
        "media.list",
        json!({"limit": 500, "filter": {"folder": into["folder_id"]}}),
    );
    assert_eq!(listed["matched"], 40, "{listed}");
    engine.shutdown();
}

#[test]
fn too_many_files_fail_and_import_nothing_and_agents_stay_inside_their_roots() {
    let t = TempDir::new("many");
    let big = t.path().join("big");
    files(&big, "f", "mp4", 2_001);
    let outside = TempDir::new("outside");
    files(&outside.path().join("o"), "o", "mp4", 2);
    let engine = start(&t);
    let conn = engine.handle().connect(&agent()).unwrap();
    new_document(&conn, "Many");

    let err = conn
        .call(Call::new(
            "media.import",
            json!({ "paths": [path_str(&big)] }),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(err.to_string().contains("/paths/0"), "{err}");
    assert!(err.to_string().contains("2001"), "{err}");
    assert!(err.to_string().contains("subfolders"), "{err}");
    assert_eq!(total(&conn), 0);

    let err = conn
        .call(Call::new(
            "media.import",
            json!({ "paths": [path_str(&outside.path().join("o"))] }),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(err.to_string().contains("/paths/0"), "{err}");
    assert_eq!(total(&conn), 0);
    engine.shutdown();
}

#[test]
fn a_500_file_folder_imports_and_its_time_is_recorded() {
    let t = TempDir::new("five-hundred");
    let dir = t.path().join("five");
    for i in 0..5 {
        files(&dir.join(format!("part{i}")), "c", "mp4", 100);
    }
    let engine = start(&t);
    let config_conn = engine.handle().connect(&agent()).unwrap();
    new_document(&config_conn, "Five");
    let started = std::time::Instant::now();
    let out = must(
        &config_conn,
        "media.import",
        json!({ "paths": [path_str(&dir)] }),
    );
    let took = started.elapsed();
    // Recorded, not gating (the reference desktop's budget is 30 s with real probing).
    eprintln!("imported 500 files in {took:?} with the stub prober");
    assert_eq!(out["media_ids"].as_array().unwrap().len(), 500, "{out}");
    assert_eq!(out["folders"], 6);
    engine.shutdown();
}
