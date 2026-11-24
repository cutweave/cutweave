// SPDX-License-Identifier: GPL-3.0-or-later
//! [`WaveformCache`]: waveform pyramids on disk, keyed by content hash and audio stream.
//!
//! One file per `(hash, stream)` in the cache directory (`<hash>.<stream>.cwwf`, format in
//! docs/reference/waveform-format.md). Writes go to a temporary file renamed into place, so a
//! reader never sees half a file; a damaged or older file is a miss and is deleted. Recently
//! read pyramids stay in memory ([`RECENT`] of them) because a timeline redraws from the same
//! one hundreds of times a second while scrolling.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use cutweave_media::{CancelToken, ContentHash, MediaBackend, MediaError};
use cutweave_model::{Duration, Time};

use crate::waveform::{PeakData, Track, Waveform, WaveformBuilder, WaveformOptions};
use crate::{CacheError, Eviction};

/// Pyramids kept in memory.
const RECENT: usize = 4;
/// File extension of a stored pyramid.
const EXTENSION: &str = "cwwf";

/// What to compute and how to report it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WaveformRequest {
    /// The audio stream (container index); `None` is the first audio stream.
    pub stream: Option<u32>,
    /// Also keep a pyramid per channel (see [`WaveformOptions::per_channel`]).
    pub per_channel: bool,
    /// The media's duration (from the probe), for progress. Without it progress jumps from 0 to 1.
    pub duration_hint: Option<Duration>,
}

/// The waveform pyramids of a cache directory.
#[derive(Debug)]
pub struct WaveformCache {
    dir: PathBuf,
    recent: Mutex<Vec<(PathBuf, Arc<Waveform>)>>,
}

impl WaveformCache {
    /// A cache in `dir` (created on first write). Production passes [`default_dir`]; tests a
    /// temporary directory.
    ///
    /// [`default_dir`]: Self::default_dir
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> WaveformCache {
        WaveformCache {
            dir: dir.into(),
            recent: Mutex::new(Vec::new()),
        }
    }

    /// `<OS cache dir>/cutweave/waveforms`: `%LOCALAPPDATA%` on Windows, `~/Library/Caches` on
    /// macOS, `$XDG_CACHE_HOME` or `~/.cache` elsewhere. Never inside a project folder. `None`
    /// if the environment names no home.
    #[must_use]
    pub fn default_dir() -> Option<PathBuf> {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let base = if cfg!(windows) {
            var("LOCALAPPDATA")?
        } else if cfg!(target_os = "macos") {
            var("HOME")?.join("Library").join("Caches")
        } else {
            var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|h| h.join(".cache")))?
        };
        Some(base.join("cutweave").join("waveforms"))
    }

    /// The directory the files live in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file for `(hash, stream)`.
    #[must_use]
    pub fn path_for(&self, hash: &ContentHash, stream: Option<u32>) -> PathBuf {
        let stream = stream.map_or_else(|| "main".to_owned(), |s| format!("s{s}"));
        // `blake3-fast:<hex>` has a colon, which Windows file names cannot hold.
        let hash = hash.to_string().replace(':', "-");
        self.dir.join(format!("{hash}.{stream}.{EXTENSION}"))
    }

    /// The stored pyramid, if there is a usable one: `per_channel` demands the per-channel
    /// tracks (a mono-only file is then a miss, [`ensure`](Self::ensure) rebuilds it). A damaged
    /// or older file is deleted and reported as a miss.
    pub fn get(
        &self,
        hash: &ContentHash,
        stream: Option<u32>,
        per_channel: bool,
    ) -> Result<Option<Arc<Waveform>>, CacheError> {
        let path = self.path_for(hash, stream);
        let usable = |w: &Waveform| !per_channel || w.has_per_channel() || w.channels() == 1;
        {
            let mut recent = self.recent.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(i) = recent.iter().position(|(p, _)| *p == path) {
                let hit = recent.remove(i);
                let wf = Arc::clone(&hit.1);
                recent.insert(0, hit);
                return Ok(usable(&wf).then_some(wf));
            }
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_error(&path, &e)),
        };
        let Some(wf) = Waveform::from_bytes(&bytes) else {
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        };
        // The file's modification time is the recency `evict_lru` orders by.
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
        let wf = Arc::new(wf);
        self.remember(path, &wf);
        Ok(usable(&wf).then_some(wf))
    }

    fn remember(&self, path: PathBuf, wf: &Arc<Waveform>) {
        let mut recent = self.recent.lock().unwrap_or_else(PoisonError::into_inner);
        recent.retain(|(p, _)| *p != path);
        recent.insert(0, (path, Arc::clone(wf)));
        recent.truncate(RECENT);
    }

    /// The stored pyramid for `hash`, or, if there is none, computed from the audio of `path`
    /// by decoding it once, stored and returned.
    ///
    /// The decode runs in one-second ranges (consecutive reads never seek). `cancel` stops it
    /// within one decoder call ([`MediaError::Cancelled`], nothing is stored); `progress` gets
    /// the fraction done (0 to 1, ending at exactly 1 after the file is written), at most once
    /// per second of audio. A job wraps this: `cancel` is a `CancelToken::linked` to the job's
    /// token and `progress` calls `JobCtx::progress`.
    ///
    /// Errors: [`CacheError::Media`] (`NoStream` for a file without audio, `Cancelled`, decoder
    /// failures), [`CacheError::Io`].
    pub fn ensure(
        &self,
        backend: &dyn MediaBackend,
        path: &Path,
        hash: &ContentHash,
        request: &WaveformRequest,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(f64),
    ) -> Result<Arc<Waveform>, CacheError> {
        if let Some(wf) = self.get(hash, request.stream, request.per_channel)? {
            progress(1.0);
            return Ok(wf);
        }
        progress(0.0);
        let wf = Arc::new(compute(backend, path, request, cancel, progress)?);
        let file = self.path_for(hash, request.stream);
        write_atomically(&file, &wf.to_bytes())?;
        self.remember(file, &wf);
        progress(1.0);
        Ok(wf)
    }

    /// `waveform(media, lod, range) -> PeakData` (WBS): the bins of `track` at `lod` overlapping
    /// `range`, from the stored pyramid; `None` if nothing is stored for `hash`, or the level or
    /// track does not exist. Never computes: a caller that gets `None` runs
    /// [`ensure`](Self::ensure) as a job.
    pub fn waveform(
        &self,
        hash: &ContentHash,
        stream: Option<u32>,
        lod: u8,
        range: std::ops::Range<Time>,
        track: Track,
    ) -> Result<Option<PeakData>, CacheError> {
        let per_channel = matches!(track, Track::Channel(_));
        Ok(self
            .get(hash, stream, per_channel)?
            .and_then(|wf| wf.peaks(lod, range, track)))
    }

    /// Deletes the stored pyramids of `hash` (every stream). Returns how many.
    pub fn remove(&self, hash: &ContentHash) -> Result<usize, CacheError> {
        let prefix = format!("{}.", hash.to_string().replace(':', "-"));
        let mut removed = 0;
        for (path, _, _) in self.files()? {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with(&prefix) && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        self.recent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        Ok(removed)
    }

    /// Entries and bytes on disk.
    pub fn usage(&self) -> Result<crate::CacheUsage, CacheError> {
        let files = self.files()?;
        Ok(crate::CacheUsage {
            entries: files.len() as u64,
            bytes: files.iter().map(|f| f.1).sum(),
        })
    }

    /// Deletes the least recently read pyramids until at most `max_bytes` remain.
    pub fn evict_lru(&self, max_bytes: u64) -> Result<Eviction, CacheError> {
        let mut files = self.files()?;
        files.sort_by_key(|f| f.2);
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        let mut out = Eviction::default();
        for (path, bytes, _) in files {
            if total <= max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= bytes;
                out.removed += 1;
                out.freed += bytes;
            }
        }
        if out.removed > 0 {
            self.recent
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
        }
        Ok(out)
    }

    /// `(path, bytes, modified)` of every stored pyramid.
    fn files(&self) -> Result<Vec<(PathBuf, u64, SystemTime)>, CacheError> {
        let rd = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_error(&self.dir, &e)),
        };
        let mut out = Vec::new();
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                out.push((
                    path,
                    meta.len(),
                    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                ));
            }
        }
        Ok(out)
    }
}

/// Decodes the audio of `path` once and builds the pyramid.
fn compute(
    backend: &dyn MediaBackend,
    path: &Path,
    request: &WaveformRequest,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(f64),
) -> Result<Waveform, CacheError> {
    let mut decoder = backend.open_audio_decoder(path, request.stream)?;
    decoder.set_cancel(Some(cancel.clone()));
    let format = decoder.format();
    let mut builder = WaveformBuilder::new(
        format.rate,
        format.channels,
        WaveformOptions {
            per_channel: request.per_channel,
        },
    );
    let step = Duration::from_secs(1).map_err(|e| MediaError::Invalid {
        message: e.to_string(),
    })?;
    let hint = request.duration_hint.map(Duration::as_secs_f64);
    let mut second: i64 = 0;
    loop {
        cancel.check()?;
        let start = Time::from_secs(second).map_err(|e| MediaError::Invalid {
            message: e.to_string(),
        })?;
        let chunk = decoder.decode_range(start, step)?;
        if chunk.frames() == 0 {
            // Past the end, unless the stream starts late or has a gap longer than a second: with
            // the media's duration known, keep reading up to it.
            #[allow(clippy::cast_precision_loss)]
            if hint.is_some_and(|h| (second as f64) < h) {
                second += 1;
                continue;
            }
            break;
        }
        builder.push_chunk(&chunk)?;
        second += 1;
        if let Some(h) = hint.filter(|h| *h > 0.0) {
            // Whole seconds of a file: far below 2^53.
            #[allow(clippy::cast_precision_loss)]
            progress((second as f64 / h).min(0.99));
        }
    }
    Ok(builder.finish())
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), CacheError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_error(dir, &e))?;
    }
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, bytes).map_err(|e| io_error(&tmp, &e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io_error(path, &e)
    })
}

fn io_error(path: &Path, e: &std::io::Error) -> CacheError {
    CacheError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use cutweave_media::testing::contract::TempDir;
    use cutweave_media::testing::{FakeAudio, FakeMedia};
    use cutweave_media::{FakeBackend, HashMode, content_hash};
    use cutweave_model::{FrameRate, SampleRate};

    use super::*;

    fn secs(s: i64) -> Duration {
        Duration::from_secs(s).expect("duration")
    }

    struct Fx {
        dir: TempDir,
        path: PathBuf,
        hash: ContentHash,
        backend: FakeBackend,
    }

    fn fixture(tag: &str, media: FakeMedia) -> Fx {
        let dir = TempDir::new(tag);
        let path = dir.path().join("a.mp4");
        std::fs::write(&path, tag.as_bytes()).expect("write");
        let hash = content_hash(&path, HashMode::Fast).expect("hash");
        let backend = FakeBackend::new().with_media(path.clone(), media);
        Fx {
            dir,
            path,
            hash,
            backend,
        }
    }

    fn cache(fx: &Fx) -> WaveformCache {
        WaveformCache::new(fx.dir.path().join("cache"))
    }

    fn req(hint: i64) -> WaveformRequest {
        WaveformRequest {
            stream: None,
            per_channel: false,
            duration_hint: Some(secs(hint)),
        }
    }

    #[test]
    fn ensure_computes_once_stores_and_serves_peaks() {
        let fx = fixture("wf-ensure", FakeMedia::clip(secs(3), FrameRate::FPS_30));
        let c = cache(&fx);
        assert!(c.get(&fx.hash, None, false).expect("get").is_none());
        let mut seen = Vec::new();
        let wf = c
            .ensure(
                &fx.backend,
                &fx.path,
                &fx.hash,
                &req(3),
                &CancelToken::new(),
                &mut |f| {
                    seen.push(f);
                },
            )
            .expect("ensure");
        assert_eq!(wf.samples(), 3 * 48_000);
        assert_eq!(wf.channels(), 2);
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "monotone: {seen:?}");
        assert_eq!((seen.first(), seen.last()), (Some(&0.0), Some(&1.0)));
        // The 440 Hz tone at -6 dBFS peaks at 16384 in every bin of level 0 (both channels have
        // the same amplitude; channel 1 is 880 Hz, the mix is smaller).
        let l0 = wf.all_peaks(0, Track::Mix).expect("lod 0");
        assert_eq!(l0.peaks.len(), (3 * 48_000_usize).div_ceil(256));
        // Served from disk by a fresh cache on the same directory, with a backend that knows
        // nothing of the file.
        let again = WaveformCache::new(c.dir());
        let mut decodes = 0;
        let got = again
            .ensure(
                &FakeBackend::new(),
                &fx.path,
                &fx.hash,
                &req(3),
                &CancelToken::new(),
                &mut |_| {
                    decodes += 1;
                },
            )
            .expect("cached");
        assert_eq!(*got, *wf);
        assert_eq!(decodes, 1, "only the final progress report");
        let usage = c.usage().expect("usage");
        assert_eq!(usage.entries, 1);
        assert_eq!(usage.bytes, wf.to_bytes().len() as u64);
        // Query through the cache.
        let at = |s: i64| Time::from_secs(s).expect("t");
        let d = again
            .waveform(&fx.hash, None, 2, at(1)..at(2), Track::Mix)
            .expect("query")
            .expect("data");
        assert_eq!(d.samples_per_bin, 4096);
        assert_eq!(d.first_bin, 48_000 / 4096);
        assert!(d.peaks.len() >= 11);
        assert!(
            again
                .waveform(&fx.hash, None, 0, at(0)..at(1), Track::Channel(0))
                .expect("query")
                .is_none(),
            "mix-only file has no channel tracks"
        );
    }

    #[test]
    fn per_channel_requests_rebuild_a_mono_only_file() {
        let fx = fixture("wf-perch", FakeMedia::clip(secs(1), FrameRate::FPS_30));
        let c = cache(&fx);
        let noop = &mut |_: f64| {};
        let a = c
            .ensure(
                &fx.backend,
                &fx.path,
                &fx.hash,
                &req(1),
                &CancelToken::new(),
                noop,
            )
            .expect("mono");
        assert!(!a.has_per_channel());
        let want = WaveformRequest {
            per_channel: true,
            ..req(1)
        };
        let b = c
            .ensure(
                &fx.backend,
                &fx.path,
                &fx.hash,
                &want,
                &CancelToken::new(),
                noop,
            )
            .expect("stereo");
        assert!(b.has_per_channel());
        assert_eq!(a.all_peaks(3, Track::Mix), b.all_peaks(3, Track::Mix));
        // A per-channel file also serves mono requests.
        assert!(c.get(&fx.hash, None, false).expect("get").is_some());
        // Streams are cached separately.
        assert!(c.get(&fx.hash, Some(1), false).expect("get").is_none());
    }

    #[test]
    fn cancelling_stops_the_decode_and_stores_nothing() {
        let fx = fixture("wf-cancel", FakeMedia::clip(secs(5), FrameRate::FPS_30));
        let c = cache(&fx);
        let token = CancelToken::new();
        token.cancel();
        let r = c.ensure(
            &fx.backend,
            &fx.path,
            &fx.hash,
            &req(5),
            &token,
            &mut |_| {},
        );
        assert!(
            matches!(r, Err(CacheError::Media(MediaError::Cancelled))),
            "{r:?}"
        );
        assert_eq!(c.usage().expect("usage").entries, 0);
        // Cancelled part-way through, from the progress callback.
        let token = CancelToken::new();
        let t2 = token.clone();
        let r = c.ensure(&fx.backend, &fx.path, &fx.hash, &req(5), &token, &mut |f| {
            if f > 0.3 {
                t2.cancel();
            }
        });
        assert!(
            matches!(r, Err(CacheError::Media(MediaError::Cancelled))),
            "{r:?}"
        );
        assert!(!c.path_for(&fx.hash, None).exists());
        let leftovers = std::fs::read_dir(c.dir()).map_or(0, Iterator::count);
        assert_eq!(leftovers, 0, "no temporary files");
    }

    #[test]
    fn files_without_audio_report_no_stream() {
        let fx = fixture(
            "wf-noaudio",
            FakeMedia::video(cutweave_media::testing::FakeVideo::new(
                64,
                36,
                FrameRate::FPS_30,
                30,
            )),
        );
        let c = cache(&fx);
        let r = c.ensure(
            &fx.backend,
            &fx.path,
            &fx.hash,
            &req(1),
            &CancelToken::new(),
            &mut |_| {},
        );
        assert!(
            matches!(r, Err(CacheError::Media(MediaError::NoStream { .. }))),
            "{r:?}"
        );
    }

    #[test]
    fn damaged_files_are_misses_and_are_deleted() {
        let fx = fixture("wf-damage", FakeMedia::clip(secs(1), FrameRate::FPS_30));
        let c = cache(&fx);
        c.ensure(
            &fx.backend,
            &fx.path,
            &fx.hash,
            &req(1),
            &CancelToken::new(),
            &mut |_| {},
        )
        .expect("ensure");
        let file = c.path_for(&fx.hash, None);
        let mut bytes = std::fs::read(&file).expect("read");
        bytes[40] ^= 0xff;
        std::fs::write(&file, &bytes).expect("damage");
        let fresh = WaveformCache::new(c.dir());
        assert!(fresh.get(&fx.hash, None, false).expect("get").is_none());
        assert!(!file.exists());
        // And it rebuilds.
        fresh
            .ensure(
                &fx.backend,
                &fx.path,
                &fx.hash,
                &req(1),
                &CancelToken::new(),
                &mut |_| {},
            )
            .expect("rebuild");
        assert!(file.exists());
    }

    #[test]
    fn late_starting_audio_keeps_its_gap_as_silence() {
        // A tone cut to 2 s read through a 4 s hint: the decoder returns empty ranges after 2 s,
        // which must not end the decode early nor add samples.
        let audio = FakeAudio::tone(SampleRate::HZ_48_000, 1, secs(2));
        let fx = fixture("wf-hint", FakeMedia::audio(audio));
        let c = cache(&fx);
        let wf = c
            .ensure(
                &fx.backend,
                &fx.path,
                &fx.hash,
                &req(4),
                &CancelToken::new(),
                &mut |_| {},
            )
            .expect("ensure");
        assert_eq!(wf.samples(), 2 * 48_000);
        // Without a hint the first empty range ends it, with the same result.
        c.remove(&fx.hash).expect("remove");
        let none = WaveformRequest {
            duration_hint: None,
            ..req(0)
        };
        let wf2 = c
            .ensure(
                &fx.backend,
                &fx.path,
                &fx.hash,
                &none,
                &CancelToken::new(),
                &mut |_| {},
            )
            .expect("ensure");
        assert_eq!(*wf, *wf2);
    }

    #[test]
    fn remove_and_eviction_manage_the_files() {
        let dir = TempDir::new("wf-evict");
        let c = WaveformCache::new(dir.path().join("w"));
        let mut hashes = Vec::new();
        let mut fixtures = Vec::new();
        for i in 0..3_u8 {
            let fx = fixture(
                &format!("wf-evict-{i}"),
                FakeMedia::clip(secs(1 + i64::from(i)), FrameRate::FPS_30),
            );
            c.ensure(
                &fx.backend,
                &fx.path,
                &fx.hash,
                &req(3),
                &CancelToken::new(),
                &mut |_| {},
            )
            .expect("ensure");
            hashes.push(fx.hash);
            fixtures.push(fx);
            // Distinct modification times.
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(c.usage().expect("usage").entries, 3);
        // Reading the oldest makes it the freshest.
        let reader = WaveformCache::new(c.dir());
        assert!(reader.get(&hashes[0], None, false).expect("get").is_some());
        let total = c.usage().expect("usage").bytes;
        let ev = c.evict_lru(total - 1).expect("evict");
        assert_eq!(ev.removed, 1);
        let alive: Vec<bool> = hashes
            .iter()
            .map(|h| {
                WaveformCache::new(c.dir())
                    .get(h, None, false)
                    .expect("get")
                    .is_some()
            })
            .collect();
        assert_eq!(
            alive,
            [true, false, true],
            "the second was the least recently read"
        );
        assert_eq!(c.remove(&hashes[0]).expect("remove"), 1);
        assert_eq!(c.usage().expect("usage").entries, 1);
        assert_eq!(c.evict_lru(0).expect("evict").removed, 1);
        assert_eq!(c.usage().expect("usage").entries, 0);
        // An absent directory is just empty.
        assert_eq!(
            WaveformCache::new(dir.path().join("none"))
                .usage()
                .expect("usage")
                .entries,
            0
        );
    }

    #[test]
    fn the_default_dir_is_outside_any_project() {
        if let Some(d) = WaveformCache::default_dir() {
            assert!(d.ends_with("cutweave/waveforms") || d.ends_with("cutweave\\waveforms"));
        }
    }

    /// 1 h of 48 kHz stereo through the whole pipeline (synthetic tone decode, peaks, pyramid,
    /// file write). Run: `cargo test --release -p cutweave-cache -- --ignored --nocapture
    /// one_hour`. Recorded in the WP report; the budget is 60 s.
    #[test]
    #[ignore = "timing measurement"]
    fn one_hour_of_stereo_computes_within_the_budget() {
        let fx = fixture(
            "wf-hour",
            FakeMedia::audio(FakeAudio::tone(SampleRate::HZ_48_000, 2, secs(3600))),
        );
        let c = cache(&fx);
        let started = std::time::Instant::now();
        let wf = c
            .ensure(
                &fx.backend,
                &fx.path,
                &fx.hash,
                &req(3600),
                &CancelToken::new(),
                &mut |_| {},
            )
            .expect("ensure");
        let took = started.elapsed();
        println!(
            "1 h stereo 48 kHz: {took:?}, file {} bytes",
            wf.to_bytes().len()
        );
        assert_eq!(wf.samples(), 3600 * 48_000);
        assert!(took.as_secs() < 60, "{took:?}");
    }
}
