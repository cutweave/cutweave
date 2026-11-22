// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike S5 MCP server (WP-0H.02). Throwaway; see `../../README.md` and `../../MCP-AGENT-RUN.md`.
//!
//! An [rmcp](https://crates.io/crates/rmcp) server over the s5 command registry, standing in for
//! the MCP module of `cutweave-api` (arch §6, §14, ADR-009):
//!
//! - [`catalog`]: the hybrid tool catalog: registry `core` entries as tools (annotations from
//!   macro metadata) plus the 4 generic tools; profiles per endpoint / `--profile`;
//! - [`server`]: the rmcp `ServerHandler` (tools, `cutweave://project` resource, change
//!   notifications for both spec revisions);
//! - [`notifier`]: one internal change feed, delivered through `resources/subscribe` (2025-11-25)
//!   or `subscriptions/listen` (2026-07-28);
//! - [`http`]: Streamable HTTP on `127.0.0.1` with Host/Origin checks and bearer agent tokens;
//! - [`auth`]: agent tokens and the run-file;
//! - [`budget`]: `tools/list` size measurement against the arch §14 budgets.
//!
//! The actor is never taken from the request: stdio uses the label the process was started with,
//! HTTP derives it from the bearer token (arch §6).

pub mod auth;
pub mod budget;
pub mod catalog;
pub mod http;
pub mod notifier;
pub mod server;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use s5_bus::{Engine, EngineConfig, Registry};

pub use catalog::{Catalog, Profile};
pub use notifier::{DocChange, Notifier};
pub use server::{CutweaveMcp, PROJECT_URI};

/// The registry the server exposes (forces `s5-ops` to link, see `s5_ops::registry`).
#[must_use]
pub fn registry() -> &'static Registry {
    s5_ops::registry()
}

/// One engine (one open project) shared by every MCP connection and transport (arch §14:
/// "one MCP endpoint = one engine = one open project"), plus its change feed.
#[derive(Clone)]
pub struct EngineHandle {
    engine: Arc<Mutex<Engine>>,
    notifier: Notifier,
}

impl EngineHandle {
    /// A fresh engine over the full registry.
    #[must_use]
    pub fn new(config: EngineConfig) -> EngineHandle {
        let mut engine = Engine::with_registry(registry(), config);
        let notifier = Notifier::bridge(engine.subscribe());
        EngineHandle {
            engine: Arc::new(Mutex::new(engine)),
            notifier,
        }
    }

    /// Runs `f` with exclusive access (the single-writer doc actor of arch §5.2, as a mutex).
    pub fn with<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> R {
        f(&mut self.lock())
    }

    fn lock(&self) -> MutexGuard<'_, Engine> {
        // Spike shortcut: keep serving after a panicking command. The spike `Tx` does not roll
        // back on unwind, so Phase 1 must either roll back in `Drop` or restart the doc actor.
        self.engine.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The change feed.
    #[must_use]
    pub fn notifier(&self) -> &Notifier {
        &self.notifier
    }
}
