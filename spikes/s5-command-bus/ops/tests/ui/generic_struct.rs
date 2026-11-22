use s5_bus::command;

#[command(name = "clip.split", summary = "Split a clip", undoable, mcp = "core", annotations(destructive = false))]
pub struct SplitClip<T> {
    pub at: T,
}

fn main() {}
