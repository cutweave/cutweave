use s5_bus::command;

#[command(name = "clip.split", summary = "Split a clip", undoable, mcp = "core", annotations(destructive = false))]
pub enum SplitClip {
    At(s5_bus::Time),
}

fn main() {}
