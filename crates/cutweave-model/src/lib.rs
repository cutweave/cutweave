// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-model`: the project document types, flicks time, IDs, schema versioning and
//! migrations (arch §4). Every other engine crate builds on it; it compiles to
//! `wasm32-unknown-unknown` so the UI can share its math (arch §3, §16).
//!
//! - **Time** ([`Time`], [`Duration`], [`FrameRate`], [`SampleRate`], [`Timecode`],
//!   [`TimeInput`]): integer flicks (1/705,600,000 s) with checked arithmetic and the only
//!   allowed frame/sample conversions (ADR-0004); JSON integers on the wire (ADR-0015).
//! - **IDs** ([`MediaId`], [`SequenceId`], [`TrackId`], [`ItemId`], [`GroupId`], [`IdGen`]):
//!   prefixed UUIDv7 strings, with a seeded generator for deterministic tests.
//! - **Entities** ([`Project`], [`MediaAsset`], [`Sequence`], [`Track`], [`Item`], [`Clip`]):
//!   the schema v1 document, held in persistent `imbl` collections so snapshots are O(1)
//!   (ADR-0005), with [`Project::validate`] checking the [`Invariant`]s of arch §4.3. Plugin
//!   data lives only in namespaced [`Custom`] maps; unknown fields survive in [`Extra`].

mod custom;
mod id;
mod item;
mod media;
mod project;
#[cfg(test)]
mod proptests;
mod rate;
mod sequence;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod time;
mod timecode;
mod validate;

pub use custom::{
    Custom, Extra, NAMESPACE_PATTERN, NamespaceError, canonical_json_pretty, is_valid_namespace,
};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use id::Uuid7Gen;
pub use id::{
    EntityId, EntityKind, GroupId, IdError, IdGen, ItemId, MediaId, SeededGen, SequenceId, TrackId,
};
pub use item::{Animatable, AudioProps, Clip, Item, Keyframes, Transform, Vec2, VisualProps};
pub use media::{
    AudioInfo, ColorPrimaries, ColorRange, ColorTags, MatrixCoefficients, MediaAsset, MediaKind,
    ProbeInfo, StreamInfo, StreamKind, TransferCharacteristics,
};
pub use project::{ItemRef, Project, ProjectMeta, ProjectSettings};
pub use rate::{FrameRate, Rounding, SampleRate};
pub use sequence::{
    Color, ColorError, ColorSpace, Sequence, SequenceFormat, Track, TrackItems, TrackKind,
};
pub use time::{
    ABSOLUTE_TIME_PATTERN, Duration, FLICKS_PER_SECOND, MAX_SAFE_FLICKS, Time, TimeError,
};
pub use timecode::{TIME_INPUT_PATTERN, TimeInput, Timecode};
pub use validate::{Invariant, MAX_CANVAS_SIDE, MAX_SEQUENCE_FPS, SEQUENCE_SAMPLE_RATES};
