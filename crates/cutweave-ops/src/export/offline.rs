// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline media and exports (WP-2F.03c): an export whose clips play media that is not on this
//! computer is **refused** before anything is queued, unless the caller passes
//! `allow_offline: true`; such clips then render as the "media offline" slate
//! (`cutweave_render::slate`) with silent audio.
//!
//! **What counts.** The enabled clips of the exported sequence that overlap the exported range
//! (any of a marker set's regions), on a video track that is not hidden or an audio track that
//! is not muted, whose asset has a file that [`ExportStarter::media_offline`] says is gone. Media
//! in the bin that the range does not play never refuses an export. An image sequence counts
//! when its first picture is gone (`media.find_missing`'s `missing`); an `incomplete` one does
//! not.
//!
//! **The refusal** is `Invalid` at `/media/<id>` (the first offline asset in id order; the id is
//! the asset's key in the project's `media` map, not a param) with a message that says how many
//! files are missing, names them and names both ways out.

use std::collections::BTreeMap;

use cutweave_commands::CmdError;
use cutweave_model::{MediaAsset, MediaId, MediaKind, Project, Sequence, Time, TrackKind};

use super::ExportStarter;
use crate::media::asset_name;

/// Most names the refusal lists; the rest are counted.
pub const MAX_LISTED_NAMES: usize = 8;

/// An asset an export would play that is not on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfflineMedia {
    /// The asset.
    pub id: MediaId,
    /// The name the bin shows.
    pub name: String,
}

/// The offline assets the export of `[start, end)` ranges of `seq` would play (see the module
/// docs), by id. `offline` answers for one asset (asked once per asset).
#[must_use]
pub fn offline_media(
    doc: &Project,
    seq: &Sequence,
    ranges: &[(Time, Time)],
    offline: &dyn Fn(&MediaAsset) -> bool,
) -> Vec<OfflineMedia> {
    let mut seen: BTreeMap<MediaId, Option<String>> = BTreeMap::new();
    for track in seq.tracks.values() {
        let plays = match track.kind {
            TrackKind::Video => !track.hidden,
            TrackKind::Audio => !track.muted,
            _ => true,
        };
        if !plays {
            continue;
        }
        for &(start, end) in ranges {
            for item in track.items.overlapping(start, end) {
                let Some(clip) = item.as_clip().filter(|c| c.enabled) else {
                    continue;
                };
                if seen.contains_key(&clip.media) {
                    continue;
                }
                let gone = doc
                    .media
                    .get(&clip.media)
                    .filter(|a| a.kind != MediaKind::SequenceRef && a.path_abs.is_some())
                    .filter(|a| offline(a))
                    .map(|a| asset_name(a).into_owned());
                seen.insert(clip.media, gone);
            }
        }
    }
    seen.into_iter()
        .filter_map(|(id, name)| Some(OfflineMedia { id, name: name? }))
        .collect()
}

/// The human sentence about `missing` (not empty): how many files, and their names.
#[must_use]
pub fn describe(missing: &[OfflineMedia]) -> String {
    let names: Vec<String> = missing
        .iter()
        .take(MAX_LISTED_NAMES)
        .map(|m| format!("`{}`", m.name))
        .collect();
    let more = match missing.len().saturating_sub(MAX_LISTED_NAMES) {
        0 => String::new(),
        n => format!(" and {n} more"),
    };
    let head = match missing.len() {
        1 => "1 media file is missing".to_owned(),
        n => format!("{n} media files are missing"),
    };
    format!("{head}: {}{more}", names.join(", "))
}

/// The refusal of an export that would play `missing` (not empty): see the module docs.
#[must_use]
pub fn refusal(missing: &[OfflineMedia]) -> CmdError {
    let pointer = missing
        .first()
        .map_or_else(|| "/media".to_owned(), |m| format!("/media/{}", m.id));
    CmdError::invalid(
        pointer,
        format!(
            "{}; relink them (media.relink_folder, media.relink) or pass `allow_offline: true` \
             to export a \"media offline\" slate in their place",
            describe(missing)
        ),
    )
}

/// `Err` with the [`refusal`] when the export of `ranges` of `seq` would play offline media, as
/// `starter` sees the disk.
pub fn check(
    doc: &Project,
    seq: &Sequence,
    ranges: &[(Time, Time)],
    starter: &dyn ExportStarter,
) -> Result<(), CmdError> {
    let missing = offline_media(doc, seq, ranges, &|asset| starter.media_offline(asset));
    if missing.is_empty() {
        Ok(())
    } else {
        Err(refusal(&missing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(n: u128, name: &str) -> OfflineMedia {
        OfflineMedia {
            id: MediaId::from_u128(n),
            name: name.to_owned(),
        }
    }

    #[test]
    fn the_refusal_points_at_the_first_asset_and_names_them_all() {
        let one = refusal(&[media(7, "a.mp4")]);
        let CmdError::Invalid { path, msg } = &one else {
            panic!("{one:?}")
        };
        assert_eq!(path, &format!("/media/{}", MediaId::from_u128(7)));
        assert!(
            msg.starts_with("1 media file is missing: `a.mp4`; "),
            "{msg}"
        );
        assert!(msg.contains("allow_offline: true"), "{msg}");

        let two = refusal(&[media(7, "a.mp4"), media(9, "b.wav")]);
        let CmdError::Invalid { path, msg } = &two else {
            panic!("{two:?}")
        };
        assert_eq!(path, &format!("/media/{}", MediaId::from_u128(7)));
        assert!(
            msg.starts_with("2 media files are missing: `a.mp4`, `b.wav`; "),
            "{msg}"
        );
    }

    #[test]
    fn long_lists_are_counted_not_spelled_out() {
        let many: Vec<OfflineMedia> = (0..11).map(|n| media(n, &format!("{n}.mp4"))).collect();
        let text = describe(&many);
        assert!(text.starts_with("11 media files are missing: `0.mp4`, "));
        assert!(text.ends_with("`7.mp4` and 3 more"), "{text}");
    }
}
