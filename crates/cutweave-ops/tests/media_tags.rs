// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.06a (ADR-0036): tags, ratings and notes on media, and `media.list` with them. Every
//! successful call runs through `H::run`, which asserts the undo/redo identity and
//! `Project::validate`; every failure, that nothing changed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, assert_code, assert_invalid};
use cutweave_model::{MediaId, to_document_string};
use serde_json::{Value, json};

fn tags_of(h: &H, media: MediaId) -> Vec<String> {
    h.doc().media.get(&media).unwrap().tags.clone()
}

fn three(h: &mut H) -> (MediaId, MediaId, MediaId) {
    (
        h.add_video("/m/a.mp4", 10, true),
        h.add_video("/m/b.mp4", 20, false),
        h.add_audio("/m/c.wav", 30),
    )
}

fn listed(h: &H, filter: Value) -> Vec<MediaId> {
    let out = h.query("media.list", json!({"filter": filter})).unwrap();
    out["media"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| common::id(&m["id"]))
        .collect()
}

#[test]
fn add_tags_trims_sorts_keeps_the_first_spelling_and_is_idempotent() {
    let mut h = H::empty();
    let (a, b, c) = three(&mut h);
    let out = h.ok(
        "media.add_tags",
        json!({"media": [a, b], "tags": [" interview ", "B-roll", "INTERVIEW"]}),
    );
    assert_eq!(out["changed"], json!([a, b]));
    assert_eq!(tags_of(&h, a), ["B-roll", "interview"]);
    assert_eq!(tags_of(&h, c), Vec::<String>::new());

    // A tag the asset has in another spelling (case, accents) is not added again.
    let out = h.ok(
        "media.add_tags",
        json!({"media": [a, c], "tags": ["b-ROLL", "Café"]}),
    );
    assert_eq!(out["changed"], json!([a, c]));
    assert_eq!(tags_of(&h, a), ["B-roll", "Café", "interview"]);
    assert_eq!(tags_of(&h, c), ["b-ROLL", "Café"]);
    let out = h.ok("media.add_tags", json!({"media": [a], "tags": ["cafe"]}));
    assert_eq!(out["changed"], json!([]));
    assert_eq!(tags_of(&h, a), ["B-roll", "Café", "interview"]);

    // The document holds no `tags` for an asset without any.
    let text = to_document_string(&h.doc()).unwrap();
    assert_eq!(text.matches("\"tags\"").count(), 3);
}

#[test]
fn tag_commands_refuse_bad_input() {
    let mut h = H::empty();
    let (a, _, _) = three(&mut h);
    for name in ["media.add_tags", "media.remove_tags"] {
        assert_invalid(h.run(name, json!({"media": [], "tags": ["x"]})), "/media");
        assert_invalid(h.run(name, json!({"media": [a], "tags": []})), "/tags");
        assert_invalid(
            h.run(name, json!({"media": [a, a], "tags": ["x"]})),
            "/media/1",
        );
        for bad in ["", "  ", "a,b", "tab\there", &"x".repeat(65)] {
            assert_invalid(
                h.run(name, json!({"media": [a], "tags": ["ok", bad]})),
                "/tags/1",
            );
        }
        let seventeen: Vec<String> = (0..17).map(|n| format!("t{n}")).collect();
        assert_invalid(
            h.run(name, json!({"media": [a], "tags": seventeen})),
            "/tags",
        );
        let many: Vec<MediaId> = (0..257).map(MediaId::from_u128).collect();
        assert_invalid(h.run(name, json!({"media": many, "tags": ["x"]})), "/media");
        assert_code(
            h.run(
                name,
                json!({"media": [MediaId::from_u128(9)], "tags": ["x"]}),
            ),
            "not_found",
        );
    }
}

#[test]
fn an_asset_holds_at_most_64_tags() {
    let mut h = H::empty();
    let (a, b, _) = three(&mut h);
    for chunk in 0..4 {
        let tags: Vec<String> = (0..16)
            .map(|n| format!("tag {:02}", chunk * 16 + n))
            .collect();
        h.ok("media.add_tags", json!({"media": [a], "tags": tags}));
    }
    assert_eq!(tags_of(&h, a).len(), 64);
    assert_invalid(
        h.run(
            "media.add_tags",
            json!({"media": [b, a], "tags": ["one more"]}),
        ),
        "/media/1",
    );
    assert_eq!(tags_of(&h, b), Vec::<String>::new(), "nothing was applied");
    // One it already has is fine.
    h.ok("media.add_tags", json!({"media": [a], "tags": ["TAG 07"]}));
}

#[test]
fn remove_tags_matches_ignoring_case_and_accents_and_is_idempotent() {
    let mut h = H::empty();
    let (a, b, c) = three(&mut h);
    h.ok(
        "media.add_tags",
        json!({"media": [a, b], "tags": ["Café", "interview", "keep"]}),
    );
    let out = h.ok(
        "media.remove_tags",
        json!({"media": [a, b, c], "tags": ["cafe", "INTERVIEW", "absent"]}),
    );
    assert_eq!(out["changed"], json!([a, b]));
    assert_eq!(tags_of(&h, a), ["keep"]);
    let again = h.ok(
        "media.remove_tags",
        json!({"media": [a, b, c], "tags": ["cafe"]}),
    );
    assert_eq!(again["changed"], json!([]));
    // The last tag gone: the field leaves the document.
    h.ok(
        "media.remove_tags",
        json!({"media": [a, b], "tags": ["keep"]}),
    );
    assert!(!to_document_string(&h.doc()).unwrap().contains("\"tags\""));
}

#[test]
fn rename_tag_rewrites_every_asset_that_has_it_in_one_step() {
    let mut h = H::empty();
    let (a, b, c) = three(&mut h);
    h.ok(
        "media.add_tags",
        json!({"media": [a, b], "tags": ["interview", "wide"]}),
    );
    h.ok("media.add_tags", json!({"media": [b, c], "tags": ["Talk"]}));
    let commits = h.commits();
    // b has both: it keeps one, in the new spelling.
    let out = h.ok(
        "media.rename_tag",
        json!({"from": "INTERVIEW", "to": " talk "}),
    );
    assert_eq!(out["changed"], json!([a, b]));
    assert_eq!(h.commits(), commits + 1, "one undo step");
    assert_eq!(tags_of(&h, a), ["talk", "wide"]);
    assert_eq!(tags_of(&h, b), ["talk", "wide"]);
    assert_eq!(
        tags_of(&h, c),
        ["Talk"],
        "an asset without `from` is untouched"
    );

    // Another spelling of the same tag respells it.
    let out = h.ok("media.rename_tag", json!({"from": "talk", "to": "Talk"}));
    assert_eq!(out["changed"], json!([a, b]));
    assert_eq!(tags_of(&h, a), ["Talk", "wide"]);
    // No asset has it: nothing changes.
    let out = h.ok("media.rename_tag", json!({"from": "nope", "to": "x"}));
    assert_eq!(out["changed"], json!([]));
    assert_invalid(
        h.run("media.rename_tag", json!({"from": "", "to": "x"})),
        "/from",
    );
    assert_invalid(
        h.run("media.rename_tag", json!({"from": "wide", "to": "a,b"})),
        "/to",
    );

    let tags = h.query("media.list_tags", json!({})).unwrap();
    assert_eq!(
        tags["tags"],
        json!([{"tag": "Talk", "count": 3}, {"tag": "wide", "count": 2}])
    );
}

#[test]
fn list_tags_counts_spellings_as_one_tag() {
    let mut h = H::empty();
    let (a, b, c) = three(&mut h);
    assert_eq!(
        h.query("media.list_tags", json!({})).unwrap()["tags"],
        json!([])
    );
    h.ok(
        "media.add_tags",
        json!({"media": [b], "tags": ["Éclair", "zoom"]}),
    );
    h.ok(
        "media.add_tags",
        json!({"media": [a, c], "tags": ["eclair", "B-roll"]}),
    );
    let tags = h.query("media.list_tags", json!({})).unwrap();
    assert_eq!(
        tags["tags"],
        json!([
            {"tag": "B-roll", "count": 2},
            {"tag": "eclair", "count": 3},
            {"tag": "zoom", "count": 1},
        ])
    );
}

#[test]
fn set_rating_rates_and_clears() {
    let mut h = H::empty();
    let (a, b, c) = three(&mut h);
    let out = h.ok("media.set_rating", json!({"media": [a, b], "rating": 4}));
    assert_eq!(out["changed"], json!([a, b]));
    let out = h.ok("media.set_rating", json!({"media": [a, c], "rating": 4}));
    assert_eq!(out["changed"], json!([c]), "a already had it");
    assert_eq!(h.doc().media[&a].rating, Some(4));
    let out = h.ok("media.set_rating", json!({"media": [a, b], "rating": null}));
    assert_eq!(out["changed"], json!([a, b]));
    assert_eq!(h.doc().media[&a].rating, None);
    let out = h.ok("media.set_rating", json!({"media": [a]}));
    assert_eq!(out["changed"], json!([]));
    assert_eq!(
        to_document_string(&h.doc())
            .unwrap()
            .matches("\"rating\"")
            .count(),
        1
    );

    for bad in [0, 6, 255] {
        assert_invalid(
            h.run("media.set_rating", json!({"media": [a], "rating": bad})),
            "/rating",
        );
    }
    assert_invalid(
        h.run("media.set_rating", json!({"media": [], "rating": 1})),
        "/media",
    );
    assert_code(
        h.run(
            "media.set_rating",
            json!({"media": [MediaId::from_u128(9)], "rating": 1}),
        ),
        "not_found",
    );
}

#[test]
fn set_note_trims_and_a_blank_note_removes_it() {
    let mut h = H::empty();
    let (a, _, _) = three(&mut h);
    let out = h.ok(
        "media.set_note",
        json!({"media": a, "note": "  Good take.\nUse the second half.\n"}),
    );
    assert_eq!(out, json!({"media_id": a, "has_note": true}));
    assert_eq!(
        h.doc().media[&a].note.as_deref(),
        Some("Good take.\nUse the second half.")
    );
    let out = h.ok("media.set_note", json!({"media": a, "note": "   "}));
    assert_eq!(out["has_note"], false);
    assert_eq!(h.doc().media[&a].note, None);
    assert!(!to_document_string(&h.doc()).unwrap().contains("\"note\""));
    h.ok(
        "media.set_note",
        json!({"media": a, "note": "x".repeat(2000)}),
    );
    h.ok("media.set_note", json!({"media": a}));
    assert_eq!(h.doc().media[&a].note, None);

    assert_invalid(
        h.run(
            "media.set_note",
            json!({"media": a, "note": "x".repeat(2001)}),
        ),
        "/note",
    );
    assert_invalid(
        h.run("media.set_note", json!({"media": a, "note": "bell\u{7}"})),
        "/note",
    );
    assert_code(
        h.run(
            "media.set_note",
            json!({"media": MediaId::from_u128(9), "note": "x"}),
        ),
        "not_found",
    );
}

#[test]
fn media_list_returns_tags_and_rating_and_the_note_only_on_request() {
    let mut h = H::empty();
    let (a, b, _) = three(&mut h);
    h.ok(
        "media.add_tags",
        json!({"media": [a], "tags": ["wide", "b-roll"]}),
    );
    h.ok("media.set_rating", json!({"media": [a], "rating": 5}));
    h.ok("media.set_note", json!({"media": a, "note": "Use this."}));
    let out = h.query("media.list", json!({})).unwrap();
    let first = &out["media"][0];
    assert_eq!(first["id"], json!(a));
    assert_eq!(first["tags"], json!(["b-roll", "wide"]));
    assert_eq!(first["rating"], 5);
    assert!(first.get("note").is_none(), "no note unless asked");
    assert_eq!(out["media"][1]["tags"], json!([]));
    assert_eq!(out["media"][1]["rating"], Value::Null);

    let out = h.query("media.list", json!({"include": ["note"]})).unwrap();
    assert_eq!(out["media"][0]["note"], "Use this.");
    assert_eq!(out["media"][1]["id"], json!(b));
    assert!(
        out["media"][1].get("note").is_none(),
        "an asset without one"
    );
    assert!(h.query("media.list", json!({"include": ["path"]})).is_err());
}

#[test]
fn media_list_filters_by_tags_rating_and_text_in_tags_and_notes() {
    let mut h = H::empty();
    let (a, b, c) = three(&mut h);
    h.ok(
        "media.add_tags",
        json!({"media": [a, b], "tags": ["Interview"]}),
    );
    h.ok("media.add_tags", json!({"media": [b, c], "tags": ["Café"]}));
    h.ok("media.set_rating", json!({"media": [a], "rating": 5}));
    h.ok("media.set_rating", json!({"media": [b], "rating": 3}));
    h.ok(
        "media.set_note",
        json!({"media": c, "note": "Ambience, naïve mix"}),
    );

    assert_eq!(listed(&h, json!({"tags": ["interview"]})), [a, b]);
    assert_eq!(listed(&h, json!({"tags": [" INTERVIEW ", "cafe"]})), [b]);
    assert_eq!(
        listed(&h, json!({"tags": ["inter"]})),
        [],
        "a whole tag, not a part"
    );
    assert_eq!(listed(&h, json!({"tags": []})), [a, b, c]);
    assert_eq!(listed(&h, json!({"min_rating": 3})), [a, b]);
    assert_eq!(listed(&h, json!({"min_rating": 4})), [a]);
    assert_eq!(listed(&h, json!({"min_rating": 1, "tags": ["cafe"]})), [b]);
    // Text: a term may sit in the name, a tag or the note; every term must sit somewhere.
    assert_eq!(listed(&h, json!({"text": "inter"})), [a, b]);
    assert_eq!(listed(&h, json!({"text": "cafe b.mp4"})), [b]);
    assert_eq!(listed(&h, json!({"text": "naive AMBIENCE"})), [c]);
    assert_eq!(listed(&h, json!({"text": "cafe wav mix"})), [c]);
    assert_eq!(listed(&h, json!({"text": "interview naive"})), []);

    let bad = |filter: Value, path: &str| {
        assert_invalid(h.query("media.list", json!({"filter": filter})), path);
    };
    bad(json!({"min_rating": 0}), "/filter/min_rating");
    bad(json!({"min_rating": 6}), "/filter/min_rating");
    bad(json!({"tags": ["a,b"]}), "/filter/tags/0");
    bad(json!({"tags": vec!["t"; 17]}), "/filter/tags");
}
