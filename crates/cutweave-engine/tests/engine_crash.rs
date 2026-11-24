// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1I.04 acceptance 5: crash recovery through the engine. A child process (this test binary
//! re-run as `child_engine`) runs a headless engine on a project and renames it N times through
//! the dispatch API, printing `ACK <i>` after each acknowledgement; the parent kills it
//! (`TerminateProcess` / `SIGKILL`) mid-edit, restarts an engine on the project, sees the
//! pending recovery, restores it with `project.recover` (no acknowledged edit lost) and lets
//! autosave write it.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod engine_support;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cutweave_api::Credential;
use cutweave_engine::autosave::AutosaveConfig;
use cutweave_engine::project_io::ProjectStore;
use cutweave_engine::{Engine, EngineConfig, EngineMode};
use engine_support::{TempDir, empty_project, must, wait_for};
use serde_json::json;

/// Renames the child applies.
const N: usize = 200;
/// The parent kills the child once this many renames were acknowledged.
const KILL_AFTER: usize = 25;
const CHILD_ENV: &str = "CUTWEAVE_ENGINE_CRASH_CHILD";

/// Child half: edits the project in `CUTWEAVE_ENGINE_CRASH_CHILD` until killed.
#[test]
fn child_engine() {
    let Ok(folder) = std::env::var(CHILD_ENV) else {
        return; // a normal test run: nothing to do
    };
    let mut config = EngineConfig::new(EngineMode::Headless);
    config.project = Some(folder.into());
    let engine = Engine::start(config).unwrap();
    let conn = engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap();
    let stdout = std::io::stdout();
    println!("READY");
    for i in 0..N {
        must(
            &conn,
            "project.rename",
            json!({ "name": format!("Edit {i}") }),
        );
        let mut out = stdout.lock();
        let _ = writeln!(out, "ACK {i}");
        let _ = out.flush();
    }
    println!("DONE");
    // The parent kills us; never shut down cleanly.
    std::thread::sleep(Duration::from_secs(120));
    drop(engine);
}

#[test]
fn an_engine_killed_mid_edit_recovers_through_project_recover_and_autosaves() {
    let t = TempDir::new("crash");
    let folder = empty_project(t.path(), "Crash");
    {
        let mut config = EngineConfig::new(EngineMode::Oneshot);
        config.project = Some(folder.clone());
        let engine = Engine::start(config).unwrap();
        let conn = engine
            .handle()
            .connect(&Credential::Agent("cli".into()))
            .unwrap();
        must(&conn, "project.new", json!({ "name": "Saved" }));
        must(&conn, "project.save", json!({}));
    }

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_engine", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, &folder)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut last_acked = None;
    for line in lines.by_ref() {
        let line = line.unwrap();
        if let Some(i) = line.strip_prefix("ACK ") {
            let i: usize = i.trim().parse().unwrap();
            last_acked = Some(i);
            if i + 1 >= KILL_AFTER {
                break;
            }
        } else if line.starts_with("DONE") {
            break;
        }
    }
    child.kill().unwrap(); // no cleanup runs in the child
    child.wait().unwrap();
    for line in lines {
        let Ok(line) = line else { break };
        if let Some(i) = line.strip_prefix("ACK ") {
            last_acked = Some(i.trim().parse().unwrap());
        }
    }
    let last_acked = last_acked.expect("the child acknowledged nothing");

    // Restart on the project: the killed engine's lock is stale and taken over; the journal
    // holds every acknowledged rename (and at most the one in flight).
    let mut config = EngineConfig::new(EngineMode::Headless);
    config.project = Some(folder.clone());
    // An idle long enough that autosave cannot save between replaying the journal and building
    // `project.recover`'s reply, even on a loaded machine; the reply must still show the restored
    // work as unsaved.
    config.autosave = Some(AutosaveConfig {
        idle: Duration::from_secs(1),
        poll: Duration::from_millis(20),
        ..AutosaveConfig::default()
    });
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    let pending = h.status().unwrap().recovery.expect("no recovery offered");
    let records = usize::try_from(pending.records).unwrap();
    assert!(
        records > last_acked && records <= last_acked + 2,
        "journal has {records} renames, {} were acknowledged",
        last_acked + 1
    );
    let conn = h.connect(&Credential::Agent("cli".into())).unwrap();
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Saved");
    let mut events = h.subscribe();
    let out = must(&conn, "project.recover", json!({}));
    let applied = usize::try_from(out["applied"].as_u64().unwrap()).unwrap();
    assert_eq!(applied, records);
    let name = format!("Edit {}", applied - 1);
    assert_eq!(must(&conn, "project.info", json!({}))["name"], name);
    assert_eq!(out["project"]["dirty"], true);

    // Autosave writes the restored work through the engine.
    wait_for(&mut events, Duration::from_secs(10), |e| {
        e.name() == "project.saved"
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while h.status().unwrap().dirty && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!h.status().unwrap().dirty);
    drop(conn);
    engine.shutdown();
    let reopened = ProjectStore::open(&folder).unwrap();
    assert_eq!(reopened.project.meta.name, name);
    assert!(reopened.report.recovery.is_none());
    println!(
        "killed after {} acknowledged renames; recovered {applied}",
        last_acked + 1
    );
}
