// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2I.03 acceptance: after a forced engine crash and recovery, a named checkpoint made before
//! the crash is restorable. A child process (this test binary re-run as `child_engine`) runs a
//! headless engine on a project and, as an agent through the dispatch API, renames the project,
//! makes named checkpoints around a save and deletes one, then prints `READY`; the parent kills
//! it (`TerminateProcess` / `SIGKILL`), restarts an engine on the project, restores the journal
//! with `project.recover` and restores the checkpoints.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod engine_support;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use cutweave_api::Credential;
use cutweave_engine::autosave::AutosaveConfig;
use cutweave_engine::{Call, Connection, Engine, EngineConfig, EngineMode};
use engine_support::{TempDir, empty_project, must};
use serde_json::json;

const CHILD_ENV: &str = "CUTWEAVE_CHECKPOINT_CRASH_CHILD";

fn rename(conn: &Connection, name: &str) {
    must(conn, "project.rename", json!({ "name": name }));
}

/// Child half: edits and checkpoints the project in `CUTWEAVE_CHECKPOINT_CRASH_CHILD`, then
/// waits to be killed.
#[test]
fn child_engine() {
    let Ok(folder) = std::env::var(CHILD_ENV) else {
        return; // a normal test run: nothing to do
    };
    let mut config = EngineConfig::new(EngineMode::Headless);
    config.project = Some(folder.into());
    // Only the explicit save below: the checkpoint after it must stay newer than the last save.
    config.autosave = Some(AutosaveConfig {
        enabled: false,
        ..AutosaveConfig::default()
    });
    let engine = Engine::start(config).unwrap();
    let conn = engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap();
    rename(&conn, "Before");
    must(&conn, "edit.checkpoint", json!({ "name": "before save" }));
    rename(&conn, "Saved");
    must(&conn, "project.save", json!({}));
    must(&conn, "edit.checkpoint", json!({ "name": "after save" }));
    rename(&conn, "After");
    must(&conn, "edit.checkpoint", json!({ "name": "doomed" }));
    must(
        &conn,
        "edit.delete_checkpoint",
        json!({ "checkpoint": "doomed" }),
    );
    rename(&conn, "Last");
    // Straight to stdout: the test harness may capture `println!`.
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "READY");
    let _ = out.flush();
    drop(out);
    // The parent kills us; never shut down cleanly.
    std::thread::sleep(Duration::from_secs(120));
    drop(engine);
}

#[test]
fn a_named_checkpoint_made_before_an_engine_crash_is_restorable_after_recovery() {
    let t = TempDir::new("checkpoint-crash");
    let folder = empty_project(t.path(), "Crash");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_engine", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, &folder)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let ready = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map_while(Result::ok)
        // The harness may print `test child_engine ... ` on the same line first.
        .any(|line| line.contains("READY"));
    child.kill().unwrap(); // no cleanup runs in the child
    child.wait().unwrap();
    assert!(ready, "the child engine did not finish its edits");

    let mut config = EngineConfig::new(EngineMode::Headless);
    config.project = Some(folder.clone());
    config.autosave = Some(AutosaveConfig {
        enabled: false,
        ..AutosaveConfig::default()
    });
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    assert!(
        h.status().unwrap().recovery.is_some(),
        "no recovery offered"
    );
    let conn = h.connect(&Credential::Agent("cli".into())).unwrap();
    must(&conn, "project.recover", json!({}));
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Last");

    let listed = must(&conn, "edit.list_checkpoints", json!({}));
    let names: Vec<&str> = listed["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["before save", "after save"]);

    // Made after the last save: its later changes are in the recovered history (the agent's
    // own), so it restores without force, as one undoable change.
    must(&conn, "edit.restore", json!({ "checkpoint": "after save" }));
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Saved");
    must(&conn, "edit.undo", json!({}));
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Last");

    // Made before the save: what changed since is no longer in the history, so it asks for
    // `force` first.
    let err = conn
        .call(Call::new(
            "edit.restore",
            json!({ "checkpoint": "before save" }),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "conflict", "{err}");
    assert!(err.to_string().contains("force"), "{err}");
    must(
        &conn,
        "edit.restore",
        json!({ "checkpoint": "before save", "force": true }),
    );
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Before");
    // The deleted one stayed deleted.
    let err = conn
        .call(Call::new("edit.restore", json!({ "checkpoint": "doomed" })))
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
    drop(conn);
    engine.shutdown();
}
