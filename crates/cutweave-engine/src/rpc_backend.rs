// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine behind the JSON-RPC endpoint (WP-1I.02): [`EngineHandle`] implements
//! [`RpcBackend`] and [`Connection`] implements [`RpcConnection`], so `cutweave-api`'s
//! [`rpc::router`] can serve `/rpc` without depending on this crate (the dependency points the
//! other way). [`rpc_mounts`] is the [`MountFactory`] the engine process uses.

use std::sync::Arc;

use cutweave_api::rpc::{
    self, BoxFuture, RpcBackend, RpcCall, RpcConfig, RpcConnection, RpcEvent, RpcEventError,
    RpcEvents, RpcReply, RpcSnapshot, RpcStatus,
};
use cutweave_api::{Credential, Mounts};
use cutweave_commands::{ActorCtx, CmdError, Registry};
use tokio::sync::broadcast::error::RecvError;

use crate::config::MountFactory;
use crate::dispatch::{Call, Connection};
use crate::engine::{EngineHandle, VERSION};
use crate::events::{EngineEvent, EventReceiver};

/// Mounts `/rpc` (the JSON-RPC WebSocket) with `config`.
pub fn rpc_mounts(config: RpcConfig) -> MountFactory {
    Box::new(move |handle: EngineHandle| Mounts {
        rpc: Some(rpc::router(Arc::new(handle), config)),
        ..Mounts::default()
    })
}

impl RpcBackend for EngineHandle {
    fn version(&self) -> String {
        VERSION.to_owned()
    }

    fn registry(&self) -> Arc<Registry> {
        EngineHandle::registry(self)
    }

    fn connect(&self, credential: &Credential) -> Result<Arc<dyn RpcConnection>, CmdError> {
        Ok(Arc::new(EngineHandle::connect(self, credential)?))
    }

    fn subscribe(&self) -> Box<dyn RpcEvents> {
        events(self)
    }

    fn status(&self) -> Result<RpcStatus, CmdError> {
        let snap = EngineHandle::snapshot(self)?;
        let status = EngineHandle::status(self)?;
        Ok(RpcStatus {
            rev: snap.rev,
            committed_rev: snap.committed_rev,
            project: serde_json::to_value(status)
                .map_err(|e| CmdError::internal(format!("project status: {e}")))?,
            // The preview ring of the playback subsystem (WP-1I.05): its announcement JSON
            // `{name, epoch, layout_version, mapping_size}` (the client also takes an object).
            shm: EngineHandle::preview_ring(self),
        })
    }

    fn snapshot(&self) -> Result<RpcSnapshot, CmdError> {
        let snap = EngineHandle::snapshot(self)?;
        Ok(RpcSnapshot {
            rev: snap.rev,
            committed_rev: snap.committed_rev,
            project: serde_json::to_value(&snap.doc)
                .map_err(|e| CmdError::internal(format!("project snapshot: {e}")))?,
        })
    }
}

impl RpcConnection for Connection {
    fn ctx(&self) -> &ActorCtx {
        Connection::ctx(self)
    }

    fn call(&self, call: RpcCall) -> BoxFuture<'static, Result<RpcReply, CmdError>> {
        let conn = self.clone();
        Box::pin(async move {
            let RpcCall {
                name,
                params,
                request_id,
                expected_rev,
                tx,
                replace,
            } = call;
            let reply = conn
                .call_async(Call {
                    name,
                    params,
                    request_id,
                    expected_rev,
                    tx,
                    replace,
                })
                .await?;
            let (request_id, tx_id) = reply
                .applied
                .as_ref()
                .map_or((None, None), |a| (Some(a.request_id), a.tx_id));
            Ok(RpcReply {
                output: reply.output,
                rev: reply.rev,
                committed_rev: reply.committed_rev,
                request_id,
                tx_id,
            })
        })
    }
}

/// A subscription to the engine's event fan-out (the RPC hub's, and MCP's resource
/// notifications).
pub(crate) fn events(handle: &EngineHandle) -> Box<dyn RpcEvents> {
    Box::new(Events(EngineHandle::subscribe(handle)))
}

/// The event fan-out as the RPC hub reads it.
struct Events(EventReceiver);

impl RpcEvents for Events {
    fn recv(&mut self) -> BoxFuture<'_, Result<RpcEvent, RpcEventError>> {
        Box::pin(async move {
            match self.0.recv().await {
                Ok(event) => Ok(to_rpc(&event)),
                Err(RecvError::Lagged(missed)) => Err(RpcEventError::Lagged(missed)),
                Err(RecvError::Closed) => Err(RpcEventError::Closed),
            }
        })
    }
}

fn to_rpc(event: &EngineEvent) -> RpcEvent {
    RpcEvent {
        name: event.name().into(),
        payload: event.payload(),
        origin: match event {
            EngineEvent::View { origin, .. } => *origin,
            _ => None,
        },
    }
}
