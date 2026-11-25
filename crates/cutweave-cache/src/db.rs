// SPDX-License-Identifier: GPL-3.0-or-later
//! [`CacheDb`]: `cache.sqlite` (arch §7).
//!
//! **Schema.** `PRAGMA user_version` is the schema version; [`MIGRATIONS`] holds one script per
//! version, applied in order inside one transaction each, so an interrupted upgrade leaves the
//! previous version intact. A database from a newer build is refused
//! ([`CacheError::SchemaTooNew`]) instead of being read wrongly. Migrations are append-only:
//! never edit one that shipped.
//!
//! **Tables (version 1).**
//!
//! - `meta(key, value)`: `access_seq`, a counter that orders accesses for the LRU (a counter, not
//!   a clock: the order is what matters and tests stay deterministic).
//! - `frame_index(content_hash, stream, blob_version, frames, bytes, last_used, blob)`: the
//!   serialized [`FrameIndex`] of one video stream of one file. `bytes` is the blob size, the unit
//!   of the size accounting.
//!
//! `disk_entries(path, content_hash, kind, meta, bytes, last_used)` (version 2): the files the
//! disk cache holds (see `crate::disk`), accounted and evicted by the same access counter.
//!
//! `proxies(content_hash, profile, state, bytes, frames, width, height, error, source_index,
//! proxy_index, last_used)` (version 3): proxies and their frame indexes (see `crate::proxy`).
//!
//! **Size accounting (stub).** [`CacheDb::usage`] sums `bytes`; [`CacheDb::evict_lru`] deletes the
//! least recently used entries until the total fits a budget. It covers `frame_index` only;
//! WP-1D.05 moves the accounting to a table shared by thumbnails, filmstrips and waveforms.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use cutweave_media::{BLOB_VERSION, ContentHash, FrameIndex, MediaBackend};
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};

use crate::CacheError;

/// The schema version this build writes (the number of [`MIGRATIONS`]).
pub const SCHEMA_VERSION: u32 = 3;

/// One script per schema version; script `i` upgrades version `i` to `i + 1`.
const MIGRATIONS: &[&str] = &[
    // 1: the frame index.
    "CREATE TABLE meta (
         key   TEXT PRIMARY KEY,
         value INTEGER NOT NULL
     ) WITHOUT ROWID;
     INSERT INTO meta (key, value) VALUES ('access_seq', 0);
     CREATE TABLE frame_index (
         content_hash TEXT    NOT NULL,
         stream       INTEGER NOT NULL,
         blob_version INTEGER NOT NULL,
         frames       INTEGER NOT NULL,
         bytes        INTEGER NOT NULL,
         last_used    INTEGER NOT NULL,
         blob         BLOB    NOT NULL,
         PRIMARY KEY (content_hash, stream)
     ) WITHOUT ROWID;
     CREATE INDEX frame_index_lru ON frame_index (last_used);",
    // 2: the disk cache of generated files (thumbnails, filmstrips; WP-1D.05).
    "CREATE TABLE disk_entries (
         path         TEXT    PRIMARY KEY,
         content_hash TEXT    NOT NULL,
         kind         INTEGER NOT NULL,
         meta         TEXT    NOT NULL,
         bytes        INTEGER NOT NULL,
         last_used    INTEGER NOT NULL
     ) WITHOUT ROWID;
     CREATE INDEX disk_entries_lru ON disk_entries (last_used);
     CREATE INDEX disk_entries_media ON disk_entries (content_hash);",
    // 3: proxies (WP-2F.01; see `crate::proxy`).
    "CREATE TABLE proxies (
         content_hash TEXT    NOT NULL,
         profile      TEXT    NOT NULL,
         state        INTEGER NOT NULL,
         bytes        INTEGER NOT NULL,
         frames       INTEGER NOT NULL,
         width        INTEGER NOT NULL,
         height       INTEGER NOT NULL,
         error        TEXT,
         source_index BLOB,
         proxy_index  BLOB,
         last_used    INTEGER NOT NULL,
         PRIMARY KEY (content_hash, profile)
     ) WITHOUT ROWID;
     CREATE INDEX proxies_state ON proxies (state, last_used);",
];

/// What the cache holds, for the size accounting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheUsage {
    /// Number of entries.
    pub entries: u64,
    /// Total bytes of the entries' payloads.
    pub bytes: u64,
}

/// What an [`evict_lru`](CacheDb::evict_lru) pass removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Eviction {
    /// Entries deleted.
    pub removed: u64,
    /// Payload bytes freed.
    pub freed: u64,
}

/// `cache.sqlite`. `Send + Sync`: one connection behind a mutex, so calls from several worker
/// threads serialize (each is a short statement or two). Separate processes (`cutweave worker`)
/// open their own `CacheDb` on the same file; SQLite's WAL mode and a busy timeout arbitrate.
#[derive(Debug)]
pub struct CacheDb {
    conn: Mutex<Connection>,
    path: Option<PathBuf>,
}

impl CacheDb {
    /// Opens (creating it and its parent directory if needed) the database at `path`, upgrading
    /// the schema to [`SCHEMA_VERSION`].
    ///
    /// Errors: [`CacheError::Corrupt`] for a file that is not a database,
    /// [`CacheError::SchemaTooNew`], [`CacheError::Io`], [`CacheError::Sqlite`].
    pub fn open(path: &Path) -> Result<CacheDb, CacheError> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| io_error(dir, &e))?;
        }
        let conn = Connection::open(path).map_err(|e| classify(path, e))?;
        let setup = |conn: &Connection| -> rusqlite::Result<()> {
            conn.busy_timeout(std::time::Duration::from_secs(5))?;
            // `journal_mode` returns a row; WAL lets readers and the one writer overlap.
            conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
            conn.execute_batch("PRAGMA synchronous = NORMAL;")
        };
        setup(&conn).map_err(|e| classify(path, e))?;
        CacheDb::from_connection(conn, Some(path.to_owned()), MIGRATIONS)
    }

    /// [`open`](Self::open), and if the file is damaged or from a newer build, moves it (with its
    /// `-wal` and `-shm` files) aside to `<name>.old` and starts a fresh database. The cache can
    /// always be rebuilt from the media, so losing it costs time, never data.
    pub fn open_or_reset(path: &Path) -> Result<CacheDb, CacheError> {
        match CacheDb::open(path) {
            Err(CacheError::Corrupt { .. } | CacheError::SchemaTooNew { .. }) => {
                for suffix in ["", "-wal", "-shm"] {
                    let from = suffixed(path, suffix);
                    let to = suffixed(path, &format!("{suffix}.old"));
                    if from.exists() {
                        // An older `.old` is replaced: one generation is plenty for a cache.
                        let _ = std::fs::remove_file(&to);
                        std::fs::rename(&from, &to).map_err(|e| io_error(&from, &e))?;
                    }
                }
                CacheDb::open(path)
            }
            other => other,
        }
    }

    /// A private in-memory database (tests, and tools that need no persistence).
    pub fn open_in_memory() -> Result<CacheDb, CacheError> {
        let conn = Connection::open_in_memory()?;
        CacheDb::from_connection(conn, None, MIGRATIONS)
    }

    fn from_connection(
        mut conn: Connection,
        path: Option<PathBuf>,
        migrations: &[&str],
    ) -> Result<CacheDb, CacheError> {
        let shown = path.as_deref().unwrap_or(Path::new(":memory:")).to_owned();
        migrate(&mut conn, migrations).map_err(|e| match e {
            CacheError::Sqlite(e) => classify(&shown, e),
            other => other,
        })?;
        Ok(CacheDb {
            conn: Mutex::new(conn),
            path,
        })
    }

    /// The file this database lives in (`None` for an in-memory database).
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The schema version of the open database (always [`SCHEMA_VERSION`] after a successful
    /// open).
    pub fn schema_version(&self) -> Result<u32, CacheError> {
        Ok(user_version(&self.lock())?)
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock cannot leave the database inconsistent (SQLite
        // transactions roll back), so a poisoned lock is still usable.
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stores `index` for `hash` (replacing an entry of the same stream) and marks it as the most
    /// recently used.
    pub fn put_frame_index(
        &self,
        hash: &ContentHash,
        index: &FrameIndex,
    ) -> Result<(), CacheError> {
        let blob = index.to_bytes();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let seq = next_access(&tx)?;
        tx.execute(
            "INSERT OR REPLACE INTO frame_index
                 (content_hash, stream, blob_version, frames, bytes, last_used, blob)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                hash.to_string(),
                index.stream(),
                BLOB_VERSION,
                i64::try_from(index.frame_count()).unwrap_or(i64::MAX),
                i64::try_from(blob.len()).unwrap_or(i64::MAX),
                seq,
                blob
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The stored index of video stream `stream` of the file with `hash`, marking it as recently
    /// used. `None` if there is none, or if it was written in an older blob format or is damaged
    /// (the entry is deleted: the caller rebuilds it).
    pub fn get_frame_index(
        &self,
        hash: &ContentHash,
        stream: u32,
    ) -> Result<Option<FrameIndex>, CacheError> {
        self.load_frame_index(hash, Some(stream))
    }

    /// The stored index of the file with `hash` whatever its stream: the one [`index_media`]
    /// built, which indexes the main video stream. (If several streams are stored, the lowest
    /// stream number.) Same hit, miss and recency rules as [`get_frame_index`].
    ///
    /// [`index_media`]: MediaBackend::index_media
    /// [`get_frame_index`]: Self::get_frame_index
    pub fn get_main_frame_index(
        &self,
        hash: &ContentHash,
    ) -> Result<Option<FrameIndex>, CacheError> {
        self.load_frame_index(hash, None)
    }

    fn load_frame_index(
        &self,
        hash: &ContentHash,
        stream: Option<u32>,
    ) -> Result<Option<FrameIndex>, CacheError> {
        let key = hash.to_string();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let row: Option<(u32, u8, Vec<u8>)> = tx
            .query_row(
                "SELECT stream, blob_version, blob FROM frame_index
                 WHERE content_hash = ?1 AND (?2 IS NULL OR stream = ?2)
                 ORDER BY stream LIMIT 1",
                params![key, stream],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((stream, version, blob)) = row else {
            return Ok(None);
        };
        let index = if version == BLOB_VERSION {
            FrameIndex::from_bytes(&blob).ok()
        } else {
            None
        };
        if index.is_some() {
            let seq = next_access(&tx)?;
            tx.execute(
                "UPDATE frame_index SET last_used = ?3 WHERE content_hash = ?1 AND stream = ?2",
                params![key, stream, seq],
            )?;
        } else {
            tx.execute(
                "DELETE FROM frame_index WHERE content_hash = ?1 AND stream = ?2",
                params![key, stream],
            )?;
        }
        tx.commit()?;
        Ok(index)
    }

    /// The index of the main video stream of `path`: the stored one for `hash`, or, if there is
    /// none, built by `backend` (a packet scan) and stored.
    ///
    /// Concurrent callers for the same file may both build it; the results are identical and the
    /// last one stored wins.
    pub fn frame_index_for(
        &self,
        backend: &dyn MediaBackend,
        path: &Path,
        hash: &ContentHash,
    ) -> Result<FrameIndex, CacheError> {
        if let Some(index) = self.get_main_frame_index(hash)? {
            return Ok(index);
        }
        let index = backend.index_media(path)?;
        self.put_frame_index(hash, &index)?;
        Ok(index)
    }

    /// Deletes the stored indexes of the file with `hash` (every stream). Returns how many.
    pub fn remove_frame_indexes(&self, hash: &ContentHash) -> Result<usize, CacheError> {
        Ok(self.lock().execute(
            "DELETE FROM frame_index WHERE content_hash = ?1",
            params![hash.to_string()],
        )?)
    }

    /// Entries and bytes held.
    pub fn usage(&self) -> Result<CacheUsage, CacheError> {
        let (entries, bytes): (i64, i64) = self.lock().query_row(
            "SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM frame_index",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(CacheUsage {
            entries: entries.unsigned_abs(),
            bytes: bytes.unsigned_abs(),
        })
    }

    /// Deletes the least recently used entries until at most `max_bytes` remain.
    pub fn evict_lru(&self, max_bytes: u64) -> Result<Eviction, CacheError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut total: i64 =
            tx.query_row("SELECT COALESCE(SUM(bytes), 0) FROM frame_index", [], |r| {
                r.get(0)
            })?;
        let budget = i64::try_from(max_bytes).unwrap_or(i64::MAX);
        let mut victims = Vec::new();
        if total > budget {
            let mut stmt = tx.prepare(
                "SELECT content_hash, stream, bytes FROM frame_index ORDER BY last_used ASC",
            )?;
            let mut rows = stmt.query([])?;
            while total > budget {
                let Some(row) = rows.next()? else { break };
                let (hash, stream, bytes): (String, u32, i64) =
                    (row.get(0)?, row.get(1)?, row.get(2)?);
                total -= bytes;
                victims.push((hash, stream, bytes));
            }
        }
        let mut out = Eviction::default();
        for (hash, stream, bytes) in victims {
            tx.execute(
                "DELETE FROM frame_index WHERE content_hash = ?1 AND stream = ?2",
                params![hash, stream],
            )?;
            out.removed += 1;
            out.freed += bytes.unsigned_abs();
        }
        tx.commit()?;
        Ok(out)
    }
}

pub(crate) fn next_access(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<i64> {
    tx.query_row(
        "UPDATE meta SET value = value + 1 WHERE key = 'access_seq' RETURNING value",
        [],
        |r| r.get(0),
    )
}

fn user_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// Brings `conn` to `migrations.len()`.
fn migrate(conn: &mut Connection, migrations: &[&str]) -> Result<(), CacheError> {
    let supported = u32::try_from(migrations.len()).unwrap_or(u32::MAX);
    let found = user_version(conn)?;
    if found > supported {
        return Err(CacheError::SchemaTooNew { found, supported });
    }
    for (i, script) in migrations.iter().enumerate().skip(found as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(script)?;
        // `user_version` cannot be bound as a parameter; the number is ours.
        tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
        tx.commit()?;
    }
    Ok(())
}

/// A SQLite failure as a [`CacheError`]: "not a database" and "corrupt" get their own kind.
fn classify(path: &Path, e: rusqlite::Error) -> CacheError {
    match e.sqlite_error_code() {
        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt) => CacheError::Corrupt {
            path: path.to_string_lossy().into_owned(),
        },
        _ => CacheError::Sqlite(e),
    }
}

fn io_error(path: &Path, e: &std::io::Error) -> CacheError {
    CacheError::Io {
        path: path.to_string_lossy().into_owned(),
        message: e.to_string(),
    }
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use cutweave_media::testing::contract::TempDir;
    use cutweave_media::testing::{FakeAudio, FakeMedia};
    use cutweave_media::{FakeBackend, HashMode, MediaError, content_hash};
    use cutweave_model::{Duration, FrameRate, SampleRate};

    use super::*;

    fn secs(s: i64) -> Duration {
        Duration::from_secs(s).expect("duration")
    }

    /// A real file (for hashing) registered in a fake backend as a 2 s 30 fps clip.
    struct Clip {
        dir: TempDir,
        path: PathBuf,
        hash: ContentHash,
        backend: FakeBackend,
    }

    fn clip(tag: &str, content: &[u8]) -> Clip {
        let dir = TempDir::new(tag);
        let path = dir.path().join("clip.mp4");
        std::fs::write(&path, content).expect("write");
        let hash = content_hash(&path, HashMode::Fast).expect("hash");
        let backend = FakeBackend::new()
            .with_media(path.clone(), FakeMedia::clip(secs(2), FrameRate::FPS_30));
        Clip {
            dir,
            path,
            hash,
            backend,
        }
    }

    #[test]
    fn open_creates_the_schema_and_reopen_keeps_the_data() {
        let c = clip("cache-open", b"one");
        let db_path = c.dir.path().join("sub").join("cache.sqlite");
        let index = c.backend.index_media(&c.path).expect("index");
        {
            let db = CacheDb::open(&db_path).expect("open");
            assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
            assert_eq!(db.path(), Some(db_path.as_path()));
            assert_eq!(db.usage().expect("usage"), CacheUsage::default());
            db.put_frame_index(&c.hash, &index).expect("put");
        }
        let db = CacheDb::open(&db_path).expect("reopen");
        assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
        assert_eq!(db.get_frame_index(&c.hash, 0).expect("get"), Some(index));
        assert_eq!(db.get_frame_index(&c.hash, 1).expect("get"), None);
    }

    #[test]
    fn migrations_apply_in_order_and_only_once() {
        let mut conn = Connection::open_in_memory().expect("open");
        let v1 = "CREATE TABLE a (x INTEGER); INSERT INTO a VALUES (1);";
        let v2 = "ALTER TABLE a ADD COLUMN y INTEGER DEFAULT 7;";
        migrate(&mut conn, &[v1]).expect("v1");
        assert_eq!(user_version(&conn).expect("version"), 1);
        // Upgrading runs only the new script: re-running v1 would fail on `CREATE TABLE a`.
        migrate(&mut conn, &[v1, v2]).expect("v2");
        assert_eq!(user_version(&conn).expect("version"), 2);
        let y: i64 = conn
            .query_row("SELECT y FROM a", [], |r| r.get(0))
            .expect("row");
        assert_eq!(y, 7);
        migrate(&mut conn, &[v1, v2]).expect("idempotent");
        // A downgrade is refused.
        assert!(matches!(
            migrate(&mut conn, &[v1]),
            Err(CacheError::SchemaTooNew {
                found: 2,
                supported: 1
            })
        ));
    }

    #[test]
    fn a_failing_migration_leaves_the_previous_version() {
        let mut conn = Connection::open_in_memory().expect("open");
        let bad = "CREATE TABLE b (x INTEGER); THIS IS NOT SQL;";
        assert!(migrate(&mut conn, &["CREATE TABLE a (x INTEGER);", bad]).is_err());
        assert_eq!(user_version(&conn).expect("version"), 1);
        let has_b: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'b'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(has_b, 0, "the half-applied script rolled back");
    }

    #[test]
    fn newer_and_damaged_files_are_refused_or_moved_aside() {
        let dir = TempDir::new("cache-reset");
        // A database from the future.
        let newer = dir.path().join("newer.sqlite");
        {
            let conn = Connection::open(&newer).expect("open");
            conn.execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 5))
                .expect("set");
        }
        assert!(matches!(
            CacheDb::open(&newer),
            Err(CacheError::SchemaTooNew { .. })
        ));
        let db = CacheDb::open_or_reset(&newer).expect("reset");
        assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
        assert!(suffixed(&newer, ".old").exists(), "the old file is kept");

        // Not a database at all.
        let junk = dir.path().join("junk.sqlite");
        std::fs::write(&junk, vec![0x5a; 4096]).expect("write");
        assert!(matches!(
            CacheDb::open(&junk),
            Err(CacheError::Corrupt { .. })
        ));
        CacheDb::open_or_reset(&junk).expect("reset");
        assert!(suffixed(&junk, ".old").exists());
        CacheDb::open(&junk).expect("the fresh file opens");
    }

    #[test]
    fn indexes_are_stored_per_hash_and_replaced_on_put() {
        let a = clip("cache-a", b"alpha");
        let b = clip("cache-b", b"bravo");
        assert_ne!(a.hash, b.hash);
        let db = CacheDb::open_in_memory().expect("open");
        let ia = a.backend.index_media(&a.path).expect("index");
        db.put_frame_index(&a.hash, &ia).expect("put");
        db.put_frame_index(&a.hash, &ia).expect("put again");
        assert_eq!(db.usage().expect("usage").entries, 1);
        assert_eq!(db.get_frame_index(&b.hash, 0).expect("get"), None);
        assert_eq!(db.get_frame_index(&a.hash, 0).expect("get"), Some(ia));
        assert_eq!(db.remove_frame_indexes(&a.hash).expect("remove"), 1);
        assert_eq!(db.get_frame_index(&a.hash, 0).expect("get"), None);
    }

    #[test]
    fn damaged_or_old_blobs_are_misses_and_are_deleted() {
        let c = clip("cache-stale", b"x");
        let db = CacheDb::open_in_memory().expect("open");
        let index = c.backend.index_media(&c.path).expect("index");
        db.put_frame_index(&c.hash, &index).expect("put");
        // Flip a byte of the stored blob.
        db.lock()
            .execute(
                "UPDATE frame_index SET blob = CAST(substr(blob, 1, 10) || x'ff' || substr(blob, 12) AS BLOB)",
                [],
            )
            .expect("damage");
        assert_eq!(db.get_frame_index(&c.hash, 0).expect("get"), None);
        assert_eq!(
            db.usage().expect("usage").entries,
            0,
            "the bad entry is gone"
        );

        // An entry from an older blob format.
        db.put_frame_index(&c.hash, &index).expect("put");
        db.lock()
            .execute("UPDATE frame_index SET blob_version = 0", [])
            .expect("age");
        assert_eq!(db.get_frame_index(&c.hash, 0).expect("get"), None);
        assert_eq!(db.usage().expect("usage").entries, 0);
    }

    #[test]
    fn frame_index_for_builds_once_then_serves_from_the_cache() {
        let c = clip("cache-for", b"content");
        let db = CacheDb::open_in_memory().expect("open");
        let built = db
            .frame_index_for(&c.backend, &c.path, &c.hash)
            .expect("build");
        assert_eq!(built.frame_count(), 60);
        assert_eq!(db.usage().expect("usage").entries, 1);
        // A backend that has never heard of the file still answers from the cache.
        let cached = db
            .frame_index_for(&FakeBackend::new(), &c.path, &c.hash)
            .expect("cached");
        assert_eq!(cached, built);
        assert_eq!(db.usage().expect("usage").entries, 1, "not stored twice");
    }

    #[test]
    fn frame_index_for_reports_files_without_pictures_and_backend_errors() {
        let dir = TempDir::new("cache-nopic");
        let path = dir.path().join("tone.wav");
        std::fs::write(&path, b"x").expect("write");
        let hash = content_hash(&path, HashMode::Fast).expect("hash");
        let backend = FakeBackend::new().with_media(
            path.clone(),
            FakeMedia::audio(FakeAudio::tone(SampleRate::HZ_48_000, 1, secs(1))),
        );
        let db = CacheDb::open_in_memory().expect("open");
        assert!(matches!(
            db.frame_index_for(&backend, &path, &hash),
            Err(CacheError::Media(MediaError::NoStream { .. }))
        ));
        assert_eq!(db.usage().expect("usage").entries, 0);
    }

    #[test]
    fn eviction_removes_the_least_recently_used_first() {
        let db = CacheDb::open_in_memory().expect("open");
        let dir = TempDir::new("cache-lru");
        let mut hashes = Vec::new();
        let mut size = 0;
        for i in 0..4_u8 {
            let path = dir.path().join(format!("{i}.mp4"));
            std::fs::write(&path, [i; 16]).expect("write");
            let hash = content_hash(&path, HashMode::Fast).expect("hash");
            let backend = FakeBackend::new()
                .with_media(path.clone(), FakeMedia::clip(secs(2), FrameRate::FPS_30));
            let index = backend.index_media(&path).expect("index");
            size = index.to_bytes().len() as u64;
            db.put_frame_index(&hash, &index).expect("put");
            hashes.push(hash);
        }
        // Touch the oldest: it is now the most recently used.
        assert!(db.get_frame_index(&hashes[0], 0).expect("get").is_some());
        assert_eq!(
            db.usage().expect("usage"),
            CacheUsage {
                entries: 4,
                bytes: 4 * size
            }
        );
        // Room for two: entries 1 and 2 (the least recently used) go.
        let ev = db.evict_lru(2 * size).expect("evict");
        assert_eq!(
            ev,
            Eviction {
                removed: 2,
                freed: 2 * size
            }
        );
        let alive: Vec<bool> = hashes
            .iter()
            .map(|h| db.get_frame_index(h, 0).expect("get").is_some())
            .collect();
        assert_eq!(alive, [true, false, false, true]);
        // Within budget: nothing happens.
        assert_eq!(db.evict_lru(u64::MAX).expect("evict"), Eviction::default());
        assert_eq!(db.evict_lru(0).expect("evict").removed, 2);
        assert_eq!(db.usage().expect("usage"), CacheUsage::default());
    }

    #[test]
    fn the_db_is_shareable_across_threads() {
        let c = clip("cache-threads", b"t");
        let db = std::sync::Arc::new(CacheDb::open_in_memory().expect("open"));
        let index = c.backend.index_media(&c.path).expect("index");
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let (db, hash, index) = (db.clone(), c.hash, index.clone());
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        db.put_frame_index(&hash, &index).expect("put");
                        assert!(db.get_frame_index(&hash, 0).expect("get").is_some());
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("thread");
        }
        assert_eq!(db.usage().expect("usage").entries, 1);
    }
}
