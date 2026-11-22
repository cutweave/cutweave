// SPDX-License-Identifier: GPL-3.0-or-later
//! Free-form JSON attached to model entities: namespaced plugin data ([`Custom`], arch §4.3)
//! and fields this version does not know ([`Extra`], kept so nothing is silently dropped).
//!
//! Both serialize with object keys sorted at every depth (also inside the stored JSON values),
//! so saved documents are byte-stable no matter how `serde_json` was configured.

use std::{borrow::Cow, fmt};

use imbl::OrdMap;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{MapAccess, Visitor},
    ser::{SerializeMap, SerializeSeq},
};
use serde_json::Value;

/// JSON Schema pattern of a [`Custom`] namespace: two or more lowercase dot-separated
/// segments, e.g. `com.example.color-tools`.
pub const NAMESPACE_PATTERN: &str = r"^[a-z0-9][a-z0-9_-]*(\.[a-z0-9][a-z0-9_-]*)+$";

/// Serializes a JSON value with object keys sorted at every depth.
pub(crate) struct Canonical<'a>(pub(crate) &'a Value);

impl Serialize for Canonical<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(map) => {
                let mut entries: Vec<(&String, &Value)> = map.iter().collect();
                entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
                let mut out = s.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    out.serialize_entry(key, &Canonical(value))?;
                }
                out.end()
            }
            Value::Array(items) => {
                let mut out = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    out.serialize_element(&Canonical(item))?;
                }
                out.end()
            }
            other => other.serialize(s),
        }
    }
}

/// `value` as pretty-printed JSON (two-space indent) with object keys sorted at every depth: a
/// stable form for snapshots, fixtures and diffs, whatever `serde_json` features are enabled.
#[must_use]
pub fn canonical_json_pretty(value: &Value) -> String {
    // Serializing a `Value` cannot fail: every key is a string.
    serde_json::to_string_pretty(&Canonical(value)).unwrap_or_default()
}

fn serialize_sorted<S: Serializer>(map: &OrdMap<String, Value>, s: S) -> Result<S::Ok, S::Error> {
    let mut out = s.serialize_map(Some(map.len()))?;
    for (key, value) in map {
        out.serialize_entry(key, &Canonical(value))?;
    }
    out.end()
}

struct JsonMapVisitor;

impl<'de> Visitor<'de> for JsonMapVisitor {
    type Value = OrdMap<String, Value>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut map = OrdMap::new();
        while let Some((key, value)) = access.next_entry::<String, Value>()? {
            map.insert(key, value);
        }
        Ok(map)
    }
}

/// Whether `key` is a valid [`Custom`] namespace (see [`NAMESPACE_PATTERN`]).
#[must_use]
pub fn is_valid_namespace(key: &str) -> bool {
    let mut segments = 0;
    for segment in key.split('.') {
        let mut bytes = segment.bytes();
        let Some(first) = bytes.next() else {
            return false;
        };
        if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
            return false;
        }
        if !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_') {
            return false;
        }
        segments += 1;
    }
    segments >= 2
}

/// A [`Custom`] key that is not a valid namespace.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{0}` is not a plugin namespace (expected lowercase dot-separated segments such as `com.example.plugin`)"
)]
pub struct NamespaceError(pub String);

/// Plugin-owned data, keyed by namespace (`com.example.plugin` → any JSON). The only place
/// plugins may store data on model entities, so they can never corrupt core fields (arch §4.3).
/// Built-in commands never read or write it.
///
/// Serialized as a JSON object with sorted keys; exported to TypeScript as
/// `{ [key in string]: JsonValue }`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Custom(OrdMap<String, Value>);

impl Custom {
    /// No data.
    #[must_use]
    pub fn new() -> Custom {
        Custom::default()
    }

    /// The data stored under `namespace`.
    #[must_use]
    pub fn get(&self, namespace: &str) -> Option<&Value> {
        self.0.get(namespace)
    }

    /// Stores `value` under `namespace`, returning the previous value. `Err` if the key is not a
    /// valid namespace.
    pub fn insert(
        &mut self,
        namespace: impl Into<String>,
        value: Value,
    ) -> Result<Option<Value>, NamespaceError> {
        let namespace = namespace.into();
        if !is_valid_namespace(&namespace) {
            return Err(NamespaceError(namespace));
        }
        Ok(self.0.insert(namespace, value))
    }

    /// Removes and returns the data under `namespace`.
    pub fn remove(&mut self, namespace: &str) -> Option<Value> {
        self.0.remove(namespace)
    }

    /// Entries in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.0.iter()
    }

    /// Number of namespaces.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there is no data.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Keys that are not valid namespaces (possible only in loaded documents).
    pub fn invalid_keys(&self) -> impl Iterator<Item = &String> {
        self.0.keys().filter(|k| !is_valid_namespace(k))
    }
}

impl Serialize for Custom {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_sorted(&self.0, s)
    }
}

impl<'de> Deserialize<'de> for Custom {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Custom, D::Error> {
        d.deserialize_map(JsonMapVisitor).map(Custom)
    }
}

impl JsonSchema for Custom {
    fn schema_name() -> Cow<'static, str> {
        "Custom".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "Plugin data keyed by namespace (e.g. \"com.example.plugin\"). Never touched by built-in commands.",
            "type": "object",
            "propertyNames": { "pattern": NAMESPACE_PATTERN },
            "additionalProperties": true
        })
    }
}

/// TypeScript shape of any JSON value (numbers are plain `number`s). Only its type
/// definition is used.
#[derive(Serialize, specta::Type)]
#[serde(untagged)]
#[allow(dead_code)]
enum JsonValue {
    Null,
    Bool(bool),
    Number(#[specta(type = specta_typescript::Number)] f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(std::collections::BTreeMap<String, JsonValue>),
}

impl specta::Type for Custom {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        <std::collections::BTreeMap<String, JsonValue> as specta::Type>::definition(types)
    }
}

/// Fields of a loaded document that this version does not know (from a newer app or another
/// tool). They are kept verbatim and written back on save, so a round trip never drops data
/// (WP-1A.03). Flattened into the owning entity on the wire; not part of the TypeScript types.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extra(OrdMap<String, Value>);

impl Extra {
    /// No unknown fields.
    #[must_use]
    pub fn new() -> Extra {
        Extra::default()
    }

    /// The unknown field `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    /// Entries in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.0.iter()
    }

    /// Number of unknown fields.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Stores an unknown field (tests and migrations).
    pub fn insert(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
        self.0.insert(key.into(), value)
    }

    /// Removes an unknown field (migrations that adopt it as a known one).
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.0.remove(key)
    }
}

impl Serialize for Extra {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_sorted(&self.0, s)
    }
}

impl<'de> Deserialize<'de> for Extra {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Extra, D::Error> {
        d.deserialize_map(JsonMapVisitor).map(Extra)
    }
}

/// In TypeScript, flattened into its entity as `{ [key in string]: unknown }`: documents may
/// carry fields the generated types do not list, and reading one yields `unknown`.
impl specta::Type for Extra {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        <std::collections::BTreeMap<String, specta_typescript::Unknown> as specta::Type>::definition(
            types,
        )
    }
}

impl JsonSchema for Extra {
    fn schema_name() -> Cow<'static, str> {
        "Extra".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "object", "additionalProperties": true })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn namespaces() {
        for ok in ["com.example", "cutweave.fx.blur", "a.b", "x1.y_2.z-3"] {
            assert!(is_valid_namespace(ok), "{ok}");
        }
        for bad in [
            "",
            "plugin",
            "Com.example",
            ".a.b",
            "a..b",
            "a.b.",
            "a.-b",
            "a b.c",
            "é.x",
        ] {
            assert!(!is_valid_namespace(bad), "{bad}");
        }
        let mut custom = Custom::new();
        assert!(custom.insert("noplugin", json!(1)).is_err());
        assert_eq!(custom.insert("com.example", json!(1)), Ok(None));
        assert_eq!(custom.insert("com.example", json!(2)), Ok(Some(json!(1))));
        assert_eq!(custom.get("com.example"), Some(&json!(2)));
        assert_eq!(custom.len(), 1);
        assert_eq!(custom.remove("com.example"), Some(json!(2)));
        assert!(custom.is_empty());
    }

    #[test]
    fn serialization_sorts_keys_at_every_depth() {
        let value: Value =
            serde_json::from_str(r#"{"z":1,"a":{"y":[{"d":1,"c":2}],"b":true}}"#).unwrap();
        let mut custom = Custom::new();
        custom.insert("org.zeta", value.clone()).unwrap();
        custom.insert("org.alpha", Value::Null).unwrap();
        assert_eq!(
            serde_json::to_string(&custom).unwrap(),
            r#"{"org.alpha":null,"org.zeta":{"a":{"b":true,"y":[{"c":2,"d":1}]},"z":1}}"#
        );
        let back: Custom = serde_json::from_str(&serde_json::to_string(&custom).unwrap()).unwrap();
        assert_eq!(back, custom);
        let extra: Extra = serde_json::from_str(r#"{"b":1,"a":2}"#).unwrap();
        assert_eq!(serde_json::to_string(&extra).unwrap(), r#"{"a":2,"b":1}"#);
        assert_eq!(extra.get("a"), Some(&json!(2)));
    }

    #[test]
    fn invalid_keys_are_reported_not_rejected_on_load() {
        let custom: Custom = serde_json::from_str(r#"{"bad":1,"com.ok":2}"#).unwrap();
        assert_eq!(custom.invalid_keys().collect::<Vec<_>>(), ["bad"]);
    }
}
