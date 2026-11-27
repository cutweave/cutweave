// SPDX-License-Identifier: GPL-3.0-or-later
//! Comparing the recovered document with the reference engine's.
//!
//! The two engines do not mint the same ids: a refused command or a cancelled drag draws ids
//! that are never used, and a recovered engine continues its id stream after the last
//! recovered transaction. So documents are compared by structure: every entity id is replaced
//! by its kind and the order of its first appearance ([`normalize`]). Under
//! `--deterministic-ids` an id starts with a counter and the document's maps are sorted by id,
//! so entities appear in the order they were made, in both engines.
//!
//! Commands sent to the reference have their ids translated ([`IdMap`]), learnt from the
//! outputs of the same command in both engines ([`IdMap::pair`]) and from the two documents
//! ([`IdMap::pair_documents`]).

use std::collections::BTreeMap;

use serde_json::Value;

/// Whether `s` starts with an entity id (`itm_` + 32 hex digits; any 2..=5 letter kind);
/// returns its length.
fn id_len(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let kind = bytes
        .iter()
        .take(6)
        .position(|b| *b == b'_')
        .filter(|n| (2..=5).contains(n))?;
    if !bytes[..kind].iter().all(u8::is_ascii_lowercase) {
        return None;
    }
    let end = kind + 1 + 32;
    let hex = bytes.get(kind + 1..end)?;
    hex.iter()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        .then_some(end)
}

/// Whether the whole of `s` is one entity id.
#[must_use]
pub fn is_id(s: &str) -> bool {
    id_len(s) == Some(s.len())
}

/// The entity ids in `text`, each once, in the order they first appear.
#[must_use]
pub fn ids_in_order(text: &str) -> Vec<&str> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        // An id starts at a letter that does not continue a word.
        let starts_word = i == 0 || !text.as_bytes()[i - 1].is_ascii_alphanumeric();
        match text.get(i..).and_then(id_len).filter(|_| starts_word) {
            Some(len) => {
                let id = &text[i..i + len];
                if seen.insert(id) {
                    out.push(id);
                }
                i += len;
            }
            None => i += text[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    out
}

/// `text` with every entity id replaced by `<kind>_<n>`, `n` counting the distinct ids in
/// order of first appearance.
#[must_use]
pub fn normalize(text: &str) -> String {
    let numbers: BTreeMap<&str, usize> = ids_in_order(text)
        .into_iter()
        .enumerate()
        .map(|(n, id)| (id, n + 1))
        .collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let starts_word = i == 0 || !text.as_bytes()[i - 1].is_ascii_alphanumeric();
        if let Some(len) = text.get(i..).and_then(id_len).filter(|_| starts_word) {
            let id = &text[i..i + len];
            let kind = id.split('_').next().unwrap_or_default();
            out.push_str(kind);
            out.push('_');
            out.push_str(&numbers.get(id).copied().unwrap_or(0).to_string());
            i += len;
        } else {
            let ch = text[i..].chars().next().unwrap_or_default();
            out.push(ch);
            i += ch.len_utf8().max(1);
        }
    }
    out
}

/// Ids of the engine under test and what the reference engine calls the same entities.
#[derive(Clone, Debug, Default)]
pub struct IdMap(BTreeMap<String, String>);

impl IdMap {
    /// Learns from the outputs of one command in both engines: ids at the same place are the
    /// same entity.
    pub fn pair(&mut self, subject: &Value, reference: &Value) {
        match (subject, reference) {
            (Value::String(a), Value::String(b)) if is_id(a) && is_id(b) => {
                self.0.insert(a.clone(), b.clone());
            }
            (Value::Array(a), Value::Array(b)) => {
                for (a, b) in a.iter().zip(b) {
                    self.pair(a, b);
                }
            }
            (Value::Object(a), Value::Object(b)) => {
                for (key, a) in a {
                    if let Some(b) = b.get(key) {
                        self.pair(a, b);
                    }
                }
            }
            _ => {}
        }
    }

    /// Learns from two documents that have the same structure: the nth id of one is the nth
    /// id of the other.
    pub fn pair_documents(&mut self, subject: &str, reference: &str) {
        for (a, b) in ids_in_order(subject)
            .into_iter()
            .zip(ids_in_order(reference))
        {
            self.0.insert(a.to_owned(), b.to_owned());
        }
    }

    /// `params` as the reference engine needs them. An id never paired is one both engines
    /// share (the fixture's) and stays.
    #[must_use]
    pub fn translate(&self, params: &Value) -> Value {
        match params {
            Value::String(s) => Value::String(self.0.get(s).cloned().unwrap_or_else(|| s.clone())),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.translate(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), self.translate(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

/// Where two JSON values first differ, as a JSON Pointer with both values (shortened).
#[must_use]
pub fn first_difference(a: &Value, b: &Value) -> Option<String> {
    fn short(v: &Value) -> String {
        let text = v.to_string();
        match text.char_indices().nth(160) {
            Some((cut, _)) => format!("{}…", &text[..cut]),
            None => text,
        }
    }
    fn walk(a: &Value, b: &Value, path: &mut String) -> Option<String> {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                for key in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                    let len = path.len();
                    path.push('/');
                    path.push_str(key);
                    let found = match (x.get(key), y.get(key)) {
                        (Some(a), Some(b)) => walk(a, b, path),
                        (Some(a), None) => Some(format!("{path}: {} vs nothing", short(a))),
                        (None, Some(b)) => Some(format!("{path}: nothing vs {}", short(b))),
                        (None, None) => None,
                    };
                    if found.is_some() {
                        return found;
                    }
                    path.truncate(len);
                }
                None
            }
            (Value::Array(x), Value::Array(y)) => {
                if x.len() != y.len() {
                    return Some(format!("{path}: {} vs {} elements", x.len(), y.len()));
                }
                for (i, (a, b)) in x.iter().zip(y).enumerate() {
                    let len = path.len();
                    path.push('/');
                    path.push_str(&i.to_string());
                    let found = walk(a, b, path);
                    if found.is_some() {
                        return found;
                    }
                    path.truncate(len);
                }
                None
            }
            _ if a == b => None,
            _ => Some(format!("{path}: {} vs {}", short(a), short(b))),
        }
    }
    walk(a, b, &mut String::new())
}

/// `document` without what a save derives: `media.*.path_rel`, the path of a source relative
/// to the project folder, is filled in when the document is written. An engine that loaded
/// the document from disk has it; one that has only saved (or never did) may not.
fn without_derived(document: &Value) -> Value {
    let mut document = document.clone();
    if let Some(media) = document.get_mut("media").and_then(Value::as_object_mut) {
        for asset in media.values_mut() {
            if let Some(asset) = asset.as_object_mut() {
                asset.remove("path_rel");
            }
        }
    }
    document
}

/// Compares two documents by structure. `Err`: where they first differ.
pub fn same_document(subject: &Value, reference: &Value) -> Result<(), String> {
    let (a, b) = (
        normalize(&without_derived(subject).to_string()),
        normalize(&without_derived(reference).to_string()),
    );
    if a == b {
        return Ok(());
    }
    let parsed = |text: &str| serde_json::from_str::<Value>(text).unwrap_or(Value::Null);
    Err(first_difference(&parsed(&a), &parsed(&b))
        .unwrap_or_else(|| "the documents differ".to_owned()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(kind: &str, n: u32) -> String {
        format!("{kind}_{n:032x}")
    }

    #[test]
    fn ids_are_recognized_and_numbered_by_first_appearance() {
        assert!(is_id(&id("itm", 1)) && is_id(&id("mrk", 2)) && is_id(&id("me", 3)));
        assert!(!is_id("itm_12") && !is_id("tx_4") && !is_id(&format!("{}x", id("itm", 1))));
        assert!(!is_id(&id("Itm", 1)) && !is_id(&id("toolong", 1)));
        let text = format!(
            "{} {} {} é 9{}",
            id("itm", 9),
            id("trk", 2),
            id("itm", 9),
            id("itm", 5)
        );
        assert_eq!(ids_in_order(&text), [id("itm", 9), id("trk", 2)]);
        assert_eq!(
            normalize(&text),
            format!("itm_1 trk_2 itm_1 é 9{}", id("itm", 5))
        );
    }

    #[test]
    fn documents_with_other_ids_and_the_same_structure_are_the_same() {
        let doc = |a: u32, b: u32, name: &str| {
            json!({"items": {id("itm", a): {"track": id("trk", b), "name": name}},
                   "order": [id("trk", b)]})
        };
        assert_eq!(same_document(&doc(1, 2, "x"), &doc(7, 9, "x")), Ok(()));
        let err = same_document(&doc(1, 2, "x"), &doc(7, 9, "y")).unwrap_err();
        assert_eq!(err, "/items/itm_1/name: \"x\" vs \"y\"");
        let saved = |rel: Value| json!({"media": {id("med", 1): {"path_abs": "/m/a.mp4", "path_rel": rel}}});
        assert_eq!(
            same_document(&saved(json!("a.mp4")), &saved(Value::Null)),
            Ok(())
        );
        let missing = json!({"items": {}, "order": [id("trk", 9)]});
        assert!(
            same_document(&doc(1, 2, "x"), &missing)
                .unwrap_err()
                .starts_with("/items")
        );
    }

    #[test]
    fn the_id_map_learns_from_outputs_and_documents() {
        let mut map = IdMap::default();
        map.pair(
            &json!({"left": id("itm", 1), "right": id("itm", 2), "linked": [id("itm", 3)], "at": 5}),
            &json!({"left": id("itm", 1), "right": id("itm", 8), "linked": [id("itm", 9)], "at": 5}),
        );
        let params = json!({"item": id("itm", 2), "items": [id("itm", 3), id("itm", 4)], "n": 1});
        assert_eq!(
            map.translate(&params),
            json!({"item": id("itm", 8), "items": [id("itm", 9), id("itm", 4)], "n": 1})
        );
        map.pair_documents(
            &format!("{} {}", id("seq", 4), id("trk", 5)),
            &format!("{} {}", id("seq", 6), id("trk", 7)),
        );
        assert_eq!(map.translate(&json!(id("trk", 5))), json!(id("trk", 7)));
    }

    #[test]
    fn the_first_difference_names_the_place() {
        let a = json!({"a": [1, 2, {"b": 3}], "c": 1});
        assert_eq!(first_difference(&a, &a), None);
        let b = json!({"a": [1, 2, {"b": 4}], "c": 1});
        assert_eq!(first_difference(&a, &b).unwrap(), "/a/2/b: 3 vs 4");
        let c = json!({"a": [1, 2], "c": 1});
        assert_eq!(first_difference(&a, &c).unwrap(), "/a: 3 vs 2 elements");
        let d = json!({"a": [1, 2, {"b": 3}], "c": 1, "d": 0});
        assert_eq!(first_difference(&a, &d).unwrap(), "/d: nothing vs 0");
    }
}
