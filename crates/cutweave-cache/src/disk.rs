// SPDX-License-Identifier: GPL-3.0-or-later
//! [`DiskCache`]: generated files (thumbnails, filmstrips) in the OS cache directory, accounted in
//! `cache.sqlite` (table `disk_entries`) and evicted least-recently-used when over a byte cap
//! (arch §7).
//!
//! The directory is disposable. Every file can be regenerated from the media, so a row whose file
//! is gone (the user cleared the cache directory) is just a miss: the row is dropped and the
//! caller regenerates. Files are written to a temporary name and renamed into place, so a reader
//! never sees a half-written file and a crash leaves at most a stray `*.tmp-*` file.
//!
//! Layout: `<root>/<kind dir>/<content hash with ':' → '-'>/<name>`; the row's `path` is the part
//! after `<root>/`, with `/` separators on every platform.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cutweave_media::ContentHash;
use rusqlite::{OptionalExtension, params};

use crate::db::next_access;
use crate::{CacheDb, CacheError, CacheUsage};

/// What a disk entry is (stored as a number: append new kinds, never renumber).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiskKind {
    /// One thumbnail image.
    Thumb,
    /// A filmstrip sprite (its manifest is the entry's `meta`).
    Filmstrip,
}

impl DiskKind {
    fn code(self) -> i64 {
        match self {
            DiskKind::Thumb => 1,
            DiskKind::Filmstrip => 2,
        }
    }

    fn dir(self) -> &'static str {
        match self {
            DiskKind::Thumb => "thumbs",
            DiskKind::Filmstrip => "filmstrips",
        }
    }
}

/// A cached file read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskEntry {
    /// The file's bytes.
    pub bytes: Vec<u8>,
    /// The text stored with it (a filmstrip's manifest).
    pub meta: String,
}

/// A size-capped LRU of files under one directory (see the module docs).
#[derive(Debug)]
pub struct DiskCache {
    db: Arc<CacheDb>,
    root: PathBuf,
    max_bytes: AtomicU64,
}

impl DiskCache {
    /// The default cap: 2 GiB.
    pub const DEFAULT_MAX_BYTES: u64 = 2 << 30;

    /// A cache of files under `root` (created on first write) accounted in `db`, holding at most
    /// `max_bytes` of file data.
    #[must_use]
    pub fn new(db: Arc<CacheDb>, root: impl Into<PathBuf>, max_bytes: u64) -> DiskCache {
        DiskCache {
            db,
            root: root.into(),
            max_bytes: AtomicU64::new(max_bytes),
        }
    }

    /// The directory the files live in.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The byte cap.
    #[must_use]
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// Changes the cap; takes effect at the next write.
    pub fn set_max_bytes(&self, max_bytes: u64) {
        self.max_bytes.store(max_bytes, Ordering::Relaxed);
    }

    fn rel(kind: DiskKind, hash: &ContentHash, name: &str) -> String {
        format!(
            "{}/{}/{name}",
            kind.dir(),
            hash.to_string().replace(':', "-")
        )
    }

    fn file(&self, rel: &str) -> PathBuf {
        rel.split('/')
            .fold(self.root.clone(), |p, part| p.join(part))
    }

    /// The entry `name` of `kind` for media `hash`, marking it most recently used. A row whose
    /// file is missing or unreadable is removed and reported as a miss.
    pub fn get(
        &self,
        kind: DiskKind,
        hash: &ContentHash,
        name: &str,
    ) -> Result<Option<DiskEntry>, CacheError> {
        let rel = DiskCache::rel(kind, hash, name);
        let meta: Option<String> = self
            .db
            .lock()
            .query_row(
                "SELECT meta FROM disk_entries WHERE path = ?1",
                params![rel],
                |r| r.get(0),
            )
            .optional()?;
        let Some(meta) = meta else { return Ok(None) };
        let Ok(bytes) = fs::read(self.file(&rel)) else {
            self.db
                .lock()
                .execute("DELETE FROM disk_entries WHERE path = ?1", params![rel])?;
            return Ok(None);
        };
        let mut conn = self.db.lock();
        let tx = conn.transaction()?;
        let seq = next_access(&tx)?;
        tx.execute(
            "UPDATE disk_entries SET last_used = ?2 WHERE path = ?1",
            params![rel, seq],
        )?;
        tx.commit()?;
        Ok(Some(DiskEntry { bytes, meta }))
    }

    /// Stores `bytes` (and `meta`) as entry `name` of `kind` for media `hash`, replacing an
    /// existing one, then evicts least-recently-used entries until the total fits the cap.
    pub fn put(
        &self,
        kind: DiskKind,
        hash: &ContentHash,
        name: &str,
        meta: &str,
        bytes: &[u8],
    ) -> Result<(), CacheError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let rel = DiskCache::rel(kind, hash, name);
        let file = self.file(&rel);
        let io = |p: &Path, e: &std::io::Error| CacheError::Io {
            path: p.display().to_string(),
            message: e.to_string(),
        };
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).map_err(|e| io(dir, &e))?;
        }
        let tmp = file.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&tmp, bytes).map_err(|e| io(&tmp, &e))?;
        if let Err(e) = fs::rename(&tmp, &file) {
            let _ = fs::remove_file(&tmp);
            return Err(io(&file, &e));
        }
        let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        let victims = {
            let mut conn = self.db.lock();
            let tx = conn.transaction()?;
            let seq = next_access(&tx)?;
            tx.execute(
                "INSERT OR REPLACE INTO disk_entries
                     (path, content_hash, kind, meta, bytes, last_used)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![rel, hash.to_string(), kind.code(), meta, size, seq],
            )?;
            let victims = evict(&tx, self.max_bytes())?;
            tx.commit()?;
            victims
        };
        for v in victims {
            let _ = fs::remove_file(self.file(&v));
        }
        Ok(())
    }

    /// Entries and bytes held.
    pub fn usage(&self) -> Result<CacheUsage, CacheError> {
        let (entries, bytes): (i64, i64) = self.db.lock().query_row(
            "SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM disk_entries",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(CacheUsage {
            entries: entries.unsigned_abs(),
            bytes: bytes.unsigned_abs(),
        })
    }

    /// Deletes every entry of media `hash` (files and rows). Returns how many.
    pub fn remove_media(&self, hash: &ContentHash) -> Result<usize, CacheError> {
        let paths: Vec<String> = {
            let conn = self.db.lock();
            let mut stmt = conn.prepare("SELECT path FROM disk_entries WHERE content_hash = ?1")?;
            let rows = stmt.query_map(params![hash.to_string()], |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        self.db.lock().execute(
            "DELETE FROM disk_entries WHERE content_hash = ?1",
            params![hash.to_string()],
        )?;
        for p in &paths {
            let _ = fs::remove_file(self.file(p));
        }
        Ok(paths.len())
    }
}

/// Deletes the oldest rows until at most `budget` bytes remain; returns their paths.
fn evict(tx: &rusqlite::Transaction<'_>, budget: u64) -> Result<Vec<String>, CacheError> {
    let mut total: i64 = tx.query_row(
        "SELECT COALESCE(SUM(bytes), 0) FROM disk_entries",
        [],
        |r| r.get(0),
    )?;
    let budget = i64::try_from(budget).unwrap_or(i64::MAX);
    let mut victims = Vec::new();
    if total > budget {
        let mut stmt = tx.prepare("SELECT path, bytes FROM disk_entries ORDER BY last_used ASC")?;
        let mut rows = stmt.query([])?;
        while total > budget {
            let Some(row) = rows.next()? else { break };
            let (path, bytes): (String, i64) = (row.get(0)?, row.get(1)?);
            total -= bytes;
            victims.push(path);
        }
    }
    for v in &victims {
        tx.execute("DELETE FROM disk_entries WHERE path = ?1", params![v])?;
    }
    Ok(victims)
}
