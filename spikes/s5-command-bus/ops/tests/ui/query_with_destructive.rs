use s5_bus::query;

#[query(name = "timeline.get", summary = "Get the timeline", mcp = "core", annotations(destructive = true))]
pub struct GetTimeline {}

fn main() {}
