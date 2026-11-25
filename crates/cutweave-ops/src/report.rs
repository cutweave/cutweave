// SPDX-License-Identifier: GPL-3.0-or-later
//! [`EditReport`]: what an edit command changed, exactly (timeline spec §3.1, INV-21;
//! ADR-0023).
//!
//! The report is *the difference between the document before and after* for one sequence, so a
//! command can build it with [`EditReport::diff`] instead of tracking its own changes, and a
//! property test can compare the two. Lists are sorted by item ID (`split` by `(left, at)`),
//! which makes a report deterministic.
//!
//! WP-2A.02 defines the type and uses it for `track.set_magnetic`; the placement, trim, link
//! and clipboard commands of WP-2A.03 to 2A.06 return it too.

use std::collections::BTreeMap;

use cutweave_commands::wire;
use cutweave_model::{GroupId, Item, ItemId, MediaId, Sequence, Time, TrackId};

/// A split: `right` is new, `left` keeps its ID; `at` is the cut time before the edit.
#[wire]
pub struct SplitReport {
    /// The left half (keeps the original ID).
    pub left: ItemId,
    /// The right half (new).
    pub right: ItemId,
    /// The cut time.
    pub at: Time,
}

/// An item whose track or start changed (duration and source did not).
#[wire]
pub struct MovedReport {
    /// The item.
    pub item: ItemId,
    /// Its track now.
    pub track: TrackId,
    /// Its start now.
    pub start: Time,
}

/// An item whose duration or `source_in` changed (its start may have too).
#[wire]
pub struct TrimmedReport {
    /// The item.
    pub item: ItemId,
    /// Its start now.
    pub start: Time,
    /// Its duration now.
    pub duration: cutweave_model::Duration,
    /// Its `source_in` now (`0` for items that play no media).
    pub source_in: Time,
}

/// An item whose `linked_group` or `attached_to` changed.
#[wire]
pub struct RelinkedReport {
    /// The item.
    pub item: ItemId,
    /// Its linked group now.
    pub linked_group: Option<GroupId>,
    /// The item it is attached to now.
    pub attached_to: Option<ItemId>,
}

/// An item whose media changed (`clip.replace`).
#[wire]
pub struct ReplacedReport {
    /// The item.
    pub item: ItemId,
    /// Its media now.
    pub media: MediaId,
}

/// An item whose `enabled` flag changed.
#[wire]
pub struct ToggledReport {
    /// The item.
    pub item: ItemId,
    /// Its flag now.
    pub enabled: bool,
}

/// The exact difference one edit command made to a sequence (INV-21).
///
/// For every item ID: present only after means in `created` or a `split[].right` (exactly one
/// of them); present only before means in `removed`; present in both means in each list whose
/// condition its fields meet, and in none otherwise. A split's left half is also in `trimmed`.
#[wire]
#[derive(Default)]
pub struct EditReport {
    /// New items, other than the right halves of splits.
    pub created: Vec<ItemId>,
    /// Items that no longer exist.
    pub removed: Vec<ItemId>,
    /// Splits, sorted by `(left, at)`.
    pub split: Vec<SplitReport>,
    /// Items whose track or start changed.
    pub moved: Vec<MovedReport>,
    /// Items whose duration or source position changed.
    pub trimmed: Vec<TrimmedReport>,
    /// Items whose link or attachment changed.
    pub relinked: Vec<RelinkedReport>,
    /// Items whose media changed.
    pub replaced: Vec<ReplacedReport>,
    /// Items whose `enabled` flag changed.
    pub toggled: Vec<ToggledReport>,
    /// Tracks created, in creation order.
    pub created_tracks: Vec<TrackId>,
}

impl EditReport {
    /// Whether the edit changed nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == EditReport::default()
    }

    /// The report of the change from `before` to `after`. `splits` are the command's own
    /// splits (a right half is only distinguishable from a created item by the command that
    /// made it); `created_tracks` are the tracks it created, in order.
    #[must_use]
    pub fn diff(
        before: &Sequence,
        after: &Sequence,
        splits: &[SplitReport],
        created_tracks: &[TrackId],
    ) -> EditReport {
        type Seen<'a> = BTreeMap<ItemId, (TrackId, &'a Item)>;
        fn index(seq: &Sequence) -> Seen<'_> {
            seq.tracks
                .values()
                .flat_map(|t| t.items.iter_by_id().map(move |(_, i)| (i.id(), (t.id, i))))
                .collect()
        }
        let (old, new) = (index(before), index(after));
        let rights: std::collections::BTreeSet<ItemId> = splits.iter().map(|s| s.right).collect();
        let mut report = EditReport {
            split: splits.to_vec(),
            created_tracks: created_tracks.to_vec(),
            ..EditReport::default()
        };
        report.split.sort_by_key(|s| (s.left, s.at));
        for (id, (track, item)) in &new {
            let Some((old_track, old_item)) = old.get(id) else {
                if !rights.contains(id) {
                    report.created.push(*id);
                }
                continue;
            };
            let source_in = |i: &Item| i.as_clip().map_or(Time::ZERO, |c| c.source_in);
            let retimed =
                item.duration() != old_item.duration() || source_in(item) != source_in(old_item);
            if retimed {
                report.trimmed.push(TrimmedReport {
                    item: *id,
                    start: item.start(),
                    duration: item.duration(),
                    source_in: source_in(item),
                });
            } else if track != old_track || item.start() != old_item.start() {
                report.moved.push(MovedReport {
                    item: *id,
                    track: *track,
                    start: item.start(),
                });
            }
            if item.linked_group() != old_item.linked_group()
                || item.attached_to() != old_item.attached_to()
            {
                report.relinked.push(RelinkedReport {
                    item: *id,
                    linked_group: item.linked_group(),
                    attached_to: item.attached_to(),
                });
            }
            if item.media() != old_item.media()
                && let Some(media) = item.media()
            {
                report.replaced.push(ReplacedReport { item: *id, media });
            }
            if item.enabled() != old_item.enabled() {
                report.toggled.push(ToggledReport {
                    item: *id,
                    enabled: item.enabled(),
                });
            }
        }
        report.removed = old
            .keys()
            .filter(|id| !new.contains_key(id))
            .copied()
            .collect();
        report
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use cutweave_model::{
        Clip, Duration, FrameRate, MediaId, SampleRate, SequenceFormat, SequenceId, Track,
        TrackKind,
    };

    use super::*;

    fn seq() -> (Sequence, TrackId, TrackId) {
        let mut s = Sequence::new(
            SequenceId::from_u128(1),
            "S",
            SequenceFormat::new(1920, 1080, FrameRate::FPS_30, SampleRate::HZ_48_000),
        );
        let (a, b) = (TrackId::from_u128(10), TrackId::from_u128(11));
        s.insert_track(0, Track::new(a, TrackKind::Video, "V1"));
        s.insert_track(1, Track::new(b, TrackKind::Video, "V2"));
        (s, a, b)
    }

    fn clip(n: u128, start: i64, dur: i64) -> Item {
        Clip::new(
            ItemId::from_u128(n),
            MediaId::from_u128(1),
            Time::from_flicks(start).unwrap(),
            Duration::from_flicks(dur).unwrap(),
        )
        .into()
    }

    #[test]
    fn an_unchanged_sequence_has_an_empty_report() {
        let (mut s, a, _) = seq();
        s.track_mut(&a).unwrap().items.insert(clip(1, 0, 100));
        assert!(EditReport::diff(&s, &s.clone(), &[], &[]).is_empty());
    }

    #[test]
    fn every_kind_of_change_lands_in_its_list() {
        let (mut before, a, b) = seq();
        for item in [
            clip(1, 0, 100),
            clip(2, 100, 100),
            clip(3, 200, 100),
            clip(4, 300, 100),
        ] {
            before.track_mut(&a).unwrap().items.insert(item);
        }
        let mut after = before.clone();
        let g = GroupId::from_u128(9);
        // 1 moved in time; 2 changed track; 3 trimmed; 4 removed; 5 created; 6 a split right.
        after
            .track_mut(&a)
            .unwrap()
            .items
            .update(&ItemId::from_u128(1), |i| {
                i.set_start(Time::from_flicks(500).unwrap())
            });
        let two = after
            .track_mut(&a)
            .unwrap()
            .items
            .remove(&ItemId::from_u128(2))
            .unwrap();
        after.track_mut(&b).unwrap().items.insert(two);
        after
            .track_mut(&a)
            .unwrap()
            .items
            .update(&ItemId::from_u128(3), |i| {
                i.set_duration(Duration::from_flicks(60).unwrap());
                i.set_linked_group(Some(g));
                i.set_enabled(false);
            });
        after
            .track_mut(&a)
            .unwrap()
            .items
            .remove(&ItemId::from_u128(4));
        after.track_mut(&b).unwrap().items.insert(clip(5, 900, 10));
        after.track_mut(&b).unwrap().items.insert(clip(6, 950, 10));
        let split = SplitReport {
            left: ItemId::from_u128(3),
            right: ItemId::from_u128(6),
            at: Time::from_flicks(250).unwrap(),
        };
        let report = EditReport::diff(&before, &after, &[split], &[b]);
        let id = |n: u128| ItemId::from_u128(n);
        assert_eq!(report.created, [id(5)]);
        assert_eq!(report.removed, [id(4)]);
        assert_eq!(report.split.len(), 1);
        assert_eq!(
            report.moved.iter().map(|m| m.item).collect::<Vec<_>>(),
            [id(1), id(2)]
        );
        assert_eq!(report.moved[1].track, b);
        assert_eq!(
            report.trimmed.iter().map(|t| t.item).collect::<Vec<_>>(),
            [id(3)]
        );
        assert_eq!(report.relinked.len(), 1);
        assert_eq!(report.relinked[0].linked_group, Some(g));
        assert_eq!(
            report.toggled,
            [ToggledReport {
                item: id(3),
                enabled: false
            }]
        );
        assert!(report.replaced.is_empty());
        assert_eq!(report.created_tracks, [b]);
    }
}
