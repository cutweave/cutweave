// SPDX-License-Identifier: GPL-3.0-or-later
//! The media probing boundary (arch §7): `media.import` learns what a file contains by asking a
//! [`MediaProbe`], so this crate needs no FFmpeg. The real prober arrives with WP-1D.02;
//! [`FakeProbe`] serves tests and headless runs.

use std::{collections::BTreeMap, time};

use cutweave_model::{
    AudioInfo, Duration, FrameRate, ProbeInfo, SampleRate, StreamInfo, StreamKind,
};

/// What probing a file found: the probe results and a content hash for relinking.
///
/// The hash is a cheap fingerprint computed during the synchronous probe (for example length
/// plus sampled chunks); background jobs may later replace it with a full hash.
#[derive(Clone, Debug, PartialEq)]
pub struct Probed {
    /// Streams, duration, rate and color tags.
    pub probe: ProbeInfo,
    /// Content hash (non-empty, no whitespace).
    pub content_hash: String,
}

/// Why a file could not be probed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProbeError {
    /// There is no readable file at the path.
    #[error("no readable file at `{0}`")]
    NotFound(String),
    /// The file exists but holds no playable audio or video.
    #[error("`{path}` is not a supported media file: {reason}")]
    Unsupported {
        /// The path.
        path: String,
        /// What the prober found.
        reason: String,
    },
    /// The probe did not finish within the import timeout (arch §7).
    #[error("probing `{0}` timed out")]
    TimedOut(String),
    /// Any other I/O failure.
    #[error("could not read `{path}`: {reason}")]
    Io {
        /// The path.
        path: String,
        /// The failure.
        reason: String,
    },
}

/// Reads media files for `media.import` (arch §7): the stream probe plus a fast content hash.
///
/// Implementations probe synchronously. `media.import` runs each probe on its own thread
/// with the per-file timeout of its [`ImportPolicy`](super::ImportPolicy) and abandons probes
/// that overrun it (reported as [`ProbeError::TimedOut`]), so an implementation should still
/// give up on a stuck file eventually but need not enforce the timeout itself.
pub trait MediaProbe: Send + Sync {
    /// Probes the file at the absolute `path`.
    fn probe(&self, path: &str) -> Result<Probed, ProbeError>;
}

/// A [`MediaProbe`] backed by an in-memory table, for tests and headless runs. Paths that were
/// not added are [`ProbeError::NotFound`]; a path can be made slow ([`FakeProbe::with_delay`]).
#[derive(Clone, Debug, Default)]
pub struct FakeProbe {
    files: BTreeMap<String, Result<Probed, ProbeError>>,
    delays: BTreeMap<String, time::Duration>,
}

impl FakeProbe {
    /// An empty table.
    #[must_use]
    pub fn new() -> FakeProbe {
        FakeProbe::default()
    }

    /// Serves `probed` for `path`.
    #[must_use]
    pub fn with(mut self, path: &str, probed: Probed) -> FakeProbe {
        self.files.insert(path.to_owned(), Ok(probed));
        self
    }

    /// Fails probing `path` with `error`.
    #[must_use]
    pub fn with_error(mut self, path: &str, error: ProbeError) -> FakeProbe {
        self.files.insert(path.to_owned(), Err(error));
        self
    }

    /// Makes probing `path` block for `delay` first (a slow or hung backend).
    #[must_use]
    pub fn with_delay(mut self, path: &str, delay: time::Duration) -> FakeProbe {
        self.delays.insert(path.to_owned(), delay);
        self
    }

    /// A 30 fps video of `secs` seconds, with a 48 kHz stereo audio stream if `audio`.
    #[must_use]
    pub fn video(path: &str, secs: i64, audio: bool) -> Probed {
        let mut streams = vec![stream(0, StreamKind::Video, "h264")];
        let mut probe = ProbeInfo {
            duration: Duration::from_secs(secs).ok(),
            fps: Some(FrameRate::FPS_30),
            ..ProbeInfo::default()
        };
        if audio {
            streams.push(stream(1, StreamKind::Audio, "aac"));
            probe.audio = Some(AudioInfo {
                rate: SampleRate::HZ_48_000,
                channels: 2,
            });
        }
        probe.streams = streams;
        Probed {
            probe,
            content_hash: fake_hash(path),
        }
    }

    /// A 48 kHz stereo audio file of `secs` seconds.
    #[must_use]
    pub fn audio(path: &str, secs: i64) -> Probed {
        let probe = ProbeInfo {
            streams: vec![stream(0, StreamKind::Audio, "pcm_s16le")],
            duration: Duration::from_secs(secs).ok(),
            audio: Some(AudioInfo {
                rate: SampleRate::HZ_48_000,
                channels: 2,
            }),
            ..ProbeInfo::default()
        };
        Probed {
            probe,
            content_hash: fake_hash(path),
        }
    }

    /// A still image (one video stream, no duration).
    #[must_use]
    pub fn image(path: &str) -> Probed {
        let probe = ProbeInfo {
            streams: vec![stream(0, StreamKind::Video, "png")],
            ..ProbeInfo::default()
        };
        Probed {
            probe,
            content_hash: fake_hash(path),
        }
    }
}

fn fake_hash(path: &str) -> String {
    format!("fake:{}", path.replace(char::is_whitespace, "_"))
}

fn stream(index: u32, kind: StreamKind, codec: &str) -> StreamInfo {
    StreamInfo {
        index,
        kind,
        codec: Some(codec.to_owned()),
        profile: None,
        width: (kind == StreamKind::Video).then_some(1920),
        height: (kind == StreamKind::Video).then_some(1080),
        pixel_format: None,
        sample_rate: (kind == StreamKind::Audio).then_some(SampleRate::HZ_48_000),
        channels: (kind == StreamKind::Audio).then_some(2),
        language: None,
        extra: cutweave_model::Extra::new(),
    }
}

impl MediaProbe for FakeProbe {
    fn probe(&self, path: &str) -> Result<Probed, ProbeError> {
        if let Some(delay) = self.delays.get(path) {
            std::thread::sleep(*delay);
        }
        self.files
            .get(path)
            .cloned()
            .unwrap_or_else(|| Err(ProbeError::NotFound(path.to_owned())))
    }
}
