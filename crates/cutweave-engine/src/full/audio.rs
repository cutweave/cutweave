// SPDX-License-Identifier: GPL-3.0-or-later
//! Playback audio of the full engine (WP-1I.05; arch §9): the mixer fed from the current
//! document's clips, played on the output `EngineConfig::audio` names, and the degrade rule:
//! when the device cannot start, the mixer plays on the null output (a wall-clock-driven clock)
//! and `engine.degraded{subsystem: "audio"}` is sent.

use std::sync::{Arc, PoisonError, RwLock};

use cutweave_audio::{
    AudioOutput, ManualClock, Mixer, MixerConfig, MixerHandle, OutputConfig, PcmSource,
    SourceError, SourceFactory, SourceRequest,
};
use cutweave_media::MediaBackend;
use cutweave_model::{Project, SampleRate, Time};
use cutweave_playback::transport::VirtualClock;

use crate::config::AudioConfig;
use crate::export::DecoderPcmFactory;
use crate::subsystems::{SubsystemError, SubsystemKind};

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

/// The running audio side of playback.
pub(crate) enum Audio {
    /// An output (device or null) playing the mixer.
    Output {
        // Field order is drop order: the output stops before the mixer handle (WP-1G.02).
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
        if let Audio::Output { output, mixer } = self {
            output.pause();
            if let Err(e) = mixer.set_segments(&[]) {
                tracing::debug!(error = %e, "cannot clear the mixer");
            }
            mixer.seek(Time::ZERO);
        }
    }

    /// Stops the output, then the mixer workers (in that order, WP-1G.02).
    pub(crate) fn stop(self) {
        if let Audio::Output { output, mixer } = self {
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

/// Starts playback audio (see the module docs); also returns why it degraded to the null
/// output, if it did.
pub(crate) fn start(
    mode: &AudioMode,
    engine: AudioConfig,
    sources: &Arc<LivePcm>,
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
    Ok((
        Audio::Output {
            output: Arc::new(output),
            mixer: Arc::new(handle),
        },
        degraded,
    ))
}
