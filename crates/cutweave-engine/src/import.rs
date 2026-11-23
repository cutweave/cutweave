// SPDX-License-Identifier: GPL-3.0-or-later
//! How the engine runs `media.import` (WP-1D.07 handoff; arch §7, §14, §18): the per-call
//! [`ImportCtx`](cutweave_ops::media::ImportCtx) parts it supplies.
//!
//! - [`ImportSink`]: registers the probed batch with `media.register` in **one batch
//!   transaction on a connection of its own** (same actor as the caller), so the import is one
//!   undo step and never collides with a transaction the caller has open. The connection is
//!   closed afterwards (a failure rolls back).
//! - [`BusAnalysisSink`]: forwards `media.indexed` / `media.analysis_failed` from analysis jobs
//!   into the event fan-out.
//! - [`NoMediaBackend`]: the prober of a core engine without the media subsystem (WP-1I.05):
//!   every file is reported in `failed[]`.

use cutweave_commands::{Actor, ActorCtx, CmdError, ConnId, DocHandle, Submit, TxKind};
use cutweave_ops::media::{
    AnalysisSink, MediaEvent, MediaProbe, MediaRegister, MediaRegistered, MediaSink, ProbeError,
    Probed,
};

use crate::events::{EngineEvent, EventBus};

/// The same actor as `actor` on connection `conn` (`system` always uses the internal
/// connection).
pub(crate) fn ctx_for(actor: &Actor, conn: ConnId) -> ActorCtx {
    let made = match actor {
        Actor::User => Ok(ActorCtx::ui(conn)),
        Actor::Agent(label) => ActorCtx::agent(conn, label.clone()),
        Actor::Plugin(id) => ActorCtx::plugin(conn, id.clone()),
        Actor::System => Ok(ActorCtx::system()),
    };
    // The actor was valid when the caller's context was built, so this cannot fail.
    made.unwrap_or_else(|_| ActorCtx::system())
}

/// Registers an import batch in one transaction on its own connection (see the module docs).
pub(crate) struct ImportSink {
    handle: DocHandle,
    ctx: ActorCtx,
}

impl ImportSink {
    /// A sink submitting to `handle` as `ctx` (a connection nobody else uses).
    pub(crate) fn new(handle: DocHandle, ctx: ActorCtx) -> ImportSink {
        ImportSink { handle, ctx }
    }

    fn register_all(&self, batch: &[MediaRegister]) -> Result<Vec<MediaRegistered>, CmdError> {
        let tx = self
            .handle
            .begin_tx(&self.ctx, TxKind::Batch, "Import media")
            .wait()?
            .id;
        let mut out = Vec::with_capacity(batch.len());
        for (i, register) in batch.iter().enumerate() {
            let params = serde_json::to_value(register)
                .map_err(|e| CmdError::internal(format!("cannot encode media.register: {e}")))?;
            let submit = Submit::new(self.ctx.clone(), "media.register", params).in_tx(tx);
            let applied = match self.handle.submit(submit).wait() {
                Ok(applied) => applied,
                Err(e) => {
                    let _ = self.handle.cancel_tx(&self.ctx, tx).wait();
                    return Err(match e {
                        CmdError::Invalid { path, msg } => {
                            CmdError::invalid(format!("/paths/{i}{path}"), msg)
                        }
                        other => other,
                    });
                }
            };
            let registered = serde_json::from_value(applied.output)
                .map_err(|e| CmdError::internal(format!("unexpected media.register output: {e}")));
            match registered {
                Ok(r) => out.push(r),
                Err(e) => {
                    let _ = self.handle.cancel_tx(&self.ctx, tx).wait();
                    return Err(e);
                }
            }
        }
        if let Err(e) = self.handle.commit_tx(&self.ctx, tx).wait() {
            let _ = self.handle.cancel_tx(&self.ctx, tx).wait();
            return Err(e);
        }
        Ok(out)
    }
}

impl MediaSink for ImportSink {
    fn register(&mut self, batch: Vec<MediaRegister>) -> Result<Vec<MediaRegistered>, CmdError> {
        self.register_all(&batch)
    }
}

impl Drop for ImportSink {
    fn drop(&mut self) {
        // Rolls back anything still open (a panic mid-import) and retires the connection.
        if self.ctx.conn() != ConnId::INTERNAL {
            drop(self.handle.close_connection(self.ctx.conn()));
        }
    }
}

/// Forwards analysis events into the engine fan-out.
#[derive(Clone, Debug)]
pub(crate) struct BusAnalysisSink(pub(crate) EventBus);

impl AnalysisSink for BusAnalysisSink {
    fn publish(&self, event: MediaEvent) {
        self.0.publish(EngineEvent::Media(event));
    }
}

/// The prober of a core engine: no media backend is attached (WP-1I.05 attaches FFmpeg).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoMediaBackend;

impl MediaProbe for NoMediaBackend {
    fn probe(&self, path: &str) -> Result<Probed, ProbeError> {
        Err(ProbeError::Io {
            path: path.to_owned(),
            reason: "this engine has no media backend attached (core only)".to_owned(),
        })
    }
}
