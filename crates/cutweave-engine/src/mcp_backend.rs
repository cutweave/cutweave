// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine behind the MCP endpoints (WP-1J.01): [`EngineHandle`] implements
//! [`mcp::Dispatch`] and [`Connection`] implements [`mcp::DispatchConn`], so `cutweave-api`'s
//! [`mcp::router`] can serve `/mcp`, `/mcp/minimal` and `/mcp/full` without depending on this
//! crate (the dependency points the other way). [`mcp_mounts`] mounts MCP alone;
//! [`api_mounts`] is what the engine process uses: `/rpc`, the MCP endpoints and `/media`
//! (WP-1I.03) together.

use std::sync::Arc;

use cutweave_api::mcp::{self, Dispatch, DispatchCall, DispatchConn, DispatchReply, McpOptions};
use cutweave_api::media::MediaOptions;
use cutweave_api::rpc::RpcConfig;
use cutweave_api::{Credential, Mounts};
use cutweave_commands::{CmdError, Registry};

use crate::config::MountFactory;
use crate::dispatch::{Call, Connection};
use crate::engine::EngineHandle;
use crate::media_http::media_mounts;
use crate::rpc_backend::rpc_mounts;

/// Mounts the MCP endpoints with `options`.
pub fn mcp_mounts(options: McpOptions) -> MountFactory {
    Box::new(move |handle: EngineHandle| Mounts {
        mcp: Some(mcp::router(handle, &options)),
        ..Mounts::default()
    })
}

/// Mounts `/rpc` (JSON-RPC, WP-1I.02), the MCP endpoints (WP-1J.01) and `/media` (WP-1I.03,
/// default options) over one engine.
pub fn api_mounts(rpc: RpcConfig, options: McpOptions) -> MountFactory {
    let rpc = rpc_mounts(rpc);
    let mcp = mcp_mounts(options);
    let media = media_mounts(MediaOptions::default());
    Box::new(move |handle: EngineHandle| {
        let rpc = rpc(handle.clone());
        let mcp = mcp(handle.clone());
        let media = media(handle);
        Mounts {
            rpc: rpc.rpc,
            mcp: mcp.mcp,
            media: media.media,
        }
    })
}

impl Dispatch for EngineHandle {
    fn registry(&self) -> Arc<Registry> {
        EngineHandle::registry(self)
    }

    fn connect(&self, credential: &Credential) -> Result<Arc<dyn DispatchConn>, CmdError> {
        Ok(Arc::new(EngineHandle::connect(self, credential)?))
    }
}

impl DispatchConn for Connection {
    fn call(&self, call: DispatchCall) -> Result<DispatchReply, CmdError> {
        let DispatchCall {
            name,
            params,
            request_id,
            expected_rev,
            tx,
        } = call;
        let reply = Connection::call(
            self,
            Call {
                name,
                params,
                request_id,
                expected_rev,
                tx,
            },
        )?;
        Ok(DispatchReply {
            output: reply.output,
            rev: reply.rev,
            committed_rev: reply.committed_rev,
            ops: reply.applied.map(|a| a.ops),
        })
    }
}
