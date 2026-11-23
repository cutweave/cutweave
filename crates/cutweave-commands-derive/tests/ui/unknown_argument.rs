// SPDX-License-Identifier: GPL-3.0-or-later
use cutweave_commands::command;

#[command(name = "clip.split", summary = "Split", undoable, mcp = "core", persistent, annotations(destructive = false))]
pub struct Split {
    pub at: i32,
}

fn main() {}
