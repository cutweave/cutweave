// SPDX-License-Identifier: GPL-3.0-or-later
//! How the engine runs the relink commands (WP-2F.03; arch §7): the
//! [`RelinkSink`](cutweave_ops::media::RelinkSink) it supplies.
//!
//! [`RelinkTx`] records the locations a relink found with `media.set_location` in **one batch
//! transaction on a connection of its own** (same actor as the caller), so a relink is one
//! undo step and never collides with a transaction the caller has open. The connection is
//! closed afterwards (a failure rolls back).

use cutweave_commands::{ActorCtx, CmdError, ConnId, DocHandle, Submit, TxKind};
use cutweave_ops::media::{MediaSetLocation, RelinkSink};

/// Records relinked locations in one transaction on its own connection (see the module docs).
pub(crate) struct RelinkTx {
    handle: DocHandle,
    ctx: ActorCtx,
}

impl RelinkTx {
    /// A sink submitting to `handle` as `ctx` (a connection nobody else uses).
    pub(crate) fn new(handle: DocHandle, ctx: ActorCtx) -> RelinkTx {
        RelinkTx { handle, ctx }
    }

    fn submit_all(&self, batch: &[MediaSetLocation]) -> Result<(), CmdError> {
        let tx = self
            .handle
            .begin_tx(&self.ctx, TxKind::Batch, "Relink media")
            .wait()?
            .id;
        for set in batch {
            let params = serde_json::to_value(set).map_err(|e| {
                CmdError::internal(format!("cannot encode media.set_location: {e}"))
            })?;
            let submit = Submit::new(self.ctx.clone(), "media.set_location", params).in_tx(tx);
            self.handle.submit(submit).wait()?;
        }
        self.handle.commit_tx(&self.ctx, tx).wait().map(drop)
    }
}

impl RelinkSink for RelinkTx {
    fn relocate(&mut self, batch: Vec<MediaSetLocation>) -> Result<(), CmdError> {
        // The connection closes when the sink is dropped, which rolls back an open transaction.
        self.submit_all(&batch)
    }
}

impl Drop for RelinkTx {
    fn drop(&mut self) {
        if self.ctx.conn() != ConnId::INTERNAL {
            drop(self.handle.close_connection(self.ctx.conn()));
        }
    }
}
