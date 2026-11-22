use s5_bus::command;

#[command(
    name = "clip.split",
    summary = "Split a clip at a timeline time, keeping the left part's id and creating a new id for the right part, which inherits all properties",
    undoable,
    mcp = "core",
    annotations(destructive = false)
)]
pub struct SplitClip {
    pub at: s5_bus::Time,
}

fn main() {}
