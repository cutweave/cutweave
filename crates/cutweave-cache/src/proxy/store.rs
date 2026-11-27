// SPDX-License-Identifier: GPL-3.0-or-later
//! [`ProxyStore`]: the proxies in the global cache: their files under `<cache>/proxies` and
//! their rows in `cache.sqlite` (table `proxies`, migration 3), keyed by the original's content
//! hash and the [`ProxyProfile`] key.
//!
//! **States.** A row is `building` from the first part on (the generator writes parts into
//! `<dir>/parts/` and joins them into `<dir>/proxy.mov`), `ready` once the joined file and its
//! two frame indexes (the proxy's and the original's: the `pts_map`, frame `n` of the proxy is
//! frame `n` of the original) are stored, `evicted` when its file was removed (the size cap, a
//! clear, `media.delete_proxy`: the original is used again; a new generation brings it back)
//! and `failed` with the error of the last attempt. A `building` row whose generation is not
//! running is an interrupted one (a cancel, a crash): its finished parts are kept and the next
//! generation continues from them.
//!
//! **Size cap.** Ready proxies are evicted least recently used first (`last_used` is the
//! database's access counter, bumped by [`ProxyStore::ready`]) whenever one is finished or the
//! cap is lowered, until the total fits; a proxy larger than the cap alone is evicted at once.
//! Parts of proxies being built are not counted.
//!
//! The directory is disposable like every cache: a row whose file is gone reads as evicted.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cutweave_media::{ContentHash, FrameIndex};
use rusqlite::{OptionalExtension, params};

use super::ProxyProfile;
use crate::db::next_access;
use crate::{CacheDb, CacheError, CacheUsage, Eviction};

/// The joined proxy file of a proxy directory.
const PROXY_FILE: &str = "proxy.mov";
/// The parts subdirectory of a proxy directory.
const PARTS_DIR: &str = "parts";

/// Where a proxy is in its life (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProxyState {
    /// Being made, or interrupted (parts may exist).
    Building,
    /// Usable.
    Ready,
    /// Removed from the cache; the original is used.
    Evicted,
    /// The last attempt failed.
    Failed,
}

impl ProxyState {
    fn code(self) -> i64 {
        match self {
            ProxyState::Building => 1,
            ProxyState::Ready => 2,
            ProxyState::Evicted => 3,
            ProxyState::Failed => 4,
        }
    }

    fn of(code: i64) -> ProxyState {
        match code {
            1 => ProxyState::Building,
            2 => ProxyState::Ready,
            4 => ProxyState::Failed,
            _ => ProxyState::Evicted,
        }
    }
}

/// A proxy row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyRecord {
    /// The original's content hash.
    pub hash: ContentHash,
    /// The profile key (`prores_proxy-960`).
    pub profile: String,
    /// Its state.
    pub state: ProxyState,
    /// The joined file (it exists only while `ready`).
    pub path: PathBuf,
    /// Size of the joined file (0 unless `ready`).
    pub bytes: u64,
    /// Frames (the original's).
    pub frames: u64,
    /// Picture size.
    pub size: (u32, u32),
    /// Why the last attempt failed (`failed` only).
    pub error: Option<String>,
}

/// A ready proxy with what decoding it needs (see [`ProxyStore::ready`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ReadyProxy {
    /// Its row.
    pub record: ProxyRecord,
    /// A content hash standing for the proxy file (derived from the original's hash and the
    /// profile): the key of its decoded frames and frame index, never the original's.
    pub identity: ContentHash,
    /// The proxy file's frame index (constant frame rate, every frame a keyframe).
    pub index: Arc<FrameIndex>,
    /// The original's frame index when the proxy was made: the `pts_map`. Proxy frame `n` is
    /// original frame `n`, shown at `source_index.time_of(n)`.
    pub source_index: Arc<FrameIndex>,
}

impl ReadyProxy {
    /// The proxy frame shown at original media time `t` (the last frame past the end), and
    /// that frame's original time.
    #[must_use]
    pub fn frame_at(&self, t: cutweave_model::Time) -> Option<(usize, cutweave_model::Time)> {
        let last = self.source_index.frame_count().checked_sub(1)?;
        let n = self.source_index.frame_at(t).min(last);
        Some((n, self.source_index.time_of(n)?))
    }
}

impl cutweave_media::ProxyFile for ReadyProxy {
    fn long_edge(&self) -> u32 {
        self.record.size.0.max(self.record.size.1)
    }
}

/// The proxies of a cache directory (see the module docs). `Send + Sync`.
#[derive(Debug)]
pub struct ProxyStore {
    db: Arc<CacheDb>,
    root: PathBuf,
    max_bytes: AtomicU64,
    /// Bumped whenever a proxy becomes ready or stops being ready, so a reader holding one
    /// knows to look again.
    generation: AtomicU64,
}

/// The stable directory name of a content hash (`blake3-fast-<hex>`).
fn hash_dir(hash: &ContentHash) -> String {
    hash.to_string().replace(':', "-")
}

fn io_error(path: &Path, e: &std::io::Error) -> CacheError {
    CacheError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

fn count(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// Rows `(key, bytes, last_used)` to evict, oldest first, so that at most `budget` bytes stay.
#[must_use]
pub fn plan_eviction<K: Clone>(rows: &[(K, u64, i64)], budget: u64) -> Vec<K> {
    let mut sorted: Vec<&(K, u64, i64)> = rows.iter().collect();
    sorted.sort_by_key(|r| r.2);
    let mut total: u64 = rows.iter().map(|r| r.1).sum();
    let mut out = Vec::new();
    for (key, bytes, _) in sorted {
        if total <= budget {
            break;
        }
        total = total.saturating_sub(*bytes);
        out.push(key.clone());
    }
    out
}

impl ProxyStore {
    /// The default cap: 50 GiB (the "Proxies" limit of Settings > Cache and storage).
    pub const DEFAULT_MAX_BYTES: u64 = 50 << 30;

    /// Proxies under `root` (created on first write) accounted in `db`, holding at most
    /// `max_bytes` of ready proxies.
    #[must_use]
    pub fn new(db: Arc<CacheDb>, root: impl Into<PathBuf>, max_bytes: u64) -> ProxyStore {
        ProxyStore {
            db,
            root: root.into(),
            max_bytes: AtomicU64::new(max_bytes),
            generation: AtomicU64::new(0),
        }
    }

    /// The directory the proxies live in.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The database.
    #[must_use]
    pub fn db(&self) -> &Arc<CacheDb> {
        &self.db
    }

    /// The byte cap.
    #[must_use]
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// Changes in which proxies are ready (a counter; compare with an earlier value).
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// The directory of the proxy of `hash` in `profile`.
    #[must_use]
    pub fn dir(&self, hash: &ContentHash, profile: &str) -> PathBuf {
        self.root.join(hash_dir(hash)).join(profile)
    }

    /// The joined file of the proxy of `hash` in `profile`.
    #[must_use]
    pub fn file(&self, hash: &ContentHash, profile: &str) -> PathBuf {
        self.dir(hash, profile).join(PROXY_FILE)
    }

    /// Part `k` of the proxy of `hash` in `profile` (while building).
    #[must_use]
    pub fn part(&self, hash: &ContentHash, profile: &str, k: usize) -> PathBuf {
        self.dir(hash, profile)
            .join(PARTS_DIR)
            .join(format!("{k:05}.mov"))
    }

    /// The content hash standing for the proxy file of `hash` in `profile`.
    #[must_use]
    pub fn identity(hash: &ContentHash, profile: &str) -> ContentHash {
        let digest = blake3::hash(format!("cutweave-proxy:{hash}:{profile}").as_bytes());
        ContentHash::from_str(&format!("blake3-fast:{}", digest.to_hex())).unwrap_or(*hash)
    }

    fn record(&self, row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<ProxyRecord>> {
        let hash: String = row.get(0)?;
        let Ok(hash) = ContentHash::from_str(&hash) else {
            return Ok(None);
        };
        let profile: String = row.get(1)?;
        let (bytes, frames, w, h): (i64, i64, i64, i64) =
            (row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?);
        Ok(Some(ProxyRecord {
            path: self.file(&hash, &profile),
            hash,
            profile,
            state: ProxyState::of(row.get(2)?),
            bytes: bytes.unsigned_abs(),
            frames: frames.unsigned_abs(),
            size: (u32::try_from(w).unwrap_or(0), u32::try_from(h).unwrap_or(0)),
            error: row.get(7)?,
        }))
    }

    const COLUMNS: &'static str =
        "content_hash, profile, state, bytes, frames, width, height, error";

    /// The row of the proxy of `hash` in `profile`, if any (no recency change).
    pub fn get(
        &self,
        hash: &ContentHash,
        profile: &str,
    ) -> Result<Option<ProxyRecord>, CacheError> {
        let conn = self.db.lock();
        let row = conn
            .query_row(
                &format!(
                    "SELECT {} FROM proxies WHERE content_hash = ?1 AND profile = ?2",
                    Self::COLUMNS
                ),
                params![hash.to_string(), profile],
                |r| self.record(r),
            )
            .optional()?;
        Ok(row.flatten())
    }

    /// Every row in `state`.
    pub fn list(&self, state: ProxyState) -> Result<Vec<ProxyRecord>, CacheError> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM proxies WHERE state = ?1 ORDER BY last_used",
            Self::COLUMNS
        ))?;
        let rows = stmt.query_map(params![state.code()], |r| self.record(r))?;
        let mut out = Vec::new();
        for row in rows {
            if let Some(r) = row? {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// The ready proxy of `hash` in `profile` with its indexes, marking it most recently used.
    /// `None` when it is not ready; a ready row whose file is gone or whose indexes do not load
    /// is marked evicted.
    pub fn ready(
        &self,
        hash: &ContentHash,
        profile: &str,
    ) -> Result<Option<Arc<ReadyProxy>>, CacheError> {
        let key = hash.to_string();
        let row: Option<(i64, Vec<u8>, Vec<u8>)> = self
            .db
            .lock()
            .query_row(
                "SELECT state, source_index, proxy_index FROM proxies
                 WHERE content_hash = ?1 AND profile = ?2",
                params![key, profile],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get::<_, Option<Vec<u8>>>(1)?.unwrap_or_default(),
                        r.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default(),
                    ))
                },
            )
            .optional()?;
        let Some((state, source, proxy)) = row else {
            return Ok(None);
        };
        if ProxyState::of(state) != ProxyState::Ready {
            return Ok(None);
        }
        let file = self.file(hash, profile);
        let indexes = FrameIndex::from_bytes(&source)
            .ok()
            .zip(FrameIndex::from_bytes(&proxy).ok());
        let Some((source_index, index)) = indexes.filter(|_| file.is_file()) else {
            self.evict(hash, profile)?;
            return Ok(None);
        };
        {
            let mut conn = self.db.lock();
            let tx = conn.transaction()?;
            let seq = next_access(&tx)?;
            tx.execute(
                "UPDATE proxies SET last_used = ?3 WHERE content_hash = ?1 AND profile = ?2",
                params![key, profile, seq],
            )?;
            tx.commit()?;
        }
        let Some(record) = self.get(hash, profile)? else {
            return Ok(None);
        };
        Ok(Some(Arc::new(ReadyProxy {
            record,
            identity: ProxyStore::identity(hash, profile),
            index: Arc::new(index),
            source_index: Arc::new(source_index),
        })))
    }

    /// Starts (or continues) building the proxy of `hash` in `profile`: the row becomes
    /// `building` (finished parts are kept) and its parts directory exists. Returns that
    /// directory.
    pub fn begin(
        &self,
        hash: &ContentHash,
        profile: &ProxyProfile,
        frames: u64,
        size: (u32, u32),
    ) -> Result<PathBuf, CacheError> {
        let key = profile.key();
        let parts = self.dir(hash, &key).join(PARTS_DIR);
        fs::create_dir_all(&parts).map_err(|e| io_error(&parts, &e))?;
        let was_ready = {
            let mut conn = self.db.lock();
            let tx = conn.transaction()?;
            let seq = next_access(&tx)?;
            let before: Option<i64> = tx
                .query_row(
                    "SELECT state FROM proxies WHERE content_hash = ?1 AND profile = ?2",
                    params![hash.to_string(), key],
                    |r| r.get(0),
                )
                .optional()?;
            tx.execute(
                "INSERT INTO proxies
                     (content_hash, profile, state, bytes, frames, width, height, error,
                      source_index, proxy_index, last_used)
                 VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, NULL, NULL, NULL, ?7)
                 ON CONFLICT (content_hash, profile) DO UPDATE SET
                     state = ?3, bytes = 0, frames = ?4, width = ?5, height = ?6,
                     error = NULL, source_index = NULL, proxy_index = NULL, last_used = ?7",
                params![
                    hash.to_string(),
                    key,
                    ProxyState::Building.code(),
                    count(frames),
                    i64::from(size.0),
                    i64::from(size.1),
                    seq
                ],
            )?;
            tx.commit()?;
            before.map(ProxyState::of) == Some(ProxyState::Ready)
        };
        if was_ready {
            self.bump();
        }
        Ok(parts)
    }

    /// Records the joined file of the proxy of `hash` in `profile` (already at
    /// [`file`](Self::file)) as ready with its indexes, removes the parts, and evicts least
    /// recently used proxies until the cap holds. Returns the proxy's row now and the rows
    /// evicted ([`enforce_limit`](Self::enforce_limit); the proxy itself among them when it
    /// alone is over the cap).
    pub fn finish(
        &self,
        hash: &ContentHash,
        profile: &str,
        index: &FrameIndex,
        source_index: &FrameIndex,
    ) -> Result<(ProxyRecord, Vec<ProxyRecord>), CacheError> {
        let file = self.file(hash, profile);
        let bytes = fs::metadata(&file).map_err(|e| io_error(&file, &e))?.len();
        {
            let mut conn = self.db.lock();
            let tx = conn.transaction()?;
            let seq = next_access(&tx)?;
            tx.execute(
                "UPDATE proxies SET state = ?3, bytes = ?4, frames = ?5, error = NULL,
                     source_index = ?6, proxy_index = ?7, last_used = ?8
                 WHERE content_hash = ?1 AND profile = ?2",
                params![
                    hash.to_string(),
                    profile,
                    ProxyState::Ready.code(),
                    count(bytes),
                    count(index.frame_count() as u64),
                    source_index.to_bytes(),
                    index.to_bytes(),
                    seq
                ],
            )?;
            tx.commit()?;
        }
        let parts = self.dir(hash, profile).join(PARTS_DIR);
        let _ = fs::remove_dir_all(&parts);
        self.bump();
        let evicted = self.enforce_limit()?;
        let record = self.get(hash, profile)?.ok_or_else(|| CacheError::Io {
            path: file.display().to_string(),
            message: "the proxy row disappeared".to_owned(),
        })?;
        Ok((record, evicted))
    }

    /// Records that building the proxy of `hash` in `profile` failed with `error` (its parts
    /// are removed: the next attempt starts over).
    pub fn fail(&self, hash: &ContentHash, profile: &str, error: &str) -> Result<(), CacheError> {
        self.db.lock().execute(
            "UPDATE proxies SET state = ?3, error = ?4, bytes = 0 WHERE content_hash = ?1 AND profile = ?2",
            params![hash.to_string(), profile, ProxyState::Failed.code(), error],
        )?;
        let _ = fs::remove_dir_all(self.dir(hash, profile));
        Ok(())
    }

    /// Removes the proxy of `hash` in `profile` (its files; the row becomes `evicted`). Returns
    /// whether there was one.
    pub fn evict(&self, hash: &ContentHash, profile: &str) -> Result<bool, CacheError> {
        let changed = self.db.lock().execute(
            "UPDATE proxies SET state = ?3, bytes = 0, source_index = NULL, proxy_index = NULL
             WHERE content_hash = ?1 AND profile = ?2 AND state != ?3",
            params![hash.to_string(), profile, ProxyState::Evicted.code()],
        )?;
        let _ = fs::remove_dir_all(self.dir(hash, profile));
        if changed > 0 {
            self.bump();
        }
        Ok(changed > 0)
    }

    /// Evicts least recently used ready proxies until the total fits the cap. Returns the rows
    /// evicted, `evicted`, with the bytes each freed.
    pub fn enforce_limit(&self) -> Result<Vec<ProxyRecord>, CacheError> {
        let budget = self.max_bytes();
        let ready = self.list(ProxyState::Ready)?;
        let rows: Vec<(usize, u64, i64)> = {
            let conn = self.db.lock();
            let mut stmt = conn.prepare(
                "SELECT last_used FROM proxies WHERE content_hash = ?1 AND profile = ?2",
            )?;
            let mut rows = Vec::with_capacity(ready.len());
            for (i, r) in ready.iter().enumerate() {
                let last: i64 =
                    stmt.query_row(params![r.hash.to_string(), r.profile], |row| row.get(0))?;
                rows.push((i, r.bytes, last));
            }
            rows
        };
        let mut evicted = Vec::new();
        for i in plan_eviction(&rows, budget) {
            let r = &ready[i];
            self.evict(&r.hash, &r.profile)?;
            tracing::info!(
                hash = %r.hash,
                profile = %r.profile,
                bytes = r.bytes,
                "proxy evicted to keep the cache under its limit"
            );
            evicted.push(ProxyRecord {
                state: ProxyState::Evicted,
                ..r.clone()
            });
        }
        Ok(evicted)
    }

    /// Changes the cap and evicts at once until the total fits it.
    pub fn shrink_to(&self, max_bytes: u64) -> Result<Eviction, CacheError> {
        self.max_bytes.store(max_bytes, Ordering::Relaxed);
        let evicted = self.enforce_limit()?;
        Ok(Eviction {
            removed: evicted.len() as u64,
            freed: evicted.iter().map(|r| r.bytes).sum(),
        })
    }

    /// Removes every proxy file (ready ones and the parts of unfinished ones); every row becomes
    /// `evicted`. Everything can be generated again from the originals.
    pub fn clear(&self) -> Result<Eviction, CacheError> {
        let usage = self.usage()?;
        let mut all = self.list(ProxyState::Ready)?;
        all.extend(self.list(ProxyState::Building)?);
        for r in &all {
            self.evict(&r.hash, &r.profile)?;
        }
        if let Ok(entries) = fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
        self.bump();
        Ok(Eviction {
            removed: usage.entries,
            freed: usage.bytes,
        })
    }

    /// Ready proxies and their bytes.
    pub fn usage(&self) -> Result<CacheUsage, CacheError> {
        let (entries, bytes): (i64, i64) = self.db.lock().query_row(
            "SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM proxies WHERE state = ?1",
            params![ProxyState::Ready.code()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(CacheUsage {
            entries: entries.unsigned_abs(),
            bytes: bytes.unsigned_abs(),
        })
    }
}
