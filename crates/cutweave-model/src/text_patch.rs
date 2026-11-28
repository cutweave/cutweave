// SPDX-License-Identifier: GPL-3.0-or-later
//! Merge patches of text styles (WP-3A.01): `text.set_style` and `text.set_run_style` take
//! one. They follow RFC 7386 like `SettingsPatch`: a field left out keeps its value, `null`
//! clears an optional one, an object merges key by key and anything else replaces. The schema
//! of a patch is the schema of the style with every field optional.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::text::{RunStyle, TextError, TextStyle};

/// RFC 7386: objects merge key by key, `null` removes a key, anything else replaces.
fn merge(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    if let Value::Object(target) = target {
        for (key, value) in patch {
            if value.is_null() {
                target.remove(key);
            } else {
                merge(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
    }
}

/// `a.b[0]` of a serde path as `/a/b/0`.
fn pointer_of(path: &serde_path_to_error::Path) -> String {
    use serde_path_to_error::Segment;
    path.iter()
        .filter_map(|segment| match segment {
            Segment::Map { key } => Some(format!("/{}", key.replace('~', "~0").replace('/', "~1"))),
            Segment::Seq { index } => Some(format!("/{index}")),
            Segment::Enum { .. } | Segment::Unknown => None,
        })
        .collect()
}

/// Drops `required` everywhere: a patch may name any subset of the keys.
fn make_partial(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            map.remove("required");
            map.values_mut().for_each(make_partial);
        }
        Value::Array(items) => items.iter_mut().for_each(make_partial),
        _ => {}
    }
}

fn partial_schema<T: JsonSchema>(generator: &mut SchemaGenerator, what: &str) -> Schema {
    let mut schema = T::json_schema(generator).to_value();
    make_partial(&mut schema);
    if let Value::Object(map) = &mut schema {
        map.insert("description".to_owned(), Value::String(what.to_owned()));
    }
    Schema::try_from(schema).unwrap_or_default()
}

fn patched<T: Serialize + serde::de::DeserializeOwned>(
    base: &T,
    patch: &Map<String, Value>,
) -> Result<T, TextError> {
    let mut merged = serde_json::to_value(base).map_err(|e| TextError::new("", e.to_string()))?;
    merge(&mut merged, &Value::Object(patch.clone()));
    serde_path_to_error::deserialize::<_, T>(&merged).map_err(|e| {
        let mut path = pointer_of(e.path());
        if path.is_empty()
            && let Some(key) = patch.keys().next()
        {
            path = format!("/{key}");
        }
        TextError::new(path, e.inner().to_string())
    })
}

/// Some of a [`TextStyle`]'s fields (RFC 7386 merge patch): a field left out keeps its value,
/// `null` clears an optional one (`glow`, `background`), an object merges key by key and a
/// list replaces the list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TextStylePatch(pub Map<String, Value>);

impl TextStylePatch {
    /// `style` with the patch merged in and checked. A key the style does not have is
    /// `Invalid` at that key.
    pub fn apply(&self, style: &TextStyle) -> Result<TextStyle, TextError> {
        let known: Vec<String> = serde_json::to_value(TextStyle::default())
            .ok()
            .and_then(|v| v.as_object().map(|m| m.keys().cloned().collect()))
            .unwrap_or_default();
        if let Some(key) = self.0.keys().find(|k| !known.contains(k)) {
            return Err(TextError::new(
                format!("/{key}"),
                format!(
                    "unknown style field `{key}`; the fields are: {}",
                    known.join(", ")
                ),
            ));
        }
        let out: TextStyle = patched(style, &self.0)?;
        out.check()?;
        Ok(out)
    }
}

impl JsonSchema for TextStylePatch {
    fn schema_name() -> Cow<'static, str> {
        "TextStylePatch".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        partial_schema::<TextStyle>(
            generator,
            "Any of the text style's fields; a field left out keeps its value, null clears glow or background.",
        )
    }
}

impl specta::Type for TextStylePatch {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        <crate::Custom as specta::Type>::definition(types)
    }
}

/// Some of a [`RunStyle`]'s fields (RFC 7386 merge patch): a field left out keeps its value
/// and `null` removes the override.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunStylePatch(pub Map<String, Value>);

impl RunStylePatch {
    /// The override with the patch merged in and checked.
    pub fn apply(&self, style: Option<&RunStyle>) -> Result<RunStyle, TextError> {
        let out: RunStyle = patched(&style.cloned().unwrap_or_default(), &self.0)?;
        out.check()?;
        Ok(out)
    }
}

impl JsonSchema for RunStylePatch {
    fn schema_name() -> Cow<'static, str> {
        "RunStylePatch".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        partial_schema::<RunStyle>(
            generator,
            "Any of a run's style overrides; null removes one.",
        )
    }
}

impl specta::Type for RunStylePatch {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        <crate::Custom as specta::Type>::definition(types)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use serde_json::json;

    use super::*;

    fn patch(v: &Value) -> TextStylePatch {
        serde_json::from_value(v.clone()).unwrap()
    }

    #[test]
    fn merges_nested_objects_and_clears_with_null() {
        let mut style = TextStyle::default();
        style.glow = None;
        let out = patch(&json!({
            "size": 40.0,
            "font": {"family": "Lora"},
            "fill": {"kind": "solid", "color": "#ff0000"},
            "glow": {"radius": 12.0, "color": "#ffffff", "intensity": 1.0}
        }))
        .apply(&style)
        .unwrap();
        assert_eq!(out.font.family, "Lora");
        assert_eq!(out.font.weight, 700, "untouched keys stay");
        assert!(out.glow.is_some());
        let cleared = patch(&json!({"glow": null})).apply(&out).unwrap();
        assert!(cleared.glow.is_none());
    }

    #[test]
    fn errors_name_the_path() {
        let style = TextStyle::default();
        let e = patch(&json!({"size": 0})).apply(&style).unwrap_err();
        assert_eq!(e.path, "/size");
        let e = patch(&json!({"fill": {"kind": "solid", "color": "red"}}))
            .apply(&style)
            .unwrap_err();
        assert!(e.path.starts_with("/fill"), "{e}");
        let e = patch(&json!({"colour": "#fff"})).apply(&style).unwrap_err();
        assert_eq!(e.path, "/colour");
    }

    #[test]
    fn run_patch_builds_and_removes_overrides() {
        let p: RunStylePatch = serde_json::from_value(json!({"size": 20.0})).unwrap();
        let s = p.apply(None).unwrap();
        assert_eq!(s.size, Some(20.0));
        let p: RunStylePatch = serde_json::from_value(json!({"size": null})).unwrap();
        assert!(p.apply(Some(&s)).unwrap().is_empty());
        let p: RunStylePatch = serde_json::from_value(json!({"size": -1})).unwrap();
        assert_eq!(p.apply(None).unwrap_err().path, "/size");
    }
}
