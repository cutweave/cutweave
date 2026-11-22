//! `edit.undo` / `edit.redo`: engine ops registered with the same macro as document commands
//! (`#[command(engine, ...)]`), so MCP/RPC/CLI see them like any other entry.

use crate::{
    CmdError, Rev, command,
    engine::{Caller, Engine, TxSummary},
    registry::EngineOp,
    wire,
};

/// Undo the caller's most recent change. History is per actor (arch §5.2): an agent's undo never
/// reverts the user's edits, and fails with `conflict` if a later edit touched the same entities.
#[command(
    name = "edit.undo",
    summary = "Undo your most recent change",
    engine,
    undoable = false,
    mcp = "core",
    annotations(destructive = false)
)]
pub struct Undo {}

/// Redo the caller's most recently undone change.
#[command(
    name = "edit.redo",
    summary = "Redo your most recently undone change",
    engine,
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false)
)]
pub struct Redo {}

/// Result of a history step.
#[wire]
pub struct HistoryStep {
    /// The transaction that was undone/redone, or null if there was nothing to do.
    pub step: Option<TxSummary>,
    /// Document revision afterwards.
    pub rev: Rev,
}

impl EngineOp for Undo {
    type Output = HistoryStep;
    fn run(&self, engine: &mut Engine, caller: &Caller) -> Result<HistoryStep, CmdError> {
        let step = engine.undo(caller)?;
        Ok(HistoryStep {
            step,
            rev: engine.rev(),
        })
    }
}

impl EngineOp for Redo {
    type Output = HistoryStep;
    fn run(&self, engine: &mut Engine, caller: &Caller) -> Result<HistoryStep, CmdError> {
        let step = engine.redo(caller)?;
        Ok(HistoryStep {
            step,
            rev: engine.rev(),
        })
    }
}
