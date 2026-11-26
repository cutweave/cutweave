// SPDX-License-Identifier: GPL-3.0-or-later
//! Errors of audio sources and of the mixer.

use cutweave_model::{ItemId, MediaId, TimeError};

/// Why an audio source could not be opened or read.
///
/// Source errors never stop the mix: the clip that hit one plays silence, the error is logged
/// once per clip and counted in [`MixerStats::source_errors`](crate::MixerStats).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SourceError {
    /// The factory knows no audio for this media.
    #[error("no audio source for media {0}")]
    UnknownMedia(MediaId),
    /// The media has no (such) audio stream.
    #[error("media {media} has no audio stream {stream:?}")]
    NoStream {
        /// The media.
        media: MediaId,
        /// The requested stream index, if any.
        stream: Option<u32>,
    },
    /// The source's PCM format cannot be mixed (no channels, or no resampler for its rate).
    #[error("unsupported audio format: {0}")]
    Format(String),
    /// Reading or decoding failed.
    #[error("audio decode failed: {0}")]
    Decode(String),
}

/// Why the mixer rejected a configuration or a segment list.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MixError {
    /// A segment uses a feature the v0 graph does not implement (speed changes, reverse; P2).
    #[error("item {item}: {reason}")]
    Unsupported {
        /// The clip.
        item: ItemId,
        /// What is unsupported.
        reason: &'static str,
    },
    /// The mixer configuration is invalid.
    #[error("invalid mixer configuration: {0}")]
    Config(&'static str),
    /// The loudness meter failed (WP-2E.02).
    #[error("loudness meter: {0}")]
    Meter(&'static str),
    /// A time or range could not be represented.
    #[error(transparent)]
    Time(#[from] TimeError),
    /// A worker thread could not be started.
    #[error("could not start an audio worker thread: {0}")]
    Spawn(String),
}

/// Why the audio output could not open or keep a stream.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum OutputError {
    /// This build has no device backend (the `device` feature is off, or the target has none
    /// yet); only the null output is available.
    #[error("no audio device backend in this build; use the null output (CUTWEAVE_AUDIO=null)")]
    NoBackend,
    /// The system has no default output device.
    #[error("no audio output device")]
    NoDevice,
    /// No output device matches the requested name.
    #[error("no audio output device matching `{0}`")]
    DeviceNotFound(String),
    /// The device cannot play the mixer's rate or any sample format the output converts to.
    #[error("unsupported output configuration: {0}")]
    Unsupported(String),
    /// The audio backend reported an error.
    #[error("audio backend error: {0}")]
    Backend(String),
    /// The output's supervisor thread could not be started or did not answer.
    #[error("audio output thread: {0}")]
    Thread(String),
}
