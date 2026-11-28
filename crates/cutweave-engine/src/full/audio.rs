// SPDX-License-Identifier: GPL-3.0-or-later
//! Playback audio of the full engine (WP-1I.05; arch §9): the mixer fed from the current
//! document's clips, played on the output `EngineConfig::audio` names, and the degrade rule:
//! when the device cannot start, the mixer plays on the null output (a wall-clock-driven clock)
//! and `engine.degraded{subsystem: "audio"}` is sent.
//!
//! **What the mixer plays** ([`AudioFeed`], ADR-0022): the sound of the transport's target, the
//! active sequence's clips with its track buses (gain, mute, exclusive solo; WP-2E.02:
//! [`mix_of`]) or one source's clip (its transient timeline, built like the preview's), from the
//! document's current revision. The document watcher feeds it after every change; the transport
//! switches it ([`TargetedAudio`]) under its control lock, paused, before the seek that primes
//! the new target's sound at its playhead. Muting or soloing a track ramps in at once: the lanes
//! of muted tracks keep playing silently.
//!
//! **Meters** ([`MeterPump`], WP-2E.02): while playing, the output publishes a meter frame 30
//! times a second; a thread drains them every 15 ms and sends each as `playback.meters`
//! ([`PlaybackMeters`]): per track bus peak and RMS, and the master's peak, RMS, true peak, LUFS
//! (momentary, short-term, integrated since the last seek) and limiter gain reduction.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::Duration as StdDuration;

use cutweave_audio::{
    AudioClock, AudioOutput, BusGraph, BusLevel, ManualClock, MeterFrame, Mixer, MixerConfig,
    MixerHandle, OutputConfig, PcmSource, SourceError, SourceFactory, SourceRequest,
};
use cutweave_commands::DocHandle;
use cutweave_media::MediaBackend;
use cutweave_model::{Project, SampleRate, SequenceId, Time, TrackId};
use cutweave_playback::transport::{AudioControl, PlaybackTarget, VirtualClock};
use cutweave_render::{AudioSegment, EvalError};
use serde::Serialize;

use super::preview::{SOURCE_SEQUENCE, Shown, active_sequence, source_timeline};
use crate::config::AudioConfig;
use crate::export::DecoderPcmFactory;
use crate::levels::mix_of;
use crate::subsystems::{SubsystemError, SubsystemEvents, SubsystemKind};

pub use crate::subsystems::METERS_EVENT as PLAYBACK_METERS_EVENT;
/// How often the meter thread looks for new frames.
const METER_POLL: StdDuration = StdDuration::from_millis(15);
/// Levels at or below this are reported as `null` (silence).
const METER_FLOOR_DB: f64 = -144.0;

/// `20 log10(x)`, `None` for silence (at or below −144 dBFS).
fn meter_db(linear: f32) -> Option<f64> {
    let linear = f64::from(linear);
    (linear > 0.0)
        .then(|| 20.0 * linear.log10())
        .filter(|db| db.is_finite() && *db > METER_FLOOR_DB)
}

/// RMS in dBFS on the AES17 scale (a sine reads its peak level: raw RMS + 3.01 dB).
fn rms_db(linear: f32) -> Option<f64> {
    meter_db(linear).map(|db| db + 3.010_299_956_639_812)
}

fn lufs(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

/// The master's meters in a `playback.meters` event.
#[derive(Clone, Debug, PartialEq, Serialize, specta::Type)]
pub struct MasterMeters {
    /// Sample peak over the window, left and right, dBFS after the limiter (null: silence).
    pub peak_db: [Option<f64>; 2],
    /// RMS over the window, left and right, dBFS on the AES17 scale (a sine reads its peak).
    pub rms_db: [Option<f64>; 2],
    /// True peak over the window, both channels, dBTP.
    pub true_peak_db: Option<f64>,
    /// Momentary loudness (the last 400 ms), LUFS; null until measurable.
    pub lufs_momentary: Option<f64>,
    /// Short-term loudness (the last 3 s), LUFS.
    pub lufs_short_term: Option<f64>,
    /// Integrated loudness since the last seek, LUFS; null below the −70 LUFS gate.
    pub lufs_integrated: Option<f64>,
    /// The deepest gain reduction of the master limiter in the window, dB (0: none).
    pub gr_db: f64,
}

/// One track bus in a `playback.meters` event (post-fader).
#[derive(Clone, Debug, PartialEq, Serialize, specta::Type)]
pub struct BusMeters {
    /// The audio track.
    pub track: TrackId,
    /// Sample peak over the window, left and right, dBFS (null: silence).
    pub peak_db: [Option<f64>; 2],
    /// RMS over the window, left and right, dBFS on the AES17 scale.
    pub rms_db: [Option<f64>; 2],
}

/// The `playback.meters` payload.
#[derive(Clone, Debug, PartialEq, Serialize, specta::Type)]
pub struct PlaybackMeters {
    /// Timeline time (flicks) of the end of the window, as heard.
    pub t: Time,
    /// The master bus.
    pub master: MasterMeters,
    /// The track buses with clips, top to bottom (at most 32).
    pub buses: Vec<BusMeters>,
}

impl PlaybackMeters {
    /// The event of one meter frame.
    #[must_use]
    pub fn of(frame: &MeterFrame) -> PlaybackMeters {
        let bus = |b: &BusLevel| BusMeters {
            track: b.track,
            peak_db: b.peak.map(meter_db),
            rms_db: b.rms.map(rms_db),
        };
        PlaybackMeters {
            t: frame.at,
            master: MasterMeters {
                peak_db: frame.peak.map(meter_db),
                rms_db: frame.rms.map(rms_db),
                true_peak_db: meter_db(frame.true_peak),
                lufs_momentary: lufs(frame.loudness.momentary),
                lufs_short_term: lufs(frame.loudness.short_term),
                lufs_integrated: lufs(frame.loudness.integrated),
                gr_db: meter_db(frame.limiter_gain).map_or(0.0, |db| (-db).max(0.0)),
            },
            buses: frame.buses().iter().map(bus).collect(),
        }
    }
}

/// The thread that sends the output's meter frames as `playback.meters` events.
pub(crate) struct MeterPump {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MeterPump {
    /// Starts draining `output`'s meters into `events`.
    pub(crate) fn spawn(
        output: Arc<AudioOutput>,
        events: SubsystemEvents,
    ) -> std::io::Result<MeterPump> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("cutweave-audio-meters".into())
            .spawn(move || {
                let mut frames = Vec::with_capacity(16);
                while !flag.load(Ordering::Acquire) {
                    std::thread::park_timeout(METER_POLL);
                    output.drain_meters(&mut frames);
                    for frame in frames.drain(..) {
                        match serde_json::to_value(PlaybackMeters::of(&frame)) {
                            Ok(payload) => events.publish(PLAYBACK_METERS_EVENT, payload),
                            Err(e) => tracing::debug!(error = %e, "cannot encode the meters"),
                        }
                    }
                }
            })?;
        Ok(MeterPump {
            stop,
            thread: Some(thread),
        })
    }

    /// Stops the thread (it lets go of the output).
    pub(crate) fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            if thread.join().is_err() {
                tracing::error!("the audio meter thread panicked");
            }
        }
    }
}

impl Drop for MeterPump {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Where playback audio goes.
#[derive(Clone, Debug, Default)]
pub enum AudioMode {
    /// `EngineConfig::audio`: the device `CUTWEAVE_AUDIO` names (default: the system default
    /// output) for [`AudioConfig::Default`], the null output for [`AudioConfig::Null`].
    #[default]
    FromEngine,
    /// No output: 1× playback follows `clock`, which the caller advances; everything else
    /// the transport times reads `host` (deterministic tests).
    Manual {
        /// The master clock.
        clock: Arc<ManualClock>,
        /// The host clock.
        host: VirtualClock,
    },
    /// The device output fails to start with this reason (tests of the degrade rule).
    FailDevice(String),
}

/// The mixer's clip sources: the current document's media, decoded with the media backend.
/// Re-pointed at every document change ([`set_doc`](Self::set_doc)); opened by the mixer's
/// worker threads, never the real-time thread.
pub(crate) struct LivePcm {
    backend: Option<Arc<dyn MediaBackend>>,
    inner: RwLock<Option<Arc<DecoderPcmFactory>>>,
}

impl LivePcm {
    pub(crate) fn new(backend: Option<Arc<dyn MediaBackend>>) -> LivePcm {
        LivePcm {
            backend,
            inner: RwLock::new(None),
        }
    }

    /// Resolves media against `doc` from now on (`None`: no document, every clip is silent).
    pub(crate) fn set_doc(&self, doc: Option<&Project>) {
        let next = match (&self.backend, doc) {
            (Some(backend), Some(doc)) => {
                Some(Arc::new(DecoderPcmFactory::new(Arc::clone(backend), doc)))
            }
            _ => None,
        };
        *self.inner.write().unwrap_or_else(PoisonError::into_inner) = next;
    }
}

impl std::fmt::Debug for LivePcm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LivePcm")
            .field("backend", &self.backend.is_some())
            .finish_non_exhaustive()
    }
}

impl SourceFactory for LivePcm {
    fn open(&self, request: &SourceRequest) -> Result<Box<dyn PcmSource>, SourceError> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        inner
            .ok_or(SourceError::UnknownMedia(request.media))?
            .open(request)
    }
}

/// The sound of `target` in `doc`: the clips of the sequence the editor shows (`shown`, else
/// the first one) over its whole length with its track buses (mute and solo applied by the
/// buses), or the one clip of a source's transient timeline (nothing when its asset is gone).
pub(crate) fn segments_of(
    doc: &Project,
    target: PlaybackTarget,
    shown: Option<SequenceId>,
) -> Result<(Vec<AudioSegment>, BusGraph), EvalError> {
    let (project, seq) = match target {
        PlaybackTarget::Sequence => match active_sequence(doc, shown) {
            Some(seq) => (None, seq),
            None => return Ok((Vec::new(), BusGraph::new())),
        },
        PlaybackTarget::Media(media) => match source_timeline(doc, media) {
            Some(project) => (Some(project), SOURCE_SEQUENCE),
            None => return Ok((Vec::new(), BusGraph::new())),
        },
    };
    let project = project.as_ref().unwrap_or(doc);
    let end = project
        .sequences
        .get(&seq)
        .map_or(Time::ZERO, |s| Time::from(s.duration()));
    mix_of(project, seq, Time::ZERO, end)
}

/// What the mixer plays (see the module docs): the sound of the transport's target in the
/// open document. Without a mixer (manual audio in tests) it only follows the target.
pub(crate) struct AudioFeed {
    mixer: Option<Arc<MixerHandle>>,
    doc: DocHandle,
    shown: Arc<Shown>,
    target: Mutex<PlaybackTarget>,
}

impl std::fmt::Debug for AudioFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioFeed")
            .field("mixer", &self.mixer.is_some())
            .field("target", &self.target())
            .finish_non_exhaustive()
    }
}

impl AudioFeed {
    /// The feed of `doc`'s sound into `mixer`, playing the sequence.
    pub(crate) fn new(
        mixer: Option<Arc<MixerHandle>>,
        doc: DocHandle,
        shown: Arc<Shown>,
    ) -> AudioFeed {
        AudioFeed {
            mixer,
            doc,
            shown,
            target: Mutex::new(PlaybackTarget::Sequence),
        }
    }

    /// The target whose sound the mixer plays.
    pub(crate) fn target(&self) -> PlaybackTarget {
        *self.target.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The mixer plays `target` from now on (the document's current revision).
    pub(crate) fn set_target(&self, target: PlaybackTarget) {
        let mut current = self.target.lock().unwrap_or_else(PoisonError::into_inner);
        *current = target;
        let snapshot = self.doc.snapshot();
        self.feed(&snapshot.doc, target);
    }

    /// Another sequence shows: the mixer plays its target's sound in the document as it is.
    pub(crate) fn shown_changed(&self) {
        let target = self.target.lock().unwrap_or_else(PoisonError::into_inner);
        let snapshot = self.doc.snapshot();
        self.feed(&snapshot.doc, *target);
    }

    /// The document changed: the mixer plays its target's sound in `doc`.
    pub(crate) fn document(&self, doc: &Project) {
        let target = self.target.lock().unwrap_or_else(PoisonError::into_inner);
        self.feed(doc, *target);
    }

    fn feed(&self, doc: &Project, target: PlaybackTarget) {
        let Some(mixer) = &self.mixer else { return };
        match segments_of(doc, target, self.shown.get()) {
            Ok((segments, buses)) => {
                if let Err(e) = mixer.set_mix(&segments, &buses) {
                    tracing::warn!(error = %e, ?target, "the mixer refused the audio to play");
                }
            }
            Err(e) => tracing::warn!(error = %e, ?target, "cannot evaluate the audio to play"),
        }
    }
}

/// The transport's audio in the full engine: the output (or the manual clock) plus the
/// [`AudioFeed`], so a new target's sound is in the mixer before the next seek primes it.
pub(crate) struct TargetedAudio {
    inner: Box<dyn AudioControl>,
    feed: Arc<AudioFeed>,
}

impl TargetedAudio {
    pub(crate) fn new(inner: Box<dyn AudioControl>, feed: Arc<AudioFeed>) -> TargetedAudio {
        TargetedAudio { inner, feed }
    }
}

impl AudioControl for TargetedAudio {
    fn clock(&self) -> AudioClock {
        self.inner.clock()
    }

    fn play(&mut self) {
        self.inner.play();
    }

    fn pause(&mut self) {
        self.inner.pause();
    }

    fn seek(&mut self, t: Time) {
        self.inner.seek(t);
    }

    fn set_target(&mut self, target: PlaybackTarget) {
        self.inner.set_target(target);
        self.feed.set_target(target);
    }
}

/// The running audio side of playback.
pub(crate) enum Audio {
    /// An output (device or null) playing the mixer.
    Output {
        // Field order is drop order: the meter thread lets go of the output, the output stops
        // before the mixer handle (WP-1G.02).
        meters: Option<MeterPump>,
        output: Arc<AudioOutput>,
        mixer: Arc<MixerHandle>,
    },
    /// A manual clock (tests).
    Manual {
        clock: Arc<ManualClock>,
        host: VirtualClock,
    },
}

impl std::fmt::Debug for Audio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Audio::Output { output, .. } => f.debug_tuple("Output").field(output).finish(),
            Audio::Manual { .. } => f.write_str("Manual"),
        }
    }
}

impl Audio {
    /// The mixer handle, when audio really plays.
    pub(crate) fn mixer(&self) -> Option<&Arc<MixerHandle>> {
        match self {
            Audio::Output { mixer, .. } => Some(mixer),
            Audio::Manual { .. } => None,
        }
    }

    /// Silence while no document plays: paused, no segments, the playhead back at 0.
    pub(crate) fn idle(&self) {
        if let Audio::Output { output, mixer, .. } = self {
            output.pause();
            if let Err(e) = mixer.set_segments(&[]) {
                tracing::debug!(error = %e, "cannot clear the mixer");
            }
            mixer.seek(Time::ZERO);
        }
    }

    /// Stops the meter thread, the output, then the mixer workers (in that order, WP-1G.02).
    pub(crate) fn stop(self) {
        if let Audio::Output {
            meters,
            output,
            mixer,
        } = self
        {
            if let Some(meters) = meters {
                meters.stop();
            }
            match Arc::try_unwrap(output) {
                Ok(output) => drop(output.stop()),
                Err(shared) => {
                    tracing::warn!("the audio output is still shared at shutdown");
                    drop(shared);
                }
            }
            drop(mixer);
        }
    }
}

/// Starts playback audio (see the module docs) with its meter thread publishing to `events`;
/// also returns why it degraded to the null output, if it did.
pub(crate) fn start(
    mode: &AudioMode,
    engine: AudioConfig,
    sources: &Arc<LivePcm>,
    events: &SubsystemEvents,
) -> Result<(Audio, Option<String>), SubsystemError> {
    let (config, fail) = match mode {
        AudioMode::Manual { clock, host } => {
            return Ok((
                Audio::Manual {
                    clock: Arc::clone(clock),
                    host: host.clone(),
                },
                None,
            ));
        }
        AudioMode::FailDevice(reason) => (OutputConfig::from_env(), Some(reason.clone())),
        AudioMode::FromEngine => (
            match engine {
                AudioConfig::Null => OutputConfig::null(),
                AudioConfig::Default => OutputConfig::from_env(),
            },
            None,
        ),
    };
    let null = OutputConfig::null();
    let rate = if fail.is_some() {
        SampleRate::HZ_48_000
    } else {
        AudioOutput::preferred_rate(&config).unwrap_or(SampleRate::HZ_48_000)
    };
    let mixer_at = |rate: SampleRate| {
        Mixer::new(
            MixerConfig::new(rate),
            Arc::clone(sources) as Arc<dyn SourceFactory>,
        )
        .map_err(|e| SubsystemError {
            kind: SubsystemKind::Audio,
            message: format!("cannot build the mixer: {e}"),
        })
    };
    let (mixer, handle) = mixer_at(rate)?;
    let started = match fail {
        Some(reason) => Err((reason, Some(mixer))),
        None => AudioOutput::start(mixer, config.clone())
            .map_err(|e| (e.error.to_string(), e.mixer.map(|m| *m))),
    };
    let mut degraded = None;
    let (output, handle) = match started {
        Ok(output) => (output, handle),
        Err((reason, mixer)) if config.device != null.device => {
            degraded = Some(format!("{reason}; playing on the null clock"));
            // A failed start hands the mixer back; rebuild it when it did not.
            let (mixer, handle) = if let Some(m) = mixer {
                (m, handle)
            } else {
                drop(handle);
                mixer_at(SampleRate::HZ_48_000)?
            };
            let output = AudioOutput::start(mixer, null).map_err(|e| SubsystemError {
                kind: SubsystemKind::Audio,
                message: format!("even the null audio output failed: {}", e.error),
            })?;
            (output, handle)
        }
        Err((reason, _)) => {
            return Err(SubsystemError {
                kind: SubsystemKind::Audio,
                message: format!("the null audio output failed: {reason}"),
            });
        }
    };
    let output = Arc::new(output);
    let meters = MeterPump::spawn(Arc::clone(&output), events.clone())
        .map_err(|e| tracing::warn!(error = %e, "no meter thread: playback.meters stays silent"))
        .ok();
    Ok((
        Audio::Output {
            meters,
            output,
            mixer: Arc::new(handle),
        },
        degraded,
    ))
}
