// SPDX-License-Identifier: GPL-3.0-or-later
//! Invertible patch ops on the ID-addressed project document (arch §5.2, ADR-0005).
//!
//! A [`PatchOp`] is RFC 6902 JSON Patch plus *preimages*: `remove` and `replace` carry the
//! value they expect to find (`old`), so every op inverts exactly ([`invert`]) and applying a
//! patch to a document that has changed since fails with [`PatchError::Mismatch`] instead of
//! silently clobbering the newer state. That check is what makes per-actor undo safe
//! (WP-1B.04): undoing actor A's edit after actor B touched the same field is a conflict, while
//! B's edits to other fields are left alone.
//!
//! Semantics, chosen so that inversion is exact (RFC 6902 differs where noted):
//!
//! - `add`: the parent must exist; an object key must be **absent** (RFC: replaced); an array
//!   index must be `0..=len` (RFC also accepts `-`).
//! - `remove` / `replace`: the target must exist and equal `old` (numbers compare by value).
//! - `move`: removes `from`, then adds at `path` resolved in the document *after* the removal
//!   (so it must then be absent). A move into its own subtree fails because the parent is gone.
//!   RFC 6902 also forbids any `path` that `from` textually prefixes, but with shifting array
//!   indices that would refuse the inverse of some valid moves.
//!
//! The same patch applies to the typed document ([`apply_patch`]) and to its JSON form
//! ([`apply_patch_json`], the UI mirror contract): whenever the typed application succeeds,
//! the JSON one succeeds too and yields exactly the serialized result.

use std::collections::BTreeSet;

use cutweave_model::{ItemId, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    apply,
    json::{self, Edit},
    path::{DocPath, EntityRef},
};

/// TypeScript shape of a patch value: any JSON value, with numbers as plain `number`s (specta
/// refuses `serde_json::Value`'s 64-bit integers). Only its type definition is used.
#[derive(Serialize, specta::Type)]
#[serde(untagged)]
#[allow(dead_code)]
enum PatchValue {
    Null,
    Bool(bool),
    Number(#[specta(type = specta_typescript::Number)] f64),
    String(String),
    Array(Vec<PatchValue>),
    Object(std::collections::BTreeMap<String, PatchValue>),
}

/// One invertible edit of the project document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum PatchOp {
    /// Adds `value` at `path` (an absent object key, or an array index `0..=len`).
    Add {
        /// Where.
        path: DocPath,
        /// What.
        #[specta(type = PatchValue)]
        value: Value,
    },
    /// Removes the value at `path`, which must equal `old`.
    Remove {
        /// Where.
        path: DocPath,
        /// The value being removed (the preimage).
        #[specta(type = PatchValue)]
        old: Value,
    },
    /// Replaces the value at `path`, which must equal `old`, with `value`.
    Replace {
        /// Where.
        path: DocPath,
        /// The value being replaced (the preimage).
        #[specta(type = PatchValue)]
        old: Value,
        /// The new value.
        #[specta(type = PatchValue)]
        value: Value,
    },
    /// Moves the value at `from` to `path` (e.g. an item to another track).
    Move {
        /// Source.
        from: DocPath,
        /// Destination (absent after the removal at `from`).
        path: DocPath,
    },
}

impl PatchOp {
    /// The inverse op: `apply(apply(doc, op), op.inverse()) == doc`.
    #[must_use]
    pub fn inverse(&self) -> PatchOp {
        match self.clone() {
            PatchOp::Add { path, value } => PatchOp::Remove { path, old: value },
            PatchOp::Remove { path, old } => PatchOp::Add { path, value: old },
            PatchOp::Replace { path, old, value } => PatchOp::Replace {
                path,
                old: value,
                value: old,
            },
            PatchOp::Move { from, path } => PatchOp::Move {
                from: path,
                path: from,
            },
        }
    }

    /// The destination path (`path` of every op).
    #[must_use]
    pub fn path(&self) -> &DocPath {
        match self {
            PatchOp::Add { path, .. }
            | PatchOp::Remove { path, .. }
            | PatchOp::Replace { path, .. }
            | PatchOp::Move { path, .. } => path,
        }
    }

    /// Every path the op reads or writes (`from` and `path` for a move).
    pub fn paths(&self) -> impl Iterator<Item = &DocPath> + '_ {
        let from = match self {
            PatchOp::Move { from, .. } => Some(from),
            _ => None,
        };
        from.into_iter().chain(std::iter::once(self.path()))
    }
}

/// An ordered list of [`PatchOp`]s, applied first to last. Serialized as a JSON array.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(transparent)]
pub struct Patch(pub Vec<PatchOp>);

impl Patch {
    /// No ops.
    #[must_use]
    pub const fn new() -> Patch {
        Patch(Vec::new())
    }

    /// The ops.
    #[must_use]
    pub fn ops(&self) -> &[PatchOp] {
        &self.0
    }

    /// Number of ops.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no ops.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterates over the ops.
    pub fn iter(&self) -> std::slice::Iter<'_, PatchOp> {
        self.0.iter()
    }

    /// Appends an op.
    pub fn push(&mut self, op: PatchOp) {
        self.0.push(op);
    }

    /// The inverse patch (see [`invert`]).
    #[must_use]
    pub fn invert(&self) -> Patch {
        invert(self)
    }

    /// Applies the patch to `doc` (see [`apply_patch`]).
    pub fn apply(&self, doc: &Project) -> Result<Project, PatchError> {
        apply_patch(doc, self)
    }

    /// Every entity the patch touches, in a stable order.
    #[must_use]
    pub fn touched(&self) -> BTreeSet<EntityRef> {
        self.0
            .iter()
            .flat_map(PatchOp::paths)
            .map(DocPath::entity)
            .collect()
    }

    /// Every item the patch touches (adds, removes, moves or edits).
    #[must_use]
    pub fn touched_items(&self) -> BTreeSet<ItemId> {
        self.touched()
            .into_iter()
            .filter_map(|e| match e {
                EntityRef::Item { item, .. } => Some(item),
                _ => None,
            })
            .collect()
    }
}

impl From<Vec<PatchOp>> for Patch {
    fn from(ops: Vec<PatchOp>) -> Patch {
        Patch(ops)
    }
}

impl FromIterator<PatchOp> for Patch {
    fn from_iter<I: IntoIterator<Item = PatchOp>>(iter: I) -> Patch {
        Patch(iter.into_iter().collect())
    }
}

impl IntoIterator for Patch {
    type Item = PatchOp;
    type IntoIter = std::vec::IntoIter<PatchOp>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Patch {
    type Item = &'a PatchOp;
    type IntoIter = std::slice::Iter<'a, PatchOp>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl Extend<PatchOp> for Patch {
    fn extend<I: IntoIterator<Item = PatchOp>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}

/// Why a patch op does not apply.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PatchError {
    /// The target (or its parent) does not exist.
    #[error("nothing at `{path}`")]
    NotFound {
        /// The path.
        path: DocPath,
    },
    /// `add` onto an existing object key.
    #[error("`{path}` already exists")]
    AlreadyExists {
        /// The path.
        path: DocPath,
    },
    /// The value at the target is not the op's `old` value: the document changed since the
    /// patch was recorded.
    #[error("`{path}` does not hold the expected value (the document changed)")]
    Mismatch {
        /// The path.
        path: DocPath,
    },
    /// The path cannot address anything: a segment that is not an ID of the right kind, an
    /// array index out of range or malformed, a scalar parent, removing the root, or moving a
    /// value into itself.
    #[error("invalid path `{path}`: {msg}")]
    InvalidPath {
        /// The path.
        path: DocPath,
        /// What is wrong.
        msg: String,
    },
    /// The result does not fit the document model (a wrong type, a missing required field, a
    /// key that differs from the entity's ID, a value that would not survive a round trip).
    #[error("`{path}` cannot hold that value: {msg}")]
    Type {
        /// The path.
        path: DocPath,
        /// What is wrong.
        msg: String,
    },
    /// An op that would change an entity's `id` (IDs are immutable; move or re-add instead).
    #[error("`{path}` is an entity id and cannot change")]
    Immutable {
        /// The path.
        path: DocPath,
    },
}

impl PatchError {
    /// The path the error is about.
    #[must_use]
    pub fn path(&self) -> &DocPath {
        match self {
            PatchError::NotFound { path }
            | PatchError::AlreadyExists { path }
            | PatchError::Mismatch { path }
            | PatchError::InvalidPath { path, .. }
            | PatchError::Type { path, .. }
            | PatchError::Immutable { path } => path,
        }
    }
}

/// The inverse of `patch`: the inverse ops in reverse order, so that
/// `apply_patch(&apply_patch(doc, p)?, &invert(p)) == Ok(doc)`.
#[must_use]
pub fn invert(patch: &Patch) -> Patch {
    patch.0.iter().rev().map(PatchOp::inverse).collect()
}

/// Applies `patch` to `doc` and returns the new document; `doc` is untouched (the persistent
/// collections make the copy O(1)). Fails on the first op that does not apply, leaving no
/// partial result.
pub fn apply_patch(doc: &Project, patch: &Patch) -> Result<Project, PatchError> {
    let mut next = doc.clone();
    for op in patch {
        apply::apply_op(&mut next, op)?;
    }
    Ok(next)
}

/// Applies `patch` to the JSON form of a document in place: what a mirror of the document
/// (the UI docStore) does with `doc.patched` events.
///
/// Atomic: if an op fails, the ops already applied are reverted and `doc` is unchanged.
pub fn apply_patch_json(doc: &mut Value, patch: &Patch) -> Result<(), PatchError> {
    for (i, op) in patch.0.iter().enumerate() {
        if let Err(e) = apply_op_json(doc, op) {
            for done in patch.0[..i].iter().rev() {
                // Each applied op's inverse applies by construction; a failure here would mean
                // a bug in the primitives, which the property tests rule out.
                let _ = apply_op_json(doc, &done.inverse());
            }
            return Err(e);
        }
    }
    Ok(())
}

fn apply_op_json(doc: &mut Value, op: &PatchOp) -> Result<(), PatchError> {
    match op {
        PatchOp::Add { path, value } => {
            json::edit(doc, &path.to_segments(), Edit::Add(value.clone()), path).map(drop)
        }
        PatchOp::Remove { path, old } => json::edit(
            doc,
            &path.to_segments(),
            Edit::Remove { expect: Some(old) },
            path,
        )
        .map(drop),
        PatchOp::Replace { path, old, value } => json::edit(
            doc,
            &path.to_segments(),
            Edit::Replace {
                value: value.clone(),
                expect: Some(old),
            },
            path,
        )
        .map(drop),
        PatchOp::Move { from, path } => {
            let value = json::edit(
                doc,
                &from.to_segments(),
                Edit::Remove { expect: None },
                from,
            )?
            .unwrap_or(Value::Null);
            if let Err(e) = json::edit(doc, &path.to_segments(), Edit::Add(value.clone()), path) {
                // Put it back; this re-adds exactly what was removed, so it applies.
                let _ = json::edit(doc, &from.to_segments(), Edit::Add(value), from);
                return Err(e);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn p(s: &str) -> DocPath {
        DocPath::parse(s).unwrap()
    }

    #[test]
    fn ops_serialize_as_json_patch_with_preimages() {
        let patch = Patch(vec![
            PatchOp::Add {
                path: p("/a"),
                value: json!(1),
            },
            PatchOp::Remove {
                path: p("/b"),
                old: json!("x"),
            },
            PatchOp::Replace {
                path: p("/c"),
                old: json!(1.0),
                value: json!(0.5),
            },
            PatchOp::Move {
                from: p("/d"),
                path: p("/e"),
            },
        ]);
        let v = serde_json::to_value(&patch).unwrap();
        assert_eq!(
            v,
            json!([
                {"op": "add", "path": "/a", "value": 1},
                {"op": "remove", "path": "/b", "old": "x"},
                {"op": "replace", "path": "/c", "old": 1.0, "value": 0.5},
                {"op": "move", "from": "/d", "path": "/e"}
            ])
        );
        assert_eq!(serde_json::from_value::<Patch>(v).unwrap(), patch);
        let bad = json!([{"op": "add", "path": "/a", "value": 1, "extra": true}]);
        assert!(serde_json::from_value::<Patch>(bad).is_err());
    }

    #[test]
    fn inversion_reverses_and_swaps() {
        let patch = Patch(vec![
            PatchOp::Add {
                path: p("/a"),
                value: json!(1),
            },
            PatchOp::Move {
                from: p("/a"),
                path: p("/b"),
            },
            PatchOp::Replace {
                path: p("/b"),
                old: json!(1),
                value: json!(2),
            },
        ]);
        let inv = invert(&patch);
        assert_eq!(
            inv.0,
            vec![
                PatchOp::Replace {
                    path: p("/b"),
                    old: json!(2),
                    value: json!(1)
                },
                PatchOp::Move {
                    from: p("/b"),
                    path: p("/a")
                },
                PatchOp::Remove {
                    path: p("/a"),
                    old: json!(1)
                },
            ]
        );
        assert_eq!(invert(&inv), patch);
    }

    #[test]
    fn json_application_is_strict_and_atomic() {
        let mut doc = json!({"a": {"b": [1, 2, 3]}, "c": 1});
        let original = doc.clone();
        let patch = Patch(vec![
            PatchOp::Add {
                path: p("/a/b/0"),
                value: json!(0),
            },
            PatchOp::Replace {
                path: p("/c"),
                old: json!(1.0),
                value: json!(2),
            },
            PatchOp::Move {
                from: p("/a/b/3"),
                path: p("/d"),
            },
        ]);
        apply_patch_json(&mut doc, &patch).unwrap();
        assert_eq!(doc, json!({"a": {"b": [0, 1, 2]}, "c": 2, "d": 3}));
        apply_patch_json(&mut doc, &invert(&patch)).unwrap();
        // The preimage `1.0` matched `1` by value, and is what the inverse restores.
        assert!(crate::json_eq(&doc, &original));
    }

    #[test]
    fn json_application_refuses_invalid_ops_without_a_trace() {
        let mut doc = json!({"a": {"b": [1, 2, 3]}, "c": 1});
        let original = doc.clone();
        // A stale preimage fails and leaves the document as it was.
        let stale = Patch(vec![
            PatchOp::Add {
                path: p("/x"),
                value: json!(true),
            },
            PatchOp::Replace {
                path: p("/c"),
                old: json!(5),
                value: json!(6),
            },
        ]);
        assert_eq!(
            apply_patch_json(&mut doc, &stale),
            Err(PatchError::Mismatch { path: p("/c") })
        );
        assert_eq!(doc, original);

        let cases = [
            (
                PatchOp::Add {
                    path: p("/c"),
                    value: json!(0),
                },
                "already exists",
            ),
            (
                PatchOp::Add {
                    path: p("/a/b/-"),
                    value: json!(0),
                },
                "invalid path",
            ),
            (
                PatchOp::Add {
                    path: p("/a/b/4"),
                    value: json!(0),
                },
                "invalid path",
            ),
            (
                PatchOp::Add {
                    path: p("/a/b/01"),
                    value: json!(0),
                },
                "invalid path",
            ),
            (
                PatchOp::Remove {
                    path: p("/zz"),
                    old: json!(0),
                },
                "nothing at",
            ),
            (
                PatchOp::Add {
                    path: p("/c/x"),
                    value: json!(0),
                },
                "invalid path",
            ),
            (
                PatchOp::Remove {
                    path: p(""),
                    old: original.clone(),
                },
                "invalid path",
            ),
            (
                // Into its own subtree: the parent is gone after the removal.
                PatchOp::Move {
                    from: p("/a"),
                    path: p("/a/b/0"),
                },
                "nothing at",
            ),
        ];
        for (op, expected) in cases {
            let err = apply_patch_json(&mut doc, &Patch(vec![op.clone()])).unwrap_err();
            assert!(err.to_string().contains(expected), "{op:?}: {err}");
            assert_eq!(doc, original);
        }
    }

    #[test]
    fn root_replace_swaps_the_document() {
        let mut doc = json!({"a": 1});
        let patch = Patch(vec![PatchOp::Replace {
            path: DocPath::root(),
            old: json!({"a": 1}),
            value: json!([1]),
        }]);
        apply_patch_json(&mut doc, &patch).unwrap();
        assert_eq!(doc, json!([1]));
    }
}
