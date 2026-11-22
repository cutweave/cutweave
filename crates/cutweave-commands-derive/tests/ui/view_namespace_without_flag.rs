// SPDX-License-Identifier: GPL-3.0-or-later
use cutweave_commands::command;

#[command(
    name = "view.set_zoom",
    summary = "Zoom the timeline",
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false)
)]
pub struct SetZoom {
    pub level: f64,
}

fn main() {}
