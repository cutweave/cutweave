// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-model`: the project document types, flicks time, IDs, schema versioning and
//! migrations (arch §4). Every other engine crate builds on it; it compiles to
//! `wasm32-unknown-unknown` so the UI can share its math (arch §3, §16).
//!
//! - **Time** ([`Time`], [`Duration`], [`FrameRate`], [`SampleRate`], [`Timecode`],
//!   [`TimeInput`]): integer flicks (1/705,600,000 s) with checked arithmetic and the only
//!   allowed frame/sample conversions (ADR-0004); JSON integers on the wire (ADR-0015).
//! - **IDs** ([`MediaId`], [`SequenceId`], [`TrackId`], [`ItemId`], [`GroupId`], [`MarkerId`],
//!   [`FolderId`], [`CollectionId`], [`IdGen`]): prefixed UUIDv7 strings, with a seeded generator for
//!   deterministic tests.
//! - **Entities** ([`Project`], [`MediaAsset`], [`MediaFolder`], [`Collection`], [`Sequence`],
//!   [`Track`], [`Item`], [`Clip`]): the schema v2 document, held in persistent `imbl` collections so
//!   snapshots are O(1) (ADR-0005), with [`Project::validate`] checking the [`Invariant`]s of
//!   arch §4.3. Plugin data lives only in namespaced [`Custom`] maps; unknown fields survive in
//!   [`Extra`].
//! - **Keyframes** ([`Animatable`], [`Keyframes`], [`Interp`], [`Easing`], [`Evaluator`],
//!   [`PropId`]): the pure, deterministic evaluator of every animated property
//!   (`docs/spec/keyframes.md`, ADR-0024), shared with the UI through wasm (arch §16).
//! - **Versioning** ([`CURRENT_SCHEMA_VERSION`], [`open`], [`migrate_to_current`],
//!   [`Migration`], [`OpenOutcome`]): ordered `vN → vN+1` migrations on the raw JSON with golden
//!   fixtures; documents from newer versions open read-only (arch §4.3, ADR-0006).

mod animate;
mod collection;
mod custom;
mod folder;
mod id;
mod item;
mod marker;
mod media;
mod migrate;
mod presets;
mod project;
#[cfg(test)]
mod proptests;
mod rate;
mod sequence;
mod snap;
mod speed;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod time;
mod timecode;
mod validate;

pub use animate::{
    AnimValue, Animatable, BEZIER_Y_LIMIT, Easing, Evaluator, Interp, Keyframe, KeyframeError,
    KeyframeRule, Keyframes, LinearRgba, MAX_KEYFRAMES, PRESET_PATTERN, PropId, PropSpec,
    PropValue, SPRING_DAMPING, SPRING_MASS, SPRING_STIFFNESS, ShortestAngle, UnknownEasing,
    UnknownProp, ValueRange, ValueType, is_preset_name, sample_times,
};
pub use collection::{
    Collection, CollectionKind, MAX_COLLECTION_MEMBERS, MAX_COLLECTION_NAME_CHARS, MAX_COLLECTIONS,
    MAX_FILTER_TAGS, MAX_FILTER_TEXT_CHARS, MAX_NOTE_CHARS, MAX_TAG_CHARS, MAX_TAGS_PER_ASSET,
    MediaFilter, RATINGS, collection_name_key, collection_name_problem, fold, note_problem,
    tag_key, tag_problem, tags_problem,
};
pub use custom::{
    Custom, Extra, NAMESPACE_PATTERN, NamespaceError, canonical_json_pretty, is_valid_namespace,
};
pub use folder::{
    MAX_FOLDER_DEPTH, MAX_FOLDER_NAME_CHARS, MAX_MEDIA_NAME_CHARS, MediaFolder, folder_name_key,
    folder_name_problem, media_name_problem,
};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use id::Uuid7Gen;
pub use id::{
    CollectionId, EntityId, EntityKind, FolderId, GroupId, IdError, IdGen, ItemId, MarkerId,
    MediaId, SeededGen, SequenceId, TrackId,
};
pub use item::{
    AudioProps, BlendMode, Clip, Crop, Fade, FadeCurve, Flip, Item, PropMut, PropRef, Transform,
    Vec2, VisualProps,
};
pub use marker::{InOut, Marker, MarkerKind, MarkerKindError, Markers};
pub use media::{
    AudioInfo, ColorPrimaries, ColorRange, ColorTags, ImageSequence, MAX_IMAGE_SEQUENCE_FRAMES,
    MatrixCoefficients, MediaAsset, MediaKind, ProbeInfo, ProxyMode, StreamInfo, StreamKind,
    TransferCharacteristics,
};
pub use migrate::{
    CURRENT_SCHEMA_VERSION, MIGRATIONS, Migration, MigrationError, MigrationReport, MigrationStep,
    Migrator, OLDEST_SCHEMA_VERSION, OpenOutcome, migrate_to_current, migrate_value, open,
    open_str, schema_version_of, to_document_string,
};
pub use presets::{ASPECT_PRESETS, AspectPreset, FRAME_RATE_PRESETS, aspect_preset};
pub use project::{
    DEFAULT_STILL_DURATION_SECS, ItemRef, MAX_STILL_DURATION_SECS, Project, ProjectMeta,
    ProjectSettings, ProxyPolicy,
};
pub use rate::{FrameRate, Rounding, SampleRate};
pub use sequence::{
    Color, ColorError, ColorSpace, Sequence, SequenceFormat, Track, TrackItems, TrackKind,
};
pub use snap::{SnapError, SnapIndex, SnapKind, SnapKinds, SnapOptions, SnapTarget, Snapped, snap};
pub use speed::{
    Freeze, MAX_SPEED_TERM, SourceMap, Speed, SpeedError, fit as fit_source_in, source_time,
};
pub use time::{
    ABSOLUTE_TIME_PATTERN, Duration, FLICKS_PER_SECOND, MAX_SAFE_FLICKS, Time, TimeError,
};
pub use timecode::{TIME_INPUT_PATTERN, TimeInput, Timecode};
pub use validate::{
    Invariant, MAX_CANVAS_SIDE, MAX_SEQUENCE_FPS, SEQUENCE_SAMPLE_RATES, TRACK_HEIGHTS,
};
