// SPDX-License-Identifier: GPL-3.0-or-later
//! MCP over stdio for an in-process engine (WP-1J.03; arch §14, ADR-0009):
//! `cutweave mcp --headless`.
//!
//! One process is one MCP session: the [`CutweaveMcp`] handler is built with a fixed credential
//! (the process boundary is the credential; the CLI uses `Credential::Agent("mcp")`, so the
//! actor is `agent:mcp`) and opens one engine connection on first use. Closing stdin ends the
//! session; the handler and its connection drop, which rolls back a transaction the session
//! still holds (WP-1B.09).
//!
//! # Spec revisions
//!
//! rmcp's server lifecycle picks the revision from the first message: an `initialize` request
//! negotiates **2025-11-25** (or an older revision the client asks for); `server/discover`
//! followed by requests carrying the protocol version in `_meta` runs **2026-07-28** without a
//! handshake. Over stdio both keep the one engine connection for the whole process, unlike
//! stateless HTTP, where every request gets a fresh connection.
//!
//! # Stdout
//!
//! Stdout carries protocol frames only: one JSON-RPC message per line, written by rmcp's
//! transport. Nothing else in the engine writes to stdout; logs go to the log file and
//! diagnostics to stderr.

use std::future::Future;

use rmcp::{
    ServiceExt as _,
    service::{RoleServer, ServerInitializeError},
    transport::async_rw::AsyncRwTransport,
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::mcp::CutweaveMcp;

/// How a stdio session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// The client closed stdin (the normal end of a stdio session).
    InputClosed,
    /// The shutdown future completed first (Ctrl-C / SIGTERM).
    Shutdown,
}

/// A stdio session that failed.
#[derive(Debug, thiserror::Error)]
pub enum StdioError {
    /// The client's first messages were not a valid session start.
    #[error("the MCP session did not start: {0}")]
    Handshake(String),
    /// The session task failed.
    #[error("the MCP session failed: {0}")]
    Session(String),
}

/// Serves `handler` over the process's stdin and stdout until stdin closes or `shutdown`
/// completes.
///
/// # Errors
/// See [`serve`].
pub async fn serve_stdio(
    handler: CutweaveMcp,
    shutdown: impl Future<Output = ()>,
) -> Result<Ended, StdioError> {
    let (read, write) = rmcp::transport::stdio_pipes();
    serve(handler, read, write, shutdown).await
}

/// Serves `handler` over `read` (client to server) and `write` (server to client), one
/// JSON-RPC message per line, until `read` ends or `shutdown` completes.
///
/// An input that closes before the session started is a normal end ([`Ended::InputClosed`]).
///
/// # Errors
/// [`StdioError::Handshake`] when the first messages are neither `initialize` nor a request
/// with the 2026-07-28 `_meta`; [`StdioError::Session`] when the session task fails.
pub async fn serve<R, W>(
    handler: CutweaveMcp,
    read: R,
    write: W,
    shutdown: impl Future<Output = ()>,
) -> Result<Ended, StdioError>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let transport = AsyncRwTransport::<RoleServer, R, W>::new_server(read, write);
    tokio::pin!(shutdown);
    let running = tokio::select! {
        started = handler.serve(transport) => match started {
            Ok(running) => running,
            Err(ServerInitializeError::ConnectionClosed(_)) => return Ok(Ended::InputClosed),
            Err(e) => return Err(StdioError::Handshake(e.to_string())),
        },
        () = &mut shutdown => return Ok(Ended::Shutdown),
    };
    if let Some(info) = running.peer().peer_info() {
        tracing::info!(
            protocol = %info.protocol_version,
            client = %info.client_info.name,
            "MCP stdio session started"
        );
    } else {
        tracing::info!("MCP stdio session started (stateless revision)");
    }
    let cancel = running.cancellation_token();
    let waiting = running.waiting();
    tokio::pin!(waiting);
    tokio::select! {
        quit = &mut waiting => {
            quit.map_err(|e| StdioError::Session(e.to_string()))?;
            tracing::info!("MCP stdio session ended: input closed");
            Ok(Ended::InputClosed)
        }
        () = &mut shutdown => {
            cancel.cancel();
            let _ = waiting.await;
            tracing::info!("MCP stdio session ended: shutdown signal");
            Ok(Ended::Shutdown)
        }
    }
}
