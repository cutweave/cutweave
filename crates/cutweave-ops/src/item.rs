// SPDX-License-Identifier: GPL-3.0-or-later
//! The `item` namespace (WP-1B.06, WP-2A.03; arch §4.3, §5.1; timeline spec §4.2, §4.3, §4.8):
//! generic edits of any timeline item: `item.move`, `item.delete` and `item.duplicate`
//! ([`edit`], on the placement core), `item.link`, `item.unlink`, `item.attach`, `item.detach` and
//! the query `item.group_select` ([`link`], WP-2A.05), `item.set_props` and the query `item.get`.
//! The media-clip specifics (`clip.split` and the trim family) live in [`crate::clip`].
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
pub mod link;
mod reset;

use std::collections::{BTreeMap, BTreeSet};

use cutweave_commands::{
    CmdError, Command, Query, QueryCtx, Registry, RegistryError, Tx, command, query, wire,
};
use cutweave_model::{
    AudioProps, BlendMode, EntityKind, GroupId, Item, ItemId, Project, PropId, SequenceId, Time,
    TimeInput, Track, TrackId, Vec2, VisualProps,
};

pub use link::{
    GroupMember, ItemAttach, ItemAttachOutput, ItemDetach, ItemGroupSelect, ItemGroupSelectOutput,
    ItemLink, ItemLinkOutput, ItemUnlink, ItemUnlinkOutput,
};
pub use reset::{ItemResetProps, ItemResetPropsOutput, ResetGroup};

pub(crate) use edit::remint;
pub use edit::{
    DuplicatedItem, ItemDelete, ItemDeleteOutput, ItemDuplicate, ItemDuplicateOutput, ItemMove,
    ItemMoveOutput, MovedItem,
};

use crate::{
    keyframe::{self, KeyValue},
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
        .register::<ItemLink>()?
        .register::<ItemUnlink>()?
        .register::<ItemAttach>()?
        .register::<ItemDetach>()?
        .register::<ItemGroupSelect>()?
        .register::<ItemSetProps>()?
        .register::<ItemResetProps>()?
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
    #[ui(widget = "vec2", unit = "px", step = 1.0, keyframeable = true)]
    pub position: Option<Vec2>,
    /// Size multiplier per axis (1.0 = aspect-fit); finite and `>= 0`.
    #[ui(
        widget = "vec2",
        unit = "%",
        scale = 100.0,
        step = 0.01,
        min = 0.0,
        linked = true,
        keyframeable = true
    )]
    pub scale: Option<Vec2>,
    /// Clockwise rotation in degrees.
    #[ui(widget = "number", unit = "°", step = 1.0, keyframeable = true)]
    pub rotation: Option<f64>,
    /// The item point `position` places and rotation turns around, in item units.
    #[ui(widget = "vec2", step = 1.0, keyframeable = true, advanced = true)]
    pub anchor: Option<Vec2>,
}

/// A partial crop: fractions `0..=1` of the upright source picture removed from each edge.
#[wire]
pub struct CropPatch {
    /// Fraction removed from the left edge.
    #[schemars(range(min = 0.0, max = 1.0))]
    #[ui(
        widget = "number",
        unit = "%",
        scale = 100.0,
        step = 0.01,
        keyframeable = true
    )]
    pub left: Option<f64>,
    /// Fraction removed from the top edge.
    #[schemars(range(min = 0.0, max = 1.0))]
    #[ui(
        widget = "number",
        unit = "%",
        scale = 100.0,
        step = 0.01,
        keyframeable = true
    )]
    pub top: Option<f64>,
    /// Fraction removed from the right edge.
    #[schemars(range(min = 0.0, max = 1.0))]
    #[ui(
        widget = "number",
        unit = "%",
        scale = 100.0,
        step = 0.01,
        keyframeable = true
    )]
    pub right: Option<f64>,
    /// Fraction removed from the bottom edge.
    #[schemars(range(min = 0.0, max = 1.0))]
    #[ui(
        widget = "number",
        unit = "%",
        scale = 100.0,
        step = 0.01,
        keyframeable = true
    )]
    pub bottom: Option<f64>,
}

/// A partial flip.
#[wire]
pub struct FlipPatch {
    /// Mirror left-right.
    #[ui(keyframeable = true)]
    pub horizontal: Option<bool>,
    /// Mirror top-bottom.
    #[ui(keyframeable = true)]
    pub vertical: Option<bool>,
}

/// Item properties to change: absent fields keep their value.
#[wire]
pub struct ItemProps {
    /// Display name; an empty string clears it (the media name is shown again).
    #[ui(group = "clip", order = 0)]
    pub name: Option<String>,
    /// Rendered and heard (`true`) or kept but skipped (`false`).
    #[ui(group = "clip", order = 1)]
    pub enabled: Option<bool>,
    /// Opacity `0.0..=1.0`.
    #[schemars(range(min = 0.0, max = 1.0))]
    #[ui(
        widget = "slider",
        step = 0.01,
        unit = "%",
        scale = 100.0,
        keyframeable = true,
        group = "compositing",
        order = 0
    )]
    pub opacity: Option<f64>,
    /// Clip gain in dB, `-144..=48` (0 = unchanged).
    #[schemars(range(min = -144.0, max = 48.0))]
    #[ui(
        unit = "dB",
        step = 0.5,
        precision = 1,
        keyframeable = true,
        group = "audio",
        order = 0
    )]
    pub audio_gain: Option<f64>,
    /// Stereo position, `-1` (left) to `1` (right).
    #[schemars(range(min = -1.0, max = 1.0))]
    #[ui(step = 0.05, keyframeable = true, group = "audio", order = 1)]
    pub audio_pan: Option<f64>,
    /// Placement on the canvas (partial).
    #[ui(group = "transform", order = 1)]
    pub transform: Option<TransformPatch>,
    /// Edges of the source picture cut away (partial).
    #[ui(group = "crop", order = 2)]
    pub crop: Option<CropPatch>,
    /// Mirroring (partial).
    #[ui(group = "flip", order = 3)]
    pub flip: Option<FlipPatch>,
    /// How the item combines with what is below.
    #[ui(group = "compositing", order = 1)]
    pub blend_mode: Option<BlendMode>,
}

/// A key `item.set_props` wrote on an animated property.
#[wire]
pub struct KeyedProp {
    /// The item.
    pub item: ItemId,
    /// The property.
    pub prop: PropId,
    /// Item time of the key, after snapping.
    pub t: Time,
    /// Sequence time of the key, after snapping.
    pub at: Time,
}

/// Result of `item.set_props`.
#[wire]
pub struct ItemSetPropsOutput {
    /// The item (the first of `items`).
    pub item: ItemId,
    /// The props whose value actually changed on any item (`name`, `enabled`, `opacity`,
    /// `audio_gain`, `transform.position`, ...); empty if everything was already as requested.
    pub changed: Vec<String>,
    /// The keys written on animated properties.
    pub keyed: Vec<KeyedProp>,
}

/// Set item properties (partial). An animated property is keyed at `at` (sequence time,
/// required then). Example: `{item, props:{opacity:0.5, crop:{left:0.25}}}`.
///
/// Absent fields keep their value; setting a value it already has is a no-op. Static properties
/// are set as before and `at` is ignored. With `auto_key: false` an animated property is
/// `Invalid`: `keyframe_clear` makes it static first. `Locked` if the track is locked.
#[command(
    name = "item.set_props",
    summary = "Set an item's name, enabled, opacity, audio, transform, crop, flip or blend mode (partial)",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = true)
)]
pub struct ItemSetProps {
    /// The item (or `items`).
    pub item: Option<ItemId>,
    /// Items to give the same properties.
    pub items: Option<Vec<ItemId>>,
    /// The properties to change; at least one.
    pub props: ItemProps,
    /// Key animated properties at `at` (default true).
    pub auto_key: Option<bool>,
    /// Sequence time (the playhead) to key animated properties at.
    pub at: Option<TimeInput>,
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

/// The order `changed` lists properties in: the P1 fields first, then the later ones.
const SET_ORDER: [PropId; 13] = [
    PropId::Opacity,
    PropId::AudioGain,
    PropId::TransformPosition,
    PropId::TransformScale,
    PropId::TransformRotation,
    PropId::TransformAnchor,
    PropId::AudioPan,
    PropId::CropLeft,
    PropId::CropTop,
    PropId::CropRight,
    PropId::CropBottom,
    PropId::FlipHorizontal,
    PropId::FlipVertical,
];

/// A validated request: the name change and the property values, in [`SET_ORDER`].
struct SetPlan {
    name: Option<String>,
    values: Vec<(PropId, KeyValue)>,
}

impl ItemProps {
    /// The given property values, in [`SET_ORDER`].
    fn values(&self) -> Vec<(PropId, KeyValue)> {
        let t = self.transform.as_ref();
        let c = self.crop.as_ref();
        let f = self.flip.as_ref();
        let num = |v: Option<f64>| v.map(KeyValue::Number);
        SET_ORDER
            .into_iter()
            .filter_map(|id| {
                let v = match id {
                    PropId::Opacity => num(self.opacity),
                    PropId::AudioGain => num(self.audio_gain),
                    PropId::AudioPan => num(self.audio_pan),
                    PropId::TransformPosition => t.and_then(|t| t.position).map(KeyValue::Vec2),
                    PropId::TransformScale => t.and_then(|t| t.scale).map(KeyValue::Vec2),
                    PropId::TransformRotation => num(t.and_then(|t| t.rotation)),
                    PropId::TransformAnchor => t.and_then(|t| t.anchor).map(KeyValue::Vec2),
                    PropId::CropLeft => num(c.and_then(|c| c.left)),
                    PropId::CropTop => num(c.and_then(|c| c.top)),
                    PropId::CropRight => num(c.and_then(|c| c.right)),
                    PropId::CropBottom => num(c.and_then(|c| c.bottom)),
                    PropId::FlipHorizontal => f.and_then(|f| f.horizontal).map(KeyValue::Bool),
                    PropId::FlipVertical => f.and_then(|f| f.vertical).map(KeyValue::Bool),
                    _ => None,
                };
                v.map(|v| (id, v))
            })
            .collect()
    }

    fn check(&self) -> Result<SetPlan, CmdError> {
        let values = self.values();
        if values.is_empty()
            && self.name.is_none()
            && self.enabled.is_none()
            && self.blend_mode.is_none()
        {
            return Err(CmdError::invalid(
                "/props",
                "give at least one of name, enabled, opacity, audio_gain, audio_pan, transform, \
                 crop, flip, blend_mode",
            ));
        }
        let name = match &self.name {
            None => None,
            Some(n) if n.trim().is_empty() => Some(String::new()),
            Some(n) => Some(check_name("/props/name", n)?),
        };
        for (id, v) in &values {
            let pointer = format!("/props/{}", id.as_str().replace('.', "/"));
            match (id, v) {
                (PropId::Opacity, KeyValue::Number(o)) if !(0.0..=1.0).contains(o) => {
                    return Err(CmdError::invalid(
                        pointer,
                        "the opacity must be between 0 and 1",
                    ));
                }
                (PropId::AudioGain, KeyValue::Number(g))
                    if !(MIN_TRACK_GAIN_DB..=MAX_TRACK_GAIN_DB).contains(g) =>
                {
                    return Err(CmdError::invalid(
                        pointer,
                        format!(
                            "the gain must be between {MIN_TRACK_GAIN_DB} and {MAX_TRACK_GAIN_DB} dB"
                        ),
                    ));
                }
                (PropId::AudioPan, KeyValue::Number(p)) if !(-1.0..=1.0).contains(p) => {
                    return Err(CmdError::invalid(
                        pointer,
                        "the pan must be between -1 (left) and 1 (right)",
                    ));
                }
                (
                    PropId::CropLeft | PropId::CropTop | PropId::CropRight | PropId::CropBottom,
                    KeyValue::Number(c),
                ) if !(0.0..=1.0).contains(c) => {
                    return Err(CmdError::invalid(
                        pointer,
                        "a crop edge must be between 0 and 1",
                    ));
                }
                (PropId::TransformScale, KeyValue::Vec2(v)) => {
                    vec2(&pointer, *v, true)?;
                }
                (_, KeyValue::Vec2(v)) => {
                    vec2(&pointer, *v, false)?;
                }
                (_, KeyValue::Number(n)) => {
                    finite(&pointer, *n)?;
                }
                _ => {}
            }
        }
        Ok(SetPlan { name, values })
    }
}

/// What one item's update did.
#[derive(Default)]
struct ItemChange {
    changed: Vec<String>,
    keyed: Vec<(PropId, Time)>,
}

impl ItemSetProps {
    fn plan(&self, doc: &Project) -> Result<(SetPlan, Vec<keyframe::Target>), CmdError> {
        let targets = keyframe::targets(doc, self.item, self.items.as_deref())?;
        Ok((self.props.check()?, targets))
    }

    /// Applies the request to one clip.
    fn update_clip(
        &self,
        clip: &mut cutweave_model::Clip,
        plan: &SetPlan,
        target: &keyframe::Target,
    ) -> Result<ItemChange, CmdError> {
        let mut out = ItemChange::default();
        if let Some(name) = plan.name.clone().map(|n| (!n.is_empty()).then_some(n))
            && clip.name != name
        {
            clip.name = name;
            out.changed.push("name".into());
        }
        if let Some(v) = self.props.enabled
            && clip.enabled != v
        {
            clip.enabled = v;
            out.changed.push("enabled".into());
        }
        let auto_key = self.auto_key.unwrap_or(true);
        for (id, value) in &plan.values {
            let pointer = format!("/props/{}", id.as_str().replace('.', "/"));
            if keyframe::is_animated(clip, *id) {
                if !auto_key {
                    return Err(CmdError::invalid(
                        "/auto_key",
                        format!(
                            "`{id}` is animated: set auto_key to true and give `at` to key it, \
                             or call keyframe_clear first"
                        ),
                    ));
                }
                let Some(at) = self.at else {
                    return Err(CmdError::invalid(
                        "/at",
                        format!(
                            "`{id}` is animated: give at (sequence time) to key it, or call \
                             keyframe_clear first"
                        ),
                    ));
                };
                let t = keyframe::TimeArg {
                    t: None,
                    at: Some(at),
                    pointer: "",
                }
                .item_time(target, Some(*id), false)?;
                if keyframe::outside(target, t) {
                    return Err(CmdError::invalid(
                        "/at",
                        "the playhead is outside the item: key it inside the item's range",
                    ));
                }
                let req = keyframe::KeyReq {
                    t,
                    value: Some(value.clone()),
                    interp: None,
                };
                let (_, changed) =
                    keyframe::set_keys(clip, *id, &[req], false, &|_| pointer.clone())?;
                if changed {
                    out.changed.push(id.to_string());
                }
                out.keyed.push((*id, t));
            } else if keyframe::set_static(clip, *id, value) {
                out.changed.push(id.to_string());
            }
        }
        if let Some(mode) = self.props.blend_mode
            && clip.visual.blend_mode != mode
        {
            clip.visual.blend_mode = mode;
            out.changed.push("blend_mode".into());
        }
        Ok(out)
    }
}

impl Command for ItemSetProps {
    type Output = ItemSetPropsOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        let (plan, targets) = self.plan(ctx.doc())?;
        for target in targets {
            let at = ctx.item(target.id)?;
            let mut clip = at
                .item
                .as_clip()
                .ok_or_else(|| CmdError::unsupported("only clips have these properties"))?
                .clone();
            self.update_clip(&mut clip, &plan, &target)?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemSetPropsOutput, CmdError> {
        let (plan, targets) = self.plan(tx.doc())?;
        let mut changed: Vec<String> = Vec::new();
        let mut keyed = Vec::new();
        for target in &targets {
            let done = tx.update_item(target.id, |item| {
                let Some(clip) = item.as_clip_mut() else {
                    return Err(CmdError::unsupported("only clips have these properties"));
                };
                self.update_clip(clip, &plan, target)
            })?;
            for c in done.changed {
                if !changed.contains(&c) {
                    changed.push(c);
                }
            }
            keyed.extend(done.keyed.into_iter().map(|(prop, t)| KeyedProp {
                item: target.id,
                prop,
                t,
                at: keyframe::sequence_time(target, t),
            }));
        }
        Ok(ItemSetPropsOutput {
            item: targets[0].id,
            changed,
            keyed,
        })
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
    /// The item this one is attached to (it follows that parent), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attached_to: Option<ItemId>,
    /// The items attached to this one, in track-ID then item-ID order.
    pub children: Vec<ItemId>,
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
            attached_to: at.item.attached_to(),
            children: at
                .sequence
                .tracks
                .values()
                .flat_map(|t| t.items.iter_by_id().map(|(_, i)| i))
                .filter(|i| i.attached_to() == Some(self.item))
                .map(Item::id)
                .collect(),
        })
    }
}
