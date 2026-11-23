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
    /// A time or range could not be represented.
    #[error(transparent)]
    Time(#[from] TimeError),
    /// A worker thread could not be started.
    #[error("could not start an audio worker thread: {0}")]
    Spawn(String),
}
