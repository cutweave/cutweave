//! Engine behaviour through the type-erased JSON surface: the S5 agent task, ADR-015 `Time`
//! validation, typed errors, request_id dedup, expected_rev, dry runs, batches, per-actor undo,
//! events and deterministic ids.

// Test crate: helpers outside #[test] fns may unwrap (clippy.toml only exempts #[test] bodies).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_pass_by_value
)]

mod common;

use std::time::Duration;

use base64::Engine as _;
use common::{agent, call_json, engine, engine_with_clock, err, ok, secs, setup_clip};
use s5_bus::{
    Actor, Caller, Engine, EngineConfig, Rev,
    engine::{Batch, Event, Request, Step},
    time::MAX_SAFE_FLICKS,
};
use serde_json::{Value, json};

#[test]
fn agent_task_import_cut_and_render_frame() {
    // The S5 pass task: "Import <fixture>, cut it at 2 s, and show me the frame at 3 s".
    let mut e = engine();
    let a = agent();
    let reply = call_json(
        &mut e,
        &a,
        "media.import",
        json!({ "path": "/footage/interview.mp4" }),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let media = reply["result"]["media"].clone();
    let item = ok(
        &mut e,
        &a,
        "timeline.insert",
        json!({ "media": media, "at": 0 }),
    )["item"]
        .clone();
    // Timecode string input, normalized with the sequence rate (30 fps).
    let split = ok(
        &mut e,
        &a,
        "clip.split",
        json!({ "item": item, "at": "00:00:02:00" }),
    );
    assert_eq!(split["left"], item);
    let frame = ok(
        &mut e,
        &a,
        "render.frame",
        json!({ "at": "3s", "width": 320 }),
    );
    assert_eq!(frame["at"], secs(3), "outputs are integer flicks");
    assert_eq!(frame["visible"], json!([split["right"]]));
    assert_eq!(
        (frame["width"].as_u64(), frame["height"].as_u64()),
        (Some(320), Some(180))
    );
    let png = base64::engine::general_purpose::STANDARD
        .decode(frame["data"].as_str().unwrap())
        .unwrap();
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let info = decoder.read_info().unwrap();
    assert_eq!((info.info().width, info.info().height), (320, 180));

    let timeline = ok(&mut e, &a, "timeline.get", json!({}));
    let items = &timeline["tracks"][0]["items"];
    assert_eq!(items.as_array().map(Vec::len), Some(2));
    assert_eq!(items[0]["end"], secs(2));
    assert_eq!(items[1]["start"], secs(2));
    assert_eq!(items[1]["source_in"], secs(2));
}

#[test]
fn time_above_max_safe_integer_is_invalid() {
    let mut e = engine();
    let a = agent();
    let (_, item) = setup_clip(&mut e, &a);
    let error = err(
        &mut e,
        &a,
        "clip.split",
        json!({ "item": item, "at": MAX_SAFE_FLICKS + 1 }),
    );
    assert_eq!(error["code"], "invalid");
    assert_eq!(error["path"], "/at");
    assert!(
        error["msg"].as_str().unwrap().contains("safe-integer"),
        "{error}"
    );
    // Fractional flicks and unparseable strings are Invalid too.
    let error = err(&mut e, &a, "clip.split", json!({ "item": item, "at": 2.5 }));
    assert_eq!(
        (error["code"].as_str(), error["path"].as_str()),
        (Some("invalid"), Some("/at"))
    );
    let error = err(
        &mut e,
        &a,
        "clip.split",
        json!({ "item": item, "at": "two seconds" }),
    );
    assert_eq!(error["path"], "/at");
    // Drop-frame timecode is explicitly unsupported in the spike.
    let error = err(
        &mut e,
        &a,
        "clip.split",
        json!({ "item": item, "at": "00:00:01;00" }),
    );
    assert!(
        error["msg"].as_str().unwrap().contains("drop-frame"),
        "{error}"
    );
}

#[test]
fn timecode_inputs_parse_against_sequence_rate() {
    let mut e = engine();
    let a = agent();
    let (_, item) = setup_clip(&mut e, &a);
    // f:45 at 30 fps = 1.5 s; "0.5s" = 0.5 s; plain integers are flicks.
    let out = ok(
        &mut e,
        &a,
        "clip.trim",
        json!({ "item": item, "start": "0.5s", "end": "f:45" }),
    );
    assert_eq!(
        (out["start"].as_i64(), out["end"].as_i64()),
        (Some(secs(1) / 2), Some(secs(3) / 2))
    );
    // After project.open at 25 fps, frame timecodes follow the new rate.
    ok(
        &mut e,
        &a,
        "project.open",
        json!({ "name": "PAL", "frame_rate": { "num": 25, "den": 1 } }),
    );
    let (_, item) = setup_clip(&mut e, &a);
    let out = ok(
        &mut e,
        &a,
        "clip.split",
        json!({ "item": item, "at": "00:00:01:05" }),
    );
    let tl = ok(&mut e, &a, "timeline.get", json!({}));
    assert_eq!(
        tl["tracks"][0]["items"][0]["end"],
        secs(1) + secs(1) / 5,
        "{out}"
    );
}

#[test]
fn typed_errors_carry_code_path_and_message() {
    let mut e = engine();
    let a = agent();
    let (media, item) = setup_clip(&mut e, &a);

    let unknown_field = err(
        &mut e,
        &a,
        "clip.split",
        json!({ "item": item, "at": 1, "bogus": 1 }),
    );
    assert_eq!(unknown_field["code"], "invalid");
    assert_eq!(
        unknown_field["path"], "/bogus",
        "serde_path_to_error points at the offending key"
    );
    assert!(
        unknown_field["msg"]
            .as_str()
            .unwrap()
            .contains("unknown field `bogus`")
    );

    let wrong_kind = err(&mut e, &a, "clip.split", json!({ "item": media, "at": 1 }));
    assert_eq!(
        (wrong_kind["code"].as_str(), wrong_kind["path"].as_str()),
        (Some("invalid"), Some("/item"))
    );

    let missing = err(
        &mut e,
        &a,
        "clip.split",
        json!({ "item": "itm_00000000000000000000000000000000", "at": 1 }),
    );
    assert_eq!(
        missing,
        json!({
            "code": "not_found", "entity": "item", "id": "itm_00000000000000000000000000000000",
            "message": "item `itm_00000000000000000000000000000000` not found"
        })
    );

    let outside = err(
        &mut e,
        &a,
        "clip.split",
        json!({ "item": item, "at": secs(30) }),
    );
    assert_eq!(outside["path"], "/at");

    let nested = err(
        &mut e,
        &a,
        "item.delete",
        json!({ "items": [item, "nope"] }),
    );
    assert_eq!(
        nested["path"], "/items/1",
        "array index in the JSON Pointer"
    );

    let unknown = call_json(&mut e, &a, "clip.explode", json!({}));
    assert_eq!(unknown["ok"], false);
    assert_eq!(unknown["error"]["code"], "not_found");
    assert_eq!(unknown["error"]["entity"], "command");
}

#[test]
fn actor_comes_from_the_caller_not_the_request() {
    let mut e = engine();
    let reply = e.invoke_json(
        &agent(),
        json!({ "name": "media.import", "params": { "path": "/a.mp4" }, "actor": "user" }),
    );
    assert_eq!(reply["ok"], false);
    assert!(
        reply["error"]["msg"]
            .as_str()
            .unwrap()
            .contains("unknown field `actor`"),
        "{reply}"
    );
}

fn import_req(path: &str, id: &str) -> Request {
    Request::new("media.import", json!({ "path": path })).with_request_id(id)
}

#[test]
fn request_id_dedup_returns_original_result() {
    let (mut e, clock) = engine_with_clock();
    let a = agent();
    let first = e.invoke(&a, import_req("/a.mp4", "req-1")).unwrap();
    let again = e.invoke(&a, import_req("/a.mp4", "req-1")).unwrap();
    assert!(again.deduplicated && !first.deduplicated);
    assert_eq!(
        again.result, first.result,
        "same media id, not a second import"
    );
    assert_eq!(again.rev, first.rev);
    assert_eq!(e.doc().media.len(), 1);
    assert_eq!(e.rev(), Rev(1));

    // Same id, different payload: Conflict(request_id_reused) with the current rev filled in.
    let reused = e
        .invoke(&a, import_req("/b.mp4", "req-1"))
        .unwrap_err()
        .to_wire();
    assert_eq!(reused["reason"], "request_id_reused");
    assert_eq!(reused["current_rev"], 1);

    // Scoped per actor: another actor's identical id is a different request.
    let other = e
        .invoke(&Caller::agent("other"), import_req("/a.mp4", "req-1"))
        .unwrap();
    assert!(!other.deduplicated);
    assert_eq!(e.doc().media.len(), 2);

    // After the 10-minute window the id is forgotten.
    clock.advance(Duration::from_secs(601));
    let later = e.invoke(&a, import_req("/a.mp4", "req-1")).unwrap();
    assert!(!later.deduplicated);
    assert_eq!(e.doc().media.len(), 3);
}

#[test]
fn failed_requests_are_not_journaled() {
    let mut e = engine();
    let a = agent();
    let bad = Request::new("media.import", json!({ "path": "/a.xyz" })).with_request_id("r");
    assert!(e.invoke(&a, bad).is_err());
    // A corrected retry with the same id must apply (nothing was journaled for the failure).
    let fixed = Request::new("media.import", json!({ "path": "/a.xyz" })).with_request_id("r");
    assert!(
        e.invoke(&a, fixed).is_err(),
        "same failing payload fails again, not deduplicated"
    );
    assert!(e.invoke(&a, import_req("/a.mp4", "r2")).is_ok());
}

#[test]
fn retried_undo_undoes_once() {
    let mut e = engine();
    let a = agent();
    setup_clip(&mut e, &a);
    let undo = || Request::new("edit.undo", json!({})).with_request_id("undo-1");
    let first = e.invoke(&a, undo()).unwrap();
    let retry = e.invoke(&a, undo()).unwrap();
    assert!(retry.deduplicated);
    assert_eq!(first.result, retry.result);
    assert_eq!(e.doc().items.len(), 0);
    assert_eq!(e.doc().media.len(), 1, "only the insert was undone");
}

#[test]
fn expected_rev_guards_against_stale_writers() {
    let mut e = engine();
    let a = agent();
    setup_clip(&mut e, &a);
    let stale = Request::new("media.import", json!({ "path": "/b.mp4" })).with_expected_rev(Rev(1));
    let error = e.invoke(&a, stale).unwrap_err().to_wire();
    assert_eq!(
        (error["reason"].as_str(), error["current_rev"].as_u64()),
        (Some("stale_rev"), Some(2))
    );
    let fresh = Request::new("media.import", json!({ "path": "/b.mp4" })).with_expected_rev(Rev(2));
    assert_eq!(e.invoke(&a, fresh).unwrap().rev, Rev(3));
}

#[test]
fn dry_run_returns_result_without_applying() {
    let mut e = engine();
    let a = agent();
    let (_, item) = setup_clip(&mut e, &a);
    let before = e.doc().clone();
    let rev = e.rev();
    let dry = Request::new("clip.split", json!({ "item": item, "at": secs(4) })).dry_run();
    let resp = e.invoke(&a, dry).unwrap();
    assert!(resp.dry_run);
    assert!(resp.result["right"].is_string());
    assert_eq!(e.doc(), &before);
    assert_eq!(e.rev(), rev);
    assert_eq!(e.undo_depth(a.actor()), 2, "dry runs add no history");
}

#[test]
fn batch_is_atomic_and_one_undo_step() {
    let mut e = engine();
    let a = agent();
    let (media, item) = setup_clip(&mut e, &a);
    let initial = e.doc().clone();
    let steps = |second_at: Value| Batch {
        steps: vec![
            Step {
                name: "clip.split".into(),
                params: json!({ "item": item, "at": secs(4) }),
            },
            Step {
                name: "clip.split".into(),
                params: json!({ "item": item, "at": second_at }),
            },
            Step {
                name: "timeline.insert".into(),
                params: json!({ "media": media, "at": secs(20) }),
            },
            Step {
                name: "timeline.get".into(),
                params: json!({}),
            },
        ],
        request_id: None,
        expected_rev: None,
        dry_run: false,
    };
    // Second split is outside the (now shorter) left part: the whole batch rolls back.
    let error = e.execute(&a, steps(json!(secs(6)))).unwrap_err().to_wire();
    assert_eq!(error["path"], "/steps/1/params/at");
    assert_eq!(e.doc(), &initial);

    let resp = e.execute(&a, steps(json!(secs(2)))).unwrap();
    assert_eq!(resp.results.len(), 4);
    assert_eq!(
        resp.results[3]["tracks"][0]["items"]
            .as_array()
            .map(Vec::len),
        Some(4),
        "queries see in-tx state"
    );
    assert_eq!(e.doc().items.len(), 4);
    e.undo(&a).unwrap();
    assert_eq!(e.doc(), &initial, "one undo reverts the whole batch");
}

#[test]
fn batch_rejects_engine_ops_and_non_undoable_commands() {
    let mut e = engine();
    let a = agent();
    for (i, name) in ["edit.undo", "project.open"].into_iter().enumerate() {
        let batch = Batch {
            steps: vec![
                Step {
                    name: "media.import".into(),
                    params: json!({ "path": "/a.mp4" }),
                },
                Step {
                    name: name.into(),
                    params: json!({ "name": "x" }),
                },
            ],
            request_id: None,
            expected_rev: None,
            dry_run: false,
        };
        let error = e.execute(&a, batch).unwrap_err().to_wire();
        assert_eq!(error["path"], "/steps/1/name", "case {i}: {error}");
    }
    assert!(e.doc().media.is_empty());
}

#[test]
fn undo_is_per_actor_and_refuses_to_clobber_later_edits() {
    let mut e = engine();
    let user = Caller::new(Actor::User);
    let bot = agent();
    let (_, item) = setup_clip(&mut e, &user);
    let before_bot = e.doc().clone();
    ok(&mut e, &bot, "media.import", json!({ "path": "/b.wav" }));
    ok(
        &mut e,
        &user,
        "clip.trim",
        json!({ "item": item, "end": secs(8) }),
    );
    let after_user_trim = e.doc().clone();

    // The agent's undo reverts only the agent's import, not the user's later trim.
    let step = ok(&mut e, &bot, "edit.undo", json!({}));
    assert_eq!(step["step"]["label"], "media.import");
    assert_eq!(e.doc().media.len(), 1);
    assert_eq!(e.doc().items, after_user_trim.items);

    // The agent splits the user's clip; the user's trim can no longer be undone safely.
    ok(
        &mut e,
        &bot,
        "clip.split",
        json!({ "item": item, "at": secs(4) }),
    );
    let snapshot = e.doc().clone();
    let conflict = err(&mut e, &user, "edit.undo", json!({}));
    assert_eq!(conflict["reason"], "history_target_modified", "{conflict}");
    assert_eq!(e.doc(), &snapshot, "failed undo changes nothing");

    // Once the agent undoes its split, the user's history is usable again.
    ok(&mut e, &bot, "edit.undo", json!({}));
    assert_eq!(
        ok(&mut e, &user, "edit.undo", json!({}))["step"]["label"],
        "clip.trim"
    );
    assert_eq!(e.doc().items, before_bot.items);
    ok(&mut e, &user, "edit.undo", json!({}));
    ok(&mut e, &user, "edit.undo", json!({}));
    assert!(e.doc().items.is_empty() && e.doc().media.is_empty());
    assert_eq!(
        ok(&mut e, &user, "edit.undo", json!({}))["step"],
        Value::Null,
        "nothing left"
    );
}

#[test]
fn redo_reapplies_and_new_edits_clear_redo() {
    let mut e = engine();
    let a = agent();
    let (_, item) = setup_clip(&mut e, &a);
    ok(
        &mut e,
        &a,
        "clip.split",
        json!({ "item": item, "at": secs(4) }),
    );
    let split = e.doc().clone();
    ok(&mut e, &a, "edit.undo", json!({}));
    ok(&mut e, &a, "edit.redo", json!({}));
    assert_eq!(e.doc(), &split);
    ok(&mut e, &a, "edit.undo", json!({}));
    ok(&mut e, &a, "media.import", json!({ "path": "/c.png" }));
    assert_eq!(ok(&mut e, &a, "edit.redo", json!({}))["step"], Value::Null);
}

#[test]
fn project_open_is_not_undoable_and_clears_history() {
    let mut e = engine();
    let a = agent();
    setup_clip(&mut e, &a);
    let out = ok(
        &mut e,
        &a,
        "project.open",
        json!({ "name": "Fresh", "width": 1280, "height": 720 }),
    );
    assert!(e.doc().items.is_empty() && e.doc().media.is_empty());
    assert_eq!(e.doc().sequence.tracks.len(), 1);
    assert_eq!(e.doc().sequence.id.as_str(), out["sequence"]);
    assert_eq!(e.undo_depth(a.actor()), 0);
    assert_eq!(ok(&mut e, &a, "edit.undo", json!({}))["step"], Value::Null);
}

#[test]
fn overlapping_insert_creates_a_track_and_overlapping_trim_conflicts() {
    let mut e = engine();
    let a = agent();
    let (media, _) = setup_clip(&mut e, &a);
    let second = ok(
        &mut e,
        &a,
        "timeline.insert",
        json!({ "media": media, "at": secs(5) }),
    );
    assert_eq!(second["created_track"], true);
    assert_eq!(e.doc().sequence.tracks.len(), 2);
    assert_eq!(
        e.doc().sequence.tracks[0].as_str(),
        second["track"],
        "new track above"
    );
    let third = ok(
        &mut e,
        &a,
        "timeline.insert",
        json!({ "media": media, "at": secs(10), "source_in": secs(2), "duration": secs(2) }),
    );
    assert_eq!(
        third["created_track"], false,
        "main track is free after 10 s"
    );
    // Extending the third clip's head back to 9 s stays within its media but overlaps the first.
    let conflict = err(
        &mut e,
        &a,
        "clip.trim",
        json!({ "item": third["item"], "start": secs(9) }),
    );
    assert_eq!(
        (conflict["code"].as_str(), conflict["reason"].as_str()),
        (Some("conflict"), Some("overlap"))
    );
    assert!(e.doc().check_invariants().is_ok());
}

#[test]
fn doc_patched_events_are_streamed() {
    let mut e = engine();
    let rx = e.subscribe();
    let a = agent();
    let (_, item) = setup_clip(&mut e, &a);
    ok(
        &mut e,
        &a,
        "clip.split",
        json!({ "item": item, "at": secs(3) }),
    );
    ok(&mut e, &a, "timeline.get", json!({}));
    ok(&mut e, &a, "edit.undo", json!({}));
    let events: Vec<Event> = rx.try_iter().collect();
    let summary: Vec<(u64, String, String, usize)> = events
        .iter()
        .map(
            |Event::DocPatched {
                 rev,
                 actor,
                 label,
                 ops,
             }| (rev.0, actor.clone(), label.clone(), ops.len()),
        )
        .collect();
    assert_eq!(
        summary,
        vec![
            (1, "agent:test".into(), "media.import".into(), 1),
            (2, "agent:test".into(), "timeline.insert".into(), 1),
            (3, "agent:test".into(), "clip.split".into(), 2),
            (4, "agent:test".into(), "undo clip.split".into(), 2),
        ]
    );
    let wire = serde_json::to_value(&events[2]).unwrap();
    assert_eq!(wire["event"], "doc.patched");
    assert_eq!(wire["ops"][0]["entity"], "item");
}

#[test]
fn deterministic_ids_give_byte_identical_documents() {
    let script = |e: &mut Engine| {
        let a = agent();
        let (_, item) = setup_clip(e, &a);
        ok(e, &a, "clip.split", json!({ "item": item, "at": secs(3) }));
        serde_json::to_string(e.doc()).unwrap()
    };
    let _ = s5_ops::registry();
    let one = script(&mut Engine::new(EngineConfig::deterministic(7)));
    let two = script(&mut Engine::new(EngineConfig::deterministic(7)));
    let other = script(&mut Engine::new(EngineConfig::deterministic(8)));
    assert_eq!(one, two);
    assert_ne!(one, other);
}

#[test]
fn queries_do_not_bump_rev_or_history() {
    let mut e = engine();
    let a = agent();
    setup_clip(&mut e, &a);
    let rev = e.rev();
    ok(&mut e, &a, "timeline.get", json!({}));
    ok(&mut e, &a, "render.frame", json!({ "at": 0 }));
    assert_eq!(e.rev(), rev);
    assert_eq!(e.undo_depth(a.actor()), 2);
}

#[test]
fn ui_only_scope_needs_the_ui_credential() {
    // arch §6: agent credentials can never use UI-only scopes, whatever the path.
    let mut e = engine();
    let params = json!({ "provider": "anthropic", "key": "sk-test-not-a-real-key" });
    for caller in [
        agent(),
        Caller::new(Actor::Plugin("p".into())),
        Caller::new(Actor::System),
    ] {
        let error = err(&mut e, &caller, "ai.set_key", params.clone());
        assert_eq!(error["code"], "forbidden", "{error}");
        assert_eq!(error["scope"], "ui");
        let batch = Batch {
            steps: vec![Step {
                name: "ai.set_key".into(),
                params: params.clone(),
            }],
            request_id: None,
            expected_rev: None,
            dry_run: false,
        };
        assert_eq!(
            e.execute(&caller, batch).unwrap_err().to_wire()["code"],
            "forbidden"
        );
    }
    assert!(!e.has_secret("anthropic"));

    let out = ok(&mut e, &Caller::user(), "ai.set_key", params);
    assert_eq!(out, json!({ "provider": "anthropic", "stored": true }));
    assert!(e.has_secret("anthropic"));
    assert!(
        !out.to_string().contains("sk-test"),
        "the key is never echoed"
    );
}

#[test]
fn batch_steps_reference_earlier_results_with_ref() {
    let mut e = engine();
    let a = agent();
    let batch = |second: Value| Batch {
        steps: vec![
            Step {
                name: "media.import".into(),
                params: json!({ "path": "/footage/a.mp4" }),
            },
            Step {
                name: "timeline.insert".into(),
                params: second,
            },
            Step {
                name: "clip.split".into(),
                params: json!({ "item": { "$ref": "#/steps/1/item" }, "at": "2s" }),
            },
        ],
        request_id: None,
        expected_rev: None,
        dry_run: false,
    };
    // Forward and dangling references are `invalid` at the `$ref` itself; nothing is applied.
    let forward = e
        .execute(
            &a,
            batch(json!({ "media": { "$ref": "#/steps/2/left" }, "at": 0 })),
        )
        .unwrap_err()
        .to_wire();
    assert_eq!(forward["path"], "/steps/1/params/media/$ref", "{forward}");
    let dangling = e
        .execute(
            &a,
            batch(json!({ "media": { "$ref": "#/steps/0/nope" }, "at": 0 })),
        )
        .unwrap_err()
        .to_wire();
    assert_eq!(dangling["code"], "invalid");
    assert_eq!(e.rev(), Rev(0));

    let resp = e
        .execute(
            &a,
            batch(json!({ "media": { "$ref": "#/steps/0/media" }, "at": 0 })),
        )
        .unwrap();
    assert_eq!(resp.results[1]["item"], resp.results[2]["left"]);
    assert_eq!(e.doc().items.len(), 2);
    assert_eq!(
        e.undo_depth(a.actor()),
        1,
        "one transaction, one undo entry"
    );
}
