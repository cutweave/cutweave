// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-ops`: built-in command and query implementations.
//!
//! Each command namespace lives in its own module and exposes `register(&mut Registry)`;
//! [`register`] calls them all (explicit registration, arch §5.1). Currently: [`view`]
//! (WP-1C.04), the UI view-state namespace.

use cutweave_commands::{Registry, RegistryError};

pub mod view;

/// Registers every built-in command and query of this crate.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    view::register(registry)
}

/// Returns the crate name.
#[must_use]
pub const fn crate_name() -> &'static str {
    "cutweave-ops"
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "cutweave-ops");
    }

    #[test]
    fn registers_without_conflicts() {
        let mut registry = Registry::new();
        register(&mut registry).expect("register");
        assert!(registry.get("view.get").is_some());
    }
}
