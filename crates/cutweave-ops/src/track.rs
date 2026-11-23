// SPDX-License-Identifier: GPL-3.0-or-later
//! The `track` namespace (WP-1B.05; arch §4.2, §5.1): `track.add`, `track.remove`, `track.set`.
//!
//! Schema v1 edits video and audio tracks only; text, caption and adjustment tracks answer
//! `Unsupported`.

use cutweave_commands::{CmdError, Command, QueryCtx, Registry, RegistryError, Tx, command, wire};
use cutweave_model::{EntityKind, Item, Sequence, SequenceId, Track, TrackId, TrackKind};

use crate::{item::unlink_orphans, util::check_name};

/// Lowest accepted track gain in dB.
pub const MIN_TRACK_GAIN_DB: f64 = -144.0;
/// Highest accepted track gain in dB.
pub const MAX_TRACK_GAIN_DB: f64 = 48.0;

/// Registers the `track.*` commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<TrackAdd>()?
        .register::<TrackRemove>()?
        .register::<TrackSet>()?;
    Ok(())
}

pub(crate) fn check_editable_kind(pointer: &str, kind: TrackKind) -> Result<(), CmdError> {
    match kind {
        TrackKind::Video | TrackKind::Audio => Ok(()),
        other => Err(CmdError::unsupported(format!(
            "{other} tracks are not editable in this version (`{pointer}`)"
        ))),
    }
}

/// The default name of the next track of `kind` in `sequence`: `V2`, `A3`, ...
pub(crate) fn next_track_name(sequence: &Sequence, kind: TrackKind) -> String {
    let n = sequence.tracks.values().filter(|t| t.kind == kind).count() + 1;
    let prefix = match kind {
        TrackKind::Video => 'V',
        TrackKind::Audio => 'A',
        TrackKind::Text => 'T',
        TrackKind::Caption => 'C',
        TrackKind::Adjustment => 'J',
    };
    format!("{prefix}{n}")
}

/// Where a new track goes in display order (top to bottom): new video tracks go above all
/// existing tracks (higher layers composite over lower ones), new audio tracks below them.
pub(crate) fn default_index(sequence: &Sequence, kind: TrackKind) -> usize {
    match kind {
        TrackKind::Audio => sequence.track_order.len(),
        _ => 0,
    }
}

// ----- track.add -----------------------------------------------------------------------------

/// Result of `track.add`.
#[wire]
#[derive(Copy)]
pub struct TrackAddOutput {
    /// The new track.
    pub track: TrackId,
}

/// Adds an empty track to a sequence. New video tracks go on top of the stack, new audio
/// tracks at the bottom, unless `index` says otherwise.
#[command(
    name = "track.add",
    summary = "Add an empty video or audio track to a sequence",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct TrackAdd {
    /// The sequence.
    pub sequence: SequenceId,
    /// `video` or `audio`.
    pub kind: TrackKind,
    /// Display name; defaults to `V<n>` / `A<n>`.
    pub name: Option<String>,
    /// Display position, 0 = top (clamped to the end).
    pub index: Option<u32>,
}

impl Command for TrackAdd {
    type Output = TrackAddOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        ctx.sequence(self.sequence)?;
        check_editable_kind("/kind", self.kind)?;
        if let Some(name) = &self.name {
            check_name("/name", name)?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TrackAddOutput, CmdError> {
        self.validate(&tx.query_ctx())?;
        let sequence = tx.sequence(self.sequence)?;
        let name = match &self.name {
            Some(n) => check_name("/name", n)?,
            None => next_track_name(sequence, self.kind),
        };
        let index = self
            .index
            .map_or_else(|| default_index(sequence, self.kind), |i| i as usize);
        let track = Track::new(tx.new_id(), self.kind, name);
        tx.insert_track(self.sequence, index, &track)?;
        Ok(TrackAddOutput { track: track.id })
    }
}

// ----- track.remove --------------------------------------------------------------------------

/// Result of `track.remove`.
#[wire]
#[derive(Copy)]
pub struct TrackRemoveOutput {
    /// The removed track.
    pub track: TrackId,
    /// How many items it held (removed with it).
    pub removed_items: u32,
}

/// Removes a track together with its items. Fails with `Locked` if the track is locked, or
/// if a linked partner that would be left alone in its group is on a locked track. Linked
/// partners of the removed items stay; a partner left alone in its group is unlinked.
#[command(
    name = "track.remove",
    summary = "Remove a track and the items on it (fails if the track is locked)",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct TrackRemove {
    /// The track.
    pub track: TrackId,
}

impl Command for TrackRemove {
    type Output = TrackRemoveOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        let (_, track) = ctx.track(self.track)?;
        if track.locked {
            return Err(CmdError::locked(EntityKind::Track, track.id));
        }
        // A partner that would be left alone in its group is unlinked, which a locked track
        // does not allow (edits touching a locked partner's track fail with `Locked`).
        for group in track.items.iter().filter_map(Item::linked_group) {
            let mut elsewhere = ctx
                .doc()
                .sequences
                .values()
                .flat_map(|s| s.tracks.values())
                .filter(|t| t.id != track.id)
                .flat_map(|t| t.items.iter().map(move |i| (t, i)))
                .filter(|(_, i)| i.linked_group() == Some(group));
            if let (Some((t, _)), None) = (elsewhere.next(), elsewhere.next())
                && t.locked
            {
                return Err(CmdError::locked(EntityKind::Track, t.id));
            }
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TrackRemoveOutput, CmdError> {
        self.validate(&tx.query_ctx())?;
        let track = tx.remove_track(self.track)?;
        // Partners on other tracks must not be left alone in a group.
        unlink_orphans(tx, track.items.iter().filter_map(Item::linked_group))?;
        Ok(TrackRemoveOutput {
            track: track.id,
            removed_items: u32::try_from(track.items.len()).unwrap_or(u32::MAX),
        })
    }
}

// ----- track.set -----------------------------------------------------------------------------

/// Result of `track.set`: the track's properties now in effect.
#[wire]
#[allow(clippy::struct_excessive_bools)] // mirrors the track's independent toggles
pub struct TrackSetOutput {
    /// The track.
    pub track: TrackId,
    /// Display name.
    pub name: String,
    /// Silenced.
    pub muted: bool,
    /// Not rendered.
    pub hidden: bool,
    /// Item edits fail with `Locked`.
    pub locked: bool,
    /// Solo.
    pub solo: bool,
    /// Gain in dB.
    pub gain: f64,
}

/// Changes track properties; absent fields keep their value. Works on locked tracks (that is
/// how a track is unlocked).
#[command(
    name = "track.set",
    summary = "Set a track's name, mute, hide, lock, solo or gain",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct TrackSet {
    /// The track.
    pub track: TrackId,
    /// New display name.
    pub name: Option<String>,
    /// Silence the track.
    pub muted: Option<bool>,
    /// Hide the track from the render.
    pub hidden: Option<bool>,
    /// Lock the track: commands that change its items fail with `Locked`.
    pub locked: Option<bool>,
    /// Solo the track.
    pub solo: Option<bool>,
    /// Track gain in dB, `-144..=48`.
    pub gain: Option<f64>,
}

impl Command for TrackSet {
    type Output = TrackSetOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        ctx.track(self.track)?;
        if self.name.is_none()
            && self.muted.is_none()
            && self.hidden.is_none()
            && self.locked.is_none()
            && self.solo.is_none()
            && self.gain.is_none()
        {
            return Err(CmdError::invalid(
                "",
                "give at least one of name, muted, hidden, locked, solo, gain",
            ));
        }
        if let Some(name) = &self.name {
            check_name("/name", name)?;
        }
        if let Some(g) = self.gain
            && !(MIN_TRACK_GAIN_DB..=MAX_TRACK_GAIN_DB).contains(&g)
        {
            return Err(CmdError::invalid(
                "/gain",
                format!("the gain must be between {MIN_TRACK_GAIN_DB} and {MAX_TRACK_GAIN_DB} dB"),
            ));
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TrackSetOutput, CmdError> {
        self.validate(&tx.query_ctx())?;
        let name = self
            .name
            .as_deref()
            .map(|n| check_name("/name", n))
            .transpose()?;
        tx.update_track(self.track, |t| {
            if let Some(name) = name {
                t.name = name;
            }
            t.muted = self.muted.unwrap_or(t.muted);
            t.hidden = self.hidden.unwrap_or(t.hidden);
            t.locked = self.locked.unwrap_or(t.locked);
            t.solo = self.solo.unwrap_or(t.solo);
            t.gain = self.gain.unwrap_or(t.gain);
            Ok(TrackSetOutput {
                track: t.id,
                name: t.name.clone(),
                muted: t.muted,
                hidden: t.hidden,
                locked: t.locked,
                solo: t.solo,
                gain: t.gain,
            })
        })
    }
}
