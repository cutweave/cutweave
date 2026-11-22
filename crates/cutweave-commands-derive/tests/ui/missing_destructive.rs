// SPDX-License-Identifier: GPL-3.0-or-later
use cutweave_commands::command;

#[command(name = "item.delete", summary = "Delete items", undoable, mcp = "core", annotations(idempotent))]
pub struct DeleteItems {
    pub items: Vec<String>,
}

fn main() {}
