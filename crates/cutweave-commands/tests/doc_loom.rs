// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.03: loom model checks of the actor's submit → publish → reply path. Built only with
//! `--cfg cutweave_loom`, which swaps the actor host's primitives for loom's (see
//! `src/doc/sync.rs`):
//!
//! ```text
//! RUSTFLAGS="--cfg cutweave_loom" cargo test -p cutweave-commands --release --test doc_loom
//! ```
//!
//! Every interleaving (up to the preemption bound) of two submitters, a snapshot reader, an
//! event subscriber and the actor thread is explored.

#![cfg(cutweave_loom)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod doc_support;

use cutweave_commands::{DocActor, Rev, Submit, TxKind};
use doc_support::{agent, config, project, ui};
use serde_json::json;

fn model(f: impl Fn() + Sync + Send + 'static) {
    let mut builder = loom::model::Builder::new();
    if builder.preemption_bound.is_none() {
        builder.preemption_bound = Some(2);
    }
    builder.check(f);
}

/// Two submitters and a reader: replies come after publication (read-your-writes), the
/// reader's revisions never go backwards, revisions are 1 and 2 in some order, and the events
/// arrive in revision order.
#[test]
fn submit_publish_reply_is_linearizable() {
    model(|| {
        let (doc, _, _) = project(1);
        let actor = DocActor::spawn(config(doc)).unwrap();
        let mut events = actor.handle().subscribe();
        let writers: Vec<_> = [(ui(), "A"), (agent(), "B")]
            .into_iter()
            .map(|(ctx, name)| {
                let h = actor.handle();
                loom::thread::spawn(move || {
                    let applied = h
                        .submit(Submit::new(ctx, "project.rename", json!({"name": name})))
                        .wait()
                        .unwrap();
                    let seen = h.snapshot();
                    assert!(seen.rev >= applied.rev, "reply before publication");
                    (applied.rev, name)
                })
            })
            .collect();

        let reader = actor.handle();
        let first = reader.snapshot();
        let second = reader.snapshot();
        assert!(second.rev >= first.rev);
        assert!(second.committed_rev <= second.rev);

        let mut results: Vec<(Rev, &str)> =
            writers.into_iter().map(|w| w.join().unwrap()).collect();
        results.sort();
        assert_eq!(
            results.iter().map(|r| r.0).collect::<Vec<_>>(),
            [Rev(1), Rev(2)]
        );
        let last = reader.snapshot();
        assert_eq!(last.rev, Rev(2));
        assert_eq!(last.doc.meta.name, results[1].1);

        // Every event was published before its reply; `edit.history_changed` events (no
        // revision) are interleaved with the document's.
        let revs: Vec<Rev> = std::iter::from_fn(|| events.try_recv().ok())
            .filter_map(|e| e.rev())
            .collect();
        assert_eq!(revs, [Rev(1), Rev(2)]);
        drop(events);
        actor.shutdown().unwrap();
    });
}

/// A reply awaited as a future from another thread is never lost: the waker registered by
/// `poll` races with the actor filling the reply.
#[test]
fn an_awaited_reply_always_wakes_its_task() {
    model(|| {
        let (doc, _, _) = project(1);
        let actor = DocActor::spawn(config(doc)).unwrap();
        let h = actor.handle();
        let waiter = loom::thread::spawn(move || {
            let pending = h.submit(Submit::new(ui(), "project.rename", json!({"name": "A"})));
            loom::future::block_on(pending).unwrap().rev
        });
        assert_eq!(waiter.join().unwrap(), Rev(1));
        actor.shutdown().unwrap();
    });
}

/// While a gesture is open the actor waits for requests with a timeout (the idle-gesture
/// timer, WP-1B.04); a request or a shutdown racing that timed wait is never lost.
#[test]
fn requests_reach_an_actor_waiting_on_the_gesture_timer() {
    model(|| {
        let (doc, _, _) = project(1);
        let actor = DocActor::spawn(config(doc)).unwrap();
        let h = actor.handle();
        let tx = h
            .begin_tx(&ui(), TxKind::Gesture, "Drag")
            .wait()
            .unwrap()
            .id;
        let client = {
            let h = actor.handle();
            loom::thread::spawn(move || {
                h.submit(Submit::new(agent(), "project.rename", json!({"name": "B"})))
                    .wait()
                    .unwrap()
                    .rev
            })
        };
        assert_eq!(client.join().unwrap(), Rev(1));
        h.cancel_tx(&ui(), tx).wait().unwrap();
        actor.shutdown().unwrap();
    });
}

/// Shutting down while a request is in flight answers it either with its result or with
/// "actor stopped", never hangs.
#[test]
fn shutdown_never_strands_a_request() {
    model(|| {
        let (doc, _, _) = project(1);
        let actor = DocActor::spawn(config(doc)).unwrap();
        let h = actor.handle();
        let client = loom::thread::spawn(move || {
            h.submit(Submit::new(ui(), "project.noop", json!({})))
                .wait()
                .map(|a| a.rev)
        });
        actor.shutdown().unwrap();
        match client.join().unwrap() {
            Ok(rev) => assert_eq!(rev, Rev(0)),
            Err(e) => assert_eq!(e.code(), "internal"),
        }
    });
}
