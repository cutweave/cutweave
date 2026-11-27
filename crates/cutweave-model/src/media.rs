// SPDX-License-Identifier: GPL-3.0-or-later
//! Media assets (arch §4.2 `MediaAsset`): files referenced by the project, or other sequences
//! used as compound clips, with their probe results.

use std::{borrow::Cow, fmt, str::FromStr};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Custom, Duration, Extra, FolderId, FrameRate, MediaId, SampleRate, SequenceId};

/// What a [`MediaAsset`] is.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// A video file (may also carry audio).
    Video,
    /// An audio-only file.
    Audio,
    /// A still image (no duration).
    Image,
    /// A numbered run of pictures that plays like video (ADR-0035); the asset's
    /// `image_sequence` field describes the run.
    ImageSequence,
    /// Another sequence of this project, used as a compound clip (arch §4.3).
    SequenceRef,
}

/// Whether one asset gets a proxy (ADR-0027), next to the project's policy.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
    specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    /// As the project's policy decides. The default.
    #[default]
    Auto,
    /// One is made whatever the policy.
    Force,
    /// None is made, and the preview does not use one that exists.
    Off,
}

impl ProxyMode {
    /// Whether it is the default, which is left out of the file.
    #[must_use]
    pub fn is_auto(&self) -> bool {
        *self == ProxyMode::Auto
    }

    /// Lowercase name for messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            ProxyMode::Auto => "auto",
            ProxyMode::Force => "force",
            ProxyMode::Off => "off",
        }
    }
}

impl MediaKind {
    /// Whether the media is a file on disk (and so has paths and a content hash).
    #[must_use]
    pub const fn is_file(self) -> bool {
        !matches!(self, MediaKind::SequenceRef)
    }

    /// Lowercase name for messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            MediaKind::Video => "video",
            MediaKind::Audio => "audio",
            MediaKind::Image => "image",
            MediaKind::ImageSequence => "image_sequence",
            MediaKind::SequenceRef => "sequence_ref",
        }
    }
}

impl fmt::Display for MediaKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Media imported into (or generated for) a project, referenced by path and content hash
/// (arch §17), or a sequence of this project used as a compound clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct MediaAsset {
    /// The media ID (also its key in `Project::media`).
    pub id: MediaId,
    /// What the media is.
    pub kind: MediaKind,
    /// The name shown in the bin (ADR-0020); absent: the file name. 1 to 255 characters
    /// without control characters.
    pub name: Option<String>,
    /// The folder of the bin it is in (ADR-0020); absent: the top level.
    pub folder: Option<FolderId>,
    /// Absolute path of the file when it was last found (file kinds only).
    pub path_abs: Option<String>,
    /// Path relative to the project folder, used for relinking moved projects (file kinds only).
    pub path_rel: Option<String>,
    /// Content hash used to relink and to key caches (file kinds only).
    pub content_hash: Option<String>,
    /// The referenced sequence (`sequence_ref` only).
    pub sequence: Option<SequenceId>,
    /// The run of pictures (`image_sequence` only, ADR-0035); left out of the file otherwise.
    /// `path_abs` and `path_rel` name the first file of the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_sequence: Option<ImageSequence>,
    /// Whether it gets a proxy (ADR-0027): as the project's policy decides, always or never.
    /// `auto` is left out of the file, so documents written before the field read the same.
    #[serde(default, skip_serializing_if = "ProxyMode::is_auto")]
    pub proxy_mode: ProxyMode,
    /// Probe results.
    pub probe: ProbeInfo,
    /// Plugin data (arch §4.3).
    pub custom: Custom,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// A numbered run of pictures imported as one asset (ADR-0035): frame `n` (from 0) is the file
/// `pattern` with the number `first + n`. Files are referenced, never copied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct ImageSequence {
    /// The file name with the counter as `%0Nd` (zero-padded to `N` digits) or `%d`
    /// (`shot_%04d.exr`); a literal `%` is written `%%`. A name, not a path: the files are in
    /// the directory of `path_abs`.
    pub pattern: String,
    /// The number of the first file.
    pub first: u32,
    /// How many numbers the run covers, from `first` (missing ones included); at least 2.
    pub count: u32,
    /// The frame rate chosen at import; changing the app setting never changes it.
    pub fps: FrameRate,
    /// The numbers with no file at import time, ascending. A missing frame shows the last
    /// picture before it.
    pub missing: Vec<u32>,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// The most numbers one image sequence may cover (about 9 hours at 30 fps).
pub const MAX_IMAGE_SEQUENCE_FRAMES: u32 = 1_000_000;

/// The path component that marks an image-sequence locator ([`ImageSequence::locator`]).
const LOCATOR_TAG: &str = "@image-sequence;";

impl ImageSequence {
    /// Where the counter is in `pattern`: the text before it, the zero-padded width (`None` for
    /// `%d`) and the text after it. `None` if the pattern has no counter, more than one, or
    /// another `%` directive.
    #[must_use]
    pub fn split_pattern(pattern: &str) -> Option<(String, Option<u8>, String)> {
        let mut before = String::new();
        let mut after = String::new();
        let mut counter: Option<Option<u8>> = None;
        let mut chars = pattern.chars().peekable();
        while let Some(c) = chars.next() {
            let out = if counter.is_some() {
                &mut after
            } else {
                &mut before
            };
            if c != '%' {
                out.push(c);
                continue;
            }
            if chars.peek() == Some(&'%') {
                chars.next();
                out.push('%');
                continue;
            }
            if counter.is_some() {
                return None;
            }
            let mut digits = String::new();
            while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                digits.push(d);
                chars.next();
            }
            if chars.next() != Some('d') {
                return None;
            }
            let width = if digits.is_empty() {
                None
            } else {
                // Only the zero-padded form: `%4d` would pad with spaces.
                let width = digits.strip_prefix('0')?.parse::<u8>().ok()?;
                (1..=20).contains(&width).then_some(width)?;
                Some(width)
            };
            counter = Some(width);
        }
        counter.map(|width| (before, width, after))
    }

    /// The pattern for a file name whose counter is `digits` (as written, so `0007` gives
    /// `%04d` and `7` gives `%d`) between `before` and `after`.
    #[must_use]
    pub fn make_pattern(before: &str, digits: &str, after: &str) -> String {
        let escape = |s: &str| s.replace('%', "%%");
        let counter = if digits.len() > 1 && digits.starts_with('0') {
            format!("%0{}d", digits.len())
        } else {
            "%d".to_owned()
        };
        format!("{}{counter}{}", escape(before), escape(after))
    }

    /// The file name of `number`, or `None` if the pattern is malformed.
    #[must_use]
    pub fn file_name(&self, number: u32) -> Option<String> {
        let (before, width, after) = Self::split_pattern(&self.pattern)?;
        Some(match width {
            Some(width) => format!(
                "{before}{number:0width$}{after}",
                width = usize::from(width)
            ),
            None => format!("{before}{number}{after}"),
        })
    }

    /// The number of the file that frame `frame` (from 0) shows: `first + frame`, or the last
    /// number before it that is not missing. Frames past the end show the last one.
    #[must_use]
    pub fn number_for_frame(&self, frame: u32) -> u32 {
        let last = self.count.saturating_sub(1);
        let mut number = self.first.saturating_add(frame.min(last));
        // `missing` is ascending: walk back over a run of missing numbers.
        while number > self.first && self.missing.binary_search(&number).is_ok() {
            number -= 1;
        }
        number
    }

    /// The playable length: `count / fps`, in whole flicks.
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        let end = self.fps.frame_to_time(i64::from(self.count)).ok()?;
        Duration::from_flicks(end.flicks()).ok()
    }

    /// The numbers with no file, as ascending inclusive ranges (`[(120, 121), (250, 250)]`).
    #[must_use]
    pub fn missing_ranges(&self) -> Vec<(u32, u32)> {
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for &n in &self.missing {
            match ranges.last_mut() {
                Some((_, end)) if end.checked_add(1) == Some(n) => *end = n,
                _ => ranges.push((n, n)),
            }
        }
        ranges
    }

    /// The runs of numbers that have files, as `(first number, how many)`, ascending: the run
    /// without its missing numbers.
    #[must_use]
    pub fn segments(&self) -> Vec<(u32, u32)> {
        let mut segments = Vec::new();
        let mut start = self.first;
        let end = self.first.saturating_add(self.count);
        for (from, to) in self.missing_ranges() {
            if from > start {
                segments.push((start, from - start));
            }
            start = to.saturating_add(1).max(start);
        }
        if end > start {
            segments.push((start, end - start));
        }
        segments
    }

    /// The path a media backend reads the whole run by (ADR-0035): the pattern in the directory
    /// of `first_file`, followed by one more path component that carries the start number, the
    /// count, the frame rate and the missing numbers
    /// (`/renders/shot_%04d.png/@image-sequence;first=98;count=300;fps=30x1;missing=120-121`).
    /// It names no file on disk (a picture has no children), so it cannot be mistaken for one;
    /// [`parse_locator`](Self::parse_locator) reads it back.
    #[must_use]
    pub fn locator(&self, first_file: &str) -> String {
        let cut = first_file.rfind(['/', '\\']).map_or(0, |i| i + 1);
        let dir = &first_file[..cut];
        let sep = if dir.contains('\\') && !dir.contains('/') {
            '\\'
        } else {
            '/'
        };
        let missing: Vec<String> = self
            .missing_ranges()
            .into_iter()
            .map(|(a, b)| {
                if a == b {
                    a.to_string()
                } else {
                    format!("{a}-{b}")
                }
            })
            .collect();
        format!(
            "{dir}{}{sep}{LOCATOR_TAG}first={};count={};fps={}x{};missing={}",
            self.pattern,
            self.first,
            self.count,
            self.fps.num(),
            self.fps.den(),
            missing.join(",")
        )
    }

    /// Reads a [`locator`](Self::locator) back: the directory (with its trailing separator;
    /// empty for a bare pattern) and the run. `None` for any other path.
    #[must_use]
    pub fn parse_locator(locator: &str) -> Option<(String, ImageSequence)> {
        let tag_at = locator.rfind(LOCATOR_TAG)?;
        let head = locator[..tag_at].strip_suffix(['/', '\\'])?;
        let cut = head.rfind(['/', '\\']).map_or(0, |i| i + 1);
        let (dir, pattern) = head.split_at(cut);
        let mut first = None;
        let mut count = None;
        let mut fps = None;
        let mut missing = Vec::new();
        for field in locator[tag_at + LOCATOR_TAG.len()..].split(';') {
            let (key, value) = field.split_once('=')?;
            match key {
                "first" => first = Some(value.parse::<u32>().ok()?),
                "count" => count = Some(value.parse::<u32>().ok()?),
                "fps" => {
                    let (num, den) = value.split_once('x')?;
                    fps = Some(FrameRate::new(num.parse().ok()?, den.parse().ok()?).ok()?);
                }
                "missing" => {
                    for range in value.split(',').filter(|r| !r.is_empty()) {
                        let (a, b) = range.split_once('-').unwrap_or((range, range));
                        let (a, b) = (a.parse::<u32>().ok()?, b.parse::<u32>().ok()?);
                        if a > b || b - a >= MAX_IMAGE_SEQUENCE_FRAMES {
                            return None;
                        }
                        missing.extend(a..=b);
                    }
                }
                _ => return None,
            }
        }
        let run = ImageSequence {
            pattern: pattern.to_owned(),
            first: first?,
            count: count?,
            fps: fps?,
            missing,
            extra: Extra::new(),
        };
        run.problem().is_none().then(|| (dir.to_owned(), run))
    }

    /// What is wrong with the description, if anything.
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        if self.pattern.contains(['/', '\\']) || Self::split_pattern(&self.pattern).is_none() {
            return Some(
                "an image sequence pattern must be a file name with exactly one %0Nd or %d counter",
            );
        }
        if !(2..=MAX_IMAGE_SEQUENCE_FRAMES).contains(&self.count) {
            return Some("an image sequence must cover between 2 and 1,000,000 numbers");
        }
        // FFmpeg's start number is a signed 32-bit integer.
        let Some(end) = self
            .first
            .checked_add(self.count)
            .filter(|end| *end <= i32::MAX.unsigned_abs())
        else {
            return Some("an image sequence's numbers must stay below 2,147,483,647");
        };
        if !self.missing.windows(2).all(|w| w[0] < w[1]) {
            return Some("an image sequence's missing numbers must be ascending without repeats");
        }
        // The first and the last number are files: the run starts and ends on a picture.
        if self
            .missing
            .iter()
            .any(|n| *n <= self.first || *n >= end - 1)
        {
            return Some(
                "an image sequence's missing numbers must lie between its first and last file",
            );
        }
        None
    }
}

impl MediaAsset {
    /// A file asset at `path_abs` with the given probe results.
    #[must_use]
    pub fn file(
        id: MediaId,
        kind: MediaKind,
        path_abs: impl Into<String>,
        probe: ProbeInfo,
    ) -> Self {
        MediaAsset {
            id,
            kind,
            name: None,
            folder: None,
            path_abs: Some(path_abs.into()),
            path_rel: None,
            content_hash: None,
            sequence: None,
            image_sequence: None,
            proxy_mode: ProxyMode::Auto,
            probe,
            custom: Custom::new(),
            extra: Extra::new(),
        }
    }

    /// A compound-clip asset that plays `sequence`.
    #[must_use]
    pub fn sequence_ref(id: MediaId, sequence: SequenceId) -> Self {
        MediaAsset {
            id,
            kind: MediaKind::SequenceRef,
            name: None,
            folder: None,
            path_abs: None,
            path_rel: None,
            content_hash: None,
            sequence: Some(sequence),
            image_sequence: None,
            proxy_mode: ProxyMode::Auto,
            probe: ProbeInfo::default(),
            custom: Custom::new(),
            extra: Extra::new(),
        }
    }

    /// Whether the media has pictures (video, image or a sequence).
    #[must_use]
    pub fn has_video(&self) -> bool {
        match self.kind {
            MediaKind::Video
            | MediaKind::Image
            | MediaKind::ImageSequence
            | MediaKind::SequenceRef => true,
            MediaKind::Audio => false,
        }
    }

    /// Whether the media can have sound (audio files, video files and sequences).
    #[must_use]
    pub fn has_audio(&self) -> bool {
        match self.kind {
            MediaKind::Audio | MediaKind::Video | MediaKind::SequenceRef => true,
            MediaKind::Image | MediaKind::ImageSequence => false,
        }
    }

    /// The path the media backend decodes this asset by: the file, or for an image sequence the
    /// locator of its run ([`ImageSequence::locator`]). `None` for media without a file.
    #[must_use]
    pub fn source_path(&self) -> Option<String> {
        let path = self.path_abs.as_ref()?;
        Some(match &self.image_sequence {
            Some(run) if self.kind == MediaKind::ImageSequence => run.locator(path),
            _ => path.clone(),
        })
    }

    /// An image-sequence asset (ADR-0035): `first_file` is the absolute path of the first
    /// picture of `run`. The probe's duration and frame rate come from the run.
    #[must_use]
    pub fn image_sequence(
        id: MediaId,
        first_file: impl Into<String>,
        run: ImageSequence,
        mut probe: ProbeInfo,
    ) -> Self {
        probe.duration = run.duration();
        probe.fps = Some(run.fps);
        probe.vfr = false;
        probe.audio = None;
        let mut asset = MediaAsset::file(id, MediaKind::ImageSequence, first_file, probe);
        asset.image_sequence = Some(run);
        asset
    }
}

/// What the media backend found in a file (WP-1D.02 fills it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct ProbeInfo {
    /// Streams in container order.
    pub streams: Vec<StreamInfo>,
    /// Playable length, if known (images have none).
    pub duration: Option<Duration>,
    /// Frame rate of the main video stream, if constant or nominal.
    pub fps: Option<FrameRate>,
    /// Variable frame rate: times map through the frame index, never through `fps` (arch §4.1).
    pub vfr: bool,
    /// Display rotation in degrees clockwise: 0, 90, 180 or 270.
    pub rotation: u16,
    /// Color tags of the main video stream.
    pub color: ColorTags,
    /// Main audio stream format, if any.
    pub audio: Option<AudioInfo>,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// The kind of a container stream.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// Pictures.
    Video,
    /// Sound.
    Audio,
    /// Subtitles or captions.
    Subtitle,
    /// Timed metadata or other data.
    Data,
    /// Attached files (fonts, cover art).
    Attachment,
}

/// One stream of a media file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct StreamInfo {
    /// Index in the container.
    pub index: u32,
    /// Stream kind.
    pub kind: StreamKind,
    /// Codec name (FFmpeg naming, e.g. `h264`, `aac`).
    pub codec: Option<String>,
    /// Codec profile (e.g. `High`, `Main 10`).
    pub profile: Option<String>,
    /// Coded width in pixels (video).
    pub width: Option<u32>,
    /// Coded height in pixels (video).
    pub height: Option<u32>,
    /// Pixel format (FFmpeg naming, e.g. `yuv420p10le`; video).
    pub pixel_format: Option<String>,
    /// Sample rate (audio).
    pub sample_rate: Option<SampleRate>,
    /// Channel count (audio).
    pub channels: Option<u16>,
    /// Language tag (e.g. `eng`).
    pub language: Option<String>,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// Format of the main audio stream.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(deny_unknown_fields)]
pub struct AudioInfo {
    /// Samples per second.
    pub rate: SampleRate,
    /// Channel count.
    pub channels: u16,
}

/// Color description of a video stream (H.273 code points by FFmpeg name). `None` means
/// unspecified in the file.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(deny_unknown_fields)]
pub struct ColorTags {
    /// Color primaries.
    pub primaries: Option<ColorPrimaries>,
    /// Transfer characteristics.
    pub transfer: Option<TransferCharacteristics>,
    /// YUV matrix coefficients.
    pub matrix: Option<MatrixCoefficients>,
    /// Sample range.
    pub range: Option<ColorRange>,
}

/// Sample range of a video stream.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum ColorRange {
    /// Limited ("TV", 16–235 for 8-bit luma).
    Limited,
    /// Full ("PC", 0–255).
    Full,
}

/// A string-valued enum of well-known names plus `Other` for anything else, kept verbatim so
/// newer probe output survives a round trip.
macro_rules! string_enum {
    (
        $(#[$doc:meta])*
        $name:ident { $($(#[$vdoc:meta])* $variant:ident = $text:literal,)+ }
    ) => {
        $(#[$doc])*
        ///
        /// Serialized as its name (e.g. `"bt709"`); unknown names are kept as `Other`.
        #[derive(Clone, Debug, PartialEq, Eq, Hash, specta::Type)]
        #[specta(type = String)]
        pub enum $name {
            $($(#[$vdoc])* $variant,)+
            /// A name this version does not know.
            Other(String),
        }

        impl $name {
            /// The wire name.
            #[must_use]
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $text,)+
                    Self::Other(name) => name,
                }
            }
        }

        impl FromStr for $name {
            type Err = std::convert::Infallible;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(match s {
                    $($text => Self::$variant,)+
                    other => Self::Other(other.to_owned()),
                })
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let name = Cow::<'de, str>::deserialize(d)?;
                let Ok(value) = name.parse();
                Ok(value)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn inline_schema() -> bool {
                true
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "examples": [$($text),+]
                })
            }
        }
    };
}

string_enum!(
    /// Color primaries (H.273).
    ColorPrimaries {
        /// BT.709 / sRGB.
        Bt709 = "bt709",
        /// BT.470 System M.
        Bt470M = "bt470m",
        /// BT.601 625-line (PAL).
        Bt470Bg = "bt470bg",
        /// BT.601 525-line (NTSC).
        Smpte170M = "smpte170m",
        /// SMPTE 240M.
        Smpte240M = "smpte240m",
        /// Generic film.
        Film = "film",
        /// BT.2020 / BT.2100.
        Bt2020 = "bt2020",
        /// SMPTE ST 428 (XYZ).
        Smpte428 = "smpte428",
        /// DCI-P3 (SMPTE RP 431-2).
        Smpte431 = "smpte431",
        /// Display P3 (SMPTE EG 432-1).
        Smpte432 = "smpte432",
        /// EBU Tech 3213-E.
        Ebu3213 = "ebu3213",
    }
);

string_enum!(
    /// Transfer characteristics (H.273).
    TransferCharacteristics {
        /// BT.709.
        Bt709 = "bt709",
        /// Gamma 2.2.
        Gamma22 = "gamma22",
        /// Gamma 2.8.
        Gamma28 = "gamma28",
        /// BT.601.
        Smpte170M = "smpte170m",
        /// SMPTE 240M.
        Smpte240M = "smpte240m",
        /// Linear light.
        Linear = "linear",
        /// sRGB (IEC 61966-2-1).
        Srgb = "iec61966-2-1",
        /// BT.2020 10-bit.
        Bt2020_10 = "bt2020-10",
        /// BT.2020 12-bit.
        Bt2020_12 = "bt2020-12",
        /// PQ (SMPTE ST 2084).
        Pq = "smpte2084",
        /// HLG (ARIB STD-B67).
        Hlg = "arib-std-b67",
    }
);

string_enum!(
    /// YUV matrix coefficients (H.273).
    MatrixCoefficients {
        /// RGB (identity, `gbr`).
        Rgb = "gbr",
        /// BT.709.
        Bt709 = "bt709",
        /// FCC.
        Fcc = "fcc",
        /// BT.601 625-line.
        Bt470Bg = "bt470bg",
        /// BT.601 525-line.
        Smpte170M = "smpte170m",
        /// SMPTE 240M.
        Smpte240M = "smpte240m",
        /// YCgCo.
        YCgCo = "ycgco",
        /// BT.2020 non-constant luminance.
        Bt2020Ncl = "bt2020nc",
        /// BT.2020 constant luminance.
        Bt2020Cl = "bt2020c",
        /// ICtCp.
        ICtCp = "ictcp",
    }
);

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn color_tags_round_trip_and_keep_unknown_names() {
        let tags: ColorTags = serde_json::from_value(json!({
            "primaries": "bt2020",
            "transfer": "arib-std-b67",
            "matrix": "something-new",
            "range": "limited",
        }))
        .unwrap();
        assert_eq!(tags.primaries, Some(ColorPrimaries::Bt2020));
        assert_eq!(tags.transfer, Some(TransferCharacteristics::Hlg));
        assert_eq!(
            tags.matrix,
            Some(MatrixCoefficients::Other("something-new".into()))
        );
        assert_eq!(tags.range, Some(ColorRange::Limited));
        let back = serde_json::to_value(&tags).unwrap();
        assert_eq!(back["matrix"], "something-new");
        assert_eq!(serde_json::from_value::<ColorTags>(back).unwrap(), tags);
        assert!(serde_json::from_value::<ColorTags>(json!({"gamut": "x"})).is_err());
        assert_eq!(TransferCharacteristics::Pq.to_string(), "smpte2084");
    }

    fn run(pattern: &str, first: u32, count: u32, missing: &[u32]) -> ImageSequence {
        ImageSequence {
            pattern: pattern.into(),
            first,
            count,
            fps: FrameRate::FPS_30,
            missing: missing.to_vec(),
            extra: Extra::new(),
        }
    }

    #[test]
    fn image_sequence_patterns_split_and_format() {
        assert_eq!(
            ImageSequence::split_pattern("shot_%04d.exr"),
            Some(("shot_".into(), Some(4), ".exr".into()))
        );
        assert_eq!(
            ImageSequence::split_pattern("100%% %d.png"),
            Some(("100% ".into(), None, ".png".into()))
        );
        for bad in [
            "plain.png",
            "a_%04d_%04d.png",
            "a_%4d.png",
            "a_%s.png",
            "a_%",
            "%00d",
        ] {
            assert_eq!(ImageSequence::split_pattern(bad), None, "{bad}");
        }
        assert_eq!(
            ImageSequence::make_pattern("shot_", "0007", ".exr"),
            "shot_%04d.exr"
        );
        assert_eq!(
            ImageSequence::make_pattern("50%_", "7", ".png"),
            "50%%_%d.png"
        );
        assert_eq!(ImageSequence::make_pattern("f", "0", ".png"), "f%d.png");
        let seq = run("shot_%04d.exr", 98, 5, &[]);
        assert_eq!(seq.file_name(98).as_deref(), Some("shot_0098.exr"));
        assert_eq!(seq.file_name(12345).as_deref(), Some("shot_12345.exr"));
        assert_eq!(
            run("f%d.png", 9, 3, &[]).file_name(10).as_deref(),
            Some("f10.png")
        );
    }

    #[test]
    fn a_missing_frame_shows_the_last_picture_before_it() {
        let seq = run("f_%03d.png", 10, 8, &[12, 13, 15]);
        let shown: Vec<u32> = (0..10).map(|f| seq.number_for_frame(f)).collect();
        assert_eq!(shown, [10, 11, 11, 11, 14, 14, 16, 17, 17, 17]);
        assert_eq!(seq.problem(), None);
    }

    #[test]
    fn locators_round_trip_and_real_paths_are_not_locators() {
        let mut seq = run("shot 50%%_%04d.png", 98, 300, &[120, 121, 250]);
        seq.fps = FrameRate::FPS_23_976;
        let unix = seq.locator("/renders/a b/shot 50%_0098.png");
        assert_eq!(
            unix,
            "/renders/a b/shot 50%%_%04d.png/@image-sequence;first=98;count=300;fps=24000x1001;missing=120-121,250"
        );
        assert_eq!(
            ImageSequence::parse_locator(&unix),
            Some(("/renders/a b/".to_owned(), seq.clone()))
        );
        let win = seq.locator("D:\\renders\\shot 50%_0098.png");
        assert!(
            win.starts_with("D:\\renders\\shot 50%%_%04d.png\\@image-sequence;"),
            "{win}"
        );
        assert_eq!(
            ImageSequence::parse_locator(&win),
            Some(("D:\\renders\\".to_owned(), seq.clone()))
        );
        let plain = run("f%d.png", 0, 2, &[]);
        assert_eq!(
            ImageSequence::parse_locator(&plain.locator("f0.png")),
            Some((String::new(), plain))
        );
        for not in [
            "/renders/shot_0098.png",
            "/renders/shot_%04d.png",
            "/renders/@image-sequence;first=1;count=2;fps=30x1;missing=",
            "/r/f%d.png/@image-sequence;first=1;count=1;fps=30x1;missing=",
            "/r/f%d.png/@image-sequence;first=1;count=5;fps=30x0;missing=",
            "/r/f%d.png/@image-sequence;first=1;count=5;fps=30x1;missing=9",
            "/r/f%d.png/@image-sequence;first=1;count=5;fps=30x1;missing=;more=1",
            "/r/f%d.png/@image-sequence;count=5;fps=30x1;missing=",
        ] {
            assert_eq!(ImageSequence::parse_locator(not), None, "{not}");
        }
    }

    #[test]
    fn segments_are_the_runs_between_the_missing_numbers() {
        assert_eq!(run("f%d.png", 10, 8, &[]).segments(), [(10, 8)]);
        let seq = run("f%d.png", 10, 8, &[12, 13, 15]);
        assert_eq!(seq.missing_ranges(), [(12, 13), (15, 15)]);
        assert_eq!(seq.segments(), [(10, 2), (14, 1), (16, 2)]);
    }

    #[test]
    fn the_source_path_of_a_sequence_is_its_locator() {
        let asset = MediaAsset::image_sequence(
            MediaId::from_u128(4),
            "/renders/shot_0001.exr",
            run("shot_%04d.exr", 1, 300, &[]),
            ProbeInfo::default(),
        );
        assert_eq!(
            asset.source_path().as_deref(),
            Some("/renders/shot_%04d.exr/@image-sequence;first=1;count=300;fps=30x1;missing=")
        );
        let still = MediaAsset::file(
            MediaId::from_u128(1),
            MediaKind::Image,
            "/media/still.png",
            ProbeInfo::default(),
        );
        assert_eq!(still.source_path().as_deref(), Some("/media/still.png"));
        let compound = MediaAsset::sequence_ref(MediaId::from_u128(2), SequenceId::from_u128(3));
        assert_eq!(compound.source_path(), None);
    }

    #[test]
    fn image_sequence_duration_is_count_over_fps() {
        let mut seq = run("f_%03d.png", 1, 300, &[]);
        assert_eq!(seq.duration(), Some(Duration::from_secs(10).unwrap()));
        seq.fps = FrameRate::FPS_23_976;
        seq.count = 24_000;
        assert_eq!(seq.duration(), Some(Duration::from_secs(1001).unwrap()));
    }

    #[test]
    fn image_sequence_problems() {
        assert!(run("f.png", 0, 2, &[]).problem().is_some());
        assert!(run("d/f%d.png", 0, 2, &[]).problem().is_some());
        assert!(run("f%d.png", 0, 1, &[]).problem().is_some());
        assert!(
            run("f%d.png", 0, MAX_IMAGE_SEQUENCE_FRAMES + 1, &[])
                .problem()
                .is_some()
        );
        assert!(run("f%d.png", u32::MAX - 1, 2, &[]).problem().is_some());
        assert!(run("f%d.png", 0, 5, &[3, 2]).problem().is_some());
        assert!(run("f%d.png", 0, 5, &[2, 2]).problem().is_some());
        assert!(run("f%d.png", 0, 5, &[0]).problem().is_some());
        assert!(run("f%d.png", 0, 5, &[4]).problem().is_some());
        assert_eq!(run("f%d.png", 0, 5, &[1, 2, 3]).problem(), None);
    }

    #[test]
    fn an_image_sequence_asset_is_video_without_audio() {
        let asset = MediaAsset::image_sequence(
            MediaId::from_u128(4),
            "/renders/shot_0001.exr",
            run("shot_%04d.exr", 1, 300, &[]),
            ProbeInfo::default(),
        );
        assert!(asset.has_video() && !asset.has_audio() && asset.kind.is_file());
        assert_eq!(asset.probe.duration, Some(Duration::from_secs(10).unwrap()));
        assert_eq!(asset.probe.fps, Some(FrameRate::FPS_30));
        let value = serde_json::to_value(&asset).unwrap();
        assert_eq!(value["kind"], "image_sequence");
        assert_eq!(value["image_sequence"]["pattern"], "shot_%04d.exr");
        assert_eq!(serde_json::from_value::<MediaAsset>(value).unwrap(), asset);
        // Other kinds leave the field out of the file.
        let still = MediaAsset::file(
            MediaId::from_u128(1),
            MediaKind::Image,
            "/media/still.png",
            ProbeInfo::default(),
        );
        assert!(
            serde_json::to_value(&still)
                .unwrap()
                .get("image_sequence")
                .is_none()
        );
    }

    #[test]
    fn asset_constructors_and_capabilities() {
        let file = MediaAsset::file(
            MediaId::from_u128(1),
            MediaKind::Image,
            "/media/still.png",
            ProbeInfo::default(),
        );
        assert!(file.has_video() && !file.has_audio());
        assert!(file.kind.is_file());
        let compound = MediaAsset::sequence_ref(MediaId::from_u128(2), SequenceId::from_u128(3));
        assert!(compound.has_video() && compound.has_audio());
        assert!(!compound.kind.is_file());
        assert_eq!(compound.kind.to_string(), "sequence_ref");
        let value = serde_json::to_value(&compound).unwrap();
        assert_eq!(value["kind"], "sequence_ref");
        assert_eq!(
            serde_json::from_value::<MediaAsset>(value).unwrap(),
            compound
        );
    }
}
