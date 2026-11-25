// SPDX-License-Identifier: GPL-3.0-or-later
//! Markers and the in/out range (WP-2A.02; timeline spec §1.5 and INV-34; ADR-0023):
//! `timeline.add_marker`, `timeline.move_marker`, `timeline.remove_marker`,
//! `timeline.set_in_out`, `timeline.clear_in_out` and the `timeline.list_markers` query.
//!
//! A marker belongs to a sequence (sequence time) or to an item (item-relative time, so it
//! moves with the item). Markers and the in/out range are metadata: no command here touches an
//! item's timing, and a ripple never moves a sequence marker or the in/out range (INV-34).
//! Marker times snap to the sequence's frame grid like every other time param (INV-24).
//! Markers on an item are an edit of that item, so they fail with `Locked` on a locked track
//! (INV-27); sequence markers and the in/out range never do.

use cutweave_commands::{
    CmdError, Command, Query, QueryCtx, Registry, RegistryError, Tx, command, query, wire,
};
use cutweave_model::{
    Color, Duration, EntityKind, InOut, ItemId, Marker, MarkerId, MarkerKind, Project, Sequence,
    SequenceId, Time, TimeInput,
};

use crate::util::{resolve_frame_duration, resolve_frame_time};

/// Longest accepted marker label, in characters.
pub const MAX_LABEL_CHARS: usize = 200;

/// Registers the marker and in/out commands and the `timeline.list_markers` query.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<TimelineAddMarker>()?
        .register::<TimelineMoveMarker>()?
        .register::<TimelineRemoveMarker>()?
        .register::<TimelineSetInOut>()?
        .register::<TimelineClearInOut>()?
        .register::<TimelineListMarkers>()?;
    Ok(())
}

/// A label of at most [`MAX_LABEL_CHARS`] characters without control characters; may be
/// empty. Surrounding white space is trimmed.
fn check_label(path: &str, label: &str) -> Result<String, CmdError> {
    let label = label.trim();
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(CmdError::invalid(
            path,
            format!("the label must be at most {MAX_LABEL_CHARS} characters"),
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(CmdError::invalid(
            path,
            "the label must not contain control characters",
        ));
    }
    Ok(label.to_owned())
}

/// A marker time: snapped to the frame grid, at least 0.
fn marker_time(path: &str, input: TimeInput, sequence: &Sequence) -> Result<Time, CmdError> {
    let t = resolve_frame_time(path, input, sequence.format.frame_rate)?;
    if t < Time::ZERO {
        return Err(CmdError::invalid(path, "a marker cannot be before time 0"));
    }
    Ok(t)
}

/// Where a marker lives.
#[derive(Clone, Copy, Debug)]
enum Owner {
    Sequence,
    Item(ItemId),
}

fn find_marker(doc: &Project, id: MarkerId) -> Result<(&Sequence, Owner, &Marker), CmdError> {
    for sequence in doc.sequences.values() {
        if let Some(m) = sequence.markers.get(&id) {
            return Ok((sequence, Owner::Sequence, m));
        }
        for track in sequence.tracks.values() {
            for item in track.items.iter() {
                if let Some(m) = item.markers().get(&id) {
                    return Ok((sequence, Owner::Item(item.id()), m));
                }
            }
        }
    }
    Err(CmdError::not_found(EntityKind::Marker, id))
}

// ----- timeline.add_marker -------------------------------------------------------------------

/// Result of `timeline.add_marker`.
#[wire]
pub struct TimelineAddMarkerOutput {
    /// The new marker.
    pub marker: MarkerId,
    /// The sequence it is in.
    pub sequence: SequenceId,
    /// The item it is on, for an item marker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<ItemId>,
    /// Its time, snapped to the frame grid (item-relative for an item marker).
    pub t: Time,
    /// Its length, for a range marker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<Duration>,
}

/// Adds a marker to a sequence, or to an item of it when `item` is given (then `t` is relative
/// to the item's start and the marker moves with it). `duration` makes a range marker. `kind`
/// is `user` (default), `beat` or `chapter`; `color` is `#rrggbb`. Times snap to the frame
/// grid. Sequence markers never move with a ripple; an item marker on a locked track fails
/// with `Locked`.
#[command(
    name = "timeline.add_marker",
    summary = "Add a marker (or range marker) to a sequence or one of its items",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = false)
)]
pub struct TimelineAddMarker {
    /// The sequence.
    pub sequence: SequenceId,
    /// Put the marker on this item of the sequence instead of on the sequence.
    pub item: Option<ItemId>,
    /// Where, `>= 0` (snapped to the frame grid; relative to the item for an item marker).
    pub t: TimeInput,
    /// Length of a range marker, `> 0` (snapped to whole frames).
    pub duration: Option<TimeInput>,
    /// Text shown with the marker (default empty), at most 200 characters.
    pub label: Option<String>,
    /// Display color, `#rrggbb` (default marigold).
    pub color: Option<Color>,
    /// `user` (default), `beat` or `chapter`.
    pub kind: Option<MarkerKind>,
}

impl TimelineAddMarker {
    fn build(&self, doc: &Project, id: MarkerId) -> Result<Marker, CmdError> {
        let sequence = doc
            .sequences
            .get(&self.sequence)
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, self.sequence))?;
        if let Some(item) = self.item {
            let at = doc
                .find_item(&item)
                .ok_or_else(|| CmdError::not_found(EntityKind::Item, item))?;
            if at.sequence.id != self.sequence {
                return Err(CmdError::invalid(
                    "/item",
                    format!("`{item}` is not an item of sequence `{}`", self.sequence),
                ));
            }
        }
        let label = self
            .label
            .as_deref()
            .map(|l| check_label("/label", l))
            .transpose()?
            .unwrap_or_default();
        let t = marker_time("/t", self.t, sequence)?;
        let duration = self
            .duration
            .map(|d| resolve_frame_duration("/duration", d, sequence.format.frame_rate))
            .transpose()?;
        if let Some(d) = duration
            && (t + d).is_err()
        {
            return Err(CmdError::invalid(
                "/duration",
                "the marker would end past the largest representable time",
            ));
        }
        let mut marker = Marker::new(id, t);
        marker.duration = duration;
        marker.label = label;
        marker.color = self.color.unwrap_or(Marker::DEFAULT_COLOR);
        marker.kind = self.kind.unwrap_or_default();
        Ok(marker)
    }
}

impl Command for TimelineAddMarker {
    type Output = TimelineAddMarkerOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.build(ctx.doc(), MarkerId::from_u128(0)).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TimelineAddMarkerOutput, CmdError> {
        let id = tx.new_id();
        let marker = self.build(tx.doc(), id)?;
        let out = TimelineAddMarkerOutput {
            marker: marker.id,
            sequence: self.sequence,
            item: self.item,
            t: marker.t,
            duration: marker.duration,
        };
        match self.item {
            Some(item) => tx.update_item(item, |i| {
                i.markers_mut().insert(marker.id, marker);
                Ok(())
            })?,
            None => tx.update_sequence(self.sequence, |s| {
                s.markers.insert(marker.id, marker);
                Ok(())
            })?,
        }
        Ok(out)
    }
}

// ----- timeline.move_marker ------------------------------------------------------------------

/// Result of `timeline.move_marker`.
#[wire]
pub struct TimelineMoveMarkerOutput {
    /// The marker.
    pub marker: MarkerId,
    /// Its time now, snapped to the frame grid.
    pub t: Time,
    /// Where it was.
    pub previous: Time,
}

/// Moves a marker to another time (snapped to the frame grid; relative to the item for an
/// item marker). Its label, color, kind and length stay.
#[command(
    name = "timeline.move_marker",
    summary = "Move a marker to another time",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct TimelineMoveMarker {
    /// The marker (on a sequence or on an item).
    pub marker: MarkerId,
    /// The new time, `>= 0`.
    pub t: TimeInput,
}

impl TimelineMoveMarker {
    fn plan(&self, doc: &Project) -> Result<(Owner, SequenceId, Time, Time), CmdError> {
        let (sequence, owner, marker) = find_marker(doc, self.marker)?;
        let t = marker_time("/t", self.t, sequence)?;
        if let Some(d) = marker.duration
            && (t + d).is_err()
        {
            return Err(CmdError::invalid(
                "/t",
                "the marker would end past the largest representable time",
            ));
        }
        Ok((owner, sequence.id, t, marker.t))
    }
}

impl Command for TimelineMoveMarker {
    type Output = TimelineMoveMarkerOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TimelineMoveMarkerOutput, CmdError> {
        let (owner, sequence, t, previous) = self.plan(tx.doc())?;
        if t != previous {
            let id = self.marker;
            match owner {
                Owner::Sequence => tx.update_sequence(sequence, |s| {
                    if let Some(m) = s.markers.get_mut(&id) {
                        m.t = t;
                    }
                    Ok(())
                })?,
                Owner::Item(item) => tx.update_item(item, |i| {
                    if let Some(m) = i.markers_mut().get_mut(&id) {
                        m.t = t;
                    }
                    Ok(())
                })?,
            }
        }
        Ok(TimelineMoveMarkerOutput {
            marker: self.marker,
            t,
            previous,
        })
    }
}

// ----- timeline.remove_marker ----------------------------------------------------------------

/// Result of `timeline.remove_marker`.
#[wire]
#[derive(Copy)]
pub struct TimelineRemoveMarkerOutput {
    /// The removed marker.
    pub marker: MarkerId,
}

/// Removes a marker.
#[command(
    name = "timeline.remove_marker",
    summary = "Remove a marker",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct TimelineRemoveMarker {
    /// The marker (on a sequence or on an item).
    pub marker: MarkerId,
}

impl Command for TimelineRemoveMarker {
    type Output = TimelineRemoveMarkerOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        find_marker(ctx.doc(), self.marker).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TimelineRemoveMarkerOutput, CmdError> {
        let (sequence, owner, _) = find_marker(tx.doc(), self.marker)?;
        let sequence = sequence.id;
        let id = self.marker;
        match owner {
            Owner::Sequence => tx.update_sequence(sequence, |s| {
                s.markers.remove(&id);
                Ok(())
            })?,
            Owner::Item(item) => tx.update_item(item, |i| {
                i.markers_mut().remove(&id);
                Ok(())
            })?,
        }
        Ok(TimelineRemoveMarkerOutput { marker: id })
    }
}

// ----- timeline.set_in_out / clear_in_out ----------------------------------------------------

/// Result of `timeline.set_in_out`.
#[wire]
pub struct TimelineSetInOutOutput {
    /// The sequence.
    pub sequence: SequenceId,
    /// The in point, snapped to the frame grid.
    pub start: Time,
    /// The out point, snapped to the frame grid.
    pub end: Time,
}

/// Sets the sequence's in and out points: the half-open range `[start, end)` that lift,
/// extract and three-point edits act on. Both ends are given (a range always has both). Times
/// snap to the frame grid and `end` must follow `start`. The range never moves with a ripple.
#[command(
    name = "timeline.set_in_out",
    summary = "Set the sequence's in and out points",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct TimelineSetInOut {
    /// The sequence.
    pub sequence: SequenceId,
    /// The in point, `>= 0`.
    pub start: TimeInput,
    /// The out point, after the in point.
    pub end: TimeInput,
}

impl TimelineSetInOut {
    fn plan(&self, doc: &Project) -> Result<(Time, Time), CmdError> {
        let sequence = doc
            .sequences
            .get(&self.sequence)
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, self.sequence))?;
        let start = marker_time("/start", self.start, sequence)?;
        let end = marker_time("/end", self.end, sequence)?;
        if end <= start {
            return Err(CmdError::invalid(
                "/end",
                "the out point must be after the in point",
            ));
        }
        Ok((start, end))
    }
}

impl Command for TimelineSetInOut {
    type Output = TimelineSetInOutOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TimelineSetInOutOutput, CmdError> {
        let (start, end) = self.plan(tx.doc())?;
        tx.update_sequence(self.sequence, |s| {
            // Keep unknown fields of an existing range.
            match &mut s.in_out {
                Some(range) => {
                    range.start = start;
                    range.end = end;
                }
                None => s.in_out = Some(InOut::new(start, end)),
            }
            Ok(())
        })?;
        Ok(TimelineSetInOutOutput {
            sequence: self.sequence,
            start,
            end,
        })
    }
}

/// Result of `timeline.clear_in_out`.
#[wire]
#[derive(Copy)]
pub struct TimelineClearInOutOutput {
    /// The sequence.
    pub sequence: SequenceId,
    /// Whether there was a range to clear.
    pub cleared: bool,
}

/// Clears the sequence's in and out points (nothing happens if none are set).
#[command(
    name = "timeline.clear_in_out",
    summary = "Clear the sequence's in and out points",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct TimelineClearInOut {
    /// The sequence.
    pub sequence: SequenceId,
}

impl Command for TimelineClearInOut {
    type Output = TimelineClearInOutOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        ctx.sequence(self.sequence).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TimelineClearInOutOutput, CmdError> {
        let had = tx.sequence(self.sequence)?.in_out.is_some();
        if had {
            tx.update_sequence(self.sequence, |s| {
                s.in_out = None;
                Ok(())
            })?;
        }
        Ok(TimelineClearInOutOutput {
            sequence: self.sequence,
            cleared: had,
        })
    }
}

// ----- marker.list ---------------------------------------------------------------------------

/// Result of `timeline.list_markers`.
#[wire]
pub struct TimelineListMarkersOutput {
    /// The sequence.
    pub sequence: SequenceId,
    /// The item whose markers these are; absent for the sequence's own markers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<ItemId>,
    /// The markers by time, then ID.
    pub markers: Vec<Marker>,
    /// The sequence's in and out points (sequence markers only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_out: Option<InOut>,
}

/// Lists a sequence's markers and its in/out range, or the markers of one item.
#[query(
    name = "timeline.list_markers",
    summary = "List the markers of a sequence (with its in/out range) or of one item",
    mcp = "catalog"
)]
pub struct TimelineListMarkers {
    /// The sequence.
    pub sequence: SequenceId,
    /// List this item's markers (item-relative times) instead of the sequence's.
    pub item: Option<ItemId>,
}

impl Query for TimelineListMarkers {
    type Output = TimelineListMarkersOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<TimelineListMarkersOutput, CmdError> {
        let sequence = ctx.sequence(self.sequence)?;
        let (markers, in_out) = match self.item {
            None => (&sequence.markers, sequence.in_out.clone()),
            Some(item) => {
                let at = ctx.item(item)?;
                if at.sequence.id != sequence.id {
                    return Err(CmdError::invalid(
                        "/item",
                        format!("`{item}` is not an item of sequence `{}`", sequence.id),
                    ));
                }
                (at.item.markers(), None)
            }
        };
        let mut markers: Vec<Marker> = markers.values().cloned().collect();
        markers.sort_by_key(|m| (m.t, m.id));
        Ok(TimelineListMarkersOutput {
            sequence: sequence.id,
            item: self.item,
            markers,
            in_out,
        })
    }
}
