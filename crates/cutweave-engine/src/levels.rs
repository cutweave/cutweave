// SPDX-License-Identifier: GPL-3.0-or-later
//! Mixing a sequence through the bus graph (WP-2E.02; arch §9): the segments and bus settings
//! playback feeds the mixer ([`mix_of`]) and the analyzer behind `audio.get_levels`
//! ([`EngineLevels`]).
//!
//! **Every audio track goes to the mixer.** `evaluate_audio` leaves out muted tracks and the
//! tracks a solo silences; the mixer's bus graph applies mute and solo itself (ramped, without
//! re-rendering a lane), so [`mix_of`] evaluates a transient copy of the project with the audio
//! tracks' mute and solo flags cleared and returns the flags in the [`BusGraph`]. The copy is
//! never a document change: it lives for one evaluation (like the source preview's transient
//! timeline) and shares everything else with the snapshot.

use std::sync::Arc;

use cutweave_audio::{BusGraph, LevelReport, Levels, LimiterConfig, analyze_levels};
use cutweave_commands::CmdError;
use cutweave_model::{Project, SequenceId, Time, TrackKind};
use cutweave_ops::audio::levels::{finite, level_db};
use cutweave_ops::audio::{
    LevelReading, LevelsAnalyzer, LevelsMeasured, LevelsRequest, TrackLevels,
};
use cutweave_render::{AudioSegment, EvalError, evaluate_audio};

use crate::export::ExportSources;

/// The clips of every audio track of `seq` in `[start, end)` (mute and solo not applied) and
/// the bus settings that apply them (see the module docs).
pub fn mix_of(
    project: &Project,
    seq: SequenceId,
    start: Time,
    end: Time,
) -> Result<(Vec<AudioSegment>, BusGraph), EvalError> {
    let sequence = project
        .sequences
        .get(&seq)
        .ok_or(EvalError::UnknownSequence(seq))?;
    let buses = BusGraph::of_sequence(sequence);
    if !buses.buses().iter().any(|b| b.muted || b.solo) {
        return Ok((evaluate_audio(project, seq, start, end)?, buses));
    }
    let mut open = project.clone();
    if let Some(sequence) = open.sequences.get_mut(&seq) {
        for bus in buses.buses() {
            if let Some(track) = sequence.track_mut(&bus.track)
                && track.kind == TrackKind::Audio
            {
                track.muted = false;
                track.solo = false;
            }
        }
    }
    Ok((evaluate_audio(&open, seq, start, end)?, buses))
}

/// `audio.get_levels` over the engine's media decoders.
pub struct EngineLevels {
    sources: Arc<dyn ExportSources>,
}

impl std::fmt::Debug for EngineLevels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineLevels").finish_non_exhaustive()
    }
}

impl EngineLevels {
    /// An analyzer decoding clips with `sources` (the export's audio sources).
    #[must_use]
    pub fn new(sources: Arc<dyn ExportSources>) -> EngineLevels {
        EngineLevels { sources }
    }
}

/// The AES17 offset: a sine's RMS + 3.01 dB is its peak level.
const AES17_DB: f64 = 3.010_299_956_639_812;

fn reading(levels: &Levels) -> LevelReading {
    let max = |v: [f32; 2]| f64::from(v[0].max(v[1]));
    let mean_square = f64::midpoint(
        f64::from(levels.rms[0]).powi(2),
        f64::from(levels.rms[1]).powi(2),
    );
    LevelReading {
        peak_db: level_db(max(levels.peak)),
        true_peak_db: level_db(max(levels.true_peak)),
        rms_db: level_db(mean_square.sqrt()).map(|db| db + AES17_DB),
        lufs_integrated: finite(levels.integrated),
        lufs_momentary_max: levels.max_momentary.and_then(finite),
        lufs_short_term_max: levels.max_short_term.and_then(finite),
        loudness_range_lu: levels.range.and_then(finite),
    }
}

/// The wire answer of a [`LevelReport`].
#[must_use]
pub fn measured(report: &LevelReport) -> LevelsMeasured {
    LevelsMeasured {
        master: reading(&report.master),
        tracks: report
            .tracks
            .iter()
            .map(|t| TrackLevels {
                track: t.track,
                levels: reading(&t.levels),
            })
            .collect(),
        limiter_reduction_db: level_db(f64::from(report.limiter_gain)).map_or(0.0, |db| -db),
        source_errors: report.source_errors,
    }
}

impl LevelsAnalyzer for EngineLevels {
    fn analyze(&self, doc: &Project, request: &LevelsRequest) -> Result<LevelsMeasured, CmdError> {
        let sequence = doc
            .sequences
            .get(&request.sequence)
            .ok_or_else(|| CmdError::invalid("/sequence", "the sequence is gone"))?;
        let rate = sequence.format.sample_rate;
        let (segments, buses) = mix_of(doc, request.sequence, request.start, request.end)
            .map_err(|e| CmdError::internal(format!("cannot evaluate the audio: {e}")))?;
        let limiter = LimiterConfig {
            enabled: request.limiter,
            ..LimiterConfig::default()
        };
        let report = analyze_levels(
            &self.sources.audio(doc),
            &segments,
            &buses,
            request.start..request.end,
            rate,
            limiter,
        )
        .map_err(|e| CmdError::internal(format!("cannot mix the audio: {e}")))?;
        Ok(measured(&report))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::{
        Clip, Duration, Extra, FrameRate, Item, ItemId, MediaAsset, MediaId, MediaKind, ProbeInfo,
        SampleRate, Sequence, SequenceFormat, StreamInfo, StreamKind, Track, TrackId,
    };

    use super::*;

    #[test]
    fn muted_and_unsoloed_tracks_still_reach_the_mixer() {
        let seq = SequenceId::from_u128(1);
        let format = SequenceFormat::new(640, 360, FrameRate::FPS_30, SampleRate::HZ_48_000);
        let mut sequence = Sequence::new(seq, "S", format);
        let mut project = Project::new("Mix");
        for n in 0..3u128 {
            let id = TrackId::from_u128(10 + n);
            sequence.insert_track(
                usize::try_from(n).unwrap(),
                Track::new(id, TrackKind::Audio, "A"),
            );
            let clip = Clip::new(
                ItemId::from_u128(20 + n),
                MediaId::from_u128(5),
                Time::ZERO,
                Duration::from_secs(1).unwrap(),
            );
            sequence
                .track_mut(&id)
                .unwrap()
                .items
                .insert(Item::from(clip));
        }
        sequence.track_mut(&TrackId::from_u128(10)).unwrap().muted = true;
        sequence.track_mut(&TrackId::from_u128(11)).unwrap().solo = true;
        project.sequences.insert(seq, sequence);
        let mut probe = ProbeInfo::default();
        probe.streams.push(StreamInfo {
            index: 0,
            kind: StreamKind::Audio,
            codec: None,
            profile: None,
            width: None,
            height: None,
            pixel_format: None,
            sample_rate: Some(SampleRate::HZ_48_000),
            channels: Some(2),
            language: None,
            extra: Extra::new(),
        });
        probe.duration = Some(Duration::from_secs(1).unwrap());
        let media = MediaId::from_u128(5);
        project.media.insert(
            media,
            MediaAsset::file(media, MediaKind::Audio, "/m/a.wav".to_owned(), probe),
        );
        let end = Time::from_secs(1).unwrap();
        let heard = evaluate_audio(&project, seq, Time::ZERO, end).unwrap();
        let (all, buses) = mix_of(&project, seq, Time::ZERO, end).unwrap();
        assert_eq!(heard.len(), 1, "only the solo is audible");
        assert_eq!(all.len(), 3, "the mixer gets every track");
        assert!(!buses.is_audible(TrackId::from_u128(10)));
        assert!(buses.is_audible(TrackId::from_u128(11)));
        assert!(!buses.is_audible(TrackId::from_u128(12)));
        // The document itself is untouched.
        assert!(project.sequences[&seq].tracks[&TrackId::from_u128(10)].muted);
    }
}
