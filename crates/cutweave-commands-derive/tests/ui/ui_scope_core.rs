// SPDX-License-Identifier: GPL-3.0-or-later
use cutweave_commands::command;

#[command(
    name = "ai.set_key",
    summary = "Store an API key",
    undoable = false,
    mcp = "core",
    scope = "ui",
    annotations(destructive = false)
)]
pub struct SetKey {
    #[secret]
    pub key: String,
}

fn main() {}
