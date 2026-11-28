// SPDX-License-Identifier: GPL-3.0-or-later
//! The crash-injection harness (WP-2J.05) in the gate: one crash at every crash point against
//! real engine processes, and the harness's self-test. The 20-crash run is
//! `cutweave-bench crash --count 20 --seed N` (`cargo run -p cutweave-bench -- crash`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use cutweave_bench::crash::clients::{Answer, Editor};
use cutweave_bench::crash::{CrashOptions, Fault, Point, run};
use cutweave_bench::fixture::{Fixture, FixtureSpec, generate};
use cutweave_bench::harness::{ChildEngine, ChildSpec};
use serde_json::{Value, json};

fn options(tag: &str, count: u32, seed: u64) -> CrashOptions {
    CrashOptions {
        exe: PathBuf::from(env!("CARGO_BIN_EXE_cutweave-bench")),
        scratch: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(tag),
        count,
        seed,
        points: None,
        media: false,
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

/// A stub-media engine on a fresh copy of the smoke project in `target/tmp/<tag>`; files in
/// its `imports` folder can be imported.
fn smoke_project(tag: &str) -> (Fixture, ChildSpec, PathBuf) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    let imports = dir.join("imports");
    std::fs::create_dir_all(&imports).unwrap();
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
        roots: vec![imports.clone()],
    };
    (fixture, spec, imports)
}

/// A call that must be acknowledged; returns its output.
fn done(editor: &mut Editor, method: &str, params: &Value, request_id: &str) -> Value {
    match editor.call(method, params, Some(request_id), None).unwrap() {
        Answer::Done { output, .. } => output,
        Answer::Refused(why) => panic!("{method} was refused: {why}"),
    }
}

/// Issue #236, found by the harness (seed 104): a save writes each source's path relative to
/// the project into the file, but the open document keeps what it had (nothing, for a file
/// imported in this session). A later change that carries the whole asset as its old value
/// (removing it, undoing its import) then does not match the saved document when the journal
/// is replayed, so the recovery stops there and drops that edit and every one after it.
#[test]
fn edits_after_a_save_survive_a_crash_when_one_removes_media_imported_before_the_save() {
    let (fixture, spec, imports) = smoke_project("crash-saved-media");
    let id = |n: u32| format!("0190f5c2-0236-7000-8000-{n:012x}");
    let sequence = &fixture.manifest.sequence;
    let file = imports.join("late.mp4");
    std::fs::write(&file, b"stub media").unwrap();

    let mut engine = ChildEngine::spawn(&spec).unwrap();
    let mut editor = Editor::connect(&engine).unwrap();
    let imported = done(
        &mut editor,
        "media.import",
        &json!({"paths": [file]}),
        &id(1),
    );
    let media = imported["media_ids"][0].clone();
    done(&mut editor, "project.save", &json!({}), &id(2));
    done(&mut editor, "media.remove", &json!({"id": media}), &id(3));
    for (n, t) in [(4, 0), (5, 705_600_000_i64)] {
        let marker = json!({"sequence": sequence, "t": t});
        done(&mut editor, "timeline.add_marker", &marker, &id(n));
    }
    let acknowledged = editor.query("project.snapshot", &json!({})).unwrap();
    engine.kill();

    let engine = ChildEngine::spawn(&spec).unwrap();
    let mut editor = Editor::connect(&engine).unwrap();
    let hello = json!({"client": "crash-test", "version": "0"});
    let offered = editor.query("rpc.hello", &hello).unwrap()["project"]["recovery"].take();
    assert_eq!(offered["records"], 3, "{offered}");
    let recovered = done(&mut editor, "project.recover", &json!({}), &id(6));
    assert_eq!(recovered["applied"], 3, "{recovered}");
    assert_eq!(recovered["damaged"], false, "{recovered}");
    let after = editor.query("project.snapshot", &json!({})).unwrap();
    assert_eq!(after["committed_rev"], acknowledged["committed_rev"]);
    assert_eq!(after["project"], acknowledged["project"]);
}

/// Issue #235, found by the harness at its `drag-commit` point: the journal has a drag's
/// commit, and after the recovery the commit sent again under its request id must get its
/// original result, as a live engine gives and as `docs/reference/rpc.md` says.
#[test]
fn a_drag_commit_sent_again_after_a_recovery_returns_its_original_result() {
    let (fixture, spec, _) = smoke_project("crash-commit-resend");
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
    let opened = done(&mut editor, "edit.begin_tx", &begin, &id(1));
    let tx = opened["tx"].as_str().unwrap().to_owned();
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
    let recovered = done(&mut editor, "project.recover", &json!({}), &id(4));
    assert_eq!(recovered["applied"], 1, "{recovered}");
    let again = editor
        .call("edit.commit_tx", &json!({}), Some(&id(3)), None)
        .unwrap();
    assert_eq!(again, commit, "the commit's original result");
}
