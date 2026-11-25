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
//!   [`rpc_backend`] puts the engine behind `cutweave-api`'s JSON-RPC endpoint (WP-1I.02),
//!   [`mcp_backend`] behind its MCP endpoints (WP-1J.01) and [`media_http`] behind its `/media`
//!   HTTP endpoints (WP-1I.03).
//! - [`export`] (WP-1E.03): `export.start` as a job: render every frame like `render.frame`,
//!   mix the range down and encode H.264 + AAC into an MP4, with what the subsystems provide
//!   ([`ExportServices`]).
//! - [`full`] (WP-1I.05): [`FullSubsystems`], the media, render, audio and playback the real
//!   engine attaches: FFmpeg decoding and the caches, the wgpu compositor (`render.frame`,
//!   export, preview), the mixer and audio output, the transport and the preview ring.
//! - [`project_io`] (WP-1C.01): the project folder format, atomic save/open and the
//!   single-writer lock (arch §17, ADR-0006).
//! - [`journal`] (WP-1C.02): the append-only command journal written before every
//!   acknowledgement, and crash recovery from it.
//! - [`ui_state`] (WP-1C.04): persistence of the `view` namespace state.
//! - [`autosave`] (WP-1C.03): idle and periodic autosave of the committed revision, rotating
//!   backups and the project file events.

pub mod agents;
pub mod autosave;
pub mod config;
#[cfg(feature = "testing")]
pub mod determinism;
pub mod dev_media;
pub mod dispatch;
mod engine;
pub mod events;
pub mod export;
pub mod full;
mod import;
pub mod journal;
pub mod launch;
pub mod logging;
pub mod mcp_backend;
pub mod media_cache;
pub mod media_caps;
pub mod media_http;
pub mod project_io;
pub mod rpc_backend;
pub mod session;
mod startup;
pub mod subsystems;
pub mod ui_state;

pub use agents::{AgentConnection, MAX_ALLOWED_ROOTS};
pub use config::{ApiOptions, AudioConfig, EngineConfig, EngineMode, GpuConfig, MountFactory};
pub use dev_media::{DEV_STUB_MEDIA_ENV, DevStubMedia, StubMediaBackend};
pub use dispatch::{Call, Connection, Reply};
pub use engine::{Engine, EngineHandle, JOB_ID_STREAM, StartError, VERSION, build_registry};
pub use events::{EngineEvent, EventReceiver, SESSION_EVENTS};
pub use export::{DecoderSources, ExportServices, ExportSources};
pub use full::{AudioMode, FullConfig, FullHandle, FullSubsystems, MediaState};
pub use import::NoMediaBackend;
pub use logging::LogConfig;
pub use mcp_backend::{api_mounts, mcp_mounts};
pub use media_cache::{CacheControl, CacheKind, DecodeHw};
pub use media_http::media_mounts;
pub use rpc_backend::rpc_mounts;
pub use session::{ProjectStatus, RecoveryInfo, SessionState};
pub use subsystems::{
    CoreOnly, DEGRADED_EVENT, EngineLink, SUBSYSTEM_EVENTS, SubsystemCall, SubsystemError,
    SubsystemEvents, SubsystemHost, SubsystemKind, Subsystems,
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
            "export.start",
            "export.list_presets",
            "export.resolve_preset",
            "export.save_preset",
            "export.delete_preset",
            "project.open",
            "project.save",
            "project.save_as",
            "project.close",
            "project.recover",
            "project.discard_recovery",
            "project.set_autosave",
            "project.set_allowed_roots",
            "project.connections",
            "media.capabilities",
            "media.cache_status",
            "media.set_cache_config",
            "media.clear_cache",
            "media.set_decode_policy",
        ] {
            assert!(registry.get(name).is_some(), "{name} is not registered");
        }
    }
}
