// SPDX-License-Identifier: GPL-3.0-or-later
//! The `track` namespace (WP-1B.05, WP-2A.02; arch §4.2, §5.1): `track.add`, `track.remove`,
//! `track.set`, `track.reorder`, `track.set_height`, `track.set_magnetic` and the `track.list`
//! query.
//!
//! Schema v2 edits video and audio tracks only; text, caption and adjustment tracks answer
//! `Unsupported`. Track counts are unlimited: above [`SOFT_TRACK_LIMIT`] tracks `track.add`
//! still succeeds and says so in its `warnings`.
//!
//! **Flags and positions never touch items (spec INV-29).** `muted`, `hidden`, `solo`, `gain`,
//! the height and the display order are properties of the track: no command here moves,
//! trims or removes an item because of them, and only the audio mix and the render read them.
//! Items move only through `track.set_magnetic` closing gaps and `track.remove`.

mod magnetic;

use cutweave_commands::{
    CmdError, Command, Query, QueryCtx, Registry, RegistryError, Tx, command, query, wire,
};
use cutweave_model::{
    EntityKind, Item, ItemId, Project, Sequence, SequenceId, TRACK_HEIGHTS, Track, TrackId,
    TrackKind,
};

pub use magnetic::{TrackSetMagnetic, TrackSetMagneticOutput};

use crate::{item::unlink_orphans, util::check_name};

/// Above this many tracks in a sequence `track.add` adds a warning to its output (the UI
/// and the renderer slow down long before any hard limit; there is none).
pub const SOFT_TRACK_LIMIT: usize = 64;

/// Lowest accepted track gain in dB.
pub const MIN_TRACK_GAIN_DB: f64 = -144.0;
/// Highest accepted track gain in dB.
pub const MAX_TRACK_GAIN_DB: f64 = 48.0;

/// Registers the `track.*` commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<TrackAdd>()?
        .register::<TrackRemove>()?
        .register::<TrackSet>()?
        .register::<TrackReorder>()?
        .register::<TrackSetHeight>()?
        .register::<TrackSetMagnetic>()?
        .register::<TrackList>()?;
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
pub struct TrackAddOutput {
    /// The new track.
    pub track: TrackId,
    /// Where it is in the display order now (0 = top).
    pub index: u32,
    /// How many tracks the sequence has now.
    pub track_count: u32,
    /// Soft warnings (more than [`SOFT_TRACK_LIMIT`] tracks), empty normally.
    pub warnings: Vec<String>,
}

/// Adds an empty track to a sequence. New video tracks go on top of the stack, new audio
/// tracks at the bottom, unless `index` says otherwise.
#[command(
    name = "track.add",
    summary = "Add an empty video or audio track to a sequence",
    undoable,
    mcp = "core",
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
        let index = index.min(sequence.track_order.len());
        let track = Track::new(tx.new_id(), self.kind, name);
        tx.insert_track(self.sequence, index, &track)?;
        let track_count = tx.sequence(self.sequence)?.tracks.len();
        let warnings = if track_count > SOFT_TRACK_LIMIT {
            vec![format!(
                "the sequence has {track_count} tracks; more than {SOFT_TRACK_LIMIT} slows the editor down"
            )]
        } else {
            Vec::new()
        };
        Ok(TrackAddOutput {
            track: track.id,
            index: u32::try_from(index).unwrap_or(u32::MAX),
            track_count: u32::try_from(track_count).unwrap_or(u32::MAX),
            warnings,
        })
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

/// Removes a track together with its items. Fails with `Locked` if the track is locked, if a
/// linked partner that would be left alone in its group is on a locked track, or if an item
/// attached to a removed item is on a locked track. Linked partners of the removed items stay;
/// a partner left alone in its group is unlinked. Items attached to a removed item are
/// detached and keep their place (spec INV-38).
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
        // Detaching a child edits it, which a locked track does not allow.
        if let Some((locked, _)) = orphaned_children(ctx.doc(), track)
            .into_iter()
            .find(|(t, _)| t.locked)
        {
            return Err(CmdError::locked(EntityKind::Track, locked.id));
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
        let children: Vec<ItemId> = {
            let (_, track) = tx.track(self.track)?;
            orphaned_children(tx.doc(), track)
                .into_iter()
                .map(|(_, child)| child)
                .collect()
        };
        for child in children {
            tx.update_item(child, |i| {
                i.set_attached_to(None);
                Ok(())
            })?;
        }
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

/// The items attached to an item of `track` (with the track each sits on), in display order:
/// removing `track` detaches them.
fn orphaned_children<'a>(doc: &'a Project, track: &Track) -> Vec<(&'a Track, ItemId)> {
    let Some(sequence) = doc
        .sequences
        .values()
        .find(|s| s.tracks.contains_key(&track.id))
    else {
        return Vec::new();
    };
    sequence
        .tracks_in_order()
        .filter(|t| t.id != track.id)
        .flat_map(|t| {
            t.items
                .iter()
                .filter(|i| i.attached_to().is_some_and(|p| track.items.contains(&p)))
                .map(move |i| (t, i.id()))
        })
        .collect()
}

// ----- track.reorder -------------------------------------------------------------------------

/// Result of `track.reorder`.
#[wire]
#[derive(Copy)]
pub struct TrackReorderOutput {
    /// The track.
    pub track: TrackId,
    /// Its display position now (0 = top).
    pub index: u32,
}

/// Moves a track to another display position (0 = top, clamped to the end). Upper video
/// tracks composite over lower ones, so this changes what is on top. Items are not touched,
/// so it also works on a locked track.
#[command(
    name = "track.reorder",
    summary = "Move a track to another position in the track stack",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct TrackReorder {
    /// The track.
    pub track: TrackId,
    /// Its new display position, 0 = top (clamped to the end).
    pub index: u32,
}

impl Command for TrackReorder {
    type Output = TrackReorderOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        ctx.track(self.track).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TrackReorderOutput, CmdError> {
        let (sequence, _) = tx.track(self.track)?;
        let sequence = sequence.id;
        let index = tx.update_sequence(sequence, |s| {
            s.track_order.retain(|t| *t != self.track);
            let index = (self.index as usize).min(s.track_order.len());
            s.track_order.insert(index, self.track);
            Ok(index)
        })?;
        Ok(TrackReorderOutput {
            track: self.track,
            index: u32::try_from(index).unwrap_or(u32::MAX),
        })
    }
}

// ----- track.set_height ----------------------------------------------------------------------

/// Result of `track.set_height`.
#[wire]
#[derive(Copy)]
pub struct TrackSetHeightOutput {
    /// The track.
    pub track: TrackId,
    /// Its row height in pixels now; `None` is the default height.
    pub height: Option<u32>,
}

/// Sets the track's row height in the timeline (a persisted UI hint, not a `view` setting:
/// it is saved with the project and undoable). `null` restores the default. Works on a
/// locked track and never touches items.
#[command(
    name = "track.set_height",
    summary = "Set a track's row height in the timeline (a saved UI hint)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct TrackSetHeight {
    /// The track.
    pub track: TrackId,
    /// Height in pixels, `8..=512`; `null` for the default.
    pub height: Option<u32>,
}

impl Command for TrackSetHeight {
    type Output = TrackSetHeightOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        ctx.track(self.track)?;
        if let Some(h) = self.height
            && !TRACK_HEIGHTS.contains(&h)
        {
            return Err(CmdError::invalid(
                "/height",
                format!(
                    "the height must be between {} and {} pixels",
                    TRACK_HEIGHTS.start(),
                    TRACK_HEIGHTS.end()
                ),
            ));
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TrackSetHeightOutput, CmdError> {
        self.validate(&tx.query_ctx())?;
        tx.update_track(self.track, |t| {
            t.height = self.height;
            Ok(TrackSetHeightOutput {
                track: t.id,
                height: t.height,
            })
        })
    }
}

// ----- track.list ----------------------------------------------------------------------------

/// One track of `track.list`.
#[wire]
#[allow(clippy::struct_excessive_bools)] // mirrors the track's independent toggles
pub struct TrackInfo {
    /// The track.
    pub id: TrackId,
    /// What it holds.
    pub kind: TrackKind,
    /// Display name.
    pub name: String,
    /// Display position, 0 = top.
    pub index: u32,
    /// Silenced.
    pub muted: bool,
    /// Not rendered.
    pub hidden: bool,
    /// Item edits fail with `Locked`.
    pub locked: bool,
    /// Solo.
    pub solo: bool,
    /// The magnetic main track.
    pub magnetic: bool,
    /// Gain in dB.
    pub gain: f64,
    /// Row height in pixels; `None` is the default.
    pub height: Option<u32>,
    /// Number of items.
    pub item_count: u32,
}

/// Result of `track.list`.
#[wire]
pub struct TrackListOutput {
    /// The sequence.
    pub sequence: SequenceId,
    /// Its tracks in display order, top to bottom.
    pub tracks: Vec<TrackInfo>,
}

/// Lists a sequence's tracks in display order with their flags, height and item counts.
#[query(
    name = "track.list",
    summary = "List a sequence's tracks (flags, height, item counts) top to bottom",
    mcp = "catalog"
)]
pub struct TrackList {
    /// The sequence.
    pub sequence: SequenceId,
}

impl Query for TrackList {
    type Output = TrackListOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<TrackListOutput, CmdError> {
        let sequence = ctx.sequence(self.sequence)?;
        let tracks = sequence
            .tracks_in_order()
            .enumerate()
            .map(|(index, t)| TrackInfo {
                id: t.id,
                kind: t.kind,
                name: t.name.clone(),
                index: u32::try_from(index).unwrap_or(u32::MAX),
                muted: t.muted,
                hidden: t.hidden,
                locked: t.locked,
                solo: t.solo,
                magnetic: t.magnetic,
                gain: t.gain,
                height: t.height,
                item_count: u32::try_from(t.items.len()).unwrap_or(u32::MAX),
            })
            .collect();
        Ok(TrackListOutput {
            sequence: sequence.id,
            tracks,
        })
    }
}
