// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2G.03b: export ranges (sequence, in/out, custom, marker sets), the output file collision
//! policy and the disk space precheck, against a recording starter whose free-space probe and
//! queue claims are injected (no disk is filled).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use common::{H, P, assert_invalid, insert_av};
use cutweave_commands::{Access, CmdError, DocSnapshot, Rev};
use cutweave_jobs::JobId;
use cutweave_ops::export::{ExportCtx, ExportPlan, ExportStarter};
use serde_json::{Value, json};

const SEC: i64 = 705_600_000;

struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cutweave-ops-ranges-{}-{}",
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

    fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What the harness injects into the starter.
#[derive(Default)]
struct Env {
    plans: Vec<ExportPlan>,
    free: Option<u64>,
    claimed: Vec<PathBuf>,
    fail_at: Option<usize>,
}

struct Recorder(Arc<Mutex<Env>>);

impl Recorder {
    fn add(&mut self, plan: ExportPlan) -> Result<JobId, CmdError> {
        let mut env = self.0.lock().unwrap();
        if env.fail_at == Some(env.plans.len()) {
            return Err(CmdError::unsupported("no more room in the queue"));
        }
        env.plans.push(plan);
        Ok(JobId::from_u128(env.plans.len() as u128))
    }
}

impl ExportStarter for Recorder {
    fn start(&mut self, plan: ExportPlan) -> Result<JobId, CmdError> {
        self.add(plan)
    }

    fn output_claimed(&self, path: &Path) -> bool {
        self.0.lock().unwrap().claimed.iter().any(|c| c == path)
    }

    fn free_space(&self, _folder: &Path) -> Option<u64> {
        self.0.lock().unwrap().free
    }
}

fn run(h: &H, name: &str, params: Value, env: &Arc<Mutex<Env>>) -> Result<Value, CmdError> {
    let mut ctx = ExportCtx {
        snapshot: Arc::new(DocSnapshot {
            rev: Rev(5),
            committed_rev: Rev(5),
            doc: h.doc(),
        }),
        allowed_roots: None,
        presets: Arc::default(),
        starter: Box::new(Recorder(Arc::clone(env))),
    };
    h.registry
        .invoke_engine_json(name, params, &mut ctx, Access::STANDARD)
}

fn export(h: &H, params: Value) -> (Result<Value, CmdError>, Vec<ExportPlan>) {
    let env = Arc::new(Mutex::new(Env::default()));
    let out = run(h, "export.start", params, &env);
    let plans = env.lock().unwrap().plans.clone();
    (out, plans)
}

/// A 1080p30 project, 5 s of picture, with markers at 0 s, 1 s, 2.5 s and 4 s.
fn project() -> (H, P) {
    let (mut h, p) = H::project();
    let a = h.add_video("/media/a.mp4", 5, true);
    insert_av(&mut h, p, a, "0");
    (h, p)
}

fn marker(h: &mut H, p: P, t: &str, duration: Option<&str>, label: &str) -> Value {
    let mut params = json!({"sequence": p.sequence, "t": t, "label": label});
    if let Some(d) = duration {
        params["duration"] = json!(d);
    }
    h.ok("timeline.add_marker", params)
}

fn four_markers(h: &mut H, p: P) -> Vec<Value> {
    vec![
        marker(h, p, "0", Some("1s"), "Intro"),
        marker(h, p, "1s", Some("1s"), "Main: part 1"),
        marker(h, p, "2.5s", Some("0.5s"), "Main 2"),
        marker(h, p, "4s", Some("1s"), ""),
    ]
}

fn marker_ids(h: &H, p: P) -> Vec<Value> {
    let out = h
        .query("timeline.list_markers", json!({"sequence": p.sequence}))
        .unwrap();
    out["markers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].clone())
        .collect()
}

fn seq_name(h: &H) -> String {
    h.doc().sequences.values().next().unwrap().name.clone()
}

fn frames(v: &Value) -> i64 {
    v.as_i64().unwrap()
}

// ----- range kinds -----------------------------------------------------------------------------

#[test]
fn range_kinds_pick_the_part_of_the_sequence() {
    let (mut h, p) = project();
    let dir = Dir::new();

    // Sequence and the old `{in, out}` shape.
    let (r, plans) = export(
        &h,
        json!({"path": dir.file("a.mp4"), "range": {"kind": "sequence"}}),
    );
    r.unwrap();
    assert_eq!(
        (plans[0].start.flicks(), plans[0].end.flicks()),
        (0, 5 * SEC)
    );
    let (r, plans) = export(
        &h,
        json!({"path": dir.file("a.mp4"), "range": {"in": "1s", "out": "3s"}}),
    );
    r.unwrap();
    assert_eq!(
        (plans[0].start.flicks(), plans[0].end.flicks()),
        (SEC, 3 * SEC)
    );
    assert_eq!(plans[0].frames, 60);
    let (r, plans) = export(
        &h,
        json!({"path": dir.file("a.mp4"), "range": {"kind": "custom", "start": "2s", "end": "4s"}}),
    );
    r.unwrap();
    assert_eq!(plans[0].frames, 60);

    // In and out points.
    let (r, _) = export(
        &h,
        json!({"path": dir.file("a.mp4"), "range": {"kind": "in_out"}}),
    );
    assert_invalid(r, "/range/kind");
    h.ok(
        "timeline.set_in_out",
        json!({"sequence": p.sequence, "start": "1s", "end": "2.5s"}),
    );
    let (r, plans) = export(
        &h,
        json!({"path": dir.file("a.mp4"), "range": {"kind": "in_out"}}),
    );
    r.unwrap();
    assert_eq!(
        (plans[0].start.flicks(), plans[0].end.flicks()),
        (SEC, 5 * SEC / 2)
    );
    assert_eq!(plans[0].frames, 45);
    // An out point past the end is clamped to the sequence end.
    h.ok(
        "timeline.set_in_out",
        json!({"sequence": p.sequence, "start": "4s", "end": "9s"}),
    );
    let (r, plans) = export(
        &h,
        json!({"path": dir.file("a.mp4"), "range": {"kind": "in_out"}}),
    );
    r.unwrap();
    assert_eq!(plans[0].end.flicks(), 5 * SEC);

    // Wrong combinations are pointed at.
    let bad = |range: Value| export(&h, json!({"path": dir.file("a.mp4"), "range": range})).0;
    assert_invalid(
        bad(json!({"kind": "sequence", "in": "1s", "out": "2s"})),
        "/range/in",
    );
    assert_invalid(bad(json!({"kind": "custom", "in": "1s"})), "/range/out");
    assert_invalid(bad(json!({"kind": "custom"})), "/range/in");
    assert_invalid(
        bad(json!({"kind": "custom", "in": "1s", "out": "9s"})),
        "/range/out",
    );
    assert_invalid(bad(json!({"in": "3s", "out": "2s"})), "/range/out");
    assert_invalid(
        bad(json!({"kind": "in_out", "markers": []})),
        "/range/markers",
    );
}

#[test]
fn a_marker_set_makes_one_file_per_marker_with_the_right_length() {
    let (mut h, p) = project();
    four_markers(&mut h, p);
    let dir = Dir::new();
    let (r, plans) = export(
        &h,
        json!({"path": dir.0.to_string_lossy(), "range": {"kind": "markers"}}),
    );
    let out = r.unwrap();
    assert_eq!(plans.len(), 4);
    let jobs = out["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 4);
    let seq = seq_name(&h);
    let names: Vec<String> = plans
        .iter()
        .map(|pl| pl.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec![
            format!("{seq}_Intro_1.mp4"),
            format!("{seq}_Main_ part 1_2.mp4"),
            format!("{seq}_Main 2_3.mp4"),
            format!("{seq}_marker_4.mp4"),
        ]
    );
    // Durations within a frame of the markers' (1 s, 1 s, 0.5 s, 1 s at 30 fps).
    let wanted = [30, 30, 15, 30];
    for (pl, w) in plans.iter().zip(wanted) {
        assert!(
            (i64::from(pl.frames) - w).abs() <= 1,
            "{} frames",
            pl.frames
        );
    }
    assert_eq!(plans[2].start.flicks(), 5 * SEC / 2);
    // The output lists each job in time order; the first one fills the top-level fields.
    assert_eq!(out["job_id"], jobs[0]["job_id"]);
    assert_eq!(jobs[1]["label"], json!("Main: part 1"));
    assert_eq!(frames(&jobs[2]["frames"]), i64::from(plans[2].frames));
    assert_eq!(out["in"], jobs[0]["in"]);
    let distinct: std::collections::BTreeSet<_> =
        jobs.iter().map(|j| j["job_id"].to_string()).collect();
    assert_eq!(distinct.len(), 4);
}

#[test]
fn marker_selection_modes_and_templates() {
    let (mut h, p) = project();
    four_markers(&mut h, p);
    marker(&mut h, p, "4.5s", None, "Point");
    let ids = marker_ids(&h, p);
    assert_eq!(ids.len(), 5);
    let dir = Dir::new();
    let tpl = dir.file("{n}-{marker_label}.mp4");

    // "All" skips the point marker; naming it is an error.
    let (r, plans) = export(&h, json!({"path": tpl, "range": {"kind": "markers"}}));
    r.unwrap();
    assert_eq!(plans.len(), 4);
    assert!(plans[0].path.ends_with("1-Intro.mp4"));
    let (r, _) = export(
        &h,
        json!({"path": tpl, "range": {"markers": [ids[4].clone()]}}),
    );
    assert_invalid(r, "/range/markers");

    // A chosen subset, in time order whatever the order given.
    let (r, plans) = export(
        &h,
        json!({"path": tpl, "range": {"markers": [ids[2].clone(), ids[0].clone()]}}),
    );
    r.unwrap();
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].start.flicks(), 0);
    let (r, _) = export(
        &h,
        json!({"path": tpl, "range": {"markers": [ids[0].clone(), ids[0].clone()]}}),
    );
    assert_invalid(r, "/range/markers/1");
    let (r, _) = export(&h, json!({"path": tpl, "range": {"markers": ["nope"]}}));
    assert!(r.is_err());

    // Pairs: each marker to the next, so n markers give n - 1 files.
    let (r, plans) = export(
        &h,
        json!({"path": tpl, "range": {"kind": "markers", "pair_mode": "pairs"}}),
    );
    r.unwrap();
    assert_eq!(plans.len(), 4);
    assert_eq!(plans[0].end, plans[1].start);
    assert_eq!(plans[3].end.flicks(), 9 * SEC / 2);
    let (r, _) = export(
        &h,
        json!({"path": tpl, "range": {"markers": [ids[0].clone()], "pair_mode": "pairs"}}),
    );
    assert_invalid(r, "/range/markers");

    // The file name must tell the files apart; tokens must be known.
    let (r, _) = export(
        &h,
        json!({"path": dir.file("all.mp4"), "range": {"kind": "markers"}}),
    );
    assert_invalid(r, "/path");
    let (r, _) = export(
        &h,
        json!({"path": dir.file("{n}-{what}.mp4"), "range": {"kind": "markers"}}),
    );
    assert_invalid(r, "/path");
    let (r, _) = export(
        &h,
        json!({"path": dir.file("sub/{n}.mp4"), "range": {"kind": "markers"}}),
    );
    assert_invalid(r, "/path");
    // One region may use a plain name.
    let (r, plans) = export(
        &h,
        json!({"path": dir.file("one.mp4"), "range": {"markers": [ids[0].clone()]}}),
    );
    r.unwrap();
    assert!(plans[0].path.ends_with("one.mp4"));
    // A marker set where no marker has a duration.
    let (mut h2, p2) = project();
    marker(&mut h2, p2, "1s", None, "a");
    let (r, _) = export(&h2, json!({"path": tpl, "range": {"kind": "markers"}}));
    assert_invalid(r, "/range/markers");
}

// ----- collisions ------------------------------------------------------------------------------

#[test]
fn fail_refuses_an_existing_file_at_the_path() {
    let (h, _) = project();
    let dir = Dir::new();
    let path = dir.file("out.mp4");
    std::fs::write(&path, b"old").unwrap();
    // As before the policy existed: the hint is about `overwrite`.
    assert_invalid(export(&h, json!({"path": path})).0, "/overwrite");
    let (r, plans) = export(&h, json!({"path": path, "collision": "fail"}));
    assert_invalid(r, "/path");
    assert!(plans.is_empty());
    let (r, _) = export(
        &h,
        json!({"path": path, "overwrite": false, "collision": "fail"}),
    );
    assert_invalid(r, "/path");
    assert_eq!(std::fs::read(&path).unwrap(), b"old");
    // A free name is fine.
    export(
        &h,
        json!({"path": dir.file("free.mp4"), "collision": "fail"}),
    )
    .0
    .unwrap();
}

#[test]
fn overwrite_keeps_the_file_until_the_job_replaces_it() {
    let (h, _) = project();
    let dir = Dir::new();
    let path = dir.file("out.mp4");
    std::fs::write(&path, b"old").unwrap();
    for params in [
        json!({"path": path, "overwrite": true}),
        json!({"path": path, "collision": "overwrite"}),
        json!({"path": path, "overwrite": true, "collision": "overwrite"}),
    ] {
        let (r, plans) = export(&h, params);
        r.unwrap();
        assert!(plans[0].overwrite);
        assert_eq!(plans[0].path, PathBuf::from(&path));
    }
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"old",
        "only a successful export replaces it"
    );
    // Saying both, differently, is a mistake.
    let (r, _) = export(
        &h,
        json!({"path": path, "overwrite": true, "collision": "rename"}),
    );
    assert_invalid(r, "/overwrite");
}

#[test]
fn rename_picks_a_free_name() {
    let (h, _) = project();
    let dir = Dir::new();
    let path = dir.file("out.mp4");
    let plan = |h: &H, env: &Arc<Mutex<Env>>| {
        let out = run(
            h,
            "export.start",
            json!({"path": path, "collision": "rename"}),
            env,
        )
        .unwrap();
        out["path"].as_str().unwrap().to_owned()
    };
    let env = Arc::new(Mutex::new(Env::default()));
    // A free name stays.
    assert_eq!(plan(&h, &env), path);
    std::fs::write(&path, b"x").unwrap();
    assert_eq!(plan(&h, &env), dir.file("out (1).mp4"));
    std::fs::write(dir.file("out (1).mp4"), b"x").unwrap();
    assert_eq!(plan(&h, &env), dir.file("out (2).mp4"));
    // A name another unfinished export writes is not free either.
    env.lock()
        .unwrap()
        .claimed
        .push(PathBuf::from(dir.file("out (2).mp4")));
    assert_eq!(plan(&h, &env), dir.file("out (3).mp4"));
    // The existing files are untouched and nothing else was created.
    assert_eq!(std::fs::read(&path).unwrap(), b"x");
    assert_eq!(dir.names(), vec!["out (1).mp4", "out.mp4"]);

    // A claimed name fails under `fail` and `overwrite` too.
    env.lock()
        .unwrap()
        .claimed
        .push(PathBuf::from(dir.file("busy.mp4")));
    for policy in ["fail", "overwrite"] {
        let r = run(
            &h,
            "export.start",
            json!({"path": dir.file("busy.mp4"), "collision": policy}),
            &env,
        );
        assert_invalid(r, "/path");
    }
}

#[test]
fn files_of_one_marker_set_never_share_a_name() {
    let (mut h, p) = project();
    marker(&mut h, p, "0", Some("1s"), "Take");
    marker(&mut h, p, "2s", Some("1s"), "Take");
    let dir = Dir::new();
    let tpl = dir.file("{marker_label}.mp4");
    let range = json!({"kind": "markers"});
    for policy in ["fail", "overwrite"] {
        let (r, plans) = export(
            &h,
            json!({"path": tpl, "range": range, "collision": policy}),
        );
        assert_invalid(r, "/path");
        assert!(plans.is_empty(), "nothing is queued when one file is wrong");
    }
    let (r, plans) = export(
        &h,
        json!({"path": tpl, "range": range, "collision": "rename"}),
    );
    r.unwrap();
    let names: Vec<_> = plans
        .iter()
        .map(|pl| pl.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["Take.mp4", "Take (1).mp4"]);
}

#[test]
fn a_marker_set_that_cannot_be_queued_fails_as_a_whole() {
    let (mut h, p) = project();
    four_markers(&mut h, p);
    let dir = Dir::new();
    // The second of four files exists: nothing is queued (checked before queueing).
    std::fs::write(dir.file("2.mp4"), b"x").unwrap();
    let (r, plans) = export(
        &h,
        json!({"path": dir.file("{n}.mp4"), "range": {"kind": "markers"}}),
    );
    assert_invalid(r, "/overwrite");
    assert!(plans.is_empty());
    // A queue that refuses the third: the error is the queue's.
    let env = Arc::new(Mutex::new(Env {
        fail_at: Some(2),
        ..Env::default()
    }));
    let r = run(
        &h,
        "export.enqueue",
        json!({"path": dir.file("{n}.mp4"), "range": {"kind": "markers"}, "collision": "rename"}),
        &env,
    );
    assert!(matches!(r, Err(CmdError::Unsupported { .. })), "{r:?}");
}

// ----- disk space ------------------------------------------------------------------------------

#[test]
fn too_little_free_space_is_refused_before_anything_is_queued() {
    let (mut h, p) = project();
    let dir = Dir::new();
    let path = dir.file("out.mp4");
    let env = Arc::new(Mutex::new(Env {
        free: Some(1_000_000),
        ..Env::default()
    }));
    let r = run(&h, "export.start", json!({"path": path}), &env);
    assert_invalid(r, "/path");
    assert!(env.lock().unwrap().plans.is_empty());
    // Plenty of room, and an unknown amount of room, both go through.
    env.lock().unwrap().free = Some(100_000_000_000);
    run(&h, "export.start", json!({"path": path}), &env).unwrap();
    env.lock().unwrap().free = None;
    run(&h, "export.start", json!({"path": path}), &env).unwrap();

    // A longer export needs more: a size that fits 1 s does not fit the whole 5 s.
    let one_sec = json!({"path": path, "range": {"in": "0", "out": "1s"}});
    env.lock().unwrap().free = Some(3_000_000);
    run(&h, "export.start", one_sec, &env).unwrap();
    assert_invalid(
        run(&h, "export.start", json!({"path": path}), &env),
        "/path",
    );

    // All the files of a marker set count together.
    four_markers(&mut h, p);
    let sum = json!({"path": dir.file("{n}.mp4"), "range": {"kind": "markers"}});
    env.lock().unwrap().free = Some(100_000_000);
    run(&h, "export.enqueue", sum.clone(), &env).unwrap();
    env.lock().unwrap().free = Some(3_000_000);
    assert_invalid(run(&h, "export.enqueue", sum, &env), "/path");
    // The message says what is needed and what is free.
    env.lock().unwrap().free = Some(1_000_000);
    let Err(CmdError::Invalid { msg, .. }) = run(&h, "export.start", json!({"path": path}), &env)
    else {
        panic!("expected Invalid")
    };
    assert!(msg.contains("disk space") && msg.contains("MB"), "{msg}");
}

#[test]
fn the_estimate_follows_bitrate_and_length() {
    use cutweave_ops::export::output::{estimate_bytes, human_bytes};
    let (h, _) = project();
    let dir = Dir::new();
    let plan_of = |params: Value| export(&h, params).1.remove(0);
    let whole = plan_of(json!({"path": dir.file("a.mp4")}));
    let half = plan_of(json!({"path": dir.file("a.mp4"), "range": {"in": "0", "out": "2.5s"}}));
    let (a, b) = (estimate_bytes(&whole), estimate_bytes(&half));
    assert!(a > 0 && (a / 2).abs_diff(b) <= a / 100, "{a} vs {b}");
    let low = plan_of(json!({"path": dir.file("a.mp4"), "preset": {"custom": {"quality": "low"}}}));
    assert!(estimate_bytes(&low) < a);
    assert!(!human_bytes(a).is_empty());
}
