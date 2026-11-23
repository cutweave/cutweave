// SPDX-License-Identifier: GPL-3.0-or-later
//! The `item` namespace (WP-1B.06; arch §4.3, §5.1): generic edits of any timeline item:
//! `item.move`, `item.delete`, `item.set_props` and the query `item.get`. The media-clip
//! specifics (`clip.split`, `clip.trim`) live in [`crate::clip`].
//!
//! # Edit semantics (the contract for the timeline UI, WP-1M.06, and for agents)
//!
//! * **Frame grid.** Every time param is resolved against the frame rate of the item's
//!   sequence and snapped to the *nearest* frame boundary (ties later), so the stored result
//!   is always frame-aligned, also at NTSC rates. The result states the time that was used.
//! * **No overlaps.** Items on one track never overlap (arch §4.3). A command that would
//!   create one fails with `Invalid` at the offending param, unless it accepts
//!   `overlap: "new_track"`, which puts the item on a new track of the same kind directly
//!   above the target (the policy of `timeline.insert`).
//! * **Locks.** An edit that adds, removes or changes an item of a locked track fails with
//!   `Locked` naming that track; this includes the tracks of linked partners. Nothing is
//!   applied in that case.
//! * **Linked groups.** Items sharing a `linked_group` (video and its audio) move, trim, split
//!   and are deleted together. `unlink: true` on `item.move` and `clip.trim` edits only the
//!   given item and removes it from the group. A group never keeps a single member: an item
//!   left alone in its group by `unlink` (or by `track.remove`) is unlinked in the same
//!   transaction. If that item sits on a locked track the edit fails with `Locked` naming that
//!   track and applies nothing, like any edit touching a locked partner's track.
//! * **No ripple.** Edits never shift other items; gaps stay gaps.
//!
//! ```text
//! split at T            trim start to T      trim end to T        move to start S
//! [  A     ]            [  A     ]           [  A     ]           [  A     ]
//! [ A1 | A2 ]           [..|  A'  ]          [  A'  |..]         ..........[  A  ]
//!      ^T                  ^T                     ^T                       ^S
//! ```

use std::collections::{BTreeMap, BTreeSet};

use cutweave_commands::{
    CmdError, Command, Query, QueryCtx, Registry, RegistryError, Tx, command, query, wire,
};
use cutweave_model::{
    AudioProps, EntityKind, GroupId, Item, ItemId, Project, SequenceId, Time, TimeInput, Track,
    TrackId, Transform, Vec2, VisualProps,
};

use crate::{
    timeline::{ItemSummary, OverlapPolicy, Slot, item_summary, materialize},
    track::{MAX_TRACK_GAIN_DB, MIN_TRACK_GAIN_DB, check_editable_kind},
    util::{check_name, resolve_frame_time},
};

/// Registers the `item.*` commands and queries.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<ItemMove>()?
        .register::<ItemDelete>()?
        .register::<ItemSetProps>()?
        .register::<ItemGet>()?;
    Ok(())
}

// ----- shared helpers ------------------------------------------------------------------------

/// An item with the track it sits on.
#[derive(Clone, Debug)]
pub(crate) struct Located {
    pub(crate) track: TrackId,
    pub(crate) item: Item,
}

/// The item followed by the other members of its linked group (in track-ID then item-ID
/// order), or just the item when `with_partners` is false or it is not linked.
pub(crate) fn members(
    doc: &Project,
    id: ItemId,
    with_partners: bool,
) -> Result<(SequenceId, Vec<Located>), CmdError> {
    let (sequence, track, item) = locate(doc, id)?;
    let mut out = vec![Located {
        track,
        item: item.clone(),
    }];
    if with_partners && let Some(group) = item.linked_group() {
        let seq = doc
            .sequences
            .get(&sequence)
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, sequence))?;
        for t in seq.tracks.values() {
            for other in t.items.iter_by_id().map(|(_, i)| i) {
                if other.id() != id && other.linked_group() == Some(group) {
                    out.push(Located {
                        track: t.id,
                        item: other.clone(),
                    });
                }
            }
        }
    }
    Ok((sequence, out))
}

fn locate(doc: &Project, id: ItemId) -> Result<(SequenceId, TrackId, &Item), CmdError> {
    let at = doc
        .find_item(&id)
        .ok_or_else(|| CmdError::not_found(EntityKind::Item, id))?;
    Ok((at.sequence.id, at.track.id, at.item))
}

/// `Locked` if the track is locked; `Unsupported` if its kind is not editable.
pub(crate) fn editable_track(track: &Track) -> Result<(), CmdError> {
    check_editable_kind("/item", track.kind)?;
    if track.locked {
        return Err(CmdError::locked(EntityKind::Track, track.id));
    }
    Ok(())
}

/// Every member's track must be editable and unlocked.
pub(crate) fn ensure_unlocked(doc: &Project, members: &[Located]) -> Result<(), CmdError> {
    for m in members {
        let track = doc
            .sequences
            .values()
            .find_map(|s| s.track(&m.track))
            .ok_or_else(|| CmdError::not_found(EntityKind::Track, m.track))?;
        editable_track(track)?;
    }
    Ok(())
}

pub(crate) fn time(pointer: &str, flicks: i64) -> Result<Time, CmdError> {
    Time::from_flicks(flicks).map_err(|e| CmdError::invalid(pointer, e.to_string()))
}

pub(crate) fn duration(pointer: &str, flicks: i64) -> Result<cutweave_model::Duration, CmdError> {
    cutweave_model::Duration::from_flicks(flicks)
        .map_err(|e| CmdError::invalid(pointer, e.to_string()))
}

/// Clears the `linked_group` of the last remaining member of every group in `groups` that is
/// left with exactly one member (a group needs two to link anything). Called after an edit
/// took members out of their groups, inside the same transaction so undo restores the link.
/// A last member on a locked track makes the transaction fail with `Locked`; commands check
/// [`ensure_orphans_unlockable`] up front so `validate` reports it too.
pub(crate) fn unlink_orphans(
    tx: &mut Tx<'_>,
    groups: impl IntoIterator<Item = GroupId>,
) -> Result<(), CmdError> {
    let groups: BTreeSet<GroupId> = groups.into_iter().collect();
    if groups.is_empty() {
        return Ok(());
    }
    let mut left: BTreeMap<GroupId, Vec<ItemId>> = BTreeMap::new();
    for t in tx.doc().sequences.values().flat_map(|s| s.tracks.values()) {
        for item in t.items.iter() {
            if let Some(g) = item.linked_group().filter(|g| groups.contains(g)) {
                left.entry(g).or_default().push(item.id());
            }
        }
    }
    for id in left.into_values().filter(|m| m.len() == 1).flatten() {
        tx.update_item(id, |item| {
            item.set_linked_group(None);
            Ok(())
        })?;
    }
    Ok(())
}

/// `Locked` (naming the partner's track) if taking `leaving` out of `group` would leave a
/// single partner on a locked track: that partner would have to be unlinked, an edit of a
/// locked track (arch §4.3).
pub(crate) fn ensure_orphans_unlockable(
    doc: &Project,
    group: Option<GroupId>,
    leaving: ItemId,
) -> Result<(), CmdError> {
    let Some(group) = group else {
        return Ok(());
    };
    let mut rest = doc
        .sequences
        .values()
        .flat_map(|s| s.tracks.values())
        .flat_map(|t| t.items.iter().map(move |i| (t, i)))
        .filter(|(_, i)| i.linked_group() == Some(group) && i.id() != leaving);
    if let (Some((t, _)), None) = (rest.next(), rest.next())
        && t.locked
    {
        return Err(CmdError::locked(EntityKind::Track, t.id));
    }
    Ok(())
}

/// The first item on `track` (not in `excluded`) that overlaps `[start, end)`.
pub(crate) fn conflict(
    track: &Track,
    start: Time,
    end: Time,
    excluded: &BTreeSet<ItemId>,
) -> Option<ItemId> {
    track
        .items
        .overlapping(start, end)
        .map(Item::id)
        .find(|id| !excluded.contains(id))
}

// ----- item.move -----------------------------------------------------------------------------

/// Where one item ended up.
#[wire]
pub struct MovedItem {
    /// The item.
    pub item: ItemId,
    /// The track it is on now.
    pub track: TrackId,
    /// Its new start.
    pub start: Time,
}

/// Result of `item.move`.
#[wire]
pub struct ItemMoveOutput {
    /// The moved items: the requested one first, then its linked partners.
    pub items: Vec<MovedItem>,
    /// Tracks created by `overlap: "new_track"`, in creation order.
    pub created_tracks: Vec<TrackId>,
}

/// Moves an item to a new start, optionally onto another track of the same kind and sequence.
/// The item's linked partners move by the same amount on their own tracks (`unlink: true`
/// moves only this item and removes it from its group). Overlap is `Invalid{path:"/start"}`
/// by default; `overlap: "new_track"` puts the item on a new track directly above the target.
/// `Locked` if the source, the target or a partner's track is locked. `start` is snapped to
/// the sequence's frame grid and must be `>= 0`.
#[command(
    name = "item.move",
    summary = "Move an item in time and/or to another track (linked partners follow)",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = true)
)]
pub struct ItemMove {
    /// The item.
    pub item: ItemId,
    /// New timeline position of the item's first frame, `>= 0`.
    pub start: TimeInput,
    /// Target track of the same sequence and kind (default: stay on the current track).
    pub to_track: Option<TrackId>,
    /// Move only this item and remove it from its linked group (default `false`).
    pub unlink: Option<bool>,
    /// Overlap handling (default `reject`).
    pub overlap: Option<OverlapPolicy>,
}

struct MovePlan {
    sequence: SequenceId,
    /// The group the primary item leaves under `unlink: true`.
    leaves: Option<GroupId>,
    /// `(item, new start, slot)` of each moved item.
    moves: Vec<(ItemId, Time, Slot)>,
    unlink: bool,
}

impl ItemMove {
    fn plan(&self, doc: &Project) -> Result<MovePlan, CmdError> {
        let unlink = self.unlink.unwrap_or(false);
        let (sequence, group) = members(doc, self.item, !unlink)?;
        let seq = &doc.sequences[&sequence];
        let rate = seq.format.frame_rate;
        let primary = &group[0];
        let start = resolve_frame_time("/start", self.start, rate)?;
        if start < Time::ZERO {
            return Err(CmdError::invalid("/start", "the time must not be negative"));
        }
        ensure_unlocked(doc, &group)?;
        if unlink {
            ensure_orphans_unlockable(doc, primary.item.linked_group(), primary.item.id())?;
        }
        let target = match self.to_track {
            None => primary.track,
            Some(t) => {
                let track = match seq.track(&t) {
                    Some(track) => track,
                    None if doc.sequences.values().any(|s| s.track(&t).is_some()) => {
                        return Err(CmdError::invalid(
                            "/to_track",
                            format!("track `{t}` is not in the item's sequence"),
                        ));
                    }
                    None => return Err(CmdError::not_found(EntityKind::Track, t)),
                };
                let from_kind = seq.track(&primary.track).map(|x| x.kind);
                if Some(track.kind) != from_kind {
                    return Err(CmdError::invalid(
                        "/to_track",
                        format!(
                            "a {} item cannot move to a {} track",
                            from_kind.map_or_else(|| "?".into(), |k| k.to_string()),
                            track.kind
                        ),
                    ));
                }
                editable_track(track)?;
                t
            }
        };
        let delta = start.flicks() - primary.item.start().flicks();
        let policy = self.overlap.unwrap_or_default();
        let excluded: BTreeSet<ItemId> = group.iter().map(|m| m.item.id()).collect();
        let mut placed: Vec<(TrackId, Time, Time)> = Vec::new();
        let mut moves = Vec::new();
        for (i, m) in group.iter().enumerate() {
            let at = if i == 0 {
                start
            } else {
                time("/start", m.item.start().flicks() + delta)
                    .map_err(|_| partner_range(&m.item))?
            };
            if at < Time::ZERO {
                return Err(partner_range(&m.item));
            }
            let end = at
                .checked_add(m.item.duration())
                .map_err(|e| CmdError::invalid("/start", e.to_string()))?;
            let track_id = if i == 0 { target } else { m.track };
            let track = &seq.tracks[&track_id];
            let busy = conflict(track, at, end, &excluded).or_else(|| {
                placed
                    .iter()
                    .find(|(t, s, e)| *t == track_id && *s < end && at < *e)
                    .map(|_| m.item.id())
            });
            let slot = match (busy, policy) {
                (None, _) => Slot::Existing(track_id),
                (Some(other), OverlapPolicy::Reject) => {
                    return Err(CmdError::invalid(
                        "/start",
                        format!(
                            "item `{}` would overlap item `{other}` on track `{track_id}`; \
                             choose a free time or use overlap \"new_track\"",
                            m.item.id()
                        ),
                    ));
                }
                (Some(_), OverlapPolicy::NewTrack) => Slot::New { above: track_id },
            };
            if matches!(slot, Slot::Existing(_)) {
                placed.push((track_id, at, end));
            }
            moves.push((m.item.id(), at, slot));
        }
        Ok(MovePlan {
            sequence,
            leaves: if unlink {
                primary.item.linked_group()
            } else {
                None
            },
            moves,
            unlink,
        })
    }
}

fn partner_range(item: &Item) -> CmdError {
    CmdError::invalid(
        "/start",
        format!(
            "linked item `{}` would start before zero; use unlink: true to move only this item",
            item.id()
        ),
    )
}

impl Command for ItemMove {
    type Output = ItemMoveOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemMoveOutput, CmdError> {
        let plan = self.plan(tx.doc())?;
        let mut created = Vec::new();
        let mut items = Vec::new();
        for (i, (id, at, slot)) in plan.moves.iter().enumerate() {
            let kind = {
                let (_, track) = tx.track(match slot {
                    Slot::Existing(t) => *t,
                    Slot::New { above } => *above,
                })?;
                track.kind
            };
            let track = materialize(tx, plan.sequence, *slot, kind, &mut created)?;
            tx.move_item(*id, track)?;
            let unlink = plan.unlink && i == 0;
            tx.update_item(*id, |item| {
                item.set_start(*at);
                if unlink {
                    item.set_linked_group(None);
                }
                Ok(())
            })?;
            items.push(MovedItem {
                item: *id,
                track,
                start: *at,
            });
        }
        unlink_orphans(tx, plan.leaves)?;
        Ok(ItemMoveOutput {
            items,
            created_tracks: created,
        })
    }
}

// ----- item.delete ---------------------------------------------------------------------------

/// Result of `item.delete`.
#[wire]
pub struct ItemDeleteOutput {
    /// Every deleted item: the requested ones in request order, then their linked partners.
    pub deleted: Vec<ItemId>,
}

/// Longest `items` list of `item.delete`.
pub const MAX_DELETE_ITEMS: usize = 10_000;

/// Deletes items and their linked partners. Gaps stay (no ripple). All or nothing: if any
/// affected track is locked the call fails with `Locked` and nothing is deleted. Unknown items
/// are `NotFound`; repeated IDs are deleted once.
#[command(
    name = "item.delete",
    summary = "Delete items and their linked partners (no ripple; fails if a track is locked)",
    undoable,
    mcp = "core",
    annotations(destructive = true, idempotent = false)
)]
pub struct ItemDelete {
    /// The items to delete, at least one.
    pub items: Vec<ItemId>,
}

impl ItemDelete {
    fn plan(&self, doc: &Project) -> Result<Vec<Located>, CmdError> {
        if self.items.is_empty() {
            return Err(CmdError::invalid("/items", "give at least one item"));
        }
        if self.items.len() > MAX_DELETE_ITEMS {
            return Err(CmdError::invalid(
                "/items",
                format!("at most {MAX_DELETE_ITEMS} items per call"),
            ));
        }
        let mut seen = BTreeSet::new();
        let mut all = Vec::new();
        for id in &self.items {
            if seen.contains(id) {
                continue;
            }
            let (_, group) = members(doc, *id, true)?;
            for m in group {
                if seen.insert(m.item.id()) {
                    all.push(m);
                }
            }
        }
        // Requested items first, partners after (stable).
        all.sort_by_key(|m| {
            self.items
                .iter()
                .position(|i| *i == m.item.id())
                .unwrap_or(usize::MAX)
        });
        ensure_unlocked(doc, &all)?;
        Ok(all)
    }
}

impl Command for ItemDelete {
    type Output = ItemDeleteOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemDeleteOutput, CmdError> {
        let all = self.plan(tx.doc())?;
        let mut deleted = Vec::with_capacity(all.len());
        for m in &all {
            tx.remove_item(m.item.id())?;
            deleted.push(m.item.id());
        }
        Ok(ItemDeleteOutput { deleted })
    }
}

// ----- item.set_props ------------------------------------------------------------------------

/// A partial transform: absent fields keep their value.
#[wire]
pub struct TransformPatch {
    /// Offset of the anchor from the canvas center, in canvas units (+y is down).
    pub position: Option<Vec2>,
    /// Size multiplier per axis (1.0 = aspect-fit); finite and `>= 0`.
    pub scale: Option<Vec2>,
    /// Clockwise rotation in degrees.
    pub rotation: Option<f64>,
    /// The item point `position` places and rotation turns around, in item units.
    pub anchor: Option<Vec2>,
}

/// Item properties to change: absent fields keep their value.
#[wire]
pub struct ItemProps {
    /// Display name; an empty string clears it (the media name is shown again).
    pub name: Option<String>,
    /// Rendered and heard (`true`) or kept but skipped (`false`).
    pub enabled: Option<bool>,
    /// Opacity `0.0..=1.0`.
    pub opacity: Option<f64>,
    /// Clip gain in dB, `-144..=48` (0 = unchanged).
    pub audio_gain: Option<f64>,
    /// Placement on the canvas (partial).
    pub transform: Option<TransformPatch>,
}

/// Result of `item.set_props`.
#[wire]
pub struct ItemSetPropsOutput {
    /// The item.
    pub item: ItemId,
    /// The props whose value actually changed (`name`, `enabled`, `opacity`, `audio_gain`,
    /// `transform.position`, ...); empty if everything was already as requested.
    pub changed: Vec<String>,
}

/// Changes an item's name, enabled flag, opacity, clip gain or transform; absent fields keep
/// their value and setting a value it already has is a no-op. `Locked` if the item's track is
/// locked.
#[command(
    name = "item.set_props",
    summary = "Set an item's name, enabled, opacity, audio_gain or transform (partial)",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = true)
)]
pub struct ItemSetProps {
    /// The item.
    pub item: ItemId,
    /// The properties to change; at least one.
    pub props: ItemProps,
}

fn finite(pointer: &str, v: f64) -> Result<f64, CmdError> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(CmdError::invalid(
            pointer,
            "the value must be a finite number",
        ))
    }
}

fn vec2(pointer: &str, v: Vec2, non_negative: bool) -> Result<Vec2, CmdError> {
    if !v.is_finite() {
        return Err(CmdError::invalid(
            pointer,
            "both components must be finite numbers",
        ));
    }
    if non_negative && (v.x < 0.0 || v.y < 0.0) {
        return Err(CmdError::invalid(pointer, "both components must be >= 0"));
    }
    Ok(v)
}

impl ItemProps {
    fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.enabled.is_none()
            && self.opacity.is_none()
            && self.audio_gain.is_none()
            && self.transform.as_ref().is_none_or(|t| {
                t.position.is_none()
                    && t.scale.is_none()
                    && t.rotation.is_none()
                    && t.anchor.is_none()
            })
    }

    fn check(&self) -> Result<Option<String>, CmdError> {
        if self.is_empty() {
            return Err(CmdError::invalid(
                "/props",
                "give at least one of name, enabled, opacity, audio_gain, transform",
            ));
        }
        let name = match &self.name {
            None => None,
            Some(n) if n.trim().is_empty() => Some(String::new()),
            Some(n) => Some(check_name("/props/name", n)?),
        };
        if let Some(o) = self.opacity
            && !(0.0..=1.0).contains(&o)
        {
            return Err(CmdError::invalid(
                "/props/opacity",
                "the opacity must be between 0 and 1",
            ));
        }
        if let Some(g) = self.audio_gain
            && !(MIN_TRACK_GAIN_DB..=MAX_TRACK_GAIN_DB).contains(&g)
        {
            return Err(CmdError::invalid(
                "/props/audio_gain",
                format!("the gain must be between {MIN_TRACK_GAIN_DB} and {MAX_TRACK_GAIN_DB} dB"),
            ));
        }
        if let Some(t) = &self.transform {
            if let Some(v) = t.position {
                vec2("/props/transform/position", v, false)?;
            }
            if let Some(v) = t.scale {
                vec2("/props/transform/scale", v, true)?;
            }
            if let Some(v) = t.anchor {
                vec2("/props/transform/anchor", v, false)?;
            }
            if let Some(r) = t.rotation {
                finite("/props/transform/rotation", r)?;
            }
        }
        Ok(name)
    }
}

impl ItemSetProps {
    fn plan(&self, doc: &Project) -> Result<Option<String>, CmdError> {
        let (_, group) = members(doc, self.item, false)?;
        ensure_unlocked(doc, &group)?;
        self.props.check()
    }
}

impl Command for ItemSetProps {
    type Output = ItemSetPropsOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemSetPropsOutput, CmdError> {
        let name = self.plan(tx.doc())?;
        let props = &self.props;
        let changed = tx.update_item(self.item, |item| {
            let Some(clip) = item.as_clip_mut() else {
                return Err(CmdError::unsupported("only clips have these properties"));
            };
            let mut changed: Vec<String> = Vec::new();
            if let Some(name) = name.map(|n| (!n.is_empty()).then_some(n))
                && clip.name != name
            {
                clip.name = name;
                changed.push("name".into());
            }
            if let Some(v) = props.enabled
                && clip.enabled != v
            {
                clip.enabled = v;
                changed.push("enabled".into());
            }
            set_opacity(&mut clip.visual, props.opacity, &mut changed);
            set_gain(&mut clip.audio, props.audio_gain, &mut changed);
            if let Some(t) = &props.transform {
                set_transform(&mut clip.visual.transform, t, &mut changed);
            }
            Ok(changed)
        })?;
        Ok(ItemSetPropsOutput {
            item: self.item,
            changed,
        })
    }
}

#[allow(clippy::float_cmp)] // exact comparison: detects whether the value changes
fn set_opacity(visual: &mut VisualProps, v: Option<f64>, changed: &mut Vec<String>) {
    if let Some(v) = v
        && *visual.opacity.value() != v
    {
        visual.opacity.set(v);
        changed.push("opacity".into());
    }
}

#[allow(clippy::float_cmp)] // exact comparison: detects whether the value changes
fn set_gain(audio: &mut AudioProps, v: Option<f64>, changed: &mut Vec<String>) {
    if let Some(v) = v
        && *audio.gain.value() != v
    {
        audio.gain.set(v);
        changed.push("audio_gain".into());
    }
}

#[allow(clippy::float_cmp)] // exact comparison: detects whether the value changes
fn set_transform(t: &mut Transform, p: &TransformPatch, changed: &mut Vec<String>) {
    if let Some(v) = p.position
        && *t.position.value() != v
    {
        t.position.set(v);
        changed.push("transform.position".into());
    }
    if let Some(v) = p.scale
        && *t.scale.value() != v
    {
        t.scale.set(v);
        changed.push("transform.scale".into());
    }
    if let Some(v) = p.rotation
        && *t.rotation.value() != v
    {
        t.rotation.set(v);
        changed.push("transform.rotation".into());
    }
    if let Some(v) = p.anchor
        && *t.anchor.value() != v
    {
        t.anchor.set(v);
        changed.push("transform.anchor".into());
    }
}

// ----- item.get ------------------------------------------------------------------------------

/// Sections `item.get` can be limited to with `fields`.
pub const ITEM_GET_FIELDS: [&str; 4] = ["name", "source", "audio", "visual"];

/// Where a clip reads from.
#[wire]
pub struct ItemSource {
    /// The media.
    pub media: cutweave_model::MediaId,
    /// Position in the media of the clip's first frame.
    pub source_in: Time,
}

/// Result of `item.get`.
#[wire]
pub struct ItemGetOutput {
    /// The sequence holding the item.
    pub sequence: SequenceId,
    /// The track holding the item.
    pub track: TrackId,
    /// The same projection `timeline.get` returns at depth 2.
    pub item: ItemSummary,
    /// The display name, if it has one (unless `fields` leaves it out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Media and source position (unless `fields` leaves it out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ItemSource>,
    /// Audio properties (unless `fields` leaves them out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioProps>,
    /// Visual properties (unless `fields` leaves them out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visual: Option<VisualProps>,
    /// The other items of the linked group, if any.
    pub linked: Vec<ItemId>,
}

/// Reads one item: the `timeline.get` item summary plus its track, source, audio and visual
/// properties. `fields` limits the extra sections (`name`, `source`, `audio`, `visual`).
#[query(
    name = "item.get",
    summary = "Get one item: summary, track, source, audio and visual properties",
    mcp = "catalog"
)]
pub struct ItemGet {
    /// The item.
    pub item: ItemId,
    /// Sections to include: any of `name`, `source`, `audio`, `visual` (default all).
    pub fields: Option<Vec<String>>,
}

impl Query for ItemGet {
    type Output = ItemGetOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<ItemGetOutput, CmdError> {
        if let Some(fields) = &self.fields {
            for (i, f) in fields.iter().enumerate() {
                if !ITEM_GET_FIELDS.contains(&f.as_str()) {
                    return Err(CmdError::invalid(
                        format!("/fields/{i}"),
                        format!("unknown field `{f}`; use {}", ITEM_GET_FIELDS.join(", ")),
                    ));
                }
            }
        }
        let wants = |name: &str| {
            self.fields
                .as_ref()
                .is_none_or(|f| f.iter().any(|x| x == name))
        };
        let at = ctx.item(self.item)?;
        let (_, group) = members(ctx.doc(), self.item, true)?;
        let clip = at.item.as_clip();
        Ok(ItemGetOutput {
            sequence: at.sequence.id,
            track: at.track.id,
            item: item_summary(at.item),
            name: clip.filter(|_| wants("name")).and_then(|c| c.name.clone()),
            source: clip.filter(|_| wants("source")).map(|c| ItemSource {
                media: c.media,
                source_in: c.source_in,
            }),
            audio: clip.filter(|_| wants("audio")).map(|c| c.audio.clone()),
            visual: clip.filter(|_| wants("visual")).map(|c| c.visual.clone()),
            linked: group.iter().skip(1).map(|m| m.item.id()).collect(),
        })
    }
}
