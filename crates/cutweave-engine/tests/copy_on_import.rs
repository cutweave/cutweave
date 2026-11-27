// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.04c: `media.import{copy_to_project}` through the engine. A file from outside the
//! project is copied under `<project>/media/` and the asset points at the copy, so the project
//! opens with nothing missing after the original is renamed away. An untitled document and a
//! full drive are refused before anything is copied; undo removes the assets and leaves the
//! copies. The media are built at test time (the dev stub prober reads files by extension).

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod engine_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cutweave_api::Credential;
use cutweave_commands::CmdError;
use cutweave_engine::{Call, Connection, Engine};
use engine_support::{TempDir, must, new_document, oneshot};
use serde_json::{Value, json};

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn agent() -> Credential {
    Credential::Agent("mcp".to_owned())
}

fn start(t: &TempDir, free: Option<u64>) -> Engine {
    let mut config = oneshot(t.path());
    config.subsystems = Some(Box::new(cutweave_engine::DevStubMedia));
    if let Some(free) = free {
        config.free_space = Some(Arc::new(move |_| Some(free)));
    }
    Engine::start(config).unwrap()
}

/// A file outside the project, in `<t>/Footage/<name>`.
fn original(t: &TempDir, name: &str) -> PathBuf {
    let folder = t.path().join("Footage");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join(name);
    std::fs::write(&file, format!("the bytes of {name}")).unwrap();
    file
}

/// A saved project in `<t>/Promo`.
fn saved_project(t: &TempDir, conn: &Connection) -> PathBuf {
    new_document(conn, "Promo");
    let folder = t.path().join("Promo");
    must(
        conn,
        "project.save_as",
        json!({ "path": path_str(&folder) }),
    );
    folder
}

fn asset_paths(conn: &Connection) -> Vec<String> {
    must(conn, "media.list", json!({}))["media"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["path"].as_str().unwrap().to_owned())
        .collect()
}

fn refused(conn: &Connection, params: Value) -> String {
    match conn.call(Call::new("media.import", params)) {
        Err(CmdError::Invalid { path, msg }) => {
            assert_eq!(path, "/copy_to_project", "{msg}");
            msg
        }
        other => panic!("expected Invalid at /copy_to_project, got {other:?}"),
    }
}

#[test]
fn a_copied_import_survives_the_original_being_renamed_away() {
    let t = TempDir::new("coi");
    let engine = start(&t, None);
    let conn = engine.handle().connect(&agent()).unwrap();
    let project = saved_project(&t, &conn);
    let file = original(&t, "clip.mp4");

    let out = must(
        &conn,
        "media.import",
        json!({ "paths": [path_str(&file)], "copy_to_project": true }),
    );
    assert_eq!(out["media_ids"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(out["media"][0]["path"], json!(path_str(&file)));
    let copy = project.join("media").join("Footage").join("clip.mp4");
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        std::fs::read(&file).unwrap(),
        "the copy holds the original's bytes"
    );
    assert_eq!(asset_paths(&conn), [path_str(&copy)]);

    // Importing the original again adds nothing and copies nothing: it is a duplicate.
    let again = must(
        &conn,
        "media.import",
        json!({ "paths": [path_str(&file)], "copy_to_project": true }),
    );
    assert_eq!(again["media"][0]["duplicate"], true, "{again}");
    assert_eq!(
        std::fs::read_dir(project.join("media").join("Footage"))
            .unwrap()
            .count(),
        1
    );

    // The original goes away; the project reopens with nothing missing.
    must(&conn, "project.save", json!({}));
    must(&conn, "project.close", json!({}));
    std::fs::rename(&file, t.path().join("Footage").join("moved-away.mp4")).unwrap();
    let opened = must(&conn, "project.open", json!({ "path": path_str(&project) }));
    assert_eq!(opened["missing_media"], json!([]), "{opened}");
    let health = must(&conn, "media.find_missing", json!({}));
    assert_eq!(health["missing"], json!([]));
    assert_eq!(health["checked"], 1);
}

#[test]
fn undo_removes_the_assets_and_leaves_the_copied_files() {
    let t = TempDir::new("coi-undo");
    let engine = start(&t, None);
    let conn = engine.handle().connect(&agent()).unwrap();
    let project = saved_project(&t, &conn);
    let file = original(&t, "clip.mp4");
    must(
        &conn,
        "media.import",
        json!({ "paths": [path_str(&file)], "copy_to_project": true }),
    );
    let copy = project.join("media").join("Footage").join("clip.mp4");
    assert!(copy.exists());

    must(&conn, "edit.undo", json!({}));

    assert!(asset_paths(&conn).is_empty(), "the asset is gone");
    assert!(copy.exists(), "undo never deletes a file");

    // Redo brings the asset back at the copy, which is still there.
    must(&conn, "edit.redo", json!({}));
    assert_eq!(asset_paths(&conn), [path_str(&copy)]);
}

#[test]
fn an_untitled_document_is_told_to_save_first() {
    let t = TempDir::new("coi-untitled");
    let engine = start(&t, None);
    let conn = engine.handle().connect(&agent()).unwrap();
    new_document(&conn, "Untitled");
    let file = original(&t, "clip.mp4");

    let msg = refused(
        &conn,
        json!({ "paths": [path_str(&file)], "copy_to_project": true }),
    );

    assert!(msg.contains("save the project first"), "{msg}");
    assert_eq!(asset_paths(&conn), Vec::<String>::new());
    // Without the flag the same import works.
    must(&conn, "media.import", json!({ "paths": [path_str(&file)] }));
    assert_eq!(asset_paths(&conn), [path_str(&file)]);
}

#[test]
fn a_full_drive_is_refused_before_anything_is_copied() {
    let t = TempDir::new("coi-full");
    let engine = start(&t, Some(1 << 20));
    let conn = engine.handle().connect(&agent()).unwrap();
    let project = saved_project(&t, &conn);
    let file = original(&t, "clip.mp4");

    let msg = refused(
        &conn,
        json!({ "paths": [path_str(&file)], "copy_to_project": true }),
    );

    assert!(msg.contains("not enough space"), "{msg}");
    assert!(!project.join("media").exists(), "nothing was copied");
    assert!(asset_paths(&conn).is_empty(), "nothing was imported");
}
