// SPDX-License-Identifier: GPL-3.0-or-later
//! Markers and the sequence in/out range (schema v2; timeline spec §1.5, ADR-0023).
//!
//! A [`Marker`] flags a time (or a time range) on a sequence or on an item. Sequence markers
//! use sequence time; item markers use item-relative time and so move with their item. The
//! in/out range ([`InOut`]) is the span that lift, extract and three-point edits act on.

use std::{fmt, str::FromStr};

use imbl::OrdMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Color, Duration, Extra, MarkerId, Time};

/// The set of markers on a sequence or an item, keyed by ID (and serialized in ID order).
pub type Markers = OrdMap<MarkerId, Marker>;

/// What a marker means.
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
pub enum MarkerKind {
    /// Placed by a person or an agent.
    #[default]
    User,
    /// A beat found by analysis (snaps like a marker, spec §6).
    Beat,
    /// A chapter start (exported to chaptered formats).
    Chapter,
}

impl MarkerKind {
    /// Lowercase name for messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            MarkerKind::User => "user",
            MarkerKind::Beat => "beat",
            MarkerKind::Chapter => "chapter",
        }
    }
}

impl fmt::Display for MarkerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A string that is not a marker kind.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown marker kind `{0}`; use user, beat or chapter")]
pub struct MarkerKindError(pub String);

impl FromStr for MarkerKind {
    type Err = MarkerKindError;
    fn from_str(s: &str) -> Result<MarkerKind, MarkerKindError> {
        match s {
            "user" => Ok(MarkerKind::User),
            "beat" => Ok(MarkerKind::Beat),
            "chapter" => Ok(MarkerKind::Chapter),
            other => Err(MarkerKindError(other.to_owned())),
        }
    }
}

/// A marker on a sequence (sequence time) or an item (item-relative time).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct Marker {
    /// The marker ID (also its key in the owner's `markers`).
    pub id: MarkerId,
    /// Where the marker is (`>= 0`).
    pub t: Time,
    /// The length of a range marker; `None` for a point marker.
    pub duration: Option<Duration>,
    /// Text shown with the marker (may be empty).
    pub label: String,
    /// Display color.
    pub color: Color,
    /// What the marker means.
    pub kind: MarkerKind,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

impl Marker {
    /// The color of a marker created without one (marigold: "here", design/README.md).
    pub const DEFAULT_COLOR: Color = Color::rgb(0xf2, 0xb1, 0x34);

    /// A point marker of kind `user` with an empty label and the default color.
    #[must_use]
    pub fn new(id: MarkerId, t: Time) -> Marker {
        Marker {
            id,
            t,
            duration: None,
            label: String::new(),
            color: Marker::DEFAULT_COLOR,
            kind: MarkerKind::User,
            extra: Extra::new(),
        }
    }
}

/// The sequence's in and out points: the half-open range `[start, end)` that lift, extract and
/// three-point edits use. Both ends or neither (`Sequence::in_out` is `None` when unset).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct InOut {
    /// The in point (`>= 0`).
    pub start: Time,
    /// The out point (`> start`).
    pub end: Time,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

impl InOut {
    /// The range `[start, end)`; validation requires `0 <= start < end`.
    #[must_use]
    pub fn new(start: Time, end: Time) -> InOut {
        InOut {
            start,
            end,
            extra: Extra::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use serde_json::json;

    use super::*;

    #[test]
    fn marker_wire_form() {
        let mut m = Marker::new(MarkerId::from_u128(1), Time::from_secs(2).unwrap());
        m.label = "Intro".into();
        m.kind = MarkerKind::Chapter;
        m.duration = Some(Duration::from_secs(1).unwrap());
        let value = serde_json::to_value(&m).unwrap();
        assert_eq!(value["t"], 1_411_200_000_i64);
        assert_eq!(value["kind"], "chapter");
        assert_eq!(value["color"], "#f2b134");
        assert_eq!(value["duration"], 705_600_000_i64);
        let back: Marker = serde_json::from_value(value).unwrap();
        assert_eq!(back, m);
        let point = serde_json::to_value(Marker::new(MarkerId::from_u128(2), Time::ZERO)).unwrap();
        assert_eq!(point["duration"], json!(null));
    }

    #[test]
    fn kinds_parse_and_print() {
        for kind in [MarkerKind::User, MarkerKind::Beat, MarkerKind::Chapter] {
            assert_eq!(kind.to_string().parse::<MarkerKind>(), Ok(kind));
        }
        assert!("bar".parse::<MarkerKind>().is_err());
    }

    #[test]
    fn in_out_keeps_unknown_fields() {
        let value = json!({"start": 0, "end": 10, "x_note": 1});
        let io: InOut = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(io.extra.get("x_note"), Some(&json!(1)));
        assert_eq!(serde_json::to_value(&io).unwrap(), value);
    }
}
