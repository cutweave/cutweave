// SPDX-License-Identifier: GPL-3.0-or-later
//! Collect files (WP-2F.04; arch §7, §17): `project.collect` makes a self-contained copy of the
//! project in another folder, the project document plus every media file it uses, and
//! `project.collect_estimate` says how much that is and whether it fits. Both are in the MCP
//! catalog.
//!
//! **What is written.** `<dest>/<name>.cutweave` and the media files: under `<dest>/media/`,
//! each in a folder named after the folder it came from (`media/Footage/clip.mp4`; a name taken
//! by another file gets ` (2)`). With [`CollectMode::CopyExternalOnly`] a file inside the
//! project folder keeps its place relative to it instead. Assets with the same content share
//! one copy. The copy's document points at the copied files; with `trim_unused` the assets no
//! timeline uses are left out of it. `generated/` is copied when `include_generated` is set.
//! The open project and its files are never written to. Proxies are cache state and are not
//! copied: the copy makes its own.
//!
//! **The job.** `project.collect` checks the request and the free space, then returns a job
//! ([`JobKind::Collect`]) that copies file by file ([`run_collect`]): each file goes to
//! `<name>.part`, is compared byte for byte with its source and only then takes its name, so
//! an interrupted run leaves no half file under a real name. Running the same collect again
//! continues: a file that is already there with the source's size, start and end is not copied
//! again. The document is written last, so a folder holds a project only when its media are
//! complete. Progress is by bytes; cancelling stops between chunks.
//!
//! **Who may write where.** Agents and headless callers may only collect into the folders the
//! user approved ([`CollectCtx::allowed_roots`]), as for `export.start`.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use cutweave_commands::{
    Annotations, CmdError, CommandSpec, Described, EngineCommand, EntryKind, McpExposure,
    Registrable, Registry, RegistryError, Scope, command, wire,
};
use cutweave_jobs::{JobCtx, JobError, JobId, JobKind, JobManager, JobSpec};
use cutweave_model::{MediaAsset, MediaId, MediaKind, Project};

use crate::media::{AllowedRoots, OUTSIDE_ROOTS_HINT, asset_name, used_media};
use crate::util::check_abs_path;

/// The label of a collect job (`job.list`, the job UI).
pub const COLLECT_JOB_LABEL: &str = "collect";
/// The folder of a collected project that holds its media.
pub const MEDIA_FOLDER: &str = "media";
/// Bytes copied between two looks at the cancel token.
const CHUNK: usize = 1 << 20;
/// Bytes compared at each end of a file to tell that a copy from an earlier run is complete.
const SAMPLE: u64 = 1 << 20;
/// Free space kept beyond what the copy needs.
const HEADROOM: u64 = 64 << 20;

/// Where collected media go.
#[wire]
#[derive(Copy, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum CollectMode {
    /// Every media file goes under `media/` (the default).
    #[default]
    CopyAll,
    /// Files inside the project folder keep their place relative to it; only the others go
    /// under `media/`.
    CopyExternalOnly,
}

/// What both collect commands take.
#[derive(Clone, Debug)]
pub struct CollectOptions {
    /// Absolute path of the folder to collect into: a new or empty folder, or the folder of
    /// an interrupted collect of this project.
    pub dest: String,
    /// Where media go (default `copy_all`).
    pub mode: Option<CollectMode>,
    /// Leave out the media no timeline uses (default false).
    pub trim_unused: Option<bool>,
    /// Also copy the project's `generated/` folder (default true).
    pub include_generated: Option<bool>,
}

// ----- the plan ------------------------------------------------------------------------------

/// One media file to copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedFile {
    /// The assets that use the file (several when they have the same content).
    pub media: Vec<MediaId>,
    /// The file now.
    pub from: PathBuf,
    /// Where it goes.
    pub to: PathBuf,
}

/// What a collect will do: the files to copy and the document of the copy.
#[derive(Clone, Debug)]
pub struct CollectPlan {
    /// The destination folder.
    pub dest: PathBuf,
    /// The files, in asset order.
    pub files: Vec<PlannedFile>,
    /// The copy's document: paths point into `dest`, trimmed assets are gone.
    pub doc: Project,
    /// The `generated/` folder to copy, when there is one and it was asked for.
    pub generated: Option<PathBuf>,
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The name of the folder that holds `path`, as a folder name for the copy.
fn parent_name(path: &str) -> String {
    let mut parts = path.rsplit(['/', '\\']);
    parts.next();
    let name = parts.next().unwrap_or("").trim_end_matches(':');
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | '"' | '|' | '?' | '*' | ':'))
        .collect();
    let clean = clean.trim().trim_end_matches('.');
    if clean.is_empty() {
        "files".to_owned()
    } else {
        clean.to_owned()
    }
}

/// `name (n).ext` for the n-th file wanting `name.ext`.
fn numbered(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

/// The path of `path` relative to `root`, when it lies inside it (compared ignoring ASCII case
/// and the kind of separator, as paths on Windows are).
fn inside(root: &str, path: &str) -> Option<Vec<String>> {
    let norm = |s: &str| -> Vec<String> {
        s.split(['/', '\\'])
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (root, path) = (norm(root), norm(path));
    if path.len() <= root.len()
        || !root
            .iter()
            .zip(&path)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
        || path.iter().any(|p| p == "..")
    {
        return None;
    }
    Some(path[root.len()..].to_vec())
}

fn is_file_asset(asset: &MediaAsset) -> bool {
    asset.kind != MediaKind::SequenceRef && asset.path_abs.is_some()
}

/// Plans collecting `doc` (a project in `project_root`, if it is saved) into `options.dest`.
#[must_use]
pub fn plan_collect(
    doc: &Project,
    project_root: Option<&str>,
    options: &CollectOptions,
) -> CollectPlan {
    let dest = PathBuf::from(&options.dest);
    let mode = options.mode.unwrap_or_default();
    let used = options
        .trim_unused
        .unwrap_or(false)
        .then(|| used_media(doc));
    let mut out = doc.clone();
    let mut files: Vec<PlannedFile> = Vec::new();
    let mut by_hash: HashMap<String, usize> = HashMap::new();
    let mut by_source: HashMap<String, usize> = HashMap::new();
    let mut taken: HashSet<String> = HashSet::new();
    for asset in doc.media.values() {
        if let Some(used) = &used
            && !used.contains(&asset.id)
        {
            out.media.remove(&asset.id);
            continue;
        }
        let (true, Some(from)) = (is_file_asset(asset), asset.path_abs.as_deref()) else {
            continue;
        };
        let known = asset
            .content_hash
            .as_ref()
            .and_then(|h| by_hash.get(h))
            .or_else(|| by_source.get(&from.to_lowercase()))
            .copied();
        let index = known.unwrap_or_else(|| {
            let kept = (mode == CollectMode::CopyExternalOnly)
                .then(|| project_root.and_then(|root| inside(root, from)))
                .flatten();
            let parts = kept.unwrap_or_else(|| {
                vec![
                    MEDIA_FOLDER.to_owned(),
                    parent_name(from),
                    file_name(from).to_owned(),
                ]
            });
            let (name, folder) = parts.split_last().map_or_else(
                || (file_name(from).to_owned(), Vec::new()),
                |(name, folder)| (name.clone(), folder.to_vec()),
            );
            let mut n = 1;
            let parts = loop {
                let candidate = if n == 1 {
                    name.clone()
                } else {
                    numbered(&name, n)
                };
                let mut parts = folder.clone();
                parts.push(candidate);
                if taken.insert(parts.join("/").to_lowercase()) {
                    break parts;
                }
                n += 1;
            };
            files.push(PlannedFile {
                media: Vec::new(),
                from: PathBuf::from(from),
                to: parts.iter().fold(dest.clone(), |p, part| p.join(part)),
            });
            files.len() - 1
        });
        if let Some(hash) = &asset.content_hash {
            by_hash.insert(hash.clone(), index);
        }
        by_source.insert(from.to_lowercase(), index);
        files[index].media.push(asset.id);
        if let Some(copy) = out.media.get_mut(&asset.id) {
            // The asset keeps the name the bin shows even if its file was renamed on the way.
            if copy.name.is_none()
                && file_name(from)
                    != files[index]
                        .to
                        .file_name()
                        .map_or("", |n| n.to_str().unwrap_or(""))
            {
                copy.name = Some(asset_name(asset).into_owned());
            }
            copy.path_abs = Some(files[index].to.to_string_lossy().into_owned());
            copy.path_rel = None;
        }
    }
    let generated = options
        .include_generated
        .unwrap_or(true)
        .then(|| project_root.map(|root| Path::new(root).join("generated")))
        .flatten()
        .filter(|dir| dir.is_dir());
    CollectPlan {
        dest,
        files,
        doc: out,
        generated,
    }
}

// ----- the copy ------------------------------------------------------------------------------

/// What a collect run did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CollectReport {
    /// Files copied by this run.
    pub copied: u32,
    /// Files that were already complete (an earlier run copied them).
    pub skipped: u32,
    /// Bytes copied by this run.
    pub bytes: u64,
}

/// The engine's part of a collect: the disk facts and the project format `cutweave-ops` does
/// not know.
pub trait CollectHost: Send + Sync {
    /// Free bytes on the drive of `folder` (or of its nearest existing parent); `None` when it
    /// cannot be told.
    fn free_space(&self, folder: &Path) -> Option<u64>;

    /// Whether `folder` already holds a project.
    fn holds_project(&self, folder: &Path) -> bool;

    /// Writes `doc` as a new project in `folder` and returns the path of its document.
    fn write_project(&self, folder: &Path, doc: &Project) -> Result<PathBuf, String>;
}

/// Progress and cancellation of a running collect (a [`JobCtx`], or a test's own).
pub trait CollectProgress {
    /// Whether the run should stop.
    fn cancelled(&self) -> bool;
    /// `done` of `total` bytes are handled; `file` is the one being copied.
    fn report(&self, done: u64, total: u64, file: &str);
}

impl CollectProgress for JobCtx {
    fn cancelled(&self) -> bool {
        self.is_cancelled()
    }

    #[allow(clippy::cast_precision_loss)] // a fraction for a progress bar
    fn report(&self, done: u64, total: u64, file: &str) {
        let fraction = if total == 0 {
            1.0
        } else {
            done as f64 / total as f64
        };
        self.progress(fraction.min(1.0), Some(file));
    }
}

fn failed(what: &str, path: &Path, e: &io::Error) -> JobError {
    JobError::failed(format!("could not {what} `{}`: {e}", path.display()))
}

/// The first and last [`SAMPLE`] bytes of a file.
fn sample(path: &Path, len: u64) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut file = fs::File::open(path)?;
    let head_len = usize::try_from(len.min(SAMPLE)).unwrap_or(usize::MAX);
    let mut head = vec![0; head_len];
    file.read_exact(&mut head)?;
    let tail_len = len.saturating_sub(SAMPLE).min(SAMPLE);
    let mut tail = vec![0; usize::try_from(tail_len).unwrap_or(usize::MAX)];
    file.seek(SeekFrom::Start(len - tail_len))?;
    file.read_exact(&mut tail)?;
    Ok((head, tail))
}

/// Whether `to` is a complete copy of `from` from an earlier run: same size, start and end.
/// (A copy only gets its name after a full comparison, so this need not read it all again.)
fn already_copied(from: &Path, to: &Path, len: u64) -> bool {
    fs::metadata(to).is_ok_and(|m| m.is_file() && m.len() == len)
        && matches!((sample(from, len), sample(to, len)), (Ok(a), Ok(b)) if a == b)
}

/// Whether the two files hold the same bytes.
fn same_bytes(a: &Path, b: &Path, progress: &dyn CollectProgress) -> Result<bool, JobError> {
    let mut fa = fs::File::open(a).map_err(|e| failed("read", a, &e))?;
    let mut fb = fs::File::open(b).map_err(|e| failed("read", b, &e))?;
    let (mut ba, mut bb) = (vec![0; CHUNK], vec![0; CHUNK]);
    loop {
        if progress.cancelled() {
            return Err(JobError::Cancelled);
        }
        let n = read_full(&mut fa, &mut ba).map_err(|e| failed("read", a, &e))?;
        let m = read_full(&mut fb, &mut bb).map_err(|e| failed("read", b, &e))?;
        if n != m || ba[..n] != bb[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

fn read_full(file: &mut fs::File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

fn part_path(to: &Path) -> PathBuf {
    let mut name = to.file_name().map_or_default(std::ffi::OsStr::to_os_string);
    name.push(".part");
    to.with_file_name(name)
}

/// Copies `from` to `to` through `<to>.part`, compares the copy with the source and gives it
/// its name. Returns the bytes copied.
fn copy_verified(
    from: &Path,
    to: &Path,
    progress: &dyn CollectProgress,
    done: &mut u64,
    total: u64,
) -> Result<u64, JobError> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| failed("create the folder", parent, &e))?;
    }
    let part = part_path(to);
    let label = from
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut source = fs::File::open(from).map_err(|e| failed("read", from, &e))?;
    let mut target = fs::File::create(&part).map_err(|e| failed("write", &part, &e))?;
    let mut buf = vec![0; CHUNK];
    let mut copied = 0_u64;
    loop {
        if progress.cancelled() {
            drop(target);
            let _ = fs::remove_file(&part);
            return Err(JobError::Cancelled);
        }
        let n = read_full(&mut source, &mut buf).map_err(|e| failed("read", from, &e))?;
        if n == 0 {
            break;
        }
        target
            .write_all(&buf[..n])
            .map_err(|e| failed("write", &part, &e))?;
        copied += n as u64;
        *done += n as u64;
        progress.report(*done, total, &label);
    }
    target.sync_all().map_err(|e| failed("write", &part, &e))?;
    drop(target);
    match same_bytes(from, &part, progress) {
        Ok(true) => {}
        Ok(false) => {
            let _ = fs::remove_file(&part);
            return Err(JobError::failed(format!(
                "the copy of `{}` does not match the original (the file changed while it was \
                 copied, or the drive is faulty); collect again",
                from.display()
            )));
        }
        Err(e) => {
            let _ = fs::remove_file(&part);
            return Err(e);
        }
    }
    if to.exists() {
        fs::remove_file(to).map_err(|e| failed("replace", to, &e))?;
    }
    fs::rename(&part, to).map_err(|e| failed("write", to, &e))?;
    Ok(copied)
}

fn copy_tree(
    from: &Path,
    to: &Path,
    progress: &dyn CollectProgress,
    report: &mut CollectReport,
) -> Result<(), JobError> {
    let entries = fs::read_dir(from).map_err(|e| failed("read", from, &e))?;
    for entry in entries {
        let entry = entry.map_err(|e| failed("read", from, &e))?;
        let kind = entry
            .file_type()
            .map_err(|e| failed("read", &entry.path(), &e))?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target, progress, report)?;
        } else if kind.is_file() {
            let len = entry.metadata().map_or(0, |m| m.len());
            if already_copied(&entry.path(), &target, len) {
                report.skipped += 1;
            } else {
                let mut done = 0;
                report.bytes += copy_verified(&entry.path(), &target, progress, &mut done, 0)?;
                report.copied += 1;
            }
        }
    }
    Ok(())
}

/// Runs `plan`: copies the files that are not there yet, then `generated/`, then writes the
/// project document (see the module docs).
///
/// # Errors
/// [`JobError::Cancelled`] when `progress` says so, [`JobError::Failed`] with what went wrong.
pub fn run_collect(
    plan: &CollectPlan,
    host: &dyn CollectHost,
    progress: &dyn CollectProgress,
) -> Result<CollectReport, JobError> {
    let sizes: Vec<u64> = plan
        .files
        .iter()
        .map(|f| fs::metadata(&f.from).map(|m| m.len()))
        .collect::<Result<_, _>>()
        .map_err(|e| JobError::failed(format!("a media file is missing: {e}")))?;
    let total: u64 = sizes.iter().sum();
    let mut done = 0_u64;
    let mut report = CollectReport::default();
    for (file, len) in plan.files.iter().zip(sizes) {
        if progress.cancelled() {
            return Err(JobError::Cancelled);
        }
        if already_copied(&file.from, &file.to, len) {
            report.skipped += 1;
            done += len;
            progress.report(done, total, "");
            continue;
        }
        report.bytes += copy_verified(&file.from, &file.to, progress, &mut done, total)?;
        report.copied += 1;
    }
    if let Some(generated) = &plan.generated {
        copy_tree(
            generated,
            &plan.dest.join("generated"),
            progress,
            &mut report,
        )?;
    }
    if progress.cancelled() {
        return Err(JobError::Cancelled);
    }
    host.write_project(&plan.dest, &plan.doc)
        .map_err(JobError::failed)?;
    progress.report(total, total, "");
    Ok(report)
}

// ----- the commands --------------------------------------------------------------------------

/// What the collect commands run against (their engine-command context); the engine builds
/// one per call.
pub struct CollectCtx {
    /// The committed document of the moment.
    pub doc: Project,
    /// The open project's folder; `None` for an untitled document.
    pub project_root: Option<String>,
    /// The folders the caller may write into: `Some` for agents and headless callers, `None`
    /// for the editor UI (whatever the user picked).
    pub allowed_roots: Option<AllowedRoots>,
    /// Runs the copy.
    pub jobs: JobManager,
    /// The engine's part.
    pub host: Arc<dyn CollectHost>,
}

impl std::fmt::Debug for CollectCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollectCtx")
            .field("project_root", &self.project_root)
            .finish_non_exhaustive()
    }
}

/// A media file a collect cannot read.
#[wire]
pub struct CollectMissing {
    /// The asset.
    pub media_id: MediaId,
    /// The name the bin shows.
    pub name: String,
    /// Where the project says its file is.
    pub path: String,
}

/// Result of `project.collect_estimate`.
#[wire]
pub struct CollectEstimate {
    /// The folder the copy goes into.
    pub dest: String,
    /// The media files to copy.
    pub files: u32,
    /// Their size in bytes.
    #[specta(type = specta_typescript::Number)]
    pub bytes: u64,
    /// Free bytes on the destination drive, when it can be told.
    #[specta(type = Option<specta_typescript::Number>)]
    pub free_bytes: Option<u64>,
    /// Whether the files fit (true when the free space cannot be told).
    pub enough_space: bool,
    /// Media whose file is missing: relink them first (`media.relink_folder`).
    pub missing: Vec<CollectMissing>,
    /// Media left out because no timeline uses them (`trim_unused`).
    pub trimmed: u32,
}

/// The plan and estimate of a request, after checking it.
fn prepare(
    options: &CollectOptions,
    ctx: &CollectCtx,
) -> Result<(CollectPlan, CollectEstimate), CmdError> {
    check_abs_path("/dest", &options.dest)?;
    if let Some(roots) = &ctx.allowed_roots
        && !roots.allows(&options.dest)
    {
        return Err(CmdError::invalid(
            "/dest",
            format!(
                "`{}` is outside the folders you may write to (the folders the user allowed); \
                 {OUTSIDE_ROOTS_HINT}",
                options.dest
            ),
        ));
    }
    let dest = Path::new(&options.dest);
    if let Some(root) = &ctx.project_root
        && (inside(root, &options.dest).is_some()
            || inside(&options.dest, root).is_some()
            || Path::new(root) == dest)
    {
        return Err(CmdError::invalid(
            "/dest",
            "choose a folder outside the project's own folder: the copy must stand on its own",
        ));
    }
    if dest.is_file() {
        return Err(CmdError::invalid("/dest", "that is a file, not a folder"));
    }
    if ctx.host.holds_project(dest) {
        return Err(CmdError::invalid(
            "/dest",
            "that folder already holds a project; choose an empty folder",
        ));
    }
    let plan = plan_collect(&ctx.doc, ctx.project_root.as_deref(), options);
    let mut bytes = 0_u64;
    let mut missing = Vec::new();
    for file in &plan.files {
        match fs::metadata(&file.from) {
            Ok(meta) if meta.is_file() => bytes += meta.len(),
            _ => missing.extend(file.media.iter().filter_map(|id| {
                let asset = ctx.doc.media.get(id)?;
                Some(CollectMissing {
                    media_id: *id,
                    name: asset_name(asset).into_owned(),
                    path: file.from.to_string_lossy().into_owned(),
                })
            })),
        }
    }
    let free_bytes = ctx.host.free_space(dest);
    let trimmed = ctx.doc.media.len() - plan.doc.media.len();
    let estimate = CollectEstimate {
        dest: options.dest.clone(),
        files: u32::try_from(plan.files.len()).unwrap_or(u32::MAX),
        bytes,
        free_bytes,
        enough_space: free_bytes.is_none_or(|free| free >= bytes.saturating_add(HEADROOM)),
        missing,
        trimmed: u32::try_from(trimmed).unwrap_or(u32::MAX),
    };
    Ok((plan, estimate))
}

/// Says what `project.collect` with the same params would copy: how many media files, how many
/// bytes, the free space on the destination drive and whether the files fit, and which media
/// cannot be copied because their file is missing. Changes nothing.
#[wire]
pub struct ProjectCollectEstimate {
    /// Absolute path of the folder to collect into: a new or empty folder, or the folder of
    /// an interrupted collect of this project.
    pub dest: String,
    /// Where media go: `copy_all` (default, everything under `media/`) or
    /// `copy_external_only` (files inside the project folder keep their place).
    pub mode: Option<CollectMode>,
    /// Leave out the media no timeline uses (default false).
    pub trim_unused: Option<bool>,
    /// Also copy the project's `generated/` folder (default true).
    pub include_generated: Option<bool>,
}

impl Described for ProjectCollectEstimate {
    const SPEC: CommandSpec = CommandSpec {
        name: "project.collect_estimate",
        summary: "Say what collecting the project would copy and whether it fits on the drive",
        kind: EntryKind::Engine,
        undoable: false,
        view: false,
        scope: Scope::Any,
        mcp: McpExposure::Catalog,
        annotations: Annotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        secret_fields: &[],
        rust_type: "ProjectCollectEstimate",
    };
    const JOURNALED: bool = false;
}

impl Registrable for ProjectCollectEstimate {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry
            .add_engine_command::<ProjectCollectEstimate>()
            .map(drop)
    }
}

impl EngineCommand for ProjectCollectEstimate {
    type Context = CollectCtx;
    type Output = CollectEstimate;

    fn run(&self, ctx: &mut CollectCtx) -> Result<CollectEstimate, CmdError> {
        prepare(
            &CollectOptions {
                dest: self.dest.clone(),
                mode: self.mode,
                trim_unused: self.trim_unused,
                include_generated: self.include_generated,
            },
            ctx,
        )
        .map(|(_, estimate)| estimate)
    }
}

/// Result of `project.collect`.
#[wire]
pub struct ProjectCollectOutput {
    /// The copy job: follow it with `job.wait` or `job.status`, stop it with `job.cancel`.
    pub job_id: JobId,
    /// What it copies.
    pub estimate: CollectEstimate,
}

/// Makes a self-contained copy of the project in another folder: the project document and
/// every media file it uses (under `media/`), with the copy's document pointing at the copied
/// files, so the folder opens on any computer. The open project and its files are not changed.
/// Returns a background job; the copy is complete when the job succeeds (the document is
/// written last). Each copied file is compared with its original. Calling it again with the
/// same folder after a cancel or a crash continues without copying finished files again.
/// Fails with `Invalid` at `/dest` when the files do not fit on the drive or the folder
/// already holds a project, and at `/media` when a media file is missing (relink it first).
/// Agents and headless callers may only collect into folders the user approved.
#[command(
    name = "project.collect",
    summary = "Copy the project and all its media into one folder that opens anywhere (a job)",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ProjectCollect {
    /// Absolute path of the folder to collect into: a new or empty folder, or the folder of
    /// an interrupted collect of this project.
    pub dest: String,
    /// Where media go: `copy_all` (default, everything under `media/`) or
    /// `copy_external_only` (files inside the project folder keep their place).
    pub mode: Option<CollectMode>,
    /// Leave out the media no timeline uses (default false).
    pub trim_unused: Option<bool>,
    /// Also copy the project's `generated/` folder (default true).
    pub include_generated: Option<bool>,
}

impl EngineCommand for ProjectCollect {
    type Context = CollectCtx;
    type Output = ProjectCollectOutput;

    fn run(&self, ctx: &mut CollectCtx) -> Result<ProjectCollectOutput, CmdError> {
        let (plan, estimate) = prepare(
            &CollectOptions {
                dest: self.dest.clone(),
                mode: self.mode,
                trim_unused: self.trim_unused,
                include_generated: self.include_generated,
            },
            ctx,
        )?;
        if let Some(first) = estimate.missing.first() {
            let more = estimate.missing.len() - 1;
            return Err(CmdError::invalid(
                "/media",
                format!(
                    "the file of `{}` is missing{}; relink it first (media.relink_folder), or \
                     remove it from the project",
                    first.name,
                    if more == 0 {
                        String::new()
                    } else {
                        format!(" (and {more} more)")
                    }
                ),
            ));
        }
        if !estimate.enough_space {
            return Err(CmdError::invalid(
                "/dest",
                format!(
                    "not enough space: the media take {} bytes and the drive has {} free",
                    estimate.bytes,
                    estimate.free_bytes.unwrap_or(0)
                ),
            ));
        }
        let host = Arc::clone(&ctx.host);
        let job = ctx.jobs.submit(
            JobSpec::new(JobKind::Collect, COLLECT_JOB_LABEL),
            move |job| run_collect(&plan, &*host, job).map(drop),
        );
        Ok(ProjectCollectOutput {
            job_id: job.id(),
            estimate,
        })
    }
}

/// The assets of `plan` by destination, for callers that show what goes where.
#[must_use]
pub fn destinations(plan: &CollectPlan) -> BTreeMap<MediaId, PathBuf> {
    plan.files
        .iter()
        .flat_map(|f| f.media.iter().map(|id| (*id, f.to.clone())))
        .collect()
}

/// Registers the collect commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<ProjectCollect>()?
        .register::<ProjectCollectEstimate>()?;
    Ok(())
}
