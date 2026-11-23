// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-engine`: composition root wiring the engine together and session lifecycle.
//!
//! - [`Engine`] / [`EngineConfig`] / [`EngineHandle`] (WP-1I.04): starts the core: registry,
//!   doc actor, project store + journal + autosave, job manager, event fan-out ([`events`]),
//!   the local API server; media/render/audio/playback attach through [`Subsystems`]
//!   (WP-1I.05). [`Connection`] / [`Call`] ([`dispatch`]) is the in-process dispatch API the
//!   RPC, MCP and CLI layers use. [`session`] holds one open project (arch §14) and the
//!   `project.*` session commands. [`launch`] is the process launch protocol (readiness line,
//!   handshake, exit codes) and [`logging`] the JSON log file and crash marker.
//!   [`rpc_backend`] puts the engine behind `cutweave-api`'s JSON-RPC endpoint (WP-1I.02).
//! - [`project_io`] (WP-1C.01): the project folder format, atomic save/open and the
//!   single-writer lock (arch §17, ADR-0006).
//! - [`journal`] (WP-1C.02): the append-only command journal written before every
//!   acknowledgement, and crash recovery from it.
//! - [`ui_state`] (WP-1C.04): persistence of the `view` namespace state.
//! - [`autosave`] (WP-1C.03): idle and periodic autosave of the committed revision, rotating
//!   backups and the project file events.

pub mod autosave;
pub mod config;
#[cfg(feature = "testing")]
pub mod determinism;
pub mod dispatch;
mod engine;
pub mod events;
mod import;
pub mod journal;
pub mod launch;
pub mod logging;
pub mod project_io;
pub mod rpc_backend;
pub mod session;
pub mod subsystems;
pub mod ui_state;

pub use config::{ApiOptions, AudioConfig, EngineConfig, EngineMode, GpuConfig, MountFactory};
pub use dispatch::{Call, Connection, Reply};
pub use engine::{Engine, EngineHandle, JOB_ID_STREAM, StartError, VERSION, build_registry};
pub use events::{EngineEvent, EventReceiver, SESSION_EVENTS};
pub use import::NoMediaBackend;
pub use logging::LogConfig;
pub use rpc_backend::rpc_mounts;
pub use session::{ProjectStatus, RecoveryInfo, SessionState};
pub use subsystems::{
    CoreOnly, SubsystemCall, SubsystemError, SubsystemEvents, SubsystemHost, SubsystemKind,
    Subsystems,
};

/// Returns the crate name.
#[must_use]
pub const fn crate_name() -> &'static str {
    "cutweave-engine"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "cutweave-engine");
    }

    #[test]
    fn the_registry_serves_ops_edit_and_session_commands() {
        let registry = build_registry().expect("registry");
        for name in [
            "project.new",
            "edit.undo",
            "view.get",
            "job.wait",
            "media.import",
            "project.open",
            "project.save",
            "project.save_as",
            "project.close",
            "project.recover",
            "project.discard_recovery",
        ] {
            assert!(registry.get(name).is_some(), "{name} is not registered");
        }
    }
}
