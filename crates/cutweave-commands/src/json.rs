// SPDX-License-Identifier: GPL-3.0-or-later
//! Patch primitives on `serde_json::Value` (the JSON form of the document, and the leaves of the
//! typed document), value equality and JSON diffing.

use serde_json::{Map, Number, Value};

use crate::{
    patch::{PatchError, PatchOp},
    path::DocPath,
};

/// One primitive edit at a path. `expect` is the preimage check of `remove` / `replace`.
#[derive(Debug)]
pub(crate) enum Edit<'a> {
    /// Add at an absent key or array index `0..=len`.
    Add(Value),
    /// Remove the present value (equal to `expect` if given).
    Remove { expect: Option<&'a Value> },
    /// Replace the present value (equal to `expect` if given).
    Replace {
        value: Value,
        expect: Option<&'a Value>,
    },
}

/// Deep equality where numbers compare by value (`1 == 1.0`), so a preimage parsed from JSON
/// text matches the serialized model no matter how the number was spelled.
#[must_use]
pub fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => number_eq(x, y),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| json_eq(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
        }
        _ => a == b,
    }
}

fn number_eq(x: &Number, y: &Number) -> bool {
    if let (Some(x), Some(y)) = (x.as_i64(), y.as_i64()) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (x.as_u64(), y.as_u64()) {
        return x == y;
    }
    match (x.as_f64(), y.as_f64()) {
        // Exact comparison is intended: both sides come from the same finite numbers.
        #[allow(clippy::float_cmp)]
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Parses an RFC 6901 array index: `0` or digits without a leading zero. `-` (append) is not
/// accepted: recorded patches always name concrete indices, so they invert exactly.
fn parse_index(segment: &str, path: &DocPath) -> Result<usize, PatchError> {
    let invalid = |msg: &str| PatchError::InvalidPath {
        path: path.clone(),
        msg: msg.to_owned(),
    };
    if segment == "-" {
        return Err(invalid("`-` is not accepted; name a concrete array index"));
    }
    let digits = !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit());
    if !digits || (segment.len() > 1 && segment.starts_with('0')) {
        return Err(invalid("array index must be a non-negative integer"));
    }
    segment
        .parse()
        .map_err(|_| invalid("array index is out of range"))
}

/// The value at `segments`, if any.
pub(crate) fn get<'a>(root: &'a Value, segments: &[String]) -> Option<&'a Value> {
    let mut cur = root;
    for segment in segments {
        cur = match cur {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => {
                let i = parse_index(segment, &DocPath::root()).ok()?;
                items.get(i)?
            }
            _ => return None,
        };
    }
    Some(cur)
}

fn check_expect(current: &Value, expect: Option<&Value>, path: &DocPath) -> Result<(), PatchError> {
    match expect {
        Some(expected) if !json_eq(current, expected) => {
            Err(PatchError::Mismatch { path: path.clone() })
        }
        _ => Ok(()),
    }
}

/// Applies `edit` at `segments` (the unescaped segments of `path`, which is used for errors).
/// Returns the removed or replaced value (`None` for `Add`). Leaves `root` unchanged on error.
pub(crate) fn edit(
    root: &mut Value,
    segments: &[String],
    edit: Edit<'_>,
    path: &DocPath,
) -> Result<Option<Value>, PatchError> {
    let Some((last, parents)) = segments.split_last() else {
        return match edit {
            Edit::Add(_) => Err(PatchError::AlreadyExists { path: path.clone() }),
            Edit::Remove { .. } => Err(PatchError::InvalidPath {
                path: path.clone(),
                msg: "cannot remove the document root".into(),
            }),
            Edit::Replace { value, expect } => {
                check_expect(root, expect, path)?;
                Ok(Some(std::mem::replace(root, value)))
            }
        };
    };
    let mut parent = root;
    for segment in parents {
        parent = match parent {
            Value::Object(map) => map
                .get_mut(segment)
                .ok_or_else(|| PatchError::NotFound { path: path.clone() })?,
            Value::Array(items) => {
                let i = parse_index(segment, path)?;
                items
                    .get_mut(i)
                    .ok_or_else(|| PatchError::NotFound { path: path.clone() })?
            }
            _ => {
                return Err(PatchError::InvalidPath {
                    path: path.clone(),
                    msg: "a parent is not an object or array".into(),
                });
            }
        };
    }
    match parent {
        Value::Object(map) => edit_object(map, last, edit, path),
        Value::Array(items) => edit_array(items, last, edit, path),
        _ => Err(PatchError::InvalidPath {
            path: path.clone(),
            msg: "the parent is not an object or array".into(),
        }),
    }
}

fn edit_object(
    map: &mut Map<String, Value>,
    key: &str,
    edit: Edit<'_>,
    path: &DocPath,
) -> Result<Option<Value>, PatchError> {
    match edit {
        Edit::Add(value) => {
            if map.contains_key(key) {
                return Err(PatchError::AlreadyExists { path: path.clone() });
            }
            map.insert(key.to_owned(), value);
            Ok(None)
        }
        Edit::Remove { expect } => {
            let current = map
                .get(key)
                .ok_or_else(|| PatchError::NotFound { path: path.clone() })?;
            check_expect(current, expect, path)?;
            Ok(map.remove(key))
        }
        Edit::Replace { value, expect } => {
            let current = map
                .get_mut(key)
                .ok_or_else(|| PatchError::NotFound { path: path.clone() })?;
            check_expect(current, expect, path)?;
            Ok(Some(std::mem::replace(current, value)))
        }
    }
}

fn edit_array(
    items: &mut Vec<Value>,
    segment: &str,
    edit: Edit<'_>,
    path: &DocPath,
) -> Result<Option<Value>, PatchError> {
    let i = parse_index(segment, path)?;
    match edit {
        Edit::Add(value) => {
            if i > items.len() {
                return Err(PatchError::InvalidPath {
                    path: path.clone(),
                    msg: format!("index {i} is past the end of an array of {}", items.len()),
                });
            }
            items.insert(i, value);
            Ok(None)
        }
        Edit::Remove { expect } => {
            let current = items
                .get(i)
                .ok_or_else(|| PatchError::NotFound { path: path.clone() })?;
            check_expect(current, expect, path)?;
            Ok(Some(items.remove(i)))
        }
        Edit::Replace { value, expect } => {
            let current = items
                .get_mut(i)
                .ok_or_else(|| PatchError::NotFound { path: path.clone() })?;
            check_expect(current, expect, path)?;
            Ok(Some(std::mem::replace(current, value)))
        }
    }
}

/// Appends the ops that turn `before` into `after` (both at `path`) to `out`: objects are
/// diffed key by key (removals, then changes, then additions, each in key order); arrays and
/// scalars that differ are replaced whole. So is an animatable that becomes keyed (an object with
/// a `keyframes` member) or stops being keyed: the static `{x, y}` of a vector and its keyed form
/// share no member, and their halfway states are not valid values (keyframes §9.1). So is a
/// fraction `{num, den}` (a frame rate, a clip speed): it is kept in lowest terms, so a halfway
/// `{2, 2}` would read back as `{1, 1}`.
/// Whether two objects at `path` are different variants of a tagged union whose variants share
/// no fields, so that their halfway states are not valid values (a text item's style, `source`
/// `named` or `inline`, and a text fill, `kind` `solid`, `linear` or `radial`, WP-3A.01): such
/// a value is replaced whole. Other `kind` fields (a marker's, a media asset's) are still
/// diffed field by field.
fn tag_differs(
    path: &DocPath,
    before: &serde_json::Map<String, Value>,
    after: &serde_json::Map<String, Value>,
) -> bool {
    let key = match path.split_last() {
        Some((_, last)) if last == "style" => "source",
        Some((_, last)) if last == "fill" => "kind",
        _ => return false,
    };
    let tag =
        |m: &serde_json::Map<String, Value>| m.get(key).and_then(Value::as_str).map(str::to_owned);
    let (b, a) = (tag(before), tag(after));
    b.is_some() && a.is_some() && b != a
}

pub(crate) fn diff(path: &DocPath, before: &Value, after: &Value, out: &mut Vec<PatchOp>) {
    if json_eq(before, after) {
        return;
    }
    let fraction = |v: &serde_json::Map<String, Value>| {
        v.len() == 2 && v.contains_key("num") && v.contains_key("den")
    };
    if let (Value::Object(b), Value::Object(a)) = (before, after)
        && b.contains_key("keyframes") == a.contains_key("keyframes")
        && !(fraction(b) && fraction(a))
        && !tag_differs(path, b, a)
    {
        for (key, old) in b {
            if !a.contains_key(key) {
                out.push(PatchOp::Remove {
                    path: path.join(key),
                    old: old.clone(),
                });
            }
        }
        for (key, old) in b {
            if let Some(new) = a.get(key) {
                diff(&path.join(key), old, new, out);
            }
        }
        for (key, new) in a {
            if !b.contains_key(key) {
                out.push(PatchOp::Add {
                    path: path.join(key),
                    value: new.clone(),
                });
            }
        }
        return;
    }
    out.push(PatchOp::Replace {
        path: path.clone(),
        old: before.clone(),
        value: after.clone(),
    });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::patch::{Patch, apply_patch_json, invert};

    #[test]
    fn a_fraction_is_replaced_whole() {
        let mut out = Vec::new();
        let before = json!({"speed": {"num": 2, "den": 1}, "x": 1});
        let after = json!({"speed": {"num": 1, "den": 2}, "x": 1});
        diff(&DocPath::root(), &before, &after, &mut out);
        assert_eq!(out.len(), 1);
        assert!(
            matches!(&out[0], PatchOp::Replace { path, .. } if path.to_string() == "/speed"),
            "{out:?}"
        );
    }

    #[test]
    fn a_text_style_and_fill_that_change_variant_are_replaced_whole() {
        let mut out = Vec::new();
        let before = json!({"style": {"source": "named", "name": "title"}});
        let after = json!({"style": {"source": "inline", "style": {"size": 4}}});
        diff(&DocPath::root(), &before, &after, &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].path().to_string(), "/style");
        // The same variant is diffed field by field, and so is another `kind` field.
        let mut out = Vec::new();
        let (b, a) = (
            json!({"fill": {"kind": "solid", "color": "#000000"}}),
            json!({"fill": {"kind": "solid", "color": "#ffffff"}}),
        );
        diff(&DocPath::root(), &b, &a, &mut out);
        assert_eq!(out[0].path().to_string(), "/fill/color");
        let mut out = Vec::new();
        let (b, a) = (
            json!({"fill": {"kind": "solid", "color": "#000000"}}),
            json!({"fill": {"kind": "linear", "angle": 0, "stops": []}}),
        );
        diff(&DocPath::root(), &b, &a, &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        let mut out = Vec::new();
        let (b, a) = (
            json!({"marker": {"kind": "note"}}),
            json!({"marker": {"kind": "chapter"}}),
        );
        diff(&DocPath::root(), &b, &a, &mut out);
        assert_eq!(out[0].path().to_string(), "/marker/kind");
    }

    #[test]
    fn numbers_compare_by_value() {
        assert!(json_eq(&json!(1), &json!(1.0)));
        assert!(json_eq(&json!({"a": [1, 2.5]}), &json!({"a": [1.0, 2.5]})));
        assert!(!json_eq(&json!(1), &json!(2)));
        assert!(!json_eq(&json!(-1), &json!(u64::MAX)));
        assert!(json_eq(&json!(u64::MAX), &json!(u64::MAX)));
        assert!(!json_eq(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
        assert!(!json_eq(&json!({"a": 1}), &json!({"b": 1})));
        assert!(!json_eq(&json!("1"), &json!(1)));
    }

    #[test]
    fn diff_produces_minimal_invertible_ops() {
        let before = json!({"a": 1, "b": {"c": [1, 2], "d": "x"}, "gone": null});
        let after = json!({"a": 1.0, "b": {"c": [1, 3], "d": "y", "e": true}, "new": 1});
        let mut ops = Vec::new();
        diff(&DocPath::root(), &before, &after, &mut ops);
        let patch = Patch(ops);
        let paths: Vec<&str> = patch.ops().iter().map(|o| o.path().as_str()).collect();
        assert_eq!(paths, ["/gone", "/b/c", "/b/d", "/b/e", "/new"]);
        let mut doc = before.clone();
        apply_patch_json(&mut doc, &patch).unwrap();
        assert!(json_eq(&doc, &after));
        apply_patch_json(&mut doc, &invert(&patch)).unwrap();
        assert_eq!(doc, before);

        let mut ops = Vec::new();
        diff(&DocPath::root(), &json!([1]), &json!({"a": 1}), &mut ops);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], PatchOp::Replace { .. }));
    }

    #[test]
    fn a_value_that_becomes_keyed_is_replaced_whole() {
        let static_vec = json!({"p": {"x": 1.0, "y": 2.0}});
        let keyed = json!({"p": {"keyframes": [{"t": 0, "value": {"x": 1.0, "y": 2.0}}]}});
        for (before, after) in [(&static_vec, &keyed), (&keyed, &static_vec)] {
            let mut ops = Vec::new();
            diff(&DocPath::root(), before, after, &mut ops);
            assert_eq!(ops.len(), 1, "{ops:?}");
            assert_eq!(ops[0].path().as_str(), "/p");
            assert!(matches!(ops[0], PatchOp::Replace { .. }));
        }
    }

    #[test]
    fn get_follows_objects_and_arrays() {
        let doc = json!({"a": [{"b": 1}]});
        let seg = |s: &str| DocPath::parse(s).unwrap().to_segments();
        assert_eq!(get(&doc, &seg("/a/0/b")), Some(&json!(1)));
        assert_eq!(get(&doc, &seg("/a/1")), None);
        assert_eq!(get(&doc, &seg("/a/x")), None);
        assert_eq!(get(&doc, &seg("/a/0/b/c")), None);
        assert_eq!(get(&doc, &seg("")), Some(&doc));
    }
}
