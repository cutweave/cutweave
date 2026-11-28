// SPDX-License-Identifier: GPL-3.0-or-later
//! Pictures of media and sound for agents (WP-2I.02; arch §14 "seeing without screenshots"):
//! `media.filmstrip` and `audio.waveform`. (`render.frame` and `render.contact_sheet`, the
//! pictures of the timeline itself, are queries of `cutweave-render`.)
//!
//! Both are read-only engine commands in the MCP catalog, reached through `cutweave_query`
//! (ADR-0039). This crate checks the request against the document and shapes the answer; the
//! engine makes the picture ([`VisualService`]): it has the thumbnail cache, the peak pyramids
//! and the decoders. A core engine without them answers `Unsupported`.
//!
//! * **`media.filmstrip`**: `count` frames of a video asset at even intervals, side by side in
//!   one JPEG whose longer side is at most `max_size`. Without `from` and `to` it is the strip
//!   of the whole clip, the one the timeline shows (the thumbnail cache, decoded on a miss);
//!   with a range it is cached under a key of its own. `tiles` says which time each tile shows.
//! * **`audio.waveform`**: the sound of a media file (its peak pyramid), of one clip as it
//!   plays, or of a sequence's mix, drawn as a PNG with a time ruler, plus `audio_levels`: the
//!   loudest peak, the peak of up to [`LEVEL_SLICES`] equal slices, the silent stretches, and
//!   on request the loudness (EBU R 128) with its short-term course.
//!
//! **Sizes.** Pictures are capped ([`MAX_STRIP_SIZE`], [`MAX_WAVE_WIDTH`],
//! [`MAX_WAVE_HEIGHT`]) so one never floods a conversation. A result names a `link` when the
//! same picture is also served over HTTP by a `/media/...` route (docs/reference/http-media.md),
//! whose ETag is the content hash and a hash of the parameters; the MCP layer answers a large
//! request with that link in place of the image.
//!
//! **Silence** ([`silences`]) is a stretch of at least [`MIN_SILENCE_MS`] whose peak stays at or
//! below [`SILENCE_BELOW_DB`]. It is found on blocks of a few milliseconds, not on the picture's
//! columns, so its edges do not depend on the picture's width.

use std::sync::Arc;

use cutweave_commands::{
    Annotations, CmdError, CommandSpec, Described, DocSnapshot, EngineCommand, EntryKind,
    McpExposure, Registrable, Registry, RegistryError, Rev, Scope, wire,
};
use cutweave_model::{
    Duration, EntityKind, FLICKS_PER_SECOND, ItemId, MediaId, ProbeInfo, Project, Sequence,
    SequenceId, StreamKind, Time, TimeInput,
};

use crate::audio::levels::level_db;

/// Tiles of a filmstrip when `count` is not given.
pub const DEFAULT_STRIP_TILES: u32 = 8;
/// Most tiles of a filmstrip.
pub const MAX_STRIP_TILES: u32 = 64;
/// The longer side of a filmstrip when `max_size` is not given, in px.
pub const DEFAULT_STRIP_SIZE: u32 = 768;
/// Smallest accepted `max_size` of a filmstrip.
pub const MIN_STRIP_SIZE: u32 = 64;
/// Largest accepted `max_size` of a filmstrip.
pub const MAX_STRIP_SIZE: u32 = 2048;
/// Narrowest tile of a filmstrip, in px.
pub const MIN_TILE_WIDTH: u32 = 8;

/// Width of a waveform picture when `width` is not given, in px.
pub const DEFAULT_WAVE_WIDTH: u32 = 768;
/// Height of a waveform picture when `height` is not given, in px.
pub const DEFAULT_WAVE_HEIGHT: u32 = 192;
/// Narrowest waveform picture.
pub const MIN_WAVE_WIDTH: u32 = 64;
/// Widest waveform picture.
pub const MAX_WAVE_WIDTH: u32 = 2048;
/// Lowest waveform picture.
pub const MIN_WAVE_HEIGHT: u32 = 32;
/// Highest waveform picture.
pub const MAX_WAVE_HEIGHT: u32 = 1024;
/// The smallest and largest `scale_db` (the dB range of the vertical axis).
pub const SCALE_DB_RANGE: (f64, f64) = (12.0, 144.0);
/// The longest stretch of a clip or a mix one `audio.waveform` call draws, in seconds: it is
/// decoded and mixed while the call waits. A media file has no such limit (its peaks are cached).
pub const MAX_MIX_SECONDS: i64 = 3600;
/// Most slices of `audio_levels.peaks_db`.
pub const LEVEL_SLICES: usize = 64;
/// A peak at or below this, in dBFS, is silence.
pub const SILENCE_BELOW_DB: f64 = -60.0;
/// The shortest stretch reported as silence, in milliseconds.
pub const MIN_SILENCE_MS: i64 = 300;

/// Registers `media.filmstrip` and `audio.waveform`.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<MediaFilmstrip>()?
        .register::<AudioWaveform>()?;
    Ok(())
}

const READ_ONLY: Annotations = Annotations {
    read_only: true,
    destructive: false,
    idempotent: true,
    open_world: false,
};

// ----- the engine side ---------------------------------------------------------------------------

/// A checked `media.filmstrip` request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StripRequest {
    /// The video asset.
    pub media: MediaId,
    /// The part of the clip (media time), or `None` for the whole clip.
    pub range: Option<(Time, Time)>,
    /// Tiles.
    pub count: u32,
    /// Widest tile in px: `count` of them, and their height, fit `max_size`.
    pub tile_width: u32,
}

/// A filmstrip as the engine made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StripMade {
    /// The JPEG, base64.
    pub data: String,
    /// Its size in bytes.
    pub bytes: u32,
    /// Width of one tile in px.
    pub tile_width: u32,
    /// Height of one tile in px.
    pub tile_height: u32,
    /// Media time of the first tile.
    pub start: Time,
    /// Media time between tiles.
    pub interval: Duration,
    /// The cache's entity tag of this strip (`<content hash>-<params hash>`).
    pub etag: String,
    /// The `/media/...` route that serves the same JPEG, if one does.
    pub link: Option<String>,
}

/// What a waveform shows, checked against the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveSource {
    /// A media file's sound, from its peak pyramid.
    Media {
        /// The asset.
        media: MediaId,
        /// The audio stream (container index); `None`: the first.
        stream: Option<u32>,
    },
    /// One clip as it plays on the timeline (its speed, gain and fades; not its track's).
    Item {
        /// Its sequence.
        sequence: SequenceId,
        /// The clip.
        item: ItemId,
    },
    /// The mix of a sequence, as an export hears it.
    Mix {
        /// The sequence.
        sequence: SequenceId,
    },
}

/// A checked `audio.waveform` request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveRequest {
    /// What to draw.
    pub source: WaveSource,
    /// Range start: media time for a media file, else sequence time.
    pub from: Time,
    /// Range end, exclusive (after `from`).
    pub to: Time,
    /// Picture width in px.
    pub width: u32,
    /// Picture height in px.
    pub height: u32,
    /// One lane per channel instead of the mix.
    pub split: bool,
    /// The dB range of the vertical axis; `None`: linear.
    pub scale_db: Option<f64>,
    /// Also measure the loudness (never for a media file).
    pub loudness: bool,
}

/// The loudness of a waveform's range.
#[derive(Clone, Debug, PartialEq)]
pub struct LoudnessMade {
    /// Integrated loudness in LUFS (not finite below the gate).
    pub integrated: f64,
    /// Short-term loudness (the last 3 s) at these times, in LUFS (not finite below the gate).
    pub short_term: Vec<(Time, f64)>,
}

/// A waveform as the engine made it.
#[derive(Clone, Debug, PartialEq)]
pub struct WaveMade {
    /// The PNG, base64.
    pub data: String,
    /// Its size in bytes.
    pub bytes: u32,
    /// The lanes drawn, top to bottom.
    pub lanes: Vec<WaveLane>,
    /// Sample rate of the sound measured.
    pub sample_rate: u32,
    /// The largest sample magnitude (0..=1, all channels) of each block, in time order.
    pub envelope: Vec<f32>,
    /// Time of the first block's first sample.
    pub envelope_start: Time,
    /// Samples per block.
    pub block_samples: u32,
    /// The loudness, when asked for.
    pub loudness: Option<LoudnessMade>,
    /// Clips whose sound could not be decoded (drawn as silence).
    pub source_errors: u32,
    /// The `/media/...` route that serves the same peaks, if one does.
    pub link: Option<String>,
}

/// What makes the pictures (the engine, which has the caches and the decoders).
pub trait VisualService: Send + Sync {
    /// The filmstrip `request` asks for.
    fn filmstrip(&self, doc: &Project, request: &StripRequest) -> Result<StripMade, CmdError>;

    /// The waveform `request` asks for.
    fn waveform(&self, doc: &Project, request: &WaveRequest) -> Result<WaveMade, CmdError>;
}

/// The context of `media.filmstrip` and `audio.waveform`: the document of the moment and the
/// engine's picture maker.
pub struct VisualCall {
    /// The document.
    pub snapshot: Arc<DocSnapshot>,
    /// The maker; `None` in an engine without media caches and decoders (`Unsupported`).
    pub service: Option<Arc<dyn VisualService>>,
}

impl std::fmt::Debug for VisualCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VisualCall")
            .field("rev", &self.snapshot.rev)
            .field("service", &self.service.is_some())
            .finish()
    }
}

impl VisualCall {
    fn service(&self, what: &str) -> Result<&Arc<dyn VisualService>, CmdError> {
        self.service.as_ref().ok_or_else(|| {
            CmdError::unsupported(format!(
                "{what} needs the media subsystem (caches and decoders), which this engine does \
                 not run"
            ))
        })
    }
}

// ----- shared checks -----------------------------------------------------------------------------

fn in_range(
    path: &str,
    value: Option<u32>,
    default: u32,
    (lo, hi): (u32, u32),
) -> Result<u32, CmdError> {
    let v = value.unwrap_or(default);
    if (lo..=hi).contains(&v) {
        Ok(v)
    } else {
        Err(CmdError::invalid(
            path,
            format!("must be within {lo}..={hi}"),
        ))
    }
}

fn media_time(path: &str, input: TimeInput, probe: &ProbeInfo) -> Result<Time, CmdError> {
    input
        .resolve(probe.fps)
        .map_err(|e| CmdError::invalid(path, e.to_string()))
}

fn sequence_time(path: &str, input: TimeInput, seq: &Sequence) -> Result<Time, CmdError> {
    crate::util::resolve_time(path, input, seq.format.frame_rate)
}

/// `from..to` (defaults `whole`) inside `whole`, not empty. `what` names what `whole` is.
fn range_within(
    from: Option<Time>,
    to: Option<Time>,
    whole: (Time, Time),
    what: &str,
) -> Result<(Time, Time), CmdError> {
    let (start, end) = (from.unwrap_or(whole.0), to.unwrap_or(whole.1));
    if start < whole.0 {
        return Err(CmdError::invalid(
            "/from",
            format!(
                "is before the start of {what} ({} flicks)",
                whole.0.flicks()
            ),
        ));
    }
    if end > whole.1 {
        return Err(CmdError::invalid(
            "/to",
            format!("is past the end of {what} ({} flicks)", whole.1.flicks()),
        ));
    }
    if end <= start {
        return Err(CmdError::invalid(
            if to.is_some() { "/to" } else { "/from" },
            if whole.1 <= whole.0 {
                format!("{what} is empty")
            } else {
                "the range is empty: `to` must be after `from`".to_owned()
            },
        ));
    }
    Ok((start, end))
}

/// The upright picture size of `probe`'s video stream (a quarter turn swaps the sides).
fn picture_size(probe: &ProbeInfo) -> Option<(u32, u32)> {
    let video = probe.streams.iter().find(|s| s.kind == StreamKind::Video)?;
    let (w, h) = (video.width?, video.height?);
    if w == 0 || h == 0 {
        return None;
    }
    Some(if probe.rotation % 180 == 90 {
        (h, w)
    } else {
        (w, h)
    })
}

/// The widest tile such that `count` tiles side by side, and one tile's height, fit `max_size`
/// (never wider than the picture), or `None` when that is under [`MIN_TILE_WIDTH`].
#[must_use]
pub fn tile_width(picture: (u32, u32), count: u32, max_size: u32) -> Option<u32> {
    let (pw, ph) = (u64::from(picture.0.max(1)), u64::from(picture.1.max(1)));
    let by_width = u64::from(max_size / count.max(1));
    // A tile `w` wide is `round(w * ph / pw)` high.
    let mut w = by_width.min(pw);
    while w > 0 && (w * ph + pw / 2) / pw > u64::from(max_size) {
        w -= 1;
    }
    u32::try_from(w).ok().filter(|w| *w >= MIN_TILE_WIDTH)
}

// ----- media.filmstrip ---------------------------------------------------------------------------

/// Frames of a video asset at even intervals, side by side in one JPEG: a quick look at what a
/// clip shows before placing or trimming it. Without `from` and `to` it is the whole clip (the
/// strip the timeline shows, from the thumbnail cache); with them, that part of it. `tiles`
/// says which media time each tile shows. Read-only; the first call for a clip decodes it.
#[wire]
pub struct MediaFilmstrip {
    /// The video (or image-sequence) asset.
    pub media: MediaId,
    /// Start in media time: flicks, "2.5s", "f:60" or a timecode (default: the clip's start).
    #[serde(default)]
    pub from: Option<TimeInput>,
    /// End in media time, exclusive (default: the clip's end).
    #[serde(default)]
    pub to: Option<TimeInput>,
    /// Tiles (default 8, at most 64).
    #[serde(default)]
    pub count: Option<u32>,
    /// Longer side of the strip in px (64-2048, default 768). Tiles are never upscaled.
    #[serde(default)]
    pub max_size: Option<u32>,
}

/// One tile of a filmstrip.
#[wire]
#[derive(Copy, Eq)]
pub struct StripTile {
    /// Media time of the frame it shows (flicks).
    pub t: Time,
    /// Its left edge on the strip, in px.
    pub x: u32,
}

/// Result of `media.filmstrip`.
#[wire]
pub struct FilmstripOutput {
    /// MIME type of `data` (`image/jpeg`).
    pub mime: String,
    /// The strip, base64 (returned over MCP as image content).
    pub data: String,
    /// Encoded size in bytes.
    pub bytes: u32,
    /// Strip width in px.
    pub width: u32,
    /// Strip height in px.
    pub height: u32,
    /// The asset.
    pub media: MediaId,
    /// Start of the part shown (media time, flicks).
    pub from: Time,
    /// End of the part shown, exclusive.
    pub to: Time,
    /// Width of one tile in px.
    pub tile_width: u32,
    /// The tiles, left to right.
    pub tiles: Vec<StripTile>,
    /// Entity tag of the picture: the asset's content hash and a hash of the parameters.
    pub etag: String,
    /// The `/media/...` path that serves the same JPEG over HTTP with that tag (the whole
    /// clip's strip only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

impl Described for MediaFilmstrip {
    const SPEC: CommandSpec = CommandSpec {
        name: "media.filmstrip",
        summary: "Show frames of a video asset at even intervals as one strip image (JPEG)",
        kind: EntryKind::Engine,
        undoable: false,
        view: false,
        scope: Scope::Any,
        mcp: McpExposure::Catalog,
        annotations: READ_ONLY,
        secret_fields: &[],
        rust_type: "MediaFilmstrip",
    };
}

impl Registrable for MediaFilmstrip {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<MediaFilmstrip>().map(drop)
    }
}

impl MediaFilmstrip {
    /// Checks the request against `doc`: the checked request and the part of the clip shown.
    pub fn request(&self, doc: &Project) -> Result<(StripRequest, (Time, Time)), CmdError> {
        let asset = doc
            .media
            .get(&self.media)
            .ok_or_else(|| CmdError::not_found(EntityKind::Media, self.media))?;
        let probe = &asset.probe;
        let Some(picture) = picture_size(probe) else {
            return Err(CmdError::invalid(
                "/media",
                "has no pictures: a filmstrip needs a video or image-sequence asset",
            ));
        };
        let count = in_range(
            "/count",
            self.count,
            DEFAULT_STRIP_TILES,
            (1, MAX_STRIP_TILES),
        )?;
        let max_size = in_range(
            "/max_size",
            self.max_size,
            DEFAULT_STRIP_SIZE,
            (MIN_STRIP_SIZE, MAX_STRIP_SIZE),
        )?;
        let Some(tile_width) = tile_width(picture, count, max_size) else {
            return Err(CmdError::invalid(
                "/max_size",
                format!(
                    "{max_size} px is too small for {count} tiles (each at least \
                     {MIN_TILE_WIDTH} px wide); raise max_size or lower count"
                ),
            ));
        };
        let from = match self.from {
            Some(t) => Some(media_time("/from", t, probe)?),
            None => None,
        };
        let to = match self.to {
            Some(t) => Some(media_time("/to", t, probe)?),
            None => None,
        };
        let Some(duration) = probe.duration else {
            return Err(CmdError::invalid(
                "/media",
                "has no known duration: it is not analysed yet",
            ));
        };
        let whole = (Time::ZERO, Time::from(duration));
        let shown = range_within(from, to, whole, "the clip")?;
        Ok((
            StripRequest {
                media: self.media,
                range: (from.is_some() || to.is_some()).then_some(shown),
                count,
                tile_width,
            },
            shown,
        ))
    }
}

impl EngineCommand for MediaFilmstrip {
    type Context = VisualCall;
    type Output = FilmstripOutput;

    fn run(&self, call: &mut VisualCall) -> Result<FilmstripOutput, CmdError> {
        let doc = &call.snapshot.doc;
        let (request, (from, to)) = self.request(doc)?;
        let made = call.service("a filmstrip")?.filmstrip(doc, &request)?;
        let tiles = (0..request.count)
            .map(|i| StripTile {
                t: Time::from_flicks(
                    made.start
                        .flicks()
                        .saturating_add(made.interval.flicks().saturating_mul(i64::from(i))),
                )
                .unwrap_or(made.start),
                x: i * made.tile_width,
            })
            .collect();
        Ok(FilmstripOutput {
            mime: "image/jpeg".to_owned(),
            data: made.data,
            bytes: made.bytes,
            width: request.count * made.tile_width,
            height: made.tile_height,
            media: request.media,
            from,
            to,
            tile_width: made.tile_width,
            tiles,
            etag: made.etag,
            link: made.link,
        })
    }
}

// ----- audio.waveform ----------------------------------------------------------------------------

/// What `audio.waveform` draws.
#[wire]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WaveTarget {
    /// A media file's own sound (its cached peaks; `from` and `to` are media times).
    Media {
        /// The asset.
        media: MediaId,
        /// The audio stream by container index (default: the first).
        #[serde(default)]
        stream: Option<u32>,
    },
    /// One clip as it plays on the timeline, with its speed, gain and fades (`from` and `to`
    /// are sequence times inside the clip).
    Item {
        /// The clip.
        item: ItemId,
    },
    /// The mix of a sequence over a range, as an export hears it.
    Range {
        /// The sequence; optional when the project has one.
        #[serde(default)]
        sequence: Option<SequenceId>,
    },
}

/// Which signals a waveform shows.
#[wire]
#[derive(Copy, Default, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WaveChannels {
    /// One lane: all channels mixed.
    #[default]
    Mix,
    /// One lane per channel (left and right for a clip or a mix).
    Split,
}

/// Draws sound as a picture (PNG): the peaks of a media file, of one clip as it plays, or of a
/// sequence's mix, over a time ruler, so loud, quiet and silent stretches can be seen. Also
/// returns `audio_levels`: the loudest peak, the peak of up to 64 equal slices of the range and
/// the silent stretches (at least 0.3 s at or below -60 dBFS), and with `loudness` the
/// integrated loudness and its short-term course in LUFS. Read-only. A clip or a mix is decoded
/// and mixed while the call waits (at most one hour of it per call); a media file is read from
/// its cached peaks.
#[wire]
pub struct AudioWaveform {
    /// What to draw: `{"kind":"media","media":id}`, `{"kind":"item","item":id}` or
    /// `{"kind":"range"}` (a sequence's mix; add `sequence` when the project has several).
    pub target: WaveTarget,
    /// Start: flicks, "2.5s", "f:60" or a timecode. Media time for a media file, else sequence
    /// time (default: the start of the file, the clip or the sequence).
    #[serde(default)]
    pub from: Option<TimeInput>,
    /// End, exclusive (default: the end of the file, the clip or the sequence).
    #[serde(default)]
    pub to: Option<TimeInput>,
    /// Picture width in px (64-2048, default 768).
    #[serde(default)]
    pub width: Option<u32>,
    /// Picture height in px (32-1024, default 192).
    #[serde(default)]
    pub height: Option<u32>,
    /// `mix` (default: one lane) or `split` (one lane per channel).
    #[serde(default)]
    pub channels: Option<WaveChannels>,
    /// Draw on a dB scale of this range (12-144; e.g. 60 puts -60 dBFS at the lane's middle and
    /// 0 dBFS at its edge) so quiet sound shows. Default: linear.
    #[serde(default)]
    pub scale_db: Option<f64>,
    /// Also measure loudness (LUFS). For a clip or a sequence's mix, not for a media file.
    #[serde(default)]
    pub loudness: Option<bool>,
}

/// One lane of a waveform picture.
#[wire]
pub struct WaveLane {
    /// The signal: `mix`, `left`, `right`, or `ch1`, `ch2`, ... of a media file.
    pub channel: String,
    /// Left edge on the picture, in px.
    pub x: u32,
    /// Top edge, in px.
    pub y: u32,
    /// Width in px (the whole range).
    pub width: u32,
    /// Height in px (full scale at the top and the bottom edge).
    pub height: u32,
}

/// A stretch of time.
#[wire]
#[derive(Copy, Eq)]
pub struct TimeSpan {
    /// Start (flicks).
    pub from: Time,
    /// End, exclusive.
    pub to: Time,
}

/// A loudness reading.
#[wire]
#[derive(Copy)]
pub struct LoudnessPoint {
    /// When (flicks): the short-term window ends here.
    pub t: Time,
    /// Short-term loudness in LUFS; null below the -70 LUFS gate or before any sound.
    pub lufs: Option<f64>,
}

/// The levels of a waveform's range.
#[wire]
pub struct AudioLevels {
    /// The loudest sample peak in dBFS; null when the range is silent.
    pub peak_db: Option<f64>,
    /// The peak of each of up to 64 equal slices of the range, in dBFS (null: silent).
    pub peaks_db: Vec<Option<f64>>,
    /// Length of one slice (flicks).
    pub slice: Duration,
    /// A peak at or below this, in dBFS, counts as silence.
    pub silence_below_db: f64,
    /// The silent stretches of at least 0.3 s, in time order.
    pub silences: Vec<TimeSpan>,
    /// Integrated loudness in LUFS (with `loudness`; null below the gate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lufs_integrated: Option<f64>,
    /// Short-term loudness (3 s window) over the range (with `loudness`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lufs_short_term: Vec<LoudnessPoint>,
}

/// Result of `audio.waveform`.
#[wire]
pub struct WaveformOutput {
    /// MIME type of `data` (`image/png`).
    pub mime: String,
    /// The picture, base64 (returned over MCP as image content).
    pub data: String,
    /// Encoded size in bytes.
    pub bytes: u32,
    /// Picture width in px.
    pub width: u32,
    /// Picture height in px.
    pub height: u32,
    /// Start of the range drawn (flicks; media time for a media file, else sequence time).
    pub from: Time,
    /// End of the range drawn, exclusive.
    pub to: Time,
    /// The sequence, for a clip or a mix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<SequenceId>,
    /// Sample rate of the sound measured, in Hz.
    pub sample_rate: u32,
    /// The lanes, top to bottom; the ruler is under them.
    pub lanes: Vec<WaveLane>,
    /// The dB range of the vertical axis; absent: linear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_db: Option<f64>,
    /// Peaks, silences and (on request) loudness of the range.
    pub audio_levels: AudioLevels,
    /// Clips whose sound could not be decoded (drawn as silence).
    pub source_errors: u32,
    /// Document revision drawn.
    pub rev: Rev,
    /// The `/media/...` path that serves the same peaks over HTTP (a media file only), as a
    /// plain PNG without the ruler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

impl Described for AudioWaveform {
    const SPEC: CommandSpec = CommandSpec {
        name: "audio.waveform",
        summary: "Draw the sound of a media file, a clip or a mix as a waveform image, with levels and silences",
        kind: EntryKind::Engine,
        undoable: false,
        view: false,
        scope: Scope::Any,
        mcp: McpExposure::Catalog,
        annotations: READ_ONLY,
        secret_fields: &[],
        rust_type: "AudioWaveform",
    };
}

impl Registrable for AudioWaveform {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<AudioWaveform>().map(drop)
    }
}

/// The sequence named by `id`, or the project's only one.
fn resolve_sequence(doc: &Project, id: Option<SequenceId>) -> Result<&Sequence, CmdError> {
    let path = "/target/sequence";
    if let Some(id) = id {
        return doc
            .sequences
            .get(&id)
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, id));
    }
    let mut all = doc.sequences.values();
    match (all.next(), all.next()) {
        (Some(s), None) => Ok(s),
        (None, _) => Err(CmdError::invalid(path, "the project has no sequence")),
        (Some(_), Some(_)) => Err(CmdError::invalid(
            path,
            format!(
                "the project has {} sequences; name one",
                doc.sequences.len()
            ),
        )),
    }
}

impl AudioWaveform {
    fn media_range(
        &self,
        doc: &Project,
        media: MediaId,
        stream: Option<u32>,
    ) -> Result<(WaveSource, (Time, Time)), CmdError> {
        let asset = doc
            .media
            .get(&media)
            .ok_or_else(|| CmdError::not_found(EntityKind::Media, media))?;
        let probe = &asset.probe;
        let has_sound = probe
            .streams
            .iter()
            .any(|s| s.kind == StreamKind::Audio && stream.is_none_or(|index| s.index == index));
        if !has_sound {
            return Err(match stream {
                Some(index) => CmdError::invalid(
                    "/target/stream",
                    format!("stream {index} of this asset is not an audio stream"),
                ),
                None => CmdError::invalid("/target/media", "has no sound"),
            });
        }
        let Some(duration) = probe.duration else {
            return Err(CmdError::invalid(
                "/target/media",
                "has no known duration: it is not analysed yet",
            ));
        };
        let from = match self.from {
            Some(t) => Some(media_time("/from", t, probe)?),
            None => None,
        };
        let to = match self.to {
            Some(t) => Some(media_time("/to", t, probe)?),
            None => None,
        };
        let range = range_within(from, to, (Time::ZERO, Time::from(duration)), "the file")?;
        Ok((WaveSource::Media { media, stream }, range))
    }

    fn timeline_range(
        &self,
        seq: &Sequence,
        whole: (Time, Time),
        what: &str,
    ) -> Result<(Time, Time), CmdError> {
        let from = match self.from {
            Some(t) => Some(sequence_time("/from", t, seq)?),
            None => None,
        };
        let to = match self.to {
            Some(t) => Some(sequence_time("/to", t, seq)?),
            None => None,
        };
        let (start, end) = range_within(from, to, whole, what)?;
        let longest = FLICKS_PER_SECOND.saturating_mul(MAX_MIX_SECONDS);
        if end.flicks().saturating_sub(start.flicks()) > longest {
            return Err(CmdError::invalid(
                "/to",
                format!(
                    "the range is longer than {MAX_MIX_SECONDS} s, the most of a clip or a mix \
                     one call draws; give a shorter `from`..`to`"
                ),
            ));
        }
        Ok((start, end))
    }

    /// Checks the request against `doc`.
    pub fn request(&self, doc: &Project) -> Result<WaveRequest, CmdError> {
        let width = in_range(
            "/width",
            self.width,
            DEFAULT_WAVE_WIDTH,
            (MIN_WAVE_WIDTH, MAX_WAVE_WIDTH),
        )?;
        let height = in_range(
            "/height",
            self.height,
            DEFAULT_WAVE_HEIGHT,
            (MIN_WAVE_HEIGHT, MAX_WAVE_HEIGHT),
        )?;
        if let Some(db) = self.scale_db
            && !(SCALE_DB_RANGE.0..=SCALE_DB_RANGE.1).contains(&db)
        {
            return Err(CmdError::invalid(
                "/scale_db",
                format!("must be within {}..={}", SCALE_DB_RANGE.0, SCALE_DB_RANGE.1),
            ));
        }
        let loudness = self.loudness.unwrap_or(false);
        let (source, (from, to)) = match &self.target {
            WaveTarget::Media { media, stream } => {
                if loudness {
                    return Err(CmdError::invalid(
                        "/loudness",
                        "a media file's cached peaks carry no loudness: use a target of kind \
                         `item` or `range`, or audio.get_levels",
                    ));
                }
                self.media_range(doc, *media, *stream)?
            }
            WaveTarget::Item { item } => {
                let Some((seq, found)) = doc
                    .sequences
                    .values()
                    .find_map(|seq| seq.find_item(item).map(|(_, found)| (seq, found)))
                else {
                    return Err(CmdError::not_found(EntityKind::Item, item));
                };
                let end = found.end().map_err(|e| CmdError::internal(e.to_string()))?;
                let range = self.timeline_range(seq, (found.start(), end), "the clip")?;
                (
                    WaveSource::Item {
                        sequence: seq.id,
                        item: *item,
                    },
                    range,
                )
            }
            WaveTarget::Range { sequence } => {
                let seq = resolve_sequence(doc, *sequence)?;
                let whole = (Time::ZERO, Time::from(seq.duration()));
                let range = self.timeline_range(seq, whole, "the sequence")?;
                (WaveSource::Mix { sequence: seq.id }, range)
            }
        };
        Ok(WaveRequest {
            source,
            from,
            to,
            width,
            height,
            split: self.channels.unwrap_or_default() == WaveChannels::Split,
            scale_db: self.scale_db,
            loudness,
        })
    }
}

fn flicks_of_samples(samples: i64, rate: u32) -> i64 {
    let flicks = i128::from(samples) * i128::from(FLICKS_PER_SECOND) / i128::from(rate.max(1));
    i64::try_from(flicks).unwrap_or(i64::MAX)
}

/// The silent stretches of `envelope` (the largest sample magnitude of each block of
/// `block_samples` samples at `rate` Hz, the first block starting at `start`): runs of blocks
/// at or below [`SILENCE_BELOW_DB`] lasting at least [`MIN_SILENCE_MS`], cut to `from..to`.
#[must_use]
pub fn silences(
    envelope: &[f32],
    start: Time,
    block_samples: u32,
    rate: u32,
    (from, to): (Time, Time),
) -> Vec<TimeSpan> {
    // A hair above the threshold, so a peak stored as `f32` at exactly -60 dBFS is silence.
    let threshold = 10f64.powf(SILENCE_BELOW_DB / 20.0) * (1.0 + 1e-6);
    let shortest = FLICKS_PER_SECOND / 1000 * MIN_SILENCE_MS;
    let time_of = |block: usize| {
        let samples = i64::try_from(block)
            .unwrap_or(i64::MAX)
            .saturating_mul(i64::from(block_samples));
        let t = start
            .flicks()
            .saturating_add(flicks_of_samples(samples, rate));
        Time::from_flicks(t).unwrap_or(to).clamp(from, to.max(from))
    };
    let mut out = Vec::new();
    let mut run: Option<usize> = None;
    for i in 0..=envelope.len() {
        let quiet = envelope
            .get(i)
            .is_some_and(|peak| f64::from(*peak) <= threshold);
        match (run, quiet) {
            (None, true) => run = Some(i),
            (Some(first), false) => {
                let (a, b) = (time_of(first), time_of(i));
                if b.flicks() - a.flicks() >= shortest {
                    out.push(TimeSpan { from: a, to: b });
                }
                run = None;
            }
            _ => {}
        }
    }
    out
}

/// The levels of a waveform: the loudest peak, the peaks of up to [`LEVEL_SLICES`] equal
/// slices of `request`'s range, the silences and, when measured, the loudness.
#[must_use]
pub fn audio_levels(made: &WaveMade, request: &WaveRequest) -> AudioLevels {
    let peak = made.envelope.iter().copied().fold(0f32, f32::max);
    let slices = made.envelope.len().clamp(1, LEVEL_SLICES);
    let peaks_db = (0..slices)
        .map(|i| {
            let a = i * made.envelope.len() / slices;
            let b = ((i + 1) * made.envelope.len() / slices).max(a);
            let peak = made.envelope[a..b].iter().copied().fold(0f32, f32::max);
            level_db(f64::from(peak))
        })
        .collect();
    let span = request.to.flicks().saturating_sub(request.from.flicks());
    let slice =
        Duration::from_flicks(span / i64::try_from(slices).unwrap_or(1)).unwrap_or(Duration::ZERO);
    let finite = |v: f64| v.is_finite().then_some(v);
    AudioLevels {
        peak_db: level_db(f64::from(peak)),
        peaks_db,
        slice,
        silence_below_db: SILENCE_BELOW_DB,
        silences: silences(
            &made.envelope,
            made.envelope_start,
            made.block_samples,
            made.sample_rate,
            (request.from, request.to),
        ),
        lufs_integrated: made.loudness.as_ref().and_then(|l| finite(l.integrated)),
        lufs_short_term: made.loudness.as_ref().map_or_else(Vec::new, |l| {
            l.short_term
                .iter()
                .map(|(t, lufs)| LoudnessPoint {
                    t: *t,
                    lufs: finite(*lufs),
                })
                .collect()
        }),
    }
}

impl EngineCommand for AudioWaveform {
    type Context = VisualCall;
    type Output = WaveformOutput;

    fn run(&self, call: &mut VisualCall) -> Result<WaveformOutput, CmdError> {
        let doc = &call.snapshot.doc;
        let request = self.request(doc)?;
        let made = call.service("a waveform")?.waveform(doc, &request)?;
        let audio_levels = audio_levels(&made, &request);
        Ok(WaveformOutput {
            mime: "image/png".to_owned(),
            data: made.data,
            bytes: made.bytes,
            width: request.width,
            height: request.height,
            from: request.from,
            to: request.to,
            sequence: match request.source {
                WaveSource::Media { .. } => None,
                WaveSource::Item { sequence, .. } | WaveSource::Mix { sequence } => Some(sequence),
            },
            sample_rate: made.sample_rate,
            lanes: made.lanes,
            scale_db: request.scale_db,
            audio_levels,
            source_errors: made.source_errors,
            rev: call.snapshot.rev,
            link: made.link,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::{
        Clip, Extra, FrameRate, Item, MediaAsset, MediaKind, SampleRate, SequenceFormat,
        StreamInfo, Track, TrackId, TrackKind,
    };
    use serde_json::json;

    use super::*;

    fn secs(s: f64) -> Time {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        Time::from_flicks((s * FLICKS_PER_SECOND as f64).round() as i64).unwrap()
    }

    fn stream(index: u32, kind: StreamKind, size: Option<(u32, u32)>) -> StreamInfo {
        StreamInfo {
            index,
            kind,
            codec: None,
            profile: None,
            width: size.map(|s| s.0),
            height: size.map(|s| s.1),
            pixel_format: None,
            sample_rate: (kind == StreamKind::Audio).then_some(SampleRate::HZ_48_000),
            channels: (kind == StreamKind::Audio).then_some(2),
            language: None,
            extra: Extra::new(),
        }
    }

    const VIDEO: u128 = 3;
    const SOUND: u128 = 4;
    const ITEM: u128 = 9;

    /// A 10 s video with sound (1920x1080, 30 fps), a 10 s sound file, and one sequence with
    /// the sound file's clip at 2 s..6 s.
    fn doc() -> (Project, SequenceId) {
        let mut project = Project::new("Visual");
        let ten = Duration::from_secs(10).unwrap();
        let mut probe = ProbeInfo {
            duration: Some(ten),
            fps: Some(FrameRate::FPS_30),
            ..ProbeInfo::default()
        };
        probe
            .streams
            .push(stream(0, StreamKind::Video, Some((1920, 1080))));
        probe.streams.push(stream(1, StreamKind::Audio, None));
        let video = MediaId::from_u128(VIDEO);
        project.media.insert(
            video,
            MediaAsset::file(video, MediaKind::Video, "/m/v.mp4".to_owned(), probe),
        );
        let mut probe = ProbeInfo {
            duration: Some(ten),
            ..ProbeInfo::default()
        };
        probe.streams.push(stream(0, StreamKind::Audio, None));
        let sound = MediaId::from_u128(SOUND);
        project.media.insert(
            sound,
            MediaAsset::file(sound, MediaKind::Audio, "/m/a.wav".to_owned(), probe),
        );
        let id = SequenceId::from_u128(7);
        let format = SequenceFormat::new(1280, 720, FrameRate::FPS_30, SampleRate::HZ_48_000);
        let mut seq = Sequence::new(id, "Main", format);
        let track = TrackId::from_u128(1);
        seq.insert_track(0, Track::new(track, TrackKind::Audio, "A1"));
        let clip = Clip::new(
            ItemId::from_u128(ITEM),
            sound,
            secs(2.0),
            Duration::from_secs(4).unwrap(),
        );
        seq.track_mut(&track)
            .unwrap()
            .items
            .insert(Item::from(clip));
        project.sequences.insert(id, seq);
        (project, id)
    }

    fn strip(v: serde_json::Value) -> MediaFilmstrip {
        serde_json::from_value(v).unwrap()
    }

    fn wave(v: serde_json::Value) -> AudioWaveform {
        serde_json::from_value(v).unwrap()
    }

    fn path_of(e: &CmdError) -> &str {
        match e {
            CmdError::Invalid { path, .. } => path,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn tiles_fit_max_size_both_ways() {
        // Landscape: the width decides.
        assert_eq!(tile_width((1920, 1080), 8, 768), Some(96));
        // Portrait: one tile's height must fit too.
        let w = tile_width((1080, 1920), 1, 768).unwrap();
        assert_eq!(w, 432);
        assert!((u64::from(w) * 1920 + 540) / 1080 <= 768);
        // Never wider than the picture; too many tiles for the size is refused.
        assert_eq!(tile_width((40, 30), 2, 768), Some(40));
        assert_eq!(tile_width((1920, 1080), 64, 64), None);
        for max_size in [64, 100, 128, 500, 767, 2048] {
            for count in [1, 3, 8, 64] {
                for picture in [(1920, 1080), (1080, 1920), (640, 640), (4096, 256)] {
                    if let Some(w) = tile_width(picture, count, max_size) {
                        let h = (u64::from(w) * u64::from(picture.1) + u64::from(picture.0) / 2)
                            / u64::from(picture.0);
                        assert!(w * count <= max_size && h <= u64::from(max_size));
                        assert!(w <= picture.0 && w >= MIN_TILE_WIDTH);
                    }
                }
            }
        }
    }

    #[test]
    fn a_filmstrip_request_is_checked_against_the_asset() {
        let (doc, _) = doc();
        let video = MediaId::from_u128(VIDEO);
        let (request, shown) = strip(json!({ "media": video })).request(&doc).unwrap();
        assert_eq!(
            request,
            StripRequest {
                media: video,
                range: None,
                count: 8,
                tile_width: 96
            }
        );
        assert_eq!(shown, (secs(0.0), secs(10.0)));
        // A part of the clip; frames resolve at the media's rate.
        let (request, shown) = strip(json!({ "media": video, "from": "f:30", "count": 4 }))
            .request(&doc)
            .unwrap();
        assert_eq!(request.range, Some((secs(1.0), secs(10.0))));
        assert_eq!(shown, (secs(1.0), secs(10.0)));
        assert_eq!(request.tile_width, 192);
        for (params, path) in [
            (json!({ "media": MediaId::from_u128(SOUND) }), "/media"),
            (json!({ "media": video, "count": 0 }), "/count"),
            (json!({ "media": video, "count": 65 }), "/count"),
            (json!({ "media": video, "max_size": 63 }), "/max_size"),
            (json!({ "media": video, "max_size": 2049 }), "/max_size"),
            (
                json!({ "media": video, "count": 64, "max_size": 64 }),
                "/max_size",
            ),
            (json!({ "media": video, "from": "4s", "to": "4s" }), "/to"),
            (json!({ "media": video, "to": "11s" }), "/to"),
            (json!({ "media": video, "from": "-1s" }), "/from"),
        ] {
            let e = strip(params.clone()).request(&doc).unwrap_err();
            assert_eq!(path_of(&e), path, "{params}: {e}");
        }
        let unknown = strip(json!({ "media": MediaId::from_u128(99) }))
            .request(&doc)
            .unwrap_err();
        assert_eq!(unknown.code(), "not_found");
    }

    #[test]
    fn a_waveform_request_resolves_its_target_and_range() {
        let (doc, seq) = doc();
        let sound = MediaId::from_u128(SOUND);
        let r = wave(json!({ "target": { "kind": "media", "media": sound } }))
            .request(&doc)
            .unwrap();
        assert_eq!(
            r.source,
            WaveSource::Media {
                media: sound,
                stream: None
            }
        );
        assert_eq!((r.from, r.to), (secs(0.0), secs(10.0)));
        assert_eq!(
            (r.width, r.height, r.split, r.scale_db),
            (768, 192, false, None)
        );
        // A clip: its own extent by default, sequence times inside it otherwise.
        let item = ItemId::from_u128(ITEM);
        let r = wave(json!({ "target": { "kind": "item", "item": item } }))
            .request(&doc)
            .unwrap();
        assert_eq!(
            r.source,
            WaveSource::Item {
                sequence: seq,
                item
            }
        );
        assert_eq!((r.from, r.to), (secs(2.0), secs(6.0)));
        let r = wave(json!({
            "target": { "kind": "item", "item": item }, "from": "3s", "to": "f:120",
            "channels": "split", "scale_db": 60, "loudness": true
        }))
        .request(&doc)
        .unwrap();
        assert_eq!((r.from, r.to), (secs(3.0), secs(4.0)));
        assert!(r.split && r.loudness);
        assert_eq!(r.scale_db, Some(60.0));
        // The mix of the project's only sequence.
        let r = wave(json!({ "target": { "kind": "range" }, "to": "5s" }))
            .request(&doc)
            .unwrap();
        assert_eq!(r.source, WaveSource::Mix { sequence: seq });
        assert_eq!((r.from, r.to), (secs(0.0), secs(5.0)));

        let media = json!({ "kind": "media", "media": sound });
        let clip = json!({ "kind": "item", "item": item });
        for (params, path) in [
            (json!({ "target": media, "width": 63 }), "/width"),
            (json!({ "target": media, "width": 2049 }), "/width"),
            (json!({ "target": media, "height": 31 }), "/height"),
            (json!({ "target": media, "height": 1025 }), "/height"),
            (json!({ "target": media, "scale_db": 6 }), "/scale_db"),
            (json!({ "target": media, "scale_db": 200 }), "/scale_db"),
            (json!({ "target": media, "loudness": true }), "/loudness"),
            (json!({ "target": media, "to": "11s" }), "/to"),
            (
                json!({ "target": { "kind": "media", "media": sound, "stream": 5 } }),
                "/target/stream",
            ),
            (json!({ "target": clip, "from": "1s" }), "/from"),
            (json!({ "target": clip, "to": "7s" }), "/to"),
            (json!({ "target": clip, "from": "4s", "to": "3s" }), "/to"),
            (json!({ "target": { "kind": "range" }, "to": "7s" }), "/to"),
        ] {
            let e = wave(params.clone()).request(&doc).unwrap_err();
            assert_eq!(path_of(&e), path, "{params}: {e}");
        }
        let unknown = wave(json!({ "target": { "kind": "item", "item": ItemId::from_u128(1) } }))
            .request(&doc)
            .unwrap_err();
        assert_eq!(unknown.code(), "not_found");
    }

    /// The envelope of `seconds` of sound at 48 kHz in 256-sample blocks: 0.5 where `loud`.
    fn envelope(seconds: f64, loud: impl Fn(f64) -> bool) -> Vec<f32> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let blocks = (seconds * 48_000.0 / 256.0).ceil() as usize;
        (0..blocks)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let t = (i * 256) as f64 / 48_000.0;
                if loud(t) { 0.5 } else { 0.0 }
            })
            .collect()
    }

    /// WP-2I.02 acceptance: a known 2 s silence is reported within 50 ms.
    #[test]
    fn a_two_second_silence_is_found_within_50_ms() {
        let fifty_ms = FLICKS_PER_SECOND / 20;
        for (start, end) in [(3.0, 5.0), (0.0, 2.0), (6.5, 8.5), (1.2345, 3.2345)] {
            let env = envelope(10.0, |t| t < start || t >= end);
            let found = silences(&env, Time::ZERO, 256, 48_000, (secs(0.0), secs(10.0)));
            assert_eq!(found.len(), 1, "{start}..{end}: {found:?}");
            let span = found[0];
            assert!(
                (span.from.flicks() - secs(start).flicks()).abs() <= fifty_ms,
                "{start}: {span:?}"
            );
            assert!(
                (span.to.flicks() - secs(end).flicks()).abs() <= fifty_ms,
                "{end}: {span:?}"
            );
        }
        // A pause shorter than 0.3 s is not a silence; one that runs to the end is.
        let env = envelope(4.0, |t| !(1.0..1.2).contains(&t) && t < 3.0);
        let found = silences(&env, Time::ZERO, 256, 48_000, (secs(0.0), secs(4.0)));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].to, secs(4.0));
        // -60 dBFS is silence, -59 dBFS is not; a later start shifts the times.
        let quiet = vec![0.001_f32; 400];
        let at = (secs(5.0), secs(9.0));
        assert_eq!(silences(&quiet, secs(5.0), 256, 48_000, at).len(), 1);
        assert_eq!(
            silences(&quiet, secs(5.0), 256, 48_000, at)[0].from,
            secs(5.0)
        );
        let audible = vec![0.001_122_f32; 400];
        assert_eq!(silences(&audible, secs(5.0), 256, 48_000, at), []);
        assert_eq!(silences(&[], Time::ZERO, 256, 48_000, at), []);
    }

    #[test]
    fn levels_slice_the_range_and_carry_the_loudness() {
        let (doc, _) = doc();
        let request = wave(json!({ "target": { "kind": "range" }, "to": "4s", "loudness": true }))
            .request(&doc)
            .unwrap();
        // 0.5 (-6.02 dBFS) for the first two seconds, then silence.
        let env = envelope(4.0, |t| t < 2.0);
        let made = WaveMade {
            data: String::new(),
            bytes: 0,
            lanes: Vec::new(),
            sample_rate: 48_000,
            envelope: env,
            envelope_start: Time::ZERO,
            block_samples: 256,
            loudness: Some(LoudnessMade {
                integrated: -9.0,
                short_term: vec![(secs(1.0), f64::NEG_INFINITY), (secs(3.0), -9.5)],
            }),
            source_errors: 0,
            link: None,
        };
        let levels = audio_levels(&made, &request);
        assert!((levels.peak_db.unwrap() + 6.0206).abs() < 0.001);
        assert_eq!(levels.peaks_db.len(), 64);
        assert_eq!(levels.slice.flicks(), secs(4.0).flicks() / 64);
        assert!(levels.peaks_db[..31].iter().all(Option::is_some));
        assert!(levels.peaks_db[33..].iter().all(Option::is_none));
        assert_eq!(levels.silences.len(), 1);
        assert_eq!(levels.silences[0].to, secs(4.0));
        assert_eq!(levels.lufs_integrated, Some(-9.0));
        assert_eq!(levels.lufs_short_term.len(), 2);
        assert_eq!(levels.lufs_short_term[0].lufs, None);
        assert_eq!(levels.lufs_short_term[1].lufs, Some(-9.5));
        // Without a measurement the loudness fields are left out of the answer.
        let made = WaveMade {
            loudness: None,
            envelope: Vec::new(),
            ..made
        };
        let json = serde_json::to_value(audio_levels(&made, &request)).unwrap();
        assert!(json.get("lufs_integrated").is_none() && json.get("lufs_short_term").is_none());
        assert_eq!(json["peak_db"], serde_json::Value::Null);
        assert_eq!(json["peaks_db"].as_array().unwrap().len(), 1);
    }
}
