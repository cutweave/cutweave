// SPDX-License-Identifier: GPL-3.0-or-later
//! Patch primitives on the typed, `imbl`-backed [`Project`].
//!
//! Paths are resolved through the persistent collections (`media`, `sequences`, `tracks`,
//! `items`) by ID, so an edit copies only the path from the root to the touched entity and
//! every other entity stays shared with older snapshots (ADR-0005). At an entity, the edit
//! falls back to the entity's JSON form: the entity (without its nested collections) is
//! serialized, edited with the JSON primitives and read back. Reading back requires an exact
//! round trip, so whenever a typed edit succeeds the same edit on the document's JSON form
//! yields exactly the serialized result (the UI mirror contract).

use std::str::FromStr;

use cutweave_model::{
    EntityId, Item, ItemId, Marker, MarkerId, Markers, MediaAsset, MediaId, Project, Sequence,
    SequenceId, Time, Track, TrackId, TrackItems,
};
use imbl::OrdMap;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    effort,
    json::{self, Edit, json_eq},
    patch::{PatchError, PatchOp},
    path::DocPath,
};

/// An entity stored in an ID-keyed map.
trait Keyed: Clone + Serialize + DeserializeOwned {
    type Key: EntityId + Serialize + DeserializeOwned;
    fn key(&self) -> Self::Key;
}

impl Keyed for MediaAsset {
    type Key = MediaId;
    fn key(&self) -> MediaId {
        self.id
    }
}

impl Keyed for Sequence {
    type Key = SequenceId;
    fn key(&self) -> SequenceId {
        self.id
    }
}

impl Keyed for Track {
    type Key = TrackId;
    fn key(&self) -> TrackId {
        self.id
    }
}

impl Keyed for Item {
    type Key = ItemId;
    fn key(&self) -> ItemId {
        self.id()
    }
}

type Out = Result<Option<Value>, PatchError>;

fn type_error(path: &DocPath, msg: impl Into<String>) -> PatchError {
    PatchError::Type {
        path: path.clone(),
        msg: msg.into(),
    }
}

fn not_found(path: &DocPath) -> PatchError {
    PatchError::NotFound { path: path.clone() }
}

pub(crate) fn to_json<T: Serialize + ?Sized>(
    value: &T,
    path: &DocPath,
) -> Result<Value, PatchError> {
    effort::converted();
    serde_json::to_value(value).map_err(|e| type_error(path, e.to_string()))
}

/// Reads `value` as `T`, requiring that `T` serializes back to the same JSON.
fn from_json<T: Serialize + DeserializeOwned>(
    value: &Value,
    path: &DocPath,
) -> Result<T, PatchError> {
    effort::converted();
    let typed = T::deserialize(value).map_err(|e| type_error(path, e.to_string()))?;
    if !json_eq(&to_json(&typed, path)?, value) {
        return Err(type_error(
            path,
            "the document model cannot hold this value exactly (e.g. a removed optional field, \
             which reads back as null)",
        ));
    }
    Ok(typed)
}

fn check_expect(current: &Value, expect: Option<&Value>, path: &DocPath) -> Result<(), PatchError> {
    match expect {
        Some(expected) if !json_eq(current, expected) => {
            Err(PatchError::Mismatch { path: path.clone() })
        }
        _ => Ok(()),
    }
}

fn parse_key<K: EntityId>(segment: &str, path: &DocPath) -> Result<K, PatchError> {
    K::from_str(segment).map_err(|e| PatchError::InvalidPath {
        path: path.clone(),
        msg: e.to_string(),
    })
}

fn check_key<V: Keyed>(key: V::Key, value: &V, path: &DocPath) -> Result<(), PatchError> {
    if value.key() == key {
        Ok(())
    } else {
        Err(type_error(
            path,
            format!(
                "the entity's id `{}` differs from its key `{key}`",
                value.key()
            ),
        ))
    }
}

fn immutable(path: &DocPath) -> PatchError {
    PatchError::Immutable { path: path.clone() }
}

/// Applies one op to `doc` in place. On error `doc` may hold a partial copy-on-write path but
/// no changed value; callers that need atomicity apply to a clone (O(1)).
pub(crate) fn apply_op(doc: &mut Project, op: &PatchOp) -> Result<(), PatchError> {
    match op {
        PatchOp::Add { path, value } => edit(doc, path, Edit::Add(value.clone())).map(drop),
        PatchOp::Remove { path, old } => {
            edit(doc, path, Edit::Remove { expect: Some(old) }).map(drop)
        }
        PatchOp::Replace { path, old, value } => edit(
            doc,
            path,
            Edit::Replace {
                value: value.clone(),
                expect: Some(old),
            },
        )
        .map(drop),
        PatchOp::Move { from, path } => {
            let value = edit(doc, from, Edit::Remove { expect: None })?.unwrap_or(Value::Null);
            if let Err(e) = edit(doc, path, Edit::Add(value.clone())) {
                // Put it back; this re-adds exactly what was removed, so it applies.
                let _ = edit(doc, from, Edit::Add(value));
                return Err(e);
            }
            Ok(())
        }
    }
}

/// Applies one primitive edit at `path`; returns the removed or replaced value.
pub(crate) fn edit(doc: &mut Project, path: &DocPath, edit: Edit<'_>) -> Out {
    edit_project(doc, &path.to_segments(), edit, path)
}

fn edit_project(doc: &mut Project, segments: &[String], edit: Edit<'_>, path: &DocPath) -> Out {
    match segments.split_first() {
        None => match edit {
            Edit::Add(_) => Err(PatchError::AlreadyExists { path: path.clone() }),
            Edit::Remove { .. } => Err(PatchError::InvalidPath {
                path: path.clone(),
                msg: "cannot remove the document root".into(),
            }),
            Edit::Replace { value, expect } => {
                let old = to_json(&*doc, path)?;
                check_expect(&old, expect, path)?;
                *doc = from_json(&value, path)?;
                Ok(Some(old))
            }
        },
        Some((head, rest)) if head == "media" => {
            edit_keyed(&mut doc.media, rest, edit, path, edit_leaf::<MediaAsset>)
        }
        Some((head, rest)) if head == "sequences" => {
            edit_keyed(&mut doc.sequences, rest, edit, path, edit_sequence)
        }
        Some(_) => edit_shallow(
            doc,
            segments,
            edit,
            path,
            &["media", "sequences"],
            |d| {
                (
                    std::mem::take(&mut d.media),
                    std::mem::take(&mut d.sequences),
                )
            },
            |d, (media, sequences)| {
                d.media = media;
                d.sequences = sequences;
            },
        ),
    }
}

/// Edits an ID-keyed map of entities: the whole map, one entity, or (through `child`) a path
/// inside one entity.
fn edit_keyed<V: Keyed>(
    map: &mut OrdMap<V::Key, V>,
    segments: &[String],
    edit: Edit<'_>,
    path: &DocPath,
    child: fn(&mut V, &[String], Edit<'_>, &DocPath) -> Out,
) -> Out {
    let Some((key, rest)) = segments.split_first() else {
        return match edit {
            Edit::Add(_) => Err(PatchError::AlreadyExists { path: path.clone() }),
            Edit::Remove { .. } => Err(type_error(path, "a required collection cannot be removed")),
            Edit::Replace { value, expect } => {
                let old = to_json(&*map, path)?;
                check_expect(&old, expect, path)?;
                let new: OrdMap<V::Key, V> = from_json(&value, path)?;
                for (key, entity) in &new {
                    check_key(*key, entity, path)?;
                }
                *map = new;
                Ok(Some(old))
            }
        };
    };
    let id: V::Key = parse_key(key, path)?;
    if !rest.is_empty() {
        let entity = map.get_mut(&id).ok_or_else(|| not_found(path))?;
        return child(entity, rest, edit, path);
    }
    match edit {
        Edit::Add(value) => {
            if map.contains_key(&id) {
                return Err(PatchError::AlreadyExists { path: path.clone() });
            }
            let entity: V = from_json(&value, path)?;
            check_key(id, &entity, path)?;
            map.insert(id, entity);
            Ok(None)
        }
        Edit::Remove { expect } => {
            let old = to_json(map.get(&id).ok_or_else(|| not_found(path))?, path)?;
            check_expect(&old, expect, path)?;
            map.remove(&id);
            Ok(Some(old))
        }
        Edit::Replace { value, expect } => {
            let old = to_json(map.get(&id).ok_or_else(|| not_found(path))?, path)?;
            check_expect(&old, expect, path)?;
            let entity: V = from_json(&value, path)?;
            check_key(id, &entity, path)?;
            map.insert(id, entity);
            Ok(Some(old))
        }
    }
}

/// Edits a path inside an entity with no nested collections (media, items) via its JSON form.
fn edit_leaf<V: Keyed>(entity: &mut V, rest: &[String], edit: Edit<'_>, path: &DocPath) -> Out {
    if rest.first().is_some_and(|s| s == "id") {
        return Err(immutable(path));
    }
    let mut value = to_json(&*entity, path)?;
    let out = json::edit(&mut value, rest, edit, path)?;
    *entity = from_json(&value, path)?;
    Ok(out)
}

fn edit_sequence(seq: &mut Sequence, rest: &[String], edit: Edit<'_>, path: &DocPath) -> Out {
    match rest.split_first() {
        Some((head, tail)) if head == "tracks" => {
            edit_keyed(&mut seq.tracks, tail, edit, path, edit_track)
        }
        Some((head, _)) if head == "id" => Err(immutable(path)),
        Some((head, [key, ..])) if head == "markers" && MarkerId::from_str(key).is_ok() => {
            edit_marker(&mut seq.markers, &rest[1..], edit, path)
        }
        _ => edit_shallow(
            seq,
            rest,
            edit,
            path,
            &["tracks"],
            |s| std::mem::take(&mut s.tracks),
            |s, tracks| s.tracks = tracks,
        ),
    }
}

/// Edits one marker of a sequence (`…/markers/<id>` and below; `segments` starts at the ID)
/// through the JSON form of that marker alone. The general path puts the whole sequence, every
/// marker with it, into its JSON form and reads it back for each such op (issue #230); what it
/// does to this marker, and what it refuses, is the same: the same JSON edit, the same exact
/// read-back, on the one member the op names.
fn edit_marker(markers: &mut Markers, segments: &[String], edit: Edit<'_>, path: &DocPath) -> Out {
    let key = &segments[0];
    let id = parse_key::<MarkerId>(key, path)?;
    let mut one = serde_json::Map::new();
    if let Some(marker) = markers.get(&id) {
        one.insert(key.clone(), to_json(marker, path)?);
    }
    let mut one = Value::Object(one);
    let out = json::edit(&mut one, segments, edit, path)?;
    match one.get(key) {
        Some(value) => {
            let marker: Marker = from_json(value, path)?;
            markers.insert(id, marker);
        }
        None => {
            markers.remove(&id);
        }
    }
    Ok(out)
}

fn edit_track(track: &mut Track, rest: &[String], edit: Edit<'_>, path: &DocPath) -> Out {
    match rest.split_first() {
        Some((head, tail)) if head == "items" => edit_items(&mut track.items, tail, edit, path),
        Some((head, _)) if head == "id" => Err(immutable(path)),
        _ => edit_shallow(
            track,
            rest,
            edit,
            path,
            &["items"],
            |t| std::mem::take(&mut t.items),
            |t, items| t.items = items,
        ),
    }
}

fn edit_items(items: &mut TrackItems, segments: &[String], edit: Edit<'_>, path: &DocPath) -> Out {
    let Some((key, rest)) = segments.split_first() else {
        return match edit {
            Edit::Add(_) => Err(PatchError::AlreadyExists { path: path.clone() }),
            Edit::Remove { .. } => Err(type_error(path, "a required collection cannot be removed")),
            Edit::Replace { value, expect } => {
                let old = to_json(&*items, path)?;
                check_expect(&old, expect, path)?;
                let new: TrackItems = from_json(&value, path)?;
                for (key, item) in new.iter_by_id() {
                    check_key(*key, item, path)?;
                }
                *items = new;
                Ok(Some(old))
            }
        };
    };
    let id: ItemId = parse_key(key, path)?;
    if !rest.is_empty() {
        if rest[0] == "id" {
            return Err(immutable(path));
        }
        if let ([field], Edit::Replace { value, expect }) = (rest, &edit)
            && field == "start"
            && let Some(old) = replace_start(items, &id, value, *expect)
        {
            return Ok(Some(old));
        }
        let mut value = to_json(items.get(&id).ok_or_else(|| not_found(path))?, path)?;
        let out = json::edit(&mut value, rest, edit, path)?;
        let item: Item = from_json(&value, path)?;
        check_key(id, &item, path)?;
        // `insert` replaces the item and keeps the start-time index in step.
        items.insert(item);
        return Ok(out);
    }
    match edit {
        Edit::Add(value) => {
            if items.contains(&id) {
                return Err(PatchError::AlreadyExists { path: path.clone() });
            }
            let item: Item = from_json(&value, path)?;
            check_key(id, &item, path)?;
            items.insert(item);
            Ok(None)
        }
        Edit::Remove { expect } => {
            let old = to_json(items.get(&id).ok_or_else(|| not_found(path))?, path)?;
            check_expect(&old, expect, path)?;
            items.remove(&id);
            Ok(Some(old))
        }
        Edit::Replace { value, expect } => {
            let old = to_json(items.get(&id).ok_or_else(|| not_found(path))?, path)?;
            check_expect(&old, expect, path)?;
            let item: Item = from_json(&value, path)?;
            check_key(id, &item, path)?;
            items.insert(item);
            Ok(Some(old))
        }
    }
}

/// `replace …/items/<id>/start` with a JSON integer of flicks: the op a ripple applies to every
/// item it shifts (issue #231). Sets the start on the typed item, which is what the general
/// path arrives at through the item's JSON form (serialize, replace the member, read back and
/// check the round trip). Returns the replaced value; `None`, and nothing changed, for anything
/// but the plain case (no such item, a preimage that does not match, a value that is not an
/// integer in range): the general path then decides and words the error.
fn replace_start(
    items: &mut TrackItems,
    id: &ItemId,
    value: &Value,
    expect: Option<&Value>,
) -> Option<Value> {
    let start = Time::from_flicks(value.as_i64()?).ok()?;
    let old = Value::from(items.get(id)?.start().flicks());
    if expect.is_some_and(|expected| !json_eq(&old, expected)) {
        return None;
    }
    // Debug builds (every test run) take the general path as well and compare.
    #[cfg(debug_assertions)]
    let general = effort::uncounted(|| replace_start_as_json(items.get(id)?, value, expect))?;
    items.update(id, |item| item.set_start(start));
    #[cfg(debug_assertions)]
    debug_assert_eq!(
        (items.get(id), &old),
        (Some(&general.0), &general.1),
        "the JSON form of a start replaced"
    );
    Some(old)
}

/// What the general path makes of `replace …/start` on `item`: the item after it and the
/// value it replaced.
#[cfg(debug_assertions)]
fn replace_start_as_json(
    item: &Item,
    value: &Value,
    expect: Option<&Value>,
) -> Option<(Item, Value)> {
    let path = DocPath::root();
    let mut json = to_json(item, &path).ok()?;
    let edit = Edit::Replace {
        value: value.clone(),
        expect,
    };
    let old = json::edit(&mut json, &["start".to_owned()], edit, &path).ok()??;
    Some((from_json(&json, &path).ok()?, old))
}

/// Edits a field of `entity` outside its nested collections: `take` swaps empty collections in
/// (and returns the real ones), the entity is edited as JSON with those collections hidden,
/// read back, and the real collections are put back. Shares the collections untouched.
fn edit_shallow<T, C>(
    entity: &mut T,
    segments: &[String],
    edit: Edit<'_>,
    path: &DocPath,
    nested: &[&str],
    take: impl Fn(&mut T) -> C,
    put: impl Fn(&mut T, C),
) -> Out
where
    T: Serialize + DeserializeOwned,
{
    let saved = take(entity);
    match edit_shallow_json(entity, segments, edit, path, nested) {
        Ok((mut new, out)) => {
            put(&mut new, saved);
            *entity = new;
            Ok(out)
        }
        Err(e) => {
            put(entity, saved);
            Err(e)
        }
    }
}

fn edit_shallow_json<T: Serialize + DeserializeOwned>(
    entity: &T,
    segments: &[String],
    edit: Edit<'_>,
    path: &DocPath,
    nested: &[&str],
) -> Result<(T, Option<Value>), PatchError> {
    let mut value = to_json(entity, path)?;
    let hidden = hide(&mut value, nested, path)?;
    let out = json::edit(&mut value, segments, edit, path)?;
    let Value::Object(map) = &mut value else {
        return Err(type_error(path, "expected an object"));
    };
    for (key, empty) in hidden {
        map.insert(key, empty);
    }
    Ok((from_json(&value, path)?, out))
}

/// Removes the (empty) nested-collection keys from an entity's JSON, returning them.
fn hide(
    value: &mut Value,
    nested: &[&str],
    path: &DocPath,
) -> Result<Vec<(String, Value)>, PatchError> {
    let Value::Object(map) = value else {
        return Err(type_error(path, "expected an object"));
    };
    Ok(nested
        .iter()
        .filter_map(|key| map.remove(*key).map(|v| ((*key).to_owned(), v)))
        .collect())
}

/// The value at `path`, read without copying any shared structure.
pub(crate) fn get(doc: &Project, path: &DocPath) -> Result<Value, PatchError> {
    let segments = path.to_segments();
    match segments.split_first() {
        None => to_json(doc, path),
        Some((head, rest)) if head == "media" => get_keyed(&doc.media, rest, path, get_leaf),
        Some((head, rest)) if head == "sequences" => {
            get_keyed(&doc.sequences, rest, path, get_sequence)
        }
        Some(_) => {
            let shallow = Project {
                media: OrdMap::new(),
                sequences: OrdMap::new(),
                ..doc.clone()
            };
            get_shallow(&shallow, &segments, path, &["media", "sequences"])
        }
    }
}

fn get_keyed<V: Keyed>(
    map: &OrdMap<V::Key, V>,
    segments: &[String],
    path: &DocPath,
    child: fn(&V, &[String], &DocPath) -> Result<Value, PatchError>,
) -> Result<Value, PatchError> {
    let Some((key, rest)) = segments.split_first() else {
        return to_json(map, path);
    };
    let id = V::Key::from_str(key).map_err(|_| not_found(path))?;
    let entity = map.get(&id).ok_or_else(|| not_found(path))?;
    if rest.is_empty() {
        to_json(entity, path)
    } else {
        child(entity, rest, path)
    }
}

fn get_leaf<V: Serialize>(
    entity: &V,
    rest: &[String],
    path: &DocPath,
) -> Result<Value, PatchError> {
    json::get(&to_json(entity, path)?, rest)
        .cloned()
        .ok_or_else(|| not_found(path))
}

fn get_shallow<T: Serialize>(
    entity: &T,
    segments: &[String],
    path: &DocPath,
    nested: &[&str],
) -> Result<Value, PatchError> {
    let mut value = to_json(entity, path)?;
    hide(&mut value, nested, path)?;
    json::get(&value, segments)
        .cloned()
        .ok_or_else(|| not_found(path))
}

fn get_sequence(seq: &Sequence, rest: &[String], path: &DocPath) -> Result<Value, PatchError> {
    match rest.split_first() {
        Some((head, tail)) if head == "tracks" => get_keyed(&seq.tracks, tail, path, get_track),
        _ => {
            let shallow = Sequence {
                tracks: OrdMap::new(),
                ..seq.clone()
            };
            get_shallow(&shallow, rest, path, &["tracks"])
        }
    }
}

fn get_track(track: &Track, rest: &[String], path: &DocPath) -> Result<Value, PatchError> {
    match rest.split_first() {
        Some((head, tail)) if head == "items" => {
            let Some((key, rest)) = tail.split_first() else {
                return to_json(&track.items, path);
            };
            let id = ItemId::from_str(key).map_err(|_| not_found(path))?;
            let item = track.items.get(&id).ok_or_else(|| not_found(path))?;
            if rest.is_empty() {
                to_json(item, path)
            } else {
                get_leaf(item, rest, path)
            }
        }
        _ => {
            let shallow = Track {
                items: TrackItems::new(),
                ..track.clone()
            };
            get_shallow(&shallow, rest, path, &["items"])
        }
    }
}
