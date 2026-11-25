// SPDX-License-Identifier: GPL-3.0-or-later
//! `item.reset_props` (WP-2C.03, keyframes §9.9; the Inspector's section and row resets and the
//! clip context menu): properties go back to their defaults, keys removed, in one undo step.

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{BlendMode, Clip, ItemId, Project, PropId, PropValue};

use crate::keyframe::{self, KeyValue};

/// A group of properties, as the Inspector's sections show them.
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResetGroup {
    /// Position, scale, rotation and anchor.
    Transform,
    /// The four crop edges.
    Crop,
    /// Flip horizontal and vertical.
    Flip,
    /// Opacity and blend mode.
    Compositing,
    /// Clip gain and pan.
    Audio,
}

impl ResetGroup {
    /// The animatable properties of the group.
    fn props(self) -> &'static [PropId] {
        match self {
            ResetGroup::Transform => &[
                PropId::TransformPosition,
                PropId::TransformScale,
                PropId::TransformRotation,
                PropId::TransformAnchor,
            ],
            ResetGroup::Crop => &[
                PropId::CropLeft,
                PropId::CropTop,
                PropId::CropRight,
                PropId::CropBottom,
            ],
            ResetGroup::Flip => &[PropId::FlipHorizontal, PropId::FlipVertical],
            ResetGroup::Compositing => &[PropId::Opacity],
            ResetGroup::Audio => &[PropId::AudioGain, PropId::AudioPan],
        }
    }
}

/// Result of `item.reset_props`.
#[wire]
pub struct ItemResetPropsOutput {
    /// The first item.
    pub item: ItemId,
    /// The properties whose value changed on any item (`opacity`, `crop.left`, `blend_mode`,
    /// ...); empty when everything already had its default.
    pub changed: Vec<String>,
}

/// Resets properties to their defaults: they become static (any keys are removed). Give
/// `group` (`transform`, `crop`, `flip`, `compositing`, `audio`) or `props` (property IDs), not
/// both; with neither, every animatable property and the blend mode are reset. The name and
/// enabled flag are never touched. `Locked` if the item's track is locked.
#[command(
    name = "item.reset_props",
    summary = "Reset an item's properties (a group, some properties, or all) to their defaults",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = true)
)]
pub struct ItemResetProps {
    /// The item (or `items`).
    pub item: Option<ItemId>,
    /// Items to reset.
    pub items: Option<Vec<ItemId>>,
    /// A section of the Inspector.
    pub group: Option<ResetGroup>,
    /// Property IDs, as `keyframe.set` takes them.
    pub props: Option<Vec<PropId>>,
}

fn default_value(id: PropId) -> KeyValue {
    match id.spec().default {
        PropValue::Scalar(v) => KeyValue::Number(v),
        PropValue::Vec2(v) => KeyValue::Vec2(v),
        PropValue::Bool(b) => KeyValue::Bool(b),
    }
}

impl ItemResetProps {
    /// The properties to reset and whether the blend mode is among them.
    fn what(&self) -> Result<(Vec<PropId>, bool), CmdError> {
        match (self.group, &self.props) {
            (Some(_), Some(_)) => Err(CmdError::invalid(
                "/props",
                "give either `group` or `props`, not both",
            )),
            (Some(g), None) => Ok((g.props().to_vec(), g == ResetGroup::Compositing)),
            (None, Some(list)) if list.is_empty() => {
                Err(CmdError::invalid("/props", "give at least one property"))
            }
            (None, Some(list)) => Ok((list.clone(), false)),
            (None, None) => Ok((PropId::ALL.to_vec(), true)),
        }
    }

    fn reset(&self, clip: &mut Clip, what: &(Vec<PropId>, bool)) -> Vec<String> {
        let mut changed = Vec::new();
        for id in &what.0 {
            if keyframe::set_static(clip, *id, &default_value(*id)) {
                changed.push(id.to_string());
            }
        }
        if what.1 && clip.visual.blend_mode != BlendMode::Normal {
            clip.visual.blend_mode = BlendMode::Normal;
            changed.push("blend_mode".into());
        }
        changed
    }

    fn clip_of(doc: &Project, id: ItemId) -> Result<&Clip, CmdError> {
        doc.find_item(&id)
            .and_then(|at| at.item.as_clip())
            .ok_or_else(|| CmdError::unsupported("only clips have these properties"))
    }
}

impl Command for ItemResetProps {
    type Output = ItemResetPropsOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.what()?;
        for t in keyframe::targets(ctx.doc(), self.item, self.items.as_deref())? {
            ItemResetProps::clip_of(ctx.doc(), t.id)?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemResetPropsOutput, CmdError> {
        let what = self.what()?;
        let targets = keyframe::targets(tx.doc(), self.item, self.items.as_deref())?;
        let mut changed: Vec<String> = Vec::new();
        for t in &targets {
            let done = tx.update_item(t.id, |item| {
                let clip = item
                    .as_clip_mut()
                    .ok_or_else(|| CmdError::unsupported("only clips have these properties"))?;
                Ok(self.reset(clip, &what))
            })?;
            for c in done {
                if !changed.contains(&c) {
                    changed.push(c);
                }
            }
        }
        Ok(ItemResetPropsOutput {
            item: targets[0].id,
            changed,
        })
    }
}
