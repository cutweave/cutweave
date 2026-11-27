// SPDX-License-Identifier: GPL-3.0-or-later
//! [`diff`]: the field-level patch between two versions of a document.
//!
//! The persistent collections make this cheap: unchanged media, sequences and tracks are
//! skipped by pointer without being visited (`imbl` diffs skip shared nodes), so the cost is
//! proportional to what changed, plus one pass over the items of each changed track.

use cutweave_model::{Collection, Item, MediaAsset, Project, Sequence, Track, TrackItems};
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
    let shallow = |s: &Sequence| Sequence {
        tracks: OrdMap::new(),
        ..s.clone()
    };
    diff_shallow(path, &shallow(before), &shallow(after), &["tracks"], out)?;
    diff_keyed(
        &path.join("tracks"),
        &before.tracks,
        &after.tracks,
        out,
        diff_track,
    )
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
                        diff_value(&path.join(bk), *bv, *av, out)?;
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

fn add<T: Serialize + ?Sized>(path: &DocPath, value: &T, out: &mut Ops) -> Result<(), PatchError> {
    out.push(PatchOp::Add {
        path: path.clone(),
        value: to_json(value, path)?,
    });
    Ok(())
}

fn remove<T: Serialize + ?Sized>(
    path: &DocPath,
    value: &T,
    out: &mut Ops,
) -> Result<(), PatchError> {
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
