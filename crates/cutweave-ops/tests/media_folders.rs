// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.07 (ADR-0020): the media folder commands and queries. Every successful call runs
//! through `H::run`, which asserts the undo/redo identity and `Project::validate`; every
//! failure, that nothing changed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, assert_code, assert_invalid, id};
use cutweave_commands::Access;
use cutweave_model::{FolderId, Item, MAX_FOLDER_DEPTH, MediaId, Project, TrackId};
use serde_json::{Value, json};

fn create(h: &mut H, parent: Option<FolderId>, name: &str) -> FolderId {
    let out = h.ok(
        "media.create_folder",
        json!({"parent": parent, "name": name}),
    );
    id(&out["folder_id"])
}

fn folder_of(h: &H, media: MediaId) -> Option<FolderId> {
    h.doc().media.get(&media).unwrap().folder
}

fn parent_of(h: &H, folder: FolderId) -> Option<FolderId> {
    h.doc().folders.get(&folder).unwrap().parent
}

fn name_of(h: &H, folder: FolderId) -> String {
    h.doc().folders.get(&folder).unwrap().name.clone()
}

fn timelines(doc: &Project) -> Vec<(TrackId, Item)> {
    let mut v: Vec<_> = doc
        .sequences
        .values()
        .flat_map(|s| s.tracks.values())
        .flat_map(|t| t.items.iter().map(|i| (t.id, i.clone())))
        .collect();
    v.sort_by_key(|(_, i)| i.id());
    v
}

#[test]
fn create_folder_names_nests_and_resolves_clashes() {
    let mut h = H::empty();
    let a = h.ok("media.create_folder", json!({}));
    assert_eq!(a["name"], "New folder");
    assert_eq!(a["parent"], Value::Null);
    let b = h.ok("media.create_folder", json!({}));
    assert_eq!(b["name"], "New folder 2");
    let broll = create(&mut h, None, "  B-roll  ");
    assert_eq!(name_of(&h, broll), "B-roll");
    let drone = create(&mut h, Some(broll), "Drone");
    assert_eq!(parent_of(&h, drone), Some(broll));
    assert_eq!(h.doc().folder_path(drone).as_deref(), Some("B-roll/Drone"));

    // A clash ignoring case fails by default and renames on request.
    assert_invalid(
        h.run("media.create_folder", json!({"name": "b-ROLL"})),
        "/name",
    );
    let renamed = h.ok(
        "media.create_folder",
        json!({"name": "b-ROLL", "on_conflict": "rename"}),
    );
    assert_eq!(renamed["name"], "b-ROLL 2");
    // The same name in another parent is fine.
    create(&mut h, Some(drone), "B-roll");

    for bad in ["", "  ", "a/b", "tab\there", &"x".repeat(256)] {
        assert_invalid(h.run("media.create_folder", json!({"name": bad})), "/name");
    }
    assert_code(
        h.run(
            "media.create_folder",
            json!({"parent": FolderId::from_u128(7)}),
        ),
        "not_found",
    );
}

#[test]
fn folders_nest_at_most_32_levels() {
    let mut h = H::empty();
    let mut parent = None;
    for level in 1..=MAX_FOLDER_DEPTH {
        parent = Some(create(&mut h, parent, &format!("L{level}")));
    }
    assert_eq!(
        h.doc().folder_depth(parent.unwrap()),
        Some(MAX_FOLDER_DEPTH)
    );
    assert_invalid(
        h.run("media.create_folder", json!({"parent": parent})),
        "/parent",
    );
}

#[test]
fn rename_folder_keeps_sibling_names_unique() {
    let mut h = H::empty();
    let a = create(&mut h, None, "Interviews");
    let b = create(&mut h, None, "Music");
    let out = h.ok(
        "media.rename_folder",
        json!({"folder": a, "name": "INTERVIEWS"}),
    );
    assert_eq!(out["name"], "INTERVIEWS");
    assert_invalid(
        h.run(
            "media.rename_folder",
            json!({"folder": b, "name": "interviews"}),
        ),
        "/name",
    );
    assert_invalid(
        h.run("media.rename_folder", json!({"folder": b, "name": "x/y"})),
        "/name",
    );
    assert_code(
        h.run(
            "media.rename_folder",
            json!({"folder": FolderId::from_u128(9), "name": "Z"}),
        ),
        "not_found",
    );
}

#[test]
fn rename_sets_and_clears_the_display_name() {
    let (mut h, _) = H::project();
    let media = h.add_video("/m/DJI_0042.mp4", 10, false);
    let out = h.ok(
        "media.rename",
        json!({"media": media, "name": " Coastline "}),
    );
    assert_eq!(out["name"], "Coastline");
    assert_eq!(out["display_name"], "Coastline");
    let listed = h.query("media.list", json!({})).unwrap();
    assert_eq!(listed["media"][0]["name"], "Coastline");
    // Text search matches the display name, not the file name any more.
    let found = h
        .query("media.list", json!({"filter": {"text": "coast"}}))
        .unwrap();
    assert_eq!(found["matched"], 1);
    let out = h.ok("media.rename", json!({"media": media}));
    assert_eq!(out["name"], Value::Null);
    assert_eq!(out["display_name"], "DJI_0042.mp4");
    assert_invalid(
        h.run("media.rename", json!({"media": media, "name": "a\nb"})),
        "/name",
    );
    assert_code(
        h.run(
            "media.rename",
            json!({"media": MediaId::from_u128(3), "name": "x"}),
        ),
        "not_found",
    );
}

#[test]
fn move_puts_assets_and_folders_in_place_and_refuses_cycles() {
    let (mut h, _) = H::project();
    let a = h.add_video("/m/a.mp4", 10, false);
    let b = h.add_audio("/m/b.wav", 10);
    let broll = create(&mut h, None, "B-roll");
    let drone = create(&mut h, None, "Drone");
    let day = create(&mut h, Some(drone), "Day 1");

    let out = h.ok(
        "media.move",
        json!({"media": [a, b], "folders": [drone], "to": broll}),
    );
    assert_eq!(out["media"], json!([a, b]));
    assert_eq!(out["folders"], json!([drone]));
    assert_eq!(
        (folder_of(&h, a), folder_of(&h, b)),
        (Some(broll), Some(broll))
    );
    assert_eq!(
        h.doc().folder_path(day).as_deref(),
        Some("B-roll/Drone/Day 1")
    );
    // Already there: nothing to report.
    let again = h.ok("media.move", json!({"media": [a], "to": broll}));
    assert_eq!(again["media"], json!([]));

    // Into itself or a subfolder: Invalid at the folder.
    assert_invalid(
        h.run("media.move", json!({"folders": [drone], "to": drone})),
        "/folders/0",
    );
    assert_invalid(
        h.run("media.move", json!({"folders": [broll], "to": day})),
        "/folders/0",
    );
    // To the top level.
    h.ok("media.move", json!({"media": [b], "folders": [day]}));
    assert_eq!(folder_of(&h, b), None);
    assert_eq!(parent_of(&h, day), None);

    assert_invalid(h.run("media.move", json!({})), "/media");
    assert_invalid(h.run("media.move", json!({"media": [a, a]})), "/media/1");
    assert_code(
        h.run(
            "media.move",
            json!({"media": [a], "to": FolderId::from_u128(1)}),
        ),
        "not_found",
    );
    assert_code(
        h.run("media.move", json!({"folders": [FolderId::from_u128(1)]})),
        "not_found",
    );
}

#[test]
fn move_fails_or_renames_on_a_name_clash() {
    let mut h = H::empty();
    let dest = create(&mut h, None, "Shoot");
    let existing = create(&mut h, Some(dest), "Day 1");
    let one = create(&mut h, None, "day 1");
    let other = create(&mut h, None, "Other");
    let two = create(&mut h, Some(other), "DAY 1");
    assert_invalid(
        h.run("media.move", json!({"folders": [one], "to": dest})),
        "/folders/0",
    );
    let out = h.ok(
        "media.move",
        json!({"folders": [one, two], "to": dest, "on_conflict": "rename"}),
    );
    assert_eq!(
        out["renamed"],
        json!([
            {"folder": one, "from": "day 1", "to": "day 1 2"},
            {"folder": two, "from": "DAY 1", "to": "DAY 1 3"},
        ])
    );
    assert_eq!(name_of(&h, existing), "Day 1");
}

#[test]
fn move_keeps_the_depth_limit() {
    let mut h = H::empty();
    // A chain of 30 and a separate subtree two levels tall.
    let mut deep = None;
    for level in 1..=30 {
        deep = Some(create(&mut h, deep, &format!("L{level}")));
    }
    let top = create(&mut h, None, "Top");
    create(&mut h, Some(top), "Inner");
    // At depth 31 and 32 it fits exactly; one more level does not.
    h.ok("media.move", json!({"folders": [top], "to": deep}));
    let deeper = create(&mut h, deep, "L31");
    assert_invalid(
        h.run("media.move", json!({"folders": [top], "to": deeper})),
        "/folders/0",
    );
}

/// The WBS fixture: 30 assets in a folder tree, 5 of them on a timeline.
#[test]
fn delete_folder_remove_keeps_used_media_and_one_undo_restores_everything() {
    let (mut h, p) = H::project();
    let shoot = create(&mut h, None, "Shoot");
    let day1 = create(&mut h, Some(shoot), "Day 1");
    let day2 = create(&mut h, Some(shoot), "Day 2");
    let deep = create(&mut h, Some(day2), "Selects");
    let elsewhere = create(&mut h, None, "Elsewhere");
    let homes = [shoot, day1, day2, deep];
    let mut assets = Vec::new();
    for n in 0..30 {
        let media = h.add_video(&format!("/m/clip_{n:02}.mp4"), 4, false);
        h.ok("media.move", json!({"media": [media], "to": homes[n % 4]}));
        assets.push(media);
    }
    let outside = h.add_audio("/m/outside.wav", 3);
    h.ok("media.move", json!({"media": [outside], "to": elsewhere}));
    let used: Vec<MediaId> = assets.iter().copied().step_by(6).collect();
    assert_eq!(used.len(), 5);
    for (n, media) in used.iter().enumerate() {
        h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": media,
                   "at": format!("{}s", n * 5), "duration": "2s"}),
        );
    }
    let before = h.doc();
    let out = h.ok(
        "media.delete_folder",
        json!({"folder": shoot, "contents": "remove"}),
    );
    let after = h.doc();
    assert_eq!(out["removed"].as_array().unwrap().len(), 25);
    let kept: Vec<MediaId> = serde_json::from_value(out["kept"].clone()).unwrap();
    assert_eq!(kept, used);
    assert_eq!(out["removed_folders"], json!([shoot, day1, day2, deep]));
    assert_eq!(after.media.len(), 6);
    for media in &used {
        assert_eq!(
            after.media.get(media).unwrap().folder,
            None,
            "kept at the top level"
        );
    }
    assert_eq!(after.media.get(&outside).unwrap().folder, Some(elsewhere));
    assert_eq!(after.folders.len(), 1);
    assert_eq!(timelines(&before), timelines(&after), "no timeline changed");

    // One undo restores the tree, the 25 assets and their folders exactly.
    let undo = h.last.as_ref().unwrap().inverse.clone();
    let restored = cutweave_commands::apply_patch(&after, &undo).unwrap();
    assert_eq!(restored, before);
}

#[test]
fn delete_folder_move_up_lifts_contents_one_level() {
    let (mut h, _) = H::project();
    let parent = create(&mut h, None, "Parent");
    let gone = create(&mut h, Some(parent), "Gone");
    let sub = create(&mut h, Some(gone), "Day 1");
    let clash = create(&mut h, Some(gone), "Keep");
    create(&mut h, Some(parent), "keep");
    let media = h.add_image("/m/still.png");
    h.ok("media.move", json!({"media": [media], "to": gone}));
    let out = h.ok(
        "media.delete_folder",
        json!({"folder": gone, "contents": "move_up"}),
    );
    assert_eq!(out["parent"], json!(parent));
    assert_eq!(out["removed_folders"], json!([gone]));
    assert_eq!(out["kept"], json!([media]));
    assert_eq!(out["moved_folders"], json!([sub, clash]));
    assert_eq!(
        out["renamed"],
        json!([{"folder": clash, "from": "Keep", "to": "Keep 2"}])
    );
    assert_eq!(folder_of(&h, media), Some(parent));
    assert_eq!(parent_of(&h, sub), Some(parent));
    assert!(!h.doc().folders.contains_key(&gone));
    assert_code(
        h.run(
            "media.delete_folder",
            json!({"folder": gone, "contents": "move_up"}),
        ),
        "not_found",
    );
}

#[test]
fn list_filters_by_folder_one_level_recursive_or_top_level() {
    let (mut h, _) = H::project();
    let top = h.add_image("/m/top.png");
    let broll = create(&mut h, None, "B-roll");
    let drone = create(&mut h, Some(broll), "Drone");
    let a = h.add_image("/m/a.png");
    let b = h.add_image("/m/b.png");
    h.ok("media.move", json!({"media": [a], "to": broll}));
    h.ok("media.move", json!({"media": [b], "to": drone}));
    let ids = |filter: Value| -> Vec<MediaId> {
        let out = h.query("media.list", json!({ "filter": filter })).unwrap();
        out["media"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| id(&m["id"]))
            .collect()
    };
    let mut all = vec![top, a, b];
    all.sort();
    assert_eq!(ids(json!({})), all);
    assert_eq!(ids(json!({"recursive": false})), [top]);
    assert_eq!(ids(json!({"folder": broll})), [a]);
    let mut both = vec![a, b];
    both.sort();
    assert_eq!(ids(json!({"folder": broll, "recursive": true})), both);
    assert_eq!(ids(json!({"folder": drone, "recursive": true})), [b]);
    let out = h
        .query("media.list", json!({"filter": {"folder": drone}}))
        .unwrap();
    assert_eq!(out["media"][0]["folder"], json!(drone));
    assert_eq!(out["media"][0]["name"], "b.png");
    assert_code(
        h.query(
            "media.list",
            json!({"filter": {"folder": FolderId::from_u128(5)}}),
        ),
        "not_found",
    );
}

#[test]
fn list_folders_returns_the_tree_with_paths_and_counts() {
    let (mut h, _) = H::project();
    let music = create(&mut h, None, "music");
    let broll = create(&mut h, None, "B-roll");
    let drone = create(&mut h, Some(broll), "Drone");
    let a = h.add_image("/m/a.png");
    let b = h.add_image("/m/b.png");
    h.add_image("/m/c.png");
    h.ok("media.move", json!({"media": [a, b], "to": drone}));
    let out = h.query("media.list_folders", json!({})).unwrap();
    assert_eq!(
        out,
        json!({
            "folders": [
                {"id": broll, "name": "B-roll", "parent": null, "path": "B-roll", "depth": 1,
                 "media_count": 0, "folder_count": 1},
                {"id": drone, "name": "Drone", "parent": broll, "path": "B-roll/Drone",
                 "depth": 2, "media_count": 2, "folder_count": 0},
                {"id": music, "name": "music", "parent": null, "path": "music", "depth": 1,
                 "media_count": 0, "folder_count": 0},
            ],
            "top_level_media": 1,
            "total_media": 3,
        })
    );
}

#[test]
fn register_and_import_put_new_media_in_a_folder() {
    let (mut h, _) = H::project();
    let folder = create(&mut h, None, "Imports");
    let probe = cutweave_ops::media::FakeProbe::video("/m/new.mp4", 5, true);
    let out = h.ok(
        "media.register",
        json!({"path": "/m/new.mp4", "probe": probe.probe,
               "content_hash": probe.content_hash, "folder": folder}),
    );
    let media: MediaId = id(&out["media_id"]);
    assert_eq!(folder_of(&h, media), Some(folder));
    // A duplicate stays where it is.
    let again = h.ok(
        "media.register",
        json!({"path": "/m/new.mp4", "probe": probe.probe,
               "content_hash": probe.content_hash}),
    );
    assert_eq!(again["created"], false);
    assert_eq!(folder_of(&h, media), Some(folder));
    assert_code(
        h.run(
            "media.register",
            json!({"path": "/m/other.mp4", "probe": probe.probe, "content_hash": "xxh3:1",
                   "folder": FolderId::from_u128(4)}),
        ),
        "not_found",
    );

    let prober = cutweave_ops::media::FakeProbe::new().with(
        "/m/imported.wav",
        cutweave_ops::media::FakeProbe::audio("/m/imported.wav", 3),
    );
    let mut ctx = h.import_ctx(prober);
    let out = h
        .registry
        .invoke_engine_json(
            "media.import",
            json!({"paths": ["/m/imported.wav"], "folder": folder}),
            &mut ctx,
            Access::STANDARD,
        )
        .unwrap();
    let imported: MediaId = id(&out["media_ids"][0]);
    assert_eq!(folder_of(&h, imported), Some(folder));
}

#[test]
fn folder_ids_are_minted_by_the_engine_generator() {
    let mut h = H::empty();
    let first = create(&mut h, None, "A");
    assert!(first.to_string().starts_with("fld_"));
    let mut again = H::empty();
    assert_eq!(create(&mut again, None, "A"), first, "deterministic ids");
}
