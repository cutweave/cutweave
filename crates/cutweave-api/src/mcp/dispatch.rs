// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine seam of the MCP server.
//!
//! `cutweave-engine` depends on this crate (it starts the [`ApiServer`](crate::ApiServer)), so
//! the MCP module cannot name the engine's `EngineHandle` / `Connection`. It reaches the engine
//! through [`Dispatch`] instead, which `cutweave_engine::mcp_backend` implements for the
//! engine's `EngineHandle` and `Connection` (`cutweave_engine::mcp_mounts` /
//! `api_mounts` build the mount). The mapping is field for field: [`DispatchCall`] is the
//! engine's `Call`, and [`DispatchReply`] its `Reply` with the doc actor's step patch.
//!
//! It parallels the JSON-RPC seam ([`RpcBackend`](crate::rpc::RpcBackend)) but stays separate:
//! MCP runs `cutweave_execute` as one blocking sequence (so a cancelled request still ends its
//! transaction) and needs each step's patch for dry runs, which `RpcReply` does not carry.

use std::sync::Arc;

use cutweave_commands::{CmdError, Patch, Registry, RequestId, Rev, TxId};
use serde_json::Value;

use crate::creds::Credential;
use crate::rpc::RpcEvents;

/// One request to the engine: a registered name, its params and the envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct DispatchCall {
    /// The command, query or engine command (`clip.split`, `timeline.get`, `job.wait`).
    pub name: String,
    /// Its params (a JSON object).
    pub params: Value,
    /// Idempotency key; the engine generates one when absent.
    pub request_id: Option<RequestId>,
    /// Fail with `Conflict{stale_rev}` unless the document is at this revision.
    pub expected_rev: Option<Rev>,
    /// Run inside this open transaction of the connection.
    pub tx: Option<TxId>,
}

impl DispatchCall {
    /// `name` with `params` and no envelope.
    #[must_use]
    pub fn new(name: impl Into<String>, params: Value) -> DispatchCall {
        DispatchCall {
            name: name.into(),
            params,
            request_id: None,
            expected_rev: None,
            tx: None,
        }
    }

    /// With an expected revision (`None` leaves it unset).
    #[must_use]
    pub fn expecting(mut self, rev: Option<Rev>) -> DispatchCall {
        self.expected_rev = rev;
        self
    }

    /// Inside an open transaction.
    #[must_use]
    pub fn in_tx(mut self, tx: TxId) -> DispatchCall {
        self.tx = Some(tx);
        self
    }
}

/// The result of a [`DispatchCall`].
#[derive(Clone, Debug, PartialEq)]
pub struct DispatchReply {
    /// The output (a JSON object), exactly what the RPC layer returns.
    pub output: Value,
    /// The live document revision after the call.
    pub rev: Rev,
    /// The committed revision after the call.
    pub committed_rev: Rev,
    /// What the call changed, when it ran on the doc actor: the committed patch of an implicit
    /// transaction, or the step's patch inside an open transaction (what a dry run predicts).
    pub ops: Option<Patch>,
}

/// The engine as the MCP server sees it.
pub trait Dispatch: Send + Sync + 'static {
    /// The registry the tool catalog is generated from (names, schemas, annotations).
    fn registry(&self) -> Arc<Registry>;

    /// Opens a connection for `credential`: its actor (`agent:<label>` for agent tokens) and
    /// access are derived from the credential, never from the request (ADR-0016). Every
    /// connection gets a connection id of its own; dropping the last reference closes it and
    /// rolls back its open transaction (WP-1B.09).
    fn connect(&self, credential: &Credential) -> Result<Arc<dyn DispatchConn>, CmdError>;

    /// A subscription to the engine's event fan-out (`doc.patched`, ...), the same stream the
    /// JSON-RPC hub reads. The MCP [`Notifier`](crate::mcp::Notifier) turns it into resource
    /// change notifications.
    fn events(&self) -> Box<dyn RpcEvents>;
}

/// One engine connection (one per MCP session; one per request on the stateless revision).
pub trait DispatchConn: Send + Sync + 'static {
    /// Runs `call` and waits for the result. Blocking (`job.wait` waits up to 60 s): the MCP
    /// server calls it on the blocking pool, never on an async worker.
    fn call(&self, call: DispatchCall) -> Result<DispatchReply, CmdError>;
}
