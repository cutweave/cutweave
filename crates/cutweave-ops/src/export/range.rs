// SPDX-License-Identifier: GPL-3.0-or-later
//! Which part of a sequence an export covers (WP-2G.03b; arch §8): the whole sequence, the
//! sequence's in/out points, a custom range, or one **region per marker**.
//!
//! `range` is `{kind, in, out, markers, pair_mode}`:
//!
//! | `kind` | Needs | Regions |
//! |---|---|---|
//! | `sequence` | nothing | the whole sequence |
//! | `in_out` | the sequence's in and out points (`timeline.set_in_out`) | `[in, out)`, the out point clamped to the sequence end |
//! | `custom` | `in` and `out` | `[in, out)` |
//! | `markers` | optional `markers` (ids; default: all the sequence's markers), optional `pair_mode` | one per marker, see below |
//!
//! With no `kind`: `markers` given means `markers`, `in` and `out` given means `custom`, and
//! an empty range is `sequence`; a range of only `{in, out}` is what it always was.
//!
//! **Marker regions.** `pair_mode: "duration"` (default): each marker with a `duration` is the
//! region `[t, t + duration)`; point markers are skipped when the markers are "all" and are an
//! error when named. `pair_mode: "pairs"`: the markers sorted by time form consecutive pairs,
//! the region of a marker runs to the next marker (so `n` markers give `n - 1` files; the last
//! marker only ends the one before it). Both ends snap to the nearest frame and the region is
//! clamped to the sequence end; a region shorter than a frame is an error. One export job per
//! region, in time order; the file names come from the path template (see `output`).

use cutweave_commands::{CmdError, wire};
use cutweave_model::{FrameRate, Marker, MarkerId, Rounding, Sequence, Time, TimeInput};

use crate::util::resolve_frame_time;

/// Which part of the sequence to export.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RangeKind {
    /// The whole sequence.
    Sequence,
    /// Between the sequence's in and out points.
    InOut,
    /// `in` to `out`.
    Custom,
    /// One file per marker.
    Markers,
}

/// How markers become regions.
#[wire]
#[derive(Copy, Default, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PairMode {
    /// Each marker's own `duration` (the default).
    #[default]
    Duration,
    /// Each marker to the next one.
    Pairs,
}

/// The part of the sequence to export: `[in, out)` snapped to frames, or a set of regions.
#[wire]
#[derive(Default)]
pub struct ExportRange {
    /// "sequence", "in_out", "custom" or "markers" (default: from the other fields).
    #[serde(default)]
    pub kind: Option<RangeKind>,
    /// Custom start: flicks, "2.5s", "f:60" or a timecode.
    #[serde(default, rename = "in", alias = "start")]
    pub start: Option<TimeInput>,
    /// Custom end, exclusive: at most the sequence end.
    #[serde(default, rename = "out", alias = "end")]
    pub end: Option<TimeInput>,
    /// Marker kind: the markers to export (default: all of the sequence's markers).
    #[serde(default)]
    pub markers: Option<Vec<MarkerId>>,
    /// Marker kind: "duration" (default; each marker's own duration) or "pairs" (each marker to
    /// the next).
    #[serde(default)]
    pub pair_mode: Option<PairMode>,
}

/// One part of the sequence, one export file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    /// Start on the sequence timeline (frame-aligned).
    pub start: Time,
    /// End, exclusive (frame-aligned).
    pub end: Time,
    /// The marker's label (marker regions only).
    pub label: Option<String>,
}

fn whole(seq: &Sequence) -> Result<Region, CmdError> {
    let end = Time::from(seq.duration());
    if end <= Time::ZERO {
        return Err(CmdError::invalid(
            "/sequence",
            "the sequence is empty: there is nothing to export",
        ));
    }
    Ok(Region {
        start: Time::ZERO,
        end,
        label: None,
    })
}

fn snap(pointer: &str, rate: FrameRate, t: Time) -> Result<Time, CmdError> {
    rate.snap_to_frame(t, Rounding::Nearest)
        .map_err(|e| CmdError::invalid(pointer, e.to_string()))
}

/// `[start, end)` as one region, checked against the sequence.
fn custom(
    seq: &Sequence,
    start: Time,
    end: Time,
    (p_in, p_out): (&str, &str),
) -> Result<Region, CmdError> {
    let seq_end = Time::from(seq.duration());
    if start < Time::ZERO {
        return Err(CmdError::invalid(p_in, "must not be negative"));
    }
    if end <= start {
        return Err(CmdError::invalid(
            p_out,
            format!("must be after `in` ({start}) by at least one frame"),
        ));
    }
    if end > seq_end {
        return Err(CmdError::invalid(
            p_out,
            format!("the sequence ends at {seq_end}"),
        ));
    }
    Ok(Region {
        start,
        end,
        label: None,
    })
}

fn reject(pointer: &str, present: bool, kind: &str) -> Result<(), CmdError> {
    if present {
        return Err(CmdError::invalid(
            pointer,
            format!("not used by range kind `{kind}`"),
        ));
    }
    Ok(())
}

/// The regions of `range` on `seq`: one, except for the `markers` kind.
///
/// # Errors
/// `Invalid` at the `/range/...` param at fault.
pub fn resolve_regions(
    range: Option<&ExportRange>,
    seq: &Sequence,
) -> Result<Vec<Region>, CmdError> {
    let rate = seq.format.frame_rate;
    let Some(range) = range else {
        return Ok(vec![whole(seq)?]);
    };
    let kind = range.kind.unwrap_or(if range.markers.is_some() {
        RangeKind::Markers
    } else if range.start.is_some() || range.end.is_some() {
        RangeKind::Custom
    } else {
        RangeKind::Sequence
    });
    let times = range.start.is_some() || range.end.is_some();
    match kind {
        RangeKind::Sequence => {
            reject("/range/in", times, "sequence")?;
            reject("/range/markers", range.markers.is_some(), "sequence")?;
            Ok(vec![whole(seq)?])
        }
        RangeKind::InOut => {
            reject("/range/in", times, "in_out")?;
            reject("/range/markers", range.markers.is_some(), "in_out")?;
            let Some(io) = &seq.in_out else {
                return Err(CmdError::invalid(
                    "/range/kind",
                    "the sequence has no in and out points; set them with timeline.set_in_out",
                ));
            };
            let seq_end = Time::from(seq.duration());
            let start = snap("/range/kind", rate, io.start)?;
            let end = snap("/range/kind", rate, io.end)?.min(seq_end);
            Ok(vec![custom(
                seq,
                start,
                end,
                ("/range/kind", "/range/kind"),
            )?])
        }
        RangeKind::Custom => {
            reject("/range/markers", range.markers.is_some(), "custom")?;
            let (Some(a), Some(b)) = (range.start, range.end) else {
                return Err(CmdError::invalid(
                    if range.start.is_some() {
                        "/range/out"
                    } else {
                        "/range/in"
                    },
                    "a custom range needs both `in` and `out`",
                ));
            };
            let start = resolve_frame_time("/range/in", a, rate)?;
            let end = resolve_frame_time("/range/out", b, rate)?;
            Ok(vec![custom(seq, start, end, ("/range/in", "/range/out"))?])
        }
        RangeKind::Markers => {
            reject("/range/in", times, "markers")?;
            marker_regions(range, seq)
        }
    }
}

fn marker_regions(range: &ExportRange, seq: &Sequence) -> Result<Vec<Region>, CmdError> {
    let rate = seq.format.frame_rate;
    let seq_end = Time::from(seq.duration());
    let named = range.markers.as_ref();
    let mut picked: Vec<&Marker> = match named {
        Some(ids) => {
            let mut v: Vec<&Marker> = Vec::with_capacity(ids.len());
            for (i, id) in ids.iter().enumerate() {
                let m = seq.markers.get(id).ok_or_else(|| {
                    CmdError::invalid(
                        format!("/range/markers/{i}"),
                        format!("the sequence has no marker `{id}`"),
                    )
                })?;
                if v.iter().any(|x| x.id == m.id) {
                    return Err(CmdError::invalid(
                        format!("/range/markers/{i}"),
                        format!("marker `{id}` is listed twice"),
                    ));
                }
                v.push(m);
            }
            v
        }
        None => seq.markers.values().collect(),
    };
    picked.sort_by_key(|m| (m.t, m.id));
    let mut regions = Vec::new();
    match range.pair_mode.unwrap_or_default() {
        PairMode::Duration => {
            for m in picked {
                let Some(d) = m.duration else {
                    if named.is_some() {
                        return Err(CmdError::invalid(
                            "/range/markers",
                            format!(
                                "marker `{}` has no duration; give it one, or use \
                                 `pair_mode: \"pairs\"`",
                                m.id
                            ),
                        ));
                    }
                    continue;
                };
                let start = snap("/range/markers", rate, m.t)?;
                let raw_end =
                    m.t.checked_add(d)
                        .map_err(|e| CmdError::invalid("/range/markers", e.to_string()))?;
                let end = snap("/range/markers", rate, raw_end)?.min(seq_end);
                regions.push(region_of(m, start, end)?);
            }
            if regions.is_empty() {
                return Err(CmdError::invalid(
                    "/range/markers",
                    "no marker has a duration, so there is nothing to export; give markers a \
                     duration or use `pair_mode: \"pairs\"`",
                ));
            }
        }
        PairMode::Pairs => {
            if picked.len() < 2 {
                return Err(CmdError::invalid(
                    "/range/markers",
                    "`pairs` needs at least two markers (each region runs to the next marker)",
                ));
            }
            for w in picked.windows(2) {
                let start = snap("/range/markers", rate, w[0].t)?;
                let end = snap("/range/markers", rate, w[1].t)?.min(seq_end);
                regions.push(region_of(w[0], start, end)?);
            }
        }
    }
    Ok(regions)
}

fn region_of(m: &Marker, start: Time, end: Time) -> Result<Region, CmdError> {
    if start >= end {
        return Err(CmdError::invalid(
            "/range/markers",
            format!(
                "marker `{}` covers less than one frame inside the sequence ({start} to {end})",
                m.id
            ),
        ));
    }
    Ok(Region {
        start,
        end,
        label: Some(m.label.clone()),
    })
}
