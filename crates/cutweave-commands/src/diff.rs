// SPDX-License-Identifier: GPL-3.0-or-later
//! [`diff`]: the field-level patch between two versions of a document.
//!
//! The persistent collections make this cheap: unchanged media, sequences and tracks are
//! skipped by pointer without being visited (`imbl` diffs skip shared nodes), so the cost is
//! proportional to what changed, plus one pass over the items of each changed track.

use cutweave_model::{Item, MediaAsset, Project, Sequence, Track, TrackItems};
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
        sequences: OrdMap::new(),
        ..p.clone()
    };
    diff_shallow(
        &root,
        &shallow(before),
        &shallow(after),
        &["media", "sequences"],
        out,
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
