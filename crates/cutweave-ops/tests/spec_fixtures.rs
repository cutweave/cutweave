// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.03 acceptance: the `edit` fixtures of `docs/spec/fixtures/` (timeline spec §9) run
//! against the real commands, table-driven (`cargo test -p cutweave-ops timeline::fixtures`).
//!
//! Each fixture builds its `before` document (25 fps, frames as given, names bound to IDs),
//! runs its command through the common harness (which also asserts undo/redo identity, document
//! validity and that a failure changes nothing), then compares the result with `after` (or
//! `error`), the stated `output` fields and the stated `report`. New entities (`+name`) are
//! bound by structure: tracks by display position, items by start order on their track.
//!
//! The commands of this work package are listed in [`COMMANDS`]; every fixture of theirs must
//! be in the table below, so a new fixture cannot be skipped silently. The trim, link and
//! replace fixtures belong to WP-2A.04, 2A.05 and 2A.11.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::H;
use cutweave_commands::CmdError;
use cutweave_model::{
    Clip, Duration, FrameRate, GroupId, Item, ItemId, MediaAsset, MediaId, MediaKind, Project,
    SampleRate, Sequence, SequenceFormat, SequenceId, Time, Track, TrackKind,
};
use cutweave_ops::media::FakeProbe;
use serde_json::{Value, json};

/// Flicks per frame at the fixtures' 25 fps.
const F: i64 = 28_224_000;

/// The commands whose fixtures this work package runs.
const COMMANDS: &[&str] = &[
    "timeline.insert",
    "timeline.overwrite",
    "item.move",
    "item.delete",
    "timeline.ripple_delete",
    "timeline.lift",
    "timeline.extract",
    "timeline.delete_gap",
    "clip.split",
    "item.duplicate",
    "track.set_magnetic",
];

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/spec/fixtures")
}

fn load(id: &str) -> Value {
    let path = fixtures_dir().join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

/// Names of the fixture bound to IDs (as JSON strings), and the reverse.
#[derive(Default)]
struct Names {
    ids: BTreeMap<String, String>,
    next: u128,
}

impl Names {
    fn mint<I: cutweave_model::EntityId>(&mut self, name: &str) -> I {
        if let Some(id) = self.ids.get(name) {
            return id.parse().ok().unwrap();
        }
        self.next += 1;
        let id = I::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + self.next);
        self.ids.insert(name.to_owned(), id.to_string());
        id
    }

    fn get(&self, name: &str) -> Option<&String> {
        self.ids.get(name)
    }

    /// Binds a new name (`+x`) to an actual ID, or checks an earlier binding.
    fn bind(&mut self, name: &str, actual: &str, what: &str) {
        if let Some(id) = self.ids.get(name) {
            assert_eq!(
                id, actual,
                "{what}: `{name}` is bound to {id}, got {actual}"
            );
            return;
        }
        assert!(name.starts_with('+'), "{what}: unknown name `{name}`");
        assert!(
            !self.ids.values().any(|v| v == actual),
            "{what}: new `{name}` is an existing entity {actual}"
        );
        self.ids.insert(name.to_owned(), actual.to_owned());
    }
}

fn frames(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

/// The `before` document of a fixture.
fn build(fx: &Value, names: &mut Names) -> Project {
    let mut doc = Project::new("Fixture");
    for (name, m) in fx["media"].as_object().unwrap() {
        let id: MediaId = names.mint(name);
        let path = format!("/fixture/{name}");
        let (kind, mut probe) = match m["kind"].as_str().unwrap() {
            "video" => (
                MediaKind::Video,
                FakeProbe::video(&path, 1, m["audio"].as_bool().unwrap_or(false)).probe,
            ),
            "audio" => (MediaKind::Audio, FakeProbe::audio(&path, 1).probe),
            _ => (MediaKind::Image, FakeProbe::image(&path).probe),
        };
        probe.duration = m
            .get("frames")
            .map(|n| Duration::from_flicks(frames(n) * F).unwrap());
        doc.media
            .insert(id, MediaAsset::file(id, kind, path, probe));
    }
    let sid: SequenceId = names.mint("S");
    let mut seq = Sequence::new(
        sid,
        "S",
        SequenceFormat::new(1920, 1080, FrameRate::FPS_25, SampleRate::HZ_48_000),
    );
    // Items first get IDs in document order, so parents can be named before they appear.
    for t in fx["before"].as_array().unwrap() {
        for i in t["items"].as_array().unwrap() {
            names.mint::<ItemId>(i["id"].as_str().unwrap());
        }
    }
    for (n, t) in fx["before"].as_array().unwrap().iter().enumerate() {
        let name = t["track"].as_str().unwrap();
        let kind = match t["kind"].as_str().unwrap() {
            "video" => TrackKind::Video,
            _ => TrackKind::Audio,
        };
        let mut track = Track::new(names.mint(name), kind, name);
        track.magnetic = t["magnetic"].as_bool().unwrap_or(false);
        track.locked = t["locked"].as_bool().unwrap_or(false);
        for i in t["items"].as_array().unwrap() {
            let media: MediaId = names.mint(i["media"].as_str().unwrap());
            let mut clip = Clip::new(
                names.mint(i["id"].as_str().unwrap()),
                media,
                Time::from_flicks(frames(&i["start"]) * F).unwrap(),
                Duration::from_flicks(frames(&i["dur"]) * F).unwrap(),
            );
            clip.source_in = Time::from_flicks(frames(&i["in"]) * F).unwrap();
            clip.linked_group = i["group"].as_str().map(|g| names.mint::<GroupId>(g));
            clip.attached_to = i["parent"].as_str().map(|p| names.mint::<ItemId>(p));
            clip.enabled = i["enabled"].as_bool().unwrap_or(true);
            track.items.insert(Item::Clip(clip));
        }
        seq.insert_track(n, track);
    }
    doc.sequences.insert(sid, seq);
    doc.validate()
        .unwrap_or_else(|e| panic!("{}: the before document is invalid: {e}", fx["id"]));
    doc
}

/// Fixture params with names replaced by IDs and times as given.
fn params(v: &Value, names: &Names) -> Value {
    match v {
        Value::String(s) => names
            .get(s)
            .map_or_else(|| v.clone(), |id| Value::String(id.clone())),
        Value::Array(a) => Value::Array(a.iter().map(|x| params(x, names)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), params(x, names)))
                .collect(),
        ),
        _ => v.clone(),
    }
}

/// An expected output value: names (bound ones too) by ID, `"f:N"` by flicks.
fn expected(v: &Value, names: &Names) -> Value {
    match v {
        Value::String(s) => {
            if let Some(n) = s.strip_prefix("f:") {
                json!(n.parse::<i64>().unwrap() * F)
            } else {
                names
                    .get(s)
                    .map_or_else(|| v.clone(), |id| Value::String(id.clone()))
            }
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| expected(x, names)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), expected(x, names)))
                .collect(),
        ),
        _ => v.clone(),
    }
}

/// Compares the document with `after`, binding new names.
fn compare_after(id: &str, fx: &Value, doc: &Project, names: &mut Names) {
    let sid: SequenceId = names.get("S").unwrap().parse().unwrap();
    let seq = &doc.sequences[&sid];
    let tracks: Vec<&Track> = seq.tracks_in_order().collect();
    let after = fx["after"].as_array().unwrap();
    assert_eq!(
        tracks.len(),
        after.len(),
        "{id}: tracks {:?}",
        tracks.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    for (t, want) in tracks.iter().zip(after) {
        let what = format!("{id}: track {}", want["track"]);
        names.bind(want["track"].as_str().unwrap(), &t.id.to_string(), &what);
        assert_eq!(
            t.magnetic,
            want["magnetic"].as_bool().unwrap_or(false),
            "{what}: magnetic"
        );
        assert_eq!(
            t.locked,
            want["locked"].as_bool().unwrap_or(false),
            "{what}: locked"
        );
        let items: Vec<&Item> = t.items.iter().collect();
        let wants = want["items"].as_array().unwrap();
        let shown = |items: &[&Item]| {
            items
                .iter()
                .map(|i| {
                    (
                        i.start().flicks() / F,
                        i.duration().flicks() / F,
                        i.id().to_string(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            items.len(),
            wants.len(),
            "{what}: items {:?}",
            shown(&items)
        );
        for (item, w) in items.iter().zip(wants) {
            let name = w["id"].as_str().unwrap();
            let what = format!("{what}, item {name}");
            names.bind(name, &item.id().to_string(), &what);
            let clip = item.as_clip().unwrap();
            assert_eq!(
                Some(&clip.media.to_string()),
                names.get(w["media"].as_str().unwrap()),
                "{what}: media"
            );
            assert_eq!(
                clip.start.flicks(),
                frames(&w["start"]) * F,
                "{what}: start"
            );
            assert_eq!(clip.duration.flicks(), frames(&w["dur"]) * F, "{what}: dur");
            assert_eq!(clip.source_in.flicks(), frames(&w["in"]) * F, "{what}: in");
            assert_eq!(
                clip.enabled,
                w["enabled"].as_bool().unwrap_or(true),
                "{what}: enabled"
            );
            match (w["group"].as_str(), clip.linked_group) {
                (None, None) => {}
                (Some(g), Some(actual)) => names.bind(g, &actual.to_string(), &what),
                (g, actual) => panic!("{what}: group {g:?}, got {actual:?}"),
            }
            match (w["parent"].as_str(), clip.attached_to) {
                (None, None) => {}
                (Some(p), Some(actual)) => names.bind(p, &actual.to_string(), &what),
                (p, actual) => panic!("{what}: parent {p:?}, got {actual:?}"),
            }
        }
    }
}

/// The fixture's report in the engine's shape (names by ID, frames by flicks).
fn expected_report(r: &Value, names: &Names) -> Value {
    let id = |v: &Value| -> Value {
        v.as_str().map_or(Value::Null, |s| {
            json!(names.get(s).unwrap_or_else(|| panic!("unbound `{s}`")))
        })
    };
    let list = |key: &str, f: &dyn Fn(&Value) -> Value| -> Vec<Value> {
        r[key]
            .as_array()
            .map_or_else(Vec::new, |a| a.iter().map(f).collect())
    };
    json!({
        "created": list("created", &|v| id(v)),
        "removed": list("removed", &|v| id(v)),
        "split": list("split", &|v| json!({"left": id(&v["left"]), "right": id(&v["right"]),
                                          "at": frames(&v["at"]) * F})),
        "moved": list("moved", &|v| json!({"item": id(&v["item"]), "track": id(&v["track"]),
                                          "start": frames(&v["start"]) * F})),
        "trimmed": list("trimmed", &|v| json!({"item": id(&v["item"]),
                                              "start": frames(&v["start"]) * F,
                                              "duration": frames(&v["dur"]) * F,
                                              "source_in": frames(&v["in"]) * F})),
        "relinked": list("relinked", &|v| json!({"item": id(&v["item"]),
                                                "linked_group": id(&v["group"]),
                                                "attached_to": id(&v["parent"])})),
        "replaced": list("replaced", &|v| json!({"item": id(&v["item"]), "media": id(&v["media"])})),
        "toggled": list("toggled", &|v| json!({"item": id(&v["item"]), "enabled": v["enabled"]})),
        "created_tracks": list("created_tracks", &|v| id(v)),
    })
}

/// Sorts every list but `created_tracks` (the fixture's lists are unordered).
fn normalized(mut report: Value) -> Value {
    if let Value::Object(map) = &mut report {
        for (k, v) in map.iter_mut() {
            if k != "created_tracks"
                && let Value::Array(a) = v
            {
                a.sort_by_key(ToString::to_string);
            }
        }
    }
    report
}

fn check_error(id: &str, fx: &Value, err: &CmdError, names: &Names) {
    let want = &fx["error"];
    assert_eq!(err.code(), want["code"].as_str().unwrap(), "{id}: {err}");
    if let Some(path) = want["path"].as_str() {
        match err {
            CmdError::Invalid { path: p, .. } => assert_eq!(p, path, "{id}: {err}"),
            other => panic!("{id}: expected Invalid at {path}, got {other}"),
        }
    }
    if let Some(name) = want["id"].as_str() {
        let expected = names.get(name).unwrap();
        match err {
            CmdError::Locked { id: actual, .. } | CmdError::NotFound { id: actual, .. } => {
                assert_eq!(actual, expected, "{id}: {err}");
            }
            other => panic!("{id}: expected an error naming {name}, got {other}"),
        }
    }
}

/// Runs one fixture.
fn run(id: &str) {
    let fx = load(id);
    assert_eq!(fx["id"], id);
    let mut names = Names::default();
    let doc = build(&fx, &mut names);
    let mut h = H::with_doc(doc.clone());
    let command = fx["command"].as_str().unwrap();
    let request = params(&fx["params"], &names);
    let result = h.run(command, request.clone());
    if fx.get("error").is_some() {
        let err = result.expect_err(&format!("{id}: expected an error"));
        check_error(id, &fx, &err, &names);
        assert_eq!(h.doc(), doc, "{id}: a failure changes nothing");
        // `validate` (a dry run) reports the same error.
        let dry = h
            .validate(command, request)
            .expect_err(&format!("{id}: validate accepted it"));
        assert_eq!(dry.code(), err.code(), "{id}: validate: {dry}");
        return;
    }
    let out = result.unwrap_or_else(|e| panic!("{id}: {e}"));
    compare_after(id, &fx, &h.doc(), &mut names);
    if let Some(output) = fx["output"].as_object() {
        for (key, want) in output {
            assert_eq!(
                out[key],
                expected(want, &names),
                "{id}: output `{key}` (got {out})"
            );
        }
    }
    if let Some(report) = fx.get("report") {
        assert_eq!(
            normalized(out["report"].clone()),
            normalized(expected_report(report, &names)),
            "{id}: report"
        );
    }
    // INV-21 for every fixture whose command reports: the report is the difference.
    if out.get("report").is_some_and(Value::is_object) {
        cutweave_ops::testing::check_edit_report(command, &out, &doc, &h.doc())
            .unwrap_or_else(|e| panic!("{id}: {e}"));
    }
}

macro_rules! fixtures {
    ($($test:ident => $file:literal,)*) => {
        /// The fixtures this file runs.
        const RUN: &[&str] = &[$($file),*];

        mod timeline {
            mod fixtures {
                $(
                    #[test]
                    fn $test() {
                        super::super::run($file);
                    }
                )*
            }
        }
    };
}

fixtures! {
    delete_free_leaves_gap => "delete-free-leaves-gap",
    delete_gap_after_end => "delete-gap-after-end",
    delete_gap_collision => "delete-gap-collision",
    delete_gap_free => "delete-gap-free",
    delete_gap_locked => "delete-gap-locked",
    delete_gap_magnetic_invalid => "delete-gap-magnetic-invalid",
    delete_locked_partner => "delete-locked-partner",
    delete_magnetic_closes_gap => "delete-magnetic-closes-gap",
    delete_magnetic_first => "delete-magnetic-first",
    delete_magnetic_locked_music_untouched => "delete-magnetic-locked-music-untouched",
    delete_magnetic_parent_detaches_children => "delete-magnetic-parent-detaches-children",
    duplicate_free_linked => "duplicate-free-linked",
    duplicate_free_new_track => "duplicate-free-new-track",
    duplicate_free_reject => "duplicate-free-reject",
    duplicate_locked => "duplicate-locked",
    duplicate_magnetic_inserts_after => "duplicate-magnetic-inserts-after",
    extract_all_tracks => "extract-all-tracks",
    extract_free_tracks_param => "extract-free-tracks-param",
    extract_locked => "extract-locked",
    extract_skips_unlisted_locked_track => "extract-skips-unlisted-locked-track",
    insert_free_insert_mode => "insert-free-insert-mode",
    insert_free_new_track => "insert-free-new-track",
    insert_free_reject_overlap => "insert-free-reject-overlap",
    insert_locked_partner_track => "insert-locked-partner-track",
    insert_locked_target => "insert-locked-target",
    insert_magnetic_at_cut => "insert-magnetic-at-cut",
    insert_magnetic_clamp_to_end => "insert-magnetic-clamp-to-end",
    insert_magnetic_empty_track => "insert-magnetic-empty-track",
    insert_magnetic_inside_splits => "insert-magnetic-inside-splits",
    lift_free_range => "lift-free-range",
    lift_locked => "lift-locked",
    lift_magnetic_closes => "lift-magnetic-closes",
    move_free_linked => "move-free-linked",
    move_free_new_track_partner => "move-free-new-track-partner",
    move_free_reject_overlap => "move-free-reject-overlap",
    move_into_magnetic_inserts => "move-into-magnetic-inserts",
    move_locked_partner => "move-locked-partner",
    move_magnetic_clamp_to_end => "move-magnetic-clamp-to-end",
    move_magnetic_reorder => "move-magnetic-reorder",
    move_negative_start => "move-negative-start",
    move_out_of_magnetic => "move-out-of-magnetic",
    move_parent_children_follow => "move-parent-children-follow",
    overwrite_free_splits_spanning => "overwrite-free-splits-spanning",
    overwrite_locked_partner => "overwrite-locked-partner",
    overwrite_magnetic_clamp_to_end => "overwrite-magnetic-clamp-to-end",
    overwrite_magnetic_span => "overwrite-magnetic-span",
    ripple_delete_all_tracks => "ripple-delete-all-tracks",
    ripple_delete_collision => "ripple-delete-collision",
    ripple_delete_free => "ripple-delete-free",
    ripple_delete_locked => "ripple-delete-locked",
    ripple_delete_magnetic => "ripple-delete-magnetic",
    set_magnetic_audio_invalid => "set-magnetic-audio-invalid",
    set_magnetic_close_gaps => "set-magnetic-close-gaps",
    set_magnetic_gaps_invalid => "set-magnetic-gaps-invalid",
    set_magnetic_locked_close_gaps => "set-magnetic-locked-close-gaps",
    set_magnetic_switches_main_track => "set-magnetic-switches-main-track",
    split_at_edge_invalid => "split-at-edge-invalid",
    split_locked_partner => "split-locked-partner",
    split_magnetic_linked_child => "split-magnetic-linked-child",
    split_still => "split-still",
    // `clip.trim` is WP-2A.04's; this one checks the magnetic ripple trim this WP routes
    // through the core (INV-203). `trim-magnetic-end-ripples` also needs `applied_delta`.
    trim_magnetic_start => "trim-magnetic-start",
}

#[test]
fn every_fixture_of_these_commands_runs() {
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(fixtures_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let fx: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        if fx["kind"] == "edit"
            && COMMANDS.contains(&fx["command"].as_str().unwrap_or(""))
            && !RUN.contains(&fx["id"].as_str().unwrap_or(""))
        {
            missing.push(fx["id"].to_string());
        }
    }
    assert!(missing.is_empty(), "fixtures not run: {missing:?}");
}
