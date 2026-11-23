// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.08 acceptance: `--deterministic-ids <seed>` gives byte-identical saved documents
//! across fresh engines (the double-run), different ids but the same structure for another
//! seed, and no id collisions when a saved project is reopened in a new engine.

#![allow(clippy::unwrap_used)] // helpers and closures, not only `#[test]` bodies

mod engine_support;

use std::collections::BTreeSet;

use cutweave_api::Credential;
use cutweave_engine::determinism::{Run, Step, double_run, normalize_ids, run_script};
use cutweave_engine::project_io::ProjectStore;
use cutweave_engine::{Engine, EngineConfig, EngineMode};
use cutweave_model::{Duration, Extra, ProbeInfo, StreamInfo, StreamKind};
use engine_support::{TempDir, must};
use serde_json::{Value, json};

fn video_probe() -> Value {
    let stream = |index, kind| StreamInfo {
        index,
        kind,
        codec: None,
        profile: None,
        width: None,
        height: None,
        pixel_format: None,
        sample_rate: None,
        channels: None,
        language: None,
        extra: Extra::default(),
    };
    let probe = ProbeInfo {
        streams: vec![stream(0, StreamKind::Video), stream(1, StreamKind::Audio)],
        duration: Some(Duration::from_secs(20).unwrap()),
        ..ProbeInfo::default()
    };
    serde_json::to_value(probe).unwrap()
}

/// A script touching every id-minting path: project, tracks, media, linked clips, a split.
fn script() -> Vec<Step> {
    vec![
        Step::fixed("project.new", json!({ "name": "Det" })),
        Step::fixed("project.rename", json!({ "name": "Det film" })),
        Step::new(
            "track.add",
            |prev| json!({ "sequence": prev[0]["sequence"], "kind": "video" }),
        ),
        Step::fixed(
            "media.register",
            json!({ "path": "/media/a.mp4", "probe": video_probe(), "content_hash": "h-a" }),
        ),
        Step::new("timeline.insert", |prev| {
            json!({
                "sequence": prev[0]["sequence"],
                "track": prev[0]["video_track"],
                "media": prev[3]["media_id"],
                "at": "0s",
                "duration": "5s",
            })
        }),
        Step::new("timeline.insert", |prev| {
            json!({
                "sequence": prev[0]["sequence"],
                "track": prev[2]["track"],
                "media": prev[3]["media_id"],
                "at": "1s",
                "duration": "4s",
            })
        }),
        Step::new(
            "clip.split",
            |prev| json!({ "item": prev[4]["clips"][0], "at": "2s" }),
        ),
    ]
}

fn sha(run: &Run) -> &str {
    &run.sha256
}

#[test]
fn the_same_script_in_two_fresh_engines_saves_identical_bytes() {
    let t = TempDir::new("det-double");
    let steps = script();
    let (a, b) = double_run(42, t.path(), "Det", &steps);
    assert_eq!(a.document, b.document);
    assert_eq!(sha(&a), sha(&b));
    assert_eq!(a.outputs, b.outputs);
    assert_eq!(sha(&a).len(), 64);
    let text = String::from_utf8(a.document.clone()).unwrap();
    // The creation stamp is frozen at the Unix epoch.
    assert!(text.contains("1970-01-01T00:00:00Z"), "{text}");
    // Several entities were minted (the script is not trivially empty).
    assert!(normalize_ids(&text).contains("itm_4"), "{text}");
}

#[test]
fn another_seed_changes_the_ids_but_not_the_structure() {
    let t = TempDir::new("det-seeds");
    let steps = script();
    let a = run_script(42, t.path(), "Det", &steps);
    let c = run_script(43, t.path(), "Det", &steps);
    assert_ne!(a.sha256, c.sha256);
    assert_ne!(a.document, c.document);
    assert_eq!(a.normalized_document(), c.normalized_document());
}

fn ids_in(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    // Collect the raw ids by scanning for the prefixes again.
    for prefix in ["med_", "seq_", "trk_", "itm_", "grp_"] {
        let mut rest = text;
        while let Some(i) = rest.find(prefix) {
            let cand = &rest[i..];
            if cand.len() >= prefix.len() + 32
                && cand[prefix.len()..prefix.len() + 32]
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit())
            {
                out.insert(cand[..prefix.len() + 32].to_owned());
            }
            rest = &rest[i + prefix.len()..];
        }
    }
    out
}

/// Opens `folder` in a new deterministic engine, mints more entities, saves, returns the
/// document text and the ids the new session minted.
fn reopen_and_extend(
    seed: u64,
    dir: &std::path::Path,
    folder: &std::path::Path,
) -> (String, Value) {
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.deterministic_ids = Some(seed);
    config.allowed_roots = vec![dir.to_path_buf()];
    config.project = Some(folder.to_path_buf());
    let engine = Engine::start(config).unwrap();
    let conn = engine
        .handle()
        .connect(&Credential::Agent("reopen".into()))
        .unwrap();
    let info = must(&conn, "project.info", json!({}));
    let seq = info["sequences"][0]["id"].clone();
    let t1 = must(
        &conn,
        "track.add",
        json!({ "sequence": seq, "kind": "video" }),
    );
    let t2 = must(
        &conn,
        "track.add",
        json!({ "sequence": seq, "kind": "audio" }),
    );
    let media = must(
        &conn,
        "media.register",
        json!({ "path": "/media/b.mp4", "probe": video_probe(), "content_hash": "h-b" }),
    );
    let minted = json!([t1["track"], t2["track"], media["media_id"]]);
    must(&conn, "project.save", json!({}));
    engine.shutdown();
    let store = ProjectStore::open(folder).unwrap();
    let path = store.store.layout().document.clone();
    drop(store);
    (std::fs::read_to_string(path).unwrap(), minted)
}

#[test]
fn reopening_a_saved_project_in_a_new_engine_never_reuses_an_id() {
    reopen_never_reuses_an_id(false);
}

/// The known gap of WP-1I.04: no journal transactions (a copied or cleanly closed project)
/// means no recorded id state, so only the document itself says which ids exist.
#[test]
fn reopening_without_any_journal_never_reuses_an_id() {
    reopen_never_reuses_an_id(true);
}

fn reopen_never_reuses_an_id(wipe_journal: bool) {
    let run_once = || {
        let t = TempDir::new("det-reopen");
        let first = run_script(42, t.path(), "Det", &script());
        let before = ids_in(&String::from_utf8(first.document).unwrap());
        let folder = t.path().join("Det");
        if wipe_journal {
            let journal = folder.join(".cutweave").join("journal");
            assert!(journal.exists(), "the saved project has a journal dir");
            std::fs::remove_dir_all(&journal).unwrap();
        }
        let (text, minted) = reopen_and_extend(42, t.path(), &folder);
        let after = ids_in(&text);
        // The three new ids are in the document, none was there before.
        for id in minted.as_array().unwrap() {
            let id = id.as_str().unwrap().to_owned();
            assert!(
                !before.contains(&id),
                "{id} collides with an id minted earlier"
            );
            assert!(after.contains(&id), "{id} missing from the saved document");
        }
        assert_eq!(after.len(), before.len() + 3);
        (text, minted)
    };
    let a = run_once();
    let b = run_once();
    assert_eq!(a, b, "the reopened run is reproducible");
}
