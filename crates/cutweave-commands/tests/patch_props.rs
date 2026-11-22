// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.01 property tests: patches invert exactly.
//!
//! - Random JSON edits on random JSON documents: `apply(apply(doc, p), invert(p)) == doc`.
//! - Random `Tx` edits (and raw field edits) on random valid projects (`arb_project`): the
//!   recorded ops and the coalesced commit patch both replay and invert exactly on the typed
//!   document, agree with the JSON mirror, survive serialization, and keep the start-time index
//!   of every track consistent; failed edits leave the transaction unchanged.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation
)]

use cutweave_commands::{
    Access, DocPath, Patch, PatchOp, Transaction, apply_patch, apply_patch_json, diff, invert,
    json_eq,
};
use cutweave_model::{
    Clip, Duration, ItemId, MediaAsset, MediaId, MediaKind, ProbeInfo, Project, SeededGen,
    Sequence, SequenceFormat, SequenceId, Time, Track, TrackId, TrackKind, testing::arb_project,
};
use proptest::{prelude::*, test_runner::FileFailurePersistence};
use serde_json::{Map, Value, json};

/// 256 cases unless `PROPTEST_CASES` says otherwise; failing seeds are saved next to this file
/// (`patch_props.proptest-regressions`, committed when one appears).
fn config() -> ProptestConfig {
    ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource(
            "proptest-regressions",
        ))),
        ..ProptestConfig::default()
    }
}

// ----- random JSON edits ---------------------------------------------------------------------

fn arb_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i32>().prop_map(Value::from),
        (-1.0e6..1.0e6_f64).prop_map(Value::from),
        "[a-z]{0,4}".prop_map(Value::from),
    ];
    leaf.prop_recursive(4, 40, 5, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            prop::collection::btree_map("[a-c~/]{1,3}", inner, 0..5)
                .prop_map(|m| Value::Object(m.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

fn all_paths(value: &Value, at: &DocPath, out: &mut Vec<DocPath>) {
    out.push(at.clone());
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                all_paths(v, &at.join(k), out);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                all_paths(v, &at.join(i), out);
            }
        }
        _ => {}
    }
}

fn get<'a>(doc: &'a Value, path: &DocPath) -> Option<&'a Value> {
    doc.pointer(path.as_str())
}

/// A free slot (absent key, or array index `0..=len`) in the container at `container`.
fn free_slot(doc: &Value, container: &DocPath, pick: u64, key: &str) -> Option<DocPath> {
    match get(doc, container)? {
        Value::Object(map) => {
            let mut key = key.to_owned();
            while map.contains_key(&key) {
                key.push('x');
            }
            Some(container.join(key))
        }
        Value::Array(items) => Some(container.join(pick as usize % (items.len() + 1))),
        _ => None,
    }
}

fn pick<T: Clone>(items: &[T], n: u64) -> Option<T> {
    (!items.is_empty()).then(|| items[n as usize % items.len()].clone())
}

fn json_op(doc: &Value, kind: u8, a: u64, b: u64, value: &Value, key: &str) -> Option<PatchOp> {
    let mut paths = Vec::new();
    all_paths(doc, &DocPath::root(), &mut paths);
    let non_root: Vec<DocPath> = paths.iter().filter(|p| !p.is_root()).cloned().collect();
    let containers: Vec<DocPath> = paths
        .iter()
        .filter(|p| get(doc, p).is_some_and(|v| v.is_object() || v.is_array()))
        .cloned()
        .collect();
    match kind % 4 {
        0 => Some(PatchOp::Add {
            path: free_slot(doc, &pick(&containers, a)?, b, key)?,
            value: value.clone(),
        }),
        1 => {
            let path = pick(&non_root, a)?;
            Some(PatchOp::Remove {
                old: get(doc, &path)?.clone(),
                path,
            })
        }
        2 => {
            let path = pick(&paths, a)?;
            Some(PatchOp::Replace {
                old: get(doc, &path)?.clone(),
                value: value.clone(),
                path,
            })
        }
        _ => {
            let from = pick(&non_root, a)?;
            let mut after = doc.clone();
            apply_patch_json(
                &mut after,
                &Patch(vec![PatchOp::Remove {
                    path: from.clone(),
                    old: get(doc, &from)?.clone(),
                }]),
            )
            .ok()?;
            let mut targets = Vec::new();
            all_paths(&after, &DocPath::root(), &mut targets);
            targets.retain(|p| get(&after, p).is_some_and(|v| v.is_object() || v.is_array()));
            Some(PatchOp::Move {
                path: free_slot(&after, &pick(&targets, b)?, a, key)?,
                from,
            })
        }
    }
}

type JsonSeed = (u8, u64, u64, Value, String);

proptest! {
    #![proptest_config(config())]

    #[test]
    fn random_json_edits_invert_exactly(
        doc in prop::collection::btree_map("[a-c]{1,2}", arb_json(), 0..5)
            .prop_map(|m| Value::Object(m.into_iter().collect::<Map<_, _>>())),
        seeds in prop::collection::vec(
            (any::<u8>(), any::<u64>(), any::<u64>(), arb_json(), "[a-c~/]{1,3}"),
            1..16,
        ),
    ) {
        let mut current = doc.clone();
        let mut patch = Patch::new();
        for (kind, a, b, value, key) in &seeds as &[JsonSeed] {
            let Some(op) = json_op(&current, *kind, *a, *b, value, key) else { continue };
            // Every generated op is valid where it is applied (moves into their own subtree
            // excepted, which are refused without a change).
            let before = current.clone();
            if apply_patch_json(&mut current, &Patch(vec![op.clone()])).is_ok() {
                patch.push(op);
            } else {
                prop_assert_eq!(&current, &before);
            }
        }
        let mut replay = doc.clone();
        apply_patch_json(&mut replay, &patch).unwrap();
        prop_assert_eq!(&replay, &current);
        apply_patch_json(&mut replay, &invert(&patch)).unwrap();
        prop_assert_eq!(&replay, &doc);
        prop_assert_eq!(invert(&invert(&patch)), patch.clone());
        let wire: Patch = serde_json::from_value(serde_json::to_value(&patch).unwrap()).unwrap();
        prop_assert_eq!(wire, patch);
    }
}

// ----- random typed edits --------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Seed {
    kind: u8,
    a: u64,
    b: u64,
    x: f64,
    flag: bool,
    text: String,
}

fn arb_seed() -> impl Strategy<Value = Seed> {
    (
        any::<u8>(),
        any::<u64>(),
        any::<u64>(),
        -4.0..4.0_f64,
        any::<bool>(),
        "[a-z]{1,6}",
    )
        .prop_map(|(kind, a, b, x, flag, text)| Seed {
            kind,
            a,
            b,
            x,
            flag,
            text,
        })
}

struct Catalog {
    items: Vec<(SequenceId, TrackId, ItemId)>,
    tracks: Vec<(SequenceId, TrackId)>,
    sequences: Vec<SequenceId>,
    media: Vec<MediaId>,
}

fn catalog(doc: &Project) -> Catalog {
    let mut items = Vec::new();
    let mut tracks = Vec::new();
    for s in doc.sequences.values() {
        for t in s.tracks.values() {
            tracks.push((s.id, t.id));
            items.extend(t.items.iter_by_id().map(|(id, _)| (s.id, t.id, *id)));
        }
    }
    Catalog {
        items,
        tracks,
        sequences: doc.sequences.keys().copied().collect(),
        media: doc.media.keys().copied().collect(),
    }
}

fn scalar_leaves(value: &Value, at: &DocPath, out: &mut Vec<(DocPath, Value)>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                scalar_leaves(v, &at.join(k), out);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                scalar_leaves(v, &at.join(i), out);
            }
        }
        other => out.push((at.clone(), other.clone())),
    }
}

fn frames(n: u64) -> i64 {
    (n % 600).cast_signed() * 23_520_000
}

/// Runs one generated edit; `Err` edits (locked tracks, missing targets, values the model
/// rejects) are expected and must not change anything.
#[allow(clippy::too_many_lines)]
fn edit(txn: &mut Transaction, ids: &mut SeededGen, s: &Seed) -> Result<(), String> {
    let c = catalog(txn.doc());
    let mut tx = txn.tx(ids, Access::STANDARD);
    let none = || "no target".to_owned();
    let r = match s.kind % 14 {
        0 => {
            let (_, _, item) = pick(&c.items, s.a).ok_or_else(none)?;
            let x = s.x;
            let flag = s.flag;
            let text = s.text.clone();
            let b = s.b;
            tx.update_item(item, move |i| {
                match b % 5 {
                    0 => i.set_start(Time::from_flicks(frames(b)).unwrap()),
                    1 => i.set_duration(Duration::from_flicks(frames(b) + 1).unwrap()),
                    2 => i.set_enabled(flag),
                    _ => {
                        let clip = i.as_clip_mut().unwrap();
                        clip.visual.opacity.set(x.abs().min(1.0));
                        clip.audio.gain.set(x);
                        clip.name = flag.then_some(text);
                    }
                }
                Ok(())
            })
        }
        1 => {
            let (_, track) = pick(&c.tracks, s.a).ok_or_else(none)?;
            let media = pick(&c.media, s.b).ok_or_else(none)?;
            let clip = Clip::new(
                tx.new_id(),
                media,
                Time::from_flicks(frames(s.b)).unwrap(),
                Duration::from_flicks(frames(s.a) + 23_520_000).unwrap(),
            );
            tx.insert_item(track, &clip.into())
        }
        2 => {
            let (_, _, item) = pick(&c.items, s.a).ok_or_else(none)?;
            tx.remove_item(item).map(drop)
        }
        3 => {
            let (_, _, item) = pick(&c.items, s.a).ok_or_else(none)?;
            let (_, track) = pick(&c.tracks, s.b).ok_or_else(none)?;
            tx.move_item(item, track)
        }
        4 => {
            let sequence = pick(&c.sequences, s.a).ok_or_else(none)?;
            let kind = if s.flag {
                TrackKind::Video
            } else {
                TrackKind::Audio
            };
            let track = Track::new(tx.new_id(), kind, s.text.clone());
            tx.insert_track(sequence, s.b as usize % 5, &track)
        }
        5 => {
            let (_, track) = pick(&c.tracks, s.a).ok_or_else(none)?;
            tx.remove_track(track).map(drop)
        }
        6 => {
            let (_, track) = pick(&c.tracks, s.a).ok_or_else(none)?;
            let (flag, x, b, text) = (s.flag, s.x, s.b, s.text.clone());
            tx.update_track(track, move |t| {
                match b % 4 {
                    0 => t.locked = flag,
                    1 => t.muted = flag,
                    2 => t.gain = x,
                    _ => t.name = text,
                }
                Ok(())
            })
        }
        7 => {
            let sequence = pick(&c.sequences, s.a).ok_or_else(none)?;
            let (text, b) = (s.text.clone(), s.b);
            tx.update_sequence(sequence, move |seq| {
                seq.name = text;
                seq.format.width = 16 + (b % 4000) as u32;
                if b % 3 == 0 {
                    // Reorder the tracks.
                    let first = seq.track_order.pop_front();
                    seq.track_order.extend(first);
                }
                Ok(())
            })
        }
        8 => {
            let media = MediaAsset::file(
                tx.new_id(),
                MediaKind::Audio,
                format!("/m/{}.wav", s.text),
                ProbeInfo::default(),
            );
            tx.insert_media(&media)
        }
        9 => {
            let media = pick(&c.media, s.a).ok_or_else(none)?;
            if s.flag {
                tx.remove_media(media).map(drop)
            } else {
                let text = s.text.clone();
                tx.update_media(media, move |m| {
                    m.path_rel = Some(text);
                    Ok(())
                })
            }
        }
        10 => match s.b % 3 {
            0 => tx.update_meta(|m| {
                m.name.clone_from(&s.text);
                Ok(())
            }),
            1 => tx.set(
                &DocPath::custom().join(format!("com.example.{}", s.text)),
                json!({"x": s.x, "on": s.flag}),
            ),
            _ => tx.set(&DocPath::meta().join(format!("x_{}", s.text)), json!(s.a)),
        },
        11 => {
            // A raw edit of one scalar field of an item, with a value of the same JSON type.
            let (sequence, track, item) = pick(&c.items, s.a).ok_or_else(none)?;
            let path = DocPath::item(sequence, track, item);
            let mut leaves = Vec::new();
            scalar_leaves(&tx.get(&path).unwrap(), &path, &mut leaves);
            let (leaf, old) = pick(&leaves, s.b).ok_or_else(none)?;
            let value = match &old {
                Value::Bool(v) => json!(!v),
                Value::Number(n) if n.is_f64() => json!(s.x),
                Value::Number(n) => json!(n.as_i64().unwrap_or(0) + 23_520_000),
                Value::String(_) | Value::Null => json!(s.text),
                _ => return Err(none()),
            };
            tx.apply(PatchOp::Replace {
                path: leaf,
                old,
                value,
            })
        }
        12 => {
            let sequence = Sequence::new(tx.new_id(), s.text.clone(), SequenceFormat::default());
            tx.insert_sequence(&sequence)
        }
        _ => {
            let sequence = pick(&c.sequences, s.a).ok_or_else(none)?;
            tx.remove_sequence(sequence).map(drop)
        }
    };
    r.map_err(|e| e.to_string())
}

fn index_is_consistent(doc: &Project) -> bool {
    doc.sequences.values().all(|s| {
        s.tracks.values().all(|t| {
            let starts: Vec<Time> = t.items.iter().map(cutweave_model::Item::start).collect();
            starts.len() == t.items.len() && starts.windows(2).all(|w| w[0] <= w[1])
        })
    })
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn random_tx_edits_replay_and_invert_exactly(
        base in arb_project(),
        seeds in prop::collection::vec(arb_seed(), 1..24),
        seed in any::<u64>(),
    ) {
        let mut ids = SeededGen::new(seed);
        let mut txn = Transaction::begin(base.clone());
        let mut applied = 0;
        for s in &seeds {
            let (doc, len) = (txn.doc().clone(), txn.ops().len());
            if edit(&mut txn, &mut ids, s).is_ok() {
                applied += 1;
            } else {
                prop_assert_eq!(txn.doc(), &doc);
                prop_assert_eq!(txn.ops().len(), len);
            }
            prop_assert!(index_is_consistent(txn.doc()));
        }
        let recorded = Patch(txn.ops().to_vec());
        let doc = txn.doc().clone();

        // The recorded ops replay and invert exactly.
        prop_assert_eq!(&apply_patch(&base, &recorded).unwrap(), &doc);
        let undone = apply_patch(&doc, &invert(&recorded)).unwrap();
        prop_assert_eq!(&undone, &base);
        prop_assert!(index_is_consistent(&undone));

        // The JSON mirror follows the typed document op by op, both ways.
        let base_json = serde_json::to_value(&base).unwrap();
        let mut mirror = base_json.clone();
        apply_patch_json(&mut mirror, &recorded).unwrap();
        prop_assert!(json_eq(&mirror, &serde_json::to_value(&doc).unwrap()));
        apply_patch_json(&mut mirror, &invert(&recorded)).unwrap();
        prop_assert!(json_eq(&mirror, &base_json));

        // The coalesced commit patch (the field-level diff) does the same.
        let field_level = diff(&base, &doc);
        prop_assert!(field_level.is_ok(), "{:?}", field_level);
        let committed = txn.commit();
        prop_assert_eq!(&committed.patch, &field_level.unwrap());
        prop_assert_eq!(&apply_patch(&base, &committed.patch).unwrap(), &doc);
        prop_assert_eq!(&apply_patch(&doc, &committed.inverse).unwrap(), &base);
        let mut mirror = base_json;
        apply_patch_json(&mut mirror, &committed.patch).unwrap();
        prop_assert!(json_eq(&mirror, &serde_json::to_value(&doc).unwrap()));

        // Both survive the wire.
        for patch in [&recorded, &committed.patch] {
            let wire: Patch = serde_json::from_value(serde_json::to_value(patch).unwrap()).unwrap();
            prop_assert_eq!(&apply_patch(&base, &wire).unwrap(), &doc);
        }
        prop_assert_eq!(committed.is_empty(), doc == base);
        let _ = applied;
    }
}
