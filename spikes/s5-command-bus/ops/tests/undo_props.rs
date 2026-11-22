//! Property tests (arch §5.2): for every undoable command, `undo(apply(doc)) == doc` and
//! `redo(undo(apply(doc))) == apply(doc)`; failed commands leave the doc untouched; model
//! invariants hold after every step. Ops run through the type-erased JSON path with randomly
//! chosen (often invalid) arguments, including timecode-string times.

// Test crate: helpers outside #[test] fns may unwrap (clippy.toml only exempts #[test] bodies).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_pass_by_value
)]

mod common;

use std::collections::BTreeSet;

use common::{agent, engine, secs};
use proptest::{collection::vec, prelude::*, test_runner::TestCaseError};
use s5_bus::{Caller, Engine, engine::Request, time::FLICKS_PER_SECOND};
use serde_json::{Value, json};

/// Undoable commands covered by a generator below. `every_undoable_command_has_a_generator`
/// fails when a new undoable command is registered without one.
const COVERED: &[&str] = &[
    "clip.split",
    "clip.trim",
    "item.delete",
    "media.import",
    "timeline.insert",
];

#[derive(Clone, Debug)]
enum Op {
    Import {
        secs: u8,
        ext: u8,
    },
    Insert {
        media: u8,
        at_frames: u16,
        track: Option<u8>,
        source_in_frames: u8,
        duration_frames: Option<u16>,
    },
    Split {
        item: u8,
        pct: u8,
    },
    Trim {
        item: u8,
        start_delta: Option<i16>,
        end_delta: Option<i16>,
    },
    Delete {
        items: Vec<u8>,
    },
}

const FRAME: i64 = FLICKS_PER_SECOND / 30;

fn import() -> impl Strategy<Value = Op> {
    (0_u8..=20, 0_u8..6).prop_map(|(secs, ext)| Op::Import { secs, ext })
}
fn insert() -> impl Strategy<Value = Op> {
    (
        any::<u8>(),
        0_u16..900,
        proptest::option::of(any::<u8>()),
        0_u8..120,
        proptest::option::of(0_u16..400),
    )
        .prop_map(
            |(media, at_frames, track, source_in_frames, duration_frames)| Op::Insert {
                media,
                at_frames,
                track,
                source_in_frames,
                duration_frames,
            },
        )
}
fn split() -> impl Strategy<Value = Op> {
    (any::<u8>(), 0_u8..=100).prop_map(|(item, pct)| Op::Split { item, pct })
}
fn trim() -> impl Strategy<Value = Op> {
    (
        any::<u8>(),
        proptest::option::of(-90_i16..90),
        proptest::option::of(-90_i16..90),
    )
        .prop_map(|(item, start_delta, end_delta)| Op::Trim {
            item,
            start_delta,
            end_delta,
        })
}
fn delete() -> impl Strategy<Value = Op> {
    vec(any::<u8>(), 0..4).prop_map(|items| Op::Delete { items })
}
fn any_op() -> impl Strategy<Value = Op> {
    prop_oneof![import(), insert(), split(), trim(), delete()]
}

fn pick<K: ToString>(keys: impl Iterator<Item = K>, sel: u8, fallback: &str) -> String {
    let keys: Vec<String> = keys.map(|k| k.to_string()).collect();
    if keys.is_empty() {
        fallback.to_owned()
    } else {
        keys[usize::from(sel) % keys.len()].clone()
    }
}

/// Frames as either integer flicks or an `f:` timecode string (both must behave identically).
fn time_arg(frames: i64) -> Value {
    if frames >= 0 && frames % 3 == 0 {
        json!(format!("f:{frames}"))
    } else {
        json!(frames * FRAME)
    }
}

fn to_request(e: &Engine, op: &Op) -> (&'static str, Value) {
    let doc = e.doc();
    let item_id = |sel| {
        pick(
            doc.items.keys(),
            sel,
            "itm_00000000000000000000000000000000",
        )
    };
    match op {
        Op::Import { secs, ext } => {
            let ext = ["mp4", "wav", "png", "mov", "txt", "jpg"][usize::from(*ext)];
            (
                "media.import",
                json!({ "path": format!("/m/clip{secs}.{ext}"), "duration": i64::from(*secs) * FLICKS_PER_SECOND }),
            )
        }
        Op::Insert {
            media,
            at_frames,
            track,
            source_in_frames,
            duration_frames,
        } => {
            let mut p = json!({
                "media": pick(doc.media.keys(), *media, "med_00000000000000000000000000000000"),
                "at": time_arg(i64::from(*at_frames)),
                "source_in": time_arg(i64::from(*source_in_frames)),
            });
            if let Some(t) = track {
                p["track"] = json!(pick(
                    doc.tracks.keys(),
                    *t,
                    "trk_00000000000000000000000000000000"
                ));
            }
            if let Some(d) = duration_frames {
                p["duration"] = time_arg(i64::from(*d));
            }
            ("timeline.insert", p)
        }
        Op::Split { item, pct } => {
            let id = item_id(*item);
            let at = doc
                .items
                .values()
                .find(|i| i.id.as_str() == id)
                .map_or(0, |i| {
                    i.start.flicks() + i.duration.flicks() * i64::from(*pct) / 100
                });
            ("clip.split", json!({ "item": id, "at": at }))
        }
        Op::Trim {
            item,
            start_delta,
            end_delta,
        } => {
            let id = item_id(*item);
            let mut p = json!({ "item": id });
            if let Some(i) = doc.items.values().find(|i| i.id.as_str() == id) {
                if let Some(d) = start_delta {
                    p["start"] = json!(i.start.flicks() + i64::from(*d) * FRAME);
                }
                if let Some(d) = end_delta {
                    p["end"] = json!(i.end().flicks() + i64::from(*d) * FRAME);
                }
            }
            ("clip.trim", p)
        }
        Op::Delete { items } => {
            let ids: Vec<String> = items.iter().map(|s| item_id(*s)).collect();
            ("item.delete", json!({ "items": ids }))
        }
    }
}

fn run(e: &mut Engine, caller: &Caller, op: &Op) -> Result<Value, s5_bus::CmdError> {
    let (name, params) = to_request(e, op);
    e.invoke(caller, Request::new(name, params))
        .map(|r| r.result)
}

/// A doc with some media, items on two video tracks and an audio track.
fn seeded() -> Engine {
    let mut e = engine();
    let a = agent();
    let mut ok = |name: &str, p: Value| e.invoke(&a, Request::new(name, p)).unwrap().result;
    let video = ok(
        "media.import",
        json!({ "path": "/m/v.mp4", "duration": secs(20) }),
    )["media"]
        .clone();
    let audio = ok("media.import", json!({ "path": "/m/a.wav" }))["media"].clone();
    ok(
        "timeline.insert",
        json!({ "media": video, "at": 0, "duration": secs(6) }),
    );
    ok(
        "timeline.insert",
        json!({ "media": video, "at": secs(8), "source_in": secs(4) }),
    );
    ok(
        "timeline.insert",
        json!({ "media": video, "at": secs(3), "duration": secs(2) }),
    );
    ok("timeline.insert", json!({ "media": audio, "at": 0 }));
    e
}

fn check_undo_redo(prefix: &[Op], last: &Op) -> Result<(), TestCaseError> {
    let mut e = seeded();
    let a = agent();
    for op in prefix {
        let _ = run(&mut e, &a, op);
        prop_assert!(
            e.doc().check_invariants().is_ok(),
            "{:?}",
            e.doc().check_invariants()
        );
    }
    let before = e.doc().clone();
    let rev = e.rev();
    if run(&mut e, &a, last).is_err() {
        prop_assert_eq!(
            e.doc(),
            &before,
            "a failed command leaves the doc untouched"
        );
        prop_assert_eq!(e.rev(), rev);
        return Ok(());
    }
    let after = e.doc().clone();
    prop_assert!(
        after.check_invariants().is_ok(),
        "{:?}",
        after.check_invariants()
    );
    if after == before {
        prop_assert_eq!(e.rev(), rev, "no-op commands do not create a revision");
        return Ok(());
    }
    let fail = |err: s5_bus::CmdError| TestCaseError::fail(err.to_string());
    prop_assert!(e.undo(&a).map_err(fail)?.is_some());
    prop_assert_eq!(e.doc(), &before, "undo(apply(doc)) == doc");
    prop_assert!(e.redo(&a).map_err(fail)?.is_some());
    prop_assert_eq!(e.doc(), &after, "redo(undo(apply(doc))) == apply(doc)");
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    #[test]
    fn undo_inverts_media_import(prefix in vec(any_op(), 0..10), last in import()) {
        check_undo_redo(&prefix, &last)?;
    }

    #[test]
    fn undo_inverts_timeline_insert(prefix in vec(any_op(), 0..10), last in insert()) {
        check_undo_redo(&prefix, &last)?;
    }

    #[test]
    fn undo_inverts_clip_split(prefix in vec(any_op(), 0..10), last in split()) {
        check_undo_redo(&prefix, &last)?;
    }

    #[test]
    fn undo_inverts_clip_trim(prefix in vec(any_op(), 0..10), last in trim()) {
        check_undo_redo(&prefix, &last)?;
    }

    #[test]
    fn undo_inverts_item_delete(prefix in vec(any_op(), 0..10), last in delete()) {
        check_undo_redo(&prefix, &last)?;
    }

    /// Undoing a whole random session returns to the start; redoing it all returns to the end.
    #[test]
    fn undo_all_then_redo_all(script in vec(any_op(), 1..25)) {
        let mut e = seeded();
        let a = agent();
        let start = e.doc().clone();
        let seeded_depth = e.undo_depth(a.actor());
        for op in &script {
            let _ = run(&mut e, &a, op);
        }
        let end = e.doc().clone();
        let mut undone = 0;
        while e.undo_depth(a.actor()) > seeded_depth {
            prop_assert!(e.undo(&a).map_err(|err| TestCaseError::fail(err.to_string()))?.is_some());
            undone += 1;
        }
        prop_assert_eq!(e.doc(), &start);
        for _ in 0..undone {
            prop_assert!(e.redo(&a).map_err(|err| TestCaseError::fail(err.to_string()))?.is_some());
        }
        prop_assert_eq!(e.doc(), &end);
    }
}

#[test]
fn every_undoable_command_has_a_generator() {
    let registered: BTreeSet<&str> = s5_ops::registry()
        .iter()
        .filter(|e| e.meta.undoable && !e.is_engine_op())
        .map(|e| e.meta.name)
        .collect();
    let covered: BTreeSet<&str> = COVERED.iter().copied().collect();
    assert_eq!(
        registered, covered,
        "add a proptest generator for new undoable commands"
    );
}

#[test]
fn generators_reach_success_paths() {
    // Guard against generators that only ever produce errors (which would make the undo
    // properties vacuous): with a fixed seed, every covered command succeeds sometimes.
    use proptest::{strategy::ValueTree, test_runner::TestRunner};
    let mut runner = TestRunner::deterministic();
    let mut successes: BTreeSet<&'static str> = BTreeSet::new();
    for _ in 0..300 {
        let op = any_op().new_tree(&mut runner).unwrap().current();
        let mut e = seeded();
        let (name, params) = to_request(&e, &op);
        if e.invoke(&agent(), Request::new(name, params)).is_ok() {
            successes.insert(name);
        }
    }
    let covered: BTreeSet<&str> = COVERED.iter().copied().collect();
    assert_eq!(successes, covered);
}
