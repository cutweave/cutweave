// SPDX-License-Identifier: GPL-3.0-or-later
//! `media.import_frame` (WP-2F.05d, ADR-0035 item 4): the frame the Preview or the Source
//! viewer shows, saved as a PNG in the project's `generated/frames/` folder and registered as
//! a picture in the bin.
//!
//! The command itself holds no renderer and writes no file: the engine's [`FrameGrabber`]
//! renders the frame at the full sequence or source size (the pixels `render.frame` gives at
//! that time), picks a new media id, and writes `<media id>.png`. The command then reads the
//! file back through the prober (so the asset is exactly what importing that file would give),
//! registers it under that id and gives it its bin name, in one transaction
//! ([`MediaSink::register_frame`](super::MediaSink::register_frame)): one undo step removes the
//! asset, and the file stays. Because the registration names the id and the path, replaying it
//! writes nothing and gives the same document.
//!
//! When the same picture is already in the project (the content hash decides), that asset is
//! returned as a `duplicate` and the file just written is removed again.

use cutweave_commands::{CmdError, Command, EngineCommand, Tx, command, wire};
use cutweave_jobs::JobId;
use cutweave_model::{
    FolderId, FrameRate, MediaId, MediaKind, Rounding, Time, TimeInput, Timecode,
};

use super::{
    ImportCtx, MediaRegister, MediaRegistered, MediaRename, asset_name, policy,
    slideshow::announce,
};
use crate::util::check_name;

/// Which picture `media.import_frame` saves.
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrameOf {
    /// The frame the Preview shows: the sequence the editor shows, composited.
    Sequence,
    /// The frame the Source viewer shows for this video asset, at its own size.
    Media(MediaId),
}

/// A frame the engine rendered and wrote to disk for `media.import_frame`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrabbedFrame {
    /// The id chosen for the new asset (the file is named after it).
    pub media_id: MediaId,
    /// Absolute path of the PNG.
    pub path: String,
    /// Picture width in px.
    pub width: u32,
    /// Picture height in px.
    pub height: u32,
    /// The rendered time: of the sequence, or of the source.
    pub time: Time,
    /// The bin name the asset gets when the caller gives none (the source's name and the
    /// timecode).
    pub label: String,
}

/// The bin name an imported frame gets when the caller gives none: the name of what was
/// rendered and the timecode of `t` at `rate` ("Sequence 1 00:00:02:12").
#[must_use]
pub fn frame_label(source_name: &str, t: Time, rate: FrameRate) -> String {
    let frame = rate.time_to_frame(t, Rounding::Floor).max(0);
    let drop_frame = rate.drop_frames_per_minute().is_some();
    let source: String = source_name.trim().chars().take(200).collect();
    match Timecode::from_frame(frame, rate, drop_frame) {
        Ok(timecode) if source.is_empty() => timecode.to_string(),
        Ok(timecode) => format!("{source} {timecode}"),
        Err(_) => format!("{source} f:{frame}").trim().to_owned(),
    }
}

/// Renders and stores the frames of `media.import_frame`: the engine implements it over its
/// renderer and the open project's folder.
pub trait FrameGrabber: Send + Sync {
    /// Renders the frame `source` shows at `at` at its full size and writes it as a PNG named
    /// after a media id the document does not use yet.
    ///
    /// # Errors
    /// `Invalid` when the document has no project folder yet (it was never saved), when `at`
    /// cannot be read, or when `source` has no picture; `NotFound` for an unknown asset;
    /// `Unsupported` without a renderer.
    fn grab(&self, source: FrameOf, at: TimeInput) -> Result<GrabbedFrame, CmdError>;

    /// Removes the file of `frame` again: no asset uses it (the registration failed, or the
    /// same picture was already in the project).
    fn discard(&self, frame: &GrabbedFrame);
}

/// Result of `media.import_frame`.
#[wire]
pub struct MediaImportFrameOutput {
    /// The picture's asset; the existing one when the same picture was already in the project.
    pub media_id: MediaId,
    /// Absolute path of the PNG the asset points at.
    pub path: String,
    /// The name the bin shows for it.
    pub name: String,
    /// Picture width in px.
    pub width: u32,
    /// Picture height in px.
    pub height: u32,
    /// The time that was rendered: of the sequence, or of the source.
    pub time: Time,
    /// `true` when the same picture was already in the project and nothing was added.
    pub duplicate: bool,
    /// Background analysis jobs queued for the new asset (wait with `job.wait`).
    pub job_ids: Vec<JobId>,
}

/// Saves the frame the Preview shows (`source: "sequence"`: the sequence the editor shows) or
/// the frame of a video asset (`source: {"media": id}`) at time `at` as a PNG picture in the
/// bin. The picture has the full sequence or source size and the same pixels as `render.frame`
/// at that time. The file goes into the project's `generated/frames/` folder, named after the
/// new asset; `name` is the name the bin shows (default: the source's name and the timecode)
/// and `folder` the bin folder. One undo step removes the asset; the file stays. Fails with
/// `Invalid` while the project has never been saved (it has no folder yet: save it first),
/// and with `Unsupported` on an engine without a renderer.
#[command(
    name = "media.import_frame",
    summary = "Save the frame the Preview or a video asset shows at a time as a picture in the bin",
    engine,
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct MediaImportFrame {
    /// `"sequence"` (the Preview) or `{"media": id}` (a video asset on its own).
    pub source: FrameOf,
    /// The time of the frame: flicks, "2.5s", "f:60" or a timecode.
    pub at: TimeInput,
    /// The name the bin shows (default: the source's name and the timecode).
    pub name: Option<String>,
    /// Bin folder (default: top level).
    pub folder: Option<FolderId>,
}

/// What [`MediaSink::register_frame`](super::MediaSink::register_frame) did.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameRegistered {
    /// The registration: the new asset, or the existing one with the same content.
    pub registered: MediaRegistered,
    /// The file that asset points at.
    pub path: String,
    /// The name the bin shows for it.
    pub name: String,
}

/// Registers the picture of an imported frame and gives it its bin name, for
/// [`MediaSink::register_frame`](super::MediaSink::register_frame) implementations that hold a
/// [`Tx`]. An asset that already existed (the same content) keeps its name and its file.
pub fn register_frame(
    tx: &mut Tx<'_>,
    register: &MediaRegister,
    name: &str,
) -> Result<FrameRegistered, CmdError> {
    register.validate(&tx.query_ctx())?;
    let registered = register.apply(tx)?;
    if registered.created {
        let rename = MediaRename {
            media: registered.media_id,
            name: Some(name.to_owned()),
        };
        rename.validate(&tx.query_ctx())?;
        rename.apply(tx)?;
    }
    let media = tx.media(registered.media_id)?;
    Ok(FrameRegistered {
        registered,
        path: media.path_abs.clone().unwrap_or_default(),
        name: asset_name(media).into_owned(),
    })
}

impl MediaImportFrame {
    /// Reads the written picture back and registers it; the caller discards the file when
    /// this fails.
    fn register(
        &self,
        ctx: &mut ImportCtx,
        frame: &GrabbedFrame,
        name: &str,
    ) -> Result<(MediaRegister, FrameRegistered), CmdError> {
        let probed = policy::probe_all(
            &ctx.prober,
            std::slice::from_ref(&frame.path),
            &ctx.policy,
            &mut || {},
        )
        .pop()
        .ok_or_else(|| CmdError::internal("the prober returned no result"))?
        .map_err(|e| CmdError::internal(format!("the saved frame cannot be read back: {e}")))?;
        let register = MediaRegister {
            media_id: Some(frame.media_id),
            path: frame.path.clone(),
            probe: probed.probe,
            content_hash: probed.content_hash,
            folder: self.folder,
        };
        if register.kind()? != MediaKind::Image {
            return Err(CmdError::internal(
                "the saved frame was not read back as a still picture",
            ));
        }
        let registered = ctx
            .sink
            .register_frame(register.clone(), name.to_owned())?;
        Ok((register, registered))
    }
}

impl EngineCommand for MediaImportFrame {
    type Context = ImportCtx;
    type Output = MediaImportFrameOutput;

    fn run(&self, ctx: &mut ImportCtx) -> Result<MediaImportFrameOutput, CmdError> {
        let given = self
            .name
            .as_deref()
            .map(|name| check_name("/name", name))
            .transpose()?;
        let Some(frames) = ctx.frames.clone() else {
            return Err(CmdError::unsupported(
                "`media.import_frame` needs the engine's renderer; this engine has none",
            ));
        };
        let frame = frames.grab(self.source, self.at)?;
        let name = given.unwrap_or_else(|| frame.label.clone());
        let (register, done) = match self.register(ctx, &frame, &name) {
            Ok(done) => done,
            Err(e) => {
                frames.discard(&frame);
                return Err(e);
            }
        };
        if !done.registered.created {
            // The same picture is in the project already: the bin keeps one, and its file.
            frames.discard(&frame);
        }
        let mut job_ids = Vec::new();
        let imported = announce(ctx, register, done.registered, &mut job_ids);
        Ok(MediaImportFrameOutput {
            media_id: imported.media_id,
            path: done.path,
            name: done.name,
            width: frame.width,
            height: frame.height,
            time: frame.time,
            duplicate: imported.duplicate,
            job_ids,
        })
    }
}
