// SPDX-License-Identifier: GPL-3.0-or-later
//! The trim family (WP-2A.04; timeline spec §3.7, §4.9–§4.13; ADR-0023): `clip.trim`,
//! `clip.ripple_trim`, `clip.roll`, `clip.slip` and `clip.slide`, on the placement core
//! ([`crate::timeline::placement`]).
//!
//! # One delta, one interval
//!
//! Each command moves edges or a source position by one delta `δ`: the trimmed edge, the cut of
//! a roll, the media under a slip, the clip of a slide. Every constraint (one frame of length,
//! the media's bounds, a neighbour on a free track, a ripple collision, time 0) allows an
//! interval of deltas that contains 0, so together they allow `[lo, hi]` ([`Limits`]). A
//! request inside it is applied as asked. Outside it, `clamp: true` applies the nearest allowed
//! delta rounded toward 0 to whole frames and reports `clamped`; otherwise the command fails
//! with `Invalid` at the time param, naming the limit and the largest delta allowed (INV-46).
//! Locks never bound the delta: an edit whose plan changes a locked track is `Locked`, also
//! when it would be out of range (INV-27).
//!
//! # Who moves
//!
//! - **trim**: the edge of the clip and of each linked partner (`unlink: true`: the clip only,
//!   which leaves its group). On free tracks nothing else moves; on the magnetic main track (a
//!   member there) it is a ripple trim (INV-203).
//! - **ripple trim**: the same edges; the clip's start stays, and what starts at or after its
//!   old end on the members' tracks (and the tracks of linked partners of what shifts, or every
//!   track with `ripple_all_tracks`) moves by the change of its end (INV-211, INV-31).
//! - **roll**: the end of `left` and the start of `right` (and of their partners) move together.
//! - **slip**: only the source position of the clip and its partners (a media-time delta).
//! - **slide**: the clip and its partners move; the clip just before it (touching it) and its
//!   partners get a longer or shorter end, the clip just after it a later or earlier start.
//!
//! Source positions follow a start edge by `round(δ × speed)` (INV-47; [`Speed`], always 1 until
//! WP-2D.01 adds speed to the model); a still keeps its source position. Attached children of a
//! trimmed clip keep their start unless it falls outside the clip (then they are clamped into
//! it); children of a slid clip move with it (INV-36). Item markers keep their item-relative times.
//!
//! **Keyframes** (WP-2C.03, keyframes §8.1) follow their content: a start-edge trim, a roll's
//! right clip and a slide's right neighbour shift their keys by `−δ`, so keys stay on the same
//! frame of the media; end-edge trims, slips and the slid clip leave keys alone. Keys outside
//! the trimmed range are kept unless `prune_keyframes` is set.
//!
//! **IDs** (INV-23): none of these commands creates items; a child that would overlap after
//! being clamped or carried gets a new track above its own (INV-37), drawn when it is created.

use std::collections::BTreeSet;

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{
    Clip, FLICKS_PER_SECOND, FrameRate, Item, ItemId, MediaAsset, Project, SampleRate, Sequence,
    SequenceId, Time, TimeInput, Track, TrackId, TrackKind,
};

use super::{clip_of, is_still, media_bound, media_of};
use crate::{
    item::{Located, members},
    keyframe::retime::Retime,
    report::EditReport,
    timeline::placement::{DryIds, Edit, Ids, Plan, dur, frame_flicks, span, time},
    track::check_editable_kind,
    util::{resolve_frame_time, resolve_sample_time},
};

// ----- speed and clip spans (INV-47) ---------------------------------------------------------

/// A clip's playback speed as an exact ratio (timeline spec INV-47): a timeline span `d` plays
/// `d × speed` of its media. The model has no speed yet (WP-2D.01), so every clip plays at
/// [`Speed::ONE`]; the trim math already takes it into account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Speed {
    num: i64,
    den: i64,
}

impl Speed {
    /// Normal speed.
    pub const ONE: Speed = Speed { num: 1, den: 1 };

    /// `num / den`, both positive.
    #[must_use]
    pub fn new(num: i64, den: i64) -> Option<Speed> {
        (num > 0 && den > 0).then_some(Speed { num, den })
    }

    /// The media span a timeline span `d` plays: `d × speed` rounded half away from zero, so
    /// `source(-d) == -source(d)` and trimming by `δ` then by `−δ` restores a source position.
    #[must_use]
    pub fn source(self, d: i64) -> i64 {
        let n = i128::from(d) * i128::from(self.num);
        let den = i128::from(self.den);
        let (q, r) = (n / den, n % den);
        let rounded = if 2 * r.abs() >= den {
            q + n.signum()
        } else {
            q
        };
        saturate(rounded)
    }

    /// The longest timeline span (at least 0) whose media span fits in `room` flicks:
    /// `source(len) <= room`.
    #[must_use]
    pub fn longest_within(self, room: i64) -> i64 {
        // source(len) <= room  <=>  len × num < (room + 1/2) × den  (half away from zero)
        //                     <=>  2·len·num < (2·room + 1)·den
        let limit = (2 * i128::from(room) + 1) * i128::from(self.den);
        let step = 2 * i128::from(self.num);
        saturate(((limit + step - 1).div_euclid(step) - 1).max(0))
    }
}

fn saturate(v: i128) -> i64 {
    i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX })
}

/// One clip as the trim math sees it: timeline start and length, the media position of its
/// first frame, its speed and how far its media goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipSpan {
    /// Timeline start (flicks).
    pub start: i64,
    /// Timeline length (flicks).
    pub len: i64,
    /// Media position of the first frame (flicks).
    pub source_in: i64,
    /// Playback speed.
    pub speed: Speed,
    /// Length of the media when it bounds the clip (not a still, known length).
    pub media_len: Option<i64>,
    /// A still: its source position never moves and nothing bounds it.
    pub still: bool,
}

impl ClipSpan {
    /// The span of `clip`, which plays `media`.
    #[must_use]
    pub fn of(clip: &Clip, media: &MediaAsset) -> ClipSpan {
        ClipSpan {
            start: clip.start.flicks(),
            len: clip.duration.flicks(),
            source_in: clip.source_in.flicks(),
            // WP-2D.01: read the clip's speed here (and the start edge of a reversed clip moves
            // its source end).
            speed: Speed::ONE,
            media_len: media_bound(media),
            still: is_still(media),
        }
    }

    /// The timeline end.
    #[must_use]
    pub fn end(&self) -> i64 {
        self.start.saturating_add(self.len)
    }

    /// The media position just after the last frame: `source_in + source(len)`.
    #[must_use]
    pub fn source_out(&self) -> i64 {
        self.source_in.saturating_add(self.speed.source(self.len))
    }

    /// The start edge moved by `d` (positive: later): the length changes by `−d` and the source
    /// position by `source(d)` (a still's stays). `keep_start`: the clip's start stays where it
    /// is (a ripple trim cuts or extends the head and pulls the rest along).
    #[must_use]
    pub fn head(self, d: i64, keep_start: bool) -> ClipSpan {
        ClipSpan {
            start: if keep_start {
                self.start
            } else {
                self.start.saturating_add(d)
            },
            len: self.len.saturating_sub(d),
            source_in: if self.still {
                self.source_in
            } else {
                self.source_in.saturating_add(self.speed.source(d))
            },
            ..self
        }
    }

    /// The end edge moved by `d`.
    #[must_use]
    pub fn tail(self, d: i64) -> ClipSpan {
        ClipSpan {
            len: self.len.saturating_add(d),
            ..self
        }
    }

    /// The media under the clip moved by `d` (media time; a still's stays).
    #[must_use]
    pub fn slip(self, d: i64) -> ClipSpan {
        ClipSpan {
            source_in: if self.still {
                self.source_in
            } else {
                self.source_in.saturating_add(d)
            },
            ..self
        }
    }

    /// The lowest start-edge delta the media allows (the source position stays at or after 0),
    /// or `None` for a still.
    #[must_use]
    pub fn head_min(&self) -> Option<i64> {
        (!self.still).then(|| -self.speed.longest_within(self.source_in))
    }

    /// The highest end-edge delta the media allows (the source stays inside the media), or
    /// `None` when nothing bounds it.
    #[must_use]
    pub fn tail_max(&self) -> Option<i64> {
        let total = self.media_len?;
        Some(
            self.speed
                .longest_within(total.saturating_sub(self.source_in))
                .saturating_sub(self.len),
        )
    }

    /// The slip deltas the media allows, `(lowest, highest)`; `None` for a still or media of
    /// unknown length (nothing to slip).
    #[must_use]
    pub fn slip_range(&self) -> Option<(i64, i64)> {
        let total = self.media_len.filter(|_| !self.still)?;
        Some((-self.source_in, total.saturating_sub(self.source_out())))
    }
}

// ----- the allowed interval (§3.7) -------------------------------------------------------------

/// The deltas every constraint allows, `[lo, hi]`, with the constraint that set each end.
struct Limits {
    lo: i64,
    hi: i64,
    lo_why: String,
    hi_why: String,
}

/// The delta a command applies.
struct Applied {
    delta: i64,
    clamped: bool,
    /// Out of range without `clamp`: reported after the lock check (INV-27).
    error: Option<CmdError>,
}

impl Limits {
    fn new() -> Limits {
        Limits {
            lo: i64::MIN,
            hi: i64::MAX,
            lo_why: String::new(),
            hi_why: String::new(),
        }
    }

    /// A constraint: `δ >= v` (`v` is at most 0 in a valid document).
    fn at_least(&mut self, v: i64, why: impl FnOnce() -> String) {
        let v = v.min(0);
        if v > self.lo {
            self.lo = v;
            self.lo_why = why();
        }
    }

    /// A constraint: `δ <= v`.
    fn at_most(&mut self, v: i64, why: impl FnOnce() -> String) {
        let v = v.max(0);
        if v < self.hi {
            self.hi = v;
            self.hi_why = why();
        }
    }

    /// INV-46: the request when allowed; otherwise, with `clamp`, the nearest allowed delta
    /// rounded toward 0 to whole grid units; otherwise the request with the error to report.
    fn resolve(&self, requested: i64, clamp: bool, grid: &Grid, pointer: &str) -> Applied {
        if (self.lo..=self.hi).contains(&requested) {
            return Applied {
                delta: requested,
                clamped: false,
                error: None,
            };
        }
        let (bound, word, why) = if requested > self.hi {
            (self.hi, "most", &self.hi_why)
        } else {
            (self.lo, "least", &self.lo_why)
        };
        if clamp {
            return Applied {
                delta: bound / grid.unit * grid.unit,
                clamped: true,
                error: None,
            };
        }
        Applied {
            delta: requested,
            clamped: false,
            error: Some(CmdError::invalid(
                pointer,
                format!(
                    "at {word} {}: {why} (asked for {}; pass `clamp: true` to stop at the limit)",
                    grid.show(bound),
                    grid.show(requested)
                ),
            )),
        }
    }
}

/// The grid an edit snaps to: frames, or the audio samples of a `fine` edit (INV-24, INV-26).
#[derive(Clone, Copy)]
struct Grid {
    rate: FrameRate,
    samples: Option<SampleRate>,
    /// One frame (or sample) in flicks.
    unit: i64,
}

impl Grid {
    fn of(seq: &Sequence, fine: bool) -> Grid {
        let rate = seq.format.frame_rate;
        if fine {
            let samples = seq.format.sample_rate;
            Grid {
                rate,
                samples: Some(samples),
                unit: samples.flicks_per_sample().unwrap_or(1).max(1),
            }
        } else {
            Grid {
                rate,
                samples: None,
                unit: frame_flicks(rate),
            }
        }
    }

    /// A time param snapped to the grid (nearest, ties later).
    fn snap(&self, pointer: &str, input: TimeInput) -> Result<i64, CmdError> {
        let t = match self.samples {
            Some(samples) => resolve_sample_time(pointer, input, self.rate, samples)?,
            None => resolve_frame_time(pointer, input, self.rate)?,
        };
        Ok(t.flicks())
    }

    /// A delta for a message: whole frames (or samples) when it is a whole number of them,
    /// seconds otherwise.
    fn show(&self, d: i64) -> String {
        if d % self.unit == 0 {
            let n = d / self.unit;
            let word = match (self.samples.is_some(), n.abs() == 1) {
                (false, true) => "frame",
                (false, false) => "frames",
                (true, true) => "sample",
                (true, false) => "samples",
            };
            format!("{n:+} {word}")
        } else {
            let ms = i128::from(d) * 1000 / i128::from(FLICKS_PER_SECOND);
            let sign = if d < 0 { "-" } else { "+" };
            let ms = ms.unsigned_abs();
            format!("{sign}{}.{:03}s", ms / 1000, ms % 1000)
        }
    }
}

/// Where an edit point goes: `to` a time, or by a `delta` (exactly one of them).
#[derive(Clone, Copy)]
enum Point {
    To(TimeInput),
    Delta(TimeInput),
}

impl Point {
    fn of(to: Option<TimeInput>, delta: Option<TimeInput>) -> Result<Point, CmdError> {
        match (to, delta) {
            (Some(t), None) => Ok(Point::To(t)),
            (None, Some(d)) => Ok(Point::Delta(d)),
            _ => Err(CmdError::invalid(
                "",
                "give exactly one of `to` (where the edit point goes) or `delta` (how far it moves)",
            )),
        }
    }

    fn pointer(self) -> &'static str {
        match self {
            Point::To(_) => "/to",
            Point::Delta(_) => "/delta",
        }
    }

    /// The requested delta of an edit point that is at `from` now.
    fn delta(self, grid: &Grid, from: i64) -> Result<i64, CmdError> {
        match self {
            Point::To(t) => Ok(grid.snap("/to", t)?.saturating_sub(from)),
            Point::Delta(d) => grid.snap("/delta", d),
        }
    }
}

// ----- shared helpers --------------------------------------------------------------------------

/// A clip of the edit with its track and span.
#[derive(Clone, Copy)]
struct Member {
    track: TrackId,
    id: ItemId,
    span: ClipSpan,
}

/// The members of a group as clips (`Unsupported` for kinds that cannot be edited yet).
fn clips(doc: &Project, seq: &Sequence, group: &[Located]) -> Result<Vec<Member>, CmdError> {
    group
        .iter()
        .map(|m| {
            if let Some(t) = seq.track(&m.track) {
                check_editable_kind("/item", t.kind)?;
            }
            let clip = clip_of(&m.item)?;
            Ok(Member {
                track: m.track,
                id: clip.id,
                span: ClipSpan::of(clip, media_of(doc, clip)?),
            })
        })
        .collect()
}

/// The items just before and just after `id` on `track`, in start order.
fn neighbours(track: &Track, id: ItemId) -> (Option<&Item>, Option<&Item>) {
    let mut prev = None;
    let mut items = track.items.iter();
    while let Some(item) = items.next() {
        if item.id() == id {
            return (prev, items.next());
        }
        prev = Some(item);
    }
    (None, None)
}

fn track_name(seq: &Sequence, track: TrackId) -> String {
    seq.track(&track)
        .map_or_else(|| track.to_string(), |t| t.name.clone())
}

/// The neighbour that stops an edge: `item` on `track` is in the way.
fn in_the_way(seq: &Sequence, track: TrackId, item: &Item) -> String {
    format!(
        "item `{}` on track `{}` is in the way",
        item.id(),
        track_name(seq, track)
    )
}

/// Length and media constraints of a start edge moving by `δ` (the end stays, or moves along
/// when `keep_start`: a ripple trim).
fn head_limits(limits: &mut Limits, m: &Member, grid: &Grid) {
    let s = m.span;
    limits.at_most(s.len - grid.unit, || {
        format!("clip `{}` would become shorter than one frame", m.id)
    });
    if let Some(lo) = s.head_min() {
        limits.at_least(lo, || format!("the media of clip `{}` starts there", m.id));
    }
}

/// Length and media constraints of an end edge moving by `δ`.
fn tail_limits(limits: &mut Limits, m: &Member, grid: &Grid) {
    let s = m.span;
    limits.at_least(grid.unit - s.len, || {
        format!("clip `{}` would become shorter than one frame", m.id)
    });
    if let Some(hi) = s.tail_max() {
        limits.at_most(hi, || format!("the media of clip `{}` ends there", m.id));
    }
}

/// A start edge moving earlier stops at the item before it on its track (unless that item's
/// end moves with it, `moving_ends`) or at time 0.
fn before_limit(limits: &mut Limits, seq: &Sequence, m: &Member, moving_ends: &BTreeSet<ItemId>) {
    let Some(track) = seq.track(&m.track) else {
        return;
    };
    match neighbours(track, m.id).0 {
        Some(prev) if moving_ends.contains(&prev.id()) => {}
        Some(prev) => {
            let end = span(prev).1;
            limits.at_least(end - m.span.start, || in_the_way(seq, m.track, prev));
        }
        None => limits.at_least(-m.span.start, || "time 0 is there".to_owned()),
    }
}

/// An end edge moving later stops at the item after it on its track (unless that item's start
/// moves with it, `moving_starts`).
fn after_limit(limits: &mut Limits, seq: &Sequence, m: &Member, moving_starts: &BTreeSet<ItemId>) {
    let Some(track) = seq.track(&m.track) else {
        return;
    };
    if let Some(next) = neighbours(track, m.id).1
        && !moving_starts.contains(&next.id())
    {
        limits.at_most(next.start().flicks() - m.span.end(), || {
            in_the_way(seq, m.track, next)
        });
    }
}

/// INV-26: a `fine` edit needs an audio clip whose linked group (all of it) is on audio tracks.
fn check_fine(seq: &Sequence, item: &Item) -> Result<(), CmdError> {
    let group = item.linked_group();
    for t in seq.tracks.values().filter(|t| t.kind != TrackKind::Audio) {
        if t.items
            .iter_by_id()
            .any(|(_, i)| i.id() == item.id() || (group.is_some() && i.linked_group() == group))
        {
            return Err(CmdError::invalid(
                "/fine",
                "a fine edit needs audio clips that are not linked to video",
            ));
        }
    }
    Ok(())
}

/// INV-26 after planning: a `fine` edit changed only audio tracks (a ripple may reach others).
fn check_fine_plan(plan: &Plan) -> Result<(), CmdError> {
    for t in plan.after.tracks.values() {
        if t.kind == TrackKind::Audio {
            continue;
        }
        let same = plan
            .before
            .track(&t.id)
            .is_some_and(|b| b.items.ptr_eq(&t.items) || b.items == t.items);
        if !same {
            return Err(CmdError::invalid(
                "/fine",
                format!(
                    "a fine edit changes only audio clips; this one would change track `{}`",
                    t.name
                ),
            ));
        }
    }
    Ok(())
}

/// What a planned trim-family command applies.
struct Planned {
    plan: Plan,
    applied: i64,
    clamped: bool,
}

impl Planned {
    fn finish(
        mut e: Edit<'_>,
        applied: Applied,
        ids: &mut dyn Ids,
        fine: bool,
    ) -> Result<Planned, CmdError> {
        if let Some(error) = applied.error {
            e.defer(error);
        }
        let plan = e.finish(ids)?;
        if fine {
            check_fine_plan(&plan)?;
        }
        Ok(Planned {
            plan,
            applied: applied.delta,
            clamped: applied.clamped,
        })
    }

    fn output(&self, report: EditReport) -> TrimOutput {
        TrimOutput {
            applied_delta: time(self.applied),
            clamped: self.clamped,
            report,
        }
    }
}

/// Result of `clip.ripple_trim`, `clip.roll`, `clip.slip` and `clip.slide`.
#[wire]
pub struct TrimOutput {
    /// The delta applied (flicks): the request, or under `clamp` the nearest one allowed.
    pub applied_delta: Time,
    /// Whether `applied_delta` differs from the request (only with `clamp: true`).
    pub clamped: bool,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

// ----- clip.trim and clip.ripple_trim ------------------------------------------------------------

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

/// Result of `clip.trim`.
#[wire]
pub struct ClipTrimOutput {
    /// The trimmed clips: the requested one first, then its linked partners.
    pub items: Vec<TrimmedClip>,
    /// How far the edge moved (flicks): the request, or under `clamp` the nearest one allowed.
    pub applied_delta: Time,
    /// Whether `applied_delta` differs from the request (only with `clamp: true`).
    pub clamped: bool,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Moves one edge of a clip to a timeline time (`to`) or by an offset (`delta`, negative:
/// earlier; give exactly one). Linked partners get the same edge; `unlink: true` trims only
/// this clip, which leaves its group. On free tracks nothing else moves, and the clip cannot
/// become shorter than one frame, read outside its media or overlap a neighbour. On the
/// magnetic main track it is a ripple trim (`clip.ripple_trim`): what follows moves with the
/// end. A request past a limit is `Invalid` at `/to` (or `/delta`) naming it; with `clamp:
/// true` the edge stops there instead (`applied_delta`, `clamped`). Times snap to frames
/// (`fine: true`: audio samples, for audio clips not linked to video). `Locked` if a track
/// involved is locked.
#[command(
    name = "clip.trim",
    summary = "Trim the start or end edge of a clip (linked partners follow; the main track closes up)",
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
    /// Stop at the nearest limit instead of failing (default `false`).
    pub clamp: Option<bool>,
    /// Snap to the audio sample grid (audio clips only).
    pub fine: Option<bool>,
    /// Drop keyframes the trim leaves outside the clip, keeping the ones that bracket it
    /// (default `false`: trims keep every key).
    pub prune_keyframes: Option<bool>,
}

/// Trims an edge and closes or opens the space after the clip: the clip's start stays, and
/// everything that starts at or after its old end moves by the change of its end, on the
/// tracks of the clip and its linked partners, the tracks of partners of what moves, or every
/// track with `ripple_all_tracks`. The end edge by `δ` lengthens the clip by `δ`; the start
/// edge by `δ` (positive cuts frames from the head; `to` is read as if the clip did not move)
/// shortens it by `δ`. Fails with `Invalid` if the shift would make items overlap on a moved
/// track or move the magnetic main track from another track, and with `Locked` if it would
/// change a locked track. `clamp`, `fine` and `unlink` as in `clip.trim`.
#[command(
    name = "clip.ripple_trim",
    summary = "Trim a clip's edge and move what follows by the same amount (ripple)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ClipRippleTrim {
    /// The clip.
    pub item: ItemId,
    /// The edge to move.
    pub edge: TrimEdge,
    /// Timeline time of the edge afterwards (frame-snapped; for the start edge, read as if the
    /// clip did not move).
    pub to: Option<TimeInput>,
    /// Offset of the edge (negative: earlier). Use seconds or flicks for negative values.
    pub delta: Option<TimeInput>,
    /// Trim only this clip and remove it from its linked group (default `false`).
    pub unlink: Option<bool>,
    /// Stop at the nearest limit instead of failing (default `false`).
    pub clamp: Option<bool>,
    /// Snap to the audio sample grid (audio clips only).
    pub fine: Option<bool>,
    /// Move what follows on every track of the sequence (default `false`).
    pub ripple_all_tracks: Option<bool>,
    /// Drop keyframes the trim leaves outside the clip (default `false`).
    pub prune_keyframes: Option<bool>,
}

/// An edge trim, rippling or not (the params `clip.trim` and `clip.ripple_trim` share).
struct EdgeTrim {
    item: ItemId,
    edge: TrimEdge,
    point: Point,
    unlink: bool,
    clamp: bool,
    fine: bool,
    prune: bool,
    /// `Some(all)`: a ripple trim (`all`: every track ripples); `None`: `clip.trim`, which
    /// ripples only on the magnetic track.
    ripple: Option<bool>,
}

impl EdgeTrim {
    fn plan(&self, doc: &Project, ids: &mut dyn Ids) -> Result<(Planned, Vec<ItemId>), CmdError> {
        let (sequence, group) = members(doc, self.item, !self.unlink)?;
        let seq = &doc.sequences[&sequence];
        let clips = clips(doc, seq, &group)?;
        if self.fine {
            check_fine(seq, &group[0].item)?;
        }
        let grid = Grid::of(seq, self.fine);
        let primary = clips[0].span;
        let at = match self.edge {
            TrimEdge::Start => primary.start,
            TrimEdge::End => primary.end(),
        };
        let pointer = self.point.pointer();
        let requested = self.point.delta(&grid, at)?;
        let magnetic = clips
            .iter()
            .any(|m| seq.track(&m.track).is_some_and(|t| t.magnetic));
        let ripple = self.ripple.or(magnetic.then_some(false));
        let ids_of: BTreeSet<ItemId> = clips.iter().map(|m| m.id).collect();
        let mut tracks: Vec<TrackId> = Vec::new();
        let mut limits = Limits::new();
        for m in &clips {
            match self.edge {
                TrimEdge::Start => head_limits(&mut limits, m, &grid),
                TrimEdge::End => tail_limits(&mut limits, m, &grid),
            }
            if ripple.is_none() {
                match self.edge {
                    TrimEdge::Start => before_limit(&mut limits, seq, m, &BTreeSet::new()),
                    TrimEdge::End => after_limit(&mut limits, seq, m, &BTreeSet::new()),
                }
            }
            if !tracks.contains(&m.track) {
                tracks.push(m.track);
            }
        }
        let theta = primary.end();
        if let Some(all) = ripple {
            let reach = ripple_reach(doc, sequence, &tracks, all, theta)?;
            ripple_limits(&mut limits, seq, &reach, theta, &ids_of, self.edge);
        }
        let applied = limits.resolve(requested, self.clamp, &grid, pointer);
        let d = applied.delta;
        let mut e = Edit::new(doc, sequence, pointer)?;
        for m in &clips {
            let after = match self.edge {
                TrimEdge::Start => m.span.head(d, ripple.is_some()),
                TrimEdge::End => m.span.tail(d),
            };
            if after != m.span {
                e.retime(m.id, after.start, after.len, after.source_in);
                rekey_edge(&mut e, m, self.edge, d, after.len, self.prune);
            }
        }
        if self.unlink
            && let Some(g) = group[0].item.linked_group()
        {
            e.update(self.item, |i| i.set_linked_group(None));
            e.mark_left(g);
        }
        let shift = match self.edge {
            TrimEdge::End => d,
            TrimEdge::Start => -d,
        };
        if let Some(all) = ripple
            && shift != 0
        {
            e.ripple(&tracks, all, theta, &move |_| shift);
        }
        let planned = Planned::finish(e, applied, ids, self.fine)?;
        Ok((planned, clips.iter().map(|m| m.id).collect()))
    }
}

/// Moves the keys of a trimmed clip with its content (keyframes §8.1): a start edge shifts
/// them by `−d`; an end edge keeps them (`prune`: drops those past the new end).
fn rekey_edge(e: &mut Edit<'_>, m: &Member, edge: TrimEdge, d: i64, new_len: i64, prune: bool) {
    match edge {
        TrimEdge::Start => e.rekey(
            m.id,
            Retime::Head {
                delta: time(d),
                duration: dur(m.span.len),
                prune,
            },
        ),
        TrimEdge::End if prune => e.rekey(
            m.id,
            Retime::Tail {
                duration: dur(new_len),
                prune,
            },
        ),
        TrimEdge::End => {}
    }
}

/// The tracks a ripple at `theta` from `tracks` reaches (INV-31), as the placement core
/// computes them: the shift amount does not matter, only which items start at or after it.
fn ripple_reach(
    doc: &Project,
    sequence: SequenceId,
    tracks: &[TrackId],
    all: bool,
    theta: i64,
) -> Result<BTreeSet<TrackId>, CmdError> {
    let mut scratch = Edit::new(doc, sequence, "")?;
    Ok(scratch.ripple(tracks, all, theta, &|_| 1))
}

/// INV-32 as a bound (§3.7): on every rippled track, the first item that shifts must stay at
/// or after the furthest end of what does not shift there (other than the trimmed clips, whose
/// ends move with the shift), and at or after time 0. The shift is `δ` for an end edge and `−δ`
/// for a start edge.
fn ripple_limits(
    limits: &mut Limits,
    seq: &Sequence,
    reach: &BTreeSet<TrackId>,
    theta: i64,
    trimmed: &BTreeSet<ItemId>,
    edge: TrimEdge,
) {
    for tid in reach {
        let Some(track) = seq.track(tid) else {
            continue;
        };
        let mut blocker: Option<(i64, &Item)> = None;
        let mut first = None;
        for item in track.items.iter() {
            let (s, e) = span(item);
            if s >= theta {
                first = Some(s);
                break;
            }
            if !trimmed.contains(&item.id()) && blocker.is_none_or(|(end, _)| e > end) {
                blocker = Some((e, item));
            }
        }
        let Some(first) = first else {
            continue;
        };
        let (floor, why) = match blocker {
            Some((end, item)) => (
                end - first,
                format!(
                    "the ripple would push into item `{}` on track `{}`",
                    item.id(),
                    track.name
                ),
            ),
            None => (
                -first,
                format!("the ripple would move track `{}` before time 0", track.name),
            ),
        };
        match edge {
            TrimEdge::End => limits.at_least(floor, || why),
            TrimEdge::Start => limits.at_most(-floor, || why),
        }
    }
}

fn trimmed_clips(plan: &Plan, ids: &[ItemId]) -> Vec<TrimmedClip> {
    ids.iter()
        .filter_map(|id| plan.after.find_item(id))
        .map(|(_, item)| TrimmedClip {
            item: item.id(),
            start: item.start(),
            duration: item.duration(),
            source_in: item.as_clip().map_or(Time::ZERO, |c| c.source_in),
        })
        .collect()
}

impl ClipTrim {
    fn edge_trim(&self) -> Result<EdgeTrim, CmdError> {
        Ok(EdgeTrim {
            item: self.item,
            edge: self.edge,
            point: Point::of(self.to, self.delta)?,
            unlink: self.unlink.unwrap_or(false),
            clamp: self.clamp.unwrap_or(false),
            fine: self.fine.unwrap_or(false),
            prune: self.prune_keyframes.unwrap_or(false),
            ripple: None,
        })
    }
}

impl Command for ClipTrim {
    type Output = ClipTrimOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.edge_trim()?
            .plan(ctx.doc(), &mut DryIds::default())
            .map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ClipTrimOutput, CmdError> {
        let doc = tx.doc().clone();
        let (planned, ids) = self.edge_trim()?.plan(&doc, tx)?;
        let report = planned.plan.commit(tx)?;
        Ok(ClipTrimOutput {
            items: trimmed_clips(&planned.plan, &ids),
            applied_delta: time(planned.applied),
            clamped: planned.clamped,
            report,
        })
    }
}

impl ClipRippleTrim {
    fn edge_trim(&self) -> Result<EdgeTrim, CmdError> {
        Ok(EdgeTrim {
            item: self.item,
            edge: self.edge,
            point: Point::of(self.to, self.delta)?,
            unlink: self.unlink.unwrap_or(false),
            clamp: self.clamp.unwrap_or(false),
            fine: self.fine.unwrap_or(false),
            prune: self.prune_keyframes.unwrap_or(false),
            ripple: Some(self.ripple_all_tracks.unwrap_or(false)),
        })
    }
}

impl Command for ClipRippleTrim {
    type Output = TrimOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.edge_trim()?
            .plan(ctx.doc(), &mut DryIds::default())
            .map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TrimOutput, CmdError> {
        let doc = tx.doc().clone();
        let (planned, _) = self.edge_trim()?.plan(&doc, tx)?;
        let report = planned.plan.commit(tx)?;
        Ok(planned.output(report))
    }
}

// ----- clip.roll ---------------------------------------------------------------------------------

/// Moves the cut between two adjacent clips of one track (`right` starts where `left` ends) to
/// a timeline time (`to`) or by an offset (`delta`; exactly one): `left` gets a later or
/// earlier end, `right` the same start (its source position follows), and so do their linked
/// partners. Nothing else moves and the track keeps its length. Both clips keep at least one
/// frame and stay inside their media; a partner stops at an item of its track that is not the
/// other side's partner. `Invalid{path:"/right"}` for clips that do not touch; limits as in
/// `clip.trim` (`clamp: true` stops at them). `Locked` if a track involved is locked.
#[command(
    name = "clip.roll",
    summary = "Move the cut between two adjacent clips (one gets longer, the other shorter)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ClipRoll {
    /// The clip before the cut.
    pub left: ItemId,
    /// The clip after the cut: it starts where `left` ends, on the same track.
    pub right: ItemId,
    /// Timeline time of the cut afterwards (frame-snapped).
    pub to: Option<TimeInput>,
    /// Offset of the cut (negative: earlier). Use seconds or flicks for negative values.
    pub delta: Option<TimeInput>,
    /// Stop at the nearest limit instead of failing (default `false`).
    pub clamp: Option<bool>,
    /// Drop keyframes the roll leaves outside the clips (default `false`).
    pub prune_keyframes: Option<bool>,
}

impl ClipRoll {
    fn plan(&self, doc: &Project, ids: &mut dyn Ids) -> Result<Planned, CmdError> {
        let point = Point::of(self.to, self.delta)?;
        let (sequence, left_group) = members(doc, self.left, true)?;
        let (right_sequence, right_group) = members(doc, self.right, true)?;
        let seq = &doc.sequences[&sequence];
        let lefts = clips(doc, seq, &left_group)?;
        let rights = clips(doc, &doc.sequences[&right_sequence], &right_group)?;
        let (l, r) = (lefts[0], rights[0]);
        if right_sequence != sequence || l.track != r.track || r.span.start != l.span.end() {
            return Err(CmdError::invalid(
                "/right",
                format!(
                    "clip `{}` does not start where clip `{}` ends on its track: a roll needs \
                     two clips that touch",
                    self.right, self.left
                ),
            ));
        }
        let grid = Grid::of(seq, false);
        let pointer = point.pointer();
        let requested = point.delta(&grid, l.span.end())?;
        let left_ids: BTreeSet<ItemId> = lefts.iter().map(|m| m.id).collect();
        let right_ids: BTreeSet<ItemId> = rights.iter().map(|m| m.id).collect();
        let mut limits = Limits::new();
        for m in &lefts {
            tail_limits(&mut limits, m, &grid);
            after_limit(&mut limits, seq, m, &right_ids);
        }
        for m in &rights {
            head_limits(&mut limits, m, &grid);
            before_limit(&mut limits, seq, m, &left_ids);
        }
        let applied = limits.resolve(requested, self.clamp.unwrap_or(false), &grid, pointer);
        let d = applied.delta;
        let mut e = Edit::new(doc, sequence, pointer)?;
        if d != 0 {
            let prune = self.prune_keyframes.unwrap_or(false);
            for m in &lefts {
                let after = m.span.tail(d);
                e.retime(m.id, after.start, after.len, after.source_in);
                rekey_edge(&mut e, m, TrimEdge::End, d, after.len, prune);
            }
            for m in &rights {
                let after = m.span.head(d, false);
                e.retime(m.id, after.start, after.len, after.source_in);
                rekey_edge(&mut e, m, TrimEdge::Start, d, after.len, prune);
            }
        }
        Planned::finish(e, applied, ids, false)
    }
}

impl Command for ClipRoll {
    type Output = TrimOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TrimOutput, CmdError> {
        let doc = tx.doc().clone();
        let planned = self.plan(&doc, tx)?;
        let report = planned.plan.commit(tx)?;
        Ok(planned.output(report))
    }
}

// ----- clip.slip ---------------------------------------------------------------------------------

/// Changes which part of the media a clip shows without moving it: its `source_in` (and its
/// linked partners') moves by `delta`, in media time and whole frames (positive: later
/// material). Positions and lengths on the timeline never change. The source range stays inside
/// the media (`clamp: true` stops at its start or end). A still or media of unknown length has
/// nothing to slip: `Invalid{path:"/item"}`. `fine: true` snaps to audio samples (audio clips
/// not linked to video). `Locked` if a track involved is locked.
#[command(
    name = "clip.slip",
    summary = "Show an earlier or later part of a clip's media without moving the clip",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ClipSlip {
    /// The clip.
    pub item: ItemId,
    /// How far the media moves under the clip (positive: later material). Use seconds or
    /// flicks for negative values.
    pub delta: TimeInput,
    /// Stop at the start or end of the media instead of failing (default `false`).
    pub clamp: Option<bool>,
    /// Snap to the audio sample grid (audio clips only).
    pub fine: Option<bool>,
}

impl ClipSlip {
    fn plan(&self, doc: &Project, ids: &mut dyn Ids) -> Result<Planned, CmdError> {
        let fine = self.fine.unwrap_or(false);
        let (sequence, group) = members(doc, self.item, true)?;
        let seq = &doc.sequences[&sequence];
        let clips = clips(doc, seq, &group)?;
        if clips[0].span.slip_range().is_none() {
            return Err(CmdError::invalid(
                "/item",
                format!(
                    "clip `{}` shows a still or media of unknown length: there is nothing to slip",
                    self.item
                ),
            ));
        }
        if fine {
            check_fine(seq, &group[0].item)?;
        }
        let grid = Grid::of(seq, fine);
        let requested = grid.snap("/delta", self.delta)?;
        let mut limits = Limits::new();
        for m in &clips {
            if let Some((lo, hi)) = m.span.slip_range() {
                limits.at_least(lo, || format!("the media of clip `{}` starts there", m.id));
                limits.at_most(hi, || format!("the media of clip `{}` ends there", m.id));
            } else if !m.span.still {
                limits.at_least(-m.span.source_in, || {
                    format!("the media of clip `{}` starts there", m.id)
                });
            }
        }
        let applied = limits.resolve(requested, self.clamp.unwrap_or(false), &grid, "/delta");
        let d = applied.delta;
        let mut e = Edit::new(doc, sequence, "/delta")?;
        if d != 0 {
            for m in &clips {
                let after = m.span.slip(d);
                if after != m.span {
                    e.retime(m.id, after.start, after.len, after.source_in);
                }
            }
        }
        Planned::finish(e, applied, ids, fine)
    }
}

impl Command for ClipSlip {
    type Output = TrimOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TrimOutput, CmdError> {
        let doc = tx.doc().clone();
        let planned = self.plan(&doc, tx)?;
        let report = planned.plan.commit(tx)?;
        Ok(planned.output(report))
    }
}

// ----- clip.slide --------------------------------------------------------------------------------

/// Moves a clip by `delta` between its neighbours, keeping what it shows: the clip and its
/// linked partners move; the clip just before it (if it touches it) gets a later or earlier
/// end and the clip just after it (if it touches it) a later or earlier start (its source
/// position follows), with their partners. A side with a gap stops the clip at the next item
/// (or time 0); the empty end of a track does not. The neighbours keep at least one frame and
/// stay inside their media. On the magnetic main track the first clip cannot slide. Children
/// attached to the clip move with it. Limits as in `clip.trim` (`clamp: true` stops at them).
/// `Locked` if a track involved is locked.
#[command(
    name = "clip.slide",
    summary = "Move a clip between its neighbours, which get longer or shorter to make room",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ClipSlide {
    /// The clip.
    pub item: ItemId,
    /// How far the clip moves (negative: earlier). Use seconds or flicks for negative values.
    pub delta: TimeInput,
    /// Stop at the nearest limit instead of failing (default `false`).
    pub clamp: Option<bool>,
    /// Drop keyframes the slide leaves outside the neighbours (default `false`).
    pub prune_keyframes: Option<bool>,
}

/// The clips a slide moves: the clip's group, and the groups of the clips touching it.
struct SlideParts {
    moving: Vec<Member>,
    before: Vec<Member>,
    after: Vec<Member>,
}

impl ClipSlide {
    fn parts(&self, doc: &Project) -> Result<(SequenceId, SlideParts), CmdError> {
        let (sequence, group) = members(doc, self.item, true)?;
        let seq = &doc.sequences[&sequence];
        let moving = clips(doc, seq, &group)?;
        let c = moving[0];
        let track = seq
            .track(&c.track)
            .ok_or_else(|| CmdError::internal("the clip's track is missing"))?;
        let (prev, next) = neighbours(track, c.id);
        let touching = |item: Option<&Item>, touches: &dyn Fn(&Item) -> bool| match item
            .filter(|i| touches(i))
        {
            Some(i) => members(doc, i.id(), true).and_then(|(_, g)| clips(doc, seq, &g)),
            None => Ok(Vec::new()),
        };
        let before = touching(prev, &|i| span(i).1 == c.span.start)?;
        let after = touching(next, &|i| i.start().flicks() == c.span.end())?;
        Ok((
            sequence,
            SlideParts {
                moving,
                before,
                after,
            },
        ))
    }

    fn plan(&self, doc: &Project, ids: &mut dyn Ids) -> Result<Planned, CmdError> {
        let (sequence, parts) = self.parts(doc)?;
        let seq = &doc.sequences[&sequence];
        let grid = Grid::of(seq, false);
        let requested = grid.snap("/delta", self.delta)?;
        let ids_of = |lists: &[&[Member]]| -> BTreeSet<ItemId> {
            lists.iter().flat_map(|l| l.iter().map(|m| m.id)).collect()
        };
        // An item whose end (or start) moves by the same delta never stops an edge next to it.
        let before_ids = ids_of(&[&parts.before]);
        let after_ids = ids_of(&[&parts.after]);
        let ends_moving = ids_of(&[&parts.moving, &parts.before]);
        let starts_moving = ids_of(&[&parts.moving, &parts.after]);
        let mut limits = Limits::new();
        for m in &parts.moving {
            before_limit(&mut limits, seq, m, &before_ids);
            after_limit(&mut limits, seq, m, &after_ids);
            let first_on_main = seq.track(&m.track).is_some_and(|t| t.magnetic)
                && !neighbours_touching_before(seq, m, &before_ids);
            if first_on_main {
                limits.at_most(0, || {
                    format!(
                        "clip `{}` is the first clip of the main track: sliding it would open a \
                         gap at the start",
                        m.id
                    )
                });
            }
        }
        for m in &parts.before {
            tail_limits(&mut limits, m, &grid);
            after_limit(&mut limits, seq, m, &starts_moving);
        }
        for m in &parts.after {
            head_limits(&mut limits, m, &grid);
            before_limit(&mut limits, seq, m, &ends_moving);
        }
        let applied = limits.resolve(requested, self.clamp.unwrap_or(false), &grid, "/delta");
        let d = applied.delta;
        let mut e = Edit::new(doc, sequence, "/delta")?;
        if d != 0 {
            slide_by(&mut e, &parts, d, self.prune_keyframes.unwrap_or(false));
        }
        Planned::finish(e, applied, ids, false)
    }
}

/// Whether the item before `m` on its track touches it and is one of `before`.
fn neighbours_touching_before(seq: &Sequence, m: &Member, before: &BTreeSet<ItemId>) -> bool {
    seq.track(&m.track)
        .and_then(|t| neighbours(t, m.id).0)
        .is_some_and(|p| before.contains(&p.id()) && span(p).1 == m.span.start)
}

/// Applies a slide by `d`: neighbours trimmed, the clip and its partners moved, their children
/// carried along (INV-36).
fn slide_by(e: &mut Edit<'_>, parts: &SlideParts, d: i64, prune: bool) {
    for m in &parts.before {
        let after = m.span.tail(d);
        e.retime(m.id, after.start, after.len, after.source_in);
        rekey_edge(e, m, TrimEdge::End, d, after.len, prune);
    }
    for m in &parts.after {
        let after = m.span.head(d, false);
        e.retime(m.id, after.start, after.len, after.source_in);
        rekey_edge(e, m, TrimEdge::Start, d, after.len, prune);
    }
    let mut deltas = std::collections::BTreeMap::new();
    for m in &parts.moving {
        e.update(m.id, |i| i.set_start(time(m.span.start + d)));
        e.mark_moved(m.id);
        deltas.insert(m.id, d);
    }
    let decided: BTreeSet<ItemId> = parts
        .moving
        .iter()
        .chain(&parts.before)
        .chain(&parts.after)
        .map(|m| m.id)
        .collect();
    e.follow(&deltas, &BTreeSet::new(), &decided);
}

impl Command for ClipSlide {
    type Output = TrimOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TrimOutput, CmdError> {
        let doc = tx.doc().clone();
        let planned = self.plan(&doc, tx)?;
        let report = planned.plan.commit(tx)?;
        Ok(planned.output(report))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const SEC: i64 = FLICKS_PER_SECOND;

    #[test]
    fn speed_rounds_half_away_from_zero_and_is_odd() {
        let s = Speed::new(3, 2).unwrap();
        assert_eq!(s.source(1), 2); // 1.5 -> 2
        assert_eq!(s.source(-1), -2);
        assert_eq!(s.source(2), 3);
        assert_eq!(Speed::ONE.source(-7), -7);
        assert_eq!(Speed::new(1, 3).unwrap().source(1), 0);
        assert!(Speed::new(0, 1).is_none());
    }

    #[test]
    fn longest_within_is_the_largest_span_that_fits() {
        for (num, den) in [(1, 1), (2, 1), (3, 2), (1, 3), (5, 7)] {
            let s = Speed::new(num, den).unwrap();
            for room in 0..50 {
                let len = s.longest_within(room);
                assert!(s.source(len) <= room, "{num}/{den} room {room}: {len}");
                assert!(s.source(len + 1) > room, "{num}/{den} room {room}: {len}");
            }
        }
    }

    #[test]
    fn a_span_knows_its_media_limits() {
        let s = ClipSpan {
            start: 10,
            len: 30,
            source_in: 210,
            speed: Speed::ONE,
            media_len: Some(250),
            still: false,
        };
        assert_eq!(s.head_min(), Some(-210));
        assert_eq!(s.tail_max(), Some(10));
        assert_eq!(s.slip_range(), Some((-210, 10)));
        let still = ClipSpan {
            media_len: None,
            still: true,
            ..s
        };
        assert_eq!((still.head_min(), still.tail_max()), (None, None));
        assert_eq!(still.slip_range(), None);
        assert_eq!(still.head(5, false).source_in, 210);
    }

    #[test]
    fn limits_clamp_toward_zero_on_the_grid_and_explain_strict_failures() {
        let grid = Grid {
            rate: FrameRate::FPS_25,
            samples: None,
            unit: SEC / 25,
        };
        let f = grid.unit;
        let mut l = Limits::new();
        l.at_most(10 * f + f / 2, || "the media ends".to_owned());
        l.at_least(-3 * f, || "time 0".to_owned());
        let a = l.resolve(4 * f, false, &grid, "/delta");
        assert_eq!(
            (a.delta, a.clamped, a.error.is_none()),
            (4 * f, false, true)
        );
        let a = l.resolve(20 * f, true, &grid, "/delta");
        assert_eq!((a.delta, a.clamped), (10 * f, true));
        let a = l.resolve(-9 * f, true, &grid, "/delta");
        assert_eq!((a.delta, a.clamped), (-3 * f, true));
        let a = l.resolve(20 * f, false, &grid, "/to");
        let err = a.error.unwrap();
        assert_eq!(err.code(), "invalid");
        let msg = err.to_string();
        assert!(
            msg.contains("+0.420s") && msg.contains("the media ends"),
            "{msg}"
        );
        let a = l.resolve(-9 * f, false, &grid, "/to");
        assert!(a.error.unwrap().to_string().contains("at least -3 frames"));
    }

    /// WP-2A.04 acceptance: at speed 2 on a 10 s source, 1,000 random trims map `source_in`
    /// and `source_out` exactly in flicks (no drift), never leave the media, and undo exactly.
    #[test]
    fn speed_two_trims_map_the_source_exactly() {
        let frame = SEC / 25;
        let speed = Speed::new(2, 1).unwrap();
        let mut s = ClipSpan {
            start: 0,
            len: 2 * SEC,
            source_in: 3 * SEC,
            speed,
            media_len: Some(10 * SEC),
            still: false,
        };
        let mut rng = 0x2A04_u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for n in 0..1000 {
            let start_edge = next() % 2 == 0;
            let frames = i64::try_from(next() % 61).unwrap() - 30;
            let grid = Grid {
                rate: FrameRate::FPS_25,
                samples: None,
                unit: frame,
            };
            let mut limits = Limits::new();
            let m = Member {
                track: TrackId::from_u128(1),
                id: ItemId::from_u128(1),
                span: s,
            };
            if start_edge {
                head_limits(&mut limits, &m, &grid);
            } else {
                tail_limits(&mut limits, &m, &grid);
            }
            let d = limits.resolve(frames * frame, true, &grid, "/delta").delta;
            let before = s;
            let out_before = s.source_out();
            s = if start_edge {
                s.head(d, false)
            } else {
                s.tail(d)
            };
            assert_eq!(s.source_out(), s.source_in + 2 * s.len, "trim {n}");
            if start_edge {
                assert_eq!(
                    s.source_out(),
                    out_before,
                    "trim {n}: the head keeps the source end"
                );
                assert_eq!(s.source_in, before.source_in + 2 * d, "trim {n}");
            } else {
                assert_eq!(s.source_in, before.source_in, "trim {n}");
            }
            assert!(
                s.source_in >= 0 && s.source_out() <= 10 * SEC,
                "trim {n}: {s:?}"
            );
            assert!(s.len >= frame, "trim {n}");
            let back = if start_edge {
                s.head(-d, false)
            } else {
                s.tail(-d)
            };
            assert_eq!(back, before, "trim {n}: trimming back restores the span");
        }
    }
}
