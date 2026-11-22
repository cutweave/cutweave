// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-api`: the engine's single local server: JSON-RPC over WebSocket, MCP, HTTP media
//! endpoints and auth (arch §6).
//!
//! WP-1I.01 ships the skeleton: loopback axum server ([`ApiServer`]), credential classes and
//! actor derivation ([`creds`]), Host/Origin validation, the run-file ([`runfile`]) and the
//! spawn handshake ([`launch`]). Handlers for `/rpc`, `/mcp` and `/media` are mounted through
//! [`Mounts`] by later work packages.

pub mod creds;
pub mod launch;
pub mod runfile;
mod server;

pub use creds::{AgentToken, Credential, Credentials, CredentialsError, Token, TokenError};
pub use runfile::{RunFile, RunFileContents, RunFileToken};
pub use server::{
    ApiConfig, ApiError, ApiServer, Authenticated, EXIT_PORT_IN_USE, Mounts, RequestId,
    RunningServer, ShutdownHandle, WS_TOKEN_SUBPROTOCOL_PREFIX, require_scope,
};
