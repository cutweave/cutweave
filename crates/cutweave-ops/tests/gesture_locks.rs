// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2A.12 with the real edit commands (spec §5 INV-304/305, ADR-0023): the items a gesture
//! soft-locks are the items its updates' `EditReport`s name, ripple-shifted items, linked group
//! members (WP-2A.05) and following attached children included, and another actor's command on
//! one of them is refused with `Conflict{item_in_gesture, holder}` before anything changes.
//! A partial (`unlink: true`) move leaves the members it does not edit unlocked.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{collections::BTreeSet, sync::Arc};

use common::{H, P, id, insert_av, item_id};
use cutweave_commands::{
    ActorCtx, Applied, CmdError, ConflictReason, ConnId, DocConfig, DocState, Submit, TxId, TxKind,
};
use cutweave_model::{ItemId, Project, TrackId};
use serde_json::{Value, json};

fn state(h: &H, doc: Project) -> DocState {
    DocState::new(DocConfig::deterministic(Arc::clone(&h.registry), doc, 7))
}

fn person() -> ActorCtx {
    ActorCtx::ui(ConnId::new(1))
}

fn agent() -> ActorCtx {
    ActorCtx::agent(ConnId::new(2), "claude").unwrap()
}

/// A `_meta.replace` update of gesture `tx`.
fn update(s: &mut DocState, tx: TxId, name: &str, params: Value) -> Applied {
    s.submit(Submit::new(person(), name, params).in_tx(tx).replacing())
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Every item an `EditReport` names (spec INV-21).
fn report_items(output: &Value) -> BTreeSet<ItemId> {
    let r = &output["report"];
    let mut out = BTreeSet::new();
    for list in ["created", "removed"] {
        for v in r[list].as_array().into_iter().flatten() {
            out.insert(item_id(v));
        }
    }
    for s in r["split"].as_array().into_iter().flatten() {
        out.insert(item_id(&s["left"]));
        out.insert(item_id(&s["right"]));
    }
    for list in ["moved", "trimmed", "relinked", "replaced", "toggled"] {
        for v in r[list].as_array().into_iter().flatten() {
            out.insert(item_id(&v["item"]));
        }
    }
    out
}

fn locked(s: &DocState, tx: TxId) -> BTreeSet<ItemId> {
    s.transaction(tx).unwrap().items.into_iter().collect()
}

/// The agent changes `item`'s gain (a field no edit of the person touches).
fn agent_gain(s: &mut DocState, item: ItemId) -> Result<Applied, CmdError> {
    s.submit(Submit::new(
        agent(),
        "item.set_props",
        json!({"item": item, "props": {"audio_gain": -6.0}}),
    ))
}

fn assert_held_by_person(result: Result<Applied, CmdError>) {
    match result {
        Err(CmdError::Conflict {
            reason: ConflictReason::ItemInGesture,
            holder: Some(holder),
            retry_after_ms: Some(_),
            ..
        }) => assert_eq!(holder.to_string(), "user"),
        other => panic!("expected item_in_gesture held by the user, got {other:?}"),
    }
}

/// A project with a video with sound at 0 s: `(harness, tracks, video clip, its audio)`.
fn av_project() -> (H, P, ItemId, ItemId) {
    let (mut h, p) = H::project();
    let media = h.add_video("/media/a.mp4", 10, true);
    let (video, audio) = insert_av(&mut h, p, media, "0s");
    (h, p, video, audio)
}

#[test]
fn a_moved_clip_locks_its_linked_sound_and_a_partial_move_does_not() {
    let (mut h, p, video, audio) = av_project();
    // A second sound in the same group: unlinking the picture alone leaves a group of two.
    let music = h.add_audio("/media/m.wav", 10);
    let track2: TrackId = id(&h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "audio"}),
    )["track"]);
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": track2, "media": music, "at": "0s"}),
    );
    let extra = item_id(&out["clips"][0]);
    h.ok("item.link", json!({"items": [video, audio, extra]}));
    let mut s = state(&h, h.doc());

    // The whole group moves with the picture: every member is locked.
    let drag = s
        .begin_tx(&person(), TxKind::Gesture, "Move clip")
        .unwrap()
        .id;
    let moved = update(
        &mut s,
        drag,
        "item.move",
        json!({"item": video, "start": "2s"}),
    );
    let named = report_items(&moved.output);
    assert_eq!(named, BTreeSet::from([video, audio, extra]));
    assert_eq!(locked(&s, drag), named);
    assert_held_by_person(agent_gain(&mut s, audio));
    assert_held_by_person(agent_gain(&mut s, extra));
    s.cancel_tx(&person(), drag).unwrap();

    // Alt-drag (`unlink: true`): only the picture moves and leaves the group; the two sounds
    // stay grouped and unlocked, so an agent may change them meanwhile.
    let drag = s
        .begin_tx(&person(), TxKind::Gesture, "Move clip")
        .unwrap()
        .id;
    let moved = update(
        &mut s,
        drag,
        "item.move",
        json!({"item": video, "start": "2s", "unlink": true}),
    );
    assert_eq!(report_items(&moved.output), BTreeSet::from([video]));
    assert_eq!(locked(&s, drag), BTreeSet::from([video]));
    agent_gain(&mut s, audio).unwrap();
    agent_gain(&mut s, extra).unwrap();
    // Moving a sound moves its group; in the committed document the picture is still in it,
    // so the agent's move would drag the held picture along: refused.
    assert_held_by_person(s.submit(Submit::new(
        agent(),
        "item.move",
        json!({"item": audio, "start": "5s"}),
    )));
    s.commit_tx(&person(), drag).unwrap();
    // Once the picture has left the group, the agent moves the two sounds freely.
    s.submit(Submit::new(
        agent(),
        "item.move",
        json!({"item": audio, "start": "5s"}),
    ))
    .unwrap();
    s.verify().unwrap();
}

#[test]
fn a_ripple_on_the_main_track_locks_what_it_shifts() {
    let (mut h, p) = H::project();
    let media = h.add_video("/media/a.mp4", 10, true);
    h.ok(
        "track.set_magnetic",
        json!({"track": p.video, "magnetic": true}),
    );
    let (first, _) = insert_av(&mut h, p, media, "0s");
    let (second, second_sound) = insert_av(&mut h, p, media, "10s");
    let (third, third_sound) = insert_av(&mut h, p, media, "20s");
    let mut s = state(&h, h.doc());

    // The person drags the first clip's end 2 s left: the clips after it close up.
    let drag = s
        .begin_tx(&person(), TxKind::Gesture, "Ripple trim")
        .unwrap()
        .id;
    let mut all = BTreeSet::new();
    for delta in ["-1s", "-2s"] {
        let trimmed = update(
            &mut s,
            drag,
            "clip.ripple_trim",
            json!({"item": first, "edge": "end", "delta": delta, "clamp": true}),
        );
        all.extend(report_items(&trimmed.output));
        // INV-304: the union of every update's report.
        assert_eq!(locked(&s, drag), all);
    }
    for shifted in [second, second_sound, third, third_sound] {
        assert!(all.contains(&shifted), "{shifted} was shifted");
        assert_held_by_person(agent_gain(&mut s, shifted));
    }
    s.commit_tx(&person(), drag).unwrap();
    agent_gain(&mut s, third).unwrap();
    s.verify().unwrap();
}

#[test]
fn an_attached_title_follows_its_clip_and_is_locked_with_it() {
    let (mut h, p, video, _) = av_project();
    let overlay: TrackId = id(&h.ok(
        "track.add",
        json!({"sequence": p.sequence, "kind": "video"}),
    )["track"]);
    let still = h.add_image("/media/logo.png");
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": overlay, "media": still, "at": "1s", "duration": "2s"}),
    );
    let title = item_id(&out["clips"][0]);
    h.ok("item.attach", json!({"items": [title], "parent": video}));
    let mut s = state(&h, h.doc());

    let drag = s
        .begin_tx(&person(), TxKind::Gesture, "Move clip")
        .unwrap()
        .id;
    let moved = update(
        &mut s,
        drag,
        "item.move",
        json!({"item": video, "start": "3s"}),
    );
    assert!(
        report_items(&moved.output).contains(&title),
        "the child follows"
    );
    assert!(locked(&s, drag).contains(&title));
    assert_held_by_person(agent_gain(&mut s, title));
    s.cancel_tx(&person(), drag).unwrap();
    agent_gain(&mut s, title).unwrap();
    s.verify().unwrap();
}
