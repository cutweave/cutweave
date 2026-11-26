// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine's part of `media.import_frame` (WP-2F.05d, ADR-0035 item 4): [`EngineFrames`]
//! renders the frame the Preview or the Source viewer shows and writes it into the open
//! project's `generated/frames/` folder.
//!
//! - **What is rendered.** `sequence`: the sequence the editor shows, composited, at the
//!   sequence size. A video asset: the transient one-clip timeline the Source viewer plays
//!   ([`source_timeline`], ADR-0022), at the asset's own upright size. Both go through the
//!   renderer `render.frame` uses, at full quality from the original media, so the pixels are
//!   the ones `render.frame` gives at that time.
//! - **Where it goes.** `<project>/generated/frames/<media id>.png`. The id is new to the
//!   document and to the folder; the registration that follows names both, so replaying the
//!   journal writes nothing and gives the same document. With `--deterministic-ids` the id
//!   comes from a seeded stream of its own.
//! - **An untitled document** has no project folder: the call is `Invalid` and asks the person
//!   to save the project first.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};

use cutweave_commands::{CmdError, DocSnapshot};
use cutweave_model::{
    Duration, IdGen, MediaId, MediaKind, Project, SeededGen, SequenceId, Time, TimeInput, Uuid7Gen,
};
use cutweave_ops::media::{FrameGrabber, FrameOf, GrabbedFrame, asset_name, frame_label};
use cutweave_render::encode::encode_png;
use cutweave_render::query::{SharedBackend, render_full_frame};

use crate::full::preview::{SOURCE_SEQUENCE, active_sequence, source_info, source_timeline};

/// Mixed into the `--deterministic-ids` seed for the ids of imported frames: a stream separate
/// from the document's (which only a transaction may advance).
const FRAME_ID_STREAM: u64 = 0x4652_414D_4549_4453; // "FRAMEIDS"

/// The folder of a project the frames go into, relative to its root.
const FRAMES_DIR: [&str; 2] = ["generated", "frames"];

/// How many ids are tried before giving up on finding a free one.
const ID_ATTEMPTS: usize = 4096;

/// Renders and stores the frames of one `media.import_frame` call.
pub(crate) struct EngineFrames {
    /// The document of the moment (what the Preview shows).
    pub(crate) snapshot: Arc<DocSnapshot>,
    /// The renderer `render.frame` uses; `None` on an engine without one.
    pub(crate) backend: Option<SharedBackend>,
    /// The open project's folder; `None` for an untitled document.
    pub(crate) project_root: Option<PathBuf>,
    /// `--deterministic-ids`.
    pub(crate) seed: Option<u64>,
}

/// What one call renders.
struct Target<'a> {
    /// The document itself, or the transient timeline of a source.
    doc: Cow<'a, Project>,
    sequence: SequenceId,
    name: String,
    /// The length of a source (a frame must be inside it); `None` for the sequence.
    length: Option<Duration>,
}

impl EngineFrames {
    /// A media id that neither the document nor `dir` uses, and its file.
    fn fresh(&self, dir: &Path) -> Result<(MediaId, PathBuf), CmdError> {
        let mut seeded = self.seed.map(|seed| SeededGen::new(seed ^ FRAME_ID_STREAM));
        let mut random = Uuid7Gen;
        for _ in 0..ID_ATTEMPTS {
            let raw = match &mut seeded {
                Some(ids) => ids.next_u128(),
                None => random.next_u128(),
            };
            let id = MediaId::from_u128(raw);
            let path = dir.join(format!("{id}.png"));
            if !self.snapshot.doc.media.contains_key(&id) && !path.exists() {
                return Ok((id, path));
            }
        }
        Err(CmdError::internal(
            "no free name was found for the picture in the project's generated/frames folder",
        ))
    }

    /// Writes `png` into the frames folder of the project at `root`, named after a new media
    /// id.
    fn store(&self, root: &Path, png: &[u8]) -> Result<(MediaId, PathBuf), CmdError> {
        let dir = FRAMES_DIR
            .iter()
            .fold(root.to_path_buf(), |dir, part| dir.join(part));
        let io = |what: &str, e: &std::io::Error| {
            CmdError::internal(format!("{what} `{}`: {e}", dir.display()))
        };
        std::fs::create_dir_all(&dir).map_err(|e| io("cannot create", &e))?;
        let (media_id, path) = self.fresh(&dir)?;
        // Written under another name first, so a picture is never seen half written.
        let partial = path.with_extension("png.part");
        std::fs::write(&partial, png).map_err(|e| io("cannot write the picture into", &e))?;
        if let Err(e) = std::fs::rename(&partial, &path) {
            let _ = std::fs::remove_file(&partial);
            return Err(io("cannot write the picture into", &e));
        }
        Ok((media_id, path))
    }
}

/// `Invalid` at `/source/media` for what [`source_info`] refuses at `/media`.
fn source_pointer(error: CmdError) -> CmdError {
    match error {
        CmdError::Invalid { path, msg } if path == "/media" => {
            CmdError::invalid("/source/media", msg)
        }
        other => other,
    }
}

/// What `source` stands for in `doc`: the sequence the editor shows, or a video asset's own
/// timeline.
fn target(doc: &Project, source: FrameOf) -> Result<Target<'_>, CmdError> {
    match source {
        FrameOf::Sequence => {
            let sequence = active_sequence(doc)
                .and_then(|id| doc.sequences.get(&id))
                .ok_or_else(|| {
                    CmdError::invalid("/source", "the project has no sequence to take a frame of")
                })?;
            Ok(Target {
                doc: Cow::Borrowed(doc),
                sequence: sequence.id,
                name: sequence.name.clone(),
                length: None,
            })
        }
        FrameOf::Media(media) => {
            let info = source_info(doc, media).map_err(source_pointer)?;
            let asset = doc
                .media
                .get(&media)
                .filter(|asset| asset.kind == MediaKind::Video)
                .ok_or_else(|| {
                    CmdError::invalid(
                        "/source/media",
                        format!("media {media} has no picture: only a video has frames to take"),
                    )
                })?;
            let timeline = source_timeline(doc, media).ok_or_else(|| {
                CmdError::internal("the source has no timeline to render a frame of")
            })?;
            Ok(Target {
                doc: Cow::Owned(timeline),
                sequence: SOURCE_SEQUENCE,
                name: asset_name(asset).into_owned(),
                length: Some(info.duration),
            })
        }
    }
}

impl FrameGrabber for EngineFrames {
    fn grab(&self, source: FrameOf, at: TimeInput) -> Result<GrabbedFrame, CmdError> {
        let Some(root) = &self.project_root else {
            return Err(CmdError::invalid(
                "",
                "the project has never been saved, so it has no folder to keep the picture in; \
                 save the project first (project.save_as), then add the frame",
            ));
        };
        let target = target(&self.snapshot.doc, source)?;
        let rate = target
            .doc
            .sequences
            .get(&target.sequence)
            .map(|s| s.format.frame_rate)
            .ok_or_else(|| CmdError::internal("the frame's sequence is gone"))?;
        let time = at
            .resolve(Some(rate))
            .map_err(|e| CmdError::invalid("/at", e.to_string()))?;
        if time < Time::ZERO {
            return Err(CmdError::invalid("/at", "the time must not be negative"));
        }
        if target
            .length
            .is_some_and(|length| time.flicks() >= length.flicks())
        {
            return Err(CmdError::invalid(
                "/at",
                "the time is past the end of the media",
            ));
        }
        let Some(backend) = &self.backend else {
            return Err(CmdError::unsupported(
                "`media.import_frame` needs the engine's renderer; this engine has none",
            ));
        };
        let image = {
            let mut renderer = backend.lock().unwrap_or_else(PoisonError::into_inner);
            render_full_frame(&mut *renderer, &target.doc, target.sequence, time)?
        };
        let mut png = Vec::new();
        encode_png(&image, &mut png)
            .map_err(|e| CmdError::internal(format!("the frame cannot be encoded: {e}")))?;
        let (media_id, path) = self.store(root, &png)?;
        Ok(GrabbedFrame {
            media_id,
            path: path.to_string_lossy().into_owned(),
            width: image.width,
            height: image.height,
            time,
            label: frame_label(&target.name, time, rate),
        })
    }

    fn discard(&self, frame: &GrabbedFrame) {
        if let Err(e) = std::fs::remove_file(&frame.path) {
            tracing::warn!(path = %frame.path, error = %e, "cannot remove an unused frame picture");
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use cutweave_commands::Rev;

    fn frames(root: Option<PathBuf>, seed: Option<u64>) -> EngineFrames {
        EngineFrames {
            snapshot: Arc::new(DocSnapshot {
                rev: Rev::ZERO,
                committed_rev: Rev::ZERO,
                doc: Project::new("Frames"),
            }),
            backend: None,
            project_root: root,
            seed,
        }
    }

    #[test]
    fn an_untitled_document_is_asked_to_be_saved_first() {
        let error = frames(None, None)
            .grab(FrameOf::Sequence, TimeInput::from(Time::ZERO))
            .unwrap_err();
        let CmdError::Invalid { path, msg } = error else {
            panic!("expected Invalid, got {error:?}");
        };
        assert_eq!(path, "");
        assert!(msg.contains("save the project first"), "{msg}");
    }

    #[test]
    fn a_seeded_id_skips_files_that_exist() {
        let dir = std::env::temp_dir().join(format!("cutweave-frames-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = frames(Some(dir.clone()), Some(42));
        let (first, path) = store.fresh(&dir).unwrap();
        assert_eq!(path, dir.join(format!("{first}.png")));
        assert_eq!(
            store.fresh(&dir).unwrap().0,
            first,
            "the same seed, the same id"
        );
        std::fs::write(&path, b"taken").unwrap();
        let (second, _) = store.fresh(&dir).unwrap();
        assert_ne!(second, first);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
