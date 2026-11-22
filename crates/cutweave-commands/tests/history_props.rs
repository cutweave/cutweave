// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.04 property tests of the undo history:
//!
//! - one actor: undoing every entry walks back through each committed document to the initial
//!   one, and redoing walks forward to the final one (`undo ∘ apply = identity`);
//! - interleaved actors on disjoint items: each actor's entries undo without conflicts and
//!   restore exactly its own part, leaving the others' alone; redo restores the final document;
//! - random interleavings of commands, transactions, undo/redo (own, another actor's, anyone's)
//!   and checkpoint restores from three connections never corrupt anything: a refused request
//!   changes nothing, the document stays valid, events replay to the snapshots, the journal
//!   replays to the committed document, and a history rebuilt from the journal is identical;
//! - checkpoint, more edits, restore: the checkpointed document; undo: the pre-restore one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod doc_support;

use std::sync::{Arc, Mutex};

use cutweave_commands::{
    ActorCtx, CmdError, CommitRecord, ConnId, DocState, JournaledCommit, Rev, Submit, TxId, TxKind,
};
use cutweave_model::{ItemId, Project, Track, TrackId};
use doc_support::{Mirror, config, items, project, replay};
use proptest::prelude::*;
use serde_json::{Value, json};

#[derive(Clone, Debug)]
enum Cmd {
    Append { track: usize, frames: u32 },
    Rename { item: usize, name: u8 },
    Toggle { item: usize, enabled: bool },
    Delete { item: usize },
    RenameProject { name: u8 },
}

fn arb_cmd() -> impl Strategy<Value = Cmd> {
    prop_oneof![
        4 => (0..3_usize, 1..20_u32).prop_map(|(track, frames)| Cmd::Append { track, frames }),
        3 => (any::<usize>(), any::<u8>()).prop_map(|(item, name)| Cmd::Rename { item, name }),
        2 => (any::<usize>(), any::<bool>()).prop_map(|(item, enabled)| Cmd::Toggle { item, enabled }),
        1 => any::<usize>().prop_map(|item| Cmd::Delete { item }),
        1 => any::<u8>().prop_map(|name| Cmd::RenameProject { name }),
    ]
}

/// The request for `cmd`, its items drawn from `pool` (appends go to `tracks[track]`).
fn request(cmd: &Cmd, pool: &[ItemId], tracks: &[TrackId]) -> (&'static str, Value) {
    let pick = |i: usize| pool[i % pool.len()];
    match *cmd {
        Cmd::Append { track, frames } => (
            "timeline.append_clip",
            json!({"track": tracks[track % tracks.len()], "frames": frames}),
        ),
        _ if pool.is_empty() => ("project.noop", json!({})),
        Cmd::Rename { item, name } => (
            "item.rename",
            json!({"item": pick(item), "name": format!("n{name}")}),
        ),
        Cmd::Toggle { item, enabled } => (
            "item.set_enabled",
            json!({"item": pick(item), "enabled": enabled}),
        ),
        Cmd::Delete { item } => ("item.delete", json!({"item": pick(item)})),
        Cmd::RenameProject { name } => ("project.rename", json!({"name": format!("p{name}")})),
    }
}

fn ctx(conn: usize) -> ActorCtx {
    match conn {
        0 => ActorCtx::ui(ConnId::new(1)),
        1 => ActorCtx::agent(ConnId::new(2), "claude").unwrap(),
        _ => ActorCtx::plugin(ConnId::new(3), "acme.tools").unwrap(),
    }
}

fn run(s: &mut DocState, ctx: &ActorCtx, name: &str, params: Value) -> Result<Value, CmdError> {
    s.submit(Submit::new(ctx.clone(), name, params))
        .map(|a| a.output)
}

fn track(doc: &Project, id: TrackId) -> Track {
    doc.sequences
        .values()
        .find_map(|s| s.track(&id))
        .unwrap()
        .clone()
}

fn track_items(doc: &Project, id: TrackId) -> Vec<ItemId> {
    track(doc, id)
        .items
        .iter_by_id()
        .map(|(id, _)| *id)
        .collect()
}

// ----- one actor ----------------------------------------------------------------------------

fn undo_redo_round_trip(cmds: &[Cmd]) {
    let (initial, tracks, _) = project(3);
    let mut s = DocState::new(config(initial.clone()));
    let u = ctx(0);
    let mut docs = vec![initial.clone()];
    for cmd in cmds {
        let (name, params) = request(cmd, &items(s.committed_doc()), &tracks);
        let applied = s.submit(Submit::new(u.clone(), name, params)).unwrap();
        if !applied.ops.is_empty() {
            docs.push(s.committed_doc().clone());
        }
    }
    for i in (0..docs.len() - 1).rev() {
        run(&mut s, &u, "edit.undo", json!({})).unwrap();
        assert_eq!(s.committed_doc(), &docs[i], "undo to entry {i}");
    }
    assert_eq!(s.committed_doc(), &initial);
    assert_eq!(
        run(&mut s, &u, "edit.undo", json!({})).unwrap_err().code(),
        "invalid"
    );
    for (i, doc) in docs.iter().enumerate().skip(1) {
        run(&mut s, &u, "edit.redo", json!({})).unwrap();
        assert_eq!(s.committed_doc(), doc, "redo to entry {i}");
    }
    assert_eq!(
        run(&mut s, &u, "edit.redo", json!({})).unwrap_err().code(),
        "invalid"
    );
    s.verify().unwrap();
}

// ----- actors on disjoint items ---------------------------------------------------------------

fn disjoint_actors(ops: &[(usize, Cmd)], order: &[usize]) {
    let (initial, tracks, _) = project(3);
    let mut s = DocState::new(config(initial.clone()));
    for (actor, cmd) in ops {
        // Each actor only touches its own track (and never the project's own fields).
        let cmd = match cmd {
            Cmd::Append { frames, .. } => Cmd::Append {
                track: *actor,
                frames: *frames,
            },
            Cmd::RenameProject { name } => Cmd::Rename {
                item: usize::from(*name),
                name: *name,
            },
            other => other.clone(),
        };
        let own = track_items(s.committed_doc(), tracks[*actor]);
        let (name, params) = request(&cmd, &own, &tracks);
        s.submit(Submit::new(ctx(*actor), name, params)).unwrap();
    }
    let last = s.committed_doc().clone();

    // Undo every entry of one actor after the other: never a conflict, and only its own track
    // goes back to the initial one.
    for &actor in order {
        let others: Vec<Track> = (0..3)
            .filter(|a| *a != actor)
            .map(|a| track(s.committed_doc(), tracks[a]))
            .collect();
        loop {
            match run(&mut s, &ctx(actor), "edit.undo", json!({})) {
                Ok(_) => {}
                Err(e) => {
                    assert_eq!(e.code(), "invalid", "{e}");
                    break;
                }
            }
        }
        assert_eq!(
            track(s.committed_doc(), tracks[actor]),
            track(&initial, tracks[actor])
        );
        let after: Vec<Track> = (0..3)
            .filter(|a| *a != actor)
            .map(|a| track(s.committed_doc(), tracks[a]))
            .collect();
        assert_eq!(
            after, others,
            "undoing {actor} touched another actor's track"
        );
    }
    assert_eq!(s.committed_doc(), &initial);
    // Redo everything, actors in the reverse order: the final document again.
    for &actor in order.iter().rev() {
        while run(&mut s, &ctx(actor), "edit.redo", json!({})).is_ok() {}
    }
    assert_eq!(s.committed_doc(), &last);
    s.verify().unwrap();
}

// ----- random interleavings ------------------------------------------------------------------

/// Whose history an undo/redo targets.
#[derive(Clone, Copy, Debug)]
enum Whose {
    Own,
    Other(usize),
    Any,
}

#[derive(Clone, Debug)]
enum Op {
    Run {
        conn: usize,
        cmd: Cmd,
    },
    Begin {
        conn: usize,
        kind: TxKind,
    },
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
    Undo {
        conn: usize,
        whose: Whose,
    },
    Redo {
        conn: usize,
        whose: Whose,
    },
    Checkpoint {
        conn: usize,
    },
    Restore {
        conn: usize,
        back: usize,
        force: bool,
    },
}

fn arb_whose() -> impl Strategy<Value = Whose> {
    prop_oneof![
        4 => Just(Whose::Own),
        2 => (0..3_usize).prop_map(Whose::Other),
        1 => Just(Whose::Any),
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
        6 => (conn.clone(), arb_cmd()).prop_map(|(conn, cmd)| Op::Run { conn, cmd }),
        1 => (conn.clone(), kind).prop_map(|(conn, kind)| Op::Begin { conn, kind }),
        3 => (conn.clone(), arb_cmd()).prop_map(|(conn, cmd)| Op::Step { conn, cmd }),
        1 => conn.clone().prop_map(|conn| Op::Commit { conn }),
        1 => conn.clone().prop_map(|conn| Op::Cancel { conn }),
        4 => (conn.clone(), arb_whose()).prop_map(|(conn, whose)| Op::Undo { conn, whose }),
        2 => (conn.clone(), arb_whose()).prop_map(|(conn, whose)| Op::Redo { conn, whose }),
        1 => conn.clone().prop_map(|conn| Op::Checkpoint { conn }),
        1 => (conn, any::<usize>(), any::<bool>())
            .prop_map(|(conn, back, force)| Op::Restore { conn, back, force }),
    ]
}

fn whose(w: Whose) -> Value {
    match w {
        Whose::Own => json!({}),
        Whose::Other(other) => json!({"actor": ctx(other).actor().to_string()}),
        Whose::Any => json!({"actor": "*"}),
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

#[allow(clippy::too_many_lines)]
fn interleavings(ops: &[Op]) {
    let (initial, tracks, _) = project(3);
    let journal: Arc<Mutex<Vec<JournaledCommit>>> = Arc::default();
    let sink = {
        let journal = Arc::clone(&journal);
        move |r: &CommitRecord<'_>| {
            journal.lock().unwrap().push(r.to_owned());
            Ok(())
        }
    };
    let mut s = DocState::new(config(initial.clone()).with_journal(Box::new(sink)));
    let mut mirror = Mirror::new(&initial, Rev(0));
    let mut open: [Option<TxId>; 3] = [None; 3];
    let mut checkpoints: Vec<String> = Vec::new();

    for op in ops {
        let before = observable(&s);
        let pool = items(s.doc());
        let result: Result<(), CmdError> = match op {
            Op::Run { conn, cmd } | Op::Step { conn, cmd } => {
                let (name, params) = request(cmd, &pool, &tracks);
                let mut submit = Submit::new(ctx(*conn), name, params);
                if let (Op::Step { .. }, Some(tx)) = (op, open[*conn]) {
                    submit = submit.in_tx(tx);
                }
                s.submit(submit).map(drop)
            }
            Op::Begin { conn, kind } => s
                .begin_tx(&ctx(*conn), *kind, "prop")
                .map(|info| open[*conn] = Some(info.id)),
            Op::Commit { conn } => match open[*conn].take() {
                Some(tx) => run(&mut s, &ctx(*conn), "edit.commit_tx", json!({"tx": tx})).map(drop),
                None => Ok(()),
            },
            Op::Cancel { conn } => match open[*conn].take() {
                Some(tx) => run(&mut s, &ctx(*conn), "edit.cancel_tx", json!({"tx": tx})).map(drop),
                None => Ok(()),
            },
            Op::Undo { conn, whose: w } => {
                run(&mut s, &ctx(*conn), "edit.undo", whose(*w)).map(drop)
            }
            Op::Redo { conn, whose: w } => {
                run(&mut s, &ctx(*conn), "edit.redo", whose(*w)).map(drop)
            }
            Op::Checkpoint { conn } => run(&mut s, &ctx(*conn), "edit.checkpoint", json!({}))
                .map(|out| checkpoints.push(out["checkpoint"].as_str().unwrap().to_owned())),
            Op::Restore { conn, back, force } => match checkpoints.len() {
                0 => Ok(()),
                n => run(
                    &mut s,
                    &ctx(*conn),
                    "edit.restore",
                    json!({"checkpoint": checkpoints[n - 1 - back % n], "force": force}),
                )
                .map(drop),
            },
        };
        if let Err(e) = &result {
            assert!(
                matches!(e.code(), "conflict" | "invalid" | "not_found" | "locked"),
                "{op:?}: unexpected {e}"
            );
            // A refused request changes nothing (a failed commit closes its transaction).
            if !matches!(op, Op::Commit { .. }) {
                assert_eq!(
                    observable(&s),
                    before,
                    "{op:?} failed but changed the state"
                );
            }
        }
        s.verify().unwrap_or_else(|e| panic!("after {op:?}: {e}"));
        // Never a corrupt committed document. (The live one may transiently show an open
        // transaction's changes colliding with newer commits; it cannot commit them.)
        s.committed_doc().validate().unwrap();
        let _ = s.take_changes();
        for event in s.take_events() {
            mirror.apply(&event);
        }
        assert_eq!(mirror.rev, s.rev());
        mirror.assert_matches(s.doc());
    }

    // The journal replays to the committed document ...
    let commits = journal.lock().unwrap().clone();
    let patches: Vec<_> = commits.iter().map(|c| c.patch.clone()).collect();
    assert_eq!(&replay(&initial, &patches), s.committed_doc());
    // ... and rebuilds the same history.
    let rebuilt = DocState::new(
        config(s.committed_doc().clone())
            .with_rev(s.committed_rev())
            .with_recovered_history(commits),
    );
    rebuilt.verify().unwrap();
    let ours = s.history_page(500, None, None).unwrap();
    let theirs = rebuilt.history_page(500, None, None).unwrap();
    assert_eq!(ours.entries, theirs.entries);
    assert_eq!(ours.stacks, theirs.stacks);
}

// ----- checkpoints ---------------------------------------------------------------------------

fn checkpoint_round_trip(before: &[Cmd], after: &[Cmd]) {
    let (initial, tracks, _) = project(3);
    let mut s = DocState::new(config(initial));
    let u = ctx(0);
    let apply = |s: &mut DocState, cmds: &[Cmd]| {
        for cmd in cmds {
            let (name, params) = request(cmd, &items(s.committed_doc()), &tracks);
            s.submit(Submit::new(u.clone(), name, params)).unwrap();
        }
    };
    apply(&mut s, before);
    let cp = run(&mut s, &u, "edit.checkpoint", json!({"name": "cp"})).unwrap();
    let at_checkpoint = s.committed_doc().clone();
    apply(&mut s, after);
    let before_restore = s.committed_doc().clone();
    let restored = run(&mut s, &u, "edit.restore", json!({"checkpoint": "cp"})).unwrap();
    assert_eq!(s.committed_doc(), &at_checkpoint);
    if before_restore == at_checkpoint {
        assert_eq!(restored["tx"], Value::Null);
        return;
    }
    assert_eq!(restored["checkpoint"], cp["checkpoint"]);
    run(&mut s, &u, "edit.undo", json!({})).unwrap();
    assert_eq!(s.committed_doc(), &before_restore);
    run(&mut s, &u, "edit.redo", json!({})).unwrap();
    assert_eq!(s.committed_doc(), &at_checkpoint);
    s.verify().unwrap();
}

proptest! {
    #![proptest_config(ProptestConfig { max_shrink_iters: 2_000, ..ProptestConfig::default() })]

    #[test]
    fn undo_all_then_redo_all_round_trips_one_actors_history(
        cmds in prop::collection::vec(arb_cmd(), 1..50),
    ) {
        undo_redo_round_trip(&cmds);
    }

    #[test]
    fn interleaved_actors_on_disjoint_items_undo_independently(
        ops in prop::collection::vec((0..3_usize, arb_cmd()), 1..60),
        order in prop::sample::select(vec![
            [0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0],
        ]),
    ) {
        disjoint_actors(&ops, &order);
    }

    #[test]
    fn random_interleavings_with_undo_and_redo_never_corrupt_anything(
        ops in prop::collection::vec(arb_op(), 1..60),
    ) {
        interleavings(&ops);
    }

    #[test]
    fn checkpoint_then_restore_round_trips(
        before in prop::collection::vec(arb_cmd(), 0..20),
        after in prop::collection::vec(arb_cmd(), 1..20),
    ) {
        checkpoint_round_trip(&before, &after);
    }
}
