// SPDX-License-Identifier: GPL-3.0-or-later
//! `edit.restore` over the fields that are left out of the file while empty or default
//! (ADR-0027, ADR-0035, ADR-0036): collections, tags, rating, note, proxy mode and policy, the
//! still duration and image sequences. The restore is a field-level diff from the document now
//! to the checkpointed one, so every such change has to be expressible as patch ops that hold
//! on the typed document and on its JSON form (the UI mirror), forwards and inverted.
//!
//! The first case is the shrunk failure `checkpoint_props` once found: the only collection is
//! deleted and another one created after the checkpoint.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::many_single_char_names
)]

use std::sync::{Arc, LazyLock};

use cutweave_commands::{
    ActorCtx, ConnId, DocConfig, DocState, Registry, Submit, apply_patch, apply_patch_json, diff,
    invert,
};
use cutweave_model::{
    BackgroundFill, BackgroundFit, BlurSource, Collection, CollectionId, CollectionKind, Duration,
    FrameRate, ImageSequence, MediaFilter, MediaId, MediaKind, Project, ProxyMode, ProxyPolicy,
};
use cutweave_ops::{
    media::FakeProbe,
    testing::{self, check_invariants},
};
use serde_json::{Value, json};

static REGISTRY: LazyLock<Arc<Registry>> = LazyLock::new(|| Arc::new(testing::registry()));

fn actor() -> ActorCtx {
    ActorCtx::ui(ConnId::new(1))
}

fn state() -> DocState {
    DocState::new(DocConfig::deterministic(
        Arc::clone(&REGISTRY),
        Project::new("Restore"),
        42,
    ))
}

fn ok(s: &mut DocState, name: &str, params: Value) -> Value {
    s.submit(Submit::new(actor(), name, params))
        .unwrap_or_else(|e| panic!("{name}: {e:?}"))
        .output
}

fn video(s: &mut DocState, path: &str) -> MediaId {
    let p = FakeProbe::video(path, 10, true);
    let out = ok(
        s,
        "media.register",
        json!({"path": path, "probe": p.probe, "content_hash": p.content_hash}),
    );
    serde_json::from_value(out["media_id"].clone()).unwrap()
}

fn collection(s: &mut DocState, name: &str, kind: Value) -> CollectionId {
    let out = ok(
        s,
        "media.create_collection",
        json!({"name": name, "kind": kind}),
    );
    serde_json::from_value(out["collection_id"].clone()).unwrap()
}

fn manual(s: &mut DocState, name: &str) -> CollectionId {
    collection(s, name, json!({"type": "manual", "media": []}))
}

/// Takes a checkpoint, runs `edits`, then: `edit.restore` gives the checkpointed document,
/// `edit.undo` the edited one and `edit.redo` the checkpointed one again.
fn restores(s: &mut DocState, edits: impl FnOnce(&mut DocState)) {
    let id = ok(s, "edit.checkpoint", json!({}))["checkpoint"]
        .as_str()
        .unwrap()
        .to_owned();
    let checkpointed = s.committed_doc().clone();
    edits(s);
    let edited = s.committed_doc().clone();
    assert_ne!(edited, checkpointed, "the edits changed nothing");

    let restored = ok(s, "edit.restore", json!({"checkpoint": id, "force": true}));
    assert!(!restored["tx"].is_null());
    assert_eq!(s.committed_doc(), &checkpointed, "restore");
    check_invariants(s.committed_doc()).unwrap();
    ok(s, "edit.undo", json!({}));
    assert_eq!(s.committed_doc(), &edited, "undo of the restore");
    ok(s, "edit.redo", json!({}));
    assert_eq!(s.committed_doc(), &checkpointed, "redo of the restore");
    s.verify().unwrap();
}

/// `diff` expresses `a → b` and `b → a`, and both patches and their inverses also hold on the
/// JSON form of the document.
fn expressible(a: &Project, b: &Project) {
    for (from, to) in [(a, b), (b, a)] {
        let patch = diff(from, to).unwrap_or_else(|e| panic!("diff: {e}"));
        assert_eq!(&apply_patch(from, &patch).unwrap(), to);
        assert_eq!(&apply_patch(to, &invert(&patch)).unwrap(), from);
        let mut mirror = serde_json::to_value(from).unwrap();
        apply_patch_json(&mut mirror, &patch).unwrap();
        assert_eq!(mirror, serde_json::to_value(to).unwrap());
        apply_patch_json(&mut mirror, &invert(&patch)).unwrap();
        assert_eq!(mirror, serde_json::to_value(from).unwrap());
    }
}

#[test]
fn the_only_collection_is_deleted_and_another_created() {
    let mut s = state();
    let first = manual(&mut s, "Selects");
    restores(&mut s, |s| {
        ok(s, "media.delete_collection", json!({"collection": first}));
        manual(s, "B-roll");
    });
}

#[test]
fn every_collection_is_replaced_by_others() {
    let mut s = state();
    let a = manual(&mut s, "A");
    let b = collection(
        &mut s,
        "B",
        json!({"type": "smart", "filter": {"used": false}}),
    );
    restores(&mut s, |s| {
        ok(s, "media.delete_collection", json!({"collection": a}));
        ok(s, "media.delete_collection", json!({"collection": b}));
        // The same names again: the restore passes through neither a clash nor an empty map.
        manual(s, "B");
        manual(s, "a");
        manual(s, "C");
    });
}

#[test]
fn the_first_collection_is_created() {
    let mut s = state();
    restores(&mut s, |s| {
        manual(s, "Selects");
    });
}

#[test]
fn the_last_collection_is_deleted() {
    let mut s = state();
    let only = manual(&mut s, "Selects");
    restores(&mut s, |s| {
        ok(s, "media.delete_collection", json!({"collection": only}));
    });
}

#[test]
fn one_of_several_collections_is_created_or_deleted() {
    let mut s = state();
    let a = manual(&mut s, "A");
    restores(&mut s, |s| {
        manual(s, "B");
    });
    let b = manual(&mut s, "B");
    restores(&mut s, |s| {
        ok(s, "media.delete_collection", json!({"collection": a}));
    });
    restores(&mut s, |s| {
        ok(s, "media.delete_collection", json!({"collection": b}));
        manual(s, "C");
    });
}

#[test]
fn a_collection_is_edited() {
    let mut s = state();
    let (m1, m2) = (video(&mut s, "/m/a.mp4"), video(&mut s, "/m/b.mp4"));
    let list = manual(&mut s, "Selects");
    let search = collection(
        &mut s,
        "Long",
        json!({"type": "smart", "filter": {"kinds": ["video"], "used": false}}),
    );
    restores(&mut s, |s| {
        ok(
            s,
            "media.add_to_collection",
            json!({"collection": list, "media": [m1, m2]}),
        );
        ok(
            s,
            "media.rename_collection",
            json!({"collection": list, "name": "Best"}),
        );
        ok(
            s,
            "media.set_collection_filter",
            json!({"collection": search, "filter": {"text": "take", "min_rating": 3}}),
        );
    });
    ok(
        &mut s,
        "media.add_to_collection",
        json!({"collection": list, "media": [m1, m2]}),
    );
    restores(&mut s, |s| {
        ok(
            s,
            "media.remove_from_collection",
            json!({"collection": list, "media": [m1, m2]}),
        );
        ok(
            s,
            "media.set_collection_filter",
            json!({"collection": search, "filter": {}}),
        );
    });
}

#[test]
fn tags_rating_and_note_appear_and_disappear() {
    let mut s = state();
    let m = video(&mut s, "/m/a.mp4");
    // Absent, then present.
    restores(&mut s, |s| {
        ok(
            s,
            "media.add_tags",
            json!({"media": [m], "tags": ["interview"]}),
        );
        ok(s, "media.set_rating", json!({"media": [m], "rating": 4}));
        ok(
            s,
            "media.set_note",
            json!({"media": m, "note": "Good take."}),
        );
    });
    ok(
        &mut s,
        "media.add_tags",
        json!({"media": [m], "tags": ["interview", "b-roll"]}),
    );
    ok(
        &mut s,
        "media.set_rating",
        json!({"media": [m], "rating": 4}),
    );
    ok(
        &mut s,
        "media.set_note",
        json!({"media": m, "note": "Good take."}),
    );
    // Present, then changed.
    restores(&mut s, |s| {
        ok(
            s,
            "media.remove_tags",
            json!({"media": [m], "tags": ["b-roll"]}),
        );
        ok(s, "media.set_rating", json!({"media": [m], "rating": 1}));
        ok(s, "media.set_note", json!({"media": m, "note": "No."}));
    });
    // Present, then absent.
    restores(&mut s, |s| {
        ok(
            s,
            "media.remove_tags",
            json!({"media": [m], "tags": ["interview", "b-roll"]}),
        );
        ok(s, "media.set_rating", json!({"media": [m], "rating": null}));
        ok(s, "media.set_note", json!({"media": m, "note": null}));
    });
}

#[test]
fn proxy_mode_and_policy_leave_and_return_to_auto() {
    let mut s = state();
    let m = video(&mut s, "/m/a.mp4");
    restores(&mut s, |s| {
        ok(
            s,
            "media.set_proxy_mode",
            json!({"media": [m], "mode": "force"}),
        );
        ok(s, "media.set_proxy_policy", json!({"policy": "all"}));
    });
    ok(
        &mut s,
        "media.set_proxy_mode",
        json!({"media": [m], "mode": "off"}),
    );
    ok(&mut s, "media.set_proxy_policy", json!({"policy": "off"}));
    restores(&mut s, |s| {
        ok(
            s,
            "media.set_proxy_mode",
            json!({"media": [m], "mode": "auto"}),
        );
        ok(s, "media.set_proxy_policy", json!({"policy": "auto"}));
    });
    restores(&mut s, |s| {
        ok(
            s,
            "media.set_proxy_mode",
            json!({"media": [m], "mode": "force"}),
        );
        ok(s, "media.set_proxy_policy", json!({"policy": "all"}));
    });
}

#[test]
fn a_new_project_with_a_still_duration_and_a_policy() {
    let mut s = state();
    restores(&mut s, |s| {
        ok(
            s,
            "project.new",
            json!({"name": "Stills", "still_duration": "2s", "proxy_policy": "off"}),
        );
    });
}

/// WP-2B.02: a sequence's background fill is left out of the file while it is the plain color.
#[test]
fn a_background_fill_appears_changes_kind_and_disappears() {
    let mut s = state();
    let created = ok(
        &mut s,
        "sequence.create",
        json!({"name": "Vertical", "preset": "9:16"}),
    );
    let sequence: cutweave_model::SequenceId =
        serde_json::from_value(created["sequence"].clone()).unwrap();
    let still = {
        let p = FakeProbe::image("/m/paper.png");
        let out = ok(
            &mut s,
            "media.register",
            json!({"path": "/m/paper.png", "probe": p.probe, "content_hash": p.content_hash}),
        );
        serde_json::from_value::<MediaId>(out["media_id"].clone()).unwrap()
    };
    let set = |s: &mut DocState, params: Value| {
        let mut params = params;
        params["sequence"] = json!(sequence);
        ok(s, "sequence.set_background", params);
    };
    // None → blur, then the blur's numbers, then another kind, then none again.
    restores(&mut s, |s| set(s, json!({"kind": "blur"})));
    set(&mut s, json!({"kind": "blur", "radius": 0.1}));
    restores(&mut s, |s| {
        set(
            s,
            json!({"kind": "blur", "radius": 0.02, "dim": 0.5, "source": "main_track"}),
        );
    });
    restores(&mut s, |s| set(s, json!({"kind": "image", "media": still})));
    restores(&mut s, |s| set(s, json!({"kind": "transparent"})));
    restores(&mut s, |s| {
        set(s, json!({"kind": "color", "color": "#336699"}))
    });
    set(
        &mut s,
        json!({"kind": "image", "media": still, "fit": "contain"}),
    );
    restores(&mut s, |s| set(s, json!({"kind": "color"})));
    restores(&mut s, |s| {
        set(s, json!({"kind": "image", "fit": "stretch"}))
    });

    // Document to document, with the JSON mirror: every pair of fills.
    let base = s.committed_doc().clone();
    let with = |fill: Option<BackgroundFill>| {
        let mut doc = base.clone();
        let mut seq = doc.sequences[&sequence].clone();
        seq.format.background_fill = fill;
        doc.sequences.insert(sequence, seq);
        doc
    };
    let fills = [
        with(None),
        with(Some(BackgroundFill::Transparent)),
        with(Some(BackgroundFill::Blur {
            source: BlurSource::Below,
            radius: 0.05,
            dim: 0.25,
        })),
        with(Some(BackgroundFill::Blur {
            source: BlurSource::MainTrack,
            radius: 0.5,
            dim: 0.0,
        })),
        with(Some(BackgroundFill::Image {
            media: still,
            fit: BackgroundFit::Cover,
        })),
        with(Some(BackgroundFill::Image {
            media: MediaId::from_u128(7),
            fit: BackgroundFit::Stretch,
        })),
    ];
    for (i, x) in fills.iter().enumerate() {
        for y in &fills[i + 1..] {
            expressible(x, y);
        }
    }
}

/// The same shapes, document to document: no command sequence is needed to reach them, and
/// the JSON mirror is checked too.
#[test]
fn optional_fields_diff_in_both_directions() {
    let mut s = state();
    let m = video(&mut s, "/m/a.mp4");
    let base = s.committed_doc().clone();
    let with = |edit: &dyn Fn(&mut Project)| {
        let mut doc = base.clone();
        edit(&mut doc);
        doc
    };
    let coll = |n: u128, name: &str, kind: CollectionKind| {
        let id = CollectionId::from_u128(n);
        (id, Collection::new(id, name, kind))
    };
    let (a, b, c) = (
        coll(1, "A", CollectionKind::manual()),
        coll(2, "B", CollectionKind::Manual { media: vec![m] }),
        coll(
            3,
            "C",
            CollectionKind::Smart {
                filter: MediaFilter {
                    used: Some(true),
                    ..MediaFilter::default()
                },
            },
        ),
    );
    let of = |entries: &[&(CollectionId, Collection)]| {
        with(&|d| {
            for (id, c) in entries {
                d.collections.insert(*id, (*c).clone());
            }
        })
    };

    // Collections: every pair of none, one, another one, two and all three.
    let sets = [
        of(&[]),
        of(&[&a]),
        of(&[&b]),
        of(&[&c]),
        of(&[&a, &b]),
        of(&[&b, &c]),
        of(&[&a, &b, &c]),
    ];
    for (i, x) in sets.iter().enumerate() {
        for y in &sets[i + 1..] {
            expressible(x, y);
        }
    }
    // One collection that changes kind (no command does it; a restore still has to cope).
    let (id, mut smart) = a.clone();
    smart.kind = c.1.kind.clone();
    expressible(&of(&[&a]), &of(&[&(id, smart)]));

    // The asset's optional fields.
    let asset = |edit: &dyn Fn(&mut cutweave_model::MediaAsset)| {
        with(&|d| {
            let mut asset = d.media[&m].clone();
            edit(&mut asset);
            d.media.insert(m, asset);
        })
    };
    let run = |count: u32, missing: Vec<u32>| ImageSequence {
        pattern: "shot_%04d.png".into(),
        first: 1,
        count,
        fps: FrameRate::FPS_30,
        missing,
        extra: cutweave_model::Extra::new(),
    };
    let variants = [
        asset(&|a| a.tags = vec!["interview".into()]),
        asset(&|a| a.tags = vec!["b-roll".into(), "interview".into()]),
        asset(&|a| a.rating = Some(5)),
        asset(&|a| a.rating = Some(1)),
        asset(&|a| a.note = Some("Good take.".into())),
        asset(&|a| a.note = Some("No.".into())),
        asset(&|a| a.proxy_mode = ProxyMode::Force),
        asset(&|a| a.proxy_mode = ProxyMode::Off),
        asset(&|a| {
            a.kind = MediaKind::ImageSequence;
            a.image_sequence = Some(run(10, vec![]));
        }),
        asset(&|a| {
            a.kind = MediaKind::ImageSequence;
            a.image_sequence = Some(run(12, vec![3, 4]));
        }),
        with(&|d| d.settings.still_duration = Duration::from_secs(2).ok()),
        with(&|d| d.settings.still_duration = Duration::from_secs(9).ok()),
        with(&|d| d.settings.proxy_policy = ProxyPolicy::Off),
        with(&|d| d.settings.proxy_policy = ProxyPolicy::All),
    ];
    for (i, x) in variants.iter().enumerate() {
        expressible(&base, x);
        for y in &variants[i + 1..] {
            expressible(x, y);
        }
    }
}
