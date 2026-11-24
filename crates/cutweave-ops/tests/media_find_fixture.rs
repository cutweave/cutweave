// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1M.12: the shared media-search fixture. The Rust predicate and sort (`media.list`) are the
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
    Clip, Duration, FrameRate, MediaAsset, MediaId, MediaKind, ProbeInfo, Project, SampleRate,
    Sequence, SequenceFormat, SequenceId, Time, Track, TrackId, TrackKind,
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
}

const fn spec(n: u128, kind: MediaKind, path: &'static str, duration: Option<i64>) -> Spec {
    Spec {
        n,
        kind,
        path_abs: Some(path),
        path_rel: None,
        duration,
        broken: false,
    }
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

fn media_id(n: u128) -> MediaId {
    MediaId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n)
}

fn build(assets: &[Spec], sequences: &[Vec<u128>]) -> Project {
    let mut doc = Project::new("Fixture");
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
    add("empty", f(json!({"text": "x"})), None);
    add("empty", f(json!({})), None);
    add("ties", f(json!({"text": "clip"})), None);
    // Every sort, both ways, unfiltered and filtered.
    for by in ["imported", "name", "duration", "kind"] {
        for order in ["asc", "desc"] {
            let sort = json!({"by": by, "order": order});
            add("library", (json!({}), sort.clone()), None);
            add("library", (json!({"used": false}), sort.clone()), None);
            add("ties", (json!({}), sort), None);
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
    for (name, (assets, sequences)) in [
        ("library", library()),
        ("empty", (Vec::new(), vec![vec![]])),
        ("ties", ties()),
    ] {
        let doc = build(&assets, &sequences);
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
    assert!(cases.len() >= 40, "{} cases", cases.len());
    assert!(cases.iter().filter(|c| c.get("pages").is_some()).count() >= 8);
    let distinct = cases
        .iter()
        .map(|c| c["ids"].to_string())
        .collect::<std::collections::HashSet<_>>();
    assert!(distinct.len() >= 25, "{} distinct results", distinct.len());
}

#[test]
fn bad_filters_and_cursors_are_refused() {
    let doc = build(&library().0, &library().1);
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
