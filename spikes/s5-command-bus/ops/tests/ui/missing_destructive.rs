use s5_bus::command;

// MCP's destructiveHint defaults to true; every command must state it.
#[command(name = "clip.split", summary = "Split a clip", undoable, mcp = "core", annotations(idempotent = false))]
pub struct SplitClip {
    pub at: s5_bus::Time,
}

fn main() {}
