// SPDX-License-Identifier: GPL-3.0-or-later
//! The `view.*` transport commands (arch §5.1 `view` namespace: not undoable, not journaled).
//! They run against the [`Transport`] (their engine-command context) and return the
//! [`PlaybackState`] after the change.
//!
//! The engine attaches them through its subsystem seam (WP-1I.05): `Subsystems::register`
//! calls [`register`], and `Subsystems::invoke` runs a call whose `context` is
//! `TypeId::of::<Transport>()` with
//! `registry.invoke_engine_json(name, params, &mut transport, caller.access())`.

use cutweave_commands::{CmdError, EngineCommand, Registry, RegistryError, command, wire};
use cutweave_model::{MediaId, Time, TimeInput};

use crate::shm::{PixelFormat, PreviewConfig};
use crate::transport::control::Transport;
use crate::transport::state::{PausedQuality, PlaybackState, PlayingQuality, Rate};

/// Plays forward at normal speed with audio (L). At the end of the content, playback starts
/// again from the beginning. Returns the playback state.
#[command(
    name = "view.play",
    summary = "Start playback at normal speed",
    view,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewPlay {}

impl EngineCommand for ViewPlay {
    type Context = Transport;
    type Output = PlaybackState;
    fn run(&self, transport: &mut Transport) -> Result<PlaybackState, CmdError> {
        Ok(transport.play())
    }
}

/// Pauses on the current position (K); the frame refines to full quality.
#[command(
    name = "view.pause",
    summary = "Pause playback",
    view,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewPause {}

impl EngineCommand for ViewPause {
    type Context = Transport;
    type Output = PlaybackState;
    fn run(&self, transport: &mut Transport) -> Result<PlaybackState, CmdError> {
        Ok(transport.pause())
    }
}

/// Moves the playhead (latest wins: a newer seek cancels the frames of older ones). While
/// stopped, a quarter-resolution frame comes first and the full-quality frame after 100 ms
/// without a newer seek; while playing, playback continues from there.
#[command(
    name = "view.seek",
    summary = "Move the playhead (scrub when stopped)",
    view,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewSeek {
    /// Where: flicks, "2.5s", "f:60" or timecode "00:00:02:12" at the rate of what plays (the
    /// sequence's, or a source's own); `>= 0`, clamped to the end of the content.
    pub t: TimeInput,
}

impl EngineCommand for ViewSeek {
    type Context = Transport;
    type Output = PlaybackState;
    fn run(&self, transport: &mut Transport) -> Result<PlaybackState, CmdError> {
        let t = self
            .t
            .resolve(Some(transport.timing().rate))
            .map_err(|e| CmdError::invalid("/t", e.to_string()))?;
        if t < Time::ZERO {
            return Err(CmdError::invalid("/t", "the playhead cannot be negative"));
        }
        Ok(transport.seek(t))
    }
}

/// Sets the playback rate (JKL): 1, 2 or 4 play forward (1 with audio; 2 and 4 video only),
/// −1, −2 or −4 play backwards frame by frame with audio muted, 0 pauses.
#[command(
    name = "view.set_rate",
    summary = "Play at a JKL rate (-4, -2, -1, 1, 2, 4) or pause (0)",
    view,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewSetRate {
    /// One of -4, -2, -1, 0, 1, 2, 4.
    pub rate: f64,
}

impl EngineCommand for ViewSetRate {
    type Context = Transport;
    type Output = PlaybackState;
    fn run(&self, transport: &mut Transport) -> Result<PlaybackState, CmdError> {
        if self.rate.abs() < 1e-9 {
            return Ok(transport.pause());
        }
        let rate = Rate::from_f64(self.rate).ok_or_else(|| {
            CmdError::invalid("/rate", "rate must be one of -4, -2, -1, 0, 1, 2 or 4")
        })?;
        Ok(transport.set_rate(rate))
    }
}

/// Pauses and moves the playhead by whole frames of the grid of what plays (negative steps
/// back), clamped to the content. The frame renders at full quality.
#[command(
    name = "view.step",
    summary = "Pause and step the playhead by frames",
    view,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ViewStep {
    /// Frames to move; negative moves back.
    pub frames: i32,
}

impl EngineCommand for ViewStep {
    type Context = Transport;
    type Output = PlaybackState;
    fn run(&self, transport: &mut Transport) -> Result<PlaybackState, CmdError> {
        Ok(transport.step(i64::from(self.frames)))
    }
}

/// Plays one media asset on its own in the preview (a source), or the timeline again without
/// `media` (ADR-0022). Playback stops on the new target's own playhead: where that asset was
/// left this session (its start the first time), or the timeline's, which does not move while
/// a source plays. `view.play`, `view.pause`, `view.seek`, `view.step` and `view.set_rate` then
/// act on the source, in its own time; the source ends by itself when its asset is removed or
/// another project opens. Nothing is written to the project: no item, no undo step. Returns
/// the playback state (`source` says what plays).
#[command(
    name = "view.set_source",
    summary = "Play a media item on its own in the preview, or the timeline again",
    view,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewSetSource {
    /// The media to play; omit it to return to the timeline.
    pub media: Option<MediaId>,
}

impl EngineCommand for ViewSetSource {
    type Context = Transport;
    type Output = PlaybackState;
    fn run(&self, transport: &mut Transport) -> Result<PlaybackState, CmdError> {
        transport.set_source_media(self.media)
    }
}

/// The preview frame size accepted by `view.set_preview_size`.
#[wire]
#[derive(Copy, Eq)]
pub struct PreviewSize {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Sets the size preview frames are rendered at: the preview canvas in device pixels (arch §10
/// step 1). Sizes above 8192 are scaled down keeping the aspect ratio; NV12 rings round them
/// down to even. The current frame re-renders at the new size.
#[command(
    name = "view.set_preview_size",
    summary = "Set the preview canvas size in device pixels",
    view,
    mcp = "hidden",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewSetPreviewSize {
    /// Width in device pixels, `>= 1`.
    pub width: u32,
    /// Height in device pixels, `>= 1`.
    pub height: u32,
}

impl EngineCommand for ViewSetPreviewSize {
    type Context = Transport;
    type Output = PreviewSize;
    fn run(&self, transport: &mut Transport) -> Result<PreviewSize, CmdError> {
        if self.width == 0 {
            return Err(CmdError::invalid("/width", "width must be at least 1"));
        }
        if self.height == 0 {
            return Err(CmdError::invalid("/height", "height must be at least 1"));
        }
        let fitted = PreviewConfig {
            width: self.width,
            height: self.height,
            format: PixelFormat::Rgba8,
        }
        .fitted();
        transport.set_preview_size(fitted.width, fitted.height);
        Ok(PreviewSize {
            width: fitted.width,
            height: fitted.height,
        })
    }
}

/// Chooses how the preview trades quality for speed: whether it uses proxies, and the size of
/// its frames while playing and when stopped.
///
/// `use_proxies` (WP-2F.02). On (the default), a clip plays, scrubs
/// and pauses on its proxy when one is ready and holds the pixels the clip is drawn at; a
/// preview that needs more than the proxy has shows the original. Off, the preview reads
/// originals only. Exports, agent frames, thumbnails and analysis read originals either way.
/// This computer's preference (`playback.proxies`): the app sends it after connecting and
/// when the transport bar's toggle changes; `playback.state` reports it (`use_proxies`) and
/// whether the frame on screen shows a proxy (`proxy`).
///
/// `playing` and `paused` (WP-2H.10; the preferences `playback.quality_playing` and
/// `playback.quality_paused`, sent the same way). While playing, `auto` (the default) follows
/// the quality ladder: full size while frames are on time, smaller when they are late; `full`,
/// `half` and `quarter` hold that size whatever the frame times (late frames are dropped).
/// When stopped, `full` (the default) refines the frame to full size and `auto` keeps it at
/// the size playback uses. `playback.state.quality` says the size of the frame on screen.
///
/// A change re-renders the current frame and applies from the next one, with no seek.
/// Returns the playback state.
#[command(
    name = "view.set_preview_quality",
    summary = "Choose the preview's use of proxies and its quality while playing and paused",
    view,
    scope = "ui",
    mcp = "hidden",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewSetPreviewQuality {
    /// Whether the preview uses proxies; absent keeps the current choice.
    #[serde(default)]
    pub use_proxies: Option<bool>,
    /// The size of frames while playing: `auto`, `full`, `half` or `quarter`; absent keeps
    /// the current choice.
    #[serde(default)]
    pub playing: Option<PlayingQuality>,
    /// The size of the frame when stopped: `full` or `auto`; absent keeps the current choice.
    #[serde(default)]
    pub paused: Option<PausedQuality>,
}

impl EngineCommand for ViewSetPreviewQuality {
    type Context = Transport;
    type Output = PlaybackState;
    fn run(&self, transport: &mut Transport) -> Result<PlaybackState, CmdError> {
        if let Some(on) = self.use_proxies {
            transport.set_use_proxies(on);
        }
        Ok(transport.set_preview_quality(self.playing, self.paused))
    }
}

/// Turns loop playback on or off (WP-2H.10). On, forward playback that reaches the end of
/// what plays (the sequence, or a source) starts again from the beginning and goes on until
/// it is paused, at 1× with its sound; reverse playback still stops at the start. Off (the
/// default), playback stops at the end. The person's preference (`playback.loop`): the app
/// sends it after connecting and when the transport bar's loop button or the setting changes.
/// Returns the playback state.
#[command(
    name = "view.set_loop",
    summary = "Turn loop playback on or off",
    view,
    scope = "ui",
    mcp = "hidden",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewSetLoop {
    /// Whether playback starts again from the beginning when it reaches the end.
    pub on: bool,
}

impl EngineCommand for ViewSetLoop {
    type Context = Transport;
    type Output = PlaybackState;
    fn run(&self, transport: &mut Transport) -> Result<PlaybackState, CmdError> {
        Ok(transport.set_looping(self.on))
    }
}

/// Registers the `view.*` transport commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<ViewPlay>()?
        .register::<ViewPause>()?
        .register::<ViewSeek>()?
        .register::<ViewSetRate>()?
        .register::<ViewStep>()?
        .register::<ViewSetSource>()?
        .register::<ViewSetPreviewSize>()?
        .register::<ViewSetPreviewQuality>()?
        .register::<ViewSetLoop>()?;
    Ok(())
}
