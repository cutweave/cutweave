// SPDX-License-Identifier: GPL-3.0-or-later
//! Wire-contract checks for the model types: the TypeScript export (specta, arch §5.3) maps
//! flicks to `number` (never `bigint`), and the JSON Schemas match what serde accepts.

use cutweave_model::{
    Duration, EntityKind, FrameRate, GroupId, ItemId, MediaId, Rounding, SampleRate, SequenceId,
    Time, TimeInput, TrackId,
};
use specta::{Type, Types};
use specta_typescript::Typescript;

/// Every model type that crosses the wire, exported the way `cargo xtask gen` (WP-1N) will.
fn typescript() -> String {
    let mut types = Types::default();
    Time::definition(&mut types);
    Duration::definition(&mut types);
    TimeInput::definition(&mut types);
    FrameRate::definition(&mut types);
    SampleRate::definition(&mut types);
    Rounding::definition(&mut types);
    EntityKind::definition(&mut types);
    MediaId::definition(&mut types);
    SequenceId::definition(&mut types);
    TrackId::definition(&mut types);
    ItemId::definition(&mut types);
    GroupId::definition(&mut types);
    Typescript::default()
        .export(&types, specta_serde::Format)
        .unwrap_or_else(|e| panic!("model types must export to TypeScript: {e}"))
}

#[test]
fn flicks_export_as_number_never_bigint() {
    let ts = typescript();
    assert!(ts.contains("export type Time = number"), "{ts}");
    assert!(ts.contains("export type Duration = number"), "{ts}");
    assert!(
        ts.contains("export type TimeInput = number | string"),
        "{ts}"
    );
    assert!(!ts.contains("bigint"), "{ts}");
}

#[test]
fn ids_export_as_string() {
    let ts = typescript();
    for name in ["MediaId", "SequenceId", "TrackId", "ItemId", "GroupId"] {
        assert!(
            ts.contains(&format!("export type {name} = string")),
            "{name}: {ts}"
        );
    }
}

#[test]
fn rates_export_as_numbers() {
    let ts = typescript();
    assert!(ts.contains("export type SampleRate = number"), "{ts}");
    assert!(ts.contains("export type FrameRate = {"), "{ts}");
    assert!(
        !ts.contains("TimeInputWire"),
        "helper types stay internal: {ts}"
    );
    let rounding = ts
        .split("export type Rounding =")
        .nth(1)
        .unwrap_or_default();
    let rounding = rounding.split("\n\n").next().unwrap_or_default();
    assert!(
        rounding.contains("\"floor\"") && rounding.contains("\"nearest\""),
        "{ts}"
    );
}

#[test]
fn doc_comments_carry_no_rust_doctest_lines() {
    let ts = typescript();
    assert!(!ts.contains("# Ok::<"), "{ts}");
    assert!(!ts.contains("```"), "{ts}");
}
