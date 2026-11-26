// SPDX-License-Identifier: GPL-3.0-or-later
//! How the engine runs `media.import` (WP-1D.07 handoff; arch §7, §14, §18): the per-call
//! [`ImportCtx`](cutweave_ops::media::ImportCtx) parts it supplies.
//!
//! - [`ImportSink`]: registers the probed batch with `media.register` in **one batch
//!   transaction on a connection of its own** (same actor as the caller), so the import is one
//!   undo step and never collides with a transaction the caller has open. The connection is
//!   closed afterwards (a failure rolls back).
//!   With folders (WP-2F.09) it submits one `media.register_tree` instead, in the same kind of
//!   transaction. A slideshow (WP-2F.05d) is its registrations followed by one
//!   `timeline.insert` per picture, and an imported frame its registration followed by
//!   `media.rename`, each again in one transaction: one undo step.
//! - [`BusAnalysisSink`]: forwards `media.indexed` / `media.analysis_failed` from analysis jobs
//!   and `media.import_progress` from the import itself into the event fan-out.
//! - [`NoMediaBackend`]: the prober of a core engine without the media subsystem (WP-1I.05):
//!   every file is reported in `failed[]`.

use cutweave_commands::{Actor, ActorCtx, CmdError, ConnId, DocHandle, Submit, TxId, TxKind};
use cutweave_model::Project;
use cutweave_ops::media::{
    AnalysisSink, FrameRegistered, MediaEvent, MediaProbe, MediaRegister, MediaRegisterTree,
    MediaRegistered, MediaSink, MediaTreeRegistered, ProbeError, Probed, SlideshowBatch,
    SlideshowPlaced, SlideshowSteps, asset_name, place_slideshow,
};
use cutweave_ops::timeline::{TimelineInsert, TimelineInsertOutput};

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

impl ImportSink {
    /// `media.register_tree` (the files and their bin folders) in one transaction.
    fn register_folders(&self, tree: &MediaRegisterTree) -> Result<MediaTreeRegistered, CmdError> {
        let tx = self
            .handle
            .begin_tx(&self.ctx, TxKind::Batch, "Import folder")
            .wait()?
            .id;
        let params = serde_json::to_value(tree)
            .map_err(|e| CmdError::internal(format!("cannot encode media.register_tree: {e}")))?;
        let submit = Submit::new(self.ctx.clone(), "media.register_tree", params).in_tx(tx);
        let registered = self
            .handle
            .submit(submit)
            .wait()
            .and_then(|applied| {
                serde_json::from_value(applied.output).map_err(|e| {
                    CmdError::internal(format!("unexpected media.register_tree output: {e}"))
                })
            })
            .and_then(|out: MediaTreeRegistered| {
                self.handle.commit_tx(&self.ctx, tx).wait().map(|_| out)
            });
        if registered.is_err() {
            let _ = self.handle.cancel_tx(&self.ctx, tx).wait();
        }
        registered
    }
}

/// The document commands of an import that is more than registrations, submitted into one open
/// transaction of the sink's connection.
struct OpenTx<'a> {
    sink: &'a ImportSink,
    tx: TxId,
}

impl OpenTx<'_> {
    fn run<T: serde::de::DeserializeOwned>(
        &self,
        name: &str,
        params: &impl serde::Serialize,
    ) -> Result<T, CmdError> {
        let params = serde_json::to_value(params)
            .map_err(|e| CmdError::internal(format!("cannot encode {name}: {e}")))?;
        let submit = Submit::new(self.sink.ctx.clone(), name, params).in_tx(self.tx);
        let applied = self.sink.handle.submit(submit).wait()?;
        serde_json::from_value(applied.output)
            .map_err(|e| CmdError::internal(format!("unexpected {name} output: {e}")))
    }
}

impl SlideshowSteps for OpenTx<'_> {
    fn doc(&self) -> Project {
        self.sink.handle.snapshot().doc.clone()
    }

    fn register(&mut self, register: &MediaRegister) -> Result<MediaRegistered, CmdError> {
        self.run("media.register", register)
    }

    fn insert(&mut self, insert: &TimelineInsert) -> Result<TimelineInsertOutput, CmdError> {
        self.run("timeline.insert", insert)
    }
}

impl ImportSink {
    /// Runs `steps` in one batch transaction labelled `label`: committed when they succeed,
    /// cancelled (nothing changes) when they fail.
    fn in_tx<T>(
        &self,
        label: &str,
        steps: impl FnOnce(&mut OpenTx<'_>) -> Result<T, CmdError>,
    ) -> Result<T, CmdError> {
        let tx = self
            .handle
            .begin_tx(&self.ctx, TxKind::Batch, label)
            .wait()?
            .id;
        let done = steps(&mut OpenTx { sink: self, tx })
            .and_then(|out| self.handle.commit_tx(&self.ctx, tx).wait().map(|_| out));
        if done.is_err() {
            let _ = self.handle.cancel_tx(&self.ctx, tx).wait();
        }
        done
    }
}

impl MediaSink for ImportSink {
    fn register(&mut self, batch: Vec<MediaRegister>) -> Result<Vec<MediaRegistered>, CmdError> {
        self.register_all(&batch)
    }

    fn register_tree(&mut self, tree: MediaRegisterTree) -> Result<MediaTreeRegistered, CmdError> {
        self.register_folders(&tree)
    }

    fn slideshow(&mut self, batch: SlideshowBatch) -> Result<SlideshowPlaced, CmdError> {
        self.in_tx("Import slideshow", |tx| place_slideshow(tx, &batch))
    }

    fn register_frame(
        &mut self,
        register: MediaRegister,
        name: String,
    ) -> Result<FrameRegistered, CmdError> {
        let registered = self.in_tx("Add frame to media bin", |tx| {
            let registered: MediaRegistered = tx.run("media.register", &register)?;
            if registered.created {
                let _: serde_json::Value = tx.run(
                    "media.rename",
                    &serde_json::json!({ "media": registered.media_id, "name": name }),
                )?;
            }
            Ok(registered)
        })?;
        let snapshot = self.handle.snapshot();
        let media = snapshot
            .doc
            .media
            .get(&registered.media_id)
            .ok_or_else(|| CmdError::internal("the registered frame is not in the document"))?;
        Ok(FrameRegistered {
            registered,
            path: media.path_abs.clone().unwrap_or_default(),
            name: asset_name(media).into_owned(),
        })
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
