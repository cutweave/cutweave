// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1A.02 acceptance: entity round trips, structural sharing, stable JSON and the invariants
//! of arch §4.3 (one hand-crafted corrupt fixture per `Invariant` variant).

// `allow-unwrap-in-tests` covers `#[test]` bodies only, not the helpers below.
#![allow(clippy::unwrap_used)]

use std::{fs, path::PathBuf, time::Instant};

use cutweave_model::{
    Clip, Duration, FrameRate, IdGen, Invariant, Item, MediaAsset, MediaKind, ProbeInfo, Project,
    SampleRate, SeededGen, Sequence, SequenceFormat, Time, Track, TrackKind,
};
use serde_json::{Value, json};

fn fixture(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path);
    fs::read_to_string(&full).unwrap_or_else(|e| panic!("reading {}: {e}", full.display()))
}

fn load(path: &str) -> Project {
    serde_json::from_str(&fixture(path)).unwrap_or_else(|e| panic!("parsing {path}: {e}"))
}

#[test]
fn default_project_round_trips() {
    let project = Project::default();
    let json = serde_json::to_string(&project).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back, project);
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
    assert_eq!(project.validate(), Ok(()));
}

/// Builds one sequence with `tracks` video tracks of `per_track` back-to-back clips.
fn big_project(tracks: usize, per_track: usize) -> Project {
    let mut ids = SeededGen::new(7);
    let mut project = Project::new("Big");
    let probe = ProbeInfo {
        duration: Some(Duration::from_secs(3600).unwrap()),
        ..ProbeInfo::default()
    };
    let media = MediaAsset::file(ids.mint(), MediaKind::Video, "/media/long.mp4", probe);
    let frame = FrameRate::FPS_30.flicks_per_frame().unwrap();
    let mut sequence = Sequence::new(ids.mint(), "Main", SequenceFormat::default());
    for t in 0..tracks {
        let mut track = Track::new(ids.mint(), TrackKind::Video, format!("V{t}"));
        for n in 0..per_track {
            let start = Time::from_flicks(i64::try_from(n).unwrap() * frame).unwrap();
            let length = Duration::from_flicks(frame).unwrap();
            track
                .items
                .insert(Clip::new(ids.mint(), media.id, start, length).into());
        }
        sequence.insert_track(t, track);
    }
    project.media.insert(media.id, media);
    project.sequences.insert(sequence.id, sequence);
    project
}

#[test]
fn clones_share_structure_and_are_cheap() {
    let project = big_project(10, 1000);
    assert_eq!(project.item_count(), 10_000);
    assert_eq!(project.validate(), Ok(()));
    let started = Instant::now();
    let a = project.clone();
    let b = project.clone();
    let elapsed = started.elapsed();
    // Budget 100 µs per clone; the coarse CI threshold is 5 ms (WP-1A.02).
    assert!(elapsed.as_millis() < 5, "two clones took {elapsed:?}");
    assert!(a.media.ptr_eq(&b.media));
    assert!(a.sequences.ptr_eq(&b.sequences));
    let (sa, sb) = (
        a.sequences.values().next().unwrap(),
        b.sequences.values().next().unwrap(),
    );
    for (ta, tb) in sa.tracks.values().zip(sb.tracks.values()) {
        assert!(ta.items.ptr_eq(&tb.items));
    }
}

#[test]
fn a_mutated_clone_leaves_the_original_untouched() {
    let original = big_project(2, 10);
    let mut edited = original.clone();
    let sequence_id = *edited.sequences.keys().next().unwrap();
    let sequence = edited.sequences.get_mut(&sequence_id).unwrap();
    let track_id = sequence.track_order[0];
    let track = sequence.track_mut(&track_id).unwrap();
    let first = track.items.iter().next().unwrap().id();
    track.items.update(&first, |i| i.set_enabled(false));
    assert_ne!(edited, original);
    let (_, item) = original
        .sequences
        .values()
        .next()
        .unwrap()
        .find_item(&first)
        .unwrap();
    assert!(item.enabled());
    assert!(
        original.media.ptr_eq(&edited.media),
        "untouched maps stay shared"
    );
}

#[test]
fn json_layout_is_stable_and_sorted() {
    let project = big_project(2, 3);
    let first = serde_json::to_string_pretty(&project).unwrap();
    let reparsed: Project = serde_json::from_str(&first).unwrap();
    assert_eq!(serde_json::to_string_pretty(&reparsed).unwrap(), first);
    // Maps are written in key order.
    let value: Value = serde_json::from_str(&first).unwrap();
    let sequence = value["sequences"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    for track in sequence["tracks"].as_object().unwrap().values() {
        let keys: Vec<&str> = track["items"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted);
    }
    // Top-level field order is the declaration order.
    let top: Vec<&str> = first
        .lines()
        .filter(|l| l.starts_with("  \"") && !l.starts_with("   "))
        .map(|l| l.trim_start().split('"').nth(1).unwrap())
        .collect();
    assert_eq!(top, ["meta", "settings", "media", "sequences", "custom"]);
}

#[test]
fn unknown_fields_survive_at_every_level() {
    let mut value: Value = serde_json::from_str(&fixture("invalid/valid-base.cutweave")).unwrap();
    let seq = "seq_0000000000017000800000000000b001";
    let track = "trk_0000000000017000800000000000c001";
    let item = "itm_0000000000017000800000000000d001";
    let media = "med_0000000000017000800000000000a001";
    value["future_root"] = json!({"z": 1, "a": 2});
    value["meta"]["future_meta"] = json!(true);
    value["settings"]["future_setting"] = json!([1, 2]);
    value["settings"]["default_format"]["future_format"] = json!("x");
    value["media"][media]["future_media"] = json!(1);
    value["media"][media]["probe"]["future_probe"] = json!(2);
    value["sequences"][seq]["future_sequence"] = json!(3);
    value["sequences"][seq]["tracks"][track]["future_track"] = json!(4);
    let clip = &mut value["sequences"][seq]["tracks"][track]["items"][item];
    clip["future_clip"] = json!(5);
    clip["audio"]["future_audio"] = json!(6);
    clip["visual"]["future_visual"] = json!(7);
    clip["visual"]["transform"]["future_transform"] = json!(8);
    let project: Project = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        project.extra.get("future_root"),
        Some(&json!({"a": 2, "z": 1}))
    );
    assert_eq!(serde_json::to_value(&project).unwrap(), value);
    assert_eq!(project.validate(), Ok(()));
}

#[test]
fn the_base_of_the_corrupt_fixtures_is_valid() {
    let project = load("invalid/valid-base.cutweave");
    assert_eq!(project.violations(), Vec::<Invariant>::new());
    assert_eq!(project.item_count(), 2);
}

/// Each corrupt fixture violates exactly one invariant, of the expected variant.
#[test]
fn validate_rejects_each_corrupt_fixture_with_the_expected_variant() {
    type Check = fn(&Invariant) -> bool;
    let cases: [(&str, Check); 13] = [
        ("overlap", |v| matches!(v, Invariant::Overlap { .. })),
        ("dangling-reference", |v| {
            matches!(v, Invariant::DanglingReference { .. })
        }),
        ("source-out-of-bounds", |v| {
            matches!(v, Invariant::SourceOutOfBounds { .. })
        }),
        ("invalid-format", |v| {
            matches!(v, Invariant::InvalidFormat { .. })
        }),
        ("compound-cycle", |v| {
            matches!(v, Invariant::CompoundCycle { .. })
        }),
        ("key-mismatch", |v| {
            matches!(v, Invariant::KeyMismatch { .. })
        }),
        ("track-order", |v| matches!(v, Invariant::TrackOrder { .. })),
        ("incompatible-media", |v| {
            matches!(v, Invariant::IncompatibleMedia { .. })
        }),
        ("invalid-timing", |v| {
            matches!(v, Invariant::InvalidTiming { .. })
        }),
        ("invalid-value", |v| {
            matches!(v, Invariant::InvalidValue { .. })
        }),
        ("custom-namespace", |v| {
            matches!(v, Invariant::CustomNamespace { .. })
        }),
        ("linked-group-spans-sequences", |v| {
            matches!(v, Invariant::LinkedGroupSpansSequences { .. })
        }),
        ("invalid-media", |v| {
            matches!(v, Invariant::InvalidMedia { .. })
        }),
    ];
    for (name, expected) in cases {
        let project = load(&format!("invalid/{name}.cutweave"));
        let first = project.validate().expect_err(name);
        assert!(expected(&first), "{name}: got {first:?}");
        let all = project.violations();
        assert_eq!(all.len(), 1, "{name}: {all:#?}");
        assert!(first.to_string().len() > 10, "{first}");
    }
}

#[test]
fn violation_details_point_at_the_problem() {
    let project = load("invalid/compound-cycle.cutweave");
    let Err(Invariant::CompoundCycle { cycle }) = project.validate() else {
        panic!("expected a cycle");
    };
    assert_eq!(cycle.len(), 3);
    assert_eq!(cycle.first(), cycle.last());
    let message = Invariant::CompoundCycle { cycle }.to_string();
    assert!(message.contains(" -> "), "{message}");

    let project = load("invalid/dangling-reference.cutweave");
    let Err(Invariant::DanglingReference { path, id, .. }) = project.validate() else {
        panic!("expected a dangling reference");
    };
    assert!(
        path.ends_with("/items/itm_0000000000017000800000000000d002/media"),
        "{path}"
    );
    assert_eq!(id, "med_0000000000017000800000000000a009");

    let value = serde_json::to_value(project.validate().unwrap_err()).unwrap();
    assert_eq!(value["invariant"], "dangling_reference");
    assert_eq!(value["kind"], "media");
}

#[test]
fn self_referencing_compound_is_a_cycle() {
    let mut ids = SeededGen::new(3);
    let mut project = Project::new("Self");
    let mut sequence = Sequence::new(ids.mint(), "Loop", SequenceFormat::default());
    let media = MediaAsset::sequence_ref(ids.mint(), sequence.id);
    let mut track = Track::new(ids.mint(), TrackKind::Video, "V1");
    track.items.insert(
        Clip::new(
            ids.mint(),
            media.id,
            Time::ZERO,
            Duration::from_secs(1).unwrap(),
        )
        .into(),
    );
    sequence.insert_track(0, track);
    project.media.insert(media.id, media);
    project.sequences.insert(sequence.id, sequence);
    let Err(Invariant::CompoundCycle { cycle }) = project.validate() else {
        panic!("expected a cycle");
    };
    assert_eq!(cycle.len(), 2);
}

#[test]
fn sequence_formats_outside_the_supported_range_are_rejected() {
    let mut project = Project::default();
    for (rate, hz, ok) in [
        (FrameRate::FPS_29_97, 48_000, true),
        (FrameRate::new(1000, 1).unwrap(), 96_000, true),
        (FrameRate::new(1001, 1).unwrap(), 48_000, false), // above 1000 fps
        (FrameRate::new(1, 2).unwrap(), 48_000, false),    // below 1 fps
        (FrameRate::new(11, 1).unwrap(), 48_000, false),   // not exact in flicks
        (FrameRate::FPS_30, 4_000, false),                 // sample rate too low
        (FrameRate::FPS_30, 22_050, true),
    ] {
        project.settings.default_format.frame_rate = rate;
        project.settings.default_format.sample_rate = SampleRate::new(hz).unwrap();
        assert_eq!(project.validate().is_ok(), ok, "{rate} @ {hz}");
    }
}

#[test]
fn json_schema_and_items_are_generated() {
    let schema = schemars::schema_for!(Project);
    let text = serde_json::to_string(&schema).unwrap();
    for needle in [
        "\"meta\"",
        "\"sequences\"",
        "\"track_order\"",
        "\"source_in\"",
    ] {
        assert!(text.contains(needle), "{needle}");
    }
    assert!(
        !text.contains("\"extra\""),
        "unknown-field storage is not part of the schema"
    );
    let item: Item = Clip::new(
        cutweave_model::ItemId::from_u128(1),
        cutweave_model::MediaId::from_u128(2),
        Time::ZERO,
        Duration::from_flicks(1).unwrap(),
    )
    .into();
    assert_eq!(serde_json::to_value(&item).unwrap()["kind"], "clip");
}

#[test]
fn seeded_id_high_water_finds_the_streams_own_ids_only() {
    // `big_project(2, 3)` mints: media, sequence, then per track the track and its items.
    let project = big_project(2, 3);
    assert_eq!(project.seeded_id_high_water(7), 2 + 2 + 2 * 3);
    assert_eq!(project.seeded_id_high_water(8), 0);
    assert_eq!(Project::new("Empty").seeded_id_high_water(7), 0);
}
