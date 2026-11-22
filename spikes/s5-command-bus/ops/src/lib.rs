//! Spike S5 toy commands and queries: stand-in for `cutweave-ops` (arch §3: model <- commands <- ops).
//!
//! Living in a separate crate from the registry proves the `inventory` registration works across
//! crates. Anything that needs the full registry must reference this crate (see [`registry`]).

mod commands;
mod queries;

pub use commands::*;
pub use queries::*;

/// The global registry, guaranteed to include this crate's entries.
///
/// `inventory` entries from a crate that the final binary never references are not linked.
/// Calling this function from a binary is the explicit "link me" reference.
#[must_use]
pub fn registry() -> &'static s5_bus::Registry {
    s5_bus::Registry::global()
}
