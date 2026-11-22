// SPDX-License-Identifier: GPL-3.0-or-later
use cutweave_commands::query;

#[query(name = "timeline.get", summary = "Get the timeline", mcp = "core", annotations(destructive = false))]
pub struct GetTimeline {
    pub depth: u32,
}

fn main() {}
