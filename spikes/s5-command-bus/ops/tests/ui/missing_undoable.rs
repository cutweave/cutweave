use s5_bus::command;

// Forgetting `undoable` must not silently produce a non-undoable command.
#[command(name = "clip.split", summary = "Split a clip", mcp = "core", annotations(destructive = false))]
pub struct SplitClip {
    pub at: s5_bus::Time,
}

fn main() {}
