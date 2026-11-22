use s5_bus::command;

// Params must serialize as a JSON object (MCP inputSchema.type = "object").
#[command(name = "clip.split", summary = "Split a clip", undoable, mcp = "core", annotations(destructive = false))]
pub struct SplitClip(pub s5_bus::Time);

fn main() {}
