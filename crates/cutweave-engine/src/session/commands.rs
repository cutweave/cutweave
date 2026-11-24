// SPDX-License-Identifier: GPL-3.0-or-later
//! The session commands (WP-1I.04; WP-1C.01/1C.02 interfaces): `project.open`, `project.save`,
//! `project.save_as`, `project.close`, `project.recover`, `project.discard_recovery`.
//!
//! They act on the engine session, not on the document, so they are engine commands whose
//! context is a [`SessionCall`]; none is undoable or journaled (they change which document the
//! engine holds, or write it to disk). The document commands of the `project` namespace
//! (`project.new`, `project.info`, `project.rename`) live in `cutweave-ops`.

use std::path::Path;
use std::sync::Arc;

use cutweave_commands::{
    ActorCtx, CmdError, EngineCommand, Registry, RegistryError, Rev, WallTime, command, wire,
};
use cutweave_model::MediaId;

use super::ProjectStatus;
use crate::autosave::AutosavePreference;
use crate::engine::Core;

/// Registers the session commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<ProjectOpen>()?
        .register::<ProjectSave>()?
        .register::<ProjectSaveAs>()?
        .register::<ProjectClose>()?
        .register::<ProjectRecover>()?
        .register::<ProjectDiscardRecovery>()?
        .register::<ProjectSetAutosave>()?
        .register::<ProjectReportFocus>()?;
    Ok(())
}

/// What the session commands run against: the engine and the caller.
pub struct SessionCall {
    pub(crate) core: Arc<Core>,
    pub(crate) caller: ActorCtx,
}

impl std::fmt::Debug for SessionCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionCall")
            .field("caller", &self.caller)
            .finish_non_exhaustive()
    }
}

impl SessionCall {
    /// `path` (the param at `pointer`) if it is absolute and the caller may use it: anywhere
    /// for the UI credential, otherwise inside the approved roots or the open project's folder
    /// (the rule `media.import` applies, arch §14, §18).
    fn path<'a>(&self, pointer: &str, path: &'a str) -> Result<&'a Path, CmdError> {
        let p = Path::new(path);
        if path.trim().is_empty() || !p.is_absolute() {
            return Err(CmdError::invalid(pointer, "must be an absolute path"));
        }
        self.core.check_path(&self.caller, pointer, path)?;
        Ok(p)
    }
}

// ----- project.open --------------------------------------------------------------------------

/// Result of `project.open`.
#[wire]
pub struct ProjectOpenOutput {
    /// The session now.
    pub project: ProjectStatus,
    /// Media whose file was found at neither its absolute nor its project-relative path.
    pub missing_media: Vec<MediaId>,
    /// The schema version the document was migrated from, if it was.
    pub migrated_from: Option<u32>,
}

/// Opens a project (its folder or its `.cutweave` file), closing the current one first (saved
/// when autosave is on, as `project.close` does). A project open in another Cutweave process
/// fails with `Invalid` at `/path` and leaves the current project open. Unsaved work from a
/// crashed session is reported in `project.recovery`; edits are refused until
/// `project.recover` or `project.discard_recovery`. Every client must resync (`doc.reset`).
/// Agents and headless callers may only open projects inside the open project's folder and
/// the folders the user approved (`Invalid` at `/path` otherwise).
#[command(
    name = "project.open",
    summary = "Open a project folder or .cutweave file (closes the current project)",
    engine,
    undoable = false,
    journaled = false,
    mcp = "core",
    annotations(destructive = false, idempotent = false)
)]
pub struct ProjectOpen {
    /// Absolute path of the project folder or of its `.cutweave` document.
    pub path: String,
}

impl EngineCommand for ProjectOpen {
    type Context = SessionCall;
    type Output = ProjectOpenOutput;

    fn run(&self, s: &mut SessionCall) -> Result<ProjectOpenOutput, CmdError> {
        let path = s.path("/path", &self.path)?;
        s.core.open_project(path)
    }
}

// ----- project.save / project.save_as -------------------------------------------------------

/// Result of `project.save` and `project.save_as`.
#[wire]
pub struct ProjectSaveOutput {
    /// The project document written.
    pub path: String,
    /// The committed revision the project file now holds.
    pub rev: Rev,
}

/// Saves the last committed state of the open project to its file (open gestures and
/// proposals are not included). Fails with `Invalid` for an untitled document (use
/// `project.save_as`) and `Unsupported` for a read-only project.
#[command(
    name = "project.save",
    summary = "Save the open project's committed state to its file",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ProjectSave {}

impl EngineCommand for ProjectSave {
    type Context = SessionCall;
    type Output = ProjectSaveOutput;

    fn run(&self, s: &mut SessionCall) -> Result<ProjectSaveOutput, CmdError> {
        s.core.save_project()
    }
}

/// Saves the document as a new project folder (created; it must not hold a project yet) and
/// continues there; the old folder keeps its last save. A journaled project keeps its undo
/// history; an untitled or read-only document is written and reopened (history restarts, every
/// client resyncs on `doc.reset`). Agents and headless callers may only save inside the open
/// project's folder and the folders the user approved (`Invalid` at `/path` otherwise).
#[command(
    name = "project.save_as",
    summary = "Save the document as a new project folder and continue there",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ProjectSaveAs {
    /// Absolute path of the new project folder; the document is `<folder>/<folder name>.cutweave`.
    pub path: String,
}

impl EngineCommand for ProjectSaveAs {
    type Context = SessionCall;
    type Output = ProjectSaveOutput;

    fn run(&self, s: &mut SessionCall) -> Result<ProjectSaveOutput, CmdError> {
        let path = s.path("/path", &self.path)?;
        s.core.save_project_as(path)
    }
}

// ----- project.close -------------------------------------------------------------------------

/// Result of `project.close`.
#[wire]
pub struct ProjectCloseOutput {
    /// The document path of the project that was closed (absent if none was open).
    pub closed: Option<String>,
    /// The session now (an untitled document).
    pub project: ProjectStatus,
}

/// Closes the open project (releasing its lock) and starts an untitled document. With
/// `save: true` the committed state is saved first; with `save: false` nothing is saved (the
/// unsaved work stays in the journal and is offered on the next open); by default it is saved
/// when autosave is on. A failed save keeps the project open. Every client must resync.
#[command(
    name = "project.close",
    summary = "Close the open project (saving first if asked or if autosave is on)",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ProjectClose {
    /// Save before closing (`true`), never (`false`), or as autosave would (absent).
    pub save: Option<bool>,
}

impl EngineCommand for ProjectClose {
    type Context = SessionCall;
    type Output = ProjectCloseOutput;

    fn run(&self, s: &mut SessionCall) -> Result<ProjectCloseOutput, CmdError> {
        s.core.close_project(self.save)
    }
}

// ----- project.recover / project.discard_recovery ----------------------------------------------

/// Result of `project.recover` and `project.discard_recovery`.
#[wire]
pub struct ProjectRecoverOutput {
    /// The session now.
    pub project: ProjectStatus,
    /// Transactions restored from the journal (0 after a discard).
    pub applied: u32,
    /// Replay stopped at a damaged record (everything before it was restored).
    pub damaged: bool,
}

/// Restores the unsaved work a crashed session left in the journal (`project.recovery_available`):
/// replays it onto the saved document, with undo history, and enables editing. The restored
/// changes are unsaved until the next save or autosave. Every client must resync.
#[command(
    name = "project.recover",
    summary = "Restore unsaved changes from the journal after a crash",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ProjectRecover {}

impl EngineCommand for ProjectRecover {
    type Context = SessionCall;
    type Output = ProjectRecoverOutput;

    fn run(&self, s: &mut SessionCall) -> Result<ProjectRecoverOutput, CmdError> {
        s.core.decide_recovery(false)
    }
}

/// Discards the unsaved work a crashed session left in the journal and continues from the
/// saved document (the discarded journal is kept in `.cutweave/backups/`). Every client must
/// resync.
#[command(
    name = "project.discard_recovery",
    summary = "Discard unsaved changes from a crashed session and keep the saved project",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct ProjectDiscardRecovery {}

impl EngineCommand for ProjectDiscardRecovery {
    type Context = SessionCall;
    type Output = ProjectRecoverOutput;

    fn run(&self, s: &mut SessionCall) -> Result<ProjectRecoverOutput, CmdError> {
        s.core.decide_recovery(true)
    }
}

// ----- project.set_autosave ----------------------------------------------------------------------

/// Result of `project.set_autosave`: the preference now in force.
#[wire]
pub struct ProjectSetAutosaveOutput {
    /// Whether autosave runs.
    pub enabled: bool,
    /// The longest time, in seconds, a dirty project goes unsaved while edits continue.
    pub interval_s: u32,
    /// Backups kept in the project's `.cutweave/backups/` folder.
    pub keep_backups: u32,
}

/// Sets how this engine autosaves, for the open project and every project it opens later: the
/// editor's own preference wins over the project's `settings.autosave` (arch §17). It takes
/// effect at once, also for a project that is already dirty, and the 2 s idle save stays.
/// Not undoable, not journaled and never marks the project dirty; it is not written to the
/// project. An engine nobody sent it to keeps the project's own setting, then the defaults.
#[command(
    name = "project.set_autosave",
    summary = "Set whether, how often and with how many backups the engine autosaves",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ProjectSetAutosave {
    /// Save automatically (the idle save and the interval save), or not at all.
    pub enabled: bool,
    /// Save at least this often, in seconds, while edits continue (1 to 86400).
    pub interval_s: u32,
    /// Backups to keep in `.cutweave/backups/` (1 to 100).
    pub keep_backups: u32,
}

impl EngineCommand for ProjectSetAutosave {
    type Context = SessionCall;
    type Output = ProjectSetAutosaveOutput;

    fn run(&self, s: &mut SessionCall) -> Result<ProjectSetAutosaveOutput, CmdError> {
        if !(1..=86_400).contains(&self.interval_s) {
            return Err(CmdError::invalid(
                "/interval_s",
                "must be between 1 and 86400 seconds",
            ));
        }
        if !(1..=100).contains(&self.keep_backups) {
            return Err(CmdError::invalid(
                "/keep_backups",
                "must be between 1 and 100",
            ));
        }
        s.core.set_autosave(AutosavePreference {
            enabled: self.enabled,
            interval_s: self.interval_s,
            keep_backups: self.keep_backups,
        })?;
        Ok(ProjectSetAutosaveOutput {
            enabled: self.enabled,
            interval_s: self.interval_s,
            keep_backups: self.keep_backups,
        })
    }
}

// ----- project.report_focus -----------------------------------------------------------------------

/// Result of `project.report_focus`.
#[wire]
pub struct ProjectReportFocusOutput {
    /// When the engine recorded the focus, milliseconds since the Unix epoch (the run-file's
    /// `focused_at`).
    pub focused_at: WallTime,
}

/// The desktop app reports that its window gained the focus. The engine records the time in
/// its run-file (`focused_at`), so an agent that does not say which engine it means gets the
/// one the person is looking at (`cutweave engines`, `docs/reference/launch-protocol.md`).
/// UI credential only and never an MCP tool: it is the app applying its own state to the
/// engine (ADR-0016). Not undoable, not journaled, never marks the project dirty.
#[command(
    name = "project.report_focus",
    summary = "The window gained the focus: record it for agent engine choice",
    engine,
    scope = "ui",
    undoable = false,
    journaled = false,
    mcp = "hidden",
    annotations(destructive = false, idempotent = true)
)]
pub struct ProjectReportFocus {}

impl EngineCommand for ProjectReportFocus {
    type Context = SessionCall;
    type Output = ProjectReportFocusOutput;

    fn run(&self, s: &mut SessionCall) -> Result<ProjectReportFocusOutput, CmdError> {
        let focused_at = WallTime::from_millis(s.core.report_focus());
        Ok(ProjectReportFocusOutput { focused_at })
    }
}
