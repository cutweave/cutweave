// SPDX-License-Identifier: GPL-3.0-or-later
//! The `clip` namespace (WP-1B.06, WP-2A.03; arch §5.1; timeline spec §4.7, §4.9): the
//! media-clip specifics `clip.split` and `clip.trim`. The edit semantics (frame grid, overlaps,
//! locks, linked groups, the magnetic track) are documented once in [`crate::item`].
//!
//! **Split** (on the placement core). The left half keeps the original item ID, the right half
//! gets a new one. A linked clip's partners are split too; the right halves then share a *new*
//! group, so the left pair and the right pair stay linked separately. Children attached to the
//! clip at or after the split time move to the right half (INV-39); item markers too. Nothing
//! ripples. IDs (INV-23): per given clip, in order, the right halves' group (when linked), then
//! the right halves in member order (the clip, then its partners by track ID and item ID).
//!
//! **Trim.** Trimming the start by `d` moves `start` and `source_in` by `d` and shortens the
//! clip by `d`; trimming the end changes only the length. A clip never reads outside its media
//! (stills have no bounds) and never overlaps its neighbour (trim does not push it away).

use std::collections::BTreeSet;

use cutweave_commands::{CmdError, Command, QueryCtx, Registry, RegistryError, Tx, command, wire};
use cutweave_model::{
    Clip, EntityKind, GroupId, Item, ItemId, MediaAsset, MediaKind, Project, Time, TimeInput,
    TrackKind,
};

use crate::{
    item::{
        Located, conflict, duration, ensure_not_magnetic, ensure_orphans_unlockable,
        ensure_unlocked, members, time, unlink_orphans,
    },
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
    /// The clip (give `item` or `items`).
    pub item: Option<ItemId>,
    /// Several clips to split at the same time.
    pub items: Option<Vec<ItemId>>,
    /// Timeline time to split at: strictly inside every clip.
    pub at: TimeInput,
    /// Snap `at` to the audio sample grid instead of frames; only for audio clips not linked
    /// to video (default `false`).
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
