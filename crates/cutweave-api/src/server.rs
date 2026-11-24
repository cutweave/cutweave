// SPDX-License-Identifier: GPL-3.0-or-later
//! The axum server: loopback listener, middleware stack, run-file lifecycle, shutdown.
//!
//! Request flow (outermost first): request id + tracing, Host/Origin validation (DNS-rebinding
//! defense, answers CORS preflights), authentication, routes. `/rpc` and `/mcp` take a bearer
//! token. `/media` (WP-1I.03) takes a bearer token too, or, for `GET` and `HEAD`, a URL signed by
//! `POST /media/sign` (an `<img>` cannot send headers, and tokens never go in URLs; see
//! [`crate::media::sign`]). `/health` needs no token but still passes the Host/Origin check.

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    Extension, Json, Router,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    serve::ListenerExt,
};
use cutweave_commands::{ActorCtx, ConnId, Scope};
use serde_json::json;
use tokio::{net::TcpListener, sync::watch, task::JoinHandle};
use tracing::Instrument;

use crate::{
    creds::{Credential, Credentials, CredentialsError, Token, hex},
    media::sign::{SIGNATURE_PARAM, SignError, UrlSigner},
    runfile::{RunFile, RunFileContents, RunFileProject, RunFileToken, RunMode},
};

/// Exit code of an engine whose port could not be bound (Electron main re-picks a port).
pub const EXIT_PORT_IN_USE: i32 = 12;

/// Subprotocol prefix a WebSocket client may use to present its token
/// (`Sec-WebSocket-Protocol: cutweave.token.<token>`) when it cannot set `Authorization`.
pub const WS_TOKEN_SUBPROTOCOL_PREFIX: &str = "cutweave.token.";

/// How long graceful shutdown waits for open connections before aborting them.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Handlers mounted under `/rpc`, `/mcp` and `/media` by later work packages.
///
/// Every mount sits behind authentication. `/rpc` and `/mcp` receive an [`Authenticated`]
/// request extension; `/media` receives it for bearer requests (with the server's
/// `Arc<UrlSigner>` for `POST /media/sign`), and nothing for a signed URL. Unset mounts
/// answer `501`.
#[derive(Default)]
pub struct Mounts {
    /// JSON-RPC over WebSocket (WP-1I.02).
    pub rpc: Option<Router>,
    /// MCP streamable HTTP (WP-1J.01).
    pub mcp: Option<Router>,
    /// Media endpoints (WP-1I.03, [`crate::media::router`]): a bearer token, or a signed URL.
    pub media: Option<Router>,
}

/// Server configuration.
pub struct ApiConfig {
    /// Port on `127.0.0.1`; `0` picks a free one.
    pub port: u16,
    /// Run-file directory; `None` = `~/.cutweave/run`.
    pub run_dir: Option<PathBuf>,
    /// The UI credential from the spawn handshake; `None` in headless mode.
    pub ui_token: Option<Token>,
    /// Labels of the agent tokens to generate (default `cli`, `mcp`).
    pub agent_labels: Vec<String>,
    /// Extra allowed `Origin` values (e.g. `app://cutweave`, configured by the launcher).
    pub allowed_origins: Vec<String>,
    /// The open project, recorded in the run-file.
    pub project: Option<RunFileProject>,
    /// Engine version, recorded in the run-file and the readiness line.
    pub version: String,
    /// Mount the `/_test/*` placeholder routes (the UI-only scope enforcement check). Tests
    /// and development only.
    pub enable_test_routes: bool,
    /// Route mount points.
    pub mounts: Mounts,
    /// The name of the preview frame ring the engine publishes, recorded in the run-file's
    /// `shm` (WP-1I.05); `None` without one.
    pub shm: Option<String>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        ApiConfig {
            port: 0,
            run_dir: None,
            ui_token: None,
            agent_labels: vec!["cli".into(), "mcp".into()],
            allowed_origins: Vec::new(),
            project: None,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            enable_test_routes: false,
            mounts: Mounts::default(),
            shm: None,
        }
    }
}

/// Failure starting the server.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// `127.0.0.1:<port>` could not be bound (exit code 12).
    #[error("cannot bind 127.0.0.1:{port}: {source}")]
    Bind {
        /// The requested port.
        port: u16,
        /// The OS error.
        source: std::io::Error,
    },
    /// Building the credential store failed.
    #[error(transparent)]
    Credentials(#[from] CredentialsError),
    /// The run-file could not be created with user-only permissions.
    #[error("cannot write the run-file: {0}")]
    RunFile(std::io::Error),
    /// No run directory was given and no home directory is known.
    #[error("no run directory: set one or define HOME/USERPROFILE")]
    NoRunDir,
}

impl ApiError {
    /// The process exit code for this failure: [`EXIT_PORT_IN_USE`] for bind failures.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            ApiError::Bind { .. } => EXIT_PORT_IN_USE,
            _ => 1,
        }
    }
}

/// The authenticated caller, attached to every request behind authentication.
#[derive(Clone, Debug)]
pub struct Authenticated {
    /// Credential-derived actor, connection and access.
    pub ctx: ActorCtx,
    /// The credential class that was presented.
    pub credential: Credential,
}

/// The request's id (`x-request-id` in the response and in logs).
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// `Forbidden` unless `ctx` may use `scope` (UI-only scopes reject agent tokens).
pub fn require_scope(ctx: &ActorCtx, scope: Scope) -> Result<(), Box<Response>> {
    if ctx.access().allows(scope) {
        Ok(())
    } else {
        Err(Box::new(error_response(
            StatusCode::FORBIDDEN,
            "forbidden",
            "this operation can only be used from the Cutweave editor UI",
            None,
        )))
    }
}

struct Shared {
    creds: Credentials,
    /// Signs and checks `/media` URLs with a per-launch key (WP-1I.03).
    signer: Arc<UrlSigner>,
    port: u16,
    allowed_origins: Vec<String>,
    next_conn: AtomicU64,
}

impl Shared {
    fn origin_allowed(&self, origin: &str) -> bool {
        self.allowed_origins.iter().any(|o| o == origin)
            || origin == format!("http://127.0.0.1:{}", self.port)
            || origin == format!("http://localhost:{}", self.port)
    }

    fn host_allowed(&self, host: &str) -> bool {
        host == format!("127.0.0.1:{}", self.port)
            || host.eq_ignore_ascii_case(&format!("localhost:{}", self.port))
    }
}

/// Entry point: [`ApiServer::start`].
pub struct ApiServer;

impl ApiServer {
    /// Binds `127.0.0.1:<port>`, writes the run-file and serves in the background.
    pub async fn start(config: ApiConfig) -> Result<RunningServer, ApiError> {
        let run_dir = config
            .run_dir
            .clone()
            .or_else(RunFile::default_dir)
            .ok_or(ApiError::NoRunDir)?;
        let creds = Credentials::generate(config.ui_token.clone(), &config.agent_labels)?;

        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, config.port)))
            .await
            .map_err(|source| ApiError::Bind {
                port: config.port,
                source,
            })?;
        let port = listener
            .local_addr()
            .map_err(|source| ApiError::Bind {
                port: config.port,
                source,
            })?
            .port();

        let reaped = RunFile::reap_stale(&run_dir);
        if reaped > 0 {
            tracing::info!(reaped, "removed stale run-files");
        }
        let contents = RunFileContents {
            pid: std::process::id(),
            port,
            agent_tokens: creds
                .agent_tokens()
                .iter()
                .map(|a| RunFileToken {
                    label: a.label.clone(),
                    token: a.token.expose().to_owned(),
                })
                .collect(),
            project: config.project.clone(),
            mode: if config.ui_token.is_some() {
                RunMode::Gui
            } else {
                RunMode::Headless
            },
            focused_at: None,
            started: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            version: config.version.clone(),
            shm: config.shm.clone(),
        };
        let run_file = RunFile::write(&run_dir, &contents).map_err(ApiError::RunFile)?;

        let signer = UrlSigner::generate().map_err(CredentialsError::from)?;
        let shared = Arc::new(Shared {
            creds,
            signer: Arc::new(signer),
            port,
            allowed_origins: config.allowed_origins,
            next_conn: AtomicU64::new(1),
        });
        let app = build_router(&shared, config.mounts, config.enable_test_routes);

        let (tx, mut rx) = watch::channel(false);
        // No Nagle delay: a WebSocket writes a response and its events back to back (WP-1I.02).
        let listener = listener.tap_io(|tcp| {
            if let Err(error) = tcp.set_nodelay(true) {
                tracing::debug!(%error, "cannot disable Nagle's algorithm");
            }
        });
        let task = tokio::spawn(async move {
            let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
                let _ = rx.wait_for(|stop| *stop).await;
            });
            if let Err(error) = serve.await {
                tracing::error!(%error, "server stopped with an error");
            }
        });
        tracing::info!(port, "api server listening on loopback");
        Ok(RunningServer {
            port,
            version: config.version,
            shared,
            stop: Arc::new(tx),
            task: Some(task),
            run_file,
        })
    }
}

/// A started server. Dropping it removes the run-file (the task is aborted by the runtime).
pub struct RunningServer {
    port: u16,
    version: String,
    shared: Arc<Shared>,
    stop: Arc<watch::Sender<bool>>,
    task: Option<JoinHandle<()>>,
    run_file: RunFile,
}

/// Cloneable trigger for graceful shutdown (signal handlers, stdin EOF).
#[derive(Clone)]
pub struct ShutdownHandle(Arc<watch::Sender<bool>>);

impl ShutdownHandle {
    /// Asks the server to stop accepting and drain.
    pub fn shutdown(&self) {
        self.0.send_replace(true);
    }
}

impl RunningServer {
    /// The bound port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// The engine version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Path of the run-file.
    #[must_use]
    pub fn run_file_path(&self) -> &std::path::Path {
        self.run_file.path()
    }

    /// The credential store (agent tokens for in-process clients and tests).
    #[must_use]
    pub fn credentials(&self) -> &Credentials {
        &self.shared.creds
    }

    /// A trigger that can stop this server from elsewhere.
    #[must_use]
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle(Arc::clone(&self.stop))
    }

    /// Stops accepting, drains connections (aborting after a grace period), removes the
    /// run-file.
    pub async fn shutdown(self) {
        self.shutdown_handle().shutdown();
        self.wait().await;
    }

    /// Waits until the server has stopped (after a [`ShutdownHandle`] fired), then removes the
    /// run-file.
    pub async fn wait(mut self) {
        // The grace period starts only once shutdown was requested.
        let _ = self.stop.subscribe().wait_for(|stop| *stop).await;
        if let Some(mut task) = self.task.take()
            && tokio::time::timeout(SHUTDOWN_GRACE, &mut task)
                .await
                .is_err()
        {
            tracing::warn!("graceful shutdown timed out; aborting open connections");
            task.abort();
        }
        self.run_file.remove();
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
        self.run_file.remove();
    }
}

fn build_router(shared: &Arc<Shared>, mounts: Mounts, test_routes: bool) -> Router {
    let placeholder = || Router::new().fallback(not_implemented);
    let mut authed = Router::new()
        .nest("/rpc", mounts.rpc.unwrap_or_else(placeholder))
        .nest("/mcp", mounts.mcp.unwrap_or_else(placeholder));
    if test_routes {
        authed = authed
            .route("/_test/whoami", post(whoami).get(whoami))
            .route("/_test/ui_only", post(ui_only).get(ui_only));
    }
    let authed = authed.layer(middleware::from_fn_with_state(
        Arc::clone(shared),
        authenticate,
    ));
    // Layered outside the nest, so the middleware sees the full path the URL was signed for.
    let media = Router::new()
        .nest("/media", mounts.media.unwrap_or_else(placeholder))
        .layer(middleware::from_fn_with_state(
            Arc::clone(shared),
            authenticate_media,
        ));

    Router::new()
        .route("/health", get(health))
        .merge(authed)
        .merge(media)
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(
            Arc::clone(shared),
            guard_host_origin,
        ))
        .layer(middleware::from_fn(request_context))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn not_found(Extension(id): Extension<RequestId>) -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        "not_found",
        "no such route",
        Some(&id.0),
    )
}

async fn not_implemented(Extension(id): Extension<RequestId>) -> Response {
    error_response(
        StatusCode::NOT_IMPLEMENTED,
        "not_implemented",
        "this endpoint is not available yet",
        Some(&id.0),
    )
}

async fn whoami(Extension(auth): Extension<Authenticated>) -> Json<serde_json::Value> {
    Json(json!({ "actor": auth.ctx.actor().to_string(), "conn": auth.ctx.conn().get() }))
}

/// Placeholder for a `Scope::UiOnly` command until real ones exist (arch §6).
async fn ui_only(Extension(auth): Extension<Authenticated>) -> Response {
    match require_scope(&auth.ctx, Scope::UiOnly) {
        Ok(()) => {
            Json(json!({ "ok": true, "actor": auth.ctx.actor().to_string() })).into_response()
        }
        Err(response) => *response,
    }
}

/// JSON error body `{"error":{"code","message"},"request_id"}`.
fn error_response(
    status: StatusCode,
    code: &str,
    message: &str,
    request_id: Option<&str>,
) -> Response {
    let body = json!({ "error": { "code": code, "message": message }, "request_id": request_id });
    (status, Json(body)).into_response()
}

fn request_id_of(req: &Request) -> Option<String> {
    req.extensions().get::<RequestId>().map(|r| r.0.clone())
}

/// Assigns the request id, opens the tracing span and logs the outcome. Only the method and
/// path are logged: never headers or query strings, so tokens cannot reach the logs.
async fn request_context(mut req: Request, next: Next) -> Response {
    let mut raw = [0u8; 8];
    let id = match getrandom::fill(&mut raw) {
        Ok(()) => format!("req_{}", hex(&raw)),
        Err(_) => "req_unavailable".to_owned(),
    };
    req.extensions_mut().insert(RequestId(id.clone()));
    let span =
        tracing::info_span!("request", id = %id, method = %req.method(), path = %req.uri().path());
    let started = Instant::now();
    let mut response = next.run(req).instrument(span.clone()).await;
    span.in_scope(|| {
        tracing::info!(
            status = response.status().as_u16(),
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "request done"
        );
    });
    if let Ok(value) = HeaderValue::from_str(&id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), value);
    }
    response
}

const CORS_ALLOW_HEADERS: &str = "authorization, content-type, if-none-match, \
                                  mcp-protocol-version, mcp-session-id, last-event-id";
const CORS_EXPOSE_HEADERS: &str =
    "x-request-id, mcp-session-id, etag, x-cutweave-filmstrip, x-cutweave-rev";

/// Rejects requests whose `Host` is not this loopback listener or whose `Origin` is not
/// absent / configured / the engine's own origin (DNS-rebinding defense, arch §6/§14).
async fn guard_host_origin(
    State(shared): State<Arc<Shared>>,
    req: Request,
    next: Next,
) -> Response {
    let rid = request_id_of(&req);
    let host_ok = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| shared.host_allowed(h));
    if !host_ok {
        return error_response(
            StatusCode::FORBIDDEN,
            "bad_host",
            "Host header not allowed",
            rid.as_deref(),
        );
    }

    let origins = req.headers().get_all(header::ORIGIN);
    let mut iter = origins.iter();
    let origin: Option<String> = match (iter.next(), iter.next()) {
        (None, _) => None,
        (Some(v), None) => match v.to_str() {
            Ok(s) if shared.origin_allowed(s) => Some(s.to_owned()),
            _ => {
                return error_response(
                    StatusCode::FORBIDDEN,
                    "bad_origin",
                    "Origin not allowed",
                    rid.as_deref(),
                );
            }
        },
        (Some(_), Some(_)) => {
            return error_response(
                StatusCode::FORBIDDEN,
                "bad_origin",
                "Origin not allowed",
                rid.as_deref(),
            );
        }
    };

    if req.method() == Method::OPTIONS
        && origin.is_some()
        && req.headers().contains_key("access-control-request-method")
    {
        let mut response = StatusCode::NO_CONTENT.into_response();
        add_cors(response.headers_mut(), origin.as_deref());
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, DELETE, OPTIONS"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(CORS_ALLOW_HEADERS),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
        return response;
    }

    let mut response = next.run(req).await;
    add_cors(response.headers_mut(), origin.as_deref());
    response
}

fn add_cors(headers: &mut HeaderMap, origin: Option<&str>) {
    let Some(origin) = origin.and_then(|o| HeaderValue::from_str(o).ok()) else {
        return;
    };
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    headers.insert(header::VARY, HeaderValue::from_static("origin"));
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static(CORS_EXPOSE_HEADERS),
    );
}

/// The presented token: `Authorization: Bearer <token>`, or a
/// `cutweave.token.<token>` entry of `Sec-WebSocket-Protocol`. Never a query parameter.
fn presented_token(headers: &HeaderMap) -> Option<&str> {
    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let (scheme, rest) = value.split_once(' ')?;
        return scheme.eq_ignore_ascii_case("bearer").then_some(rest.trim());
    }
    headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())?
        .split(',')
        .find_map(|p| p.trim().strip_prefix(WS_TOKEN_SUBPROTOCOL_PREFIX))
}

/// Maps the presented token to a credential and attaches the derived [`Authenticated`].
async fn authenticate(State(shared): State<Arc<Shared>>, mut req: Request, next: Next) -> Response {
    let rid = request_id_of(&req);
    // CORS preflights carry no credentials; the Host/Origin layer answers them before here.
    let credential = presented_token(req.headers()).and_then(|t| shared.creds.authenticate(t));
    let Some(credential) = credential else {
        let mut response = error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing or invalid bearer token",
            rid.as_deref(),
        );
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return response;
    };
    let conn = ConnId::new(shared.next_conn.fetch_add(1, Ordering::Relaxed));
    let Ok(ctx) = credential.actor_ctx(conn) else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "credential has no valid actor",
            rid.as_deref(),
        );
    };
    req.extensions_mut()
        .insert(Authenticated { ctx, credential });
    next.run(req).await
}

fn unauthorized(code: &str, message: &str, rid: Option<&str>) -> Response {
    let mut response = error_response(StatusCode::UNAUTHORIZED, code, message, rid);
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

/// `/media` (WP-1I.03): a bearer token as for `/rpc` (the request also gets the URL signer, for
/// `POST /media/sign`), or, for `GET` and `HEAD`, a valid unexpired signature over the path and
/// query. A presented but unknown token is refused even when the URL is signed.
async fn authenticate_media(
    State(shared): State<Arc<Shared>>,
    mut req: Request,
    next: Next,
) -> Response {
    let rid = request_id_of(&req);
    if let Some(token) = presented_token(req.headers()) {
        let Some(credential) = shared.creds.authenticate(token) else {
            return unauthorized(
                "unauthorized",
                "missing or invalid bearer token",
                rid.as_deref(),
            );
        };
        let conn = ConnId::new(shared.next_conn.fetch_add(1, Ordering::Relaxed));
        let Ok(ctx) = credential.actor_ctx(conn) else {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "credential has no valid actor",
                rid.as_deref(),
            );
        };
        req.extensions_mut()
            .insert(Authenticated { ctx, credential });
        req.extensions_mut().insert(Arc::clone(&shared.signer));
        return next.run(req).await;
    }
    let signed = req
        .uri()
        .query()
        .is_some_and(|q| q.contains(&format!("{SIGNATURE_PARAM}=")));
    if !signed {
        return unauthorized(
            "unauthorized",
            "send a bearer token, or use a URL from POST /media/sign",
            rid.as_deref(),
        );
    }
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        return unauthorized(
            "bad_signature",
            "signed URLs are for GET and HEAD only",
            rid.as_deref(),
        );
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    match shared
        .signer
        .verify(req.uri().path(), req.uri().query(), now)
    {
        Ok(()) => next.run(req).await,
        Err(SignError::Expired) => unauthorized(
            "expired_signature",
            &SignError::Expired.to_string(),
            rid.as_deref(),
        ),
        Err(e) => unauthorized("bad_signature", &e.to_string(), rid.as_deref()),
    }
}
