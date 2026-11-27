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

/// ADR-0036: relinking changes where an asset's file is, never its tags, rating, note or the
/// collections it is on.
#[test]
fn relinking_keeps_tags_rating_note_and_collections() {
    let mut h = H::empty();
    let m = h.add_video("/old/a.mp4", 10, true);
    let other = h.add_video("/old/b.mp4", 10, true);
    h.ok(
        "media.add_tags",
        json!({"media": [m, other], "tags": ["Café", "keep"]}),
    );
    h.ok("media.set_rating", json!({"media": [m], "rating": 5}));
    h.ok("media.set_note", json!({"media": m, "note": "Best take"}));
    h.ok(
        "media.create_collection",
        json!({"name": "Selects", "kind": {"type": "manual", "media": [other, m]}}),
    );
    let before = h.doc();
    let kept = |h: &H, id: MediaId| {
        let now = &h.doc().media[&id];
        let was = &before.media[&id];
        assert_eq!(
            (&now.tags, now.rating, &now.note),
            (&was.tags, was.rating, &was.note)
        );
        assert_eq!(h.doc().collections, before.collections);
    };

    engine(
        &h,
        "media.relink",
        FakeProbe::new().with("/new/a.mp4", moved("/old/a.mp4", 10)),
        MemoryFiles::new().with_file("/new/a.mp4"),
        json!({"media": m, "path": "/new/a.mp4"}),
    )
    .unwrap();
    assert_eq!(path_of(&h, m), "/new/a.mp4");
    kept(&h, m);

    h.ok(
        "media.set_location",
        json!({"media": other, "path": "/new/b.mp4"}),
    );
    assert_eq!(path_of(&h, other), "/new/b.mp4");
    kept(&h, other);

    engine(
        &h,
        "media.relink_folder",
        FakeProbe::new().with("/third/a.mp4", moved("/old/a.mp4", 10)),
        MemoryFiles::new().with_file("/third/a.mp4"),
        json!({"folder": "/third", "media": [m]}),
    )
    .unwrap();
    kept(&h, m);
    assert_eq!(h.doc().media[&m].tags, ["Café", "keep"]);
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

// ----- image sequences (#208) ---------------------------------------------------------------

/// The pictures `f_0001.png` … of a run of `count` numbers in `dir`, without the numbers in
/// `absent`. Each has the content of the same picture in `content_of` (the folder it was
/// imported from), so a moved run keeps its hash.
fn pictures(dir: &str, content_of: &str, count: u32, absent: &[u32]) -> FakeProbe {
    (1..=count)
        .filter(|n| !absent.contains(n))
        .fold(FakeProbe::new(), |probe, n| {
            let name = format!("f_{n:04}.png");
            probe.with(
                &format!("{dir}/{name}"),
                FakeProbe::image(&format!("{content_of}/{name}")),
            )
        })
}

/// The same pictures as files on the disk.
fn picture_files(disk: MemoryFiles, dir: &str, count: u32, absent: &[u32]) -> MemoryFiles {
    (1..=count)
        .filter(|n| !absent.contains(n))
        .fold(disk, |disk, n| {
            disk.with_file(&format!("{dir}/f_{n:04}.png"))
        })
}

/// Imports the run `/old/r/f_0001.png` … `f_0006.png` (number 3 has no file) as one asset.
fn add_run(h: &H) -> MediaId {
    let mut ctx = h.import_ctx(pictures("/old/r", "/old/r", 6, &[3]));
    let out = h
        .registry
        .invoke_engine_json(
            "media.import_sequence",
            json!({"path": "/old/r"}),
            &mut ctx,
            Access::STANDARD,
        )
        .unwrap();
    assert_eq!(out["image_sequence"]["missing"], json!([3]));
    id(&out["media_id"])
}

#[test]
fn find_missing_tells_a_missing_run_from_an_incomplete_one() {
    let h = H::empty();
    let run = add_run(&h);
    let find = |disk: MemoryFiles, verify: bool| {
        engine(
            &h,
            "media.find_missing",
            pictures("/old/r", "/old/r", 6, &[3]),
            disk,
            json!({"verify": verify}),
        )
        .unwrap()
    };
    // Every picture that had a file is there (number 3 never had one): nothing to report,
    // and reading the run finds its content unchanged.
    for verify in [false, true] {
        let out = find(picture_files(MemoryFiles::new(), "/old/r", 6, &[3]), verify);
        assert_eq!(out["missing"], json!([]), "verify {verify}");
        assert_eq!(out["checked"], 1);
    }
    // The first picture is there, two others are gone.
    let out = find(
        picture_files(MemoryFiles::new(), "/old/r", 6, &[3, 4, 6]),
        false,
    );
    assert_eq!(
        out["missing"],
        json!([{"media_id": run, "name": "f_0001.png", "path": "/old/r/f_0001.png",
                "status": "incomplete", "missing_frames": 2}])
    );
    // The first picture is gone: the asset is missing, whatever else is there.
    let out = find(
        picture_files(MemoryFiles::new(), "/old/r", 6, &[1, 3]),
        false,
    );
    assert_eq!(out["missing"][0]["status"], "missing");
    assert_eq!(out["missing"][0]["missing_frames"], 1);
    let out = find(MemoryFiles::new().with_folder("/old/r"), false);
    assert_eq!(out["missing"][0]["status"], "missing");
    assert_eq!(out["missing"][0]["missing_frames"], 5);
}

#[test]
fn find_missing_leaves_missing_frames_out_for_other_media() {
    let mut h = H::empty();
    h.add_video("/old/a.mp4", 10, true);
    let out = engine(
        &h,
        "media.find_missing",
        FakeProbe::new(),
        MemoryFiles::new(),
        json!({}),
    )
    .unwrap();
    assert_eq!(out["missing"][0]["status"], "missing");
    assert_eq!(out["missing"][0]["missing_frames"], Value::Null);
}

#[test]
fn relink_moves_a_run_from_any_of_its_pictures_or_its_folder() {
    for path in ["/new/r/f_0005.png", "/new/r/f_0001.png", "/new/r"] {
        let h = H::empty();
        let run = add_run(&h);
        let hash = h.doc().media[&run].content_hash.clone();
        let commits = h.commits();
        let out = engine(
            &h,
            "media.relink",
            pictures("/new/r", "/old/r", 6, &[3]),
            picture_files(MemoryFiles::new(), "/new/r", 6, &[3]),
            json!({"media": run, "path": path}),
        )
        .unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(out["from"], "/old/r/f_0001.png", "{path}");
        assert_eq!(out["to"], "/new/r/f_0001.png", "{path}");
        assert_eq!(out["matched_by"], "hash", "{path}");
        assert_eq!(path_of(&h, run), "/new/r/f_0001.png");
        assert_eq!(h.doc().media[&run].content_hash, hash);
        assert_eq!(h.commits(), commits + 1, "one undo step");
        assert_eq!(h.doc().violations(), vec![]);
    }
}

#[test]
fn relink_refuses_a_run_that_is_not_all_there_and_files_that_are_not_of_it() {
    let h = H::empty();
    let run = add_run(&h);
    let commits = h.commits();
    // Picture 5 did not come along.
    let err = engine(
        &h,
        "media.relink",
        pictures("/new/r", "/old/r", 6, &[3, 5]),
        picture_files(MemoryFiles::new(), "/new/r", 6, &[3, 5]),
        json!({"media": run, "path": "/new/r/f_0001.png"}),
    )
    .unwrap_err();
    assert!(err.message().contains("1 of the pictures"), "{err}");
    assert_invalid(Err(err), "/path");
    // A file that is not a picture of the run: another name, a number outside it, the number
    // written another way.
    let disk = picture_files(MemoryFiles::new(), "/new/r", 6, &[3])
        .with_file("/new/r/cover.png")
        .with_file("/new/r/f_0007.png")
        .with_file("/new/r/f_1.png");
    for path in ["/new/r/cover.png", "/new/r/f_0007.png", "/new/r/f_1.png"] {
        assert_invalid(
            engine(
                &h,
                "media.relink",
                pictures("/new/r", "/old/r", 6, &[3]),
                disk.clone(),
                json!({"media": run, "path": path}),
            ),
            "/path",
        );
    }
    // An empty folder.
    assert_invalid(
        engine(
            &h,
            "media.relink",
            FakeProbe::new(),
            MemoryFiles::new().with_folder("/empty"),
            json!({"media": run, "path": "/empty"}),
        ),
        "/path",
    );
    assert_eq!(h.commits(), commits);
    assert_eq!(path_of(&h, run), "/old/r/f_0001.png");
}

#[test]
fn relink_takes_other_pictures_with_the_names_of_the_run() {
    let h = H::empty();
    let run = add_run(&h);
    let old = h.doc().media[&run].clone();
    // A re-render: the same names and numbers, other content.
    let out = engine(
        &h,
        "media.relink",
        pictures("/v2/r", "/v2/r", 6, &[]),
        picture_files(MemoryFiles::new(), "/v2/r", 6, &[]),
        json!({"media": run, "path": "/v2/r"}),
    )
    .unwrap();
    assert_eq!(out["matched_by"], "chosen");
    let asset = h.doc().media[&run].clone();
    assert_eq!(asset.path_abs.as_deref(), Some("/v2/r/f_0001.png"));
    assert_ne!(asset.content_hash, old.content_hash);
    assert_eq!(asset.probe.duration, old.probe.duration);
    assert_eq!(asset.image_sequence, old.image_sequence);
    assert_eq!(h.doc().violations(), vec![]);
}

#[test]
fn relink_folder_finds_a_moved_run_by_its_content_in_one_undo_step() {
    let mut h = H::empty();
    let clip = h.add_video("/old/a.mp4", 10, true);
    let run = add_run(&h);
    let commits = h.commits();
    // The run moved two folders down; a copy with a picture gone and a re-render are there too.
    let prober =
        pictures("/new/shots/r", "/old/r", 6, &[3]).with("/new/a.mp4", moved("/old/a.mp4", 10));
    let prober = [1, 2, 4, 6].iter().fold(prober, |p, n| {
        let name = format!("f_{n:04}.png");
        p.with(
            &format!("/new/broken/{name}"),
            FakeProbe::image(&format!("/old/r/{name}")),
        )
    });
    let prober = (1..=6).fold(prober, |p, n| {
        let path = format!("/new/another/f_{n:04}.png");
        p.with(&path, FakeProbe::image(&path))
    });
    let disk = picture_files(MemoryFiles::new(), "/new/shots/r", 6, &[3]);
    let disk = picture_files(disk, "/new/broken", 6, &[3, 5]);
    let disk = picture_files(disk, "/new/another", 6, &[]).with_file("/new/a.mp4");
    let out = engine(
        &h,
        "media.relink_folder",
        prober,
        disk,
        json!({"folder": "/new"}),
    )
    .unwrap();
    assert_eq!(out["still_missing"], json!([]));
    let of = |id: MediaId| {
        out["relinked"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["media_id"] == json!(id))
            .unwrap()
            .clone()
    };
    assert_eq!(of(run)["to"], "/new/shots/r/f_0001.png");
    assert_eq!(of(run)["matched_by"], "hash");
    assert_eq!(of(clip)["to"], "/new/a.mp4");
    assert_eq!(path_of(&h, run), "/new/shots/r/f_0001.png");
    assert_eq!(h.commits(), commits + 1, "one undo step for both");
    assert_eq!(h.doc().violations(), vec![]);
}

#[test]
fn relink_folder_takes_the_one_complete_run_with_the_names_and_passes_over_incomplete_ones() {
    let h = H::empty();
    let run = add_run(&h);
    // Only an incomplete copy: nothing is relinked.
    let out = engine(
        &h,
        "media.relink_folder",
        pictures("/new/broken", "/old/r", 6, &[3, 5]),
        picture_files(MemoryFiles::new(), "/new/broken", 6, &[3, 5]),
        json!({"folder": "/new"}),
    )
    .unwrap();
    assert_eq!(out["relinked"], json!([]));
    assert_eq!(out["still_missing"], json!([run]));
    // One complete run with other content (a re-render), in the searched folder itself.
    let out = engine(
        &h,
        "media.relink_folder",
        pictures("/v2", "/v2", 6, &[]),
        picture_files(MemoryFiles::new(), "/v2", 6, &[]),
        json!({"folder": "/v2", "recursive": false}),
    )
    .unwrap();
    assert_eq!(out["relinked"][0]["to"], "/v2/f_0001.png");
    assert_eq!(out["relinked"][0]["matched_by"], "name");
    assert_eq!(path_of(&h, run), "/v2/f_0001.png");
    assert_eq!(h.doc().violations(), vec![]);

    // Two re-renders: neither is taken.
    let h = H::empty();
    let run = add_run(&h);
    let prober = (1..=6).fold(pictures("/n/a", "/n/a", 6, &[]), |p, n| {
        let path = format!("/n/b/f_{n:04}.png");
        p.with(&path, FakeProbe::image(&path))
    });
    let disk = picture_files(MemoryFiles::new(), "/n/a", 6, &[]);
    let out = engine(
        &h,
        "media.relink_folder",
        prober,
        picture_files(disk, "/n/b", 6, &[]),
        json!({"folder": "/n"}),
    )
    .unwrap();
    assert_eq!(out["relinked"], json!([]));
    assert_eq!(out["still_missing"], json!([run]));
}

#[test]
fn relink_folder_completes_a_run_whose_first_picture_is_still_there() {
    let h = H::empty();
    let run = add_run(&h);
    // At its old place the run lost a picture; the whole run is also in `/backup/r`.
    let disk = picture_files(MemoryFiles::new(), "/old/r", 6, &[3, 4]);
    let disk = picture_files(disk, "/backup/r", 6, &[3]);
    let out = engine(
        &h,
        "media.relink_folder",
        pictures("/backup/r", "/old/r", 6, &[3]),
        disk,
        json!({"folder": "/backup"}),
    )
    .unwrap();
    assert_eq!(out["relinked"][0]["to"], "/backup/r/f_0001.png");
    assert_eq!(out["relinked"][0]["matched_by"], "hash");
    assert_eq!(path_of(&h, run), "/backup/r/f_0001.png");
}
