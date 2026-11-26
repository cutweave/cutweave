// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.12 acceptance property: interleaved actors. A (the person, connection 1) opens
//! gestures and proposals and sends updates (accumulating or `_meta.replace`), commits,
//! cancels, drops its connection or lets the gesture idle out; B (an agent, connection 2)
//! runs random commands and undoes its own. After every step:
//!
//! - history stays linear: each entry builds on the one before it, the newest on the
//!   committed revision, one per journaled commit;
//! - B never mutates an item A's open transaction locks, and B's commands on other items always
//!   succeed: B's command fails with `item_in_gesture` exactly when the same request on the
//!   committed document alone would touch a locked item (or names an item only A's
//!   transaction has), and otherwise behaves exactly as it would there;
//! - A's updates never meet a lock (B holds none) and a refused request changes nothing;
//! - after A cancels (or drops, or idles out), the document equals the state as if A's
//!   transaction never happened plus B's edits; after A commits it is that plus A's net
//!   change, and undoing A's commit at once gives the state before it back (undo identity),
//!   redo the state after it.
//!
//! The commands are item-level (append, rename, move, trim, delete, ripple shift): every op
//! of A's transaction is under an item it locks, so "disjoint" is decided by items alone.
//! Track removal and connection churn of three actors are WP-1B.09's `doc_locks_props`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod doc_support;

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use cutweave_commands::{
    ActorCtx, Applied, CmdError, CommitRecord, ConflictReason, ConnId, DocConfig, DocEvent,
    DocState, ManualClock, Patch, PatchOp, Submit, TxId, TxKind, WallTime,
};
use cutweave_model::{ItemId, Project, TrackId};
use doc_support::{Mirror, config, items, project, registry, replay};
use proptest::prelude::*;
use serde_json::{Value, json};

#[derive(Clone, Debug)]
enum Cmd {
    Append { track: usize, frames: u8 },
    Rename { item: usize, name: u8 },
    Move { item: usize, at: u16 },
    Trim { item: usize, frames: u8 },
    Delete { item: usize },
    Shift { track: usize, at: u16, frames: u8 },
}

#[derive(Clone, Debug)]
enum Op {
    /// A opens a gesture or a proposal (nothing if it has one open).
    Begin {
        gesture: bool,
    },
    /// A's update in its open transaction (an implicit command when it has none).
    Update {
        cmd: Cmd,
        replace: bool,
    },
    /// B runs a command (implicit).
    Agent {
        cmd: Cmd,
    },
    /// B undoes its latest change.
    AgentUndo,
    Commit,
    Cancel,
    /// A's connection drops; it reconnects with a new connection id.
    Drop,
    /// Time passes (gestures idle for 30 s are cancelled).
    Idle {
        secs: u8,
    },
}

fn arb_cmd() -> impl Strategy<Value = Cmd> {
    prop_oneof![
        3 => (0..3_usize, 1..20_u8).prop_map(|(track, frames)| Cmd::Append { track, frames }),
        3 => (any::<usize>(), any::<u8>()).prop_map(|(item, name)| Cmd::Rename { item, name }),
        3 => (any::<usize>(), any::<u16>()).prop_map(|(item, at)| Cmd::Move { item, at }),
        2 => (any::<usize>(), 1..40_u8).prop_map(|(item, frames)| Cmd::Trim { item, frames }),
        1 => any::<usize>().prop_map(|item| Cmd::Delete { item }),
        2 => (0..3_usize, any::<u16>(), 1..10_u8)
            .prop_map(|(track, at, frames)| Cmd::Shift { track, at, frames }),
    ]
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => any::<bool>().prop_map(|gesture| Op::Begin { gesture }),
        8 => (arb_cmd(), any::<bool>()).prop_map(|(cmd, replace)| Op::Update { cmd, replace }),
        8 => arb_cmd().prop_map(|cmd| Op::Agent { cmd }),
        1 => Just(Op::AgentUndo),
        2 => Just(Op::Commit),
        1 => Just(Op::Cancel),
        1 => Just(Op::Drop),
        1 => (0..40_u8).prop_map(|secs| Op::Idle { secs }),
    ]
}

/// The request for `cmd`, items drawn from `live` (what each actor sees).
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
        Cmd::Shift { track, at, frames } => (
            "timeline.shift_after",
            json!({"track": tracks[track], "at": at % 300, "frames": frames}),
        ),
    }
}

fn is_lock_conflict(e: &CmdError) -> bool {
    matches!(
        e,
        CmdError::Conflict {
            reason: ConflictReason::ItemInGesture,
            holder: Some(_),
            ..
        }
    )
}

/// Whether `patch` touches one of `locked` (an op on the item, or on a path above where it is
/// in `doc`).
fn touches(patch: &Patch, locked: &BTreeSet<ItemId>, docs: &[&Project]) -> bool {
    let places: Vec<String> = locked
        .iter()
        .flat_map(|item| {
            docs.iter().filter_map(move |doc| {
                let found = doc.find_item(item)?;
                Some(format!(
                    "/sequences/{}/tracks/{}/items/{item}",
                    found.sequence.id, found.track.id
                ))
            })
        })
        .collect();
    patch.iter().flat_map(PatchOp::paths).any(|at| {
        let text = at.as_str();
        locked
            .iter()
            .any(|item| text.contains(&format!("/items/{item}")))
            || places
                .iter()
                .any(|place| place == text || place.starts_with(&format!("{text}/")))
    })
}

/// The JSON of every item in `locked` that exists in `doc` (with its track).
fn locked_values(
    doc: &Project,
    locked: &BTreeSet<ItemId>,
) -> Vec<(ItemId, Option<(TrackId, Value)>)> {
    locked
        .iter()
        .map(|item| {
            let found = doc
                .find_item(item)
                .map(|f| (f.track.id, serde_json::to_value(f.item).unwrap()));
            (*item, found)
        })
        .collect()
}

/// The same request on the committed document alone, by B, in a fresh state.
fn alone(committed: &Project, name: &str, params: &Value, seed: u64) -> Result<Applied, CmdError> {
    let mut scratch = DocState::new(DocConfig::deterministic(
        registry(),
        committed.clone(),
        seed,
    ));
    scratch.submit(Submit::new(agent(), name, params.clone()))
}

fn person(generation: u64) -> ActorCtx {
    ActorCtx::ui(ConnId::new(1 + 1_000 * generation))
}

fn agent() -> ActorCtx {
    ActorCtx::agent(ConnId::new(2), "claude").unwrap()
}

fn assert_history_linear(s: &DocState, journaled: usize) {
    let page = s.history_page(1_000, None, None).unwrap();
    assert_eq!(
        page.entries.len(),
        journaled,
        "one history entry per commit"
    );
    if let Some(newest) = page.entries.first() {
        assert_eq!(newest.rev, s.committed_rev());
    }
    for pair in page.entries.windows(2) {
        assert_eq!(
            pair[0].prev_rev, pair[1].rev,
            "history is not linear: {} does not build on {}",
            pair[0].tx_id, pair[1].tx_id
        );
    }
}

/// A's open transaction: its id, kind, the committed document when it began and B's changes
/// committed since.
struct Open {
    tx: TxId,
    kind: TxKind,
    base: Project,
    agent_patches: Vec<Patch>,
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
    let mut generation = 0_u64;
    let mut open: Option<Open> = None;
    let mut mirror = Mirror::new(&initial, s.rev());
    let bot = agent();

    for (step, op) in ops.iter().enumerate() {
        let a = person(generation);
        let locked: BTreeSet<ItemId> = s
            .transactions()
            .into_iter()
            .filter(|t| t.actor == *a.actor())
            .flat_map(|t| t.items)
            .collect();
        let before_doc = s.doc().clone();
        let before_committed = s.committed_doc().clone();
        let before_rev = (s.rev(), s.committed_rev());
        let locked_before = locked_values(&before_committed, &locked);
        let mut closed_without_commit = false;

        match op {
            Op::Begin { gesture } => {
                if open.is_none() {
                    let kind = if *gesture {
                        TxKind::Gesture
                    } else {
                        TxKind::Proposal
                    };
                    let tx = s.begin_tx(&a, kind, "A").unwrap().id;
                    open = Some(Open {
                        tx,
                        kind,
                        base: s.committed_doc().clone(),
                        agent_patches: Vec::new(),
                    });
                }
            }
            Op::Update { cmd, replace } => {
                let (name, params) = request(cmd, s.doc(), &tracks);
                let mut submit = Submit::new(a.clone(), name, params);
                if let Some(o) = &open {
                    submit = submit.in_tx(o.tx);
                    if *replace && o.kind == TxKind::Gesture {
                        submit = submit.replacing();
                    }
                }
                match s.submit(submit) {
                    Ok(_) => {}
                    Err(e) => {
                        // B holds no lock, so A never meets one.
                        assert!(!is_lock_conflict(&e), "{op:?}: {e}");
                        assert!(matches!(e.code(), "invalid" | "not_found"), "{op:?}: {e}");
                        assert_eq!(s.doc(), &before_doc, "{op:?} failed but changed it");
                        assert_eq!((s.rev(), s.committed_rev()), before_rev);
                    }
                }
            }
            Op::Agent { cmd } => {
                let (name, params) = request(cmd, s.doc(), &tracks);
                let reference = alone(&before_committed, name, &params, 9_000 + step as u64);
                let result = s.submit(Submit::new(bot.clone(), name, params.clone()));
                match (&reference, &result) {
                    (Ok(alone), Ok(applied)) => {
                        // Disjoint: it did exactly what it does alone.
                        assert!(
                            !touches(&alone.ops, &locked, &[&before_committed, &before_doc]),
                            "{op:?} touched a locked item"
                        );
                        if name != "timeline.append_clip" {
                            assert_eq!(applied.ops, alone.ops, "{op:?}");
                        }
                        if let Some(o) = &mut open {
                            o.agent_patches.push(applied.ops.clone());
                        }
                    }
                    (Ok(alone), Err(e)) => {
                        assert!(is_lock_conflict(e), "{op:?}: {e}");
                        assert!(
                            touches(&alone.ops, &locked, &[&before_committed, &before_doc]),
                            "{op:?} on items A does not lock was refused: {e}"
                        );
                    }
                    (Err(alone), Err(e)) => {
                        let only_in_a = matches!(alone, CmdError::NotFound { id, .. }
                            if locked.iter().any(|i| i.to_string() == *id));
                        if only_in_a {
                            assert!(is_lock_conflict(e), "{op:?}: {e}");
                        } else {
                            assert_eq!(alone.code(), e.code(), "{op:?}: {alone} / {e}");
                        }
                    }
                    (Err(alone), Ok(_)) => panic!("{op:?} failed alone ({alone}) but succeeded"),
                }
                if result.is_err() {
                    assert_eq!(s.doc(), &before_doc, "{op:?} failed but changed it");
                    assert_eq!((s.rev(), s.committed_rev()), before_rev);
                }
            }
            Op::AgentUndo => match s.submit(Submit::new(bot.clone(), "edit.undo", json!({}))) {
                Ok(applied) => {
                    if let Some(o) = &mut open {
                        o.agent_patches.push(applied.ops.clone());
                    }
                }
                Err(e) => {
                    assert!(matches!(e.code(), "conflict" | "invalid"), "{op:?}: {e}");
                    assert_eq!(s.doc(), &before_doc);
                }
            },
            Op::Commit => {
                if let Some(o) = open.take() {
                    match s.commit_tx(&a, o.tx) {
                        Ok(closed) => {
                            let without = replay(&o.base, &o.agent_patches);
                            assert_eq!(&without, &before_committed);
                            assert_eq!(
                                &replay(&without, std::slice::from_ref(&closed.patch)),
                                s.committed_doc()
                            );
                            if !closed.patch.is_empty() {
                                // Undo identity: A's undo at once gives the state before back.
                                let after = s.committed_doc().clone();
                                s.submit(Submit::new(a.clone(), "edit.undo", json!({})))
                                    .unwrap();
                                assert_eq!(s.committed_doc(), &without, "undo identity");
                                s.submit(Submit::new(a.clone(), "edit.redo", json!({})))
                                    .unwrap();
                                assert_eq!(s.committed_doc(), &after, "redo");
                            }
                        }
                        Err(e) => {
                            // Built on changes B committed since, it would break an invariant:
                            // still open; it can be updated or cancelled.
                            assert_eq!(e.code(), "conflict", "{op:?}: {e}");
                            assert!(s.transaction(o.tx).is_some());
                            assert_eq!(s.doc(), &before_doc);
                            open = Some(o);
                        }
                    }
                }
            }
            Op::Cancel => {
                if let Some(o) = &open {
                    s.cancel_tx(&a, o.tx).unwrap();
                    closed_without_commit = true;
                }
            }
            Op::Drop => {
                s.close_connection(a.conn());
                generation += 1;
                closed_without_commit = open.is_some();
            }
            Op::Idle { secs } => {
                clock.advance(Duration::from_secs(u64::from(*secs)));
                let expired = s.expire_idle();
                if let Some(o) = &open
                    && expired.iter().any(|t| t.tx_id == o.tx)
                {
                    assert_eq!(o.kind, TxKind::Gesture, "only gestures expire");
                    closed_without_commit = true;
                }
            }
        }

        // After A's transaction ended without committing: as if it never happened, plus B.
        if closed_without_commit && let Some(o) = open.take() {
            assert!(s.transaction(o.tx).is_none());
            assert_eq!(s.doc(), s.committed_doc());
            assert_eq!(&replay(&o.base, &o.agent_patches), s.committed_doc());
        }

        // B never mutates an item A's open transaction locked (only A, by committing).
        if !matches!(op, Op::Commit) {
            assert_eq!(
                locked_values(s.committed_doc(), &locked),
                locked_before,
                "after {op:?}: a locked item changed"
            );
        }
        // Events replay to the live document; history stays linear.
        let _ = s.take_changes();
        for event in s.take_events() {
            if let DocEvent::GestureCancelled(g) = &event {
                assert_eq!(g.actor, *person(0).actor());
            }
            mirror.apply(&event);
        }
        mirror.assert_matches(s.doc());
        s.verify().unwrap_or_else(|e| panic!("after {op:?}: {e}"));
        s.committed_doc().validate().unwrap();
        let journaled = journal.lock().unwrap().len();
        assert_history_linear(&s, journaled);
        assert_eq!(
            &replay(&initial, &journal.lock().unwrap()),
            s.committed_doc()
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig { max_shrink_iters: 2_000, ..ProptestConfig::default() })]

    /// Interleaved actors A (gestures, proposals, replacing updates) and B (commands, undo).
    #[test]
    fn interleaved_actors_keep_history_linear_and_locked_items_untouched(
        ops in prop::collection::vec(arb_op(), 1..80),
    ) {
        interleavings(&ops);
    }
}
