// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1C.02 journal tests: recovery and undo after it, torn-tail truncation at every byte
//! offset, corruption, secret redaction, the dedup window across restarts and saves,
//! deterministic ids after recovery, rotation and compaction, gestures, discard, stale
//! journals, and `replay(journal) == live document` as a property.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names,
    missing_docs
)]

mod journal_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cutweave_commands::{
    Actor, Applied, CommitRecord, DedupRecord, DocConfig, DocState, EntryCause, Fingerprint,
    JournalStep, ManualClock, Patch, REDACTED, RequestId, Rev, Submit, TxId, TxKind, WallTime,
};
use cutweave_engine::journal::{
    IdTracker, JournalConfig, JournalError, RecoverOptions, StopReason, SyncPolicy,
};
use cutweave_engine::project_io::{ProjectIoError, ProjectStore};
use cutweave_model::{IdGen, ItemId, Project, SeededGen};
use journal_support::{
    SEED, SENTINEL, SessionOptions, TempDir, agent, all_journal_bytes, append, contains,
    create_project, fixture, items, open_session, registry, submit, tracks, ui,
};
use proptest::prelude::*;
use serde_json::json;

fn journal_dir(folder: &Path) -> PathBuf {
    folder.join(".cutweave").join("journal")
}

fn backups_dir(folder: &Path) -> PathBuf {
    folder.join(".cutweave").join("backups")
}

fn segments(folder: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(journal_dir(folder))
        .map_or_else(|_| Vec::new(), |d| d.map(|e| e.unwrap().path()).collect());
    out.retain(|p| p.extension().is_some_and(|x| x == "jnl"));
    out.sort();
    out
}

fn backups(folder: &Path) -> Vec<String> {
    fs::read_dir(backups_dir(folder)).map_or_else(
        |_| Vec::new(),
        |d| {
            d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        },
    )
}

fn rid(n: u128) -> RequestId {
    RequestId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 | n)
}

#[test]
fn unsaved_work_is_recovered_with_history_and_undo_reverts_the_last_recovered_tx() {
    let t = TempDir::new("undo");
    let folder = create_project(&t.0, "Undo", &fixture(2));
    let (before_last, live) = {
        let s = open_session(&folder, &SessionOptions::default());
        assert!(s.report.recovery.is_none());
        let tr = tracks(&s.handle.snapshot().doc);
        let a = append(&s.handle, tr[0], 10);
        append(&s.handle, tr[1], 20);
        submit(
            &s.handle,
            &agent(),
            "project.rename",
            json!({"name": "agent"}),
        )
        .unwrap();
        let before_last = s.handle.committed_snapshot();
        submit(
            &s.handle,
            &ui(),
            "item.rename",
            json!({"item": a, "name": "last"}),
        )
        .unwrap();
        (before_last, s.handle.committed_snapshot())
        // dropped without saving: the journal holds 4 transactions
    };

    let s = open_session(&folder, &SessionOptions::default());
    let available = s.report.recovery.clone().expect("recovery offered");
    assert_eq!(available.records, 4);
    assert_eq!(available.base_rev, Rev::ZERO);
    assert_eq!(available.last_rev, live.committed_rev);
    assert!(!available.damaged);
    assert_eq!(s.recovered.report.applied, 4);
    assert!(s.recovered.report.stopped.is_none());
    assert_eq!(s.handle.committed_snapshot().doc, live.doc);
    assert_eq!(
        s.handle.committed_snapshot().committed_rev,
        live.committed_rev
    );

    // The user's undo reverts their last recovered transaction (the item rename) and nothing
    // else; the agent's rename stays.
    submit(&s.handle, &ui(), "edit.undo", json!({})).unwrap();
    assert_eq!(s.handle.committed_snapshot().doc, before_last.doc);
    // The agent's own undo reverts its rename.
    submit(&s.handle, &agent(), "edit.undo", json!({})).unwrap();
    assert_eq!(
        s.handle.committed_snapshot().doc.meta.name,
        "Journal fixture"
    );
}

#[test]
fn truncating_the_last_record_at_every_offset_recovers_the_previous_one() {
    let t = TempDir::new("torn");
    let folder = create_project(&t.0, "Torn", &fixture(1));
    let (path, full, cut_from, prev, prev_rev) = {
        let s = open_session(&folder, &SessionOptions::default());
        let tr = tracks(&s.handle.snapshot().doc);
        for i in 0..4 {
            let item = append(&s.handle, tr[0], 5 + i);
            submit(
                &s.handle,
                &ui(),
                "item.rename",
                json!({"item": item, "name": format!("n{i}")}),
            )
            .unwrap();
        }
        let path = s.journal.segment_path();
        let cut_from = fs::metadata(&path).unwrap().len();
        let prev = s.handle.committed_snapshot();
        submit(
            &s.handle,
            &ui(),
            "project.rename",
            json!({"name": "the last one"}),
        )
        .unwrap();
        s.journal.sync().unwrap();
        let full = fs::read(&path).unwrap();
        (path, full, cut_from, prev.doc.clone(), prev.committed_rev)
    };
    assert!(full.len() as u64 > cut_from + 8);

    let mut checked = 0;
    for len in cut_from..full.len() as u64 {
        fs::write(&path, &full[..usize::try_from(len).unwrap()]).unwrap();
        let opened = ProjectStore::open(&folder).unwrap();
        let mut store = opened.store;
        let available = opened.report.recovery.clone().unwrap();
        assert_eq!(available.records, 8, "offset {len}");
        assert_eq!(available.damaged, len != cut_from, "offset {len}");
        let recovered = store
            .recover(&opened.project, &RecoverOptions::at(WallTime::UNIX_EPOCH))
            .unwrap();
        assert_eq!(recovered.rev, prev_rev, "offset {len}");
        assert_eq!(recovered.project, prev, "offset {len}");
        if len == cut_from {
            assert!(recovered.report.stopped.is_none());
        } else {
            let stop = recovered.report.stopped.clone().unwrap();
            assert_eq!(stop.reason, StopReason::Truncated, "offset {len}");
            assert_eq!(stop.offset, cut_from);
        }
        checked += 1;
    }
    assert_eq!(checked, full.len() as u64 - cut_from);

    // Resuming cuts the torn tail off and new transactions follow the last valid one.
    fs::write(&path, &full[..full.len() - 3]).unwrap();
    {
        let s = open_session(&folder, &SessionOptions::default());
        assert_eq!(s.recovered.rev, prev_rev);
        submit(
            &s.handle,
            &ui(),
            "project.rename",
            json!({"name": "after recovery"}),
        )
        .unwrap();
    }
    let s = open_session(&folder, &SessionOptions::default());
    assert!(s.recovered.report.stopped.is_none());
    assert_eq!(s.recovered.report.applied, 9);
    assert_eq!(s.handle.snapshot().doc.meta.name, "after recovery");
}

#[test]
fn a_corrupt_record_stops_replay_and_its_segment_is_preserved() {
    let t = TempDir::new("corrupt");
    let folder = create_project(&t.0, "Corrupt", &fixture(1));
    let (path, offsets, docs) = {
        let s = open_session(&folder, &SessionOptions::default());
        let tr = tracks(&s.handle.snapshot().doc);
        let path = s.journal.segment_path();
        let mut offsets = Vec::new();
        let mut docs = Vec::new();
        for i in 0..5 {
            offsets.push(fs::metadata(&path).unwrap().len());
            docs.push(s.handle.committed_snapshot().doc.clone());
            append(&s.handle, tr[0], 3 + i);
        }
        (path, offsets, docs)
    };
    // Flip a byte inside the third transaction's payload.
    let mut bytes = fs::read(&path).unwrap();
    let at = usize::try_from(offsets[2]).unwrap() + 20;
    bytes[at] ^= 0x55;
    fs::write(&path, &bytes).unwrap();

    let s = open_session(&folder, &SessionOptions::default());
    let stop = s.recovered.report.stopped.clone().unwrap();
    assert_eq!(stop.reason, StopReason::Checksum);
    assert_eq!(stop.offset, offsets[2]);
    assert_eq!(s.recovered.report.applied, 2);
    assert_eq!(s.handle.committed_snapshot().doc, docs[2]);
    assert!(
        backups(&folder)
            .iter()
            .any(|b| b.starts_with("journal-replaced-")),
        "the damaged segment is kept for inspection: {:?}",
        backups(&folder)
    );
    // Journaling continues after the last valid record.
    let tr = tracks(&s.handle.snapshot().doc);
    append(&s.handle, tr[0], 9);
    let expected = s.handle.committed_snapshot().doc.clone();
    drop(s);
    let s = open_session(&folder, &SessionOptions::default());
    assert!(s.recovered.report.stopped.is_none());
    assert_eq!(s.handle.committed_snapshot().doc, expected);
}

#[test]
fn secrets_never_reach_the_journal_or_its_carried_requests() {
    let t = TempDir::new("secret");
    let folder = create_project(&t.0, "Secret", &fixture(1));
    {
        let s = open_session(&folder, &SessionOptions::default());
        submit(
            &s.handle,
            &ui(),
            "ai.set_key",
            json!({"provider": "acme", "key": SENTINEL}),
        )
        .unwrap();
        // Inside a batch, too.
        let tx = s
            .handle
            .begin_tx(&ui(), TxKind::Batch, "keys")
            .wait()
            .unwrap();
        s.handle
            .submit(
                Submit::new(
                    ui(),
                    "ai.set_key",
                    json!({"provider": "beta", "key": SENTINEL}),
                )
                .in_tx(tx.id),
            )
            .wait()
            .unwrap();
        s.handle.commit_tx(&ui(), tx.id).wait().unwrap();
        // A save carries the requests into the next segment.
        let mut store = s.store;
        store.save_snapshot(&s.handle.committed_snapshot()).unwrap();
        submit(
            &s.handle,
            &ui(),
            "ai.set_key",
            json!({"provider": "gamma", "key": SENTINEL}),
        )
        .unwrap();

        // Defense in depth: a record that somehow still carries the secret is redacted.
        let record = Applied {
            request_id: rid(99),
            rev: Rev(100),
            committed_rev: Rev(100),
            tx_id: Some(TxId::new(100)),
            output: json!({}),
            ops: Patch::new(),
            replayed: false,
        };
        let steps = [JournalStep {
            command: "ai.set_key".into(),
            params: json!({"provider": "raw", "key": SENTINEL}),
            record: DedupRecord {
                fingerprint: Fingerprint::of(&Actor::User, "ai.set_key", &json!({})),
                at: WallTime::UNIX_EPOCH,
                applied: record,
            },
        }];
        let journal = store.journal().unwrap().clone();
        journal
            .append(&CommitRecord {
                rev: Rev(100),
                prev_committed_rev: journal.last_rev(),
                tx_id: TxId::new(100),
                kind: None,
                actor: &Actor::User,
                label: None,
                at: WallTime::UNIX_EPOCH,
                steps: &steps,
                patch: &Patch::new(),
                cause: EntryCause::Edit,
                undoable: true,
            })
            .unwrap();
        journal.sync().unwrap();
        drop(s.actor);
        drop(store);
    }
    let everything = all_journal_bytes(&folder.join(".cutweave"));
    assert_ne!(everything.len(), 0);
    assert!(
        !contains(&everything, SENTINEL.as_bytes()),
        "the secret reached a file under .cutweave"
    );
    assert!(contains(&everything, REDACTED.as_bytes()));
}

#[test]
fn the_dedup_window_survives_restarts_and_saves() {
    let t = TempDir::new("dedup");
    let folder = create_project(&t.0, "Dedup", &fixture(1));
    let t0 = WallTime::from_millis(1_700_000_000_000);
    let options = |at: WallTime| SessionOptions {
        clock: Arc::new(ManualClock::new(at)),
        ..SessionOptions::default()
    };
    let resend = |h: &cutweave_commands::DocHandle, id: RequestId, frames: u32| {
        let track = tracks(&h.snapshot().doc)[0];
        h.submit(
            Submit::new(
                ui(),
                "timeline.append_clip",
                json!({"track": track, "frames": frames}),
            )
            .with_request_id(id),
        )
        .wait()
        .unwrap()
    };
    let (first, second) = {
        let mut s = open_session(&folder, &options(t0));
        let first = resend(&s.handle, rid(1), 10);
        // A save folds the first request into the document; it must stay deduplicated.
        s.store
            .save_snapshot(&s.handle.committed_snapshot())
            .unwrap();
        s.clock.advance(Duration::from_secs(60));
        let second = resend(&s.handle, rid(2), 20);
        (first, second)
    };

    // 9 minutes later, after a restart: both resends return the original results.
    {
        let s = open_session(&folder, &options(t0.saturating_add(Duration::from_mins(9))));
        let count = items(&s.handle.snapshot().doc).len();
        let again = resend(&s.handle, rid(1), 10);
        assert!(again.replayed);
        assert_eq!(again.output, first.output);
        assert_eq!(again.rev, first.rev);
        let again = resend(&s.handle, rid(2), 20);
        assert!(again.replayed);
        assert_eq!(again.output, second.output);
        assert_eq!(
            items(&s.handle.snapshot().doc).len(),
            count,
            "nothing re-applied"
        );
    }

    // 11 minutes after the first (10 after the second): the first is forgotten and applies
    // again; the second is still remembered.
    let s = open_session(
        &folder,
        &options(t0.saturating_add(Duration::from_mins(11))),
    );
    let count = items(&s.handle.snapshot().doc).len();
    assert!(!resend(&s.handle, rid(1), 10).replayed);
    assert!(resend(&s.handle, rid(2), 20).replayed);
    assert_eq!(items(&s.handle.snapshot().doc).len(), count + 1);
}

#[test]
fn deterministic_ids_continue_after_recovery_and_after_a_save() {
    // Reference: one uninterrupted session minting 6 ids and 6 engine request ids.
    let t = TempDir::new("ids");
    let reference_folder = create_project(&t.0, "Reference", &fixture(1));
    let reference: Vec<(ItemId, RequestId)> = {
        let s = open_session(&reference_folder, &SessionOptions::default());
        let tr = tracks(&s.handle.snapshot().doc);
        (0..6)
            .map(|i| {
                let a = submit(
                    &s.handle,
                    &ui(),
                    "timeline.append_clip",
                    json!({"track": tr[0], "frames": 1 + i}),
                )
                .unwrap();
                (
                    serde_json::from_value(a.output["item"].clone()).unwrap(),
                    a.request_id,
                )
            })
            .collect()
    };

    let folder = create_project(&t.0, "Crashy", &fixture(1));
    let mut got = Vec::new();
    for (session, n) in [(0, 2), (1, 2), (2, 2)] {
        let mut s = open_session(&folder, &SessionOptions::default());
        let tr = tracks(&s.handle.snapshot().doc);
        for _ in 0..n {
            let i = u32::try_from(got.len()).unwrap();
            let a = submit(
                &s.handle,
                &ui(),
                "timeline.append_clip",
                json!({"track": tr[0], "frames": 1 + i}),
            )
            .unwrap();
            got.push((
                serde_json::from_value(a.output["item"].clone()).unwrap(),
                a.request_id,
            ));
        }
        if session == 1 {
            // Saved: the next session has no transaction to read positions from, only the
            // new segment's header.
            s.store
                .save_snapshot(&s.handle.committed_snapshot())
                .unwrap();
        }
    }
    assert_eq!(
        got, reference,
        "ids after recovery continue the uninterrupted sequence"
    );
}

#[test]
fn the_request_id_stream_matches_doc_config_deterministic() {
    let run = |config: DocConfig| {
        let mut state = DocState::new(config);
        let track = tracks(state.doc())[0];
        (0..3)
            .map(|_| {
                let a = state
                    .submit(Submit::new(
                        ui(),
                        "timeline.append_clip",
                        json!({"track": track, "frames": 2}),
                    ))
                    .unwrap();
                (a.request_id, a.output)
            })
            .collect::<Vec<_>>()
    };
    let doc = fixture(1);
    let ours = run(IdTracker::new(SEED).config(registry(), doc.clone()));
    let theirs = run(DocConfig::deterministic(registry(), doc, SEED));
    assert_eq!(ours, theirs);
    // And a tracker resumed after 3 ids continues the same streams.
    let mut entity = SeededGen::new(SEED);
    for _ in 0..3 {
        entity.next_u128();
    }
    let tracker = IdTracker::new(SEED);
    let _ = run(tracker.config(registry(), fixture(1)));
    assert_eq!(tracker.state().entity, 3);
    assert_eq!(tracker.state().request, 3);
}

#[test]
fn saves_rotate_and_compact_the_journal_and_keep_the_previous_one() {
    let t = TempDir::new("rotate");
    let folder = create_project(&t.0, "Rotate", &fixture(1));
    let mut s = open_session(&folder, &SessionOptions::default());
    let tr = tracks(&s.handle.snapshot().doc);
    assert_eq!(segments(&folder).len(), 1);
    for i in 0..3 {
        append(&s.handle, tr[0], 1 + i);
    }
    let first = s.journal.segment_path();
    s.store
        .save_snapshot(&s.handle.committed_snapshot())
        .unwrap();
    assert_eq!(s.journal.base_rev(), Rev(3));
    assert_eq!(
        segments(&folder).len(),
        2,
        "the previous journal is kept until the next save"
    );
    append(&s.handle, tr[0], 7);
    s.store
        .save_snapshot(&s.handle.committed_snapshot())
        .unwrap();
    let segs = segments(&folder);
    assert_eq!(segs.len(), 2);
    assert!(
        !segs.contains(&first),
        "the journal two saves back is deleted"
    );
    // Saving the same revision again still rotates (the bytes may differ).
    s.store
        .save_snapshot(&s.handle.committed_snapshot())
        .unwrap();
    assert_eq!(segments(&folder).len(), 2);
    let saved = s.handle.committed_snapshot().doc.clone();
    drop(s);
    let s = open_session(&folder, &SessionOptions::default());
    assert!(s.report.recovery.is_none(), "everything was saved");
    assert_eq!(s.recovered.rev, Rev(4));
    assert_eq!(s.handle.committed_snapshot().doc, saved);
}

#[test]
fn a_crash_between_rotation_and_the_document_write_loses_nothing() {
    let t = TempDir::new("midsave");
    let folder = create_project(&t.0, "Mid Save", &fixture(1));
    let expected = {
        let s = open_session(&folder, &SessionOptions::default());
        let tr = tracks(&s.handle.snapshot().doc);
        append(&s.handle, tr[0], 1);
        append(&s.handle, tr[0], 2);
        // The save rotated the journal, then died before writing the document.
        let rotation = s.journal.rotate(Rev(2), "0000-not-on-disk").unwrap();
        drop(rotation);
        append(&s.handle, tr[0], 3);
        s.handle.committed_snapshot()
    };
    assert_eq!(segments(&folder).len(), 2);
    let s = open_session(&folder, &SessionOptions::default());
    assert_eq!(s.recovered.report.applied, 3);
    assert!(s.recovered.report.stopped.is_none());
    assert_eq!(s.handle.committed_snapshot().doc, expected.doc);
    assert_eq!(s.recovered.rev, expected.committed_rev);
}

#[test]
fn gestures_are_journaled_once_on_commit_and_cancelled_ones_never() {
    let t = TempDir::new("gesture");
    let folder = create_project(&t.0, "Gesture", &fixture(1));
    {
        let s = open_session(&folder, &SessionOptions::default());
        let tr = tracks(&s.handle.snapshot().doc);
        let item = append(&s.handle, tr[0], 4);
        let appends = s.journal.stats().appends;
        let g = s
            .handle
            .begin_tx(&ui(), TxKind::Gesture, "drag")
            .wait()
            .unwrap();
        for i in 0..5 {
            s.handle
                .submit(
                    Submit::new(
                        ui(),
                        "item.rename",
                        json!({"item": item, "name": format!("g{i}")}),
                    )
                    .in_tx(g.id),
                )
                .wait()
                .unwrap();
        }
        s.handle.cancel_tx(&ui(), g.id).wait().unwrap();
        // Opening the first transaction reserved a block of transaction ids (ADR-0043).
        let appends = appends + 1;
        assert_eq!(
            s.journal.stats().appends,
            appends,
            "the commands of a cancelled gesture are not journaled"
        );
        let g = s
            .handle
            .begin_tx(&ui(), TxKind::Gesture, "drag")
            .wait()
            .unwrap();
        for i in 0..5 {
            s.handle
                .submit(
                    Submit::new(
                        ui(),
                        "item.rename",
                        json!({"item": item, "name": format!("h{i}")}),
                    )
                    .in_tx(g.id),
                )
                .wait()
                .unwrap();
        }
        s.handle.commit_tx(&ui(), g.id).wait().unwrap();
        assert_eq!(
            s.journal.stats().appends,
            appends + 1,
            "one record per committed gesture"
        );
    }
    let s = open_session(&folder, &SessionOptions::default());
    let last = s.recovered.commits.last().unwrap();
    assert_eq!(last.kind, Some(TxKind::Gesture));
    assert_eq!(last.steps.len(), 5);
    assert_eq!(last.label.as_deref(), Some("drag"));
    // One undo step for the whole gesture.
    submit(&s.handle, &ui(), "edit.undo", json!({})).unwrap();
    let doc = s.handle.committed_snapshot().doc.clone();
    let name = doc
        .find_item(&items(&doc)[0])
        .and_then(|r| r.item.as_clip().and_then(|c| c.name.clone()));
    assert_eq!(name, None);
}

/// Issue #235. A client that lost the engine mid-drag sends what it had in flight again,
/// under the same request ids. The journal had the first drag's commit: its begin, update and
/// commit get their original results back and nothing is applied twice. The engine died with
/// the second drag open: its edits are gone, its update and commit are refused, and its begin
/// (never journaled) runs now and opens a new, empty transaction.
#[test]
fn a_recovered_gesture_answers_its_requests_again_and_a_lost_one_is_refused() {
    let t = TempDir::new("gesture-resend");
    let folder = create_project(&t.0, "Resend", &fixture(1));
    let send = |h: &cutweave_commands::DocHandle, n: u128, name: &str, params, tx: Option<TxId>| {
        let request = Submit::new(ui(), name, params).with_request_id(rid(n));
        h.submit(tx.map_or(request.clone(), |tx| request.in_tx(tx)))
            .wait()
    };
    let begin = json!({"kind": "gesture", "label": "Drag"});
    let name_of = |h: &cutweave_commands::DocHandle, item: ItemId| {
        let doc = h.committed_snapshot().doc.clone();
        doc.find_item(&item)
            .and_then(|r| r.item.as_clip().and_then(|c| c.name.clone()))
    };
    let tx_of =
        |opened: &Applied| -> TxId { serde_json::from_value(opened.output["tx"].clone()).unwrap() };

    let (item, opened, renamed, committed, orphan) = {
        let s = open_session(&folder, &SessionOptions::default());
        let h = &s.handle;
        let item = append(h, tracks(&h.snapshot().doc)[0], 4);
        let opened = send(h, 1, "edit.begin_tx", begin.clone(), None).unwrap();
        let rename = json!({"item": item, "name": "kept"});
        let renamed = send(h, 2, "item.rename", rename, Some(tx_of(&opened))).unwrap();
        let committed = send(h, 3, "edit.commit_tx", json!({}), None).unwrap();
        let orphan = send(h, 4, "edit.begin_tx", begin.clone(), None).unwrap();
        let rename = json!({"item": item, "name": "lost"});
        send(h, 5, "item.rename", rename, Some(tx_of(&orphan))).unwrap();
        (item, opened, renamed, committed, tx_of(&orphan))
    };

    let s = open_session(&folder, &SessionOptions::default());
    let h = &s.handle;
    let recovered = h.snapshot();
    assert_eq!(name_of(h, item).as_deref(), Some("kept"));
    let last = s.recovered.commits.last().unwrap();
    let commands: Vec<&str> = last.steps.iter().map(|s| s.command.as_str()).collect();
    assert_eq!(commands, ["item.rename", "edit.begin_tx", "edit.commit_tx"]);
    assert_eq!(last.label.as_deref(), Some("Drag"));

    // The committed drag: every request is answered from the journal, as often as it is sent.
    for _ in 0..2 {
        let again = send(h, 3, "edit.commit_tx", json!({}), None).unwrap();
        assert!(again.replayed);
        assert_eq!(again.output, committed.output);
        assert_eq!(again.rev, committed.rev);
        assert_eq!(again.committed_rev, committed.committed_rev);
    }
    let again = send(h, 1, "edit.begin_tx", begin.clone(), None).unwrap();
    assert!(again.replayed);
    assert_eq!(again.output, opened.output);
    let rename = json!({"item": item, "name": "kept"});
    let again = send(h, 2, "item.rename", rename, Some(tx_of(&opened))).unwrap();
    assert!(again.replayed);
    assert_eq!(again.output, renamed.output);
    assert_eq!(h.snapshot().rev, recovered.rev, "nothing was applied again");
    assert_eq!(h.locks(&ui()).wait().unwrap().transactions.len(), 0);

    // The drag the engine died with: nothing of it is answered as done.
    let rename = json!({"item": item, "name": "lost"});
    let err = send(h, 5, "item.rename", rename, Some(orphan)).unwrap_err();
    assert!(
        matches!(err, cutweave_commands::CmdError::NotFound { .. }),
        "{err:?}"
    );
    let err = send(h, 6, "edit.commit_tx", json!({}), None).unwrap_err();
    let none =
        cutweave_commands::CmdError::invalid("/tx", "this connection has no open transaction");
    assert_eq!(err, none);
    let err = send(h, 7, "edit.cancel_tx", json!({}), None).unwrap_err();
    assert_eq!(err, none);
    assert_eq!(h.snapshot().doc, recovered.doc);
    let reopened = send(h, 4, "edit.begin_tx", begin, None).unwrap();
    assert!(!reopened.replayed, "its begin was never journaled: it runs");
    assert_eq!(h.locks(&ui()).wait().unwrap().transactions.len(), 1);
    send(h, 8, "edit.cancel_tx", json!({}), None).unwrap();

    // Still one undo step for the whole recovered drag.
    submit(h, &ui(), "edit.undo", json!({})).unwrap();
    assert_eq!(name_of(h, item), None);
}

/// Sends `name` under request id `n` (optionally inside `tx`), as a client does.
fn send(
    h: &cutweave_commands::DocHandle,
    n: u128,
    name: &str,
    params: serde_json::Value,
    tx: Option<TxId>,
) -> Result<Applied, cutweave_commands::CmdError> {
    let request = Submit::new(ui(), name, params).with_request_id(rid(n));
    h.submit(tx.map_or(request.clone(), |tx| request.in_tx(tx)))
        .wait()
}

fn opened_tx(opened: &Applied) -> TxId {
    serde_json::from_value(opened.output["tx"].clone()).unwrap()
}

fn is_not_found(result: &Result<Applied, cutweave_commands::CmdError>) -> bool {
    matches!(result, Err(cutweave_commands::CmdError::NotFound { .. }))
}

/// ADR-0043: a cancel and a commit of a transaction that changed nothing are journaled as
/// markers. After a restart, and after a save and another restart, their repeats get the
/// original results back, the document is what it was, and nothing is open.
#[test]
fn a_cancel_and_an_empty_commit_sent_again_after_a_restart_get_their_results_back() {
    let t = TempDir::new("close-markers");
    let folder = create_project(&t.0, "Closes", &fixture(1));
    let gesture = json!({"kind": "gesture", "label": "Drag"});
    let batch = json!({"label": "Batch"});
    let (item, cancelled, committed, a, b, live) = {
        let s = open_session(&folder, &SessionOptions::default());
        let h = &s.handle;
        let item = append(h, tracks(&h.snapshot().doc)[0], 4);
        let a = opened_tx(&send(h, 1, "edit.begin_tx", gesture.clone(), None).unwrap());
        let rename = json!({"item": item, "name": "dropped"});
        send(h, 2, "item.rename", rename, Some(a)).unwrap();
        let cancelled = send(h, 3, "edit.cancel_tx", json!({"tx": a}), None).unwrap();
        let b = opened_tx(&send(h, 4, "edit.begin_tx", batch.clone(), None).unwrap());
        let committed = send(h, 5, "edit.commit_tx", json!({"tx": b}), None).unwrap();
        assert_eq!(committed.output["changed"], false);
        (item, cancelled, committed, a, b, h.committed_snapshot())
    };
    let check = |s: &journal_support::Session, when: &str| {
        let h = &s.handle;
        assert_eq!(h.committed_snapshot().doc, live.doc, "{when}");
        for _ in 0..2 {
            let again = send(h, 3, "edit.cancel_tx", json!({"tx": a}), None).unwrap();
            assert!(again.replayed, "{when}");
            assert_eq!(again.output, cancelled.output, "{when}");
            assert_eq!(again.rev, cancelled.rev, "{when}");
            let again = send(h, 5, "edit.commit_tx", json!({"tx": b}), None).unwrap();
            assert!(again.replayed, "{when}");
            assert_eq!(again.output, committed.output, "{when}");
        }
        let again = send(h, 1, "edit.begin_tx", gesture.clone(), None).unwrap();
        assert!(again.replayed, "{when}");
        let again = send(h, 4, "edit.begin_tx", batch.clone(), None).unwrap();
        assert!(again.replayed, "{when}");
        assert_eq!(h.locks(&ui()).wait().unwrap().transactions.len(), 0);
        assert_eq!(h.committed_snapshot().doc, live.doc, "{when}");
        // Under a request id the journal never saw, the same close is a new request.
        let fresh = send(h, 9, "edit.cancel_tx", json!({"tx": a}), None);
        assert!(is_not_found(&fresh), "{when}");
    };
    {
        let mut s = open_session(&folder, &SessionOptions::default());
        assert_eq!(
            s.recovered.report.applied, 1,
            "markers are not transactions"
        );
        assert!(s.recovered.report.stopped.is_none());
        check(&s, "after a restart");
        // The rename of the cancelled gesture was rolled back and is not remembered.
        let doc = s.handle.committed_snapshot().doc.clone();
        let name = doc
            .find_item(&item)
            .and_then(|r| r.item.as_clip().and_then(|c| c.name.clone()));
        assert_eq!(name, None);
        s.store
            .save_snapshot(&s.handle.committed_snapshot())
            .unwrap();
    }
    let s = open_session(&folder, &SessionOptions::default());
    assert!(s.report.recovery.is_none());
    check(&s, "after a save and a restart");
}

/// ADR-0043: the journal knows which transaction ids were handed out, so on a frozen clock
/// (`--deterministic-ids`) too a restarted engine never issues the id of a transaction the
/// one before it died with, whether the unsaved work is restored or discarded, across a save,
/// and when the engine restarts twice without opening anything.
#[test]
fn transaction_ids_never_repeat_across_restarts() {
    let t = TempDir::new("tx-ids");
    let folder = create_project(&t.0, "TxIds", &fixture(1));
    let open_one = |s: &journal_support::Session| {
        s.handle
            .begin_tx(&ui(), TxKind::Gesture, "drag")
            .wait()
            .unwrap()
            .id
    };
    let a = {
        let s = open_session(&folder, &SessionOptions::default());
        append(&s.handle, tracks(&s.handle.snapshot().doc)[0], 4);
        open_one(&s)
        // The engine dies with A open.
    };
    let b = {
        let mut s = open_session(&folder, &SessionOptions::default());
        assert!(s.recovered.next_tx > a, "{} after {a}", s.recovered.next_tx);
        let b = open_one(&s);
        assert!(b > a);
        // The close of A sent again finds nothing, and B is still there.
        let commit = send(&s.handle, 1, "edit.commit_tx", json!({"tx": a}), None);
        assert!(is_not_found(&commit));
        let cancel = send(&s.handle, 2, "edit.cancel_tx", json!({"tx": a}), None);
        assert!(is_not_found(&cancel));
        assert_eq!(s.handle.locks(&ui()).wait().unwrap().transactions.len(), 1);
        s.handle.cancel_tx(&ui(), b).wait().unwrap();
        s.store
            .save_snapshot(&s.handle.committed_snapshot())
            .unwrap();
        b
    };
    let c = {
        let s = open_session(&folder, &SessionOptions::default());
        assert!(s.report.recovery.is_none());
        let c = open_one(&s);
        assert!(c > b, "the save carried the reservation: {c} after {b}");
        s.handle.cancel_tx(&ui(), c).wait().unwrap();
        append(&s.handle, tracks(&s.handle.snapshot().doc)[0], 2);
        c
    };
    // Discarding the unsaved work cuts the tail of the journal, markers included.
    drop(open_session(
        &folder,
        &SessionOptions {
            discard: true,
            ..SessionOptions::default()
        },
    ));
    // That engine opened nothing and died; the next one still knows.
    let s = open_session(&folder, &SessionOptions::default());
    assert!(s.recovered.next_tx > c, "{} after {c}", s.recovered.next_tx);
    assert!(open_one(&s) > c);
}

/// A marker cut short by a crash is a torn tail like any other: recovery keeps everything
/// before it, and the close it would have remembered is refused, not reported as done.
#[test]
fn a_torn_marker_loses_only_what_it_remembered() {
    let t = TempDir::new("torn-marker");
    let folder = create_project(&t.0, "TornMarker", &fixture(1));
    let (path, full, cut_from, a, live) = {
        let s = open_session(&folder, &SessionOptions::default());
        let h = &s.handle;
        append(h, tracks(&h.snapshot().doc)[0], 4);
        let begin = json!({"kind": "gesture", "label": "Drag"});
        let a = opened_tx(&send(h, 1, "edit.begin_tx", begin, None).unwrap());
        let path = s.journal.segment_path();
        s.journal.sync().unwrap();
        let cut_from = fs::metadata(&path).unwrap().len();
        send(h, 2, "edit.cancel_tx", json!({"tx": a}), None).unwrap();
        s.journal.sync().unwrap();
        let full = fs::read(&path).unwrap();
        (path, full, cut_from, a, h.committed_snapshot())
    };
    assert!(
        full.len() as u64 > cut_from + 8,
        "the cancel wrote a marker"
    );
    for len in cut_from..full.len() as u64 {
        fs::write(&path, &full[..usize::try_from(len).unwrap()]).unwrap();
        let opened = ProjectStore::open(&folder).unwrap();
        let mut store = opened.store;
        let recovered = store
            .recover(&opened.project, &RecoverOptions::at(WallTime::UNIX_EPOCH))
            .unwrap();
        assert_eq!(recovered.project, live.doc, "offset {len}");
        assert_eq!(recovered.rev, live.committed_rev, "offset {len}");
        assert!(recovered.next_tx > a, "offset {len}");
        let remembered = recovered.requests.iter().any(|r| r.request_id() == rid(2));
        assert!(!remembered, "offset {len}");
        let stop = recovered.report.stopped.map(|s| (s.reason, s.offset));
        let expected = (len != cut_from).then_some((StopReason::Truncated, cut_from));
        assert_eq!(stop, expected, "offset {len}");
    }
    // Resumed after the tear: the cancel sent again is refused, and work goes on.
    fs::write(&path, &full[..full.len() - 3]).unwrap();
    {
        let s = open_session(&folder, &SessionOptions::default());
        let again = send(&s.handle, 2, "edit.cancel_tx", json!({"tx": a}), None);
        assert!(is_not_found(&again));
        append(&s.handle, tracks(&s.handle.snapshot().doc)[0], 2);
    }
    let s = open_session(&folder, &SessionOptions::default());
    assert!(s.recovered.report.stopped.is_none());
    assert_eq!(s.recovered.report.applied, 2);
}

/// A journal written before ADR-0043 has no markers: it recovers as it always did, and ids
/// continue past its last committed transaction.
#[test]
fn a_journal_without_markers_recovers_as_before() {
    let t = TempDir::new("no-markers");
    let folder = create_project(&t.0, "NoMarkers", &fixture(1));
    let (live, last_tx) = {
        let s = open_session(&folder, &SessionOptions::default());
        let h = &s.handle;
        let tr = tracks(&h.snapshot().doc);
        append(h, tr[0], 4);
        let last = submit(h, &ui(), "project.rename", json!({"name": "old"})).unwrap();
        (h.committed_snapshot(), last.tx_id.unwrap())
    };
    let bytes = all_journal_bytes(&journal_dir(&folder));
    let marker = br#""type":"tx""#;
    assert!(!contains(&bytes, marker), "no transaction was opened");
    let s = open_session(&folder, &SessionOptions::default());
    assert_eq!(s.recovered.report.applied, 2);
    assert_eq!(s.handle.committed_snapshot().doc, live.doc);
    assert_eq!(s.recovered.next_tx, TxId::new(last_tx.get() + 1));
}

#[test]
fn discarding_keeps_the_saved_document_and_preserves_the_journal() {
    let t = TempDir::new("discard");
    let folder = create_project(&t.0, "Discard", &fixture(1));
    let saved = {
        let mut s = open_session(&folder, &SessionOptions::default());
        let tr = tracks(&s.handle.snapshot().doc);
        append(&s.handle, tr[0], 1);
        s.store
            .save_snapshot(&s.handle.committed_snapshot())
            .unwrap();
        let saved = s.handle.committed_snapshot();
        append(&s.handle, tr[0], 2);
        append(&s.handle, tr[0], 3);
        saved
    };
    let s = open_session(
        &folder,
        &SessionOptions {
            discard: true,
            ..SessionOptions::default()
        },
    );
    assert_eq!(s.report.recovery.as_ref().unwrap().records, 2);
    assert_eq!(s.handle.committed_snapshot().doc, saved.doc);
    assert_eq!(s.recovered.rev, saved.committed_rev);
    assert!(
        backups(&folder)
            .iter()
            .any(|b| b.starts_with("journal-replaced-"))
    );
    let tr = tracks(&s.handle.snapshot().doc);
    append(&s.handle, tr[0], 9);
    let expected = s.handle.committed_snapshot().doc.clone();
    drop(s);
    let s = open_session(&folder, &SessionOptions::default());
    assert_eq!(
        s.recovered.report.applied, 1,
        "only the work after the discard"
    );
    assert_eq!(s.handle.committed_snapshot().doc, expected);
}

#[test]
fn a_journal_for_another_document_is_preserved_and_never_replayed() {
    let t = TempDir::new("stale");
    let folder = create_project(&t.0, "Stale", &fixture(1));
    {
        let s = open_session(&folder, &SessionOptions::default());
        let tr = tracks(&s.handle.snapshot().doc);
        append(&s.handle, tr[0], 1);
    }
    // The document is replaced behind our back (a git checkout, a copy from elsewhere).
    let other = Project::new("Replaced");
    let doc_path = folder.join("Stale.cutweave");
    fs::write(
        &doc_path,
        cutweave_model::to_document_string(&other).unwrap(),
    )
    .unwrap();
    let s = open_session(&folder, &SessionOptions::default());
    assert!(s.report.recovery.is_none());
    assert_eq!(s.handle.committed_snapshot().doc.meta.name, "Replaced");
    assert!(
        backups(&folder)
            .iter()
            .any(|b| b.starts_with("journal-stale-"))
    );
}

#[test]
fn saving_or_starting_is_refused_while_recovery_is_pending() {
    let t = TempDir::new("pending");
    let folder = create_project(&t.0, "Pending", &fixture(1));
    {
        let s = open_session(&folder, &SessionOptions::default());
        let tr = tracks(&s.handle.snapshot().doc);
        append(&s.handle, tr[0], 1);
    }
    let opened = ProjectStore::open(&folder).unwrap();
    let mut store = opened.store;
    assert!(store.recovery().is_some());
    assert!(matches!(
        store.save(&opened.project),
        Err(ProjectIoError::Journal(JournalError::RecoveryPending))
    ));
    assert!(matches!(
        store.start_journal(JournalConfig::default()),
        Err(ProjectIoError::Journal(JournalError::RecoveryPending))
    ));
    let recovered = store
        .recover(&opened.project, &RecoverOptions::default())
        .unwrap();
    assert_eq!(recovered.report.applied, 1);
    let journal = store
        .start_journal(JournalConfig::default().with_sync(SyncPolicy::Manual))
        .unwrap();
    // A journaled store saves committed snapshots only (it must know the revision).
    assert!(matches!(
        store.save(&recovered.project),
        Err(ProjectIoError::Journal(JournalError::RevisionRequired))
    ));
    store.save_at(recovered.rev, &recovered.project).unwrap();
    assert_eq!(journal.base_rev(), recovered.rev);
}

#[test]
fn save_as_moves_the_journal_with_the_project() {
    let t = TempDir::new("saveas");
    let folder = create_project(&t.0, "Original", &fixture(1));
    let copy = t.0.join("Copy");
    let expected = {
        let mut s = open_session(&folder, &SessionOptions::default());
        let tr = tracks(&s.handle.snapshot().doc);
        append(&s.handle, tr[0], 1);
        s.store
            .save_as_snapshot(&copy, &s.handle.committed_snapshot())
            .unwrap();
        append(&s.handle, tr[0], 2);
        s.handle.committed_snapshot().doc.clone()
    };
    // The copy recovers the work after the save-as; the original opens at its last save.
    let s = open_session(&copy, &SessionOptions::default());
    assert_eq!(s.recovered.report.applied, 1);
    assert_eq!(s.handle.committed_snapshot().doc, expected);
    drop(s);
    let s = open_session(&folder, &SessionOptions::default());
    assert!(s.report.recovery.is_none());
    assert_eq!(items(&s.handle.snapshot().doc).len(), 0);
    assert!(
        backups(&folder)
            .iter()
            .any(|b| b.starts_with("journal-saved-as-"))
    );
}

#[test]
fn group_commit_syncs_within_the_window() {
    let t = TempDir::new("group");
    let folder = create_project(&t.0, "Group", &fixture(1));
    let s = open_session(
        &folder,
        &SessionOptions {
            sync: SyncPolicy::Group {
                max_delay: Duration::from_millis(20),
            },
            ..SessionOptions::default()
        },
    );
    let tr = tracks(&s.handle.snapshot().doc);
    append(&s.handle, tr[0], 1);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !s.journal.is_synced() {
        assert!(
            std::time::Instant::now() < deadline,
            "the flusher never synced"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(s.journal.stats().syncs >= 1);
}

// ----- property: replay(journal) == live document --------------------------------------------

#[derive(Clone, Debug)]
enum Action {
    Append(u8, u8),
    Rename(u8, u8),
    Delete(u8),
    RenameProject(u8),
    Undo(bool),
    Redo(bool),
    Batch(Vec<(u8, u8)>),
    Gesture(u8, bool),
    Save,
    Secret,
}

fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        4 => (any::<u8>(), 1..30_u8).prop_map(|(t, f)| Action::Append(t, f)),
        2 => (any::<u8>(), any::<u8>()).prop_map(|(i, n)| Action::Rename(i, n)),
        1 => any::<u8>().prop_map(Action::Delete),
        1 => any::<u8>().prop_map(Action::RenameProject),
        2 => any::<bool>().prop_map(Action::Undo),
        1 => any::<bool>().prop_map(Action::Redo),
        1 => prop::collection::vec((any::<u8>(), 1..10_u8), 1..4).prop_map(Action::Batch),
        1 => (1..4_u8, any::<bool>()).prop_map(|(n, c)| Action::Gesture(n, c)),
        1 => Just(Action::Save),
        1 => Just(Action::Secret),
    ]
}

fn run_action(s: &mut journal_support::Session, a: &Action) {
    let h = &s.handle;
    let doc = h.snapshot().doc.clone();
    let tr = tracks(&doc);
    let all = items(&doc);
    let item = |i: u8| (!all.is_empty()).then(|| all[usize::from(i) % all.len()]);
    let who = |agent_side: bool| if agent_side { agent() } else { ui() };
    match a {
        Action::Append(t, f) => {
            let _ = submit(
                h,
                &ui(),
                "timeline.append_clip",
                json!({"track": tr[usize::from(*t) % tr.len()], "frames": f}),
            );
        }
        Action::Rename(i, n) => {
            if let Some(item) = item(*i) {
                let _ = submit(
                    h,
                    &agent(),
                    "item.rename",
                    json!({"item": item, "name": format!("r{n}")}),
                );
            }
        }
        Action::Delete(i) => {
            if let Some(item) = item(*i) {
                let _ = submit(h, &ui(), "item.delete", json!({"item": item}));
            }
        }
        Action::RenameProject(n) => {
            let _ = submit(h, &ui(), "project.rename", json!({"name": format!("p{n}")}));
        }
        Action::Undo(a) => {
            let _ = submit(h, &who(*a), "edit.undo", json!({}));
        }
        Action::Redo(a) => {
            let _ = submit(h, &who(*a), "edit.redo", json!({}));
        }
        Action::Batch(steps) => {
            let tx = h.begin_tx(&ui(), TxKind::Batch, "batch").wait().unwrap();
            for (t, f) in steps {
                let _ = h
                    .submit(
                        Submit::new(
                            ui(),
                            "timeline.append_clip",
                            json!({"track": tr[usize::from(*t) % tr.len()], "frames": f}),
                        )
                        .in_tx(tx.id),
                    )
                    .wait();
            }
            let _ = h.commit_tx(&ui(), tx.id).wait();
        }
        Action::Gesture(n, commit) => {
            if let Some(item) = item(*n) {
                let g = h.begin_tx(&ui(), TxKind::Gesture, "drag").wait().unwrap();
                for k in 0..*n {
                    let _ = h
                        .submit(
                            Submit::new(
                                ui(),
                                "item.rename",
                                json!({"item": item, "name": format!("g{k}")}),
                            )
                            .in_tx(g.id),
                        )
                        .wait();
                }
                if *commit {
                    let _ = h.commit_tx(&ui(), g.id).wait();
                } else {
                    let _ = h.cancel_tx(&ui(), g.id).wait();
                }
            }
        }
        Action::Save => {
            s.store.save_snapshot(&h.committed_snapshot()).unwrap();
        }
        Action::Secret => {
            let _ = submit(
                h,
                &ui(),
                "ai.set_key",
                json!({"provider": "p", "key": SENTINEL}),
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    #[test]
    fn replaying_the_journal_reproduces_the_live_document(actions in prop::collection::vec(action(), 1..40)) {
        let t = TempDir::new("prop");
        let folder = create_project(&t.0, "Prop", &fixture(2));
        // The undo history is rebuilt from the transactions since the last save.
        let saved = actions.iter().any(|a| matches!(a, Action::Save));
        let (live, history_after_undo) = {
            let mut s = open_session(&folder, &SessionOptions::default());
            for a in &actions {
                run_action(&mut s, a);
            }
            let live = s.handle.committed_snapshot();
            // What one more undo would do (checked again after recovery).
            let undo = if saved {
                None
            } else {
                submit(&s.handle, &ui(), "edit.undo", json!({})).map(|a| a.ops).ok()
            };
            let after = s.handle.committed_snapshot();
            // Put the document back to `live` (the undo is journaled too; recovery must
            // replay it and its redo).
            if undo.is_some() {
                submit(&s.handle, &ui(), "edit.redo", json!({})).unwrap();
            }
            prop_assert_eq!(&s.handle.committed_snapshot().doc, &live.doc);
            (s.handle.committed_snapshot(), undo.map(|ops| (ops, after.doc.clone())))
        };
        let s = open_session(&folder, &SessionOptions::default());
        prop_assert!(s.recovered.report.stopped.is_none());
        prop_assert_eq!(&s.handle.committed_snapshot().doc, &live.doc);
        prop_assert_eq!(s.recovered.rev, live.committed_rev);
        if let Some((ops, after)) = history_after_undo {
            let undone = submit(&s.handle, &ui(), "edit.undo", json!({})).unwrap();
            prop_assert_eq!(undone.ops, ops);
            prop_assert_eq!(&s.handle.committed_snapshot().doc, &after);
        }
        let bytes = all_journal_bytes(&folder.join(".cutweave"));
        prop_assert!(!contains(&bytes, SENTINEL.as_bytes()));
    }
}
