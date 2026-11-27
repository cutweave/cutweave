// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.06a (ADR-0036): collections of media. Every successful call runs through `H::run`,
//! which asserts the undo/redo identity and `Project::validate`; every failure, that nothing
//! changed.
//!
//! Pass criteria of the ADR covered here: the smart collection "video, longer than 60 s, tag
//! interview, unused" returns exactly the expected set and follows an edit (the query answers
//! well inside 100 ms); a manual collection with a member that no longer exists opens, lists
//! and edits without error; removing an asset takes it off every manual collection in the same
//! undo step.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::many_single_char_names
)]

mod common;

use std::time::Instant;

use common::{H, P, assert_code, assert_invalid, id};
use cutweave_commands::apply_patch;
use cutweave_model::{
    Collection, CollectionId, CollectionKind, MediaId, OpenOutcome, Project, open_str,
    to_document_string,
};
use serde_json::{Value, json};

const SEC: i64 = 705_600_000;

fn manual(h: &mut H, name: &str, media: &[MediaId]) -> CollectionId {
    let out = h.ok(
        "media.create_collection",
        json!({"name": name, "kind": {"type": "manual", "media": media}}),
    );
    id(&out["collection_id"])
}

fn smart(h: &mut H, name: &str, filter: Value) -> CollectionId {
    let out = h.ok(
        "media.create_collection",
        json!({"name": name, "kind": {"type": "smart", "filter": filter}}),
    );
    id(&out["collection_id"])
}

fn stored(h: &H, collection: CollectionId) -> Vec<MediaId> {
    h.doc().collections[&collection].manual_members().to_vec()
}

/// The members as `media.list` returns them.
fn listed(h: &H, collection: CollectionId) -> Vec<MediaId> {
    let out = h
        .query(
            "media.list",
            json!({"filter": {"collection": collection}, "limit": 500}),
        )
        .unwrap();
    out["media"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| id(&m["id"]))
        .collect()
}

fn summaries(h: &H) -> Value {
    h.query("media.list_collections", json!({})).unwrap()["collections"].clone()
}

/// Puts five seconds of `media` on the video track, in slot `n` (ten seconds apart).
fn place(h: &mut H, p: P, media: MediaId, n: usize) -> Value {
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": media,
               "at": format!("{}s", n * 10), "duration": "5s", "link": false}),
    )
}

fn three(h: &mut H) -> (MediaId, MediaId, MediaId) {
    (
        h.add_video("/m/a.mp4", 10, true),
        h.add_video("/m/b.mp4", 20, false),
        h.add_audio("/m/c.wav", 30),
    )
}

#[test]
fn create_rename_and_delete_keep_the_document_tidy() {
    let mut h = H::empty();
    let (a, b, _) = three(&mut h);
    assert!(
        !to_document_string(&h.doc())
            .unwrap()
            .contains("collections")
    );
    assert_eq!(summaries(&h), json!([]));

    let out = h.ok(
        "media.create_collection",
        json!({"name": "  Selects  ", "kind": {"type": "manual"}}),
    );
    assert_eq!(out["name"], "Selects");
    let selects: CollectionId = id(&out["collection_id"]);
    assert!(selects.to_string().starts_with("col_"));
    let best = manual(&mut h, "best takes", &[b, a]);
    assert_eq!(stored(&h, best), [b, a]);

    // Names are unique ignoring case; a collection may change the case of its own.
    assert_invalid(
        h.run(
            "media.create_collection",
            json!({"name": "SELECTS", "kind": {"type": "manual"}}),
        ),
        "/name",
    );
    assert_invalid(
        h.run(
            "media.rename_collection",
            json!({"collection": best, "name": "selects"}),
        ),
        "/name",
    );
    let out = h.ok(
        "media.rename_collection",
        json!({"collection": best, "name": " Best Takes "}),
    );
    assert_eq!(out, json!({"collection_id": best, "name": "Best Takes"}));
    for bad in ["", "  ", "line\nbreak", &"x".repeat(256)] {
        assert_invalid(
            h.run(
                "media.rename_collection",
                json!({"collection": best, "name": bad}),
            ),
            "/name",
        );
    }

    assert_eq!(
        summaries(&h),
        json!([
            {"id": best, "name": "Best Takes", "kind": "manual", "filter": null, "media_count": 2},
            {"id": selects, "name": "Selects", "kind": "manual", "filter": null, "media_count": 0},
        ])
    );

    // Deleting never touches the assets; the last one takes the map out of the file.
    h.ok("media.delete_collection", json!({"collection": best}));
    assert_eq!(h.doc().media.len(), 3);
    h.ok("media.delete_collection", json!({"collection": selects}));
    assert!(
        !to_document_string(&h.doc())
            .unwrap()
            .contains("collections")
    );
    assert_code(
        h.run("media.delete_collection", json!({"collection": selects})),
        "not_found",
    );
    assert_code(
        h.run(
            "media.rename_collection",
            json!({"collection": selects, "name": "x"}),
        ),
        "not_found",
    );
}

#[test]
fn create_refuses_bad_members_and_filters() {
    let mut h = H::empty();
    let (a, _, _) = three(&mut h);
    let create = |h: &mut H, kind: Value| {
        h.run(
            "media.create_collection",
            json!({"name": "C", "kind": kind}),
        )
    };
    assert_invalid(
        create(&mut h, json!({"type": "manual", "media": [a, a]})),
        "/kind/media/1",
    );
    assert_code(
        create(
            &mut h,
            json!({"type": "manual", "media": [MediaId::from_u128(9)]}),
        ),
        "not_found",
    );
    let other = manual(&mut h, "Other", &[]);
    // No collection of collections.
    assert_invalid(
        create(
            &mut h,
            json!({"type": "smart", "filter": {"collection": other}}),
        ),
        "/kind/filter/collection",
    );
    assert_invalid(
        create(
            &mut h,
            json!({"type": "smart", "filter": {"min_rating": 9}}),
        ),
        "/kind/filter/min_rating",
    );
    assert_invalid(
        create(
            &mut h,
            json!({"type": "smart", "filter": {"min_duration": 9, "max_duration": 1}}),
        ),
        "/kind/filter/min_duration",
    );
    assert_code(
        create(
            &mut h,
            json!({"type": "smart", "filter": {"folder": cutweave_model::FolderId::from_u128(4)}}),
        ),
        "not_found",
    );
    assert!(
        create(
            &mut h,
            json!({"type": "smart", "filter": {"imported_within_days": 3}})
        )
        .is_err()
    );
    assert!(create(&mut h, json!({"type": "playlist"})).is_err());
}

#[test]
fn members_are_added_in_order_and_removed_idempotently() {
    let mut h = H::empty();
    let (a, b, c) = three(&mut h);
    let col = manual(&mut h, "Selects", &[b]);
    let out = h.ok(
        "media.add_to_collection",
        json!({"collection": col, "media": [c, b, a]}),
    );
    assert_eq!(
        out,
        json!({"collection_id": col, "added": [c, a], "members": 3})
    );
    assert_eq!(stored(&h, col), [b, c, a], "the order they were added in");
    assert_eq!(
        listed(&h, col),
        [a, b, c],
        "media.list orders as asked (by id)"
    );
    let commits = h.commits();
    let again = h.ok(
        "media.add_to_collection",
        json!({"collection": col, "media": [a, c]}),
    );
    assert_eq!(again["added"], json!([]));
    assert_eq!(h.commits(), commits + 1);
    assert_eq!(stored(&h, col), [b, c, a]);

    let out = h.ok(
        "media.remove_from_collection",
        json!({"collection": col, "media": [c, MediaId::from_u128(77)]}),
    );
    assert_eq!(
        out,
        json!({"collection_id": col, "removed": [c], "members": 2})
    );
    assert_eq!(stored(&h, col), [b, a]);
    let again = h.ok(
        "media.remove_from_collection",
        json!({"collection": col, "media": [c]}),
    );
    assert_eq!(again["removed"], json!([]));
    assert_eq!(h.doc().media.len(), 3, "the assets stay");

    for name in ["media.add_to_collection", "media.remove_from_collection"] {
        assert_invalid(
            h.run(name, json!({"collection": col, "media": []})),
            "/media",
        );
        assert_invalid(
            h.run(name, json!({"collection": col, "media": [a, a]})),
            "/media/1",
        );
        assert_code(
            h.run(
                name,
                json!({"collection": CollectionId::from_u128(5), "media": [a]}),
            ),
            "not_found",
        );
    }
    assert_code(
        h.run(
            "media.add_to_collection",
            json!({"collection": col, "media": [MediaId::from_u128(77)]}),
        ),
        "not_found",
    );
}

#[test]
fn a_smart_collection_has_a_filter_and_no_stored_members() {
    let mut h = H::empty();
    let (a, b, c) = three(&mut h);
    h.ok("media.add_tags", json!({"media": [a, c], "tags": ["keep"]}));
    let col = smart(&mut h, "Keepers", json!({"tags": [" KEEP "]}));
    assert_eq!(listed(&h, col), [a, c]);
    assert_eq!(
        summaries(&h),
        json!([{"id": col, "name": "Keepers", "kind": "smart",
                "filter": {"tags": ["KEEP"]}, "media_count": 2}])
    );
    for name in ["media.add_to_collection", "media.remove_from_collection"] {
        assert_invalid(
            h.run(name, json!({"collection": col, "media": [b]})),
            "/collection",
        );
    }

    let out = h.ok(
        "media.set_collection_filter",
        json!({"collection": col, "filter": {"kinds": ["video"]}}),
    );
    assert_eq!(out, json!({"collection_id": col, "members": 2}));
    assert_eq!(listed(&h, col), [a, b]);
    assert_invalid(
        h.run(
            "media.set_collection_filter",
            json!({"collection": col, "filter": {"collection": col}}),
        ),
        "/filter/collection",
    );
    assert_invalid(
        h.run(
            "media.set_collection_filter",
            json!({"collection": col, "filter": {"text": "x".repeat(257)}}),
        ),
        "/filter/text",
    );
    let plain = manual(&mut h, "Plain", &[a]);
    assert_invalid(
        h.run(
            "media.set_collection_filter",
            json!({"collection": plain, "filter": {}}),
        ),
        "/collection",
    );

    // The filter of `media.list` narrows a collection further.
    let out = h
        .query(
            "media.list",
            json!({"filter": {"collection": col, "text": "b.mp4"}}),
        )
        .unwrap();
    assert_eq!(out["matched"], 1);
    assert_eq!(out["media"][0]["id"], json!(b));
    assert_code(
        h.query(
            "media.list",
            json!({"filter": {"collection": CollectionId::from_u128(5)}}),
        ),
        "not_found",
    );
}

/// ADR-0036 pass criterion: "video, longer than 60 s, tag interview, unused" returns exactly the
/// expected set on a fixture, and its membership follows an edit.
#[test]
fn the_unused_long_interviews_collection_is_exact_and_follows_edits() {
    let (mut h, p): (H, P) = H::project();
    // (path, seconds, video?, tagged?, used?)
    let specs: [(&str, i64, bool, bool, bool); 8] = [
        ("/m/01 long interview.mp4", 120, true, true, false), // in
        ("/m/02 short interview.mp4", 60, true, true, false), // exactly 60 s: not longer
        ("/m/03 long broll.mp4", 300, true, false, false),    // no tag
        ("/m/04 used interview.mp4", 90, true, true, true),   // on the timeline
        ("/m/05 interview audio.wav", 200, false, true, false), // not video
        ("/m/06 just over.mp4", 61, true, true, false),       // in
        ("/m/07 long interview b.mp4", 600, true, true, false), // in
        ("/m/08 untagged used.mp4", 80, true, false, true),
    ];
    let mut ids = Vec::new();
    for (path, secs, video, tagged, used) in specs {
        let media = if video {
            h.add_video(path, secs, false)
        } else {
            h.add_audio(path, secs)
        };
        if tagged {
            h.ok(
                "media.add_tags",
                json!({"media": [media], "tags": ["Interview"]}),
            );
        }
        if used {
            place(&mut h, p, media, ids.len());
        }
        ids.push(media);
    }
    let col = smart(
        &mut h,
        "Unused long interviews",
        json!({"kinds": ["video"], "min_duration": 60 * SEC + 1, "tags": ["interview"], "used": false}),
    );
    assert_eq!(listed(&h, col), [ids[0], ids[5], ids[6]]);
    assert_eq!(summaries(&h)[0]["media_count"], 3);

    // Edits that change the membership, each seen by the very next query.
    let timed = |h: &H| {
        let t = Instant::now();
        let members = listed(h, col);
        (members, t.elapsed())
    };
    h.ok(
        "media.add_tags",
        json!({"media": [ids[2]], "tags": ["interview"]}),
    );
    let (members, took) = timed(&h);
    assert_eq!(members, [ids[0], ids[2], ids[5], ids[6]], "a tag was added");
    assert!(took.as_millis() < 100, "the query took {took:?}");

    let placed = place(&mut h, p, ids[0], 20);
    assert_eq!(timed(&h).0, [ids[2], ids[5], ids[6]], "an asset got used");
    h.ok(
        "media.remove_tags",
        json!({"media": [ids[5]], "tags": ["INTERVIEW"]}),
    );
    assert_eq!(timed(&h).0, [ids[2], ids[6]], "a tag was removed");
    h.ok("item.delete", json!({"items": placed["clips"]}));
    assert_eq!(
        timed(&h).0,
        [ids[0], ids[2], ids[6]],
        "the clip was deleted"
    );
    h.ok("media.remove", json!({"id": ids[6]}));
    assert_eq!(timed(&h).0, [ids[0], ids[2]], "an asset was removed");
    assert_eq!(summaries(&h)[0]["media_count"], 2);
}

/// A document as an older build may leave it: a manual collection that names an asset the
/// build removed.
fn with_stale_member() -> (H, CollectionId, MediaId, MediaId, MediaId) {
    let mut h = H::empty();
    let (a, b, _) = three(&mut h);
    let gone = MediaId::from_u128(0xdead);
    let col = CollectionId::from_u128(0xc01);
    let mut doc = h.doc();
    doc.collections.insert(
        col,
        Collection::new(
            col,
            "Selects",
            CollectionKind::Manual {
                media: vec![a, gone, b],
            },
        ),
    );
    // It opens: the text round trip yields an editable, valid document.
    let text = to_document_string(&doc).unwrap();
    let OpenOutcome::Editable { project, .. } = open_str(&text).unwrap() else {
        panic!("the document must open for editing");
    };
    assert_eq!(project, doc);
    project.validate().unwrap();
    h.replace_doc(project);
    (h, col, a, b, gone)
}

#[test]
fn a_member_that_no_longer_exists_is_skipped_by_queries() {
    let (h, col, a, b, gone) = with_stale_member();
    assert_eq!(stored(&h, col), [a, gone, b]);
    assert_eq!(listed(&h, col), [a, b]);
    assert_eq!(summaries(&h)[0]["media_count"], 2);
}

#[test]
fn a_member_that_no_longer_exists_is_dropped_by_the_next_edit() {
    // Adding.
    let (mut h, col, a, b, _) = with_stale_member();
    let c = h.add_audio("/m/d.wav", 5);
    let out = h.ok(
        "media.add_to_collection",
        json!({"collection": col, "media": [c]}),
    );
    assert_eq!(out["members"], 3);
    assert_eq!(stored(&h, col), [a, b, c]);

    // Adding an asset that is already on it still tidies the list.
    let (mut h, col, a, b, _) = with_stale_member();
    let out = h.ok(
        "media.add_to_collection",
        json!({"collection": col, "media": [a]}),
    );
    assert_eq!(
        out,
        json!({"collection_id": col, "added": [], "members": 2})
    );
    assert_eq!(stored(&h, col), [a, b]);

    // Removing (also the stale member by name, which is not an error).
    let (mut h, col, a, b, gone) = with_stale_member();
    let out = h.ok(
        "media.remove_from_collection",
        json!({"collection": col, "media": [gone, a]}),
    );
    assert_eq!(
        out,
        json!({"collection_id": col, "removed": [a], "members": 1})
    );
    assert_eq!(stored(&h, col), [b]);

    // Renaming.
    let (mut h, col, a, b, _) = with_stale_member();
    h.ok(
        "media.rename_collection",
        json!({"collection": col, "name": "Picks"}),
    );
    assert_eq!(stored(&h, col), [a, b]);

    // Removing another member's asset.
    let (mut h, col, a, b, _) = with_stale_member();
    h.ok("media.remove", json!({"id": a}));
    assert_eq!(stored(&h, col), [b]);

    // Deleting the collection.
    let (mut h, col, ..) = with_stale_member();
    h.ok("media.delete_collection", json!({"collection": col}));
    assert!(h.doc().collections.is_empty());
}

#[test]
fn removing_an_asset_takes_it_off_every_manual_collection_and_undo_puts_it_back() {
    let mut h = H::empty();
    let (a, b, c) = three(&mut h);
    let first = manual(&mut h, "First", &[a, b, c]);
    let second = manual(&mut h, "Second", &[b]);
    let third = manual(&mut h, "Third", &[c]);
    let videos = smart(&mut h, "Videos", json!({"kinds": ["video"]}));
    let before: Project = h.doc();

    h.ok("media.remove", json!({"id": b}));
    assert_eq!(stored(&h, first), [a, c]);
    assert_eq!(stored(&h, second), Vec::<MediaId>::new());
    assert_eq!(stored(&h, third), [c]);
    assert_eq!(listed(&h, videos), [a]);
    let committed = h.last.clone().unwrap();
    let undone = apply_patch(&committed.doc, &committed.inverse).unwrap();
    assert_eq!(
        undone, before,
        "one undo restores the asset and its memberships"
    );
    assert_eq!(
        undone.collections[&first].manual_members(),
        [a, b, c],
        "at its old position"
    );
}

#[test]
fn deleting_a_folder_with_its_media_takes_them_off_manual_collections() {
    let (mut h, p) = H::project();
    let (a, b, c) = three(&mut h);
    let folder: cutweave_model::FolderId =
        id(&h.ok("media.create_folder", json!({"name": "Day 1"}))["folder_id"]);
    h.ok("media.move", json!({"media": [a, b], "to": folder}));
    // `a` is on a timeline, so the folder's deletion keeps it.
    place(&mut h, p, a, 0);
    let col = manual(&mut h, "Selects", &[a, b, c]);
    let before = h.doc();
    let out = h.ok(
        "media.delete_folder",
        json!({"folder": folder, "contents": "remove"}),
    );
    assert_eq!(out["removed"], json!([b]));
    assert_eq!(stored(&h, col), [a, c]);
    let committed = h.last.clone().unwrap();
    assert_eq!(
        apply_patch(&committed.doc, &committed.inverse).unwrap(),
        before
    );

    // `move_up` removes nothing, so nothing leaves a collection.
    let folder: cutweave_model::FolderId =
        id(&h.ok("media.create_folder", json!({"name": "Day 2"}))["folder_id"]);
    h.ok("media.move", json!({"media": [c], "to": folder}));
    h.ok(
        "media.delete_folder",
        json!({"folder": folder, "contents": "move_up"}),
    );
    assert_eq!(stored(&h, col), [a, c]);
}

#[test]
fn a_smart_collection_whose_folder_is_gone_is_empty_not_an_error() {
    let mut h = H::empty();
    let (a, _, _) = three(&mut h);
    let folder: cutweave_model::FolderId =
        id(&h.ok("media.create_folder", json!({"name": "Day 1"}))["folder_id"]);
    h.ok("media.move", json!({"media": [a], "to": folder}));
    let col = smart(&mut h, "Day one", json!({"folder": folder}));
    assert_eq!(listed(&h, col), [a]);
    h.ok(
        "media.delete_folder",
        json!({"folder": folder, "contents": "move_up"}),
    );
    h.doc().validate().unwrap();
    assert_eq!(listed(&h, col), Vec::<MediaId>::new());
    assert_eq!(summaries(&h)[0]["media_count"], 0);
    // It can be pointed somewhere that exists again.
    h.ok(
        "media.set_collection_filter",
        json!({"collection": col, "filter": {"kinds": ["audio"]}}),
    );
    assert_eq!(listed(&h, col).len(), 1);
}

#[test]
fn limits_a_thousand_collections_and_ten_thousand_members() {
    let mut h = H::empty();
    let (a, _, _) = three(&mut h);
    // Build the full document by hand: a thousand creations through the harness are slow.
    let mut doc = h.doc();
    for n in 0..1000_u128 {
        let cid = CollectionId::from_u128(0x1_0000 + n);
        doc.collections.insert(
            cid,
            Collection::new(cid, format!("C{n}"), CollectionKind::manual()),
        );
    }
    let full = CollectionId::from_u128(0x1_0000);
    let members: Vec<MediaId> = (0..10_000_u128)
        .map(|n| MediaId::from_u128(0x9_0000 + n))
        .collect();
    doc.collections.get_mut(&full).unwrap().kind = CollectionKind::Manual { media: members };
    doc.validate().unwrap();
    h.replace_doc(doc);
    assert_invalid(
        h.run(
            "media.create_collection",
            json!({"name": "One more", "kind": {"type": "manual"}}),
        ),
        "/name",
    );
    // The 10,000 stored members name no asset, so an edit drops them and there is room again.
    let out = h.ok(
        "media.add_to_collection",
        json!({"collection": full, "media": [a]}),
    );
    assert_eq!(out["members"], 1);
    let too_many: Vec<MediaId> = (0..10_001_u128).map(MediaId::from_u128).collect();
    assert_invalid(
        h.run(
            "media.add_to_collection",
            json!({"collection": full, "media": too_many}),
        ),
        "/media",
    );
}
