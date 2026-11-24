// SPDX-License-Identifier: GPL-3.0-or-later
//! What the transport plays (ADR-0022, WP-1H.04): the document's active sequence, or one media
//! asset on its own (a *source*). [`SourceInfo`] describes a source (`playback.state.source`) and
//! a [`SourceResolver`] finds one for `view.set_source`.
//!
//! The transport keeps one playhead per target: the sequence's does not move while a source
//! plays, and every source's is kept for the session. The frame source renders a source as a
//! transient one-clip timeline it builds from the asset's probe ([`RenderRequest::target`]
//! names the target of each frame); nothing is written to the document.
//!
//! [`RenderRequest::target`]: crate::transport::RenderRequest::target

use cutweave_commands::{CmdError, wire};
use cutweave_model::{Duration, FrameRate, MediaId, Time};

use crate::transport::source::SequenceTiming;

/// What the transport plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PlaybackTarget {
    /// The document's active sequence.
    #[default]
    Sequence,
    /// One media asset outside any sequence (a source).
    Media(MediaId),
}

impl PlaybackTarget {
    /// The asset a source plays; `None` for the sequence.
    #[must_use]
    pub const fn media(self) -> Option<MediaId> {
        match self {
            PlaybackTarget::Sequence => None,
            PlaybackTarget::Media(id) => Some(id),
        }
    }
}

/// A media asset the preview plays instead of the sequence (`playback.state.source`).
#[wire]
#[derive(Copy, Eq)]
pub struct SourceInfo {
    /// The asset.
    pub media: MediaId,
    /// Its length in flicks: the source's own time runs from 0 to here.
    pub duration: Duration,
    /// The frame grid of the source: the asset's own frame rate (30 fps for audio-only assets).
    pub fps: FrameRate,
}

impl SourceInfo {
    /// The source's frame grid and extent.
    #[must_use]
    pub fn timing(&self) -> SequenceTiming {
        SequenceTiming::new(self.fps, Time::from(self.duration))
    }

    /// The target it is.
    #[must_use]
    pub const fn target(&self) -> PlaybackTarget {
        PlaybackTarget::Media(self.media)
    }
}

/// Finds the media asset `view.set_source` names (the engine resolves it against the open
/// document). Called on the control path, never on the render thread.
pub trait SourceResolver: Send + Sync {
    /// The source that plays `media`: its length and frame grid.
    ///
    /// # Errors
    /// `NotFound` for an asset the document does not have; `Invalid` for one with nothing to
    /// play (a still image, a compound clip, no known length).
    fn resolve(&self, media: MediaId) -> Result<SourceInfo, CmdError>;
}
