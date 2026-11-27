// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2B.02: the optional `background_fill` of a sequence format. A document without one reads
//! and writes as before (no schema bump); one with a fill round-trips; the numbers are checked.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use cutweave_model::{
    BackgroundFill, BackgroundFit, BlurSource, Invariant, MediaId, Project, SequenceFormat,
};
use serde_json::json;

#[test]
fn a_format_without_a_fill_reads_and_writes_as_before() {
    let format = SequenceFormat::default();
    let written = serde_json::to_value(&format).unwrap();
    assert!(written.get("background_fill").is_none(), "{written}");
    // The stored form of every project made before this field existed.
    let stored = json!({
        "width": 1920, "height": 1080,
        "frame_rate": written["frame_rate"], "sample_rate": written["sample_rate"],
        "color_space": "rec709", "background": "#000000"
    });
    let read: SequenceFormat = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(read, format);
    assert_eq!(serde_json::to_value(&read).unwrap(), stored);
}

#[test]
fn every_fill_round_trips_with_its_kind() {
    let fills = [
        (
            BackgroundFill::Blur {
                source: BlurSource::MainTrack,
                radius: 0.05,
                dim: 0.25,
            },
            json!({"kind": "blur", "source": "main_track", "radius": 0.05, "dim": 0.25}),
        ),
        (
            BackgroundFill::Image {
                media: MediaId::from_u128(7),
                fit: BackgroundFit::Contain,
            },
            json!({"kind": "image", "media": MediaId::from_u128(7), "fit": "contain"}),
        ),
        (BackgroundFill::Transparent, json!({"kind": "transparent"})),
    ];
    for (fill, stored) in fills {
        let mut format = SequenceFormat::default();
        format.background_fill = Some(fill.clone());
        let written = serde_json::to_value(&format).unwrap();
        assert_eq!(written["background_fill"], stored);
        let read: SequenceFormat = serde_json::from_value(written).unwrap();
        assert_eq!(read.background_fill, Some(fill));
    }
    // What a writer may leave out takes its default.
    let short: BackgroundFill =
        serde_json::from_value(json!({"kind": "blur", "radius": 0.1})).unwrap();
    assert_eq!(
        short,
        BackgroundFill::Blur {
            source: BlurSource::Below,
            radius: 0.1,
            dim: 0.0
        }
    );
    let image: BackgroundFill =
        serde_json::from_value(json!({"kind": "image", "media": MediaId::from_u128(7)})).unwrap();
    assert_eq!(image.media(), Some(MediaId::from_u128(7)));
    assert!(serde_json::from_value::<BackgroundFill>(json!({"kind": "gradient"})).is_err());
}

#[test]
fn the_numbers_of_a_blur_are_validated_and_a_missing_image_is_tolerated() {
    let with = |fill: BackgroundFill| {
        let mut project = Project::new("fills");
        project.settings.default_format.background_fill = Some(fill);
        project
            .violations()
            .into_iter()
            .filter(|v| matches!(v, Invariant::InvalidFormat { .. }))
            .count()
    };
    let blur = |radius, dim| BackgroundFill::Blur {
        source: BlurSource::Below,
        radius,
        dim,
    };
    assert_eq!(with(blur(0.0, 0.0)), 0);
    assert_eq!(with(blur(1.0, 1.0)), 0);
    assert_eq!(with(blur(1.01, 0.0)), 1);
    assert_eq!(with(blur(-0.01, 0.0)), 1);
    assert_eq!(with(blur(0.1, 1.5)), 1);
    assert_eq!(with(blur(f64::NAN, 0.0)), 1);
    assert_eq!(with(BackgroundFill::Transparent), 0);
    // An image that names no asset draws nothing; it is not a violation (an older build can
    // remove an asset it does not know a background uses).
    assert_eq!(
        with(BackgroundFill::Image {
            media: MediaId::from_u128(404),
            fit: BackgroundFit::Cover
        }),
        0
    );
}
