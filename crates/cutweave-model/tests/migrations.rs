// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1A.03, WP-2A.02 and WP-2F.07 acceptance: golden fixtures per schema version, the v0 → v1
//! and v1 → v2 migrations, read-only newer documents and lossless round trips.
//!
//! The current-version (v2) fixtures are exactly what `to_document_string` writes for the
//! projects built below. The v1 fixtures are the frozen v1 files of the same projects; they
//! must migrate to them (the migration of legacy shapes is covered by `v1/legacy-needs-fixes`).
//! `v2/folders` holds what v1 cannot express (nested media folders and renamed assets,
//! ADR-0020), so it has no v1 counterpart.
//! After an intended format change, regenerate the v2 fixtures with
//! `CUTWEAVE_BLESS_FIXTURES=1 cargo nextest run -p cutweave-model -E 'binary(migrations)'`.
//! Fixtures of older versions are frozen: they must keep loading (through migrations) as the
//! same projects forever.

// `allow-unwrap-in-tests` covers `#[test]` bodies only, not the helpers below.
#![allow(clippy::unwrap_used)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use cutweave_model::{
    AudioInfo, CURRENT_SCHEMA_VERSION, Clip, Color, ColorPrimaries, ColorRange, ColorTags,
    Duration, FolderId, FrameRate, IdGen, MatrixCoefficients, MediaAsset, MediaFolder, MediaKind,
    MigrationError, OLDEST_SCHEMA_VERSION, OpenOutcome, ProbeInfo, Project, SampleRate, SeededGen,
    Sequence, SequenceFormat, StreamInfo, StreamKind, Time, Track, TrackKind,
    TransferCharacteristics, Vec2, canonical_json_pretty, migrate_to_current, migrate_value, open,
    open_str, schema_version_of, to_document_string,
};
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn secs(s: i64) -> Time {
    Time::from_secs(s).unwrap()
}

fn span_secs(s: i64) -> Duration {
    Duration::from_secs(s).unwrap()
}

fn video_probe(seconds: i64) -> ProbeInfo {
    ProbeInfo {
        streams: vec![
            StreamInfo {
                index: 0,
                kind: StreamKind::Video,
                codec: Some("h264".into()),
                profile: Some("High".into()),
                width: Some(1920),
                height: Some(1080),
                pixel_format: Some("yuv420p".into()),
                sample_rate: None,
                channels: None,
                language: None,
                extra: cutweave_model::Extra::new(),
            },
            StreamInfo {
                index: 1,
                kind: StreamKind::Audio,
                codec: Some("aac".into()),
                profile: Some("LC".into()),
                width: None,
                height: None,
                pixel_format: None,
                sample_rate: Some(SampleRate::HZ_48_000),
                channels: Some(2),
                language: Some("eng".into()),
                extra: cutweave_model::Extra::new(),
            },
        ],
        duration: Some(span_secs(seconds)),
        fps: Some(FrameRate::FPS_30),
        vfr: false,
        rotation: 0,
        color: ColorTags {
            primaries: Some(ColorPrimaries::Bt709),
            transfer: Some(TransferCharacteristics::Bt709),
            matrix: Some(MatrixCoefficients::Bt709),
            range: Some(ColorRange::Limited),
        },
        audio: Some(AudioInfo {
            rate: SampleRate::HZ_48_000,
            channels: 2,
        }),
        extra: cutweave_model::Extra::new(),
    }
}

fn audio_probe(seconds: i64) -> ProbeInfo {
    ProbeInfo {
        duration: Some(span_secs(seconds)),
        audio: Some(AudioInfo {
            rate: SampleRate::HZ_44_100,
            channels: 2,
        }),
        ..ProbeInfo::default()
    }
}

fn stamp(project: &mut Project) {
    project.meta.created = Some("2026-11-22T09:30:00Z".into());
    project.meta.app_version = Some("0.1.0".into());
}

/// `v2/empty.cutweave` (and the frozen `v1/empty.cutweave`): a fresh project.
fn expected_empty() -> Project {
    let mut project = Project::new("Empty");
    stamp(&mut project);
    project
}

/// `v2/three-clips.cutweave`: the M1 shape, two videos and music on one sequence.
fn expected_three_clips() -> Project {
    let mut ids = SeededGen::new(1);
    let mut project = Project::new("Three clips");
    stamp(&mut project);
    let mut a = MediaAsset::file(
        ids.mint(),
        MediaKind::Video,
        "/media/a.mp4",
        video_probe(10),
    );
    a.path_rel = Some("../media/a.mp4".into());
    a.content_hash = Some("xxh3:5f1d7a9c3b2e4d60".into());
    let b = MediaAsset::file(ids.mint(), MediaKind::Video, "/media/b.mp4", video_probe(8));
    let music = MediaAsset::file(
        ids.mint(),
        MediaKind::Audio,
        "/media/m.wav",
        audio_probe(60),
    );
    let mut sequence = Sequence::new(ids.mint(), "Main", SequenceFormat::default());
    let mut v1 = Track::new(ids.mint(), TrackKind::Video, "V1");
    let mut a1 = Track::new(ids.mint(), TrackKind::Audio, "A1");
    v1.items
        .insert(Clip::new(ids.mint(), a.id, Time::ZERO, span_secs(2)).into());
    let mut second = Clip::new(ids.mint(), b.id, secs(2), span_secs(3));
    second.source_in = secs(1);
    v1.items.insert(second.into());
    let mut bed = Clip::new(ids.mint(), music.id, Time::ZERO, span_secs(5));
    bed.audio.gain.set(-6.0);
    a1.items.insert(bed.into());
    sequence.insert_track(0, v1);
    sequence.insert_track(1, a1);
    for media in [a, b, music] {
        project.media.insert(media.id, media);
    }
    project.sequences.insert(sequence.id, sequence);
    project
}

/// `v2/edge-cases.cutweave`: NTSC rates, Unicode, adjacent and far-away clips, every media kind,
/// a compound clip, linked items, every track flag, plugin data and unknown fields.
#[allow(clippy::too_many_lines)]
fn expected_edge_cases() -> Project {
    let mut ids = SeededGen::new(2);
    let mut project = Project::new("Café ☕ — 日本語 \"quotes\"");
    stamp(&mut project);
    project.settings.default_format =
        SequenceFormat::new(3840, 2160, FrameRate::FPS_23_976, SampleRate::HZ_96_000);
    project
        .custom
        .insert(
            "com.example.notes",
            json!({"pinned": true, "tags": ["b", "a"]}),
        )
        .unwrap();
    project
        .extra
        .insert("x_future_root", json!({"z": [1, {"b": 2, "a": 1}]}));

    let mut phone = MediaAsset::file(
        ids.mint(),
        MediaKind::Video,
        "C:\\Users\\Ana\\Vídeos\\phone.mov",
        ProbeInfo {
            fps: Some(FrameRate::FPS_29_97),
            vfr: true,
            rotation: 90,
            color: ColorTags {
                primaries: Some(ColorPrimaries::Bt2020),
                transfer: Some(TransferCharacteristics::Hlg),
                matrix: Some(MatrixCoefficients::Other("future-matrix".into())),
                range: Some(ColorRange::Full),
            },
            ..video_probe(120)
        },
    );
    phone.probe.extra.insert("x_future_probe", json!(1));
    let still = MediaAsset::file(
        ids.mint(),
        MediaKind::Image,
        "/stills/logo.png",
        ProbeInfo::default(),
    );
    let voice = MediaAsset::file(
        ids.mint(),
        MediaKind::Audio,
        "/audio/voice.flac",
        audio_probe(30),
    );

    let nested_format =
        SequenceFormat::new(1080, 1920, FrameRate::FPS_29_97, SampleRate::HZ_48_000);
    let mut nested = Sequence::new(ids.mint(), "Nested 9:16", nested_format);
    let mut nested_v = Track::new(ids.mint(), TrackKind::Video, "V1");
    // 119 frames at 29.97 fps: the v1 file has 4 s, which the v2 migration floors to the grid.
    let nested_frame = FrameRate::FPS_29_97.flicks_per_frame().unwrap();
    nested_v.items.insert(
        Clip::new(
            ids.mint(),
            still.id,
            Time::ZERO,
            Duration::from_flicks(119 * nested_frame).unwrap(),
        )
        .into(),
    );
    nested.insert_track(0, nested_v);
    let compound = MediaAsset::sequence_ref(ids.mint(), nested.id);

    let mut format = SequenceFormat::new(1920, 1080, FrameRate::FPS_59_94, SampleRate::HZ_44_100);
    format.background = Color {
        r: 0x12,
        g: 0x34,
        b: 0x56,
        a: 0x80,
    };
    format.extra.insert("x_future_format", json!("hdr10"));
    let mut main = Sequence::new(ids.mint(), "Main 59.94", format);
    main.custom
        .insert("org.cutweave.markers", json!([]))
        .unwrap();

    let frame = FrameRate::FPS_59_94.flicks_per_frame().unwrap();
    let at = |frames: i64| Time::from_flicks(frames * frame).unwrap();
    let len = |frames: i64| Duration::from_flicks(frames * frame).unwrap();

    let mut top = Track::new(ids.mint(), TrackKind::Video, "Overlay ✨");
    top.hidden = true;
    top.locked = true;
    let mut overlay = Clip::new(ids.mint(), still.id, at(10), len(1));
    overlay.name = Some("Logo".into());
    overlay.enabled = false;
    overlay.visual.opacity.set(0.0);
    overlay
        .visual
        .transform
        .position
        .set(Vec2::new(-0.25, 0.375));
    overlay.visual.transform.scale.set(Vec2::new(0.5, 0.5));
    overlay.visual.transform.rotation.set(-12.5);
    overlay.visual.transform.anchor.set(Vec2::new(0.0, 1.0));
    overlay.visual.extra.insert("x_blend_mode", json!("screen"));
    top.items.insert(overlay.into());
    // A clip near the end of the representable range (about 100 days in).
    top.items
        .insert(Clip::new(ids.mint(), still.id, secs(8_640_000), len(1)).into());

    let mut main_v = Track::new(ids.mint(), TrackKind::Video, "Main");
    main_v.magnetic = true;
    let group = ids.mint();
    let mut first = Clip::new(ids.mint(), phone.id, Time::ZERO, len(300));
    first.linked_group = Some(group);
    first
        .custom
        .insert("com.example.grade", json!({"lut": "teal-orange"}))
        .unwrap();
    first.extra.insert("x_future_clip", json!(null));
    let mut adjacent = Clip::new(ids.mint(), compound.id, at(300), len(240));
    adjacent.source_in = Time::ZERO;
    main_v.items.insert(first.into());
    main_v.items.insert(adjacent.into());

    let mut sound = Track::new(ids.mint(), TrackKind::Audio, "Dialog");
    sound.muted = true;
    sound.solo = true;
    sound.gain = -3.5;
    sound.extra.insert("x_future_track", json!({"bus": 2}));
    let mut linked = Clip::new(ids.mint(), phone.id, Time::ZERO, len(300));
    linked.linked_group = Some(group);
    linked.audio.gain.set(4.25);
    linked.audio.extra.insert("x_pan", json!(-0.5));
    sound.items.insert(linked.into());
    let mut vo = Clip::new(ids.mint(), voice.id, at(300), len(600));
    vo.source_in = Time::from_flicks(705_600_000 / 3).unwrap();
    sound.items.insert(vo.into());

    let titles = Track::new(ids.mint(), TrackKind::Text, "Titles (v2)");
    let adjust = Track::new(ids.mint(), TrackKind::Adjustment, "");
    for (i, track) in [top, main_v, titles, adjust, sound].into_iter().enumerate() {
        main.insert_track(i, track);
    }
    for media in [phone, still, voice, compound] {
        project.media.insert(media.id, media);
    }
    project.sequences.insert(nested.id, nested);
    project.sequences.insert(main.id, main);
    project
}

/// `v2/folders.cutweave` (ADR-0020): nested folders (three levels, Unicode and an empty
/// folder), assets in folders and at the top level, display names, folder plugin data and an
/// unknown folder field; one clip uses a renamed asset in a folder.
fn expected_folders() -> Project {
    let mut ids = SeededGen::new(3);
    let mut project = Project::new("Folders");
    stamp(&mut project);
    let broll_id: FolderId = ids.mint();
    let mut broll = MediaFolder::new(broll_id, "B-roll", None);
    broll
        .custom
        .insert("com.example.bin", json!({"color": "teal"}))
        .unwrap();
    let drone = MediaFolder::new(ids.mint(), "Drone", Some(broll_id));
    let mut day = MediaFolder::new(ids.mint(), "Día 1 — 日本", Some(drone.id));
    day.extra.insert("x_future_folder", json!({"sort": "name"}));
    let empty = MediaFolder::new(ids.mint(), "Empty", None);
    let music_folder = MediaFolder::new(ids.mint(), "Music", None);

    let mut aerial = MediaAsset::file(
        ids.mint(),
        MediaKind::Video,
        "/media/DJI_0042.mp4",
        video_probe(30),
    );
    aerial.name = Some("Coastline aerial".into());
    aerial.folder = Some(day.id);
    let mut walk = MediaAsset::file(
        ids.mint(),
        MediaKind::Video,
        "/media/walk.mov",
        video_probe(12),
    );
    walk.folder = Some(broll_id);
    let mut song = MediaAsset::file(
        ids.mint(),
        MediaKind::Audio,
        "/media/song.wav",
        audio_probe(90),
    );
    song.folder = Some(music_folder.id);
    song.name = Some("Theme (final mix)".into());
    let mut logo = MediaAsset::file(
        ids.mint(),
        MediaKind::Image,
        "/stills/logo.png",
        ProbeInfo::default(),
    );
    logo.name = Some("Logo ✨".into());

    let mut sequence = Sequence::new(ids.mint(), "Main", SequenceFormat::default());
    let mut v1 = Track::new(ids.mint(), TrackKind::Video, "V1");
    v1.items
        .insert(Clip::new(ids.mint(), aerial.id, Time::ZERO, span_secs(4)).into());
    sequence.insert_track(0, v1);
    for folder in [broll, drone, day, empty, music_folder] {
        project.folders.insert(folder.id, folder);
    }
    for media in [aerial, walk, song, logo] {
        project.media.insert(media.id, media);
    }
    project.sequences.insert(sequence.id, sequence);
    project
}

/// The current fixtures that are the migrated form of a frozen v1 file.
fn migrated_fixtures() -> [(&'static str, Project); 3] {
    [
        ("empty", expected_empty()),
        ("three-clips", expected_three_clips()),
        ("edge-cases", expected_edge_cases()),
    ]
}

fn current_fixtures() -> Vec<(&'static str, Project)> {
    let mut all: Vec<_> = migrated_fixtures().into();
    all.push(("folders", expected_folders()));
    all
}

fn editable(outcome: OpenOutcome) -> (Project, cutweave_model::MigrationReport) {
    match outcome {
        OpenOutcome::Editable { project, report } => (project, report),
        OpenOutcome::ReadOnly { reason, .. } => panic!("unexpected read-only: {reason}"),
    }
}

#[test]
fn the_expected_projects_are_valid() {
    for (name, project) in current_fixtures() {
        assert_eq!(project.violations(), vec![], "{name}");
    }
}

/// Each current fixture loads as the expected project and is byte-for-byte what the engine
/// writes for it.
#[test]
fn current_fixtures_load_as_the_expected_projects() {
    let dir = fixtures().join(format!("v{CURRENT_SCHEMA_VERSION}"));
    let bless = std::env::var_os("CUTWEAVE_BLESS_FIXTURES").is_some();
    for (name, expected) in current_fixtures() {
        let path = dir.join(format!("{name}.cutweave"));
        let written = to_document_string(&expected).unwrap();
        if bless {
            fs::create_dir_all(&dir).unwrap();
            fs::write(&path, &written).unwrap();
        }
        let text = read(&path);
        let (project, report) = editable(open_str(&text).unwrap());
        assert_eq!(project, expected, "{name}");
        assert!(!report.migrated(), "{name}");
        assert_eq!(
            text, written,
            "{name}: the saved form changed; see the module docs"
        );
    }
}

#[test]
fn every_schema_version_has_a_fixture_directory() {
    for version in OLDEST_SCHEMA_VERSION..=CURRENT_SCHEMA_VERSION {
        let dir = fixtures().join(format!("v{version}"));
        let files: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap_or_else(|e| {
                panic!(
                    "schema version {version} needs fixtures in {}: {e}",
                    dir.display()
                )
            })
            .map(|entry| entry.unwrap().path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "cutweave"))
            .collect();
        assert!(
            !files.is_empty(),
            "{} has no .cutweave fixtures",
            dir.display()
        );
        for file in files {
            let doc: Value = serde_json::from_str(&read(&file)).unwrap();
            assert_eq!(schema_version_of(&doc), Ok(version), "{}", file.display());
        }
    }
}

/// Every fixture of every version opens as an editable, valid, current project.
#[test]
fn every_fixture_migrates_to_a_valid_current_project() {
    for version in OLDEST_SCHEMA_VERSION..=CURRENT_SCHEMA_VERSION {
        for entry in fs::read_dir(fixtures().join(format!("v{version}"))).unwrap() {
            let path = entry.unwrap().path();
            let (project, report) = editable(open_str(&read(&path)).unwrap());
            assert_eq!(report.from_version, version);
            assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
            assert_eq!(
                report.steps.len(),
                usize::try_from(CURRENT_SCHEMA_VERSION - version).unwrap()
            );
            assert_eq!(project.meta.schema_version, CURRENT_SCHEMA_VERSION);
            assert_eq!(project.violations(), vec![], "{}", path.display());
        }
    }
}

#[test]
fn the_v0_fixture_migrates_to_the_current_three_clip_project() {
    let text = read(&fixtures().join("v0/three-clips.cutweave"));
    let (project, report) = editable(open_str(&text).unwrap());
    assert_eq!(project, expected_three_clips());
    assert_eq!((report.from_version, report.to_version), (0, 2));
    assert_eq!(report.steps.len(), 2);
    assert!(report.steps[0].description.contains("pre-release"));
    let (migrated, _) = migrate_value(serde_json::from_str(&text).unwrap()).unwrap();
    insta::assert_snapshot!("v0_three_clips_migrated", canonical_json_pretty(&migrated));
}

/// The frozen v1 files migrate to the same projects the v2 fixtures hold.
#[test]
fn the_v1_fixtures_migrate_to_the_v2_projects() {
    for (name, expected) in migrated_fixtures() {
        let text = read(&fixtures().join(format!("v1/{name}.cutweave")));
        let (project, report) = editable(open_str(&text).unwrap());
        assert_eq!(project, expected, "{name}");
        assert_eq!((report.from_version, report.to_version), (1, 2), "{name}");
        assert_eq!(report.steps.len(), 1, "{name}");
        // The migrated document is what the engine saves: the v2 fixture, byte for byte.
        let v2 = read(&fixtures().join(format!("v2/{name}.cutweave")));
        assert_eq!(to_document_string(&project).unwrap(), v2, "{name}");
        // Saving the migrated document and migrating it again changes nothing (ADR-0020).
        let (again, again_report) = editable(open_str(&v2).unwrap());
        assert_eq!(again, project, "{name}");
        assert!(!again_report.migrated(), "{name}");
        assert_eq!(to_document_string(&again).unwrap(), v2, "{name}");
    }
}

/// ADR-0020: a v1 document has no folders, so every asset lands at the top level under its
/// file name.
#[test]
fn the_v1_migration_puts_every_asset_at_the_top_level() {
    for (name, _) in migrated_fixtures() {
        let text = read(&fixtures().join(format!("v1/{name}.cutweave")));
        let (project, _) = editable(open_str(&text).unwrap());
        assert!(project.folders.is_empty(), "{name}");
        for media in project.media.values() {
            assert_eq!((&media.name, media.folder), (&None, None), "{name}");
        }
        let (migrated, _) = migrate_value(serde_json::from_str(&text).unwrap()).unwrap();
        assert_eq!(migrated["folders"], json!({}), "{name}");
        for media in migrated["media"].as_object().unwrap().values() {
            assert_eq!(media["name"], Value::Null, "{name}");
            assert_eq!(media["folder"], Value::Null, "{name}");
        }
    }
}

#[test]
fn the_folders_fixture_keeps_its_tree() {
    let text = read(&fixtures().join("v2/folders.cutweave"));
    let (project, _) = editable(open_str(&text).unwrap());
    let paths: Vec<String> = project
        .folders
        .keys()
        .map(|f| project.folder_path(*f).unwrap())
        .collect();
    assert_eq!(
        paths,
        [
            "B-roll",
            "B-roll/Drone",
            "B-roll/Drone/Día 1 — 日本",
            "Empty",
            "Music"
        ]
    );
    let named: Vec<&str> = project
        .media
        .values()
        .filter_map(|m| m.name.as_deref())
        .collect();
    assert_eq!(named, ["Coastline aerial", "Theme (final mix)", "Logo ✨"]);
}

/// A v1 document with every shape the v2 migration repairs: an off-grid clip duration, a
/// misaligned and a single-member linked group, and several magnetic tracks (one of them with a
/// gap). Pinned as a golden snapshot, with the notes the user sees.
#[test]
fn the_v1_migration_repairs_legacy_shapes() {
    let text = read(&fixtures().join("v1/legacy-needs-fixes.cutweave"));
    let (migrated, report) = migrate_value(serde_json::from_str(&text).unwrap()).unwrap();
    assert_eq!((report.from_version, report.to_version), (1, 2));
    let notes = &report.steps[0].notes;
    assert_eq!(
        notes,
        &[
            "floored 2 clip duration(s) to a whole number of frames",
            "dissolved 2 linked group(s) that were not aligned",
            "kept the first magnetic track of a sequence and cleared 1 other flag(s)",
        ]
    );
    insta::assert_snapshot!(
        "v1_legacy_needs_fixes_migrated",
        canonical_json_pretty(&migrated)
    );
    let (project, _) = editable(open_str(&text).unwrap());
    assert_eq!(project.violations(), vec![]);
    // Durations are whole frames now (30 fps: 23,520,000 flicks per frame).
    for track in project.sequences.values().flat_map(|s| s.tracks.values()) {
        for item in track.items.iter() {
            assert_eq!(item.duration().flicks() % 23_520_000, 0, "{}", item.id());
        }
    }
    let to_value = to_document_string(&project).unwrap();
    let (again, again_report) = editable(open_str(&to_value).unwrap());
    assert_eq!(again, project, "the migrated document round-trips");
    assert!(!again_report.migrated());
}

/// Migrating twice is the same as migrating once (a v2 document is left alone).
#[test]
fn the_v1_migration_is_idempotent_on_v2_documents() {
    for (name, _) in current_fixtures() {
        let v2: Value =
            serde_json::from_str(&read(&fixtures().join(format!("v2/{name}.cutweave")))).unwrap();
        let (again, report) = migrate_value(v2.clone()).unwrap();
        assert_eq!(again, v2, "{name}");
        assert!(!report.migrated(), "{name}");
    }
}

#[test]
fn a_newer_document_opens_read_only_and_keeps_unknown_fields() {
    let text = read(&fixtures().join("newer/v999.cutweave"));
    let OpenOutcome::ReadOnly {
        project,
        found_version,
        reason,
    } = open_str(&text).unwrap()
    else {
        panic!("a v999 document must open read-only");
    };
    assert_eq!(found_version, 999);
    assert!(reason.contains("newer version"), "{reason}");
    assert_eq!(project.meta.schema_version, 999);
    assert!(project.extra.get("transcripts").is_some());
    // Nothing is dropped: the read-only view re-serializes to the same JSON.
    let original: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(serde_json::to_value(&project).unwrap(), original);
    assert_eq!(
        migrate_to_current(original).unwrap_err(),
        MigrationError::NewerVersion {
            found: 999,
            supported: CURRENT_SCHEMA_VERSION
        }
    );
}

#[test]
fn unknown_fields_survive_open_and_save() {
    let mut doc: Value = serde_json::from_str(&read(
        &fixtures().join(format!("v{CURRENT_SCHEMA_VERSION}/three-clips.cutweave")),
    ))
    .unwrap();
    doc["x_plugin_index"] = json!({"b": 1, "a": 2});
    let seq = doc["sequences"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    doc["sequences"][&seq]["x_markers"] = json!([{"t": 0}]);
    let (project, _) = editable(open(doc.clone()).unwrap());
    let saved = to_document_string(&project).unwrap();
    let reopened: Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(reopened, doc);
    let (again, _) = editable(open_str(&saved).unwrap());
    assert_eq!(again, project);
}

#[test]
fn opening_rejects_documents_that_are_not_projects() {
    assert_eq!(open(json!([1])).unwrap_err(), MigrationError::NotAnObject);
    assert_eq!(
        open(json!({"meta": {}})).unwrap_err(),
        MigrationError::MissingVersion
    );
    let err = open(json!({"meta": {"schema_version": 2}})).unwrap_err();
    assert!(matches!(err, MigrationError::Malformed { .. }), "{err:?}");
    let mut doc: Value =
        serde_json::from_str(&read(&fixtures().join("v0/three-clips.cutweave"))).unwrap();
    doc["sequences"] = json!([]);
    let err = open(doc).unwrap_err();
    assert!(
        matches!(err, MigrationError::Step { from: 0, .. }),
        "{err:?}"
    );
    assert!(err.to_string().contains("/sequences"), "{err}");
}

#[test]
fn saving_is_deterministic_and_ends_with_a_newline() {
    let project = expected_edge_cases();
    let a = to_document_string(&project).unwrap();
    let b = to_document_string(&project.clone()).unwrap();
    assert_eq!(a, b);
    assert!(a.ends_with("}\n"));
    assert!(!a.contains('\r'));
}
