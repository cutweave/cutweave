// SPDX-License-Identifier: GPL-3.0-or-later
use cutweave_commands::command;

#[command(name = "clip.select", summary = "Select clips", view, mcp = "catalog")]
pub struct Select {
    pub items: Vec<String>,
}

fn main() {}
