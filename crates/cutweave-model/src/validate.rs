// SPDX-License-Identifier: GPL-3.0-or-later
//! Document invariants (arch §4.3) and [`Project::validate`].
//!
//! Commands keep these invariants; property tests assert them after every command
//! (WP-1B.07). Paths in reports are JSON Pointers into the saved document.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{
    Clip, Custom, Duration, EntityKind, GroupId, Item, ItemId, MAX_SAFE_FLICKS, MediaId, MediaKind,
    Project, SequenceFormat, SequenceId, Time, TrackId, TrackKind, Vec2, item::Animatable,
};

/// Largest canvas side accepted by validation.
pub const MAX_CANVAS_SIDE: u32 = 16_384;
/// Highest sequence frame rate accepted by validation (fps).
pub const MAX_SEQUENCE_FPS: u32 = 1000;
/// Sequence sample rates accepted by validation (Hz).
pub const SEQUENCE_SAMPLE_RATES: std::ops::RangeInclusive<u32> = 8_000..=384_000;

/// A broken document invariant: what [`Project::validate`] reports.
#[derive(
    Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize, schemars::JsonSchema, specta::Type,
)]
#[serde(tag = "invariant", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Invariant {
    /// A map key differs from the ID inside the entity.
    #[error("`{path}/{key}` holds an entity whose id is `{id}`")]
    KeyMismatch {
        /// The map (`/media`, `/sequences/<id>/tracks`, …).
        path: String,
        /// The key.
        key: String,
        /// The entity's own ID.
        id: String,
    },
    /// A reference to an entity that does not exist.
    #[error("`{path}` refers to {kind} `{id}`, which does not exist")]
    DanglingReference {
        /// Where the reference is.
        path: String,
        /// The kind of entity referred to.
        kind: EntityKind,
        /// The missing ID.
        id: String,
    },
    /// A media asset whose fields contradict its kind.
    #[error("media `{media}` is invalid: {reason}")]
    InvalidMedia {
        /// The media.
        media: MediaId,
        /// What is wrong.
        reason: String,
    },
    /// A sequence format (or the default format) outside the supported range.
    #[error("format at `{path}` is invalid: {reason}")]
    InvalidFormat {
        /// The format.
        path: String,
        /// What is wrong.
        reason: String,
    },
    /// `track_order` is not exactly the keys of `tracks`.
    #[error("track order of sequence `{sequence}` is invalid: {reason}")]
    TrackOrder {
        /// The sequence.
        sequence: SequenceId,
        /// What is wrong.
        reason: String,
    },
    /// A clip whose media cannot play on its track (e.g. audio on a video track).
    #[error(
        "item `{item}` on {track_kind} track `{track}` cannot play {media_kind} media `{media}`"
    )]
    IncompatibleMedia {
        /// The item.
        item: ItemId,
        /// Its track.
        track: TrackId,
        /// The track kind.
        track_kind: TrackKind,
        /// The media.
        media: MediaId,
        /// The media kind.
        media_kind: MediaKind,
    },
    /// An item with a non-positive duration, a negative start or source position, or an end
    /// past the largest representable time.
    #[error("item `{item}` has invalid timing: {reason}")]
    InvalidTiming {
        /// The item.
        item: ItemId,
        /// What is wrong.
        reason: String,
    },
    /// A clip that reads past the end of its media (`source_in + duration > media duration`).
    #[error(
        "item `{item}` reads media `{media}` up to {source_end} but the media lasts {media_duration}"
    )]
    SourceOutOfBounds {
        /// The item.
        item: ItemId,
        /// The media.
        media: MediaId,
        /// `source_in + duration`.
        source_end: Time,
        /// The media duration.
        media_duration: Duration,
    },
    /// Two items of one track overlap (arch §4.3: never).
    #[error("items `{first}` and `{second}` overlap on track `{track}` of sequence `{sequence}`")]
    Overlap {
        /// The sequence.
        sequence: SequenceId,
        /// The track.
        track: TrackId,
        /// The earlier item.
        first: ItemId,
        /// The item starting inside it.
        second: ItemId,
    },
    /// Sequences that contain each other through compound clips (arch §4.3).
    #[error("compound clips form a cycle: {}", display_cycle(cycle))]
    CompoundCycle {
        /// The sequences on the cycle; the first is repeated at the end.
        cycle: Vec<SequenceId>,
    },
    /// A linked group with items in two sequences.
    #[error("linked group `{group}` has items in sequences `{first}` and `{second}`")]
    LinkedGroupSpansSequences {
        /// The group.
        group: GroupId,
        /// One sequence.
        first: SequenceId,
        /// Another sequence.
        second: SequenceId,
    },
    /// A number that is not finite or outside its range (e.g. opacity above 1).
    #[error("`{path}` is invalid: {reason}")]
    InvalidValue {
        /// The value.
        path: String,
        /// What is wrong.
        reason: String,
    },
    /// Plugin data under a key that is not a namespace (arch §4.3).
    #[error(
        "custom data key `{key}` at `{path}` is not a plugin namespace such as `com.example.plugin`"
    )]
    CustomNamespace {
        /// The custom map.
        path: String,
        /// The key.
        key: String,
    },
}

fn display_cycle(cycle: &[SequenceId]) -> String {
    cycle
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" -> ")
}

impl Project {
    /// Checks every document invariant and returns the first violation, in a deterministic
    /// order (media, then sequences, by ID).
    pub fn validate(&self) -> Result<(), Invariant> {
        match self.violations().into_iter().next() {
            Some(violation) => Err(violation),
            None => Ok(()),
        }
    }

    /// Every violated invariant, in a deterministic order (empty for a valid document).
    #[must_use]
    pub fn violations(&self) -> Vec<Invariant> {
        let mut out = Vec::new();
        check_custom("", &self.custom, &mut out);
        check_format(
            "/settings/default_format",
            &self.settings.default_format,
            &mut out,
        );
        self.check_media(&mut out);
        let mut groups = BTreeMap::new();
        for (key, sequence) in &self.sequences {
            let path = format!("/sequences/{key}");
            if *key != sequence.id {
                out.push(Invariant::KeyMismatch {
                    path: "/sequences".into(),
                    key: key.to_string(),
                    id: sequence.id.to_string(),
                });
            }
            check_format(&format!("{path}/format"), &sequence.format, &mut out);
            check_track_order(*key, sequence, &mut out);
            for (track_key, track) in &sequence.tracks {
                let track_path = format!("{path}/tracks/{track_key}");
                if *track_key != track.id {
                    out.push(Invariant::KeyMismatch {
                        path: format!("{path}/tracks"),
                        key: track_key.to_string(),
                        id: track.id.to_string(),
                    });
                }
                check_finite(&format!("{track_path}/gain"), track.gain, &mut out);
                check_custom(&track_path, &track.custom, &mut out);
                for (item_key, item) in track.items.iter_by_id() {
                    let item_path = format!("{track_path}/items/{item_key}");
                    if *item_key != item.id() {
                        out.push(Invariant::KeyMismatch {
                            path: format!("{track_path}/items"),
                            key: item_key.to_string(),
                            id: item.id().to_string(),
                        });
                    }
                    self.check_item(&item_path, *track_key, track.kind, item, &mut out);
                    if let Some(group) = item.linked_group() {
                        let first = *groups.entry(group).or_insert(*key);
                        if first != *key {
                            out.push(Invariant::LinkedGroupSpansSequences {
                                group,
                                first,
                                second: *key,
                            });
                        }
                    }
                }
                if let Some((first, second)) = track.items.first_overlap() {
                    out.push(Invariant::Overlap {
                        sequence: *key,
                        track: *track_key,
                        first,
                        second,
                    });
                }
            }
            check_custom(&path, &sequence.custom, &mut out);
        }
        self.check_compound_cycles(&mut out);
        dedup_linked_groups(&mut out);
        out
    }

    fn check_media(&self, out: &mut Vec<Invariant>) {
        for (key, media) in &self.media {
            let path = format!("/media/{key}");
            if *key != media.id {
                out.push(Invariant::KeyMismatch {
                    path: "/media".into(),
                    key: key.to_string(),
                    id: media.id.to_string(),
                });
            }
            let invalid = |reason: &str| Invariant::InvalidMedia {
                media: *key,
                reason: reason.to_owned(),
            };
            if media.kind.is_file() {
                if media.path_abs.as_deref().is_none_or(str::is_empty) {
                    out.push(invalid("a file asset needs a non-empty path_abs"));
                }
                if media.sequence.is_some() {
                    out.push(invalid("only sequence_ref media may reference a sequence"));
                }
            } else {
                match media.sequence {
                    None => out.push(invalid("sequence_ref media needs a sequence")),
                    Some(sequence) if !self.sequences.contains_key(&sequence) => {
                        out.push(Invariant::DanglingReference {
                            path: format!("{path}/sequence"),
                            kind: EntityKind::Sequence,
                            id: sequence.to_string(),
                        });
                    }
                    Some(_) => {}
                }
            }
            if !matches!(media.probe.rotation, 0 | 90 | 180 | 270) {
                out.push(invalid("probe rotation must be 0, 90, 180 or 270"));
            }
            check_custom(&path, &media.custom, out);
        }
    }

    fn check_item(
        &self,
        path: &str,
        track: TrackId,
        track_kind: TrackKind,
        item: &Item,
        out: &mut Vec<Invariant>,
    ) {
        let id = item.id();
        let timing = |reason: &str| Invariant::InvalidTiming {
            item: id,
            reason: reason.to_owned(),
        };
        if item.duration().is_zero() {
            out.push(timing("duration must be positive"));
        }
        if item.start() < Time::ZERO {
            out.push(timing("start must not be negative"));
        }
        if item.end_wide() > i128::from(MAX_SAFE_FLICKS) {
            out.push(timing("the item ends past the largest representable time"));
        }
        check_custom(path, item.custom(), out);
        match item {
            Item::Clip(clip) => self.check_clip(path, track, track_kind, clip, out),
        }
    }

    fn check_clip(
        &self,
        path: &str,
        track: TrackId,
        track_kind: TrackKind,
        clip: &Clip,
        out: &mut Vec<Invariant>,
    ) {
        if clip.source_in < Time::ZERO {
            out.push(Invariant::InvalidTiming {
                item: clip.id,
                reason: "source_in must not be negative".into(),
            });
        }
        match self.media.get(&clip.media) {
            None => out.push(Invariant::DanglingReference {
                path: format!("{path}/media"),
                kind: EntityKind::Media,
                id: clip.media.to_string(),
            }),
            Some(media) => {
                if !track_kind.accepts(media.kind) {
                    out.push(Invariant::IncompatibleMedia {
                        item: clip.id,
                        track,
                        track_kind,
                        media: clip.media,
                        media_kind: media.kind,
                    });
                }
                if let Some(media_duration) = media.probe.duration
                    && media.kind != MediaKind::Image
                {
                    let end =
                        i128::from(clip.source_in.flicks()) + i128::from(clip.duration.flicks());
                    if end > i128::from(media_duration.flicks()) {
                        out.push(Invariant::SourceOutOfBounds {
                            item: clip.id,
                            media: clip.media,
                            source_end: clip.source_in.saturating_add(clip.duration),
                            media_duration,
                        });
                    }
                }
            }
        }
        let gain = format!("{path}/audio/gain");
        check_finite(&gain, *clip.audio.gain.value(), out);
        let visual = &clip.visual;
        let opacity = *visual.opacity.value();
        if !(0.0..=1.0).contains(&opacity) {
            out.push(Invariant::InvalidValue {
                path: format!("{path}/visual/opacity"),
                reason: format!("opacity must be between 0 and 1, got {opacity}"),
            });
        }
        let transform = format!("{path}/visual/transform");
        check_vec2(
            &format!("{transform}/position"),
            &visual.transform.position,
            out,
        );
        check_vec2(&format!("{transform}/scale"), &visual.transform.scale, out);
        check_vec2(
            &format!("{transform}/anchor"),
            &visual.transform.anchor,
            out,
        );
        check_finite(
            &format!("{transform}/rotation"),
            *visual.transform.rotation.value(),
            out,
        );
    }

    fn check_compound_cycles(&self, out: &mut Vec<Invariant>) {
        let mut edges: BTreeMap<SequenceId, BTreeSet<SequenceId>> = BTreeMap::new();
        for (key, sequence) in &self.sequences {
            let targets = edges.entry(*key).or_default();
            for track in sequence.tracks.values() {
                for item in track.items.iter() {
                    let target = item
                        .media()
                        .and_then(|m| self.media.get(&m))
                        .and_then(|m| m.sequence)
                        .filter(|s| self.sequences.contains_key(s));
                    targets.extend(target);
                }
            }
        }
        let mut state: BTreeMap<SequenceId, Visit> = BTreeMap::new();
        let mut stack = Vec::new();
        for start in edges.keys() {
            if !state.contains_key(start) {
                visit(*start, &edges, &mut state, &mut stack, out);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Visit {
    OnStack,
    Done,
}

fn visit(
    node: SequenceId,
    edges: &BTreeMap<SequenceId, BTreeSet<SequenceId>>,
    state: &mut BTreeMap<SequenceId, Visit>,
    stack: &mut Vec<SequenceId>,
    out: &mut Vec<Invariant>,
) {
    state.insert(node, Visit::OnStack);
    stack.push(node);
    for next in edges.get(&node).into_iter().flatten() {
        match state.get(next) {
            Some(Visit::OnStack) => {
                let from = stack.iter().position(|s| s == next).unwrap_or(0);
                let mut cycle = stack[from..].to_vec();
                cycle.push(*next);
                out.push(Invariant::CompoundCycle { cycle });
            }
            Some(Visit::Done) => {}
            None => visit(*next, edges, state, stack, out),
        }
    }
    stack.pop();
    state.insert(node, Visit::Done);
}

/// Keeps one report per linked group.
fn dedup_linked_groups(out: &mut Vec<Invariant>) {
    let mut seen = BTreeSet::new();
    out.retain(|v| match v {
        Invariant::LinkedGroupSpansSequences { group, .. } => seen.insert(*group),
        _ => true,
    });
}

fn check_custom(path: &str, custom: &Custom, out: &mut Vec<Invariant>) {
    for key in custom.invalid_keys() {
        out.push(Invariant::CustomNamespace {
            path: format!("{path}/custom"),
            key: key.clone(),
        });
    }
}

fn check_finite(path: &str, value: f64, out: &mut Vec<Invariant>) {
    if !value.is_finite() {
        out.push(Invariant::InvalidValue {
            path: path.to_owned(),
            reason: format!("must be a finite number, got {value}"),
        });
    }
}

fn check_vec2(path: &str, value: &Animatable<Vec2>, out: &mut Vec<Invariant>) {
    if !value.value().is_finite() {
        out.push(Invariant::InvalidValue {
            path: path.to_owned(),
            reason: "both components must be finite numbers".into(),
        });
    }
}

fn check_track_order(key: SequenceId, sequence: &crate::Sequence, out: &mut Vec<Invariant>) {
    let reason = |text: String| Invariant::TrackOrder {
        sequence: key,
        reason: text,
    };
    let mut seen = BTreeSet::new();
    for id in &sequence.track_order {
        if !seen.insert(*id) {
            out.push(reason(format!("track `{id}` is listed twice")));
        } else if !sequence.tracks.contains_key(id) {
            out.push(reason(format!("track `{id}` is listed but does not exist")));
        }
    }
    for id in sequence.tracks.keys() {
        if !seen.contains(id) {
            out.push(reason(format!("track `{id}` is missing from track_order")));
        }
    }
}

fn check_format(path: &str, format: &SequenceFormat, out: &mut Vec<Invariant>) {
    let mut invalid = |reason: String| {
        out.push(Invariant::InvalidFormat {
            path: path.to_owned(),
            reason,
        });
    };
    for (side, value) in [("width", format.width), ("height", format.height)] {
        if value == 0 || value > MAX_CANVAS_SIDE {
            invalid(format!(
                "{side} must be between 1 and {MAX_CANVAS_SIDE}, got {value}"
            ));
        }
    }
    let rate = format.frame_rate;
    let (num, den) = (u64::from(rate.num()), u64::from(rate.den()));
    if num < den || num > u64::from(MAX_SEQUENCE_FPS) * den {
        invalid(format!(
            "frame rate {rate} must be between 1 and {MAX_SEQUENCE_FPS} fps"
        ));
    } else if !rate.is_exact() {
        invalid(format!(
            "frame rate {rate} is not a whole number of flicks per frame (ADR-0004)"
        ));
    }
    let hz = format.sample_rate.hz();
    if !SEQUENCE_SAMPLE_RATES.contains(&hz) {
        invalid(format!(
            "sample rate {hz} Hz must be between {} and {} Hz",
            SEQUENCE_SAMPLE_RATES.start(),
            SEQUENCE_SAMPLE_RATES.end()
        ));
    } else if !format.sample_rate.is_exact() {
        invalid(format!(
            "sample rate {hz} Hz is not a whole number of flicks per sample (ADR-0004)"
        ));
    }
}
