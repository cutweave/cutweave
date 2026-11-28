// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.03: the document actor runs commands serially, publishes immutable snapshots that
//! readers load without locking, bumps the revision once per committed transaction, checks
//! `expected_rev`, broadcasts events in order, survives panicking commands and journals before
//! it acknowledges.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod doc_support;

use std::{
    future::Future,
    sync::{Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
    thread,
    time::Duration,
};

use cutweave_commands::{
    Actor, ActorCtx, ActorOptions, CHANGED_SINCE_REVS, ChangedSince, CmdError, CommitRecord,
    ConflictReason, DocActor, DocEvent, DocMetrics, Outcome, PatchCause, REDACTED, RecvError,
    RequestSample, Rev, Submit, TryRecvError,
};
use doc_support::{
    GATE, Mirror, agent, append, config, drain, item_name, next_patched, project, spawn, submit, ui,
};
use serde_json::json;

fn block_on<F: Future>(future: F) -> F::Output {
    struct Unpark(thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }
        thread::park();
    }
}

#[test]
fn submit_applies_publishes_then_replies_with_the_new_rev() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc.clone());
    let h = actor.handle();
    let mut events = h.subscribe();
    assert_eq!(h.snapshot().rev, Rev(0));

    let applied = submit(
        &h,
        &ui(),
        "timeline.append_clip",
        json!({"track": tracks[0], "frames": 30}),
    )
    .unwrap();
    assert_eq!((applied.rev, applied.committed_rev), (Rev(1), Rev(1)));
    assert!(applied.tx_id.is_some() && !applied.replayed);
    assert!(!applied.ops.is_empty());

    // Read-your-writes: the snapshot is published before the reply.
    let snapshot = h.snapshot();
    assert_eq!(snapshot.rev, Rev(1));
    assert!(snapshot.is_committed());
    assert_eq!(snapshot.doc.item_count(), 1);
    assert!(Arc::ptr_eq(&snapshot, &h.committed_snapshot()));

    let event = events.try_recv().unwrap();
    let DocEvent::Patched(p) = &*event else {
        panic!("{event:?}")
    };
    assert_eq!(
        (p.rev, p.prev_rev, p.committed_rev),
        (Rev(1), Rev(0), Rev(1))
    );
    assert_eq!(p.actor, Actor::User);
    assert_eq!(p.cause, PatchCause::Command);
    assert_eq!(p.request_id, Some(applied.request_id));
    assert_eq!(p.tx_id, applied.tx_id.unwrap());
    assert_eq!(p.ops, applied.ops);
    let mut mirror = Mirror::new(&doc, Rev(0));
    mirror.apply(&event);
    mirror.assert_matches(&snapshot.doc);
}

#[test]
fn rev_bumps_once_per_committed_change_only() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let mut events = h.subscribe();
    for expected in 1..=5 {
        let item = append(&h, &ui(), tracks[0], 10);
        assert_eq!(h.snapshot().rev, Rev(expected));
        assert!(h.snapshot().doc.find_item(&item).is_some());
    }
    // No-ops, queries and failures leave the revision alone and emit nothing.
    let noop = submit(&h, &ui(), "project.noop", json!({})).unwrap();
    assert_eq!((noop.rev, noop.ops.len()), (Rev(5), 0));
    let count = submit(&h, &ui(), "timeline.count_items", json!({})).unwrap();
    assert_eq!(count.output, json!({"count": 5}));
    assert_eq!(count.rev, Rev(5));
    let err = submit(&h, &ui(), "project.rename", json!({"name": "fail"})).unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert_eq!(h.snapshot().rev, Rev(5));
    assert_eq!(h.snapshot().doc.meta.name, "Doc actor fixture");
    let revs: Vec<Rev> = drain(&mut events).iter().filter_map(|e| e.rev()).collect();
    assert_eq!(revs, (1..=5).map(Rev).collect::<Vec<_>>());
}

#[test]
fn stale_expected_rev_is_a_conflict_with_the_current_rev() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    append(&h, &ui(), tracks[0], 10);
    let second = append(&h, &ui(), tracks[0], 10);

    let stale =
        Submit::new(agent(), "project.rename", json!({"name": "B"})).with_expected_rev(Rev(1));
    let err = h.submit(stale).wait().unwrap_err();
    let CmdError::Conflict {
        reason,
        current_rev,
        expected_rev,
        changed_since,
        ..
    } = &err
    else {
        panic!("{err:?}")
    };
    assert_eq!(*reason, ConflictReason::StaleRev);
    assert_eq!(*current_rev, Some(Rev(2)));
    // WP-2I.04: the conflict names the revision the caller expected and the items touched
    // since (the second clip; the first was there at rev 1).
    assert_eq!(*expected_rev, Some(Rev(1)));
    assert_eq!(*changed_since, Some(ChangedSince::Items(vec![second])));
    assert_eq!(
        serde_json::to_value(&err).unwrap()["changed_since"],
        json!({"items": [second]})
    );
    assert_eq!(h.snapshot().doc.meta.name, "Doc actor fixture");
    assert_eq!(h.snapshot().rev, Rev(2));

    let fresh =
        Submit::new(agent(), "project.rename", json!({"name": "B"})).with_expected_rev(Rev(2));
    assert_eq!(h.submit(fresh).wait().unwrap().rev, Rev(3));
    // A future revision is just as stale.
    let ahead =
        Submit::new(agent(), "project.rename", json!({"name": "C"})).with_expected_rev(Rev(9));
    let err = h.submit(ahead).wait().unwrap_err();
    assert!(
        matches!(
            &err,
            CmdError::Conflict {
                current_rev: Some(Rev(3)),
                expected_rev: Some(Rev(9)),
                changed_since: Some(ChangedSince::Unknown),
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(
        serde_json::to_value(&err).unwrap()["changed_since"],
        json!("unknown")
    );
}

/// WP-2I.04: `changed_since` covers the last 200 changes of the document the engine has open;
/// an older `expected_rev`, or one from before it was opened, gets `unknown`. A change that
/// touched no item (a rename) is an empty list. The other conflicts carry neither field.
#[test]
fn changed_since_is_bounded_and_unknown_beyond_what_is_kept() {
    let (doc, tracks, _) = project(1);
    let actor = DocActor::spawn(config(doc).with_rev(Rev(10))).unwrap();
    let h = actor.handle();
    let changed = |expected: u64| {
        let stale = Submit::new(agent(), "project.rename", json!({"name": "never"}))
            .with_expected_rev(Rev(expected));
        match h.submit(stale).wait().unwrap_err() {
            CmdError::Conflict { changed_since, .. } => changed_since,
            other => panic!("{other:?}"),
        }
    };
    let first = append(&h, &ui(), tracks[0], 10);
    submit(&h, &ui(), "project.rename", json!({"name": "B"})).unwrap();
    assert_eq!(h.snapshot().rev, Rev(12));
    assert_eq!(changed(10), Some(ChangedSince::Items(vec![first])));
    assert_eq!(changed(11), Some(ChangedSince::Items(vec![])));
    // Before this actor opened the document: nothing is known about those revisions.
    assert_eq!(changed(9), Some(ChangedSince::Unknown));

    for n in 0..CHANGED_SINCE_REVS {
        submit(
            &h,
            &ui(),
            "project.rename",
            json!({"name": format!("n{n}")}),
        )
        .unwrap();
    }
    let now = 12 + CHANGED_SINCE_REVS as u64;
    assert_eq!(h.snapshot().rev, Rev(now));
    assert_eq!(changed(now - 200), Some(ChangedSince::Items(vec![])));
    assert_eq!(changed(now - 201), Some(ChangedSince::Unknown));
    assert_eq!(changed(10), Some(ChangedSince::Unknown));
    // Nothing changed through any of that.
    assert_eq!(h.snapshot().rev, Rev(now));
}

#[test]
fn a_reader_holding_an_old_snapshot_never_sees_later_commits() {
    let (doc, tracks, _) = project(2);
    let actor = spawn(doc);
    let h = actor.handle();
    let item = append(&h, &ui(), tracks[0], 10);
    let old = h.snapshot();
    let frozen = serde_json::to_value(&old.doc).unwrap();
    let copy = old.doc.clone();

    submit(
        &h,
        &ui(),
        "item.rename",
        json!({"item": item, "name": "later"}),
    )
    .unwrap();
    submit(&h, &ui(), "project.rename", json!({"name": "Renamed"})).unwrap();
    submit(&h, &ui(), "item.delete", json!({"item": item})).unwrap();
    for _ in 0..50 {
        append(&h, &ui(), tracks[1], 1);
    }

    assert_eq!(old.rev, Rev(1));
    assert_eq!(old.doc, copy);
    assert_eq!(serde_json::to_value(&old.doc).unwrap(), frozen);
    assert_eq!(item_name(&old.doc, item), None);
    assert!(h.snapshot().doc.find_item(&item).is_none());
    assert_eq!(h.snapshot().rev, Rev(54));
}

#[test]
fn readers_never_block_while_a_command_runs() {
    let (doc, _, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let blocked = h.submit(Submit::new(ui(), "project.block", json!({})));
    GATE.wait_entered();

    // The actor is inside a command: snapshot loads and queries still return immediately,
    // and see the state before it.
    let reader = {
        let h = h.clone();
        thread::spawn(move || {
            for _ in 0..10_000 {
                let s = h.snapshot();
                assert_eq!(s.rev, Rev(0));
                assert_eq!(s.doc.meta.name, "Doc actor fixture");
            }
            h.query(&ui(), "timeline.count_items", json!({})).unwrap()
        })
    };
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || tx.send(reader.join()).unwrap());
    let counted = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("readers blocked behind the writer")
        .unwrap();
    assert_eq!(counted, json!({"count": 0}));
    assert!(!blocked.is_ready());

    GATE.release();
    assert_eq!(blocked.wait().unwrap().rev, Rev(1));
    assert_eq!(h.snapshot().doc.meta.name, "blocked");
}

#[test]
fn a_panicking_command_is_rolled_back_and_the_actor_keeps_running() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let mut events = h.subscribe();
    let err = submit(&h, &ui(), "project.explode", json!({})).unwrap_err();
    assert_eq!(err.code(), "internal");
    assert!(err.message().contains("boom"), "{err}");
    let snapshot = h.snapshot();
    assert_eq!(snapshot.rev, Rev(0));
    assert_eq!(snapshot.doc.meta.name, "Doc actor fixture");

    let event = events.try_recv().unwrap();
    let DocEvent::CommandPanicked(p) = &*event else {
        panic!("{event:?}")
    };
    assert_eq!(p.command, "project.explode");
    assert_eq!(p.actor, Actor::User);
    assert_eq!(p.rev, Rev(0));
    assert!(p.message.contains("boom"));

    // Still serving.
    append(&h, &ui(), tracks[0], 1);
    assert_eq!(h.snapshot().rev, Rev(1));
    let stats = h.stats();
    assert_eq!(stats.panicked, 1);
    assert_eq!((stats.applied, stats.failed), (1, 1));
}

#[test]
fn panic_messages_of_commands_with_secrets_are_withheld() {
    let (doc, _, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let mut events = h.subscribe();
    let err = submit(
        &h,
        &ui(),
        "ai.set_key",
        json!({"provider": "panic", "key": "sk-live-secret"}),
    )
    .unwrap_err();
    assert!(!err.message().contains("sk-live-secret"), "{err}");
    let event = events.try_recv().unwrap();
    let DocEvent::CommandPanicked(p) = &*event else {
        panic!("{event:?}")
    };
    assert!(!p.message.contains("sk-live-secret"), "{}", p.message);
}

#[test]
fn the_actor_and_scopes_come_from_the_credential() {
    let (doc, _, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let mut events = h.subscribe();
    submit(&h, &agent(), "project.rename", json!({"name": "By agent"})).unwrap();
    // (`edit.history_changed` events are interleaved; only `doc.patched` matters here.)
    assert_eq!(next_patched(&mut events).actor.to_string(), "agent:claude");
    let plugin = ActorCtx::plugin(cutweave_commands::ConnId::new(5), "acme.tools").unwrap();
    submit(&h, &plugin, "project.rename", json!({"name": "By plugin"})).unwrap();
    assert_eq!(
        next_patched(&mut events).actor,
        Actor::plugin("acme.tools").unwrap()
    );
    submit(
        &h,
        &ActorCtx::system(),
        "project.rename",
        json!({"name": "Sys"}),
    )
    .unwrap();
    assert_eq!(next_patched(&mut events).actor, Actor::System);

    // UI-only scopes are refused for agents before anything runs.
    let err = submit(
        &h,
        &agent(),
        "ai.set_key",
        json!({"provider": "x", "key": "k"}),
    );
    assert_eq!(err.unwrap_err().code(), "forbidden");
    submit(
        &h,
        &ui(),
        "ai.set_key",
        json!({"provider": "x", "key": "k"}),
    )
    .unwrap();
}

#[test]
fn unknown_and_malformed_requests_fail_cleanly() {
    let (doc, _, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let err = submit(&h, &ui(), "clip.nope", json!({})).unwrap_err();
    assert_eq!(err.code(), "not_found");
    let err = submit(&h, &ui(), "project.rename", json!({"name": 3})).unwrap_err();
    assert_eq!(
        err,
        CmdError::Invalid {
            path: "/name".into(),
            msg: "invalid type: integer `3`, expected a string".into()
        }
    );
    let err = submit(
        &h,
        &ui(),
        "item.rename",
        json!({"item": doc_support::stray_item(), "name": "x"}),
    )
    .unwrap_err();
    assert_eq!(err.code(), "not_found");
    assert_eq!(h.snapshot().rev, Rev(0));
}

#[test]
fn engine_commands_run_against_the_doc_state() {
    let (doc, _, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let opened = submit(
        &h,
        &ui(),
        "edit.begin_tx",
        json!({"kind": "gesture", "label": "Drag"}),
    )
    .unwrap();
    assert!(opened.tx_id.is_none());
    let tx: cutweave_commands::TxId = serde_json::from_value(opened.output["tx"].clone()).unwrap();
    let open = h.run(|state| state.transactions()).wait().unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, tx);
    assert_eq!(open[0].actor, Actor::User);
    assert_eq!(open[0].label, "Drag");
    // A second one on the same connection is refused.
    let err = submit(
        &h,
        &ui(),
        "edit.begin_tx",
        json!({"kind": "batch", "label": "x"}),
    );
    assert_eq!(err.unwrap_err().code(), "invalid");

    // Engine commands of other contexts do not run here.
    let err = submit(&h, &ui(), "view.set_playhead", json!({"at": 0})).unwrap_err();
    assert_eq!(err.code(), "unsupported");
}

#[test]
fn the_journal_sees_every_commit_before_it_is_acknowledged() {
    let (doc, tracks, _) = project(1);
    let journal: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let sink = {
        let journal = Arc::clone(&journal);
        move |record: &CommitRecord<'_>| {
            journal
                .lock()
                .unwrap()
                .push(serde_json::to_value(record).unwrap());
            Ok(())
        }
    };
    let actor = DocActor::spawn(config(doc).with_journal(Box::new(sink))).unwrap();
    let h = actor.handle();
    let applied = submit(
        &h,
        &ui(),
        "timeline.append_clip",
        json!({"track": tracks[0], "frames": 5}),
    )
    .unwrap();
    submit(&h, &ui(), "project.noop", json!({})).unwrap();
    submit(
        &h,
        &ui(),
        "ai.set_key",
        json!({"provider": "p", "key": "sk-1"}),
    )
    .unwrap();

    let journal = journal.lock().unwrap();
    // The no-op and the key (no document change) are not journaled.
    assert_eq!(journal.len(), 1);
    let record = &journal[0];
    assert_eq!(record["rev"], json!(1));
    assert_eq!(record["prev_committed_rev"], json!(0));
    assert_eq!(record["actor"], json!("user"));
    assert_eq!(record["kind"], json!(null));
    assert_eq!(record["patch"], serde_json::to_value(&applied.ops).unwrap());
    let step = &record["steps"][0];
    assert_eq!(step["command"], json!("timeline.append_clip"));
    assert_eq!(
        step["record"]["applied"]["request_id"],
        json!(applied.request_id.to_string())
    );
    assert_eq!(step["record"]["applied"]["output"], applied.output);
}

#[test]
fn journal_params_are_redacted() {
    let (doc, _, _) = project(1);
    let journal: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let sink = {
        let journal = Arc::clone(&journal);
        move |record: &CommitRecord<'_>| {
            journal
                .lock()
                .unwrap()
                .extend(record.steps.iter().map(|s| s.params.clone()));
            Ok(())
        }
    };
    let actor = DocActor::spawn(config(doc).with_journal(Box::new(sink))).unwrap();
    let h = actor.handle();
    // A batch containing a secret-bearing command plus a real change.
    let ctx = ui();
    let tx = h
        .begin_tx(&ctx, cutweave_commands::TxKind::Batch, "keys")
        .wait()
        .unwrap()
        .id;
    h.submit(
        Submit::new(
            ctx.clone(),
            "ai.set_key",
            json!({"provider": "p", "key": "sk-2"}),
        )
        .in_tx(tx),
    )
    .wait()
    .unwrap();
    h.submit(Submit::new(ctx.clone(), "project.rename", json!({"name": "K"})).in_tx(tx))
        .wait()
        .unwrap();
    h.commit_tx(&ctx, tx).wait().unwrap();
    let journal = journal.lock().unwrap();
    assert_eq!(journal[0], json!({"provider": "p", "key": REDACTED}));
    assert_eq!(journal[1], json!({"name": "K"}));
}

#[test]
fn a_failing_journal_aborts_the_commit() {
    let (doc, tracks, _) = project(1);
    let sink = |_: &CommitRecord<'_>| Err(CmdError::internal("disk full"));
    let actor = DocActor::spawn(config(doc).with_journal(Box::new(sink))).unwrap();
    let h = actor.handle();
    let mut events = h.subscribe();
    let err = submit(
        &h,
        &ui(),
        "timeline.append_clip",
        json!({"track": tracks[0], "frames": 5}),
    )
    .unwrap_err();
    assert_eq!(err, CmdError::internal("disk full"));
    assert_eq!(h.snapshot().rev, Rev(0));
    assert_eq!(h.snapshot().doc.item_count(), 0);
    assert_eq!(events.try_recv().unwrap_err(), TryRecvError::Empty);
}

#[test]
fn shutdown_answers_queued_requests_then_stops() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let mut events = h.subscribe();
    let pending: Vec<_> = (0..20)
        .map(|_| {
            h.submit(Submit::new(
                ui(),
                "timeline.append_clip",
                json!({"track": tracks[0], "frames": 1}),
            ))
        })
        .collect();
    actor.shutdown().unwrap();
    for (i, p) in pending.into_iter().enumerate() {
        assert_eq!(p.wait().unwrap().rev, Rev(i as u64 + 1));
    }
    // Surviving handles: snapshots stay readable, requests fail, streams close after draining.
    assert_eq!(h.snapshot().rev, Rev(20));
    let err = submit(&h, &ui(), "project.noop", json!({})).unwrap_err();
    assert_eq!(err.code(), "internal");
    assert_eq!(drain_until_closed(&mut events), 20);
}

/// The number of `doc.patched` events until the stream closes.
fn drain_until_closed(events: &mut cutweave_commands::EventStream) -> usize {
    let mut n = 0;
    loop {
        match events.recv_blocking() {
            Ok(event) => n += usize::from(event.as_patched().is_some()),
            Err(RecvError::Closed) => return n,
            Err(e) => panic!("{e}"),
        }
    }
}

#[test]
fn replies_and_events_are_futures_usable_from_any_executor() {
    let (doc, tracks, _) = project(1);
    let actor = spawn(doc);
    let h = actor.handle();
    let mut events = h.subscribe();
    let applied = block_on(h.submit(Submit::new(
        ui(),
        "timeline.append_clip",
        json!({"track": tracks[0], "frames": 1}),
    )))
    .unwrap();
    let event = block_on(events.recv()).unwrap();
    assert_eq!(event.rev(), Some(applied.rev));
    // An event published while the receiver waits wakes it (skipping the first commit's
    // `edit.history_changed`, which has no revision).
    let waiter = thread::spawn(move || {
        loop {
            if let Some(rev) = block_on(events.recv()).unwrap().rev() {
                break Some(rev);
            }
        }
    });
    thread::sleep(Duration::from_millis(20));
    append(&h, &ui(), tracks[0], 1);
    assert_eq!(waiter.join().unwrap(), Some(Rev(2)));
}

#[derive(Default)]
struct Recorder {
    samples: Mutex<Vec<(String, Outcome)>>,
    panics: Mutex<Vec<String>>,
}

impl DocMetrics for Recorder {
    fn request(&self, sample: &RequestSample<'_>) {
        assert!(sample.exec < Duration::from_secs(5));
        self.samples
            .lock()
            .unwrap()
            .push((sample.name.to_owned(), sample.outcome));
    }

    fn command_panicked(&self, command: &str) {
        self.panics.lock().unwrap().push(command.to_owned());
    }
}

#[test]
fn metrics_hooks_see_every_request() {
    let (doc, tracks, _) = project(1);
    let recorder = Arc::new(Recorder::default());
    let actor = DocActor::spawn_with(
        config(doc),
        ActorOptions {
            metrics: recorder.clone(),
            ..ActorOptions::default()
        },
    )
    .unwrap();
    let h = actor.handle();
    append(&h, &ui(), tracks[0], 1);
    let id = cutweave_commands::RequestId::from_u128(77);
    let again = || {
        h.submit(Submit::new(ui(), "project.rename", json!({"name": "R"})).with_request_id(id))
            .wait()
            .unwrap()
    };
    again();
    again();
    let _ = submit(&h, &ui(), "project.explode", json!({}));
    h.run(|_| ()).wait().unwrap();
    drop(h);
    actor.shutdown().unwrap();

    let samples = recorder.samples.lock().unwrap();
    let got: Vec<(&str, Outcome)> = samples.iter().map(|(n, o)| (n.as_str(), *o)).collect();
    assert_eq!(
        got,
        vec![
            ("timeline.append_clip", Outcome::Applied),
            ("project.rename", Outcome::Applied),
            ("project.rename", Outcome::Replayed),
            ("project.explode", Outcome::Failed { code: "internal" }),
            ("run", Outcome::Applied),
        ]
    );
    assert_eq!(*recorder.panics.lock().unwrap(), vec!["project.explode"]);
}

#[test]
fn handles_streams_and_replies_cross_threads() {
    fn send_sync<T: Send + Sync>() {}
    fn send<T: Send>() {}
    send_sync::<cutweave_commands::DocHandle>();
    send_sync::<cutweave_commands::DocActor>();
    send::<cutweave_commands::EventStream>();
    send::<cutweave_commands::Pending<cutweave_commands::Applied>>();
    send::<cutweave_commands::DocState>();
    send_sync::<cutweave_commands::DocSnapshot>();
}
