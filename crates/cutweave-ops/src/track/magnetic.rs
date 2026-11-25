// SPDX-License-Identifier: GPL-3.0-or-later
//! `track.set_magnetic` (timeline spec §4.17, INV-6, INV-7, INV-281; ADR-0023): make a video
//! track the sequence's main (magnetic) track, or turn the magnet off.
//!
//! Making a track magnetic needs it to be gap-free from 0. With `close_gaps: true` the command
//! closes every gap itself, left to right, the way `timeline.delete_gap` does: a ripple with
//! the gap's end as threshold and the gap's width as shift, on the track and on every track
//! that holds a linked partner of an item that shifts (INV-30, INV-31); children attached to
//! shifted items follow them (INV-36). The ripple is the placement core's
//! ([`crate::timeline::placement`], WP-2A.03), and so are the lock and collision judgements:
//! `Locked` for a locked track the closing would change, `Invalid` for items it would make
//! overlap (INV-32). `validate` and `apply` share the plan (INV-20).

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{EntityKind, Project, Sequence, Track, TrackId, TrackKind};

use crate::{
    report::EditReport,
    timeline::{
        magnetic::close_gaps,
        placement::{DryIds, Edit, Ids, Plan, first_gap},
    },
};

/// Result of `track.set_magnetic`.
#[wire]
pub struct TrackSetMagneticOutput {
    /// The track.
    pub track: TrackId,
    /// Whether it is the main track now.
    pub magnetic: bool,
    /// The track that was the main track and no longer is, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<TrackId>,
    /// What changed (items moved by closing gaps).
    pub report: EditReport,
}

/// Makes a video track the main (magnetic) track, or turns the magnet off. Making a track the
/// main track clears the flag of the previous one in the same step. A track with gaps (or that
/// does not start at 0) cannot become the main track unless `close_gaps` is set, which closes
/// every gap from left to right; linked partners ripple with their items. Turning the magnet
/// off only clears the flag and also works on a locked track. Closing gaps on a locked track,
/// or on a track the ripple reaches, fails with `Locked`.
#[command(
    name = "track.set_magnetic",
    summary = "Make a video track the magnetic main track (gap-free from 0), or turn the magnet off",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct TrackSetMagnetic {
    /// The track.
    pub track: TrackId,
    /// `true` makes it the main track, `false` clears its flag.
    pub magnetic: bool,
    /// Close gaps (and the space before the first item) when making the track magnetic,
    /// instead of failing (default `false`).
    pub close_gaps: Option<bool>,
}

fn find_track(doc: &Project, id: TrackId) -> Result<(&Sequence, &Track), CmdError> {
    doc.sequences
        .values()
        .find_map(|s| s.tracks.get(&id).map(|t| (s, t)))
        .ok_or_else(|| CmdError::not_found(EntityKind::Track, id))
}

/// The sequence after the command, and the track that lost its flag.
fn plan(
    cmd: &TrackSetMagnetic,
    doc: &Project,
    ids: &mut dyn Ids,
) -> Result<(Plan, Option<TrackId>), CmdError> {
    let (sequence, track) = find_track(doc, cmd.track)?;
    let mut e = Edit::new(doc, sequence.id, "/track")?;
    if !cmd.magnetic {
        if let Some(t) = e.seq.track_mut(&track.id) {
            t.magnetic = false;
        }
        return Ok((e.finish(ids)?, None));
    }
    if track.kind != TrackKind::Video {
        return Err(CmdError::invalid(
            "/track",
            format!(
                "only a video track can be the main track; `{}` is a {} track",
                track.name, track.kind
            ),
        ));
    }
    let previous = sequence
        .tracks
        .values()
        .find(|t| t.magnetic && t.id != track.id)
        .map(|t| t.id);
    if let Some((from, to)) = first_gap(track)
        && !cmd.close_gaps.unwrap_or(false)
    {
        let rate = sequence.format.frame_rate;
        let describe = |t: i64| {
            cutweave_model::Time::from_flicks(t).ok().map_or_else(
                || t.to_string(),
                |t| {
                    format!(
                        "frame {}",
                        rate.time_to_frame(t, cutweave_model::Rounding::Nearest)
                    )
                },
            )
        };
        return Err(CmdError::invalid(
            "/track",
            format!(
                "track `{}` has a gap from {} to {}; a main track is gap-free from 0. \
                 Pass close_gaps to close it",
                track.name,
                describe(from),
                describe(to)
            ),
        ));
    }
    // The previous main track becomes free first, so the closing ripple may move it; the flag
    // goes on afterwards, so `finish` checks that the new main track is gap-free.
    if let Some(p) = previous
        && let Some(t) = e.seq.track_mut(&p)
    {
        t.magnetic = false;
    }
    close_gaps(&mut e, track.id);
    if let Some(t) = e.seq.track_mut(&track.id) {
        t.magnetic = true;
    }
    Ok((e.finish(ids)?, previous))
}

impl Command for TrackSetMagnetic {
    type Output = TrackSetMagneticOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TrackSetMagneticOutput, CmdError> {
        let doc = tx.doc().clone();
        let (plan, previous) = plan(self, &doc, tx)?;
        let report = plan.commit(tx)?;
        Ok(TrackSetMagneticOutput {
            track: self.track,
            magnetic: self.magnetic,
            previous,
            report,
        })
    }
}
