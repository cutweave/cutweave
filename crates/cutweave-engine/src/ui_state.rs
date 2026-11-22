// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistence of the `view` namespace state in `<project>/.cutweave/ui-state.json`
//! (WP-1C.04; arch §5.1, §17).
//!
//! [`UiStateStore`] owns the in-memory [`ViewCtx`] (the context `view.*` commands run against)
//! and its file. View state is separate from the document: not undoable, not journaled, no
//! `rev`. The document actor hosts one store per open project and:
//!
//! 1. builds it with [`UiStateStore::open_for`] on `project.open` (never fails),
//! 2. runs `view.*` commands through [`UiStateStore::with_ctx`] and forwards
//!    [`ViewCtx::take_event`] as `view.changed` to the other clients,
//! 3. forwards document snapshots with [`UiStateStore::set_doc`] after each commit,
//! 4. calls [`UiStateStore::poll`] when [`UiStateStore::deadline`] passes (debounced writes),
//! 5. calls [`UiStateStore::flush`] with each project save and on close,
//! 6. emits [`UiStateStore::take_warnings`] as warning events.
//!
//! Time is passed in (`Instant`), so debouncing is testable without sleeping.
//!
//! Writes are atomic (temp file, `fsync`, rename). Reading is tolerant: a missing, corrupt or
//! old file yields defaults plus a warning and never fails the project open; unknown keys are
//! preserved on the next write.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cutweave_model::Project;
use cutweave_ops::view::ViewCtx;

use crate::project_io::{ProjectIoError, ProjectLayout, ProjectStore, atomic::write_atomic};

/// A change is written this long after the last change in a burst.
pub const DEBOUNCE: Duration = Duration::from_millis(500);
/// A continuous burst (scrubbing) is still written at least this often.
pub const MAX_WAIT: Duration = Duration::from_secs(2);

/// The view state of one open project and its file.
#[derive(Debug)]
pub struct UiStateStore {
    path: PathBuf,
    meta_dir: PathBuf,
    writable: bool,
    ctx: ViewCtx,
    saved_rev: u32,
    first_dirty: Option<Instant>,
    last_change: Option<Instant>,
    warnings: Vec<String>,
    writes: u64,
}

impl UiStateStore {
    /// Loads the view state of the project held by `store` (read-only stores never write).
    #[must_use]
    pub fn open_for(store: &ProjectStore, doc: &Project) -> UiStateStore {
        UiStateStore::open(store.layout(), doc, store.read_only().is_none())
    }

    /// Loads `layout`'s `ui-state.json`, validating selection IDs against `doc`. Never fails:
    /// problems become [`take_warnings`](Self::take_warnings) and defaults.
    #[must_use]
    pub fn open(layout: &ProjectLayout, doc: &Project, writable: bool) -> UiStateStore {
        let path = layout.ui_state_file();
        let mut warnings = Vec::new();
        let ctx = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(value) => {
                    let (ctx, w) = ViewCtx::from_json(&value, doc.clone());
                    warnings.extend(w);
                    ctx
                }
                Err(e) => {
                    warnings.push(format!(
                        "ui-state.json is corrupt ({e}); using default view state"
                    ));
                    ViewCtx::new(doc.clone())
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => ViewCtx::new(doc.clone()),
            Err(e) => {
                warnings.push(format!(
                    "ui-state.json could not be read ({e}); using default view state"
                ));
                ViewCtx::new(doc.clone())
            }
        };
        UiStateStore {
            path,
            meta_dir: layout.meta_dir(),
            writable,
            saved_rev: ctx.rev(),
            ctx,
            first_dirty: None,
            last_change: None,
            warnings,
            writes: 0,
        }
    }

    /// The view context, read-only.
    #[must_use]
    pub fn ctx(&self) -> &ViewCtx {
        &self.ctx
    }

    /// Runs `f` (typically `registry.invoke_engine_json(.., ctx, ..)`) against the context at
    /// time `now`. A change schedules a debounced write.
    pub fn with_ctx<R>(&mut self, now: Instant, f: impl FnOnce(&mut ViewCtx) -> R) -> R {
        let before = self.ctx.rev();
        let out = f(&mut self.ctx);
        if self.ctx.rev() != before {
            self.mark(now);
        }
        out
    }

    /// Hands over a newer document snapshot; selected entities that no longer exist are
    /// dropped. Returns whether the selection changed.
    pub fn set_doc(&mut self, now: Instant, doc: &Project) -> bool {
        self.with_ctx(now, |ctx| ctx.set_doc(doc))
    }

    fn mark(&mut self, now: Instant) {
        self.first_dirty.get_or_insert(now);
        self.last_change = Some(now);
    }

    /// Whether the state differs from what is on disk.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.writable && self.ctx.rev() != self.saved_rev
    }

    /// When the pending write is due (`None` if nothing is pending): [`DEBOUNCE`] after the
    /// last change, but never later than [`MAX_WAIT`] after the first unwritten one.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        if !self.is_dirty() {
            return None;
        }
        let (first, last) = (self.first_dirty?, self.last_change?);
        Some((last + DEBOUNCE).min(first + MAX_WAIT))
    }

    /// Writes if the debounce deadline has passed. Returns whether it wrote. A failed write
    /// keeps the state dirty and retries one [`DEBOUNCE`] later.
    pub fn poll(&mut self, now: Instant) -> Result<bool, ProjectIoError> {
        match self.deadline() {
            Some(due) if due <= now => match self.flush() {
                Err(e) => {
                    self.first_dirty = Some(now);
                    self.last_change = Some(now);
                    Err(e)
                }
                ok => ok,
            },
            _ => Ok(false),
        }
    }

    /// Writes now if dirty (with each project save and on close). Returns whether it wrote.
    pub fn flush(&mut self) -> Result<bool, ProjectIoError> {
        if !self.is_dirty() {
            return Ok(false);
        }
        let mut bytes =
            serde_json::to_vec_pretty(&self.ctx.to_json()).map_err(ProjectIoError::Serialize)?;
        bytes.push(b'\n');
        fs::create_dir_all(&self.meta_dir)
            .map_err(|e| ProjectIoError::io("create .cutweave", &self.meta_dir, e))?;
        write_atomic(&self.path, &bytes)
            .map_err(|e| ProjectIoError::io("write ui-state.json", &self.path, e))?;
        self.saved_rev = self.ctx.rev();
        self.first_dirty = None;
        self.last_change = None;
        self.writes += 1;
        Ok(true)
    }

    /// Flushes and consumes the store, to observe the error (drop also flushes, silently).
    pub fn close(mut self) -> Result<(), ProjectIoError> {
        self.flush().map(drop)
    }

    /// Problems found while loading, once (for warning events).
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// How many times the file was written (observability, tests).
    #[must_use]
    pub fn writes(&self) -> u64 {
        self.writes
    }
}

impl Drop for UiStateStore {
    fn drop(&mut self) {
        // Best effort: the project is closing and there is nobody left to tell.
        let _ = self.flush();
    }
}
