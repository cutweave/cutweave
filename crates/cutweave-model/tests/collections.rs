// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.06a (ADR-0036): tags, ratings, notes and collections in the document.
//!
//! - The fields are optional and left out of the file while empty, so a v3 document written
//!   before them reads and saves the same (no schema bump, no migration).
//! - A build from before them keeps them through open and save (unknown fields survive in
//!   `extra`); the test replays that with the document shape of such a build.
//! - A manual collection may name an asset that no longer exists; every other rule of the ADR
//!   is an invariant of `Project::validate`.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use cutweave_model::{
    CURRENT_SCHEMA_VERSION, Collection, CollectionId, CollectionKind, Invariant,
    MAX_COLLECTION_MEMBERS, MAX_COLLECTIONS, MediaAsset, MediaFilter, MediaId, MediaKind,
    OpenOutcome, ProbeInfo, Project, open_str, to_document_string,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn mid(n: u128) -> MediaId {
    MediaId::from_u128(n)
}

fn cid(n: u128) -> CollectionId {
    CollectionId::from_u128(n)
}

fn asset(doc: &mut Project, n: u128) -> MediaId {
    let id = mid(n);
    doc.media.insert(
        id,
        MediaAsset::file(id, MediaKind::Image, "/m/x.png", ProbeInfo::default()),
    );
    id
}

/// Three assets with tags, a rating and a note, a manual collection with a member that names no
/// asset, and a smart collection.
fn annotated() -> Project {
    let mut doc = Project::new("Annotated");
    let (a, b, c) = (asset(&mut doc, 1), asset(&mut doc, 2), asset(&mut doc, 3));
    {
        let a = doc.media.get_mut(&a).unwrap();
        a.tags = vec!["b-roll".into(), "Café".into(), "interview".into()];
        a.rating = Some(4);
        a.note = Some("Good take.\nUse the second half.".into());
    }
    doc.media.get_mut(&b).unwrap().rating = Some(1);
    doc.collections.insert(
        cid(1),
        Collection::new(
            cid(1),
            "Selects",
            CollectionKind::Manual {
                media: vec![c, mid(99), a],
            },
        ),
    );
    doc.collections.insert(
        cid(2),
        Collection::new(
            cid(2),
            "Unused interviews",
            CollectionKind::Smart {
                filter: MediaFilter {
                    kinds: Some(vec![MediaKind::Video]),
                    tags: Some(vec!["interview".into()]),
                    used: Some(false),
                    ..MediaFilter::default()
                },
            },
        ),
    );
    doc
}

fn editable(text: &str) -> Project {
    match open_str(text).unwrap() {
        OpenOutcome::Editable { project, report } => {
            assert!(report.steps.is_empty(), "no migration ran");
            project
        }
        other => panic!("expected an editable document: {other:?}"),
    }
}

#[test]
fn no_schema_bump_and_nothing_new_in_a_document_without_the_fields() {
    assert_eq!(CURRENT_SCHEMA_VERSION, 3);
    let mut doc = Project::new("Plain");
    asset(&mut doc, 1);
    let json = serde_json::to_value(&doc).unwrap();
    assert!(json.get("collections").is_none());
    let media = &json["media"][mid(1).to_string()];
    for key in ["tags", "rating", "note"] {
        assert!(media.get(key).is_none(), "`{key}` is left out while empty");
    }
}

#[test]
fn v3_documents_written_before_the_fields_read_and_save_the_same() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v3");
    let mut seen = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("\r\n", "\n");
        let doc = editable(&text);
        assert!(doc.collections.is_empty());
        assert!(
            doc.media
                .values()
                .all(|m| m.tags.is_empty() && m.rating.is_none() && m.note.is_none())
        );
        assert_eq!(
            to_document_string(&doc).unwrap(),
            text,
            "{} saves byte for byte",
            path.display()
        );
        seen += 1;
    }
    assert!(seen >= 5, "{seen} v3 fixtures");
}

#[test]
fn the_fields_have_their_wire_form_and_round_trip() {
    let doc = annotated();
    assert_eq!(doc.violations(), vec![]);
    let json = serde_json::to_value(&doc).unwrap();
    let a = &json["media"][mid(1).to_string()];
    assert_eq!(a["tags"], json!(["b-roll", "Café", "interview"]));
    assert_eq!(a["rating"], 4);
    assert_eq!(a["note"], "Good take.\nUse the second half.");
    assert!(json["media"][mid(2).to_string()].get("tags").is_none());
    assert_eq!(
        json["collections"][cid(1).to_string()],
        json!({
            "id": cid(1),
            "name": "Selects",
            "kind": {"type": "manual", "media": [mid(3), mid(99), mid(1)]},
            "custom": {},
        })
    );
    assert_eq!(
        json["collections"][cid(2).to_string()]["kind"],
        json!({
            "type": "smart",
            "filter": {"kinds": ["video"], "used": false, "tags": ["interview"]},
        })
    );
    let text = to_document_string(&doc).unwrap();
    assert_eq!(editable(&text), doc);
}

/// A media asset as a build from before ADR-0036 reads it: the fields it knows, everything else
/// in `extra`.
#[derive(Serialize, Deserialize)]
struct OldAsset {
    id: Value,
    kind: Value,
    name: Value,
    folder: Value,
    path_abs: Value,
    path_rel: Value,
    content_hash: Value,
    sequence: Value,
    probe: Value,
    custom: Value,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

/// The project as that build reads it.
#[derive(Serialize, Deserialize)]
struct OldProject {
    meta: Value,
    settings: Value,
    media: BTreeMap<String, OldAsset>,
    folders: Value,
    sequences: Value,
    custom: Value,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[test]
fn an_older_build_keeps_the_fields_through_open_and_save() {
    let doc = annotated();
    let text = to_document_string(&doc).unwrap();
    let old: OldProject = serde_json::from_str(&text).unwrap();
    assert!(old.extra.contains_key("collections"));
    let first = old.media.get(&mid(1).to_string()).unwrap();
    for key in ["tags", "rating", "note"] {
        assert!(first.extra.contains_key(key), "`{key}` lands in extra");
    }
    let saved = serde_json::to_string_pretty(&old).unwrap();
    assert_eq!(editable(&saved), doc, "nothing was lost");
}

#[test]
fn an_older_build_that_removes_an_asset_leaves_a_member_that_still_opens() {
    let doc = annotated();
    let text = to_document_string(&doc).unwrap();
    let mut old: OldProject = serde_json::from_str(&text).unwrap();
    // The older build removes asset 3 and knows nothing of the collection that names it.
    old.media.remove(&mid(3).to_string());
    let reopened = editable(&serde_json::to_string(&old).unwrap());
    assert_eq!(reopened.violations(), vec![]);
    assert_eq!(
        reopened.collections[&cid(1)].manual_members(),
        [mid(3), mid(99), mid(1)]
    );
    assert!(!reopened.media.contains_key(&mid(3)));
}

fn reasons(doc: &Project) -> Vec<String> {
    doc.violations()
        .into_iter()
        .map(|v| match v {
            Invariant::InvalidMedia { reason, .. }
            | Invariant::InvalidCollection { reason, .. } => reason,
            other => other.to_string(),
        })
        .collect()
}

#[test]
fn tags_ratings_and_notes_are_checked() {
    let cases: Vec<(&str, Box<dyn Fn(&mut MediaAsset)>)> = vec![
        (
            "sorted",
            Box::new(|m| m.tags = vec!["interview".into(), "b-roll".into()]),
        ),
        (
            "sorted",
            Box::new(|m| m.tags = vec!["cafe".into(), "Café".into()]),
        ),
        ("comma", Box::new(|m| m.tags = vec!["a,b".into()])),
        ("empty", Box::new(|m| m.tags = vec![String::new()])),
        ("space", Box::new(|m| m.tags = vec![" padded".into()])),
        ("64 characters", Box::new(|m| m.tags = vec!["x".repeat(65)])),
        (
            "64 tags",
            Box::new(|m| m.tags = (0..65).map(|n| format!("t{n:03}")).collect()),
        ),
        ("1 to 5", Box::new(|m| m.rating = Some(0))),
        ("1 to 5", Box::new(|m| m.rating = Some(6))),
        ("empty", Box::new(|m| m.note = Some("  ".into()))),
        ("2000", Box::new(|m| m.note = Some("x".repeat(2001)))),
        ("control", Box::new(|m| m.note = Some("a\u{0}b".into()))),
    ];
    for (needle, edit) in cases {
        let mut doc = annotated();
        edit(doc.media.get_mut(&mid(2)).unwrap());
        let all = reasons(&doc);
        assert_eq!(all.len(), 1, "{needle}: {all:?}");
        assert!(all[0].contains(needle), "{needle}: {all:?}");
    }
}

#[test]
fn collections_are_checked() {
    let cases: Vec<(&str, Box<dyn Fn(&mut Project)>)> = vec![
        (
            "empty",
            Box::new(|d| d.collections.get_mut(&cid(1)).unwrap().name = " ".into()),
        ),
        (
            "255",
            Box::new(|d| d.collections.get_mut(&cid(1)).unwrap().name = "x".repeat(256)),
        ),
        (
            "same name",
            Box::new(|d| d.collections.get_mut(&cid(2)).unwrap().name = "SELECTS".into()),
        ),
        (
            "twice",
            Box::new(|d| {
                d.collections.get_mut(&cid(1)).unwrap().kind = CollectionKind::Manual {
                    media: vec![mid(1), mid(2), mid(1)],
                };
            }),
        ),
        (
            "10000 members",
            Box::new(|d| {
                d.collections.get_mut(&cid(1)).unwrap().kind = CollectionKind::Manual {
                    media: (0..=MAX_COLLECTION_MEMBERS as u128).map(mid).collect(),
                };
            }),
        ),
        (
            "must not name a collection",
            Box::new(|d| {
                d.collections.get_mut(&cid(2)).unwrap().kind = CollectionKind::Smart {
                    filter: MediaFilter {
                        collection: Some(cid(1)),
                        ..MediaFilter::default()
                    },
                };
            }),
        ),
        (
            "/min_rating",
            Box::new(|d| {
                d.collections.get_mut(&cid(2)).unwrap().kind = CollectionKind::Smart {
                    filter: MediaFilter {
                        min_rating: Some(9),
                        ..MediaFilter::default()
                    },
                };
            }),
        ),
        (
            "holds an entity whose id",
            Box::new(|d| d.collections.get_mut(&cid(1)).unwrap().id = cid(7)),
        ),
        (
            "plugin namespace",
            Box::new(|d| {
                let _ = d
                    .collections
                    .get_mut(&cid(1))
                    .unwrap()
                    .custom
                    .insert("com.example.plugin", json!(1));
                let mut raw = serde_json::to_value(&*d).unwrap();
                let custom = &mut raw["collections"][cid(1).to_string()]["custom"];
                *custom = json!({"not a namespace": 1});
                *d = serde_json::from_value(raw).unwrap();
            }),
        ),
    ];
    for (needle, edit) in cases {
        let mut doc = annotated();
        edit(&mut doc);
        let all = reasons(&doc);
        assert_eq!(all.len(), 1, "{needle}: {all:?}");
        assert!(all[0].contains(needle), "{needle}: {all:?}");
    }
}

#[test]
fn at_most_a_thousand_collections() {
    let mut doc = Project::new("Many");
    for n in 0..MAX_COLLECTIONS as u128 {
        let id = cid(n + 1);
        doc.collections.insert(
            id,
            Collection::new(id, format!("C{n}"), CollectionKind::manual()),
        );
    }
    assert_eq!(doc.violations(), vec![]);
    let id = cid(5000);
    doc.collections.insert(
        id,
        Collection::new(id, "One more", CollectionKind::manual()),
    );
    let all = reasons(&doc);
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(all[0].contains("1000 collections"));
}

#[test]
fn a_filter_refuses_unknown_parts_so_an_older_build_cannot_misread_one() {
    let mut raw = serde_json::to_value(annotated()).unwrap();
    raw["collections"][cid(2).to_string()]["kind"]["filter"]["imported_within_days"] = json!(3);
    assert!(serde_json::from_value::<Project>(raw).is_err());
}
