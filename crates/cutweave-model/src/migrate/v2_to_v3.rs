// SPDX-License-Identifier: GPL-3.0-or-later
//! v2 → v3 (WP-2F.05c, ADR-0035): image sequences.
//!
//! v3 adds the media kind `image_sequence` and `MediaAsset.image_sequence`. A v2 document has
//! no sequences of pictures, so **nothing is rewritten**: the version number alone changes. It
//! exists so that a build that reads v2 refuses a document with an image sequence ("saved by a
//! newer version") instead of failing on a kind it does not know, or showing the run as its
//! first picture.
//!
//! One thing this step still does: v2 was one version for several work packages (#168), so a v2
//! file written by an early build can lack fields a later v2 build added. They are filled in
//! here, as opening such a file always did; a complete v2 document is untouched.

use serde_json::Value;

use super::{Migration, MigrationError, v1_to_v2};

/// The v2 → v3 migration.
pub(super) struct V2ToV3;

impl Migration for V2ToV3 {
    fn source_version(&self) -> u32 {
        2
    }

    fn description(&self) -> &'static str {
        "allows image sequences as media (nothing in an older project changes)"
    }

    fn migrate(&self, doc: &mut Value, _notes: &mut Vec<String>) -> Result<(), MigrationError> {
        v1_to_v2::fill_in_v2(doc)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use serde_json::json;

    use super::*;

    #[test]
    fn a_complete_v2_document_is_not_rewritten() {
        let doc = json!({"meta": {"schema_version": 2}, "folders": {},
            "media": {"m": {"kind": "image", "name": null, "folder": null}},
            "sequences": {}});
        let mut migrated = doc.clone();
        let mut notes = Vec::new();
        V2ToV3.migrate(&mut migrated, &mut notes).unwrap();
        assert_eq!(migrated, doc);
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn an_early_v2_document_gets_the_fields_later_v2_builds_added() {
        let mut doc = json!({"meta": {"schema_version": 2}, "media": {"m": {}}});
        V2ToV3.migrate(&mut doc, &mut Vec::new()).unwrap();
        assert_eq!(doc["folders"], json!({}));
        assert_eq!(doc["media"]["m"]["folder"], json!(null));
    }
}
