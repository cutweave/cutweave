// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.04: `project.collect` and `project.collect_estimate`. The media are small files made at
//! test time in a temp folder; the engine's part (free space, writing the project) is a stand-in.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names
)]

mod common;

use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use common::{H, assert_invalid};
use cutweave_commands::{Access, CmdError, McpExposure};
use cutweave_jobs::{JobError, JobState};
use cutweave_model::{MediaId, Project};
use cutweave_ops::{
    media::AllowedRoots,
    project::collect::{
        CollectCtx, CollectHost, CollectMode, CollectOptions, CollectProgress, plan_collect,
        run_collect,
    },
    testing::job_manager,
};
use serde_json::{Value, json};

/// A temp folder removed when dropped.
struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!(
            "cwc-{tag}-{}-{}",
            std::process::id(),
            format!("{:?}", std::thread::current().id())
                .chars()
                .filter(char::is_ascii_digit)
                .collect::<String>()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        rel.split('/').fold(self.0.clone(), |p, part| p.join(part))
    }

    fn write(&self, rel: &str, bytes: &[u8]) -> String {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The engine's part: a drive with `free` bytes; "writing the project" records the document.
struct Host {
    free: Option<u64>,
    written: Mutex<Vec<(PathBuf, Project)>>,
}

impl Host {
    fn new(free: Option<u64>) -> Arc<Host> {
        Arc::new(Host {
            free,
            written: Mutex::new(Vec::new()),
        })
    }
}

impl CollectHost for Host {
    fn free_space(&self, _folder: &Path) -> Option<u64> {
        self.free
    }

    fn holds_project(&self, folder: &Path) -> bool {
        folder.join("Existing.cutweave").exists()
    }

    fn write_project(&self, folder: &Path, doc: &Project) -> Result<PathBuf, String> {
        fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let path = folder.join("Copy.cutweave");
        fs::write(&path, "{}").map_err(|e| e.to_string())?;
        self.written
            .lock()
            .unwrap()
            .push((folder.to_path_buf(), doc.clone()));
        Ok(path)
    }
}

/// Runs to the end, or says "cancelled" once `stop_after` bytes were reported.
struct Progress {
    stop_after: Option<u64>,
    seen: Cell<u64>,
}

impl Progress {
    fn all() -> Progress {
        Progress {
            stop_after: None,
            seen: Cell::new(0),
        }
    }

    fn until(bytes: u64) -> Progress {
        Progress {
            stop_after: Some(bytes),
            seen: Cell::new(0),
        }
    }
}

impl CollectProgress for Progress {
    fn cancelled(&self) -> bool {
        self.stop_after
            .is_some_and(|limit| self.seen.get() >= limit)
    }

    fn report(&self, done: u64, _total: u64, _file: &str) {
        self.seen.set(done);
    }
}

fn options(dest: &Path) -> CollectOptions {
    CollectOptions {
        dest: s(dest),
        mode: None,
        trim_unused: None,
        include_generated: None,
    }
}

/// A project in `<t>/Promo` with four assets: two outside it (same file name, different
/// folders), one inside it, one unused sound outside.
struct Fixture {
    t: Temp,
    h: H,
    a: MediaId,
    b: MediaId,
    inside: MediaId,
    unused: MediaId,
}

fn fixture(tag: &str) -> Fixture {
    let t = Temp::new(tag);
    let (mut h, p) = H::project();
    let a_path = t.write("Footage/day1/clip.mp4", &vec![1_u8; 3_000_000]);
    let b_path = t.write("Footage/day2/clip.mp4", &vec![2_u8; 1_500_000]);
    let in_path = t.write("Promo/assets/logo.mp4", b"logo bytes");
    let un_path = t.write("Sounds/rain.wav", b"rain");
    let a = h.add_video(&a_path, 10, true);
    let b = h.add_video(&b_path, 10, true);
    let inside = h.add_video(&in_path, 10, false);
    let unused = h.add_audio(&un_path, 4);
    for (media, at) in [(a, "0s"), (b, "10s"), (inside, "20s")] {
        h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": media, "at": at}),
        );
    }
    Fixture {
        t,
        h,
        a,
        b,
        inside,
        unused,
    }
}

impl Fixture {
    fn root(&self) -> String {
        s(&self.t.path("Promo"))
    }

    fn ctx(&self, host: &Arc<Host>) -> CollectCtx {
        CollectCtx {
            doc: self.h.doc(),
            project_root: Some(self.root()),
            allowed_roots: None,
            jobs: job_manager(),
            host: Arc::clone(host) as Arc<dyn CollectHost>,
        }
    }

    fn call(&self, name: &str, ctx: &mut CollectCtx, params: Value) -> Result<Value, CmdError> {
        self.h
            .registry
            .invoke_engine_json(name, params, ctx, Access::STANDARD)
    }
}

// ----- the plan -----------------------------------------------------------------------------

#[test]
fn every_file_goes_under_media_in_a_folder_named_after_its_own() {
    let f = fixture("plan");
    let dest = f.t.path("Out");
    let plan = plan_collect(&f.h.doc(), Some(&f.root()), &options(&dest));
    let to: Vec<PathBuf> = plan.files.iter().map(|x| x.to.clone()).collect();
    assert_eq!(plan.files.len(), 4);
    assert!(to.contains(&dest.join("media").join("day1").join("clip.mp4")));
    assert!(to.contains(&dest.join("media").join("day2").join("clip.mp4")));
    assert!(to.contains(&dest.join("media").join("assets").join("logo.mp4")));
    assert!(to.contains(&dest.join("media").join("Sounds").join("rain.wav")));
    // The copy is named after its folder and points at the copies; the open document is as
    // it was.
    assert_eq!(plan.doc.meta.name, "Out");
    assert_eq!(f.h.doc().meta.name, "Test");
    let copy = &plan.doc.media[&f.a];
    assert_eq!(
        copy.path_abs.as_deref(),
        Some(s(&dest.join("media").join("day1").join("clip.mp4")).as_str())
    );
    assert!(
        f.h.doc().media[&f.a]
            .path_abs
            .as_deref()
            .unwrap()
            .contains("Footage")
    );
}

#[test]
fn a_taken_name_gets_a_number_and_the_asset_keeps_its_name() {
    let t = Temp::new("clash");
    let mut h = H::empty();
    let one = h.add_video(&t.write("x/shots/clip.mp4", b"one"), 5, false);
    let two = h.add_video(&t.write("y/shots/clip.mp4", b"two"), 5, false);
    let dest = t.path("Out");
    let plan = plan_collect(&h.doc(), None, &options(&dest));
    let shots = dest.join("media").join("shots");
    assert_eq!(plan.files[0].to, shots.join("clip.mp4"));
    assert_eq!(plan.files[1].to, shots.join("clip (2).mp4"));
    assert_eq!(plan.doc.media[&one].name, None);
    assert_eq!(plan.doc.media[&two].name.as_deref(), Some("clip.mp4"));
}

#[test]
fn external_only_keeps_the_place_of_files_inside_the_project() {
    let f = fixture("external");
    let dest = f.t.path("Out");
    let mut o = options(&dest);
    o.mode = Some(CollectMode::CopyExternalOnly);
    let plan = plan_collect(&f.h.doc(), Some(&f.root()), &o);
    let of = |id: MediaId| {
        plan.files
            .iter()
            .find(|x| x.media.contains(&id))
            .unwrap()
            .to
            .clone()
    };
    assert_eq!(of(f.inside), dest.join("assets").join("logo.mp4"));
    assert_eq!(of(f.b), dest.join("media").join("day2").join("clip.mp4"));
}

#[test]
fn trimming_leaves_unused_media_out_of_the_copy_only() {
    let f = fixture("trim");
    let mut o = options(&f.t.path("Out"));
    o.trim_unused = Some(true);
    let plan = plan_collect(&f.h.doc(), Some(&f.root()), &o);
    assert_eq!(plan.files.len(), 3);
    assert!(!plan.doc.media.contains_key(&f.unused));
    assert!(f.h.doc().media.contains_key(&f.unused));
    plan.doc.validate().unwrap();
}

// ----- the copy -----------------------------------------------------------------------------

fn part_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(read) = fs::read_dir(dir) else {
        return out;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(part_files(&path));
        } else if path.extension().is_some_and(|e| e == "part") {
            out.push(path);
        }
    }
    out
}

#[test]
fn a_collect_copies_every_file_and_writes_the_project_last() {
    let f = fixture("run");
    let dest = f.t.path("Out");
    let host = Host::new(None);
    let originals: Vec<(PathBuf, std::time::SystemTime)> =
        f.h.doc()
            .media
            .values()
            .map(|m| PathBuf::from(m.path_abs.clone().unwrap()))
            .map(|p| (p.clone(), fs::metadata(&p).unwrap().modified().unwrap()))
            .collect();
    let plan = plan_collect(&f.h.doc(), Some(&f.root()), &options(&dest));
    let report = run_collect(&plan, &*host, &Progress::all()).unwrap();
    assert_eq!((report.copied, report.skipped), (4, 0));
    assert_eq!(report.bytes, 3_000_000 + 1_500_000 + 10 + 4);
    for file in &plan.files {
        assert_eq!(fs::read(&file.from).unwrap(), fs::read(&file.to).unwrap());
    }
    assert_eq!(part_files(&dest), Vec::<PathBuf>::new());
    let written = host.written.lock().unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].0, dest);
    // The originals were only read.
    for (path, modified) in originals {
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    }
}

#[test]
fn an_interrupted_collect_continues_without_copying_finished_files_again() {
    let f = fixture("resume");
    let dest = f.t.path("Out");
    let host = Host::new(None);
    let plan = plan_collect(&f.h.doc(), Some(&f.root()), &options(&dest));
    // Stopped in the middle of the second file (about half of the bytes).
    let stopped = run_collect(&plan, &*host, &Progress::until(3_500_000));
    assert_eq!(stopped, Err(JobError::Cancelled));
    assert!(host.written.lock().unwrap().is_empty(), "no project yet");
    assert_eq!(part_files(&dest), Vec::<PathBuf>::new(), "no half file");
    assert!(plan.files[0].to.exists());
    assert!(!plan.files[1].to.exists());

    let report = run_collect(&plan, &*host, &Progress::all()).unwrap();
    assert_eq!((report.copied, report.skipped), (3, 1));
    assert_eq!(report.bytes, 1_500_000 + 10 + 4);
    assert_eq!(host.written.lock().unwrap().len(), 1);
}

#[test]
fn a_damaged_leftover_is_copied_again() {
    let f = fixture("damaged");
    let dest = f.t.path("Out");
    let host = Host::new(None);
    let plan = plan_collect(&f.h.doc(), Some(&f.root()), &options(&dest));
    run_collect(&plan, &*host, &Progress::all()).unwrap();
    // Same size, other bytes at the end.
    let mut bytes = fs::read(&plan.files[1].to).unwrap();
    *bytes.last_mut().unwrap() ^= 0xff;
    fs::write(&plan.files[1].to, bytes).unwrap();
    let report = run_collect(&plan, &*host, &Progress::all()).unwrap();
    assert_eq!((report.copied, report.skipped), (1, 3));
    assert_eq!(
        fs::read(&plan.files[1].from).unwrap(),
        fs::read(&plan.files[1].to).unwrap()
    );
}

#[test]
fn the_generated_folder_comes_along_unless_declined() {
    let f = fixture("generated");
    f.t.write("Promo/generated/voice/take1.wav", b"generated sound");
    let host = Host::new(None);
    let dest = f.t.path("Out");
    let plan = plan_collect(&f.h.doc(), Some(&f.root()), &options(&dest));
    run_collect(&plan, &*host, &Progress::all()).unwrap();
    assert_eq!(
        fs::read(dest.join("generated").join("voice").join("take1.wav")).unwrap(),
        b"generated sound"
    );
    let mut o = options(&f.t.path("Out2"));
    o.include_generated = Some(false);
    assert!(
        plan_collect(&f.h.doc(), Some(&f.root()), &o)
            .generated
            .is_none()
    );
}

// ----- the commands -------------------------------------------------------------------------

#[test]
fn the_estimate_counts_files_bytes_and_free_space() {
    let f = fixture("estimate");
    let host = Host::new(Some(10_000_000_000));
    let dest = s(&f.t.path("Out"));
    let out = f
        .call(
            "project.collect_estimate",
            &mut f.ctx(&host),
            json!({"dest": dest}),
        )
        .unwrap();
    assert_eq!(out["files"], 4);
    assert_eq!(out["bytes"], 4_500_014_u64);
    assert_eq!(out["free_bytes"], 10_000_000_000_u64);
    assert_eq!(out["enough_space"], true);
    assert_eq!(out["missing"], json!([]));
    assert!(!f.t.path("Out").exists(), "an estimate writes nothing");
    let trimmed = f
        .call(
            "project.collect_estimate",
            &mut f.ctx(&host),
            json!({"dest": dest, "trim_unused": true}),
        )
        .unwrap();
    assert_eq!(
        (trimmed["files"].clone(), trimmed["trimmed"].clone()),
        (json!(3), json!(1))
    );
}

#[test]
fn collect_refuses_a_full_drive_and_missing_media_before_copying() {
    let f = fixture("refuse");
    let dest = s(&f.t.path("Out"));
    let small = Host::new(Some(1_000_000));
    let estimate = f
        .call(
            "project.collect_estimate",
            &mut f.ctx(&small),
            json!({"dest": dest}),
        )
        .unwrap();
    assert_eq!(estimate["enough_space"], false);
    assert_invalid(
        f.call("project.collect", &mut f.ctx(&small), json!({"dest": dest})),
        "/dest",
    );

    fs::remove_file(f.t.path("Sounds/rain.wav")).unwrap();
    let roomy = Host::new(None);
    let estimate = f
        .call(
            "project.collect_estimate",
            &mut f.ctx(&roomy),
            json!({"dest": dest}),
        )
        .unwrap();
    assert_eq!(estimate["missing"][0]["media_id"], json!(f.unused));
    assert_eq!(estimate["missing"][0]["name"], "rain.wav");
    assert_invalid(
        f.call("project.collect", &mut f.ctx(&roomy), json!({"dest": dest})),
        "/media",
    );
    // Trimmed away, the missing file no longer matters.
    let out = f
        .call(
            "project.collect",
            &mut f.ctx(&roomy),
            json!({"dest": dest, "trim_unused": true}),
        )
        .unwrap();
    assert_eq!(out["estimate"]["files"], 3);
    assert!(!f.t.path("Out").join("media").join("Sounds").exists());
}

#[test]
fn collect_validates_the_destination() {
    let f = fixture("dest");
    let host = Host::new(None);
    let run =
        |dest: &str, ctx: &mut CollectCtx| f.call("project.collect", ctx, json!({"dest": dest}));
    assert_invalid(run("relative/out", &mut f.ctx(&host)), "/dest");
    // Inside the project, or around it.
    assert_invalid(run(&s(&f.t.path("Promo/copy")), &mut f.ctx(&host)), "/dest");
    assert_invalid(run(&f.root(), &mut f.ctx(&host)), "/dest");
    assert_invalid(run(&s(&f.t.0), &mut f.ctx(&host)), "/dest");
    // A file, and a folder that holds a project.
    let file = f.t.write("a-file.txt", b"x");
    assert_invalid(run(&file, &mut f.ctx(&host)), "/dest");
    f.t.write("Taken/Existing.cutweave", b"{}");
    assert_invalid(run(&s(&f.t.path("Taken")), &mut f.ctx(&host)), "/dest");
    // Outside the folders an agent may write to.
    let mut ctx = f.ctx(&host);
    ctx.allowed_roots = Some(AllowedRoots::new([f.t.path("Allowed")]));
    assert_invalid(run(&s(&f.t.path("Out")), &mut ctx), "/dest");
    assert!(!f.t.path("Out").exists());
}

#[test]
fn collect_runs_as_a_job_that_ends_with_the_project_written() {
    let f = fixture("job");
    let host = Host::new(None);
    let mut ctx = f.ctx(&host);
    let jobs = ctx.jobs.clone();
    let dest = f.t.path("Out");
    let out = f
        .call("project.collect", &mut ctx, json!({"dest": s(&dest)}))
        .unwrap();
    let id = serde_json::from_value(out["job_id"].clone()).unwrap();
    let done = jobs.wait(id, Duration::from_secs(30)).unwrap();
    assert_eq!(done.state, JobState::Succeeded, "{done:?}");
    assert_eq!(done.label, "collect");
    assert!(dest.join("Copy.cutweave").exists());
    assert!(dest.join("media").join("day1").join("clip.mp4").exists());
}

#[test]
fn the_collect_commands_are_in_the_mcp_catalog() {
    let h = H::empty();
    for (name, read_only) in [
        ("project.collect", false),
        ("project.collect_estimate", true),
    ] {
        let meta = &h.registry.get(name).unwrap().meta;
        assert_eq!(meta.mcp, McpExposure::Catalog, "{name}");
        assert_eq!(meta.annotations.read_only, read_only, "{name}");
        assert!(!meta.annotations.destructive, "{name}");
        assert!(meta.annotations.idempotent, "{name}");
    }
}
