// SPDX-License-Identifier: GPL-3.0-or-later
//! A dev-only stand-in media backend (integrator task "dev stub media", R27).
//!
//! Until the FFmpeg backend (WP-1I.05) lands, a core engine cannot probe files, so
//! `media.import` reports every file in `failed[]`. With `--dev-stub-media` (or
//! `CUTWEAVE_DEV_STUB_MEDIA=1`) the engine attaches [`DevStubMedia`] instead: any existing,
//! readable file with a known extension is "probed" from its extension alone, so demos and UI
//! work can import files and put clips on the timeline. **Nothing is decoded**: the data is
//! made up and clearly marked (codec `"stub"`, `"stub": true` in the probe's extra fields).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use cutweave_model::{
    AudioInfo, Duration, Extra, FrameRate, ProbeInfo, SampleRate, StreamInfo, StreamKind,
};
use cutweave_ops::media::{MediaProbe, ProbeError, Probed};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::subsystems::Subsystems;

/// The environment variable equivalent of `--dev-stub-media`.
pub const DEV_STUB_MEDIA_ENV: &str = "CUTWEAVE_DEV_STUB_MEDIA";

/// The codec name every stub stream carries.
pub const STUB_CODEC: &str = "stub";

/// Seconds of media the stub reports for video and audio files.
const STUB_SECS: i64 = 10;
/// Bytes sampled from the start and the end of a file for its content hash.
const SAMPLE: u64 = 64 * 1024;

const VIDEO: &[&str] = &["mp4", "mov", "mkv", "webm", "m4v", "avi"];
const AUDIO: &[&str] = &["wav", "mp3", "aac", "m4a", "flac", "ogg", "opus"];
const IMAGE: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];

/// Whether an environment value switches the stub on (`1`, `true`, `yes`, `on`).
#[must_use]
pub fn env_enables(value: Option<&str>) -> bool {
    value
        .map(|v| v.trim().to_ascii_lowercase())
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
}

/// The stand-in prober: plausible, made-up probe data chosen by file extension.
#[derive(Clone, Copy, Debug, Default)]
pub struct StubMediaBackend;

impl MediaProbe for StubMediaBackend {
    fn probe(&self, path: &str) -> Result<Probed, ProbeError> {
        let io = |e: std::io::Error| ProbeError::Io {
            path: path.to_owned(),
            reason: e.to_string(),
        };
        let mut file = File::open(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ProbeError::NotFound(path.to_owned()),
            _ => io(e),
        })?;
        let meta = file.metadata().map_err(io)?;
        if !meta.is_file() {
            return Err(ProbeError::NotFound(path.to_owned()));
        }
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let mut probe = if VIDEO.contains(&ext.as_str()) {
            video()
        } else if AUDIO.contains(&ext.as_str()) {
            audio()
        } else if IMAGE.contains(&ext.as_str()) {
            image()
        } else {
            return Err(ProbeError::Unsupported {
                path: path.to_owned(),
                reason: format!("the stub media backend does not know the extension `{ext}`"),
            });
        };
        probe.extra.insert("stub", Value::Bool(true));
        Ok(Probed {
            probe,
            content_hash: hash(&mut file, meta.len()).map_err(io)?,
        })
    }
}

/// `len` plus the first and last [`SAMPLE`] bytes, so a big file is cheap to fingerprint.
fn hash(file: &mut File, len: u64) -> std::io::Result<String> {
    let mut sha = Sha256::new();
    sha.update(len.to_le_bytes());
    let mut buf = Vec::new();
    file.seek(SeekFrom::Start(0))?;
    file.by_ref().take(SAMPLE).read_to_end(&mut buf)?;
    sha.update(&buf);
    if len > SAMPLE {
        buf.clear();
        file.seek(SeekFrom::Start(len.saturating_sub(SAMPLE).max(SAMPLE)))?;
        file.by_ref().take(SAMPLE).read_to_end(&mut buf)?;
        sha.update(&buf);
    }
    let hex: String = sha.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("stub-sha256:{hex}"))
}

fn stream(index: u32, kind: StreamKind) -> StreamInfo {
    let video = kind == StreamKind::Video;
    StreamInfo {
        index,
        kind,
        codec: Some(STUB_CODEC.to_owned()),
        profile: None,
        width: video.then_some(1920),
        height: video.then_some(1080),
        pixel_format: None,
        sample_rate: (!video).then_some(SampleRate::HZ_48_000),
        channels: (!video).then_some(2),
        language: None,
        extra: Extra::new(),
    }
}

fn stereo_48k() -> AudioInfo {
    AudioInfo {
        rate: SampleRate::HZ_48_000,
        channels: 2,
    }
}

fn video() -> ProbeInfo {
    ProbeInfo {
        streams: vec![stream(0, StreamKind::Video), stream(1, StreamKind::Audio)],
        duration: Duration::from_secs(STUB_SECS).ok(),
        fps: Some(FrameRate::FPS_30),
        audio: Some(stereo_48k()),
        ..ProbeInfo::default()
    }
}

fn audio() -> ProbeInfo {
    ProbeInfo {
        streams: vec![stream(0, StreamKind::Audio)],
        duration: Duration::from_secs(STUB_SECS).ok(),
        audio: Some(stereo_48k()),
        ..ProbeInfo::default()
    }
}

fn image() -> ProbeInfo {
    ProbeInfo {
        streams: vec![stream(0, StreamKind::Video)],
        ..ProbeInfo::default()
    }
}

/// The [`Subsystems`] that attach only [`StubMediaBackend`] (everything else stays core).
#[derive(Clone, Copy, Debug, Default)]
pub struct DevStubMedia;

impl Subsystems for DevStubMedia {
    fn media_probe(&self) -> Option<Arc<dyn MediaProbe>> {
        Some(Arc::new(StubMediaBackend))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn temp(name: &str, bytes: &[u8]) -> (std::path::PathBuf, String) {
        let f = std::env::temp_dir().join(format!("cutweave-stub-{}-{name}", std::process::id()));
        std::fs::write(&f, bytes).unwrap();
        let s = f.to_string_lossy().into_owned();
        (f, s)
    }

    #[test]
    fn probes_by_extension_and_marks_the_data_as_stub() {
        let (f, p) = temp("a.MP4", b"x");
        let v = StubMediaBackend.probe(&p).unwrap();
        assert_eq!(v.probe.streams.len(), 2);
        assert_eq!(v.probe.fps, Some(FrameRate::FPS_30));
        assert_eq!(v.probe.streams[0].codec.as_deref(), Some("stub"));
        assert_eq!(v.probe.extra.get("stub"), Some(&Value::Bool(true)));
        assert!(v.content_hash.starts_with("stub-sha256:"));
        std::fs::remove_file(f).unwrap();
        let (f, p) = temp("a.wav", b"x");
        let a = StubMediaBackend.probe(&p).unwrap();
        assert_eq!(a.probe.streams.len(), 1);
        assert!(a.probe.fps.is_none());
        std::fs::remove_file(f).unwrap();
        let (f, p) = temp("a.png", b"x");
        let i = StubMediaBackend.probe(&p).unwrap();
        assert!(i.probe.duration.is_none() && i.probe.audio.is_none());
        std::fs::remove_file(f).unwrap();
    }

    #[test]
    fn errors_for_unknown_extensions_and_missing_files() {
        let (f, p) = temp("a.xyz", b"x");
        assert!(matches!(
            StubMediaBackend.probe(&p),
            Err(ProbeError::Unsupported { .. })
        ));
        std::fs::remove_file(f).unwrap();
        assert!(matches!(
            StubMediaBackend.probe(&p),
            Err(ProbeError::NotFound(_))
        ));
    }

    #[test]
    fn the_hash_follows_the_content() {
        let big = vec![7u8; 300_000];
        let mut other = big.clone();
        other[299_999] = 8;
        let (f1, p1) = temp("h1.mp4", &big);
        let (f2, p2) = temp("h2.mp4", &other);
        let (f3, p3) = temp("h3.mp4", &big);
        let h = |p: &str| StubMediaBackend.probe(p).unwrap().content_hash;
        assert_ne!(h(&p1), h(&p2));
        assert_eq!(h(&p1), h(&p3));
        for f in [f1, f2, f3] {
            std::fs::remove_file(f).unwrap();
        }
    }

    #[test]
    fn the_flag_or_the_env_enables_the_stub() {
        let parse = |args: &str| {
            crate::launch::EngineArgs::parse(args.split_whitespace().map(str::to_owned)).unwrap()
        };
        assert!(!parse("--headless --port 0").dev_stub_media);
        assert!(parse("--headless --port 0 --dev-stub-media").dev_stub_media);
        let from_env = parse("--headless --port 0").with_dev_stub_env(Some("1"));
        assert!(from_env.dev_stub_media);
        let off = parse("--headless --port 0 --dev-stub-media").with_dev_stub_env(Some("0"));
        assert!(off.dev_stub_media, "the flag wins over a falsy env");
    }

    #[test]
    fn env_values() {
        assert!(env_enables(Some("1")) && env_enables(Some(" TRUE ")));
        assert!(!env_enables(Some("0")) && !env_enables(Some("")) && !env_enables(None));
    }
}
