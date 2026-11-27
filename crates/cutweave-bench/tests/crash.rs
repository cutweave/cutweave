// SPDX-License-Identifier: GPL-3.0-or-later
//! The crash-injection harness (WP-2J.05) in the gate: one crash at every crash point against
//! real engine processes, and the harness's self-test. The 20-crash run is
//! `cutweave-bench crash --count 20 --seed N` (`cargo run -p cutweave-bench -- crash`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use cutweave_bench::crash::clients::{Answer, Editor};
use cutweave_bench::crash::{CrashOptions, Fault, Point, run};
use cutweave_bench::fixture::{FixtureSpec, generate};
use cutweave_bench::harness::{ChildEngine, ChildSpec};
use serde_json::json;

fn options(tag: &str, count: u32, seed: u64) -> CrashOptions {
    CrashOptions {
        exe: PathBuf::from(env!("CARGO_BIN_EXE_cutweave-bench")),
        scratch: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(tag),
        count,
        seed,
        points: None,
        fault: None,
        progress: false,
    }
}

#[test]
fn a_crash_at_every_point_loses_nothing() {
    let count = u32::try_from(Point::ALL.len()).unwrap();
    let report = run(&options("crash-smoke", count, 0x2105)).unwrap();
    assert!(report.passed(), "{}", report.table());
    let mut points: Vec<Point> = report.rows.iter().map(|r| r.point).collect();
    points.sort();
    assert_eq!(points, Point::ALL, "{}", report.table());
    assert!(report.acknowledged > 50, "{}", report.table());
    // The work is real work: most of it is accepted.
    assert!(
        report.refused * 3 < report.acknowledged,
        "{}",
        report.table()
    );
    // Every recovery replayed a journal, and the points that leave a command in flight did.
    assert!(
        report.rows.iter().all(|r| r.recovered_rev > 0),
        "{}",
        report.table()
    );
    assert!(
        report.rows.iter().any(|r| r.in_flight > 0),
        "{}",
        report.table()
    );
}

/// The harness must notice a lost edit: here the journal's last record is cut off after the
/// engine acknowledged it, and the harness is not told.
#[test]
fn an_acknowledged_edit_missing_after_the_recovery_fails_the_run() {
    let mut options = options("crash-self-test", 2, 7);
    options.points = Some(vec![Point::TornTail]);
    options.fault = Some(Fault::LoseAcknowledged);
    let report = run(&options).unwrap();
    assert!(!report.passed(), "{}", report.table());
    assert_eq!(report.rows.len(), 1, "a failed crash ends the run");
    let failure = report.failure().unwrap();
    assert!(failure.contains("acknowledged edits are lost"), "{failure}");
    assert!(report.table().contains("--seed 7"), "{}", report.table());
}

/// Issue #235, found by the harness at its `drag-commit` point: the journal has a drag's
/// commit, and after the recovery the commit sent again under its request id must get its
/// original result, as a live engine gives and as `docs/reference/rpc.md` says.
#[test]
#[ignore = "issue #235: a resent edit.commit_tx is refused after a recovery"]
fn a_drag_commit_sent_again_after_a_recovery_returns_its_original_result() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("crash-commit-resend");
    let _ = std::fs::remove_dir_all(&dir);
    let fixture = generate(&FixtureSpec::smoke()).unwrap();
    let folder = fixture.write(&dir).unwrap();
    let spec = ChildSpec {
        exe: PathBuf::from(env!("CARGO_BIN_EXE_cutweave-bench")),
        scratch: dir.join("run"),
        project: Some(folder),
        stub_media: true,
        gui: false,
        deterministic_ids: Some(1),
        env: Vec::new(),
        roots: Vec::new(),
    };
    let id = |n: u32| format!("0190f5c2-0235-7000-8000-{n:012x}");
    let manifest = &fixture.manifest;
    let clip = manifest
        .tracks
        .iter()
        .filter(|t| t.video && !t.main)
        .flat_map(|t| &t.items)
        .find(|i| i.slot_len > i.duration)
        .unwrap();
    let to = (clip.slot_start + clip.slot_len - clip.duration) * manifest.frame;

    let mut engine = ChildEngine::spawn(&spec).unwrap();
    let mut editor = Editor::connect(&engine).unwrap();
    let begin = json!({"label": "Move clip", "kind": "gesture"});
    let opened = editor
        .call("edit.begin_tx", &begin, Some(&id(1)), None)
        .unwrap();
    let tx = opened.output().unwrap()["tx"].as_str().unwrap().to_owned();
    let moved = json!({"item": clip.id, "start": to});
    let update = editor
        .call("item.move", &moved, Some(&id(2)), Some(&tx))
        .unwrap();
    assert!(matches!(update, Answer::Done { .. }), "{update:?}");
    let commit = editor
        .call("edit.commit_tx", &json!({}), Some(&id(3)), None)
        .unwrap();
    assert!(matches!(commit, Answer::Done { .. }), "{commit:?}");
    engine.kill();

    let engine = ChildEngine::spawn(&spec).unwrap();
    let mut editor = Editor::connect(&engine).unwrap();
    let recovered = editor
        .call("project.recover", &json!({}), Some(&id(4)), None)
        .unwrap();
    assert_eq!(recovered.output().unwrap()["applied"], 1, "{recovered:?}");
    let again = editor
        .call("edit.commit_tx", &json!({}), Some(&id(3)), None)
        .unwrap();
    assert_eq!(again, commit, "the commit's original result");
}
