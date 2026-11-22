use s5_bus::command;

#[command(name = "clip.split", summary = "Split a clip", undoable, mcp = "core", annotations(destructive = false), hidden)]
pub struct SplitClip {
    pub at: s5_bus::Time,
}

fn main() {}
