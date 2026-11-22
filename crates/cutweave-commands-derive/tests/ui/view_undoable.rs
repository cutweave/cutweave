// SPDX-License-Identifier: GPL-3.0-or-later
use cutweave_commands::command;

#[command(name = "view.set_zoom", summary = "Zoom the timeline", view, undoable, mcp = "catalog")]
pub struct SetZoom {
    pub level: f64,
}

fn main() {}
