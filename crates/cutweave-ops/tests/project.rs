// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.05: `project.new`, `project.info`, `project.rename`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names
)]

mod common;

use common::{H, assert_code, assert_invalid, id};
use cutweave_model::{FrameRate, TrackKind};
use serde_json::json;

// ----- project.new ---------------------------------------------------------------------------

#[test]
fn new_creates_one_sequence_with_v1_and_a1() {
    let (h, p) = H::project();
    let doc = h.doc();
    assert_eq!(doc.meta.name, "Test");
    assert_eq!(doc.sequences.len(), 1);
    let seq = &doc.sequences[&p.sequence];
    let fmt = &seq.format;
    assert_eq!(
        (fmt.width, fmt.height, fmt.frame_rate),
        (1920, 1080, FrameRate::FPS_30)
    );
    assert_eq!(fmt.sample_rate.hz(), 48_000);
    let order: Vec<_> = seq
        .tracks_in_order()
        .map(|t| (t.kind, t.name.as_str()))
        .collect();
    assert_eq!(order, [(TrackKind::Video, "V1"), (TrackKind::Audio, "A1")]);
    assert_eq!(doc.settings.default_format, *fmt);
}

#[test]
fn new_applies_a_partial_format() {
    let mut h = H::empty();
    let out = h.ok(
        "project.new",
        json!({"name": " Shorts ", "format": {"width": 1080, "height": 1920, "frame_rate": {"num": 24, "den": 1}}}),
    );
    let doc = h.doc();
    assert_eq!(doc.meta.name, "Shorts", "names are trimmed");
    let seq = &doc.sequences[&id::<cutweave_model::SequenceId>(&out["sequence"])];
    assert_eq!((seq.format.width, seq.format.height), (1080, 1920));
    assert_eq!(seq.format.frame_rate, FrameRate::FPS_24);
    assert_eq!(
        seq.format.sample_rate.hz(),
        48_000,
        "unset fields keep the default"
    );
}

#[test]
fn new_rejects_a_bad_name_and_format_with_paths() {
    let mut h = H::empty();
    assert_invalid(h.run("project.new", json!({"name": "  "})), "/name");
    assert_invalid(
        h.run("project.new", json!({"name": "x", "format": {"width": 0}})),
        "/format",
    );
    assert_invalid(
        h.run(
            "project.new",
            json!({"name": "x", "format": {"frame_rate": {"num": 2000, "den": 1}}}),
        ),
        "/format",
    );
    // Schema-level problems name the exact argument.
    assert_invalid(
        h.run(
            "project.new",
            json!({"name": "x", "format": {"width": "wide"}}),
        ),
        "/format/width",
    );
    assert_invalid(
        h.run("project.new", json!({"name": "x", "bogus": 1})),
        "/bogus",
    );
    assert_eq!(h.commits(), 0);
}

#[test]
fn new_refuses_a_project_that_already_has_content() {
    let (mut h, _) = H::project();
    assert_invalid(h.run("project.new", json!({"name": "Again"})), "");
    let mut h = H::empty();
    h.add_audio("/m/a.wav", 5);
    assert_invalid(h.run("project.new", json!({"name": "Again"})), "");
}

#[test]
fn new_undo_redo_identity_and_dry_run() {
    let mut h = H::empty();
    h.validate("project.new", json!({"name": "x"})).unwrap();
    assert!(h.validate("project.new", json!({"name": ""})).is_err());
    h.ok("project.new", json!({"name": "x"}));
    let c = h.last.as_ref().unwrap();
    assert_eq!(c.base.sequences.len(), 0);
    assert_eq!(c.doc.sequences.len(), 1);
}

#[test]
fn new_is_deterministic_under_seeded_ids() {
    let a = {
        let mut h = H::empty();
        h.ok("project.new", json!({"name": "x"}))
    };
    let b = {
        let mut h = H::empty();
        h.ok("project.new", json!({"name": "x"}))
    };
    assert_eq!(a, b);
}

// ----- project.info -------------------------------------------------------------------------

#[test]
fn info_summarizes_the_project() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/a.mp4", 10, true);
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0}),
    );
    let info = h.query("project.info", json!({})).unwrap();
    assert_eq!(info["name"], "Test");
    assert_eq!(info["media_count"], 1);
    let seq = &info["sequences"][0];
    assert_eq!(seq["id"], json!(p.sequence));
    assert_eq!(seq["video_tracks"], 1);
    assert_eq!(seq["audio_tracks"], 1);
    assert_eq!(seq["item_count"], 2);
    assert_eq!(seq["duration"], 10 * 705_600_000_i64);
}

#[test]
fn info_of_an_empty_project() {
    let h = H::empty();
    let info = h.query("project.info", json!({})).unwrap();
    assert_eq!(info["sequences"], json!([]));
    assert_eq!(info["media_count"], 0);
    assert_eq!(
        info["schema_version"],
        cutweave_model::CURRENT_SCHEMA_VERSION
    );
}

#[test]
fn info_rejects_unknown_params() {
    let h = H::empty();
    assert_invalid(
        h.query("project.info", json!({"verbose": true})),
        "/verbose",
    );
}

#[test]
fn info_changes_nothing_and_commands_are_not_queries() {
    let (mut h, _) = H::project();
    let before = h.doc();
    h.query("project.info", json!({})).unwrap();
    assert_eq!(h.doc(), before);
    assert_code(
        h.query("project.rename", json!({"name": "x"})),
        "unsupported",
    );
    // Run inside a transaction, the query commits nothing.
    h.ok("project.info", json!({}));
    assert!(h.last.as_ref().unwrap().is_empty());
}

// ----- project.rename -----------------------------------------------------------------------

#[test]
fn rename_changes_the_name() {
    let (mut h, _) = H::project();
    let out = h.ok("project.rename", json!({"name": "  Final cut "}));
    assert_eq!(out["name"], "Final cut");
    assert_eq!(h.doc().meta.name, "Final cut");
}

#[test]
fn rename_rejects_empty_long_and_control_names() {
    let (mut h, _) = H::project();
    assert_invalid(h.run("project.rename", json!({"name": ""})), "/name");
    assert_invalid(
        h.run("project.rename", json!({"name": "x".repeat(201)})),
        "/name",
    );
    assert_invalid(h.run("project.rename", json!({"name": "a\nb"})), "/name");
    assert_invalid(h.run("project.rename", json!({})), "");
    assert_eq!(h.doc().meta.name, "Test");
}

#[test]
fn rename_to_the_same_name_is_a_no_op_commit() {
    let (mut h, _) = H::project();
    h.ok("project.rename", json!({"name": "Test"}));
    assert!(
        h.last.as_ref().unwrap().is_empty(),
        "idempotent: nothing changed"
    );
}

#[test]
fn rename_undo_redo_identity() {
    let (mut h, _) = H::project();
    h.ok("project.rename", json!({"name": "Renamed"}));
    let c = h.last.as_ref().unwrap();
    assert_eq!(c.base.meta.name, "Test");
    assert_eq!(c.doc.meta.name, "Renamed");
}

#[test]
fn rename_has_no_locked_or_not_found_cases() {
    // Nothing it touches can be locked or missing; unknown params fail the schema.
    let (mut h, _) = H::project();
    assert_invalid(h.run("project.rename", json!({"name": "a", "x": 1})), "/x");
}
