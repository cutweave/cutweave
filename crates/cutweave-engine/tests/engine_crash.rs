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
use cutweave_commands::{RequestId, TxId};
use cutweave_engine::autosave::AutosaveConfig;
use cutweave_engine::project_io::ProjectStore;
use cutweave_engine::{Call, Engine, EngineConfig, EngineMode};
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

/// Starts the child engine on `folder`, kills it after `KILL_AFTER` acknowledged renames and
/// returns the last acknowledged index.
fn crash_child_mid_edit(folder: &std::path::Path) -> usize {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_engine", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, folder)
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
    last_acked.expect("the child acknowledged nothing")
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

    let last_acked = crash_child_mid_edit(&folder);

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

/// Issue #145: after a crash Restore, a save must leave the engine clean and tell clients so
/// (`project.dirty` with `dirty: false`), or the editor keeps its unsaved dot.
#[test]
fn a_save_after_project_recover_reports_clean() {
    let t = TempDir::new("crash-save");
    let folder = empty_project(t.path(), "CrashSave");
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
    crash_child_mid_edit(&folder);

    let mut config = EngineConfig::new(EngineMode::Headless);
    config.project = Some(folder.clone());
    // Autosave must not save for us: only the explicit save below does.
    config.autosave = Some(AutosaveConfig {
        idle: Duration::from_secs(3600),
        interval: Duration::from_secs(3600),
        poll: Duration::from_secs(3600),
        ..AutosaveConfig::default()
    });
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    let conn = h.connect(&Credential::Agent("cli".into())).unwrap();
    let mut events = h.subscribe();
    let out = must(&conn, "project.recover", json!({}));
    assert_eq!(out["project"]["dirty"], true);
    // Save at once, before the first autosave tick could publish `project.dirty`: a client
    // that follows only events (it saw `dirty: true` in the status) must be told it is clean.
    must(&conn, "project.save", json!({}));
    wait_for(&mut events, Duration::from_secs(10), |e| {
        e.name() == "project.dirty" && e.payload()["dirty"] == false
    });
    assert!(!h.status().unwrap().dirty);
    drop(conn);
    engine.shutdown();
    let reopened = ProjectStore::open(&folder).unwrap();
    assert!(reopened.project.meta.name.starts_with("Edit "));
}

const OPEN_TX_CHILD_ENV: &str = "CUTWEAVE_ENGINE_CRASH_OPEN_TX_CHILD";

/// Child half of the issue #251 test: one journaled rename, then a gesture with a rename in
/// it that is still open when the parent kills us. Prints the gesture's id as `TX <id>`.
#[test]
fn child_engine_with_an_open_gesture() {
    let Ok(folder) = std::env::var(OPEN_TX_CHILD_ENV) else {
        return; // a normal test run: nothing to do
    };
    let mut config = EngineConfig::new(EngineMode::Headless);
    config.project = Some(folder.into());
    let engine = Engine::start(config).unwrap();
    let conn = engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap();
    must(&conn, "project.rename", json!({ "name": "Journaled" }));
    let begin = json!({ "kind": "gesture", "label": "Drag" });
    let opened = must(&conn, "edit.begin_tx", begin);
    let tx: TxId = serde_json::from_value(opened["tx"].clone()).unwrap();
    conn.call(Call::new("project.rename", json!({ "name": "Lost" })).in_tx(tx))
        .unwrap();
    let stdout = std::io::stdout();
    {
        let mut out = stdout.lock();
        // Its own line: the test harness prints the test's name before the first one.
        let _ = writeln!(out, "READY");
        let _ = writeln!(out, "TX {tx}");
        let _ = out.flush();
    }
    // The parent kills us; never shut down cleanly.
    std::thread::sleep(Duration::from_secs(120));
    drop(engine);
}

/// Issue #251: an engine is killed with gesture A open (the journal never saw it). After the
/// restart a client opens gesture B and then sends A's commit and cancel again, naming A. The
/// restarted engine numbers its transactions past every id the killed one issued, so neither
/// reaches B: both are `not_found`, and B stays open with its change until its owner commits.
#[test]
fn a_close_sent_again_for_a_gesture_the_engine_died_with_never_reaches_a_new_one() {
    let t = TempDir::new("crash-tx-ids");
    let folder = empty_project(t.path(), "CrashTx");
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
    let a: TxId = {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "child_engine_with_an_open_gesture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(OPEN_TX_CHILD_ENV, &folder)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let id = lines
            .map_while(Result::ok)
            .find_map(|line| line.strip_prefix("TX ").map(|id| id.trim().to_owned()));
        child.kill().unwrap(); // no cleanup runs in the child
        child.wait().unwrap();
        id.expect("the child opened no gesture").parse().unwrap()
    };

    let mut config = EngineConfig::new(EngineMode::Headless);
    config.project = Some(folder.clone());
    config.autosave = Some(AutosaveConfig {
        idle: Duration::from_secs(3600),
        interval: Duration::from_secs(3600),
        poll: Duration::from_secs(3600),
        ..AutosaveConfig::default()
    });
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    let conn = h.connect(&Credential::Agent("cli".into())).unwrap();
    must(&conn, "project.recover", json!({}));
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Journaled");

    let begin = json!({ "kind": "gesture", "label": "Drag" });
    let opened = must(&conn, "edit.begin_tx", begin);
    let b: TxId = serde_json::from_value(opened["tx"].clone()).unwrap();
    assert!(b > a, "{b} was issued after a restart that followed {a}");
    conn.call(Call::new("project.rename", json!({ "name": "During B" })).in_tx(b))
        .unwrap();

    for (n, close) in [(1, "edit.commit_tx"), (2, "edit.cancel_tx")] {
        let id = RequestId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 | n);
        let err = conn
            .call(Call::new(close, json!({ "tx": a })).with_request_id(id))
            .unwrap_err();
        assert_eq!(err.code(), "not_found", "{close}: {err}");
        let locks = must(&conn, "edit.locks", json!({}));
        let open = locks["transactions"].as_array().unwrap();
        assert_eq!(open.len(), 1, "{close}: {locks}");
        assert_eq!(open[0]["tx"], json!(b), "{close}: {locks}");
        assert_eq!(h.snapshot().unwrap().doc.meta.name, "During B");
    }
    let committed = must(&conn, "edit.commit_tx", json!({ "tx": b }));
    assert_eq!(committed["changed"], true);
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "During B");
    drop(conn);
    engine.shutdown();
}

const CLOSES_CHILD_ENV: &str = "CUTWEAVE_ENGINE_CRASH_CLOSES_CHILD";

fn close_id(n: u128) -> RequestId {
    RequestId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0100 | n)
}

/// Child half of the ADR-0043 kill test: one journaled rename; a gesture with a rename in it,
/// cancelled; a batch that changes nothing, committed. Prints both transaction ids and both
/// results, then waits to be killed (with a third gesture open, so nothing shuts down).
#[test]
fn child_engine_that_closes_transactions_without_a_change() {
    let Ok(folder) = std::env::var(CLOSES_CHILD_ENV) else {
        return; // a normal test run: nothing to do
    };
    let mut config = EngineConfig::new(EngineMode::Headless);
    config.project = Some(folder.into());
    let engine = Engine::start(config).unwrap();
    let conn = engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap();
    must(&conn, "project.rename", json!({ "name": "Journaled" }));
    let begin = json!({ "kind": "gesture", "label": "Drag" });
    let opened = must(&conn, "edit.begin_tx", begin.clone());
    let a: TxId = serde_json::from_value(opened["tx"].clone()).unwrap();
    conn.call(Call::new("project.rename", json!({ "name": "Dropped" })).in_tx(a))
        .unwrap();
    let cancelled = conn
        .call(Call::new("edit.cancel_tx", json!({ "tx": a })).with_request_id(close_id(1)))
        .unwrap();
    let opened = must(&conn, "edit.begin_tx", json!({ "label": "Batch" }));
    let b: TxId = serde_json::from_value(opened["tx"].clone()).unwrap();
    let committed = conn
        .call(Call::new("edit.commit_tx", json!({ "tx": b })).with_request_id(close_id(2)))
        .unwrap();
    must(&conn, "edit.begin_tx", begin);
    let stdout = std::io::stdout();
    {
        let mut out = stdout.lock();
        // Its own line: the test harness prints the test's name before the first one.
        let _ = writeln!(out, "READY");
        let _ = writeln!(out, "CANCELLED {a} {}", cancelled.output);
        let _ = writeln!(out, "COMMITTED {b} {}", committed.output);
        let _ = writeln!(out, "DONE");
        let _ = out.flush();
    }
    // The parent kills us; never shut down cleanly.
    std::thread::sleep(Duration::from_secs(120));
    drop(engine);
}

/// ADR-0043 through a real kill: the engine cancelled one transaction and committed another
/// that changed nothing, acknowledged both, and was killed. After the restart the same
/// requests sent again get the results the killed engine gave, marked as repeats; nothing is
/// open, the document is the journaled one, and a close the journal never saw is refused.
#[test]
fn a_cancel_and_an_empty_commit_sent_again_after_a_kill_get_their_results_back() {
    let t = TempDir::new("crash-closes");
    let folder = empty_project(t.path(), "CrashCloses");
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
    let (cancelled, committed) = {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "child_engine_that_closes_transactions_without_a_change",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CLOSES_CHILD_ENV, &folder)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let (mut cancelled, mut committed) = (None, None);
        for line in lines.by_ref().map_while(Result::ok) {
            let closed = |rest: &str| {
                let (tx, output) = rest.split_once(' ').unwrap();
                let tx: TxId = tx.parse().unwrap();
                let output: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
                (tx, output)
            };
            if let Some(rest) = line.strip_prefix("CANCELLED ") {
                cancelled = Some(closed(rest));
            } else if let Some(rest) = line.strip_prefix("COMMITTED ") {
                committed = Some(closed(rest));
            } else if line.starts_with("DONE") {
                break;
            }
        }
        child.kill().unwrap(); // no cleanup runs in the child
        child.wait().unwrap();
        (
            cancelled.expect("the child cancelled nothing"),
            committed.expect("the child committed nothing"),
        )
    };

    let mut config = EngineConfig::new(EngineMode::Headless);
    config.project = Some(folder.clone());
    config.autosave = Some(AutosaveConfig {
        idle: Duration::from_secs(3600),
        interval: Duration::from_secs(3600),
        poll: Duration::from_secs(3600),
        ..AutosaveConfig::default()
    });
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    let conn = h.connect(&Credential::Agent("cli".into())).unwrap();
    must(&conn, "project.recover", json!({}));
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Journaled");

    for _ in 0..2 {
        for (n, close, (tx, output)) in [
            (1, "edit.cancel_tx", &cancelled),
            (2, "edit.commit_tx", &committed),
        ] {
            let again = conn
                .call(Call::new(close, json!({ "tx": tx })).with_request_id(close_id(n)))
                .unwrap_or_else(|e| panic!("{close} sent again was refused: {e}"));
            assert_eq!(&again.output, output, "{close}");
            assert!(again.applied.is_some_and(|a| a.replayed), "{close}");
        }
    }
    assert_eq!(committed.1["changed"], false);
    let locks = must(&conn, "edit.locks", json!({}));
    assert_eq!(
        locks["transactions"].as_array().unwrap().len(),
        0,
        "{locks}"
    );
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Journaled");
    // The same closes under request ids the killed engine never saw: honest refusals.
    for (n, close, (tx, _)) in [
        (3, "edit.cancel_tx", &cancelled),
        (4, "edit.commit_tx", &committed),
    ] {
        let err = conn
            .call(Call::new(close, json!({ "tx": tx })).with_request_id(close_id(n)))
            .unwrap_err();
        assert_eq!(err.code(), "not_found", "{close}: {err}");
    }
    drop(conn);
    engine.shutdown();
}
