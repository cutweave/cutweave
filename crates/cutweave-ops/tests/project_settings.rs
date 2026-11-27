// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2H.08: `project.set_settings` (one undoable merge patch for every `project.settings`
//! key), `project.info` returning the settings, `project.new{settings}` and
//! `media.set_proxy_policy` as a named shortcut of the same patch.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, assert_invalid};
use cutweave_commands::McpExposure;
use cutweave_model::{Duration, ProxyPolicy};
use serde_json::json;

fn flicks(secs: i64) -> i64 {
    Duration::from_secs(secs).unwrap().flicks()
}

#[test]
fn set_settings_patches_the_keys_it_names_as_one_undo_step() {
    let (mut h, _) = H::project();
    let commits = h.commits();
    // `H::run` asserts the undo and redo identity of every command it runs.
    let out = h.ok(
        "project.set_settings",
        json!({"patch": {"still_duration": flicks(3), "proxy_policy": "all"}}),
    );
    assert_eq!(h.commits(), commits + 1);
    let doc = h.doc();
    assert_eq!(doc.settings.still_duration().flicks(), flicks(3));
    assert_eq!(doc.settings.proxy_policy, ProxyPolicy::All);
    assert_eq!(out["settings"]["still_duration"], flicks(3));
    assert_eq!(out["settings"]["proxy_policy"], "all");
    assert_eq!(out["settings"]["default_format"]["width"], 1920);
    let before = &h.last.as_ref().unwrap().base;
    assert_eq!(before.settings.still_duration, None);

    // An object value merges; the keys it leaves out stay.
    h.ok(
        "project.set_settings",
        json!({"patch": {"default_format": {"width": 1080, "height": 1920}}}),
    );
    let doc = h.doc();
    assert_eq!(
        (
            doc.settings.default_format.width,
            doc.settings.default_format.height
        ),
        (1080, 1920)
    );
    assert_eq!(doc.settings.default_format.sample_rate.hz(), 48_000);
    assert_eq!(doc.settings.proxy_policy, ProxyPolicy::All);
    // The sequences that exist keep their format.
    assert!(doc.sequences.values().all(|s| s.format.width == 1920));

    // A time is flicks or a seconds string, as in every command.
    h.ok(
        "project.set_settings",
        json!({"patch": {"still_duration": "2.5s"}}),
    );
    assert_eq!(h.doc().settings.still_duration().flicks(), flicks(5) / 2);

    // `null` puts an optional key back to its default.
    h.ok(
        "project.set_settings",
        json!({"patch": {"still_duration": null}}),
    );
    assert_eq!(h.doc().settings.still_duration, None);
    assert_eq!(h.doc().settings.still_duration().flicks(), flicks(5));
}

#[test]
fn set_settings_refuses_unknown_keys_and_bad_values_at_their_path() {
    let (mut h, _) = H::project();
    assert_invalid(
        h.run("project.set_settings", json!({"patch": {"nope": true}})),
        "/patch/nope",
    );
    assert_invalid(
        h.run(
            "project.set_settings",
            json!({"patch": {"default_format": {"widht": 4}}}),
        ),
        "/patch/default_format/widht",
    );
    assert_invalid(
        h.run(
            "project.set_settings",
            json!({"patch": {"proxy_policy": "some"}}),
        ),
        "/patch/proxy_policy",
    );
    // Semantic checks: the same bounds as `project.new`.
    assert_invalid(
        h.run(
            "project.set_settings",
            json!({"patch": {"still_duration": 0}}),
        ),
        "/patch/still_duration",
    );
    assert_invalid(
        h.run(
            "project.set_settings",
            json!({"patch": {"still_duration": flicks(3601)}}),
        ),
        "/patch/still_duration",
    );
    assert_invalid(
        h.run(
            "project.set_settings",
            json!({"patch": {"default_format": {"width": 0}}}),
        ),
        "/patch/default_format",
    );
    assert_invalid(
        h.run(
            "project.set_settings",
            json!({"patch": {"default_format": {"frame_rate": {"num": 2000, "den": 1}}}}),
        ),
        "/patch/default_format",
    );
    // A patch that is not an object, or is missing.
    assert_invalid(h.run("project.set_settings", json!({"patch": 3})), "/patch");
    assert!(h.run("project.set_settings", json!({})).is_err());
    // A dry run says the same.
    assert!(
        h.validate("project.set_settings", json!({"patch": {"nope": 1}}))
            .is_err()
    );
    assert!(
        h.validate(
            "project.set_settings",
            json!({"patch": {"proxy_policy": "off"}})
        )
        .is_ok()
    );
}

#[test]
fn set_settings_is_undoable_and_in_the_mcp_catalog() {
    let (h, _) = H::project();
    let spec = &h.registry.get("project.set_settings").unwrap().meta;
    assert!(spec.undoable);
    assert_eq!(spec.mcp, McpExposure::Catalog);
    // The patch schema lists the keys of `ProjectSettings` (the dialog draws from it).
    let patch = &spec.input_schema["properties"]["patch"];
    for key in ["default_format", "still_duration", "proxy_policy"] {
        assert!(patch["properties"].get(key).is_some(), "{key}");
    }
    assert!(patch.get("required").is_none());
}

#[test]
fn the_proxy_policy_shortcut_is_the_same_patch() {
    let (mut a, _) = H::project();
    let (mut b, _) = H::project();
    a.ok("media.set_proxy_policy", json!({"policy": "off"}));
    b.ok(
        "project.set_settings",
        json!({"patch": {"proxy_policy": "off"}}),
    );
    assert_eq!(a.doc().settings, b.doc().settings);
    assert_eq!(
        a.last.as_ref().unwrap().patch,
        b.last.as_ref().unwrap().patch,
        "the two commands record the same document patch"
    );
}

#[test]
fn info_returns_the_settings() {
    let (mut h, _) = H::project();
    let info = h.query("project.info", json!({})).unwrap();
    assert_eq!(info["settings"]["default_format"], info["default_format"]);
    assert_eq!(info["settings"]["proxy_policy"], "auto");
    assert_eq!(
        info["settings"]["still_duration"],
        flicks(5),
        "the value in effect, also while the file stores none"
    );
    h.ok(
        "project.set_settings",
        json!({"patch": {"still_duration": flicks(3), "proxy_policy": "off"}}),
    );
    let info = h.query("project.info", json!({})).unwrap();
    assert_eq!(info["settings"]["still_duration"], flicks(3));
    assert_eq!(info["settings"]["proxy_policy"], "off");
}

#[test]
fn new_takes_settings() {
    let mut h = H::empty();
    let out = h.ok(
        "project.new",
        json!({"name": "S", "settings": {
            "still_duration": flicks(3),
            "proxy_policy": "all",
            "default_format": {"width": 1080, "height": 1920},
        }}),
    );
    let doc = h.doc();
    assert_eq!(doc.settings.still_duration().flicks(), flicks(3));
    assert_eq!(doc.settings.proxy_policy, ProxyPolicy::All);
    let seq = &doc.sequences[&common::id::<cutweave_model::SequenceId>(&out["sequence"])];
    assert_eq!((seq.format.width, seq.format.height), (1080, 1920));
    assert_eq!(seq.format, doc.settings.default_format);
}

#[test]
fn new_mixes_settings_with_the_named_params_but_not_for_the_same_key() {
    // Different keys: both apply.
    let mut h = H::empty();
    h.ok(
        "project.new",
        json!({"name": "S", "still_duration": "2s", "format": {"width": 1280},
               "settings": {"proxy_policy": "off"}}),
    );
    let doc = h.doc();
    assert_eq!(doc.settings.still_duration().flicks(), flicks(2));
    assert_eq!(doc.settings.proxy_policy, ProxyPolicy::Off);
    assert_eq!(doc.settings.default_format.width, 1280);

    // The same key both ways is refused at the `settings` key.
    let mut h = H::empty();
    assert_invalid(
        h.run(
            "project.new",
            json!({"name": "S", "still_duration": "2s",
                   "settings": {"still_duration": flicks(3)}}),
        ),
        "/settings/still_duration",
    );
    assert_invalid(
        h.run(
            "project.new",
            json!({"name": "S", "proxy_policy": "off", "settings": {"proxy_policy": "all"}}),
        ),
        "/settings/proxy_policy",
    );
    assert_invalid(
        h.run(
            "project.new",
            json!({"name": "S", "format": {"width": 1280},
                   "settings": {"default_format": {"height": 720}}}),
        ),
        "/settings/default_format",
    );
    // The same checks as `project.set_settings`.
    assert_invalid(
        h.run("project.new", json!({"name": "S", "settings": {"nope": 1}})),
        "/settings/nope",
    );
    assert_invalid(
        h.run(
            "project.new",
            json!({"name": "S", "settings": {"still_duration": flicks(3601)}}),
        ),
        "/settings/still_duration",
    );
    assert_invalid(
        h.run(
            "project.new",
            json!({"name": "S", "settings": {"default_format": {"width": 0}}}),
        ),
        "/settings/default_format",
    );
}
