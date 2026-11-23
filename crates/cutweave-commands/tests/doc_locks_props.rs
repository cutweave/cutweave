// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.09 property tests: random interleavings of begin (batch, gesture, proposal), steps,
//! implicit commands, undo, commit, cancel, connection drops (then reconnects) and idle time
//! from three connections of three actors. After every step:
//!
//! - the state's invariants hold (`DocState::verify`: locks are exactly the items each open
//!   gesture or proposal touched, one transaction per connection, no item locked by two
//!   actors);
//! - no change by one actor touched an item another actor's open transaction had locked, and
//!   every refusal changed nothing (a refused commit leaves its batch open);
//! - a dropped connection owns nothing any more, and the locks announced by `tx.opened` /
//!   `tx.locked` / `tx.closed` events equal `edit.locks` (no dangling lock in UIs either);
//! - the committed document equals the replay of the journaled (committed) transactions, and
//!   the events replay to the live document.
//!
//! At the end every connection drops: no transaction and no lock is left, and the live
//! document is the committed one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod doc_support;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use cutweave_commands::{
    Actor, ActorCtx, CmdError, CommitRecord, ConflictReason, ConnId, DocEvent, DocState,
    ManualClock, Patch, Rev, Submit, TxId, TxKind, WallTime,
};
use cutweave_model::{ItemId, Project, TrackId};
use doc_support::{Mirror, config, items, project, replay};
use proptest::prelude::*;
use serde_json::{Value, json};

#[derive(Clone, Debug)]
enum Cmd {
    Append { track: usize, frames: u32 },
    Rename { item: usize, name: u8 },
    Move { item: usize, at: u16 },
    Trim { item: usize, frames: u8 },
    Delete { item: usize },
    RemoveTrack { track: usize },
}

#[derive(Clone, Debug)]
enum Op {
    Begin {
        conn: usize,
        kind: TxKind,
    },
    /// In the connection's open transaction (implicit when it has none).
    Step {
        conn: usize,
        cmd: Cmd,
    },
    /// An implicit single-command transaction.
    Run {
        conn: usize,
        cmd: Cmd,
    },
    Undo {
        conn: usize,
    },
    Commit {
        conn: usize,
    },
    Cancel {
        conn: usize,
    },
    /// The connection closes; the client reconnects with a new connection id.
    Drop {
        conn: usize,
    },
    /// Time passes (gestures idle for 30 s expire).
    Idle {
        secs: u8,
    },
}

fn arb_cmd() -> impl Strategy<Value = Cmd> {
    prop_oneof![
        3 => (0..3_usize, 1..20_u32).prop_map(|(track, frames)| Cmd::Append { track, frames }),
        3 => (any::<usize>(), any::<u8>()).prop_map(|(item, name)| Cmd::Rename { item, name }),
        3 => (any::<usize>(), any::<u16>()).prop_map(|(item, at)| Cmd::Move { item, at }),
        2 => (any::<usize>(), 1..40_u8).prop_map(|(item, frames)| Cmd::Trim { item, frames }),
        1 => any::<usize>().prop_map(|item| Cmd::Delete { item }),
        1 => (0..3_usize).prop_map(|track| Cmd::RemoveTrack { track }),
    ]
}

fn arb_op() -> impl Strategy<Value = Op> {
    let conn = 0..3_usize;
    let kind = prop_oneof![
        1 => Just(TxKind::Batch),
        2 => Just(TxKind::Gesture),
        2 => Just(TxKind::Proposal)
    ];
    prop_oneof![
        3 => (conn.clone(), kind).prop_map(|(conn, kind)| Op::Begin { conn, kind }),
        8 => (conn.clone(), arb_cmd()).prop_map(|(conn, cmd)| Op::Step { conn, cmd }),
        4 => (conn.clone(), arb_cmd()).prop_map(|(conn, cmd)| Op::Run { conn, cmd }),
        1 => conn.clone().prop_map(|conn| Op::Undo { conn }),
        2 => conn.clone().prop_map(|conn| Op::Commit { conn }),
        1 => conn.clone().prop_map(|conn| Op::Cancel { conn }),
        2 => conn.prop_map(|conn| Op::Drop { conn }),
        1 => (0..40_u8).prop_map(|secs| Op::Idle { secs }),
    ]
}

/// Connection `slot` (0: the UI, 1: an agent, 2: a plugin) in its `generation`-th session.
fn ctx(slot: usize, generation: u64) -> ActorCtx {
    let conn = ConnId::new(1 + slot as u64 * 1_000 + generation);
    match slot {
        0 => ActorCtx::ui(conn),
        1 => ActorCtx::agent(conn, "claude").unwrap(),
        _ => ActorCtx::plugin(conn, "acme.tools").unwrap(),
    }
}

/// The request for `cmd`, items drawn from the live document.
fn request(cmd: &Cmd, live: &Project, tracks: &[TrackId]) -> (&'static str, Value) {
    let all = items(live);
    let pick = |i: usize| (!all.is_empty()).then(|| all[i % all.len()]);
    let or_noop = |item: Option<ItemId>, f: &dyn Fn(ItemId) -> (&'static str, Value)| {
        item.map_or(("project.noop", json!({})), f)
    };
    match *cmd {
        Cmd::Append { track, frames } => (
            "timeline.append_clip",
            json!({"track": tracks[track], "frames": frames}),
        ),
        Cmd::Rename { item, name } => or_noop(pick(item), &|item| {
            (
                "item.rename",
                json!({"item": item, "name": format!("n{name}")}),
            )
        }),
        Cmd::Move { item, at } => or_noop(pick(item), &|item| {
            ("clip.move", json!({"item": item, "at": at % 600}))
        }),
        Cmd::Trim { item, frames } => or_noop(pick(item), &|item| {
            ("clip.trim", json!({"item": item, "frames": frames}))
        }),
        Cmd::Delete { item } => or_noop(pick(item), &|item| ("item.delete", json!({"item": item}))),
        Cmd::RemoveTrack { track } => ("track.remove", json!({"track": tracks[track]})),
    }
}

fn observable(s: &DocState) -> (Rev, Rev, Project, Project, usize) {
    (
        s.rev(),
        s.committed_rev(),
        s.doc().clone(),
        s.committed_doc().clone(),
        s.transactions().len(),
    )
}

/// Item → actor of the open transactions that lock it.
fn holders(s: &DocState) -> BTreeMap<ItemId, Actor> {
    s.transactions()
        .into_iter()
        .flat_map(|t| t.items.into_iter().map(move |i| (i, t.actor.clone())))
        .collect()
}

fn is_lock_conflict(e: &CmdError) -> bool {
    matches!(
        e,
        CmdError::Conflict {
            reason: ConflictReason::ItemInGesture,
            ..
        }
    )
}

#[allow(clippy::too_many_lines)]
fn interleavings(ops: &[Op]) {
    let (initial, tracks, _) = project(3);
    let journal: Arc<Mutex<Vec<Patch>>> = Arc::default();
    let sink = {
        let journal = Arc::clone(&journal);
        move |r: &CommitRecord<'_>| {
            journal.lock().unwrap().push(r.patch.clone());
            Ok(())
        }
    };
    let clock = Arc::new(ManualClock::new(WallTime::from_millis(1_000_000)));
    let mut s = DocState::new(
        config(initial.clone())
            .with_journal(Box::new(sink))
            .with_clock(Arc::clone(&clock) as _),
    );
    let mut generation = [0_u64; 3];
    let mut open: [Option<TxId>; 3] = [None; 3];
    let mut mirror = Mirror::new(&initial, Rev(0));
    // The locks UIs know of, from `tx.*` events only.
    let mut announced: BTreeMap<TxId, Vec<ItemId>> = BTreeMap::new();
    let mut replayed = initial.clone();
    let mut journaled = 0;

    for op in ops {
        let before = observable(&s);
        let locked_before = holders(&s);
        let conn_of = |slot: usize| ctx(slot, generation[slot]);
        // `Ok(Some((actor, patch)))`: a change by `actor`.
        let result: Result<Option<(Actor, Patch)>, CmdError> = match op {
            Op::Begin { conn, kind } => s.begin_tx(&conn_of(*conn), *kind, "prop").map(|info| {
                open[*conn] = Some(info.id);
                None
            }),
            Op::Step { conn, cmd } | Op::Run { conn, cmd } => {
                let c = conn_of(*conn);
                let (name, params) = request(cmd, s.doc(), &tracks);
                let mut submit = Submit::new(c.clone(), name, params);
                if let (Op::Step { .. }, Some(tx)) = (op, open[*conn]) {
                    submit = submit.in_tx(tx);
                }
                s.submit(submit).map(|a| Some((c.actor().clone(), a.ops)))
            }
            Op::Undo { conn } => {
                let c = conn_of(*conn);
                s.submit(Submit::new(c.clone(), "edit.undo", json!({})))
                    .map(|a| Some((c.actor().clone(), a.ops)))
            }
            Op::Commit { conn } => match open[*conn] {
                Some(tx) => {
                    let c = conn_of(*conn);
                    match s.commit_tx(&c, tx) {
                        Ok(closed) => {
                            open[*conn] = None;
                            Ok(Some((c.actor().clone(), closed.patch)))
                        }
                        Err(e) => {
                            if is_lock_conflict(&e) {
                                // Refused by a lock: still open, nothing changed.
                                assert!(s.transaction(tx).is_some(), "{op:?}: {e}");
                                assert_eq!(observable(&s), before, "{op:?}");
                            }
                            Err(e)
                        }
                    }
                }
                None => Ok(None),
            },
            Op::Cancel { conn } => match open[*conn].take() {
                Some(tx) => s.cancel_tx(&conn_of(*conn), tx).map(|_| None),
                None => Ok(None),
            },
            Op::Drop { conn } => {
                let gone = conn_of(*conn).conn();
                s.close_connection(gone);
                assert!(s.transaction_of(gone).is_none(), "{op:?}: still owns one");
                assert!(
                    s.begin_tx(&conn_of(*conn), TxKind::Gesture, "late")
                        .is_err()
                );
                open[*conn] = None;
                generation[*conn] += 1;
                Ok(None)
            }
            Op::Idle { secs } => {
                clock.advance(Duration::from_secs(u64::from(*secs)));
                s.expire_idle();
                Ok(None)
            }
        };
        // Closed transactions (expired, or discarded by a refused commit) are gone for their
        // owners too.
        for tx in &mut open {
            if tx.is_some_and(|id| s.transaction(id).is_none()) {
                *tx = None;
            }
        }

        match &result {
            Ok(Some((actor, patch))) => {
                // Never a change to an item another actor's open transaction locked.
                let foreign: BTreeSet<ItemId> = patch
                    .touched_items()
                    .into_iter()
                    .filter(|i| locked_before.get(i).is_some_and(|holder| holder != actor))
                    .collect();
                assert!(foreign.is_empty(), "{op:?} touched locked {foreign:?}");
            }
            Ok(None) => {}
            Err(e) => {
                assert!(
                    matches!(e.code(), "conflict" | "invalid" | "not_found" | "locked"),
                    "{op:?}: unexpected {e}"
                );
                // A refused request changes nothing (a conflicted commit closes its tx).
                if !matches!(op, Op::Commit { .. }) {
                    assert_eq!(
                        observable(&s),
                        before,
                        "{op:?} failed but changed the state"
                    );
                }
            }
        }
        s.verify().unwrap_or_else(|e| panic!("after {op:?}: {e}"));
        s.committed_doc().validate().unwrap();

        // Events: the live document, and the locks UIs were told about.
        let _ = s.take_changes();
        for event in s.take_events() {
            mirror.apply(&event);
            match &event {
                DocEvent::TxOpened(t) | DocEvent::TxLocked(t) => {
                    announced.insert(t.tx_id, t.items.clone());
                }
                DocEvent::TxClosed(t) => {
                    let known = announced.remove(&t.tx_id);
                    assert_eq!(
                        known.as_ref(),
                        Some(&t.items),
                        "{op:?}: released other items"
                    );
                }
                _ => {}
            }
        }
        mirror.assert_matches(s.doc());
        let listed: BTreeMap<TxId, Vec<ItemId>> = s
            .locks(None)
            .transactions
            .into_iter()
            .map(|t| (t.tx, t.items))
            .collect();
        assert_eq!(
            announced, listed,
            "after {op:?}: events and edit.locks disagree"
        );

        // The committed document is the replay of what was journaled.
        let journal = journal.lock().unwrap();
        for patch in &journal[journaled..] {
            replayed = patch.apply(&replayed).unwrap();
        }
        journaled = journal.len();
        assert_eq!(&replayed, s.committed_doc(), "after {op:?}");
    }

    // Every connection drops: nothing is left open or locked.
    for (slot, generation) in generation.iter().enumerate() {
        s.close_connection(ctx(slot, *generation).conn());
    }
    assert_eq!(s.transactions().len(), 0);
    assert_eq!(s.locks(None).transactions.len(), 0);
    assert_eq!(s.doc(), s.committed_doc());
    s.verify().unwrap();
    let patches = journal.lock().unwrap().clone();
    assert_eq!(&replay(&initial, &patches), s.committed_doc());
}

proptest! {
    #![proptest_config(ProptestConfig { max_shrink_iters: 2_000, ..ProptestConfig::default() })]

    /// At least 100 (proptest's default: 256) random interleavings with 3 connections.
    #[test]
    fn random_interleavings_never_leave_a_dangling_lock_or_an_unreplayable_document(
        ops in prop::collection::vec(arb_op(), 1..80),
    ) {
        interleavings(&ops);
    }
}
