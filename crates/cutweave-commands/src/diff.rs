// SPDX-License-Identifier: GPL-3.0-or-later
//! [`diff`]: the field-level patch between two versions of a document.
//!
//! The persistent collections make this cheap: unchanged media, sequences and tracks are
//! skipped by pointer without being visited (`imbl` diffs skip shared nodes), so the cost is
//! proportional to what changed, plus one pass over the items of each changed track.

use cutweave_model::{
    BackgroundFill, Collection, Item, Markers, MediaAsset, Project, Sequence, Track, TrackItems,
};
use imbl::{OrdMap, ordmap::DiffItem};
use serde::Serialize;
use serde_json::Value;

use crate::{
    apply::to_json,
    json,
    patch::{Patch, PatchError, PatchOp, apply_patch},
    path::DocPath,
};

type Ops = Vec<PatchOp>;

/// The ops that turn `before` into `after`: entities added or removed whole, changed entities
/// as field-level `replace` / `add` / `remove` ops (arrays are replaced whole).
///
/// The result is verified: `apply_patch(before, &diff(before, after)?) == after`. An `Err`
/// means the change cannot be expressed field by field (the model would have to pass through
/// an invalid intermediate state); callers then fall back to the ops they recorded, or to a
/// coarser patch.
pub fn diff(before: &Project, after: &Project) -> Result<Patch, PatchError> {
    let mut ops = Vec::new();
    diff_project(before, after, &mut ops)?;
    let patch = Patch(ops);
    if apply_patch(before, &patch).as_ref() == Ok(after) {
        Ok(patch)
    } else {
        Err(PatchError::Type {
            path: DocPath::root(),
            msg: "the change cannot be expressed as field-level ops".into(),
        })
    }
}

fn diff_project(before: &Project, after: &Project, out: &mut Ops) -> Result<(), PatchError> {
    let root = DocPath::root();
    let shallow = |p: &Project| Project {
        media: OrdMap::new(),
        collections: OrdMap::new(),
        sequences: OrdMap::new(),
        ..p.clone()
    };
    diff_shallow(
        &root,
        &shallow(before),
        &shallow(after),
        &["media", "collections", "sequences"],
        out,
    )?;
    diff_omitted_while_empty(
        &root.join("collections"),
        &before.collections,
        &after.collections,
        out,
        diff_collection,
    )?;
    diff_keyed(
        &root.join("media"),
        &before.media,
        &after.media,
        out,
        diff_value::<MediaAsset>,
    )?;
    diff_keyed(
        &root.join("sequences"),
        &before.sequences,
        &after.sequences,
        out,
        diff_sequence,
    )
}

pub(crate) fn diff_sequence(
    path: &DocPath,
    before: &Sequence,
    after: &Sequence,
    out: &mut Ops,
) -> Result<(), PatchError> {
    // The markers are diffed by themselves, like the tracks: a sequence can hold thousands, and
    // every command that changes anything in it would put them all into their JSON form to
    // compare them (issue #230). The key stays in the stripped form, empty, to keep its place.
    let shallow = |s: &Sequence| -> Result<Value, PatchError> {
        let mut s = Sequence {
            tracks: OrdMap::new(),
            markers: Markers::new(),
            ..s.clone()
        };
        s.format.background_fill = None;
        let mut v = to_json(&s, path)?;
        if let Value::Object(map) = &mut v {
            // As `diff_shallow` strips it (the last member takes its place).
            map.remove("tracks");
        }
        Ok(v)
    };
    let own = shallow(before)?;
    let from = out.len();
    json::diff(path, &own, &shallow(after)?, out);
    if !before.markers.ptr_eq(&after.markers) {
        let mut markers = Vec::new();
        diff_markers(
            &path.join("markers"),
            &before.markers,
            &after.markers,
            &mut markers,
        )?;
        let at = from + markers_place(path, &own, &out[from..]);
        out.splice(at..at, markers);
    }
    diff_background_fill(
        &path.join("format").join("background_fill"),
        before.format.background_fill.as_ref(),
        after.format.background_fill.as_ref(),
        out,
    )?;
    diff_keyed(
        &path.join("tracks"),
        &before.tracks,
        &after.tracks,
        out,
        diff_track,
    )
}

/// The ops between two versions of a sequence's markers, in the order the JSON diff of the two
/// maps gives them: the markers that went, then the changed ones field by field, then the new
/// ones, each group in ID order. Markers in nodes both versions share are not visited.
fn diff_markers(
    path: &DocPath,
    before: &Markers,
    after: &Markers,
    out: &mut Ops,
) -> Result<(), PatchError> {
    for change in before.diff(after) {
        if let DiffItem::Remove(key, value) = change {
            remove(&path.join(key), value, out)?;
        }
    }
    for change in before.diff(after) {
        if let DiffItem::Update {
            old: (key, old),
            new: (_, new),
        } = change
        {
            diff_value(&path.join(key), old, new, out)?;
        }
    }
    for change in before.diff(after) {
        if let DiffItem::Add(key, value) = change {
            add(&path.join(key), value, out)?;
        }
    }
    Ok(())
}

/// Where the ops of a sequence's markers go among `ops`, the JSON diff of its other fields
/// (`own` is the sequence's JSON form they were found in, at `path`): where that diff would
/// have put them. It emits the members that went, then the changes of the members both sides
/// have in the order `own` lists them, then the new members; `markers` is always a member of
/// both, so its ops come before the changes of the members listed after it.
fn markers_place(path: &DocPath, own: &Value, ops: &[PatchOp]) -> usize {
    let later: Vec<&str> = own.as_object().map_or_else(Vec::new, |map| {
        map.keys()
            .skip_while(|key| *key != "markers")
            .skip(1)
            .map(String::as_str)
            .collect()
    });
    let member = path.depth() + 1;
    ops.iter()
        .position(|op| {
            let at = op.path();
            let whole = at.depth() == member;
            match op {
                PatchOp::Remove { .. } if whole => false,
                PatchOp::Add { .. } if whole => true,
                _ => at
                    .segments()
                    .nth(member - 1)
                    .is_some_and(|key| later.contains(&key.as_ref())),
            }
        })
        .unwrap_or(ops.len())
}

/// A sequence's background fill (WP-2B.02) is left out of the document while there is none, and
/// its kinds share no field: it appears and disappears whole, and one that changes kind is
/// replaced whole (the halfway states are not valid values). The same kind is diffed field by
/// field.
fn diff_background_fill(
    path: &DocPath,
    before: Option<&BackgroundFill>,
    after: Option<&BackgroundFill>,
    out: &mut Ops,
) -> Result<(), PatchError> {
    match (before, after) {
        (None, None) => Ok(()),
        (None, Some(new)) => add(path, new, out),
        (Some(old), None) => remove(path, old, out),
        (Some(old), Some(new)) if std::mem::discriminant(old) == std::mem::discriminant(new) => {
            diff_value(path, old, new, out)
        }
        (Some(old), Some(new)) => {
            out.push(PatchOp::Replace {
                path: path.clone(),
                old: to_json(old, path)?,
                value: to_json(new, path)?,
            });
            Ok(())
        }
    }
}

pub(crate) fn diff_track(
    path: &DocPath,
    before: &Track,
    after: &Track,
    out: &mut Ops,
) -> Result<(), PatchError> {
    let shallow = |t: &Track| Track {
        items: TrackItems::new(),
        ..t.clone()
    };
    diff_shallow(path, &shallow(before), &shallow(after), &["items"], out)?;
    if before.items.ptr_eq(&after.items) {
        return Ok(());
    }
    diff_items(&path.join("items"), &before.items, &after.items, out)
}

/// Merge-joins two item maps in ID order. Items in nodes shared by both versions compare by
/// address before falling back to `==`.
fn diff_items(
    path: &DocPath,
    before: &TrackItems,
    after: &TrackItems,
    out: &mut Ops,
) -> Result<(), PatchError> {
    let mut b = before.iter_by_id().peekable();
    let mut a = after.iter_by_id().peekable();
    loop {
        match (b.peek(), a.peek()) {
            (Some((bk, bv)), Some((ak, av))) => match bk.cmp(ak) {
                std::cmp::Ordering::Less => {
                    remove(&path.join(bk), *bv, out)?;
                    b.next();
                }
                std::cmp::Ordering::Greater => {
                    add(&path.join(ak), *av, out)?;
                    a.next();
                }
                std::cmp::Ordering::Equal => {
                    if !std::ptr::eq::<Item>(*bv, *av) && bv != av {
                        let path = path.join(bk);
                        if !diff_moved(&path, bv, av, out) {
                            diff_value(&path, *bv, *av, out)?;
                        }
                    }
                    b.next();
                    a.next();
                }
            },
            (Some((bk, bv)), None) => {
                remove(&path.join(bk), *bv, out)?;
                b.next();
            }
            (None, Some((ak, av))) => {
                add(&path.join(ak), *av, out)?;
                a.next();
            }
            (None, None) => return Ok(()),
        }
    }
}

/// The op for two versions of an item that differ in nothing but `start`: what a ripple does
/// to every item it shifts, so a ripple of 2,000 items is 2,000 of these (issue #231). It is
/// the op [`diff_value`] finds (`replace …/start`, flicks as JSON integers), without putting
/// both items into their JSON form to find it. `false`, and nothing pushed, when the items
/// differ in anything else.
fn diff_moved(path: &DocPath, before: &Item, after: &Item, out: &mut Ops) -> bool {
    if before.start() == after.start() {
        return false;
    }
    let mut moved = before.clone();
    moved.set_start(after.start());
    if moved != *after {
        return false;
    }
    let op = PatchOp::Replace {
        path: path.join("start"),
        old: Value::from(before.start().flicks()),
        value: Value::from(after.start().flicks()),
    };
    // Debug builds (every test run) compute the general diff as well and compare.
    #[cfg(debug_assertions)]
    crate::effort::uncounted(|| {
        let mut general = Vec::new();
        debug_assert!(diff_value(path, before, after, &mut general).is_ok());
        debug_assert_eq!(
            general,
            std::slice::from_ref(&op),
            "the JSON diff of a moved item"
        );
    });
    out.push(op);
    true
}

fn diff_keyed<K, V>(
    path: &DocPath,
    before: &OrdMap<K, V>,
    after: &OrdMap<K, V>,
    out: &mut Ops,
    child: impl Fn(&DocPath, &V, &V, &mut Ops) -> Result<(), PatchError>,
) -> Result<(), PatchError>
where
    K: Ord + Clone + std::fmt::Display,
    V: Clone + PartialEq + Serialize,
{
    for change in before.diff(after) {
        match change {
            DiffItem::Add(key, value) => add(&path.join(key), value, out)?,
            DiffItem::Remove(key, value) => remove(&path.join(key), value, out)?,
            DiffItem::Update {
                old: (key, old),
                new: (_, new),
            } => child(&path.join(key), old, new, out)?,
        }
    }
    Ok(())
}

/// Diffs an ID-keyed map that is left out of the document while it is empty
/// (`Project::collections`, ADR-0036), so that no op leaves it present and empty: the document
/// model cannot hold that, and neither can the JSON form of the document, where the key is
/// then absent. A map that appears or disappears is added or removed whole; otherwise entries
/// are added, then changed, then removed, so the map is never empty in between.
fn diff_omitted_while_empty<K, V>(
    path: &DocPath,
    before: &OrdMap<K, V>,
    after: &OrdMap<K, V>,
    out: &mut Ops,
    child: impl Fn(&DocPath, &V, &V, &mut Ops) -> Result<(), PatchError>,
) -> Result<(), PatchError>
where
    K: Ord + Clone + std::fmt::Display + Serialize,
    V: Clone + PartialEq + Serialize,
{
    match (before.is_empty(), after.is_empty()) {
        (true, true) => return Ok(()),
        (true, false) => return add(path, after, out),
        (false, true) => return remove(path, before, out),
        (false, false) => {}
    }
    for change in before.diff(after) {
        if let DiffItem::Add(key, value) = change {
            add(&path.join(key), value, out)?;
        }
    }
    for change in before.diff(after) {
        if let DiffItem::Update {
            old: (key, old),
            new: (_, new),
        } = change
        {
            child(&path.join(key), old, new, out)?;
        }
    }
    for change in before.diff(after) {
        if let DiffItem::Remove(key, value) = change {
            remove(&path.join(key), value, out)?;
        }
    }
    Ok(())
}

/// A collection that changes kind is replaced whole: a manual and a smart collection share no
/// member of `kind`, and the halfway states are not valid values.
fn diff_collection(
    path: &DocPath,
    before: &Collection,
    after: &Collection,
    out: &mut Ops,
) -> Result<(), PatchError> {
    if before.kind.name() == after.kind.name() {
        return diff_value(path, before, after, out);
    }
    out.push(PatchOp::Replace {
        path: path.clone(),
        old: to_json(before, path)?,
        value: to_json(after, path)?,
    });
    Ok(())
}

fn add<T: Serialize>(path: &DocPath, value: &T, out: &mut Ops) -> Result<(), PatchError> {
    out.push(PatchOp::Add {
        path: path.clone(),
        value: to_json(value, path)?,
    });
    Ok(())
}

fn remove<T: Serialize>(path: &DocPath, value: &T, out: &mut Ops) -> Result<(), PatchError> {
    out.push(PatchOp::Remove {
        path: path.clone(),
        old: to_json(value, path)?,
    });
    Ok(())
}

pub(crate) fn diff_value<T: Serialize>(
    path: &DocPath,
    before: &T,
    after: &T,
    out: &mut Ops,
) -> Result<(), PatchError> {
    json::diff(path, &to_json(before, path)?, &to_json(after, path)?, out);
    Ok(())
}

/// Diffs an entity's own fields; `nested` names its (emptied) collections, diffed separately.
fn diff_shallow<T: Serialize>(
    path: &DocPath,
    before: &T,
    after: &T,
    nested: &[&str],
    out: &mut Ops,
) -> Result<(), PatchError> {
    let strip = |value: &T| -> Result<Value, PatchError> {
        let mut v = to_json(value, path)?;
        if let Value::Object(map) = &mut v {
            for key in nested {
                map.remove(*key);
            }
        }
        Ok(v)
    };
    json::diff(path, &strip(before)?, &strip(after)?, out);
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::{CollectionId, CollectionKind, MediaFilter};

    use super::*;
    use crate::patch::{apply_patch_json, invert};

    fn with(collections: &[(u128, &str, CollectionKind)]) -> Project {
        let mut doc = Project::new("Diff");
        for (n, name, kind) in collections {
            let id = CollectionId::from_u128(*n);
            doc.collections
                .insert(id, Collection::new(id, *name, kind.clone()));
        }
        doc
    }

    /// The ops of `before → after` as `op path`, after checking that the patch and its inverse
    /// hold on the typed document and on its JSON form.
    fn ops(before: &Project, after: &Project) -> Vec<String> {
        let patch = diff(before, after).unwrap();
        assert_eq!(&apply_patch(after, &invert(&patch)).unwrap(), before);
        let mut mirror = serde_json::to_value(before).unwrap();
        apply_patch_json(&mut mirror, &patch).unwrap();
        assert_eq!(mirror, serde_json::to_value(after).unwrap());
        apply_patch_json(&mut mirror, &invert(&patch)).unwrap();
        assert_eq!(mirror, serde_json::to_value(before).unwrap());
        patch
            .iter()
            .map(|op| {
                let kind = match op {
                    PatchOp::Add { .. } => "add",
                    PatchOp::Remove { .. } => "remove",
                    PatchOp::Replace { .. } => "replace",
                    PatchOp::Move { .. } => "move",
                };
                format!("{kind} {}", op.path())
            })
            .collect()
    }

    /// A sequence's ops as they were before its markers were diffed by themselves: the JSON
    /// diff of the whole sequence but its tracks.
    fn whole_json_diff(path: &DocPath, before: &Sequence, after: &Sequence) -> Ops {
        let shallow = |s: &Sequence| {
            let mut s = Sequence {
                tracks: OrdMap::new(),
                ..s.clone()
            };
            s.format.background_fill = None;
            s
        };
        let mut out = Vec::new();
        diff_shallow(
            path,
            &shallow(before),
            &shallow(after),
            &["tracks"],
            &mut out,
        )
        .unwrap();
        out
    }

    /// Markers that come, go and change in one edit, alone and together with each of the
    /// sequence's other fields, listed before and after them: the same ops in the same order
    /// as the JSON diff of the whole sequence.
    #[test]
    fn a_sequences_markers_diff_as_part_of_its_json_form_would() {
        use cutweave_model::{InOut, Marker, MarkerId, SequenceFormat, SequenceId, Time, TrackId};
        let t = |n: i64| Time::from_flicks(n).unwrap();
        let id = SequenceId::from_u128(1);
        let path = DocPath::sequence(id);
        let mut before = Sequence::new(id, "Before", SequenceFormat::default());
        for n in 1..=6 {
            let m = MarkerId::from_u128(n);
            before
                .markers
                .insert(m, Marker::new(m, t(i64::try_from(n).unwrap())));
        }
        before.track_order.push_back(TrackId::from_u128(7));
        before.extra =
            serde_json::from_value(serde_json::json!({"later": 1, "gone": true})).unwrap();
        let edits: [fn(&mut Sequence); 9] = [
            |s| drop(s.markers.remove(&MarkerId::from_u128(2))),
            |s| s.markers.get_mut(&MarkerId::from_u128(3)).unwrap().label = "three".into(),
            |s| {
                let m = MarkerId::from_u128(9);
                s.markers.insert(m, Marker::new(m, Time::ZERO));
            },
            |s| {
                s.markers.get_mut(&MarkerId::from_u128(5)).unwrap().duration =
                    Some(cutweave_model::Duration::from_flicks(4).unwrap());
            },
            |s| s.name = "After".into(),
            |s| s.format.width += 2,
            |s| s.track_order.push_back(TrackId::from_u128(8)),
            |s| s.in_out = Some(InOut::new(Time::ZERO, Time::from_flicks(9).unwrap())),
            |s| {
                s.extra =
                    serde_json::from_value(serde_json::json!({"later": 2, "new": []})).unwrap();
            },
        ];
        for mask in 1_u32..(1 << edits.len()) {
            let mut after = before.clone();
            for (n, edit) in edits.iter().enumerate() {
                if mask & (1 << n) != 0 {
                    edit(&mut after);
                }
            }
            let mut ops = Vec::new();
            diff_sequence(&path, &before, &after, &mut ops).unwrap();
            assert_eq!(
                ops,
                whole_json_diff(&path, &before, &after),
                "edits {mask:#b}"
            );
        }
    }

    #[test]
    fn collections_are_never_left_present_and_empty() {
        let manual = CollectionKind::manual;
        let (a, b) = (CollectionId::from_u128(1), CollectionId::from_u128(2));
        let none = with(&[]);
        let one = with(&[(1, "A", manual())]);
        let other = with(&[(2, "B", manual())]);

        // The map appears and disappears whole (its key is absent while it is empty).
        assert_eq!(ops(&none, &one), ["add /collections"]);
        assert_eq!(ops(&one, &none), ["remove /collections"]);
        // Every entry replaced: the new ones are added before the old ones are removed.
        assert_eq!(
            ops(&one, &other),
            [
                format!("add /collections/{b}"),
                format!("remove /collections/{a}")
            ]
        );
        assert_eq!(
            ops(&other, &one),
            [
                format!("add /collections/{a}"),
                format!("remove /collections/{b}")
            ]
        );
        // An entry that stays is diffed field by field.
        let renamed = with(&[(1, "Best", manual())]);
        assert_eq!(
            ops(&one, &renamed),
            [format!("replace /collections/{a}/name")]
        );
    }

    #[test]
    fn a_collection_that_changes_kind_is_replaced_whole() {
        let a = CollectionId::from_u128(1);
        let manual = with(&[(1, "A", CollectionKind::manual())]);
        let smart = with(&[(
            1,
            "A",
            CollectionKind::Smart {
                filter: MediaFilter::default(),
            },
        )]);
        assert_eq!(ops(&manual, &smart), [format!("replace /collections/{a}")]);
        assert_eq!(ops(&smart, &manual), [format!("replace /collections/{a}")]);
    }
}
