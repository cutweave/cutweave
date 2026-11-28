// SPDX-License-Identifier: GPL-3.0-or-later
//! The export queue over a real job manager, with tasks that wait for the test to end them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::manager::JobConfig;

const LONG: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Task(u32);

struct Codec;

impl TaskCodec<Task> for Codec {
    fn encode(&self, task: &Task) -> Result<Vec<u8>, String> {
        Ok(task.0.to_le_bytes().to_vec())
    }
    fn decode(&self, bytes: &[u8]) -> Result<Task, String> {
        let b: [u8; 4] = bytes.try_into().map_err(|_| "not a task".to_owned())?;
        Ok(Task(u32::from_le_bytes(b)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    Ok,
    Fail,
    /// Fails with the machine code `disk_full`.
    DiskFull,
}

/// What the tasks did and how the test ends them.
#[derive(Default)]
struct World {
    /// Task numbers in the order they started.
    started: Vec<u32>,
    /// How each task ends, once the test says so.
    ends: BTreeMap<u32, End>,
}

#[derive(Clone, Default)]
struct Rig(Arc<(Mutex<World>, Condvar)>);

impl Rig {
    fn runner(&self) -> Runner<Task> {
        let rig = self.clone();
        Arc::new(move |task: &Task, ctx: &JobCtx| {
            let (world, cv) = &*rig.0;
            world.lock().unwrap().started.push(task.0);
            cv.notify_all();
            loop {
                let end = world.lock().unwrap().ends.remove(&task.0);
                if let Some(end) = end {
                    return match end {
                        End::Ok => Ok(()),
                        End::Fail => Err(JobError::failed(format!("task {} failed", task.0))),
                        End::DiskFull => Err(JobError::coded(
                            crate::error_code::DISK_FULL,
                            format!("task {} ran out of space", task.0),
                        )),
                    };
                }
                ctx.check()?;
                std::thread::sleep(Duration::from_millis(2));
            }
        })
    }

    fn end(&self, n: u32, end: End) {
        self.0.0.lock().unwrap().ends.insert(n, end);
    }

    fn started(&self) -> Vec<u32> {
        self.0.0.lock().unwrap().started.clone()
    }

    /// Waits until `n` tasks have started.
    fn await_started(&self, n: usize) -> Vec<u32> {
        let (world, cv) = &*self.0;
        let deadline = Instant::now() + LONG;
        let mut w = world.lock().unwrap();
        while w.started.len() < n {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "only {:?} started", w.started);
            w = cv.wait_timeout(w, left).unwrap().0;
        }
        w.started.clone()
    }
}

fn manager() -> JobManager {
    JobManager::new(JobConfig {
        export_slots: 1,
        ..JobConfig::default()
    })
    .unwrap()
}

fn queue(jobs: &JobManager, rig: &Rig) -> ExportQueue<Task> {
    ExportQueue::new(jobs.clone(), rig.runner(), Arc::new(Codec), 1)
}

fn item(n: u32) -> QueueItem<Task> {
    QueueItem {
        label: "export".into(),
        output: Some(PathBuf::from(format!("/out/{n}.mp4"))),
        task: Task(n),
    }
}

fn wait(jobs: &JobManager, id: JobId) -> JobInfo {
    let info = jobs.wait(id, LONG).unwrap();
    assert!(info.state.is_terminal(), "{info:?}");
    info
}

fn states(q: &ExportQueue<Task>) -> Vec<(u32, JobState)> {
    q.view()
        .entries
        .iter()
        .map(|e| (e.task.0, e.info.state))
        .collect()
}

/// Lets the dispatcher and the manager settle for a moment (only to show that nothing
/// more starts).
fn settle() {
    std::thread::sleep(Duration::from_millis(60));
}

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cutweave-queue-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn jobs_run_one_at_a_time_in_queue_order() {
    let (jobs, rig) = (manager(), Rig::default());
    let q = queue(&jobs, &rig);
    q.start();
    let ids: Vec<JobId> = (1..=3)
        .map(|n| q.enqueue(item(n), false).unwrap())
        .collect();
    assert_eq!(rig.await_started(1), [1]);
    settle();
    assert_eq!(rig.started(), [1], "one export at a time");
    // job.status sees the waiting ones from the start.
    assert_eq!(jobs.status(ids[2]).unwrap().state, JobState::Queued);
    for (n, id) in (1..=3).zip(&ids) {
        rig.end(n, End::Ok);
        assert_eq!(wait(&jobs, *id).state, JobState::Succeeded);
    }
    assert_eq!(rig.started(), [1, 2, 3]);
    assert_eq!(
        states(&q),
        [
            (1, JobState::Succeeded),
            (2, JobState::Succeeded),
            (3, JobState::Succeeded)
        ]
    );
}

#[test]
fn nothing_starts_before_start_or_while_paused_and_start_resumes() {
    let (jobs, rig) = (manager(), Rig::default());
    let q = queue(&jobs, &rig);
    let a = q.enqueue(item(1), false).unwrap();
    settle();
    assert_eq!(rig.started(), Vec::<u32>::new(), "the engine is not up yet");
    q.start();
    rig.await_started(1);
    q.pause(true).unwrap();
    let b = q.enqueue(item(2), false).unwrap();
    rig.end(1, End::Ok);
    wait(&jobs, a);
    settle();
    assert_eq!(rig.started(), [1], "a paused queue starts nothing");
    assert!(q.view().paused);
    // Adding with `resume` (export.start) resumes it.
    let c = q.enqueue(item(3), true).unwrap();
    assert!(!q.view().paused);
    assert_eq!(rig.await_started(2), [1, 2]);
    rig.end(2, End::Ok);
    rig.end(3, End::Ok);
    wait(&jobs, b);
    wait(&jobs, c);
    assert_eq!(rig.started(), [1, 2, 3]);
}

#[test]
fn waiting_jobs_can_be_reordered_and_running_ones_cannot() {
    let (jobs, rig) = (manager(), Rig::default());
    let q = queue(&jobs, &rig);
    q.start();
    let ids: Vec<JobId> = (1..=4)
        .map(|n| q.enqueue(item(n), false).unwrap())
        .collect();
    rig.await_started(1);
    q.reorder(ids[3], Some(ids[1])).unwrap(); // 4 before 2
    q.reorder(ids[2], None).unwrap(); // 3 to the end (it already is)
    q.reorder(ids[1], None).unwrap(); // 2 to the end
    let order: Vec<u32> = q.view().entries.iter().map(|e| e.task.0).collect();
    assert_eq!(order, [1, 4, 3, 2]);
    assert!(
        matches!(q.reorder(ids[0], None), Err(QueueError::Invalid(m)) if m.contains("running"))
    );
    assert!(matches!(
        q.reorder(ids[1], Some(ids[1])),
        Err(QueueError::Invalid(_))
    ));
    assert_eq!(
        q.reorder(ids[1], Some(JobId::from_u128(9))),
        Err(QueueError::NotFound(JobId::from_u128(9)))
    );
    for n in [1, 4, 3, 2] {
        rig.end(n, End::Ok);
    }
    for id in &ids {
        wait(&jobs, *id);
    }
    assert_eq!(rig.started(), [1, 4, 3, 2]);
}

#[test]
fn cancelling_ends_a_job_and_the_queue_goes_on() {
    let (jobs, rig) = (manager(), Rig::default());
    let q = queue(&jobs, &rig);
    q.start();
    let ids: Vec<JobId> = (1..=3)
        .map(|n| q.enqueue(item(n), false).unwrap())
        .collect();
    rig.await_started(1);
    // A waiting one ends at once, without running.
    assert_eq!(q.cancel(ids[1]).unwrap().state, JobState::Cancelled);
    // The running one stops; the next waiting one starts.
    let asked = q.cancel(ids[0]).unwrap();
    assert!(asked.cancel_requested || asked.state == JobState::Cancelled);
    assert_eq!(wait(&jobs, ids[0]).state, JobState::Cancelled);
    assert_eq!(rig.await_started(2), [1, 3]);
    rig.end(3, End::Ok);
    wait(&jobs, ids[2]);
    assert_eq!(
        states(&q),
        [
            (1, JobState::Cancelled),
            (2, JobState::Cancelled),
            (3, JobState::Succeeded)
        ]
    );
    // Cancelling a finished job changes nothing; an unknown one is not found.
    assert_eq!(q.cancel(ids[2]).unwrap().state, JobState::Succeeded);
    assert_eq!(
        q.cancel(JobId::from_u128(77)),
        Err(QueueError::NotFound(JobId::from_u128(77)))
    );
}

#[test]
fn a_retry_runs_a_failed_or_cancelled_job_again_with_its_id_and_place() {
    let (jobs, rig) = (manager(), Rig::default());
    let q = queue(&jobs, &rig);
    q.start();
    let a = q.enqueue(item(1), false).unwrap();
    rig.await_started(1);
    rig.end(1, End::Fail);
    let failed = wait(&jobs, a);
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.error.as_deref(), Some("task 1 failed"));
    q.pause(true).unwrap();
    let b = q.enqueue(item(2), false).unwrap();
    q.retry(a).unwrap();
    assert_eq!(jobs.status(a).unwrap().state, JobState::Queued);
    assert_eq!(q.view().entries[0].info.id, a, "it keeps its place");
    q.pause(false).unwrap();
    // It is ahead of job 2, so it runs first.
    assert_eq!(rig.await_started(2), [1, 1]);
    rig.end(1, End::Ok);
    assert_eq!(wait(&jobs, a).state, JobState::Succeeded);
    rig.end(2, End::Ok);
    wait(&jobs, b);
    // Success is final; running or queued jobs are not retried.
    assert!(matches!(q.retry(a), Err(QueueError::Invalid(m)) if m.contains("succeeded")));
}

#[test]
fn removing_forgets_finished_jobs_cancels_waiting_ones_and_refuses_running_ones() {
    let (jobs, rig) = (manager(), Rig::default());
    let q = queue(&jobs, &rig);
    q.start();
    let ids: Vec<JobId> = (1..=3)
        .map(|n| q.enqueue(item(n), false).unwrap())
        .collect();
    rig.await_started(1);
    assert!(
        matches!(q.remove(ids[0]), Err(QueueError::Invalid(m)) if m.contains("cancel it first"))
    );
    q.remove(ids[1]).unwrap();
    assert_eq!(jobs.status(ids[1]).unwrap().state, JobState::Cancelled);
    assert!(!q.owns(ids[1]));
    rig.end(1, End::Ok);
    wait(&jobs, ids[0]);
    q.remove(ids[0]).unwrap();
    assert_eq!(rig.await_started(2), [1, 3]);
    rig.end(3, End::Ok);
    wait(&jobs, ids[2]);
    assert_eq!(states(&q), [(3, JobState::Succeeded)]);
    assert_eq!(q.remove(ids[0]), Err(QueueError::NotFound(ids[0])));
}

#[test]
fn two_unfinished_jobs_never_write_one_file() {
    let (jobs, rig) = (manager(), Rig::default());
    let q = queue(&jobs, &rig);
    q.start();
    let a = q.enqueue(item(1), false).unwrap();
    assert!(matches!(
        q.enqueue(item(1), false),
        Err(QueueError::OutputInUse(_))
    ));
    rig.await_started(1);
    rig.end(1, End::Fail);
    wait(&jobs, a);
    // Finished: the file is free again; a retry of the failed one then collides.
    let b = q.enqueue(item(1), false).unwrap();
    assert!(matches!(q.retry(a), Err(QueueError::OutputInUse(_))));
    rig.await_started(2);
    rig.end(1, End::Ok);
    wait(&jobs, b);
}

#[test]
fn a_store_holds_the_queue_across_a_restart_and_interrupted_jobs_run_again() {
    let dir = temp("restart");
    let file = dir.join(QUEUE_FILE);
    let (one, two, three);
    {
        // The first engine: job 1 finishes, job 2 is running when the engine goes away.
        let (jobs, rig) = (manager(), Rig::default());
        let q = queue(&jobs, &rig);
        let interrupted = q.attach(
            Box::new(SqliteQueueStore::open(&file).unwrap()),
            Attach::Suspend,
        );
        assert_eq!(interrupted, Vec::<PathBuf>::new());
        q.start();
        one = q.enqueue(item(1), false).unwrap();
        two = q.enqueue(item(2), false).unwrap();
        three = q.enqueue(item(3), false).unwrap();
        rig.await_started(1);
        rig.end(1, End::Ok);
        wait(&jobs, one);
        rig.await_started(2);
        // A clean stop keeps the stored states, as a kill does.
        q.shutdown();
        jobs.cancel_all();
        wait(&jobs, two);
    }
    let stored = SqliteQueueStore::open(&file).unwrap().load().unwrap();
    let saved: Vec<(u128, JobState)> = stored
        .entries
        .iter()
        .map(|e| (e.id.as_u128(), e.state))
        .collect();
    assert_eq!(
        saved,
        [
            (one.as_u128(), JobState::Succeeded),
            (two.as_u128(), JobState::Running),
            (three.as_u128(), JobState::Queued)
        ]
    );
    // The next engine: job 1 is remembered, job 2 starts again, then job 3.
    let (jobs, rig) = (manager(), Rig::default());
    let q = queue(&jobs, &rig);
    let interrupted = q.attach(
        Box::new(SqliteQueueStore::open(&file).unwrap()),
        Attach::Suspend,
    );
    assert_eq!(interrupted, [PathBuf::from("/out/2.mp4")]);
    assert_eq!(jobs.status(one).unwrap().state, JobState::Succeeded);
    assert_eq!(jobs.status(two).unwrap().state, JobState::Queued);
    q.start();
    assert_eq!(rig.await_started(1), [2]);
    rig.end(2, End::Ok);
    assert_eq!(wait(&jobs, two).state, JobState::Succeeded);
    rig.end(3, End::Ok);
    assert_eq!(wait(&jobs, three).state, JobState::Succeeded);
    // A fresh ID never collides with a restored one.
    let four = q.enqueue(item(4), false).unwrap();
    assert!(![one, two, three].contains(&four));
    rig.end(4, End::Ok);
    wait(&jobs, four);
    drop(q);
    drop(jobs);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_failure_keeps_its_code_across_a_restart_and_a_retry_clears_it() {
    let dir = temp("codes");
    let file = dir.join(QUEUE_FILE);
    let (full, plain);
    {
        let (jobs, rig) = (manager(), Rig::default());
        let q = queue(&jobs, &rig);
        q.attach(
            Box::new(SqliteQueueStore::open(&file).unwrap()),
            Attach::Suspend,
        );
        q.start();
        full = q.enqueue(item(1), false).unwrap();
        plain = q.enqueue(item(2), false).unwrap();
        rig.await_started(1);
        rig.end(1, End::DiskFull);
        let info = wait(&jobs, full);
        assert_eq!(info.error_code.as_deref(), Some("disk_full"));
        rig.await_started(2);
        rig.end(2, End::Fail);
        assert_eq!(wait(&jobs, plain).error_code.as_deref(), Some("failed"));
        // The queue reports what the manager reported.
        let view = q.view();
        assert_eq!(
            view.entries[0].info.error_code.as_deref(),
            Some("disk_full")
        );
        assert_eq!(view.entries[1].info.error_code.as_deref(), Some("failed"));
        q.shutdown();
    }
    // The next engine reads the codes from the file (its manager never ran these jobs).
    let (jobs, rig) = (manager(), Rig::default());
    let q = queue(&jobs, &rig);
    q.attach(
        Box::new(SqliteQueueStore::open(&file).unwrap()),
        Attach::Suspend,
    );
    let restored = jobs.status(full).unwrap();
    assert_eq!(restored.state, JobState::Failed);
    assert_eq!(restored.error.as_deref(), Some("task 1 ran out of space"));
    assert_eq!(restored.error_code.as_deref(), Some("disk_full"));
    assert_eq!(
        q.entry(plain).unwrap().info.error_code.as_deref(),
        Some("failed")
    );
    // A retry forgets the failure, in the queue and in the file.
    q.retry(full).unwrap();
    let again = q.entry(full).unwrap().info;
    assert_eq!(
        (again.state, again.error, again.error_code),
        (JobState::Queued, None, None)
    );
    let stored = SqliteQueueStore::open(&file).unwrap().load().unwrap();
    assert_eq!(stored.entries[0].error_code, None);
    assert_eq!(stored.entries[1].error_code.as_deref(), Some("failed"));
    q.shutdown();
    jobs.cancel_all();
    drop(q);
    drop(jobs);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_queue_file_written_before_error_codes_opens_with_its_failures_uncoded() {
    // The file as a build before ADR-0038 wrote it: schema version 1, no `error_code` column.
    let dir = temp("old-file");
    let file = dir.join(QUEUE_FILE);
    let (old_failure, waiting) = (JobId::from_u128(0xA1), JobId::from_u128(0xA2));
    {
        let db = rusqlite::Connection::open(&file).unwrap();
        db.execute_batch(
            "CREATE TABLE export_jobs (
                id TEXT PRIMARY KEY,
                position INTEGER NOT NULL,
                label TEXT NOT NULL,
                output TEXT,
                state TEXT NOT NULL,
                error TEXT,
                payload BLOB NOT NULL
            );
            CREATE TABLE queue_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            PRAGMA user_version = 1;",
        )
        .unwrap();
        for (position, id, task, state, error) in [
            (1, old_failure, 1u32, "failed", Some("the disk is full")),
            (2, waiting, 2u32, "queued", None),
        ] {
            db.execute(
                "INSERT INTO export_jobs (id, position, label, output, state, error, payload)                  VALUES (?1, ?2, 'export', ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    id.to_string(),
                    position,
                    format!("/out/{task}.mp4"),
                    state,
                    error,
                    task.to_le_bytes().to_vec()
                ],
            )
            .unwrap();
        }
    }
    let (jobs, rig) = (manager(), Rig::default());
    let q = queue(&jobs, &rig);
    q.attach(
        Box::new(SqliteQueueStore::open(&file).unwrap()),
        Attach::Suspend,
    );
    // The old failure reads as it was written: its message, and no code.
    let old = jobs.status(old_failure).unwrap();
    assert_eq!(old.state, JobState::Failed);
    assert_eq!(old.error.as_deref(), Some("the disk is full"));
    assert_eq!(old.error_code, None);
    assert_eq!(q.view().entries.len(), 2);
    // The queued job runs; when it fails now, its code is stored in the same file.
    q.start();
    assert_eq!(rig.await_started(1), [2]);
    rig.end(2, End::DiskFull);
    assert_eq!(
        wait(&jobs, waiting).error_code.as_deref(),
        Some("disk_full")
    );
    q.shutdown();
    let stored = SqliteQueueStore::open(&file).unwrap().load().unwrap();
    assert_eq!(stored.entries[0].error_code, None);
    assert_eq!(stored.entries[1].error_code.as_deref(), Some("disk_full"));
    drop(q);
    drop(jobs);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_untitled_queue_moves_into_a_project_and_a_project_queue_stays_behind() {
    let dir = temp("switch");
    let (jobs, rig) = (manager(), Rig::default());
    let q = queue(&jobs, &rig);
    q.start();
    let a = q.enqueue(item(1), false).unwrap();
    let b = q.enqueue(item(2), false).unwrap();
    rig.await_started(1);
    // Saved as a project: the jobs move into its store and keep running.
    let first = dir.join("first.sqlite");
    q.attach(
        Box::new(SqliteQueueStore::open(&first).unwrap()),
        Attach::Carry,
    );
    assert_eq!(jobs.status(a).unwrap().state, JobState::Running);
    let ids: Vec<JobId> = SqliteQueueStore::open(&first)
        .unwrap()
        .load()
        .unwrap()
        .entries
        .iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(ids, [a, b]);
    // Another project opened: the first project's jobs stop and stay in its store.
    let second = dir.join("second.sqlite");
    q.attach(
        Box::new(SqliteQueueStore::open(&second).unwrap()),
        Attach::Suspend,
    );
    assert_eq!(wait(&jobs, a).state, JobState::Cancelled);
    assert_eq!(q.view().entries.len(), 0);
    let kept: Vec<JobState> = SqliteQueueStore::open(&first)
        .unwrap()
        .load()
        .unwrap()
        .entries
        .iter()
        .map(|e| e.state)
        .collect();
    assert_eq!(kept, [JobState::Running, JobState::Queued]);
    // Back to the first project: both run again, from the start.
    let interrupted = q.attach(
        Box::new(SqliteQueueStore::open(&first).unwrap()),
        Attach::Suspend,
    );
    assert_eq!(interrupted, [PathBuf::from("/out/1.mp4")]);
    assert_eq!(rig.await_started(2), [1, 1]);
    rig.end(1, End::Ok);
    wait(&jobs, a);
    rig.end(2, End::Ok);
    wait(&jobs, b);
    assert_eq!(rig.started(), [1, 1, 2]);
    drop(q);
    drop(jobs);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn after_shutdown_nothing_is_queued_or_written() {
    let (jobs, rig) = (manager(), Rig::default());
    let q = queue(&jobs, &rig);
    q.start();
    q.shutdown();
    assert_eq!(q.enqueue(item(1), false), Err(QueueError::ShutDown));
    assert_eq!(q.pause(true), Err(QueueError::ShutDown));
    assert_eq!(rig.started(), Vec::<u32>::new());
}
