// SPDX-License-Identifier: GPL-3.0-or-later
//! The `clip` namespace (WP-1B.06, WP-2A.03, WP-2A.04, WP-2D.01; arch §5.1; timeline spec §4.7,
//! §4.9–§4.13, INV-47): the media-clip specifics `clip.split`, the trim family in [`trim`]
//! (`clip.trim`, `clip.ripple_trim`, `clip.roll`, `clip.slip`, `clip.slide`) and speed in
//! [`speed`] (`clip.set_speed`, `clip.reverse`, `clip.reset_speed`, `clip.freeze_frame`). The edit semantics
//! (frame grid, overlaps, locks, linked groups, the magnetic track) are documented once in
//! [`crate::item`].
//!
//! **Split** (on the placement core). The left half keeps the original item ID, the right half
//! gets a new one. A linked clip's partners are split too; the right halves then share a *new*
//! group, so the left pair and the right pair stay linked separately. Children attached to the
//! clip at or after the split time move to the right half (INV-39); item markers too. Nothing
//! ripples. IDs (INV-23): per given clip, in order, the right halves' group (when linked), then
//! the right halves in member order (the clip, then its partners by track ID and item ID).

pub mod speed;
pub mod trim;

use std::collections::BTreeSet;

use cutweave_commands::{CmdError, Command, QueryCtx, Registry, RegistryError, Tx, command, wire};
use cutweave_model::{
    Clip, EntityKind, GroupId, Item, ItemId, MediaAsset, MediaKind, Project, Time, TimeInput,
    TrackKind,
};

pub use speed::{
    ClipFreezeFrame, ClipFreezeFrameOutput, ClipResetSpeed, ClipReverse, ClipSetSpeed,
    ClipSpeedOutput, KeyframeTiming, SpeedClip, SpeedInput, SpeedKeep,
};
pub use trim::{
    ClipRippleTrim, ClipRoll, ClipSlide, ClipSlip, ClipSpan, ClipTrim, ClipTrimOutput, Speed,
    TrimEdge, TrimOutput, TrimmedClip,
};

use crate::{
    report::EditReport,
    timeline::{
        edit::given_items,
        placement::{DryIds, Edit, Ids, Plan, span},
    },
    track::check_editable_kind,
    util::{resolve_frame_time, resolve_sample_time},
};

/// Registers the `clip.*` commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<ClipSplit>()?
        .register::<ClipTrim>()?
        .register::<ClipRippleTrim>()?
        .register::<ClipRoll>()?
        .register::<ClipSlip>()?
        .register::<ClipSlide>()?
        .register::<ClipSetSpeed>()?
        .register::<ClipReverse>()?
        .register::<ClipResetSpeed>()?
        .register::<ClipFreezeFrame>()?;
    Ok(())
}

/// Whether the clip's source range is bounded by the media length (everything but stills and
/// media of unknown length).
pub(crate) fn media_bound(media: &MediaAsset) -> Option<i64> {
    media
        .probe
        .duration
        .filter(|_| media.kind != MediaKind::Image)
        .map(cutweave_model::Duration::flicks)
}

pub(crate) fn is_still(media: &MediaAsset) -> bool {
    media.kind == MediaKind::Image
}

pub(crate) fn clip_of(item: &Item) -> Result<&Clip, CmdError> {
    item.as_clip()
        .ok_or_else(|| CmdError::unsupported("only clips can be split or trimmed"))
}

pub(crate) fn media_of<'a>(doc: &'a Project, clip: &Clip) -> Result<&'a MediaAsset, CmdError> {
    doc.media
        .get(&clip.media)
        .ok_or_else(|| CmdError::not_found(EntityKind::Media, clip.media))
}

// ----- clip.split ----------------------------------------------------------------------------

/// One split item.
#[wire]
pub struct SplitPair {
    /// The left half (keeps the original item ID).
    pub left: ItemId,
    /// The right half (new ID).
    pub right: ItemId,
}

/// Result of `clip.split`.
#[wire]
pub struct ClipSplitOutput {
    /// The left half of the (first) requested clip (keeps its ID).
    pub left: ItemId,
    /// The right half of the (first) requested clip (new ID).
    pub right: ItemId,
    /// The time that was split at, snapped to the frame grid (or the sample grid, `fine`).
    pub at: Time,
    /// The other clips that were split: the first clip's linked partners, then the other
    /// requested clips each followed by its partners.
    pub linked: Vec<SplitPair>,
    /// The new group shared by the first clip's right halves, if it is linked.
    pub right_group: Option<GroupId>,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Splits a clip (`item`), or several (`items`), into two at a timeline time that lies strictly
/// inside each (`Invalid{path:"/at"}` at or beyond the edges). The halves are contiguous:
/// durations sum to the original and the right half's `source_in` continues where the left
/// half ends. Linked partners are split too, and children attached at or after the time move
/// to the right half. `at` is snapped to the sequence's frame grid (`fine: true`: the audio
/// sample grid, for audio clips not linked to video). `Locked` if a track involved is locked.
#[command(
    name = "clip.split",
    summary = "Split a clip (and its linked partners) at a timeline time",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = false)
)]
pub struct ClipSplit {
    /// The clip (or `items`).
    pub item: Option<ItemId>,
    /// Clips to split at once.
    pub items: Option<Vec<ItemId>>,
    /// Timeline time to split at: strictly inside the clip.
    pub at: TimeInput,
    /// Audio sample grid.
    pub fine: Option<bool>,
}

struct SplitPlan {
    plan: Plan,
    at: Time,
    pairs: Vec<(ItemId, ItemId)>,
}

fn plan_split(cmd: &ClipSplit, doc: &Project, ids: &mut dyn Ids) -> Result<SplitPlan, CmdError> {
    let given = match (cmd.item, cmd.items.as_deref()) {
        (Some(i), None) => vec![i],
        (None, Some([])) => return Err(CmdError::invalid("/items", "give at least one clip")),
        (None, Some(list)) => list.to_vec(),
        _ => {
            return Err(CmdError::invalid(
                "",
                "give exactly one of `item` or `items`",
            ));
        }
    };
    let (sequence, given) = given_items(doc, &given)?;
    let seq = &doc.sequences[&sequence];
    let mut e = Edit::new(doc, sequence, "/at")?;
    for id in &given {
        for (t, m) in e.members(*id) {
            if let Some(track) = e.seq.track(&t) {
                check_editable_kind("/item", track.kind)?;
            }
            if let Some((_, item)) = e.locate(m) {
                clip_of(item)?;
            }
        }
    }
    let rate = seq.format.frame_rate;
    let at = if cmd.fine.unwrap_or(false) {
        let audio_only = given
            .iter()
            .flat_map(|id| e.members(*id))
            .all(|(t, _)| e.seq.track(&t).is_some_and(|t| t.kind == TrackKind::Audio));
        if !audio_only {
            return Err(CmdError::invalid(
                "/fine",
                "a fine edit needs audio clips that are not linked to video",
            ));
        }
        resolve_sample_time("/at", cmd.at, rate, seq.format.sample_rate)?
    } else {
        resolve_frame_time("/at", cmd.at, rate)?
    };
    let t = at.flicks();
    for id in &given {
        let Some((_, item)) = e.locate(*id) else {
            continue;
        };
        let (s, end) = span(item);
        if !(s < t && t < end) {
            return Err(CmdError::invalid(
                "/at",
                format!(
                    "the split time must be strictly inside clip `{id}` ({} .. {end})",
                    item.start()
                ),
            ));
        }
    }
    let mut pairs = Vec::new();
    let mut done = BTreeSet::new();
    for id in &given {
        if done.contains(id) {
            continue;
        }
        let split = e.split(ids, *id, t);
        done.extend(split.iter().map(|(l, _)| *l));
        pairs.extend(split);
    }
    Ok(SplitPlan {
        plan: e.finish(ids)?,
        at,
        pairs,
    })
}

impl Command for ClipSplit {
    type Output = ClipSplitOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_split(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ClipSplitOutput, CmdError> {
        let doc = tx.doc().clone();
        let SplitPlan { plan, at, pairs } = plan_split(self, &doc, tx)?;
        let report = plan.commit(tx)?;
        let mut pairs = pairs.into_iter();
        let (left, right) = pairs
            .next()
            .ok_or_else(|| CmdError::internal("clip.split split nothing"))?;
        let right_group = plan
            .after
            .find_item(&right)
            .and_then(|(_, i)| i.linked_group());
        Ok(ClipSplitOutput {
            left,
            right,
            at,
            linked: pairs
                .map(|(left, right)| SplitPair { left, right })
                .collect(),
            right_group,
            report,
        })
    }
}
