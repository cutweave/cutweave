// SPDX-License-Identifier: GPL-3.0-or-later
//! ADR-0027 (WP-2F.03b): the project's proxy policy and an asset's proxy mode. Both are
//! optional document fields that are left out of the file while `auto`, set by the undoable
//! `media.set_proxy_policy` and `media.set_proxy_mode`; `project.new` takes the policy a new
//! project starts with; [`wanted`] decides from both.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{H, assert_code, assert_invalid};
use cutweave_commands::McpExposure;
use cutweave_model::{MediaId, Project, ProxyMode, ProxyPolicy};
use cutweave_ops::media::proxy::wanted;
use serde_json::{Value, json};

fn saved(doc: &Project) -> Value {
    serde_json::to_value(doc).unwrap()
}

#[test]
fn a_new_project_takes_the_policy_it_is_given_and_auto_without_one() {
    let mut h = H::empty();
    h.ok("project.new", json!({"name": "Plain"}));
    assert_eq!(h.doc().settings.proxy_policy, ProxyPolicy::Auto);
    assert!(
        saved(&h.doc())["settings"].get("proxy_policy").is_none(),
        "`auto` is not written"
    );

    let mut h = H::empty();
    h.ok("project.new", json!({"name": "Off", "proxy_policy": "off"}));
    assert_eq!(h.doc().settings.proxy_policy, ProxyPolicy::Off);
    assert_eq!(saved(&h.doc())["settings"]["proxy_policy"], "off");

    let mut h = H::empty();
    assert_code(
        h.run(
            "project.new",
            json!({"name": "Bad", "proxy_policy": "some"}),
        ),
        "invalid",
    );
}

#[test]
fn the_policy_is_set_and_undone_and_read_back_from_the_file() {
    let (mut h, _) = H::project();
    // `H::run` asserts the undo and redo identity of every command it runs.
    let out = h.ok("media.set_proxy_policy", json!({"policy": "all"}));
    assert_eq!(out, json!({"policy": "all", "previous": "auto"}));
    assert_eq!(h.doc().settings.proxy_policy, ProxyPolicy::All);
    let before = &h.last.as_ref().unwrap().base;
    assert_eq!(before.settings.proxy_policy, ProxyPolicy::Auto);

    // The file keeps it, and a document without the field reads `auto`.
    let mut json = saved(&h.doc());
    assert_eq!(json["settings"]["proxy_policy"], "all");
    let again: Project = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(again, h.doc());
    json["settings"]
        .as_object_mut()
        .unwrap()
        .remove("proxy_policy");
    let older: Project = serde_json::from_value(json).unwrap();
    assert_eq!(older.settings.proxy_policy, ProxyPolicy::Auto);

    let out = h.ok("media.set_proxy_policy", json!({"policy": "auto"}));
    assert_eq!(out["previous"], "all");
    assert!(saved(&h.doc())["settings"].get("proxy_policy").is_none());
    assert_code(
        h.run("media.set_proxy_policy", json!({"policy": "sometimes"})),
        "invalid",
    );
}

#[test]
fn a_mode_is_set_on_video_assets_in_one_step_and_undone() {
    let (mut h, _) = H::project();
    let a = h.add_video("/m/a.mp4", 10, true);
    let b = h.add_video("/m/b.mp4", 10, false);
    let sound = h.add_audio("/m/s.wav", 10);
    let commits = h.commits();
    let out = h.ok(
        "media.set_proxy_mode",
        json!({"media": [a, b], "mode": "force"}),
    );
    assert_eq!(out, json!({"media": [a, b], "mode": "force"}));
    assert_eq!(h.commits(), commits + 1, "one undo step for both");
    assert_eq!(h.doc().media[&a].proxy_mode, ProxyMode::Force);
    assert_eq!(h.doc().media[&b].proxy_mode, ProxyMode::Force);
    let json = saved(&h.doc());
    assert_eq!(json["media"][a.to_string()]["proxy_mode"], "force");
    assert!(
        json["media"][sound.to_string()].get("proxy_mode").is_none(),
        "`auto` is not written"
    );

    h.ok("media.set_proxy_mode", json!({"media": [a], "mode": "off"}));
    assert_eq!(h.doc().media[&a].proxy_mode, ProxyMode::Off);
    assert_eq!(h.doc().media[&b].proxy_mode, ProxyMode::Force);
    h.ok(
        "media.set_proxy_mode",
        json!({"media": [a], "mode": "auto"}),
    );
    assert!(
        saved(&h.doc())["media"][a.to_string()]
            .get("proxy_mode")
            .is_none()
    );

    // Only video has proxies; the list is 1..=256 distinct assets of the project.
    assert_invalid(
        h.run(
            "media.set_proxy_mode",
            json!({"media": [sound], "mode": "off"}),
        ),
        "/media/0",
    );
    assert_invalid(
        h.run("media.set_proxy_mode", json!({"media": [], "mode": "off"})),
        "/media",
    );
    assert_invalid(
        h.run(
            "media.set_proxy_mode",
            json!({"media": [a, a], "mode": "off"}),
        ),
        "/media/1",
    );
    assert_code(
        h.run(
            "media.set_proxy_mode",
            json!({"media": [MediaId::from_u128(77)], "mode": "off"}),
        ),
        "not_found",
    );
}

#[test]
fn the_mode_wins_over_the_policy() {
    let (mut h, _) = H::project();
    // A plain 1080p clip: `auto` gives it no proxy.
    let m = h.add_video("/m/a.mp4", 10, true);
    let wants = |h: &H| wanted(&h.doc(), &h.doc().media[&m]).is_some();
    assert!(!wants(&h));
    h.ok("media.set_proxy_policy", json!({"policy": "all"}));
    assert!(wants(&h));
    h.ok("media.set_proxy_mode", json!({"media": [m], "mode": "off"}));
    assert!(!wants(&h), "off: none, whatever the policy");
    h.ok("media.set_proxy_policy", json!({"policy": "off"}));
    h.ok(
        "media.set_proxy_mode",
        json!({"media": [m], "mode": "force"}),
    );
    assert_eq!(
        wanted(&h.doc(), &h.doc().media[&m]).as_deref(),
        Some("its proxy mode is `force`")
    );
    h.ok(
        "media.set_proxy_mode",
        json!({"media": [m], "mode": "auto"}),
    );
    assert!(!wants(&h), "auto follows the policy, which is off");
}

#[test]
fn both_commands_are_undoable_catalog_tools() {
    let h = H::empty();
    for name in ["media.set_proxy_policy", "media.set_proxy_mode"] {
        let meta = &h.registry.get(name).unwrap().meta;
        assert!(meta.undoable, "{name}");
        assert_eq!(meta.mcp, McpExposure::Catalog, "{name}");
        assert!(!meta.annotations.read_only, "{name}");
    }
}
