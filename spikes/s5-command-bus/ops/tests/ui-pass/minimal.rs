// The smallest well-formed command and query.
use s5_bus::{CmdError, Command, Query, Tx, command, doc::Doc, query, wire};

/// Rename the project.
#[command(
    name = "project.rename",
    summary = "Rename the project",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct Rename {
    /// New name.
    pub name: String,
}

/// Empty result.
#[wire]
pub struct Done {}

impl Command for Rename {
    type Output = Done;
    fn apply(&self, tx: &mut Tx<'_>) -> Result<Done, CmdError> {
        tx.set_meta(s5_bus::doc::ProjectMeta { name: self.name.clone() });
        Ok(Done {})
    }
}

/// Count items.
#[query(name = "item.count", summary = "Count timeline items", mcp = "hidden")]
pub struct CountItems {}

/// Item count.
#[wire]
pub struct Count {
    /// Number of items.
    pub count: u32,
}

impl Query for CountItems {
    type Output = Count;
    fn run(&self, doc: &Doc) -> Result<Count, CmdError> {
        Ok(Count { count: u32::try_from(doc.items.len()).unwrap_or(u32::MAX) })
    }
}

fn main() {
    assert!(s5_bus::Registry::global().get("project.rename").is_some());
}
