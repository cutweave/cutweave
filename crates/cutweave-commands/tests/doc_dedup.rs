// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.03: `request_id` idempotency (arch §5.1, ADR-0015). A repeat within the 10-minute
//! wall-clock window returns the original result without re-applying; the window is measured
//! with an injectable clock and survives a restart through the journaled dedup records.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod doc_support;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use cutweave_commands::{
    ActorCtx, Applied, CmdError, CommitRecord, CommitSink, ConflictReason, ConnId, DedupRecord,
    DocActor, ManualClock, RequestId, Rev, Submit, TxId, TxKind, WallTime,
};
use cutweave_model::SeededGen;
use doc_support::{agent, config, drain, project, spawn, ui};
use serde_json::{Value, json};

const MINUTE: Duration = Duration::from_mins(1);

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(WallTime::from_millis(1_700_000_000_000)))
}

fn append_with(id: RequestId, ctx: &ActorCtx, track: cutweave_model::TrackId) -> Submit {
    Submit::new(
        ctx.clone(),
        "timeline.append_clip",
        json!({"track": track, "frames": 30}),
    )
    .with_request_id(id)
}

#[test]
fn a_repeat_within_the_window_applies_once_and_returns_the_original_result() {
    let (doc, tracks, _) = project(1);
    let clock = clock();
    let actor = DocActor::spawn(config(doc).with_clock(clock.clone())).unwrap();
    let h = actor.handle();
    let mut events = h.subscribe();
    let id = RequestId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001);

    let first = h.submit(append_with(id, &ui(), tracks[0])).wait().unwrap();
    assert_eq!(first.rev, Rev(1));
    assert!(!first.replayed);
    // One `doc.patched` (plus the history's `edit.history_changed`).
    assert_eq!(
        drain(&mut events)
            .iter()
            .filter(|e| e.as_patched().is_some())
            .count(),
        1
    );

    // 9 minutes later (fake clock): same result, nothing re-applied, no event, no new rev.
    clock.advance(9 * MINUTE);
    let again = h.submit(append_with(id, &ui(), tracks[0])).wait().unwrap();
    assert!(again.replayed);
    assert_eq!(again.output, first.output);
    assert_eq!(again.rev, first.rev);
    assert_eq!(again.ops, first.ops);
    assert_eq!(again.tx_id, first.tx_id);
    assert_eq!(h.snapshot().rev, Rev(1));
    assert_eq!(h.snapshot().doc.item_count(), 1);
    assert_eq!(drain(&mut events).len(), 0);
    assert_eq!(h.stats().replayed, 1);

    // 11 minutes after the first: outside the window, so it is a new request.
    clock.advance(2 * MINUTE);
    let fresh = h.submit(append_with(id, &ui(), tracks[0])).wait().unwrap();
    assert!(!fresh.replayed);
    assert_eq!(fresh.rev, Rev(2));
    assert_ne!(fresh.output, first.output);
    assert_eq!(h.snapshot().doc.item_count(), 2);
}

#[test]
fn the_window_is_inclusive_at_ten_minutes() {
    let (doc, tracks, _) = project(1);
    let clock = clock();
    let actor = DocActor::spawn(config(doc).with_clock(clock.clone())).unwrap();
    let h = actor.handle();
    let id = RequestId::from_u128(5);
    h.submit(append_with(id, &ui(), tracks[0])).wait().unwrap();
    clock.advance(10 * MINUTE);
    assert!(
        h.submit(append_with(id, &ui(), tracks[0]))
            .wait()
            .unwrap()
            .replayed
    );
    clock.advance(Duration::from_millis(1));
    assert!(
        !h.submit(append_with(id, &ui(), tracks[0]))
            .wait()
            .unwrap()
            .replayed
    );
}

#[test]
fn reusing_a_request_id_for_another_request_is_a_conflict() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let id = RequestId::from_u128(9);
    h.submit(append_with(id, &ui(), tracks[0])).wait().unwrap();

    let other_payload =
        Submit::new(ui(), "project.rename", json!({"name": "X"})).with_request_id(id);
    let err = h.submit(other_payload).wait().unwrap_err();
    assert!(matches!(
        err,
        CmdError::Conflict {
            reason: ConflictReason::RequestIdReused,
            current_rev: Some(Rev(1)),
            ..
        }
    ));
    // The same payload from another actor is not theirs to replay.
    let err = h
        .submit(append_with(id, &agent(), tracks[0]))
        .wait()
        .unwrap_err();
    assert!(matches!(
        err,
        CmdError::Conflict {
            reason: ConflictReason::RequestIdReused,
            ..
        }
    ));
    assert_eq!(h.snapshot().rev, Rev(1));
}

#[test]
fn key_order_and_connections_do_not_matter_for_a_resend() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let id = RequestId::from_u128(11);
    let first = h
        .submit(
            Submit::new(
                ActorCtx::ui(ConnId::new(1)),
                "timeline.append_clip",
                json!({"track": tracks[0], "frames": 30}),
            )
            .with_request_id(id),
        )
        .wait()
        .unwrap();
    // After a reconnect: a new connection of the same credential, keys in another order.
    let params: Value =
        serde_json::from_str(&format!(r#"{{"frames": 30.0, "track": "{}"}}"#, tracks[0])).unwrap();
    let resent = h
        .submit(
            Submit::new(ActorCtx::ui(ConnId::new(7)), "timeline.append_clip", params)
                .with_request_id(id),
        )
        .wait()
        .unwrap();
    assert!(resent.replayed);
    assert_eq!(resent.output, first.output);
    assert_eq!(h.snapshot().rev, Rev(1));
}

#[test]
fn failed_requests_are_not_remembered() {
    let (doc, _, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let id = RequestId::from_u128(13);
    let fail = || {
        h.submit(Submit::new(ui(), "project.rename", json!({"name": "fail"})).with_request_id(id))
            .wait()
    };
    assert_eq!(fail().unwrap_err().code(), "invalid");
    // It runs (and fails) again rather than replaying the error.
    assert_eq!(fail().unwrap_err().code(), "invalid");
    assert_eq!(h.stats().failed, 2);
    // So the id is still free for a corrected request.
    let ok = h
        .submit(Submit::new(ui(), "project.rename", json!({"name": "ok"})).with_request_id(id))
        .wait()
        .unwrap();
    assert_eq!(ok.rev, Rev(1));
}

#[test]
fn the_engine_fills_in_missing_request_ids_deterministically() {
    let run = || {
        let (doc, tracks, _) = project(1);
        let actor = spawn(doc);
        let h = actor.handle();
        (0..3)
            .map(|_| {
                h.submit(Submit::new(
                    ui(),
                    "timeline.append_clip",
                    json!({"track": tracks[0], "frames": 1}),
                ))
                .wait()
                .unwrap()
                .request_id
            })
            .collect::<Vec<_>>()
    };
    let a = run();
    assert_eq!(a, run(), "same seed, same request ids");
    assert_ne!(a[0], a[1]);
    assert_ne!(a[1], a[2]);
}

#[test]
fn the_window_survives_a_restart_through_the_journal() {
    let (doc, tracks, _) = project(1);
    let clock = clock();
    let journal: Arc<Mutex<Vec<DedupRecord>>> = Arc::default();
    let sink = {
        let journal = Arc::clone(&journal);
        move |record: &CommitRecord<'_>| {
            journal
                .lock()
                .unwrap()
                .extend(record.steps.iter().map(|s| s.record.clone()));
            Ok(())
        }
    };
    let id = RequestId::from_u128(21);
    let (first, saved) = {
        let actor = DocActor::spawn(
            config(doc)
                .with_clock(clock.clone())
                .with_journal(Box::new(sink)),
        )
        .unwrap();
        let h = actor.handle();
        let first = h.submit(append_with(id, &ui(), tracks[0])).wait().unwrap();
        (first, h.committed_snapshot())
    };

    // The "journal" round-trips through JSON like the real one will.
    let persisted: Vec<DedupRecord> =
        serde_json::from_value(serde_json::to_value(&*journal.lock().unwrap()).unwrap()).unwrap();
    assert_eq!(persisted.len(), 1);
    clock.advance(5 * MINUTE);
    // A fresh id stream: a seeded generator restarted at the same seed would reissue the ids
    // already in the document (recovery restores the generator in WP-1C.02).
    let restarted = DocActor::spawn(
        config(saved.doc.clone())
            .with_rev(saved.rev)
            .with_ids(Box::new(SeededGen::new(43)))
            .with_clock(clock.clone())
            .with_restored_requests(persisted),
    )
    .unwrap();
    let h = restarted.handle();
    let resent = h.submit(append_with(id, &ui(), tracks[0])).wait().unwrap();
    assert!(resent.replayed);
    assert_eq!(resent.output, first.output);
    assert_eq!(resent.rev, Rev(1));
    assert_eq!(h.snapshot().doc, saved.doc);

    clock.advance(6 * MINUTE);
    let later = h.submit(append_with(id, &ui(), tracks[0])).wait().unwrap();
    assert!(!later.replayed);
    assert_eq!(later.rev, Rev(2));
}

#[test]
fn requests_inside_an_open_transaction_are_remembered_until_it_closes() {
    let (doc, tracks, _) = project(2);
    let actor = spawn(doc);
    let h = actor.handle();
    let ctx = ui();

    // Committed: the step's result is kept.
    let tx = h.begin_tx(&ctx, TxKind::Gesture, "drag").wait().unwrap().id;
    let id = RequestId::from_u128(31);
    let step = h
        .submit(append_with(id, &ctx, tracks[0]).in_tx(tx))
        .wait()
        .unwrap();
    let resent = h
        .submit(append_with(id, &ctx, tracks[0]).in_tx(tx))
        .wait()
        .unwrap();
    assert!(resent.replayed);
    assert_eq!(resent.output, step.output);
    h.commit_tx(&ctx, tx).wait().unwrap();
    let after_commit = h.submit(append_with(id, &ctx, tracks[0])).wait().unwrap();
    assert!(after_commit.replayed);
    assert_eq!(after_commit.output, step.output);

    // Cancelled: nothing was applied, so the id runs again.
    let tx = h.begin_tx(&ctx, TxKind::Gesture, "drag").wait().unwrap().id;
    let id = RequestId::from_u128(32);
    let cancelled = h
        .submit(append_with(id, &ctx, tracks[1]).in_tx(tx))
        .wait()
        .unwrap();
    h.cancel_tx(&ctx, tx).wait().unwrap();
    let rerun = h.submit(append_with(id, &ctx, tracks[1])).wait().unwrap();
    assert!(!rerun.replayed);
    assert_ne!(rerun.output, cancelled.output);
    assert_eq!(h.snapshot().doc.item_count(), 2);
}

/// A journal that keeps what every commit asks it to remember, and can refuse a write.
#[derive(Clone, Default)]
struct Remembering {
    records: Arc<Mutex<Vec<DedupRecord>>>,
    refuse: Arc<Mutex<bool>>,
}

impl Remembering {
    /// The remembered requests, round-tripped through JSON like the real journal's.
    fn persisted(&self) -> Vec<DedupRecord> {
        serde_json::from_value(serde_json::to_value(&*self.records.lock().unwrap()).unwrap())
            .unwrap()
    }
}

impl CommitSink for Remembering {
    fn commit(&mut self, record: &CommitRecord<'_>) -> Result<(), CmdError> {
        if *self.refuse.lock().unwrap() {
            return Err(CmdError::internal("disk full"));
        }
        self.records
            .lock()
            .unwrap()
            .extend(record.steps.iter().map(|s| s.record.clone()));
        Ok(())
    }
}

fn request(id: u128, ctx: &ActorCtx, name: &str, params: Value) -> Submit {
    Submit::new(ctx.clone(), name, params).with_request_id(RequestId::from_u128(id))
}

fn opened_tx(opened: &Applied) -> TxId {
    serde_json::from_value(opened.output["tx"].clone()).unwrap()
}

/// Issue #235: the requests that open and commit a transaction are journaled with it, so a
/// restarted engine answers a repeat of either with the original result, like its steps.
#[test]
fn a_committed_transaction_answers_its_begin_and_commit_again_after_a_restart() {
    for kind in ["gesture", "batch", "proposal"] {
        let (doc, tracks, _) = project(1);
        let clock = clock();
        let journal = Remembering::default();
        let ctx = ui();
        let begin = || {
            let params = json!({"kind": kind, "label": "Drag"});
            request(41, &ctx, "edit.begin_tx", params)
        };
        let step = |tx: Option<TxId>| {
            let step = append_with(RequestId::from_u128(42), &ctx, tracks[0]);
            tx.map_or(step.clone(), |tx| step.in_tx(tx))
        };
        let commit = || request(43, &ctx, "edit.commit_tx", json!({}));
        let (opened, stepped, committed, saved) = {
            let actor = DocActor::spawn(
                config(doc)
                    .with_clock(clock.clone())
                    .with_journal(Box::new(journal.clone())),
            )
            .unwrap();
            let h = actor.handle();
            let opened = h.submit(begin()).wait().unwrap();
            let stepped = h.submit(step(Some(opened_tx(&opened)))).wait().unwrap();
            let committed = h.submit(commit()).wait().unwrap();
            assert_eq!(committed.output["changed"], true);
            // Live: a repeat returns the same result.
            let again = h.submit(commit()).wait().unwrap();
            assert!(again.replayed);
            assert_eq!(again.output, committed.output);
            assert_eq!(again.rev, committed.rev);
            (opened, stepped, committed, h.committed_snapshot())
        };

        let persisted = journal.persisted();
        assert_eq!(persisted.len(), 3, "begin, the step and the commit");
        clock.advance(MINUTE);
        let restarted = DocActor::spawn(
            config(saved.doc.clone())
                .with_rev(saved.rev)
                .with_ids(Box::new(SeededGen::new(43)))
                .with_clock(clock.clone())
                .with_restored_requests(persisted),
        )
        .unwrap();
        let h = restarted.handle();
        // Twice: a repeat is answered as often as it is sent.
        for _ in 0..2 {
            let again = h.submit(commit()).wait().unwrap();
            assert!(again.replayed, "{kind}");
            assert_eq!(again.output, committed.output);
            assert_eq!(again.rev, committed.rev);
            assert_eq!(again.committed_rev, committed.committed_rev);
            assert_eq!(again.ops, committed.ops);
        }
        let again = h.submit(begin()).wait().unwrap();
        assert!(again.replayed, "{kind}");
        assert_eq!(again.output, opened.output);
        let again = h.submit(step(None)).wait().unwrap();
        assert!(again.replayed);
        assert_eq!(again.output, stepped.output);
        // Nothing was applied again and the repeated begin opened nothing.
        assert_eq!(h.snapshot().doc, saved.doc);
        assert_eq!(h.snapshot().rev, saved.rev);
        assert_eq!(h.locks(&ctx).wait().unwrap().transactions.len(), 0);
    }
}

/// The request id of a journaled commit stays bound to it: another request under it is a
/// conflict, as for any remembered request.
#[test]
fn the_request_id_of_a_journaled_commit_cannot_be_reused_after_a_restart() {
    let (doc, tracks, _) = project(1);
    let journal = Remembering::default();
    let ctx = ui();
    let saved = {
        let actor = DocActor::spawn(config(doc).with_journal(Box::new(journal.clone()))).unwrap();
        let h = actor.handle();
        let begin = json!({"label": "Batch"});
        let opened = h
            .submit(request(51, &ctx, "edit.begin_tx", begin))
            .wait()
            .unwrap();
        let step = append_with(RequestId::from_u128(52), &ctx, tracks[0]);
        h.submit(step.in_tx(opened_tx(&opened))).wait().unwrap();
        h.submit(request(53, &ctx, "edit.commit_tx", json!({})))
            .wait()
            .unwrap();
        h.committed_snapshot()
    };
    let restarted = DocActor::spawn(
        config(saved.doc.clone())
            .with_rev(saved.rev)
            .with_ids(Box::new(SeededGen::new(43)))
            .with_restored_requests(journal.persisted()),
    )
    .unwrap();
    let h = restarted.handle();
    for other in [
        request(53, &ctx, "edit.cancel_tx", json!({})),
        request(53, &agent(), "edit.commit_tx", json!({})),
    ] {
        let err = h.submit(other).wait().unwrap_err();
        let reused = matches!(
            err,
            CmdError::Conflict {
                reason: ConflictReason::RequestIdReused,
                ..
            }
        );
        assert!(reused, "{err:?}");
    }
}

/// A commit the journal refuses changes nothing: the transaction stays open with the steps it
/// had, and the commit sent again (it failed, so it runs) is journaled once.
#[test]
fn a_commit_the_journal_refuses_leaves_the_transaction_as_it_was() {
    let (doc, tracks, _) = project(1);
    let journal = Remembering::default();
    let actor = DocActor::spawn(config(doc).with_journal(Box::new(journal.clone()))).unwrap();
    let h = actor.handle();
    let ctx = ui();
    let begin = json!({"kind": "gesture", "label": "Drag"});
    let opened = h
        .submit(request(61, &ctx, "edit.begin_tx", begin))
        .wait()
        .unwrap();
    let tx = opened_tx(&opened);
    let step = append_with(RequestId::from_u128(62), &ctx, tracks[0]);
    h.submit(step.in_tx(tx)).wait().unwrap();
    let commit = || request(63, &ctx, "edit.commit_tx", json!({}));

    *journal.refuse.lock().unwrap() = true;
    let err = h.submit(commit()).wait().unwrap_err();
    assert!(matches!(err, CmdError::Internal { .. }), "{err:?}");
    let steps = h
        .run(move |state| state.transaction(tx).map(|t| t.steps))
        .wait()
        .unwrap();
    assert_eq!(steps, Some(1), "still open, with its one step");
    assert_eq!(h.committed_snapshot().doc.item_count(), 0);

    *journal.refuse.lock().unwrap() = false;
    let committed = h.submit(commit()).wait().unwrap();
    assert!(!committed.replayed);
    assert_eq!(h.committed_snapshot().doc.item_count(), 1);
    let ids: Vec<RequestId> = journal
        .records
        .lock()
        .unwrap()
        .iter()
        .map(DedupRecord::request_id)
        .collect();
    assert_eq!(ids, [62, 61, 63].map(RequestId::from_u128));
}

/// A cancelled transaction never reaches the journal, and neither does one the engine died
/// with: after a restart its cancel or commit sent again is not answered from memory and
/// finds no transaction. It does not report a success: the transaction's changes are gone.
#[test]
fn a_cancel_or_commit_the_journal_never_had_is_refused_after_a_restart() {
    let (doc, tracks, _) = project(1);
    let journal = Remembering::default();
    let ctx = ui();
    let (saved, tx) = {
        let actor = DocActor::spawn(config(doc).with_journal(Box::new(journal.clone()))).unwrap();
        let h = actor.handle();
        let begin = json!({"kind": "gesture", "label": "Drag"});
        let opened = h
            .submit(request(71, &ctx, "edit.begin_tx", begin))
            .wait()
            .unwrap();
        let tx = opened_tx(&opened);
        let step = append_with(RequestId::from_u128(72), &ctx, tracks[0]);
        h.submit(step.in_tx(tx)).wait().unwrap();
        h.submit(request(73, &ctx, "edit.cancel_tx", json!({})))
            .wait()
            .unwrap();
        (h.committed_snapshot(), tx)
    };
    assert_eq!(journal.records.lock().unwrap().len(), 0);
    // The restarted engine issues transaction ids past the last journaled one, as recovery
    // does; here none was journaled, so `tx` would be the next one.
    let restarted = DocActor::spawn(config(saved.doc.clone()).with_rev(saved.rev)).unwrap();
    let h = restarted.handle();
    let none = CmdError::invalid("/tx", "this connection has no open transaction");
    for (id, name) in [(73, "edit.cancel_tx"), (74, "edit.commit_tx")] {
        let err = h
            .submit(request(id, &ctx, name, json!({})))
            .wait()
            .unwrap_err();
        assert_eq!(err, none);
        let err = h
            .submit(request(id, &ctx, name, json!({"tx": tx})))
            .wait()
            .unwrap_err();
        assert!(matches!(err, CmdError::NotFound { .. }), "{err:?}");
    }
    assert_eq!(h.snapshot().doc, saved.doc);
    assert_eq!(h.snapshot().rev, saved.rev);
}
