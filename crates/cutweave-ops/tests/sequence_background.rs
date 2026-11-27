// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2B.02: `sequence.set_background`: the color, blur, image and transparent fills.
//!
//! `H::run` asserts the undo/redo identity and `Project::validate` after every success and that
//! a failure changes nothing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, assert_code, assert_invalid, id};
use cutweave_model::{BackgroundFill, BackgroundFit, BlurSource, MediaId, SequenceFormat};
use cutweave_ops::media::used_media;
use serde_json::{Value, json};

fn format(h: &H, sequence: cutweave_model::SequenceId) -> SequenceFormat {
    h.doc().sequences[&sequence].format.clone()
}

#[test]
fn a_new_sequence_has_no_fill_and_saves_none() {
    let (h, p) = H::project();
    let f = format(&h, p.sequence);
    assert_eq!(f.background_fill, None);
    let json = serde_json::to_value(&f).unwrap();
    assert!(json.get("background_fill").is_none(), "{json}");
}

#[test]
fn blur_takes_defaults_then_keeps_what_is_not_given() {
    let (mut h, p) = H::project();
    let out = h.ok(
        "sequence.set_background",
        json!({"sequence": p.sequence, "kind": "blur"}),
    );
    assert_eq!(
        out["format"]["background_fill"],
        json!({"kind": "blur", "source": "below", "radius": 0.05, "dim": 0.25})
    );
    h.ok(
        "sequence.set_background",
        json!({"sequence": p.sequence, "kind": "blur", "radius": 0.1, "source": "main_track"}),
    );
    assert_eq!(
        format(&h, p.sequence).background_fill,
        Some(BackgroundFill::Blur {
            source: BlurSource::MainTrack,
            radius: 0.1,
            dim: 0.25
        })
    );
    // The color under the fill can change in the same call, and stays when not given.
    h.ok(
        "sequence.set_background",
        json!({"sequence": p.sequence, "kind": "blur", "dim": 0.5, "color": "#102030"}),
    );
    let f = format(&h, p.sequence);
    assert_eq!(f.background.to_string(), "#102030");
    assert_eq!(
        f.background_fill,
        Some(BackgroundFill::Blur {
            source: BlurSource::MainTrack,
            radius: 0.1,
            dim: 0.5
        })
    );
}

#[test]
fn color_removes_the_fill_and_transparent_sets_it() {
    let (mut h, p) = H::project();
    h.ok(
        "sequence.set_background",
        json!({"sequence": p.sequence, "kind": "transparent"}),
    );
    assert_eq!(
        format(&h, p.sequence).background_fill,
        Some(BackgroundFill::Transparent)
    );
    h.ok(
        "sequence.set_background",
        json!({"sequence": p.sequence, "kind": "color", "color": "#336699"}),
    );
    let f = format(&h, p.sequence);
    assert_eq!(f.background_fill, None);
    assert_eq!(f.background.to_string(), "#336699");
    // The same again changes nothing (and is still fine).
    let before = h.doc();
    h.ok(
        "sequence.set_background",
        json!({"sequence": p.sequence, "kind": "color"}),
    );
    assert_eq!(h.doc().sequences, before.sequences);
}

#[test]
fn image_needs_a_still_of_the_project_and_counts_as_a_use() {
    let (mut h, p) = H::project();
    let still = h.add_image("/m/paper.png");
    let video = h.add_video("/m/a.mp4", 10, true);
    assert_invalid(
        h.run(
            "sequence.set_background",
            json!({"sequence": p.sequence, "kind": "image"}),
        ),
        "/media",
    );
    assert_invalid(
        h.run(
            "sequence.set_background",
            json!({"sequence": p.sequence, "kind": "image", "media": video}),
        ),
        "/media",
    );
    assert_code(
        h.run(
            "sequence.set_background",
            json!({"sequence": p.sequence, "kind": "image", "media": MediaId::from_u128(77)}),
        ),
        "not_found",
    );
    assert!(!used_media(&h.doc()).contains(&still));
    let out = h.ok(
        "sequence.set_background",
        json!({"sequence": p.sequence, "kind": "image", "media": still}),
    );
    assert_eq!(out["format"]["background_fill"]["fit"], "cover");
    assert_eq!(
        id::<MediaId>(&out["format"]["background_fill"]["media"]),
        still
    );
    assert!(used_media(&h.doc()).contains(&still));
    // The image stays when only the fit is given.
    h.ok(
        "sequence.set_background",
        json!({"sequence": p.sequence, "kind": "image", "fit": "contain"}),
    );
    assert_eq!(
        format(&h, p.sequence).background_fill,
        Some(BackgroundFill::Image {
            media: still,
            fit: BackgroundFit::Contain
        })
    );
    // A used image cannot be removed from the project.
    assert!(h.run("media.remove", json!({"id": still})).is_err());
    h.ok(
        "sequence.set_background",
        json!({"sequence": p.sequence, "kind": "color"}),
    );
    h.ok("media.remove", json!({"id": still}));
}

#[test]
fn bad_numbers_fields_of_another_kind_and_unknown_sequences_are_refused() {
    let (mut h, p) = H::project();
    for (params, path) in [
        (json!({"kind": "blur", "radius": 1.5}), "/radius"),
        (json!({"kind": "blur", "radius": -0.1}), "/radius"),
        (json!({"kind": "blur", "dim": 2}), "/dim"),
        (json!({"kind": "color", "radius": 0.1}), "/radius"),
        (json!({"kind": "transparent", "fit": "cover"}), "/fit"),
        (json!({"kind": "image", "dim": 0.5}), "/dim"),
        (json!({"kind": "transparent", "color": "#000000"}), "/color"),
    ] {
        let mut params: Value = params;
        params["sequence"] = json!(p.sequence);
        assert_invalid(h.run("sequence.set_background", params), path);
    }
    assert_code(
        h.run(
            "sequence.set_background",
            json!({"sequence": cutweave_model::SequenceId::from_u128(9), "kind": "blur"}),
        ),
        "not_found",
    );
}

#[test]
fn a_duplicate_keeps_the_fill_and_set_format_leaves_it() {
    let (mut h, p) = H::project();
    h.ok(
        "sequence.set_background",
        json!({"sequence": p.sequence, "kind": "blur", "radius": 0.02}),
    );
    let fill = format(&h, p.sequence).background_fill;
    let out = h.ok("sequence.duplicate", json!({"sequence": p.sequence}));
    assert_eq!(format(&h, id(&out["sequence"])).background_fill, fill);
    h.ok(
        "sequence.set_format",
        json!({"sequence": p.sequence, "background": "#010203", "width": 1080, "height": 1920}),
    );
    assert_eq!(format(&h, p.sequence).background_fill, fill);
}
