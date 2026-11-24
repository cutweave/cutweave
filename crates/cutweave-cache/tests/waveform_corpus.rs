// SPDX-License-Identifier: GPL-3.0-or-later
//! Waveform pyramids of real corpus files through the FFmpeg backend (WP-1D.06). Needs
//! `--features ffmpeg` (docs/dev/ffmpeg.md: `. scripts/ffmpeg/env.sh` first) and the corpus
//! (`CUTWEAVE_CORPUS_DIR`, or `testdata/corpus` of this checkout or the main checkout); a missing
//! corpus skips with a message.
#![cfg(feature = "ffmpeg")]

use std::path::{Path, PathBuf};

use cutweave_cache::{Track, WaveformCache, WaveformRequest};
use cutweave_media::{CancelToken, FfmpegBackend, HashMode, MediaBackend, content_hash};
use cutweave_model::Time;

fn corpus_file(name: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("CUTWEAVE_CORPUS_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let local = root.join("testdata/corpus");
            if local.is_dir() {
                return Some(local);
            }
            // A worktree's `.git` is a file: `gitdir: <main>/.git/worktrees/<name>`.
            let gitdir = std::fs::read_to_string(root.join(".git")).ok()?;
            let gitdir = PathBuf::from(gitdir.strip_prefix("gitdir:")?.trim());
            let main = gitdir.parent()?.parent()?.parent()?;
            Some(main.join("testdata/corpus"))
        })?;
    let path = dir.join(name);
    if path.is_file() {
        Some(path)
    } else {
        eprintln!("skipping: {} is not in the corpus", path.display());
        None
    }
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cutweave-wf-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn corpus_files_get_pyramids_that_match_their_probe() {
    let backend = FfmpegBackend::new().expect("the loaded FFmpeg matches the build");
    for name in [
        "audio_48000.wav",
        "audio_44100.wav",
        "sintel_trailer_480p.mp4",
    ] {
        let Some(path) = corpus_file(name) else {
            return;
        };
        let info = backend.probe(&path).expect("probe");
        let audio = info.audio.expect("audio");
        let hash = content_hash(&path, HashMode::Fast).expect("hash");
        let dir = scratch(name);
        let cache = WaveformCache::new(&dir);
        let request = WaveformRequest {
            per_channel: true,
            duration_hint: info.duration,
            ..WaveformRequest::default()
        };
        let mut last = 0.0;
        let wf = cache
            .ensure(
                &backend,
                &path,
                &hash,
                &request,
                &CancelToken::new(),
                &mut |f| {
                    assert!(f >= last, "progress went back: {last} -> {f}");
                    last = f;
                },
            )
            .expect("ensure");
        assert!((last - 1.0).abs() < f64::EPSILON);
        assert_eq!(wf.rate(), audio.rate);
        assert_eq!(wf.channels(), audio.channels);
        let expected = info.duration.expect("duration").as_secs_f64() * f64::from(audio.rate.hz());
        #[allow(clippy::cast_precision_loss)]
        let got = wf.samples() as f64;
        assert!(
            (got - expected).abs() <= 0.5 * f64::from(audio.rate.hz()),
            "{name}: {got} samples, probe says {expected}"
        );
        let all = wf.all_peaks(0, Track::Mix).expect("lod 0");
        assert!(all.peaks.iter().any(|p| p.max > 100), "{name}: silent?");
        // From disk, in a fresh cache, with identical content.
        let again = WaveformCache::new(&dir)
            .get(&hash, None, true)
            .expect("get")
            .expect("hit");
        assert_eq!(*again, *wf);
        let at = |s: i64| Time::from_secs(s).expect("t");
        let one_second = again.peaks(0, at(0)..at(1), Track::Mix).expect("range");
        assert_eq!(
            one_second.peaks.len(),
            (audio.rate.hz() as usize).div_ceil(256)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_corpus_file_without_audio_has_no_waveform() {
    let backend = FfmpegBackend::new().expect("the loaded FFmpeg matches the build");
    let Some(path) = corpus_file("no_audio_stream.mp4") else {
        return;
    };
    let hash = content_hash(&path, HashMode::Fast).expect("hash");
    let dir = scratch("noaudio");
    let r = WaveformCache::new(&dir).ensure(
        &backend,
        &path,
        &hash,
        &WaveformRequest::default(),
        &CancelToken::new(),
        &mut |_| {},
    );
    assert!(r.is_err(), "no audio stream");
    let _ = std::fs::remove_dir_all(&dir);
}
