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

mod id;
#[cfg(test)]
mod proptests;
mod rate;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod time;
mod timecode;

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use id::Uuid7Gen;
pub use id::{
    EntityId, EntityKind, GroupId, IdError, IdGen, ItemId, MediaId, SeededGen, SequenceId, TrackId,
};
pub use rate::{FrameRate, Rounding, SampleRate};
pub use time::{
    ABSOLUTE_TIME_PATTERN, Duration, FLICKS_PER_SECOND, MAX_SAFE_FLICKS, Time, TimeError,
};
pub use timecode::{TIME_INPUT_PATTERN, TimeInput, Timecode};
