// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.01: `Tx` records invertible ops for every mutation; patches replay and invert exactly
//! on the typed document and on its JSON mirror.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{SplitClip, dur, fixture, secs};
use cutweave_commands::{
    Access, CmdError, DocPath, EntityRef, Patch, PatchError, PatchOp, Registry, Transaction,
    apply_patch, apply_patch_json, diff, invert, json_eq,
};
use cutweave_model::{
    Clip, EntityKind, IdGen, Item, ItemId, MediaAsset, MediaKind, ProbeInfo, Project, SeededGen,
    Sequence, SequenceFormat, TimeInput, Track, TrackKind,
};
use serde_json::{Value, json};

fn to_json(doc: &Project) -> Value {
    serde_json::to_value(doc).unwrap()
}

/// The patch reproduces `after` from `before` on the typed document and on its JSON mirror,
/// and its inverse restores `before`.
fn assert_round_trip(before: &Project, after: &Project, patch: &Patch) {
    assert_eq!(&apply_patch(before, patch).unwrap(), after, "forward");
    assert_eq!(
        &apply_patch(after, &invert(patch)).unwrap(),
        before,
        "inverse"
    );
    let mut mirror = to_json(before);
    apply_patch_json(&mut mirror, patch).unwrap();
    assert!(json_eq(&mirror, &to_json(after)), "JSON mirror");
    // The patch survives the wire.
    let wire: Patch = serde_json::from_value(serde_json::to_value(patch).unwrap()).unwrap();
    assert_eq!(
        &apply_patch(before, &wire).unwrap(),
        after,
        "after a JSON round trip"
    );
}

fn split(doc: &Project, ids: &mut SeededGen, item: ItemId, at: TimeInput) -> (Transaction, Value) {
    let mut registry = Registry::new();
    registry.add_command::<SplitClip>().unwrap();
    let mut txn = Transaction::begin(doc.clone());
    let out = registry
        .invoke_json(
            "clip.split",
            json!({ "item": item, "at": at }),
            &mut txn.tx(ids, Access::STANDARD),
        )
        .unwrap();
    (txn, out)
}

#[test]
fn hand_written_command_round_trips_through_its_patch() {
    // WP-1B.01 acceptance: apply_patch(apply_patch(doc, forward), invert(forward)) == doc.
    let (doc, ids, mut gen_) = fixture();
    let (txn, out) = split(&doc, &mut gen_, ids.b, TimeInput::Frame(210));
    let committed = txn.commit();
    let forward = &committed.patch;
    let after = apply_patch(&doc, forward).unwrap();
    assert_eq!(after, committed.doc);
    assert_eq!(apply_patch(&after, &invert(forward)).unwrap(), doc);
    assert_eq!(committed.inverse, invert(forward));
    assert_round_trip(&doc, &committed.doc, forward);

    // The split itself: left keeps its start, right starts at frame 210 (7 s) with a
    // contiguous source range, and the result is still a valid document.
    let right: ItemId = serde_json::from_value(out["right"].clone()).unwrap();
    let after = committed.doc;
    after.validate().unwrap();
    let left = after
        .find_item(&ids.b)
        .unwrap()
        .item
        .as_clip()
        .unwrap()
        .clone();
    let right = after
        .find_item(&right)
        .unwrap()
        .item
        .as_clip()
        .unwrap()
        .clone();
    assert_eq!((left.start, left.duration), (secs(4), dur(3)));
    assert_eq!((right.start, right.duration), (secs(7), dur(3)));
    assert_eq!(right.source_in, secs(13));
    // Field-level ops: the left half changes only its duration.
    let left_path = DocPath::item(ids.sequence, ids.video, ids.b);
    assert!(forward.iter().any(|op| matches!(op,
        PatchOp::Replace { path, .. } if *path == left_path.join("duration"))));
    assert_eq!(
        forward.touched_items(),
        [ids.b, right.id].into_iter().collect()
    );
}

#[test]
fn every_tx_mutator_records_invertible_ops() {
    let (doc, ids, mut gen_) = fixture();
    let mut txn = Transaction::begin(doc.clone());
    let mut tx = txn.tx(&mut gen_, Access::STANDARD);

    // Media.
    let media = MediaAsset::file(
        tx.new_id(),
        MediaKind::Audio,
        "/m/b.wav",
        ProbeInfo::default(),
    );
    tx.insert_media(&media).unwrap();
    tx.update_media(media.id, |m| {
        m.path_rel = Some("b.wav".into());
        Ok(())
    })
    .unwrap();
    // Sequences and tracks.
    let seq = Sequence::new(tx.new_id(), "Shorts", SequenceFormat::default());
    tx.insert_sequence(&seq).unwrap();
    tx.update_sequence(seq.id, |s| {
        s.name = "Shorts 9:16".into();
        s.format.width = 1080;
        s.format.height = 1920;
        Ok(())
    })
    .unwrap();
    let v2 = Track::new(tx.new_id(), TrackKind::Video, "V2");
    tx.insert_track(ids.sequence, 0, &v2).unwrap();
    tx.update_track(ids.audio, |t| {
        t.muted = true;
        t.gain = -6.0;
        Ok(())
    })
    .unwrap();
    // Items.
    let c = Clip::new(tx.new_id(), ids.media, secs(20), dur(2));
    tx.insert_item(v2.id, &c.clone().into()).unwrap();
    tx.update_item(ids.a, |i| {
        i.set_enabled(false);
        let clip = i.as_clip_mut().unwrap();
        clip.visual.opacity.set(0.5);
        clip.name = Some("intro".into());
        Ok(())
    })
    .unwrap();
    tx.move_item(c.id, ids.video).unwrap();
    let removed = tx.remove_item(ids.b).unwrap();
    assert_eq!(removed.id(), ids.b);
    // Project-level and generic.
    tx.update_meta(|m| {
        m.name = "Renamed".into();
        Ok(())
    })
    .unwrap();
    tx.set(
        &DocPath::custom().join("com.example.test"),
        json!({"k": [1, 2]}),
    )
    .unwrap();
    tx.set(&DocPath::meta().join("future_field"), json!(true))
        .unwrap();
    let old = tx
        .remove_path(&DocPath::meta().join("future_field"))
        .unwrap();
    assert_eq!(old, json!(true));
    tx.remove_track(v2.id).unwrap();
    tx.remove_sequence(seq.id).unwrap();
    tx.remove_media(media.id).unwrap();

    let recorded = Patch(txn.ops().to_vec());
    let committed = txn.commit();
    assert_round_trip(&doc, &committed.doc, &recorded);
    assert_round_trip(&doc, &committed.doc, &committed.patch);
    // Coalesced: entities added then removed in the same transaction leave no trace.
    assert!(committed.patch.len() < recorded.len());
    let touched = committed.patch.touched();
    assert!(!touched.contains(&EntityRef::Media(media.id)));
    assert!(!touched.contains(&EntityRef::Sequence(seq.id)));
    let after = &committed.doc;
    assert_eq!(after.meta.name, "Renamed");
    let a = after.find_item(&ids.a).unwrap().item.as_clip().unwrap();
    assert_eq!(*a.visual.opacity.value(), 0.5);
    assert!(!a.enabled);
    assert_eq!(after.find_item(&c.id).unwrap().track.id, ids.video);
    assert!(after.find_item(&ids.b).is_none());
    after.validate().unwrap();
}

#[test]
fn track_insertion_and_removal_keep_the_display_order() {
    let (doc, ids, mut gen_) = fixture();
    let mut txn = Transaction::begin(doc.clone());
    let mut tx = txn.tx(&mut gen_, Access::STANDARD);
    let top = Track::new(tx.new_id(), TrackKind::Video, "V2");
    let bottom = Track::new(tx.new_id(), TrackKind::Audio, "A2");
    tx.insert_track(ids.sequence, 0, &top).unwrap();
    tx.insert_track(ids.sequence, 99, &bottom).unwrap();
    let order = |doc: &Project| -> Vec<String> {
        doc.sequences[&ids.sequence]
            .tracks_in_order()
            .map(|t| t.name.clone())
            .collect()
    };
    assert_eq!(order(tx.doc()), ["V2", "V1", "A1", "A2"]);
    tx.remove_track(ids.video).unwrap();
    assert_eq!(order(tx.doc()), ["V2", "A1", "A2"]);
    tx.doc().validate().unwrap();
    let committed = txn.commit();
    assert_round_trip(&doc, &committed.doc, &committed.patch);
    // Duplicate IDs are refused.
    let mut txn = Transaction::begin(committed.doc);
    let mut tx = txn.tx(&mut gen_, Access::STANDARD);
    let err = tx.insert_track(ids.sequence, 0, &top).unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(err.message().contains("already exists"), "{err}");
}

#[test]
fn locked_tracks_refuse_item_edits_but_not_unlocking() {
    let (mut doc, ids, mut gen_) = fixture();
    doc.sequences
        .get_mut(&ids.sequence)
        .unwrap()
        .track_mut(&ids.video)
        .unwrap()
        .locked = true;
    let mut txn = Transaction::begin(doc.clone());
    let mut tx = txn.tx(&mut gen_, Access::STANDARD);
    let locked = CmdError::locked(EntityKind::Track, ids.video);
    assert_eq!(
        tx.update_item(ids.a, |i| {
            i.set_enabled(false);
            Ok(())
        }),
        Err(locked.clone())
    );
    assert_eq!(tx.remove_item(ids.a), Err(locked.clone()));
    assert_eq!(tx.move_item(ids.a, ids.audio), Err(locked.clone()));
    let c = Clip::new(tx.new_id(), ids.media, secs(30), dur(1));
    assert_eq!(tx.insert_item(ids.video, &c.into()), Err(locked.clone()));
    assert_eq!(
        tx.set(
            &DocPath::item(ids.sequence, ids.video, ids.a).join("enabled"),
            false
        ),
        Err(locked.clone())
    );
    assert_eq!(tx.remove_track(ids.video).map(|_| ()), Err(locked));
    // Failed mutators changed nothing.
    assert!(txn.is_empty());
    assert_eq!(txn.doc(), &doc);

    // A locked track's own fields can change, and unlocking then editing in one go works.
    let mut tx = txn.tx(&mut gen_, Access::STANDARD);
    tx.update_track(ids.video, |t| {
        t.name = "Locked V1".into();
        Ok(())
    })
    .unwrap();
    tx.update_track(ids.video, |t| {
        t.locked = false;
        t.items.update(&ids.a, |i| i.set_enabled(false));
        Ok(())
    })
    .unwrap();
    let committed = txn.commit();
    assert_round_trip(&doc, &committed.doc, &committed.patch);
}

#[test]
fn savepoints_and_failed_commands_roll_back() {
    let (doc, ids, mut gen_) = fixture();
    let mut registry = Registry::new();
    registry
        .add_command::<SplitClip>()
        .unwrap()
        .add_command::<common::RenameThenFail>()
        .unwrap();
    let mut txn = Transaction::begin(doc.clone());
    let mark = txn.mark();
    {
        let mut tx = txn.tx(&mut gen_, Access::STANDARD);
        registry
            .invoke_json("clip.split", json!({"item": ids.a, "at": "2s"}), &mut tx)
            .unwrap();
        // A command that fails after changing something is rolled back to where it started.
        let err = registry
            .invoke_json("project.rename", json!({"name": "fail"}), &mut tx)
            .unwrap_err();
        assert_eq!(err, CmdError::invalid("/name", "refused after renaming"));
    }
    assert_eq!(txn.doc().meta.name, "Fixture");
    assert_eq!(txn.ops_since(&mark).len(), txn.ops().len());
    let after_split = txn.doc().clone();
    let inner = txn.mark();
    txn.tx(&mut gen_, Access::STANDARD)
        .update_meta(|m| {
            m.name = "Tmp".into();
            Ok(())
        })
        .unwrap();
    assert_eq!(txn.ops_since(&inner).len(), 1);
    txn.rollback_to(inner);
    assert_eq!(txn.doc(), &after_split);
    txn.rollback_to(mark);
    assert_eq!(txn.doc(), &doc);
    assert!(txn.is_empty());
    assert!(txn.clone().commit().is_empty());
    assert_eq!(txn.cancel(), doc);
}

#[test]
fn gesture_updates_coalesce_into_one_op_per_field() {
    let (doc, ids, mut gen_) = fixture();
    let mut txn = Transaction::begin(doc.clone());
    for step in 1..=200 {
        txn.tx(&mut gen_, Access::STANDARD)
            .update_item(ids.b, |i| {
                i.set_start(cutweave_model::Time::from_flicks(4 * 705_600_000 + step).unwrap());
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(txn.ops().len(), 200);
    let committed = txn.commit();
    assert_eq!(committed.patch.len(), 1);
    assert_round_trip(&doc, &committed.doc, &committed.patch);
}

#[test]
fn stale_patches_conflict_instead_of_clobbering() {
    let (doc, ids, mut gen_) = fixture();
    let (txn, _) = split(&doc, &mut gen_, ids.a, TimeInput::Absolute(secs(1)));
    let agent = txn.commit();
    // Someone else then changes a field the agent's split also changed.
    let mut txn = Transaction::begin(agent.doc.clone());
    txn.tx(&mut gen_, Access::STANDARD)
        .update_item(ids.a, |i| {
            i.set_duration(dur(2));
            Ok(())
        })
        .unwrap();
    let user = txn.commit();
    let err = apply_patch(&user.doc, &agent.inverse).unwrap_err();
    assert!(matches!(err, PatchError::Mismatch { .. }), "{err}");
    // While an edit of an unrelated field leaves the agent's undo applicable.
    let mut txn = Transaction::begin(agent.doc.clone());
    txn.tx(&mut gen_, Access::STANDARD)
        .update_item(ids.a, |i| {
            i.set_enabled(false);
            Ok(())
        })
        .unwrap();
    let user = txn.commit();
    let undone = apply_patch(&user.doc, &agent.inverse).unwrap();
    let a = undone.find_item(&ids.a).unwrap().item;
    assert_eq!(a.duration(), dur(4));
    assert!(!a.enabled());
}

#[test]
fn typed_application_rejects_what_the_model_cannot_hold() {
    let (doc, ids, _) = fixture();
    let item = DocPath::item(ids.sequence, ids.video, ids.a);
    let get = |op: PatchOp| apply_patch(&doc, &Patch(vec![op])).unwrap_err();
    let err = get(PatchOp::Replace {
        path: item.join("id"),
        old: json!(ids.a),
        value: json!(ids.b),
    });
    assert!(matches!(err, PatchError::Immutable { .. }), "{err}");
    let err = get(PatchOp::Replace {
        path: item.join("enabled"),
        old: json!(true),
        value: json!("yes"),
    });
    assert!(matches!(err, PatchError::Type { .. }), "{err}");
    // Removing an optional field would read back as null: not representable.
    let err = get(PatchOp::Remove {
        path: item.join("name"),
        old: Value::Null,
    });
    assert!(matches!(err, PatchError::Type { .. }), "{err}");
    let err = get(PatchOp::Add {
        path: DocPath::parse(&format!(
            "/sequences/{}/tracks/{}/items/nope",
            ids.sequence, ids.video
        ))
        .unwrap(),
        value: json!({}),
    });
    assert!(matches!(err, PatchError::InvalidPath { .. }), "{err}");
    // An entity whose id differs from its key.
    let mut b = serde_json::to_value(doc.find_item(&ids.b).unwrap().item).unwrap();
    b["start"] = json!(705_600_000_i64 * 30);
    let err = get(PatchOp::Add {
        path: DocPath::item(ids.sequence, ids.audio, ItemId::from_u128(99)),
        value: b,
    });
    assert!(matches!(err, PatchError::Type { .. }), "{err}");
    let err = get(PatchOp::Remove {
        path: DocPath::root().join("media"),
        old: json!({}),
    });
    assert!(matches!(err, PatchError::Type { .. }), "{err}");
    let err = get(PatchOp::Remove {
        path: item.join("start"),
        old: json!(1),
    });
    assert!(matches!(err, PatchError::Mismatch { .. }), "{err}");
}

#[test]
fn moves_between_tracks_and_collections() {
    let (doc, ids, _) = fixture();
    let from = DocPath::item(ids.sequence, ids.video, ids.b);
    let to = DocPath::item(ids.sequence, ids.audio, ids.b);
    let patch = Patch(vec![PatchOp::Move { from, path: to }]);
    let after = apply_patch(&doc, &patch).unwrap();
    assert_eq!(after.find_item(&ids.b).unwrap().track.id, ids.audio);
    assert_round_trip(&doc, &after, &patch);
    // Into another item's slot, or into itself: refused, nothing changes.
    let bad = Patch(vec![PatchOp::Move {
        from: DocPath::item(ids.sequence, ids.video, ids.a),
        path: DocPath::item(ids.sequence, ids.video, ids.b),
    }]);
    assert!(matches!(
        apply_patch(&doc, &bad),
        Err(PatchError::Type { .. } | PatchError::AlreadyExists { .. })
    ));
}

#[test]
fn whole_entities_and_root_replace() {
    let (doc, ids, mut gen_) = fixture();
    let mut other = doc.clone();
    other.meta.name = "Other".into();
    other
        .sequences
        .get_mut(&ids.sequence)
        .unwrap()
        .track_mut(&ids.audio)
        .unwrap()
        .items
        .insert(Clip::new(gen_.mint(), ids.media, secs(0), dur(1)).into());
    let patch = Patch(vec![PatchOp::Replace {
        path: DocPath::root(),
        old: to_json(&doc),
        value: to_json(&other),
    }]);
    assert_round_trip(&doc, &other, &patch);
    let field_level = diff(&doc, &other).unwrap();
    assert_eq!(field_level.len(), 2, "{field_level:?}");
    assert_round_trip(&doc, &other, &field_level);
    // Whole-collection replace.
    let items = DocPath::track(ids.sequence, ids.audio).join("items");
    let patch = Patch(vec![PatchOp::Replace {
        path: items.clone(),
        old: json!({}),
        value: to_json(&other)["sequences"][ids.sequence.to_string()]["tracks"]
            [ids.audio.to_string()]["items"]
            .clone(),
    }]);
    let mut expected = doc.clone();
    expected
        .sequences
        .get_mut(&ids.sequence)
        .unwrap()
        .track_mut(&ids.audio)
        .unwrap()
        .items = other.sequences[&ids.sequence].tracks[&ids.audio]
        .items
        .clone();
    assert_round_trip(&doc, &expected, &patch);
}

#[test]
fn reads_see_the_working_document() {
    let (doc, ids, mut gen_) = fixture();
    let mut txn = Transaction::begin(doc);
    let mut tx = txn.tx(&mut gen_, Access::UI);
    assert_eq!(tx.access(), Access::UI);
    assert_eq!(
        tx.get(&DocPath::item(ids.sequence, ids.video, ids.a).join("start")),
        Some(json!(0))
    );
    assert_eq!(
        tx.get(&DocPath::track(ids.sequence, ids.video).join("name")),
        Some(json!("V1"))
    );
    assert_eq!(
        tx.get(&DocPath::meta().join("name")),
        Some(json!("Fixture"))
    );
    assert_eq!(tx.get(&DocPath::meta().join("nope")), None);
    assert_eq!(
        tx.get(&DocPath::sequence(ids.sequence).join("tracks").join("x")),
        None
    );
    tx.update_item(ids.a, |i| {
        i.set_start(secs(1));
        Ok(())
    })
    .unwrap();
    assert_eq!(tx.item(ids.a).unwrap().item.start(), secs(1));
    assert_eq!(tx.query_ctx().item(ids.a).unwrap().item.start(), secs(1));
    assert_eq!(
        tx.item(ItemId::from_u128(1)).unwrap_err(),
        CmdError::not_found(EntityKind::Item, ItemId::from_u128(1))
    );
    // The start-time index followed the edit.
    let starts: Vec<_> = tx.doc().sequences[&ids.sequence].tracks[&ids.video]
        .items
        .iter()
        .map(Item::start)
        .collect();
    assert_eq!(starts, [secs(1), secs(4)]);
}
