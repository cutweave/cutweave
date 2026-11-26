// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.04b: `project.archive` through the engine. A project with media in several places is
//! archived as a zip and as a folder; every SHA-256 of `archive.json` verifies; the zip is
//! extracted "on another machine" (the originals are gone) and the project opens with no
//! missing media; the original project was never written to. The media are built at test time
//! (the dev stub prober reads files by extension).

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod engine_support;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cutweave_api::Credential;
use cutweave_engine::{Call, Connection, Engine};
use engine_support::{TempDir, must, new_document, oneshot};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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

/// Every file below `dir` with its modification time, without the engine's own bookkeeping
/// folder (`.cutweave`), which the engine writes to while a project is open.
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

/// A saved project `<t>/Promo` with six assets from two folders outside it, one inside it,
/// and a generated file.
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
    let generated = folder.join("generated");
    std::fs::create_dir_all(&generated).unwrap();
    std::fs::write(generated.join("voice.wav"), "a generated voice").unwrap();
    let out = must(conn, "media.import", json!({ "paths": paths }));
    assert_eq!(out["media_ids"].as_array().unwrap().len(), 7, "{out}");
    must(conn, "project.save", json!({}));
    folder
}

fn wait(conn: &Connection, job: &Value) -> Value {
    must(
        conn,
        "job.wait",
        json!({ "job_id": job, "timeout_ms": 60_000 }),
    )
}

fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        })
}

/// Checks every entry of the manifest in `folder` against the files there, and that the
/// folder holds no file the manifest does not list. Returns the number of files.
fn verify_folder(folder: &Path) -> usize {
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(folder.join("archive.json")).unwrap()).unwrap();
    assert_eq!(manifest["format"], "cutweave-archive");
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["hash"], "sha256");
    let files = manifest["files"].as_array().unwrap();
    let mut listed: Vec<PathBuf> = Vec::new();
    for file in files {
        let rel = file["path"].as_str().unwrap();
        let path = rel.split('/').fold(folder.to_path_buf(), |p, x| p.join(x));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert_eq!(file["size"], bytes.len() as u64, "{rel}");
        assert_eq!(file["hash"], sha256(&bytes), "{rel}");
        listed.push(path);
    }
    assert!(
        files
            .iter()
            .any(|f| f["path"] == manifest["project"] && manifest["project"] != Value::Null),
        "the project document is listed: {manifest}"
    );
    listed.push(folder.join("archive.json"));
    listed.sort();
    let on_disk: Vec<PathBuf> = mtimes(folder).into_iter().map(|(p, _)| p).collect();
    assert_eq!(on_disk, listed);
    files.len()
}

/// Extracts `zip` into `into` the way any unzip tool does.
fn extract(zip: &Path, into: &Path) {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip).unwrap()).unwrap();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        assert_eq!(
            entry.compression(),
            zip::CompressionMethod::Stored,
            "{:?}",
            entry.name()
        );
        let name = entry.name().unwrap().into_owned();
        let path = name.split('/').fold(into.to_path_buf(), |p, x| p.join(x));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

#[test]
fn an_extracted_zip_verifies_and_opens_with_nothing_missing() {
    let t = TempDir::new("archive-zip");
    let engine = start(&t);
    let conn = engine.handle().connect(&agent()).unwrap();
    let original = project(&t, &conn);
    let before = mtimes(&original);
    let footage = mtimes(&t.path().join("Footage"));

    let zip = t.path().join("Vault").join("Promo 2026.zip");
    let out = must(
        &conn,
        "project.archive",
        json!({ "dest": path_str(&zip), "format": "zip" }),
    );
    assert_eq!(out["estimate"]["files"], 7, "{out}");
    assert_eq!(out["estimate"]["dest"], path_str(&zip), "{out}");
    let done = wait(&conn, &out["job_id"]);
    assert_eq!(done["state"], "succeeded", "{done}");
    // Only the zip: no `.part`, no staging folder.
    let made: Vec<PathBuf> = mtimes(zip.parent().unwrap())
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert_eq!(made, vec![zip.clone()]);

    // The original project and its media were only read.
    assert_eq!(mtimes(&original), before);
    assert_eq!(mtimes(&t.path().join("Footage")), footage);
    // The same name again is refused: an archive is never overwritten.
    let err = conn
        .call(Call::new(
            "project.archive",
            json!({ "dest": path_str(&zip), "format": "zip" }),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "invalid", "{err}");

    // "Another machine": the originals are gone; the zip is extracted somewhere else.
    must(&conn, "project.close", json!({}));
    std::fs::remove_dir_all(t.path().join("Footage")).unwrap();
    std::fs::remove_dir_all(&original).unwrap();
    let elsewhere = t.path().join("Elsewhere");
    extract(&zip, &elsewhere);
    let folder = elsewhere.join("Promo 2026");
    assert!(folder.join("Promo 2026.cutweave").is_file());
    assert!(
        folder
            .join("media")
            .join("day1")
            .join("clip0.mp4")
            .is_file()
    );
    assert!(folder.join("generated").join("voice.wav").is_file());
    // 7 media, the generated file and the document; every hash verifies.
    assert_eq!(verify_folder(&folder), 9);

    let opened = must(&conn, "project.open", json!({ "path": path_str(&folder) }));
    assert_eq!(opened["missing_media"], json!([]), "{opened}");
    let health = must(&conn, "media.find_missing", json!({}));
    assert_eq!(health["missing"], json!([]));
    assert_eq!(health["checked"], 7);
}

#[test]
fn a_folder_archive_verifies_and_agents_stay_in_their_folders() {
    let t = TempDir::new("archive-folder");
    let outside = TempDir::new("archive-outside");
    let engine = start(&t);
    let conn = engine.handle().connect(&agent()).unwrap();
    let original = project(&t, &conn);
    let before = mtimes(&original);

    let dest = t.path().join("Vault").join("Promo kept");
    let out = must(
        &conn,
        "project.archive",
        json!({ "dest": path_str(&dest), "format": "folder", "include_generated": false }),
    );
    let done = wait(&conn, &out["job_id"]);
    assert_eq!(done["state"], "succeeded", "{done}");
    assert!(dest.join("Promo kept.cutweave").is_file());
    // 7 media and the document; `generated/` was declined.
    assert_eq!(verify_folder(&dest), 8);
    assert!(!dest.join("generated").exists());
    assert_eq!(mtimes(&original), before);

    // A finished archive is not written into again, and an agent stays in its folders.
    for (folder, format) in [
        (dest.clone(), "folder"),
        (outside.path().join("Out"), "folder"),
        (outside.path().join("Out.zip"), "zip"),
    ] {
        let err = conn
            .call(Call::new(
                "project.archive",
                json!({ "dest": path_str(&folder), "format": format }),
            ))
            .unwrap_err();
        assert_eq!(err.code(), "invalid", "{err}");
    }
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
}
