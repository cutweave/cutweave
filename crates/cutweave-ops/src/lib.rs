// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-ops`: built-in command and query implementations.
//!
//! Each command namespace lives in its own module and exposes `register(&mut Registry)`;
//! [`register`] calls them all (explicit registration, arch §5.1). Currently: [`view`]
//! (WP-1C.04), the UI view-state namespace, and the WP-1B.05 ops: [`project`], [`media`],
//! [`track`] and [`timeline`]; the WP-1B.06 clip edits: [`clip`] (split, trim) and [`item`]
//! (move, delete, set_props, get); the WP-1D.07 background jobs: [`job`] (status, list,
//! wait, cancel) plus the import's analysis jobs ([`media::Analyzer`]); and the WP-1E.03
//! [`export`] command (`export.start`, run by the engine's export pipeline).

use cutweave_commands::{Registry, RegistryError};

pub mod audio;
pub mod clip;
pub mod detail;
pub mod export;
pub mod item;
pub mod job;
pub mod keyframe;
pub mod media;
pub mod project;
pub mod report;
pub mod sequence;
#[cfg(feature = "testing")]
pub mod testing;
pub mod timeline;
pub mod track;
mod util;
pub mod view;
pub mod visual;

pub use detail::Detail;
pub use util::MAX_NAME_CHARS;

/// Registers every built-in command and query of this crate.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    project::register(registry)?;
    media::register(registry)?;
    track::register(registry)?;
    sequence::register(registry)?;
    timeline::register(registry)?;
    clip::register(registry)?;
    item::register(registry)?;
    audio::register(registry)?;
    keyframe::register(registry)?;
    job::register(registry)?;
    export::register(registry)?;
    visual::register(registry)?;
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
