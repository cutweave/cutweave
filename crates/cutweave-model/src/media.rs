// SPDX-License-Identifier: GPL-3.0-or-later
//! Media assets (arch §4.2 `MediaAsset`): files referenced by the project, or other sequences
//! used as compound clips, with their probe results.

use std::{borrow::Cow, fmt, str::FromStr};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Custom, Duration, Extra, FrameRate, MediaId, SampleRate, SequenceId};

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
    /// Another sequence of this project, used as a compound clip (arch §4.3).
    SequenceRef,
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
    /// Absolute path of the file when it was last found (file kinds only).
    pub path_abs: Option<String>,
    /// Path relative to the project folder, used for relinking moved projects (file kinds only).
    pub path_rel: Option<String>,
    /// Content hash used to relink and to key caches (file kinds only).
    pub content_hash: Option<String>,
    /// The referenced sequence (`sequence_ref` only).
    pub sequence: Option<SequenceId>,
    /// Probe results.
    pub probe: ProbeInfo,
    /// Plugin data (arch §4.3).
    pub custom: Custom,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
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
            path_abs: Some(path_abs.into()),
            path_rel: None,
            content_hash: None,
            sequence: None,
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
            path_abs: None,
            path_rel: None,
            content_hash: None,
            sequence: Some(sequence),
            probe: ProbeInfo::default(),
            custom: Custom::new(),
            extra: Extra::new(),
        }
    }

    /// Whether the media has pictures (video, image or a sequence).
    #[must_use]
    pub fn has_video(&self) -> bool {
        match self.kind {
            MediaKind::Video | MediaKind::Image | MediaKind::SequenceRef => true,
            MediaKind::Audio => false,
        }
    }

    /// Whether the media can have sound (audio files, video files and sequences).
    #[must_use]
    pub fn has_audio(&self) -> bool {
        match self.kind {
            MediaKind::Audio | MediaKind::Video | MediaKind::SequenceRef => true,
            MediaKind::Image => false,
        }
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
