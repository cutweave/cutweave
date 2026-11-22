// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.02 acceptance: the `SplitItem` example of arch §5.1 compiles verbatim, registers, and
//! its input schema matches the reviewed snapshot; the command then runs and undoes through
//! its recorded patch.

#![allow(clippy::unwrap_used)]
// The arch example is kept verbatim, and its fields carry no doc comments.
#![allow(missing_docs)]

use cutweave_commands::{
    Access, CmdError, Command, QueryCtx, Registry, RegistryError, Transaction, Tx, apply_patch,
    command, wire,
};
use cutweave_model::{
    Clip, Duration, FrameRate, IdGen, ItemId, MediaAsset, MediaKind, ProbeInfo, Project,
    SampleRate, SeededGen, Sequence, SequenceFormat, Time, Track, TrackKind,
};
use serde_json::json;

// ---- arch §5.1, verbatim ----

/// Defined once; generates JSON Schema, TS types, MCP tool/catalog entry, CLI help and docs.
#[command(
    name = "clip.split",
    summary = "Split an item at a timeline time",
    undoable,
    mcp = "core",                    // core | catalog | hidden
    annotations(destructive = false, idempotent = false)
)]
pub struct SplitItem {
    pub item: ItemId,
    pub at: Time, // flicks, or "00:01:02:03" / "62.5s" accepted by parser
}

// ---- end of the verbatim part ----

/// The two halves of a split.
#[wire]
pub struct SplitOutput {
    /// The original item, now ending at `at`.
    pub left: ItemId,
    /// The new item, starting at `at`.
    pub right: ItemId,
}

fn invalid_at() -> CmdError {
    CmdError::invalid("/at", "must be strictly inside the item")
}

impl Command for SplitItem {
    type Output = SplitOutput; // { left: ItemId, right: ItemId }

    fn validate(&self, ctx: &QueryCtx) -> Result<(), CmdError> {
        let at = ctx.item(self.item)?;
        let end = at
            .item
            .end()
            .map_err(|e| CmdError::internal(e.to_string()))?;
        if self.at <= at.item.start() || self.at >= end {
            return Err(invalid_at());
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx) -> Result<Self::Output, CmdError> {
        let at = tx.item(self.item)?;
        let track = at.track.id;
        let left = at.item.as_clip().ok_or_else(invalid_at)?.clone();
        let offset = (self.at - left.start).map_err(|_| invalid_at())?;
        let mut right = left.clone();
        right.id = tx.new_id();
        right.start = self.at;
        right.duration = (left.duration - offset).map_err(|_| invalid_at())?;
        right.source_in = (left.source_in + offset).map_err(|_| invalid_at())?;
        tx.update_item(self.item, |item| {
            item.set_duration(offset);
            Ok(())
        })?;
        tx.insert_item(track, &right.clone().into())?;
        Ok(SplitOutput {
            left: left.id,
            right: right.id,
        })
    }
}

/// What `cutweave-ops` will look like: one explicit registration function per crate.
fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register::<SplitItem>()?;
    Ok(())
}

#[test]
fn the_arch_example_registers_with_the_reviewed_input_schema() {
    let mut registry = Registry::new();
    register(&mut registry).unwrap();
    let entry = registry.get("clip.split").unwrap();
    assert_eq!(entry.meta.summary, "Split an item at a timeline time");
    assert!(entry.meta.undoable);
    // Canonical (sorted) keys: the workspace build enables `serde_json/preserve_order` (via
    // xtask), which changes map order depending on which crates are built together.
    insta::assert_snapshot!(
        "clip_split_input_schema",
        cutweave_model::canonical_json_pretty(&entry.meta.input_schema)
    );
    assert_eq!(
        entry.meta.output_schema["properties"]["right"]["description"],
        "The new item, starting at `at`."
    );
    // Registering twice is refused.
    assert_eq!(
        register(&mut registry).unwrap_err(),
        RegistryError::Duplicate {
            name: "clip.split".into()
        }
    );
}

#[test]
fn the_arch_example_runs_and_undoes() {
    let mut ids = SeededGen::new(1);
    let mut doc = Project::new("Split");
    let probe = ProbeInfo {
        duration: Some(Duration::from_secs(60).unwrap()),
        ..ProbeInfo::default()
    };
    let media = MediaAsset::file(ids.mint(), MediaKind::Video, "/a.mp4", probe);
    let format = SequenceFormat::new(1920, 1080, FrameRate::FPS_30, SampleRate::HZ_48_000);
    let mut sequence = Sequence::new(ids.mint(), "Main", format);
    let mut track = Track::new(ids.mint(), TrackKind::Video, "V1");
    let clip = Clip::new(
        ids.mint(),
        media.id,
        Time::ZERO,
        Duration::from_secs(10).unwrap(),
    );
    track.items.insert(clip.clone().into());
    sequence.insert_track(0, track);
    doc.media.insert(media.id, media);
    doc.sequences.insert(sequence.id, sequence);

    let mut registry = Registry::new();
    register(&mut registry).unwrap();
    let mut txn = Transaction::begin(doc.clone());
    let mut tx = txn.tx(&mut ids, Access::STANDARD);
    // "62.5s"-style and integer inputs; outside the item is refused by `validate`.
    let err = registry
        .invoke_json("clip.split", json!({"item": clip.id, "at": "12s"}), &mut tx)
        .unwrap_err();
    assert_eq!(err, invalid_at());
    let out = registry
        .invoke_json(
            "clip.split",
            json!({"item": clip.id, "at": "2.5s"}),
            &mut tx,
        )
        .unwrap();
    assert_eq!(out["left"], json!(clip.id));
    let committed = txn.commit();
    committed.doc.validate().unwrap();
    assert_eq!(committed.doc.item_count(), 2);
    assert_eq!(
        apply_patch(&committed.doc, &committed.inverse).unwrap(),
        doc
    );
}
