// SPDX-License-Identifier: GPL-3.0-or-later
//! [`EngineConfig`]: everything [`Engine::start`](crate::Engine::start) needs (WP-1I.04).

use std::path::PathBuf;
use std::sync::Arc;

use cutweave_api::{Mounts, Token};
use cutweave_commands::Clock;
use cutweave_jobs::JobConfig;

use crate::autosave::AutosaveConfig;
use crate::engine::EngineHandle;
use crate::journal::SyncPolicy;
use crate::logging::LogConfig;
use crate::subsystems::Subsystems;

/// How the engine runs (arch §2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineMode {
    /// The desktop app's daemon (`cutweave engine`): the UI credential arrives over the spawn
    /// handshake, the API server always runs, and the process exits when stdin closes.
    Gui,
    /// A server without UI (`cutweave mcp --headless`, CI): no UI credential class at all;
    /// the API server runs when a port is given.
    #[default]
    Headless,
    /// An in-process engine for one CLI invocation (`cutweave exec --headless`): no server and
    /// no run-file.
    Oneshot,
}

impl EngineMode {
    /// The lowercase name (`gui`, `headless`, `oneshot`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            EngineMode::Gui => "gui",
            EngineMode::Headless => "headless",
            EngineMode::Oneshot => "oneshot",
        }
    }
}

/// Audio output settings, consumed by the audio subsystem (WP-1I.05). The core engine ignores
/// them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioConfig {
    /// The system's default output device.
    #[default]
    Default,
    /// No device: a null clock (headless rendering, CI).
    Null,
}

/// GPU settings, consumed by the render subsystem (WP-1I.05). The core engine ignores them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuConfig {
    /// The best adapter; a failure exits with code 11 in `Gui` mode.
    #[default]
    Auto,
    /// The software rasterizer (headless rendering, CI).
    Software,
}

/// Builds the routes the API server mounts (`/rpc` from WP-1I.02, `/mcp` from WP-1J.01,
/// `/media` from WP-1I.03) once the engine core is up, so they can dispatch through the
/// [`EngineHandle`].
pub type MountFactory = Box<dyn FnOnce(EngineHandle) -> Mounts + Send>;

/// The local API server (WP-1I.01) as the engine starts it.
pub struct ApiOptions {
    /// Run-file directory; `None` = `~/.cutweave/run`.
    pub run_dir: Option<PathBuf>,
    /// Extra allowed `Origin` values (`app://cutweave`, set by the launcher).
    pub allowed_origins: Vec<String>,
    /// Labels of the agent tokens written to the run-file (default `cli`, `mcp`).
    pub agent_labels: Vec<String>,
    /// Mount the `/_test/*` routes of the server (tests only).
    pub enable_test_routes: bool,
    /// Routes to mount (none in the core engine: `/rpc`, `/mcp` and `/media` answer 501).
    pub mounts: Option<MountFactory>,
}

impl Default for ApiOptions {
    fn default() -> ApiOptions {
        ApiOptions {
            run_dir: None,
            allowed_origins: Vec::new(),
            agent_labels: vec!["cli".into(), "mcp".into()],
            enable_test_routes: false,
            mounts: None,
        }
    }
}

impl std::fmt::Debug for ApiOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiOptions")
            .field("run_dir", &self.run_dir)
            .field("allowed_origins", &self.allowed_origins)
            .field("agent_labels", &self.agent_labels)
            .field("enable_test_routes", &self.enable_test_routes)
            .field("mounts", &self.mounts.is_some())
            .finish()
    }
}

/// Everything [`Engine::start`](crate::Engine::start) needs. `EngineConfig::default()` is a
/// headless core engine without a server, project, logging or determinism.
pub struct EngineConfig {
    /// How the engine runs.
    pub mode: EngineMode,
    /// The project (folder or `.cutweave` file) to open at start; `None` starts with an
    /// untitled document. A project another process holds fails the start with exit code 10.
    pub project: Option<PathBuf>,
    /// The API server's port on `127.0.0.1` (`0` = any free port). `None`: no server in
    /// `Headless` mode; `Gui` always serves (`None` means `0`); `Oneshot` never serves.
    pub port: Option<u16>,
    /// The UI credential from the spawn handshake (`Gui` mode only; a headless engine has no
    /// UI credential class and refuses one).
    pub ui_token: Option<Token>,
    /// `--deterministic-ids <seed>` (WP-1B.08): seeds the document's id streams and, separately,
    /// the job ids, and freezes the clock at the Unix epoch unless [`clock`](Self::clock) is
    /// set.
    pub deterministic_ids: Option<u64>,
    /// Folders agents and headless callers may import media from, besides the open project's
    /// folder (arch §14, §18).
    pub allowed_roots: Vec<PathBuf>,
    /// Audio output (WP-1I.05).
    pub audio: AudioConfig,
    /// GPU (WP-1I.05).
    pub gpu: GpuConfig,
    /// The JSON log file and the crash marker (off unless enabled).
    pub log: LogConfig,
    /// The local API server.
    pub api: ApiOptions,
    /// Overrides the project's `settings.autosave` timing (tests use short intervals).
    pub autosave: Option<AutosaveConfig>,
    /// Journal durability.
    pub journal_sync: SyncPolicy,
    /// The engine's clock (doc actor, journal, autosave); `None` = system time.
    pub clock: Option<Arc<dyn Clock>>,
    /// Job budgets.
    pub jobs: JobConfig,
    /// Async worker threads of the engine runtime (API server, job coordination); at least 1.
    pub worker_threads: usize,
    /// Capacity of the engine event fan-out; a subscriber that falls this far behind gets
    /// `Lagged` and must resync.
    pub event_capacity: usize,
    /// Media, render, audio and playback (WP-1I.05); `None` runs the core only.
    pub subsystems: Option<Box<dyn Subsystems>>,
}

/// Default number of async worker threads.
pub const DEFAULT_WORKER_THREADS: usize = 2;
/// Default capacity of the engine event fan-out.
pub const DEFAULT_EVENT_CAPACITY: usize = 4096;

impl Default for EngineConfig {
    fn default() -> EngineConfig {
        EngineConfig {
            mode: EngineMode::Headless,
            project: None,
            port: None,
            ui_token: None,
            deterministic_ids: None,
            allowed_roots: Vec::new(),
            audio: AudioConfig::default(),
            gpu: GpuConfig::default(),
            log: LogConfig::disabled(),
            api: ApiOptions::default(),
            autosave: None,
            journal_sync: SyncPolicy::default(),
            clock: None,
            jobs: JobConfig::default(),
            worker_threads: DEFAULT_WORKER_THREADS,
            event_capacity: DEFAULT_EVENT_CAPACITY,
            subsystems: None,
        }
    }
}

impl EngineConfig {
    /// A default configuration in `mode`.
    #[must_use]
    pub fn new(mode: EngineMode) -> EngineConfig {
        EngineConfig {
            mode,
            ..EngineConfig::default()
        }
    }

    /// The port the server binds, or `None` when this configuration runs no server.
    #[must_use]
    pub fn server_port(&self) -> Option<u16> {
        match self.mode {
            EngineMode::Gui => Some(self.port.unwrap_or(0)),
            EngineMode::Headless => self.port,
            EngineMode::Oneshot => None,
        }
    }
}

impl std::fmt::Debug for EngineConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineConfig")
            .field("mode", &self.mode)
            .field("project", &self.project)
            .field("port", &self.port)
            .field("ui_token", &self.ui_token.as_ref().map(|_| "<redacted>"))
            .field("deterministic_ids", &self.deterministic_ids)
            .field("allowed_roots", &self.allowed_roots)
            .field("audio", &self.audio)
            .field("gpu", &self.gpu)
            .field("log", &self.log)
            .field("api", &self.api)
            .field("autosave", &self.autosave)
            .field("journal_sync", &self.journal_sync)
            .field("jobs", &self.jobs)
            .field("worker_threads", &self.worker_threads)
            .field("subsystems", &self.subsystems.is_some())
            .finish_non_exhaustive()
    }
}
