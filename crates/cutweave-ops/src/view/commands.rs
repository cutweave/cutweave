// SPDX-License-Identifier: GPL-3.0-or-later
//! The `view.*` engine commands (arch §5.1, §14). All run against a [`ViewCtx`], are never
//! undoable or journaled, and are exposed through the MCP catalog (`view_get` / `view_set`).
//!
//! `view.get` is an engine command rather than a query because a [`Query`](cutweave_commands::Query)
//! only sees the document snapshot; it is idempotent and has no side effects. The registry
//! reserves `readOnlyHint` for queries.

use cutweave_commands::{CmdError, EngineCommand, command, wire};
use cutweave_model::{SequenceId, TimeInput};

use super::state::{SelectMode, SelectionRef, UiState, ViewCtx, ViewPatch};

/// Returns the current view state: playhead, zoom, scroll, selection, panel layout, preview
/// quality, master gain and the source marks (in and out per media asset).
#[command(
    name = "view.get",
    summary = "Get the UI view state (playhead, zoom, selection, ...)",
    view,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewGet {}

impl EngineCommand for ViewGet {
    type Context = ViewCtx;
    type Output = UiState;
    fn run(&self, ctx: &mut ViewCtx) -> Result<UiState, CmdError> {
        Ok(ctx.state().clone())
    }
}

/// Merges a partial update into the view state: present fields replace the current value,
/// absent ones are kept. Nothing is applied if any field is invalid. Not undoable; never
/// changes the document.
#[command(
    name = "view.set",
    summary = "Update the UI view state (partial merge; not undoable)",
    view,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewSet {
    /// The fields to change.
    pub patch: ViewPatch,
}

impl EngineCommand for ViewSet {
    type Context = ViewCtx;
    type Output = UiState;
    fn run(&self, ctx: &mut ViewCtx) -> Result<UiState, CmdError> {
        ctx.apply_patch(&self.patch)?;
        Ok(ctx.state().clone())
    }
}

/// Moves the playhead.
#[command(
    name = "view.set_playhead",
    summary = "Move the playhead",
    view,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewSetPlayhead {
    /// New playhead, in flicks or seconds text; `>= 0`.
    pub at: TimeInput,
}

impl EngineCommand for ViewSetPlayhead {
    type Context = ViewCtx;
    type Output = UiState;
    fn run(&self, ctx: &mut ViewCtx) -> Result<UiState, CmdError> {
        ctx.set_playhead(self.at)?;
        Ok(ctx.state().clone())
    }
}

/// Changes the selection. Every ID that is added must exist in the project.
#[command(
    name = "view.select",
    summary = "Select, add, remove or toggle entities in the selection",
    view,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ViewSelect {
    /// The entities to act on.
    pub ids: Vec<SelectionRef>,
    /// How to combine them with the current selection (default `replace`).
    pub mode: Option<SelectMode>,
}

impl EngineCommand for ViewSelect {
    type Context = ViewCtx;
    type Output = UiState;
    fn run(&self, ctx: &mut ViewCtx) -> Result<UiState, CmdError> {
        ctx.select(&self.ids, self.mode.unwrap_or_default())?;
        Ok(ctx.state().clone())
    }
}

/// Sets the timeline zoom.
#[command(
    name = "view.set_zoom",
    summary = "Set the timeline zoom (pixels per second)",
    view,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewSetZoom {
    /// Pixels per second, `0.001..=100000`.
    pub zoom: f64,
}

impl EngineCommand for ViewSetZoom {
    type Context = ViewCtx;
    type Output = UiState;
    fn run(&self, ctx: &mut ViewCtx) -> Result<UiState, CmdError> {
        ctx.set_zoom(self.zoom)?;
        Ok(ctx.state().clone())
    }
}

/// Result of `view.set_active_sequence`.
#[wire]
pub struct ViewSetActiveSequenceOutput {
    /// The sequence the editor shows now.
    pub sequence: SequenceId,
}

/// Chooses the sequence the editor shows (WP-2B.01). Not undoable; never changes the project.
#[command(
    name = "view.set_active_sequence",
    summary = "Show a sequence in the editor",
    view,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ViewSetActiveSequence {
    /// The sequence; it must exist.
    pub sequence: SequenceId,
}

impl EngineCommand for ViewSetActiveSequence {
    type Context = ViewCtx;
    type Output = ViewSetActiveSequenceOutput;
    fn run(&self, ctx: &mut ViewCtx) -> Result<ViewSetActiveSequenceOutput, CmdError> {
        ctx.set_active_sequence(self.sequence)?;
        Ok(ViewSetActiveSequenceOutput {
            sequence: self.sequence,
        })
    }
}
