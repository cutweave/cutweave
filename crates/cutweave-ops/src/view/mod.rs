// SPDX-License-Identifier: GPL-3.0-or-later
//! The `view` namespace (WP-1C.04; arch §5.1): playhead, zoom, scroll, selection, panel layout,
//! preview quality and master gain.
//!
//! View state is not undoable, not journaled, never bumps the document `rev` and never emits
//! `doc.patched`. Commands run against a [`ViewCtx`] that the engine owns separately from the
//! document; after a call the engine reads [`ViewCtx::take_event`] and sends `view.changed` to
//! the other clients. Persistence (`.cutweave/ui-state.json`) is in `cutweave_engine::ui_state`.

mod commands;
mod state;

use cutweave_commands::{Registry, RegistryError};

pub use commands::{ViewGet, ViewSelect, ViewSet, ViewSetPlayhead, ViewSetZoom};
pub use state::{
    MAX_MASTER_GAIN, MAX_PANEL_LAYOUT_BYTES, MAX_SELECTION, MAX_ZOOM, MIN_ZOOM, PreviewQuality,
    Scroll, SelectMode, SelectionRef, UI_STATE_VERSION, UiState, ViewChanged, ViewCtx, ViewPatch,
};

/// Registers the `view.*` commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<ViewGet>()?
        .register::<ViewSet>()?
        .register::<ViewSetPlayhead>()?
        .register::<ViewSelect>()?
        .register::<ViewSetZoom>()?;
    Ok(())
}
