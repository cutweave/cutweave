// SPDX-License-Identifier: GPL-3.0-or-later
//! The MCP server (WP-1J.01; arch §6, §14, ADR-0009): rmcp `=3.11.0` over Streamable HTTP,
//! mounted at `/mcp` behind the server's Host/Origin check and bearer-token authentication.
//!
//! - [`router`] builds the mount: `/mcp` (standard profile), `/mcp/minimal` and `/mcp/full`,
//!   all over one engine (one endpoint = one engine = one open project).
//! - [`catalog`]: the tools, generated from the command registry (core tools plus
//!   `cutweave_search_commands`, `cutweave_describe_command`, `cutweave_execute` and
//!   `cutweave_query`), profiles and the tool-list budget.
//! - [`execute`]: `cutweave_execute`, a transactional batch with `$ref`s and dry runs.
//! - [`results`]: structured results, typed tool errors (`code`, `message`, `path`, `hint`) and
//!   untrusted-text wrapping.
//! - [`handler`]: the rmcp `ServerHandler`, one engine connection per MCP session.
//! - [`dispatch`]: the seam to the engine ([`Dispatch`]), since the engine depends on this
//!   crate.
//!
//! # Spec revisions
//!
//! rmcp negotiates the revision; the handler code is the same for both. **2025-11-25**
//! (stateful): `initialize`, an `Mcp-Session-Id` session (`legacy_session_mode`), one engine
//! connection per session. **2026-07-28** (stateless): `server/discover`, per-request `_meta`
//! and the `MCP-Protocol-Version` header; every request gets a fresh handler and connection.
//! Older revisions rmcp knows are negotiated too. Resources and their change notifications
//! (`resources/subscribe` vs `subscriptions/listen`) arrive with WP-1J.02.
//!
//! # Security
//!
//! The server's middleware has already checked `Host` (exact loopback `host:port`), `Origin`
//! and the bearer token before a request reaches rmcp, and attached the credential
//! ([`Authenticated`](crate::Authenticated)); the actor is derived from it, never from the
//! request. rmcp's own Host and Origin validation is enabled as a second layer (it is off by
//! default, Spike S5 finding 4).

pub mod catalog;
pub mod dispatch;
pub mod execute;
pub mod handler;
pub mod results;

use std::sync::Arc;

use axum::Router;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

pub use catalog::{Budget, Catalog, ListSize, P1_GATE, Profile};
pub use dispatch::{Dispatch, DispatchCall, DispatchConn, DispatchReply};
pub use handler::{CutweaveMcp, INSTRUCTIONS};

/// Options of the `/mcp` mount.
#[derive(Clone, Debug)]
pub struct McpOptions {
    /// Browser origins rmcp's second-layer check accepts (RFC 6454, `:*` = any port). The
    /// server's own check, which runs first, pins the port.
    pub allowed_origins: Vec<String>,
    /// Hosts rmcp's second-layer check accepts (loopback names; the server pins the port).
    pub allowed_hosts: Vec<String>,
}

impl Default for McpOptions {
    fn default() -> McpOptions {
        McpOptions {
            allowed_origins: vec!["http://127.0.0.1:*".into(), "http://localhost:*".into()],
            allowed_hosts: vec!["127.0.0.1".into(), "localhost".into()],
        }
    }
}

/// The `/mcp` mount ([`Mounts::mcp`](crate::Mounts)): one Streamable HTTP endpoint per
/// [`Profile`] over `dispatch`.
pub fn router(dispatch: impl Dispatch, options: &McpOptions) -> Router {
    let dispatch: Arc<dyn Dispatch> = Arc::new(dispatch);
    let registry = dispatch.registry();
    let mut router = Router::new();
    for profile in Profile::ALL {
        let catalog = Arc::new(Catalog::new(Arc::clone(&registry), profile));
        let dispatch = Arc::clone(&dispatch);
        let config = StreamableHttpServerConfig::default()
            .with_allowed_hosts(options.allowed_hosts.clone())
            .with_allowed_origins(options.allowed_origins.clone())
            .enforce_origin_validation()
            .with_legacy_session_mode(true);
        let service = StreamableHttpService::new(
            move || {
                Ok(CutweaveMcp::new(
                    Arc::clone(&dispatch),
                    Arc::clone(&catalog),
                    None,
                ))
            },
            Arc::new(LocalSessionManager::default()),
            config,
        );
        router = router.route_service(profile.route(), service);
    }
    router
}
