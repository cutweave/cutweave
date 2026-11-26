// SPDX-License-Identifier: GPL-3.0-or-later
//! `clip.replace` (WP-2A.11; timeline spec INV-8, INV-24, INV-25, INV-47): swaps the media of a
//! clip and keeps everything done to it: its ID, position, name, enabled state, transform,
//! crop, opacity, blend, all keyframes, audio settings, speed, markers, links and attachments.
//! Only `media`, `source_in` and `duration` change (and, for a picture turned into a still, the
//! speed fields, which a still does not have).
//!
//! # Where the new media starts and how long the clip is
//!
//! - `keep_duration` (default): the clip plays the new media from `source_in` (default: its
//!   start) for the old length. If the new media has less left than that, the clip is
//!   shortened to what is left (`shortened: true`), on the frame grid.
//! - `keep_source_in`: the clip plays the new media from the source position the old media was
//!   played from (`source_in` is not allowed); shortened the same way.
//! - `fit_to_source`: the clip plays the new media from `source_in` (default: its start) to its
//!   end, so its length changes, either way. Stills and media of unknown length have no end:
//!   the length stays.
//!
//! A length change ripples as in `clip.set_speed`: on the magnetic main track what follows moves
//! with the clip's end (gapless); on a free track the clip keeps its start, and growing into the
//! next item is `Invalid` at `/mode`.
//!
//! # Kinds
//!
//! Pictures (video and stills) replace pictures and sound replaces sound; audio and video
//! cannot replace each other (`Invalid` at `/media`). A video turned into a still resets speed
//! and direction (`speed_reset: true`). Compound clips and freeze frames are `Unsupported`.
//!
//! # Linked clips
//!
//! The clip's linked partners that play the same media are replaced together. If the new media
//! has no sound, a partner on an audio track is unlinked and muted instead (disabled, on its
//! old media, no longer than the new clip) and listed in `muted`. A linked clip playing other
//! media is `Invalid` at `/item`. `Locked` if a track involved is locked; nothing changes then.

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{
    EntityKind, FrameRate, ItemId, MediaAsset, MediaId, MediaKind, Project, Rounding, Speed, Time,
    TimeInput, TrackKind,
};

use super::{clip_of, media_bound, media_of};
use crate::{
    item::members,
    report::EditReport,
    timeline::{
        edit::given_items,
        placement::{DryIds, Edit, Ids, Plan, frame_flicks, span, time},
    },
    track::check_editable_kind,
    util::resolve_time,
};

/// How the new media is lined up and how long the clip becomes.
// The variants are documented in the command, not one by one: undocumented variants keep the
// schema a plain string `enum`.
#[allow(missing_docs)]
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplaceMode {
    KeepDuration,
    KeepSourceIn,
    FitToSource,
}

/// Result of `clip.replace`.
#[wire]
pub struct ClipReplaceOutput {
    /// The clip (its ID is unchanged).
    pub item: ItemId,
    /// The clips that now play the new media: `item` first, then its linked partners.
    pub replaced: Vec<ItemId>,
    /// Partners that were unlinked and muted because the new media has no sound.
    pub muted: Vec<ItemId>,
    /// The clip got shorter than before (the new media was too short).
    pub shortened: bool,
    /// Speed and direction were reset (a video became a still).
    pub speed_reset: bool,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Swaps the media of a clip (`item`) for `media` and keeps everything else: position, name,
/// transform, crop, opacity, blend, keyframes, audio settings, markers, links. `mode`:
/// `keep_duration` (default) plays the new media from `source_in` (default its start) for the
/// old length, shortening the clip (`shortened: true`) if the media is shorter; `keep_source_in`
/// starts where the old media was played from; `fit_to_source` plays from `source_in` to the
/// end of the new media, changing the length (the main track ripples, a free track may not grow
/// into the next item). Linked partners are replaced together; with no sound in the new media
/// they are unlinked and muted. Video may become a still (speed resets); audio and video
/// cannot replace each other (`Invalid` at `/media`). `Locked` if a track involved is locked.
#[command(
    name = "clip.replace",
    summary = "Replace a clip's media and keep its transform, keyframes, audio settings, links and position",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ClipReplace {
    /// The clip.
    pub item: ItemId,
    /// The new media.
    pub media: MediaId,
    /// `keep_duration` (default), `keep_source_in` or `fit_to_source`.
    pub mode: Option<ReplaceMode>,
    /// Where the new media starts playing (default: its start). Not for `keep_source_in`, and
    /// not for stills.
    pub source_in: Option<TimeInput>,
}

struct ReplacePlan {
    plan: Plan,
    replaced: Vec<ItemId>,
    muted: Vec<ItemId>,
    shortened: bool,
    speed_reset: bool,
}

fn has_sound(media: &MediaAsset) -> bool {
    match media.kind {
        MediaKind::Audio => true,
        MediaKind::Video => media.probe.audio.is_some(),
        _ => false,
    }
}

fn is_picture(kind: MediaKind) -> bool {
    matches!(kind, MediaKind::Video | MediaKind::Image)
}

/// `len` shortened so that its end is on the frame grid (never later).
fn floor_len(rate: FrameRate, start: i64, len: i64) -> i64 {
    let end = start.saturating_add(len);
    rate.snap_to_frame(time(end), Rounding::Floor)
        .map_or(end, Time::flicks)
        .saturating_sub(start)
}

#[allow(clippy::too_many_lines)] // checks, source start, length, then the edit, in order
fn plan_replace(
    cmd: &ClipReplace,
    doc: &Project,
    ids: &mut dyn Ids,
) -> Result<ReplacePlan, CmdError> {
    let (sequence, _) = given_items(doc, &[cmd.item])?;
    let (_, group) = members(doc, cmd.item, true)?;
    let seq = &doc.sequences[&sequence];
    let rate = seq.format.frame_rate;
    let new = doc
        .media
        .get(&cmd.media)
        .ok_or_else(|| CmdError::not_found(EntityKind::Media, cmd.media))?;
    let clip = clip_of(&group[0].item)?;
    let old = media_of(doc, clip)?;
    if new.kind == MediaKind::SequenceRef || old.kind == MediaKind::SequenceRef {
        return Err(CmdError::unsupported(
            "compound clips cannot be replaced this way",
        ));
    }
    if clip.freeze.is_some() {
        return Err(CmdError::unsupported(format!(
            "clip `{}` is a freeze frame, which cannot be replaced",
            clip.id
        )));
    }
    if cmd.media == clip.media {
        return Err(CmdError::invalid(
            "/media",
            "the clip already plays this media",
        ));
    }
    if is_picture(old.kind) != is_picture(new.kind) {
        let name = |k| if is_picture(k) { "picture" } else { "sound" };
        return Err(CmdError::invalid(
            "/media",
            format!(
                "a {} clip cannot be replaced by {} media",
                name(old.kind),
                name(new.kind)
            ),
        ));
    }
    for m in &group {
        if let Some(t) = seq.track(&m.track) {
            check_editable_kind("/item", t.kind)?;
        }
        let c = clip_of(&m.item)?;
        if c.media != clip.media || c.freeze.is_some() {
            return Err(CmdError::invalid(
                "/item",
                format!(
                    "clip `{}` is linked to `{}`, which plays other media; unlink them first",
                    clip.id, c.id
                ),
            ));
        }
    }
    let sound = has_sound(new);
    let on_audio = |track| seq.track(track).is_some_and(|t| t.kind == TrackKind::Audio);
    if on_audio(&group[0].track) && !sound {
        return Err(CmdError::invalid("/media", "the new media has no sound"));
    }

    // Where the new media starts.
    let still = new.kind == MediaKind::Image;
    let mode = cmd.mode.unwrap_or(ReplaceMode::KeepDuration);
    if cmd.source_in.is_some() && (mode == ReplaceMode::KeepSourceIn || still) {
        return Err(CmdError::invalid(
            "/source_in",
            if still {
                "a still has no source position"
            } else {
                "`keep_source_in` keeps the old source position"
            },
        ));
    }
    let src = if still {
        Time::ZERO
    } else if mode == ReplaceMode::KeepSourceIn {
        clip.source_in
    } else if let Some(t) = cmd.source_in {
        resolve_time("/source_in", t, rate)?
    } else {
        Time::ZERO
    };
    if src < Time::ZERO {
        return Err(CmdError::invalid(
            "/source_in",
            "the source position must not be negative",
        ));
    }
    let src = src.flicks();

    // Speed: a still has none.
    let speed_reset = still && (!clip.speed.is_one() || clip.reverse);
    let speed = if still { Speed::ONE } else { clip.speed };
    let reverse = clip.reverse && !still;

    // How long.
    let (start, end) = span(&group[0].item);
    let old_len = end - start;
    let frame = frame_flicks(rate);
    let new_len = match media_bound(new) {
        None => old_len,
        Some(total) => {
            let room = total - src;
            if room <= 0 {
                return Err(CmdError::invalid(
                    "/source_in",
                    "the source position is at or past the end of the new media",
                ));
            }
            if mode != ReplaceMode::FitToSource && speed.source(old_len) <= room {
                old_len
            } else {
                let len = floor_len(rate, start, speed.longest_within(room));
                if len < frame {
                    return Err(CmdError::invalid(
                        "/source_in",
                        "less than one frame of media is left after the source position",
                    ));
                }
                len
            }
        }
    };
    let delta = new_len - old_len;

    let mut e = Edit::new(doc, sequence, "/mode")?;
    let mut replaced = Vec::new();
    let mut muted = Vec::new();
    let mut tracks = Vec::new();
    let mut left = None;
    for m in &group {
        let id = m.item.id();
        if !tracks.contains(&m.track) {
            tracks.push(m.track);
        }
        if on_audio(&m.track) && !sound {
            // Unlinked and muted: it stays on its media, no longer than the new clip.
            let own = m.item.as_clip().map_or(0, |c| c.source_in.flicks());
            e.retime(id, start, old_len.min(new_len), own);
            e.update(id, |i| {
                left = i.linked_group();
                i.set_linked_group(None);
                i.set_enabled(false);
            });
            muted.push(id);
            continue;
        }
        e.retime(id, start, new_len, src);
        e.update(id, |i| {
            if let Some(c) = i.as_clip_mut() {
                c.media = cmd.media;
                c.speed = speed;
                c.reverse = reverse;
            }
        });
        replaced.push(id);
    }
    if let Some(g) = left {
        e.mark_left(g);
    }
    if delta != 0 {
        let magnetic = tracks
            .iter()
            .any(|t| seq.track(t).is_some_and(|t| t.magnetic));
        if magnetic {
            e.ripple(&tracks, false, end, &move |_| delta);
        } else if delta > 0 {
            for m in &group {
                if let Some(other) = e.blocker(m.track, end, end + delta, Some(m.item.id())) {
                    let name = e.track_name(m.track);
                    e.defer(e.invalid(format!(
                        "clip `{}` would run into item `{other}` on track `{name}`; use \
                         `keep_duration` or make room first",
                        m.item.id()
                    )));
                }
            }
        }
    }
    Ok(ReplacePlan {
        plan: e.finish(ids)?,
        replaced,
        muted,
        shortened: new_len < old_len,
        speed_reset,
    })
}

impl Command for ClipReplace {
    type Output = ClipReplaceOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_replace(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ClipReplaceOutput, CmdError> {
        let doc = tx.doc().clone();
        let planned = plan_replace(self, &doc, tx)?;
        let report = planned.plan.commit(tx)?;
        Ok(ClipReplaceOutput {
            item: self.item,
            replaced: planned.replaced,
            muted: planned.muted,
            shortened: planned.shortened,
            speed_reset: planned.speed_reset,
            report,
        })
    }
}
