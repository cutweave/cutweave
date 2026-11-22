use s5_bus::command;

#[command(name = "clip.split", summary = "Split a clip", undoable, mcp = "primary", annotations(destructive = false))]
pub struct SplitClip {
    pub at: s5_bus::Time,
}

fn main() {}
