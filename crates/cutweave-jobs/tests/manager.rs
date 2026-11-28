// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1D.07: the job manager: budgets, waits, cancellation, dependencies, progress events,
//! history and ID generation. WP-2G.03a: holds, given IDs, completion callbacks, restored jobs
//! and progress counters.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use cutweave_jobs::{
    DEFAULT_PROXY_SLOTS, Hold, JobConfig, JobCtx, JobError, JobEvent, JobFilter, JobId, JobInfo,
    JobKind, JobManager, JobProgress, JobSpec, JobState, MemoryHistory, Submit, SubmitError,
};
use cutweave_model::{MediaId, SeededGen};

const LONG: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(10);

fn manager(decode: usize, export: usize) -> JobManager {
    JobManager::new(JobConfig {
        decode_slots: decode,
        export_slots: export,
        ..JobConfig::default()
    })
    .unwrap()
}

fn spec(label: &str) -> JobSpec {
    JobSpec::new(JobKind::Analysis, label)
}

/// A body that signals `started` and then runs until cancelled (at most [`LONG`]).
fn until_cancelled(started: mpsc::Sender<()>) -> impl FnOnce(&JobCtx) -> Result<(), JobError> {
    move |ctx| {
        let _ = started.send(());
        ctx.sleep(LONG)?;
        Err(JobError::failed("was never cancelled"))
    }
}

#[test]
fn the_budget_semaphore_limits_concurrency_per_class() {
    let jm = manager(2, 1);
    let decode_now = Arc::new(AtomicUsize::new(0));
    let decode_max = Arc::new(AtomicUsize::new(0));
    let export_now = Arc::new(AtomicUsize::new(0));
    let export_max = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for i in 0..12 {
        let (kind, now, max) = if i % 3 == 0 {
            (JobKind::Export, &export_now, &export_max)
        } else {
            (JobKind::Analysis, &decode_now, &decode_max)
        };
        let (now, max) = (Arc::clone(now), Arc::clone(max));
        handles.push(jm.submit(JobSpec::new(kind, "count"), move |ctx| {
            let running = now.fetch_add(1, Ordering::SeqCst) + 1;
            max.fetch_max(running, Ordering::SeqCst);
            ctx.sleep(Duration::from_millis(40))?;
            now.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }));
    }
    for h in &handles {
        assert_eq!(h.wait(SETTLE).unwrap().state, JobState::Succeeded);
    }
    assert_eq!(decode_max.load(Ordering::SeqCst), 2, "decode budget");
    assert_eq!(export_max.load(Ordering::SeqCst), 1, "export budget");
    assert_eq!(jm.active(), 0);
}

/// A body that counts itself in `now` / `max` while it runs, until `release` is set.
fn counted(
    now: &Arc<AtomicUsize>,
    max: &Arc<AtomicUsize>,
    release: &Arc<AtomicBool>,
) -> impl FnOnce(&JobCtx) -> Result<(), JobError> + use<> {
    let (now, max, release) = (Arc::clone(now), Arc::clone(max), Arc::clone(release));
    move |ctx| {
        let running = now.fetch_add(1, Ordering::SeqCst) + 1;
        max.fetch_max(running, Ordering::SeqCst);
        while !release.load(Ordering::SeqCst) {
            ctx.sleep(Duration::from_millis(5))?;
        }
        now.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Waits until `cond` holds (at most [`SETTLE`]).
fn eventually(what: &str, cond: impl Fn() -> bool) {
    let until = Instant::now() + SETTLE;
    while !cond() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_proxy_budget_changes_while_jobs_run() {
    let jm = manager(4, 1);
    let (now, max) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let release = Arc::new(AtomicBool::new(false));
    let submit = |release: &Arc<AtomicBool>| {
        jm.submit(
            JobSpec::new(JobKind::Proxy, "proxy"),
            counted(&now, &max, release),
        )
    };
    // Two run; with one slot the running jobs finish, and the next ones go one at a time.
    let first = [submit(&release), submit(&release)];
    eventually("two proxy jobs", || now.load(Ordering::SeqCst) == 2);
    jm.set_proxy_slots(1);
    assert_eq!(jm.proxy_slots(), 1);
    let one = Arc::new(AtomicBool::new(false));
    let second = [submit(&one), submit(&one)];
    release.store(true, Ordering::SeqCst);
    for job in &first {
        assert_eq!(job.wait(SETTLE).unwrap().state, JobState::Succeeded);
    }
    eventually("one proxy job", || now.load(Ordering::SeqCst) == 1);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(now.load(Ordering::SeqCst), 1, "one slot, one job");
    // Three slots: the waiting job starts at once, and a third next to them.
    jm.set_proxy_slots(3);
    let third = submit(&one);
    eventually("three proxy jobs", || now.load(Ordering::SeqCst) == 3);
    // Zero is one: proxies never stop altogether.
    jm.set_proxy_slots(0);
    assert_eq!(jm.proxy_slots(), 1);
    one.store(true, Ordering::SeqCst);
    for job in second.iter().chain([&third]) {
        assert_eq!(job.wait(SETTLE).unwrap().state, JobState::Succeeded);
    }
    let last = Arc::new(AtomicBool::new(false));
    let tail = [submit(&last), submit(&last)];
    eventually("one of the last two", || now.load(Ordering::SeqCst) == 1);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(now.load(Ordering::SeqCst), 1, "back to one slot");
    last.store(true, Ordering::SeqCst);
    for job in &tail {
        assert_eq!(job.wait(SETTLE).unwrap().state, JobState::Succeeded);
    }
    assert_eq!(max.load(Ordering::SeqCst), 3);
}

#[test]
fn proxy_jobs_run_two_at_a_time_and_one_while_an_export_runs() {
    let jm = manager(4, 1);
    assert_eq!(jm.config().proxy_slots, DEFAULT_PROXY_SLOTS);
    assert_eq!(DEFAULT_PROXY_SLOTS, 2);
    let (now, max) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    // Two proxies run together; a third waits.
    let first = Arc::new(AtomicBool::new(false));
    let a = jm.submit(
        JobSpec::new(JobKind::Proxy, "proxy"),
        counted(&now, &max, &first),
    );
    let b = jm.submit(
        JobSpec::new(JobKind::Proxy, "proxy"),
        counted(&now, &max, &first),
    );
    eventually("two proxy jobs", || now.load(Ordering::SeqCst) == 2);
    // An export starts: its reservation takes the slot the first proxy to finish frees.
    let export_release = Arc::new(AtomicBool::new(false));
    let (en, em) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let export = jm.submit(
        JobSpec::new(JobKind::Export, "export"),
        counted(&en, &em, &export_release),
    );
    eventually("the export", || en.load(Ordering::SeqCst) == 1);
    let later = Arc::new(AtomicBool::new(false));
    let c = jm.submit(
        JobSpec::new(JobKind::Proxy, "proxy"),
        counted(&now, &max, &later),
    );
    let d = jm.submit(
        JobSpec::new(JobKind::Proxy, "proxy"),
        counted(&now, &max, &later),
    );
    // The running proxies finish; while the export runs only one of the next two starts.
    first.store(true, Ordering::SeqCst);
    assert_eq!(a.wait(SETTLE).unwrap().state, JobState::Succeeded);
    assert_eq!(b.wait(SETTLE).unwrap().state, JobState::Succeeded);
    eventually("one proxy next to the export", || {
        now.load(Ordering::SeqCst) == 1
    });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        now.load(Ordering::SeqCst),
        1,
        "one proxy job while exporting"
    );
    let queued = [c.status().unwrap().state, d.status().unwrap().state];
    assert!(queued.contains(&JobState::Queued), "{queued:?}");
    // The export ends: the other proxy starts.
    export_release.store(true, Ordering::SeqCst);
    assert_eq!(export.wait(SETTLE).unwrap().state, JobState::Succeeded);
    eventually("both proxies again", || now.load(Ordering::SeqCst) == 2);
    later.store(true, Ordering::SeqCst);
    assert_eq!(c.wait(SETTLE).unwrap().state, JobState::Succeeded);
    assert_eq!(d.wait(SETTLE).unwrap().state, JobState::Succeeded);
    assert_eq!(max.load(Ordering::SeqCst), 2);
    assert_eq!(jm.active(), 0);
}

#[test]
fn the_default_decode_budget_is_half_the_cores() {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    assert_eq!(JobConfig::default().decode_slots, (cores / 2).max(1));
    assert_eq!(JobConfig::default().export_slots, 1);
}

#[test]
fn wait_times_out_on_a_running_job_and_returns_at_once_for_a_finished_one() {
    let jm = manager(2, 1);
    let (tx, started) = mpsc::channel();
    let long = jm.submit(spec("long"), until_cancelled(tx));
    started.recv_timeout(SETTLE).unwrap();

    let t = Instant::now();
    let info = jm.wait(long.id(), Duration::from_millis(100)).unwrap();
    let waited = t.elapsed();
    assert_eq!(info.state, JobState::Running);
    assert!(waited >= Duration::from_millis(100), "{waited:?}");
    assert!(waited < Duration::from_secs(1), "{waited:?}");

    let quick = jm.submit(spec("quick"), |_| Ok(()));
    assert_eq!(quick.wait(SETTLE).unwrap().state, JobState::Succeeded);
    let t = Instant::now();
    let info = jm.wait(quick.id(), Duration::from_secs(60)).unwrap();
    assert_eq!(info.state, JobState::Succeeded);
    assert_eq!(info.progress.fraction, 1.0);
    assert!(
        t.elapsed() < Duration::from_millis(100),
        "{:?}",
        t.elapsed()
    );

    assert!(
        jm.wait(JobId::from_u128(1), Duration::from_secs(5))
            .is_none()
    );
    long.cancel();
}

#[test]
fn cancelling_a_running_job_stops_it_within_500_ms() {
    let jm = manager(2, 1);
    let (tx, started) = mpsc::channel();
    let job = jm.submit(spec("long"), until_cancelled(tx));
    started.recv_timeout(SETTLE).unwrap();
    let t = Instant::now();
    let info = jm.cancel(job.id()).unwrap();
    assert_eq!(info.state, JobState::Running);
    assert!(info.cancel_requested);
    let done = job.wait(SETTLE).unwrap();
    let took = t.elapsed();
    assert_eq!(done.state, JobState::Cancelled);
    assert!(done.cancel_requested);
    assert!(took < Duration::from_millis(500), "{took:?}");
    // Cancelling again, or a finished job, changes nothing.
    assert_eq!(jm.cancel(job.id()).unwrap(), done);
    assert!(jm.cancel(JobId::from_u128(1)).is_none());
}

#[test]
fn cancelling_a_queued_job_ends_it_at_once_without_running_it() {
    let jm = manager(1, 1);
    let (tx, started) = mpsc::channel();
    let blocker = jm.submit(spec("blocker"), until_cancelled(tx));
    started.recv_timeout(SETTLE).unwrap();
    let ran = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ran);
    let queued = jm.submit(spec("queued"), move |_| {
        flag.store(true, Ordering::SeqCst);
        Ok(())
    });
    assert_eq!(queued.status().unwrap().state, JobState::Queued);
    let info = queued.cancel().unwrap();
    assert_eq!(info.state, JobState::Cancelled);
    assert!(info.cancel_requested);
    blocker.cancel();
    assert_eq!(blocker.wait(SETTLE).unwrap().state, JobState::Cancelled);
    // Give the queued job's task a chance to (wrongly) run it.
    let after = jm.submit(spec("after"), |_| Ok(()));
    after.wait(SETTLE).unwrap();
    assert!(!ran.load(Ordering::SeqCst));
    assert_eq!(queued.status().unwrap().state, JobState::Cancelled);
}

#[test]
fn failures_and_panics_fail_the_job_with_a_message() {
    let jm = manager(2, 1);
    let failed = jm.submit(spec("fails"), |_| Err(JobError::failed("corrupt GOP")));
    let info = failed.wait(SETTLE).unwrap();
    assert_eq!(info.state, JobState::Failed);
    assert_eq!(info.error.as_deref(), Some("corrupt GOP"));
    // A plain failure has the general code; a coded one its own (ADR-0038).
    assert_eq!(info.error_code.as_deref(), Some("failed"));
    let coded = jm.submit(spec("coded"), |_| {
        Err(JobError::coded(
            cutweave_jobs::error_code::DISK_FULL,
            "no space left",
        ))
    });
    let info = coded.wait(SETTLE).unwrap();
    assert_eq!(info.error.as_deref(), Some("no space left"));
    assert_eq!(info.error_code.as_deref(), Some("disk_full"));
    assert_eq!(jm.status(coded.id()).unwrap().error_code, info.error_code);

    let panics = jm.submit(spec("panics"), |_| panic!("decoder exploded"));
    let info = panics.wait(SETTLE).unwrap();
    assert_eq!(info.state, JobState::Failed);
    assert!(
        info.error.as_deref().unwrap().contains("decoder exploded"),
        "{info:?}"
    );
    assert_eq!(info.error_code.as_deref(), Some("failed"));
    // The manager keeps working; a job that succeeds has no code.
    let ok = jm.submit(spec("ok"), |_| Ok(()));
    let info = ok.wait(SETTLE).unwrap();
    assert_eq!(info.state, JobState::Succeeded);
    assert_eq!(info.error_code, None);
}

#[test]
fn dependencies_gate_jobs_without_holding_a_budget_slot() {
    let jm = manager(2, 1);
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = |name: &'static str| {
        let order = Arc::clone(&order);
        move |ctx: &JobCtx| {
            ctx.sleep(Duration::from_millis(30))?;
            order.lock().unwrap().push(name);
            Ok(())
        }
    };
    let (tx, started) = mpsc::channel();
    let gate = jm.submit(spec("gate"), until_cancelled(tx));
    started.recv_timeout(SETTLE).unwrap();
    let waiting = jm.submit(spec("waiting").after([gate.id()]), log("waiting"));
    // `waiting` must not hold the second decode slot while `gate` runs.
    let free = jm.submit(spec("free"), log("free"));
    assert_eq!(free.wait(SETTLE).unwrap().state, JobState::Succeeded);
    assert_eq!(waiting.status().unwrap().state, JobState::Queued);
    assert_eq!(*order.lock().unwrap(), ["free"]);

    // A cancelled dependency cancels the dependant.
    gate.cancel();
    let info = waiting.wait(SETTLE).unwrap();
    assert_eq!(info.state, JobState::Cancelled);
    assert_eq!(info.error_code, None, "a cancelled job has no code");
    assert!(info.error.unwrap().contains("gate"));

    // A failed dependency skips (fails) the dependant; a successful one lets it run after.
    let first = jm.submit(spec("first"), log("first"));
    let second = jm.submit(spec("second").after([first.id()]), log("second"));
    let broken = jm.submit(spec("broken"), |_| Err(JobError::failed("nope")));
    let skipped = jm.submit(spec("skipped").after([broken.id()]), log("skipped"));
    assert_eq!(second.wait(SETTLE).unwrap().state, JobState::Succeeded);
    let info = skipped.wait(SETTLE).unwrap();
    assert_eq!(info.state, JobState::Failed);
    assert_eq!(info.error_code.as_deref(), Some("failed"));
    assert!(info.error.unwrap().starts_with("skipped"), "skipped");
    assert_eq!(*order.lock().unwrap(), ["free", "first", "second"]);
}

#[test]
fn progress_is_stored_and_broadcast_in_order() {
    let jm = manager(2, 1);
    let mut events = jm.subscribe();
    let media = MediaId::from_u128(5);
    let job = jm.submit(spec("index").media(media), |ctx| {
        ctx.progress(0.5, Some("half"));
        ctx.progress(0.501, Some("half")); // below the step: stored, not broadcast
        ctx.progress(f64::NAN, None); // ignored
        ctx.progress(2.0, Some("done")); // clamped
        Ok(())
    });
    let done = job.wait(SETTLE).unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    assert_eq!(done.media_id, Some(media));
    let mut seen = Vec::new();
    while let Ok(JobEvent::Progress(info)) = events.try_recv() {
        assert_eq!(info.id, job.id());
        seen.push((info.state, info.progress.fraction, info.progress.message));
    }
    assert_eq!(
        seen,
        [
            (JobState::Queued, 0.0, None),
            (JobState::Running, 0.0, None),
            (JobState::Running, 0.5, Some("half".to_owned())),
            (JobState::Running, 1.0, Some("done".to_owned())),
            (JobState::Succeeded, 1.0, Some("done".to_owned())),
        ]
    );
}

#[test]
fn list_is_newest_first_filtered_and_finished_jobs_age_out_of_history() {
    let jm = JobManager::builder()
        .config(JobConfig {
            decode_slots: 2,
            ..JobConfig::default()
        })
        .history(Arc::new(MemoryHistory::new(3)))
        .build()
        .unwrap();
    let media = MediaId::from_u128(9);
    let ids: Vec<JobId> = (0..4)
        .map(|i| {
            let s = if i == 2 {
                spec("on").media(media)
            } else {
                spec("x")
            };
            let h = jm.submit(s, |_| Ok(()));
            h.wait(SETTLE).unwrap();
            h.id()
        })
        .collect();
    let listed: Vec<JobId> = jm
        .list(&JobFilter::default(), 50)
        .iter()
        .map(|j| j.id)
        .collect();
    assert_eq!(listed, [ids[3], ids[2], ids[1]], "the oldest aged out");
    assert!(jm.status(ids[0]).is_none());
    let on = jm.list(
        &JobFilter {
            media_id: Some(media),
            ..JobFilter::default()
        },
        50,
    );
    assert_eq!(on.len(), 1);
    assert_eq!(on[0].id, ids[2]);
    assert_eq!(jm.list(&JobFilter::default(), 1).len(), 1);

    let (tx, started) = mpsc::channel();
    let running = jm.submit(spec("running"), until_cancelled(tx));
    started.recv_timeout(SETTLE).unwrap();
    let active = jm.list(
        &JobFilter {
            state: Some(JobState::Running),
            ..JobFilter::default()
        },
        50,
    );
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, running.id());
    assert_eq!(jm.list(&JobFilter::default(), 50)[0].id, running.id());
    running.cancel();
}

#[test]
fn job_ids_come_from_the_injected_generator() {
    let jm = JobManager::builder()
        .ids(Box::new(SeededGen::new(9)))
        .build()
        .unwrap();
    let mut expected = SeededGen::new(9);
    for _ in 0..3 {
        let h = jm.submit(spec("x"), |_| Ok(()));
        assert_eq!(h.id(), JobId::mint(&mut expected));
        assert!(h.id().to_string().starts_with("job_"));
    }
}

#[test]
fn dropping_the_manager_cancels_running_jobs() {
    let jm = manager(2, 1);
    let (tx, started) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    jm.submit(spec("long"), move |ctx| {
        let _ = tx.send(());
        let r = ctx.sleep(LONG);
        let _ = done_tx.send(r.clone());
        r
    });
    started.recv_timeout(SETTLE).unwrap();
    drop(jm);
    assert_eq!(
        done.recv_timeout(Duration::from_secs(2)).unwrap(),
        Err(JobError::Cancelled)
    );
}

#[test]
fn it_runs_on_an_engine_runtime_and_waits_asynchronously() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_time()
        .build()
        .unwrap();
    let jm = JobManager::builder()
        .runtime(rt.handle().clone())
        .build()
        .unwrap();
    let (tx, started) = mpsc::channel();
    let long = jm.submit(spec("long"), until_cancelled(tx));
    started.recv_timeout(SETTLE).unwrap();
    let quick = jm.submit(spec("quick"), |_| Ok(()));
    let jm2 = jm.clone();
    let (q, l) = rt.block_on(async move {
        let q = jm2.wait_async(quick.id(), SETTLE).await.unwrap();
        let l = jm2
            .wait_async(long.id(), Duration::from_millis(50))
            .await
            .unwrap();
        (q, l)
    });
    assert_eq!(q.state, JobState::Succeeded);
    assert_eq!(l.state, JobState::Running);
    jm.cancel_all();
    drop(jm);
    rt.shutdown_timeout(Duration::from_secs(5));
}

#[test]
fn held_jobs_wait_for_their_release_and_given_ids_are_reused_only_when_free() {
    let jm = manager(2, 1);
    let hold = Hold::new();
    let ran = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(std::sync::Mutex::new(None));
    let (flag, record) = (Arc::clone(&ran), Arc::clone(&seen));
    let id = JobId::from_u128(42);
    let h = jm
        .submit_with(
            JobSpec::new(JobKind::Export, "export"),
            Submit {
                id: Some(id),
                hold: Some(hold.clone()),
                on_done: Some(Box::new(move |info: &JobInfo| {
                    *record.lock().unwrap() = Some(info.state);
                })),
            },
            move |_| {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(h.id(), id);
    // An ID in use is refused.
    assert_eq!(
        jm.submit_with(
            spec("again"),
            Submit {
                id: Some(id),
                ..Submit::default()
            },
            |_| Ok(())
        )
        .unwrap_err(),
        SubmitError::InUse(id)
    );
    std::thread::sleep(Duration::from_millis(50));
    assert!(!ran.load(Ordering::SeqCst), "held");
    assert_eq!(jm.status(id).unwrap().state, JobState::Queued);
    hold.release();
    let done = jm.wait(id, SETTLE).unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    // The callback ran before the waiter woke.
    assert_eq!(*seen.lock().unwrap(), Some(JobState::Succeeded));
    assert!(ran.load(Ordering::SeqCst));
    // Once finished, the ID may run again (a retry), and the new run supersedes the old.
    let again = jm
        .submit_with(
            spec("retry"),
            Submit {
                id: Some(id),
                ..Submit::default()
            },
            |_| Err(JobError::failed("no")),
        )
        .unwrap();
    assert_eq!(jm.wait(again.id(), SETTLE).unwrap().state, JobState::Failed);
    assert_eq!(jm.status(id).unwrap().state, JobState::Failed);
}

#[test]
fn a_held_job_can_be_cancelled_and_fresh_ids_skip_known_ones() {
    let jm = JobManager::builder()
        .ids(Box::new(SeededGen::new(5)))
        .build()
        .unwrap();
    let mut ids = SeededGen::new(5);
    let first = JobId::mint(&mut ids);
    let second = JobId::mint(&mut ids);
    // A restored job holds the generator's first ID; a finished one its second.
    let held = jm
        .submit_with(
            spec("held"),
            Submit {
                id: Some(first),
                hold: Some(Hold::new()),
                on_done: None,
            },
            |_| Ok(()),
        )
        .unwrap();
    jm.record(&JobInfo {
        id: second,
        kind: JobKind::Export,
        label: "export".into(),
        media_id: None,
        state: JobState::Succeeded,
        progress: JobProgress::new(1.0, None),
        cancel_requested: false,
        error: None,
        error_code: None,
    });
    assert_eq!(jm.status(second).unwrap().state, JobState::Succeeded);
    let fresh = jm.mint();
    assert!(fresh != first && fresh != second);
    let h = jm.submit(spec("x"), |_| Ok(()));
    assert!(![first, second, fresh].contains(&h.id()));
    assert_eq!(held.cancel().unwrap().state, JobState::Cancelled);
}

#[test]
fn counters_ride_along_with_progress_and_only_jobs_with_counters_send_them() {
    let jm = manager(2, 1);
    let mut events = jm.subscribe();
    let (tx, go) = mpsc::channel::<()>();
    let h = jm.submit(JobSpec::new(JobKind::Export, "export"), move |ctx| {
        ctx.report(JobProgress {
            fraction: 0.5,
            message: Some("frame 5 of 10".into()),
            frame: Some(5),
            frames: Some(10),
            fps: Some(f64::NAN),
            eta_ms: Some(1000),
            bytes: Some(4096),
        });
        let _ = go.recv_timeout(SETTLE);
        Ok(())
    });
    eventually("the report", || {
        jm.status(h.id()).unwrap().progress.frame == Some(5)
    });
    let p = jm.status(h.id()).unwrap().progress;
    assert_eq!(
        (p.frames, p.eta_ms, p.bytes),
        (Some(10), Some(1000), Some(4096))
    );
    assert_eq!(p.fps, None, "a speed that is not a number is dropped");
    tx.send(()).unwrap();
    jm.wait(h.id(), SETTLE).unwrap();
    let mut with_counters = 0;
    while let Ok(e) = events.try_recv() {
        let v = serde_json::to_value(e.job()).unwrap();
        if v["progress"].get("frame").is_some() {
            with_counters += 1;
        }
    }
    assert!(with_counters >= 1);
    let plain = jm.submit(spec("index"), |ctx| {
        ctx.progress(0.5, Some("half"));
        Ok(())
    });
    let v = serde_json::to_value(jm.wait(plain.id(), SETTLE).unwrap()).unwrap();
    assert_eq!(
        v["progress"],
        serde_json::json!({"fraction": 1.0, "message": "half"})
    );
}
