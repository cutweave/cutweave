// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.03 property tests: for random sequences of commands, transactions (batch, gesture,
//! proposal; commit, cancel, connection drop), failures and panics from three connections:
//!
//! - the snapshot at every revision N equals the initial document with the patches of events
//!   1..N applied, on the typed document and on its JSON mirror (the UI docStore contract);
//! - the committed document equals the initial document with the journaled patches applied,
//!   and `committed_snapshot()` never shows an open transaction's changes;
//! - revisions only increase, `committed_rev <= rev`, and a failed request changes nothing;
//! - the state's own invariants hold after every step (`DocState::verify`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod doc_support;

use std::sync::{Arc, Mutex, Once};

use cutweave_commands::{
    ActorCtx, CommitRecord, ConnId, DocState, Patch, Rev, Submit, TxId, TxKind,
};
use cutweave_model::{ItemId, Project, TrackId};
use doc_support::{Mirror, config, items, project, replay, spawn};
use proptest::prelude::*;
use serde_json::{Value, json};

/// Silences the expected `project.explode` panics; everything else still prints.
fn quiet_expected_panics() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if info.payload().downcast_ref::<&str>() != Some(&"boom") {
                default(info);
            }
        }));
    });
}

#[derive(Clone, Debug)]
enum Cmd {
    Append { track: usize, frames: u32 },
    Rename { item: usize, name: u8 },
    Toggle { item: usize, enabled: bool },
    Delete { item: usize },
    RenameProject { name: u8 },
    Fail,
    Noop,
    Panic,
}

#[derive(Clone, Debug)]
enum Op {
    /// An implicit single-command transaction.
    Run {
        conn: usize,
        cmd: Cmd,
    },
    Begin {
        conn: usize,
        kind: TxKind,
    },
    /// A command in the connection's open transaction (implicit if it has none).
    Step {
        conn: usize,
        cmd: Cmd,
    },
    Commit {
        conn: usize,
    },
    Cancel {
        conn: usize,
    },
    Drop {
        conn: usize,
    },
}

fn arb_cmd() -> impl Strategy<Value = Cmd> {
    prop_oneof![
        4 => (0..3_usize, 1..20_u32).prop_map(|(track, frames)| Cmd::Append { track, frames }),
        3 => (any::<usize>(), any::<u8>()).prop_map(|(item, name)| Cmd::Rename { item, name }),
        2 => (any::<usize>(), any::<bool>()).prop_map(|(item, enabled)| Cmd::Toggle { item, enabled }),
        1 => any::<usize>().prop_map(|item| Cmd::Delete { item }),
        2 => any::<u8>().prop_map(|name| Cmd::RenameProject { name }),
        1 => Just(Cmd::Fail),
        1 => Just(Cmd::Noop),
        1 => Just(Cmd::Panic),
    ]
}

fn arb_op() -> impl Strategy<Value = Op> {
    let conn = 0..3_usize;
    let kind = prop_oneof![
        Just(TxKind::Batch),
        Just(TxKind::Gesture),
        Just(TxKind::Proposal)
    ];
    prop_oneof![
        3 => (conn.clone(), arb_cmd()).prop_map(|(conn, cmd)| Op::Run { conn, cmd }),
        3 => (conn.clone(), kind).prop_map(|(conn, kind)| Op::Begin { conn, kind }),
        8 => (conn.clone(), arb_cmd()).prop_map(|(conn, cmd)| Op::Step { conn, cmd }),
        2 => conn.clone().prop_map(|conn| Op::Commit { conn }),
        1 => conn.clone().prop_map(|conn| Op::Cancel { conn }),
        1 => conn.prop_map(|conn| Op::Drop { conn }),
    ]
}

fn ctx(conn: usize) -> ActorCtx {
    session(conn, 0)
}

/// Connection `conn` after `reconnects` drops (a closed connection id is never reused).
fn session(conn: usize, reconnects: u64) -> ActorCtx {
    let id = ConnId::new(1 + conn as u64 + 100 * reconnects);
    match conn {
        0 => ActorCtx::ui(id),
        1 => ActorCtx::agent(id, "claude").unwrap(),
        _ => ActorCtx::plugin(id, "acme.tools").unwrap(),
    }
}

fn pick(all: &[ItemId], i: usize) -> Option<ItemId> {
    (!all.is_empty()).then(|| all[i % all.len()])
}

/// The request for `cmd`, items drawn from the live document.
fn request(cmd: &Cmd, live: &Project, tracks: &[TrackId]) -> (&'static str, Value) {
    let all = items(live);
    match *cmd {
        Cmd::Append { track, frames } => (
            "timeline.append_clip",
            json!({"track": tracks[track], "frames": frames}),
        ),
        Cmd::Rename { item, name } => match pick(&all, item) {
            Some(item) => (
                "item.rename",
                json!({"item": item, "name": format!("n{name}")}),
            ),
            None => ("project.noop", json!({})),
        },
        Cmd::Toggle { item, enabled } => match pick(&all, item) {
            Some(item) => (
                "item.set_enabled",
                json!({"item": item, "enabled": enabled}),
            ),
            None => ("project.noop", json!({})),
        },
        Cmd::Delete { item } => match pick(&all, item) {
            Some(item) => ("item.delete", json!({"item": item})),
            None => ("project.noop", json!({})),
        },
        Cmd::RenameProject { name } => ("project.rename", json!({"name": format!("p{name}")})),
        Cmd::Fail => ("project.rename", json!({"name": "fail"})),
        Cmd::Noop => ("project.noop", json!({})),
        Cmd::Panic => ("project.explode", json!({})),
    }
}

/// Everything observable about the state, to check that failures change nothing.
fn observable(s: &DocState) -> (Rev, Rev, Project, Project, usize) {
    (
        s.rev(),
        s.committed_rev(),
        s.doc().clone(),
        s.committed_doc().clone(),
        s.transactions().len(),
    )
}

fn run_ops(ops: &[Op]) {
    quiet_expected_panics();
    let (initial, tracks, _) = project(3);
    let journal: Arc<Mutex<Vec<(Rev, Rev, Patch)>>> = Arc::default();
    let sink = {
        let journal = Arc::clone(&journal);
        move |r: &CommitRecord<'_>| {
            journal
                .lock()
                .unwrap()
                .push((r.prev_committed_rev, r.rev, r.patch.clone()));
            Ok(())
        }
    };
    let mut state = DocState::new(config(initial.clone()).with_journal(Box::new(sink)));
    let mut mirror = Mirror::new(&initial, Rev(0));
    let mut open: [Option<TxId>; 3] = [None; 3];
    let mut reconnects = [0_u64; 3];

    for op in ops {
        let before = observable(&state);
        let ctx = |conn: usize| session(conn, reconnects[conn]);
        let result: Result<(), String> = match op {
            Op::Run { conn, cmd } | Op::Step { conn, cmd } => {
                let (name, params) = request(cmd, state.doc(), &tracks);
                let mut submit = Submit::new(ctx(*conn), name, params);
                if let (Op::Step { .. }, Some(tx)) = (op, open[*conn]) {
                    submit = submit.in_tx(tx);
                }
                state.submit(submit).map(drop).map_err(|e| e.to_string())
            }
            Op::Begin { conn, kind } => state
                .begin_tx(&ctx(*conn), *kind, "prop")
                .map(|info| open[*conn] = Some(info.id))
                .map_err(|e| e.to_string()),
            Op::Commit { conn } => match open[*conn].take() {
                Some(tx) => state
                    .commit_tx(&ctx(*conn), tx)
                    .map(drop)
                    .map_err(|e| e.to_string()),
                None => Ok(()),
            },
            Op::Cancel { conn } => match open[*conn].take() {
                Some(tx) => state
                    .cancel_tx(&ctx(*conn), tx)
                    .map(drop)
                    .map_err(|e| e.to_string()),
                None => Ok(()),
            },
            Op::Drop { conn } => {
                open[*conn] = None;
                state.close_connection(ctx(*conn).conn());
                // The client reconnects with a new connection.
                reconnects[*conn] += 1;
                Ok(())
            }
        };
        // A failed request changes nothing (a failed commit closes its transaction).
        if result.is_err() && !matches!(op, Op::Commit { .. }) {
            assert_eq!(
                observable(&state),
                before,
                "{op:?} failed but changed the state"
            );
        }
        assert!(state.rev() >= before.0, "rev went backwards");
        assert!(state.committed_rev() <= state.rev());
        state
            .verify()
            .unwrap_or_else(|e| panic!("after {op:?}: {e}"));

        // Events replay to the snapshot at the current revision.
        let _ = state.take_changes();
        for event in state.take_events() {
            mirror.apply(&event);
        }
        assert_eq!(mirror.rev, state.rev());
        mirror.assert_matches(state.doc());
        if state.rev() == state.committed_rev() {
            assert_eq!(state.doc(), state.committed_doc());
        }
    }

    // The journal replays to the committed document, revision after revision.
    let journal = journal.lock().unwrap();
    let mut prev = Rev(0);
    for (prev_committed, rev, _) in journal.iter() {
        assert_eq!(*prev_committed, prev);
        assert!(rev > prev_committed);
        prev = *rev;
    }
    assert_eq!(prev, state.committed_rev());
    let patches: Vec<Patch> = journal.iter().map(|(.., p)| p.clone()).collect();
    assert_eq!(&replay(&initial, &patches), state.committed_doc());
}

proptest! {
    #![proptest_config(ProptestConfig { max_shrink_iters: 2_000, ..ProptestConfig::default() })]

    #[test]
    fn events_and_journal_replay_to_the_snapshots(ops in prop::collection::vec(arb_op(), 1..50)) {
        run_ops(&ops);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Through the actor: after every reply, the published snapshot at revision N equals the
    /// replay of the patches of events 1..N.
    #[test]
    fn actor_snapshots_equal_the_replay_of_their_events(
        cmds in prop::collection::vec((0..3_usize, arb_cmd()), 1..40),
    ) {
        quiet_expected_panics();
        let (initial, tracks, _) = project(3);
        let actor = spawn(initial.clone());
        let h = actor.handle();
        let mut events = h.subscribe();
        let mut mirror = Mirror::new(&initial, Rev(0));
        for (conn, cmd) in &cmds {
            let live = h.snapshot();
            let (name, params) = request(cmd, &live.doc, &tracks);
            let result = h.submit(Submit::new(ctx(*conn), name, params)).wait();
            let snapshot = h.snapshot();
            if let Ok(applied) = &result {
                prop_assert!(snapshot.rev >= applied.rev);
            } else {
                prop_assert_eq!(snapshot.rev, live.rev);
                prop_assert_eq!(&snapshot.doc, &live.doc);
            }
            while let Ok(event) = events.try_recv() {
                mirror.apply(&event);
            }
            prop_assert_eq!(mirror.rev, snapshot.rev);
            mirror.assert_matches(&snapshot.doc);
            prop_assert!(Arc::ptr_eq(&snapshot, &h.committed_snapshot()));
        }
    }
}
