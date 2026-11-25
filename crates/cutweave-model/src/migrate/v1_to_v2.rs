// SPDX-License-Identifier: GPL-3.0-or-later
//! v1 → v2 (WP-2A.02, timeline spec §1.5, ADR-0023; WP-2F.07 joins this step, ADR-0020).
//!
//! **One v2.** Nothing was released between the timeline epic (2A) and the media folders
//! (2F.07), so both change the format in this single migration. A work package that needs
//! another v2 field adds a function to [`STEPS`]; it never adds a v3. The steps run in order on
//! the raw JSON, and every step is written so that it can run on a document of any v1 shape.
//!
//! What this migration does today:
//!
//! 1. **Timeline fields** (`add_timeline_fields`): `Sequence.markers` (`{}`),
//!    `Sequence.in_out` (`null`), `Track.height` (`null`), `Item.attached_to` (`null`) and
//!    `Item.markers` (`{}`).
//! 2. **Frame grid** (`floor_durations`): every clip duration that is not a whole number of
//!    frames of its sequence is floored to whole frames, at least one (INV-3, INV-25).
//! 3. **Aligned groups** (`dissolve_misaligned_groups`): a linked group with fewer than two
//!    members, or whose members differ in `start` or `duration`, is dissolved (INV-8).
//! 4. **One main track** (`normalize_magnetic`): of several magnetic tracks only the first in
//!    display order keeps the flag (INV-6). The flag is also cleared on a track that is not a
//!    video track or not gap-free from 0 (INV-6, INV-7); P1 never sets it, so a real document
//!    only loses it when it was hand-written.
//!
//! Adding a step for another work package: write `fn(&mut Value, &mut Vec<String>) ->
//! Result<(), MigrationError>`, append it to [`STEPS`], extend `tests/fixtures/v2/` through
//! the bless command in `tests/migrations.rs`, and describe the step in the list above.

use serde_json::{Map, Value};

use super::{Migration, MigrationError, object_field};

const FROM: u32 = 1;
const FLICKS_PER_SECOND: i128 = 705_600_000;

type JsonObject = Map<String, Value>;
type Step = fn(&mut JsonObject, &mut Vec<String>) -> Result<(), MigrationError>;

/// The steps of the v1 → v2 migration, in order. Append only.
const STEPS: &[Step] = &[
    add_timeline_fields,
    floor_durations,
    dissolve_misaligned_groups,
    normalize_magnetic,
];

/// The v1 → v2 migration.
pub(super) struct V1ToV2;

impl Migration for V1ToV2 {
    fn source_version(&self) -> u32 {
        FROM
    }

    fn description(&self) -> &'static str {
        "adds markers, in/out range, track heights and attachments; floors clip durations to whole \
         frames, dissolves misaligned linked groups and keeps one magnetic track"
    }

    fn migrate(&self, doc: &mut Value, notes: &mut Vec<String>) -> Result<(), MigrationError> {
        let Value::Object(root) = doc else {
            return Err(MigrationError::step(FROM, "", "expected an object"));
        };
        for step in STEPS {
            step(root, notes)?;
        }
        Ok(())
    }
}

/// Every sequence of the document with its JSON Pointer (`/sequences/<id>`).
fn sequences(root: &mut JsonObject) -> Result<Vec<(String, &mut JsonObject)>, MigrationError> {
    let Some(map) = object_field(FROM, root, "sequences", "")? else {
        return Ok(Vec::new());
    };
    map.iter_mut()
        .map(|(key, value)| match value {
            Value::Object(seq) => Ok((format!("/sequences/{key}"), seq)),
            _ => Err(MigrationError::step(
                FROM,
                format!("/sequences/{key}"),
                "expected an object",
            )),
        })
        .collect()
}

/// Every track of a sequence with its JSON Pointer.
fn tracks<'v>(
    seq: &'v mut JsonObject,
    path: &str,
) -> Result<Vec<(String, &'v mut JsonObject)>, MigrationError> {
    let Some(map) = object_field(FROM, seq, "tracks", path)? else {
        return Ok(Vec::new());
    };
    map.iter_mut()
        .map(|(key, value)| match value {
            Value::Object(track) => Ok((format!("{path}/tracks/{key}"), track)),
            _ => Err(MigrationError::step(
                FROM,
                format!("{path}/tracks/{key}"),
                "expected an object",
            )),
        })
        .collect()
}

/// Every item of a track with its JSON Pointer.
fn items<'v>(
    track: &'v mut JsonObject,
    path: &str,
) -> Result<Vec<(String, &'v mut JsonObject)>, MigrationError> {
    let Some(map) = object_field(FROM, track, "items", path)? else {
        return Ok(Vec::new());
    };
    map.iter_mut()
        .map(|(key, value)| match value {
            Value::Object(item) => Ok((format!("{path}/items/{key}"), item)),
            _ => Err(MigrationError::step(
                FROM,
                format!("{path}/items/{key}"),
                "expected an object",
            )),
        })
        .collect()
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

fn add_timeline_fields(
    root: &mut JsonObject,
    _notes: &mut Vec<String>,
) -> Result<(), MigrationError> {
    for (path, seq) in sequences(root)? {
        seq.entry("markers").or_insert_with(empty_object);
        seq.entry("in_out").or_insert(Value::Null);
        for (track_path, track) in tracks(seq, &path)? {
            track.entry("height").or_insert(Value::Null);
            for (_, item) in items(track, &track_path)? {
                item.entry("attached_to").or_insert(Value::Null);
                item.entry("markers").or_insert_with(empty_object);
            }
        }
    }
    Ok(())
}

/// Flicks per frame of a sequence's `format.frame_rate`, if the rate is exact.
fn flicks_per_frame(seq: &JsonObject) -> Option<i128> {
    let rate = seq.get("format")?.get("frame_rate")?;
    let num = i128::from(rate.get("num")?.as_u64()?);
    let den = i128::from(rate.get("den")?.as_u64()?);
    let total = FLICKS_PER_SECOND * den;
    (num > 0 && total % num == 0).then_some(total / num)
}

fn floor_durations(root: &mut JsonObject, notes: &mut Vec<String>) -> Result<(), MigrationError> {
    let mut floored = 0_usize;
    for (path, seq) in sequences(root)? {
        let Some(frame) = flicks_per_frame(seq) else {
            continue;
        };
        for (track_path, track) in tracks(seq, &path)? {
            for (item_path, item) in items(track, &track_path)? {
                let Some(duration) = item.get("duration").and_then(Value::as_i64) else {
                    continue;
                };
                let d = i128::from(duration);
                if d % frame != 0 {
                    let new = (d - d % frame).max(frame);
                    let new = i64::try_from(new).map_err(|_| {
                        MigrationError::step(FROM, format!("{item_path}/duration"), "too large")
                    })?;
                    item.insert("duration".into(), Value::from(new));
                    floored += 1;
                }
            }
        }
    }
    if floored > 0 {
        notes.push(format!(
            "floored {floored} clip duration(s) to a whole number of frames"
        ));
    }
    Ok(())
}

/// A linked item's place: where it is, to unlink it later.
struct Member {
    sequence: String,
    track: String,
    item: String,
    start: Option<i64>,
    duration: Option<i64>,
}

fn dissolve_misaligned_groups(
    root: &mut JsonObject,
    notes: &mut Vec<String>,
) -> Result<(), MigrationError> {
    let mut groups: std::collections::BTreeMap<String, Vec<Member>> =
        std::collections::BTreeMap::new();
    for (path, seq) in sequences(root)? {
        for (track_path, track) in tracks(seq, &path)? {
            for (item_path, item) in items(track, &track_path)? {
                if let Some(group) = item.get("linked_group").and_then(Value::as_str) {
                    let split = |p: &str| p.rsplit('/').next().unwrap_or_default().to_owned();
                    groups.entry(group.to_owned()).or_default().push(Member {
                        sequence: path.clone(),
                        track: split(&track_path),
                        item: split(&item_path),
                        start: item.get("start").and_then(Value::as_i64),
                        duration: item.get("duration").and_then(Value::as_i64),
                    });
                }
            }
        }
    }
    let mut dissolved = 0_usize;
    for members in groups.values() {
        let aligned = members.len() >= 2
            && members
                .iter()
                .all(|m| m.start == members[0].start && m.duration == members[0].duration);
        if aligned {
            continue;
        }
        dissolved += 1;
        for m in members {
            let item = root
                .get_mut("sequences")
                .and_then(|s| s.get_mut(m.sequence.rsplit('/').next().unwrap_or_default()))
                .and_then(|s| s.get_mut("tracks"))
                .and_then(|t| t.get_mut(&m.track))
                .and_then(|t| t.get_mut("items"))
                .and_then(|i| i.get_mut(&m.item))
                .and_then(Value::as_object_mut);
            if let Some(item) = item {
                item.insert("linked_group".into(), Value::Null);
            }
        }
    }
    if dissolved > 0 {
        notes.push(format!(
            "dissolved {dissolved} linked group(s) that were not aligned"
        ));
    }
    Ok(())
}

/// The track IDs of a sequence in display order (`track_order`, then any track it forgot).
fn display_order(seq: &JsonObject) -> Vec<String> {
    let mut order: Vec<String> = seq
        .get("track_order")
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        });
    if let Some(tracks) = seq.get("tracks").and_then(Value::as_object) {
        for key in tracks.keys() {
            if !order.contains(key) {
                order.push(key.clone());
            }
        }
    }
    order
}

/// Whether a track starts at 0 and has no gap (an empty track qualifies).
fn gap_free(track: &JsonObject) -> bool {
    let Some(items) = track.get("items").and_then(Value::as_object) else {
        return true;
    };
    let mut spans: Vec<(i64, i64)> = items
        .values()
        .filter_map(|i| Some((i.get("start")?.as_i64()?, i.get("duration")?.as_i64()?)))
        .collect();
    spans.sort_unstable();
    let mut expected = 0_i64;
    for (start, duration) in spans {
        if start != expected {
            return false;
        }
        expected = start.saturating_add(duration);
    }
    true
}

fn normalize_magnetic(
    root: &mut JsonObject,
    notes: &mut Vec<String>,
) -> Result<(), MigrationError> {
    let (mut extra, mut cleared) = (0_usize, 0_usize);
    for (_, seq) in sequences(root)? {
        let order = display_order(seq);
        let mut kept = false;
        for id in order {
            let Some(track) = seq
                .get_mut("tracks")
                .and_then(|t| t.get_mut(&id))
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            if track.get("magnetic").and_then(Value::as_bool) != Some(true) {
                continue;
            }
            if kept {
                track.insert("magnetic".into(), Value::Bool(false));
                extra += 1;
            } else if track.get("kind").and_then(Value::as_str) != Some("video") || !gap_free(track)
            {
                track.insert("magnetic".into(), Value::Bool(false));
                cleared += 1;
                kept = true;
            } else {
                kept = true;
            }
        }
    }
    if extra > 0 {
        notes.push(format!(
            "kept the first magnetic track of a sequence and cleared {extra} other flag(s)"
        ));
    }
    if cleared > 0 {
        notes.push(format!(
            "cleared {cleared} magnetic flag(s) on tracks that are not gap-free video tracks"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use serde_json::json;

    use super::*;

    fn run(doc: Value) -> (Value, Vec<String>) {
        let mut doc = doc;
        let mut notes = Vec::new();
        V1ToV2.migrate(&mut doc, &mut notes).unwrap();
        (doc, notes)
    }

    #[allow(clippy::needless_pass_by_value)] // call sites pass `json!` temporaries
    fn seq(tracks: Value, order: Value) -> Value {
        json!({"sequences": {"seq_1": {
            "format": {"frame_rate": {"num": 30, "den": 1}},
            "tracks": tracks, "track_order": order
        }}})
    }

    fn item(start: i64, duration: i64) -> Value {
        json!({"start": start, "duration": duration, "linked_group": null})
    }

    const FRAME: i64 = 23_520_000;

    #[test]
    fn a_magnetic_flag_on_an_audio_track_or_a_track_with_a_gap_is_cleared() {
        let doc = seq(
            json!({
                "a": {"kind": "audio", "magnetic": true, "items": {}},
                "v": {"kind": "video", "magnetic": true, "items": {"i": item(FRAME, FRAME)}},
            }),
            json!(["a", "v"]),
        );
        let (out, notes) = run(doc);
        let tracks = &out["sequences"]["seq_1"]["tracks"];
        assert_eq!(tracks["a"]["magnetic"], false);
        // The first flag was the audio one; the later video flag counts as the extra.
        assert_eq!(tracks["v"]["magnetic"], false);
        assert_eq!(notes.len(), 2, "{notes:?}");

        let doc = seq(
            json!({"v": {"kind": "video", "magnetic": true, "items": {"i": item(FRAME, FRAME)}}}),
            json!(["v"]),
        );
        let (out, notes) = run(doc);
        assert_eq!(out["sequences"]["seq_1"]["tracks"]["v"]["magnetic"], false);
        assert_eq!(
            notes,
            ["cleared 1 magnetic flag(s) on tracks that are not gap-free video tracks"]
        );
    }

    #[test]
    fn a_gap_free_video_track_keeps_its_flag_and_empty_ones_too() {
        let doc = seq(
            json!({
                "v": {"kind": "video", "magnetic": true, "items": {
                    "i": item(0, FRAME), "j": item(FRAME, 2 * FRAME)}},
                "w": {"kind": "video", "magnetic": false, "items": {}},
            }),
            json!(["v", "w"]),
        );
        let (out, notes) = run(doc);
        assert_eq!(out["sequences"]["seq_1"]["tracks"]["v"]["magnetic"], true);
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn durations_floor_to_whole_frames_but_never_below_one() {
        let doc = seq(
            json!({"v": {"kind": "video", "items": {
                "i": item(0, 3 * FRAME + 5),
                "j": item(10 * FRAME, FRAME - 1),
                "k": item(20 * FRAME, FRAME),
            }}}),
            json!(["v"]),
        );
        let (out, notes) = run(doc);
        let items = &out["sequences"]["seq_1"]["tracks"]["v"]["items"];
        assert_eq!(items["i"]["duration"], 3 * FRAME);
        assert_eq!(items["j"]["duration"], FRAME);
        assert_eq!(items["k"]["duration"], FRAME);
        assert_eq!(items["i"]["attached_to"], json!(null));
        assert_eq!(items["i"]["markers"], json!({}));
        assert_eq!(
            notes,
            ["floored 2 clip duration(s) to a whole number of frames"]
        );
    }

    #[test]
    fn existing_v2_fields_are_never_overwritten() {
        let mut doc = seq(
            json!({"v": {"kind": "video", "items": {"i": item(0, FRAME)}}}),
            json!(["v"]),
        );
        doc["sequences"]["seq_1"]["markers"] = json!({"mrk": {"t": 1}});
        doc["sequences"]["seq_1"]["tracks"]["v"]["height"] = json!(40);
        let (out, _) = run(doc);
        assert_eq!(out["sequences"]["seq_1"]["markers"]["mrk"]["t"], 1);
        assert_eq!(out["sequences"]["seq_1"]["tracks"]["v"]["height"], 40);
    }
}
