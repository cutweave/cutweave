// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.12: the soft-lock extension (arch §5.2, spec §5 INV-301–305, ADR-0023). The conflict
//! names its holder and a retry hint; refused requests leave everything byte-identical; idle
//! gestures are cancelled after 30 s on the injectable clock (by the state and by the actor's
//! own timer) and announced by `gesture.cancelled`; proposals never expire; gesture updates
//! with `_meta.replace` replace each other atomically while the lock set only grows; ripple-
//! shifted items are locked.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod doc_support;

use std::{sync::Arc, time::Duration};

use cutweave_commands::{
    Actor, ActorCtx, Applied, CmdError, ConflictReason, ConnId, DocActor, DocEvent, DocState,
    GESTURE_IDLE_TIMEOUT, GestureCancelReason, ManualClock, PatchCause, Rev, Submit, TxId, TxKind,
    TxOutcome, WallTime,
};
use cutweave_model::{ItemId, Project, TrackId};
use doc_support::{FRAME, Mirror, agent, config, item_name, items, project, ui};
use serde_json::{Value, json};

fn idle_ms() -> u64 {
    u64::try_from(GESTURE_IDLE_TIMEOUT.as_millis()).unwrap()
}

fn plugin() -> ActorCtx {
    ActorCtx::plugin(ConnId::new(3), "acme.tools").unwrap()
}

/// A state on a manual clock.
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

/// A `_meta.replace` update of gesture `tx`.
fn update(
    s: &mut DocState,
    ctx: &ActorCtx,
    tx: TxId,
    name: &str,
    params: Value,
) -> Result<Applied, CmdError> {
    s.submit(Submit::new(ctx.clone(), name, params).in_tx(tx).replacing())
}

fn append(s: &mut DocState, ctx: &ActorCtx, track: TrackId, frames: u32) -> ItemId {
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

/// The holder, hint and detail of an `item_in_gesture` conflict (panics on anything else).
fn in_use(e: &CmdError) -> (String, Option<u64>, String) {
    match e {
        CmdError::Conflict {
            reason: ConflictReason::ItemInGesture,
            retry_after_ms,
            holder: Some(holder),
            detail,
            ..
        } => (holder.to_string(), *retry_after_ms, detail.clone()),
        other => panic!("expected item_in_gesture with a holder, got {other:?}"),
    }
}

fn start_frame(doc: &Project, item: ItemId) -> i64 {
    doc.find_item(&item).unwrap().item.start().flicks() / FRAME
}

/// Everything a refused request must leave unchanged, byte for byte.
fn fingerprint(s: &DocState) -> (Vec<u8>, Vec<u8>, Rev, Rev, Vec<u8>, usize) {
    (
        serde_json::to_vec(s.doc()).unwrap(),
        serde_json::to_vec(s.committed_doc()).unwrap(),
        s.rev(),
        s.committed_rev(),
        serde_json::to_vec(&s.locks(None)).unwrap(),
        history_len(s),
    )
}

fn history_len(s: &DocState) -> usize {
    s.history_page(1_000, None, None).unwrap().entries.len()
}

#[test]
fn the_conflict_names_the_holder_with_a_retry_hint_on_the_wire() {
    let (doc, tracks, _) = project(2);
    let (mut s, clock) = clocked(doc);
    let (user, bot) = (ui(), agent());
    let x = append(&mut s, &user, tracks[0], 30);
    let y = append(&mut s, &user, tracks[1], 30);

    // The person drags X: the agent is told who holds it and when to retry.
    let drag = s.begin_tx(&user, TxKind::Gesture, "Move clip").unwrap().id;
    update(
        &mut s,
        &user,
        drag,
        "clip.move",
        json!({"item": x, "at": 40}),
    )
    .unwrap();
    clock.advance(Duration::from_secs(10));
    let err = run(&mut s, &bot, "clip.move", json!({"item": x, "at": 90})).unwrap_err();
    let (holder, retry, detail) = in_use(&err);
    assert_eq!(holder, "user");
    assert_eq!(retry, Some(idle_ms() - 10_000));
    assert!(
        detail.contains(&drag.to_string()) && detail.contains("Move clip"),
        "{detail}"
    );
    let wire = serde_json::to_value(&err).unwrap();
    assert_eq!(wire["code"], "conflict");
    assert_eq!(wire["reason"], "item_in_gesture");
    assert_eq!(wire["holder"], "user");
    assert_eq!(wire["retry_after_ms"], idle_ms() - 10_000);
    assert!(wire["current_rev"].is_u64());
    assert!(
        wire["message"]
            .as_str()
            .unwrap()
            .contains("item_in_gesture")
    );
    let back: CmdError = serde_json::from_value(wire).unwrap();
    assert_eq!(back, err);

    // The agent's proposal holds Y: the person is told who, with no time hint.
    let proposal = s.begin_tx(&bot, TxKind::Proposal, "Tidy up").unwrap().id;
    run_in(
        &mut s,
        &bot,
        proposal,
        "item.rename",
        json!({"item": y, "name": "b"}),
    )
    .unwrap();
    let err = run(
        &mut s,
        &user,
        "item.rename",
        json!({"item": y, "name": "a"}),
    )
    .unwrap_err();
    let (holder, retry, detail) = in_use(&err);
    assert_eq!((holder.as_str(), retry), ("agent:claude", None));
    assert!(detail.contains("accepted or rejected"), "{detail}");
    assert!(
        serde_json::to_value(&err)
            .unwrap()
            .get("retry_after_ms")
            .is_none()
    );

    // Other conflict reasons carry no holder.
    let stale = s
        .submit(Submit::new(bot.clone(), "project.noop", json!({})).with_expected_rev(Rev(0)))
        .unwrap_err();
    assert!(
        matches!(stale, CmdError::Conflict { holder: None, .. }),
        "{stale:?}"
    );
    // Same-actor commands are never blocked by the lock.
    run(&mut s, &bot, "clip.trim", json!({"item": y, "frames": 20})).unwrap();
    s.verify().unwrap();
}

/// Acceptance: rejected commands do not change the doc or the rev (byte-identical snapshot),
/// nor the history, the locks or the events.
#[test]
fn refused_requests_leave_everything_byte_identical() {
    let (doc, tracks, _) = project(3);
    let (mut s, _clock) = clocked(doc);
    let (user, bot, other) = (ui(), agent(), plugin());
    let x = append(&mut s, &user, tracks[0], 30);
    let y = append(&mut s, &user, tracks[1], 30);
    let z = append(&mut s, &bot, tracks[2], 30);
    let drag = s.begin_tx(&user, TxKind::Gesture, "Move clip").unwrap().id;
    update(
        &mut s,
        &user,
        drag,
        "clip.move",
        json!({"item": x, "at": 40}),
    )
    .unwrap();
    let proposal = s.begin_tx(&bot, TxKind::Proposal, "Tidy").unwrap().id;
    run_in(
        &mut s,
        &bot,
        proposal,
        "item.rename",
        json!({"item": y, "name": "p"}),
    )
    .unwrap();
    let batch = s
        .begin_tx(&other, TxKind::Batch, "Plugin batch")
        .unwrap()
        .id;
    run_in(
        &mut s,
        &other,
        batch,
        "item.rename",
        json!({"item": z, "name": "q"}),
    )
    .unwrap();
    let _ = s.take_events();
    let _ = s.take_changes();
    let before = fingerprint(&s);

    let refusals: Vec<(&str, Result<Applied, CmdError>)> = vec![
        (
            "agent edits X",
            run(&mut s, &bot, "item.rename", json!({"item": x, "name": "n"})),
        ),
        (
            "agent deletes X",
            run(&mut s, &bot, "item.delete", json!({"item": x})),
        ),
        (
            "agent removes X's track",
            run(&mut s, &bot, "track.remove", json!({"track": tracks[0]})),
        ),
        (
            "agent ripples X",
            run(
                &mut s,
                &bot,
                "timeline.shift_after",
                json!({"track": tracks[0], "at": 0, "frames": 5}),
            ),
        ),
        (
            "person edits the proposal's Y",
            run(&mut s, &user, "clip.trim", json!({"item": y, "frames": 4})),
        ),
        (
            // INV-305: a replacing update that would change another actor's item.
            "person's update onto Y",
            update(
                &mut s,
                &user,
                drag,
                "clip.trim",
                json!({"item": y, "frames": 4}),
            ),
        ),
        (
            "plugin step onto X",
            run_in(
                &mut s,
                &other,
                batch,
                "item.rename",
                json!({"item": x, "name": "q"}),
            ),
        ),
    ];
    for (what, result) in refusals {
        let err = result.expect_err(what);
        assert!(
            matches!(err.code(), "conflict" | "not_found"),
            "{what}: {err}"
        );
        assert_eq!(fingerprint(&s), before, "{what} changed something");
        assert!(s.take_events().is_empty(), "{what} emitted events");
        assert!(!s.take_changes().any(), "{what} reported changes");
    }
    // A refused update leaves the previous one in place (INV-302) and the gesture open.
    assert_eq!(start_frame(s.doc(), x), 40);
    assert!(s.transaction(drag).is_some());
    s.verify().unwrap();
}

/// Acceptance: a gesture idle for 30 s is cancelled and the document rolled back; activity
/// every 29 s keeps it alive (fake clock).
#[test]
fn activity_every_29_s_keeps_a_gesture_alive_and_30_s_idle_cancels_it() {
    let (doc, tracks, _) = project(1);
    let (mut s, clock) = clocked(doc);
    let user = ui();
    let x = append(&mut s, &user, tracks[0], 30);
    let committed = s.committed_doc().clone();
    let drag = s.begin_tx(&user, TxKind::Gesture, "Move clip").unwrap().id;
    // Ten minutes of a slow drag: an update every 29 s, some of them refused (an overlap
    // never happens here, so a missing item stands in for a refused position).
    for i in 0..20_u32 {
        clock.advance(Duration::from_secs(29));
        assert!(s.expire_idle().is_empty(), "expired after {i} updates");
        if i % 5 == 4 {
            let gone = doc_support::stray_item();
            run_in(
                &mut s,
                &user,
                drag,
                "clip.move",
                json!({"item": gone, "at": 1}),
            )
            .unwrap_err();
        } else {
            update(
                &mut s,
                &user,
                drag,
                "clip.move",
                json!({"item": x, "at": 31 + i}),
            )
            .unwrap();
        }
        assert!(s.transaction(drag).is_some());
    }
    let moved = s.doc().clone();
    assert_ne!(&moved, &committed);
    let _ = s.take_events();

    // 29.999 s without an update: still open; the 30th second cancels it.
    clock.advance(
        GESTURE_IDLE_TIMEOUT
            .checked_sub(Duration::from_millis(1))
            .unwrap(),
    );
    assert_eq!(s.expire_idle(), vec![]);
    assert_eq!(s.doc(), &moved);
    clock.advance(Duration::from_millis(1));
    let closed = s.expire_idle();
    assert_eq!(closed.len(), 1);
    assert_eq!(
        (closed[0].tx_id, closed[0].outcome),
        (drag, TxOutcome::Expired)
    );
    assert_eq!(s.doc(), &committed, "rolled back");
    assert_eq!(s.committed_doc(), &committed);
    assert!(s.locks(None).transactions.is_empty(), "locks released");
    let events = s.take_events();
    let names: Vec<&str> = events.iter().map(DocEvent::name).collect();
    assert_eq!(names, ["doc.patched", "tx.closed", "gesture.cancelled"]);
    assert_eq!(events[0].as_patched().unwrap().cause, PatchCause::Rollback);
    let cancelled = events[2].as_gesture_cancelled().unwrap();
    assert_eq!(
        (
            cancelled.tx_id,
            cancelled.reason,
            &cancelled.actor,
            &cancelled.items[..]
        ),
        (
            drag,
            GestureCancelReason::IdleTimeout,
            &Actor::User,
            &[x][..]
        )
    );
    assert_eq!(
        (cancelled.rev, cancelled.committed_rev),
        (s.rev(), s.committed_rev())
    );
    assert_eq!(
        serde_json::to_value(&events[2]).unwrap()["event"],
        "gesture.cancelled"
    );
    assert_eq!(
        serde_json::to_value(&events[2]).unwrap()["reason"],
        "idle_timeout"
    );
    // Its owner learns on its next update; nothing was committed.
    let err = update(
        &mut s,
        &user,
        drag,
        "clip.move",
        json!({"item": x, "at": 5}),
    )
    .unwrap_err();
    assert_eq!(err.code(), "not_found");
    assert_eq!(history_len(&s), 1);
    s.verify().unwrap();
}

#[test]
fn proposals_never_expire_and_are_cancellable() {
    let (doc, tracks, _) = project(1);
    let (mut s, clock) = clocked(doc);
    let (user, bot) = (ui(), agent());
    let x = append(&mut s, &user, tracks[0], 30);
    let committed = s.committed_doc().clone();
    let proposal = s.begin_tx(&bot, TxKind::Proposal, "Copilot").unwrap().id;
    run_in(
        &mut s,
        &bot,
        proposal,
        "item.rename",
        json!({"item": x, "name": "p"}),
    )
    .unwrap();
    clock.advance(Duration::from_hours(24));
    assert_eq!(s.expire_idle(), vec![]);
    assert_eq!(s.next_expiry(), None);
    let (holder, retry, _) = in_use(
        &run(
            &mut s,
            &user,
            "item.rename",
            json!({"item": x, "name": "u"}),
        )
        .unwrap_err(),
    );
    assert_eq!((holder.as_str(), retry), ("agent:claude", None));
    // `_meta.replace` is for gestures only.
    let err = update(
        &mut s,
        &bot,
        proposal,
        "item.rename",
        json!({"item": x, "name": "q"}),
    )
    .unwrap_err();
    assert!(
        matches!(&err, CmdError::Invalid { path, .. } if path == "/_meta/replace"),
        "{err:?}"
    );
    let _ = s.take_events();
    s.cancel_tx(&bot, proposal).unwrap();
    assert_eq!(s.doc(), &committed);
    // Explicit cancels are announced by `tx.closed` only.
    assert!(
        s.take_events()
            .iter()
            .all(|e| !matches!(e, DocEvent::GestureCancelled(_)))
    );
    run(
        &mut s,
        &user,
        "item.rename",
        json!({"item": x, "name": "u"}),
    )
    .unwrap();
}

/// The doc actor cancels an idle gesture on its own timer: no request needs to arrive.
#[test]
fn the_actor_cancels_an_idle_gesture_on_its_own_timer() {
    let (doc, tracks, _) = project(1);
    let clock = Arc::new(ManualClock::new(WallTime::from_millis(1_000_000)));
    let actor = DocActor::spawn(config(doc).with_clock(Arc::clone(&clock) as _)).unwrap();
    let h = actor.handle();
    let user = ui();
    let x = doc_support::append(&h, &user, tracks[0], 30);
    let committed = h.committed_snapshot().doc.clone();
    let drag = h
        .begin_tx(&user, TxKind::Gesture, "Move clip")
        .wait()
        .unwrap()
        .id;
    h.submit(
        Submit::new(user.clone(), "clip.move", json!({"item": x, "at": 50}))
            .in_tx(drag)
            .replacing(),
    )
    .wait()
    .unwrap();
    let mut events = h.subscribe();
    clock.advance(GESTURE_IDLE_TIMEOUT);
    // Bounded wait: the actor re-checks idle gestures at least once a second.
    let cancelled = loop {
        let event = events
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .expect("the actor did not cancel the idle gesture within 10 s");
        if let Some(g) = event.as_gesture_cancelled() {
            break g.clone();
        }
    };
    assert_eq!((cancelled.tx_id, &cancelled.items[..]), (drag, &[x][..]));
    assert_eq!(h.snapshot().doc, committed);
    assert_eq!(h.locks(&user).wait().unwrap().transactions, vec![]);
    drop(h);
    actor.shutdown().unwrap();
}

/// INV-301/303: replacing updates commit the effect of the last one applied once; what an
/// earlier update created disappears with it.
#[test]
#[allow(clippy::too_many_lines)]
fn replacing_updates_commit_the_last_update_applied_once() {
    let (doc, tracks, _) = project(2);
    let (mut s, _clock) = clocked(doc);
    let user = ui();
    let x = append(&mut s, &user, tracks[0], 30);
    let committed = s.committed_doc().clone();
    let mut mirror = Mirror::new(s.doc(), s.rev());
    let _ = s.take_events();

    let drag = s.begin_tx(&user, TxKind::Gesture, "Drag").unwrap().id;
    // Update 1 adds a clip (an item the next update no longer needs), update 2 moves X.
    let first = update(
        &mut s,
        &user,
        drag,
        "timeline.append_clip",
        json!({"track": tracks[1], "frames": 10}),
    )
    .unwrap();
    let extra: ItemId = serde_json::from_value(first.output["item"].clone()).unwrap();
    assert!(items(s.doc()).contains(&extra));
    let rev = s.rev();
    update(
        &mut s,
        &user,
        drag,
        "clip.move",
        json!({"item": x, "at": 60}),
    )
    .unwrap();
    assert_eq!(s.rev(), rev.next());
    assert!(
        !items(s.doc()).contains(&extra),
        "replaced: the added clip is gone"
    );
    assert_eq!(start_frame(s.doc(), x), 60);
    // The same position again changes nothing (no revision, no event).
    let rev = s.rev();
    for e in s.take_events() {
        mirror.apply(&e);
    }
    update(
        &mut s,
        &user,
        drag,
        "clip.move",
        json!({"item": x, "at": 60}),
    )
    .unwrap();
    assert_eq!(s.rev(), rev);
    assert_eq!(s.take_events(), vec![]);
    // Every later position replaces the last.
    for at in [70, 45, 80] {
        update(
            &mut s,
            &user,
            drag,
            "clip.move",
            json!({"item": x, "at": at}),
        )
        .unwrap();
    }
    for e in s.take_events() {
        mirror.apply(&e);
    }
    mirror.assert_matches(s.doc());
    s.verify().unwrap();

    let closed = s.commit_tx(&user, drag).unwrap();
    // One move from where it began: the net patch only changes X's start.
    assert_eq!(
        closed.patch.touched_items().into_iter().collect::<Vec<_>>(),
        [x]
    );
    assert_eq!(start_frame(s.committed_doc(), x), 80);
    assert_eq!(items(s.committed_doc()), items(&committed));
    assert_eq!(history_len(&s), 2);
    // Undo identity.
    s.submit(Submit::new(user.clone(), "edit.undo", json!({})))
        .unwrap();
    assert_eq!(s.committed_doc(), &committed);

    // A gesture whose last update ends where it began commits nothing.
    let drag = s.begin_tx(&user, TxKind::Gesture, "Drag").unwrap().id;
    update(
        &mut s,
        &user,
        drag,
        "clip.move",
        json!({"item": x, "at": 50}),
    )
    .unwrap();
    let rev = s.rev();
    update(
        &mut s,
        &user,
        drag,
        "clip.move",
        json!({"item": x, "at": start_frame(&committed, x)}),
    )
    .unwrap();
    assert_eq!(s.rev(), rev.next(), "rolled back live");
    assert_eq!(s.doc(), &committed);
    let closed = s.commit_tx(&user, drag).unwrap();
    assert!(closed.patch.is_empty());
    assert_eq!(
        history_len(&s),
        3,
        "no entry for a gesture that changed nothing"
    );
    s.verify().unwrap();
}

/// INV-302 and INV-304: a refused replacing update keeps the previous one; the lock set is
/// the union of every update's items and only grows until the gesture closes.
#[test]
fn a_refused_update_keeps_the_previous_one_and_locks_only_grow() {
    let (doc, tracks, _) = project(2);
    let (mut s, _clock) = clocked(doc);
    let (user, bot) = (ui(), agent());
    let x = append(&mut s, &user, tracks[0], 30);
    let y = append(&mut s, &user, tracks[1], 30);
    let _blocker = append(&mut s, &user, tracks[0], 30); // X ends where it starts

    let drag = s.begin_tx(&user, TxKind::Gesture, "Drag").unwrap().id;
    update(
        &mut s,
        &user,
        drag,
        "item.rename",
        json!({"item": x, "name": "first"}),
    )
    .unwrap();
    // Overlapping the blocker is refused: the rename stays, the gesture stays open.
    let rev = s.rev();
    let err = update(
        &mut s,
        &user,
        drag,
        "clip.trim",
        json!({"item": x, "frames": 45}),
    )
    .unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert_eq!(s.rev(), rev);
    assert_eq!(item_name(s.doc(), x).as_deref(), Some("first"));

    // The next update renames Y instead: X is back as it was, but stays locked.
    update(
        &mut s,
        &user,
        drag,
        "item.rename",
        json!({"item": y, "name": "second"}),
    )
    .unwrap();
    assert_eq!(item_name(s.doc(), x), None);
    let mut both = vec![x, y];
    both.sort();
    assert_eq!(s.locks(None).transactions[0].items, both);
    let (holder, _, _) =
        in_use(&run(&mut s, &bot, "item.rename", json!({"item": x, "name": "b"})).unwrap_err());
    assert_eq!(holder, "user");
    s.verify().unwrap();

    // Commit: only the last update lands; both locks are released.
    s.commit_tx(&user, drag).unwrap();
    assert_eq!(item_name(s.committed_doc(), x), None);
    assert_eq!(item_name(s.committed_doc(), y).as_deref(), Some("second"));
    run(&mut s, &bot, "item.rename", json!({"item": x, "name": "b"})).unwrap();
    s.verify().unwrap();
}

#[test]
fn replace_needs_an_open_gesture() {
    let (doc, tracks, _) = project(1);
    let (mut s, _clock) = clocked(doc);
    let user = ui();
    let x = append(&mut s, &user, tracks[0], 30);
    let before = fingerprint(&s);
    let implicit = s
        .submit(
            Submit::new(user.clone(), "item.rename", json!({"item": x, "name": "n"})).replacing(),
        )
        .unwrap_err();
    assert!(
        matches!(&implicit, CmdError::Invalid { path, .. } if path == "/_meta/replace"),
        "{implicit:?}"
    );
    let batch = s.begin_tx(&user, TxKind::Batch, "Batch").unwrap().id;
    let err = update(
        &mut s,
        &user,
        batch,
        "item.rename",
        json!({"item": x, "name": "n"}),
    )
    .unwrap_err();
    assert!(
        matches!(&err, CmdError::Invalid { path, .. } if path == "/_meta/replace"),
        "{err:?}"
    );
    s.cancel_tx(&user, batch).unwrap();
    assert_eq!(fingerprint(&s), before);
}

/// Replacing updates compose with another actor's live proposal on other items, and the
/// events replay to the live document.
#[test]
fn replacing_updates_compose_with_another_actors_proposal() {
    let (doc, tracks, _) = project(2);
    let (mut s, _clock) = clocked(doc);
    let (user, bot) = (ui(), agent());
    let x = append(&mut s, &user, tracks[0], 30);
    let y = append(&mut s, &user, tracks[1], 30);
    let mut mirror = Mirror::new(s.doc(), s.rev());
    let _ = s.take_events();
    let proposal = s.begin_tx(&bot, TxKind::Proposal, "Copilot").unwrap().id;
    run_in(
        &mut s,
        &bot,
        proposal,
        "item.rename",
        json!({"item": y, "name": "p"}),
    )
    .unwrap();
    let drag = s.begin_tx(&user, TxKind::Gesture, "Drag").unwrap().id;
    for at in [40, 55, 35] {
        update(
            &mut s,
            &user,
            drag,
            "clip.move",
            json!({"item": x, "at": at}),
        )
        .unwrap();
    }
    for e in s.take_events() {
        mirror.apply(&e);
    }
    mirror.assert_matches(s.doc());
    assert_eq!(item_name(s.doc(), y).as_deref(), Some("p"));
    assert_eq!(start_frame(s.doc(), x), 35);
    s.verify().unwrap();
    s.commit_tx(&user, drag).unwrap();
    s.commit_tx(&bot, proposal).unwrap();
    assert_eq!(start_frame(s.committed_doc(), x), 35);
    assert_eq!(item_name(s.committed_doc(), y).as_deref(), Some("p"));
    s.verify().unwrap();
}

/// Issue #231: a replacing update tells subscribers what it changed in the live document, not
/// the gesture's earlier update undone and the new one applied. A drag along a track that
/// ripples shifts the same clips on every update but for the one it passed: the event of such
/// an update names that clip, not every shifted clip twice. Counted in ops, not timed.
#[test]
fn a_replacing_update_broadcasts_the_net_change_of_the_live_document() {
    const CLIPS: u32 = 40;
    let (doc, tracks, _) = project(2);
    let (mut s, _clock) = clocked(doc);
    let user = ui();
    let clips: Vec<ItemId> = (0..CLIPS)
        .map(|_| append(&mut s, &user, tracks[0], 30))
        .collect();
    let mut mirror = Mirror::new(s.doc(), s.rev());
    let _ = s.take_events();
    let drag = s.begin_tx(&user, TxKind::Gesture, "Ripple").unwrap().id;
    // The ops of the one `doc.patched` an update causes, after the mirror followed it.
    let mut ripple = |s: &mut DocState, from_clip: u32| -> (usize, usize) {
        let step = update(
            s,
            &user,
            drag,
            "timeline.shift_after",
            json!({"track": tracks[0], "at": from_clip * 30, "frames": 10}),
        )
        .unwrap();
        let events = s.take_events();
        let mut sent = 0;
        for e in &events {
            if let DocEvent::Patched(p) = e {
                assert_eq!(p.cause, PatchCause::Live);
                sent += p.ops.len();
            }
            mirror.apply(e);
        }
        mirror.assert_matches(s.doc());
        (step.ops.len(), sent)
    };
    // Everything from the first clip on moves: nothing was live before, so that is the change.
    assert_eq!(ripple(&mut s, 0), (40, 40));
    // From clip 20 on: the update shifts 20 clips, which are where they were already; the
    // first 20 go back. 20 ops, not the 40 undone and the 20 applied.
    assert_eq!(ripple(&mut s, 20), (20, 20));
    // One clip further: 19 clips shifted, one changed.
    assert_eq!(ripple(&mut s, 21), (19, 1));
    assert_eq!(start_frame(s.doc(), clips[20]), 20 * 30);
    assert_eq!(start_frame(s.doc(), clips[21]), 21 * 30 + 10);
    // And back.
    assert_eq!(ripple(&mut s, 20), (20, 1));
    s.verify().unwrap();
    // What the gesture commits is its last update applied once, as before.
    let closed = s.commit_tx(&user, drag).unwrap();
    assert_eq!(closed.patch.len(), 20);
    assert_eq!(closed.patch.touched_items().len(), 20);
    s.verify().unwrap();
}

/// Ripple-shifted items are in the touched set (spec INV-304): the gesture's ripple locks
/// them, and another actor's ripple that would shift a locked item is refused while one that
/// shifts only other items proceeds.
#[test]
fn ripple_shifted_items_are_locked() {
    let (doc, tracks, _) = project(2);
    let (mut s, _clock) = clocked(doc);
    let (user, bot) = (ui(), agent());
    let first = append(&mut s, &user, tracks[0], 30); // 0-30
    let second = append(&mut s, &user, tracks[0], 30); // 30-60
    let third = append(&mut s, &user, tracks[0], 30); // 60-90
    let other = append(&mut s, &user, tracks[1], 30); // other track

    // The person's ripple from frame 30 shifts B and C (A stays).
    let drag = s.begin_tx(&user, TxKind::Gesture, "Ripple").unwrap().id;
    update(
        &mut s,
        &user,
        drag,
        "timeline.shift_after",
        json!({"track": tracks[0], "at": 30, "frames": 10}),
    )
    .unwrap();
    let mut shifted = vec![second, third];
    shifted.sort();
    assert_eq!(s.locks(None).transactions[0].items, shifted);
    for item in [second, third] {
        in_use(
            &run(
                &mut s,
                &bot,
                "item.rename",
                json!({"item": item, "name": "x"}),
            )
            .unwrap_err(),
        );
    }
    run(
        &mut s,
        &bot,
        "item.rename",
        json!({"item": first, "name": "x"}),
    )
    .unwrap();
    // The agent's ripple of track 1 shifts only D; one of track 0 from 0 would shift B and C.
    run(
        &mut s,
        &bot,
        "timeline.shift_after",
        json!({"track": tracks[1], "at": 0, "frames": 5}),
    )
    .unwrap();
    let err = run(
        &mut s,
        &bot,
        "timeline.shift_after",
        json!({"track": tracks[0], "at": 0, "frames": 5}),
    )
    .unwrap_err();
    assert_eq!(in_use(&err).0, "user");
    assert_eq!(start_frame(s.committed_doc(), other), 5);
    s.verify().unwrap();
}
