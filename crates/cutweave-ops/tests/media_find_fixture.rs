// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1M.12, WP-2F.06a: the shared media-search fixture. The Rust predicate and sort (`media.list`) are the
//! reference; this test builds the documents, runs every case through the engine's query and
//! checks the result against `tests/fixtures/media-find.json`, which the editor's TypeScript
//! predicate is tested against (`packages/editor/src/media/find.test.ts`).
//! `CUTWEAVE_BLESS=1 cargo test -p cutweave-ops --test media_find_fixture` rewrites the file
//! after an intended change.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::bool_to_int_with_if
)]

mod common;

use std::{fs, path::PathBuf};

use common::H;
use cutweave_model::{
    Clip, Collection, CollectionId, CollectionKind, Duration, FolderId, FrameRate, MediaAsset,
    MediaFolder, MediaId, MediaKind, ProbeInfo, Project, SampleRate, Sequence, SequenceFormat,
    SequenceId, Time, Track, TrackId, TrackKind,
};
use cutweave_ops::media::FakeProbe;
use serde_json::{Value, json};

const SEC: i64 = 705_600_000;

/// One asset of a fixture document.
struct Spec {
    n: u128,
    kind: MediaKind,
    path_abs: Option<&'static str>,
    path_rel: Option<&'static str>,
    /// Length in flicks (`None`: a still, or unknown).
    duration: Option<i64>,
    /// No streams: nothing can be read from the file.
    broken: bool,
    /// The display name (ADR-0020); `None`: the file name.
    name: Option<&'static str>,
    /// The bin folder, by number (`folder_id`); `None`: the top level.
    folder: Option<u128>,
    /// Tags, in stored order (ADR-0036).
    tags: &'static [&'static str],
    rating: Option<u8>,
    note: Option<&'static str>,
}

/// A collection of a fixture document: number, name and what it holds (the stored JSON of
/// `Collection::kind`, with members and folders by number).
type CollectionSpec = (u128, &'static str, Value);

/// A fixture document: its assets, the media each sequence uses, its folders and collections.
type Doc = (
    Vec<Spec>,
    Vec<Vec<u128>>,
    Vec<FolderSpec>,
    Vec<CollectionSpec>,
);

/// A folder of a fixture document: number, name and the number of its parent.
type FolderSpec = (u128, &'static str, Option<u128>);

const fn spec(n: u128, kind: MediaKind, path: &'static str, duration: Option<i64>) -> Spec {
    Spec {
        n,
        kind,
        path_abs: Some(path),
        path_rel: None,
        duration,
        broken: false,
        name: None,
        folder: None,
        tags: &[],
        rating: None,
        note: None,
    }
}

/// `spec` with tags, a rating and a note.
const fn noted(
    mut s: Spec,
    tags: &'static [&'static str],
    rating: Option<u8>,
    note: Option<&'static str>,
) -> Spec {
    s.tags = tags;
    s.rating = rating;
    s.note = note;
    s
}

/// `spec` with a display name and a folder.
const fn placed(mut s: Spec, name: Option<&'static str>, folder: Option<u128>) -> Spec {
    s.name = name;
    s.folder = folder;
    s
}

fn library() -> (Vec<Spec>, Vec<Vec<u128>>) {
    use MediaKind::{Audio, Image, SequenceRef, Video};
    let mut broken = spec(9, Video, "/m/corrupt.mov", Some(8 * SEC));
    broken.broken = true;
    let mut rel = spec(10, Audio, "C:\\abs\\ignored.wav", Some(61 * SEC));
    rel.path_rel = Some("sounds\\Rain Loop.wav");
    let mut seq = spec(12, SequenceRef, "", None);
    seq.path_abs = None;
    let assets = vec![
        spec(1, Video, "/m/Beach Day.MP4", Some(12 * SEC)),
        spec(2, Audio, "/m/beach-day-music.wav", Some(185 * SEC)),
        spec(3, Video, "/m/Café Interview.mp4", Some(10 * SEC)),
        spec(4, Video, "/m/cafe_broll.mov", Some(10 * SEC - 1)),
        spec(5, Audio, "/m/ÉCLAIR theme.wav", Some(60 * SEC)),
        spec(6, Image, "/m/title card.png", None),
        spec(7, Image, "/m/logo.png", None),
        spec(8, Video, "/m/Naïve Intro.mp4", Some(300 * SEC)),
        broken,
        rel,
        spec(11, Video, "/other/Beach Day.MP4", Some(12 * SEC)),
        seq,
        spec(13, Video, "/m/long-take.mp4", Some(301 * SEC)),
        spec(14, Audio, "/m/ñandú.wav", Some(4 * SEC)),
    ];
    // Two sequences: the first uses 1, 2 and 6; the second 2, 8 and 12.
    (assets, vec![vec![1, 2, 6], vec![2, 8, 12]])
}

fn ties() -> (Vec<Spec>, Vec<Vec<u128>>) {
    use MediaKind::{Audio, Video};
    (
        vec![
            spec(3, Video, "/a/clip.mp4", Some(5 * SEC)),
            spec(1, Video, "/b/clip.mp4", Some(5 * SEC)),
            spec(2, Audio, "/c/clip.wav", Some(5 * SEC)),
            spec(4, Video, "/d/CLIP.mp4", Some(5 * SEC)),
        ],
        vec![vec![]],
    )
}

/// Display names and folders (ADR-0020): Interviews > Day 1, and B-roll, next to the top level.
fn folders() -> Doc {
    use MediaKind::{Audio, Image, Video};
    let assets = vec![
        placed(
            spec(1, Video, "/m/a.mp4", Some(5 * SEC)),
            Some("Opening"),
            Some(1),
        ),
        placed(spec(2, Video, "/m/b.mp4", Some(6 * SEC)), None, Some(2)),
        placed(
            spec(3, Audio, "/m/c.wav", Some(7 * SEC)),
            Some("Thème"),
            None,
        ),
        placed(
            spec(4, Video, "/m/d.mp4", Some(8 * SEC)),
            Some("interview wide"),
            Some(2),
        ),
        placed(spec(5, Image, "/m/e.png", None), None, Some(3)),
        placed(
            spec(6, Video, "/m/f.mp4", Some(9 * SEC)),
            Some("A-roll"),
            Some(3),
        ),
        spec(7, Video, "/m/Opening night.mp4", Some(10 * SEC)),
    ];
    let tree = vec![
        (1, "Interviews", None),
        (2, "Day 1", Some(1)),
        (3, "B-roll", None),
    ];
    (assets, vec![vec![1, 4]], tree, Vec::new())
}

fn with_tree((assets, sequences): (Vec<Spec>, Vec<Vec<u128>>)) -> Doc {
    (assets, sequences, Vec::new(), Vec::new())
}

/// Tags, ratings, notes and collections (ADR-0036): a shoot with interviews and b-roll in two
/// folders, three manual or smart collections, and one smart collection whose folder is gone.
fn tagged() -> Doc {
    use MediaKind::{Audio, Image, Video};
    let assets = vec![
        noted(
            placed(
                spec(1, Video, "/t/A001.mp4", Some(120 * SEC)),
                None,
                Some(1),
            ),
            &["Café", "interview"],
            Some(5),
            Some("Best answer at 01:10. Naïve question first."),
        ),
        noted(
            placed(spec(2, Video, "/t/A002.mp4", Some(60 * SEC)), None, Some(1)),
            &["interview"],
            Some(3),
            None,
        ),
        noted(
            spec(3, Video, "/t/B001 cafe.mp4", Some(300 * SEC)),
            &["b-roll", "Wide"],
            Some(4),
            None,
        ),
        noted(
            spec(4, Video, "/t/A003.mp4", Some(90 * SEC)),
            &["Interview", "wide"],
            None,
            Some("Out of focus\nuse the audio only"),
        ),
        noted(
            spec(5, Audio, "/t/interview room tone.wav", Some(200 * SEC)),
            &["INTERVIEW"],
            Some(1),
            None,
        ),
        noted(
            spec(6, Video, "/t/A004.mp4", Some(61 * SEC)),
            &["interview", "keep"],
            Some(2),
            Some("ÉCLAIR in the background"),
        ),
        noted(
            spec(7, Image, "/t/poster.png", None),
            &["keep"],
            Some(5),
            None,
        ),
        spec(8, Video, "/t/untagged.mp4", Some(600 * SEC)),
        noted(
            placed(
                spec(9, Video, "/t/A005.mp4", Some(601 * SEC)),
                None,
                Some(2),
            ),
            &["interview", "日本語"],
            Some(4),
            Some("wide shot"),
        ),
    ];
    let tree = vec![(1, "Day 1", None), (2, "Day 2", Some(1))];
    let m = |n: u128| media_id(n);
    let collections = vec![
        // A manual list in the order it was built, with a member that names no asset.
        (
            1,
            "Selects",
            json!({"type": "manual", "media": [m(9), m(3), m(99), m(1)]}),
        ),
        (2, "Empty", json!({"type": "manual", "media": []})),
        // ADR-0036's example: video, longer than 60 s, tagged interview, unused.
        (
            3,
            "Unused long interviews",
            json!({"type": "smart", "filter": {"kinds": ["video"], "min_duration": 60 * SEC + 1,
                   "tags": ["interview"], "used": false}}),
        ),
        (
            4,
            "Day 1 and below, rated",
            json!({"type": "smart", "filter": {"folder": folder_id(1), "recursive": true,
                   "min_rating": 4}}),
        ),
        (
            5,
            "Wide in words",
            json!({"type": "smart", "filter": {"text": "wide"}}),
        ),
        // Its folder no longer exists: no members.
        (
            6,
            "Gone folder",
            json!({"type": "smart", "filter": {"folder": folder_id(77)}}),
        ),
    ];
    // Assets 4 and 7 are on a timeline.
    (assets, vec![vec![4, 7]], tree, collections)
}

fn collection_id(n: u128) -> CollectionId {
    CollectionId::from_u128(0x0190_0000_0000_7000_b000_0000_0000_0000 + n)
}

fn folder_id(n: u128) -> FolderId {
    FolderId::from_u128(0x0190_0000_0000_7000_a000_0000_0000_0000 + n)
}

fn media_id(n: u128) -> MediaId {
    MediaId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n)
}

fn build(
    assets: &[Spec],
    sequences: &[Vec<u128>],
    tree: &[FolderSpec],
    collections: &[CollectionSpec],
) -> Project {
    let mut doc = Project::new("Fixture");
    for (n, name, kind) in collections {
        let id = collection_id(*n);
        let kind: CollectionKind = serde_json::from_value(kind.clone()).unwrap();
        doc.collections.insert(id, Collection::new(id, *name, kind));
    }
    for (n, name, parent) in tree {
        let id = folder_id(*n);
        doc.folders
            .insert(id, MediaFolder::new(id, *name, parent.map(folder_id)));
    }
    for s in assets {
        let probe = if s.broken {
            ProbeInfo {
                duration: s.duration.map(|d| Duration::from_flicks(d).unwrap()),
                ..ProbeInfo::default()
            }
        } else {
            let path = s.path_abs.unwrap_or("/x");
            let mut p = match s.kind {
                MediaKind::Audio => FakeProbe::audio(path, 1),
                MediaKind::Image => FakeProbe::image(path),
                _ => FakeProbe::video(path, 1, false),
            }
            .probe;
            p.duration = s.duration.map(|d| Duration::from_flicks(d).unwrap());
            if s.kind == MediaKind::SequenceRef {
                p.streams.clear();
            }
            p
        };
        let mut asset = MediaAsset::file(media_id(s.n), s.kind, s.path_abs.unwrap_or(""), probe);
        asset.path_abs = s.path_abs.map(str::to_owned);
        asset.path_rel = s.path_rel.map(str::to_owned);
        asset.name = s.name.map(str::to_owned);
        asset.folder = s.folder.map(folder_id);
        asset.tags = s.tags.iter().map(|t| (*t).to_owned()).collect();
        asset.rating = s.rating;
        asset.note = s.note.map(str::to_owned);
        if s.kind == MediaKind::SequenceRef {
            // A compound clip of the first sequence, used on the second.
            asset.sequence = Some(SequenceId::from_u128(
                0x0190_0000_0000_7000_9000_0000_0000_0000,
            ));
        }
        doc.media.insert(asset.id, asset);
    }
    let format = SequenceFormat::new(1920, 1080, FrameRate::FPS_30, SampleRate::HZ_48_000);
    for (i, used) in sequences.iter().enumerate() {
        let base = 0x0190_0000_0000_7000_9000_0000_0000_0000 + (i as u128) * 0x1000;
        let mut sequence =
            Sequence::new(SequenceId::from_u128(base), format!("S{i}"), format.clone());
        let mut video = Track::new(TrackId::from_u128(base + 1), TrackKind::Video, "V1");
        let mut audio = Track::new(TrackId::from_u128(base + 2), TrackKind::Audio, "A1");
        for (k, n) in used.iter().enumerate() {
            let item = cutweave_model::ItemId::from_u128(base + 0x10 + k as u128);
            let start = Time::from_secs(k as i64 * 2).unwrap();
            let clip = Clip::new(item, media_id(*n), start, Duration::from_secs(1).unwrap());
            let on_audio = doc.media.get(&media_id(*n)).unwrap().kind == MediaKind::Audio;
            if on_audio {
                audio.items.insert(clip.into());
            } else {
                video.items.insert(clip.into());
            }
        }
        sequence.insert_track(0, video);
        sequence.insert_track(1, audio);
        doc.sequences.insert(sequence.id, sequence);
    }
    doc
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/media-find.json")
}

fn ids_of(out: &Value) -> Vec<Value> {
    out["media"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].clone())
        .collect()
}

/// Every page of `params` at `limit`, following the cursor.
fn pages(h: &H, params: &Value, limit: u64) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let mut p = params.clone();
        p["limit"] = json!(limit);
        if !cursor.is_null() {
            p["cursor"] = cursor.clone();
        }
        let page = h.query("media.list", p).unwrap();
        out.push(ids_of(&page));
        cursor = page["next_cursor"].clone();
        if cursor.is_null() {
            return out;
        }
    }
}

#[allow(clippy::too_many_lines)]
fn cases() -> Vec<(&'static str, Value, Value, Option<u64>)> {
    let mut c: Vec<(&'static str, Value, Value, Option<u64>)> = Vec::new();
    let none = Value::Null;
    let f = |filter: Value| (filter, none.clone());
    let mut add = |doc: &'static str, (filter, sort): (Value, Value), limit: Option<u64>| {
        c.push((doc, filter, sort, limit));
    };
    // The filter alone (default order).
    add("library", f(json!({})), None);
    add("library", f(json!({"text": "beach"})), None);
    add("library", f(json!({"text": "BEACH day"})), None);
    add("library", f(json!({"text": "  day   beach  "})), None);
    add("library", f(json!({"text": "cafe"})), None);
    add("library", f(json!({"text": "café"})), None);
    add("library", f(json!({"text": "ECLAIR"})), None);
    add("library", f(json!({"text": "naive"})), None);
    add("library", f(json!({"text": "nandu"})), None);
    add("library", f(json!({"text": "rain"})), None);
    add("library", f(json!({"text": "ignored"})), None);
    add("library", f(json!({"text": "\\"})), None);
    add("library", f(json!({"text": "zzz"})), None);
    add("library", f(json!({"text": "   "})), None);
    add("library", f(json!({"text": ".mp4"})), None);
    add("library", f(json!({"kinds": ["audio"]})), None);
    add("library", f(json!({"kinds": ["video", "image"]})), None);
    add("library", f(json!({"kinds": ["sequence_ref"]})), None);
    add("library", f(json!({"kinds": []})), None);
    add("library", f(json!({"min_duration": 10 * SEC})), None);
    add("library", f(json!({"max_duration": 10 * SEC - 1})), None);
    add(
        "library",
        f(json!({"min_duration": 10 * SEC, "max_duration": 60 * SEC - 1})),
        None,
    );
    add(
        "library",
        f(json!({"min_duration": 60 * SEC, "max_duration": 300 * SEC - 1})),
        None,
    );
    add("library", f(json!({"min_duration": 300 * SEC})), None);
    add("library", f(json!({"min_duration": 0})), None);
    add(
        "library",
        f(json!({"min_duration": 12 * SEC, "max_duration": 12 * SEC})),
        None,
    );
    add("library", f(json!({"used": true})), None);
    add("library", f(json!({"used": false})), None);
    add("library", f(json!({"problems": true})), None);
    add("library", f(json!({"problems": false})), None);
    add(
        "library",
        f(json!({"text": "beach", "kinds": ["video"], "used": false})),
        None,
    );
    add(
        "library",
        f(json!({"kinds": ["audio"], "used": false, "min_duration": 5 * SEC})),
        None,
    );
    add(
        "library",
        f(json!({"kinds": ["image"], "max_duration": 5 * SEC})),
        None,
    );
    add("library", f(json!({"used": true, "problems": true})), None);
    // Display names and folders (ADR-0020): the name replaces the file name in matching and in
    // the name sort; `folder` is one level unless `recursive`, and `recursive: false` alone is
    // the top level.
    let fid = |n: u128| folder_id(n);
    add("folders", f(json!({"text": "opening"})), None);
    add("folders", f(json!({"text": "a.mp4"})), None);
    add("folders", f(json!({"text": "theme"})), None);
    add("folders", f(json!({"text": "INTERVIEW"})), None);
    add("folders", f(json!({"text": "roll"})), None);
    add("folders", f(json!({"folder": fid(1)})), None);
    add(
        "folders",
        f(json!({"folder": fid(1), "recursive": true})),
        None,
    );
    add("folders", f(json!({"folder": fid(2)})), None);
    add("folders", f(json!({"folder": fid(3), "text": "a"})), None);
    add("folders", f(json!({"recursive": false})), None);
    add("folders", f(json!({"recursive": true})), None);
    add(
        "folders",
        f(json!({"folder": fid(1), "recursive": true, "kinds": ["video"], "used": true})),
        None,
    );
    add(
        "folders",
        f(json!({"folder": fid(1), "recursive": false, "used": false})),
        None,
    );
    // Tags, ratings, notes and collections (ADR-0036).
    let cid = |n: u128| collection_id(n);
    add("tagged", f(json!({"tags": ["interview"]})), None);
    add("tagged", f(json!({"tags": ["INTERVIEW", "wide"]})), None);
    add("tagged", f(json!({"tags": [" cafe "]})), None);
    add("tagged", f(json!({"tags": ["inter"]})), None);
    add("tagged", f(json!({"tags": ["日本語"]})), None);
    add("tagged", f(json!({"tags": []})), None);
    add(
        "tagged",
        f(json!({"tags": ["keep"], "kinds": ["image"]})),
        None,
    );
    add("tagged", f(json!({"min_rating": 1})), None);
    add("tagged", f(json!({"min_rating": 4})), None);
    add("tagged", f(json!({"min_rating": 5})), None);
    add(
        "tagged",
        f(json!({"min_rating": 3, "tags": ["interview"]})),
        None,
    );
    add("tagged", f(json!({"min_rating": 2, "used": true})), None);
    // Text in the name, a tag or the note; every term somewhere.
    add("tagged", f(json!({"text": "cafe"})), None);
    add("tagged", f(json!({"text": "interview"})), None);
    add("tagged", f(json!({"text": "wide"})), None);
    add("tagged", f(json!({"text": "naive A001"})), None);
    add("tagged", f(json!({"text": "eclair keep mp4"})), None);
    add("tagged", f(json!({"text": "focus audio"})), None);
    add("tagged", f(json!({"text": "interview room"})), None);
    add("tagged", f(json!({"text": "wide zzz"})), None);
    add("tagged", f(json!({"text": "日本"})), None);
    add("tagged", f(json!({"text": "only\tUSE"})), None);
    add(
        "tagged",
        f(json!({"text": "interview", "tags": ["wide"]})),
        None,
    );
    for n in 1..=6 {
        add("tagged", f(json!({"collection": cid(n)})), None);
    }
    add(
        "tagged",
        f(json!({"collection": cid(1), "kinds": ["video"], "min_rating": 5})),
        None,
    );
    add(
        "tagged",
        f(json!({"collection": cid(3), "text": "a00"})),
        None,
    );
    add(
        "tagged",
        f(json!({"collection": cid(3), "folder": fid(1), "recursive": true})),
        None,
    );
    add(
        "tagged",
        f(json!({"collection": cid(4), "used": false})),
        None,
    );
    add(
        "tagged",
        f(json!({"collection": cid(5), "tags": ["interview"]})),
        None,
    );
    add(
        "tagged",
        (
            json!({"collection": cid(1)}),
            json!({"by": "name", "order": "desc"}),
        ),
        Some(2),
    );
    add(
        "tagged",
        (
            json!({"tags": ["interview"]}),
            json!({"by": "duration", "order": "asc"}),
        ),
        Some(4),
    );
    add("empty", f(json!({"text": "x"})), None);
    add("empty", f(json!({})), None);
    add("ties", f(json!({"text": "clip"})), None);
    // Every sort, both ways, unfiltered and filtered.
    for by in ["imported", "name", "duration", "kind"] {
        for order in ["asc", "desc"] {
            let sort = json!({"by": by, "order": order});
            add("library", (json!({}), sort.clone()), None);
            add("library", (json!({"used": false}), sort.clone()), None);
            add("ties", (json!({}), sort.clone()), None);
            add("folders", (json!({}), sort), None);
        }
    }
    add("library", (json!({}), json!({"by": "name"})), None);
    // Paging under each sort.
    for by in ["imported", "name", "duration", "kind"] {
        for order in ["asc", "desc"] {
            add(
                "library",
                (json!({}), json!({"by": by, "order": order})),
                Some(3),
            );
        }
    }
    add(
        "library",
        (
            json!({"text": "a"}),
            json!({"by": "duration", "order": "desc"}),
        ),
        Some(2),
    );
    add(
        "ties",
        (json!({}), json!({"by": "name", "order": "asc"})),
        Some(1),
    );
    c
}

fn generate() -> Value {
    let mut documents = Vec::new();
    let mut harnesses = Vec::new();
    for (name, (assets, sequences, tree, collections)) in [
        ("library", with_tree(library())),
        ("empty", (Vec::new(), vec![vec![]], Vec::new(), Vec::new())),
        ("ties", with_tree(ties())),
        ("folders", folders()),
        ("tagged", tagged()),
    ] {
        let doc = build(&assets, &sequences, &tree, &collections);
        doc.validate().unwrap();
        documents.push(json!({
            "name": name,
            "media": assets.iter().map(|s| json!({
                "id": media_id(s.n),
                "kind": s.kind,
                "path_abs": s.path_abs,
                "path_rel": s.path_rel,
                "duration": s.duration,
                "streams": if s.broken || s.kind == MediaKind::SequenceRef { 0 } else { 1 },
                "name": s.name,
                "folder": s.folder.map(folder_id),
                "tags": s.tags,
                "rating": s.rating,
                "note": s.note,
            })).collect::<Vec<_>>(),
            "collections": collections.iter().map(|(n, name, kind)| json!({
                "id": collection_id(*n),
                "name": name,
                "kind": kind,
            })).collect::<Vec<_>>(),
            "folders": tree.iter().map(|(n, name, parent)| json!({
                "id": folder_id(*n),
                "name": name,
                "parent": parent.map(folder_id),
            })).collect::<Vec<_>>(),
            "sequences": sequences.iter().map(|u| u.iter().map(|n| media_id(*n)).collect::<Vec<_>>()).collect::<Vec<_>>(),
        }));
        harnesses.push((name, H::with_doc(doc)));
    }
    let cases: Vec<Value> = cases()
        .into_iter()
        .map(|(doc, filter, sort, limit)| {
            let h = &harnesses.iter().find(|(n, _)| *n == doc).unwrap().1;
            let mut params = json!({"limit": 500});
            if !filter.as_object().is_some_and(serde_json::Map::is_empty) {
                params["filter"] = filter.clone();
            }
            if !sort.is_null() {
                params["sort"] = sort.clone();
            }
            let out = h.query("media.list", params.clone()).unwrap();
            let mut case = json!({
                "document": doc,
                "filter": if filter.as_object().is_some_and(serde_json::Map::is_empty) { Value::Null } else { filter },
                "sort": sort,
                "ids": ids_of(&out),
                "matched": out["matched"],
            });
            if let Some(limit) = limit {
                params.as_object_mut().unwrap().remove("limit");
                let paged = pages(h, &params, limit);
                let flat: Vec<Value> = paged.iter().flatten().cloned().collect();
                assert_eq!(flat, ids_of(&out), "pages of {doc} {params} concatenate to the list");
                case["limit"] = json!(limit);
                case["pages"] = json!(paged);
            }
            case
        })
        .collect();
    json!({
        "comment": "Generated by crates/cutweave-ops/tests/media_find_fixture.rs; do not edit. `ids` is the full result of media.list in order; `pages` the pages at `limit`.",
        "documents": documents,
        "cases": cases,
    })
}

#[test]
fn media_find_fixture_matches_the_engine() {
    let mut text = serde_json::to_string_pretty(&generate()).unwrap();
    text.push('\n');
    let path = fixture_path();
    if std::env::var_os("CUTWEAVE_BLESS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &text).unwrap();
    }
    let on_disk = fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        on_disk == text,
        "{} is out of date: run `CUTWEAVE_BLESS=1 cargo test -p cutweave-ops --test media_find_fixture`",
        path.display()
    );
}

#[test]
fn the_fixture_has_enough_cases_and_they_are_not_trivial() {
    let v = generate();
    let cases = v["cases"].as_array().unwrap();
    assert!(cases.len() >= 80, "{} cases", cases.len());
    for part in ["tags", "min_rating", "collection"] {
        let with = cases
            .iter()
            .filter(|c| !c["filter"][part].is_null())
            .count();
        assert!(with >= 5, "{with} cases with `{part}`");
    }
    assert!(cases.iter().filter(|c| c.get("pages").is_some()).count() >= 8);
    let distinct = cases
        .iter()
        .map(|c| c["ids"].to_string())
        .collect::<std::collections::HashSet<_>>();
    assert!(distinct.len() >= 45, "{} distinct results", distinct.len());
}

#[test]
fn bad_filters_and_cursors_are_refused() {
    let doc = build(&library().0, &library().1, &[], &[]);
    let h = H::with_doc(doc);
    let bad =
        |params: Value, path: &str| common::assert_invalid(h.query("media.list", params), path);
    bad(
        json!({"filter": {"min_duration": 10, "max_duration": 5}}),
        "/filter/min_duration",
    );
    bad(json!({"filter": {"text": "x".repeat(257)}}), "/filter/text");
    bad(
        json!({"sort": {"by": "name"}, "cursor": media_id(999)}),
        "/cursor",
    );
    let _ = (SequenceId::from_u128(0), TrackId::from_u128(0));
}
