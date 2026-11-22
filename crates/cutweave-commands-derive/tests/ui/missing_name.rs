// SPDX-License-Identifier: GPL-3.0-or-later
use cutweave_commands::command;

#[command(summary = "Split", undoable, mcp = "core", annotations(destructive = false))]
pub struct Split {
    pub at: i32,
}

fn main() {}
