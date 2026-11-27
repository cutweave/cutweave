// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.03c: `export.start` and `export.enqueue` refuse an export that would play missing
//! media (`Invalid` at `/media/<id>`, the files named in the message) unless `allow_offline`
//! is true, and the queue keeps the choice with the job.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use common::{H, P, insert_av};
use cutweave_commands::{Access, CmdError, DocSnapshot, Rev};
use cutweave_jobs::JobId;
use cutweave_jobs::queue::TaskCodec;
use cutweave_model::{MediaAsset, MediaId};
use cutweave_ops::export::queue::PlanCodec;
use cutweave_ops::export::{ExportCtx, ExportPlan, ExportStarter};
use serde_json::{Value, json};

/// A temp folder removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cw-ops-offline-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn file(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Records the plans it is given; the files in `gone` are not on its disk.
struct Recorder {
    plans: Arc<Mutex<Vec<ExportPlan>>>,
    gone: Vec<String>,
    asked: Arc<Mutex<Vec<MediaId>>>,
}

impl ExportStarter for Recorder {
    fn start(&mut self, plan: ExportPlan) -> Result<JobId, CmdError> {
        self.plans.lock().unwrap().push(plan);
        Ok(JobId::from_u128(7))
    }

    fn media_offline(&self, asset: &MediaAsset) -> bool {
        self.asked.lock().unwrap().push(asset.id);
        asset
            .path_abs
            .as_ref()
            .is_some_and(|p| self.gone.contains(p))
    }
}

struct Run {
    out: Result<Value, CmdError>,
    plans: Vec<ExportPlan>,
    asked: Vec<MediaId>,
}

/// Runs `command` against the harness's document on a disk without the files `gone`.
fn export(h: &H, command: &str, params: Value, gone: &[&str]) -> Run {
    let plans = Arc::new(Mutex::new(Vec::new()));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut ctx = ExportCtx {
        snapshot: Arc::new(DocSnapshot {
            rev: Rev(5),
            committed_rev: Rev(5),
            doc: h.doc(),
        }),
        allowed_roots: None,
        presets: Arc::default(),
        starter: Box::new(Recorder {
            plans: Arc::clone(&plans),
            gone: gone.iter().map(|&p| p.to_owned()).collect(),
            asked: Arc::clone(&asked),
        }),
    };
    let out = h
        .registry
        .invoke_engine_json(command, params, &mut ctx, Access::STANDARD);
    let plans = plans.lock().unwrap().clone();
    let asked = asked.lock().unwrap().clone();
    Run { out, plans, asked }
}

struct Fixture {
    h: H,
    p: P,
    a: MediaId,
    b: MediaId,
}

/// A 1080p30 project with a 3 s clip of `a.mp4` and a 2 s clip of `b.mp4` back to back, and
/// `unused.mp4` in the bin only.
fn project() -> Fixture {
    let (mut h, p) = H::project();
    let a = h.add_video("/media/a.mp4", 3, true);
    let b = h.add_video("/media/b.mp4", 2, true);
    h.add_video("/media/unused.mp4", 2, true);
    insert_av(&mut h, p, a, "0");
    insert_av(&mut h, p, b, "3s");
    Fixture { h, p, a, b }
}

fn invalid(out: Result<Value, CmdError>) -> (String, String) {
    match out {
        Err(CmdError::Invalid { path, msg }) => (path, msg),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn missing_media_refuses_the_export_and_names_the_asset() {
    let f = project();
    let dir = Dir::new();
    for command in ["export.start", "export.enqueue"] {
        let run = export(
            &f.h,
            command,
            json!({ "path": dir.file("out.mp4") }),
            &["/media/b.mp4"],
        );
        let (path, msg) = invalid(run.out);
        assert_eq!(path, format!("/media/{}", f.b), "{command}");
        assert_eq!(
            msg,
            "1 media file is missing: `b.mp4`; relink them (media.relink_folder, media.relink) \
             or pass `allow_offline: true` to export a \"media offline\" slate in their place",
            "{command}"
        );
        assert!(run.plans.is_empty(), "{command}: nothing is queued");
        // `allow_offline: false` is the default spelled out.
        let run = export(
            &f.h,
            command,
            json!({ "path": dir.file("out.mp4"), "allow_offline": false }),
            &["/media/b.mp4"],
        );
        assert_eq!(invalid(run.out).0, format!("/media/{}", f.b), "{command}");
    }
}

#[test]
fn every_missing_file_is_named_and_the_path_is_the_first_by_id() {
    let f = project();
    let dir = Dir::new();
    let run = export(
        &f.h,
        "export.start",
        json!({ "path": dir.file("out.mp4") }),
        &["/media/a.mp4", "/media/b.mp4"],
    );
    let (path, msg) = invalid(run.out);
    let first = f.a.min(f.b);
    assert_eq!(path, format!("/media/{first}"));
    let (x, y) = if first == f.a {
        ("a.mp4", "b.mp4")
    } else {
        ("b.mp4", "a.mp4")
    };
    assert!(
        msg.starts_with(&format!("2 media files are missing: `{x}`, `{y}`; ")),
        "{msg}"
    );
}

#[test]
fn allow_offline_exports_and_the_plan_says_so() {
    let f = project();
    let dir = Dir::new();
    let run = export(
        &f.h,
        "export.start",
        json!({ "path": dir.file("out.mp4"), "allow_offline": true }),
        &["/media/b.mp4"],
    );
    let out = run.out.unwrap();
    assert_eq!(out["frames"], json!(150));
    let [plan] = run.plans.as_slice() else {
        panic!("one plan expected")
    };
    assert!(plan.allow_offline);
    assert!(run.asked.is_empty(), "the disk is not asked when allowed");

    // Nothing missing: the default plan refuses media that goes missing later.
    let run = export(
        &f.h,
        "export.start",
        json!({ "path": dir.file("ok.mp4") }),
        &[],
    );
    run.out.unwrap();
    assert!(!run.plans[0].allow_offline);
}

#[test]
fn only_media_the_exported_range_plays_counts() {
    let f = project();
    let dir = Dir::new();
    // The bin's unused asset is never asked about.
    let run = export(
        &f.h,
        "export.start",
        json!({ "path": dir.file("bin.mp4") }),
        &["/media/unused.mp4"],
    );
    run.out.unwrap();
    let mut asked = run.asked.clone();
    asked.sort();
    asked.dedup();
    let mut used = vec![f.a, f.b];
    used.sort();
    assert_eq!(asked, used, "each used asset is asked about once");
    assert_eq!(run.asked.len(), 2, "once per asset, not once per clip");

    // `b` plays from 3 s on: the first two seconds export without it.
    let run = export(
        &f.h,
        "export.start",
        json!({ "path": dir.file("head.mp4"), "range": {"kind": "custom", "in": "0", "out": "2s"} }),
        &["/media/b.mp4"],
    );
    run.out.unwrap();
    let run = export(
        &f.h,
        "export.start",
        json!({ "path": dir.file("tail.mp4"), "range": {"kind": "custom", "in": "2s", "out": "4s"} }),
        &["/media/b.mp4"],
    );
    assert_eq!(invalid(run.out).0, format!("/media/{}", f.b));
}

#[test]
fn disabled_clips_do_not_count() {
    let mut f = project();
    let dir = Dir::new();
    let doc = f.h.doc();
    let seq = &doc.sequences[&f.p.sequence];
    // Every clip of `b` (picture and linked sound) is disabled.
    let clips: Vec<_> = seq
        .tracks
        .values()
        .flat_map(|t| t.items.iter())
        .filter(|i| i.media() == Some(f.b))
        .map(cutweave_model::Item::id)
        .collect();
    assert_eq!(clips.len(), 2);
    for item in clips {
        f.h.ok(
            "item.set_props",
            json!({ "item": item, "props": {"enabled": false} }),
        );
    }
    let run = export(
        &f.h,
        "export.start",
        json!({ "path": dir.file("out.mp4") }),
        &["/media/b.mp4"],
    );
    run.out.unwrap();
}

#[test]
fn a_parameter_error_is_reported_before_the_missing_media() {
    let f = project();
    let run = export(
        &f.h,
        "export.start",
        json!({ "path": "relative/out.mp4" }),
        &["/media/b.mp4"],
    );
    assert_eq!(invalid(run.out).0, "/path");
}

#[test]
fn a_stored_job_keeps_the_choice_and_older_jobs_refuse() {
    let f = project();
    let dir = Dir::new();
    let run = export(
        &f.h,
        "export.start",
        json!({ "path": dir.file("out.mp4"), "allow_offline": true }),
        &["/media/b.mp4"],
    );
    let plan = &run.plans[0];
    let bytes = PlanCodec.encode(plan).unwrap();
    assert!(PlanCodec.decode(&bytes).unwrap().allow_offline);

    // A job stored before the field existed.
    let mut stored: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(stored["allow_offline"], json!(true));
    stored.as_object_mut().unwrap().remove("allow_offline");
    let old = serde_json::to_vec(&stored).unwrap();
    assert!(!PlanCodec.decode(&old).unwrap().allow_offline);
}
