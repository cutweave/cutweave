// SPDX-License-Identifier: GPL-3.0-or-later
//! The bus graph (WP-2E.02, arch §9): one bus per audio track with gain, pan, mute and solo, and
//! the master bus (the true-peak [`Limiter`], then the monitoring volume).
//!
//! ```text
//! lane (Σ clips) ─▶ track bus: × gain × pan, mute/solo (ramped) ─▶ Σ ─▶ master: limiter ─▶ × volume ─▶ out
//!                   └ meters: peak, RMS (post-fader)                         └ meters: peak, RMS, LUFS, gain reduction
//! ```
//!
//! The master volume (`view.set{master_gain}`) is a monitoring setting, so it comes after the
//! limiter: turning playback up or down never changes how the mix is limited, and what plays is
//! the export's audio times the volume.
//!
//! **Solo is exclusive solo-in-place.** When any audio track is soloed, every audio track that
//! is not soloed is silent; video tracks are not affected (they have no buses). Mute wins over
//! solo: a muted track is silent even when it is soloed, and it still counts as a solo, so a
//! track that is soloed and muted silences everything ([`BusGraph::is_audible`]).
//!
//! **Pan** is a balance (the bus carries the lane's stereo mix): unity at the centre, the far
//! side attenuated with the constant-power curve (the stereo law of the clip pan, WP-2E.01).
//! The model has no track pan yet (ADR-0034, Proposed): buses read `0` from the document and
//! [`BusParams::pan`] is set only in code.
//!
//! **Changes ramp.** The real-time mixer moves a bus's gains to new values over the mixer's ramp
//! length (10 ms), so muting, soloing and gain changes during playback never click, and the
//! lane keeps its prefetched audio (a muted track keeps rendering, so unmuting is immediate).
//! The offline mixdown starts at the final values: in a steady state both are bit-identical.

use cutweave_model::{Sequence, TrackId, TrackKind};
use cutweave_render::AudioSegment;

use crate::automation::pan_gains;
use crate::dsp::{CHANNELS, Ramp, db_to_gain};
use crate::limiter::Limiter;

/// One track bus's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusParams {
    /// The track.
    pub track: TrackId,
    /// Track gain in dB (NaN is unity, ≤ −144 dB is silence, above +24 dB is clamped).
    pub gain_db: f64,
    /// Balance, `-1` (left) … `1` (right).
    pub pan: f64,
    /// Silenced.
    pub muted: bool,
    /// Soloed.
    pub solo: bool,
}

impl BusParams {
    /// A unity bus for `track`: 0 dB, centred, neither muted nor soloed.
    #[must_use]
    pub const fn new(track: TrackId) -> BusParams {
        BusParams {
            track,
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            solo: false,
        }
    }

    /// The bus's left and right gains, mute and solo aside.
    #[must_use]
    pub fn fader_gains(&self) -> [f32; CHANNELS] {
        let g = f64::from(db_to_gain(self.gain_db));
        let [l, r] = pan_gains(if self.pan.is_nan() { 0.0 } else { self.pan }, false);
        #[allow(clippy::cast_possible_truncation)]
        [(g * l) as f32, (g * r) as f32]
    }
}

/// The settings of every track bus of a mix, in track order (see the module docs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BusGraph {
    buses: Vec<BusParams>,
}

impl BusGraph {
    /// No buses: every track plays at its segments' track gain.
    #[must_use]
    pub fn new() -> BusGraph {
        BusGraph::default()
    }

    /// The buses of `sequence`'s audio tracks, top to bottom: the track's gain, mute and solo
    /// (pan 0 until the model has a track pan).
    #[must_use]
    pub fn of_sequence(sequence: &Sequence) -> BusGraph {
        BusGraph {
            buses: sequence
                .tracks_in_order()
                .filter(|t| t.kind == TrackKind::Audio)
                .map(|t| BusParams {
                    track: t.id,
                    gain_db: t.gain,
                    pan: 0.0,
                    muted: t.muted,
                    solo: t.solo,
                })
                .collect(),
        }
    }

    /// One unmuted, unsoloed bus per track of `segments`, at the track gain the segments carry
    /// (for callers that evaluated audible tracks only).
    #[must_use]
    pub fn from_segments(segments: &[AudioSegment]) -> BusGraph {
        let mut graph = BusGraph::new();
        for seg in segments {
            if graph.get(seg.track).is_none() {
                graph.buses.push(BusParams {
                    gain_db: seg.track_gain_db,
                    ..BusParams::new(seg.track)
                });
            }
        }
        graph
    }

    /// Sets (or adds, at the end) the bus of `params.track`.
    pub fn set(&mut self, params: BusParams) {
        match self.buses.iter_mut().find(|b| b.track == params.track) {
            Some(b) => *b = params,
            None => self.buses.push(params),
        }
    }

    /// [`set`](Self::set), by value.
    #[must_use]
    pub fn with(mut self, params: BusParams) -> BusGraph {
        self.set(params);
        self
    }

    /// The buses, in track order.
    #[must_use]
    pub fn buses(&self) -> &[BusParams] {
        &self.buses
    }

    /// The bus of `track`.
    #[must_use]
    pub fn get(&self, track: TrackId) -> Option<&BusParams> {
        self.buses.iter().find(|b| b.track == track)
    }

    /// Whether any bus is soloed (muted or not).
    #[must_use]
    pub fn any_solo(&self) -> bool {
        self.buses.iter().any(|b| b.solo)
    }

    /// Whether `track` is heard: not muted, and soloed when any bus is. A track the graph does
    /// not know is heard unless a bus is soloed.
    #[must_use]
    pub fn is_audible(&self, track: TrackId) -> bool {
        let solo = self.any_solo();
        match self.get(track) {
            Some(b) => !b.muted && (!solo || b.solo),
            None => !solo,
        }
    }

    /// The left and right gains of `track`'s bus with mute and solo applied (`[0, 0]` when it is
    /// not heard); `None` when the graph does not know the track.
    #[must_use]
    pub fn gains(&self, track: TrackId) -> Option<[f32; CHANNELS]> {
        let bus = self.get(track)?;
        Some(if self.is_audible(track) {
            bus.fader_gains()
        } else {
            [0.0; CHANNELS]
        })
    }

    /// [`gains`](Self::gains), or for a track the graph does not know, unity at
    /// `fallback_db` (silent while a bus is soloed).
    #[must_use]
    pub fn gains_or(&self, track: TrackId, fallback_db: f64) -> [f32; CHANNELS] {
        self.gains(track).unwrap_or_else(|| {
            if self.any_solo() {
                [0.0; CHANNELS]
            } else {
                let g = db_to_gain(fallback_db);
                [g, g]
            }
        })
    }
}

/// Left and right gains that move linearly to new targets over a fixed number of frames, and
/// are exact constants otherwise (the stereo form of [`Ramp`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StereoRamp {
    current: [f32; CHANNELS],
    target: [f32; CHANNELS],
    step: [f32; CHANNELS],
    left: u32,
}

impl StereoRamp {
    /// Steady gains.
    pub const fn at(gains: [f32; CHANNELS]) -> StereoRamp {
        StereoRamp {
            current: gains,
            target: gains,
            step: [0.0; CHANNELS],
            left: 0,
        }
    }

    /// Heads to `target` over `frames` frames from the current values (at once when `frames`
    /// is 0). Targets equal to the current ones leave the ramp untouched.
    #[allow(clippy::cast_precision_loss)]
    pub fn set_target(&mut self, target: [f32; CHANNELS], frames: u32) {
        if target.map(f32::to_bits) == self.target.map(f32::to_bits) {
            return;
        }
        self.target = target;
        if frames == 0 {
            self.current = target;
            self.left = 0;
        } else {
            self.step = [
                (target[0] - self.current[0]) / frames as f32,
                (target[1] - self.current[1]) / frames as f32,
            ];
            self.left = frames;
        }
    }

    /// `buf *= gains`, frame by frame (interleaved stereo).
    pub fn apply(&mut self, buf: &mut [f32]) {
        let (frames, _) = buf.as_chunks_mut::<CHANNELS>();
        if self.left == 0 {
            let [l, r] = self.current;
            for f in frames {
                f[0] *= l;
                f[1] *= r;
            }
            return;
        }
        for f in frames {
            let [l, r] = self.next();
            f[0] *= l;
            f[1] *= r;
        }
    }

    #[inline]
    fn next(&mut self) -> [f32; CHANNELS] {
        if self.left == 0 {
            return self.current;
        }
        self.left -= 1;
        if self.left == 0 {
            self.current = self.target;
            self.step = [0.0; CHANNELS];
        } else {
            self.current = [
                self.current[0] + self.step[0],
                self.current[1] + self.step[1],
            ];
        }
        self.current
    }
}

/// The track bus stage shared by the real-time mixer and the offline mixdown: `lane` gets the
/// bus gains (in place, so meters read the post-fader signal), then is added to `out`.
pub(crate) fn mix_bus(gains: &mut StereoRamp, lane: &mut [f32], out: &mut [f32]) {
    gains.apply(lane);
    for (o, s) in out.iter_mut().zip(lane.iter()) {
        *o += *s;
    }
}

/// Peak and energy of a signal over a meter window, per channel.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LevelAccumulator {
    pub peak: [f32; CHANNELS],
    pub sum_sq: [f64; CHANNELS],
    pub frames: u64,
}

impl LevelAccumulator {
    /// Adds the interleaved stereo `buf`.
    pub fn add(&mut self, buf: &[f32]) {
        let (frames, _) = buf.as_chunks::<CHANNELS>();
        for f in frames {
            for c in 0..CHANNELS {
                let v = f[c];
                self.peak[c] = self.peak[c].max(v.abs());
                self.sum_sq[c] += f64::from(v) * f64::from(v);
            }
        }
        self.frames += frames.len() as u64;
    }

    /// The RMS of each channel (0 when nothing was added).
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    pub fn rms(&self) -> [f32; CHANNELS] {
        if self.frames == 0 {
            return [0.0; CHANNELS];
        }
        let n = self.frames as f64;
        self.sum_sq.map(|s| (s / n).sqrt() as f32)
    }

    /// The reading so far, then starts again.
    pub fn take(&mut self) -> ([f32; CHANNELS], [f32; CHANNELS]) {
        let out = (self.peak, self.rms());
        *self = LevelAccumulator::default();
        out
    }
}

/// The level of one track bus over a meter window (linear, post-fader).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusLevel {
    /// The track.
    pub track: TrackId,
    /// Peak absolute sample value, left and right, after the bus gain, pan, mute and solo.
    pub peak: [f32; CHANNELS],
    /// RMS, left and right (a full-scale sine reads 0.707).
    pub rms: [f32; CHANNELS],
}

impl BusLevel {
    /// A silent entry (placeholder in fixed-size arrays).
    pub const SILENT: BusLevel = BusLevel {
        track: TrackId::from_u128(0),
        peak: [0.0; CHANNELS],
        rms: [0.0; CHANNELS],
    };
}

/// The master bus: the limiter, then the gain (the monitoring volume in playback, unity
/// offline).
pub(crate) struct MasterBus {
    pub gain: Ramp,
    pub limiter: Limiter,
}

impl MasterBus {
    /// Applies the master stage to `buf` in place (delayed by the limiter's latency).
    pub fn process(&mut self, buf: &mut [f32]) {
        self.limiter.process(buf);
        self.gain.scale(buf);
    }

    /// Frames the master delays its input.
    pub fn latency(&self) -> usize {
        self.limiter.latency()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use super::*;

    fn t(n: u128) -> TrackId {
        TrackId::from_u128(n)
    }

    fn graph(spec: &[(u128, bool, bool)]) -> BusGraph {
        let mut g = BusGraph::new();
        for &(id, muted, solo) in spec {
            g.set(BusParams {
                muted,
                solo,
                ..BusParams::new(t(id))
            });
        }
        g
    }

    fn heard(g: &BusGraph) -> Vec<u128> {
        g.buses()
            .iter()
            .filter(|b| g.is_audible(b.track))
            .map(|b| b.track.as_u128())
            .collect()
    }

    #[test]
    fn no_solo_plays_everything_unmuted() {
        let g = graph(&[(1, false, false), (2, true, false), (3, false, false)]);
        assert_eq!(heard(&g), [1, 3]);
        assert_eq!(g.gains(t(2)), Some([0.0, 0.0]));
        assert_eq!(g.gains(t(1)), Some([1.0, 1.0]));
        assert_eq!(g.gains(t(9)), None);
        assert!(
            g.is_audible(t(9)),
            "unknown tracks play while nothing is soloed"
        );
    }

    #[test]
    fn one_solo_silences_the_others() {
        let g = graph(&[(1, false, false), (2, false, true), (3, false, false)]);
        assert_eq!(heard(&g), [2]);
        assert!(!g.is_audible(t(9)), "unknown tracks are not soloed");
        assert_eq!(g.gains_or(t(9), 0.0), [0.0, 0.0]);
    }

    #[test]
    fn several_solos_are_heard_together() {
        let g = graph(&[
            (1, false, true),
            (2, false, false),
            (3, false, true),
            (4, false, false),
        ]);
        assert_eq!(heard(&g), [1, 3]);
    }

    #[test]
    fn mute_wins_over_solo_and_still_counts_as_a_solo() {
        // Soloed and muted: silent, and the solo still silences the unsoloed tracks.
        let g = graph(&[(1, true, true), (2, false, false)]);
        assert_eq!(heard(&g), Vec::<u128>::new());
        // With another solo: only the unmuted solo plays.
        let g = graph(&[(1, true, true), (2, false, true), (3, false, false)]);
        assert_eq!(heard(&g), [2]);
        // A muted, unsoloed track stays muted when another is soloed.
        let g = graph(&[(1, true, false), (2, false, true)]);
        assert_eq!(heard(&g), [2]);
        // Unsoloing the last solo brings back every unmuted track.
        let mut g = graph(&[(1, false, false), (2, true, false), (3, false, true)]);
        g.set(BusParams::new(t(3)));
        assert_eq!(heard(&g), [1, 3]);
    }

    #[test]
    fn gain_and_pan() {
        let mut p = BusParams::new(t(1));
        p.gain_db = -6.020_6;
        let [l, r] = p.fader_gains();
        assert!((l - 0.5).abs() < 1e-5 && (r - 0.5).abs() < 1e-5);
        p = BusParams::new(t(1));
        assert_eq!(p.fader_gains(), [1.0, 1.0], "centre is exact unity");
        p.pan = 1.0;
        let [l, r] = p.fader_gains();
        assert!(l.abs() < 1e-7 && r == 1.0, "hard right: {l} {r}");
        p.pan = -0.5;
        let [l, r] = p.fader_gains();
        assert_eq!(l, 1.0);
        assert!(r < 1.0 && r > 0.5);
        p.pan = f64::NAN;
        assert_eq!(p.fader_gains(), [1.0, 1.0]);
    }

    #[test]
    fn from_segments_keeps_the_first_track_gain() {
        use cutweave_model::{Animatable, Duration, Fade, ItemId, MediaId, SampleRate, Time};
        let seg = |track: u128, db: f64| AudioSegment {
            item: ItemId::from_u128(1),
            track: t(track),
            media: MediaId::from_u128(1),
            stream: None,
            dst_start: Time::ZERO,
            dst_duration: Duration::from_flicks(1).unwrap(),
            src_start: Time::ZERO,
            clip_gain_db: 0.0,
            clip_start: Time::ZERO,
            clip_duration: Duration::from_flicks(1).unwrap(),
            gain: Animatable::Static(0.0),
            pan: Animatable::Static(0.0),
            fade_in: Fade::default(),
            fade_out: Fade::default(),
            track_gain_db: db,
            reverse: false,
            speed: (1, 1),
            sample_rate: SampleRate::HZ_48_000,
        };
        let g = BusGraph::from_segments(&[seg(2, -6.0), seg(1, 0.0), seg(2, -3.0)]);
        assert_eq!(g.buses().len(), 2);
        assert_eq!(g.buses()[0].track, t(2));
        assert_eq!(g.get(t(2)).unwrap().gain_db, -6.0);
    }

    #[test]
    fn stereo_ramps_reach_their_targets_exactly_and_split_alike() {
        let src = vec![1.0f32; 2 * 100];
        let run = |cuts: &[usize]| {
            let mut ramp = StereoRamp::at([1.0, 1.0]);
            ramp.set_target([0.0, 0.5], 64);
            let mut buf = src.clone();
            let mut at = 0;
            for &c in cuts.iter().chain(std::iter::once(&100)) {
                ramp.apply(&mut buf[at * 2..c * 2]);
                at = c;
            }
            (buf, ramp)
        };
        let (a, ramp) = run(&[]);
        let (b, _) = run(&[17, 40]);
        assert_eq!(a, b);
        assert_eq!(a[2 * 63], 0.0);
        assert_eq!(a[2 * 63 + 1], 0.5);
        assert_eq!(ramp, StereoRamp::at([0.0, 0.5]));
    }

    #[test]
    fn level_accumulator_reads_peak_and_rms() {
        let mut acc = LevelAccumulator::default();
        let n = 4800;
        let buf: Vec<f32> = (0..n)
            .flat_map(|i| {
                #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
                let v = (0.5
                    * (2.0 * std::f64::consts::PI * 1000.0 * f64::from(i) / 48_000.0).sin())
                    as f32;
                [v, 0.0]
            })
            .collect();
        acc.add(&buf);
        let (peak, rms) = acc.take();
        assert!((peak[0] - 0.5).abs() < 1e-3);
        assert!((rms[0] - 0.5 / std::f32::consts::SQRT_2).abs() < 1e-4);
        assert_eq!(peak[1], 0.0);
        assert_eq!(acc.take(), ([0.0; 2], [0.0; 2]));
    }
}
