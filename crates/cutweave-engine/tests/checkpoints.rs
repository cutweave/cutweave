// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2I.03: checkpoints in the journal (arch §5.2, §17). Markers are journaled before they are
//! acknowledged; a save carries the older checkpoints into the new segment as patches from the
//! saved document; recovery after a crash (or a plain reopen) rebuilds every checkpoint, which
//! restores to exactly the document it captured. Deleted ones stay deleted, ids are never
//! reused, discarding unsaved work keeps the checkpoints made before the save, and save-as
//! moves them with the journal.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod journal_support;

use cutweave_commands::{Applied, CmdError, ConflictReason, DocHandle, Rev};
use cutweave_engine::journal::{FORMAT_VERSION, SyncPolicy};
use cutweave_model::{ItemId, Project};
use journal_support::{
    Session, SessionOptions, TempDir, agent, all_journal_bytes, append, contains, create_project,
    fixture, open_session, submit, tracks, ui,
};
use serde_json::{Value, json};

fn ok(h: &DocHandle, ctx: &cutweave_commands::ActorCtx, name: &str, params: Value) -> Applied {
    submit(h, ctx, name, params).unwrap()
}

fn rename(h: &DocHandle, item: ItemId, name: &str) {
    ok(h, &ui(), "item.rename", json!({"item": item, "name": name}));
}

fn checkpoint(h: &DocHandle, name: &str) -> (String, Project) {
    let out = ok(h, &ui(), "edit.checkpoint", json!({"name": name})).output;
    let doc = h.committed_snapshot().doc.clone();
    (out["checkpoint"].as_str().unwrap().to_owned(), doc)
}

fn listed(h: &DocHandle) -> Vec<(String, Option<String>)> {
    let out = ok(h, &ui(), "edit.list_checkpoints", json!({})).output;
    out["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["checkpoint"].as_str().unwrap().to_owned(),
                c["name"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

fn restore(h: &DocHandle, checkpoint: &str, force: bool) -> Result<Applied, CmdError> {
    submit(
        h,
        &ui(),
        "edit.restore",
        json!({"checkpoint": checkpoint, "force": force}),
    )
}

fn save(s: &mut Session) {
    let snapshot = s.handle.committed_snapshot();
    s.store.save_snapshot(&snapshot).unwrap();
}

/// Stops a session without saving (what a crash leaves on disk: every acknowledged entry was
/// written to the OS before its acknowledgement).
fn stop(s: Session) {
    drop(s);
}

fn session(folder: &std::path::Path) -> Session {
    open_session(
        folder,
        &SessionOptions {
            sync: SyncPolicy::Manual,
            ..SessionOptions::default()
        },
    )
}

#[test]
fn checkpoints_survive_saves_and_a_crash_and_restore_exactly() {
    let t = TempDir::new("cp-crash");
    let folder = create_project(&t.0, "Checkpoints", &fixture(2));
    let mut s = session(&folder);
    let h = s.handle.clone();
    let tr = tracks(&h.snapshot().doc);
    let x = append(&h, tr[0], 30);
    let (early_id, early) = checkpoint(&h, "early");
    rename(&h, x, "one");
    save(&mut s);
    let (_, saved) = checkpoint(&h, "at save");
    rename(&h, x, "two");
    save(&mut s);
    append(&h, tr[1], 10);
    let (late_id, late) = checkpoint(&h, "late");
    ok(&h, &ui(), "edit.checkpoint", json!({"name": "dropped"}));
    ok(
        &h,
        &ui(),
        "edit.delete_checkpoint",
        json!({"checkpoint": "dropped"}),
    );
    rename(&h, x, "three");
    let before = listed(&h);
    let last = h.committed_snapshot().doc.clone();
    stop(s);

    let s = session(&folder);
    let h = s.handle.clone();
    assert_eq!(h.committed_snapshot().doc, last);
    assert_eq!(s.recovered.checkpoints.len(), 3);
    assert_eq!(listed(&h), before);
    // The latest one is newer than the last save: its history is recovered, no force needed.
    restore(&h, "late", false).unwrap();
    assert_eq!(h.committed_snapshot().doc, late);
    ok(&h, &ui(), "edit.undo", json!({}));
    // Older ones revert changes the recovered history no longer holds: force.
    let err = restore(&h, "early", false).unwrap_err();
    assert!(
        matches!(
            &err,
            CmdError::Conflict {
                reason: ConflictReason::HistoryTargetModified,
                ..
            }
        ),
        "{err}"
    );
    restore(&h, &early_id, true).unwrap();
    assert_eq!(h.committed_snapshot().doc, early);
    restore(&h, "at save", true).unwrap();
    assert_eq!(h.committed_snapshot().doc, saved);
    // Ids are never handed out twice, even the deleted one's.
    let next = ok(&h, &ui(), "edit.checkpoint", json!({})).output;
    assert_eq!(next["checkpoint"], json!("cp_5"));
    assert_eq!(late_id, "cp_3");
    stop(s);
}

#[test]
fn closing_and_reopening_keeps_the_checkpoints() {
    let t = TempDir::new("cp-reopen");
    let folder = create_project(&t.0, "Reopen", &fixture(1));
    let mut s = session(&folder);
    let h = s.handle.clone();
    let x = append(&h, tracks(&h.snapshot().doc)[0], 30);
    let (_, doc) = checkpoint(&h, "keep");
    rename(&h, x, "edited");
    save(&mut s);
    s.store.close().unwrap();
    drop(s.actor);

    let s = session(&folder);
    assert!(s.report.recovery.is_none(), "nothing unsaved");
    assert_eq!(
        listed(&s.handle),
        [("cp_1".to_owned(), Some("keep".to_owned()))]
    );
    restore(&s.handle, "keep", true).unwrap();
    assert_eq!(s.handle.committed_snapshot().doc, doc);
    stop(s);
}

#[test]
fn discarding_unsaved_work_keeps_the_checkpoints_from_before_the_save() {
    let t = TempDir::new("cp-discard");
    let folder = create_project(&t.0, "Discard", &fixture(1));
    let mut s = session(&folder);
    let h = s.handle.clone();
    let x = append(&h, tracks(&h.snapshot().doc)[0], 30);
    let (_, doc) = checkpoint(&h, "saved one");
    save(&mut s);
    rename(&h, x, "unsaved");
    checkpoint(&h, "unsaved one");
    stop(s);

    let s = open_session(
        &folder,
        &SessionOptions {
            sync: SyncPolicy::Manual,
            discard: true,
            ..SessionOptions::default()
        },
    );
    assert_eq!(
        listed(&s.handle),
        [("cp_1".to_owned(), Some("saved one".to_owned()))]
    );
    // At the saved document the checkpoint is the document itself: nothing to revert.
    let restored = restore(&s.handle, "saved one", false).unwrap();
    assert_eq!(restored.output["tx"], Value::Null);
    assert_eq!(s.handle.committed_snapshot().doc, doc);
    stop(s);
}

#[test]
fn save_as_moves_the_checkpoints_with_the_journal() {
    let t = TempDir::new("cp-save-as");
    let folder = create_project(&t.0, "Original", &fixture(1));
    let mut s = session(&folder);
    let h = s.handle.clone();
    let x = append(&h, tracks(&h.snapshot().doc)[0], 30);
    let (_, doc) = checkpoint(&h, "before");
    rename(&h, x, "after");
    let copy = t.0.join("Copy");
    s.store
        .save_as_snapshot(&copy, &h.committed_snapshot())
        .unwrap();
    rename(&h, x, "later");
    stop(s);

    let s = session(&copy);
    assert_eq!(
        listed(&s.handle),
        [("cp_1".to_owned(), Some("before".to_owned()))]
    );
    restore(&s.handle, "before", true).unwrap();
    assert_eq!(s.handle.committed_snapshot().doc, doc);
    stop(s);
}

#[test]
fn markers_are_journal_entries_and_a_resent_request_is_deduplicated() {
    let t = TempDir::new("cp-format");
    let folder = create_project(&t.0, "Format", &fixture(1));
    let s = session(&folder);
    let h = s.handle.clone();
    let request = cutweave_commands::Submit::new(
        agent(),
        "edit.checkpoint",
        json!({"name": "once", "note": "a note"}),
    )
    .with_request_id(cutweave_commands::RequestId::from_u128(0xC0FFEE));
    let first = h.submit(request.clone()).wait().unwrap();
    // A request without an id draws one from the deterministic stream.
    let unnamed = ok(&h, &ui(), "edit.checkpoint", json!({})).output;
    assert_eq!(s.journal.checkpoints().len(), 2);
    stop(s);

    let bytes = all_journal_bytes(&folder.join(".cutweave").join("journal"));
    assert!(contains(&bytes, br#""type":"checkpoint""#));
    assert!(contains(&bytes, br#""note":"a note""#));
    assert!(contains(
        &bytes,
        format!(r#""format":{FORMAT_VERSION}"#).as_bytes()
    ));

    let s = session(&folder);
    let again = s.handle.submit(request).wait().unwrap();
    assert!(again.replayed);
    assert_eq!(again.output, first.output);
    assert_eq!(listed(&s.handle).len(), 2);
    assert_eq!(s.recovered.rev, Rev::ZERO);
    // The stream continues after the marker's request id: the same request again is new.
    let next = ok(&s.handle, &ui(), "edit.checkpoint", json!({}));
    assert!(!next.replayed);
    assert_ne!(next.output["checkpoint"], unnamed["checkpoint"]);
    assert_eq!(listed(&s.handle).len(), 3);
    stop(s);
}
