// SPDX-License-Identifier: GPL-3.0-or-later
//! v0 → v1. Schema v0 is the pre-release draft format; no released Cutweave wrote it. This
//! migration exists to prove the framework end to end (WP-1A.03) and documents what v0 was:
//!
//! - a track's `items` was an array ordered by start; v1 keys items by ID;
//! - a media asset's file path was `path`; v1 has `path_abs` plus `path_rel` and
//!   `content_hash` (both `null` when unknown);
//! - there was no plugin data; v1 has a `custom` object on the project, media, sequences,
//!   tracks and items.

use serde_json::{Map, Value};

use super::{Migration, MigrationError, object_field};

const FROM: u32 = 0;

type JsonObject = Map<String, Value>;

/// The v0 → v1 migration.
pub(super) struct V0ToV1;

fn ensure_custom(entity: &mut Map<String, Value>) {
    entity
        .entry("custom")
        .or_insert_with(|| Value::Object(Map::new()));
}

fn each_object<'v>(
    map: &'v mut Map<String, Value>,
    path: &str,
) -> Result<Vec<(String, &'v mut JsonObject)>, MigrationError> {
    map.iter_mut()
        .map(|(key, value)| match value {
            Value::Object(entity) => Ok((key.clone(), entity)),
            _ => Err(MigrationError::step(
                FROM,
                format!("{path}/{key}"),
                "expected an object",
            )),
        })
        .collect()
}

fn migrate_media(media: &mut Map<String, Value>) -> Result<(), MigrationError> {
    for (key, asset) in each_object(media, "/media")? {
        if let Some(path) = asset.remove("path") {
            if asset.contains_key("path_abs") {
                return Err(MigrationError::step(
                    FROM,
                    format!("/media/{key}"),
                    "has both `path` and `path_abs`",
                ));
            }
            asset.insert("path_abs".into(), path);
        }
        asset.entry("path_abs").or_insert(Value::Null);
        asset.entry("path_rel").or_insert(Value::Null);
        asset.entry("content_hash").or_insert(Value::Null);
        ensure_custom(asset);
    }
    Ok(())
}

/// Turns `[{"id": "itm_a", …}, …]` into `{"itm_a": {…}, …}`.
fn key_items_by_id(items: &mut Value, path: &str) -> Result<(), MigrationError> {
    let Value::Array(list) = items else {
        return Err(MigrationError::step(
            FROM,
            path,
            "expected an array of items",
        ));
    };
    let mut keyed = Map::new();
    for (index, item) in std::mem::take(list).into_iter().enumerate() {
        let item_path = format!("{path}/{index}");
        let Value::Object(mut item) = item else {
            return Err(MigrationError::step(FROM, item_path, "expected an object"));
        };
        let Some(id) = item.get("id").and_then(Value::as_str).map(str::to_owned) else {
            return Err(MigrationError::step(
                FROM,
                item_path,
                "item has no string `id`",
            ));
        };
        ensure_custom(&mut item);
        if keyed.insert(id.clone(), Value::Object(item)).is_some() {
            return Err(MigrationError::step(
                FROM,
                item_path,
                format!("duplicate item id `{id}`"),
            ));
        }
    }
    *items = Value::Object(keyed);
    Ok(())
}

fn migrate_sequences(sequences: &mut Map<String, Value>) -> Result<(), MigrationError> {
    for (key, sequence) in each_object(sequences, "/sequences")? {
        let path = format!("/sequences/{key}");
        ensure_custom(sequence);
        let Some(tracks) = object_field(FROM, sequence, "tracks", &path)? else {
            continue;
        };
        for (track_key, track) in each_object(tracks, &format!("{path}/tracks"))? {
            let track_path = format!("{path}/tracks/{track_key}");
            ensure_custom(track);
            if let Some(items) = track.get_mut("items") {
                key_items_by_id(items, &format!("{track_path}/items"))?;
            }
        }
    }
    Ok(())
}

impl Migration for V0ToV1 {
    fn source_version(&self) -> u32 {
        FROM
    }

    fn description(&self) -> &'static str {
        "pre-release draft to v1: track items keyed by id, media `path` renamed to `path_abs`, plugin `custom` data added"
    }

    fn migrate(&self, doc: &mut Value, notes: &mut Vec<String>) -> Result<(), MigrationError> {
        let root = doc.as_object_mut().ok_or(MigrationError::NotAnObject)?;
        ensure_custom(root);
        if let Some(media) = object_field(FROM, root, "media", "")? {
            migrate_media(media)?;
        }
        if let Some(sequences) = object_field(FROM, root, "sequences", "")? {
            migrate_sequences(sequences)?;
        }
        notes.push("converted the pre-release draft format".into());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run(mut doc: Value) -> Result<Value, MigrationError> {
        V0ToV1.migrate(&mut doc, &mut Vec::new())?;
        Ok(doc)
    }

    #[test]
    fn keys_items_and_renames_paths() {
        let out = run(json!({
            "meta": {"schema_version": 0},
            "media": {"med_a": {"id": "med_a", "path": "/x.mp4"}},
            "sequences": {"seq_a": {"tracks": {"trk_a": {"items": [
                {"id": "itm_b", "start": 0},
                {"id": "itm_a", "start": 5}
            ]}}}}
        }))
        .unwrap();
        assert_eq!(out["media"]["med_a"]["path_abs"], "/x.mp4");
        assert!(out["media"]["med_a"].get("path").is_none());
        assert_eq!(out["media"]["med_a"]["path_rel"], Value::Null);
        let items = &out["sequences"]["seq_a"]["tracks"]["trk_a"]["items"];
        assert_eq!(items["itm_a"]["start"], 5);
        assert_eq!(items["itm_b"]["custom"], json!({}));
        assert_eq!(out["custom"], json!({}));
        assert_eq!(
            out["sequences"]["seq_a"]["tracks"]["trk_a"]["custom"],
            json!({})
        );
    }

    #[test]
    fn rejects_malformed_drafts_with_a_path() {
        let dup = run(json!({"sequences": {"s": {"tracks": {"t": {"items": [
            {"id": "itm_a"}, {"id": "itm_a"}
        ]}}}}}));
        assert!(
            matches!(dup, Err(MigrationError::Step { ref path, .. }) if path == "/sequences/s/tracks/t/items/1")
        );
        let no_id = run(json!({"sequences": {"s": {"tracks": {"t": {"items": [{}]}}}}}));
        assert!(no_id.is_err());
        let not_list = run(json!({"sequences": {"s": {"tracks": {"t": {"items": {}}}}}}));
        assert!(not_list.is_err());
        let both = run(json!({"media": {"m": {"path": "a", "path_abs": "b"}}}));
        assert!(both.is_err());
        let bad_media = run(json!({"media": {"m": 3}}));
        assert!(
            matches!(bad_media, Err(MigrationError::Step { ref path, .. }) if path == "/media/m")
        );
        assert!(run(json!([])).is_err());
        assert!(run(json!({"media": []})).is_err());
    }
}
