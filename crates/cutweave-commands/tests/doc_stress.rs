// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.03 concurrency acceptance: many submitters, readers that never block, monotonic
//! revisions, events in order, and a final document equal to a sequential replay of the
//! recorded order. Every test runs under a deadlock watchdog: 30 s without progress fails
//! (never hangs).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod doc_support;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use cutweave_commands::{
    ActorCtx, Applied, CommitRecord, ConnId, DocActor, DocState, Patch, RecvError, Rev, Submit,
    TxKind,
};
use cutweave_model::ItemId;
use doc_support::{Mirror, config, drain, project, replay, spawn_with_capacity};
use serde_json::{Value, json};

const WRITERS: usize = 8;
const COMMANDS: usize = 10_000;

/// Completed steps, bumped by the test body (every reply, replayed command and event).
#[derive(Default)]
struct Progress(AtomicU64);

impl Progress {
    fn tick(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// No progress for this long is a deadlock.
const STALL: Duration = Duration::from_secs(30);
/// A run this long has gone wrong even if it moves.
const CAP: Duration = Duration::from_mins(5);

/// Runs `body` on its own thread and fails if it stops making progress for 30 s (a deadlock)
/// or runs past 5 minutes. Unoptimized test builds run commands ~10× slower than release (the
/// 10,000-command test takes ~2 s in release, 10-20 s in debug depending on load), so total
/// wall time alone would be a flaky deadlock signal.
fn watchdog<T: Send + 'static>(body: impl FnOnce(Arc<Progress>) -> T + Send + 'static) -> T {
    let progress = Arc::new(Progress::default());
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let progress = Arc::clone(&progress);
        thread::spawn(move || {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(progress)));
            let _ = tx.send(out);
        });
    }
    let start = Instant::now();
    let (mut seen, mut since) = (0, Instant::now());
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Ok(out)) => return out,
            Ok(Err(panic)) => std::panic::resume_unwind(panic),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let now = progress.0.load(Ordering::Relaxed);
                if now != seen {
                    (seen, since) = (now, Instant::now());
                }
                assert!(
                    since.elapsed() < STALL,
                    "deadlock: no progress for {STALL:?} (stuck after {seen} steps)"
                );
                assert!(start.elapsed() < CAP, "the test ran past {CAP:?}");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                panic!("the test thread vanished")
            }
        }
    }
}

/// The `i`-th command of writer `w`: always a real change, so every one commits a revision.
fn command(
    w: usize,
    i: usize,
    track: cutweave_model::TrackId,
    mine: &[ItemId],
) -> (&'static str, Value) {
    match (i % 8, mine.last()) {
        (1 | 3 | 5, Some(item)) => (
            "item.rename",
            json!({"item": item, "name": format!("w{w}-{i}")}),
        ),
        // The last clip was appended this round: enabled, then disabled at 2, enabled at 6.
        (2 | 6, Some(item)) => (
            "item.set_enabled",
            json!({"item": item, "enabled": i % 8 == 6}),
        ),
        (4 | 7, _) => ("project.rename", json!({"name": format!("w{w}-{i}")})),
        _ => (
            "timeline.append_clip",
            json!({"track": track, "frames": 1 + i % 5}),
        ),
    }
}

#[test]
fn eight_writers_ten_thousand_commands_equal_a_sequential_replay() {
    watchdog(|progress| {
        let (doc, tracks, _) = project(WRITERS);
        // Each commit also emits an `edit.history_changed` event.
        let actor = spawn_with_capacity(doc.clone(), 2 * COMMANDS + 16);
        let h = actor.handle();
        let mut events = h.subscribe();
        let done = Arc::new(AtomicBool::new(false));

        // A reader that never stops loading: revisions only go up, snapshots are coherent.
        let reader = {
            let (h, done) = (h.clone(), Arc::clone(&done));
            thread::spawn(move || {
                let (mut last, mut reads) = (Rev(0), 0_u64);
                while !done.load(Ordering::Relaxed) {
                    let s = h.snapshot();
                    assert!(s.rev >= last, "revision went backwards");
                    assert!(s.is_committed());
                    // Leave the CPU to the actor on small machines.
                    thread::yield_now();
                    last = s.rev;
                    reads += 1;
                }
                reads
            })
        };

        let writers: Vec<_> = (0..WRITERS)
            .map(|w| {
                let (h, track, progress) = (h.clone(), tracks[w], Arc::clone(&progress));
                thread::spawn(move || {
                    let ctx = ActorCtx::agent(ConnId::new(10 + w as u64), format!("w{w}")).unwrap();
                    let mut mine = Vec::new();
                    let mut log = Vec::new();
                    for i in 0..COMMANDS / WRITERS {
                        let (name, params) = command(w, i, track, &mine);
                        let applied: Applied = h
                            .submit(Submit::new(ctx.clone(), name, params.clone()))
                            .wait()
                            .unwrap();
                        progress.tick();
                        // Read-your-writes.
                        assert!(h.snapshot().rev >= applied.rev);
                        assert!(!applied.ops.is_empty());
                        if name == "timeline.append_clip" {
                            mine.push(
                                serde_json::from_value(applied.output["item"].clone()).unwrap(),
                            );
                        }
                        log.push((applied.rev, ctx.clone(), name, params));
                    }
                    log
                })
            })
            .collect();
        let mut log: Vec<_> = writers
            .into_iter()
            .flat_map(|w| w.join().unwrap())
            .collect();
        done.store(true, Ordering::Relaxed);
        assert!(reader.join().unwrap() > 0);

        // Exactly one revision per command, no gaps, no duplicates.
        log.sort_by_key(|(rev, ..)| *rev);
        let revs: Vec<u64> = log.iter().map(|(rev, ..)| rev.0).collect();
        assert_eq!(revs, (1..=COMMANDS as u64).collect::<Vec<_>>());
        let last = h.snapshot();
        assert_eq!(last.rev, Rev(COMMANDS as u64));

        // Sequential replay of the recorded order (same seed, so the same ids).
        let mut state = DocState::new(config(doc.clone()));
        for (rev, ctx, name, params) in log {
            let applied = state.submit(Submit::new(ctx, name, params)).unwrap();
            assert_eq!(applied.rev, rev);
            progress.tick();
        }
        assert_eq!(state.doc(), &last.doc);

        // Every event, in order; replaying them reproduces the document.
        let events: Vec<_> = drain(&mut events)
            .into_iter()
            .filter(|e| e.as_patched().is_some())
            .collect();
        assert_eq!(events.len(), COMMANDS);
        let mut mirror = Mirror::new(&doc, Rev(0));
        for e in &events {
            mirror.apply(e);
            progress.tick();
        }
        mirror.assert_matches(&last.doc);
        assert_eq!(h.stats().applied, COMMANDS as u64);
    });
}

#[test]
fn gestures_and_implicit_commands_interleave_consistently() {
    watchdog(|progress| {
        let (doc, tracks, _) = project(6);
        let journal: Arc<Mutex<Vec<(Rev, Patch)>>> = Arc::default();
        let sink = {
            let journal = Arc::clone(&journal);
            move |r: &CommitRecord<'_>| {
                journal.lock().unwrap().push((r.rev, r.patch.clone()));
                Ok(())
            }
        };
        let actor = DocActor::spawn(config(doc.clone()).with_journal(Box::new(sink))).unwrap();
        let h = actor.handle();
        let mut events = h.subscribe();

        // Two connections run gestures on their own tracks; four others edit theirs directly.
        let gesturers: Vec<_> = (0..2)
            .map(|g| {
                let (h, track, progress) = (h.clone(), tracks[g], Arc::clone(&progress));
                thread::spawn(move || {
                    let ctx = ActorCtx::ui(ConnId::new(100 + g as u64));
                    for round in 0..40 {
                        let tx = h.begin_tx(&ctx, TxKind::Gesture, "drag").wait().unwrap().id;
                        for step in 0..5 {
                            h.submit(
                                Submit::new(
                                    ctx.clone(),
                                    "timeline.append_clip",
                                    json!({"track": track, "frames": 1 + step}),
                                )
                                .in_tx(tx),
                            )
                            .wait()
                            .unwrap();
                            progress.tick();
                        }
                        if round % 3 == 0 {
                            h.cancel_tx(&ctx, tx).wait().unwrap();
                        } else {
                            h.commit_tx(&ctx, tx).wait().unwrap();
                        }
                    }
                })
            })
            .collect();
        let editors: Vec<_> = (2..6)
            .map(|e| {
                let (h, track, progress) = (h.clone(), tracks[e], Arc::clone(&progress));
                thread::spawn(move || {
                    let ctx =
                        ActorCtx::agent(ConnId::new(200 + e as u64), format!("e{e}")).unwrap();
                    for i in 0..300 {
                        let applied = h
                            .submit(Submit::new(
                                ctx.clone(),
                                "timeline.append_clip",
                                json!({"track": track, "frames": 1 + i % 3}),
                            ))
                            .wait()
                            .unwrap();
                        progress.tick();
                        let snap = h.snapshot();
                        assert!(
                            snap.rev >= applied.rev && snap.committed_rev >= applied.committed_rev
                        );
                    }
                })
            })
            .collect();
        for t in gesturers.into_iter().chain(editors) {
            t.join().unwrap();
        }

        h.run(|s| s.verify()).wait().unwrap().unwrap();
        let live = h.snapshot();
        assert!(live.is_committed());
        assert_eq!(h.committed_snapshot().doc, live.doc);
        // 4 × 300 direct + 2 × (40 − 14 cancelled) × 5 committed clips.
        assert_eq!(live.doc.item_count(), 4 * 300 + 2 * 26 * 5);

        // The journal replays to the committed document, in revision order.
        let journal = journal.lock().unwrap();
        assert!(journal.windows(2).all(|w| w[0].0 < w[1].0));
        let patches: Vec<Patch> = journal.iter().map(|(_, p)| p.clone()).collect();
        assert_eq!(replay(&doc, &patches), live.doc);

        // The events replay to the live document.
        let mut mirror = Mirror::new(&doc, Rev(0));
        for e in drain(&mut events) {
            mirror.apply(&e);
        }
        mirror.assert_matches(&live.doc);
        assert_eq!(mirror.rev, live.rev);
    });
}

#[test]
fn a_slow_subscriber_lags_then_resyncs_to_the_live_document() {
    watchdog(|progress| {
        let (doc, tracks, _) = project(4);
        let actor = spawn_with_capacity(doc, 8);
        let h = actor.handle();
        let mut slow = h.subscribe();
        let writers: Vec<_> = (0..4)
            .map(|w| {
                let (h, track, progress) = (h.clone(), tracks[w], Arc::clone(&progress));
                thread::spawn(move || {
                    let ctx = ActorCtx::agent(ConnId::new(w as u64 + 1), "bot").unwrap();
                    for i in 0..500 {
                        h.submit(Submit::new(
                            ctx.clone(),
                            "timeline.append_clip",
                            json!({"track": track, "frames": 1 + i % 2}),
                        ))
                        .wait()
                        .unwrap();
                        progress.tick();
                    }
                })
            })
            .collect();

        // A consumer that is too slow on purpose: it lags, resyncs from a snapshot and keeps
        // a mirror that must end equal to the live document.
        let mut mirror: Option<Mirror> = None;
        let mut lags = 0;
        let mut writers = Some(writers);
        loop {
            match slow.recv_timeout(Duration::from_millis(50)) {
                Ok(Some(event)) => {
                    if let Some(m) = mirror.as_mut() {
                        m.apply(&event);
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Ok(None) => {
                    if writers.is_none() {
                        break;
                    }
                    if writers
                        .as_ref()
                        .unwrap()
                        .iter()
                        .all(thread::JoinHandle::is_finished)
                    {
                        for w in writers.take().unwrap() {
                            w.join().unwrap();
                        }
                        if mirror.is_none() {
                            let s = slow.resync();
                            mirror = Some(Mirror::new(&s.doc, s.rev));
                        }
                    }
                }
                Err(RecvError::Lagged { missed }) => {
                    assert!(missed > 0);
                    lags += 1;
                    let s = slow.resync();
                    mirror = Some(Mirror::new(&s.doc, s.rev));
                }
                Err(RecvError::Closed) => panic!("closed"),
            }
        }
        assert!(lags > 0, "the subscriber should have lagged");
        assert!(h.stats().lagged >= 1);
        let live = h.snapshot();
        let mirror = mirror.unwrap();
        assert_eq!(mirror.rev, live.rev);
        mirror.assert_matches(&live.doc);
        assert_eq!(live.rev, Rev(2000));
    });
}
