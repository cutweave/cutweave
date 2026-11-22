use s5_bus::command;

#[command(name = "clip.Split-Now", summary = "Split a clip", undoable, mcp = "core", annotations(destructive = false))]
pub struct SplitClip {
    pub at: s5_bus::Time,
}

fn main() {}
