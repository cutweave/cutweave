// SPDX-License-Identifier: GPL-3.0-or-later
//! [`FrameCache`]: decoded frames in RAM, least-recently-used by bytes (arch §7).
//!
//! Scrubbing and preview ask for the same few frames over and over; this keeps the last ones
//! decoded. The key is `(content hash, stream, pts)`: the hash makes it survive relinking and
//! rename, and a frame is addressed by its own pts, never by index or fps (arch §4.1).
//!
//! Frames are shared as `Arc<VideoFrame>`: a hit is a refcount bump, never a copy, and a frame a
//! caller still holds stays valid after it is evicted. The cache is `Send + Sync` behind one
//! mutex; lookups are a hash probe and a counter update, so the lock is held for microseconds and
//! never across a decode ([`FrameCache::get_or_insert_with`] decodes outside it).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use cutweave_media::{ContentHash, VideoFrame};
use cutweave_model::Time;

/// What identifies a cached frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameKey {
    /// Content hash of the media file.
    pub hash: ContentHash,
    /// Container stream index.
    pub stream: u32,
    /// Presentation time of the frame in the source.
    pub pts: Time,
}

/// Counters since creation (or [`FrameCache::reset_stats`]), plus the current size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameCacheStats {
    /// Lookups that found a frame.
    pub hits: u64,
    /// Lookups that did not.
    pub misses: u64,
    /// Frames dropped to make room.
    pub evictions: u64,
    /// Frames held now.
    pub frames: usize,
    /// Bytes of picture data held now.
    pub bytes: usize,
}

#[derive(Debug)]
struct Entry {
    frame: Arc<VideoFrame>,
    bytes: usize,
    seq: u64,
}

#[derive(Debug, Default)]
struct Inner {
    map: HashMap<FrameKey, Entry>,
    /// Recency order: lowest sequence number = least recently used.
    order: BTreeMap<u64, FrameKey>,
    seq: u64,
    bytes: usize,
    budget: usize,
    stats: FrameCacheStats,
}

impl Inner {
    fn touch(&mut self, key: &FrameKey) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(e) = self.map.get_mut(key) {
            self.order.remove(&e.seq);
            e.seq = seq;
            self.order.insert(seq, *key);
        }
    }

    fn remove(&mut self, key: &FrameKey) {
        if let Some(e) = self.map.remove(key) {
            self.order.remove(&e.seq);
            self.bytes -= e.bytes;
        }
    }

    fn shrink(&mut self) {
        while self.bytes > self.budget {
            let Some((_, key)) = self.order.pop_first() else {
                break;
            };
            if let Some(e) = self.map.remove(&key) {
                self.bytes -= e.bytes;
                self.stats.evictions += 1;
            }
        }
    }
}

/// An LRU of decoded frames, bounded by bytes (see the module docs).
#[derive(Debug)]
pub struct FrameCache {
    inner: Mutex<Inner>,
}

/// Bytes of picture data in `frame`.
fn frame_bytes(frame: &VideoFrame) -> usize {
    frame.planes.iter().map(|p| p.data.len()).sum()
}

impl FrameCache {
    /// The default budget when RAM is unknown: 1 GiB.
    pub const DEFAULT_BUDGET: usize = 1 << 30;

    /// A cache holding at most `budget_bytes` of picture data.
    #[must_use]
    pub fn new(budget_bytes: usize) -> FrameCache {
        FrameCache {
            inner: Mutex::new(Inner {
                budget: budget_bytes,
                ..Inner::default()
            }),
        }
    }

    /// The default budget: 1 GiB, or a quarter of `total_ram` bytes when that is smaller. Pass
    /// `None` when the machine's RAM is unknown.
    #[must_use]
    pub fn default_budget(total_ram: Option<u64>) -> usize {
        let quarter = total_ram.map(|r| usize::try_from(r / 4).unwrap_or(usize::MAX));
        quarter.map_or(FrameCache::DEFAULT_BUDGET, |q| {
            q.min(FrameCache::DEFAULT_BUDGET)
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The byte budget.
    #[must_use]
    pub fn budget(&self) -> usize {
        self.lock().budget
    }

    /// Changes the budget (evicting at once if it shrank).
    pub fn set_budget(&self, budget_bytes: usize) {
        let mut g = self.lock();
        g.budget = budget_bytes;
        g.shrink();
    }

    /// The frame for `key`, marking it most recently used.
    #[must_use]
    pub fn get(&self, key: &FrameKey) -> Option<Arc<VideoFrame>> {
        let mut g = self.lock();
        if let Some(frame) = g.map.get(key).map(|e| Arc::clone(&e.frame)) {
            g.touch(key);
            g.stats.hits += 1;
            Some(frame)
        } else {
            g.stats.misses += 1;
            None
        }
    }

    /// Stores `frame` under `key` as the most recently used, evicting the oldest frames while
    /// over budget. A frame larger than the whole budget is not stored.
    pub fn insert(&self, key: FrameKey, frame: Arc<VideoFrame>) {
        let bytes = frame_bytes(&frame);
        let mut g = self.lock();
        g.remove(&key);
        if bytes > g.budget {
            return;
        }
        g.seq += 1;
        let seq = g.seq;
        g.order.insert(seq, key);
        g.map.insert(key, Entry { frame, bytes, seq });
        g.bytes += bytes;
        g.shrink();
    }

    /// The frame for `key`, or the one `decode` makes (run without the cache locked, so other
    /// threads keep reading; two racing misses may both decode, and the later insert wins).
    pub fn get_or_insert_with<E>(
        &self,
        key: FrameKey,
        decode: impl FnOnce() -> Result<VideoFrame, E>,
    ) -> Result<Arc<VideoFrame>, E> {
        if let Some(f) = self.get(&key) {
            return Ok(f);
        }
        let frame = Arc::new(decode()?);
        self.insert(key, Arc::clone(&frame));
        Ok(frame)
    }

    /// Drops every frame of the media with `hash` (it changed on disk, or was removed).
    pub fn remove_media(&self, hash: &ContentHash) {
        let mut g = self.lock();
        let keys: Vec<FrameKey> = g.map.keys().filter(|k| k.hash == *hash).copied().collect();
        for k in keys {
            g.remove(&k);
        }
    }

    /// Drops everything.
    pub fn clear(&self) {
        let mut g = self.lock();
        g.map.clear();
        g.order.clear();
        g.bytes = 0;
    }

    /// Counters and current size.
    #[must_use]
    pub fn stats(&self) -> FrameCacheStats {
        let g = self.lock();
        FrameCacheStats {
            frames: g.map.len(),
            bytes: g.bytes,
            ..g.stats
        }
    }

    /// Zeroes the hit, miss and eviction counters.
    pub fn reset_stats(&self) {
        self.lock().stats = FrameCacheStats::default();
    }
}

#[cfg(test)]
mod tests {
    use cutweave_media::testing::contract::TempDir;
    use cutweave_media::{HashMode, PixelFormat, Plane, content_hash};
    use cutweave_model::ColorTags;

    use super::*;

    fn hash(n: u8) -> ContentHash {
        let dir = TempDir::new("fc-hash");
        let p = dir.path().join("f");
        std::fs::write(&p, [n; 8]).expect("write");
        content_hash(&p, HashMode::Fast).expect("hash")
    }

    fn frame(pts: i64, bytes: usize) -> Arc<VideoFrame> {
        Arc::new(VideoFrame {
            pts: Time::from_flicks(pts).expect("time"),
            width: 1,
            height: 1,
            format: PixelFormat::Rgba8,
            planes: vec![Plane {
                data: vec![0; bytes],
                stride: 4,
            }],
            color: ColorTags::default(),
            damaged: false,
        })
    }

    fn key(h: ContentHash, pts: i64) -> FrameKey {
        FrameKey {
            hash: h,
            stream: 0,
            pts: Time::from_flicks(pts).expect("time"),
        }
    }

    #[test]
    fn hits_share_the_frame_and_count() {
        let c = FrameCache::new(1000);
        let h = hash(1);
        c.insert(key(h, 5), frame(5, 100));
        let first = c.get(&key(h, 5)).expect("hit");
        let second = c.get(&key(h, 5)).expect("hit");
        assert!(Arc::ptr_eq(&first, &second));
        assert!(c.get(&key(h, 6)).is_none());
        let s = c.stats();
        assert_eq!((s.hits, s.misses, s.frames, s.bytes), (2, 1, 1, 100));
    }

    #[test]
    fn the_least_recently_used_frame_goes_first() {
        let c = FrameCache::new(300);
        let h = hash(2);
        for pts in 0..3 {
            c.insert(key(h, pts), frame(pts, 100));
        }
        // Touch 0 so 1 is now the oldest.
        assert!(c.get(&key(h, 0)).is_some());
        c.insert(key(h, 3), frame(3, 100));
        assert!(c.get(&key(h, 1)).is_none(), "oldest evicted");
        assert!(c.get(&key(h, 0)).is_some());
        assert!(c.get(&key(h, 2)).is_some());
        assert!(c.get(&key(h, 3)).is_some());
        let s = c.stats();
        assert_eq!((s.evictions, s.bytes), (1, 300));
    }

    #[test]
    fn a_frame_bigger_than_the_budget_is_not_stored_and_replacing_adjusts_bytes() {
        let c = FrameCache::new(100);
        let h = hash(3);
        c.insert(key(h, 0), frame(0, 101));
        assert_eq!(c.stats().frames, 0);
        c.insert(key(h, 1), frame(1, 60));
        c.insert(key(h, 1), frame(1, 40));
        assert_eq!(c.stats().bytes, 40);
        c.set_budget(10);
        assert_eq!(c.stats().frames, 0, "shrinking evicts");
    }

    #[test]
    fn get_or_insert_decodes_once_and_remove_media_drops_only_that_media() {
        let c = FrameCache::new(1000);
        let (a, b) = (hash(4), hash(5));
        let mut decodes = 0;
        for _ in 0..3 {
            let f = c
                .get_or_insert_with(key(a, 7), || {
                    decodes += 1;
                    Ok::<_, ()>((*frame(7, 10)).clone())
                })
                .expect("frame");
            assert_eq!(f.pts.flicks(), 7);
        }
        assert_eq!(decodes, 1);
        assert!(
            c.get_or_insert_with(key(a, 8), || Err("boom")).is_err(),
            "errors pass through and store nothing"
        );
        c.insert(key(b, 7), frame(7, 10));
        c.remove_media(&a);
        assert!(c.get(&key(a, 7)).is_none());
        assert!(c.get(&key(b, 7)).is_some());
        c.clear();
        assert_eq!(c.stats().frames, 0);
    }

    #[test]
    fn the_default_budget_is_a_gib_or_a_quarter_of_ram() {
        assert_eq!(FrameCache::default_budget(None), 1 << 30);
        assert_eq!(FrameCache::default_budget(Some(64 << 30)), 1 << 30);
        assert_eq!(FrameCache::default_budget(Some(2 << 30)), 512 << 20);
    }
}
