// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.04 scenario tests: transactions through `edit.*`, undo/redo, per-actor history and its
//! safety rule, the gesture idle timeout, history paging and limits, events and the journal.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names
)]

mod doc_support;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use cutweave_commands::{
    Actor, ActorCtx, Applied, CmdError, CommitRecord, ConflictReason, DocActor, DocConfig,
    DocState, EntryCause, HistoryChange, HistoryLimits, HistoryPage, JournaledCommit, ManualClock,
    PatchCause, Rev, Submit, TxId, TxKind, TxOutcome, WallTime,
};
use cutweave_model::{ItemId, Project, TrackId};
use doc_support::{Mirror, agent, config, drain, item_name, items, project, ui};
use serde_json::{Value, json};

fn state(doc: Project) -> DocState {
    DocState::new(config(doc))
}

fn run(s: &mut DocState, ctx: &ActorCtx, name: &str, params: Value) -> Result<Applied, CmdError> {
    s.submit(Submit::new(ctx.clone(), name, params))
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

fn rename(s: &mut DocState, ctx: &ActorCtx, item: ItemId, name: &str) -> Applied {
    run(s, ctx, "item.rename", json!({"item": item, "name": name})).unwrap()
}

fn scope(actor: Option<&str>) -> Value {
    actor.map_or_else(|| json!({}), |a| json!({"actor": a}))
}

fn undo(s: &mut DocState, ctx: &ActorCtx, actor: Option<&str>) -> Result<Applied, CmdError> {
    run(s, ctx, "edit.undo", scope(actor))
}

fn redo(s: &mut DocState, ctx: &ActorCtx, actor: Option<&str>) -> Result<Applied, CmdError> {
    run(s, ctx, "edit.redo", scope(actor))
}

fn history(s: &mut DocState) -> HistoryPage {
    let out = run(s, &ui(), "edit.history", json!({"limit": 500}))
        .unwrap()
        .output;
    serde_json::from_value(out).unwrap()
}

fn tx_of(applied: &Applied) -> TxId {
    serde_json::from_value(applied.output["tx"].clone()).unwrap()
}

fn reason(e: &CmdError) -> Option<ConflictReason> {
    match e {
        CmdError::Conflict { reason, .. } => Some(*reason),
        _ => None,
    }
}

fn detail(e: &CmdError) -> String {
    match e {
        CmdError::Conflict { detail, .. } => detail.clone(),
        other => panic!("expected a conflict, got {other:?}"),
    }
}

/// Everything a refused request must leave alone.
fn observable(s: &DocState) -> (Rev, Rev, Project, Project) {
    (
        s.rev(),
        s.committed_rev(),
        s.doc().clone(),
        s.committed_doc().clone(),
    )
}

/// A tiny deterministic generator (no extra dependency).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % n as u64) as usize
    }
}

fn random_command(rng: &mut Lcg, doc: &Project, tracks: &[TrackId]) -> (&'static str, Value) {
    let all = items(doc);
    let pick = |rng: &mut Lcg| all[rng.next(all.len())];
    match (rng.next(5), all.is_empty()) {
        (0, _) | (_, true) => (
            "timeline.append_clip",
            json!({"track": tracks[rng.next(tracks.len())], "frames": 1 + rng.next(30)}),
        ),
        (1, _) => (
            "item.rename",
            json!({"item": pick(rng), "name": format!("n{}", rng.next(1000))}),
        ),
        (2, _) => (
            "item.set_enabled",
            json!({"item": pick(rng), "enabled": rng.next(2) == 0}),
        ),
        (3, _) => ("item.delete", json!({"item": pick(rng)})),
        _ => (
            "project.rename",
            json!({"name": format!("p{}", rng.next(1000))}),
        ),
    }
}

// ----- acceptance 1: 50 random commands undo to the initial document, redo to the final ------

#[test]
fn fifty_random_commands_undo_to_the_initial_document_and_redo_to_the_final_one() {
    let (initial, tracks, _) = project(3);
    let mut s = state(initial.clone());
    let mut rng = Lcg(0x5EED);
    // The committed document after each history entry.
    let mut docs = vec![initial.clone()];
    while docs.len() <= 50 {
        let (name, params) = random_command(&mut rng, s.committed_doc(), &tracks);
        let applied = run(&mut s, &ui(), name, params).unwrap();
        if !applied.ops.is_empty() {
            docs.push(s.committed_doc().clone());
        }
    }
    let last = docs.len() - 1;
    assert_eq!(last, 50);
    assert_eq!(history(&mut s).stacks[0].undo_depth, 50);

    for i in (0..last).rev() {
        undo(&mut s, &ui(), None).unwrap();
        assert_eq!(s.committed_doc(), &docs[i], "undo down to entry {i}");
        s.verify().unwrap();
    }
    assert_eq!(s.committed_doc(), &initial);
    assert_eq!(undo(&mut s, &ui(), None).unwrap_err().code(), "invalid");

    for (i, doc) in docs.iter().enumerate().skip(1) {
        redo(&mut s, &ui(), None).unwrap();
        assert_eq!(s.committed_doc(), doc, "redo up to entry {i}");
    }
    assert_eq!(s.committed_doc(), &docs[last]);
    assert_eq!(redo(&mut s, &ui(), None).unwrap_err().code(), "invalid");
    s.verify().unwrap();
}

// ----- acceptance 2: gestures ------------------------------------------------------------------

#[test]
fn a_gesture_of_200_updates_is_one_history_entry_and_undoes_as_one() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let item = append(&mut s, &ui(), tracks[0], 30);
    let pre = s.committed_doc().clone();
    let entries_before = history(&mut s).entries.len();
    let _ = s.take_events();

    let opened = run(
        &mut s,
        &ui(),
        "edit.begin_tx",
        json!({"kind": "gesture", "label": "Drag"}),
    )
    .unwrap();
    let tx = tx_of(&opened);
    assert_eq!(opened.output["kind"], json!("gesture"));
    for i in 0..200 {
        s.submit(
            Submit::new(
                ui(),
                "item.rename",
                json!({"item": item, "name": format!("n{i}")}),
            )
            .in_tx(tx),
        )
        .unwrap();
    }
    // Every update was broadcast live; the committed document did not move.
    let live: Vec<_> = s
        .take_events()
        .into_iter()
        .filter_map(|e| e.as_patched().cloned())
        .collect();
    assert_eq!(live.len(), 200);
    assert!(
        live.iter()
            .all(|p| p.cause == PatchCause::Live && p.tx_id == tx)
    );
    assert_eq!(s.committed_doc(), &pre);

    let committed = run(&mut s, &ui(), "edit.commit_tx", json!({})).unwrap();
    assert_eq!(committed.output["tx"], json!(tx));
    assert_eq!(committed.output["changed"], json!(true));
    let page = history(&mut s);
    assert_eq!(page.entries.len(), entries_before + 1, "exactly one entry");
    let entry = &page.entries[0];
    assert_eq!(
        (entry.tx_id, entry.kind, entry.label.as_str(), entry.cause),
        (tx, Some(TxKind::Gesture), "Drag", EntryCause::Edit)
    );
    assert!(entry.can_undo);
    let post = s.committed_doc().clone();
    assert_eq!(item_name(&post, item).as_deref(), Some("n199"));

    // One undo reverts all 200 updates; one redo brings them back.
    let undone = undo(&mut s, &ui(), None).unwrap();
    assert_eq!(undone.output["entry"], json!(tx));
    assert_eq!(s.committed_doc(), &pre);
    redo(&mut s, &ui(), None).unwrap();
    assert_eq!(s.committed_doc(), &post);
}

#[test]
fn cancelling_a_gesture_restores_the_pre_tx_document_and_records_nothing() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let item = append(&mut s, &ui(), tracks[0], 30);
    let pre = s.committed_doc().clone();
    let entries = history(&mut s).entries.len();
    let tx = tx_of(
        &run(
            &mut s,
            &ui(),
            "edit.begin_tx",
            json!({"kind": "gesture", "label": "Drag"}),
        )
        .unwrap(),
    );
    for i in 0..10 {
        s.submit(
            Submit::new(
                ui(),
                "item.rename",
                json!({"item": item, "name": format!("{i}")}),
            )
            .in_tx(tx),
        )
        .unwrap();
    }
    assert_ne!(s.doc(), &pre);
    let _ = s.take_events();
    let cancelled = run(&mut s, &ui(), "edit.cancel_tx", json!({"tx": tx})).unwrap();
    assert_eq!(cancelled.output["kind"], json!("gesture"));
    assert_eq!(s.doc(), &pre);
    assert_eq!(s.committed_doc(), &pre);
    assert_eq!(history(&mut s).entries.len(), entries);
    let events = s.take_events();
    assert!(events.iter().any(|e| {
        e.as_patched()
            .is_some_and(|p| p.cause == PatchCause::Rollback)
    }));
    // No transaction left to commit or cancel.
    let err = run(&mut s, &ui(), "edit.commit_tx", json!({})).unwrap_err();
    assert_eq!(
        err,
        CmdError::invalid("/tx", "this connection has no open transaction")
    );
}

fn clocked(doc: Project) -> (DocState, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(WallTime::from_millis(1_000_000)));
    let s = DocState::new(config(doc).with_clock(Arc::clone(&clock) as _));
    (s, clock)
}

#[test]
fn a_gesture_idle_for_30_seconds_is_cancelled_and_the_document_restored() {
    let (doc, tracks, _) = project(1);
    let (mut s, clock) = clocked(doc);
    let item = append(&mut s, &ui(), tracks[0], 30);
    let pre = s.committed_doc().clone();
    let tx = s.begin_tx(&ui(), TxKind::Gesture, "Drag").unwrap().id;
    let proposal = s
        .begin_tx(&agent(), TxKind::Proposal, "Copilot turn")
        .unwrap()
        .id;
    s.submit(Submit::new(ui(), "item.rename", json!({"item": item, "name": "x"})).in_tx(tx))
        .unwrap();
    // Activity resets the timer.
    clock.advance(Duration::from_secs(20));
    s.submit(Submit::new(ui(), "item.rename", json!({"item": item, "name": "y"})).in_tx(tx))
        .unwrap();
    clock.advance(Duration::from_millis(29_999));
    assert_eq!(s.expire_idle().len(), 0);
    assert!(s.transaction(tx).is_some());
    assert_eq!(item_name(s.doc(), item).as_deref(), Some("y"));
    let _ = s.take_events();

    clock.advance(Duration::from_millis(1));
    let expired = s.expire_idle();
    assert_eq!(expired.len(), 1);
    assert_eq!(
        (expired[0].tx_id, expired[0].outcome),
        (tx, TxOutcome::Expired)
    );
    assert!(s.transaction(tx).is_none());
    assert_eq!(s.doc(), &pre, "live document restored");
    assert_eq!(s.committed_doc(), &pre);
    let events = s.take_events();
    assert!(events.iter().any(|e| {
        e.as_patched()
            .is_some_and(|p| p.cause == PatchCause::Rollback && p.tx_id == tx)
    }));
    // Its owner learns on the next update; nothing was recorded.
    let err = s
        .submit(Submit::new(ui(), "item.rename", json!({"item": item, "name": "z"})).in_tx(tx))
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
    assert_eq!(history(&mut s).entries.len(), 1);
    // Proposals never expire.
    clock.advance(Duration::from_secs(3600));
    assert_eq!(s.expire_idle().len(), 0);
    assert!(s.transaction(proposal).is_some());
}

#[test]
fn every_request_expires_idle_gestures_first() {
    let (doc, tracks, _) = project(1);
    let (mut s, clock) = clocked(doc);
    let item = append(&mut s, &ui(), tracks[0], 30);
    let tx = s.begin_tx(&ui(), TxKind::Gesture, "Drag").unwrap().id;
    s.submit(Submit::new(ui(), "item.rename", json!({"item": item, "name": "x"})).in_tx(tx))
        .unwrap();
    clock.advance(Duration::from_secs(30));
    // The agent's edit of the same item no longer collides with the abandoned gesture.
    rename(&mut s, &agent(), item, "agent");
    assert!(s.transaction(tx).is_none());
    assert_eq!(item_name(s.committed_doc(), item).as_deref(), Some("agent"));
}

#[test]
fn the_actor_cancels_an_idle_gesture_on_its_own_timer() {
    let (doc, tracks, _) = project(1);
    let config = config(doc)
        .with_clock(Arc::new(cutweave_commands::SystemClock))
        .with_gesture_timeout(Duration::from_millis(100));
    let actor = DocActor::spawn(config).unwrap();
    let h = actor.handle();
    let item = doc_support::append(&h, &ui(), tracks[0], 30);
    let pre = h.committed_snapshot();
    let mut events = h.subscribe();
    let tx = h
        .begin_tx(&ui(), TxKind::Gesture, "Drag")
        .wait()
        .unwrap()
        .id;
    h.submit(Submit::new(ui(), "item.rename", json!({"item": item, "name": "x"})).in_tx(tx))
        .wait()
        .unwrap();
    // No further request: the actor's timer rolls the gesture back.
    loop {
        let event = events
            .recv_timeout(Duration::from_secs(20))
            .unwrap()
            .expect("the gesture was not expired");
        if event
            .as_patched()
            .is_some_and(|p| p.cause == PatchCause::Rollback && p.tx_id == tx)
        {
            break;
        }
    }
    assert_eq!(h.snapshot().doc, pre.doc);
    assert!(h.run(move |s| s.transaction(tx)).wait().unwrap().is_none());
}

// ----- acceptance 3: two actors interleave edits ----------------------------------------------

#[test]
fn scenario_1_each_actor_undoes_only_its_own_changes() {
    let (initial, tracks, _) = project(2);
    let mut s = state(initial.clone());
    let (u, a) = (ui(), agent());
    let x = append(&mut s, &u, tracks[0], 10);
    let y = append(&mut s, &a, tracks[1], 10);
    rename(&mut s, &u, x, "user's");
    rename(&mut s, &a, y, "agent's");

    undo(&mut s, &u, None).unwrap();
    assert_eq!(item_name(s.committed_doc(), x), None);
    assert_eq!(item_name(s.committed_doc(), y).as_deref(), Some("agent's"));

    let undone = undo(&mut s, &a, None).unwrap();
    assert_eq!(undone.output["actor"], json!("agent:claude"));
    assert_eq!(item_name(s.committed_doc(), y), None);
    assert_eq!(items(s.committed_doc()).len(), 2);

    undo(&mut s, &a, None).unwrap();
    assert_eq!(items(s.committed_doc()), vec![x]);
    undo(&mut s, &u, None).unwrap();
    assert_eq!(s.committed_doc(), &initial);
    for ctx in [&u, &a] {
        let err = undo(&mut s, ctx, None).unwrap_err();
        assert_eq!(err.code(), "invalid");
        assert!(err.message().contains("nothing to undo"), "{err}");
    }
    s.verify().unwrap();
}

#[test]
fn scenario_2_undo_conflicts_when_another_actor_changed_the_same_field_since() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let (u, a) = (ui(), agent());
    let x = append(&mut s, &u, tracks[0], 10);
    rename(&mut s, &a, x, "agent's");
    let users = rename(&mut s, &u, x, "user's");

    let before = observable(&s);
    let err = undo(&mut s, &a, None).unwrap_err();
    assert_eq!(reason(&err), Some(ConflictReason::HistoryTargetModified));
    let detail = detail(&err);
    assert!(detail.contains("item.rename"), "{detail}");
    assert!(
        detail.contains(&format!("user in {}", users.tx_id.unwrap())),
        "{detail}"
    );
    assert!(detail.contains("/name"), "{detail}");
    assert!(matches!(err, CmdError::Conflict { current_rev: Some(rev), .. } if rev == before.0));
    assert_eq!(observable(&s), before, "a refused undo changes nothing");

    // Once the user's change is undone, the agent's undo applies.
    undo(&mut s, &u, None).unwrap();
    assert_eq!(item_name(s.committed_doc(), x).as_deref(), Some("agent's"));
    undo(&mut s, &a, None).unwrap();
    assert_eq!(item_name(s.committed_doc(), x), None);

    // Removing what another actor edited since is refused the same way.
    let z = append(&mut s, &a, tracks[0], 5);
    rename(&mut s, &u, z, "mine now");
    let err = undo(&mut s, &a, None).unwrap_err();
    assert_eq!(reason(&err), Some(ConflictReason::HistoryTargetModified));
    assert!(items(s.committed_doc()).contains(&z));
}

#[test]
fn scenario_3_undo_conflicts_when_the_result_would_break_an_invariant() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let (u, a) = (ui(), agent());
    let x = append(&mut s, &a, tracks[0], 30);
    run(&mut s, &a, "item.delete", json!({"item": x})).unwrap();
    // The user fills the gap the agent's delete left.
    let z = append(&mut s, &u, tracks[0], 30);
    // Undoing the delete re-adds X: its paths are untouched, but X would overlap Z.
    let before = observable(&s);
    let err = undo(&mut s, &a, None).unwrap_err();
    assert_eq!(reason(&err), Some(ConflictReason::HistoryTargetModified));
    assert!(detail(&err).contains("overlap"), "{}", detail(&err));
    assert_eq!(observable(&s), before);
    // After the user undoes Z, the agent's undo is safe again.
    undo(&mut s, &u, None).unwrap();
    undo(&mut s, &a, None).unwrap();
    assert_eq!(items(s.committed_doc()), vec![x]);
    assert!(!items(s.committed_doc()).contains(&z));
    s.committed_doc().validate().unwrap();
}

#[test]
fn the_user_undoes_and_redoes_the_agents_turn_with_events() {
    let (doc, tracks, _) = project(2);
    let actor = DocActor::spawn(config(doc)).unwrap();
    let h = actor.handle();
    let x = doc_support::append(&h, &ui(), tracks[0], 10);
    // The agent's turn: a proposal with two steps, accepted as one entry.
    let turn = h
        .begin_tx(&agent(), TxKind::Proposal, "Tighten")
        .wait()
        .unwrap()
        .id;
    for step in [
        json!({"item": x, "name": "tight"}),
        json!({"item": x, "name": "tighter"}),
    ] {
        h.submit(Submit::new(agent(), "item.rename", step).in_tx(turn))
            .wait()
            .unwrap();
    }
    h.commit_tx(&agent(), turn).wait().unwrap();
    // The user keeps editing elsewhere.
    let y = doc_support::append(&h, &ui(), tracks[1], 10);
    let mut events = h.subscribe();
    let mut mirror = Mirror::new(&h.snapshot().doc, h.snapshot().rev);

    // "Undo agent turn".
    let undone =
        doc_support::submit(&h, &ui(), "edit.undo", json!({"actor": "agent:claude"})).unwrap();
    assert_eq!(undone.output["entry"], json!(turn));
    assert_eq!(undone.output["label"], json!("Tighten"));
    assert_eq!(undone.tx_id, Some(tx_of(&undone)));
    let snapshot = h.snapshot();
    assert_eq!(item_name(&snapshot.doc, x), None);
    assert!(
        items(&snapshot.doc).contains(&y),
        "the user's later edit is kept"
    );

    let got = drain(&mut events);
    let patched: Vec<_> = got.iter().filter_map(|e| e.as_patched()).collect();
    assert_eq!(patched.len(), 1);
    assert_eq!(
        (&patched[0].actor, patched[0].cause, patched[0].request_id),
        (&Actor::User, PatchCause::Undo, Some(undone.request_id))
    );
    let changed: Vec<_> = got.iter().filter_map(|e| e.as_history()).collect();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].change, HistoryChange::Undone);
    assert_eq!(changed[0].tx_id, Some(turn));
    assert_eq!(changed[0].stacks.actor, Actor::agent("claude").unwrap());
    assert_eq!(changed[0].stacks.redo_depth, 1);
    assert_eq!(changed[0].stacks.next_redo.as_ref().unwrap().tx_id, turn);
    for e in &got {
        mirror.apply(e);
    }
    mirror.assert_matches(&snapshot.doc);

    // The agent's own stacks: nothing left to undo, the turn to redo.
    let err = doc_support::submit(&h, &agent(), "edit.undo", json!({})).unwrap_err();
    assert_eq!(err.code(), "invalid");
    let redone =
        doc_support::submit(&h, &ui(), "edit.redo", json!({"actor": "agent:claude"})).unwrap();
    assert_eq!(redone.output["entry"], json!(turn));
    assert_eq!(item_name(&h.snapshot().doc, x).as_deref(), Some("tighter"));
    for e in &drain(&mut events) {
        assert!(e.as_patched().is_none_or(|p| p.cause == PatchCause::Redo));
        mirror.apply(e);
    }
    mirror.assert_matches(&h.snapshot().doc);
}

#[test]
fn the_any_scope_undoes_the_latest_change_by_anyone() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let (u, a) = (ui(), agent());
    let x = append(&mut s, &u, tracks[0], 10);
    rename(&mut s, &a, x, "agent's");
    let undone = undo(&mut s, &u, Some("*")).unwrap();
    assert_eq!(undone.output["actor"], json!("agent:claude"));
    assert_eq!(item_name(s.committed_doc(), x), None);
    let undone = undo(&mut s, &u, Some("*")).unwrap();
    assert_eq!(undone.output["actor"], json!("user"));
    assert_eq!(items(s.committed_doc()).len(), 0);
    // Redo `*` re-applies the most recently undone first.
    let redone = redo(&mut s, &a, Some("*")).unwrap();
    assert_eq!(redone.output["actor"], json!("user"));
    let redone = redo(&mut s, &a, Some("*")).unwrap();
    assert_eq!(redone.output["actor"], json!("agent:claude"));
    assert_eq!(item_name(s.committed_doc(), x).as_deref(), Some("agent's"));
    let err = run(&mut s, &u, "edit.undo", json!({"actor": "nobody"})).unwrap_err();
    assert_eq!(err.code(), "invalid");
}

#[test]
fn undo_any_right_after_redo_any_reverts_that_redo() {
    // Found by the WP-1B.07 property harness: `*` picked the stack top committed last, not the
    // one applied last, so a redone older entry was skipped for a newer one of another actor.
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let (u, a) = (ui(), agent());
    let x = append(&mut s, &u, tracks[0], 10);
    let y = append(&mut s, &a, tracks[0], 10);
    assert_eq!(
        undo(&mut s, &u, Some("*")).unwrap().output["actor"],
        json!("agent:claude")
    );
    rename(&mut s, &u, x, "after"); // newer than the agent's entry
    let before_redo = s.committed_doc().clone();
    let redone = redo(&mut s, &u, Some("*")).unwrap();
    assert_eq!(redone.output["actor"], json!("agent:claude"));
    assert!(items(s.committed_doc()).contains(&y));
    let undone = undo(&mut s, &u, Some("*")).unwrap();
    assert_eq!(undone.output["entry"], redone.output["entry"]);
    assert_eq!(s.committed_doc(), &before_redo);
    assert_eq!(item_name(s.committed_doc(), x).as_deref(), Some("after"));
    s.verify().unwrap();
}

// ----- redo --------------------------------------------------------------------------------

#[test]
fn a_new_change_clears_only_that_actors_redo_stack() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let (u, a) = (ui(), agent());
    let x = append(&mut s, &u, tracks[0], 10);
    rename(&mut s, &u, x, "one");
    undo(&mut s, &u, None).unwrap();
    // Another actor's change leaves the user's redo stack alone ...
    run(&mut s, &a, "project.rename", json!({"name": "agent"})).unwrap();
    assert_eq!(
        history(&mut s)
            .stacks
            .iter()
            .find(|st| st.actor == Actor::User)
            .unwrap()
            .redo_depth,
        1
    );
    redo(&mut s, &u, None).unwrap();
    assert_eq!(item_name(s.committed_doc(), x).as_deref(), Some("one"));
    // ... the user's own new change clears it.
    undo(&mut s, &u, None).unwrap();
    rename(&mut s, &u, x, "two");
    let err = redo(&mut s, &u, None).unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(err.message().contains("nothing to redo"), "{err}");
    let page = history(&mut s);
    assert!(page.entries.iter().all(|e| !e.can_redo));
}

#[test]
fn redo_conflicts_when_the_same_field_changed_after_the_undo() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let (u, a) = (ui(), agent());
    let x = append(&mut s, &u, tracks[0], 10);
    rename(&mut s, &u, x, "one");
    undo(&mut s, &u, None).unwrap();
    let agents = rename(&mut s, &a, x, "agent's");
    let before = observable(&s);
    let err = redo(&mut s, &u, None).unwrap_err();
    assert_eq!(reason(&err), Some(ConflictReason::HistoryTargetModified));
    let detail = detail(&err);
    assert!(
        detail.contains(&format!("agent:claude in {}", agents.tx_id.unwrap())),
        "{detail}"
    );
    assert_eq!(observable(&s), before);
}

// ----- open transactions, soft locks, non-undoable changes ----------------------------------

#[test]
fn undo_respects_another_connections_open_gesture() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let (u, a) = (ui(), agent());
    let x = append(&mut s, &u, tracks[0], 10);
    let tx = s.begin_tx(&a, TxKind::Gesture, "Drag").unwrap().id;
    s.submit(
        Submit::new(
            a.clone(),
            "item.rename",
            json!({"item": x, "name": "dragging"}),
        )
        .in_tx(tx),
    )
    .unwrap();
    let before = observable(&s);
    let err = undo(&mut s, &u, None).unwrap_err();
    assert_eq!(reason(&err), Some(ConflictReason::ItemInGesture));
    assert_eq!(observable(&s), before);
    s.cancel_tx(&a, tx).unwrap();
    undo(&mut s, &u, None).unwrap();
    assert_eq!(items(s.committed_doc()).len(), 0);
    s.verify().unwrap();
}

#[test]
fn a_batch_commits_as_one_entry_and_engine_commands_refuse_to_run_inside_it() {
    let (initial, tracks, _) = project(1);
    let mut s = state(initial.clone());
    let tx = tx_of(
        &run(
            &mut s,
            &ui(),
            "edit.begin_tx",
            json!({"label": "Rough cut"}),
        )
        .unwrap(),
    );
    for frames in [10, 20, 30] {
        s.submit(
            Submit::new(
                ui(),
                "timeline.append_clip",
                json!({"track": tracks[0], "frames": frames}),
            )
            .in_tx(tx),
        )
        .unwrap();
    }
    let err = s
        .submit(Submit::new(ui(), "edit.undo", json!({})).in_tx(tx))
        .unwrap_err();
    assert_eq!(err.code(), "unsupported");
    run(&mut s, &ui(), "edit.commit_tx", json!({"tx": tx})).unwrap();
    let page = history(&mut s);
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].kind, Some(TxKind::Batch));
    undo(&mut s, &ui(), None).unwrap();
    assert_eq!(s.committed_doc(), &initial);
    // Labels are checked.
    let err = run(&mut s, &ui(), "edit.begin_tx", json!({"label": ""})).unwrap_err();
    assert_eq!(err.code(), "invalid");
}

#[test]
fn non_undoable_changes_are_logged_but_never_undone() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let x = append(&mut s, &ui(), tracks[0], 10);
    run(&mut s, &ui(), "project.stamp", json!({"name": "stamped"})).unwrap();
    let page = history(&mut s);
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].label, "project.stamp");
    assert!(!page.entries[0].can_undo);
    // Undo skips it: the append is undone, the stamp stays.
    undo(&mut s, &ui(), None).unwrap();
    assert!(!items(s.committed_doc()).contains(&x));
    assert_eq!(s.committed_doc().meta.name, "stamped");
    // A transaction with a non-undoable step is not undoable as a whole.
    let tx = s.begin_tx(&ui(), TxKind::Batch, "mixed").unwrap().id;
    s.submit(Submit::new(ui(), "project.rename", json!({"name": "renamed"})).in_tx(tx))
        .unwrap();
    s.submit(Submit::new(ui(), "project.stamp", json!({"name": "stamped again"})).in_tx(tx))
        .unwrap();
    s.commit_tx(&ui(), tx).unwrap();
    assert!(!history(&mut s).entries[0].can_undo);
    assert_eq!(undo(&mut s, &ui(), None).unwrap_err().code(), "invalid");
}

// ----- edit.history, limits and memory -------------------------------------------------------

#[test]
fn history_pages_newest_first_with_a_cursor_and_filters_by_actor() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let x = append(&mut s, &ui(), tracks[0], 10);
    for i in 0..4 {
        rename(&mut s, &agent(), x, &format!("{i}"));
    }
    undo(&mut s, &agent(), None).unwrap();
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut params = json!({"limit": 2});
        if let Some(c) = &cursor {
            params["cursor"] = json!(c);
        }
        let page: HistoryPage =
            serde_json::from_value(run(&mut s, &ui(), "edit.history", params).unwrap().output)
                .unwrap();
        assert!(page.entries.len() <= 2);
        seen.extend(page.entries);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 6);
    assert!(seen.windows(2).all(|w| w[0].rev > w[1].rev), "newest first");
    assert!(matches!(seen[0].cause, EntryCause::Undo { .. }));
    assert_eq!(seen[0].label, "Undo item.rename");
    assert_eq!(seen[1].status, cutweave_commands::EntryStatus::Undone);
    assert!(seen[1].can_redo);

    let agents: HistoryPage = serde_json::from_value(
        run(
            &mut s,
            &ui(),
            "edit.history",
            json!({"actor": "agent:claude"}),
        )
        .unwrap()
        .output,
    )
    .unwrap();
    assert_eq!(agents.entries.len(), 5);
    assert_eq!(agents.stacks.len(), 2);
    assert!(agents.usage.bytes > 0);

    for bad in [json!({"cursor": "nope"}), json!({"limit": 501})] {
        assert_eq!(
            run(&mut s, &ui(), "edit.history", bad).unwrap_err().code(),
            "invalid"
        );
    }
    // Read-only: not remembered for dedup, never changes the document.
    let meta = &s.registry().get("edit.history").unwrap().meta;
    assert!(meta.annotations.read_only && !meta.is_journaled());
}

#[test]
fn undo_depth_and_memory_are_bounded() {
    let (doc, tracks, _) = project(1);
    let limits = HistoryLimits {
        max_entries: 1_000,
        max_bytes: usize::MAX,
        max_undo_depth: 3,
    };
    let mut s = DocState::new(config(doc).with_history_limits(limits));
    let x = append(&mut s, &ui(), tracks[0], 10);
    for i in 0..5 {
        rename(&mut s, &ui(), x, &format!("{i}"));
    }
    for _ in 0..3 {
        undo(&mut s, &ui(), None).unwrap();
    }
    assert_eq!(item_name(s.committed_doc(), x).as_deref(), Some("1"));
    assert_eq!(undo(&mut s, &ui(), None).unwrap_err().code(), "invalid");

    // A byte budget evicts entries no stack needs first, and keeps the accounting exact.
    let (doc, tracks, _) = project(1);
    let mut s = DocState::new(config(doc).with_history_limits(HistoryLimits {
        max_entries: 1_000,
        max_bytes: 8 * 1024,
        max_undo_depth: 2,
    }));
    let x = append(&mut s, &agent(), tracks[0], 10);
    let mine = append(&mut s, &ui(), tracks[0], 10);
    for i in 0..200 {
        rename(&mut s, &agent(), x, &format!("name number {i}"));
    }
    let page = history(&mut s);
    assert!(page.usage.bytes <= 8 * 1024, "{:?}", page.usage);
    assert!(page.usage.evicted > 0);
    s.verify().unwrap();
    // The user's single entry survived the agent's churn.
    let user = page
        .stacks
        .iter()
        .find(|st| st.actor == Actor::User)
        .unwrap();
    assert_eq!(user.undo_depth, 1);
    undo(&mut s, &ui(), None).unwrap();
    assert!(!items(s.committed_doc()).contains(&mine));
}

// ----- checkpoints ---------------------------------------------------------------------------

#[test]
fn checkpoint_restore_round_trips_and_the_restore_is_undoable() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let x = append(&mut s, &ui(), tracks[0], 10);
    let cp = run(
        &mut s,
        &ui(),
        "edit.checkpoint",
        json!({"name": "before captions", "note": "safe"}),
    )
    .unwrap()
    .output;
    assert_eq!(cp["checkpoint"], json!("cp_1"));
    assert_eq!(cp["rev"], json!(1));
    let at_checkpoint = s.committed_doc().clone();
    rename(&mut s, &ui(), x, "captioned");
    append(&mut s, &ui(), tracks[0], 5);
    let before_restore = s.committed_doc().clone();

    let restored = run(
        &mut s,
        &ui(),
        "edit.restore",
        json!({"checkpoint": "before captions"}),
    )
    .unwrap();
    assert_eq!(s.committed_doc(), &at_checkpoint);
    assert_eq!(
        restored.output["reverted"],
        json!([{"actor": "user", "txs": ["tx_2", "tx_3"]}])
    );
    let page = history(&mut s);
    assert_eq!(
        page.entries[0].label,
        "Restore checkpoint \"before captions\""
    );
    assert!(matches!(page.entries[0].cause, EntryCause::Restore { .. }));
    // Undo the restore; redo it.
    undo(&mut s, &ui(), None).unwrap();
    assert_eq!(s.committed_doc(), &before_restore);
    redo(&mut s, &ui(), None).unwrap();
    assert_eq!(s.committed_doc(), &at_checkpoint);
    // Already there: nothing to do.
    let again = run(&mut s, &ui(), "edit.restore", json!({"checkpoint": "cp_1"})).unwrap();
    assert_eq!(again.output["tx"], json!(null));

    let listed = run(&mut s, &agent(), "edit.list_checkpoints", json!({})).unwrap();
    assert_eq!(
        listed.output["checkpoints"][0]["name"],
        json!("before captions")
    );
    let err = run(&mut s, &ui(), "edit.restore", json!({"checkpoint": "nope"})).unwrap_err();
    assert_eq!(err.code(), "not_found");
    for bad in [
        json!({"name": ""}),
        json!({"name": "cp_7"}),
        json!({"name": " x"}),
    ] {
        assert_eq!(
            run(&mut s, &ui(), "edit.checkpoint", bad)
                .unwrap_err()
                .code(),
            "invalid"
        );
    }
}

#[test]
fn restore_refuses_to_revert_other_actors_changes_unless_forced() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let x = append(&mut s, &agent(), tracks[0], 10);
    run(
        &mut s,
        &agent(),
        "edit.checkpoint",
        json!({"name": "agent start"}),
    )
    .unwrap();
    let checkpointed = s.committed_doc().clone();
    rename(&mut s, &agent(), x, "agent's");
    let users = rename(&mut s, &ui(), x, "user's");
    let before = observable(&s);
    let err = run(
        &mut s,
        &agent(),
        "edit.restore",
        json!({"checkpoint": "agent start"}),
    )
    .unwrap_err();
    assert_eq!(reason(&err), Some(ConflictReason::HistoryTargetModified));
    assert!(
        detail(&err).contains(&format!("user ({})", users.tx_id.unwrap())),
        "{}",
        detail(&err)
    );
    assert_eq!(observable(&s), before);
    let forced = run(
        &mut s,
        &agent(),
        "edit.restore",
        json!({"checkpoint": "agent start", "force": true}),
    )
    .unwrap();
    assert_eq!(s.committed_doc(), &checkpointed);
    assert_eq!(forced.output["reverted"].as_array().unwrap().len(), 2);
}

// ----- the journal and recovery -------------------------------------------------------------

type Journal = Arc<Mutex<Vec<(Value, JournaledCommit)>>>;

fn journaled(doc: Project) -> (DocConfig, Journal) {
    let journal: Journal = Arc::default();
    let sink = {
        let journal = Arc::clone(&journal);
        move |r: &CommitRecord<'_>| {
            journal
                .lock()
                .unwrap()
                .push((serde_json::to_value(r).unwrap(), r.to_owned()));
            Ok(())
        }
    };
    (config(doc).with_journal(Box::new(sink)), journal)
}

#[test]
fn undo_and_redo_are_journaled_with_their_request() {
    let (doc, tracks, _) = project(1);
    let (config, journal) = journaled(doc);
    let mut s = DocState::new(config);
    let x = append(&mut s, &ui(), tracks[0], 10);
    let renamed = rename(&mut s, &ui(), x, "a");
    let undone = undo(&mut s, &ui(), None).unwrap();
    let journal = journal.lock().unwrap();
    assert_eq!(journal.len(), 3);
    let (record, owned) = &journal[2];
    assert_eq!(
        record["cause"],
        json!({"type": "undo", "of": renamed.tx_id.unwrap()})
    );
    assert_eq!(record["label"], json!("Undo item.rename"));
    assert_eq!(record["undoable"], json!(false));
    assert_eq!(record["actor"], json!("user"));
    assert_eq!(record["tx_id"], json!(undone.tx_id.unwrap()));
    assert_eq!(record["patch"], serde_json::to_value(&undone.ops).unwrap());
    assert_eq!(record["steps"][0]["command"], json!("edit.undo"));
    assert_eq!(
        record["steps"][0]["record"]["applied"]["output"],
        undone.output
    );
    assert_eq!(journal[0].0["cause"], json!({"type": "edit"}));
    // The owned form reads the JSON back.
    assert_eq!(
        &serde_json::from_value::<JournaledCommit>(record.clone()).unwrap(),
        owned
    );
}

#[test]
fn recovered_history_undoes_the_last_recovered_transaction() {
    let (initial, tracks, _) = project(2);
    let (config, journal) = journaled(initial);
    let mut s = DocState::new(config);
    let (u, a) = (ui(), agent());
    let x = append(&mut s, &u, tracks[0], 10);
    let y = append(&mut s, &a, tracks[1], 10);
    rename(&mut s, &u, x, "one");
    undo(&mut s, &u, None).unwrap();
    rename(&mut s, &a, y, "agent");
    rename(&mut s, &u, x, "two");

    // "Restart": the recovered document plus the journal's history.
    let commits: Vec<JournaledCommit> = journal
        .lock()
        .unwrap()
        .iter()
        .map(|(_, c)| c.clone())
        .collect();
    let next_tx = commits.iter().map(|c| c.tx_id.get()).max().unwrap() + 1;
    let mut recovered = DocState::new(
        doc_support::config(s.committed_doc().clone())
            .with_rev(s.committed_rev())
            .with_first_tx_id(TxId::new(next_tx))
            .with_recovered_history(commits),
    );
    recovered.verify().unwrap();
    // Both behave the same from here on.
    for (ctx, actor) in [(&u, None), (&a, None), (&u, None), (&u, None), (&a, None)] {
        let original = undo(&mut s, ctx, actor).map(|_| s.committed_doc().clone());
        let replayed = undo(&mut recovered, ctx, actor).map(|_| recovered.committed_doc().clone());
        assert_eq!(
            original.map_err(|e| e.code()),
            replayed.map_err(|e| e.code())
        );
    }
    assert_eq!(recovered.committed_doc(), s.committed_doc());
    assert_eq!(items(recovered.committed_doc()).len(), 0);
}

// ----- request_id dedup and transactions -------------------------------------------------------

#[test]
fn a_repeated_undo_request_returns_the_original_result_and_undoes_once() {
    let (doc, tracks, _) = project(1);
    let clock = Arc::new(ManualClock::new(WallTime::from_millis(1)));
    let mut s = DocState::new(config(doc).with_clock(Arc::clone(&clock) as _));
    let x = append(&mut s, &ui(), tracks[0], 10);
    rename(&mut s, &ui(), x, "a");
    rename(&mut s, &ui(), x, "b");
    let id = cutweave_commands::RequestId::from_u128(0xABCD);
    let request = || Submit::new(ui(), "edit.undo", json!({})).with_request_id(id);
    let first = s.submit(request()).unwrap();
    let after = observable(&s);
    let second = s.submit(request()).unwrap();
    assert!(second.replayed && !first.replayed);
    assert_eq!(
        (second.output.clone(), second.rev, second.tx_id),
        (first.output.clone(), first.rev, first.tx_id)
    );
    assert_eq!(observable(&s), after, "undone once");
    assert_eq!(item_name(s.committed_doc(), x).as_deref(), Some("a"));
    // The same id for another request is refused.
    let err = s
        .submit(Submit::new(ui(), "edit.undo", json!({"actor": "*"})).with_request_id(id))
        .unwrap_err();
    assert_eq!(reason(&err), Some(ConflictReason::RequestIdReused));
    // Past the window, it is a new request.
    clock.advance(Duration::from_mins(11));
    let third = s.submit(request()).unwrap();
    assert!(!third.replayed);
    assert_eq!(item_name(s.committed_doc(), x), None);
}

#[test]
fn resending_an_undone_commands_request_does_not_reapply_it() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let x = append(&mut s, &ui(), tracks[0], 10);
    let id = cutweave_commands::RequestId::from_u128(0x1234);
    let rename_req =
        || Submit::new(ui(), "item.rename", json!({"item": x, "name": "a"})).with_request_id(id);
    let original = s.submit(rename_req()).unwrap();
    undo(&mut s, &ui(), None).unwrap();
    let before = observable(&s);
    // A client retry after a lost reply: the original result, nothing re-applied.
    let resent = s.submit(rename_req()).unwrap();
    assert!(resent.replayed);
    assert_eq!(resent.output, original.output);
    assert_eq!(observable(&s), before);
    assert_eq!(item_name(s.committed_doc(), x), None);
}

#[test]
fn gesture_steps_dedup_across_the_commit_and_stay_one_entry() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let x = append(&mut s, &ui(), tracks[0], 10);
    let tx = s.begin_tx(&ui(), TxKind::Gesture, "Drag").unwrap().id;
    let id = cutweave_commands::RequestId::from_u128(0x77);
    let step = || {
        Submit::new(ui(), "item.rename", json!({"item": x, "name": "dragged"}))
            .with_request_id(id)
            .in_tx(tx)
    };
    let first = s.submit(step()).unwrap();
    // A resend while still open replays.
    assert!(s.submit(step()).unwrap().replayed);
    s.commit_tx(&ui(), tx).unwrap();
    // And after the commit, too: nothing re-applied, still one entry for the gesture.
    let again = s.submit(step()).unwrap();
    assert!(again.replayed);
    assert_eq!(again.output, first.output);
    assert_eq!(history(&mut s).entries.len(), 2);
    // Read-only requests are not remembered: a repeat runs again.
    let read = || {
        Submit::new(ui(), "edit.history", json!({}))
            .with_request_id(cutweave_commands::RequestId::from_u128(0x99))
    };
    assert!(!s.submit(read()).unwrap().replayed);
    assert!(!s.submit(read()).unwrap().replayed);
}

#[test]
fn a_transaction_that_newer_commits_made_invalid_cannot_commit() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let tx = s.begin_tx(&ui(), TxKind::Gesture, "Drop clip").unwrap().id;
    // The gesture appends a clip at the end of the track (live) ...
    s.submit(
        Submit::new(
            ui(),
            "timeline.append_clip",
            json!({"track": tracks[0], "frames": 10}),
        )
        .in_tx(tx),
    )
    .unwrap();
    // ... and the agent commits another clip into the same gap (other ids: no soft lock).
    append(&mut s, &agent(), tracks[0], 10);
    let before = observable(&s);
    let err = s.commit_tx(&ui(), tx).unwrap_err();
    assert_eq!(reason(&err), Some(ConflictReason::StaleRev));
    assert!(detail(&err).contains("overlap"), "{}", detail(&err));
    // Nothing changed; the owner can still cancel.
    assert_eq!(observable(&s), before);
    s.committed_doc().validate().unwrap();
    s.cancel_tx(&ui(), tx).unwrap();
    s.doc().validate().unwrap();
    s.verify().unwrap();
}

// ----- `steps` (WP-2H.11) ----------------------------------------------------------------------

fn moved(s: &mut DocState, name: &str, params: Value) -> Result<Value, CmdError> {
    run(s, &ui(), name, params).map(|a| a.output)
}

/// What a history leaves behind: cause and label of every entry, newest first.
fn trail(s: &mut DocState) -> Vec<(String, String)> {
    history(s)
        .entries
        .iter()
        .map(|e| (format!("{:?}", e.cause), e.label.clone()))
        .collect()
}

fn three_changes() -> (DocState, ItemId) {
    let (doc, tracks, _) = project(2);
    let mut s = state(doc);
    let (u, a) = (ui(), agent());
    let x = append(&mut s, &u, tracks[0], 10);
    rename(&mut s, &a, x, "agent's");
    append(&mut s, &u, tracks[1], 10);
    (s, x)
}

#[test]
fn undo_steps_equal_the_same_number_of_single_star_undos() {
    let (mut batched, _) = three_changes();
    let (mut single, _) = three_changes();
    let out = moved(&mut batched, "edit.undo", json!({"actor": "*", "steps": 3})).unwrap();
    assert_eq!(out["done"], json!(3));
    assert!(out.get("stopped").is_none());
    for _ in 0..3 {
        undo(&mut single, &ui(), Some("*")).unwrap();
    }
    assert_eq!(batched.committed_doc(), single.committed_doc());
    assert_eq!(trail(&mut batched), trail(&mut single));
    assert_eq!(
        history(&mut batched).entries.len(),
        6,
        "each step is a record"
    );
    // Redo the same way, and one at a time afterwards.
    let out = moved(&mut batched, "edit.redo", json!({"actor": "*", "steps": 3})).unwrap();
    assert_eq!(out["done"], json!(3));
    for _ in 0..3 {
        redo(&mut single, &ui(), Some("*")).unwrap();
    }
    assert_eq!(batched.committed_doc(), single.committed_doc());
    assert_eq!(trail(&mut batched), trail(&mut single));
    batched.verify().unwrap();
}

#[test]
fn steps_default_to_one_and_are_bounded() {
    let (mut s, _) = three_changes();
    let out = moved(&mut s, "edit.undo", json!({"actor": "*"})).unwrap();
    assert_eq!(out["done"], json!(1));
    for bad in [0, 101] {
        let err = run(
            &mut s,
            &ui(),
            "edit.undo",
            json!({"actor": "*", "steps": bad}),
        )
        .unwrap_err();
        assert_eq!(err.code(), "invalid", "{bad}");
    }
}

#[test]
fn a_conflict_at_the_second_step_leaves_the_first_applied_and_reports_done() {
    let (doc, tracks, _) = project(1);
    let mut s = state(doc);
    let u = ui();
    run(&mut s, &u, "project.rename", json!({"name": "A"})).unwrap();
    // Not undoable, and it touches what the rename changed.
    run(&mut s, &u, "project.stamp", json!({"name": "B"})).unwrap();
    let x = append(&mut s, &u, tracks[0], 10);
    let out = moved(&mut s, "edit.undo", json!({"actor": "*", "steps": 3})).unwrap();
    assert_eq!(out["done"], json!(1));
    let stopped = out["stopped"].as_str().unwrap();
    assert!(stopped.contains("project.stamp"), "{stopped}");
    assert!(!items(s.committed_doc()).contains(&x));
    assert_eq!(s.committed_doc().meta.name, "B");
    // A conflict at the very first step is the error itself.
    let err = run(&mut s, &u, "edit.undo", json!({"actor": "*", "steps": 2})).unwrap_err();
    assert_eq!(reason(&err), Some(ConflictReason::HistoryTargetModified));
}

#[test]
fn running_out_of_changes_stops_the_run_with_done() {
    let (mut s, _) = three_changes();
    let out = moved(&mut s, "edit.undo", json!({"actor": "*", "steps": 9})).unwrap();
    assert_eq!(out["done"], json!(3));
    assert!(out["stopped"].as_str().unwrap().contains("nothing to undo"));
}
