// SPDX-License-Identifier: GPL-3.0-or-later
//! Engine events of the autosave service (WP-1C.03): `project.dirty`, `project.saved`,
//! `project.recovery_available` (and `project.autosave_failed`).
//!
//! These are engine-level events, not document events ([`cutweave_commands::DocEvent`]): they
//! describe the project *file*, not the document. [`PROJECT_EVENTS`] lists them as
//! `(name, summary)` so the integrator can register them in the event catalog.

use cutweave_commands::{Rev, WallTime};
use serde::{Deserialize, Serialize};

use crate::journal::RecoveryAvailable;

/// `(name, summary)` of every event in this module, for the event catalog (WP-1N.01).
pub const PROJECT_EVENTS: &[(&str, &str)] = &[
    (
        "project.dirty",
        "The project gained or lost unsaved changes (last saved revision differs from the committed one)",
    ),
    (
        "project.saved",
        "The project document was written to disk (autosave or explicit save)",
    ),
    (
        "project.recovery_available",
        "Unsaved work from a previous session can be restored or discarded",
    ),
    (
        "project.autosave_failed",
        "An autosave attempt failed; it is retried automatically",
    ),
];

/// Payload of `project.dirty`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ProjectDirty {
    /// Whether the committed revision differs from the last saved one.
    pub dirty: bool,
}

/// Payload of `project.saved`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ProjectSaved {
    /// The committed revision that was written.
    pub rev: Rev,
    /// The project document path.
    pub path: String,
}

/// Payload of `project.recovery_available`; also what a recovery dialog shows (what a restore
/// would bring back).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct RecoveryPayload {
    /// Transactions a restore would replay.
    #[specta(type = specta_typescript::Number)]
    pub records: usize,
    /// The committed revision of the saved document.
    pub base_rev: Rev,
    /// The revision a restore reaches.
    pub last_rev: Rev,
    /// When the last of those transactions was committed.
    pub last_ts: WallTime,
    /// The journal ends with a damaged record (replay stops there).
    pub damaged: bool,
}

impl From<&RecoveryAvailable> for RecoveryPayload {
    fn from(r: &RecoveryAvailable) -> Self {
        RecoveryPayload {
            records: r.records,
            base_rev: r.base_rev,
            last_rev: r.last_rev,
            last_ts: r.last_ts,
            damaged: r.damaged,
        }
    }
}

/// Payload of `project.autosave_failed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct AutosaveFailed {
    /// What went wrong (paths and OS messages only; never secrets).
    pub message: String,
}

/// An engine event about the project file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "name", content = "data")]
pub enum ProjectEvent {
    /// `project.dirty`.
    #[serde(rename = "project.dirty")]
    Dirty(ProjectDirty),
    /// `project.saved`.
    #[serde(rename = "project.saved")]
    Saved(ProjectSaved),
    /// `project.recovery_available`.
    #[serde(rename = "project.recovery_available")]
    RecoveryAvailable(RecoveryPayload),
    /// `project.autosave_failed`.
    #[serde(rename = "project.autosave_failed")]
    AutosaveFailed(AutosaveFailed),
}

impl ProjectEvent {
    /// The event for unsaved work found on open ([`crate::project_io::OpenReport::recovery`]).
    #[must_use]
    pub fn recovery_available(r: &RecoveryAvailable) -> ProjectEvent {
        ProjectEvent::RecoveryAvailable(r.into())
    }

    /// The wire name, as listed in [`PROJECT_EVENTS`].
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            ProjectEvent::Dirty(_) => "project.dirty",
            ProjectEvent::Saved(_) => "project.saved",
            ProjectEvent::RecoveryAvailable(_) => "project.recovery_available",
            ProjectEvent::AutosaveFailed(_) => "project.autosave_failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shape_and_catalog_agree() {
        let events = [
            ProjectEvent::Dirty(ProjectDirty { dirty: true }),
            ProjectEvent::Saved(ProjectSaved {
                rev: Rev(3),
                path: "a.cutweave".into(),
            }),
            ProjectEvent::AutosaveFailed(AutosaveFailed {
                message: "x".into(),
            }),
        ];
        for e in &events {
            let v = serde_json::to_value(e).unwrap_or_default();
            assert_eq!(v["name"], e.name());
            assert!(PROJECT_EVENTS.iter().any(|(n, _)| *n == e.name()));
        }
        let v = serde_json::to_value(&events[0]).unwrap_or_default();
        assert_eq!(v["data"]["dirty"], true);
        assert_eq!(PROJECT_EVENTS.len(), 4);
    }
}
