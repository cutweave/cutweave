use s5_bus::command;

// New namespaces require an ADR (arch §5.1), so the macro only accepts the documented list.
#[command(name = "clips.split", summary = "Split a clip", undoable, mcp = "core", annotations(destructive = false))]
pub struct SplitClip {
    pub at: s5_bus::Time,
}

fn main() {}
