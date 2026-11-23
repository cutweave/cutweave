// SPDX-License-Identifier: GPL-3.0-or-later
//! The cpal backend (WASAPI on Windows, CoreAudio on macOS): device lookup, format
//! negotiation, buffer size, and the stream whose data callback runs [`StreamCallback`].
//!
//! * **Rate.** The stream always runs at the mixer's rate. The device's default config is used
//!   when it matches; otherwise a supported range that contains the rate; otherwise the default
//!   format at the mixer's rate (shared-mode WASAPI converts any rate, `AUTOCONVERTPCM`).
//! * **Format.** The device's default sample format if the output converts to it, else the
//!   best of F32 > I32 > I24 > I16 > the rest. The mix is converted in the callback.
//! * **Buffer.** The requested frames per callback, clamped to the device's reported range.
//!   cpal's WASAPI backend is shared-mode only and always calls back at the device period
//!   (480 frames at 48 kHz on the S2 machine): the request only sizes the endpoint buffer.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, ErrorKind, SampleFormat, StreamConfig, SupportedBufferSize};
use cutweave_model::SampleRate;

use super::callback::{OutputSample, StreamCallback};
use super::{
    Backend, DeviceInfo, ErrorSink, LiveStream, OutputDevice, OutputError, OutputInfo, StreamFault,
    StreamRequest,
};

/// The cpal host of this platform.
pub(crate) struct CpalBackend {
    host: cpal::Host,
}

impl CpalBackend {
    pub fn new() -> CpalBackend {
        CpalBackend {
            host: cpal::default_host(),
        }
    }
}

fn backend_error(context: &str, e: &cpal::Error) -> OutputError {
    match e.kind() {
        ErrorKind::DeviceNotAvailable => OutputError::NoDevice,
        ErrorKind::UnsupportedConfig => OutputError::Unsupported(format!("{context}: {e}")),
        _ => OutputError::Backend(format!("{context}: {e}")),
    }
}

/// The device's display name (never panics, unlike `Display` on a vanished device).
fn device_name(d: &cpal::Device) -> String {
    d.description()
        .map_or_else(|_| "unknown device".to_owned(), |x| x.name().to_owned())
}

fn device_id(d: &cpal::Device) -> Option<String> {
    d.id().ok().map(|id| id.to_string())
}

fn find_device(host: &cpal::Host, device: &OutputDevice) -> Result<cpal::Device, OutputError> {
    match device {
        OutputDevice::Default | OutputDevice::Null => {
            host.default_output_device().ok_or(OutputError::NoDevice)
        }
        OutputDevice::Named(needle) => {
            let lower = needle.to_lowercase();
            host.output_devices()
                .map_err(|e| backend_error("listing output devices", &e))?
                .find(|d| {
                    device_name(d).to_lowercase().contains(&lower)
                        || device_id(d).as_deref() == Some(needle.as_str())
                })
                .ok_or_else(|| OutputError::DeviceNotFound(needle.clone()))
        }
    }
}

/// The rate the device prefers (its default config).
pub(crate) fn preferred_rate(device: &OutputDevice) -> Result<SampleRate, OutputError> {
    let host = cpal::default_host();
    let d = find_device(&host, device)?;
    let config = d
        .default_output_config()
        .map_err(|e| backend_error("default output config", &e))?;
    SampleRate::new(config.sample_rate())
        .map_err(|_| OutputError::Unsupported(format!("device rate {} Hz", config.sample_rate())))
}

/// Lists the output devices of the default host.
pub(crate) fn output_devices() -> Result<Vec<DeviceInfo>, OutputError> {
    let host = cpal::default_host();
    let default = host.default_output_device().as_ref().and_then(device_id);
    let devices = host
        .output_devices()
        .map_err(|e| backend_error("listing output devices", &e))?;
    Ok(devices
        .map(|d| {
            let id = device_id(&d).unwrap_or_default();
            let config = d.default_output_config().ok();
            DeviceInfo {
                is_default: default.as_deref() == Some(id.as_str()),
                name: device_name(&d),
                id,
                default_rate: config
                    .as_ref()
                    .map(cpal::SupportedStreamConfig::sample_rate),
                channels: config.as_ref().map(cpal::SupportedStreamConfig::channels),
                sample_format: config.as_ref().map(|c| format!("{:?}", c.sample_format())),
                buffer_range: config.as_ref().and_then(|c| match c.buffer_size() {
                    SupportedBufferSize::Range { min, max } => Some((*min, *max)),
                    SupportedBufferSize::Unknown => None,
                }),
            }
        })
        .collect())
}

/// Preference among sample formats (higher is better); `None`: the output cannot write it.
fn format_rank(f: SampleFormat) -> Option<u8> {
    Some(match f {
        SampleFormat::F32 => 9,
        SampleFormat::I32 => 8,
        SampleFormat::I24 => 7,
        SampleFormat::I16 => 6,
        SampleFormat::F64 => 5,
        SampleFormat::U16 => 4,
        SampleFormat::U32 | SampleFormat::U24 => 3,
        SampleFormat::I64 | SampleFormat::U64 => 2,
        SampleFormat::I8 | SampleFormat::U8 => 1,
        _ => return None,
    })
}

/// Channels, format and buffer-size range to open `device` with at `rate`.
fn negotiate(
    device: &cpal::Device,
    rate: u32,
) -> Result<(u16, SampleFormat, SupportedBufferSize), OutputError> {
    let default = device
        .default_output_config()
        .map_err(|e| backend_error("default output config", &e))?;
    if default.sample_rate() == rate && format_rank(default.sample_format()).is_some() {
        return Ok((
            default.channels(),
            default.sample_format(),
            *default.buffer_size(),
        ));
    }
    let best = device.supported_output_configs().ok().and_then(|ranges| {
        ranges
            .filter(|r| r.min_sample_rate() <= rate && rate <= r.max_sample_rate())
            .filter_map(|r| {
                let rank = format_rank(r.sample_format())?;
                Some(((r.channels() == default.channels(), rank, r.channels()), r))
            })
            .max_by_key(|(key, _)| *key)
            .map(|(_, r)| r)
    });
    if let Some(r) = best {
        return Ok((r.channels(), r.sample_format(), *r.buffer_size()));
    }
    if format_rank(default.sample_format()).is_none() {
        return Err(OutputError::Unsupported(format!(
            "sample format {:?}",
            default.sample_format()
        )));
    }
    // No range lists the rate; shared-mode WASAPI resamples, so try the default format anyway.
    Ok((
        default.channels(),
        default.sample_format(),
        *default.buffer_size(),
    ))
}

/// The requested frames per callback, clamped to the device's range.
fn buffer_for(requested: Option<u32>, range: SupportedBufferSize) -> Option<u32> {
    let frames = requested?;
    Some(match range {
        SupportedBufferSize::Range { min, max } if min <= max => frames.clamp(min, max),
        _ => frames,
    })
}

struct CpalStream {
    stream: cpal::Stream,
    info: OutputInfo,
    device_id: Option<String>,
}

impl LiveStream for CpalStream {
    fn info(&self) -> OutputInfo {
        self.info.clone()
    }

    fn device_id(&self) -> Option<String> {
        self.device_id.clone()
    }
}

impl Drop for CpalStream {
    fn drop(&mut self) {
        // Dropping the stream stops it and joins its thread, which drops the data callback.
        let _ = self.stream.pause();
    }
}

fn fault_of(e: &cpal::Error) -> StreamFault {
    match e.kind() {
        ErrorKind::Xrun => StreamFault::Xrun,
        ErrorKind::RealtimeDenied => StreamFault::Warning(e.to_string()),
        // `DeviceChanged` (the stream was rerouted) included: the stream is rebuilt so its
        // format and latency match the new device.
        _ => StreamFault::Fatal(e.to_string()),
    }
}

fn build<T: cpal::SizedSample + OutputSample>(
    device: &cpal::Device,
    config: StreamConfig,
    mut callback: StreamCallback,
    errors: ErrorSink,
) -> Result<cpal::Stream, cpal::Error> {
    let channels = usize::from(config.channels);
    device.build_output_stream::<T, _, _>(
        config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            let ts = info.timestamp();
            let latency = ts.playback.saturating_duration_since(ts.callback);
            let latency_ns = u64::try_from(latency.as_nanos()).unwrap_or(u64::MAX);
            callback.process(data, channels, latency_ns);
        },
        move |e: cpal::Error| errors.report(fault_of(&e)),
        None,
    )
}

impl Backend for CpalBackend {
    fn open(
        &mut self,
        request: &StreamRequest,
        callback: StreamCallback,
        errors: ErrorSink,
    ) -> Result<Box<dyn LiveStream>, OutputError> {
        let device = find_device(&self.host, &request.device)?;
        let rate = request.rate.hz();
        let (channels, format, range) = negotiate(&device, rate)?;
        let buffer_used = buffer_for(request.buffer_frames, range);
        let config = StreamConfig {
            channels,
            sample_rate: rate,
            buffer_size: buffer_used.map_or(BufferSize::Default, BufferSize::Fixed),
        };
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, callback, errors),
            SampleFormat::F64 => build::<f64>(&device, config, callback, errors),
            SampleFormat::I8 => build::<i8>(&device, config, callback, errors),
            SampleFormat::I16 => build::<i16>(&device, config, callback, errors),
            SampleFormat::I24 => build::<cpal::I24>(&device, config, callback, errors),
            SampleFormat::I32 => build::<i32>(&device, config, callback, errors),
            SampleFormat::I64 => build::<i64>(&device, config, callback, errors),
            SampleFormat::U8 => build::<u8>(&device, config, callback, errors),
            SampleFormat::U16 => build::<u16>(&device, config, callback, errors),
            SampleFormat::U24 => build::<cpal::U24>(&device, config, callback, errors),
            SampleFormat::U32 => build::<u32>(&device, config, callback, errors),
            SampleFormat::U64 => build::<u64>(&device, config, callback, errors),
            other => {
                drop(callback);
                return Err(OutputError::Unsupported(format!("sample format {other:?}")));
            }
        }
        .map_err(|e| backend_error("opening the output stream", &e))?;
        stream
            .play()
            .map_err(|e| backend_error("starting the output stream", &e))?;
        let info = OutputInfo {
            host: self.host.id().name().to_owned(),
            device: device_name(&device),
            rate: request.rate,
            channels,
            sample_format: format!("{format:?}"),
            buffer_requested: request.buffer_frames,
            buffer_used,
            period_frames: stream.buffer_size().ok(),
        };
        Ok(Box::new(CpalStream {
            stream,
            info,
            device_id: device_id(&device),
        }))
    }

    fn default_device_id(&mut self) -> Option<String> {
        self.host
            .default_output_device()
            .as_ref()
            .and_then(device_id)
    }
}
