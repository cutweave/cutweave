// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared test helpers: rmcp clients for both spec revisions over every transport, and a raw
//! HTTP/1.1 helper for the security tests.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value
)]

use std::{fmt::Write as _, net::SocketAddr, time::Duration};

use base64::Engine as _;
use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt as _, RoleClient, ServiceExt as _,
    model::{
        CallToolRequestParams, CallToolResult, ClientConfig, ContentBlock, ProtocolVersion,
        ResourceUpdatedNotificationParam,
    },
    service::{NotificationContext, RunningService, ServiceError},
    transport::{
        IntoTransport, StreamableHttpClientTransport,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use s5_bus::{Caller, EngineConfig};
use s5_mcp::{
    Catalog, CutweaveMcp, EngineHandle, Profile,
    auth::AgentToken,
    http::{self, HttpOptions, RunningHttp},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    sync::mpsc,
};

pub const FLICKS: i64 = 705_600_000;

/// The two spec revisions under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Revision {
    /// 2025-11-25: `initialize` handshake, sessions, `resources/subscribe`.
    Legacy,
    /// 2026-07-28: `server/discover`, per-request `_meta`, stateless, `subscriptions/listen`.
    Modern,
}

impl Revision {
    pub const ALL: [Revision; 2] = [Revision::Legacy, Revision::Modern];

    pub fn version(self) -> ProtocolVersion {
        match self {
            Revision::Legacy => ProtocolVersion::V_2025_11_25,
            Revision::Modern => ProtocolVersion::V_2026_07_28,
        }
    }
}

/// Client handler that records legacy `notifications/resources/updated`.
#[derive(Clone)]
pub struct Recorder {
    version: ProtocolVersion,
    updates: mpsc::UnboundedSender<String>,
}

impl ClientHandler for Recorder {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default().with_protocol_version(self.version.clone())
    }

    async fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let _ = self.updates.send(params.uri);
    }
}

pub struct Client {
    pub service: RunningService<RoleClient, Recorder>,
    pub updates: mpsc::UnboundedReceiver<String>,
    pub revision: Revision,
}

/// Connects a client speaking `revision` over `transport`.
pub async fn connect<T, E, A>(transport: T, revision: Revision) -> Client
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    connect_with(transport, revision, revision.version()).await
}

/// Like [`connect`], but a legacy client may ask for another `initialize` version.
pub async fn connect_with<T, E, A>(
    transport: T,
    revision: Revision,
    version: ProtocolVersion,
) -> Client
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let (tx, updates) = mpsc::unbounded_channel();
    let handler = Recorder {
        version: version.clone(),
        updates: tx,
    };
    let service = match revision {
        Revision::Legacy => handler.serve(transport).await.expect("initialize"),
        Revision::Modern => handler
            .serve_with_lifecycle(
                transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![version],
                },
            )
            .await
            .expect("discover"),
    };
    Client {
        service,
        updates,
        revision,
    }
}

/// Deterministic engine.
pub fn engine() -> EngineHandle {
    EngineHandle::new(EngineConfig::deterministic(42))
}

/// Server and client in one process over an in-memory duplex pipe (the stdio framing, minus the
/// process boundary). The actor is `agent:test`.
pub async fn in_process(engine: EngineHandle, profile: Profile, revision: Revision) -> Client {
    let (server_io, client_io) = tokio::io::duplex(1 << 22);
    let handler = CutweaveMcp::new(
        engine,
        Catalog::shared(profile),
        Some(Caller::agent("test")),
    );
    tokio::spawn(async move {
        if let Ok(running) = handler.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    connect(client_io, revision).await
}

pub const ALLOWED_ORIGIN: &str = "http://localhost:6274";

/// HTTP server on a random port with one agent token (`agent:http-test`).
pub async fn http_server(engine: EngineHandle) -> (RunningHttp, AgentToken) {
    let token = AgentToken::generate("http-test").unwrap();
    let running = http::serve(
        engine,
        HttpOptions {
            port: 0,
            tokens: vec![token.clone()],
            allowed_origins: vec![ALLOWED_ORIGIN.into()],
        },
    )
    .await
    .unwrap();
    (running, token)
}

/// rmcp Streamable HTTP client with a bearer token.
pub async fn http_client(url: &str, secret: &str, revision: Revision) -> Client {
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url).auth_header(secret),
    );
    connect(transport, revision).await
}

/// Child process speaking stdio (`s5-mcp stdio ...`).
pub async fn stdio_child(args: &[&str], revision: Revision) -> Client {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_s5-mcp"));
    cmd.arg("stdio").args(args).env("S5_MCP_LOG", "warn");
    let transport = rmcp::transport::TokioChildProcess::new(cmd).expect("spawn s5-mcp");
    connect(transport, revision).await
}

impl Client {
    pub fn negotiated(&self) -> ProtocolVersion {
        self.service
            .peer_info()
            .expect("peer info")
            .protocol_version
            .clone()
    }

    pub async fn try_call(&self, tool: &str, args: Value) -> Result<CallToolResult, ServiceError> {
        let Value::Object(args) = args else {
            panic!("args must be an object")
        };
        self.service
            .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args))
            .await
    }

    pub async fn call(&self, tool: &str, args: Value) -> CallToolResult {
        self.try_call(tool, args).await.expect("tools/call")
    }

    /// Calls and returns `structuredContent`, panicking on a tool error.
    pub async fn ok(&self, tool: &str, args: Value) -> Value {
        let result = self.call(tool, args).await;
        assert_ne!(
            result.is_error,
            Some(true),
            "{tool} failed: {}",
            text_of(&result)
        );
        result.structured_content.expect("structuredContent")
    }

    /// Calls and returns the typed error, panicking on success.
    pub async fn err(&self, tool: &str, args: Value) -> Value {
        let result = self.call(tool, args).await;
        assert_eq!(result.is_error, Some(true), "{tool} unexpectedly succeeded");
        let v: Value = serde_json::from_str(&text_of(&result)).expect("error JSON");
        v["error"].clone()
    }

    /// Waits for the next legacy `resources/updated` notification.
    pub async fn next_update(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(10), self.updates.recv())
            .await
            .expect("notification within 10 s")
            .expect("notification channel open")
    }

    pub async fn close(self) {
        let _ = self.service.cancel().await;
    }
}

/// Concatenated text content.
pub fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect()
}

/// The image blocks of a result: (base64 data, mime).
pub fn images_of(result: &CallToolResult) -> Vec<(String, String)> {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            ContentBlock::Image(i) => Some((i.data.clone(), i.mime_type.clone())),
            _ => None,
        })
        .collect()
}

/// Decodes a base64 PNG; returns (width, height, top-left RGB).
pub fn decode_png(data: &str) -> (u32, u32, [u8; 3]) {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .expect("base64");
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("png header");
    let mut buf = vec![0; reader.output_buffer_size().expect("size")];
    let info = reader.next_frame(&mut buf).expect("png frame");
    (info.width, info.height, [buf[0], buf[1], buf[2]])
}

/// A raw HTTP/1.1 exchange (`Connection: close`).
pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl RawResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// POSTs `body` with exactly the given headers (plus Content-Length / Connection: close).
pub async fn raw_post(
    addr: SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> RawResponse {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut req = format!("POST {path} HTTP/1.1\r\n");
    for (k, v) in headers {
        let _ = write!(req, "{k}: {v}\r\n");
    }
    let _ = write!(
        req,
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut raw))
        .await
        .expect("response within 10 s")
        .unwrap();
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    RawResponse {
        status,
        headers,
        body: body.to_owned(),
    }
}

/// Headers a well-behaved local agent sends.
pub fn good_headers<'a>(host: &'a str, bearer: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("Host", host),
        ("Authorization", bearer),
        ("Content-Type", "application/json"),
        ("Accept", "application/json, text/event-stream"),
    ]
}

/// A legacy `initialize` request body.
pub fn initialize_body() -> String {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "raw", "version": "0" }
        }
    })
    .to_string()
}

/// A 2026-07-28 `server/discover` request body.
pub fn discover_body() -> String {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "server/discover",
        "params": { "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": { "name": "raw", "version": "0" }
        } }
    })
    .to_string()
}

/// Seconds as flicks.
pub fn secs(s: i64) -> i64 {
    s * FLICKS
}
