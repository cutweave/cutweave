// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1E.03: `export.start` validation and planning: the sequence, the frame-aligned range,
//! the preset and custom settings, the output path rules (absolute `.mp4`, allowlist, never a
//! source file, existing folder, `overwrite`) and the hand-off to the engine's starter. The
//! pipeline itself (render, mix, encode) is tested in `cutweave-engine`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use common::{H, P, assert_code, assert_invalid, insert_av};
use cutweave_commands::{Access, CmdError, DocSnapshot, Rev};
use cutweave_jobs::JobId;
use cutweave_model::{Duration, FrameRate, Time};
use cutweave_ops::export::{ExportCtx, ExportHw, ExportPlan, ExportQuality, ExportStarter};
use cutweave_ops::media::AllowedRoots;
use serde_json::{Value, json};

const SEC: i64 = 705_600_000;

/// A temp folder removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cutweave-ops-export-{}-{}",
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

/// Records the plans it is given.
struct Recorder {
    plans: Arc<Mutex<Vec<ExportPlan>>>,
    fail: Option<CmdError>,
}

impl ExportStarter for Recorder {
    fn start(&mut self, plan: ExportPlan) -> Result<JobId, CmdError> {
        if let Some(e) = &self.fail {
            return Err(e.clone());
        }
        self.plans.lock().unwrap().push(plan);
        Ok(JobId::from_u128(7))
    }
}

/// Runs `export.start` against the harness's document.
fn export(
    h: &H,
    params: Value,
    roots: Option<AllowedRoots>,
) -> (Result<Value, CmdError>, Vec<ExportPlan>) {
    let plans = Arc::new(Mutex::new(Vec::new()));
    let mut ctx = ExportCtx {
        snapshot: Arc::new(DocSnapshot {
            rev: Rev(5),
            committed_rev: Rev(5),
            doc: h.doc(),
        }),
        allowed_roots: roots,
        starter: Box::new(Recorder {
            plans: Arc::clone(&plans),
            fail: None,
        }),
    };
    let out = h
        .registry
        .invoke_engine_json("export.start", params, &mut ctx, Access::STANDARD);
    let plans = plans.lock().unwrap().clone();
    (out, plans)
}

/// A 1080p30 project with a 3 s and a 2 s clip back to back (5 s).
fn project() -> (H, P) {
    let (mut h, p) = H::project();
    let a = h.add_video("/media/a.mp4", 3, true);
    let b = h.add_video("/media/b.mp4", 2, true);
    insert_av(&mut h, p, a, "0");
    insert_av(&mut h, p, b, "3s");
    (h, p)
}

fn secs(s: i64) -> Time {
    Time::from_secs(s).unwrap()
}

#[test]
fn the_default_preset_exports_the_whole_sequence_at_1080p() {
    let (h, p) = project();
    let dir = Dir::new();
    let path = dir.file("out.mp4");
    let (out, plans) = export(&h, json!({ "path": path }), None);
    let out = out.unwrap();
    assert_eq!(out["job_id"], json!(JobId::from_u128(7)));
    assert_eq!(out["path"], json!(path));
    assert_eq!(out["sequence"], json!(p.sequence));
    assert_eq!(out["in"], json!(0));
    assert_eq!(out["out"], json!(5 * SEC));
    assert_eq!(
        (out["width"].clone(), out["height"].clone()),
        (json!(1920), json!(1080))
    );
    assert_eq!(out["frame_rate"], json!({"num": 30, "den": 1}));
    assert_eq!(out["frames"], json!(150));
    assert_eq!(out["duration"], json!(5 * SEC));
    assert_eq!(out["rev"], json!(5));

    let [plan] = plans.as_slice() else {
        panic!("one plan expected: {plans:?}")
    };
    assert_eq!(plan.sequence, p.sequence);
    assert_eq!(plan.path, PathBuf::from(&path));
    assert_eq!((plan.start, plan.end), (Time::ZERO, secs(5)));
    assert_eq!(
        (plan.width, plan.height, plan.frame_rate),
        (1920, 1080, FrameRate::FPS_30)
    );
    assert_eq!(plan.frames, 150);
    assert_eq!(plan.duration, Duration::from_secs(5).unwrap());
    assert_eq!(plan.quality, ExportQuality::High);
    assert_eq!(plan.hw, ExportHw::Auto);
    assert!(!plan.overwrite);
    assert_eq!(plan.snapshot.doc, h.doc(), "the plan carries the snapshot");
    assert_eq!(plan.time_of(0).unwrap(), Time::ZERO);
    assert_eq!(plan.time_of(30).unwrap(), secs(1));
    // Nothing was written next to the output (the writability probe cleans up).
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[test]
fn ranges_snap_to_frames_and_stay_inside_the_sequence() {
    let (h, _) = project();
    let dir = Dir::new();
    let path = dir.file("out.mp4");
    let (out, plans) = export(
        &h,
        json!({ "path": path, "range": {"in": "1.01s", "out": "f:75"} }),
        None,
    );
    out.unwrap();
    assert_eq!(
        (plans[0].start, plans[0].end),
        (secs(1), Time::from_flicks(SEC * 5 / 2).unwrap())
    );
    assert_eq!(plans[0].frames, 45);
    assert_eq!(
        plans[0].time_of(1).unwrap(),
        Time::from_flicks(SEC + SEC / 30).unwrap()
    );

    let range = |r: Value| export(&h, json!({ "path": path, "range": r }), None).0;
    assert_invalid(range(json!({"in": "2s", "out": "2s"})), "/range/out");
    assert_invalid(range(json!({"in": "3s", "out": "2s"})), "/range/out");
    assert_invalid(range(json!({"in": 0, "out": "6s"})), "/range/out");
    assert_invalid(range(json!({"in": "-1s", "out": "2s"})), "/range/in");
    assert_invalid(range(json!({"in": "nonsense", "out": "2s"})), "/range/in");
    assert_code(range(json!({"in": 0})), "invalid");
    // The whole sequence, spelled out, is fine.
    assert!(range(json!({"in": 0, "out": "5s"})).is_ok());
}

#[test]
fn custom_settings_override_the_sequence_format() {
    let (h, _) = project();
    let dir = Dir::new();
    let path = dir.file("out.mp4");
    let (out, plans) = export(
        &h,
        json!({
            "path": path,
            "preset": {"custom": {"width": 1280, "height": 720, "fps": {"num": 25, "den": 1}, "quality": "low"}},
            "hw": "off",
        }),
        None,
    );
    out.unwrap();
    let plan = &plans[0];
    assert_eq!(
        (plan.width, plan.height, plan.frame_rate),
        (1280, 720, FrameRate::FPS_25)
    );
    assert_eq!(plan.frames, 125);
    assert_eq!((plan.quality, plan.hw), (ExportQuality::Low, ExportHw::Off));

    // Absent fields come from the sequence.
    let (_, plans) = export(
        &h,
        json!({ "path": path, "preset": {"custom": {"quality": "medium"}} }),
        None,
    );
    let plan = &plans[0];
    assert_eq!(
        (plan.width, plan.height, plan.frame_rate),
        (1920, 1080, FrameRate::FPS_30)
    );
    assert_eq!(plan.quality, ExportQuality::Medium);

    // A 23.976 export of a 30 fps sequence covers the range with whole frames.
    let (_, plans) = export(
        &h,
        json!({ "path": path, "preset": {"custom": {"fps": {"num": 24000, "den": 1001}}} }),
        None,
    );
    assert_eq!(plans[0].frames, 120, "ceil(5 s x 23.976)");

    let custom = |c: Value| export(&h, json!({ "path": path, "preset": {"custom": c} }), None).0;
    assert_invalid(custom(json!({"width": 1281})), "/preset/custom/width");
    assert_invalid(custom(json!({"width": 8})), "/preset/custom/width");
    assert_invalid(custom(json!({"height": 10_000})), "/preset/custom/height");
    assert_invalid(
        custom(json!({"fps": {"num": 1000, "den": 1}})),
        "/preset/custom/fps",
    );
    assert_invalid(
        custom(json!({"fps": {"num": 11, "den": 1}})),
        "/preset/custom/fps",
    );
    assert_code(custom(json!({"quality": "best"})), "invalid");
    assert_code(
        export(&h, json!({ "path": path, "preset": "h265-mkv" }), None).0,
        "invalid",
    );
    assert!(
        export(
            &h,
            json!({ "path": path, "preset": "h264-mp4-1080p" }),
            None
        )
        .0
        .is_ok()
    );
}

#[test]
fn the_sequence_is_named_or_the_only_one() {
    let dir = Dir::new();
    let path = dir.file("out.mp4");
    let (h, p) = project();
    assert!(
        export(&h, json!({ "path": path, "sequence": p.sequence }), None)
            .0
            .is_ok()
    );
    assert_code(
        export(
            &h,
            json!({ "path": path, "sequence": "seq_00000000000000000000000000000099" }),
            None,
        )
        .0,
        "not_found",
    );
    // No sequence at all, and an empty one.
    assert_invalid(
        export(&H::empty(), json!({ "path": path }), None).0,
        "/sequence",
    );
    let (empty, _) = H::project();
    assert_invalid(export(&empty, json!({ "path": path }), None).0, "/sequence");
}

#[test]
fn output_paths_are_checked_before_anything_is_queued() {
    let (mut h, p) = project();
    let dir = Dir::new();
    let run = |h: &H, params: Value| export(h, params, None);

    assert_invalid(run(&h, json!({ "path": "out.mp4" })).0, "/path");
    assert_invalid(run(&h, json!({ "path": dir.file("out.mov") })).0, "/path");
    assert_invalid(run(&h, json!({ "path": dir.file(".mp4") })).0, "/path");
    assert_invalid(
        run(&h, json!({ "path": dir.file("missing/out.mp4") })).0,
        "/path",
    );
    let sub = dir.0.join("sub");
    std::fs::create_dir_all(sub.join("x.mp4")).unwrap();
    assert_invalid(
        run(
            &h,
            json!({ "path": sub.join("..").join("y.mp4").to_string_lossy() }),
        )
        .0,
        "/path",
    );
    assert_invalid(
        run(
            &h,
            json!({ "path": sub.join("x.mp4").to_string_lossy(), "overwrite": true }),
        )
        .0,
        "/path",
    );

    // An existing file needs `overwrite: true`.
    let existing = dir.file("existing.mp4");
    std::fs::write(&existing, b"old").unwrap();
    let (r, plans) = run(&h, json!({ "path": existing }));
    assert_invalid(r, "/overwrite");
    assert!(plans.is_empty());
    let (r, plans) = run(&h, json!({ "path": existing, "overwrite": true }));
    r.unwrap();
    assert!(plans[0].overwrite);
    assert_eq!(
        std::fs::read(&existing).unwrap(),
        b"old",
        "only the job replaces it"
    );

    // Never a source file of the project, even with overwrite.
    let source = dir.file("source.mp4");
    std::fs::write(&source, b"media").unwrap();
    let m = h.add_video(&source, 1, false);
    h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": m, "at": "5s"}),
    );
    assert_invalid(
        run(&h, json!({ "path": source, "overwrite": true })).0,
        "/path",
    );
    if cfg!(windows) {
        let upper = source.to_uppercase();
        assert_invalid(
            run(&h, json!({ "path": upper, "overwrite": true })).0,
            "/path",
        );
    }
    assert_eq!(std::fs::read(&source).unwrap(), b"media");
}

#[test]
fn agents_write_only_inside_their_allowed_folders() {
    let (h, _) = project();
    let allowed = Dir::new();
    let elsewhere = Dir::new();
    let roots = || Some(AllowedRoots::new([allowed.0.clone()]));
    assert!(
        export(&h, json!({ "path": allowed.file("a.mp4") }), roots())
            .0
            .is_ok()
    );
    assert_invalid(
        export(&h, json!({ "path": elsewhere.file("a.mp4") }), roots()).0,
        "/path",
    );
    // The UI (no roots) writes anywhere.
    assert!(
        export(&h, json!({ "path": elsewhere.file("a.mp4") }), None)
            .0
            .is_ok()
    );
}

#[test]
fn starter_errors_and_unknown_params_reach_the_caller() {
    let (h, _) = project();
    let dir = Dir::new();
    let mut ctx = ExportCtx {
        snapshot: Arc::new(DocSnapshot {
            rev: Rev(1),
            committed_rev: Rev(1),
            doc: h.doc(),
        }),
        allowed_roots: None,
        starter: Box::new(Recorder {
            plans: Arc::default(),
            fail: Some(CmdError::unsupported("no media backend")),
        }),
    };
    let r = h.registry.invoke_engine_json(
        "export.start",
        json!({ "path": dir.file("a.mp4") }),
        &mut ctx,
        Access::STANDARD,
    );
    assert_code(r, "unsupported");
    assert_code(
        export(&h, json!({ "path": dir.file("a.mp4"), "fps": 30 }), None).0,
        "invalid",
    );
    assert_code(export(&h, json!({}), None).0, "invalid");
    assert!(Path::new(&dir.file("a.mp4")).parent().unwrap().is_dir());
}
