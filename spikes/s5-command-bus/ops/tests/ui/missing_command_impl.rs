use s5_bus::command;

// The attribute registers the type, so forgetting `impl Command` is a compile error.
#[command(name = "clip.split", summary = "Split a clip", undoable, mcp = "core", annotations(destructive = false))]
pub struct SplitClip {
    pub at: s5_bus::Time,
}

fn main() {}
