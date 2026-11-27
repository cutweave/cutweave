// SPDX-License-Identifier: GPL-3.0-or-later
//! A partial [`ProjectSettings`] (WP-2H.08): the merge patch `project.set_settings` and
//! `project.new` take, checked against the settings schema. The schema is derived from
//! [`ProjectSettings`], so a key added to the struct is patchable (and listed in the generated
//! command schema, which the Project settings dialog draws) with no other change.

use std::{borrow::Cow, sync::OnceLock};

use schemars::{JsonSchema, Schema, SchemaGenerator, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ProjectSettings;

/// Some of the project's settings: an object with any of the keys of `project.settings`
/// (RFC 7386 merge patch). A key that is left out keeps its value, `null` puts an optional key
/// back to its default, and an object value is merged key by key.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SettingsPatch(pub Map<String, Value>);

impl SettingsPatch {
    /// A patch that sets one top-level key.
    #[must_use]
    pub fn of(key: &str, value: Value) -> SettingsPatch {
        let mut map = Map::new();
        map.insert(key.to_owned(), value);
        SettingsPatch(map)
    }

    /// Whether the patch names the top-level key.
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }
}

/// Why a [`SettingsPatch`] was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("settings patch is invalid at `{pointer}`: {reason}")]
pub struct SettingsPatchError {
    /// JSON pointer of the offending key inside the patch (`/still_duration`).
    pub pointer: String,
    /// What is wrong.
    pub reason: String,
}

/// The schema of [`ProjectSettings`] with every subschema inline, generated once.
fn settings_schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        SchemaSettings::draft2020_12()
            .with(|s| {
                s.inline_subschemas = true;
                s.meta_schema = None;
            })
            .for_deserialize()
            .into_generator()
            .into_root_schema_for::<ProjectSettings>()
            .to_value()
    })
}

/// The `properties` of an object schema, looking through a nullable or single-variant union.
fn properties(schema: &Value) -> Option<&Map<String, Value>> {
    if let Some(found) = schema.get("properties").and_then(Value::as_object) {
        return Some(found);
    }
    ["anyOf", "oneOf", "allOf"]
        .iter()
        .filter_map(|k| schema.get(*k).and_then(Value::as_array))
        .flatten()
        .find_map(properties)
}

/// Fails at the first key of `patch` that the schema does not have.
fn check_known(
    schema: &Value,
    patch: &Map<String, Value>,
    at: &str,
) -> Result<(), SettingsPatchError> {
    let Some(known) = properties(schema) else {
        return Ok(());
    };
    for (key, value) in patch {
        let pointer = format!("{at}/{}", escape(key));
        let Some(sub) = known.get(key) else {
            let mut names: Vec<&str> = known.keys().map(String::as_str).collect();
            names.sort_unstable();
            return Err(SettingsPatchError {
                pointer,
                reason: format!(
                    "unknown setting `{key}`; the settings here are: {}",
                    names.join(", ")
                ),
            });
        };
        if let Value::Object(inner) = value {
            check_known(sub, inner, &pointer)?;
        }
    }
    Ok(())
}

/// One JSON pointer token (RFC 6901).
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

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

/// `default_format.width` (a serde path) as `/default_format/width`.
fn pointer_of(path: &serde_path_to_error::Path) -> String {
    use serde_path_to_error::Segment;
    path.iter()
        .filter_map(|segment| match segment {
            Segment::Map { key } => Some(format!("/{}", escape(key))),
            Segment::Seq { index } => Some(format!("/{index}")),
            Segment::Enum { variant } => Some(format!("/{}", escape(variant))),
            Segment::Unknown => None,
        })
        .collect()
}

impl ProjectSettings {
    /// These settings with `patch` merged in. Fails at the first key the settings do not have,
    /// or whose value is not of its type. The values themselves (ranges) are the caller's to
    /// check. Fields this version does not know (`extra`) are kept and cannot be patched.
    pub fn patched(&self, patch: &SettingsPatch) -> Result<ProjectSettings, SettingsPatchError> {
        check_known(settings_schema(), &patch.0, "")?;
        let internal = |e: serde_json::Error| SettingsPatchError {
            pointer: String::new(),
            reason: e.to_string(),
        };
        let base = serde_json::to_value(self).map_err(internal)?;
        let mut merged = base.clone();
        merge(&mut merged, &Value::Object(patch.0.clone()));
        match serde_path_to_error::deserialize::<_, ProjectSettings>(&merged) {
            Ok(settings) => Ok(settings),
            Err(error) => {
                let reason = error.inner().to_string();
                let mut pointer = pointer_of(error.path());
                if pointer.is_empty() {
                    // The path is lost behind the flattened `extra`: name the first key of the
                    // patch that fails on its own.
                    pointer = patch
                        .0
                        .iter()
                        .find(|(key, value)| {
                            let mut one = base.clone();
                            merge(
                                &mut one,
                                &Value::Object(SettingsPatch::of(key, (*value).clone()).0),
                            );
                            serde_json::from_value::<ProjectSettings>(one).is_err()
                        })
                        .map_or_default(|(key, _)| format!("/{}", escape(key)));
                }
                Err(SettingsPatchError { pointer, reason })
            }
        }
    }
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

impl JsonSchema for SettingsPatch {
    fn schema_name() -> Cow<'static, str> {
        "SettingsPatch".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = <ProjectSettings as JsonSchema>::json_schema(generator).to_value();
        make_partial(&mut schema);
        if let Value::Object(map) = &mut schema {
            map.insert(
                "description".to_owned(),
                Value::String(
                    "Any of the project's settings; a key left out keeps its value.".to_owned(),
                ),
            );
            map.insert("additionalProperties".to_owned(), Value::Bool(false));
        }
        Schema::try_from(schema).unwrap_or_default()
    }
}

impl specta::Type for SettingsPatch {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        <crate::Custom as specta::Type>::definition(types)
    }
}

/// The project's settings with the values in effect: the optional keys the file leaves out
/// while they are the default (`still_duration`, `proxy_policy`) are filled in.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(transparent)]
pub struct SettingsInEffect(pub ProjectSettings);

impl From<&ProjectSettings> for SettingsInEffect {
    fn from(settings: &ProjectSettings) -> Self {
        SettingsInEffect(settings.clone())
    }
}

impl Serialize for SettingsInEffect {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let mut value = serde_json::to_value(&self.0).map_err(S::Error::custom)?;
        if let Some(map) = value.as_object_mut() {
            let still = serde_json::to_value(self.0.still_duration()).map_err(S::Error::custom)?;
            map.entry("still_duration").or_insert(still);
            let policy = serde_json::to_value(self.0.proxy_policy).map_err(S::Error::custom)?;
            map.entry("proxy_policy").or_insert(policy);
        }
        value.serialize(serializer)
    }
}

impl JsonSchema for SettingsInEffect {
    fn schema_name() -> Cow<'static, str> {
        <ProjectSettings as JsonSchema>::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        <ProjectSettings as JsonSchema>::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        <ProjectSettings as JsonSchema>::json_schema(generator)
    }
}

impl specta::Type for SettingsInEffect {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        <ProjectSettings as specta::Type>::definition(types)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{Duration, ProxyPolicy};

    fn patch(value: Value) -> SettingsPatch {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_patch_changes_only_the_keys_it_names() {
        let base = ProjectSettings::default();
        let three = Duration::from_secs(3).unwrap();
        let next = base
            .patched(&patch(json!({ "still_duration": three.flicks() })))
            .unwrap();
        assert_eq!(next.still_duration, Some(three));
        assert_eq!(next.default_format, base.default_format);
        assert_eq!(next.proxy_policy, ProxyPolicy::Auto);

        let next = next
            .patched(&patch(
                json!({ "proxy_policy": "all", "default_format": { "width": 1280 } }),
            ))
            .unwrap();
        assert_eq!(next.proxy_policy, ProxyPolicy::All);
        assert_eq!(next.default_format.width, 1280);
        assert_eq!(next.default_format.height, base.default_format.height);
        assert_eq!(next.still_duration, Some(three));
    }

    #[test]
    fn null_puts_an_optional_key_back_to_its_default() {
        let base = ProjectSettings {
            still_duration: Some(Duration::from_secs(3).unwrap()),
            proxy_policy: ProxyPolicy::Off,
            ..ProjectSettings::default()
        };
        let next = base
            .patched(&patch(
                json!({ "still_duration": null, "proxy_policy": null }),
            ))
            .unwrap();
        assert_eq!(next, ProjectSettings::default());
    }

    #[test]
    fn an_unknown_key_fails_at_its_path() {
        let base = ProjectSettings::default();
        let e = base.patched(&patch(json!({ "nope": 1 }))).unwrap_err();
        assert_eq!(e.pointer, "/nope");
        assert!(e.reason.contains("still_duration"), "{e}");
        let e = base
            .patched(&patch(json!({ "default_format": { "widht": 10 } })))
            .unwrap_err();
        assert_eq!(e.pointer, "/default_format/widht");
    }

    #[test]
    fn a_value_of_the_wrong_type_fails_at_its_key() {
        let base = ProjectSettings::default();
        for (bad, pointer) in [
            (json!({ "proxy_policy": "sometimes" }), "/proxy_policy"),
            (json!({ "still_duration": "long" }), "/still_duration"),
            (json!({ "default_format": null }), "/default_format"),
            (
                json!({ "default_format": { "width": "wide" } }),
                "/default_format",
            ),
        ] {
            let e = base.patched(&patch(bad.clone())).unwrap_err();
            assert!(e.pointer.starts_with(pointer), "{bad}: {e}");
        }
    }

    #[test]
    fn fields_of_a_newer_version_are_kept_and_cannot_be_patched() {
        let base: ProjectSettings = serde_json::from_value(json!({
            "default_format": ProjectSettings::default().default_format,
            "from_the_future": { "x": 1 },
        }))
        .unwrap();
        let next = base
            .patched(&patch(json!({ "proxy_policy": "off" })))
            .unwrap();
        assert_eq!(
            serde_json::to_value(&next).unwrap()["from_the_future"],
            json!({ "x": 1 })
        );
        let e = base
            .patched(&patch(json!({ "from_the_future": null })))
            .unwrap_err();
        assert_eq!(e.pointer, "/from_the_future");
    }

    /// WP-2H.08: the patch schema is derived, so a key added to `ProjectSettings` is in it.
    #[test]
    fn the_patch_schema_lists_every_settings_key_and_requires_none() {
        // Inline subschemas, as the command registry generates them.
        let patch = SchemaSettings::draft2020_12()
            .with(|s| s.inline_subschemas = true)
            .into_generator()
            .into_root_schema_for::<SettingsPatch>()
            .to_value();
        let mut keys: Vec<&String> = patch["properties"].as_object().unwrap().keys().collect();
        let mut expected: Vec<&String> = properties(settings_schema()).unwrap().keys().collect();
        keys.sort();
        expected.sort();
        assert_eq!(keys, expected);
        assert!(keys.iter().any(|k| *k == "still_duration"));
        assert!(!patch.to_string().contains("\"required\""));
    }
}
