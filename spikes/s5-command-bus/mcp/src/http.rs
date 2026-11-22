// SPDX-License-Identifier: GPL-3.0-or-later
//! Streamable HTTP on `127.0.0.1` (arch §6, §14).
//!
//! One axum router, one rmcp `StreamableHttpService` per profile endpoint (`/mcp/minimal`,
//! `/mcp`, `/mcp/full`), all over the same engine. Every request passes [`guard`] first:
//!
//! 1. `Host` must be `127.0.0.1:<port>` or `localhost:<port>`, else **403** (DNS rebinding).
//! 2. `Origin`, if present, must be in the allowlist (empty by default), else **403**.
//! 3. `Authorization: Bearer <agent token>`, else **401** with `WWW-Authenticate: Bearer`.
//!
//! The matched token's label becomes [`AgentIdentity`] in the request extensions; rmcp hands the
//! request `Parts` to the handler, which derives the actor from it (never from the body).
//! rmcp's own Host/Origin validation is configured with the same lists as a second layer.
//!
//! Sessions: `legacy_session_mode` is on, so 2025-11-25 clients get an `Mcp-Session-Id` session
//! (needed for `resources/subscribe` notifications on the GET stream); 2026-07-28 requests are
//! always served statelessly by rmcp regardless of that flag.

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    EngineHandle,
    auth::{AgentToken, authenticate},
    catalog::{Catalog, Profile},
    server::CutweaveMcp,
};

/// The authenticated agent, inserted by [`guard`] and read by the handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentIdentity {
    /// Label (`agent:<label>`).
    pub label: String,
}

/// HTTP server options.
#[derive(Clone, Debug, Default)]
pub struct HttpOptions {
    /// Port on 127.0.0.1; 0 picks a random free port.
    pub port: u16,
    /// Accepted agent tokens.
    pub tokens: Vec<AgentToken>,
    /// Browser origins allowed to call (exact `scheme://host:port`). Empty: any request with an
    /// `Origin` header is refused; agents (non-browser clients) send none.
    pub allowed_origins: Vec<String>,
}

struct Guard {
    hosts: [String; 2],
    tokens: Vec<AgentToken>,
    origins: Vec<String>,
}

/// A running server.
pub struct RunningHttp {
    addr: SocketAddr,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

impl RunningHttp {
    /// Bound address.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Endpoint URL of `profile`.
    #[must_use]
    pub fn url(&self, profile: Profile) -> String {
        format!("http://{}{}", self.addr, profile.http_path())
    }

    /// Stops accepting requests, ends sessions and waits for the server task.
    pub async fn shutdown(self) {
        self.stop.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.task).await;
    }
}

/// Binds `127.0.0.1:<port>` and serves every profile endpoint over `engine`.
pub async fn serve(engine: EngineHandle, options: HttpOptions) -> std::io::Result<RunningHttp> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, options.port)).await?;
    let addr = listener.local_addr()?;
    let stop = CancellationToken::new();
    let app = router(&engine, addr.port(), options, &stop);
    let shutdown = stop.clone();
    let task = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
        {
            tracing::error!("http server failed: {e}");
        }
    });
    tracing::info!("MCP Streamable HTTP on http://{addr}/mcp (+ /mcp/minimal, /mcp/full)");
    Ok(RunningHttp { addr, stop, task })
}

/// The router (exposed for tests that drive it without a socket).
pub fn router(
    engine: &EngineHandle,
    port: u16,
    options: HttpOptions,
    stop: &CancellationToken,
) -> Router {
    let guard = Arc::new(Guard {
        hosts: [format!("127.0.0.1:{port}"), format!("localhost:{port}")],
        tokens: options.tokens,
        origins: options
            .allowed_origins
            .iter()
            .map(|o| o.to_ascii_lowercase())
            .collect(),
    });
    let mut app = Router::new();
    for profile in Profile::ALL {
        let catalog = Arc::new(Catalog::new(crate::registry(), profile));
        let engine = engine.clone();
        let config = StreamableHttpServerConfig::default()
            .with_allowed_hosts(guard.hosts.clone())
            .with_allowed_origins(guard.origins.clone())
            .enforce_origin_validation()
            .with_legacy_session_mode(true)
            .with_json_response(true)
            .with_cancellation_token(stop.child_token());
        let service = StreamableHttpService::new(
            move || Ok(CutweaveMcp::new(engine.clone(), catalog.clone(), None)),
            Arc::new(LocalSessionManager::default()),
            config,
        );
        app = app.route_service(profile.http_path(), service);
    }
    app.layer(axum::middleware::from_fn_with_state(guard, guard_layer))
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

async fn guard_layer(
    State(guard): State<Arc<Guard>>,
    mut request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let host = header_str(headers, header::HOST).map(str::to_ascii_lowercase);
    if !host.as_ref().is_some_and(|h| guard.hosts.contains(h)) {
        tracing::warn!(?host, "403: Host not allowed (possible DNS rebinding)");
        return (
            StatusCode::FORBIDDEN,
            "Forbidden: Host header is not allowed",
        )
            .into_response();
    }
    if headers.contains_key(header::ORIGIN) {
        let origin = header_str(headers, header::ORIGIN).map(str::to_ascii_lowercase);
        if !origin.as_ref().is_some_and(|o| guard.origins.contains(o)) {
            tracing::warn!(?origin, "403: Origin not allowed");
            return (StatusCode::FORBIDDEN, "Forbidden: Origin is not allowed").into_response();
        }
    }
    let Some(token) = authenticate(header_str(headers, header::AUTHORIZATION), &guard.tokens)
    else {
        tracing::warn!("401: missing or wrong bearer token");
        let mut response = (
            StatusCode::UNAUTHORIZED,
            "Unauthorized: agent bearer token required",
        )
            .into_response();
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"cutweave\""),
        );
        return response;
    };
    let identity = AgentIdentity {
        label: token.label.clone(),
    };
    request.extensions_mut().insert(identity);
    next.run(request).await
}
