// SPDX-License-Identifier: GPL-3.0-or-later
//! Speed, reverse and freeze frames (WP-2D.01; timeline spec INV-47; keyframes §8.1):
//! `clip.set_speed`, `clip.reverse`, `clip.reset_speed` and `clip.freeze_frame`, on the
//! placement core ([`crate::timeline::placement`]).
//!
//! # Speed
//!
//! A speed is an exact fraction from 1/10 to 100 ([`Speed`]); a clip of length `d` plays
//! `round(d × speed)` of its media (INV-47). Linked partners get the same speed, so a video clip
//! and its sound stay together (INV-8).
//!
//! - `keep: "in_point"` (default): the clip keeps the media it plays, so its length becomes
//!   `d × old / new`, on the frame grid (the end snaps to the nearest frame, ties later; at least
//!   one frame; a frame less when the nearest one would read past the media). The arithmetic is
//!   rational, so 2× over 10 s gives 5 s and 1/3× gives 30 s, to the flick, and changing the speed
//!   back gives the length back whenever the first change landed on the grid.
//! - `keep: "duration"`: the length stays and the clip plays more or less media: forwards from its
//!   `source_in`, reversed up to its source end (the first frame it shows stays). Past the media
//!   it is `Invalid` at `/speed`, naming the fastest speed the media allows.
//! - **Neighbours**: on the magnetic main track the change ripples (what follows the clip moves
//!   by the change of its length, on the tracks of the clip and its partners, INV-31). On a free
//!   track the clip keeps its start; growing into the next item is `Invalid` at `/speed` naming
//!   it, unless `ripple: true` moves what follows as on the main track.
//! - **Keyframes** (keyframes §8.1, owner decision 2026-11-25): `keyframes: "keep"` (default)
//!   leaves keys at their item times; `"scale"` maps each key to `round_half_even(t × old / new)`
//!   snapped to the frame grid in sequence time, so keys stay on the same media frames; where two
//!   keys land on one time the later one wins.
//!
//! # Reverse
//!
//! `clip.reverse` plays the same media range backwards (or forwards again): nothing moves, keys
//! stay (§8.1: the animation stays with the item).
//!
//! # Freeze frame
//!
//! `clip.freeze_frame` freezes the frame a clip shows at `at` (CapCut): the clip (and its linked
//! partners) is split there, everything after `at` on its track and its partners' tracks moves
//! later by the freeze's length (default 3 s), and a still of that frame is placed in between:
//! a new clip of the same media with `freeze: {at_source}`, normal speed, no keys (each animated
//! property takes its value at `at`), silent and unlinked. On the main track it stays gapless;
//! the sound of a linked partner pauses for the freeze. At the clip's first frame nothing is
//! split and the still goes before it. IDs (INV-23): the still, then the split's right group (when
//! linked) and its right halves.
//!
//! Stills (images) and freeze frames have no speed or direction (`Unsupported`); only clips with a
//! picture can be frozen.

use std::collections::BTreeSet;

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{
    AnimValue, Animatable, AudioProps, Clip, Custom, Duration, Extra, FrameRate, Freeze, Item,
    ItemId, MediaKind, Project, PropId, PropMut, Rounding, SequenceId, SourceMap, Speed, Time,
    TimeInput, TrackKind, ValueRange, fit_source_in,
};

use super::{clip_of, media_bound, media_of};
use crate::{
    item::{Located, members},
    report::EditReport,
    timeline::{
        edit::given_items,
        placement::{DryIds, Edit, Ids, Landing, PlacementMode, Plan, frame_flicks, span, time},
    },
    track::check_editable_kind,
    util::{resolve_frame_duration, resolve_frame_time},
};

/// The default length of a freeze frame (the editor's setting `freeze.default_duration` passes
/// its own).
pub const DEFAULT_FREEZE_SECONDS: i64 = 3;

// ----- wire types ------------------------------------------------------------------------------

/// A speed: a number (`2`, `0.5`; exact as written), text (`"1/3"`, `"2x"`, `"50%"`) or an exact
/// fraction `{"num": 1, "den": 3}`. From 0.1 to 100.
#[wire]
#[serde(untagged)]
pub enum SpeedInput {
    /// A number, exact as written (`0.1` is 1/10).
    Number(#[specta(type = specta_typescript::Number)] f64),
    /// `"1/3"`, `"2x"`, `"0.5"`, `"50%"`.
    Text(String),
    /// An exact fraction.
    Fraction(Speed),
}

impl SpeedInput {
    fn resolve(&self, pointer: &str) -> Result<Speed, CmdError> {
        match self {
            SpeedInput::Number(v) => Speed::from_f64(*v),
            SpeedInput::Text(t) => Speed::parse(t),
            SpeedInput::Fraction(s) => Ok(*s),
        }
        .map_err(|e| CmdError::invalid(pointer, e.to_string()))
    }
}

/// What a speed change keeps: `in_point` (default) keeps the media the clip plays and changes
/// its length; `duration` keeps its length and plays more or less media.
// The variants are documented above, not one by one: undocumented variants keep the schema a
// plain string `enum`.
#[allow(missing_docs)]
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpeedKeep {
    InPoint,
    Duration,
}

/// What a speed change does to keyframes: `keep` (default) leaves them at their item times;
/// `scale` stretches them with the speed so they stay on the same media frames.
#[allow(missing_docs)]
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeyframeTiming {
    Keep,
    Scale,
}

/// A clip after a speed change, a reverse or a reset.
#[wire]
pub struct SpeedClip {
    /// The clip.
    pub item: ItemId,
    /// Its start.
    pub start: Time,
    /// Its length.
    pub duration: Duration,
    /// Its source position.
    pub source_in: Time,
    /// Its speed.
    pub speed: Speed,
    /// Whether it plays backwards.
    pub reverse: bool,
}

/// Result of `clip.set_speed`, `clip.reverse` and `clip.reset_speed`.
#[wire]
pub struct ClipSpeedOutput {
    /// The clips changed: each requested clip (in request order) followed by its linked
    /// partners.
    pub items: Vec<SpeedClip>,
    /// What changed (timeline spec §3.1). A change of speed or direction alone (same length and
    /// source position) shows only in `items`.
    pub report: EditReport,
}

/// Result of `clip.freeze_frame`.
#[wire]
pub struct ClipFreezeFrameOutput {
    /// The new still.
    pub freeze: ItemId,
    /// The part of the clip before the still (keeps the clip's ID); `None` when frozen at the
    /// clip's first frame.
    pub before: Option<ItemId>,
    /// The part after the still: the right half of the split, or the whole clip when frozen at
    /// its first frame.
    pub after: ItemId,
    /// The timeline time frozen (frame-snapped), where the still starts.
    pub at: Time,
    /// The media position of the frozen frame.
    pub at_source: Time,
    /// The still's length.
    pub duration: Duration,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

// ----- clip.set_speed ----------------------------------------------------------------------------

/// Plays a clip (`item`), or several (`items`), and their linked partners at `speed`: a number
/// (`2`, `0.5`), text (`"1/3"`, `"2x"`, `"50%"`) or `{num, den}`, from 0.1 to 100, exact. With
/// `keep: "in_point"` (default) the clip plays the same media and its length changes (snapped to
/// frames); with `keep: "duration"` its length stays and it plays more or less media. On the main
/// track what follows moves with the clip's end; on a free track the clip keeps its start and
/// may not grow into the next item (`Invalid` at `/speed`) unless `ripple: true`. `keyframes:
/// "scale"` stretches keys with the speed (default `"keep"`). Stills and freeze frames are
/// `Unsupported`; reading past the media is `Invalid`; `Locked` if a track involved is locked.
#[command(
    name = "clip.set_speed",
    summary = "Change a clip's playback speed (0.1x to 100x, exact; linked partners follow)",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = true)
)]
pub struct ClipSetSpeed {
    /// The clip (or `items`).
    pub item: Option<ItemId>,
    /// Clips to change at once.
    pub items: Option<Vec<ItemId>>,
    /// The speed: `2`, `0.5`, `"1/3"`, `"2x"`, `"50%"` or `{num, den}` (0.1 to 100).
    pub speed: SpeedInput,
    /// `in_point` (default): same media, new length; `duration`: same length, more or less media.
    pub keep: Option<SpeedKeep>,
    /// On a free track, move what follows instead of failing at the next item (default `false`;
    /// the main track always ripples).
    pub ripple: Option<bool>,
    /// `keep` (default): keys stay at their item times; `scale`: they stretch with the speed.
    pub keyframes: Option<KeyframeTiming>,
}

/// The requested clips of `item` / `items`.
fn given(item: Option<ItemId>, items: Option<&[ItemId]>) -> Result<Vec<ItemId>, CmdError> {
    match (item, items) {
        (Some(i), None) => Ok(vec![i]),
        (None, Some([])) => Err(CmdError::invalid("/items", "give at least one clip")),
        (None, Some(list)) => Ok(list.to_vec()),
        _ => Err(CmdError::invalid(
            "",
            "give exactly one of `item` or `items`",
        )),
    }
}

/// What a speed request does to every clip it names.
struct SpeedRequest {
    items: Vec<ItemId>,
    /// The new speed (`None`: each clip keeps its own).
    speed: Option<Speed>,
    /// The new direction (`None`: toggle each group's, by its first member).
    reverse: Option<bool>,
    /// Whether the direction changes at all (`clip.set_speed` leaves it).
    set_reverse: bool,
    keep: SpeedKeep,
    ripple: bool,
    keyframes: KeyframeTiming,
}

/// A planned speed change: the plan and the changed clips in output order.
struct SpeedPlan {
    plan: Plan,
    order: Vec<ItemId>,
}

/// The linked groups of the requested clips (request order; a clip named twice, or a partner
/// of a clip named earlier, counts once), each checked to be speedable.
fn speed_groups(
    doc: &Project,
    items: &[ItemId],
) -> Result<(SequenceId, Vec<Vec<Located>>), CmdError> {
    let (sequence, given) = given_items(doc, items)?;
    let seq = &doc.sequences[&sequence];
    let mut seen = BTreeSet::new();
    let mut groups = Vec::new();
    for id in given {
        if seen.contains(&id) {
            continue;
        }
        let (_, group) = members(doc, id, true)?;
        for m in &group {
            if let Some(t) = seq.track(&m.track) {
                check_editable_kind("/item", t.kind)?;
            }
            let clip = clip_of(&m.item)?;
            if media_of(doc, clip)?.kind == MediaKind::Image {
                return Err(CmdError::unsupported(format!(
                    "clip `{}` shows a still image, which has no speed or direction",
                    clip.id
                )));
            }
            if clip.freeze.is_some() {
                return Err(CmdError::unsupported(format!(
                    "clip `{}` is a freeze frame, which has no speed or direction",
                    clip.id
                )));
            }
            seen.insert(clip.id);
        }
        groups.push(group);
    }
    Ok((sequence, groups))
}

/// `n / d` rounded half to even (`d > 0`).
fn div_half_even(n: i128, d: i128) -> i128 {
    let (q, r) = (n.div_euclid(d), n.rem_euclid(d));
    match (2 * r).cmp(&d) {
        std::cmp::Ordering::Less => q,
        std::cmp::Ordering::Greater => q + 1,
        std::cmp::Ordering::Equal => q + (q & 1),
    }
}

/// The time on the frame grid of `rate` nearest to `t` flicks (ties later), saturating.
fn snap(rate: FrameRate, t: i64, rounding: Rounding) -> i64 {
    rate.snap_to_frame(time(t), rounding)
        .map_or(t, Time::flicks)
}

/// `scale_times` for a speed change (keyframes §8.1): every key of every animated property of
/// `clip` (which starts at `start`) maps to `round_half_even(t × num / den)`, snapped to the
/// frame grid in sequence time.
fn scale_keys(clip: &mut Clip, (num, den): (u64, u64), start: i64, rate: FrameRate) {
    fn one<T: AnimValue>(a: &mut Animatable<T>, f: &dyn Fn(Time) -> Time) {
        if a.is_animated() {
            *a = a.retimed(f);
        }
    }
    let f = |t: Time| -> Time {
        let scaled = div_half_even(i128::from(t.flicks()) * i128::from(num), i128::from(den));
        let scaled = i64::try_from(scaled).unwrap_or(if scaled < 0 { i64::MIN } else { i64::MAX });
        let abs = snap(rate, start.saturating_add(scaled), Rounding::Nearest);
        time(abs.saturating_sub(start))
    };
    for id in PropId::ALL {
        match clip.prop_mut(id) {
            PropMut::Scalar(a) => one(a, &f),
            PropMut::Vec2(a) => one(a, &f),
            PropMut::Bool(a) => one(a, &f),
        }
    }
}

/// The fastest speed (two decimals, rounded down) at which `room` flicks of media fill `len`.
fn fastest(room: i64, len: i64) -> String {
    let hundredths = (i128::from(room.max(0)) * 100) / i128::from(len.max(1));
    format!("{}.{:02}x", hundredths / 100, hundredths % 100)
}

/// One group's new `(length, source_in per member)` at `speed` and direction `reverse`, or
/// `Invalid` at `/speed` when its media is too short.
fn retimed_group(
    doc: &Project,
    e: &Edit<'_>,
    group: &[Located],
    speed: Speed,
    reverse: bool,
    keep: SpeedKeep,
    rate: FrameRate,
) -> Result<(i64, Vec<i64>), CmdError> {
    let clip = |id: ItemId| -> Result<Clip, CmdError> {
        e.locate(id)
            .and_then(|(_, i)| i.as_clip().cloned())
            .ok_or_else(|| CmdError::internal("a speed change lost a clip"))
    };
    let first = clip(group[0].item.id())?;
    let (start, len) = (first.start.flicks(), first.duration.flicks());
    // Each member: (map, media length).
    let mut maps = Vec::with_capacity(group.len());
    for m in group {
        let c = clip(m.item.id())?;
        let bound = media_bound(media_of(doc, &c)?);
        maps.push((c.id, SourceMap::of(&c), bound));
    }
    // Where each member reads at length `l`: forwards from `source_in`, reversed up to the
    // source end; `None` when that leaves the media.
    let sources = |l: i64| -> Option<Vec<i64>> {
        maps.iter()
            .map(|(_, map, bound)| {
                let span = speed.source(l);
                let s = if reverse {
                    fit_source_in(map.source_out().saturating_sub(span), span, *bound)
                } else {
                    fit_source_in(map.source_in, span, *bound)
                };
                let inside = s >= 0 && bound.is_none_or(|total| s.saturating_add(span) <= total);
                inside.then_some(s)
            })
            .collect()
    };
    let frame = frame_flicks(rate);
    let new_len = match keep {
        SpeedKeep::Duration => len,
        SpeedKeep::InPoint => {
            let (num, den) = first.speed.ratio_to(speed);
            let exact = i128::from(len) * i128::from(num) / i128::from(den.max(1));
            let exact = i64::try_from(exact).unwrap_or(i64::MAX);
            let end = snap(rate, start.saturating_add(exact), Rounding::Nearest);
            let nearest = end.saturating_sub(start).max(frame);
            if sources(nearest).is_some() {
                nearest
            } else {
                let floor = snap(rate, start.saturating_add(exact), Rounding::Floor);
                floor.saturating_sub(start).max(frame)
            }
        }
    };
    if let Some(found) = sources(new_len) {
        return Ok((new_len, found));
    }
    // Name the first member whose media is too short, and the fastest speed it allows.
    for (id, map, bound) in &maps {
        let room = if reverse {
            map.source_out()
        } else if let Some(total) = bound {
            total.saturating_sub(map.source_in)
        } else {
            continue;
        };
        if speed.source(new_len) > room {
            return Err(CmdError::invalid(
                "/speed",
                format!(
                    "clip `{id}` would play past the {} of its media at {speed}x; its media \
                     allows up to {} for its length",
                    if reverse { "start" } else { "end" },
                    fastest(room, new_len)
                ),
            ));
        }
    }
    Err(CmdError::invalid(
        "/speed",
        format!("the clips cannot play at {speed}x"),
    ))
}

fn plan_speed(req: &SpeedRequest, doc: &Project, ids: &mut dyn Ids) -> Result<SpeedPlan, CmdError> {
    let (sequence, groups) = speed_groups(doc, &req.items)?;
    let seq = &doc.sequences[&sequence];
    let rate = seq.format.frame_rate;
    let mut e = Edit::new(doc, sequence, "/speed")?;
    // The latest group first, so a ripple never moves a group still to change.
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by_key(|&g| std::cmp::Reverse(groups[g][0].item.start()));
    for g in order {
        let group = &groups[g];
        let first = group[0]
            .item
            .as_clip()
            .ok_or_else(|| CmdError::internal("a speed group without a clip"))?;
        let speed = req.speed.unwrap_or(first.speed);
        let reverse = match (req.set_reverse, req.reverse) {
            (false, _) => first.reverse,
            (true, Some(r)) => r,
            (true, None) => !first.reverse,
        };
        let (start, end) = span(&group[0].item);
        let len = end - start;
        let (new_len, sources) = retimed_group(doc, &e, group, speed, reverse, req.keep, rate)?;
        let ratio = first.speed.ratio_to(speed);
        let mut tracks = Vec::new();
        for (m, source) in group.iter().zip(sources) {
            let id = m.item.id();
            e.retime(id, start, new_len, source);
            let scale = req.keyframes == KeyframeTiming::Scale && ratio.0 != ratio.1;
            e.update(id, |i| {
                if let Some(c) = i.as_clip_mut() {
                    c.speed = speed;
                    c.reverse = reverse;
                    if scale {
                        scale_keys(c, ratio, start, rate);
                    }
                }
            });
            if !tracks.contains(&m.track) {
                tracks.push(m.track);
            }
        }
        let delta = new_len - len;
        if delta == 0 {
            continue;
        }
        let magnetic = tracks
            .iter()
            .any(|t| seq.track(t).is_some_and(|t| t.magnetic));
        if magnetic || req.ripple {
            e.ripple(&tracks, false, end, &move |_| delta);
        } else if delta > 0 {
            for m in group {
                if let Some(other) = e.blocker(m.track, end, end + delta, Some(m.item.id())) {
                    let name = e.track_name(m.track);
                    e.defer(e.invalid(format!(
                        "clip `{}` would run into item `{other}` on track `{name}`; pass \
                         `ripple: true` to move what follows",
                        m.item.id()
                    )));
                }
            }
        }
    }
    let order = groups.iter().flatten().map(|m| m.item.id()).collect();
    Ok(SpeedPlan {
        plan: e.finish(ids)?,
        order,
    })
}

fn speed_output(planned: &SpeedPlan, tx: &mut Tx<'_>) -> Result<ClipSpeedOutput, CmdError> {
    let report = planned.plan.commit(tx)?;
    let items = planned
        .order
        .iter()
        .filter_map(|id| planned.plan.after.find_item(id))
        .filter_map(|(_, item)| item.as_clip())
        .map(|c| SpeedClip {
            item: c.id,
            start: c.start,
            duration: c.duration,
            source_in: c.source_in,
            speed: c.speed,
            reverse: c.reverse,
        })
        .collect();
    Ok(ClipSpeedOutput { items, report })
}

impl ClipSetSpeed {
    fn request(&self) -> Result<SpeedRequest, CmdError> {
        Ok(SpeedRequest {
            items: given(self.item, self.items.as_deref())?,
            speed: Some(self.speed.resolve("/speed")?),
            reverse: None,
            set_reverse: false,
            keep: self.keep.unwrap_or(SpeedKeep::InPoint),
            ripple: self.ripple.unwrap_or(false),
            keyframes: self.keyframes.unwrap_or(KeyframeTiming::Keep),
        })
    }
}

impl Command for ClipSetSpeed {
    type Output = ClipSpeedOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_speed(&self.request()?, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ClipSpeedOutput, CmdError> {
        let doc = tx.doc().clone();
        let planned = plan_speed(&self.request()?, &doc, tx)?;
        speed_output(&planned, tx)
    }
}

// ----- clip.reverse --------------------------------------------------------------------------

/// Plays a clip (`item`), or several (`items`), and their linked partners backwards over the same
/// media range, or forwards again: `reverse: true` or `false`, or omitted to toggle each clip.
/// Nothing moves and keyframes stay. Stills and freeze frames are `Unsupported`; `Locked` if a
/// track involved is locked. Reversed long-GOP video without a proxy scrubs slowly (WP-2D.01b).
#[command(
    name = "clip.reverse",
    summary = "Play a clip backwards, or forwards again (linked partners follow)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ClipReverse {
    /// The clip (or `items`).
    pub item: Option<ItemId>,
    /// Clips to change at once.
    pub items: Option<Vec<ItemId>>,
    /// `true` backwards, `false` forwards; omitted: toggle.
    pub reverse: Option<bool>,
}

impl ClipReverse {
    fn request(&self) -> Result<SpeedRequest, CmdError> {
        Ok(SpeedRequest {
            items: given(self.item, self.items.as_deref())?,
            speed: None,
            reverse: self.reverse,
            set_reverse: true,
            keep: SpeedKeep::Duration,
            ripple: false,
            keyframes: KeyframeTiming::Keep,
        })
    }
}

impl Command for ClipReverse {
    type Output = ClipSpeedOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_speed(&self.request()?, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ClipSpeedOutput, CmdError> {
        let doc = tx.doc().clone();
        let planned = plan_speed(&self.request()?, &doc, tx)?;
        speed_output(&planned, tx)
    }
}

// ----- clip.reset_speed ----------------------------------------------------------------------

/// Plays a clip (`item`), or several (`items`), and their linked partners forwards at normal speed
/// again, keeping the media they play (their length changes as in `clip.set_speed` with `keep:
/// "in_point"`; `ripple` and `keyframes` as there).
#[command(
    name = "clip.reset_speed",
    summary = "Play a clip forwards at normal speed again (linked partners follow)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ClipResetSpeed {
    /// The clip (or `items`).
    pub item: Option<ItemId>,
    /// Clips to reset at once.
    pub items: Option<Vec<ItemId>>,
    /// On a free track, move what follows instead of failing at the next item (default `false`).
    pub ripple: Option<bool>,
    /// `keep` (default): keys stay at their item times; `scale`: they stretch with the speed.
    pub keyframes: Option<KeyframeTiming>,
}

impl ClipResetSpeed {
    fn request(&self) -> Result<SpeedRequest, CmdError> {
        Ok(SpeedRequest {
            items: given(self.item, self.items.as_deref())?,
            speed: Some(Speed::ONE),
            reverse: Some(false),
            set_reverse: true,
            keep: SpeedKeep::InPoint,
            ripple: self.ripple.unwrap_or(false),
            keyframes: self.keyframes.unwrap_or(KeyframeTiming::Keep),
        })
    }
}

impl Command for ClipResetSpeed {
    type Output = ClipSpeedOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_speed(&self.request()?, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ClipSpeedOutput, CmdError> {
        let doc = tx.doc().clone();
        let planned = plan_speed(&self.request()?, &doc, tx)?;
        speed_output(&planned, tx)
    }
}

// ----- clip.freeze_frame ---------------------------------------------------------------------

/// Freezes the frame a clip shows at timeline time `at` (frame-snapped, inside the clip): the clip
/// and its linked partners are split there, what follows on their tracks moves later by
/// `duration` (default 3 s, frame-snapped), and a still of that frame goes in between (a clip of
/// the same media with `freeze`, silent, unlinked, each animated property at its value at `at`).
/// On the main track it stays gapless. At the clip's first frame nothing is split. Clips without
/// a picture, stills and freeze frames are `Unsupported`; `Locked` if a track involved is locked.
#[command(
    name = "clip.freeze_frame",
    summary = "Freeze a clip's frame at a time: split there and insert a still of it (3 s by default)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ClipFreezeFrame {
    /// The clip.
    pub item: ItemId,
    /// Timeline time of the frame to freeze, inside the clip (frame-snapped).
    pub at: TimeInput,
    /// Length of the still (default 3 s; frame-snapped, at least one frame).
    pub duration: Option<TimeInput>,
}

struct FreezePlan {
    plan: Plan,
    freeze: ItemId,
    at: i64,
    at_source: i64,
    duration: i64,
}

/// Each animated property of `clip` replaced by its value at item time `rel` (in its stored
/// range): the still's properties (keyframes §8.1).
fn freeze_props(clip: &mut Clip, rel: Time) {
    fn one<T: AnimValue>(a: &mut Animatable<T>, rel: Time, range: ValueRange) {
        if a.is_animated() {
            *a = Animatable::Static(T::from_eval(&T::clamp(a.eval(rel), range)));
        }
    }
    for id in PropId::ALL {
        let range = id.spec().stored;
        match clip.prop_mut(id) {
            PropMut::Scalar(a) => one(a, rel, range),
            PropMut::Vec2(a) => one(a, rel, range),
            PropMut::Bool(a) => one(a, rel, range),
        }
    }
}

fn plan_freeze(
    cmd: &ClipFreezeFrame,
    doc: &Project,
    ids: &mut dyn Ids,
) -> Result<FreezePlan, CmdError> {
    let (sequence, group) = members(doc, cmd.item, false)?;
    let located = &group[0];
    let seq = &doc.sequences[&sequence];
    let track = seq
        .track(&located.track)
        .ok_or_else(|| CmdError::internal("a clip without its track"))?;
    check_editable_kind("/item", track.kind)?;
    let clip = clip_of(&located.item)?;
    let media = media_of(doc, clip)?;
    // An image sequence is a moving picture too (ADR-0035).
    let moving = matches!(media.kind, MediaKind::Video | MediaKind::ImageSequence);
    if track.kind != TrackKind::Video || !moving {
        return Err(CmdError::unsupported(format!(
            "clip `{}` has no moving picture to freeze (only video clips can be frozen)",
            clip.id
        )));
    }
    if clip.freeze.is_some() {
        return Err(CmdError::unsupported(format!(
            "clip `{}` is already a freeze frame",
            clip.id
        )));
    }
    let rate = seq.format.frame_rate;
    let at = resolve_frame_time("/at", cmd.at, rate)?.flicks();
    let (start, end) = span(&located.item);
    if !(start <= at && at < end) {
        return Err(CmdError::invalid(
            "/at",
            format!(
                "the freeze time must lie inside clip `{}` ({start} .. {end})",
                clip.id
            ),
        ));
    }
    let duration = match cmd.duration {
        Some(d) => resolve_frame_duration("/duration", d, rate)?,
        None => resolve_frame_duration(
            "/duration",
            TimeInput::from(time(
                DEFAULT_FREEZE_SECONDS * cutweave_model::FLICKS_PER_SECOND,
            )),
            rate,
        )?,
    }
    .flicks();
    let rel = at - start;
    let at_source = SourceMap::of(clip)
        .at(rel)
        .ok_or_else(|| CmdError::invalid("/at", "the freeze time has no frame"))?;
    let mut e = Edit::new(doc, sequence, "/at")?;
    let freeze: ItemId = e.mint(ids);
    let mut still = Clip::new(freeze, clip.media, time(at), dur_of(duration));
    still.source_in = time(at_source);
    still.freeze = Some(Freeze {
        at_source: time(at_source),
    });
    still.visual = clip.visual.clone();
    freeze_props(&mut still, time(rel));
    still.audio = AudioProps::default();
    still.custom = Custom::new();
    still.extra = Extra::new();
    e.place_movers(
        ids,
        vec![Landing {
            item: Item::Clip(still),
            dest: located.track,
            start: at,
            old: at,
        }],
        PlacementMode::Insert,
        located.track,
        false,
    );
    Ok(FreezePlan {
        plan: e.finish(ids)?,
        freeze,
        at,
        at_source,
        duration,
    })
}

fn dur_of(flicks: i64) -> Duration {
    Duration::from_flicks(flicks.max(0)).unwrap_or(Duration::ZERO)
}

impl Command for ClipFreezeFrame {
    type Output = ClipFreezeFrameOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_freeze(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ClipFreezeFrameOutput, CmdError> {
        let doc = tx.doc().clone();
        let planned = plan_freeze(self, &doc, tx)?;
        let report = planned.plan.commit(tx)?;
        let right = planned
            .plan
            .splits
            .iter()
            .find(|s| s.left == self.item)
            .map(|s| s.right);
        let (before, after) = match right {
            Some(r) => (Some(self.item), r),
            None => (None, self.item),
        };
        Ok(ClipFreezeFrameOutput {
            freeze: planned.freeze,
            before,
            after,
            at: time(planned.at),
            at_source: time(planned.at_source),
            duration: dur_of(planned.duration),
            report,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_even_rounds_ties_to_even() {
        assert_eq!(div_half_even(5, 2), 2);
        assert_eq!(div_half_even(7, 2), 4);
        assert_eq!(div_half_even(-5, 2), -2);
        assert_eq!(div_half_even(-7, 2), -4);
        assert_eq!(div_half_even(10, 3), 3);
        assert_eq!(div_half_even(11, 3), 4);
    }

    #[test]
    fn the_fastest_speed_is_rounded_down() {
        assert_eq!(fastest(10, 3), "3.33x");
        assert_eq!(fastest(20, 10), "2.00x");
    }
}
