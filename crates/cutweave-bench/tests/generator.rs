// SPDX-License-Identifier: GPL-3.0-or-later
//! The fixture generator (WP-2J.01): the same spec gives the same bytes, the counts are the
//! ones asked for, and the written folder opens as a project.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use cutweave_bench::fixture::{
    CANONICAL_MEDIA_ROOT, Fixture, FixtureSpec, generate, generate_at, read_manifest,
};
use cutweave_engine::project_io::ProjectStore;
use cutweave_model::Item;

fn scratch(tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("gen-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn keyframes(fixture: &Fixture) -> usize {
    let doc = serde_json::to_value(&fixture.project).unwrap();
    fn count(v: &serde_json::Value) -> usize {
        match v {
            serde_json::Value::Object(map) => map
                .iter()
                .map(|(k, v)| match (k.as_str(), v) {
                    ("keyframes", serde_json::Value::Array(keys)) => keys.len(),
                    _ => count(v),
                })
                .sum(),
            serde_json::Value::Array(items) => items.iter().map(count).sum(),
            _ => 0,
        }
    }
    count(&doc)
}

#[test]
fn the_same_spec_gives_the_same_bytes() {
    let spec = FixtureSpec::smoke();
    let (a, b) = (generate(&spec).unwrap(), generate(&spec).unwrap());
    assert_eq!(a.document().unwrap(), b.document().unwrap());
    assert_eq!(a.sha256(), b.sha256());
    assert_eq!(a.manifest, b.manifest);
    assert_eq!(a.sha256().len(), 64);
}

#[test]
fn another_seed_gives_another_document() {
    let spec = FixtureSpec::smoke();
    let other = FixtureSpec {
        seed: spec.seed + 1,
        ..spec.clone()
    };
    assert_ne!(
        generate(&spec).unwrap().sha256(),
        generate(&other).unwrap().sha256()
    );
}

#[test]
fn the_counts_are_the_ones_asked_for() {
    for spec in [FixtureSpec::smoke(), FixtureSpec::p_short()] {
        let fixture = generate(&spec).unwrap();
        let sequence = fixture.project.sequences.values().next().unwrap();
        let tracks: Vec<_> = sequence.tracks_in_order().collect();
        assert_eq!(tracks.len(), spec.tracks as usize, "{}", spec.name);
        let items: usize = tracks.iter().map(|t| t.items.len()).sum();
        assert_eq!(items, spec.items as usize, "{}", spec.name);
        assert_eq!(fixture.item_count(), spec.items as usize);
        assert_eq!(keyframes(&fixture), spec.keyframes as usize, "{}", spec.name);
        assert_eq!(fixture.manifest.keyframes, spec.keyframes);
        assert_eq!(sequence.format.width, spec.width);
        assert_eq!(sequence.format.height, spec.height);
        assert_eq!(tracks.iter().filter(|t| t.magnetic).count(), 1);
        assert!(
            tracks
                .iter()
                .flat_map(|t| t.items.iter())
                .all(|i| matches!(i, Item::Clip(_)))
        );
        fixture.project.validate().unwrap();
    }
}

#[test]
fn p_short_is_vertical_with_three_video_layers() {
    let fixture = generate(&FixtureSpec::p_short()).unwrap();
    assert!(fixture.manifest.tracks.iter().all(|t| t.video));
    assert_eq!(fixture.manifest.tracks.len(), 3);
    let spec = &fixture.manifest.spec;
    assert_eq!((spec.width, spec.height), (1080, 1920));
}

#[test]
fn the_presets_have_the_sizes_the_wbs_names() {
    let p400 = FixtureSpec::p_400();
    assert_eq!((p400.items, p400.tracks, p400.duration_secs), (400, 8, 3600));
    let p5k = FixtureSpec::p_5k();
    assert_eq!((p5k.items, p5k.keyframes), (5000, 50_000));
    assert_eq!(FixtureSpec::preset("P-5K"), Some(p5k));
    assert_eq!(FixtureSpec::preset("nope"), None);
}

#[test]
fn a_bad_spec_is_refused() {
    let mut spec = FixtureSpec::smoke();
    spec.audio_tracks = Some(spec.tracks);
    assert!(generate(&spec).is_err());
    let mut spec = FixtureSpec::smoke();
    spec.keyframes = 1_000_000;
    assert!(generate(&spec).unwrap_err().to_string().contains("do not fit"));
}

#[test]
fn the_written_folder_opens_and_keeps_the_canonical_hash_in_its_manifest() {
    let dir = scratch("write");
    let spec = FixtureSpec::smoke();
    let fixture = generate(&spec).unwrap();
    let folder = fixture.write(&dir).unwrap();
    // Writing again replaces the fixture.
    assert_eq!(fixture.write(&dir).unwrap(), folder);
    let opened = ProjectStore::open(&folder).unwrap();
    let on_disk = &opened.project;
    assert_eq!(on_disk.media.len(), fixture.project.media.len());
    let media_dir = folder.join("media");
    for asset in on_disk.media.values() {
        let path = PathBuf::from(asset.path_abs.as_ref().unwrap());
        assert!(path.starts_with(&media_dir), "{}", path.display());
    }
    opened.store.close().unwrap();
    let manifest = read_manifest(&folder).unwrap();
    assert_eq!(manifest, fixture.manifest);
    // The media root is the only thing that differs from the canonical document.
    let rooted = generate_at(&spec, "/elsewhere").unwrap();
    assert_ne!(rooted.sha256(), fixture.sha256());
    assert_eq!(
        rooted.document().unwrap().replace("/elsewhere", CANONICAL_MEDIA_ROOT),
        fixture.document().unwrap()
    );
}

/// The big presets: a second or two each in a release build, slow in a debug one, so they run
/// with the benchmarks (`cargo test -p cutweave-bench --release -- --ignored`) and through
/// `cargo xtask gen-bench-project --check`.
#[test]
#[ignore = "slow in debug builds"]
fn the_big_presets_generate_and_are_deterministic() {
    for spec in [FixtureSpec::p_400(), FixtureSpec::p_5k()] {
        let (a, b) = (generate(&spec).unwrap(), generate(&spec).unwrap());
        assert_eq!(a.sha256(), b.sha256(), "{}", spec.name);
        assert_eq!(a.item_count(), spec.items as usize);
        assert_eq!(keyframes(&a), spec.keyframes as usize);
    }
}
