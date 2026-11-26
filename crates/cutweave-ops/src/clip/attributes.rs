// SPDX-License-Identifier: GPL-3.0-or-later
//! `clip.paste_attributes` (WP-2A.06): copies groups of properties from one clip onto others,
//! keys and all. Effects do not exist yet, so there is no `effects` attribute; it will be
//! added with them.

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{Clip, EntityKind, ItemId, Project};

use crate::keyframe;

/// A group of properties, as `item.reset_props` and the Inspector name them.
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Attribute {
    /// Position, scale, rotation and anchor.
    Transform,
    /// The four crop edges.
    Crop,
    /// Flip horizontal and vertical.
    Flip,
    /// Opacity and blend mode.
    Compositing,
    /// Clip gain, pan and fades.
    Audio,
}

/// Result of `clip.paste_attributes`.
#[wire]
pub struct ClipPasteAttributesOutput {
    /// The clips whose properties changed (those that already matched are left out).
    pub changed: Vec<ItemId>,
}

/// Copies the chosen property groups (`transform`, `crop`, `flip`, `compositing`, `audio`) of
/// `from` onto each clip of `to`, with their keyframes. Names, timing, media and links are not
/// touched. `Locked` if a target's track is locked; nothing changes then.
#[command(
    name = "clip.paste_attributes",
    summary = "Copy transform, crop, flip, compositing or audio settings of a clip onto other clips",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ClipPasteAttributes {
    /// The clip to read the values from.
    pub from: ItemId,
    /// The clips to change, at least one, of one sequence.
    pub to: Vec<ItemId>,
    /// The groups to copy, at least one.
    pub which: Vec<Attribute>,
}

fn apply_to(clip: &mut Clip, from: &Clip, which: &[Attribute]) {
    for a in which {
        match a {
            Attribute::Transform => clip.visual.transform = from.visual.transform.clone(),
            Attribute::Crop => clip.visual.crop = from.visual.crop.clone(),
            Attribute::Flip => clip.visual.flip = from.visual.flip.clone(),
            Attribute::Compositing => {
                clip.visual.opacity = from.visual.opacity.clone();
                clip.visual.blend_mode = from.visual.blend_mode;
            }
            Attribute::Audio => clip.audio = from.audio.clone(),
        }
    }
}

impl ClipPasteAttributes {
    fn source<'a>(&self, doc: &'a Project) -> Result<&'a Clip, CmdError> {
        if self.which.is_empty() {
            return Err(CmdError::invalid("/which", "give at least one group"));
        }
        if self.to.contains(&self.from) {
            return Err(CmdError::invalid(
                "/to",
                "the source clip cannot be one of the targets",
            ));
        }
        doc.find_item(&self.from)
            .ok_or_else(|| CmdError::not_found(EntityKind::Item, self.from))?
            .item
            .as_clip()
            .ok_or_else(|| CmdError::unsupported("only clips have these properties"))
    }
}

impl Command for ClipPasteAttributes {
    type Output = ClipPasteAttributesOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.source(ctx.doc())?;
        keyframe::targets(ctx.doc(), None, Some(&self.to)).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ClipPasteAttributesOutput, CmdError> {
        let from = self.source(tx.doc())?.clone();
        let targets = keyframe::targets(tx.doc(), None, Some(&self.to))?;
        let mut changed = Vec::new();
        for t in &targets {
            let did = tx.update_item(t.id, |item| {
                let clip = item
                    .as_clip_mut()
                    .ok_or_else(|| CmdError::unsupported("only clips have these properties"))?;
                let before = clip.clone();
                apply_to(clip, &from, &self.which);
                Ok(*clip != before)
            })?;
            if did {
                changed.push(t.id);
            }
        }
        Ok(ClipPasteAttributesOutput { changed })
    }
}
