// SPDX-License-Identifier: GPL-3.0-or-later
//! Project folder format, atomic save/open and the single-writer lock (WP-1C.01; arch §17,
//! ADR-0006).
//!
//! ```text
//! MyVideo/
//! ├─ MyVideo.cutweave      JSON document (written only by atomic replace)
//! ├─ generated/  compositions/
//! └─ .cutweave/{journal/, backups/, lock, lock.guard}
//! ```
//!
//! [`ProjectStore`] owns the folder and its lock. Callers must pass only the last *committed*
//! snapshot to [`ProjectStore::save`] (arch §5.2); the store never sees an open transaction.
//!
//! Guarantees:
//!
//! - **Atomic saves:** temp file next to the document, `fsync`, rename over, `fsync` the
//!   directory. A crash at any point leaves the previous document intact and loadable.
//! - **Single writer:** see [`LockInfo`]; a second writer gets [`ProjectIoError::Locked`] or, if
//!   the caller asks, a read-only view.
//! - **Never overwrite what we cannot understand:** documents from a newer schema open read-only
//!   and refuse `save`; a migrated document's original is preserved in `.cutweave/backups/`
//!   before the first save over it.
//! - **Path safety:** every write goes to a fixed location inside the project folder; project
//!   names are validated for all platforms.
//! - **Moved projects:** media is resolved by absolute path, then by the folder-relative path;
//!   `path_rel` is recomputed on every save and on save-as.
//! - **No lost work (WP-1C.02):** an editable store finds the [journal](crate::journal) chain
//!   that continues its document and reports unsaved work ([`OpenReport::recovery`]). The
//!   engine then calls, on every editable open: [`ProjectStore::recover`] (or
//!   [`ProjectStore::discard_recovery`]), [`ProjectStore::start_journal`], and saves with
//!   [`ProjectStore::save_snapshot`], which rotates the journal around the document write.
//!   Saving or editing is refused while unsaved work awaits that decision.

pub(crate) mod atomic;
mod lock;
mod paths;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cutweave_commands::{DocSnapshot, Rev};
use cutweave_model::{
    MediaId, MigrationError, MigrationReport, OpenOutcome, Project, open_str, to_document_string,
};
use sha2::{Digest, Sha256};

pub use lock::{LockInfo, LockedReason, host_name};
pub use paths::{DOCUMENT_EXTENSION, ProjectLayout, relative_to, resolve_relative, validate_name};

use crate::journal::{
    Journal, JournalConfig, JournalError, PendingJournal, RecoverOptions, Recovered,
    RecoveryAvailable,
};
use atomic::{Step, write_atomic, write_atomic_with};
use lock::ProjectLock;

/// SHA-256 of `bytes`, lowercase hex: the identity a journal segment records for the saved
/// document it continues.
#[must_use]
pub fn document_sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Errors from project I/O.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProjectIoError {
    /// A filesystem operation failed.
    #[error("{op} failed for {}: {source}", path.display())]
    Io {
        /// What was being done.
        op: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The OS error.
        #[source]
        source: io::Error,
    },
    /// Another process has the project open for editing.
    #[error("{}", locked_message(holder.as_ref(), *reason))]
    Locked {
        /// Who holds it, when known.
        holder: Option<LockInfo>,
        /// Why it counts as held.
        reason: LockedReason,
    },
    /// The store is read-only (newer schema, held by another process, or requested).
    #[error("the project is open read-only: {0}")]
    ReadOnly(ReadOnlyReason),
    /// The project document already exists.
    #[error("a project already exists at {}", .0.display())]
    AlreadyExists(PathBuf),
    /// No project document at the path.
    #[error("no .cutweave project document found at {}", .0.display())]
    NotFound(PathBuf),
    /// More than one `.cutweave` file in the folder and none matches the folder name.
    #[error("several .cutweave documents in {}; open one directly", .0.display())]
    Ambiguous(PathBuf),
    /// The project name cannot be a folder/file name on every platform.
    #[error("invalid project name {name:?}: {reason}")]
    InvalidName {
        /// The offending name.
        name: String,
        /// Why.
        reason: String,
    },
    /// A write would land outside the project folder.
    #[error("refusing to write outside the project folder: {}", .0.display())]
    OutsideProject(PathBuf),
    /// The document could not be parsed or migrated.
    #[error("cannot open the project document: {0}")]
    Document(#[from] MigrationError),
    /// The project could not be serialized.
    #[error("cannot serialize the project: {0}")]
    Serialize(#[source] serde_json::Error),
    /// The command journal failed or refused (WP-1C.02).
    #[error(transparent)]
    Journal(#[from] JournalError),
}

fn locked_message(holder: Option<&LockInfo>, reason: LockedReason) -> String {
    let who = holder.map_or_else(String::new, |h| {
        format!(
            " (pid {} on {}, since unix time {})",
            h.pid, h.host, h.started
        )
    });
    match reason {
        LockedReason::HeldByLiveProcess => {
            format!("the project is already open in another Cutweave process{who}")
        }
        LockedReason::ForeignHost => format!(
            "the project has a lock from another computer{who}; if it is no longer open there, \
             open it again and choose to take over the lock"
        ),
    }
}

impl ProjectIoError {
    pub(crate) fn io(op: &'static str, path: &Path, source: io::Error) -> Self {
        ProjectIoError::Io {
            op,
            path: path.to_path_buf(),
            source,
        }
    }
    pub(crate) fn invalid_name(name: &str, reason: &str) -> Self {
        ProjectIoError::InvalidName {
            name: name.to_owned(),
            reason: reason.to_owned(),
        }
    }
}

/// Why a store is read-only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadOnlyReason {
    /// Saved by a newer Cutweave (arch §4.3); never written, never locked.
    NewerSchema {
        /// The document's schema version.
        found_version: u32,
        /// Warning text for the user.
        message: String,
    },
    /// Another process holds the lock and the caller asked for a read-only fallback.
    Locked(Option<LockInfo>),
    /// The caller asked for read-only.
    Requested,
}

impl std::fmt::Display for ReadOnlyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadOnlyReason::NewerSchema { message, .. } => f.write_str(message),
            ReadOnlyReason::Locked(_) => f.write_str("another Cutweave process is editing it"),
            ReadOnlyReason::Requested => f.write_str("opened read-only on request"),
        }
    }
}

/// What to do when another process holds the project.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnLocked {
    /// Fail with [`ProjectIoError::Locked`].
    #[default]
    Fail,
    /// Open a read-only view.
    ReadOnly,
}

/// Options for [`ProjectStore::open_with`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenOptions {
    /// Behaviour when the project is locked.
    pub on_locked: OnLocked,
    /// Open read-only without taking the lock or touching the folder.
    pub read_only: bool,
    /// Take over a lock record left by another host.
    pub steal_foreign_lock: bool,
}

/// A media file that could not be found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingMedia {
    /// The asset.
    pub id: MediaId,
    /// Its recorded absolute path.
    pub path_abs: Option<String>,
    /// Its recorded project-relative path.
    pub path_rel: Option<String>,
}

/// A media file found through its relative path after the project moved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelinkedMedia {
    /// The asset.
    pub id: MediaId,
    /// The old (missing) absolute path.
    pub from: Option<String>,
    /// The new absolute path, now stored in the opened project.
    pub to: String,
}

/// What opening a project found and did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenReport {
    /// Set when schema migrations ran.
    pub migration: Option<MigrationReport>,
    /// Why the project is read-only, if it is.
    pub read_only: Option<ReadOnlyReason>,
    /// Media found by relative path only; `path_abs` was updated in the returned project
    /// (editable opens only).
    pub relinked_media: Vec<RelinkedMedia>,
    /// Media found by neither path (relinking is a P2 feature).
    pub missing_media: Vec<MissingMedia>,
    /// The record of a crashed holder whose stale lock was taken over.
    pub stale_lock_taken: Option<LockInfo>,
    /// Unsaved work in the journal (editable opens only): call [`ProjectStore::recover`] or
    /// [`ProjectStore::discard_recovery`] before editing (arch §17).
    pub recovery: Option<RecoveryAvailable>,
}

/// The result of opening a project.
#[derive(Debug)]
pub struct Opened {
    /// The store (holds the lock unless read-only).
    pub store: ProjectStore,
    /// The document.
    pub project: Project,
    /// Findings.
    pub report: OpenReport,
}

/// An open project folder with its lock.
#[derive(Debug)]
pub struct ProjectStore {
    /// The running journal (dropped first: synced before the lock is released).
    journal: Option<Journal>,
    /// The journal between open and [`start_journal`](Self::start_journal).
    pending_journal: Option<PendingJournal>,
    /// SHA-256 of the document bytes on disk.
    disk_sha256: Option<String>,
    layout: ProjectLayout,
    lock: Option<ProjectLock>,
    read_only: Option<ReadOnlyReason>,
    /// Original schema version when the document was migrated and not yet saved.
    pending_premigration_backup: Option<u32>,
}

impl ProjectStore {
    /// Creates a new project in `folder` (created if missing, must hold no project yet) and
    /// writes `project`. The document is `<folder>/<folder name>.cutweave`.
    pub fn create(folder: &Path, project: &Project) -> Result<ProjectStore, ProjectIoError> {
        let layout = ProjectLayout::for_folder(folder)?;
        if layout.document.exists() {
            return Err(ProjectIoError::AlreadyExists(layout.document));
        }
        make_layout_dirs(&layout)?;
        let acquired = ProjectLock::acquire(&layout, false)?;
        let mut store = ProjectStore::writable(layout, acquired.lock);
        store.save(project)?;
        Ok(store)
    }

    fn writable(layout: ProjectLayout, lock: ProjectLock) -> ProjectStore {
        ProjectStore {
            journal: None,
            pending_journal: None,
            disk_sha256: None,
            layout,
            lock: Some(lock),
            read_only: None,
            pending_premigration_backup: None,
        }
    }

    /// Opens `path` (a `.cutweave` file or a project folder) for editing; fails if locked.
    pub fn open(path: &Path) -> Result<Opened, ProjectIoError> {
        Self::open_with(path, &OpenOptions::default())
    }

    /// [`open`](Self::open) with options.
    pub fn open_with(path: &Path, options: &OpenOptions) -> Result<Opened, ProjectIoError> {
        let layout = locate(path)?;
        let bytes = fs::read(&layout.document)
            .map_err(|e| ProjectIoError::io("read document", &layout.document, e))?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| {
                ProjectIoError::io(
                    "read document",
                    &layout.document,
                    io::Error::new(io::ErrorKind::InvalidData, e),
                )
            })?
            .trim_start_matches('\u{feff}');
        let outcome = open_str(text)?;
        let mut report = OpenReport::default();

        let (mut project, editable) = match outcome {
            OpenOutcome::Editable { project, report: m } => {
                if m.migrated() {
                    report.migration = Some(m);
                }
                (project, true)
            }
            OpenOutcome::ReadOnly {
                project,
                found_version,
                reason,
            } => {
                report.read_only = Some(ReadOnlyReason::NewerSchema {
                    found_version,
                    message: reason,
                });
                (project, false)
            }
        };

        let mut lock = None;
        if editable && options.read_only {
            report.read_only = Some(ReadOnlyReason::Requested);
        } else if editable {
            match ProjectLock::acquire(&layout, options.steal_foreign_lock) {
                Ok(acquired) => {
                    report.stale_lock_taken = acquired.stale;
                    lock = Some(acquired.lock);
                }
                Err(ProjectIoError::Locked { holder, .. })
                    if options.on_locked == OnLocked::ReadOnly =>
                {
                    report.read_only = Some(ReadOnlyReason::Locked(holder));
                }
                Err(e) => return Err(e),
            }
        }

        resolve_media(&layout, &mut project, editable, &mut report);
        let disk_sha256 = document_sha256(&bytes);
        let mut pending_journal = None;
        if lock.is_some() {
            // Crash leftovers from an interrupted save; we hold the lock, so they are ours.
            let _ = fs::remove_file(atomic::temp_path_for(&layout.document));
            let pending = PendingJournal::scan(
                &layout.journal_dir(),
                &layout.backups_dir(),
                disk_sha256.clone(),
            )
            .map_err(|e| ProjectIoError::io("read journal", &layout.journal_dir(), e))?;
            report.recovery = pending.available();
            pending_journal = Some(pending);
        }
        let store = ProjectStore {
            journal: None,
            pending_journal,
            disk_sha256: Some(disk_sha256),
            layout,
            lock,
            read_only: report.read_only.clone(),
            pending_premigration_backup: report.migration.as_ref().map(|m| m.from_version),
        };
        Ok(Opened {
            store,
            project,
            report,
        })
    }

    /// Saves `project` (the last committed snapshot) atomically. `path_rel` of every media asset
    /// is recomputed against the folder.
    ///
    /// For stores without a running journal (new projects, tools). Once
    /// [`start_journal`](Self::start_journal) ran, save with
    /// [`save_snapshot`](Self::save_snapshot), which knows the revision to rotate the journal
    /// at. Refused while the journal holds unsaved work awaiting [`recover`](Self::recover) or
    /// [`discard_recovery`](Self::discard_recovery) (saving would orphan it).
    pub fn save(&mut self, project: &Project) -> Result<(), ProjectIoError> {
        self.save_inner(project, &mut |_| Ok(()))
    }

    fn save_inner(
        &mut self,
        project: &Project,
        hook: &mut dyn FnMut(Step) -> io::Result<()>,
    ) -> Result<(), ProjectIoError> {
        if self.journal.is_some() {
            return Err(JournalError::RevisionRequired.into());
        }
        self.write_document(project, None, hook)
    }

    /// Saves the committed snapshot `snapshot` (from
    /// [`DocHandle::committed_snapshot`](cutweave_commands::DocHandle::committed_snapshot)):
    /// rotates the journal to a new segment over this revision, writes the document atomically,
    /// then retires the superseded journal (WP-1C.02).
    pub fn save_snapshot(&mut self, snapshot: &DocSnapshot) -> Result<(), ProjectIoError> {
        if snapshot.rev != snapshot.committed_rev {
            return Err(JournalError::NotCommitted {
                rev: snapshot.rev,
                committed_rev: snapshot.committed_rev,
            }
            .into());
        }
        self.save_at(snapshot.committed_rev, &snapshot.doc)
    }

    /// [`save_snapshot`](Self::save_snapshot) with the committed revision `rev` of `project`.
    pub fn save_at(&mut self, rev: Rev, project: &Project) -> Result<(), ProjectIoError> {
        self.write_document(project, Some(rev), &mut |_| Ok(()))
    }

    fn write_document(
        &mut self,
        project: &Project,
        rev: Option<Rev>,
        hook: &mut dyn FnMut(Step) -> io::Result<()>,
    ) -> Result<(), ProjectIoError> {
        if let Some(reason) = &self.read_only {
            return Err(ProjectIoError::ReadOnly(reason.clone()));
        }
        if self
            .pending_journal
            .as_ref()
            .is_some_and(|p| p.available().is_some())
        {
            return Err(JournalError::RecoveryPending.into());
        }
        let text =
            to_document_string(&self.persisted_form(project)).map_err(ProjectIoError::Serialize)?;
        let sha = document_sha256(text.as_bytes());
        self.layout.ensure_inside(&self.layout.document)?;
        self.preserve_premigration_original()?;
        let rotation = match (&self.journal, rev) {
            (Some(journal), Some(rev)) => Some(journal.rotate(rev, &sha)?),
            (Some(_), None) => return Err(JournalError::RevisionRequired.into()),
            (None, _) => None,
        };
        write_atomic_with(&self.layout.document, text.as_bytes(), hook)
            .map_err(|e| ProjectIoError::io("save document", &self.layout.document, e))?;
        if let (Some(journal), Some(rotation)) = (&self.journal, rotation) {
            journal.retire(rotation);
        }
        self.disk_sha256 = Some(sha.clone());
        if self.journal.is_none() && self.lock.is_some() {
            // Not journaling yet: the journal now has to continue this document.
            self.pending_journal =
                Some(
                    PendingJournal::scan(
                        &self.layout.journal_dir(),
                        &self.layout.backups_dir(),
                        sha,
                    )
                    .map_err(|e| {
                        ProjectIoError::io("read journal", &self.layout.journal_dir(), e)
                    })?,
                );
        }
        Ok(())
    }

    // ----- journal (WP-1C.02) ----------------------------------------------------------------

    /// Unsaved work found in the journal and not yet recovered or discarded.
    #[must_use]
    pub fn recovery(&self) -> Option<RecoveryAvailable> {
        self.pending_journal.as_ref()?.available()
    }

    /// Replays the journal onto `base` (the project [`open`](Self::open) returned) and decides
    /// that the journal continues after the last replayed transaction. Call it on every
    /// editable open: with nothing to recover it returns `base` at the saved revision. After a
    /// crash, a torn last record (never an acknowledged transaction) is dropped; corrupt
    /// records stop the replay and their segment is preserved in `.cutweave/backups/` when the
    /// journal starts. See [`Recovered::configure`] to build the doc actor from the result.
    pub fn recover(
        &mut self,
        base: &Project,
        options: &RecoverOptions,
    ) -> Result<Recovered, ProjectIoError> {
        let pending = self.pending_for_decision()?;
        Ok(pending.recover(base, options, false))
    }

    /// Discards the unsaved work in the journal: the doc actor continues from the saved
    /// document `base`. The discarded journal is preserved in `.cutweave/backups/` (never
    /// deleted), and requests remembered from before the last save stay deduplicated.
    pub fn discard_recovery(
        &mut self,
        base: &Project,
        options: &RecoverOptions,
    ) -> Result<Recovered, ProjectIoError> {
        let pending = self.pending_for_decision()?;
        Ok(pending.recover(base, options, true))
    }

    fn pending_for_decision(&mut self) -> Result<&mut PendingJournal, ProjectIoError> {
        if let Some(reason) = &self.read_only {
            return Err(ProjectIoError::ReadOnly(reason.clone()));
        }
        if self.journal.is_some() {
            return Err(JournalError::AlreadyStarted.into());
        }
        self.pending_journal
            .as_mut()
            .ok_or(ProjectIoError::Journal(JournalError::Closed))
    }

    /// Starts the journal after [`recover`](Self::recover) or
    /// [`discard_recovery`](Self::discard_recovery); pass [`Journal::sink`] to
    /// [`DocConfig::with_journal`](cutweave_commands::DocConfig::with_journal). From now on save
    /// with [`save_snapshot`](Self::save_snapshot).
    pub fn start_journal(&mut self, config: JournalConfig) -> Result<Journal, ProjectIoError> {
        if !self.pending_for_decision()?.decided() {
            return Err(JournalError::RecoveryPending.into());
        }
        let Some(pending) = self.pending_journal.take() else {
            return Err(JournalError::Closed.into());
        };
        match pending.start(config) {
            Ok(journal) => {
                self.pending_journal = None;
                self.journal = Some(journal.clone());
                Ok(journal)
            }
            Err(e) => {
                // Scan again, so the decision can be retried.
                if let Some(sha) = &self.disk_sha256 {
                    self.pending_journal = PendingJournal::scan(
                        &self.layout.journal_dir(),
                        &self.layout.backups_dir(),
                        sha.clone(),
                    )
                    .ok();
                }
                Err(e.into())
            }
        }
    }

    /// The running journal.
    #[must_use]
    pub fn journal(&self) -> Option<&Journal> {
        self.journal.as_ref()
    }

    /// Keeps the original of a migrated document in `.cutweave/backups/` before the first save
    /// over it, so a bad migration can never cost the user their file.
    fn preserve_premigration_original(&mut self) -> Result<(), ProjectIoError> {
        let Some(version) = self.pending_premigration_backup else {
            return Ok(());
        };
        let name = self
            .layout
            .document
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let backup = self
            .layout
            .backups_dir()
            .join(format!("{name}.pre-migration-v{version}"));
        self.layout.ensure_inside(&backup)?;
        if !backup.exists() {
            let original = fs::read(&self.layout.document)
                .map_err(|e| ProjectIoError::io("read original", &self.layout.document, e))?;
            fs::create_dir_all(self.layout.backups_dir())
                .map_err(|e| ProjectIoError::io("create directory", &backup, e))?;
            write_atomic(&backup, &original)
                .map_err(|e| ProjectIoError::io("write backup", &backup, e))?;
        }
        self.pending_premigration_backup = None;
        Ok(())
    }

    /// Saves a copy of the project as a new project folder and moves this store (and its lock) to
    /// it. `path_rel` is recomputed for the new location. The old folder is left untouched.
    /// Contents of `generated/` and `compositions/` are not copied (out of scope for WP-1C.01).
    pub fn save_as(&mut self, folder: &Path, project: &Project) -> Result<(), ProjectIoError> {
        if self.journal.is_some() {
            return Err(JournalError::RevisionRequired.into());
        }
        let mut next = self.save_as_target(folder)?;
        next.save(project)?;
        // Swapping releases the old lock (Drop) only after the new copy is safely on disk.
        std::mem::swap(self, &mut next);
        Ok(())
    }

    /// [`save_as`](Self::save_as) for a journaled store: the document goes to the new folder,
    /// then the running journal moves with it (a new segment there over this revision), and
    /// the old folder's journal goes to its backups, so the old project reopens at its last
    /// save. Transactions committed meanwhile follow the journal.
    pub fn save_as_snapshot(
        &mut self,
        folder: &Path,
        snapshot: &DocSnapshot,
    ) -> Result<(), ProjectIoError> {
        let Some(journal) = self.journal.clone() else {
            return self.save_as(folder, &snapshot.doc);
        };
        if snapshot.rev != snapshot.committed_rev {
            return Err(JournalError::NotCommitted {
                rev: snapshot.rev,
                committed_rev: snapshot.committed_rev,
            }
            .into());
        }
        let mut next = self.save_as_target(folder)?;
        next.write_document(&snapshot.doc, None, &mut |_| Ok(()))?;
        next.pending_journal = None;
        let sha = next.disk_sha256.clone().unwrap_or_default();
        let rotation =
            journal.relocate(&next.layout.journal_dir(), snapshot.committed_rev, &sha)?;
        journal.retire_into(rotation, &self.layout.backups_dir());
        next.journal = self.journal.take();
        std::mem::swap(self, &mut next);
        Ok(())
    }

    fn save_as_target(&self, folder: &Path) -> Result<ProjectStore, ProjectIoError> {
        // (A fresh store: no journal, no pending decision.)
        if matches!(self.read_only, Some(ReadOnlyReason::NewerSchema { .. })) {
            return Err(ProjectIoError::ReadOnly(
                self.read_only.clone().unwrap_or(ReadOnlyReason::Requested),
            ));
        }
        let layout = ProjectLayout::for_folder(folder)?;
        if layout.document.exists() {
            return Err(ProjectIoError::AlreadyExists(layout.document));
        }
        if layout.root == self.layout.root {
            return Err(ProjectIoError::AlreadyExists(layout.root));
        }
        make_layout_dirs(&layout)?;
        let acquired = ProjectLock::acquire(&layout, false)?;
        Ok(ProjectStore::writable(layout, acquired.lock))
    }

    /// Syncs and closes the journal, then releases the lock. Also happens on drop; use this to
    /// observe errors. Saves nothing: unsaved work stays in the journal and is offered for
    /// recovery on the next open.
    pub fn close(mut self) -> Result<(), ProjectIoError> {
        if let Some(journal) = self.journal.take() {
            journal.close()?;
        }
        match self.lock.take() {
            Some(mut lock) => lock
                .release()
                .map_err(|e| ProjectIoError::io("release lock", &self.layout.lock_file(), e)),
            None => Ok(()),
        }
    }

    /// `project` as it is written to disk: `path_rel` recomputed against this folder.
    #[must_use]
    pub fn persisted_form(&self, project: &Project) -> Project {
        let mut out = project.clone();
        for (id, asset) in &project.media {
            if let Some(abs) = &asset.path_abs
                && let Some(slot) = out.media.get_mut(id)
            {
                slot.path_rel = relative_to(&self.layout.root, Path::new(abs));
            }
        }
        out
    }

    /// The project folder layout.
    #[must_use]
    pub fn layout(&self) -> &ProjectLayout {
        &self.layout
    }
    /// The project folder.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.layout.root
    }
    /// The `.cutweave` document.
    #[must_use]
    pub fn document_path(&self) -> &Path {
        &self.layout.document
    }
    /// Why saving is refused, or `None` when the store is writable.
    #[must_use]
    pub fn read_only(&self) -> Option<&ReadOnlyReason> {
        self.read_only.as_ref()
    }
    /// The lock record this store holds (`None` when read-only).
    #[must_use]
    pub fn lock_info(&self) -> Option<&LockInfo> {
        self.lock.as_ref().map(ProjectLock::info)
    }
}

impl Drop for ProjectStore {
    /// Closes the journal (synced) before the lock is released: once the store is gone, no
    /// transaction can be journaled into a folder this process no longer holds.
    fn drop(&mut self) {
        if let Some(journal) = self.journal.take()
            && let Err(e) = journal.close()
        {
            tracing::error!(error = %e, "the journal did not close cleanly");
        }
    }
}

fn make_layout_dirs(layout: &ProjectLayout) -> Result<(), ProjectIoError> {
    for dir in [
        layout.root.clone(),
        layout.generated_dir(),
        layout.compositions_dir(),
        layout.journal_dir(),
        layout.backups_dir(),
    ] {
        layout.ensure_inside(&dir)?;
        fs::create_dir_all(&dir).map_err(|e| ProjectIoError::io("create directory", &dir, e))?;
    }
    Ok(())
}

/// Finds the document for `path`: a `.cutweave` file, or a folder holding one.
fn locate(path: &Path) -> Result<ProjectLayout, ProjectIoError> {
    let meta = fs::metadata(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => ProjectIoError::NotFound(path.to_path_buf()),
        _ => ProjectIoError::io("stat", path, e),
    })?;
    if meta.is_file() {
        return ProjectLayout::for_document(path);
    }
    let named = ProjectLayout::for_folder(path)?;
    if named.document.is_file() {
        return Ok(named);
    }
    let entries = fs::read_dir(&named.root)
        .map_err(|e| ProjectIoError::io("read directory", &named.root, e))?;
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case(DOCUMENT_EXTENSION))
        })
        .collect();
    match found.len() {
        0 => Err(ProjectIoError::NotFound(named.root)),
        1 => ProjectLayout::for_document(&found.remove(0)),
        _ => Err(ProjectIoError::Ambiguous(named.root)),
    }
}

/// Resolves media against the filesystem: absolute path first, then the folder-relative path (the
/// project was moved). Newly found locations are written into `project` for editable opens.
fn resolve_media(
    layout: &ProjectLayout,
    project: &mut Project,
    editable: bool,
    report: &mut OpenReport,
) {
    let ids: Vec<MediaId> = project.media.keys().copied().collect();
    for id in ids {
        let Some(asset) = project.media.get(&id) else {
            continue;
        };
        if asset.path_abs.is_none() && asset.path_rel.is_none() {
            continue; // not a file asset
        }
        let abs_ok = asset
            .path_abs
            .as_deref()
            .is_some_and(|p| Path::new(p).exists());
        if abs_ok {
            continue;
        }
        let rel_hit = asset
            .path_rel
            .as_deref()
            .and_then(|r| resolve_relative(&layout.root, r))
            .filter(|p| p.exists());
        match rel_hit {
            Some(found) => {
                let to = found.to_string_lossy().into_owned();
                report.relinked_media.push(RelinkedMedia {
                    id,
                    from: asset.path_abs.clone(),
                    to: to.clone(),
                });
                if editable && let Some(a) = project.media.get_mut(&id) {
                    a.path_abs = Some(to);
                }
            }
            None => report.missing_media.push(MissingMedia {
                id,
                path_abs: asset.path_abs.clone(),
                path_rel: asset.path_rel.clone(),
            }),
        }
    }
}

#[cfg(test)]
mod tests;
