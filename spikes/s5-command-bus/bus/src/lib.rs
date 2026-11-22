//! Spike S5 command-bus runtime (WP-0H.01). Throwaway; see `../README.md`.
//!
//! Stand-in for `cutweave-model` + `cutweave-commands` (arch §4, §5):
//!
//! - [`time`]: flicks [`Time`] newtype with the ADR-015 wire format;
//! - [`ids`]: prefixed string IDs and the deterministic-id generator;
//! - [`doc`]: a toy in-memory project document;
//! - [`tx`]: [`Tx`], which records forward + inverse patch ops for every mutation;
//! - [`registry`]: the `#[command]` / `#[query]` registry that MCP (WP-0H.02) iterates;
//! - [`engine`]: the single-writer doc actor (dedup, undo/redo, batches, events);
//! - [`codegen`]: JSON Schema catalog + TypeScript generation.

// Lets the attribute macros emit `::s5_bus::...` paths that also resolve inside this crate.
extern crate self as s5_bus;

pub mod codegen;
pub mod doc;
mod edit_ops;
pub mod engine;
pub mod error;
pub mod ids;
pub mod registry;
pub mod time;
pub mod tx;

pub use engine::{Actor, Caller, Engine, EngineConfig};
pub use error::{CmdError, ConflictReason, EntityKind};
pub use ids::{ItemId, MediaId, SequenceId, TrackId};
pub use registry::{
    Annotations, Command, CommandEntry, CommandMeta, Described, EngineOp, EntryKind, McpTier,
    Query, Registry,
};
pub use s5_bus_macros::{command, query, wire};
pub use time::{FrameRate, Time};
pub use tx::{Patch, Rev, Tx};

/// Re-exports used by macro-generated code. Not part of the API.
#[doc(hidden)]
pub mod __private {
    pub use inventory;
    pub use schemars;
    pub use serde;
    pub use serde_json;
    pub use specta;
}
