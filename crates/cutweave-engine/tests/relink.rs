// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.03: missing media through the engine. A project whose media folder was moved opens
//! with every asset missing; one `media.relink_folder` finds them all again (renamed files
//! too), and one undo puts the old paths back. The media are built at test time (the dev stub
//! prober reads files by extension and hashes their bytes).

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod engine_support;

use std::path::Path;

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

fn missing(conn: &Connection) -> Vec<Value> {
    must(conn, "media.find_missing", json!({}))["missing"]
        .as_array()
        .unwrap()
        .clone()
}

/// A saved project `<t>/Promo` with twenty assets from `<t>/Footage`; returns its folder.
fn project_with_twenty(t: &TempDir, conn: &Connection) -> std::path::PathBuf {
    let footage = t.path().join("Footage");
    std::fs::create_dir_all(&footage).unwrap();
    let mut paths = Vec::new();
    for i in 0..20 {
        let ext = if i % 5 == 0 { "wav" } else { "mp4" };
        let file = footage.join(format!("clip{i:02}.{ext}"));
        std::fs::write(&file, format!("content of clip {i}")).unwrap();
        paths.push(path_str(&file));
    }
    new_document(conn, "Promo");
    let out = must(conn, "media.import", json!({ "paths": paths }));
    assert_eq!(out["media_ids"].as_array().unwrap().len(), 20, "{out}");
    let folder = t.path().join("Promo");
    must(
        conn,
        "project.save_as",
        json!({ "path": path_str(&folder) }),
    );
    folder
}

#[test]
fn a_moved_media_folder_is_found_again_with_one_command_and_one_undo() {
    let t = TempDir::new("relink");
    let engine = start(&t);
    let conn = engine.handle().connect(&agent()).unwrap();
    let project = project_with_twenty(&t, &conn);
    assert_eq!(missing(&conn), Vec::<Value>::new());

    // The footage moves to another place, three files get new names, and the project is
    // opened again.
    must(&conn, "project.close", json!({}));
    let footage = t.path().join("Footage");
    let moved = t.path().join("Archive").join("2026").join("Footage");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&footage, &moved).unwrap();
    for i in [1, 2, 5] {
        let ext = if i % 5 == 0 { "wav" } else { "mp4" };
        std::fs::rename(
            moved.join(format!("clip{i:02}.{ext}")),
            moved.join(format!("renamed take {i}.{ext}")),
        )
        .unwrap();
    }
    let opened = must(&conn, "project.open", json!({ "path": path_str(&project) }));
    assert_eq!(opened["missing_media"].as_array().unwrap().len(), 20);
    let gone = missing(&conn);
    assert_eq!(gone.len(), 20);
    assert!(gone.iter().all(|m| m["status"] == "missing"));
    let old_paths: Vec<Value> = gone.iter().map(|m| m["path"].clone()).collect();

    let out = must(
        &conn,
        "media.relink_folder",
        json!({ "folder": path_str(&t.path().join("Archive")) }),
    );
    assert_eq!(out["relinked"].as_array().unwrap().len(), 20, "{out}");
    assert_eq!(out["still_missing"], json!([]), "{out}");
    assert!(
        out["relinked"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["matched_by"] == "hash"),
        "{out}"
    );
    assert_eq!(missing(&conn), Vec::<Value>::new());

    // One undo step for the whole relink.
    must(&conn, "edit.undo", json!({}));
    let again = missing(&conn);
    assert_eq!(again.len(), 20);
    let paths_again: Vec<Value> = again.iter().map(|m| m["path"].clone()).collect();
    assert_eq!(paths_again, old_paths);
    must(&conn, "edit.redo", json!({}));
    assert_eq!(missing(&conn), Vec::<Value>::new());
}

#[test]
fn one_asset_is_relinked_to_a_named_file() {
    let t = TempDir::new("relink-one");
    let engine = start(&t);
    let conn = engine.handle().connect(&agent()).unwrap();
    project_with_twenty(&t, &conn);
    let footage = t.path().join("Footage");
    let from = footage.join("clip03.mp4");
    let to = footage.join("clip03 final.mp4");
    std::fs::rename(&from, &to).unwrap();
    let gone = missing(&conn);
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0]["name"], "clip03.mp4");

    let out = must(
        &conn,
        "media.relink",
        json!({ "media": gone[0]["media_id"], "path": path_str(&to) }),
    );
    assert_eq!(out["matched_by"], "hash");
    assert_eq!(out["from"], json!(path_str(&from)));
    assert_eq!(missing(&conn), Vec::<Value>::new());
}

#[test]
fn agents_only_search_folders_they_may_read() {
    let t = TempDir::new("relink-roots");
    let outside = TempDir::new("relink-outside");
    let engine = start(&t);
    let conn = engine.handle().connect(&agent()).unwrap();
    project_with_twenty(&t, &conn);
    let err = conn
        .call(Call::new(
            "media.relink_folder",
            json!({ "folder": path_str(outside.path()) }),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "invalid", "{err}");
    // Inside the allowed folder the search runs (and finds nothing to do).
    let out = must(
        &conn,
        "media.relink_folder",
        json!({ "folder": path_str(t.path()) }),
    );
    assert_eq!(out["relinked"], json!([]));
}
