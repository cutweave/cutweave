// SPDX-License-Identifier: GPL-3.0-or-later
//! The engines the surfaces run against, and what the harness observes on them.
//!
//! Every run gets a fresh in-process engine with `--deterministic-ids 42` (seeded ids, the clock
//! frozen at the Unix epoch) and the dev stub media backend, configured like the production
//! engine of that surface:
//!
//! - [`EngineKind::Server`]: `cutweave engine --headless --port 0 --deterministic-ids 42
//!   --dev-stub-media --allow-root <root> --run-dir <dir>` (`cutweave_engine::launch`): `/rpc` and
//!   `/mcp` mounted, a run-file with the agent tokens. The CLI attaches to it, the raw JSON-RPC
//!   client connects to it.
//! - [`EngineKind::Stdio`]: `cutweave mcp --headless --deterministic-ids 42 --dev-stub-media
//!   --allow-root <root>` (`cutweave-cli`'s `serve_headless`): no server, no run-file, null audio
//!   and software GPU; the MCP handler is served over stdio.
//!
//! The harness subscribes to the engine's event fan-out before the first call, so every
//! surface's `doc.patched` sequence is captured the same way.

use std::path::Path;
use std::time::{Duration, Instant};

use cutweave_api::mcp::McpOptions;
use cutweave_api::rpc::RpcConfig;
use cutweave_engine::{
    AudioConfig, DevStubMedia, Engine, EngineConfig, EngineEvent, EngineMode, EventReceiver,
    GpuConfig, LogConfig, Subsystems, api_mounts,
};
use serde_json::Value;
use tokio::sync::broadcast::error::TryRecvError;

/// The seed of `--deterministic-ids` every run uses.
pub const SEED: u64 = 42;

/// The agent token labels of the server engine: the defaults (`cli`, `mcp`) plus `rpc`, so the
/// raw JSON-RPC client edits as an actor of its own (`agent:rpc`).
pub const AGENT_LABELS: [&str; 3] = ["cli", "mcp", "rpc"];

/// Which production engine configuration to mirror.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineKind {
    /// `cutweave engine --headless --port 0 ...`.
    Server,
    /// `cutweave mcp --headless ...`.
    Stdio,
}

/// A running engine plus its event subscription.
pub struct ParityEngine {
    pub engine: Engine,
    events: EventReceiver,
    /// Every `doc.patched` payload seen so far, in order.
    patches: Vec<Value>,
}

impl ParityEngine {
    /// Starts an engine of `kind` confined to `root`; `run_dir` holds a server's run-file.
    pub fn start(kind: EngineKind, root: &Path, run_dir: &Path) -> ParityEngine {
        let mut config = EngineConfig::new(EngineMode::Headless);
        config.deterministic_ids = Some(SEED);
        config.allowed_roots = vec![root.to_path_buf()];
        config.subsystems = Some(Box::new(DevStubMedia) as Box<dyn Subsystems>);
        config.log = LogConfig::disabled();
        match kind {
            EngineKind::Server => {
                config.port = Some(0);
                config.api.run_dir = Some(run_dir.to_path_buf());
                config.api.agent_labels = AGENT_LABELS.iter().map(|&l| l.to_owned()).collect();
                config.api.mounts = Some(api_mounts(RpcConfig::default(), McpOptions::default()));
            }
            EngineKind::Stdio => {
                config.audio = AudioConfig::Null;
                config.gpu = GpuConfig::Software;
            }
        }
        let engine = Engine::start(config).expect("start the engine");
        let events = engine.handle().subscribe();
        ParityEngine {
            engine,
            events,
            patches: Vec::new(),
        }
    }

    /// The agent token labelled `label` (server engines).
    pub fn token(&self, label: &str) -> String {
        self.engine
            .credentials()
            .expect("a server engine has credentials")
            .agent_tokens()
            .iter()
            .find(|t| t.label == label)
            .unwrap_or_else(|| panic!("no `{label}` agent token"))
            .token
            .expose()
            .to_owned()
    }

    /// The server's port.
    pub fn port(&self) -> u16 {
        self.engine.port().expect("a server engine has a port")
    }

    fn drain(&mut self) {
        loop {
            match self.events.try_recv() {
                Ok(event) => self.keep(&event),
                Err(TryRecvError::Empty | TryRecvError::Closed) => return,
                Err(TryRecvError::Lagged(n)) => panic!("the harness lagged {n} engine events"),
            }
        }
    }

    fn keep(&mut self, event: &EngineEvent) {
        if event.name() == "doc.patched" {
            self.patches.push(event.payload());
        }
    }

    /// The `doc.patched` payloads up to the document's current live revision (waiting for the
    /// fan-out to deliver them).
    pub fn patches(&mut self) -> Vec<Value> {
        let rev = self
            .engine
            .handle()
            .snapshot()
            .expect("the document snapshot")
            .rev
            .0;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.drain();
            let seen = self
                .patches
                .iter()
                .filter_map(|p| p["rev"].as_u64())
                .max()
                .unwrap_or(0);
            if seen >= rev {
                return self.patches.clone();
            }
            assert!(
                Instant::now() < deadline,
                "no doc.patched for rev {rev} (last seen {seen})"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Shuts the engine down.
    pub fn shutdown(self) {
        self.engine.shutdown();
    }
}
