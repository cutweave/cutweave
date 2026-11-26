// SPDX-License-Identifier: GPL-3.0-or-later
//! Archive a project (WP-2F.04b; arch §7, §17): `project.archive` bundles the project document,
//! every media file it uses and `generated/` into one folder or one `.zip`, with a file list
//! (`archive.json`) that gives the size and SHA-256 of every file, so the bundle can be stored
//! or handed over and checked later. It is in the MCP catalog.
//!
//! **What is written.** The tree is the one `project.collect` makes ([`super::collect`]):
//! `<name>.cutweave`, the media under `media/`, `generated/` when asked for, plus
//! [`MANIFEST_NAME`]. Proxies and caches are never part of it.
//!
//! - [`ArchiveFormat::Folder`]: `dest` is the folder. The files are copied by the collect
//!   copier (each through `<name>.part`, compared with its source), then every file of the copy
//!   is read once more for its hash and the manifest is written last. A run that stops before
//!   that leaves the copied media but no project document, so the same call continues it.
//! - [`ArchiveFormat::Zip`]: `dest` is the `.zip` file. It holds one folder named after the file
//!   (`Promo.zip` holds `Promo/…`), so extracting it anywhere gives a project folder. Entries
//!   are stored, not compressed (media do not shrink, and a stored entry can be read in place);
//!   files of 4 GB and more and archives past 4 GB use Zip64. The zip is written to
//!   `<dest>.part`, read back entry by entry against the manifest, and only then takes its
//!   name: an interrupted or cancelled run leaves no file under the final name.
//!
//! **The manifest** ([`ArchiveManifest`]) lists every file but itself, with paths relative to
//! its own folder, sorted. `scripts/verify-archive.py` checks a folder or a zip against it.
//!
//! The open project and its files are never written to, so this is not an undo step. Agents
//! and headless callers may only archive into the folders the user approved, as for
//! `project.collect`.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use cutweave_commands::{CmdError, EngineCommand, Registry, RegistryError, command, wire};
use cutweave_jobs::{JobError, JobId, JobKind, JobSpec};
use cutweave_model::Project;
use serde::{Deserialize, Serialize};

use super::collect::{
    CHUNK, CollectCtx, CollectEstimate, CollectHost, CollectOptions, CollectPlan, CollectProgress,
    check_dest, check_ready, collect_into, estimate, failed, part_path, plan_collect, prepare,
    read_full,
};

/// The label of an archive job (`job.list`, the job UI).
pub const ARCHIVE_JOB_LABEL: &str = "archive";
/// The file list of an archive, next to the project document.
pub const MANIFEST_NAME: &str = "archive.json";
/// What [`ArchiveManifest::format`] says.
pub const MANIFEST_FORMAT: &str = "cutweave-archive";
/// The version of the manifest's layout.
pub const MANIFEST_VERSION: u32 = 1;
/// The hash the manifest uses ([`CollectHost::hasher`]).
pub const MANIFEST_HASH: &str = "sha256";
/// The folder of a collected project that holds its generated files.
const GENERATED_FOLDER: &str = "generated";
/// A zip entry of this size or more is written with Zip64 sizes (the classic fields end at
/// 4 GB - 1; the margin keeps a file that grows a little while it is read inside its entry).
const ZIP64_FILE: u64 = 0xFFFF_0000;

/// What an archive is.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFormat {
    /// A folder: the collected project plus `archive.json`.
    Folder,
    /// One `.zip` file holding that folder (stored entries, Zip64 when needed).
    Zip,
}

/// One file of an archive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveFile {
    /// The path relative to the manifest's folder, with `/`.
    pub path: String,
    /// The size in bytes.
    pub size: u64,
    /// The hash of the content, in lowercase hex ([`ArchiveManifest::hash`] names the kind).
    pub hash: String,
}

/// `archive.json`: every file of an archive but the manifest itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveManifest {
    /// [`MANIFEST_FORMAT`].
    pub format: String,
    /// [`MANIFEST_VERSION`].
    pub version: u32,
    /// The project document, as a path of `files`.
    pub project: String,
    /// The kind of hash: [`MANIFEST_HASH`].
    pub hash: String,
    /// The files, sorted by path.
    pub files: Vec<ArchiveFile>,
}

// ----- shared pieces -------------------------------------------------------------------------

/// A file that goes into the archive.
struct Entry {
    /// Its path in the archive, with `/`.
    path: String,
    /// The file to read.
    source: PathBuf,
    /// Its size when the run started.
    size: u64,
}

/// One half of a run's progress: the copy (or the writing) is the first, the check the second.
struct Half<'a> {
    inner: &'a dyn CollectProgress,
    second: bool,
}

impl CollectProgress for Half<'_> {
    fn cancelled(&self) -> bool {
        self.inner.cancelled()
    }

    fn report(&self, done: u64, total: u64, file: &str) {
        let done = done.min(total);
        let at = if self.second { total + done } else { done };
        self.inner.report(at, total.saturating_mul(2), file);
    }
}

fn check_cancel(progress: &dyn CollectProgress) -> Result<(), JobError> {
    if progress.cancelled() {
        Err(JobError::Cancelled)
    } else {
        Ok(())
    }
}

/// `path` below `root`, with `/` (the archive's way of writing paths).
fn relative(root: &Path, path: &Path) -> Result<String, JobError> {
    let rel = path.strip_prefix(root).map_err(|_| {
        JobError::failed(format!(
            "`{}` is not inside `{}`",
            path.display(),
            root.display()
        ))
    })?;
    Ok(rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

/// Every file below `dir`, in name order.
fn tree(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), JobError> {
    let mut entries = fs::read_dir(dir)
        .and_then(Iterator::collect::<Result<Vec<_>, _>>)
        .map_err(|e| failed("read", dir, &e))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| failed("read", &path, &e))?;
        if kind.is_dir() {
            tree(&path, out)?;
        } else if kind.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn entry(path: String, source: PathBuf) -> Result<Entry, JobError> {
    let meta = fs::metadata(&source).map_err(|e| failed("read", &source, &e))?;
    if !meta.is_file() {
        return Err(JobError::failed(format!(
            "`{}` is not a file",
            source.display()
        )));
    }
    Ok(Entry {
        path,
        source,
        size: meta.len(),
    })
}

fn manifest_of(project: &str, mut files: Vec<ArchiveFile>) -> ArchiveManifest {
    files.sort_by(|a, b| a.path.cmp(&b.path));
    ArchiveManifest {
        format: MANIFEST_FORMAT.to_owned(),
        version: MANIFEST_VERSION,
        project: project.to_owned(),
        hash: MANIFEST_HASH.to_owned(),
        files,
    }
}

fn manifest_bytes(manifest: &ArchiveManifest) -> Result<Vec<u8>, JobError> {
    let mut bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|e| JobError::failed(format!("could not write the file list: {e}")))?;
    bytes.push(b'\n');
    Ok(bytes)
}

// ----- the folder ----------------------------------------------------------------------------

/// Archives `plan` as a folder: collects it ([`super::collect::run_collect`]), hashes every
/// file of the copy and writes `archive.json`. Returns the manifest.
///
/// # Errors
/// [`JobError::Cancelled`] when `progress` says so, [`JobError::Failed`] with what went wrong.
pub fn run_archive_folder(
    plan: &CollectPlan,
    host: &dyn CollectHost,
    progress: &dyn CollectProgress,
) -> Result<ArchiveManifest, JobError> {
    let copying = Half {
        inner: progress,
        second: false,
    };
    let (_, document) = collect_into(plan, host, &copying)?;
    let listed = list_folder(plan, &document, host, progress);
    if listed.is_err() {
        // Without its document the folder is an interrupted archive again, so the same call
        // continues it (a folder that holds a project is refused).
        let _ = fs::remove_file(&document);
    }
    listed
}

/// The second half of a folder archive: hashes the copy and writes `archive.json`.
fn list_folder(
    plan: &CollectPlan,
    document: &Path,
    host: &dyn CollectHost,
    progress: &dyn CollectProgress,
) -> Result<ArchiveManifest, JobError> {
    let mut entries = vec![entry(
        relative(&plan.dest, document)?,
        document.to_path_buf(),
    )?];
    for file in &plan.files {
        entries.push(entry(relative(&plan.dest, &file.to)?, file.to.clone())?);
    }
    if plan.generated.is_some() {
        let copied = plan.dest.join(GENERATED_FOLDER);
        let mut files = Vec::new();
        if copied.is_dir() {
            tree(&copied, &mut files)?;
        }
        for file in files {
            entries.push(entry(relative(&plan.dest, &file)?, file)?);
        }
    }
    let checking = Half {
        inner: progress,
        second: true,
    };
    let total: u64 = entries.iter().map(|e| e.size).sum();
    let mut done = 0_u64;
    let mut buf = vec![0; CHUNK];
    let mut files = Vec::with_capacity(entries.len());
    for e in &entries {
        let mut hasher = host.hasher();
        let mut source = fs::File::open(&e.source).map_err(|x| failed("read", &e.source, &x))?;
        let mut size = 0_u64;
        loop {
            check_cancel(progress)?;
            let n = read_full(&mut source, &mut buf).map_err(|x| failed("read", &e.source, &x))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            size += n as u64;
            done += n as u64;
            checking.report(done, total, &e.path);
        }
        files.push(ArchiveFile {
            path: e.path.clone(),
            size,
            hash: hasher.finish(),
        });
    }
    let manifest = manifest_of(&entries[0].path, files);
    let path = plan.dest.join(MANIFEST_NAME);
    let part = part_path(&path);
    fs::write(&part, manifest_bytes(&manifest)?).map_err(|e| failed("write", &part, &e))?;
    if path.exists() {
        fs::remove_file(&path).map_err(|e| failed("replace", &path, &e))?;
    }
    fs::rename(&part, &path).map_err(|e| failed("write", &path, &e))?;
    checking.report(total, total, "");
    Ok(manifest)
}

// ----- the zip -------------------------------------------------------------------------------

/// The folder a zip's project document is written in before it goes into the zip, next to
/// the zip's `.part` file. It is removed when the run ends.
fn staging_of(zip: &Path) -> PathBuf {
    let mut name = part_path(zip)
        .file_name()
        .map_or_default(std::ffi::OsStr::to_os_string);
    name.push(".d");
    zip.with_file_name(name)
}

/// The name of the one folder a zip holds: the zip's file name without `.zip`.
fn top_of(zip: &Path) -> String {
    zip.file_stem()
        .map_or_else(String::new, |n| n.to_string_lossy().trim().to_owned())
}

/// Plans the zip at `zip`: a collect plan whose destination is the staging folder of the zip
/// (the document is written there; media are read where they are).
#[must_use]
pub fn plan_zip(
    doc: &Project,
    project_root: Option<&str>,
    zip: &Path,
    trim_unused: Option<bool>,
    include_generated: Option<bool>,
) -> CollectPlan {
    plan_collect(
        doc,
        project_root,
        &zip_options(zip, trim_unused, include_generated),
    )
}

fn zip_options(
    zip: &Path,
    trim_unused: Option<bool>,
    include_generated: Option<bool>,
) -> CollectOptions {
    CollectOptions {
        dest: staging_of(zip)
            .join(top_of(zip))
            .to_string_lossy()
            .into_owned(),
        mode: None,
        trim_unused,
        include_generated,
    }
}

/// The date of a file for its zip entry (UTC; the zip format has no time zone). `None` before
/// 1980 or when it cannot be told: the entry then gets the format's earliest date.
fn zip_time(path: &Path) -> Option<zip::DateTime> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let secs = modified.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let days = i64::try_from(secs / 86_400).ok()?;
    let rest = secs % 86_400;
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's `civil_from_days`).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    zip::DateTime::from_date_and_time(
        u16::try_from(year).ok()?,
        u8::try_from(month).ok()?,
        u8::try_from(day).ok()?,
        u8::try_from(rest / 3_600).ok()?,
        u8::try_from(rest % 3_600 / 60).ok()?,
        u8::try_from(rest % 60).ok()?,
    )
    .ok()
}

fn zip_failed(part: &Path, e: &zip::result::ZipError) -> JobError {
    JobError::failed(format!("could not write `{}`: {e}", part.display()))
}

fn stored(size: u64, source: Option<&Path>) -> zip::write::SimpleFileOptions {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(size >= ZIP64_FILE);
    match source.and_then(zip_time) {
        Some(time) => options.last_modified_time(time),
        None => options,
    }
}

/// Archives `plan` (from [`plan_zip`]) as the zip file `zip` (see the module docs). Returns
/// the manifest that is in the zip.
///
/// # Errors
/// [`JobError::Cancelled`] when `progress` says so, [`JobError::Failed`] with what went wrong.
/// Either way no file is left under the zip's name, and its `.part` file is removed.
pub fn run_archive_zip(
    plan: &CollectPlan,
    zip: &Path,
    host: &dyn CollectHost,
    progress: &dyn CollectProgress,
) -> Result<ArchiveManifest, JobError> {
    let part = part_path(zip);
    let staging = staging_of(zip);
    // What an interrupted run left behind.
    let _ = fs::remove_dir_all(&staging);
    let result = write_zip(plan, zip, &part, host, progress);
    let _ = fs::remove_dir_all(&staging);
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

fn write_zip(
    plan: &CollectPlan,
    zip: &Path,
    part: &Path,
    host: &dyn CollectHost,
    progress: &dyn CollectProgress,
) -> Result<ArchiveManifest, JobError> {
    let top = top_of(zip);
    // The document first: it is small, and nothing else is written before the files are known
    // to be there.
    let document = host
        .write_project(&plan.dest, &plan.doc)
        .map_err(JobError::failed)?;
    let mut entries = vec![entry(relative(&plan.dest, &document)?, document)?];
    for file in &plan.files {
        entries.push(entry(relative(&plan.dest, &file.to)?, file.from.clone())?);
    }
    if let Some(generated) = &plan.generated {
        let mut files = Vec::new();
        tree(generated, &mut files)?;
        for file in files {
            let path = format!("{GENERATED_FOLDER}/{}", relative(generated, &file)?);
            entries.push(entry(path, file)?);
        }
    }
    let total: u64 = entries.iter().map(|e| e.size).sum();
    if let Some(parent) = zip.parent() {
        fs::create_dir_all(parent).map_err(|e| failed("create the folder", parent, &e))?;
    }
    if zip.exists() {
        return Err(JobError::failed(format!(
            "`{}` is already there; choose another name",
            zip.display()
        )));
    }

    let writing = Half {
        inner: progress,
        second: false,
    };
    let target = fs::File::create(part).map_err(|e| failed("write", part, &e))?;
    let mut writer = zip::ZipWriter::new(target);
    let mut done = 0_u64;
    let mut buf = vec![0; CHUNK];
    let mut files = Vec::with_capacity(entries.len());
    for e in &entries {
        check_cancel(progress)?;
        let mut hasher = host.hasher();
        let mut source = fs::File::open(&e.source).map_err(|x| failed("read", &e.source, &x))?;
        writer
            .start_file(format!("{top}/{}", e.path), stored(e.size, Some(&e.source)))
            .map_err(|x| zip_failed(part, &x))?;
        let mut size = 0_u64;
        loop {
            check_cancel(progress)?;
            let n = read_full(&mut source, &mut buf).map_err(|x| failed("read", &e.source, &x))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            writer
                .write_all(&buf[..n])
                .map_err(|x| failed("write", part, &x))?;
            size += n as u64;
            done += n as u64;
            writing.report(done, total, &e.path);
        }
        files.push(ArchiveFile {
            path: e.path.clone(),
            size,
            hash: hasher.finish(),
        });
    }
    let manifest = manifest_of(&entries[0].path, files);
    let bytes = manifest_bytes(&manifest)?;
    writer
        .start_file(
            format!("{top}/{MANIFEST_NAME}"),
            stored(bytes.len() as u64, None),
        )
        .map_err(|x| zip_failed(part, &x))?;
    writer
        .write_all(&bytes)
        .map_err(|x| failed("write", part, &x))?;
    let target = writer.finish().map_err(|x| zip_failed(part, &x))?;
    target.sync_all().map_err(|e| failed("write", part, &e))?;
    drop(target);

    verify_zip(part, &top, &manifest, host, progress)?;
    check_cancel(progress)?;
    fs::rename(part, zip).map_err(|e| failed("write", zip, &e))?;
    progress.report(1, 1, "");
    Ok(manifest)
}

/// Reads every entry of the written zip back and compares it with the manifest.
fn verify_zip(
    part: &Path,
    top: &str,
    manifest: &ArchiveManifest,
    host: &dyn CollectHost,
    progress: &dyn CollectProgress,
) -> Result<(), JobError> {
    let checking = Half {
        inner: progress,
        second: true,
    };
    let broken = |what: &str| {
        JobError::failed(format!(
            "the archive does not read back as it was written ({what}; the drive may be \
             faulty); archive again"
        ))
    };
    let total: u64 = manifest.files.iter().map(|f| f.size).sum();
    let file = fs::File::open(part).map_err(|e| failed("read", part, &e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| broken(&e.to_string()))?;
    let mut done = 0_u64;
    let mut buf = vec![0; CHUNK];
    for expected in &manifest.files {
        let mut entry = archive
            .by_name(&format!("{top}/{}", expected.path))
            .map_err(|e| broken(&format!("`{}`: {e}", expected.path)))?;
        let mut hasher = host.hasher();
        let mut size = 0_u64;
        loop {
            check_cancel(progress)?;
            let n = entry
                .read(&mut buf)
                .map_err(|e| broken(&format!("`{}`: {e}", expected.path)))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            size += n as u64;
            done += n as u64;
            checking.report(done, total, &expected.path);
        }
        if size != expected.size || hasher.finish() != expected.hash {
            return Err(broken(&format!("`{}` differs", expected.path)));
        }
    }
    Ok(())
}

// ----- the command ---------------------------------------------------------------------------

/// Result of `project.archive`.
#[wire]
pub struct ProjectArchiveOutput {
    /// The archive job: follow it with `job.wait` or `job.status`, stop it with `job.cancel`.
    pub job_id: JobId,
    /// What it bundles; `dest` is the folder or the `.zip` file.
    pub estimate: CollectEstimate,
}

/// Bundles the project for storage or a handover: the project document, every media file it
/// uses (under `media/`), its `generated/` folder, and a file list `archive.json` with the size
/// and SHA-256 of every file. `format: "folder"` writes a project folder at `dest`;
/// `format: "zip"` writes one `.zip` file at `dest` (the name must end in `.zip`) that holds
/// that folder, uncompressed, with Zip64 for large files. Proxies and caches are left out. The
/// open project and its files are not changed. Returns a background job; the archive is
/// complete when the job succeeds: the zip gets its name only after it was read back and
/// compared with the file list, so a cancelled or failed run leaves no `.zip`. A folder
/// archive that was cancelled continues when called again with the same folder. Fails with
/// `Invalid` at `/dest` when the files do not fit on the drive, the folder already holds a
/// project or the `.zip` already exists, and at `/media` when a media file is missing (relink
/// it first). `project.collect_estimate` with the same folder says what would be bundled.
/// Agents and headless callers may only archive into folders the user approved.
#[command(
    name = "project.archive",
    summary = "Bundle the project, its media and a checked file list into a folder or a zip (a job)",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ProjectArchive {
    /// Absolute path of the archive: for `folder` a new or empty folder (or the folder of an
    /// interrupted archive of this project), for `zip` a `.zip` file that does not exist yet.
    pub dest: String,
    /// `folder` (a project folder with `archive.json`) or `zip` (one `.zip` holding it).
    pub format: ArchiveFormat,
    /// Leave out the media no timeline uses (default false).
    pub trim_unused: Option<bool>,
    /// Also bundle the project's `generated/` folder (default true).
    pub include_generated: Option<bool>,
}

impl EngineCommand for ProjectArchive {
    type Context = CollectCtx;
    type Output = ProjectArchiveOutput;

    fn run(&self, ctx: &mut CollectCtx) -> Result<ProjectArchiveOutput, CmdError> {
        let host = Arc::clone(&ctx.host);
        let spec = JobSpec::new(JobKind::Collect, ARCHIVE_JOB_LABEL);
        let (job, estimate) = match self.format {
            ArchiveFormat::Folder => {
                let options = CollectOptions {
                    dest: self.dest.clone(),
                    mode: None,
                    trim_unused: self.trim_unused,
                    include_generated: self.include_generated,
                };
                let (plan, estimate) = prepare(&options, ctx)?;
                check_ready(&estimate)?;
                let job = ctx.jobs.submit(spec, move |job| {
                    run_archive_folder(&plan, &*host, job).map(drop)
                });
                (job, estimate)
            }
            ArchiveFormat::Zip => {
                check_dest(&self.dest, ctx)?;
                let zip = PathBuf::from(&self.dest);
                let is_zip = zip
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("zip"));
                if !is_zip || top_of(&zip).is_empty() {
                    return Err(CmdError::invalid(
                        "/dest",
                        "a zip archive needs a file name that ends in `.zip`, for example \
                         `Promo.zip`",
                    ));
                }
                if zip.exists() {
                    return Err(CmdError::invalid(
                        "/dest",
                        "that name is already taken; choose a `.zip` file that does not exist yet",
                    ));
                }
                let options = zip_options(&zip, self.trim_unused, self.include_generated);
                let (plan, mut estimate) = estimate(&options, ctx);
                estimate.dest.clone_from(&self.dest);
                check_ready(&estimate)?;
                let job = ctx.jobs.submit(spec, move |job| {
                    run_archive_zip(&plan, &zip, &*host, job).map(drop)
                });
                (job, estimate)
            }
        };
        Ok(ProjectArchiveOutput {
            job_id: job.id(),
            estimate,
        })
    }
}

/// Registers the archive command.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register::<ProjectArchive>()?;
    Ok(())
}
