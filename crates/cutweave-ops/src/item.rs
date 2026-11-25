// SPDX-License-Identifier: GPL-3.0-or-later
//! The `item` namespace (WP-1B.06, WP-2A.03; arch §4.3, §5.1; timeline spec §4.2, §4.3, §4.8):
//! generic edits of any timeline item: `item.move`, `item.delete` and `item.duplicate`
//! ([`edit`], on the placement core), `item.set_props` and the query `item.get`. The media-clip
//! specifics (`clip.split` and the trim family) live in [`crate::clip`].
//!
//! # Edit semantics (the contract for the timeline UI and for agents)
//!
//! The normative rules are `docs/spec/timeline-edit-semantics.md` (ADR-0023). In short:
//!
//! * **Frame grid.** Every time param is resolved against the frame rate of the item's
//!   sequence and snapped to the *nearest* frame boundary (ties later), so the stored result
//!   is always frame-aligned, also at NTSC rates. The result states the time that was used.
//! * **No overlaps.** Items on one track never overlap (arch §4.3). On free tracks a placement
//!   that would create one fails with `Invalid` by default (`reject`); `new_track` puts the item
//!   on a new track of the same kind directly above, `overwrite` cuts out what is there and
//!   `insert` ripples it later.
//! * **The magnetic main track** is gap-free from 0: placing on it inserts by default, and
//!   deleting or moving an item off it closes the space. A ripple reaches the tracks of the
//!   linked partners of what shifts.
//! * **Locks.** An edit that would add, remove or change an item of a locked track fails with
//!   `Locked` naming that track; this includes the tracks of linked partners, of rippled items
//!   and of attached children. Nothing is applied in that case.
//! * **Linked groups.** Items sharing a `linked_group` (video and its audio) are aligned and
//!   move, trim, split, are deleted and duplicated together. `unlink: true` on `item.move` and
//!   `clip.trim` edits only the given item and removes it from the group; a group never keeps a
//!   single member.
//! * **Attachments.** A child (`attached_to`) follows its parent's moves, is detached and stays
//!   when its parent is deleted, and moves to the right half when its parent is split.
//! * **Edit reports.** Every edit returns `report`: exactly what changed.

pub mod edit;

use std::collections::{BTreeMap, BTreeSet};

use cutweave_commands::{
    CmdError, Command, Query, QueryCtx, Registry, RegistryError, Tx, command, query, wire,
};
use cutweave_model::{
    AudioProps, EntityKind, GroupId, Item, ItemId, Project, SequenceId, Time, Track, TrackId,
    Transform, Vec2, VisualProps,
};

pub use edit::{
    DuplicatedItem, ItemDelete, ItemDeleteOutput, ItemDuplicate, ItemDuplicateOutput, ItemMove,
    ItemMoveOutput, MovedItem,
};

use crate::{
    timeline::{ItemSummary, item_summary},
    track::{MAX_TRACK_GAIN_DB, MIN_TRACK_GAIN_DB, check_editable_kind},
    util::check_name,
};

/// Longest `items` list of `item.delete` (and the other commands that take a list of items).
pub const MAX_DELETE_ITEMS: usize = 10_000;

/// Registers the `item.*` commands and queries.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<ItemMove>()?
        .register::<ItemDelete>()?
        .register::<ItemDuplicate>()?
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

/// Clears the `linked_group` of the last remaining member of every group in `groups` that is
/// left with exactly one member (a group needs two to link anything). Called after an edit
/// took members out of their groups, inside the same transaction so undo restores the link.
/// A last member on a locked track makes the transaction fail with `Locked`.
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
