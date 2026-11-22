// SPDX-License-Identifier: GPL-3.0-or-later
use cutweave_commands::command;

#[command(name = "widget.spin", summary = "Spin", undoable, mcp = "core", annotations(destructive = false))]
pub struct Spin {
    pub turns: i32,
}

fn main() {}
