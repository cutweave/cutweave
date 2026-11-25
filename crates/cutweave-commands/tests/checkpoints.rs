// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2I.03: checkpoints through `edit.*`. Their journal markers (what each change made and
//! removed), deletion, recovery after a restart through a [`CheckpointLedger`] (restorable, ids
//! never reused, requests still deduplicated, `force` needed for changes made before the
//! recovered history), restore against other connections' open transactions, the restore
//! budget on a 5,000-item document, and a property test: any sequence of edits and checkpoint
//! operations rebuilds, after a restart, to the same checkpoints.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod doc_support;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use cutweave_commands::{
    ActorCtx, Applied, CheckpointLedger, CheckpointRecord, CmdError, CommitRecord, CommitSink,
    ConflictReason, ConnId, DedupRecord, DocConfig, DocState, JournaledCheckpoint, JournaledCommit,
    RequestId, Rev, Submit, TxId,
};
use cutweave_model::{ItemId, Project, TrackId};
use doc_support::{SEED, agent, items, project, registry, ui};
use proptest::prelude::*;
use serde_json::{Value, json};

fn run(s: &mut DocState, ctx: &ActorCtx, name: &str, params: Value) -> Result<Applied, CmdError> {
    s.submit(Submit::new(ctx.clone(), name, params))
}

fn ok(s: &mut DocState, ctx: &ActorCtx, name: &str, params: Value) -> Value {
    run(s, ctx, name, params).unwrap().output
}

fn append(s: &mut DocState, ctx: &ActorCtx, track: TrackId, frames: u32) -> ItemId {
    let out = ok(
        s,
        ctx,
        "timeline.append_clip",
        json!({"track": track, "frames": frames}),
    );
    serde_json::from_value(out["item"].clone()).unwrap()
}

fn rename(s: &mut DocState, ctx: &ActorCtx, item: ItemId, name: &str) {
    ok(s, ctx, "item.rename", json!({"item": item, "name": name}));
}

fn names(s: &mut DocState) -> Vec<(String, Option<String>)> {
    let out = ok(s, &ui(), "edit.list_checkpoints", json!({}));
    out["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["checkpoint"].as_str().unwrap().to_owned(),
                c["name"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

// ----- a journal that keeps everything ----------------------------------------------------------

#[derive(Clone, Debug)]
enum Entry {
    Commit(JournaledCommit),
    Marker(JournaledCheckpoint),
}

#[derive(Default)]
struct Kept {
    entries: Vec<Entry>,
    /// Refuse checkpoint markers (a failing journal).
    refuse: bool,
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Kept>>);

impl CommitSink for Recorder {
    fn commit(&mut self, record: &CommitRecord<'_>) -> Result<(), CmdError> {
        self.0
            .lock()
            .unwrap()
            .entries
            .push(Entry::Commit(record.to_owned()));
        Ok(())
    }

    fn checkpoint(&mut self, record: &CheckpointRecord<'_>) -> Result<(), CmdError> {
        let mut kept = self.0.lock().unwrap();
        if kept.refuse {
            return Err(CmdError::internal("disk full"));
        }
        // The marker's JSON form is what the journal stores and reads back.
        let json = serde_json::to_value(record).unwrap();
        assert!(json.get("doc").is_none(), "the document is not journaled");
        let back: JournaledCheckpoint = serde_json::from_value(json).unwrap();
        assert_eq!(back, record.to_owned());
        kept.entries.push(Entry::Marker(back));
        Ok(())
    }
}

impl Recorder {
    fn entries(&self) -> Vec<Entry> {
        self.0.lock().unwrap().entries.clone()
    }

    fn markers(&self) -> Vec<JournaledCheckpoint> {
        self.entries()
            .into_iter()
            .filter_map(|e| match e {
                Entry::Marker(m) => Some(m),
                Entry::Commit(_) => None,
            })
            .collect()
    }
}

fn journaled(doc: Project) -> (DocState, Recorder) {
    let recorder = Recorder::default();
    let config =
        DocConfig::deterministic(registry(), doc, SEED).with_journal(Box::new(recorder.clone()));
    (DocState::new(config), recorder)
}

/// What an engine restart rebuilds from `entries` over `initial`, as the journal does: the
/// document replayed, the history from the commits after `saved_at` (the last save: earlier
/// ones are folded into the saved document), the checkpoints and the remembered requests.
fn restart(initial: &Project, entries: &[Entry], saved_at: Rev) -> (DocState, Recorder) {
    let mut doc = initial.clone();
    let mut rev = Rev::ZERO;
    let mut ledger = CheckpointLedger::new();
    let mut commits = Vec::new();
    let mut requests: Vec<DedupRecord> = Vec::new();
    let mut max_tx = 0;
    for entry in entries {
        match entry {
            Entry::Commit(c) => {
                doc = c.patch.apply(&doc).unwrap();
                rev = c.rev;
                max_tx = max_tx.max(c.tx_id.get());
                requests.extend(c.steps.iter().map(|s| s.record.clone()));
                if c.prev_committed_rev >= saved_at {
                    commits.push(c.clone());
                }
            }
            Entry::Marker(m) => {
                ledger.replay(m, (m.rev == rev).then_some(&doc));
                requests.extend(m.steps.iter().map(|s| s.record.clone()));
            }
        }
    }
    let recorder = Recorder::default();
    let config = DocConfig::deterministic(registry(), doc, SEED ^ 1)
        .with_rev(rev)
        .with_first_tx_id(TxId::new(max_tx + 1))
        .with_restored_requests(requests)
        .with_recovered_history(commits)
        .with_recovered_checkpoints(ledger)
        .with_journal(Box::new(recorder.clone()));
    (DocState::new(config), recorder)
}

// ----- markers -----------------------------------------------------------------------------------

#[test]
fn every_change_is_journaled_as_a_marker_before_it_takes_effect() {
    let (doc, tracks, _) = project(1);
    let (mut s, journal) = journaled(doc);
    append(&mut s, &ui(), tracks[0], 10);
    let made = ok(
        &mut s,
        &agent(),
        "edit.checkpoint",
        json!({"name": "a", "note": "n"}),
    );
    assert_eq!(made["checkpoint"], json!("cp_1"));
    // Reusing the name displaces the first one.
    let again = ok(&mut s, &agent(), "edit.checkpoint", json!({"name": "a"}));
    assert_eq!(again["checkpoint"], json!("cp_2"));
    let deleted = ok(
        &mut s,
        &ui(),
        "edit.delete_checkpoint",
        json!({"checkpoint": "a"}),
    );
    assert_eq!(deleted["checkpoint"], json!("cp_2"));
    assert_eq!(deleted["name"], json!("a"));
    assert!(names(&mut s).is_empty());

    let markers = journal.markers();
    assert_eq!(markers.len(), 3);
    let first = &markers[0];
    assert_eq!(first.rev, Rev(1));
    assert_eq!(first.created.as_ref().unwrap().note.as_deref(), Some("n"));
    assert!(first.removed.is_empty());
    assert_eq!(first.issued, 1);
    // The request rides along, with the result a resend gets.
    assert_eq!(first.steps[0].command, "edit.checkpoint");
    assert_eq!(first.steps[0].record.applied.output, made);
    assert_eq!(
        markers[1].removed,
        [cutweave_commands::CheckpointId::new(1)]
    );
    assert!(markers[2].created.is_none());
    assert_eq!(
        markers[2].removed,
        [cutweave_commands::CheckpointId::new(2)]
    );
    assert_eq!(markers[2].issued, 2);
    s.verify().unwrap();
}

#[test]
fn markers_name_what_the_limits_trim() {
    let (doc, _, _) = project(1);
    let (mut s, journal) = journaled(doc);
    for _ in 0..cutweave_commands::MAX_CHECKPOINTS {
        ok(&mut s, &ui(), "edit.checkpoint", json!({}));
    }
    // Using the oldest keeps it: the second oldest is trimmed instead.
    ok(&mut s, &ui(), "edit.restore", json!({"checkpoint": "cp_1"}));
    ok(&mut s, &ui(), "edit.checkpoint", json!({}));
    let last = journal.markers().pop().unwrap();
    assert_eq!(last.removed, [cutweave_commands::CheckpointId::new(2)]);
    assert_eq!(names(&mut s).len(), cutweave_commands::MAX_CHECKPOINTS);
    s.verify().unwrap();
}

#[test]
fn a_journal_error_leaves_the_checkpoints_alone() {
    let (doc, _, _) = project(1);
    let (mut s, journal) = journaled(doc);
    ok(&mut s, &ui(), "edit.checkpoint", json!({"name": "kept"}));
    journal.0.lock().unwrap().refuse = true;
    let err = run(&mut s, &ui(), "edit.checkpoint", json!({"name": "kept"})).unwrap_err();
    assert_eq!(err.code(), "internal");
    let err = run(
        &mut s,
        &ui(),
        "edit.delete_checkpoint",
        json!({"checkpoint": "kept"}),
    )
    .unwrap_err();
    assert_eq!(err.code(), "internal");
    assert_eq!(
        names(&mut s),
        [("cp_1".to_owned(), Some("kept".to_owned()))]
    );
    journal.0.lock().unwrap().refuse = false;
    // The failed attempt spent no id.
    let next = ok(&mut s, &ui(), "edit.checkpoint", json!({}));
    assert_eq!(next["checkpoint"], json!("cp_2"));
}

#[test]
fn deleting_an_unknown_checkpoint_is_not_found() {
    let (doc, _, _) = project(1);
    let (mut s, journal) = journaled(doc);
    let err = run(
        &mut s,
        &ui(),
        "edit.delete_checkpoint",
        json!({"checkpoint": "cp_9"}),
    )
    .unwrap_err();
    assert_eq!(err.code(), "not_found");
    assert!(journal.markers().is_empty());
}

// ----- after a restart ---------------------------------------------------------------------------

#[test]
fn checkpoints_come_back_after_a_restart_and_restore() {
    let (initial, tracks, _) = project(2);
    let (mut s, journal) = journaled(initial.clone());
    let x = append(&mut s, &agent(), tracks[0], 10);
    ok(
        &mut s,
        &agent(),
        "edit.checkpoint",
        json!({"name": "before trim"}),
    );
    let at_checkpoint = s.committed_doc().clone();
    rename(&mut s, &agent(), x, "trimmed");
    append(&mut s, &agent(), tracks[1], 5);
    ok(&mut s, &agent(), "edit.checkpoint", json!({}));
    ok(&mut s, &agent(), "edit.checkpoint", json!({"name": "gone"}));
    ok(
        &mut s,
        &agent(),
        "edit.delete_checkpoint",
        json!({"checkpoint": "gone"}),
    );
    let before_restart = s.committed_doc().clone();
    let listed = names(&mut s);

    let (mut r, _) = restart(&initial, &journal.entries(), Rev::ZERO);
    r.verify().unwrap();
    assert_eq!(r.committed_doc(), &before_restart);
    assert_eq!(names(&mut r), listed);
    // The agent's own changes since the checkpoint are in the recovered history: no force.
    let restored = ok(
        &mut r,
        &agent(),
        "edit.restore",
        json!({"checkpoint": "before trim"}),
    );
    assert_eq!(r.committed_doc(), &at_checkpoint);
    assert_eq!(restored["reverted_unknown"], json!(false));
    // Undoable like any restore.
    ok(&mut r, &agent(), "edit.undo", json!({}));
    assert_eq!(r.committed_doc(), &before_restart);
    // Ids continue where they were: `cp_3` (deleted) is never handed out again.
    let next = ok(&mut r, &agent(), "edit.checkpoint", json!({}));
    assert_eq!(next["checkpoint"], json!("cp_4"));
    r.verify().unwrap();
}

#[test]
fn a_checkpoint_from_before_the_last_save_needs_force_after_a_restart() {
    let (initial, tracks, _) = project(1);
    let (mut s, journal) = journaled(initial.clone());
    let x = append(&mut s, &agent(), tracks[0], 10);
    ok(
        &mut s,
        &agent(),
        "edit.checkpoint",
        json!({"name": "early"}),
    );
    let early = s.committed_doc().clone();
    rename(&mut s, &agent(), x, "one");
    let saved_at = s.committed_rev();
    rename(&mut s, &agent(), x, "two");

    // Only the transactions after the save are in the recovered history.
    let (mut r, _) = restart(&initial, &journal.entries(), saved_at);
    let err = run(
        &mut r,
        &agent(),
        "edit.restore",
        json!({"checkpoint": "early"}),
    )
    .unwrap_err();
    assert!(
        matches!(&err, CmdError::Conflict { reason: ConflictReason::HistoryTargetModified, detail, .. }
            if detail.contains("restarted") && detail.contains("force")),
        "{err}"
    );
    let forced = ok(
        &mut r,
        &agent(),
        "edit.restore",
        json!({"checkpoint": "early", "force": true}),
    );
    assert_eq!(forced["reverted_unknown"], json!(true));
    assert_eq!(r.committed_doc(), &early);
}

#[test]
fn a_resent_checkpoint_request_after_a_restart_returns_the_original() {
    let (initial, _, _) = project(1);
    let (mut s, journal) = journaled(initial.clone());
    let ctx = ui();
    let request = Submit::new(ctx.clone(), "edit.checkpoint", json!({"name": "once"}))
        .with_request_id(RequestId::from_u128(0x1234));
    let first = s.submit(request.clone()).unwrap();
    let deletion = Submit::new(ctx, "edit.delete_checkpoint", json!({"checkpoint": "cp_9"}));
    assert!(s.submit(deletion).is_err());

    let (mut r, _) = restart(&initial, &journal.entries(), Rev::ZERO);
    let again = r.submit(request).unwrap();
    assert!(again.replayed);
    assert_eq!(again.output, first.output);
    assert_eq!(names(&mut r).len(), 1);
}

// ----- restore and other connections ----------------------------------------------------------

#[test]
fn restore_keeps_other_connections_open_transactions_and_respects_their_locks() {
    let (doc, tracks, _) = project(2);
    let (mut s, _) = journaled(doc);
    let x = append(&mut s, &agent(), tracks[0], 10);
    let y = append(&mut s, &ui(), tracks[1], 10);
    ok(&mut s, &agent(), "edit.checkpoint", json!({"name": "cp"}));
    rename(&mut s, &agent(), x, "agent edit");

    // The person drags another clip: a gesture on y, untouched by the restore.
    let person = ActorCtx::ui(ConnId::new(7));
    let tx = ok(
        &mut s,
        &person,
        "edit.begin_tx",
        json!({"kind": "gesture", "label": "Move"}),
    )["tx"]
        .clone();
    let tx: TxId = serde_json::from_value(tx).unwrap();
    s.submit(
        Submit::new(
            person.clone(),
            "item.rename",
            json!({"item": y, "name": "dragging"}),
        )
        .in_tx(tx),
    )
    .unwrap();
    ok(
        &mut s,
        &agent(),
        "edit.restore",
        json!({"checkpoint": "cp"}),
    );
    assert_eq!(doc_support::item_name(s.committed_doc(), x), None);
    // The gesture is still open, still live, and commits.
    assert_eq!(
        doc_support::item_name(s.doc(), y).as_deref(),
        Some("dragging")
    );
    ok(&mut s, &person, "edit.commit_tx", json!({}));
    assert_eq!(
        doc_support::item_name(s.committed_doc(), y).as_deref(),
        Some("dragging")
    );
    s.verify().unwrap();

    // A gesture on the very item the restore would revert: the restore waits for it.
    ok(&mut s, &agent(), "edit.checkpoint", json!({"name": "cp2"}));
    rename(&mut s, &agent(), x, "again");
    let tx = ok(
        &mut s,
        &person,
        "edit.begin_tx",
        json!({"kind": "gesture", "label": "Move"}),
    )["tx"]
        .clone();
    let tx: TxId = serde_json::from_value(tx).unwrap();
    s.submit(
        Submit::new(
            person.clone(),
            "item.rename",
            json!({"item": x, "name": "held"}),
        )
        .in_tx(tx),
    )
    .unwrap();
    let before = s.committed_doc().clone();
    let err = run(
        &mut s,
        &agent(),
        "edit.restore",
        json!({"checkpoint": "cp2", "force": true}),
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            CmdError::Conflict {
                reason: ConflictReason::ItemInGesture,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(s.committed_doc(), &before);
    s.verify().unwrap();
}

// ----- performance -------------------------------------------------------------------------------

/// Arch acceptance (WP-2I.03): a restore on a 5,000-item document completes in under 50 ms.
#[test]
fn restore_on_a_5000_item_document_takes_under_50_ms() {
    let (doc, tracks, _) = project(10);
    let mut s = DocState::new(DocConfig::deterministic(registry(), doc, SEED));
    let mut ids = Vec::with_capacity(5_000);
    for i in 0..5_000 {
        ids.push(append(&mut s, &ui(), tracks[i % tracks.len()], 30));
    }
    ok(&mut s, &ui(), "edit.checkpoint", json!({"name": "base"}));
    let base = s.committed_doc().clone();
    // 500 changes spread over the document: renames, deletions and new clips.
    for (n, item) in ids.iter().step_by(10).enumerate() {
        match n % 3 {
            0 => rename(&mut s, &ui(), *item, &format!("r{n}")),
            1 => {
                ok(&mut s, &ui(), "item.delete", json!({"item": item}));
            }
            _ => {
                append(&mut s, &ui(), tracks[n % tracks.len()], 15);
            }
        }
    }
    let edited = s.committed_doc().clone();
    let mut times = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        ok(&mut s, &ui(), "edit.restore", json!({"checkpoint": "base"}));
        times.push(t.elapsed());
        assert_eq!(s.committed_doc(), &base);
        ok(&mut s, &ui(), "edit.undo", json!({}));
        assert_eq!(s.committed_doc(), &edited);
    }
    let best = times.iter().min().copied().unwrap();
    println!("restore of 500 changes on 5,000 items: {times:?}");
    assert!(best < Duration::from_millis(50), "restore took {times:?}");
}

// ----- property: a restart rebuilds the same checkpoints ------------------------------------------

#[derive(Clone, Debug)]
enum Op {
    Append(usize),
    Rename(usize),
    Delete(usize),
    Undo(bool),
    Checkpoint(Option<&'static str>),
    DeleteCheckpoint(u64),
    Restore(u64),
}

const NAMES: [&str; 3] = ["alpha", "beta", "gamma"];

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0..2usize).prop_map(Op::Append),
        2 => any::<usize>().prop_map(Op::Rename),
        1 => any::<usize>().prop_map(Op::Delete),
        1 => any::<bool>().prop_map(Op::Undo),
        3 => prop::option::of(prop::sample::select(NAMES.to_vec())).prop_map(Op::Checkpoint),
        1 => (1..12u64).prop_map(Op::DeleteCheckpoint),
        1 => (1..12u64).prop_map(Op::Restore),
    ]
}

fn apply_op(s: &mut DocState, tracks: &[TrackId], op: &Op, n: usize) {
    let actor = if n % 3 == 0 { agent() } else { ui() };
    let all = items(s.committed_doc());
    let pick = |i: usize| (!all.is_empty()).then(|| all[i % all.len()]);
    let _ = match op {
        Op::Append(t) => run(
            s,
            &actor,
            "timeline.append_clip",
            json!({"track": tracks[*t], "frames": 5}),
        ),
        Op::Rename(i) => match pick(*i) {
            Some(item) => run(
                s,
                &actor,
                "item.rename",
                json!({"item": item, "name": format!("n{n}")}),
            ),
            None => return,
        },
        Op::Delete(i) => match pick(*i) {
            Some(item) => run(s, &actor, "item.delete", json!({"item": item})),
            None => return,
        },
        Op::Undo(anyone) => run(
            s,
            &actor,
            "edit.undo",
            if *anyone {
                json!({"actor": "*"})
            } else {
                json!({})
            },
        ),
        Op::Checkpoint(name) => run(s, &actor, "edit.checkpoint", json!({"name": name})),
        Op::DeleteCheckpoint(id) => run(
            s,
            &actor,
            "edit.delete_checkpoint",
            json!({"checkpoint": format!("cp_{id}")}),
        ),
        Op::Restore(id) => run(
            s,
            &actor,
            "edit.restore",
            json!({"checkpoint": format!("cp_{id}"), "force": true}),
        ),
    };
}

/// The document each checkpoint restores to (restore, read, undo).
fn restored_docs(s: &mut DocState) -> Vec<(String, Project)> {
    let ids: Vec<String> = names(s).into_iter().map(|(id, _)| id).collect();
    let mut out = Vec::new();
    for id in ids {
        let restored = ok(
            s,
            &ui(),
            "edit.restore",
            json!({"checkpoint": id, "force": true}),
        );
        out.push((id, s.committed_doc().clone()));
        if !restored["tx"].is_null() {
            ok(s, &ui(), "edit.undo", json!({"actor": "user"}));
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn a_restart_rebuilds_the_same_checkpoints(ops in prop::collection::vec(arb_op(), 1..60)) {
        let (initial, tracks, _) = project(2);
        let (mut s, journal) = journaled(initial.clone());
        for (n, op) in ops.iter().enumerate() {
            apply_op(&mut s, &tracks, op, n);
        }
        s.verify().unwrap();
        let (mut r, _) = restart(&initial, &journal.entries(), Rev::ZERO);
        r.verify().unwrap();
        prop_assert_eq!(r.committed_doc(), s.committed_doc());
        prop_assert_eq!(names(&mut r), names(&mut s));
        let live = restored_docs(&mut s);
        let recovered = restored_docs(&mut r);
        prop_assert_eq!(live, recovered);
        // Both hand out the same next id.
        let a = ok(&mut s, &ui(), "edit.checkpoint", json!({}));
        let b = ok(&mut r, &ui(), "edit.checkpoint", json!({}));
        prop_assert_eq!(&a["checkpoint"], &b["checkpoint"]);
    }
}
