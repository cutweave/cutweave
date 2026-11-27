// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.04b: `project.archive`. The media are small files made at test time in a temp folder;
//! the engine's part (free space, writing the project, the hash) is a stand-in, so the hashes
//! here are FNV-1a, not SHA-256 (the engine's test checks the real ones).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names
)]

mod common;

use std::{
    cell::Cell,
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use common::{H, assert_invalid};
use cutweave_commands::{Access, CmdError, McpExposure};
use cutweave_jobs::{JobError, JobState};
use cutweave_model::{Extra, FrameRate, ImageSequence, MediaAsset, MediaId, Project};
use cutweave_ops::{
    media::{AllowedRoots, FakeProbe},
    project::{
        archive::{ArchiveManifest, MANIFEST_NAME, plan_zip, run_archive_folder, run_archive_zip},
        collect::{
            CollectCtx, CollectHost, CollectOptions, CollectProgress, ContentHasher, plan_collect,
        },
    },
    testing::job_manager,
};
use serde_json::{Value, json};

/// A temp folder removed when dropped.
struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!(
            "cwa-{tag}-{}-{}",
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
        s(&path)
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

/// FNV-1a (64 bit) in hex: the stand-in for the engine's SHA-256.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn of(bytes: &[u8]) -> String {
        let mut h = Box::new(Fnv::new());
        h.update(bytes);
        h.finish()
    }
}

impl ContentHasher for Fnv {
    fn update(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(self: Box<Self>) -> String {
        format!("{:016x}", self.0)
    }
}

/// The engine's part: a drive with `free` bytes; a project is `<folder>/<folder name>.cutweave`
/// holding the document's name.
struct Host {
    free: Option<u64>,
}

impl Host {
    fn new(free: Option<u64>) -> Arc<Host> {
        Arc::new(Host { free })
    }
}

impl CollectHost for Host {
    fn free_space(&self, _folder: &Path) -> Option<u64> {
        self.free
    }

    fn holds_project(&self, folder: &Path) -> bool {
        fs::read_dir(folder).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "cutweave"))
        })
    }

    fn write_project(&self, folder: &Path, doc: &Project) -> Result<PathBuf, String> {
        fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let name = folder.file_name().unwrap().to_string_lossy().into_owned();
        let path = folder.join(format!("{name}.cutweave"));
        fs::write(&path, format!("project {}", doc.meta.name)).map_err(|e| e.to_string())?;
        Ok(path)
    }

    fn hasher(&self) -> Box<dyn ContentHasher> {
        Box::new(Fnv::new())
    }
}

/// Runs to the end, or says "cancelled" once `stop_after` bytes were reported.
struct Progress {
    stop_after: Option<u64>,
    seen: Cell<u64>,
    last: Cell<(u64, u64)>,
}

impl Progress {
    fn all() -> Progress {
        Progress {
            stop_after: None,
            seen: Cell::new(0),
            last: Cell::new((0, 0)),
        }
    }

    fn until(bytes: u64) -> Progress {
        Progress {
            stop_after: Some(bytes),
            ..Progress::all()
        }
    }
}

impl CollectProgress for Progress {
    fn cancelled(&self) -> bool {
        self.stop_after
            .is_some_and(|limit| self.seen.get() >= limit)
    }

    fn report(&self, done: u64, total: u64, _file: &str) {
        assert!(done <= total, "progress past the end: {done} of {total}");
        self.seen.set(done);
        self.last.set((done, total));
    }
}

const CLIP_A: usize = 3_000_000;
const CLIP_B: usize = 1_500_000;

/// A project in `<t>/Promo` with a generated file and four assets: two outside it (same file
/// name, different folders), one inside it, one unused sound outside.
struct Fixture {
    t: Temp,
    h: H,
}

fn fixture(tag: &str) -> Fixture {
    let t = Temp::new(tag);
    let (mut h, p) = H::project();
    let a_path = t.write("Footage/day1/clip.mp4", &vec![1_u8; CLIP_A]);
    let b_path = t.write("Footage/day2/clip.mp4", &vec![2_u8; CLIP_B]);
    let in_path = t.write("Promo/assets/logo.mp4", b"logo bytes");
    let un_path = t.write("Sounds/rain.wav", b"rain");
    t.write("Promo/generated/voice/take 1.wav", b"generated voice");
    let a = h.add_video(&a_path, 10, true);
    let b = h.add_video(&b_path, 10, true);
    let inside = h.add_video(&in_path, 10, false);
    h.add_audio(&un_path, 4);
    for (media, at) in [(a, "0s"), (b, "10s"), (inside, "20s")] {
        h.ok(
            "timeline.insert",
            json!({"sequence": p.sequence, "track": p.video, "media": media, "at": at}),
        );
    }
    Fixture { t, h }
}

/// What the fixture's archive holds, by path.
const FILES: [&str; 6] = [
    "Handover.cutweave",
    "generated/voice/take 1.wav",
    "media/Sounds/rain.wav",
    "media/assets/logo.mp4",
    "media/day1/clip.mp4",
    "media/day2/clip.mp4",
];

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

    fn call(&self, ctx: &mut CollectCtx, params: Value) -> Result<Value, CmdError> {
        self.h
            .registry
            .invoke_engine_json("project.archive", params, ctx, Access::STANDARD)
    }

    fn folder_plan(&self, dest: &Path) -> cutweave_ops::project::collect::CollectPlan {
        plan_collect(
            &self.h.doc(),
            Some(&self.root()),
            &CollectOptions {
                dest: s(dest),
                mode: None,
                trim_unused: None,
                include_generated: None,
            },
        )
    }
}

/// Every file below `dir`, as paths relative to it with `/`.
fn files_below(dir: &Path) -> BTreeSet<String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap();
                out.insert(s(rel).replace('\\', "/"));
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(dir, dir, &mut out);
    out
}

fn paths(manifest: &ArchiveManifest) -> Vec<&str> {
    manifest.files.iter().map(|f| f.path.as_str()).collect()
}

/// Every entry of the zip: its name, whether it is stored, and its bytes.
fn read_zip(path: &Path) -> Vec<(String, bool, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    (0..archive.len())
        .map(|i| {
            let mut entry = archive.by_index(i).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            (
                entry.name().unwrap().into_owned(),
                entry.compression() == zip::CompressionMethod::Stored,
                bytes,
            )
        })
        .collect()
}

// ----- the folder ---------------------------------------------------------------------------

#[test]
fn a_folder_archive_lists_every_file_with_its_size_and_hash() {
    let f = fixture("folder");
    let host = Host::new(None);
    let dest = f.t.path("Out/Handover");
    let progress = Progress::all();
    let manifest = run_archive_folder(&f.folder_plan(&dest), &*host, &progress).unwrap();

    assert_eq!(manifest.format, "cutweave-archive");
    assert_eq!(manifest.version, 1);
    assert_eq!(manifest.hash, "sha256");
    assert_eq!(manifest.project, "Handover.cutweave");
    assert_eq!(paths(&manifest), FILES);
    // The folder holds exactly those files and the manifest, nothing half-written.
    let mut on_disk: BTreeSet<String> = FILES.iter().map(|p| (*p).to_owned()).collect();
    on_disk.insert(MANIFEST_NAME.to_owned());
    assert_eq!(files_below(&dest), on_disk);
    // Every size and hash is that of the file in the folder.
    for file in &manifest.files {
        let bytes = fs::read(dest.join(&file.path)).unwrap();
        assert_eq!(file.size, bytes.len() as u64, "{}", file.path);
        assert_eq!(file.hash, Fnv::of(&bytes), "{}", file.path);
    }
    // The manifest on disk is the one returned.
    let written: ArchiveManifest =
        serde_json::from_slice(&fs::read(dest.join(MANIFEST_NAME)).unwrap()).unwrap();
    assert_eq!(written, manifest);
    // Progress ends at the end.
    let (done, total) = progress.last.get();
    assert_eq!(done, total);
    assert!(total > 0);
}

#[test]
fn a_folder_archive_cancelled_while_it_is_checked_continues() {
    let f = fixture("folder-cancel");
    let host = Host::new(None);
    let dest = f.t.path("Out/Handover");
    let plan = f.folder_plan(&dest);
    // Past the copy (reported up to the media's size), two megabytes into the check.
    let stop = (CLIP_A + CLIP_B) as u64 + 2_000_000;
    let err = run_archive_folder(&plan, &*host, &Progress::until(stop)).unwrap_err();
    assert!(matches!(err, JobError::Cancelled), "{err:?}");
    // The media are kept, but the folder holds no project and no file list yet.
    let left = files_below(&dest);
    assert!(left.contains("media/day1/clip.mp4"), "{left:?}");
    assert!(!left.contains("Handover.cutweave"), "{left:?}");
    assert!(!left.contains(MANIFEST_NAME), "{left:?}");
    assert!(!host.holds_project(&dest));
    // The same call finishes it.
    let manifest = run_archive_folder(&plan, &*host, &Progress::all()).unwrap();
    assert_eq!(paths(&manifest), FILES);
    assert!(dest.join(MANIFEST_NAME).is_file());
}

#[test]
fn trimming_leaves_unused_media_out_of_the_archive() {
    let f = fixture("trim");
    let host = Host::new(None);
    let dest = f.t.path("Handover");
    let mut options = CollectOptions {
        dest: s(&dest),
        mode: None,
        trim_unused: Some(true),
        include_generated: Some(false),
    };
    let plan = plan_collect(&f.h.doc(), Some(&f.root()), &options);
    let manifest = run_archive_folder(&plan, &*host, &Progress::all()).unwrap();
    assert_eq!(
        paths(&manifest),
        [
            "Handover.cutweave",
            "media/assets/logo.mp4",
            "media/day1/clip.mp4",
            "media/day2/clip.mp4",
        ]
    );
    assert!(!dest.join("generated").exists());
    // (The options are plain data: the same request without trimming lists the sound again.)
    options.trim_unused = None;
    assert_eq!(
        plan_collect(&f.h.doc(), Some(&f.root()), &options)
            .files
            .len(),
        4
    );
}

// ----- the zip ------------------------------------------------------------------------------

#[test]
fn a_zip_holds_the_same_tree_stored_under_one_folder() {
    let f = fixture("zip");
    let host = Host::new(None);
    let zip = f.t.path("Out/Handover.zip");
    let plan = plan_zip(&f.h.doc(), Some(&f.root()), &zip, None, None);
    let progress = Progress::all();
    let manifest = run_archive_zip(&plan, &zip, &*host, &progress).unwrap();
    assert_eq!(paths(&manifest), FILES);
    assert_eq!(manifest.project, "Handover.cutweave");

    // Only the zip is left: no `.part`, no staging folder.
    assert_eq!(
        files_below(&f.t.path("Out")),
        BTreeSet::from(["Handover.zip".to_owned()])
    );
    let entries = read_zip(&zip);
    let names: BTreeSet<&str> = entries.iter().map(|(n, _, _)| n.as_str()).collect();
    let mut expected: BTreeSet<String> = FILES.iter().map(|p| format!("Handover/{p}")).collect();
    expected.insert("Handover/archive.json".to_owned());
    assert_eq!(
        names,
        expected.iter().map(String::as_str).collect::<BTreeSet<_>>()
    );
    for (name, stored, bytes) in &entries {
        assert!(stored, "{name} is stored, not compressed");
        let path = name.strip_prefix("Handover/").unwrap();
        if path == MANIFEST_NAME {
            let inside: ArchiveManifest = serde_json::from_slice(bytes).unwrap();
            assert_eq!(inside, manifest);
            continue;
        }
        let listed = manifest.files.iter().find(|f| f.path == path).unwrap();
        assert_eq!(listed.size, bytes.len() as u64, "{name}");
        assert_eq!(listed.hash, Fnv::of(bytes), "{name}");
    }
    // The media are the originals, byte for byte.
    let bytes_of = |path: &str| &entries.iter().find(|(n, _, _)| n == path).unwrap().2;
    assert_eq!(
        bytes_of("Handover/media/day1/clip.mp4"),
        &vec![1_u8; CLIP_A]
    );
    assert_eq!(
        bytes_of("Handover/media/day2/clip.mp4"),
        &vec![2_u8; CLIP_B]
    );
    assert_eq!(
        bytes_of("Handover/generated/voice/take 1.wav"),
        b"generated voice"
    );
    // The zip is the media plus a little bookkeeping: nothing was recompressed.
    let size = fs::metadata(&zip).unwrap().len();
    assert!(size > (CLIP_A + CLIP_B) as u64);
    let (done, total) = progress.last.get();
    assert_eq!(done, total);
}

#[test]
fn a_cancelled_or_failed_zip_leaves_no_file() {
    let f = fixture("zip-cancel");
    let host = Host::new(None);
    fs::create_dir_all(f.t.path("Out")).unwrap();
    let zip = f.t.path("Out/Handover.zip");
    let plan = plan_zip(&f.h.doc(), Some(&f.root()), &zip, None, None);
    // Cancelled while the first clip is written, and again while the zip is read back.
    let all = (CLIP_A + CLIP_B) as u64;
    for stop in [1_000_000, all + 2_000_000] {
        let err = run_archive_zip(&plan, &zip, &*host, &Progress::until(stop)).unwrap_err();
        assert!(matches!(err, JobError::Cancelled), "{err:?}");
        assert_eq!(files_below(&f.t.path("Out")), BTreeSet::new(), "at {stop}");
    }
    // A media file that went away: the run fails before it writes anything.
    let moved = f.t.path("Footage/day2/gone.mp4");
    fs::rename(f.t.path("Footage/day2/clip.mp4"), &moved).unwrap();
    let err = run_archive_zip(&plan, &zip, &*host, &Progress::all()).unwrap_err();
    assert!(matches!(err, JobError::Failed(_)), "{err:?}");
    assert_eq!(files_below(&f.t.path("Out")), BTreeSet::new());
    // With the file back, the same call makes the zip.
    fs::rename(&moved, f.t.path("Footage/day2/clip.mp4")).unwrap();
    run_archive_zip(&plan, &zip, &*host, &Progress::all()).unwrap();
    assert!(zip.is_file());
}

/// Nightly only (WBS WP-2F.04, "large-file test with sparse 5 GB file"): it writes more than
/// 10 GB to the temp drive and takes minutes. Run it with
/// `cargo nextest run -p cutweave-ops --test project_archive --run-ignored only`.
#[test]
#[ignore = "nightly: writes and reads back a 5 GB zip"]
fn a_file_past_4_gb_goes_into_a_zip64_archive() {
    const BIG: u64 = 5 << 30;
    let t = Temp::new("zip64");
    let mut h = H::empty();
    let big = t.path("Footage/big.mp4");
    fs::create_dir_all(big.parent().unwrap()).unwrap();
    // No bytes are written for the file itself (it reads as zeros); the zip is real.
    fs::File::create(&big).unwrap().set_len(BIG).unwrap();
    h.add_video(&s(&big), 10, false);
    let host = Host::new(None);
    let zip = t.path("Out/Big.zip");
    let plan = plan_zip(&h.doc(), None, &zip, None, None);
    let manifest = run_archive_zip(&plan, &zip, &*host, &Progress::all()).unwrap();
    let listed = manifest
        .files
        .iter()
        .find(|f| f.path == "media/Footage/big.mp4")
        .unwrap();
    assert_eq!(listed.size, BIG);
    assert!(fs::metadata(&zip).unwrap().len() > BIG);
    // It reads back with its full size (the run itself compared every byte's hash).
    let mut archive = zip::ZipArchive::new(fs::File::open(&zip).unwrap()).unwrap();
    assert_eq!(
        archive.by_name("Big/media/Footage/big.mp4").unwrap().size(),
        BIG
    );
}

// ----- the command --------------------------------------------------------------------------

#[test]
fn archive_refuses_what_cannot_be_archived() {
    let f = fixture("refuse");
    let host = Host::new(None);
    let folder = |dest: &Path| json!({"dest": s(dest), "format": "folder"});
    let zip = |dest: &Path| json!({"dest": s(dest), "format": "zip"});
    let run = |params: Value, ctx: &mut CollectCtx| f.call(ctx, params);

    assert_invalid(
        run(
            json!({"dest": "relative/out", "format": "folder"}),
            &mut f.ctx(&host),
        ),
        "/dest",
    );
    // Inside the project's own folder.
    let inside = f.t.path("Promo/backup");
    assert_invalid(run(folder(&inside), &mut f.ctx(&host)), "/dest");
    assert_invalid(
        run(zip(&f.t.path("Promo/backup.zip")), &mut f.ctx(&host)),
        "/dest",
    );
    // A zip needs a `.zip` name, and one that is free.
    assert_invalid(
        run(zip(&f.t.path("Out/Handover")), &mut f.ctx(&host)),
        "/dest",
    );
    assert_invalid(run(zip(&f.t.path("Out/.zip")), &mut f.ctx(&host)), "/dest");
    let taken = PathBuf::from(f.t.write("Out/Taken.zip", b"x"));
    assert_invalid(run(zip(&taken), &mut f.ctx(&host)), "/dest");
    assert_eq!(fs::read(&taken).unwrap(), b"x", "an existing zip is kept");
    // A folder that already holds a project.
    f.t.write("Held/Held.cutweave", b"{}");
    assert_invalid(run(folder(&f.t.path("Held")), &mut f.ctx(&host)), "/dest");
    // Too little space, for both formats.
    let full = Host::new(Some(1_000_000));
    assert_invalid(run(folder(&f.t.path("Out/A")), &mut f.ctx(&full)), "/dest");
    assert_invalid(run(zip(&f.t.path("Out/A.zip")), &mut f.ctx(&full)), "/dest");
    // Outside the folders an agent may write to.
    let mut ctx = f.ctx(&host);
    ctx.allowed_roots = Some(AllowedRoots::new([f.t.path("Allowed")]));
    assert_invalid(run(zip(&f.t.path("Out/B.zip")), &mut ctx), "/dest");
    assert_invalid(run(folder(&f.t.path("Out/B")), &mut ctx), "/dest");
    // A missing media file.
    fs::remove_file(f.t.path("Sounds/rain.wav")).unwrap();
    assert_invalid(run(folder(&f.t.path("Out/C")), &mut f.ctx(&host)), "/media");
    assert_invalid(
        run(zip(&f.t.path("Out/C.zip")), &mut f.ctx(&host)),
        "/media",
    );
    // ... which trimming leaves out, so that request is fine.
    let mut ctx = f.ctx(&host);
    let jobs = ctx.jobs.clone();
    let out = run(
        json!({"dest": s(&f.t.path("Out/D.zip")), "format": "zip", "trim_unused": true}),
        &mut ctx,
    )
    .unwrap();
    let id = serde_json::from_value(out["job_id"].clone()).unwrap();
    let done = jobs.wait(id, Duration::from_secs(60)).unwrap();
    assert_eq!(done.state, JobState::Succeeded, "{done:?}");
    // Nothing refused left anything behind.
    assert_eq!(
        files_below(&f.t.path("Out")),
        BTreeSet::from(["D.zip".to_owned(), "Taken.zip".to_owned()])
    );
}

#[test]
fn archive_runs_as_a_job_for_both_formats() {
    let f = fixture("job");
    let host = Host::new(None);
    for (format, dest, made) in [
        ("folder", "Out/Handover", "Out/Handover/archive.json"),
        ("zip", "Out/Handover.zip", "Out/Handover.zip"),
    ] {
        let mut ctx = f.ctx(&host);
        let jobs = ctx.jobs.clone();
        let dest = f.t.path(dest);
        let out = f
            .call(&mut ctx, json!({"dest": s(&dest), "format": format}))
            .unwrap();
        assert_eq!(out["estimate"]["dest"], s(&dest), "{format}");
        assert_eq!(out["estimate"]["files"], 4, "{format}");
        let id = serde_json::from_value(out["job_id"].clone()).unwrap();
        let done = jobs.wait(id, Duration::from_secs(60)).unwrap();
        assert_eq!(done.state, JobState::Succeeded, "{format}: {done:?}");
        assert_eq!(done.label, "archive");
        assert!(f.t.path(made).is_file(), "{format}");
    }
    // The project's own folder was only read.
    assert_eq!(
        files_below(&f.t.path("Promo")),
        BTreeSet::from([
            "assets/logo.mp4".to_owned(),
            "generated/voice/take 1.wav".to_owned()
        ])
    );
}

#[test]
fn archive_is_in_the_mcp_catalog() {
    let h = H::empty();
    let meta = &h.registry.get("project.archive").unwrap().meta;
    assert_eq!(meta.mcp, McpExposure::Catalog);
    assert!(!meta.annotations.read_only);
    assert!(!meta.annotations.destructive);
    assert!(meta.annotations.idempotent);
    assert!(!meta.undoable);
}

// ----- image sequences (#208) ---------------------------------------------------------------

#[test]
fn an_archive_holds_every_picture_of_an_image_sequence() {
    let t = Temp::new("seq");
    let mut h = H::empty();
    // `f_0001.png` … `f_0005.png`; number 3 had no file at import.
    let mut first = String::new();
    for number in [1_u32, 2, 4, 5] {
        let path = t.write(
            &format!("Renders/shotA/f_{number:04}.png"),
            format!("picture {number}").as_bytes(),
        );
        if number == 1 {
            first = path;
        }
    }
    let run = ImageSequence {
        pattern: "f_%04d.png".to_owned(),
        first: 1,
        count: 5,
        fps: FrameRate::FPS_30,
        missing: vec![3],
        extra: Extra::new(),
    };
    let probe = FakeProbe::image(&first).probe;
    let mut asset = MediaAsset::image_sequence(MediaId::from_u128(900), first, run, probe);
    asset.content_hash = Some("fake:run".to_owned());
    let mut doc = h.doc();
    doc.media.insert(asset.id, asset);
    h.replace_doc(doc);
    let pictures = [1, 2, 4, 5].map(|n| format!("media/shotA/f_{n:04}.png"));
    let mut listed = vec!["Handover.cutweave".to_owned()];
    listed.extend(pictures.iter().cloned());

    let host = Host::new(None);
    let dest = t.path("Out/Handover");
    let options = CollectOptions {
        dest: s(&dest),
        mode: None,
        trim_unused: None,
        include_generated: None,
    };
    let plan = plan_collect(&h.doc(), None, &options);
    let manifest = run_archive_folder(&plan, &*host, &Progress::all()).unwrap();
    assert_eq!(paths(&manifest), listed);
    for (n, path) in [1, 2, 4, 5].iter().zip(&pictures) {
        let bytes = fs::read(dest.join(path)).unwrap();
        assert_eq!(bytes, format!("picture {n}").into_bytes());
        let file = manifest.files.iter().find(|f| &f.path == path).unwrap();
        assert_eq!(
            (file.size, &file.hash),
            (bytes.len() as u64, &Fnv::of(&bytes))
        );
    }

    let zip = t.path("Zip/Handover.zip");
    let plan = plan_zip(&h.doc(), None, &zip, None, None);
    let manifest = run_archive_zip(&plan, &zip, &*host, &Progress::all()).unwrap();
    assert_eq!(paths(&manifest), listed);
    let entries = read_zip(&zip);
    for (n, path) in [1, 2, 4, 5].iter().zip(&pictures) {
        let (_, _, bytes) = entries
            .iter()
            .find(|(name, _, _)| name == &format!("Handover/{path}"))
            .unwrap();
        assert_eq!(*bytes, format!("picture {n}").into_bytes());
    }
}
