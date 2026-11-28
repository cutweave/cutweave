// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.03: the transaction seam (arch §5.2, ADR-0017). Gestures and proposals update the
//! live document while `committed_snapshot()` keeps the last committed one; batches stay
//! invisible until they commit; transactions belong to their connection; commands of other
//! connections proceed on untouched items and fail fast on touched ones. (WP-1B.09's soft
//! locks have their own suite: `doc_locks.rs`.)

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod doc_support;

use std::sync::{Arc, Mutex};

use cutweave_commands::{
    CmdError, CommitRecord, ConflictReason, DocActor, DocEvent, DocHandle, GESTURE_IDLE_TIMEOUT,
    PatchCause, ResourceKind, Rev, Submit, TxKind, TxOutcome, diff,
};
use doc_support::{Mirror, agent, append, config, drain, item_name, project, spawn, submit, ui};
use serde_json::json;

fn rename_in(
    h: &DocHandle,
    ctx: &cutweave_commands::ActorCtx,
    tx: cutweave_commands::TxId,
    item: cutweave_model::ItemId,
    name: &str,
) -> cutweave_commands::Applied {
    h.submit(
        Submit::new(
            ctx.clone(),
            "item.rename",
            json!({"item": item, "name": name}),
        )
        .in_tx(tx),
    )
    .wait()
    .unwrap()
}

fn verify(h: &DocHandle) {
    h.run(|state| state.verify()).wait().unwrap().unwrap();
}

#[test]
fn a_gesture_is_live_while_the_committed_snapshot_keeps_the_pre_tx_document() {
    let (doc, tracks, _) = project(1);
    let journal: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let sink = {
        let journal = Arc::clone(&journal);
        move |r: &CommitRecord<'_>| {
            journal
                .lock()
                .unwrap()
                .push(serde_json::to_value(r).unwrap());
            Ok(())
        }
    };
    let actor = DocActor::spawn(config(doc).with_journal(Box::new(sink))).unwrap();
    let h = actor.handle();
    let item = append(&h, &ui(), tracks[0], 30);
    let pre = h.committed_snapshot();
    assert_eq!(pre.rev, Rev(1));
    let mut events = h.subscribe();
    let mut mirror = Mirror::new(&pre.doc, pre.rev);

    let ctx = ui();
    let tx = h.begin_tx(&ctx, TxKind::Gesture, "Drag").wait().unwrap().id;
    for (i, name) in ["a", "ab", "abc"].into_iter().enumerate() {
        let step = rename_in(&h, &ctx, tx, item, name);
        let n = i as u64 + 1;
        assert_eq!((step.rev, step.committed_rev), (Rev(1 + n), Rev(1)));
        let live = h.snapshot();
        let committed = h.committed_snapshot();
        assert_eq!((live.rev, live.committed_rev), (Rev(1 + n), Rev(1)));
        assert!(!live.is_committed());
        assert_eq!(item_name(&live.doc, item).as_deref(), Some(name));
        // The committed snapshot is still exactly the pre-tx document.
        assert_eq!(committed.rev, Rev(1));
        assert_eq!(committed.doc, pre.doc);
    }
    let (doc_events, tx_events): (Vec<_>, Vec<_>) = drain(&mut events)
        .into_iter()
        .partition(|e| e.rev().is_some());
    for event in doc_events {
        let p = event.as_patched().unwrap();
        assert_eq!(
            (p.cause, p.committed_rev, p.tx_id),
            (PatchCause::Live, Rev(1), tx)
        );
        mirror.apply(&event);
    }
    // `tx.opened`, then one `tx.locked` when the first rename touched the item.
    match &tx_events[..] {
        [opened, locked] => {
            let (DocEvent::TxOpened(o), DocEvent::TxLocked(l)) = (&**opened, &**locked) else {
                panic!("{tx_events:?}")
            };
            assert_eq!((o.tx_id, o.kind, o.items.len()), (tx, TxKind::Gesture, 0));
            assert_eq!((l.tx_id, &l.items[..]), (tx, &[item][..]));
        }
        other => panic!("{other:?}"),
    }
    mirror.assert_matches(&h.snapshot().doc);
    assert!(
        journal.lock().unwrap().len() == 1,
        "nothing journaled before commit"
    );
    verify(&h);

    let closed = h.commit_tx(&ctx, tx).wait().unwrap();
    assert_eq!(closed.outcome, TxOutcome::Committed);
    assert_eq!((closed.rev, closed.committed_rev), (Rev(5), Rev(5)));
    assert_eq!(closed.patch, diff(&pre.doc, &h.snapshot().doc).unwrap());
    assert_eq!(closed.patch.len(), 1, "200 renames coalesce into one op");
    let live = h.snapshot();
    assert!(live.is_committed());
    assert_eq!(h.committed_snapshot().doc, live.doc);
    let event = events.try_recv().unwrap();
    let p = event.as_patched().unwrap();
    assert_eq!(
        (p.cause, p.rev, p.committed_rev),
        (PatchCause::Commit, Rev(5), Rev(5))
    );
    assert!(p.ops.is_empty(), "the live document already showed it");
    mirror.apply(&event);
    mirror.assert_matches(&live.doc);

    let journal = journal.lock().unwrap();
    let record = &journal[1];
    assert_eq!(record["kind"], json!("gesture"));
    assert_eq!(record["label"], json!("Drag"));
    assert_eq!(record["rev"], json!(5));
    assert_eq!(record["prev_committed_rev"], json!(1));
    assert_eq!(record["steps"].as_array().unwrap().len(), 3);
    assert_eq!(
        record["patch"],
        serde_json::to_value(&closed.patch).unwrap()
    );
}

#[test]
fn cancelling_a_gesture_restores_the_committed_document() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let item = append(&h, &ui(), tracks[0], 30);
    let pre = h.snapshot();
    let mut events = h.subscribe();
    let mut mirror = Mirror::new(&pre.doc, pre.rev);
    let ctx = ui();
    let tx = h.begin_tx(&ctx, TxKind::Gesture, "Drag").wait().unwrap().id;
    rename_in(&h, &ctx, tx, item, "x");
    h.submit(
        Submit::new(
            ctx.clone(),
            "timeline.append_clip",
            json!({"track": tracks[0], "frames": 3}),
        )
        .in_tx(tx),
    )
    .wait()
    .unwrap();
    let closed = h.cancel_tx(&ctx, tx).wait().unwrap();
    assert_eq!(closed.outcome, TxOutcome::Cancelled);
    assert!(closed.patch.is_empty());
    let live = h.snapshot();
    assert_eq!(live.doc, pre.doc);
    assert_eq!(live.rev, Rev(4));
    assert_eq!(h.committed_snapshot().rev, Rev(1));
    let events = drain(&mut events);
    let patched: Vec<_> = events.iter().filter_map(|e| e.as_patched()).collect();
    assert_eq!(patched.last().unwrap().cause, PatchCause::Rollback);
    // Then `tx.closed`, releasing the renamed and the appended item.
    let ended = events.last().unwrap().as_tx_closed().unwrap();
    assert_eq!((ended.tx_id, ended.outcome), (tx, TxOutcome::Cancelled));
    assert_eq!(ended.items.len(), 2);
    assert!(ended.items.contains(&item));
    for e in &events {
        mirror.apply(e);
    }
    mirror.assert_matches(&pre.doc);
    verify(&h);
}

#[test]
fn a_batch_is_invisible_until_it_commits() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let mut events = h.subscribe();
    let ctx = agent();
    let tx = h
        .begin_tx(&ctx, TxKind::Batch, "Rough cut")
        .wait()
        .unwrap()
        .id;
    for _ in 0..3 {
        let step = h
            .submit(
                Submit::new(
                    ctx.clone(),
                    "timeline.append_clip",
                    json!({"track": tracks[0], "frames": 10}),
                )
                .in_tx(tx),
            )
            .wait()
            .unwrap();
        assert_eq!(step.rev, Rev(0));
        assert!(!step.ops.is_empty());
    }
    // Later steps see the earlier ones.
    assert_eq!(h.snapshot().doc.item_count(), 0);
    assert_eq!(h.snapshot().rev, Rev(0));
    // Only `tx.opened`: a batch shows nothing and soft-locks nothing.
    let seen = drain(&mut events);
    assert_eq!(seen.len(), 1, "{seen:?}");
    let DocEvent::TxOpened(opened) = &*seen[0] else {
        panic!("{seen:?}")
    };
    assert_eq!((opened.kind, opened.items.len()), (TxKind::Batch, 0));

    let closed = h.commit_tx(&ctx, tx).wait().unwrap();
    assert_eq!(closed.rev, Rev(1));
    assert_eq!(h.snapshot().doc.item_count(), 3);
    // One `doc.patched` (plus the history's `edit.history_changed`).
    let events: Vec<_> = drain(&mut events)
        .into_iter()
        .filter(|e| e.as_patched().is_some())
        .collect();
    assert_eq!(events.len(), 1);
    let p = events[0].as_patched().unwrap();
    assert_eq!((p.cause, p.rev), (PatchCause::Commit, Rev(1)));
    assert_eq!(p.ops, closed.patch);
    assert_eq!(p.actor.to_string(), "agent:claude");
}

#[test]
fn only_the_owning_connection_may_use_its_transaction() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let item = append(&h, &ui(), tracks[0], 30);
    let tx = h
        .begin_tx(&ui(), TxKind::Gesture, "Drag")
        .wait()
        .unwrap()
        .id;
    let locked = |e: CmdError| match e {
        CmdError::Locked {
            kind: ResourceKind::Transaction,
            by,
            tx: Some(t),
            ..
        } => {
            assert_eq!(by.as_deref(), Some("user"));
            assert_eq!(t, tx.to_string());
        }
        other => panic!("{other:?}"),
    };
    locked(
        h.submit(Submit::new(agent(), "item.rename", json!({"item": item, "name": "x"})).in_tx(tx))
            .wait()
            .unwrap_err(),
    );
    locked(h.commit_tx(&agent(), tx).wait().unwrap_err());
    locked(h.cancel_tx(&agent(), tx).wait().unwrap_err());
    // Unknown transactions.
    let missing = cutweave_commands::TxId::new(999);
    let err = h.commit_tx(&ui(), missing).wait().unwrap_err();
    assert_eq!(err, CmdError::not_found(ResourceKind::Transaction, missing));
    // A command naming a transaction cannot be an engine command.
    let err = h
        .submit(
            Submit::new(
                ui(),
                "edit.begin_tx",
                json!({"kind": "batch", "label": "x"}),
            )
            .in_tx(tx),
        )
        .wait()
        .unwrap_err();
    assert_eq!(err.code(), "unsupported");
    // The owner still can.
    h.commit_tx(&ui(), tx).wait().unwrap();
}

#[test]
fn other_connections_edit_untouched_items_and_fail_fast_on_touched_ones() {
    let (doc, tracks, _) = project(2);
    let actor = spawn(doc);
    let h = actor.handle();
    let a = append(&h, &ui(), tracks[0], 30);
    let b = append(&h, &ui(), tracks[1], 30);
    let mut events = h.subscribe();
    let mut mirror = Mirror::new(&h.snapshot().doc, h.snapshot().rev);
    let ctx = ui();
    let tx = h
        .begin_tx(&ctx, TxKind::Gesture, "Drag A")
        .wait()
        .unwrap()
        .id;
    rename_in(&h, &ctx, tx, a, "dragging");
    assert_eq!(h.snapshot().rev, Rev(3));

    // The agent renames B meanwhile: committed alone, live has both.
    let applied = submit(
        &h,
        &agent(),
        "item.rename",
        json!({"item": b, "name": "agent"}),
    )
    .unwrap();
    assert_eq!((applied.committed_rev, applied.rev), (Rev(4), Rev(5)));
    let live = h.snapshot();
    let committed = h.committed_snapshot();
    assert_eq!((live.rev, live.committed_rev), (Rev(5), Rev(4)));
    assert_eq!(item_name(&live.doc, a).as_deref(), Some("dragging"));
    assert_eq!(item_name(&live.doc, b).as_deref(), Some("agent"));
    assert_eq!(item_name(&committed.doc, a), None);
    assert_eq!(item_name(&committed.doc, b).as_deref(), Some("agent"));
    verify(&h);

    // The agent touches A: fails fast, nothing changes.
    let err = submit(
        &h,
        &agent(),
        "item.rename",
        json!({"item": a, "name": "agent"}),
    )
    .unwrap_err();
    let CmdError::Conflict {
        reason,
        retry_after_ms,
        current_rev,
        holder,
        detail,
        ..
    } = &err
    else {
        panic!("{err:?}")
    };
    assert_eq!(*reason, ConflictReason::ItemInGesture);
    // WP-2A.12: the holder is named.
    assert_eq!(
        holder.as_ref().map(ToString::to_string).as_deref(),
        Some("user")
    );
    // The time left until the gesture's idle deadline (the test clock stands still).
    assert_eq!(
        *retry_after_ms,
        Some(u64::try_from(GESTURE_IDLE_TIMEOUT.as_millis()).unwrap())
    );
    assert_eq!(*current_rev, Some(Rev(5)));
    assert!(detail.contains(&tx.to_string()), "{detail}");
    assert_eq!(h.snapshot().rev, Rev(5));
    // Deleting it, too.
    let err = submit(&h, &agent(), "item.delete", json!({"item": a})).unwrap_err();
    assert_eq!(err.code(), "conflict");

    h.commit_tx(&ctx, tx).wait().unwrap();
    let live = h.snapshot();
    assert!(live.is_committed());
    assert_eq!(h.committed_snapshot().doc, live.doc);
    assert_eq!(item_name(&live.doc, a).as_deref(), Some("dragging"));
    assert_eq!(item_name(&live.doc, b).as_deref(), Some("agent"));
    for e in drain(&mut events) {
        mirror.apply(&e);
    }
    mirror.assert_matches(&live.doc);
    // Now the agent may touch A.
    submit(
        &h,
        &agent(),
        "item.rename",
        json!({"item": a, "name": "agent"}),
    )
    .unwrap();
    verify(&h);
}

#[test]
fn closing_a_connection_rolls_back_its_transactions() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let item = append(&h, &ui(), tracks[0], 30);
    let pre = h.snapshot();
    let ctx = ui();
    let tx = h.begin_tx(&ctx, TxKind::Gesture, "Drag").wait().unwrap().id;
    rename_in(&h, &ctx, tx, item, "x");
    let closed = h.close_connection(ctx.conn()).wait().unwrap();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].outcome, TxOutcome::RolledBack);
    assert_eq!(h.snapshot().doc, pre.doc);
    assert_eq!(h.run(|s| s.transactions()).wait().unwrap().len(), 0);
    // Other connections are unaffected.
    let none = h.close_connection(agent().conn()).wait().unwrap();
    assert_eq!(none.len(), 0);
}

#[test]
fn live_transactions_of_two_connections_compose_in_order() {
    let (doc, tracks, _) = project(2);
    let actor = spawn(doc);
    let h = actor.handle();
    let a = append(&h, &ui(), tracks[0], 30);
    let b = append(&h, &ui(), tracks[1], 30);
    let pre = h.snapshot();
    let mut events = h.subscribe();
    let mut mirror = Mirror::new(&pre.doc, pre.rev);
    let (user, bot) = (ui(), agent());
    let gesture = h
        .begin_tx(&user, TxKind::Gesture, "Drag")
        .wait()
        .unwrap()
        .id;
    let proposal = h
        .begin_tx(&bot, TxKind::Proposal, "Suggest")
        .wait()
        .unwrap()
        .id;
    rename_in(&h, &user, gesture, a, "user");
    rename_in(&h, &bot, proposal, b, "agent");
    rename_in(&h, &user, gesture, a, "user2");
    let live = h.snapshot();
    assert_eq!(item_name(&live.doc, a).as_deref(), Some("user2"));
    assert_eq!(item_name(&live.doc, b).as_deref(), Some("agent"));
    verify(&h);
    // Both edit the same item: the second one is refused.
    let err = h
        .submit(
            Submit::new(bot.clone(), "item.rename", json!({"item": a, "name": "no"}))
                .in_tx(proposal),
        )
        .wait()
        .unwrap_err();
    assert!(matches!(
        err,
        CmdError::Conflict {
            reason: ConflictReason::ItemInGesture,
            ..
        }
    ));

    // The user cancels: the agent's proposal stays live.
    h.cancel_tx(&user, gesture).wait().unwrap();
    let live = h.snapshot();
    assert_eq!(item_name(&live.doc, a), None);
    assert_eq!(item_name(&live.doc, b).as_deref(), Some("agent"));
    verify(&h);
    // The agent's proposal is accepted.
    let closed = h.commit_tx(&bot, proposal).wait().unwrap();
    assert_eq!(closed.kind, TxKind::Proposal);
    let live = h.snapshot();
    assert!(live.is_committed());
    assert_eq!(h.committed_snapshot().doc, live.doc);
    for e in drain(&mut events) {
        if let DocEvent::Patched(_) = &*e {
            mirror.apply(&e);
        }
    }
    mirror.assert_matches(&live.doc);
    verify(&h);
}

#[test]
fn between_two_batches_the_first_committer_wins() {
    let (doc, _, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let (user, bot) = (ui(), agent());
    let t1 = h.begin_tx(&user, TxKind::Batch, "one").wait().unwrap().id;
    let t2 = h.begin_tx(&bot, TxKind::Batch, "two").wait().unwrap().id;
    let rename = |ctx: &cutweave_commands::ActorCtx, tx, name: &str| {
        h.submit(Submit::new(ctx.clone(), "project.rename", json!({"name": name})).in_tx(tx))
            .wait()
            .unwrap();
    };
    rename(&user, t1, "First");
    rename(&bot, t2, "Second");
    h.commit_tx(&user, t1).wait().unwrap();
    let info = h.run(move |s| s.transaction(t2)).wait().unwrap().unwrap();
    assert!(info.conflicted);
    // Further steps and the commit of the loser fail; it is closed.
    let err = h
        .submit(Submit::new(bot.clone(), "project.rename", json!({"name": "x"})).in_tx(t2))
        .wait()
        .unwrap_err();
    assert_eq!(err.code(), "conflict");
    let err = h.commit_tx(&bot, t2).wait().unwrap_err();
    assert_eq!(err.code(), "conflict");
    assert_eq!(h.run(|s| s.transactions()).wait().unwrap().len(), 0);
    assert_eq!(h.snapshot().doc.meta.name, "First");
    verify(&h);
}

#[test]
fn an_open_batch_soft_locks_what_it_touched() {
    let (doc, _, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let user = ui();
    let tx = h
        .begin_tx(&user, TxKind::Batch, "rename")
        .wait()
        .unwrap()
        .id;
    h.submit(Submit::new(user.clone(), "project.rename", json!({"name": "Mine"})).in_tx(tx))
        .wait()
        .unwrap();
    let err = submit(&h, &agent(), "project.rename", json!({"name": "Theirs"})).unwrap_err();
    assert!(matches!(
        err,
        CmdError::Conflict {
            reason: ConflictReason::ItemInGesture,
            ..
        }
    ));
    h.commit_tx(&user, tx).wait().unwrap();
    assert_eq!(h.snapshot().doc.meta.name, "Mine");
}

#[test]
fn empty_transactions_close_without_a_revision() {
    let (doc, _, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let mut events = h.subscribe();
    for kind in [TxKind::Batch, TxKind::Gesture, TxKind::Proposal] {
        let tx = h.begin_tx(&ui(), kind, "nothing").wait().unwrap().id;
        let closed = h.commit_tx(&ui(), tx).wait().unwrap();
        assert_eq!((closed.rev, closed.committed_rev), (Rev(0), Rev(0)));
        let tx = h.begin_tx(&ui(), kind, "nothing").wait().unwrap().id;
        h.cancel_tx(&ui(), tx).wait().unwrap();
    }
    assert_eq!(h.snapshot().rev, Rev(0));
    // No document event; one `tx.opened` and one `tx.closed` each.
    let seen = drain(&mut events);
    assert!(seen.iter().all(|e| e.rev().is_none()), "{seen:?}");
    assert_eq!(seen.len(), 12);
}
