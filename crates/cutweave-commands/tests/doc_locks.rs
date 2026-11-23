// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.09: transaction ownership, connection-drop rollback and the minimal soft lock
//! (arch §5.2, ADR-0017). Scenarios: a UI gesture and an agent on the same and on different
//! items, a connection dropped mid-gesture, copilot proposals accepted and rejected while they
//! hold locks, idle gestures releasing theirs, saves and the journal never seeing an open
//! transaction, `edit.locks` and the `tx.*` events.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod doc_support;

use std::sync::{Arc, Mutex};

use cutweave_commands::{
    ActorCtx, Applied, CmdError, CommitRecord, ConflictReason, ConnId, DocActor, DocEvent,
    DocHandle, DocState, GESTURE_IDLE_TIMEOUT, JournaledCommit, LockList, ManualClock, PatchCause,
    ResourceKind, Rev, SOFT_LOCK_RETRY_MS, Submit, TxId, TxKind, TxOutcome, WallTime,
};
use cutweave_model::{ItemId, Project, TrackId};
use doc_support::{
    FRAME, Mirror, agent, append, config, drain, item_name, project, replay, spawn, submit, ui,
};
use serde_json::{Value, json};

/// The idle budget of a gesture, in ms (the `retry_after_ms` right after it moved).
fn idle_ms() -> u64 {
    u64::try_from(GESTURE_IDLE_TIMEOUT.as_millis()).unwrap()
}

/// The reason, hint and detail of an `item_in_gesture` conflict (panics on anything else).
fn in_gesture(e: &CmdError) -> (Option<u64>, String) {
    match e {
        CmdError::Conflict {
            reason: ConflictReason::ItemInGesture,
            retry_after_ms,
            detail,
            ..
        } => (*retry_after_ms, detail.clone()),
        other => panic!("expected item_in_gesture, got {other:?}"),
    }
}

/// A plugin on connection 3 (a third actor).
fn plugin() -> ActorCtx {
    ActorCtx::plugin(ConnId::new(3), "acme.tools").unwrap()
}

/// The start and duration of an item, in flicks.
fn placement(doc: &Project, item: ItemId) -> (i64, i64) {
    let found = doc.find_item(&item).unwrap();
    (found.item.start().flicks(), found.item.duration().flicks())
}

fn step(h: &DocHandle, ctx: &ActorCtx, tx: TxId, name: &str, params: Value) -> Applied {
    h.submit(Submit::new(ctx.clone(), name, params).in_tx(tx))
        .wait()
        .unwrap()
}

fn verify(h: &DocHandle) {
    h.run(|s| s.verify()).wait().unwrap().unwrap();
}

fn locks(h: &DocHandle) -> LockList {
    h.locks(&ui()).wait().unwrap()
}

/// A state on a manual clock (fake time for idle deadlines).
fn clocked(doc: Project) -> (DocState, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(WallTime::from_millis(1_000_000)));
    let s = DocState::new(config(doc).with_clock(Arc::clone(&clock) as _));
    (s, clock)
}

fn run(s: &mut DocState, ctx: &ActorCtx, name: &str, params: Value) -> Result<Applied, CmdError> {
    s.submit(Submit::new(ctx.clone(), name, params))
}

fn run_in(
    s: &mut DocState,
    ctx: &ActorCtx,
    tx: TxId,
    name: &str,
    params: Value,
) -> Result<Applied, CmdError> {
    s.submit(Submit::new(ctx.clone(), name, params).in_tx(tx))
}

fn append_in(s: &mut DocState, ctx: &ActorCtx, track: TrackId, frames: u32) -> ItemId {
    let out = run(
        s,
        ctx,
        "timeline.append_clip",
        json!({"track": track, "frames": frames}),
    )
    .unwrap()
    .output;
    serde_json::from_value(out["item"].clone()).unwrap()
}

/// Acceptance 1: connection A opens a gesture moving item X; connection B's `clip.trim` on X
/// is refused, on Y it succeeds; after A drops, X is restored to its pre-tx state and B's
/// retry succeeds.
#[test]
fn acceptance_a_dropped_gesture_restores_its_item_and_releases_the_lock() {
    let (doc, tracks, _) = project(2);
    let actor = spawn(doc);
    let h = actor.handle();
    let (owner, other) = (ui(), agent());
    let x = append(&h, &owner, tracks[0], 30);
    let y = append(&h, &owner, tracks[1], 30);
    let pre = h.committed_snapshot();
    let mut events = h.subscribe();
    let mut mirror = Mirror::new(&pre.doc, pre.rev);

    let tx = h
        .begin_tx(&owner, TxKind::Gesture, "Move X")
        .wait()
        .unwrap()
        .id;
    for at in [40, 60, 90] {
        step(&h, &owner, tx, "clip.move", json!({"item": x, "at": at}));
    }
    let dragged = h.snapshot();
    assert_eq!(placement(&dragged.doc, x).0, 90 * FRAME);

    // B trims X (another field than the move's): refused before anything is applied.
    let err = submit(&h, &other, "clip.trim", json!({"item": x, "frames": 10})).unwrap_err();
    let (retry, detail) = in_gesture(&err);
    assert_eq!(retry, Some(idle_ms()));
    assert!(detail.contains(&x.to_string()), "{detail}");
    assert!(
        detail.contains(&tx.to_string()) && detail.contains("user"),
        "{detail}"
    );
    assert_eq!(*h.snapshot(), *dragged, "a refused request changes nothing");
    // On Y it proceeds (committed alone; the live document keeps the gesture on top).
    let trimmed = submit(&h, &other, "clip.trim", json!({"item": y, "frames": 10})).unwrap();
    assert_eq!(trimmed.committed_rev, h.committed_snapshot().rev);
    assert_eq!(placement(&h.snapshot().doc, y).1, 10 * FRAME);
    assert_eq!(placement(&h.snapshot().doc, x).0, 90 * FRAME);

    // A drops mid-gesture: rolled back immediately, X is as before the gesture.
    let closed = h.close_connection(owner.conn()).wait().unwrap();
    assert_eq!(closed.len(), 1);
    assert_eq!(
        (closed[0].tx_id, closed[0].outcome),
        (tx, TxOutcome::RolledBack)
    );
    let live = h.snapshot();
    assert_eq!(placement(&live.doc, x), placement(&pre.doc, x));
    assert_eq!(live.doc, h.committed_snapshot().doc);
    assert_eq!(locks(&h).transactions.len(), 0);

    // `doc.patched{cause: rollback}` restores X in mirrors, then `tx.closed` releases it.
    let seen = drain(&mut events);
    for e in &seen {
        mirror.apply(e);
    }
    mirror.assert_matches(&live.doc);
    let rollback = seen
        .iter()
        .filter_map(|e| e.as_patched())
        .next_back()
        .unwrap();
    assert_eq!((rollback.cause, rollback.tx_id), (PatchCause::Rollback, tx));
    let ended = seen.last().unwrap().as_tx_closed().unwrap();
    assert_eq!((ended.tx_id, ended.outcome), (tx, TxOutcome::RolledBack));
    assert_eq!(ended.items, [x]);

    // B's retry succeeds.
    submit(&h, &other, "clip.trim", json!({"item": x, "frames": 10})).unwrap();
    assert_eq!(placement(&h.snapshot().doc, x), (0, 10 * FRAME));
    verify(&h);
}

#[test]
fn every_kind_of_change_by_another_actor_is_refused_on_a_locked_item() {
    let (doc, tracks, _) = project(3);
    let actor = spawn(doc);
    let h = actor.handle();
    let user = ui();
    let x = append(&h, &user, tracks[0], 30);
    let y = append(&h, &user, tracks[1], 30);
    // The agent renamed X earlier (something to undo).
    submit(&h, &agent(), "item.rename", json!({"item": x, "name": "a"})).unwrap();
    let gesture = h
        .begin_tx(&user, TxKind::Gesture, "Drag X")
        .wait()
        .unwrap()
        .id;
    step(
        &h,
        &user,
        gesture,
        "clip.move",
        json!({"item": x, "at": 50}),
    );
    let before = h.snapshot();

    let refused = |ctx: &ActorCtx, tx: Option<TxId>, name: &str, params: Value| {
        let mut request = Submit::new(ctx.clone(), name, params);
        request.tx = tx;
        let err = h.submit(request).wait().unwrap_err();
        in_gesture(&err);
        assert_eq!(*h.snapshot(), *before, "`{name}` changed something");
    };
    // Implicit commands, whatever field they change.
    for (name, params) in [
        ("item.rename", json!({"item": x, "name": "b"})),
        ("item.set_enabled", json!({"item": x, "enabled": false})),
        ("clip.trim", json!({"item": x, "frames": 5})),
        ("item.delete", json!({"item": x})),
    ] {
        refused(&agent(), None, name, params);
    }
    // A change above the item: removing its track.
    refused(&agent(), None, "track.remove", json!({"track": tracks[0]}));
    // Undoing the agent's earlier change of X.
    refused(&agent(), None, "edit.undo", json!({}));
    // Steps of the agent's own batch and of a plugin's proposal.
    let batch = h
        .begin_tx(&agent(), TxKind::Batch, "Fix")
        .wait()
        .unwrap()
        .id;
    refused(
        &agent(),
        Some(batch),
        "item.rename",
        json!({"item": x, "name": "c"}),
    );
    let proposal = h
        .begin_tx(&plugin(), TxKind::Proposal, "Tidy")
        .wait()
        .unwrap()
        .id;
    refused(
        &plugin(),
        Some(proposal),
        "clip.move",
        json!({"item": x, "at": 70}),
    );
    let info = h
        .run(move |s| s.transaction(proposal))
        .wait()
        .unwrap()
        .unwrap();
    assert_eq!(
        (info.steps, info.items.len()),
        (0, 0),
        "nothing recorded or locked"
    );

    // Everything else proceeds: Y, the third track, the project.
    step(
        &h,
        &agent(),
        batch,
        "clip.trim",
        json!({"item": y, "frames": 5}),
    );
    step(
        &h,
        &plugin(),
        proposal,
        "item.rename",
        json!({"item": y, "name": "p"}),
    );
    submit(&h, &agent(), "track.remove", json!({"track": tracks[2]})).unwrap();
    submit(&h, &agent(), "project.rename", json!({"name": "Agent's"})).unwrap();
    verify(&h);
}

#[test]
fn the_lock_holders_own_actor_is_not_blocked_but_only_the_owner_uses_the_transaction() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let user = ui();
    let x = append(&h, &user, tracks[0], 30);
    let tx = h
        .begin_tx(&user, TxKind::Gesture, "Drag")
        .wait()
        .unwrap()
        .id;
    step(&h, &user, tx, "clip.move", json!({"item": x, "at": 50}));

    // The same connection, outside the gesture (a keyboard trim while dragging).
    submit(&h, &user, "clip.trim", json!({"item": x, "frames": 20})).unwrap();
    // The same actor on another connection (a second editor window).
    let window = ActorCtx::ui(ConnId::new(9));
    submit(&h, &window, "item.rename", json!({"item": x, "name": "w"})).unwrap();
    // ... but the field the gesture changed still collides (the field-level backstop).
    let err = submit(&h, &window, "clip.move", json!({"item": x, "at": 10})).unwrap_err();
    in_gesture(&err);
    // Only the owning connection may run in, commit or cancel it, also for the same actor.
    let not_owner = |e: CmdError| match e {
        CmdError::Locked {
            kind: ResourceKind::Transaction,
            by: Some(by),
            tx: Some(t),
            ..
        } => assert_eq!((by.as_str(), t), ("user", tx.to_string())),
        other => panic!("{other:?}"),
    };
    not_owner(
        h.submit(Submit::new(window.clone(), "project.noop", json!({})).in_tx(tx))
            .wait()
            .unwrap_err(),
    );
    not_owner(h.commit_tx(&window, tx).wait().unwrap_err());
    not_owner(h.cancel_tx(&window, tx).wait().unwrap_err());
    // One open transaction per connection.
    let err = h
        .begin_tx(&user, TxKind::Batch, "another")
        .wait()
        .unwrap_err();
    assert_eq!(err.code(), "invalid");

    h.commit_tx(&user, tx).wait().unwrap();
    let doc = h.snapshot().doc.clone();
    assert_eq!(placement(&doc, x), (50 * FRAME, 20 * FRAME));
    assert_eq!(item_name(&doc, x).as_deref(), Some("w"));
    verify(&h);
}

#[test]
fn retry_after_counts_down_to_the_gesture_deadline_and_proposals_give_none() {
    let (doc, tracks, _) = project(2);
    let (mut s, clock) = clocked(doc);
    let (user, bot) = (ui(), agent());
    let x = append_in(&mut s, &user, tracks[0], 30);
    let y = append_in(&mut s, &user, tracks[1], 30);
    let gesture = s.begin_tx(&user, TxKind::Gesture, "Drag").unwrap().id;
    run_in(
        &mut s,
        &user,
        gesture,
        "clip.move",
        json!({"item": x, "at": 40}),
    )
    .unwrap();
    let trim_x = json!({"item": x, "frames": 5});
    let retry =
        |s: &mut DocState| in_gesture(&run(s, &bot, "clip.trim", trim_x.clone()).unwrap_err()).0;

    assert_eq!(retry(&mut s), Some(30_000));
    clock.advance(std::time::Duration::from_secs(10));
    assert_eq!(retry(&mut s), Some(20_000));
    // Activity pushes the deadline back.
    run_in(
        &mut s,
        &user,
        gesture,
        "clip.move",
        json!({"item": x, "at": 41}),
    )
    .unwrap();
    assert_eq!(retry(&mut s), Some(30_000));
    // Close to the deadline the hint never drops below the floor.
    clock.advance(std::time::Duration::from_millis(29_900));
    assert_eq!(retry(&mut s), Some(SOFT_LOCK_RETRY_MS));
    let expires = s.transaction(gesture).unwrap().expires_at.unwrap();
    assert_eq!(expires.as_millis(), 1_000_000 + 10_000 + 30_000);

    // A proposal has no deadline: no hint, the agent waits for a person.
    let proposal = s
        .begin_tx(&bot, TxKind::Proposal, "Copilot turn")
        .unwrap()
        .id;
    run_in(
        &mut s,
        &bot,
        proposal,
        "item.rename",
        json!({"item": y, "name": "p"}),
    )
    .unwrap();
    let err = run(&mut s, &user, "clip.trim", json!({"item": y, "frames": 5})).unwrap_err();
    let (retry, detail) = in_gesture(&err);
    assert_eq!(retry, None);
    assert!(detail.contains("accepted or rejected"), "{detail}");
    assert_eq!(s.transaction(proposal).unwrap().expires_at, None);
    s.verify().unwrap();
}

#[test]
fn an_idle_gesture_expires_and_releases_its_locks() {
    let (doc, tracks, _) = project(1);
    let (mut s, clock) = clocked(doc);
    let (user, bot) = (ui(), agent());
    let x = append_in(&mut s, &user, tracks[0], 30);
    let pre = s.committed_doc().clone();
    let tx = s.begin_tx(&user, TxKind::Gesture, "Drag").unwrap().id;
    run_in(&mut s, &user, tx, "clip.move", json!({"item": x, "at": 40})).unwrap();
    let _ = s.take_events();
    clock.advance(std::time::Duration::from_secs(29));
    in_gesture(&run(&mut s, &bot, "clip.trim", json!({"item": x, "frames": 5})).unwrap_err());

    // At the deadline, the agent's own request expires the gesture first, then runs.
    clock.advance(std::time::Duration::from_secs(1));
    run(&mut s, &bot, "clip.trim", json!({"item": x, "frames": 5})).unwrap();
    assert_eq!(s.transactions().len(), 0);
    assert_eq!(placement(s.doc(), x), (placement(&pre, x).0, 5 * FRAME));
    let events = s.take_events();
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            DocEvent::Patched(p) if p.cause == PatchCause::Rollback => "rollback",
            DocEvent::Patched(_) => "patched",
            DocEvent::TxClosed(t) => {
                assert_eq!(
                    (t.tx_id, t.outcome, &t.items[..]),
                    (tx, TxOutcome::Expired, &[x][..])
                );
                "tx.closed"
            }
            DocEvent::History(_) => "history",
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(kinds[..3], ["rollback", "tx.closed", "patched"]);
    assert!(kinds[3..].iter().all(|k| *k == "history"), "{kinds:?}");
    s.verify().unwrap();
}

#[test]
fn a_proposal_holds_its_locks_without_timeout_and_is_accepted_as_one_agent_step() {
    let (doc, tracks, _) = project(3);
    let (mut s, clock) = clocked(doc);
    let (user, copilot) = (ui(), agent());
    let x = append_in(&mut s, &user, tracks[0], 30);
    let y = append_in(&mut s, &user, tracks[1], 30);
    let free = append_in(&mut s, &user, tracks[2], 30);
    let proposal = s
        .begin_tx(&copilot, TxKind::Proposal, "Copilot turn")
        .unwrap()
        .id;
    run_in(
        &mut s,
        &copilot,
        proposal,
        "item.rename",
        json!({"item": x, "name": "hook"}),
    )
    .unwrap();
    run_in(
        &mut s,
        &copilot,
        proposal,
        "clip.move",
        json!({"item": x, "at": 60}),
    )
    .unwrap();
    run_in(
        &mut s,
        &copilot,
        proposal,
        "clip.trim",
        json!({"item": y, "frames": 12}),
    )
    .unwrap();
    let mut items = vec![x, y];
    items.sort();
    assert_eq!(s.transaction(proposal).unwrap().items, items);

    // The user works on Z meanwhile; X and Y wait for the proposal.
    run(
        &mut s,
        &user,
        "item.rename",
        json!({"item": free, "name": "mine"}),
    )
    .unwrap();
    in_gesture(&run(&mut s, &user, "clip.trim", json!({"item": x, "frames": 5})).unwrap_err());
    // No idle timeout: an hour later it is still open and still locks.
    clock.advance(std::time::Duration::from_secs(3600));
    assert_eq!(s.expire_idle().len(), 0);
    in_gesture(&run(&mut s, &user, "clip.trim", json!({"item": y, "frames": 5})).unwrap_err());

    // Accept: one undo step tagged with the agent, locks released.
    let _ = s.take_events();
    let closed = s.commit_tx(&copilot, proposal).unwrap();
    assert_eq!(
        (closed.kind, closed.outcome),
        (TxKind::Proposal, TxOutcome::Committed)
    );
    assert_eq!(s.transactions().len(), 0);
    let ended = s.take_events().into_iter().find_map(|e| match e {
        DocEvent::TxClosed(t) => Some(t),
        _ => None,
    });
    assert_eq!(ended.unwrap().items, items);
    let history = s.history_page(10, None, None).unwrap();
    let entry = &history.entries[0];
    assert_eq!(
        (entry.tx_id, entry.actor.to_string()),
        (proposal, "agent:claude".to_owned())
    );
    assert_eq!(entry.label, "Copilot turn");
    // The user's retry succeeds; "undo agent turn" still undoes the whole turn.
    run(&mut s, &user, "clip.trim", json!({"item": x, "frames": 5})).unwrap();
    run(&mut s, &user, "edit.undo", json!({"actor": "agent:claude"})).unwrap();
    assert_eq!(item_name(s.doc(), x), None);
    assert_eq!(placement(s.doc(), x), (0, 5 * FRAME));
    assert_eq!(placement(s.doc(), y).1, 30 * FRAME);
    s.verify().unwrap();
}

#[test]
fn rejecting_a_proposal_rolls_it_back_and_releases_its_locks() {
    let (doc, tracks, _) = project(1);
    let (mut s, _clock) = clocked(doc);
    let (user, copilot) = (ui(), agent());
    let x = append_in(&mut s, &user, tracks[0], 30);
    let pre = s.committed_doc().clone();
    let proposal = s
        .begin_tx(&copilot, TxKind::Proposal, "Copilot turn")
        .unwrap()
        .id;
    run_in(
        &mut s,
        &copilot,
        proposal,
        "clip.trim",
        json!({"item": x, "frames": 3}),
    )
    .unwrap();
    in_gesture(&run(&mut s, &user, "item.delete", json!({"item": x})).unwrap_err());
    let closed = s.cancel_tx(&copilot, proposal).unwrap();
    assert_eq!(closed.outcome, TxOutcome::Cancelled);
    assert_eq!(s.doc(), &pre);
    assert_eq!(s.locks(None).transactions.len(), 0);
    assert_eq!(
        s.history_page(10, None, None).unwrap().entries.len(),
        1,
        "nothing logged"
    );
    run(&mut s, &user, "item.delete", json!({"item": x})).unwrap();
    s.verify().unwrap();
}

#[test]
fn items_created_in_a_gesture_are_locked_too() {
    let (doc, tracks, _) = project(1);
    let (mut s, _clock) = clocked(doc);
    let (user, bot) = (ui(), agent());
    let tx = s
        .begin_tx(&user, TxKind::Gesture, "Paint clips")
        .unwrap()
        .id;
    let out = run_in(
        &mut s,
        &user,
        tx,
        "timeline.append_clip",
        json!({"track": tracks[0], "frames": 5}),
    )
    .unwrap()
    .output;
    let new: ItemId = serde_json::from_value(out["item"].clone()).unwrap();
    // Visible live, not committed: the agent's command explains why it cannot touch it.
    assert!(s.doc().find_item(&new).is_some());
    in_gesture(
        &run(
            &mut s,
            &bot,
            "item.rename",
            json!({"item": new, "name": "b"}),
        )
        .unwrap_err(),
    );
    s.cancel_tx(&user, tx).unwrap();
    // Cancelled: now it really does not exist.
    let err = run(
        &mut s,
        &bot,
        "item.rename",
        json!({"item": new, "name": "b"}),
    )
    .unwrap_err();
    assert_eq!(err, doc_support::item_not_found(new));
    s.verify().unwrap();
}

#[test]
fn a_batch_locks_nothing_but_cannot_commit_over_another_actors_locked_item() {
    let (doc, tracks, _) = project(1);
    let (mut s, _clock) = clocked(doc);
    let (user, bot) = (ui(), agent());
    let x = append_in(&mut s, &user, tracks[0], 30);
    let batch = s.begin_tx(&bot, TxKind::Batch, "Rename pass").unwrap().id;
    run_in(
        &mut s,
        &bot,
        batch,
        "item.rename",
        json!({"item": x, "name": "agent"}),
    )
    .unwrap();
    assert_eq!(s.transaction(batch).unwrap().items.len(), 0);
    // The batch does not block the user's gesture on X (other fields).
    let gesture = s.begin_tx(&user, TxKind::Gesture, "Drag").unwrap().id;
    run_in(
        &mut s,
        &user,
        gesture,
        "clip.move",
        json!({"item": x, "at": 40}),
    )
    .unwrap();
    // The batch's commit waits for the gesture, and stays open.
    let (retry, _) = in_gesture(&s.commit_tx(&bot, batch).unwrap_err());
    assert_eq!(retry, Some(30_000));
    assert!(s.transaction(batch).is_some());
    s.commit_tx(&user, gesture).unwrap();
    s.commit_tx(&bot, batch).unwrap();
    assert_eq!(item_name(s.doc(), x).as_deref(), Some("agent"));
    assert_eq!(placement(s.doc(), x).0, 40 * FRAME);
    s.verify().unwrap();
}

#[test]
fn edit_locks_lists_who_is_editing_what() {
    let (doc, tracks, _) = project(2);
    let (mut s, _clock) = clocked(doc);
    let (user, bot) = (ui(), agent());
    let x = append_in(&mut s, &user, tracks[0], 30);
    let y = append_in(&mut s, &user, tracks[1], 30);
    let gesture = s.begin_tx(&user, TxKind::Gesture, "Drag X").unwrap().id;
    run_in(
        &mut s,
        &user,
        gesture,
        "clip.move",
        json!({"item": x, "at": 40}),
    )
    .unwrap();
    let proposal = s
        .begin_tx(&bot, TxKind::Proposal, "Copilot turn")
        .unwrap()
        .id;
    run_in(
        &mut s,
        &bot,
        proposal,
        "item.rename",
        json!({"item": y, "name": "p"}),
    )
    .unwrap();
    let batch = s.begin_tx(&plugin(), TxKind::Batch, "Import").unwrap().id;

    let out = run(&mut s, &bot, "edit.locks", json!({})).unwrap();
    assert!(out.ops.is_empty() && out.tx_id.is_none());
    let list: LockList = serde_json::from_value(out.output.clone()).unwrap();
    assert_eq!(list, s.locks(Some(bot.conn())));
    let [drag, turn, import] = &list.transactions[..] else {
        panic!("{list:?}")
    };
    assert_eq!(
        (drag.tx, drag.kind, drag.actor.to_string()),
        (gesture, TxKind::Gesture, "user".into())
    );
    assert_eq!((&drag.items[..], drag.mine), (&[x][..], false));
    assert_eq!(
        drag.expires_at.unwrap().as_millis(),
        1_000_000 + u64::try_from(GESTURE_IDLE_TIMEOUT.as_millis()).unwrap()
    );
    assert_eq!(
        (turn.tx, &turn.items[..], turn.mine, turn.expires_at),
        (proposal, &[y][..], true, None)
    );
    assert_eq!(
        (import.tx, import.items.len(), import.label.as_str()),
        (batch, 0, "Import")
    );
    assert_eq!(list.holder(x).map(|t| t.tx), Some(gesture));
    assert_eq!(list.holder(y).map(|t| t.tx), Some(proposal));
    // Wire form: ids and actors as strings.
    assert_eq!(
        out.output["transactions"][0]["items"][0],
        json!(x.to_string())
    );
    assert_eq!(
        out.output["transactions"][1]["actor"],
        json!("agent:claude")
    );
    // Read-only: not remembered for dedup, nothing changes.
    assert_eq!(s.rev(), list.rev);
}

#[test]
fn tx_events_announce_opened_locked_and_closed() {
    let (doc, tracks, _) = project(2);
    let actor = spawn(doc);
    let h = actor.handle();
    let user = ui();
    let x = append(&h, &user, tracks[0], 30);
    let y = append(&h, &user, tracks[1], 30);
    let mut events = h.subscribe();
    let tx = h
        .begin_tx(&user, TxKind::Gesture, "Drag")
        .wait()
        .unwrap()
        .id;
    step(&h, &user, tx, "clip.move", json!({"item": x, "at": 40}));
    step(&h, &user, tx, "clip.move", json!({"item": x, "at": 41}));
    step(&h, &user, tx, "clip.move", json!({"item": y, "at": 40}));
    h.commit_tx(&user, tx).wait().unwrap();
    let mut both = vec![x, y];
    both.sort();
    let wire: Vec<Value> = drain(&mut events)
        .iter()
        .map(|e| serde_json::to_value(&**e).unwrap())
        .collect();
    let names: Vec<&str> = wire.iter().map(|e| e["event"].as_str().unwrap()).collect();
    assert_eq!(
        names[..8],
        [
            "tx.opened",
            "doc.patched",
            "tx.locked",
            "doc.patched",
            "doc.patched",
            "tx.locked",
            "doc.patched",
            "tx.closed",
        ]
    );
    assert!(
        names[8..].iter().all(|n| *n == "edit.history_changed"),
        "{names:?}"
    );
    assert_eq!(
        wire[0],
        json!({"event": "tx.opened", "tx_id": tx.to_string(), "kind": "gesture", "label": "Drag",
               "actor": "user", "items": [], "rev": 2})
    );
    assert_eq!(wire[2]["items"], json!([x.to_string()]));
    assert_eq!(wire[5]["items"], serde_json::to_value(&both).unwrap());
    assert_eq!(
        wire[7],
        json!({"event": "tx.closed", "tx_id": tx.to_string(), "kind": "gesture", "actor": "user",
               "outcome": "committed", "items": serde_json::to_value(&both).unwrap(),
               "rev": 6, "committed_rev": 6})
    );
}

#[test]
fn a_closed_connection_cannot_open_transactions() {
    let (doc, _, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let gone = ActorCtx::agent(ConnId::new(5), "late").unwrap();
    assert_eq!(h.close_connection(gone.conn()).wait().unwrap().len(), 0);
    assert!(h.run(move |s| s.is_closed(ConnId::new(5))).wait().unwrap());
    // A begin that raced the close is refused, so no transaction (or lock) is stranded.
    let err = h
        .begin_tx(&gone, TxKind::Gesture, "late")
        .wait()
        .unwrap_err();
    assert_eq!(err.code(), "invalid");
    let err = submit(
        &h,
        &gone,
        "edit.begin_tx",
        json!({"kind": "proposal", "label": "x"}),
    )
    .unwrap_err();
    assert_eq!(err.code(), "invalid");
    // Single commands it sent before closing still run.
    submit(&h, &gone, "project.rename", json!({"name": "late"})).unwrap();
    // Internal jobs never close.
    h.close_connection(ConnId::INTERNAL).wait().unwrap();
    let tx = h
        .begin_tx(&ActorCtx::system(), TxKind::Batch, "job")
        .wait()
        .unwrap()
        .id;
    h.commit_tx(&ActorCtx::system(), tx).wait().unwrap();
    assert_eq!(locks(&h).transactions.len(), 0);
}

/// Acceptance 2: a save during an open gesture writes a document equal to the pre-tx committed
/// one (byte-compare), and nothing of the gesture is journaled; a crash with it open loses
/// only the gesture.
#[test]
fn acceptance_saves_and_the_journal_never_see_an_open_gesture() {
    let (initial, tracks, _) = project(1);
    let journal: Arc<Mutex<Vec<JournaledCommit>>> = Arc::default();
    let sink = {
        let journal = Arc::clone(&journal);
        move |r: &CommitRecord<'_>| {
            journal.lock().unwrap().push(r.to_owned());
            Ok(())
        }
    };
    let actor = DocActor::spawn(config(initial.clone()).with_journal(Box::new(sink))).unwrap();
    let h = actor.handle();
    let user = ui();
    let x = append(&h, &user, tracks[0], 30);
    // What a save writes: the committed snapshot, serialized.
    let save = |h: &DocHandle| serde_json::to_vec_pretty(&h.committed_snapshot().doc).unwrap();
    let pre_bytes = save(&h);
    let journaled = || journal.lock().unwrap().len();
    assert_eq!(journaled(), 1);

    let tx = h
        .begin_tx(&user, TxKind::Gesture, "Drag")
        .wait()
        .unwrap()
        .id;
    for at in 31..80 {
        step(&h, &user, tx, "clip.move", json!({"item": x, "at": at}));
        if at % 10 == 0 {
            assert_eq!(save(&h), pre_bytes, "a save during the gesture");
            assert_ne!(
                serde_json::to_vec_pretty(&h.snapshot().doc).unwrap(),
                pre_bytes
            );
        }
    }
    step(
        &h,
        &user,
        tx,
        "timeline.append_clip",
        json!({"track": tracks[0], "frames": 3}),
    );
    assert_eq!(save(&h), pre_bytes);
    assert_eq!(journaled(), 1, "nothing journaled for the open gesture");
    // Dropping the connection journals nothing either.
    h.close_connection(user.conn()).wait().unwrap();
    assert_eq!(save(&h), pre_bytes);
    assert_eq!(journaled(), 1);

    // A crash with a gesture open loses only that gesture: recovery replays the journal.
    let again = ActorCtx::ui(ConnId::new(11));
    let tx = h
        .begin_tx(&again, TxKind::Gesture, "Drag")
        .wait()
        .unwrap()
        .id;
    step(&h, &again, tx, "clip.trim", json!({"item": x, "frames": 7}));
    drop(h);
    actor.shutdown().unwrap();
    let commits = journal.lock().unwrap().clone();
    assert_eq!(commits.len(), 1);
    let patches: Vec<_> = commits.iter().map(|c| c.patch.clone()).collect();
    let recovered = replay(&initial, &patches);
    assert_eq!(serde_json::to_vec_pretty(&recovered).unwrap(), pre_bytes);
    let state = DocState::new(
        config(recovered)
            .with_rev(commits[0].rev)
            .with_recovered_history(commits),
    );
    assert_eq!(state.transactions().len(), 0);
    state.verify().unwrap();
    assert_eq!(state.committed_rev(), Rev(1));
}
