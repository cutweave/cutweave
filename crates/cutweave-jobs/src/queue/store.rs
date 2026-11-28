// SPDX-License-Identifier: GPL-3.0-or-later
//! Where a queue keeps its jobs between engine runs: the [`QueueStore`] seam, the in-memory
//! [`MemoryQueueStore`] (an untitled project) and [`SqliteQueueStore`]
//! (`<project>/.cutweave/exports.sqlite`, ADR-0030).
//!
//! # `exports.sqlite`
//!
//! ```text
//! export_jobs(id TEXT PRIMARY KEY,      -- job_<32 hex>
//!             position INTEGER NOT NULL, -- queue order, ascending
//!             label TEXT NOT NULL,
//!             output TEXT,               -- the file the job writes
//!             state TEXT NOT NULL,       -- queued | running | succeeded | failed | cancelled
//!             error TEXT,
//!             payload BLOB NOT NULL,     -- the job itself, encoded by the queue's codec
//!             error_code TEXT)           -- a failed job's machine code (version 2, ADR-0038)
//! queue_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL)  -- `paused`: "0" | "1"
//! ```
//!
//! WAL journal, `synchronous = NORMAL`: every write is in the file before the call returns, so
//! a killed engine loses none (a power cut may lose the last ones). The schema evolves through
//! append-only migrations counted by `PRAGMA user_version` (now 2); a database from a newer
//! build is refused (the queue then runs in memory and says so).

use std::path::{Path, PathBuf};
use std::str::FromStr;

use rusqlite::{Connection, OptionalExtension, params};

use crate::id::JobId;
use crate::types::JobState;

/// A store failure (the queue logs it and keeps going in memory where it can).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct StoreError(pub String);

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> StoreError {
        StoreError(e.to_string())
    }
}

/// One job as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredEntry {
    /// The job.
    pub id: JobId,
    /// Its label.
    pub label: String,
    /// The file it writes.
    pub output: Option<String>,
    /// Its state when last written (`running`: it was running when the engine stopped).
    pub state: JobState,
    /// Why it failed or was cancelled.
    pub error: Option<String>,
    /// A failed job's machine code (`None` for a job stored before codes existed).
    pub error_code: Option<String>,
    /// The job itself (the queue's codec).
    pub payload: Vec<u8>,
}

/// A stored queue: whether it is paused and its jobs in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredQueue {
    /// No job starts while paused.
    pub paused: bool,
    /// The jobs, first in line first.
    pub entries: Vec<StoredEntry>,
}

/// Where a queue keeps its jobs. Writes happen while the queue's lock is held: keep them
/// short.
pub trait QueueStore: Send {
    /// Whether jobs outlive the engine here (`false`: nothing is written, so the queue does not
    /// encode payloads).
    fn persistent(&self) -> bool;
    /// What the store holds.
    fn load(&mut self) -> Result<StoredQueue, StoreError>;
    /// Adds a job at the end of the queue (or replaces the job with that ID, keeping its
    /// place).
    fn insert(&mut self, entry: &StoredEntry) -> Result<(), StoreError>;
    /// Records a job's new state, with why it failed or was cancelled and a failure's machine
    /// code.
    fn update(
        &mut self,
        id: JobId,
        state: JobState,
        error: Option<&str>,
        error_code: Option<&str>,
    ) -> Result<(), StoreError>;
    /// Puts the jobs in this order (IDs it does not hold are ignored).
    fn reorder(&mut self, order: &[JobId]) -> Result<(), StoreError>;
    /// Forgets a job.
    fn remove(&mut self, id: JobId) -> Result<(), StoreError>;
    /// Records whether the queue is paused.
    fn set_paused(&mut self, paused: bool) -> Result<(), StoreError>;
    /// A label for logs (the file).
    fn describe(&self) -> String;
}

/// A queue that lives only as long as the engine (an untitled project): writes nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryQueueStore;

impl QueueStore for MemoryQueueStore {
    fn persistent(&self) -> bool {
        false
    }
    fn load(&mut self) -> Result<StoredQueue, StoreError> {
        Ok(StoredQueue::default())
    }
    fn insert(&mut self, _: &StoredEntry) -> Result<(), StoreError> {
        Ok(())
    }
    fn update(
        &mut self,
        _: JobId,
        _: JobState,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<(), StoreError> {
        Ok(())
    }
    fn reorder(&mut self, _: &[JobId]) -> Result<(), StoreError> {
        Ok(())
    }
    fn remove(&mut self, _: JobId) -> Result<(), StoreError> {
        Ok(())
    }
    fn set_paused(&mut self, _: bool) -> Result<(), StoreError> {
        Ok(())
    }
    fn describe(&self) -> String {
        "memory".to_owned()
    }
}

/// The schema version this build writes.
pub const QUEUE_SCHEMA_VERSION: i64 = 2;

/// The file name of a project's export queue in its `.cutweave` folder.
pub const QUEUE_FILE: &str = "exports.sqlite";

/// Append-only migrations: `MIGRATIONS[n]` takes the database from version `n` to `n + 1`.
const MIGRATIONS: [&str; 2] = [
    "CREATE TABLE export_jobs (
        id TEXT PRIMARY KEY,
        position INTEGER NOT NULL,
        label TEXT NOT NULL,
        output TEXT,
        state TEXT NOT NULL,
        error TEXT,
        payload BLOB NOT NULL
    );
    CREATE TABLE queue_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    // ADR-0038: a failed job's machine code. Jobs stored before it have none.
    "ALTER TABLE export_jobs ADD COLUMN error_code TEXT;",
];

/// A queue in a SQLite file (see the module docs).
#[derive(Debug)]
pub struct SqliteQueueStore {
    db: Connection,
    path: PathBuf,
}

fn state_name(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Succeeded => "succeeded",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
    }
}

fn state_of(name: &str) -> Option<JobState> {
    Some(match name {
        "queued" => JobState::Queued,
        "running" => JobState::Running,
        "succeeded" => JobState::Succeeded,
        "failed" => JobState::Failed,
        "cancelled" => JobState::Cancelled,
        _ => return None,
    })
}

impl SqliteQueueStore {
    /// Opens (or creates) the queue at `path`; its folder must exist.
    ///
    /// # Errors
    /// When the file cannot be opened or migrated, or was written by a newer build.
    pub fn open(path: &Path) -> Result<SqliteQueueStore, StoreError> {
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > QUEUE_SCHEMA_VERSION {
            return Err(StoreError(format!(
                "`{}` was written by a newer Cutweave (queue schema {version}, this build reads \
                 {QUEUE_SCHEMA_VERSION})",
                path.display()
            )));
        }
        let mut store = SqliteQueueStore {
            db,
            path: path.to_path_buf(),
        };
        let from = usize::try_from(version.max(0)).unwrap_or(0);
        for (n, sql) in MIGRATIONS.iter().enumerate().skip(from) {
            let tx = store.db.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(
                None,
                "user_version",
                i64::try_from(n + 1).unwrap_or(i64::MAX),
            )?;
            tx.commit()?;
        }
        Ok(store)
    }

    /// The file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl QueueStore for SqliteQueueStore {
    fn persistent(&self) -> bool {
        true
    }

    fn load(&mut self) -> Result<StoredQueue, StoreError> {
        let paused = self
            .db
            .query_row(
                "SELECT value FROM queue_meta WHERE key = 'paused'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .is_some_and(|v| v == "1");
        let mut stmt = self.db.prepare(
            "SELECT id, label, output, state, error, payload, error_code FROM export_jobs \
             ORDER BY position, rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Vec<u8>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let (id, label, output, state, error, payload, error_code) = row?;
            let (Ok(id), Some(state)) = (JobId::from_str(&id), state_of(&state)) else {
                tracing::warn!(id, state, file = %self.path.display(), "skipping an unreadable queued job");
                continue;
            };
            entries.push(StoredEntry {
                id,
                label,
                output,
                state,
                error,
                error_code,
                payload,
            });
        }
        Ok(StoredQueue { paused, entries })
    }

    fn insert(&mut self, entry: &StoredEntry) -> Result<(), StoreError> {
        let id = entry.id.to_string();
        let updated = self.db.execute(
            "UPDATE export_jobs SET label = ?2, output = ?3, state = ?4, error = ?5, \
             payload = ?6, error_code = ?7 WHERE id = ?1",
            params![
                id,
                entry.label,
                entry.output,
                state_name(entry.state),
                entry.error,
                entry.payload,
                entry.error_code
            ],
        )?;
        if updated == 0 {
            self.db.execute(
                "INSERT INTO export_jobs \
                 (id, position, label, output, state, error, payload, error_code) \
                 VALUES (?1, (SELECT COALESCE(MAX(position), 0) + 1 FROM export_jobs), \
                 ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    entry.label,
                    entry.output,
                    state_name(entry.state),
                    entry.error,
                    entry.payload,
                    entry.error_code
                ],
            )?;
        }
        Ok(())
    }

    fn update(
        &mut self,
        id: JobId,
        state: JobState,
        error: Option<&str>,
        error_code: Option<&str>,
    ) -> Result<(), StoreError> {
        self.db.execute(
            "UPDATE export_jobs SET state = ?2, error = ?3, error_code = ?4 WHERE id = ?1",
            params![id.to_string(), state_name(state), error, error_code],
        )?;
        Ok(())
    }

    fn reorder(&mut self, order: &[JobId]) -> Result<(), StoreError> {
        let tx = self.db.transaction()?;
        {
            let mut stmt = tx.prepare("UPDATE export_jobs SET position = ?2 WHERE id = ?1")?;
            for (n, id) in order.iter().enumerate() {
                stmt.execute(params![
                    id.to_string(),
                    i64::try_from(n + 1).unwrap_or(i64::MAX)
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn remove(&mut self, id: JobId) -> Result<(), StoreError> {
        self.db.execute(
            "DELETE FROM export_jobs WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    fn set_paused(&mut self, paused: bool) -> Result<(), StoreError> {
        self.db.execute(
            "INSERT INTO queue_meta (key, value) VALUES ('paused', ?1) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![if paused { "1" } else { "0" }],
        )?;
        Ok(())
    }

    fn describe(&self) -> String {
        self.path.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn temp_db(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cutweave-queue-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(QUEUE_FILE)
    }

    fn entry(n: u128, state: JobState) -> StoredEntry {
        StoredEntry {
            id: JobId::from_u128(n),
            label: "export".into(),
            output: Some(format!("/out/{n}.mp4")),
            state,
            error: None,
            error_code: None,
            payload: vec![u8::try_from(n).unwrap(); 3],
        }
    }

    #[test]
    fn jobs_order_states_and_the_pause_survive_reopening() {
        let path = temp_db("reopen");
        {
            let mut s = SqliteQueueStore::open(&path).unwrap();
            assert!(s.persistent());
            assert_eq!(s.load().unwrap(), StoredQueue::default());
            for n in 1..=3 {
                s.insert(&entry(n, JobState::Queued)).unwrap();
            }
            s.update(JobId::from_u128(1), JobState::Succeeded, None, None)
                .unwrap();
            s.update(
                JobId::from_u128(2),
                JobState::Failed,
                Some("disk full"),
                Some("disk_full"),
            )
            .unwrap();
            s.reorder(&[
                JobId::from_u128(1),
                JobId::from_u128(3),
                JobId::from_u128(2),
            ])
            .unwrap();
            s.set_paused(true).unwrap();
        }
        let mut s = SqliteQueueStore::open(&path).unwrap();
        let q = s.load().unwrap();
        assert!(q.paused);
        let ids: Vec<u128> = q.entries.iter().map(|e| e.id.as_u128()).collect();
        assert_eq!(ids, [1, 3, 2]);
        assert_eq!(q.entries[0].state, JobState::Succeeded);
        assert_eq!(q.entries[2].error.as_deref(), Some("disk full"));
        assert_eq!(q.entries[2].error_code.as_deref(), Some("disk_full"));
        assert_eq!(q.entries[0].error_code, None);
        assert_eq!(q.entries[1].payload, vec![3, 3, 3]);
        // Replacing keeps the place; removing forgets; new ones go last.
        s.insert(&StoredEntry {
            state: JobState::Queued,
            ..entry(3, JobState::Queued)
        })
        .unwrap();
        s.remove(JobId::from_u128(1)).unwrap();
        s.insert(&entry(4, JobState::Queued)).unwrap();
        s.set_paused(false).unwrap();
        let q = s.load().unwrap();
        assert!(!q.paused);
        let ids: Vec<u128> = q.entries.iter().map(|e| e.id.as_u128()).collect();
        assert_eq!(ids, [3, 2, 4]);
        drop(s);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The first schema, as builds before ADR-0038 created it (no `error_code` column).
    const VERSION_1: &str = "CREATE TABLE export_jobs (
            id TEXT PRIMARY KEY,
            position INTEGER NOT NULL,
            label TEXT NOT NULL,
            output TEXT,
            state TEXT NOT NULL,
            error TEXT,
            payload BLOB NOT NULL
        );
        CREATE TABLE queue_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

    #[test]
    fn a_queue_written_before_error_codes_opens_and_takes_them() {
        let path = temp_db("before-codes");
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch(VERSION_1).unwrap();
            db.pragma_update(None, "user_version", 1).unwrap();
            for (n, state, error) in [(1i64, "failed", Some("disk full")), (2, "queued", None)] {
                db.execute(
                    "INSERT INTO export_jobs (id, position, label, output, state, error, payload) \
                     VALUES (?1, ?2, 'export', '/out/a.mp4', ?3, ?4, x'010203')",
                    params![
                        JobId::from_u128(n.unsigned_abs().into()).to_string(),
                        n,
                        state,
                        error
                    ],
                )
                .unwrap();
            }
            db.execute(
                "INSERT INTO queue_meta (key, value) VALUES ('paused', '1')",
                [],
            )
            .unwrap();
        }
        let mut s = SqliteQueueStore::open(&path).unwrap();
        let q = s.load().unwrap();
        assert!(q.paused);
        assert_eq!(q.entries.len(), 2);
        // The old failure keeps its message and has no code.
        assert_eq!(q.entries[0].state, JobState::Failed);
        assert_eq!(q.entries[0].error.as_deref(), Some("disk full"));
        assert_eq!(q.entries[0].error_code, None);
        assert_eq!(q.entries[0].payload, vec![1, 2, 3]);
        // New failures are stored with their code, and the file is at this build's version.
        s.update(
            JobId::from_u128(2),
            JobState::Failed,
            Some("cannot create the file"),
            Some("output_unwritable"),
        )
        .unwrap();
        drop(s);
        let mut s = SqliteQueueStore::open(&path).unwrap();
        let q = s.load().unwrap();
        assert_eq!(
            q.entries[1].error_code.as_deref(),
            Some("output_unwritable")
        );
        assert_eq!(q.entries[0].error_code, None);
        let version: i64 =
            s.db.pragma_query_value(None, "user_version", |r| r.get(0))
                .unwrap();
        assert_eq!(version, QUEUE_SCHEMA_VERSION);
        drop(s);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let path = temp_db("newer");
        {
            let db = Connection::open(&path).unwrap();
            db.pragma_update(None, "user_version", QUEUE_SCHEMA_VERSION + 1)
                .unwrap();
        }
        let e = SqliteQueueStore::open(&path).unwrap_err();
        assert!(e.0.contains("newer Cutweave"), "{e}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn states_round_trip_through_their_names() {
        for s in [
            JobState::Queued,
            JobState::Running,
            JobState::Succeeded,
            JobState::Failed,
            JobState::Cancelled,
        ] {
            assert_eq!(state_of(state_name(s)), Some(s));
        }
        assert_eq!(state_of("done"), None);
        let mut m = MemoryQueueStore;
        assert!(!m.persistent());
        m.insert(&entry(1, JobState::Queued)).unwrap();
        assert_eq!(m.load().unwrap(), StoredQueue::default());
    }
}
