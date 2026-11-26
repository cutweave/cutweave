// SPDX-License-Identifier: GPL-3.0-or-later
//! Snapping (WP-2A.07, `docs/spec/timeline-edit-semantics.md` §6): a pure function that pulls a
//! raw time to the nearest snap candidate within a pixel threshold, and always lands on the
//! frame grid. The editor runs it through the wasm module (`cutweave-wasm`), the engine through
//! this crate, so both snap to exactly the same frame.
//!
//! A [`SnapIndex`] holds the candidates (already moved onto the frame grid) in one sorted array;
//! a query binary-searches the window around each probe, so it costs `O(log n + k)` for `k`
//! candidates inside the tolerance. Candidates of one kind are replaced as a whole
//! ([`SnapIndex::set_kind`]), which is how the editor keeps the index current when a document
//! patch changes only some kinds (a merge of sorted runs, no full rebuild).

use std::ops::{BitOr, BitOrAssign};

use crate::{FLICKS_PER_SECOND, FrameRate, Rounding, Time, TimeError};

/// What a snap candidate is. The discriminant is the rank used to break ties (spec §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum SnapKind {
    /// The playhead.
    Playhead = 0,
    /// The start and end of an item, and the timeline start.
    Edge = 1,
    /// A sequence or item marker, or an end of a range marker.
    Marker = 2,
    /// A marker of kind `beat`.
    Beat = 3,
    /// The sequence in or out point.
    InOut = 4,
}

impl SnapKind {
    /// Every kind, in rank order.
    pub const ALL: [SnapKind; 5] = [
        SnapKind::Playhead,
        SnapKind::Edge,
        SnapKind::Marker,
        SnapKind::Beat,
        SnapKind::InOut,
    ];

    /// The kind with this rank (the wire code), if there is one.
    #[must_use]
    pub const fn from_code(code: u32) -> Option<SnapKind> {
        match code {
            0 => Some(SnapKind::Playhead),
            1 => Some(SnapKind::Edge),
            2 => Some(SnapKind::Marker),
            3 => Some(SnapKind::Beat),
            4 => Some(SnapKind::InOut),
            _ => None,
        }
    }

    /// The rank, which is also the wire code.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Lowercase name, as the fixtures spell it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            SnapKind::Playhead => "playhead",
            SnapKind::Edge => "edge",
            SnapKind::Marker => "marker",
            SnapKind::Beat => "beat",
            SnapKind::InOut => "in_out",
        }
    }

    /// The kind with this name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<SnapKind> {
        SnapKind::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// A set of [`SnapKind`]s (a bit per rank).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SnapKinds(u8);

impl SnapKinds {
    /// No kind: snapping off (or the hold-modifier override): the result is the frame snap.
    pub const NONE: SnapKinds = SnapKinds(0);
    /// Every kind.
    pub const ALL: SnapKinds = SnapKinds(0b1_1111);

    /// The set of one kind.
    #[must_use]
    pub const fn only(kind: SnapKind) -> SnapKinds {
        SnapKinds(1 << kind.code())
    }

    /// The set from its bits (bit `n` is the kind of rank `n`); unknown bits are dropped.
    #[must_use]
    pub const fn from_bits(bits: u32) -> SnapKinds {
        SnapKinds((bits & 0b1_1111) as u8)
    }

    /// The bits (bit `n` is the kind of rank `n`).
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0 as u32
    }

    /// Whether `kind` is in the set.
    #[must_use]
    pub const fn contains(self, kind: SnapKind) -> bool {
        self.0 & (1 << kind.code()) != 0
    }

    /// This set without `kind`.
    #[must_use]
    pub const fn without(self, kind: SnapKind) -> SnapKinds {
        SnapKinds(self.0 & !(1 << kind.code()))
    }
}

impl BitOr for SnapKinds {
    type Output = SnapKinds;
    fn bitor(self, rhs: SnapKinds) -> SnapKinds {
        SnapKinds(self.0 | rhs.0)
    }
}

impl BitOrAssign for SnapKinds {
    fn bitor_assign(&mut self, rhs: SnapKinds) {
        self.0 |= rhs.0;
    }
}

impl FromIterator<SnapKind> for SnapKinds {
    fn from_iter<I: IntoIterator<Item = SnapKind>>(iter: I) -> SnapKinds {
        iter.into_iter()
            .fold(SnapKinds::NONE, |set, k| set | SnapKinds::only(k))
    }
}

/// A snap candidate: a kind and a time on the frame grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapTarget {
    /// What the candidate is.
    pub kind: SnapKind,
    /// Where it is, in flicks (frame-aligned).
    pub t: i64,
}

/// A failed snap.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum SnapError {
    /// A time left the safe range.
    #[error(transparent)]
    Time(#[from] TimeError),
    /// `px_per_second` must be finite and positive.
    #[error("px_per_second must be finite and positive, got {0}")]
    BadZoom(f64),
    /// `threshold_px` must be finite and not negative.
    #[error("threshold_px must be finite and not negative, got {0}")]
    BadThreshold(f64),
}

/// How a snap is asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapOptions {
    /// The zoom: CSS pixels per second of timeline.
    pub px_per_second: f64,
    /// The snap distance in pixels (8 by default, the Timeline preference "Snap distance").
    pub threshold_px: f64,
    /// The kinds that may attract. [`SnapKinds::NONE`] is snapping off or Ctrl/Cmd held.
    pub enabled: SnapKinds,
}

impl SnapOptions {
    /// The snap distance when the preference is untouched.
    pub const DEFAULT_THRESHOLD_PX: f64 = 8.0;

    /// Every kind enabled, at the default distance.
    #[must_use]
    pub const fn new(px_per_second: f64) -> SnapOptions {
        SnapOptions {
            px_per_second,
            threshold_px: SnapOptions::DEFAULT_THRESHOLD_PX,
            enabled: SnapKinds::ALL,
        }
    }

    /// The tolerance in flicks (INV-323: it halves when the zoom doubles).
    pub fn tolerance(&self) -> Result<f64, SnapError> {
        if !(self.px_per_second.is_finite() && self.px_per_second > 0.0) {
            return Err(SnapError::BadZoom(self.px_per_second));
        }
        if !(self.threshold_px.is_finite() && self.threshold_px >= 0.0) {
            return Err(SnapError::BadThreshold(self.threshold_px));
        }
        #[allow(clippy::cast_precision_loss)] // 705 600 000 is exact in f64.
        Ok(self.threshold_px * FLICKS_PER_SECOND as f64 / self.px_per_second)
    }
}

/// The outcome of a snap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapped {
    /// The snapped raw time, in flicks, on the frame grid.
    pub t: i64,
    /// The candidate the winning probe landed on (draw a guide there), if any.
    pub target: Option<SnapTarget>,
    /// The index of the winning probe offset (0 without a target).
    pub probe: usize,
}

/// The snap candidates of a sequence, sorted by time, on the sequence's frame grid.
#[derive(Clone, Debug)]
pub struct SnapIndex {
    rate: FrameRate,
    /// Sorted by `(t, rank)`.
    items: Vec<SnapTarget>,
}

impl SnapIndex {
    /// An index of `candidates` (any order; each time is moved to the nearest frame, ties later).
    pub fn new(
        rate: FrameRate,
        candidates: impl IntoIterator<Item = (SnapKind, i64)>,
    ) -> Result<SnapIndex, SnapError> {
        let mut items = candidates
            .into_iter()
            .map(|(kind, t)| {
                Ok(SnapTarget {
                    kind,
                    t: grid(rate, t)?,
                })
            })
            .collect::<Result<Vec<_>, SnapError>>()?;
        items.sort_unstable_by_key(|c| (c.t, c.kind));
        Ok(SnapIndex { rate, items })
    }

    /// The frame rate whose grid the candidates and results are on.
    #[must_use]
    pub fn rate(&self) -> FrameRate {
        self.rate
    }

    /// How many candidates there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The candidates, sorted by time.
    #[must_use]
    pub fn targets(&self) -> &[SnapTarget] {
        &self.items
    }

    /// Replaces every candidate of `kind` with `times` (any order): sorts the new run, drops
    /// the old one and merges, `O(n + m log m)`. On error the index is unchanged.
    pub fn set_kind(
        &mut self,
        kind: SnapKind,
        times: impl IntoIterator<Item = i64>,
    ) -> Result<(), SnapError> {
        let mut fresh = times
            .into_iter()
            .map(|t| {
                Ok(SnapTarget {
                    kind,
                    t: grid(self.rate, t)?,
                })
            })
            .collect::<Result<Vec<_>, SnapError>>()?;
        fresh.sort_unstable_by_key(|c| c.t);
        let mut merged = Vec::with_capacity(self.items.len() + fresh.len());
        let mut fresh = fresh.into_iter().peekable();
        for old in self.items.iter().filter(|c| c.kind != kind) {
            while let Some(c) = fresh.next_if(|c| (c.t, c.kind) < (old.t, old.kind)) {
                merged.push(c);
            }
            merged.push(*old);
        }
        merged.extend(fresh);
        self.items = merged;
        Ok(())
    }

    /// Snaps a time. See [`snap`].
    pub fn snap(
        &self,
        r: i64,
        offsets: &[i64],
        options: &SnapOptions,
    ) -> Result<Snapped, SnapError> {
        snap(self, r, offsets, options)
    }
}

/// `t` on the frame grid (nearest, ties later).
fn grid(rate: FrameRate, t: i64) -> Result<i64, SnapError> {
    Ok(rate
        .snap_to_frame(Time::from_flicks(t)?, Rounding::Nearest)?
        .flicks())
}

/// Snaps the raw time `r` (flicks) to the candidates of `index` (spec §6).
///
/// `offsets` are the probes, relative to `r` (`[0]` for an edge, the playhead or a marker;
/// `[0, duration]` for a clip; every edge of every moving item for a multi-item move). The pair
/// of a probe `oᵢ` and a candidate `c` is eligible when its kind is enabled, `|c − (r + oᵢ)|` is
/// within the tolerance and `c − oᵢ >= 0`. The nearest pair wins; ties go to the higher-ranked
/// kind, then the earlier probe, then the earlier time. The result is `c − oᵢ`; without an
/// eligible pair it is `r` on the nearest frame. It is always on the frame grid (INV-321).
pub fn snap(
    index: &SnapIndex,
    r: i64,
    offsets: &[i64],
    options: &SnapOptions,
) -> Result<Snapped, SnapError> {
    let tolerance = options.tolerance()?;
    let plain = grid(index.rate, r)?;
    let mut best: Option<((i64, SnapKind, usize, i64), SnapTarget, i64)> = None;
    if options.enabled != SnapKinds::NONE {
        // A distance is a whole number of flicks, so `d <= tolerance` is `d <= floor(tolerance)`.
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let reach = if tolerance >= i64::MAX as f64 {
            i64::MAX
        } else {
            tolerance.floor() as i64
        };
        for (i, &o) in offsets.iter().enumerate() {
            let probe = r.saturating_add(o);
            let lo = probe.saturating_sub(reach);
            let hi = probe.saturating_add(reach);
            let from = index.items.partition_point(|c| c.t < lo);
            for c in index.items[from..].iter().take_while(|c| c.t <= hi) {
                if !options.enabled.contains(c.kind) || c.t.saturating_sub(o) < 0 {
                    continue;
                }
                let key = ((c.t - probe).abs(), c.kind, i, c.t);
                if best.as_ref().is_none_or(|(k, ..)| key < *k) {
                    best = Some((key, *c, o));
                }
            }
        }
    }
    Ok(match best {
        Some(((_, _, probe, _), target, o)) => Snapped {
            // Whole-frame probes keep this on the grid; at an inexact rate the difference of two
            // frame starts may not be, so it is moved there (INV-321 comes first).
            t: grid(index.rate, target.t - o)?,
            target: Some(target),
            probe,
        },
        None => Snapped {
            t: plain,
            target: None,
            probe: 0,
        },
    })
}

#[cfg(test)]
mod tests;
