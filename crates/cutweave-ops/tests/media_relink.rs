// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.03: `media.set_location`, `media.relink`, `media.relink_folder` and
//! `media.find_missing`, through the `MediaProbe` and `RelinkFiles` boundaries.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, assert_code, assert_invalid, id};
use cutweave_commands::{Access, CmdError, McpExposure};
use cutweave_model::MediaId;
use cutweave_ops::media::{AllowedRoots, FakeProbe, ImportPolicy, MemoryFiles, Probed};
use serde_json::{Value, json};

/// The content of `original` found at `path` (same hash, same streams).
fn moved(original: &str, secs: i64) -> Probed {
    FakeProbe::video(original, secs, true)
}

fn engine(
    h: &H,
    name: &str,
    prober: FakeProbe,
    files: MemoryFiles,
    params: Value,
) -> Result<Value, CmdError> {
    let mut ctx = h.relink_ctx(prober, files);
    h.registry
        .invoke_engine_json(name, params, &mut ctx, Access::STANDARD)
}

fn path_of(h: &H, media: MediaId) -> String {
    h.doc().media[&media].path_abs.clone().unwrap()
}

// ----- media.set_location -------------------------------------------------------------------

#[test]
fn set_location_moves_the_asset_and_undo_puts_it_back() {
    let mut h = H::empty();
    let m = h.add_video("/old/a.mp4", 10, true);
    let hash = h.doc().media[&m].content_hash.clone();
    // A saved project also holds the path relative to its folder.
    let mut doc = h.doc();
    doc.media.get_mut(&m).unwrap().path_rel = Some("../old/a.mp4".into());
    h.replace_doc(doc);
    // `H::run` asserts the undo and redo identity of every command it runs.
    h.ok(
        "media.set_location",
        json!({"media": m, "path": "/new/a.mp4"}),
    );
    assert_eq!(path_of(&h, m), "/new/a.mp4");
    assert_eq!(h.doc().media[&m].content_hash, hash, "content unchanged");
    assert_eq!(
        h.doc().media[&m].path_rel,
        None,
        "the old relative path is gone"
    );
    let before = &h.last.as_ref().unwrap().base;
    assert_eq!(before.media[&m].path_abs.as_deref(), Some("/old/a.mp4"));
    assert_eq!(before.media[&m].path_rel.as_deref(), Some("../old/a.mp4"));
}

#[test]
fn set_location_can_record_new_content() {
    let mut h = H::empty();
    let m = h.add_video("/old/a.mp4", 10, true);
    let other = FakeProbe::video("/new/b.mp4", 12, true);
    h.ok(
        "media.set_location",
        json!({"media": m, "path": "/new/b.mp4", "content_hash": other.content_hash,
               "probe": other.probe}),
    );
    let doc = h.doc();
    assert_eq!(
        doc.media[&m].content_hash.as_deref(),
        Some(other.content_hash.as_str())
    );
    assert_eq!(doc.media[&m].probe, other.probe);
}

#[test]
fn set_location_validates_its_params() {
    let (mut h, p) = H::project();
    let m = h.add_video("/old/a.mp4", 10, true);
    assert_invalid(
        h.run(
            "media.set_location",
            json!({"media": m, "path": "relative.mp4"}),
        ),
        "/path",
    );
    assert_invalid(
        h.run(
            "media.set_location",
            json!({"media": m, "path": "/new/a.mp4", "content_hash": "has space"}),
        ),
        "/content_hash",
    );
    assert_code(
        h.run(
            "media.set_location",
            json!({"media": MediaId::from_u128(999), "path": "/new/a.mp4"}),
        ),
        "not_found",
    );
    // A compound clip has no file.
    let mut doc = h.doc();
    let compound = cutweave_model::MediaAsset::sequence_ref(MediaId::from_u128(77), p.sequence);
    doc.media.insert(compound.id, compound);
    h.replace_doc(doc);
    assert_invalid(
        h.run(
            "media.set_location",
            json!({"media": MediaId::from_u128(77), "path": "/new/a.mp4"}),
        ),
        "/media",
    );
    let meta = &h.registry.get("media.set_location").unwrap().meta;
    assert_eq!(meta.mcp, McpExposure::Hidden);
    assert!(meta.undoable);
}

// ----- media.relink -------------------------------------------------------------------------

#[test]
fn relink_points_the_asset_at_the_moved_file() {
    let mut h = H::empty();
    let m = h.add_video("/old/a.mp4", 10, true);
    let commits = h.commits();
    let out = engine(
        &h,
        "media.relink",
        FakeProbe::new().with("/new/renamed.mp4", moved("/old/a.mp4", 10)),
        MemoryFiles::new().with_file("/new/renamed.mp4"),
        json!({"media": m, "path": "/new/renamed.mp4"}),
    )
    .unwrap();
    assert_eq!(out["media_id"], json!(m));
    assert_eq!(out["from"], "/old/a.mp4");
    assert_eq!(out["to"], "/new/renamed.mp4");
    assert_eq!(out["matched_by"], "hash");
    assert_eq!(path_of(&h, m), "/new/renamed.mp4");
    assert_eq!(h.commits(), commits + 1);
}

#[test]
fn relink_accepts_other_content_of_the_same_kind_that_is_long_enough() {
    let mut h = H::empty();
    let m = h.add_video("/old/a.mp4", 10, true);
    let replacement = FakeProbe::video("/new/regraded.mp4", 10, true);
    let out = engine(
        &h,
        "media.relink",
        FakeProbe::new().with("/new/regraded.mp4", replacement.clone()),
        MemoryFiles::new().with_file("/new/regraded.mp4"),
        json!({"media": m, "path": "/new/regraded.mp4"}),
    )
    .unwrap();
    assert_eq!(out["matched_by"], "chosen");
    let doc = h.doc();
    assert_eq!(
        doc.media[&m].content_hash.as_deref(),
        Some(replacement.content_hash.as_str())
    );
}

#[test]
fn relink_refuses_files_that_cannot_stand_in() {
    let mut h = H::empty();
    let m = h.add_video("/old/a.mp4", 10, true);
    let prober = || {
        FakeProbe::new()
            .with(
                "/new/short.mp4",
                FakeProbe::video("/new/short.mp4", 4, true),
            )
            .with("/new/sound.wav", FakeProbe::audio("/new/sound.wav", 10))
    };
    let files = || {
        MemoryFiles::new()
            .with_file("/new/short.mp4")
            .with_file("/new/sound.wav")
    };
    for path in ["/new/short.mp4", "/new/sound.wav", "/new/absent.mp4"] {
        assert_invalid(
            engine(
                &h,
                "media.relink",
                prober(),
                files(),
                json!({"media": m, "path": path}),
            ),
            "/path",
        );
    }
    assert_invalid(
        engine(
            &h,
            "media.relink",
            prober(),
            files(),
            json!({"media": m, "path": "short.mp4"}),
        ),
        "/path",
    );
    assert_code(
        engine(
            &h,
            "media.relink",
            prober(),
            files(),
            json!({"media": MediaId::from_u128(999), "path": "/new/short.mp4"}),
        ),
        "not_found",
    );
    assert_eq!(path_of(&h, m), "/old/a.mp4", "nothing changed");
}

#[test]
fn relink_stays_inside_the_callers_folders() {
    let mut h = H::empty();
    let m = h.add_video("/old/a.mp4", 10, true);
    let mut ctx = h.relink_ctx(
        FakeProbe::new().with("/elsewhere/a.mp4", moved("/old/a.mp4", 10)),
        MemoryFiles::new().with_file("/elsewhere/a.mp4"),
    );
    ctx.policy = ImportPolicy::restricted(AllowedRoots::new(["/allowed"]));
    let r = h.registry.invoke_engine_json(
        "media.relink",
        json!({"media": m, "path": "/elsewhere/a.mp4"}),
        &mut ctx,
        Access::STANDARD,
    );
    assert_invalid(r, "/path");
    let r = h.registry.invoke_engine_json(
        "media.relink_folder",
        json!({"folder": "/elsewhere"}),
        &mut ctx,
        Access::STANDARD,
    );
    assert_invalid(r, "/folder");
}

// ----- media.relink_folder ------------------------------------------------------------------

/// Twenty assets under `/old`, and where their files are now: under `/moved`, every third one
/// renamed, every fourth one a folder deeper.
fn twenty(h: &mut H) -> (Vec<MediaId>, FakeProbe, MemoryFiles) {
    let mut ids = Vec::new();
    let mut prober = FakeProbe::new();
    let mut files = MemoryFiles::new();
    for i in 0..20 {
        let old = format!("/old/clip{i:02}.mp4");
        ids.push(h.add_video(&old, 5 + i, true));
        let name = if i % 3 == 0 {
            format!("renamed-{i}.mp4")
        } else {
            format!("clip{i:02}.mp4")
        };
        let new = if i % 4 == 0 {
            format!("/moved/deep/er/{name}")
        } else {
            format!("/moved/{name}")
        };
        prober = prober.with(&new, moved(&old, 5 + i));
        files = files.with_file(&new);
    }
    // Other files in the folder are left alone.
    prober = prober.with(
        "/moved/unrelated.mp4",
        FakeProbe::video("/moved/unrelated.mp4", 9, true),
    );
    files = files.with_file("/moved/unrelated.mp4");
    files = files.with_file("/moved/notes.txt");
    (ids, prober, files)
}

#[test]
fn relink_folder_restores_every_asset_with_one_command() {
    let mut h = H::empty();
    let (ids, prober, files) = twenty(&mut h);
    let commits = h.commits();
    let out = engine(
        &h,
        "media.relink_folder",
        prober,
        files,
        json!({"folder": "/moved"}),
    )
    .unwrap();
    assert_eq!(out["relinked"].as_array().unwrap().len(), 20);
    assert_eq!(out["still_missing"], json!([]));
    assert!(
        out["relinked"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["matched_by"] == "hash")
    );
    assert_eq!(h.commits(), commits + 1, "one transaction: one undo step");
    assert_eq!(path_of(&h, ids[0]), "/moved/deep/er/renamed-0.mp4");
    assert_eq!(path_of(&h, ids[1]), "/moved/clip01.mp4");
    assert!(h.doc().media.values().all(|m| {
        m.path_abs
            .as_deref()
            .is_some_and(|p| p.starts_with("/moved/"))
    }));
}

#[test]
fn relink_folder_without_recursion_only_looks_at_the_folder_itself() {
    let mut h = H::empty();
    let (ids, prober, files) = twenty(&mut h);
    let out = engine(
        &h,
        "media.relink_folder",
        prober,
        files,
        json!({"folder": "/moved", "recursive": false}),
    )
    .unwrap();
    assert_eq!(out["relinked"].as_array().unwrap().len(), 15);
    let missing: Vec<MediaId> = id(&out["still_missing"]);
    assert_eq!(missing.len(), 5);
    assert!(missing.contains(&ids[0]));
    assert_eq!(path_of(&h, ids[0]), "/old/clip00.mp4");
}

#[test]
fn relink_folder_leaves_assets_whose_file_is_there_alone() {
    let mut h = H::empty();
    let here = h.add_video("/old/here.mp4", 10, true);
    let gone = h.add_video("/old/gone.mp4", 10, true);
    let out = engine(
        &h,
        "media.relink_folder",
        FakeProbe::new()
            .with("/moved/here.mp4", moved("/old/here.mp4", 10))
            .with("/moved/gone.mp4", moved("/old/gone.mp4", 10)),
        MemoryFiles::new()
            .with_file("/old/here.mp4")
            .with_file("/moved/here.mp4")
            .with_file("/moved/gone.mp4"),
        json!({"folder": "/moved"}),
    )
    .unwrap();
    assert_eq!(out["relinked"].as_array().unwrap().len(), 1);
    assert_eq!(path_of(&h, here), "/old/here.mp4");
    assert_eq!(path_of(&h, gone), "/moved/gone.mp4");
}

#[test]
fn relink_folder_falls_back_to_the_name_and_duration() {
    let mut h = H::empty();
    let m = h.add_video("/old/interview.mp4", 10, true);
    let other = h.add_video("/old/broll.mp4", 10, true);
    // Re-wrapped files: other bytes, same name and length. A file with the right name but
    // another length is not taken.
    let out = engine(
        &h,
        "media.relink_folder",
        FakeProbe::new()
            .with(
                "/moved/Interview.MP4",
                FakeProbe::video("/moved/Interview.MP4", 10, true),
            )
            .with(
                "/moved/broll.mp4",
                FakeProbe::video("/moved/broll.mp4", 3, true),
            ),
        MemoryFiles::new()
            .with_file("/moved/Interview.MP4")
            .with_file("/moved/broll.mp4"),
        json!({"folder": "/moved"}),
    )
    .unwrap();
    assert_eq!(out["relinked"].as_array().unwrap().len(), 1);
    assert_eq!(out["relinked"][0]["matched_by"], "name");
    assert_eq!(out["relinked"][0]["media_id"], json!(m));
    assert_eq!(out["still_missing"], json!([other]));
    assert_eq!(path_of(&h, m), "/moved/Interview.MP4");
}

#[test]
fn relink_folder_can_be_limited_to_some_assets() {
    let mut h = H::empty();
    let (ids, prober, files) = twenty(&mut h);
    let out = engine(
        &h,
        "media.relink_folder",
        prober,
        files,
        json!({"folder": "/moved", "media": [ids[1], ids[2]]}),
    )
    .unwrap();
    assert_eq!(out["relinked"].as_array().unwrap().len(), 2);
    assert_eq!(out["still_missing"], json!([]));
    assert_eq!(path_of(&h, ids[3]), "/old/clip03.mp4");
}

#[test]
fn relink_folder_that_finds_nothing_changes_nothing() {
    let mut h = H::empty();
    let m = h.add_video("/old/a.mp4", 10, true);
    let commits = h.commits();
    let out = engine(
        &h,
        "media.relink_folder",
        FakeProbe::new(),
        MemoryFiles::new().with_folder("/empty"),
        json!({"folder": "/empty"}),
    )
    .unwrap();
    assert_eq!(out["relinked"], json!([]));
    assert_eq!(out["still_missing"], json!([m]));
    assert_eq!(h.commits(), commits);
}

#[test]
fn relink_folder_validates_the_folder_and_the_assets() {
    let mut h = H::empty();
    let m = h.add_video("/old/a.mp4", 10, true);
    let run = |h: &H, params: Value| {
        engine(
            h,
            "media.relink_folder",
            FakeProbe::new(),
            MemoryFiles::new().with_folder("/moved"),
            params,
        )
    };
    assert_invalid(run(&h, json!({"folder": "moved"})), "/folder");
    assert_invalid(run(&h, json!({"folder": "/not-there"})), "/folder");
    assert_invalid(
        run(&h, json!({"folder": "/moved", "media": [m, m]})),
        "/media/1",
    );
    assert_code(
        run(
            &h,
            json!({"folder": "/moved", "media": [MediaId::from_u128(999)]}),
        ),
        "not_found",
    );
}

// ----- media.find_missing -------------------------------------------------------------------

#[test]
fn find_missing_lists_the_assets_whose_file_is_gone() {
    let mut h = H::empty();
    let here = h.add_video("/m/here.mp4", 10, true);
    let gone = h.add_audio("/m/gone.wav", 4);
    let out = engine(
        &h,
        "media.find_missing",
        FakeProbe::new(),
        MemoryFiles::new().with_file("/m/here.mp4"),
        json!({}),
    )
    .unwrap();
    assert_eq!(out["checked"], 2);
    let missing = out["missing"].as_array().unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0]["media_id"], json!(gone));
    assert_eq!(missing[0]["name"], "gone.wav");
    assert_eq!(missing[0]["path"], "/m/gone.wav");
    assert_eq!(missing[0]["status"], "missing");
    let _ = here;
}

#[test]
fn find_missing_with_verify_reports_changed_files() {
    let mut h = H::empty();
    let same = h.add_video("/m/same.mp4", 10, true);
    let edited = h.add_video("/m/edited.mp4", 10, true);
    let run = |h: &H, params: Value| {
        engine(
            h,
            "media.find_missing",
            FakeProbe::new()
                .with("/m/same.mp4", moved("/m/same.mp4", 10))
                .with(
                    "/m/edited.mp4",
                    FakeProbe::video("/m/other-content.mp4", 10, true),
                ),
            MemoryFiles::new()
                .with_file("/m/same.mp4")
                .with_file("/m/edited.mp4"),
            params,
        )
        .unwrap()
    };
    assert_eq!(run(&h, json!({}))["missing"], json!([]));
    let out = run(&h, json!({"verify": true}));
    let missing = out["missing"].as_array().unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0]["media_id"], json!(edited));
    assert_eq!(missing[0]["status"], "changed");
    let _ = same;
}

#[test]
fn the_relink_commands_are_in_the_mcp_catalog() {
    let h = H::empty();
    for (name, read_only) in [
        ("media.relink", false),
        ("media.relink_folder", false),
        ("media.find_missing", true),
    ] {
        let meta = &h.registry.get(name).unwrap().meta;
        assert_eq!(meta.mcp, McpExposure::Catalog, "{name}");
        assert_eq!(meta.annotations.read_only, read_only, "{name}");
        assert!(!meta.annotations.destructive, "{name}");
        assert!(
            !meta.undoable,
            "{name}: the sink's transaction is the undo step"
        );
    }
}
