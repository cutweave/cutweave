// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1C.02 crash-recovery test (arch §17, exit 3).
//!
//! A child process (this test binary, re-run as `child_worker`) opens a journaled project and
//! applies N = 500 random ops through the registry with up to 5 requests in flight, printing
//! `ACK <i>` after each op's acknowledgement, and saves twice along the way (journal
//! rotation). The parent kills it (`TerminateProcess` on Windows, `SIGKILL` elsewhere) at a
//! random acknowledged op, then recovers the project in-process and checks:
//!
//! - the recovered document is exactly the reference document after some op `j` with
//!   `last acked <= j < last acked + 1 + DEPTH`: no acknowledged op lost, no partial op;
//! - re-submitting every unacknowledged op with its original `request_id` applies each exactly
//!   once (journaled ones return their original result), and every later document equals the
//!   uninterrupted reference run's, up to the final one.
//!
//! 20 trials (`CUTWEAVE_CRASH_TRIALS` overrides), each with its own kill point.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    missing_docs
)]

mod journal_support;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cutweave_commands::{Applied, Rev};
use cutweave_engine::journal::SyncPolicy;
use journal_support::{
    DEPTH, Op, OpResult, Reference, Rng, SessionOptions, TempDir, create_project, drive, fixture,
    open_session, run_batch, submit_single,
};

/// Ops per run.
const N: usize = 500;
/// The workload seed (ops and request ids).
const WORKLOAD_SEED: u64 = 0x1C02;
/// After these ops (acknowledged) the child saves the project (journal rotation).
const SAVES: [usize; 2] = [150, 350];

/// Child half: runs the workload against the project in `CUTWEAVE_CRASH_CHILD` until killed.
#[test]
fn child_worker() {
    let Ok(folder) = std::env::var("CUTWEAVE_CRASH_CHILD") else {
        return; // a normal test run: nothing to do
    };
    let mut session = open_session(
        Path::new(&folder),
        &SessionOptions {
            sync: SyncPolicy::default(),
            ..SessionOptions::default()
        },
    );
    let handle = session.handle.clone();
    let stdout = std::io::stdout();
    println!("READY");
    drive(
        &handle,
        WORKLOAD_SEED,
        N,
        |i, _| {
            let mut out = stdout.lock();
            let _ = writeln!(out, "ACK {i}");
            let _ = out.flush();
        },
        |i| {
            if SAVES.contains(&i) {
                session
                    .store
                    .save_snapshot(&handle.committed_snapshot())
                    .unwrap();
                println!("SAVED {i}");
            }
        },
    );
    println!("DONE");
    // The parent kills us; never return normally.
    std::thread::sleep(std::time::Duration::from_secs(120));
}

/// The uninterrupted run, in-process, over its own copy of the project.
fn reference(dir: &Path) -> Reference {
    let folder = create_project(dir, "Reference", &fixture(3));
    let session = open_session(&folder, &SessionOptions::default());
    drive(&session.handle, WORKLOAD_SEED, N, |_, _| {}, |_| {})
}

/// `a` equals `b` up to the transaction id (a resent batch opens a fresh transaction, so later
/// ids may shift) and the `replayed` flag.
fn same_result(a: &Applied, b: &Applied) -> bool {
    a.request_id == b.request_id
        && a.rev == b.rev
        && a.committed_rev == b.committed_rev
        && a.ops == b.ops
}

struct Trial {
    kill_after: usize,
    last_acked: Option<usize>,
    recovered_at: Option<usize>,
    replayed: usize,
    applied_after: usize,
}

fn run_trial(
    dir: &Path,
    n: usize,
    kill_after: usize,
    jitter_us: u64,
    reference: &Reference,
) -> Trial {
    let folder = create_project(dir, &format!("Trial {n}"), &fixture(3));
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_worker", "--nocapture", "--test-threads=1"])
        .env("CUTWEAVE_CRASH_CHILD", &folder)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut last_acked = None;
    for line in lines.by_ref() {
        let line = line.unwrap();
        if let Some(i) = line.strip_prefix("ACK ") {
            let i: usize = i.trim().parse().unwrap();
            last_acked = Some(i);
            if i >= kill_after {
                break;
            }
        } else if line.starts_with("DONE") {
            break;
        }
    }
    // Land anywhere inside the following ops (mid-command, mid-append, mid-rotation).
    std::thread::sleep(Duration::from_micros(jitter_us));
    child.kill().unwrap(); // TerminateProcess / SIGKILL: no cleanup runs in the child
    child.wait().unwrap();
    // Every acknowledgement the child printed before it died counts as acknowledged.
    for line in lines {
        let Ok(line) = line else { break };
        if let Some(i) = line.strip_prefix("ACK ") {
            last_acked = Some(i.trim().parse().unwrap());
        }
    }

    let session = open_session(&folder, &SessionOptions::default());
    let recovered = &session.recovered;
    assert!(
        recovered
            .report
            .stopped
            .as_ref()
            .is_none_or(|s| s.reason == cutweave_engine::journal::StopReason::Truncated),
        "trial {n}: replay stopped on corruption: {:?}",
        recovered.report.stopped
    );
    // The recovered document is the reference after exactly one op boundary at or after the
    // last acknowledged op, within the in-flight window.
    let floor = last_acked.map_or(Rev::ZERO, |i| reference.revs[i]);
    assert!(
        recovered.rev >= floor,
        "trial {n}: acknowledged work lost: recovered rev {} < acked rev {floor}",
        recovered.rev
    );
    let first = last_acked.map_or(0, |i| i + 1);
    let recovered_at = (first.saturating_sub(1)..N)
        .find(|&j| reference.revs[j] == recovered.rev && reference.docs[j] == recovered.project);
    match recovered_at {
        Some(j) => assert!(
            j < first + DEPTH,
            "trial {n}: recovered op {j} beyond the in-flight window after {last_acked:?}"
        ),
        None => assert!(
            last_acked.is_none() && recovered.project == reference.initial,
            "trial {n}: recovered document (rev {}) matches no op boundary (partial op?)",
            recovered.rev
        ),
    }

    // Resend every op not acknowledged, with its original request ids, then finish the run.
    let h = &session.handle;
    let first = last_acked.map_or(0, |i| i + 1);
    let (mut replayed, mut applied_after) = (0, 0);
    for i in first..N {
        // Ops the journal already had (unacknowledged) are replayed without changing anything.
        let expected_doc = &reference.docs[recovered_at.map_or(i, |j| i.max(j))];
        match (&reference.ops[i], &reference.results[i]) {
            (Op::Single(req), OpResult::Single(want)) => {
                let got = submit_single(h, req).wait();
                match (&got, want) {
                    (Ok(got), Ok(want)) => {
                        if got.replayed {
                            replayed += 1;
                            assert!(
                                i < first + DEPTH,
                                "trial {n}: op {i} replayed but was never in flight"
                            );
                        } else {
                            applied_after += 1;
                        }
                        if req.name != "edit.undo" {
                            assert!(
                                same_result(got, want) && got.output == want.output,
                                "trial {n}: op {i} ({}) result differs:\n got {got:?}\nwant {want:?}",
                                req.name
                            );
                        }
                    }
                    (Err(got), Err(want)) => {
                        assert_eq!(got.code(), want.code(), "trial {n}: op {i}");
                    }
                    _ => panic!(
                        "trial {n}: op {i} ({}): got {got:?}, want {want:?}",
                        req.name
                    ),
                }
            }
            (
                Op::Batch {
                    begin,
                    steps,
                    commit,
                },
                OpResult::Batch(want),
            ) => {
                let got = run_batch(h, *begin, steps, *commit);
                assert_eq!(got.is_ok(), want.is_ok(), "trial {n}: batch op {i}");
                applied_after += 1;
            }
            _ => unreachable!(),
        }
        assert_eq!(
            &h.committed_snapshot().doc,
            expected_doc,
            "trial {n}: document after op {i} differs from the uninterrupted run (applied twice or lost?)"
        );
    }
    assert_eq!(
        h.committed_snapshot().doc,
        *reference.docs.last().unwrap(),
        "trial {n}: final document differs from the uninterrupted run"
    );
    drop(session);
    Trial {
        kill_after,
        last_acked,
        recovered_at,
        replayed,
        applied_after,
    }
}

#[test]
fn killed_child_recovers_every_acknowledged_op_and_resends_apply_once() {
    let t = TempDir::new("crash");
    let started = Instant::now();
    let reference = reference(&t.0);
    let trials: usize = std::env::var("CUTWEAVE_CRASH_TRIALS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let mut rng = Rng::new(
        std::env::var("CUTWEAVE_CRASH_SEED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0x5EED_1C02),
    );
    let mut summary = Vec::new();
    for n in 0..trials {
        let kill_after = rng.below(N - 1);
        let jitter_us = rng.next() % 3_000;
        let trial = run_trial(&t.0, n, kill_after, jitter_us, &reference);
        summary.push(trial);
    }
    for (n, t) in summary.iter().enumerate() {
        println!(
            "trial {n:2}: kill after ACK {:3} | last acked {:?} | recovered at op {:?} | resent: {} replayed, {} applied",
            t.kill_after, t.last_acked, t.recovered_at, t.replayed, t.applied_after
        );
    }
    println!(
        "{trials} kills, 0 acknowledged ops lost, every resend applied exactly once ({:.1?})",
        started.elapsed()
    );
}
