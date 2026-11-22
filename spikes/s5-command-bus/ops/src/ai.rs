// SPDX-License-Identifier: GPL-3.0-or-later
//! `ai.set_key`: a UI-only engine op (arch §6 "UI-only scopes"). Added in WP-0H.02 so the MCP
//! security tests can prove that an agent credential is refused a UI-only scope even when it
//! reaches the command through the generic `cutweave_execute` tool.

use s5_bus::{
    CmdError, EngineOp, command,
    engine::{Caller, Engine},
    wire,
};

/// Store an AI provider API key in the engine (spike stand-in for the OS keychain). The key is
/// never echoed in results, events or the document.
#[command(
    name = "ai.set_key",
    summary = "Store an AI provider API key (UI only)",
    engine,
    undoable = false,
    scope = "ui",
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct SetAiKey {
    /// Provider id, e.g. `anthropic`.
    pub provider: String,
    /// The secret.
    pub key: String,
}

/// Result of `ai.set_key` (never contains the key).
#[wire]
pub struct SetAiKeyOutput {
    /// Provider id.
    pub provider: String,
    /// Always true on success.
    pub stored: bool,
}

impl EngineOp for SetAiKey {
    type Output = SetAiKeyOutput;

    fn run(&self, engine: &mut Engine, _caller: &Caller) -> Result<SetAiKeyOutput, CmdError> {
        let provider = self.provider.trim();
        if provider.is_empty() {
            return Err(CmdError::invalid("/provider", "must not be empty"));
        }
        if self.key.is_empty() {
            return Err(CmdError::invalid("/key", "must not be empty"));
        }
        engine.set_secret(provider, self.key.clone());
        Ok(SetAiKeyOutput {
            provider: provider.to_owned(),
            stored: true,
        })
    }
}
