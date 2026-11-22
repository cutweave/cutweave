use s5_bus::command;

#[command(name = "ai.set_key", summary = "Store a key", undoable = false, scope = "ui", mcp = "core", annotations(destructive = false))]
pub struct SetKey {
    pub key: String,
}

fn main() {}
