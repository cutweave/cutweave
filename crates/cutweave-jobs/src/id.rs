// SPDX-License-Identifier: GPL-3.0-or-later
//! [`JobId`]: `job_` followed by 32 lowercase hex digits, minted from the engine's injected
//! [`IdGen`] (so `--deterministic-ids` covers jobs too, ADR-0015).
//!
//! Jobs are not document entities, so the ID lives here rather than in `cutweave-model` (whose
//! entity-ID set is part of the project format).

use std::{borrow::Cow, fmt, str::FromStr};

use cutweave_model::IdGen;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// The JSON Schema pattern of a [`JobId`].
pub const JOB_ID_PATTERN: &str = "^job_[0-9a-f]{32}$";

/// The ID of a background job: `job_…` (a UUIDv7, so IDs sort by creation).
///
/// Serialized as a string, exported to TypeScript as `string`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, specta::Type)]
#[specta(type = String)]
pub struct JobId(u128);

/// A string that is not a job ID.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("expected a job id (`job_` followed by 32 lowercase hex digits), got `{0}`")]
pub struct JobIdError(pub String);

impl JobId {
    /// Wraps a raw 128-bit value.
    #[must_use]
    pub const fn from_u128(raw: u128) -> JobId {
        JobId(raw)
    }

    /// The raw 128-bit value.
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }

    /// A fresh ID from `ids`.
    pub fn mint(ids: &mut (impl IdGen + ?Sized)) -> JobId {
        JobId(ids.next_u128())
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "job_{:032x}", self.0)
    }
}

impl fmt::Debug for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for JobId {
    type Err = JobIdError;

    fn from_str(s: &str) -> Result<JobId, JobIdError> {
        let err = || JobIdError(s.to_owned());
        let hex = s.strip_prefix("job_").ok_or_else(err)?;
        if hex.len() != 32 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(err());
        }
        u128::from_str_radix(hex, 16).map(JobId).map_err(|_| err())
    }
}

impl Serialize for JobId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for JobId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<JobId, D::Error> {
        let s = Cow::<'de, str>::deserialize(d)?;
        s.parse().map_err(de::Error::custom)
    }
}

impl JsonSchema for JobId {
    fn schema_name() -> Cow<'static, str> {
        "JobId".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": JOB_ID_PATTERN,
            "description": "A job id: `^job_[0-9a-f]{32}$`."
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::SeededGen;

    use super::*;

    #[test]
    fn display_parse_and_serde_round_trip() {
        let id = JobId::from_u128(0x0190_f5c2_a1b2_7c3d_8e4f_5a6b_7c8d_9e0f);
        let text = id.to_string();
        assert_eq!(text, "job_0190f5c2a1b27c3d8e4f5a6b7c8d9e0f");
        assert_eq!(text.parse::<JobId>(), Ok(id));
        assert_eq!(format!("{id:?}"), text);
        assert_eq!(serde_json::to_value(id).unwrap(), text);
        assert_eq!(
            serde_json::from_value::<JobId>(serde_json::json!(text)).unwrap(),
            id
        );
    }

    #[test]
    fn parsing_rejects_other_prefixes_and_shapes() {
        let hex = "0190f5c2a1b27c3d8e4f5a6b7c8d9e0f";
        for bad in [
            format!("med_{hex}"),
            format!("job{hex}"),
            format!("job_{}", hex.to_uppercase()),
            format!("job_{}", &hex[1..]),
            format!("job_{hex}0"),
            format!("job_+{}", &hex[1..]),
        ] {
            assert!(bad.parse::<JobId>().is_err(), "{bad}");
        }
        assert!(serde_json::from_value::<JobId>(serde_json::json!(42)).is_err());
    }

    #[test]
    fn seeded_ids_are_reproducible_and_ordered() {
        let mut a = SeededGen::new(7);
        let mut b = SeededGen::new(7);
        let ia: Vec<JobId> = (0..20).map(|_| JobId::mint(&mut a)).collect();
        let ib: Vec<JobId> = (0..20).map(|_| JobId::mint(&mut b)).collect();
        assert_eq!(ia, ib);
        assert!(ia.windows(2).all(|w| w[0] < w[1]));
    }
}
