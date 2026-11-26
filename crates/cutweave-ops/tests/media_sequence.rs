// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.05c (ADR-0035): `media.import_sequence` through the `MediaProbe` boundary, and what an
//! image sequence is to the rest of the commands (video for tracks, no sound).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, assert_invalid, id};
use cutweave_commands::{Access, CmdError, McpExposure};
use cutweave_model::{Duration, FrameRate, MediaId, MediaKind};
use cutweave_ops::media::{AllowedRoots, FakeProbe, ImportPolicy};
use serde_json::{Value, json};

/// `/r/shot_0001.png` … `shot_0300.png` without the numbers in `missing`, plus a still.
fn run_of(count: u32, missing: &[u32]) -> FakeProbe {
    (1..=count)
        .filter(|n| !missing.contains(n))
        .fold(FakeProbe::new(), |probe, n| {
            let path = format!("/r/shot_{n:04}.png");
            probe.with(&path, FakeProbe::image(&path))
        })
        .with("/r/cover.png", FakeProbe::image("/r/cover.png"))
        .with("/m/a.mp4", FakeProbe::video("/m/a.mp4", 10, true))
}

fn import_with(
    h: &H,
    prober: FakeProbe,
    fps: Option<FrameRate>,
    params: Value,
) -> Result<Value, CmdError> {
    let mut ctx = h.import_ctx(prober).with_sequence_fps(fps);
    h.registry
        .invoke_engine_json("media.import_sequence", params, &mut ctx, Access::STANDARD)
}

fn import(h: &H, prober: FakeProbe, params: Value) -> Result<Value, CmdError> {
    import_with(h, prober, None, params)
}

#[test]
fn a_run_imports_as_one_asset_from_any_of_its_files_or_its_folder() {
    for path in ["/r/shot_0150.png", "/r/shot_0001.png", "/r"] {
        let h = H::empty();
        let out = import(&h, run_of(300, &[]), json!({"path": path})).unwrap();
        assert_eq!(out["kind"], "image_sequence", "{path}");
        assert_eq!(out["duplicate"], false);
        assert_eq!(out["path"], "/r/shot_0001.png");
        assert_eq!(out["warnings"], json!([]));
        assert_eq!(
            out["image_sequence"],
            json!({"pattern": "shot_%04d.png", "first": 1, "count": 300,
                   "fps": {"num": 30, "den": 1}, "missing": []})
        );
        let doc = h.doc();
        assert_eq!(doc.media.len(), 1, "one asset, not 300");
        let asset = &doc.media[&id::<MediaId>(&out["media_id"])];
        assert_eq!(asset.kind, MediaKind::ImageSequence);
        assert_eq!(asset.path_abs.as_deref(), Some("/r/shot_0001.png"));
        assert_eq!(asset.probe.duration, Some(Duration::from_secs(10).unwrap()));
        assert_eq!(asset.probe.fps, Some(FrameRate::FPS_30));
        assert!(asset.has_video() && !asset.has_audio());
        assert_eq!(asset.image_sequence.as_ref().unwrap().count, 300);
        assert_eq!(h.commits(), 1, "one transaction: one undo step");
        assert_eq!(doc.violations(), vec![]);
    }
}

#[test]
fn the_frame_rate_is_the_param_then_the_shown_sequences_then_30() {
    let h = H::empty();
    let given = json!({"path": "/r", "fps": {"num": 24000, "den": 1001}});
    let out = import_with(&h, run_of(48, &[]), Some(FrameRate::FPS_25), given).unwrap();
    assert_eq!(
        out["image_sequence"]["fps"],
        json!({"num": 24000, "den": 1001})
    );
    let asset = &h.doc().media[&id::<MediaId>(&out["media_id"])];
    assert_eq!(
        asset.probe.duration,
        FrameRate::FPS_23_976
            .frame_to_time(48)
            .ok()
            .and_then(|t| Duration::from_flicks(t.flicks()).ok())
    );

    let h = H::empty();
    let out = import_with(
        &h,
        run_of(50, &[]),
        Some(FrameRate::FPS_25),
        json!({"path": "/r"}),
    )
    .unwrap();
    assert_eq!(out["image_sequence"]["fps"], json!({"num": 25, "den": 1}));
    let asset = &h.doc().media[&id::<MediaId>(&out["media_id"])];
    assert_eq!(asset.probe.duration, Some(Duration::from_secs(2).unwrap()));

    let h = H::empty();
    let out = import(&h, run_of(60, &[]), json!({"path": "/r"})).unwrap();
    assert_eq!(out["image_sequence"]["fps"], json!({"num": 30, "den": 1}));

    assert_invalid(
        import(
            &h,
            run_of(60, &[]),
            json!({"path": "/r", "fps": {"num": 0, "den": 1}}),
        ),
        "/fps",
    );
}

#[test]
fn gaps_are_kept_on_the_asset_and_named_in_warnings() {
    let h = H::empty();
    let out = import(&h, run_of(300, &[120, 121, 250]), json!({"path": "/r"})).unwrap();
    assert_eq!(out["image_sequence"]["count"], 300);
    assert_eq!(out["image_sequence"]["missing"], json!([120, 121, 250]));
    assert_eq!(
        out["warnings"],
        json!([
            "Pictures 120 to 121 are missing: the picture before shows instead.",
            "Picture 250 is missing: the picture before shows instead."
        ])
    );
    let asset = &h.doc().media[&id::<MediaId>(&out["media_id"])];
    let run = asset.image_sequence.as_ref().unwrap();
    assert_eq!(run.missing, [120, 121, 250]);
    assert_eq!(
        run.number_for_frame(120),
        119,
        "frame 120 is picture 121: missing"
    );
    assert_eq!(h.doc().violations(), vec![]);
}

#[test]
fn the_same_run_again_is_a_duplicate_and_another_rate_is_another_asset() {
    let h = H::empty();
    let first = import(&h, run_of(12, &[]), json!({"path": "/r"})).unwrap();
    let again = import(&h, run_of(12, &[]), json!({"path": "/r/shot_0003.png"})).unwrap();
    assert_eq!(again["duplicate"], true);
    assert_eq!(again["media_id"], first["media_id"]);
    assert_eq!(h.doc().media.len(), 1);
    let slower = json!({"path": "/r", "fps": {"num": 12, "den": 1}});
    let other = import(&h, run_of(12, &[]), slower).unwrap();
    assert_eq!(other["duplicate"], false);
    assert_ne!(other["media_id"], first["media_id"]);
    // A still of the first picture is its own asset too: `media.import` never guesses a run.
    let mut ctx = h.import_ctx(run_of(12, &[]));
    let still = h
        .registry
        .invoke_engine_json(
            "media.import",
            json!({"paths": ["/r/shot_0001.png"]}),
            &mut ctx,
            Access::STANDARD,
        )
        .unwrap();
    assert_eq!(still["media"][0]["kind"], "image");
    assert_eq!(still["media"][0]["duplicate"], false);
    assert_eq!(h.doc().media.len(), 3);
}

#[test]
fn what_is_not_a_run_is_invalid_and_changes_nothing() {
    let h = H::empty();
    for (params, needle) in [
        (json!({"path": "/r/cover.png"}), "no number at the end"),
        (json!({"path": "/r/shot_0999.png"}), "no readable file"),
        (json!({"path": "/nowhere"}), "no readable file"),
        (json!({"path": "/m/a.mp4"}), "no number at the end"),
        (json!({"path": "r/shot_0001.png"}), "not an absolute path"),
    ] {
        match import(&h, run_of(12, &[]), params.clone()) {
            Err(CmdError::Invalid { path, msg }) => {
                assert_eq!(path, "/path", "{params}");
                assert!(msg.contains(needle), "{params}: {msg}");
            }
            other => panic!("{params}: {other:?}"),
        }
    }
    // One picture is a still, not a run.
    let single = FakeProbe::new().with("/s/f_01.png", FakeProbe::image("/s/f_01.png"));
    match import(&h, single, json!({"path": "/s/f_01.png"})) {
        Err(CmdError::Invalid { msg, .. }) => assert!(msg.contains("two or more"), "{msg}"),
        other => panic!("{other:?}"),
    }
    // A numbered run of videos is not pictures.
    let clips = FakeProbe::new()
        .with("/v/take1.mp4", FakeProbe::video("/v/take1.mp4", 3, false))
        .with("/v/take2.mp4", FakeProbe::video("/v/take2.mp4", 3, false));
    assert_invalid(import(&h, clips, json!({"path": "/v/take1.mp4"})), "/path");
    // An unknown bin folder.
    let unknown = import(
        &h,
        run_of(12, &[]),
        json!({"path": "/r", "folder": "fld_00000000000000000000000000000001"}),
    );
    assert_eq!(unknown.unwrap_err().code(), "not_found");
    assert_eq!(h.doc().media.len(), 0);
    assert_eq!(h.commits(), 0);
}

#[test]
fn agents_import_only_from_allowed_folders() {
    let h = H::empty();
    let restricted = |roots: &[&str]| {
        ImportPolicy::restricted(AllowedRoots::new(roots.iter().map(|r| (*r).to_owned())))
    };
    let mut ctx = h
        .import_ctx(run_of(12, &[]))
        .with_policy(restricted(&["/project"]));
    let refused = h.registry.invoke_engine_json(
        "media.import_sequence",
        json!({"path": "/r"}),
        &mut ctx,
        Access::STANDARD,
    );
    match refused {
        Err(CmdError::Invalid { path, msg }) => {
            assert_eq!(path, "/path");
            assert!(msg.contains("outside the folders"), "{msg}");
        }
        other => panic!("{other:?}"),
    }
    let mut ctx = h
        .import_ctx(run_of(12, &[]))
        .with_policy(restricted(&["/r"]));
    h.registry
        .invoke_engine_json(
            "media.import_sequence",
            json!({"path": "/r/shot_0002.png"}),
            &mut ctx,
            Access::STANDARD,
        )
        .unwrap();
    assert_eq!(h.doc().media.len(), 1);
}

#[test]
fn the_command_is_in_the_mcp_catalog_and_not_a_document_command() {
    let h = H::empty();
    let entry = h.registry.get("media.import_sequence").unwrap();
    assert_eq!(entry.meta.mcp, McpExposure::Catalog);
    assert!(!entry.meta.annotations.read_only);
    assert!(!entry.meta.annotations.destructive);
}

/// Sequences are video for tracks: a sequence goes on a video track with its whole length,
/// never on an audio track, and inserting it makes no linked audio clip.
#[test]
fn a_sequence_cuts_like_video_without_sound() {
    let mut h = H::empty();
    let out = import(&h, run_of(90, &[]), json!({"path": "/r"})).unwrap();
    let media = out["media_id"].clone();
    let made = h.ok("sequence.create", json!({"name": "Main"}));
    let sequence = made["sequence"].clone();
    let (video, audio) = (made["video_track"].clone(), made["audio_track"].clone());
    let inserted = h.ok(
        "timeline.insert",
        json!({"sequence": sequence, "track": video, "media": media, "at": 0}),
    );
    let items = inserted["clips"].as_array().unwrap();
    assert_eq!(items.len(), 1, "no linked audio clip: {inserted}");
    assert_eq!(inserted["linked_group"], Value::Null);
    let doc = h.doc();
    let seq = &doc.sequences[&id::<cutweave_model::SequenceId>(&sequence)];
    let clip = seq
        .tracks
        .values()
        .flat_map(|t| t.items.iter())
        .next()
        .unwrap();
    assert_eq!(
        clip.duration(),
        Duration::from_secs(3).unwrap(),
        "90 frames at 30 fps"
    );
    let on_audio = h.run(
        "timeline.insert",
        json!({"sequence": sequence, "track": audio, "media": media, "at": 0}),
    );
    assert!(on_audio.is_err(), "an image sequence has no sound");
    assert_eq!(h.doc().violations(), vec![]);
}

/// One undo step: the sink's single transaction holds the one new asset.
#[test]
fn the_import_is_one_transaction() {
    let mut h = H::empty();
    import(&h, run_of(12, &[4]), json!({"path": "/r"})).unwrap();
    assert_eq!(h.commits(), 1);
    h.ok("project.rename", json!({"name": "after import"}));
    assert_eq!(h.last.as_ref().unwrap().base.media.len(), 1);
}
