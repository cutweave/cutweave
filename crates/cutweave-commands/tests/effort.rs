// SPDX-License-Identifier: GPL-3.0-or-later
//! What an edit costs must not grow with what it does not touch (issues #230, #231). Counted,
//! not timed, so the tests hold on a busy machine: `effort::json_conversions` counts every
//! conversion of a part of the document to or from its JSON form, which is where the time of
//! a ripple over 2,000 clips and of a command in a sequence with thousands of markers went.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use cutweave_commands::{
    Access, Committed, Patch, Transaction, apply_patch, apply_patch_json, diff,
    effort::json_conversions, invert, json_eq,
};
use cutweave_model::{
    Clip, Duration, IdGen, Item, ItemId, Marker, MarkerId, MediaId, Project, SeededGen, Sequence,
    SequenceFormat, SequenceId, Time, Track, TrackId, TrackKind,
};

const CLIP: i64 = 705_600_000; // one second, in flicks

// The sequence and the track are for the tests of the edits that follow.
#[allow(dead_code)]
struct Scene {
    doc: Project,
    sequence: SequenceId,
    track: TrackId,
    items: Vec<ItemId>,
}

/// One sequence with one video track of `clips` clips back to back and `markers` markers.
fn doc(clips: usize, markers: usize) -> Scene {
    let mut ids = SeededGen::new(11);
    let mut doc = Project::new("Effort");
    let sequence: SequenceId = ids.mint();
    let mut seq = Sequence::new(sequence, "Main", SequenceFormat::default());
    let track: TrackId = ids.mint();
    let mut lane = Track::new(track, TrackKind::Video, "V1");
    let media: MediaId = ids.mint();
    let mut items = Vec::new();
    let mut at = 0;
    for _ in 0..clips {
        let id: ItemId = ids.mint();
        lane.items.insert(Item::Clip(Clip::new(
            id,
            media,
            Time::from_flicks(at).unwrap(),
            Duration::from_flicks(CLIP).unwrap(),
        )));
        items.push(id);
        at += CLIP;
    }
    seq.track_order.push_back(track);
    seq.tracks.insert(track, lane);
    for n in 0..markers {
        let id: MarkerId = ids.mint();
        let t = Time::from_flicks(i64::try_from(n).unwrap() * CLIP).unwrap();
        seq.markers.insert(id, Marker::new(id, t));
    }
    doc.sequences.insert(sequence, seq);
    Scene {
        doc,
        sequence,
        track,
        items,
    }
}

/// Runs `edit` in a transaction on `doc` and commits; returns the commit and the conversions
/// the whole of it made.
fn committed(doc: &Project, edit: impl FnOnce(&mut cutweave_commands::Tx<'_>)) -> (Committed, u64) {
    let mut ids = SeededGen::new(5);
    let mut txn = Transaction::begin(doc.clone());
    let before = json_conversions();
    edit(&mut txn.tx(&mut ids, Access::STANDARD));
    let done = txn.commit();
    (done, json_conversions() - before)
}

/// The commit is what it was before the shortcuts: the verified field-level diff, which
/// replays and inverts on the typed document and on its JSON mirror.
fn assert_is_the_diff(before: &Project, done: &Committed) {
    assert_eq!(done.patch, diff(before, &done.doc).unwrap());
    assert_eq!(apply_patch(before, &done.patch).unwrap(), done.doc);
    assert_eq!(&apply_patch(&done.doc, &done.inverse).unwrap(), before);
    let mut mirror = serde_json::to_value(before).unwrap();
    apply_patch_json(&mut mirror, &done.patch).unwrap();
    assert!(json_eq(&mirror, &serde_json::to_value(&done.doc).unwrap()));
    apply_patch_json(&mut mirror, &invert(&done.patch)).unwrap();
    assert!(json_eq(&mirror, &serde_json::to_value(before).unwrap()));
}

/// Committing one write converts nothing: its ops are the transaction's patch as they are.
/// (They used to be computed a second time from the two documents and applied again to
/// verify them, which cost as much as the edit.)
#[test]
fn committing_one_write_converts_nothing_more() {
    let d = doc(30, 0);
    let mut ids = SeededGen::new(5);
    let mut txn = Transaction::begin(d.doc.clone());
    txn.tx(&mut ids, Access::STANDARD)
        .update_item(d.items[3], |i| {
            i.set_duration(Duration::from_flicks(CLIP / 2).unwrap());
            Ok(())
        })
        .unwrap();
    let before = json_conversions();
    let done = txn.commit();
    assert_eq!(json_conversions() - before, 0);
    assert_eq!(done.patch.len(), 1);
    assert_is_the_diff(&d.doc, &done);
}

/// The patch of one command with one write is taken as recorded; several writes are still
/// coalesced by the diff, and a rollback forgets the shortcut with the ops.
#[test]
fn the_commit_patch_is_the_diff_whatever_was_recorded() {
    let d = doc(6, 2);
    let (a, b) = (d.items[0], d.items[1]);
    let rename = |name: &'static str| {
        move |i: &mut Item| {
            let Item::Clip(c) = i else { unreachable!() };
            c.name = Some(name.to_owned());
            Ok(())
        }
    };
    // Two writes of one field: one op, the net change.
    let (done, _) = committed(&d.doc, |tx| {
        tx.update_item(a, rename("first")).unwrap();
        tx.update_item(a, rename("second")).unwrap();
    });
    assert_eq!(done.patch.len(), 1);
    assert_is_the_diff(&d.doc, &done);
    // A write rolled back, then another.
    let (done, _) = committed(&d.doc, |tx| {
        let mark = tx.savepoint();
        tx.update_item(a, rename("dropped")).unwrap();
        tx.rollback_to(mark);
        tx.update_item(b, rename("kept")).unwrap();
    });
    assert_eq!(done.patch.len(), 1);
    assert_is_the_diff(&d.doc, &done);
    // A write that changes nothing, then one that does.
    let (done, _) = committed(&d.doc, |tx| {
        tx.update_item(a, |_| Ok(())).unwrap();
        tx.update_track(d.track, |t| {
            t.muted = true;
            Ok(())
        })
        .unwrap();
    });
    assert_eq!(done.patch.len(), 1);
    assert_is_the_diff(&d.doc, &done);
    // Nothing at all.
    let (done, _) = committed(&d.doc, |_| {});
    assert_eq!(done.patch, Patch::new());
}
