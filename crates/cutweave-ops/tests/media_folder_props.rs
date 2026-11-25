// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.07 acceptance (ADR-0020): for 1,000 random sequences of folder commands (create,
//! rename, move, delete) mixed with asset renames, registrations, removals and the timeline
//! edits that make assets used or unused, after every command:
//!
//! - a success undoes and redoes exactly (`H::run` asserts `undo(apply(doc)) == doc`, redo and
//!   `Project::validate`), a failure changes nothing;
//! - the folder tree is a tree: every parent and every asset's folder exists (no orphan), no
//!   cycle, at most 32 levels, sibling names unique ignoring case;
//! - the folder commands never change a timeline, and never remove an asset a timeline uses;
//! - the document survives serialize → deserialize and the `.cutweave` text round trip.
//!
//! The registry-driven harness of WP-1B.07 (`registry_props.rs`) also drives these commands
//! together with every other one; this test concentrates on them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::{BTreeSet, HashSet};

use common::{H, id};
use cutweave_model::{
    FolderId, Item, MAX_FOLDER_DEPTH, MediaId, OpenOutcome, Project, TrackId, folder_name_key,
    open_str, to_document_string,
};
use cutweave_ops::{media::used_media, testing::arb_command_among};
use proptest::{
    collection::vec,
    prelude::*,
    strategy::ValueTree,
    test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
};
use serde_json::{Value, json};

/// The commands under test plus the media and timeline commands that add, use and free assets.
const NAMES: &[&str] = &[
    "media.create_folder",
    "media.rename_folder",
    "media.rename",
    "media.move",
    "media.delete_folder",
    "media.list_folders",
    "media.list",
    "media.register",
    "media.remove",
    "timeline.insert",
    "item.delete",
];

/// Commands that must leave every timeline alone and keep every used asset.
const FOLDER_COMMANDS: &[&str] = &[
    "media.create_folder",
    "media.rename_folder",
    "media.rename",
    "media.move",
    "media.delete_folder",
];

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The request `seed` picks for `doc`.
fn generate(h: &H, doc: &Project, seed: u64) -> (String, Value) {
    let mut bytes = [0_u8; 32];
    let mut s = seed;
    for chunk in bytes.chunks_mut(8) {
        s = splitmix(s);
        chunk.copy_from_slice(&s.to_le_bytes());
    }
    let config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
    arb_command_among(&h.registry, doc, NAMES)
        .new_tree(&mut runner)
        .expect("the strategies never reject")
        .current()
}

/// A project with three assets (one on the timeline) and a small folder tree holding two of
/// them.
fn start() -> H {
    let (mut h, p) = H::project();
    let video = h.add_video("/m/v.mp4", 30, true);
    let image = h.add_image("/m/i.png");
    h.add_audio("/m/a.wav", 20);
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": video, "at": 0,
               "duration": "2s"}),
    );
    let broll: FolderId = id(&h.ok("media.create_folder", json!({"name": "B-roll"}))["folder_id"]);
    let drone: FolderId = id(&h.ok(
        "media.create_folder",
        json!({"name": "Drone", "parent": broll}),
    )["folder_id"]);
    h.ok("media.move", json!({"media": [video], "to": drone}));
    h.ok("media.move", json!({"media": [image], "to": broll}));
    h
}

fn timelines(doc: &Project) -> Vec<(TrackId, Item)> {
    let mut v: Vec<_> = doc
        .sequences
        .values()
        .flat_map(|s| s.tracks.values())
        .flat_map(|t| t.items.iter().map(|i| (t.id, i.clone())))
        .collect();
    v.sort_by_key(|(_, i)| i.id());
    v
}

/// The folder tree rules, checked independently of `Project::validate`.
fn check_tree(name: &str, doc: &Project) {
    let mut siblings: HashSet<(Option<FolderId>, String)> = HashSet::new();
    for folder in doc.folders.values() {
        if let Some(parent) = folder.parent {
            assert!(
                doc.folders.contains_key(&parent),
                "{name}: orphan folder {}",
                folder.id
            );
        }
        let depth = doc.folder_depth(folder.id);
        assert!(
            depth.is_some_and(|d| d <= MAX_FOLDER_DEPTH),
            "{name}: folder {} is on a cycle or too deep ({depth:?})",
            folder.id
        );
        assert!(
            siblings.insert((folder.parent, folder_name_key(&folder.name))),
            "{name}: sibling name clash on `{}`",
            folder.name
        );
    }
    for media in doc.media.values() {
        if let Some(folder) = media.folder {
            assert!(
                doc.folders.contains_key(&folder),
                "{name}: asset {} is in a missing folder",
                media.id
            );
        }
    }
}

fn check_after(name: &str, before: &Project, after: &Project) {
    after
        .validate()
        .unwrap_or_else(|e| panic!("{name}: the document is invalid: {e}"));
    check_tree(name, after);
    let json = serde_json::to_string(after).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, after, "{name}: serde round trip");
    if FOLDER_COMMANDS.contains(&name) {
        assert_eq!(
            timelines(before),
            timelines(after),
            "{name} changed a timeline"
        );
        let used: BTreeSet<MediaId> = used_media(before).into_iter().collect();
        for media in used {
            assert!(
                after.media.contains_key(&media),
                "{name} removed used asset {media}"
            );
        }
    }
}

/// Runs one sequence of commands; how many folder commands succeeded.
fn run_case(seeds: &[u64]) -> usize {
    let mut h = start();
    let mut folder_successes = 0;
    for (n, seed) in seeds.iter().enumerate() {
        let before = h.doc();
        let (name, params) = generate(&h, &before, *seed);
        let result = h.run(&name, params);
        let after = h.doc();
        match result {
            Err(_) => assert_eq!(
                before, after,
                "step {n}: {name} failed but changed the document"
            ),
            Ok(_) if FOLDER_COMMANDS.contains(&name.as_str()) => folder_successes += 1,
            Ok(_) => {}
        }
        check_after(&name, &before, &after);
    }
    // The saved file round trips byte for byte.
    let doc = h.doc();
    let text = to_document_string(&doc).unwrap();
    let OpenOutcome::Editable { project, report } = open_str(&text).unwrap() else {
        panic!("a current document opens editable");
    };
    assert!(!report.migrated());
    assert_eq!(project, doc);
    assert_eq!(to_document_string(&project).unwrap(), text);
    folder_successes
}

#[test]
fn random_folder_commands_keep_the_tree_valid_and_undoable() {
    let config = Config {
        cases: 1000,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::deterministic_rng(RngAlgorithm::ChaCha));
    let successes = std::cell::Cell::new(0_usize);
    let result = runner.run(&vec(any::<u64>(), 1..=24), |seeds| {
        successes.set(successes.get() + run_case(&seeds));
        Ok(())
    });
    if let Err(e) = result {
        panic!("{e}");
    }
    // The sequences really exercise the folder commands.
    let successes = successes.get();
    assert!(
        successes >= 2_000,
        "only {successes} folder commands succeeded in 1000 sequences"
    );
}
