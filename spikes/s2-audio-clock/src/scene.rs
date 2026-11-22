// SPDX-License-Identifier: GPL-3.0-or-later
//! Track layouts ("scenes") for the spike.

use crate::synth::SourceKind;
use crate::track::{Speed, TrackSpec};

/// Which set of tracks to play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    /// 8 tracks: click (A/V reference) + 7 synthetic beds at 44.1/48/96 kHz, stretched 0.5×–2×.
    Mix,
    /// Click track only, loud: for camera/mic or loopback A/V offset measurement.
    Sync,
    /// Stereo 1 kHz sine at -23 dBFS through the full chain: integrated loudness must read -23 LUFS.
    RefTone,
}

impl Scene {
    /// Parses `mix`, `sync` or `ref-tone`.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "mix" => Ok(Self::Mix),
            "sync" => Ok(Self::Sync),
            "ref-tone" | "reftone" => Ok(Self::RefTone),
            _ => Err(format!("unknown scene `{s}` (mix | sync | ref-tone)")),
        }
    }

    /// Name used in file names and logs.
    pub fn name(self) -> &'static str {
        match self {
            Self::Mix => "mix",
            Self::Sync => "sync",
            Self::RefTone => "ref-tone",
        }
    }
}

fn tone(partials: &[(f64, f32)], tremolo: Option<(f64, f32)>, detune: f64) -> SourceKind {
    SourceKind::Tone {
        partials: partials.to_vec(),
        tremolo,
        detune,
    }
}

/// Click track (the A/V reference): 44.1 kHz source, resampled, never stretched.
pub fn click_track(interval: f64, gain_db: f32) -> TrackSpec {
    TrackSpec {
        name: "click",
        kind: SourceKind::Clicks {
            interval,
            amp: 0.7,
            freq: 2000.0,
        },
        src_rate: 44_100,
        speed: Speed::Fixed(1.0),
        stretch: false,
        gain_db,
        pan: 0.0,
    }
}

/// Tracks for `scene`, truncated to `max_tracks`.
pub fn tracks(scene: Scene, click_interval: f64, max_tracks: usize) -> Vec<TrackSpec> {
    let mut v = match scene {
        Scene::Sync => vec![click_track(click_interval, 0.0)],
        Scene::RefTone => vec![TrackSpec {
            name: "ref-1k",
            kind: tone(&[(1000.0, 10f32.powf(-23.0 / 20.0))], None, 0.0),
            src_rate: 44_100,
            speed: Speed::Fixed(1.0),
            stretch: false,
            gain_db: 0.0,
            pan: 0.0,
        }],
        Scene::Mix => vec![
            click_track(click_interval, -3.0),
            TrackSpec {
                name: "pad",
                kind: tone(&[(220.0, 0.5), (440.0, 0.2), (660.0, 0.1)], None, 0.7),
                src_rate: 44_100,
                speed: Speed::Fixed(0.5),
                stretch: true,
                gain_db: -20.0,
                pan: -0.5,
            },
            TrackSpec {
                name: "lead",
                kind: tone(&[(440.0, 0.5), (880.0, 0.15)], Some((5.0, 0.3)), 0.0),
                src_rate: 96_000,
                speed: Speed::Fixed(2.0),
                stretch: true,
                gain_db: -24.0,
                pan: 0.5,
            },
            TrackSpec {
                name: "noise",
                kind: SourceKind::NoiseBursts {
                    period: 1.5,
                    burst: 0.2,
                    amp: 0.5,
                },
                src_rate: 96_000,
                speed: Speed::Fixed(1.0),
                stretch: true,
                gain_db: -26.0,
                pan: -0.3,
            },
            TrackSpec {
                name: "chord",
                kind: tone(&[(261.63, 0.3), (329.63, 0.3), (392.0, 0.3)], None, 0.5),
                src_rate: 48_000,
                speed: Speed::Fixed(0.75),
                stretch: true,
                gain_db: -24.0,
                pan: 0.3,
            },
            TrackSpec {
                name: "bass",
                kind: tone(&[(55.0, 0.6), (110.0, 0.3)], None, 0.0),
                src_rate: 44_100,
                speed: Speed::Fixed(1.5),
                stretch: true,
                gain_db: -18.0,
                pan: 0.0,
            },
            TrackSpec {
                name: "ticks",
                kind: SourceKind::NoiseBursts {
                    period: 0.25,
                    burst: 0.015,
                    amp: 0.6,
                },
                src_rate: 44_100,
                speed: Speed::Sweep {
                    min: 0.5,
                    max: 2.0,
                    period_s: 20.0,
                },
                stretch: true,
                gain_db: -28.0,
                pan: 0.7,
            },
            TrackSpec {
                name: "drone",
                kind: tone(&[(82.41, 0.4), (123.47, 0.3)], Some((0.25, 0.5)), 0.3),
                src_rate: 96_000,
                speed: Speed::Fixed(1.25),
                stretch: true,
                gain_db: -26.0,
                pan: -0.7,
            },
        ],
    };
    v.truncate(max_tracks.max(1));
    v
}
