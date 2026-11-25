// SPDX-License-Identifier: GPL-3.0-or-later
//! The `clip` namespace (WP-1B.06; arch §5.1): the media-clip specifics `clip.split` and
//! `clip.trim`. The edit semantics (frame grid, overlaps, locks, linked groups, no ripple) are
//! documented once in [`crate::item`].
//!
//! **Split.** The left half keeps the original item ID, the right half gets a new one. A linked
//! clip's partners that contain the split time are split too; the right halves then share a
//! *new* group, so the left pair and the right pair stay linked separately.
//!
//! **Trim.** Trimming the start by `d` moves `start` and `source_in` by `d` and shortens the
//! clip by `d`; trimming the end changes only the length. A clip never reads outside its media
//! (stills have no bounds) and never overlaps its neighbour (trim does not push it away).

use std::collections::BTreeSet;

use cutweave_commands::{CmdError, Command, QueryCtx, Registry, RegistryError, Tx, command, wire};
use cutweave_model::{
    Clip, EntityKind, GroupId, Item, ItemId, MediaAsset, MediaKind, Project, Time, TimeInput,
};

use crate::{
    item::{
        Located, conflict, duration, ensure_not_magnetic, ensure_orphans_unlockable,
        ensure_unlocked, members, time, unlink_orphans,
    },
    util::resolve_frame_time,
};

/// Registers the `clip.*` commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register::<ClipSplit>()?.register::<ClipTrim>()?;
    Ok(())
}

/// Whether the clip's source range is bounded by the media length (everything but stills and
/// media of unknown length).
fn media_bound(media: &MediaAsset) -> Option<i64> {
    media
        .probe
        .duration
        .filter(|_| media.kind != MediaKind::Image)
        .map(cutweave_model::Duration::flicks)
}

fn is_still(media: &MediaAsset) -> bool {
    media.kind == MediaKind::Image
}

fn clip_of(item: &Item) -> Result<&Clip, CmdError> {
    item.as_clip()
        .ok_or_else(|| CmdError::unsupported("only clips can be split or trimmed"))
}

fn media_of<'a>(doc: &'a Project, clip: &Clip) -> Result<&'a MediaAsset, CmdError> {
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
    /// The left half of the requested item (keeps its ID).
    pub left: ItemId,
    /// The right half of the requested item (new ID).
    pub right: ItemId,
    /// The time that was split at, snapped to the frame grid.
    pub at: Time,
    /// The linked partners that were split too.
    pub linked: Vec<SplitPair>,
    /// The new group shared by the right halves, if there were partners.
    pub right_group: Option<GroupId>,
}

/// Splits a clip into two at a timeline time that lies strictly inside it (`Invalid{path:"/at"}`
/// at or beyond the edges). The halves are contiguous: durations sum to the original and the
/// right half's `source_in` continues where the left half ends. Linked partners that contain
/// the time are split too. `at` is snapped to the sequence's frame grid. `Locked` if the clip's
/// track or a partner's track is locked.
#[command(
    name = "clip.split",
    summary = "Split a clip (and its linked partners) at a timeline time",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = false)
)]
pub struct ClipSplit {
    /// The clip.
    pub item: ItemId,
    /// Timeline time to split at: strictly inside the clip.
    pub at: TimeInput,
}

struct SplitPlan {
    at: Time,
    targets: Vec<Located>,
}

impl ClipSplit {
    fn plan(&self, doc: &Project) -> Result<SplitPlan, CmdError> {
        let (sequence, group) = members(doc, self.item, true)?;
        let rate = doc.sequences[&sequence].format.frame_rate;
        let at = resolve_frame_time("/at", self.at, rate)?;
        clip_of(&group[0].item)?;
        ensure_unlocked(doc, &group)?;
        let inside = |i: &Item| -> Result<bool, CmdError> {
            let end = i
                .end()
                .map_err(|e| CmdError::invalid("/at", e.to_string()))?;
            Ok(i.start() < at && at < end)
        };
        if !inside(&group[0].item)? {
            let end = group[0]
                .item
                .end()
                .map_err(|e| CmdError::invalid("/at", e.to_string()))?;
            return Err(CmdError::invalid(
                "/at",
                format!(
                    "the split time must be strictly inside the clip ({} .. {end})",
                    group[0].item.start()
                ),
            ));
        }
        let mut targets = vec![group[0].clone()];
        for partner in &group[1..] {
            if clip_of(&partner.item).is_ok() && inside(&partner.item)? {
                targets.push(partner.clone());
            }
        }
        Ok(SplitPlan { at, targets })
    }
}

impl Command for ClipSplit {
    type Output = ClipSplitOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ClipSplitOutput, CmdError> {
        let SplitPlan { at, targets } = self.plan(tx.doc())?;
        let right_group = (targets.len() > 1).then(|| tx.new_id::<GroupId>());
        let mut pairs = Vec::with_capacity(targets.len());
        for target in &targets {
            let clip = clip_of(&target.item)?;
            let still = is_still(media_of(tx.doc(), clip)?);
            let cut = at.flicks() - clip.start.flicks();
            let end = clip.start.flicks() + clip.duration.flicks();
            let left_len = duration("/at", cut)?;
            let right_len = duration("/at", end - at.flicks())?;
            let mut right = clip.clone();
            right.id = tx.new_id();
            right.start = at;
            right.duration = right_len;
            right.linked_group = right_group;
            // Item markers stay with the half that plays their time (item-relative).
            let (left_markers, right_markers) = split_markers(&clip.markers, cut);
            right.markers = right_markers;
            if !still {
                right.source_in = time("/at", clip.source_in.flicks() + cut)?;
            }
            let (left_id, right_id) = (clip.id, right.id);
            tx.update_item(left_id, |i| {
                i.set_duration(left_len);
                *i.markers_mut() = left_markers;
                Ok(())
            })?;
            tx.insert_item(target.track, &Item::Clip(right))?;
            pairs.push(SplitPair {
                left: left_id,
                right: right_id,
            });
        }
        let first = pairs.remove(0);
        Ok(ClipSplitOutput {
            left: first.left,
            right: first.right,
            at,
            linked: pairs,
            right_group,
        })
    }
}

/// Splits item markers at the item-relative time `cut` (flicks): markers before it stay, the
/// others go to the right half re-based to its start. A range marker belongs to the half it
/// starts in.
fn split_markers(
    markers: &cutweave_model::Markers,
    cut: i64,
) -> (cutweave_model::Markers, cutweave_model::Markers) {
    let mut left = cutweave_model::Markers::new();
    let mut right = cutweave_model::Markers::new();
    for m in markers.values() {
        if m.t.flicks() < cut {
            left.insert(m.id, m.clone());
        } else {
            let mut moved = m.clone();
            moved.t = Time::from_flicks(m.t.flicks() - cut).unwrap_or(Time::ZERO);
            right.insert(moved.id, moved);
        }
    }
    (left, right)
}

// ----- clip.trim -----------------------------------------------------------------------------

/// Which edge of a clip to trim.
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrimEdge {
    /// The first frame (changes `start`, `source_in` and the length).
    Start,
    /// The last frame (changes the length).
    End,
}

/// Result of `clip.trim`.
#[wire]
pub struct ClipTrimOutput {
    /// The trimmed clips: the requested one first, then its linked partners.
    pub items: Vec<TrimmedClip>,
}

/// A clip after trimming.
#[wire]
pub struct TrimmedClip {
    /// The clip.
    pub item: ItemId,
    /// Its start.
    pub start: Time,
    /// Its length.
    pub duration: cutweave_model::Duration,
    /// Its source position.
    pub source_in: Time,
}

/// Moves one edge of a clip to a timeline time (`to`) or by an offset (`delta`, negative to
/// move the edge earlier; give exactly one). The clip cannot become empty, extend past the
/// start or end of its media, or overlap a neighbour: `Invalid{path:"/to"}` (or `/delta`).
/// Linked partners get the same edge at the same time; `unlink: true` trims only this clip and
/// removes it from its group. Times are snapped to the sequence's frame grid. `Locked` if a
/// track involved is locked.
#[command(
    name = "clip.trim",
    summary = "Trim the start or end edge of a clip (linked partners follow; no ripple)",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = true)
)]
pub struct ClipTrim {
    /// The clip.
    pub item: ItemId,
    /// The edge to move.
    pub edge: TrimEdge,
    /// Timeline time of the edge afterwards (frame-snapped).
    pub to: Option<TimeInput>,
    /// Offset of the edge from where it is now (negative: earlier). Use seconds or flicks for
    /// negative values.
    pub delta: Option<TimeInput>,
    /// Trim only this clip and remove it from its linked group (default `false`).
    pub unlink: Option<bool>,
}

/// The clip with its `edge` moved to the timeline time `t`.
fn trimmed(
    clip: &Clip,
    media: &MediaAsset,
    edge: TrimEdge,
    t: Time,
    pointer: &str,
) -> Result<Clip, CmdError> {
    let (start, len) = (clip.start.flicks(), clip.duration.flicks());
    let mut out = clip.clone();
    let who = format!("clip `{}`", clip.id);
    match edge {
        TrimEdge::Start => {
            let d = t.flicks() - start;
            if d >= len {
                return Err(CmdError::invalid(
                    pointer,
                    format!("{who} would become empty: the start must stay before its end"),
                ));
            }
            if t < Time::ZERO {
                return Err(CmdError::invalid(
                    pointer,
                    format!("{who} would start before zero"),
                ));
            }
            out.start = t;
            out.duration = duration(pointer, len - d)?;
            if !is_still(media) {
                let src = clip.source_in.flicks() + d;
                if src < 0 {
                    return Err(CmdError::invalid(
                        pointer,
                        format!("{who} cannot start before the beginning of its media"),
                    ));
                }
                out.source_in = time(pointer, src)?;
            }
        }
        TrimEdge::End => {
            let new_len = t.flicks() - start;
            if new_len <= 0 {
                return Err(CmdError::invalid(
                    pointer,
                    format!("{who} would become empty: the end must stay after its start"),
                ));
            }
            if let Some(total) = media_bound(media)
                && i128::from(clip.source_in.flicks()) + i128::from(new_len) > i128::from(total)
            {
                return Err(CmdError::invalid(
                    pointer,
                    format!("{who} would read past the end of its media"),
                ));
            }
            out.duration = duration(pointer, new_len)?;
        }
    }
    Ok(out)
}

struct TrimPlan {
    /// `(track, after)` for the requested clip first, then partners.
    clips: Vec<(Located, Clip)>,
    unlink: bool,
    /// The group the requested clip leaves under `unlink: true`.
    leaves: Option<GroupId>,
}

impl ClipTrim {
    fn plan(&self, doc: &Project) -> Result<TrimPlan, CmdError> {
        let unlink = self.unlink.unwrap_or(false);
        let (sequence, group) = members(doc, self.item, !unlink)?;
        let rate = doc.sequences[&sequence].format.frame_rate;
        let primary = clip_of(&group[0].item)?;
        let edge_time = |c: &Clip| match self.edge {
            TrimEdge::Start => c.start.flicks(),
            TrimEdge::End => c.start.flicks() + c.duration.flicks(),
        };
        let (pointer, t) = match (self.to, self.delta) {
            (Some(to), None) => ("/to", resolve_frame_time("/to", to, rate)?),
            (None, Some(delta)) => {
                let d = resolve_frame_time("/delta", delta, rate)?;
                ("/delta", time("/delta", edge_time(primary) + d.flicks())?)
            }
            _ => {
                return Err(CmdError::invalid(
                    "",
                    "give exactly one of `to` (edge time) or `delta` (edge offset)",
                ));
            }
        };
        ensure_not_magnetic(doc, &group)?;
        ensure_unlocked(doc, &group)?;
        if unlink {
            let first = &group[0].item;
            ensure_orphans_unlockable(doc, first.linked_group(), first.id())?;
        }
        let excluded: BTreeSet<ItemId> = group.iter().map(|m| m.item.id()).collect();
        let mut clips = Vec::new();
        for m in group {
            let before = clip_of(&m.item)?;
            let media = media_of(doc, before)?;
            let after = trimmed(before, media, self.edge, t, pointer)?;
            let track = doc
                .sequences
                .values()
                .find_map(|s| s.track(&m.track))
                .ok_or_else(|| CmdError::not_found(EntityKind::Track, m.track))?;
            let end = Item::Clip(after.clone())
                .end()
                .map_err(|e| CmdError::invalid(pointer, e.to_string()))?;
            if let Some(other) = conflict(track, after.start, end, &excluded) {
                return Err(CmdError::invalid(
                    pointer,
                    format!(
                        "clip `{}` would overlap item `{other}` on track `{}`",
                        after.id, m.track
                    ),
                ));
            }
            clips.push((m, after));
        }
        let leaves = if unlink {
            clips.first().and_then(|(m, _)| m.item.linked_group())
        } else {
            None
        };
        Ok(TrimPlan {
            clips,
            unlink,
            leaves,
        })
    }
}

impl Command for ClipTrim {
    type Output = ClipTrimOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ClipTrimOutput, CmdError> {
        let plan = self.plan(tx.doc())?;
        let mut items = Vec::with_capacity(plan.clips.len());
        for (i, (_, after)) in plan.clips.iter().enumerate() {
            let unlink = plan.unlink && i == 0;
            tx.update_item(after.id, |item| {
                if let Some(c) = item.as_clip_mut() {
                    c.start = after.start;
                    c.duration = after.duration;
                    c.source_in = after.source_in;
                    if unlink {
                        c.linked_group = None;
                    }
                }
                Ok(())
            })?;
            items.push(TrimmedClip {
                item: after.id,
                start: after.start,
                duration: after.duration,
                source_in: after.source_in,
            });
        }
        unlink_orphans(tx, plan.leaves)?;
        Ok(ClipTrimOutput { items })
    }
}
