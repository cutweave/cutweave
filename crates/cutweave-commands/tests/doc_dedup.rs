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
    DocActor, JournaledTxMarker, ManualClock, RequestId, Rev, Submit, TX_IDS_RESERVED, TxId,
    TxKind, TxMarker, TxOutcome, WallTime,
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
    /// The highest transaction id a marker said may be handed out, and the markers so far.
    issued: Arc<Mutex<u64>>,
    markers: Arc<Mutex<Vec<JournaledTxMarker>>>,
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

    fn tx_marker(&mut self, record: &TxMarker<'_>) -> Result<(), CmdError> {
        if *self.refuse.lock().unwrap() {
            return Err(CmdError::internal("disk full"));
        }
        self.records
            .lock()
            .unwrap()
            .extend(record.steps.iter().map(|s| s.record.clone()));
        let mut issued = self.issued.lock().unwrap();
        *issued = (*issued).max(record.issued);
        // Round-tripped through JSON like the real journal's.
        let marker = serde_json::from_value(serde_json::to_value(record).unwrap()).unwrap();
        self.markers.lock().unwrap().push(marker);
        Ok(())
    }
}

impl Remembering {
    /// The first transaction id a restarted engine may issue: past everything reserved.
    fn next_tx(&self) -> TxId {
        TxId::new(*self.issued.lock().unwrap() + 1)
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

/// One engine run of the issue #251 tests: an actor whose transaction ids are per run.
fn run_of(
    doc: cutweave_model::Project,
    rev: Rev,
    clock: &Arc<ManualClock>,
    journal: &Remembering,
) -> DocActor {
    DocActor::spawn(
        config(doc)
            .with_rev(rev)
            .with_clock(clock.clone())
            .with_journal(Box::new(journal.clone()))
            .with_tx_ids_per_run(),
    )
    .unwrap()
}

/// Opens a gesture under request id `id` and appends one clip to `track` in it.
fn open_drag(
    h: &cutweave_commands::DocHandle,
    ctx: &ActorCtx,
    id: u128,
    track: cutweave_model::TrackId,
) -> TxId {
    let begin = json!({"kind": "gesture", "label": "Drag"});
    let opened = h
        .submit(request(id, ctx, "edit.begin_tx", begin))
        .wait()
        .unwrap();
    let tx = opened_tx(&opened);
    let step = append_with(RequestId::from_u128(id + 1), ctx, track);
    h.submit(step.in_tx(tx)).wait().unwrap();
    tx
}

/// Issue #251: the engine died with transaction A open (never journaled), so the restarted
/// engine knows nothing of A's id. It must not hand that id to a later transaction: a commit
/// or cancel of A sent again after the client opened B would then close B. With per-run ids the
/// resend finds no such transaction and B is untouched.
#[test]
fn a_close_naming_a_transaction_from_before_a_restart_never_reaches_a_later_one() {
    for close in ["edit.commit_tx", "edit.cancel_tx"] {
        let (doc, tracks, _) = project(2);
        let clock = clock();
        let journal = Remembering::default();
        let ctx = ui();
        let (saved, a) = {
            let actor = run_of(doc, Rev::ZERO, &clock, &journal);
            let h = actor.handle();
            let a = open_drag(&h, &ctx, 81, tracks[0]);
            (h.committed_snapshot(), a)
            // The engine dies here, with A open.
        };
        assert_eq!(journal.records.lock().unwrap().len(), 0);

        clock.advance(Duration::from_secs(1));
        let restarted = run_of(saved.doc.clone(), saved.rev, &clock, &journal);
        let h = restarted.handle();
        let b = open_drag(&h, &ctx, 84, tracks[1]);
        assert!(b > a, "{b} was issued after a restart that followed {a}");
        let live = h.snapshot();
        assert_eq!(live.doc.item_count(), 1, "B's clip shows live");

        // A's close, sent again under its request id and naming A.
        let err = h
            .submit(request(83, &ctx, close, json!({"tx": a})))
            .wait()
            .unwrap_err();
        assert!(
            matches!(&err, CmdError::NotFound { id, .. } if *id == a.to_string()),
            "{close}: {err:?}"
        );
        // B is as it was: open, with its one step, still live and uncommitted.
        let steps = h
            .run(move |state| state.transaction(b).map(|t| t.steps))
            .wait()
            .unwrap();
        assert_eq!(steps, Some(1), "{close}");
        assert_eq!(h.snapshot().doc, live.doc);
        assert_eq!(h.snapshot().rev, live.rev);
        assert_eq!(h.committed_snapshot().doc, saved.doc);
        // And its owner still closes it.
        let closed = h
            .submit(request(86, &ctx, "edit.commit_tx", json!({"tx": b})))
            .wait()
            .unwrap();
        assert_eq!(closed.output["changed"], true);
        assert_eq!(h.committed_snapshot().doc.item_count(), 1);
    }
}

/// Per-run ids never go below what the journal holds: when the clock stepped back across the
/// restart, numbering continues past the last journaled transaction. A frozen clock
/// (`--deterministic-ids`) keeps the plain numbering.
#[test]
fn per_run_transaction_ids_continue_past_the_journaled_ones() {
    let (doc, tracks, _) = project(1);
    let clock = clock();
    let journal = Remembering::default();
    let ctx = ui();
    let actor = run_of(doc, Rev::ZERO, &clock, &journal);
    let h = actor.handle();
    let first = open_drag(&h, &ctx, 1, tracks[0]);
    h.submit(request(3, &ctx, "edit.commit_tx", json!({"tx": first})))
        .wait()
        .unwrap();
    let saved = h.committed_snapshot();
    drop(actor);

    let earlier = Arc::new(ManualClock::new(WallTime::from_millis(1_000)));
    let restarted = DocActor::spawn(
        config(saved.doc.clone())
            .with_rev(saved.rev)
            .with_ids(Box::new(SeededGen::new(43)))
            .with_clock(earlier)
            .with_first_tx_id(TxId::new(first.get() + 1))
            .with_tx_ids_per_run(),
    )
    .unwrap();
    let next = open_drag(&restarted.handle(), &ctx, 4, tracks[0]);
    assert_eq!(next.get(), first.get() + 1);

    let (doc, tracks, _) = project(1);
    let frozen = Arc::new(ManualClock::new(WallTime::UNIX_EPOCH));
    let actor = run_of(doc, Rev::ZERO, &frozen, &Remembering::default());
    assert_eq!(open_drag(&actor.handle(), &ctx, 1, tracks[0]), TxId::new(1));
}

/// ADR-0043 (B1): `tx` stays optional. A commit or cancel that names no
/// transaction means "this connection's open one", so one sent again after the client opened
/// another transaction closes that one. Nothing in the request tells the engine it is a
/// repeat; a client that sends a close again must name the transaction, as the editor, the
/// CLI and the MCP server do. Pinned so it only changes on purpose.
#[test]
fn a_close_that_names_no_transaction_means_the_connections_open_one() {
    let (doc, tracks, _) = project(2);
    let clock = clock();
    let journal = Remembering::default();
    let ctx = ui();
    let saved = {
        let actor = run_of(doc, Rev::ZERO, &clock, &journal);
        let h = actor.handle();
        open_drag(&h, &ctx, 81, tracks[0]);
        h.committed_snapshot()
    };
    clock.advance(Duration::from_secs(1));
    let restarted = run_of(saved.doc.clone(), saved.rev, &clock, &journal);
    let h = restarted.handle();
    let b = open_drag(&h, &ctx, 84, tracks[1]);
    let closed = h
        .submit(request(83, &ctx, "edit.commit_tx", json!({})))
        .wait()
        .unwrap();
    assert_eq!(closed.output["tx"], json!(b));
    assert_eq!(h.committed_snapshot().doc.item_count(), 1);
}

/// The engine after a restart, as recovery builds it from what `journal` holds: the remembered
/// requests, and transaction ids past the last reservation. The clock is frozen (plain
/// numbering), so nothing but the journal keeps ids from repeating.
fn restarted_from(journal: &Remembering, doc: cutweave_model::Project, rev: Rev) -> DocActor {
    DocActor::spawn(
        config(doc)
            .with_rev(rev)
            .with_ids(Box::new(SeededGen::new(43)))
            .with_journal(Box::new(journal.clone()))
            .with_restored_requests(journal.persisted())
            .with_first_tx_id(journal.next_tx()),
    )
    .unwrap()
}

/// ADR-0043: a cancel is journaled as a marker with the request that opened the transaction,
/// so after a restart its repeat gets the original result back instead of a refusal. The
/// cancelled transaction's commands stay forgotten, as on a running engine, and a close that
/// never ran is still refused.
#[test]
fn a_cancel_sent_again_after_a_restart_gets_its_result_back() {
    for named in [false, true] {
        let (doc, tracks, _) = project(1);
        let journal = Remembering::default();
        let ctx = ui();
        let cancel = |tx: TxId| {
            let params = if named { json!({"tx": tx}) } else { json!({}) };
            request(73, &ctx, "edit.cancel_tx", params)
        };
        let begin = || {
            let params = json!({"kind": "gesture", "label": "Drag"});
            request(71, &ctx, "edit.begin_tx", params)
        };
        let (saved, tx, opened, cancelled) = {
            let actor =
                DocActor::spawn(config(doc).with_journal(Box::new(journal.clone()))).unwrap();
            let h = actor.handle();
            let opened = h.submit(begin()).wait().unwrap();
            let tx = opened_tx(&opened);
            let step = append_with(RequestId::from_u128(72), &ctx, tracks[0]);
            h.submit(step.in_tx(tx)).wait().unwrap();
            let cancelled = h.submit(cancel(tx)).wait().unwrap();
            assert_eq!(cancelled.output["tx"], json!(tx));
            (h.committed_snapshot(), tx, opened, cancelled)
        };
        let markers = journal.markers.lock().unwrap().clone();
        let closed = markers.last().unwrap().closed.unwrap();
        assert_eq!(
            (closed.tx_id, closed.kind, closed.outcome),
            (tx, TxKind::Gesture, TxOutcome::Cancelled)
        );
        let ids: Vec<RequestId> = journal
            .persisted()
            .iter()
            .map(DedupRecord::request_id)
            .collect();
        assert_eq!(ids, [71, 73].map(RequestId::from_u128), "begin and cancel");

        let restarted = restarted_from(&journal, saved.doc.clone(), saved.rev);
        let h = restarted.handle();
        for _ in 0..2 {
            let again = h.submit(cancel(tx)).wait().unwrap();
            assert!(again.replayed, "named: {named}");
            assert_eq!(again.output, cancelled.output);
            assert_eq!(again.rev, cancelled.rev);
            assert_eq!(again.ops, cancelled.ops);
        }
        let again = h.submit(begin()).wait().unwrap();
        assert!(again.replayed);
        assert_eq!(again.output, opened.output);
        assert_eq!(h.locks(&ctx).wait().unwrap().transactions.len(), 0);
        assert_eq!(h.snapshot().doc, saved.doc);
        assert_eq!(h.snapshot().rev, saved.rev);

        // A close that never ran is not answered from memory.
        let err = h
            .submit(request(74, &ctx, "edit.commit_tx", json!({})))
            .wait()
            .unwrap_err();
        assert_eq!(
            err,
            CmdError::invalid("/tx", "this connection has no open transaction")
        );
        let err = h
            .submit(request(75, &ctx, "edit.commit_tx", json!({"tx": tx})))
            .wait()
            .unwrap_err();
        assert!(matches!(err, CmdError::NotFound { .. }), "{err:?}");
        // The cancelled transaction's command runs as a new request, as it does live.
        let rerun = h
            .submit(append_with(RequestId::from_u128(72), &ctx, tracks[0]))
            .wait()
            .unwrap();
        assert!(!rerun.replayed);
    }
}

/// ADR-0043: a commit of a transaction that changed nothing is journaled as a marker (it has
/// no revision to carry), with its commands and the request that opened it. After a restart
/// its repeat gets the original result back.
#[test]
fn an_empty_commit_sent_again_after_a_restart_gets_its_result_back() {
    for kind in ["gesture", "batch", "proposal"] {
        let (doc, _, _) = project(1);
        let journal = Remembering::default();
        let ctx = ui();
        let commit = |tx: TxId| request(92, &ctx, "edit.commit_tx", json!({"tx": tx}));
        let (saved, tx, committed) = {
            let actor =
                DocActor::spawn(config(doc).with_journal(Box::new(journal.clone()))).unwrap();
            let h = actor.handle();
            let begin = json!({"kind": kind, "label": "Drag"});
            let opened = h
                .submit(request(91, &ctx, "edit.begin_tx", begin))
                .wait()
                .unwrap();
            let tx = opened_tx(&opened);
            let committed = h.submit(commit(tx)).wait().unwrap();
            assert_eq!(committed.output["changed"], false);
            assert_eq!(committed.rev, Rev::ZERO);
            (h.committed_snapshot(), tx, committed)
        };
        let markers = journal.markers.lock().unwrap().clone();
        let closed = markers.last().unwrap().closed.unwrap();
        assert_eq!((closed.tx_id, closed.outcome), (tx, TxOutcome::Committed));
        assert_eq!(journal.persisted().len(), 2, "begin and commit");

        let restarted = restarted_from(&journal, saved.doc.clone(), saved.rev);
        let h = restarted.handle();
        for _ in 0..2 {
            let again = h.submit(commit(tx)).wait().unwrap();
            assert!(again.replayed, "{kind}");
            assert_eq!(again.output, committed.output);
            assert_eq!(again.rev, committed.rev);
            assert_eq!(again.committed_rev, committed.committed_rev);
        }
        // Under another request id it is a new request, and finds nothing to commit.
        let err = h
            .submit(request(93, &ctx, "edit.commit_tx", json!({"tx": tx})))
            .wait()
            .unwrap_err();
        assert!(matches!(err, CmdError::NotFound { .. }), "{err:?}");
        assert_eq!(h.snapshot().doc, saved.doc);
        assert_eq!(h.snapshot().rev, saved.rev);
    }
}

/// A close the journal could not take still stands (it lost nothing), and stays what it was
/// before ADR-0043 after a restart: refused, never reported as done.
#[test]
fn a_close_the_journal_never_had_is_refused_after_a_restart() {
    for close in ["edit.cancel_tx", "edit.commit_tx"] {
        let (doc, _, _) = project(1);
        let journal = Remembering::default();
        let ctx = ui();
        let (saved, tx) = {
            let actor =
                DocActor::spawn(config(doc).with_journal(Box::new(journal.clone()))).unwrap();
            let h = actor.handle();
            let begin = json!({"kind": "gesture", "label": "Drag"});
            let opened = h
                .submit(request(71, &ctx, "edit.begin_tx", begin))
                .wait()
                .unwrap();
            let tx = opened_tx(&opened);
            *journal.refuse.lock().unwrap() = true;
            let closed = h
                .submit(request(73, &ctx, close, json!({"tx": tx})))
                .wait()
                .unwrap();
            assert!(!closed.replayed);
            // Live, the repeat is still answered.
            let again = h
                .submit(request(73, &ctx, close, json!({"tx": tx})))
                .wait()
                .unwrap();
            assert!(again.replayed);
            *journal.refuse.lock().unwrap() = false;
            (h.committed_snapshot(), tx)
        };
        assert_eq!(journal.persisted().len(), 0);

        let restarted = restarted_from(&journal, saved.doc.clone(), saved.rev);
        let h = restarted.handle();
        let err = h
            .submit(request(73, &ctx, close, json!({"tx": tx})))
            .wait()
            .unwrap_err();
        assert!(matches!(err, CmdError::NotFound { .. }), "{close}: {err:?}");
        let err = h
            .submit(request(76, &ctx, close, json!({})))
            .wait()
            .unwrap_err();
        assert_eq!(
            err,
            CmdError::invalid("/tx", "this connection has no open transaction")
        );
        assert_eq!(h.snapshot().doc, saved.doc);
    }
}

/// ADR-0043: the journal is told which transaction ids may be handed out before the first of
/// them is, a block at a time. The engine dies with A open (nothing else of A is journaled);
/// the restarted one numbers past the reservation, on a frozen clock too, so A's close sent
/// again never reaches B.
#[test]
fn transaction_ids_are_reserved_in_the_journal_before_they_are_handed_out() {
    let (doc, tracks, _) = project(2);
    let journal = Remembering::default();
    let ctx = ui();
    let (saved, a) = {
        let actor = DocActor::spawn(config(doc).with_journal(Box::new(journal.clone()))).unwrap();
        let h = actor.handle();
        let a = open_drag(&h, &ctx, 81, tracks[0]);
        (h.committed_snapshot(), a)
        // The engine dies here, with A open.
    };
    assert_eq!(a, TxId::new(1));
    assert_eq!(*journal.issued.lock().unwrap(), TX_IDS_RESERVED);
    let markers = journal.markers.lock().unwrap().clone();
    assert_eq!(markers.len(), 1, "one reservation, nothing else");
    assert_eq!(markers[0].closed, None);
    assert_eq!(markers[0].steps.len(), 0);

    let restarted = restarted_from(&journal, saved.doc.clone(), saved.rev);
    let h = restarted.handle();
    let b = open_drag(&h, &ctx, 84, tracks[1]);
    assert_eq!(b, TxId::new(TX_IDS_RESERVED + 1));
    for (id, close) in [(83, "edit.commit_tx"), (87, "edit.cancel_tx")] {
        let err = h
            .submit(request(id, &ctx, close, json!({"tx": a})))
            .wait()
            .unwrap_err();
        assert!(matches!(err, CmdError::NotFound { .. }), "{close}: {err:?}");
    }
    let steps = h
        .run(move |state| state.transaction(b).map(|t| t.steps))
        .wait()
        .unwrap();
    assert_eq!(steps, Some(1));
}

/// One reservation covers a block: the transactions inside it write nothing more, and the
/// first one past it reserves the next block.
#[test]
fn a_reservation_covers_a_block_of_transaction_ids() {
    let (doc, _, _) = project(1);
    let journal = Remembering::default();
    let ctx = ui();
    let actor = DocActor::spawn(config(doc).with_journal(Box::new(journal.clone()))).unwrap();
    let h = actor.handle();
    let reservations = || {
        let markers = journal.markers.lock().unwrap();
        markers.iter().filter(|m| m.closed.is_none()).count()
    };
    for n in 1..=TX_IDS_RESERVED + 1 {
        let tx = h.begin_tx(&ctx, TxKind::Batch, "b").wait().unwrap().id;
        assert_eq!(tx, TxId::new(n));
        h.cancel_tx(&ctx, tx).wait().unwrap();
        assert_eq!(reservations(), if n <= TX_IDS_RESERVED { 1 } else { 2 });
    }
    assert_eq!(*journal.issued.lock().unwrap(), 2 * TX_IDS_RESERVED);
}

/// A journal that cannot take the reservation refuses the transaction: its id would be free
/// again after a restart.
#[test]
fn a_transaction_is_not_opened_when_its_id_cannot_be_reserved() {
    let (doc, _, _) = project(1);
    let journal = Remembering::default();
    let ctx = ui();
    let actor = DocActor::spawn(config(doc).with_journal(Box::new(journal.clone()))).unwrap();
    let h = actor.handle();
    *journal.refuse.lock().unwrap() = true;
    let begin = || request(1, &ctx, "edit.begin_tx", json!({"label": "Batch"}));
    let err = h.submit(begin()).wait().unwrap_err();
    assert!(matches!(err, CmdError::Internal { .. }), "{err:?}");
    assert_eq!(h.locks(&ctx).wait().unwrap().transactions.len(), 0);
    *journal.refuse.lock().unwrap() = false;
    let opened = h.submit(begin()).wait().unwrap();
    assert_eq!(opened_tx(&opened), TxId::new(1));
}
