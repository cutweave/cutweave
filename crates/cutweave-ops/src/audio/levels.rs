// SPDX-License-Identifier: GPL-3.0-or-later
//! `audio.get_levels` (WP-2E.02; arch §9): the peak, true peak, RMS and loudness of a sequence's
//! mix over a range, by offline analysis, for agents (and the mixer panel's numbers).
//!
//! The engine mixes the range as an export would (the same lanes, track buses with their gain,
//! mute and solo, and the master limiter unless `limiter: false`) and measures the master and
//! every track bus. This crate checks the request and shapes the answer; the mixing and
//! measuring belong to the engine ([`LevelsAnalyzer`]), which has the media decoders. A core
//! engine without them answers `Unsupported`.
//!
//! **Units.** Levels are dBFS, `null` when the signal is silent (below −144 dBFS). Peak and true
//! peak are the louder channel's. RMS is on the AES17 scale, the convention of level meters: a
//! sine reads its peak level (a full-scale sine is 0 dBFS RMS), which is the raw RMS + 3.01 dB,
//! over both channels. Loudness is EBU R 128 / ITU-R BS.1770 in LUFS (`null` when everything is
//! below the −70 LUFS gate); the loudness range is in LU. Track levels are post-fader: a muted
//! track, or one a solo silences, reads silence.

use std::sync::Arc;

use cutweave_commands::{
    Annotations, CmdError, CommandSpec, Described, DocSnapshot, EngineCommand, EntryKind,
    McpExposure, Registrable, Registry, RegistryError, Scope, wire,
};
use cutweave_model::{EntityKind, Project, SequenceId, Time, TimeInput, TrackId};

use crate::util::resolve_time;

/// Levels at or below this are silence (`null` on the wire).
pub const SILENCE_DBFS: f64 = -144.0;

/// What the engine measures for one signal.
#[wire]
#[derive(Copy, Default)]
pub struct LevelReading {
    /// Sample peak in dBFS (the louder channel); null when silent.
    #[serde(default)]
    pub peak_db: Option<f64>,
    /// True peak in dBTP (4× oversampled; the louder channel); null when silent.
    #[serde(default)]
    pub true_peak_db: Option<f64>,
    /// RMS in dBFS on the AES17 scale (a sine reads its peak level), over both channels; null
    /// when silent.
    #[serde(default)]
    pub rms_db: Option<f64>,
    /// Integrated loudness in LUFS; null when everything is below the −70 LUFS gate.
    #[serde(default)]
    pub lufs_integrated: Option<f64>,
    /// The loudest momentary loudness (400 ms), sampled every 100 ms, in LUFS (master only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lufs_momentary_max: Option<f64>,
    /// The loudest short-term loudness (3 s), sampled every 100 ms, in LUFS (master only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lufs_short_term_max: Option<f64>,
    /// Loudness range in LU (master only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness_range_lu: Option<f64>,
}

/// One track bus's levels (post-fader).
#[wire]
pub struct TrackLevels {
    /// The audio track.
    pub track: TrackId,
    /// Its levels after its gain, pan, mute and solo.
    pub levels: LevelReading,
}

/// What the engine measured over the range.
#[wire]
pub struct LevelsMeasured {
    /// The master output.
    pub master: LevelReading,
    /// The audio tracks with clips in the range, top to bottom.
    pub tracks: Vec<TrackLevels>,
    /// The deepest gain reduction of the master limiter in dB (0: it never limited).
    pub limiter_reduction_db: f64,
    /// Clips whose audio could not be decoded (they were measured as silence).
    pub source_errors: u32,
}

/// Result of `audio.get_levels`.
#[wire]
pub struct GetLevelsOutput {
    /// The measured sequence.
    pub sequence: SequenceId,
    /// Range start (sequence time, flicks).
    #[serde(rename = "in")]
    pub start: Time,
    /// Range end, exclusive.
    #[serde(rename = "out")]
    pub end: Time,
    /// Whether the master limiter was on (as in an export).
    pub limiter: bool,
    /// The master output.
    pub master: LevelReading,
    /// The audio tracks with clips in the range, top to bottom (post-fader).
    pub tracks: Vec<TrackLevels>,
    /// The deepest gain reduction of the master limiter in dB (0: it never limited).
    pub limiter_reduction_db: f64,
    /// Clips whose audio could not be decoded (measured as silence).
    pub source_errors: u32,
}

/// Measures the audio of a sequence over a range, mixed as an export would be (track gain,
/// mute and solo, the master limiter at −1 dBTP unless `limiter` is false): sample peak, true
/// peak (dBTP), RMS (dBFS, AES17: a sine reads its peak level), integrated loudness (LUFS),
/// the loudest momentary and short-term loudness and the loudness range of the master, and
/// the same levels (without momentary, short-term and range) for every audio track
/// (post-fader). Silent values are null. Read-only; it decodes and mixes the whole range before
/// it answers, so a long range takes a while.
#[wire]
pub struct GetLevels {
    /// Sequence; optional when the project has one.
    #[serde(default)]
    pub sequence: Option<SequenceId>,
    /// Start: flicks, "2.5s", "f:60" or a timecode (default: 0).
    #[serde(default, rename = "in", alias = "start")]
    pub start: Option<TimeInput>,
    /// End, exclusive, at most the sequence end (default: the sequence end).
    #[serde(default, rename = "out", alias = "end")]
    pub end: Option<TimeInput>,
    /// Measure after the master limiter, as an export does (default true).
    #[serde(default)]
    pub limiter: Option<bool>,
}

impl Described for GetLevels {
    const SPEC: CommandSpec = CommandSpec {
        name: "audio.get_levels",
        summary: "Measure peak, true peak, RMS and LUFS loudness of a sequence's mix (and each track) over a range",
        kind: EntryKind::Engine,
        undoable: false,
        view: false,
        scope: Scope::Any,
        mcp: McpExposure::Catalog,
        annotations: Annotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        secret_fields: &[],
        rust_type: "GetLevels",
    };
}

impl Registrable for GetLevels {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<GetLevels>().map(drop)
    }
}

/// A checked `audio.get_levels` request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelsRequest {
    /// The sequence.
    pub sequence: SequenceId,
    /// Range start.
    pub start: Time,
    /// Range end, exclusive (after `start`).
    pub end: Time,
    /// Measure after the limiter.
    pub limiter: bool,
}

/// What mixes and measures (the engine, which has the decoders).
pub trait LevelsAnalyzer: Send + Sync {
    /// Mixes `request`'s range of `doc` and measures it.
    fn analyze(&self, doc: &Project, request: &LevelsRequest) -> Result<LevelsMeasured, CmdError>;
}

/// The context of `audio.get_levels`: the document of the moment and the engine's analyzer.
pub struct LevelsCtx {
    /// The document to measure.
    pub snapshot: Arc<DocSnapshot>,
    /// The analyzer; `None` in an engine without media decoding (`Unsupported`).
    pub analyzer: Option<Arc<dyn LevelsAnalyzer>>,
}

impl GetLevels {
    /// Resolves the sequence and the range against `doc`.
    pub fn request(&self, doc: &Project) -> Result<LevelsRequest, CmdError> {
        let sequence = if let Some(id) = self.sequence {
            doc.sequences
                .get(&id)
                .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, id))?
        } else {
            let mut all = doc.sequences.values();
            match (all.next(), all.next()) {
                (Some(s), None) => s,
                (None, _) => {
                    return Err(CmdError::invalid(
                        "/sequence",
                        "the project has no sequence to measure",
                    ));
                }
                (Some(_), Some(_)) => {
                    return Err(CmdError::invalid(
                        "/sequence",
                        format!(
                            "the project has {} sequences; name the one to measure",
                            doc.sequences.len()
                        ),
                    ));
                }
            }
        };
        let rate = sequence.format.frame_rate;
        let seq_end = Time::from(sequence.duration());
        let start = match &self.start {
            Some(t) => resolve_time("/in", *t, rate)?,
            None => Time::ZERO,
        };
        let end = match &self.end {
            Some(t) => resolve_time("/out", *t, rate)?,
            None => seq_end,
        };
        if start < Time::ZERO {
            return Err(CmdError::invalid("/in", "must not be negative"));
        }
        if end > seq_end {
            return Err(CmdError::invalid(
                "/out",
                format!(
                    "is past the end of the sequence ({} flicks)",
                    seq_end.flicks()
                ),
            ));
        }
        if end <= start {
            return Err(CmdError::invalid(
                if self.end.is_some() { "/out" } else { "/in" },
                if seq_end <= Time::ZERO {
                    "the sequence is empty: there is nothing to measure".to_owned()
                } else {
                    "the range is empty: `out` must be after `in`".to_owned()
                },
            ));
        }
        Ok(LevelsRequest {
            sequence: sequence.id,
            start,
            end,
            limiter: self.limiter.unwrap_or(true),
        })
    }
}

impl EngineCommand for GetLevels {
    type Context = LevelsCtx;
    type Output = GetLevelsOutput;

    fn run(&self, ctx: &mut LevelsCtx) -> Result<GetLevelsOutput, CmdError> {
        let doc = &ctx.snapshot.doc;
        let request = self.request(doc)?;
        let analyzer = ctx.analyzer.as_ref().ok_or_else(|| {
            CmdError::unsupported(
                "measuring audio levels needs the media subsystem (decoders), which this engine \
                 does not run",
            )
        })?;
        let measured = analyzer.analyze(doc, &request)?;
        Ok(GetLevelsOutput {
            sequence: request.sequence,
            start: request.start,
            end: request.end,
            limiter: request.limiter,
            master: measured.master,
            tracks: measured.tracks,
            limiter_reduction_db: measured.limiter_reduction_db,
            source_errors: measured.source_errors,
        })
    }
}

/// `20 log10(linear)` in dB, `None` at or below [`SILENCE_DBFS`] (or not finite).
#[must_use]
pub fn level_db(linear: f64) -> Option<f64> {
    if linear > 0.0 {
        let db = 20.0 * linear.log10();
        (db.is_finite() && db > SILENCE_DBFS).then_some(db)
    } else {
        None
    }
}

/// A loudness or range value, `None` when it is not finite (below the gate).
#[must_use]
pub fn finite(value: f64) -> Option<f64> {
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::{
        FrameRate, Item, ItemId, MediaId, SampleRate, Sequence, SequenceFormat, Track, TrackKind,
    };

    use super::*;

    fn doc(seconds: i64) -> (Project, SequenceId) {
        let mut project = Project::new("Levels");
        let id = SequenceId::from_u128(7);
        let format = SequenceFormat::new(1280, 720, FrameRate::FPS_30, SampleRate::HZ_48_000);
        let mut seq = Sequence::new(id, "Main", format);
        let track = TrackId::from_u128(1);
        seq.insert_track(0, Track::new(track, TrackKind::Audio, "A1"));
        if seconds > 0 {
            let clip = cutweave_model::Clip::new(
                ItemId::from_u128(9),
                MediaId::from_u128(3),
                Time::ZERO,
                cutweave_model::Duration::from_secs(seconds).unwrap(),
            );
            seq.track_mut(&track)
                .unwrap()
                .items
                .insert(Item::from(clip));
        }
        project.sequences.insert(id, seq);
        (project, id)
    }

    fn params(v: serde_json::Value) -> GetLevels {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn the_range_defaults_to_the_whole_sequence_and_is_checked() {
        let (doc, id) = doc(10);
        let r = params(serde_json::json!({})).request(&doc).unwrap();
        assert_eq!(r.sequence, id);
        assert_eq!((r.start, r.end), (Time::ZERO, Time::from_secs(10).unwrap()));
        assert!(r.limiter);
        let r = params(serde_json::json!({"in": "2s", "out": "f:90", "limiter": false}))
            .request(&doc)
            .unwrap();
        assert_eq!(
            (r.start, r.end),
            (Time::from_secs(2).unwrap(), Time::from_secs(3).unwrap())
        );
        assert!(!r.limiter);
        for bad in [
            serde_json::json!({"out": "11s"}),
            serde_json::json!({"in": "3s", "out": "3s"}),
            serde_json::json!({"in": "-1s"}),
            serde_json::json!({"sequence": "seq_00000000000000000000000000000063"}),
        ] {
            assert!(params(bad.clone()).request(&doc).is_err(), "{bad}");
        }
        let (empty, _) = doc_empty();
        assert!(params(serde_json::json!({})).request(&empty).is_err());
    }

    fn doc_empty() -> (Project, SequenceId) {
        doc(0)
    }

    #[test]
    fn without_an_analyzer_it_is_unsupported() {
        let (doc, _) = doc(1);
        let mut ctx = LevelsCtx {
            snapshot: Arc::new(DocSnapshot {
                rev: cutweave_commands::Rev::ZERO,
                committed_rev: cutweave_commands::Rev::ZERO,
                doc,
            }),
            analyzer: None,
        };
        let e = params(serde_json::json!({})).run(&mut ctx).unwrap_err();
        assert_eq!(e.code(), "unsupported");
    }

    #[test]
    fn decibels() {
        assert!((level_db(0.1).unwrap() + 20.0).abs() < 1e-12);
        assert_eq!(level_db(0.0), None);
        assert_eq!(level_db(1e-9), None);
        assert_eq!(finite(f64::NEG_INFINITY), None);
        assert_eq!(finite(-23.0), Some(-23.0));
    }
}
