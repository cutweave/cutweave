// SPDX-License-Identifier: GPL-3.0-or-later
use cutweave_commands::query;

#[query(name = "timeline.get", summary = "Get the timeline", undoable, mcp = "core")]
pub struct GetTimeline {
    pub depth: u32,
}

fn main() {}
