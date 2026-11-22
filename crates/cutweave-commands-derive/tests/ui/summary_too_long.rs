// SPDX-License-Identifier: GPL-3.0-or-later
use cutweave_commands::command;

#[command(
    name = "clip.split",
    summary = "Split an item at a timeline time, keeping both halves linked and preserving all effects and keyframes",
    undoable,
    mcp = "core",
    annotations(destructive = false)
)]
pub struct Split {
    pub at: i32,
}

fn main() {}
