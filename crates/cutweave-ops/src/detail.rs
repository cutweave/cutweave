// SPDX-License-Identifier: GPL-3.0-or-later
//! The `detail` of the read queries (`timeline.get`, `media.list`; WP-2I.08, ADR-0040
//! question 5): `full` is the whole result and the default, `concise` leaves out what a caller
//! can ask for again, so a long timeline or a big bin costs an agent fewer tokens.
//!
//! A field that concise may leave out is an `Option` that is skipped when `None`: a full
//! result always carries it (also when its value is `null`), so a call without `detail`
//! returns exactly the bytes it returned before the parameter existed, and the output schema
//! describes both forms (ADR-0028).

use cutweave_commands::wire;
use serde::{Deserialize, Deserializer};

/// How much a read query returns for each entry: `full` (default) or `concise`.
// Variants documented above: a plain string `enum` keeps the MCP core tools small (R33).
#[allow(missing_docs)]
#[wire]
#[derive(Copy, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    Concise,
    #[default]
    Full,
}

impl Detail {
    /// `value` in a full result; `None` (left out) in a concise one.
    pub(crate) fn full_only<T>(self, value: T) -> Option<T> {
        (self == Detail::Full).then_some(value)
    }

    /// `value`, except in a concise result when it is `default`: then `None` (left out).
    pub(crate) fn unless_default<T: PartialEq>(self, value: T, default: &T) -> Option<T> {
        (self == Detail::Full || value != *default).then_some(value)
    }
}

/// Deserializes a field that is present, `null` included, as `Some`: with `#[serde(default)]`
/// a field that concise left out is `None` and a full result's `null` stays `Some(None)`, so a
/// result read back serializes to the same bytes.
pub(crate) fn present<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_keeps_everything_and_concise_drops_defaults() {
        assert_eq!(Detail::default(), Detail::Full);
        assert_eq!(Detail::Full.full_only(1), Some(1));
        assert_eq!(Detail::Concise.full_only(1), None);
        assert_eq!(Detail::Full.unless_default(false, &false), Some(false));
        assert_eq!(Detail::Concise.unless_default(false, &false), None);
        assert_eq!(Detail::Concise.unless_default(true, &false), Some(true));
        // A nullable field: `null` is its default.
        assert_eq!(Detail::Full.unless_default(None::<u32>, &None), Some(None));
        assert_eq!(Detail::Concise.unless_default(None::<u32>, &None), None);
        assert_eq!(
            Detail::Concise.unless_default(Some(96), &None),
            Some(Some(96))
        );
    }
}
