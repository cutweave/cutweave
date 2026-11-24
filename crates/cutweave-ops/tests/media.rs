// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1B.05: `media.register`, `media.import` (through the `MediaProbe` boundary),
//! `media.list`, `media.remove`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names
)]

mod common;

use common::{H, assert_code, assert_invalid, id};
use cutweave_commands::{Access, CmdError};
use cutweave_model::{MediaId, MediaKind, ProbeInfo};
use cutweave_ops::media::{FakeProbe, ProbeError, kind_of_probe};
use serde_json::{Value, json};

fn reg(path: &str, probed: &cutweave_ops::media::Probed) -> Value {
    json!({"path": path, "probe": probed.probe, "content_hash": probed.content_hash})
}

// ----- media.register -----------------------------------------------------------------------

#[test]
fn register_derives_the_kind_from_the_streams() {
    let mut h = H::empty();
    let video = FakeProbe::video("/m/v.mp4", 10, true);
    let audio = FakeProbe::audio("/m/a.wav", 10);
    let image = FakeProbe::image("/m/i.png");
    for (path, probed, kind) in [
        ("/m/v.mp4", &video, "video"),
        ("/m/a.wav", &audio, "audio"),
        ("/m/i.png", &image, "image"),
    ] {
        let out = h.ok("media.register", reg(path, probed));
        assert_eq!(out["kind"], kind);
        assert_eq!(out["created"], true);
    }
    assert_eq!(h.doc().media.len(), 3);
    assert_eq!(kind_of_probe(&video.probe), Some(MediaKind::Video));
}

#[test]
fn register_deduplicates_by_content_hash() {
    let mut h = H::empty();
    let p = FakeProbe::video("/m/v.mp4", 10, true);
    let first = h.ok("media.register", reg("/m/v.mp4", &p));
    let again = h.ok("media.register", reg("/elsewhere/copy.mp4", &p));
    assert_eq!(again["created"], false);
    assert_eq!(again["media_id"], first["media_id"]);
    assert!(h.last.as_ref().unwrap().is_empty(), "nothing changed");
    assert_eq!(h.doc().media.len(), 1);
}

#[test]
fn register_honours_a_given_id_and_rejects_a_taken_one() {
    let mut h = H::empty();
    let wanted = MediaId::from_u128(7);
    let a = FakeProbe::audio("/m/a.wav", 3);
    let mut params = reg("/m/a.wav", &a);
    params["media_id"] = json!(wanted);
    assert_eq!(h.ok("media.register", params)["media_id"], json!(wanted));
    let b = FakeProbe::audio("/m/b.wav", 3);
    let mut params = reg("/m/b.wav", &b);
    params["media_id"] = json!(wanted);
    assert_invalid(h.run("media.register", params), "/media_id");
}

#[test]
fn register_validates_path_hash_and_probe_with_field_paths() {
    let mut h = H::empty();
    let p = FakeProbe::video("/m/v.mp4", 10, false);
    let mut bad = reg("relative/v.mp4", &p);
    assert_invalid(h.run("media.register", bad.clone()), "/path");
    bad["path"] = json!("/m/v.mp4");
    bad["content_hash"] = json!("has space");
    assert_invalid(h.run("media.register", bad.clone()), "/content_hash");
    bad["content_hash"] = json!("ok");
    bad["probe"]["streams"] = json!([]);
    assert_invalid(h.run("media.register", bad.clone()), "/probe/streams");
    bad["probe"] = json!(ProbeInfo {
        duration: Some(cutweave_model::Duration::ZERO),
        ..p.probe.clone()
    });
    assert_invalid(h.run("media.register", bad.clone()), "/probe/duration");
    bad["probe"] = json!(ProbeInfo {
        rotation: 45,
        ..p.probe.clone()
    });
    assert_invalid(h.run("media.register", bad), "/probe/rotation");
    assert_eq!(h.commits(), 0);
}

#[test]
fn register_accepts_windows_and_unc_paths() {
    let mut h = H::empty();
    for (i, path) in ["C:\\Videos\\a.mp4", "D:/x/b.mp4", "\\\\nas\\share\\c.mp4"]
        .iter()
        .enumerate()
    {
        let p = FakeProbe::video(path, 5, false);
        let out = h.run("media.register", reg(path, &p)).unwrap();
        assert_eq!(out["created"], true, "{i}");
    }
}

#[test]
fn register_undo_redo_identity() {
    let mut h = H::empty();
    h.add_video("/m/v.mp4", 10, true);
    let c = h.last.as_ref().unwrap();
    assert_eq!(c.base.media.len(), 0);
    assert_eq!(c.doc.media.len(), 1);
}

#[test]
fn register_is_hidden_from_mcp() {
    let h = H::empty();
    let meta = &h.registry.get("media.register").unwrap().meta;
    assert_eq!(meta.mcp, cutweave_commands::McpExposure::Hidden);
}

// ----- media.import -------------------------------------------------------------------------

fn import(h: &H, prober: FakeProbe, params: Value) -> Result<Value, CmdError> {
    let mut ctx = h.import_ctx(prober);
    h.registry
        .invoke_engine_json("media.import", params, &mut ctx, Access::STANDARD)
}

fn prober() -> FakeProbe {
    FakeProbe::new()
        .with("/m/a.mp4", FakeProbe::video("/m/a.mp4", 10, true))
        .with("/m/b.wav", FakeProbe::audio("/m/b.wav", 4))
        .with("/m/c.png", FakeProbe::image("/m/c.png"))
        .with(
            "/m/dup.mp4",
            FakeProbe::video("/m/a.mp4", 10, true), // same content as a.mp4
        )
}

#[test]
fn import_returns_media_ids_synchronously() {
    let h = H::empty();
    let out = import(
        &h,
        prober(),
        json!({"paths": ["/m/a.mp4", "/m/b.wav", "/m/c.png"]}),
    )
    .unwrap();
    let media = out["media"].as_array().unwrap();
    assert_eq!(media.len(), 3);
    let kinds: Vec<_> = media.iter().map(|m| m["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["video", "audio", "image"]);
    assert_eq!(media[0]["path"], "/m/a.mp4");
    let ids: Vec<&Value> = media.iter().map(|m| &m["media_id"]).collect();
    assert_eq!(out["media_ids"], json!(ids));
    assert_eq!(out["failed"], json!([]));
    assert_eq!(out["job_ids"], json!([]), "no analysis without an analyzer");
    let doc = h.doc();
    assert_eq!(doc.media.len(), 3);
    for m in media {
        let mid: MediaId = id(&m["media_id"]);
        assert!(doc.media.contains_key(&mid));
        assert_eq!(m["duplicate"], false);
    }
    assert_eq!(h.commits(), 1, "one transaction for the whole import");
}

#[test]
fn import_reports_duplicates_and_changes_nothing_for_them() {
    let h = H::empty();
    import(&h, prober(), json!({"paths": ["/m/a.mp4"]})).unwrap();
    let out = import(&h, prober(), json!({"paths": ["/m/dup.mp4", "/m/b.wav"]})).unwrap();
    assert_eq!(out["media"][0]["duplicate"], true);
    assert_eq!(out["media"][1]["duplicate"], false);
    assert_eq!(h.doc().media.len(), 2);
}

#[test]
fn import_registers_what_it_can_and_reports_the_rest_in_failed() {
    // WP-1D.07 replaced 1B.05's all-or-nothing rule: unreadable files no longer block the rest.
    let h = H::empty();
    let out = import(
        &h,
        prober(),
        json!({"paths": ["/m/a.mp4", "/m/missing.mp4"]}),
    )
    .unwrap();
    assert_eq!(out["media"].as_array().unwrap().len(), 1);
    assert_eq!(out["media_ids"], json!([out["media"][0]["media_id"]]));
    assert_eq!(out["failed"][0]["path"], "/m/missing.mp4");
    assert_eq!(out["failed"][0]["code"], "not_found");
    assert_eq!(h.doc().media.len(), 1);
    assert_eq!(h.commits(), 1);
}

#[test]
fn import_of_only_unreadable_files_changes_nothing() {
    let h = H::empty();
    let out = import(&h, prober(), json!({"paths": ["/m/missing.mp4"]})).unwrap();
    assert_eq!(out["media"], json!([]));
    assert_eq!(out["failed"].as_array().unwrap().len(), 1);
    assert_eq!(h.commits(), 0, "no empty transaction");
}

#[test]
fn import_reports_probe_errors_with_codes_and_messages() {
    let h = H::empty();
    let p = FakeProbe::new()
        .with_error(
            "/m/x.txt",
            ProbeError::Unsupported {
                path: "/m/x.txt".into(),
                reason: "text".into(),
            },
        )
        .with_error("/m/slow.mp4", ProbeError::TimedOut("/m/slow.mp4".into()));
    let out = import(&h, p, json!({"paths": ["/m/x.txt", "/m/slow.mp4"]})).unwrap();
    let failed = &out["failed"];
    assert_eq!(failed[0]["code"], "unsupported");
    assert!(
        failed[0]["error"]
            .as_str()
            .unwrap()
            .contains("not a supported media file")
    );
    assert_eq!(failed[1]["code"], "timed_out");
    assert!(failed[1]["error"].as_str().unwrap().contains("timed out"));
    assert!(h.doc().media.is_empty());
}

#[test]
fn import_validates_the_paths_before_probing() {
    let h = H::empty();
    assert_invalid(import(&h, prober(), json!({"paths": []})), "/paths");
    assert_invalid(
        import(&h, prober(), json!({"paths": ["rel.mp4"]})),
        "/paths/0",
    );
    assert_invalid(
        import(&h, prober(), json!({"paths": ["/m/a.mp4", "/m/a.mp4"]})),
        "/paths/1",
    );
    let too_many: Vec<String> = (0..257).map(|i| format!("/m/{i}.mp4")).collect();
    assert_invalid(import(&h, prober(), json!({"paths": too_many})), "/paths");
    assert_eq!(h.commits(), 0);
}

#[test]
fn import_with_the_wrong_context_is_an_internal_error() {
    let h = H::empty();
    let e = h
        .registry
        .invoke_engine_json(
            "media.import",
            json!({"paths": ["/m/a.mp4"]}),
            &mut 0_u8,
            Access::STANDARD,
        )
        .unwrap_err();
    assert_eq!(e.code(), "internal");
}

#[test]
fn import_is_one_undo_step_of_the_sink_transaction() {
    let mut h = H::empty();
    // The sink commits through the document; a later command's undo identity still holds.
    import(&h, prober(), json!({"paths": ["/m/a.mp4", "/m/b.wav"]})).unwrap();
    h.ok("project.rename", json!({"name": "after import"}));
    assert_eq!(h.doc().media.len(), 2);
    assert_eq!(h.last.as_ref().unwrap().base.media.len(), 2);
}

// ----- media.list ---------------------------------------------------------------------------

fn many(h: &mut H, n: usize) -> Vec<MediaId> {
    (0..n)
        .map(|i| h.add_audio(&format!("/m/{i}.wav"), 2))
        .collect()
}

#[test]
fn list_pages_through_all_media_in_id_order() {
    let mut h = H::empty();
    let mut ids = many(&mut h, 5);
    ids.sort();
    let first = h.query("media.list", json!({"limit": 2})).unwrap();
    assert_eq!(first["total"], 5);
    assert_eq!(first["media"].as_array().unwrap().len(), 2);
    let mut seen: Vec<MediaId> = first["media"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| id(&m["id"]))
        .collect();
    let mut cursor = first["next_cursor"].clone();
    while !cursor.is_null() {
        let page = h
            .query("media.list", json!({"limit": 2, "cursor": cursor}))
            .unwrap();
        seen.extend(
            page["media"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| id::<MediaId>(&m["id"])),
        );
        cursor = page["next_cursor"].clone();
    }
    assert_eq!(seen, ids);
}

#[test]
fn list_defaults_and_summaries() {
    let mut h = H::empty();
    h.add_video("/m/v.mp4", 12, true);
    let out = h.query("media.list", json!({})).unwrap();
    let m = &out["media"][0];
    assert_eq!(m["kind"], "video");
    assert_eq!(m["path"], "/m/v.mp4");
    assert_eq!(m["has_video"], true);
    assert_eq!(m["has_audio"], true);
    assert_eq!(m["duration"], 12 * 705_600_000_i64);
    assert!(out["next_cursor"].is_null());
}

#[test]
fn list_of_an_empty_project_is_empty() {
    let h = H::empty();
    let out = h.query("media.list", json!({})).unwrap();
    assert_eq!(out["media"], json!([]));
    assert_eq!(out["total"], 0);
}

#[test]
fn list_validates_limit_and_cursor() {
    let h = H::empty();
    assert_invalid(h.query("media.list", json!({"limit": 0})), "/limit");
    assert_invalid(h.query("media.list", json!({"limit": 501})), "/limit");
    assert_invalid(h.query("media.list", json!({"cursor": "nope"})), "/cursor");
}

#[test]
fn list_with_an_unknown_cursor_id_continues_after_it() {
    let mut h = H::empty();
    many(&mut h, 3);
    let out = h
        .query("media.list", json!({"cursor": MediaId::from_u128(0)}))
        .unwrap();
    assert_eq!(out["media"].as_array().unwrap().len(), 3);
}

// ----- media.remove -------------------------------------------------------------------------

#[test]
fn remove_deletes_an_unused_asset() {
    let mut h = H::empty();
    let m = h.add_audio("/m/a.wav", 3);
    let out = h.ok("media.remove", json!({"id": m}));
    assert_eq!(out["media_id"], json!(m));
    assert!(h.doc().media.is_empty());
}

#[test]
fn remove_unknown_media_is_not_found() {
    let mut h = H::empty();
    assert_code(
        h.run("media.remove", json!({"id": MediaId::from_u128(1)})),
        "not_found",
    );
}

#[test]
fn remove_referenced_media_is_invalid_at_id() {
    let (mut h, p) = H::project();
    let m = h.add_video("/m/v.mp4", 10, false);
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": 0}),
    );
    assert_invalid(h.run("media.remove", json!({"id": m})), "/id");
    assert!(h.doc().media.contains_key(&m));
}

#[test]
fn remove_works_on_a_locked_track_project_once_unreferenced() {
    // Locks guard items, not assets: an unused asset is removable whatever is locked.
    let (mut h, p) = H::project();
    let m = h.add_audio("/m/a.wav", 3);
    h.ok("track.set", json!({"track": p.audio, "locked": true}));
    h.ok("media.remove", json!({"id": m}));
}

#[test]
fn remove_undo_redo_identity_restores_the_asset() {
    let mut h = H::empty();
    let m = h.add_audio("/m/a.wav", 3);
    h.ok("media.remove", json!({"id": m}));
    let c = h.last.as_ref().unwrap();
    assert!(c.base.media.contains_key(&m));
    assert!(!c.doc.media.contains_key(&m));
}

// ----- media.list: filter, sort (WP-1M.12; the shared cases are in media_find_fixture.rs) ----

fn names(out: &Value) -> Vec<String> {
    out["media"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let p = m["path"].as_str().unwrap();
            p.rsplit('/').next().unwrap().to_owned()
        })
        .collect()
}

#[test]
fn list_filters_by_text_kind_and_length() {
    let mut h = H::empty();
    h.add_video("/m/Beach Day.mp4", 12, true);
    h.add_audio("/m/beach.wav", 90);
    h.add_image("/m/Café.png");
    let by_text = h
        .query("media.list", json!({"filter": {"text": "BEACH"}}))
        .unwrap();
    assert_eq!(names(&by_text), ["Beach Day.mp4", "beach.wav"]);
    assert_eq!(by_text["matched"], 2);
    assert_eq!(by_text["total"], 3);
    let accents = h
        .query("media.list", json!({"filter": {"text": "cafe"}}))
        .unwrap();
    assert_eq!(names(&accents), ["Café.png"]);
    let audio = h
        .query(
            "media.list",
            json!({"filter": {"kinds": ["audio"], "min_duration": 60 * 705_600_000_i64}}),
        )
        .unwrap();
    assert_eq!(names(&audio), ["beach.wav"]);
    // A still has no length, so any bound excludes it.
    let bounded = h
        .query(
            "media.list",
            json!({"filter": {"max_duration": 705_600_000_000_i64}}),
        )
        .unwrap();
    assert_eq!(names(&bounded), ["Beach Day.mp4", "beach.wav"]);
}

#[test]
fn list_sorts_and_pages_in_the_requested_order() {
    let mut h = H::empty();
    h.add_audio("/m/b.wav", 5);
    h.add_audio("/m/C.wav", 3);
    h.add_audio("/m/a.wav", 9);
    let sorted = |sort: Value| names(&h.query("media.list", json!({"sort": sort})).unwrap());
    assert_eq!(sorted(json!({"by": "name"})), ["a.wav", "b.wav", "C.wav"]);
    assert_eq!(
        sorted(json!({"by": "duration", "order": "desc"})),
        ["a.wav", "b.wav", "C.wav"]
    );
    let first = h
        .query("media.list", json!({"sort": {"by": "name"}, "limit": 2}))
        .unwrap();
    assert_eq!(names(&first), ["a.wav", "b.wav"]);
    let second = h
        .query(
            "media.list",
            json!({"sort": {"by": "name"}, "limit": 2, "cursor": first["next_cursor"]}),
        )
        .unwrap();
    assert_eq!(names(&second), ["C.wav"]);
    assert!(second["next_cursor"].is_null());
}

#[test]
fn list_knows_which_media_is_used_on_a_timeline() {
    let (mut h, p) = H::project();
    let used = h.add_video("/m/used.mp4", 4, false);
    h.add_video("/m/unused.mp4", 4, false);
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": used, "at": "0"}),
    );
    let on = h
        .query("media.list", json!({"filter": {"used": true}}))
        .unwrap();
    assert_eq!(names(&on), ["used.mp4"]);
    let off = h
        .query("media.list", json!({"filter": {"used": false}}))
        .unwrap();
    assert_eq!(names(&off), ["unused.mp4"]);
}
