use s5_bus::query;

#[query(name = "timeline.get", summary = "Get the timeline", undoable, mcp = "core")]
pub struct GetTimeline {}

fn main() {}
