// SPDX-License-Identifier: GPL-3.0-or-later
//! Re-snapping every item of a sequence to a new frame grid (WP-2B.01).
//!
//! # Algorithm
//!
//! Every item has two edges, its start and its end. Collect the distinct edge times of the
//! whole sequence (all tracks) and sort them: `t0 < t1 < ...`. Each time gets a frame number of
//! the new rate, in order:
//!
//! ```text
//! f(t0) = nearest_frame(t0)
//! f(ti) = max(nearest_frame(ti), f(t(i-1)) + 1)
//! ```
//!
//! The map is strictly increasing, so it keeps what the old grid had:
//!
//! * equal edges stay equal: no overlap appears, adjacent items stay adjacent, a magnetic track
//!   stays gap-free from 0, linked partners (same start and length) stay aligned, an attached
//!   child starts inside its parent;
//! * different edges stay different: every item keeps at least one frame.
//!
//! Where the new rate has longer frames than the old (30 to 24 fps) rounding can bring two
//! edges onto one frame; the `+ 1` then pushes the later edge, and everything after it, by one
//! frame. That only happens for items shorter than a new frame (or edges off the old grid),
//! and the total drift is the number of such collisions.
//!
//! A clip must not read past the end of its media (INV-25): when the rounded end would, the
//! edge is lowered to the last whole frame that fits (never below the previous edge plus one).
//! A clip with less than one new frame of media left cannot be kept and fails the command.
//!
//! Keyframe times, item markers, sequence markers and the in/out range are not on the grid by
//! rule (keyframes spec §3.2) and stay as they are.

use std::collections::{BTreeMap, BTreeSet};

use cutweave_commands::CmdError;
use cutweave_model::{
    Duration, FrameRate, Item, ItemId, MediaKind, Project, Rounding, Sequence, Time,
};

/// The new times of every item of a sequence, by item.
pub struct Regrid {
    /// `(start, duration)` per item, in flicks.
    pub items: BTreeMap<ItemId, (Time, Duration)>,
}

fn invalid(msg: impl Into<String>) -> CmdError {
    CmdError::invalid("/frame_rate", msg)
}

fn time(f: i64) -> Result<Time, CmdError> {
    Time::from_flicks(f).map_err(|e| invalid(e.to_string()))
}

/// Plans the new `(start, duration)` of every item of `sequence` at `rate`.
pub fn plan(doc: &Project, sequence: &Sequence, rate: FrameRate) -> Result<Regrid, CmdError> {
    let fpf = rate
        .flicks_per_frame()
        .ok_or_else(|| invalid("the frame rate is not a whole number of flicks per frame"))?;
    // Every item with its edges in the old time.
    let mut edges = BTreeSet::new();
    let mut clips = Vec::new();
    for track in sequence.tracks.values() {
        for item in track.items.iter() {
            let (s, e) = (item.start().flicks(), end_of(item)?);
            edges.insert(s);
            edges.insert(e);
            clips.push((item, s, e));
        }
    }
    // The media room of each clip: how long it may be, from its source position.
    let room = |item: &Item| -> Option<i64> {
        let clip = item.as_clip()?;
        let media = doc.media.get(&clip.media)?;
        if media.kind == MediaKind::Image {
            return None;
        }
        let total = media.probe.duration?.flicks();
        Some((total - clip.source_in.flicks()).max(0))
    };
    // Items ending at each edge, for the media caps.
    let mut enders: BTreeMap<i64, Vec<(i64, i64)>> = BTreeMap::new();
    for (item, s, e) in &clips {
        if let Some(r) = room(item) {
            enders.entry(*e).or_default().push((*s, r));
        }
    }
    let mut frame_of: BTreeMap<i64, i64> = BTreeMap::new();
    let mut previous: Option<i64> = None;
    for t in edges {
        let nearest = rate.time_to_frame(time(t)?, Rounding::Nearest);
        let mut f = previous.map_or(nearest, |p| nearest.max(p + 1));
        // Never past the media: lower the edge to the last frame that fits.
        for (start, room) in enders.get(&t).into_iter().flatten() {
            let start_frame = frame_of[start];
            let cap = start_frame + room.div_euclid(fpf);
            if f > cap {
                let floor = previous
                    .map_or(start_frame + 1, |p| p + 1)
                    .max(start_frame + 1);
                if cap < floor {
                    return Err(invalid(format!(
                        "a clip would read past the end of its media at {rate} fps; trim it by a \
                         frame first"
                    )));
                }
                f = cap;
            }
        }
        frame_of.insert(t, f);
        previous = Some(f);
    }
    let at = |frame: i64| -> Result<i64, CmdError> {
        rate.frame_to_time(frame)
            .map(Time::flicks)
            .map_err(|e| invalid(e.to_string()))
    };
    let mut items = BTreeMap::new();
    for (item, s, e) in clips {
        let (new_s, new_e) = (at(frame_of[&s])?, at(frame_of[&e])?);
        let duration = Duration::from_flicks(new_e - new_s).map_err(|e| invalid(e.to_string()))?;
        items.insert(item.id(), (time(new_s)?, duration));
    }
    Ok(Regrid { items })
}

fn end_of(item: &Item) -> Result<i64, CmdError> {
    i64::try_from(i128::from(item.start().flicks()) + i128::from(item.duration().flicks()))
        .map_err(|_| invalid("an item ends past the largest representable time"))
}
