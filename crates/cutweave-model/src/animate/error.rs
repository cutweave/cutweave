// SPDX-License-Identifier: GPL-3.0-or-later
//! Errors of keyframe construction and key-list edits (keyframes §3.3, §9.1).

use std::fmt;

/// A keyframe invariant of keyframes §3.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyframeRule {
    /// KF-1: a keyed value has 1 to 100,000 keys.
    Kf1,
    /// KF-2: key times strictly increase.
    Kf2,
    /// KF-3: every `t` is an integer within ±(2⁵³ − 1) flicks.
    Kf3,
    /// KF-4: every value is of the property's type and within its stored range; numbers are
    /// finite.
    Kf4,
    /// KF-5: `interp` is one of the four kinds with exactly its fields, within their ranges.
    Kf5,
    /// KF-6: discrete properties (booleans, enums) use only `hold`.
    Kf6,
    /// KF-7: a `preset` hint matches `^[a-z][a-z0-9_]{0,63}$`.
    Kf7,
}

impl KeyframeRule {
    /// `"KF-1"` … `"KF-7"`.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            KeyframeRule::Kf1 => "KF-1",
            KeyframeRule::Kf2 => "KF-2",
            KeyframeRule::Kf3 => "KF-3",
            KeyframeRule::Kf4 => "KF-4",
            KeyframeRule::Kf5 => "KF-5",
            KeyframeRule::Kf6 => "KF-6",
            KeyframeRule::Kf7 => "KF-7",
        }
    }
}

impl fmt::Display for KeyframeRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Why a keyed value or a key-list edit was rejected. Commands (WP-2C.03) map
/// [`Invariant`](Self::Invariant) and [`Invalid`](Self::Invalid) to `Invalid{path, msg}` and
/// [`NotFound`](Self::NotFound) to `NotFound`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyframeError {
    /// A document invariant is broken. `path` is a JSON pointer into the animatable
    /// (`/keyframes/1/t`), empty for the value as a whole.
    #[error("{rule} at `{path}`: {msg}")]
    Invariant {
        /// The broken invariant.
        rule: KeyframeRule,
        /// Where, as a JSON pointer relative to the animatable.
        path: String,
        /// What is wrong.
        msg: String,
    },
    /// The property is not animated, or no key sits at an addressed time.
    #[error("{0}")]
    NotFound(String),
    /// A key would land on another key, or a time would leave ±(2⁵³ − 1) flicks.
    #[error("{0}")]
    Invalid(String),
}

impl KeyframeError {
    pub(crate) fn rule(
        rule: KeyframeRule,
        path: impl Into<String>,
        msg: impl Into<String>,
    ) -> Self {
        KeyframeError::Invariant {
            rule,
            path: path.into(),
            msg: msg.into(),
        }
    }

    /// The error's code in the test vectors: the invariant (`"KF-4"`), `"NotFound"` or
    /// `"Invalid"`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            KeyframeError::Invariant { rule, .. } => rule.code(),
            KeyframeError::NotFound(_) => "NotFound",
            KeyframeError::Invalid(_) => "Invalid",
        }
    }

    /// The same error with `prefix` prepended to its JSON pointer.
    #[must_use]
    pub(crate) fn under(self, prefix: &str) -> Self {
        match self {
            KeyframeError::Invariant { rule, path, msg } => KeyframeError::Invariant {
                rule,
                path: format!("{prefix}{path}"),
                msg,
            },
            other => other,
        }
    }
}
