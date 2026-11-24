// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardware decode (WP-1D.04): which GPU decoders exist per OS, which stream classes may use
//! them, the cached per-codec probe that fills
//! [`Capabilities::hw_decode`](crate::Capabilities::hw_decode), and the choice a decoder makes
//! under [`HwMode::Auto`](crate::HwMode::Auto). Path (a) of Spike S3 (ADR-0003): libavcodec's
//! own H.264/HEVC decoders with a hwaccel, frames copied to system memory as NV12/P010.
//!
//! - **Devices** ([`candidates`], the order `Auto` tries them): Windows NVDEC (`cuda`), then
//!   D3D11VA on an NVIDIA, AMD or Intel GPU; macOS VideoToolbox; Linux NVDEC, then VA-API. A GPU
//!   is chosen by vendor, never by adapter index (S3 finding H7: DXGI indexes change inside a
//!   process once CUDA is loaded). `CUTWEAVE_HW` (or [`FfmpegBackend::with_hw_decoders`]) turns
//!   hardware decode off or restricts it to named devices ([`Policy::parse`]).
//! - **Stream classes** ([`eligible`]): H.264 8-bit 4:2:0 and HEVC 8/10-bit 4:2:0, long GOP
//!   (intra-only streams and proxies stay on the CPU, S3 H8), at least 144 pixels each way (below
//!   that the GPU gains nothing and NVDEC's HEVC minimum is 144). These are the classes the S3
//!   corpus showed bit-exact; everything else decodes in software without trying.
//! - **Probe** ([`verified`]): per (device, codec), once per process: the device opens, the
//!   decoder has a hwaccel for it, and a 256×256 test stream (encoded here with libx264 /
//!   libx265, B-frames included) decodes in hardware **bit-exactly** like software. A device that
//!   fails any step is skipped for that codec, with the reason logged.
//! - **H.264 keyframes that are not IDR pictures** (S3 H10, reproduced in WP-1D.04): AMD's
//!   D3D11VA decoder returns wrong pixels late in a GOP when decoding starts at a non-IDR
//!   I-frame (x264 `open-gop` streams), while NVIDIA's decoders are bit-exact. A decoder on any
//!   device but NVIDIA's ([`Choice::idr_starts_only`]) therefore checks each keyframe a seek
//!   lands on ([`h264_idr`]) and goes over to software at the first one that is not IDR. Decoding
//!   that started at an IDR picture stays in hardware through later non-IDR keyframes.
//!
//! [`FfmpegBackend::with_hw_decoders`]: super::FfmpegBackend::with_hw_decoders

use std::collections::HashMap;
use std::ffi::{CString, c_int};
use std::hash::Hash;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use ffmpeg_next::codec::{self, threading};
use ffmpeg_next::format::Pixel;
use ffmpeg_next::{Dictionary, Error, Packet, Rational, decoder, encoder, frame};

use super::sys::{self, HwDecode, HwDevice};
use crate::HwCodec;

/// Codecs that decode in hardware (codec-id names), in the order `hw_decode` lists them.
/// AV1 and VP9 have hwaccels too, but no corpus test has shown them exact yet (ADR-0003: a
/// stream class runs in hardware only where the corpus shows it exact).
pub(super) const CODECS: [&str; 2] = ["h264", "hevc"];

/// The smallest width and height decoded in hardware.
pub(super) const MIN_SIZE: u32 = 144;

/// A GPU decode API (an FFmpeg hardware device type).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Api {
    /// NVIDIA NVDEC through CUDA (Windows, Linux).
    Cuda,
    /// Direct3D 11 video decoding (Windows, any vendor).
    D3d11va,
    /// VA-API (Linux: Intel, AMD).
    Vaapi,
    /// VideoToolbox (macOS).
    VideoToolbox,
}

impl Api {
    /// FFmpeg's device type name.
    fn device_type(self) -> &'static str {
        match self {
            Api::Cuda => "cuda",
            Api::D3d11va => "d3d11va",
            Api::Vaapi => "vaapi",
            Api::VideoToolbox => "videotoolbox",
        }
    }

    /// FFmpeg's hwaccel name, the suffix of an implementation name (`h264_nvdec`).
    fn hwaccel(self) -> &'static str {
        match self {
            Api::Cuda => "nvdec",
            Api::D3d11va => "d3d11va",
            Api::Vaapi => "vaapi",
            Api::VideoToolbox => "videotoolbox",
        }
    }
}

/// PCI vendor ids.
const NVIDIA: u16 = 0x10de;
const AMD: u16 = 0x1002;
const INTEL: u16 = 0x8086;

/// The GPU vendors D3D11VA tries, in order.
const D3D11VA_VENDORS: [u16; 3] = [NVIDIA, AMD, INTEL];

fn vendor_name(id: u16) -> String {
    match id {
        NVIDIA => "nvidia".to_owned(),
        AMD => "amd".to_owned(),
        INTEL => "intel".to_owned(),
        _ => format!("vendor=0x{id:04x}"),
    }
}

/// One hardware decode device: an API, plus the GPU vendor (D3D11VA) or DRM node (VA-API).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Device {
    pub api: Api,
    /// PCI vendor id for D3D11VA (FFmpeg's `vendor_id` option).
    pub vendor: Option<u16>,
    /// VA-API render node (`/dev/dri/renderD129`); `None` opens the default one.
    pub node: Option<String>,
}

impl Device {
    const fn of(api: Api) -> Device {
        Device {
            api,
            vendor: None,
            node: None,
        }
    }

    fn d3d11va(vendor: u16) -> Device {
        Device {
            api: Api::D3d11va,
            vendor: Some(vendor),
            node: None,
        }
    }

    /// `cuda`, `d3d11va:nvidia`, `vaapi`, `vaapi:/dev/dri/renderD129`, `videotoolbox`: what
    /// `CUTWEAVE_HW` takes, logs show and `HwCodec::device` holds.
    pub(super) fn label(&self) -> String {
        let api = self.api.device_type();
        match (&self.node, self.vendor) {
            (Some(node), _) => format!("{api}:{node}"),
            (None, Some(v)) => format!("{api}:{}", vendor_name(v)),
            (None, None) => api.to_owned(),
        }
    }

    /// The implementation name for `codec` (`h264_nvdec`, `hevc_d3d11va`).
    fn implementation(&self, codec: &str) -> String {
        format!("{codec}_{}", self.api.hwaccel())
    }

    /// Whether H.264 decoding started at a non-IDR I-frame is known to be exact here: NVIDIA's
    /// decoders (NVDEC, D3D11VA) were bit-exact in S3 and WP-1D.04; AMD's D3D11VA was not (H10),
    /// and the rest are unmeasured.
    fn exact_from_non_idr_keyframes(&self) -> bool {
        self.api == Api::Cuda || self.vendor == Some(NVIDIA)
    }

    /// Opens the device.
    fn open(&self) -> Result<HwDevice, String> {
        let kind = CString::new(self.api.device_type()).map_err(|e| e.to_string())?;
        let node = self
            .node
            .as_deref()
            .map(CString::new)
            .transpose()
            .map_err(|e| e.to_string())?;
        HwDevice::create(&kind, node.as_deref(), self.vendor).map_err(|e| match e {
            Error::Other { errno } if errno == ffmpeg_next::error::ENOSYS => {
                format!("{} is not in this FFmpeg build", self.api.device_type())
            }
            e => {
                let why = e.to_string();
                if why.is_empty() {
                    format!("no such GPU or driver ({e:?})")
                } else {
                    format!("the device did not open: {why}")
                }
            }
        })
    }
}

/// The hardware decoders for this OS, in the order `Auto` tries them.
pub(super) fn candidates() -> Vec<Device> {
    if cfg!(windows) {
        std::iter::once(Device::of(Api::Cuda))
            .chain(D3D11VA_VENDORS.map(Device::d3d11va))
            .collect()
    } else if cfg!(target_os = "macos") {
        vec![Device::of(Api::VideoToolbox)]
    } else {
        vec![Device::of(Api::Cuda), Device::of(Api::Vaapi)]
    }
}

/// Where a decoder fails on purpose (tests): a pseudo hardware decoder that decodes in software
/// while claiming to be hardware.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Faults {
    /// Attaching the device fails while opening.
    pub open: bool,
    /// Sending this packet (counted from the decoder's open, from 0) fails as the hardware.
    pub packet: Option<u64>,
    /// Copying the first returned frame to system memory fails.
    pub transfer: bool,
    /// Behaves like a decoder that is not exact from non-IDR H.264 keyframes (S3 H10).
    pub idr_starts_only: bool,
}

/// Which hardware decoders a backend's `Auto` decoders may use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Policy {
    /// None: every decoder is software (`CUTWEAVE_HW=off`).
    Off,
    /// These devices, in this order (the OS's [`candidates`] by default).
    Devices(Vec<Device>),
    /// Tests: a pseudo hardware decoder failing as told, for every eligible stream.
    #[cfg(test)]
    Pseudo(Faults),
}

impl Policy {
    /// The OS's candidates.
    pub(super) fn auto() -> Policy {
        Policy::Devices(candidates())
    }

    /// From `CUTWEAVE_HW`: unset means `auto`; a value that does not parse is logged and means
    /// `off` (whoever set the variable wanted something other than the default).
    pub(super) fn from_env() -> Policy {
        match std::env::var("CUTWEAVE_HW") {
            Err(_) => Policy::auto(),
            Ok(value) => Policy::parse(&value).unwrap_or_else(|why| {
                tracing::warn!(
                    target: "cutweave_media",
                    "CUTWEAVE_HW={value:?} is not valid ({why}); hardware decode is off"
                );
                Policy::Off
            }),
        }
    }

    /// Parses a policy: `off` (also `none`, `sw`, `0`); `auto` or empty (also `on`, `1`); or a
    /// comma-separated list of devices tried in that order: `cuda` (or `nvdec`), `d3d11va`
    /// (every vendor), `d3d11va:nvidia|amd|intel|vendor=0xNNNN`, `vaapi`,
    /// `vaapi:/dev/dri/renderDN`, `videotoolbox`. Case-insensitive except for the DRM node.
    pub(super) fn parse(text: &str) -> Result<Policy, String> {
        let text = text.trim();
        match text.to_ascii_lowercase().as_str() {
            "off" | "none" | "sw" | "0" | "false" => return Ok(Policy::Off),
            "" | "auto" | "on" | "1" | "true" => return Ok(Policy::auto()),
            _ => {}
        }
        let mut devices: Vec<Device> = Vec::new();
        for item in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (api, arg) = item
                .split_once(':')
                .map_or((item, None), |(a, b)| (a, Some(b)));
            let parsed = match (api.to_ascii_lowercase().as_str(), arg) {
                ("cuda" | "nvdec", None) => vec![Device::of(Api::Cuda)],
                ("d3d11va" | "d3d11", None) => D3D11VA_VENDORS.map(Device::d3d11va).to_vec(),
                ("d3d11va" | "d3d11", Some(v)) => {
                    vec![Device::d3d11va(parse_vendor(&v.to_ascii_lowercase())?)]
                }
                ("vaapi", None) => vec![Device::of(Api::Vaapi)],
                ("vaapi", Some(node)) if node.starts_with("/dev/dri/") => vec![Device {
                    node: Some(node.to_owned()),
                    ..Device::of(Api::Vaapi)
                }],
                ("videotoolbox" | "vt", None) => vec![Device::of(Api::VideoToolbox)],
                _ => return Err(format!("unknown hardware decoder `{item}`")),
            };
            for d in parsed {
                if !devices.contains(&d) {
                    devices.push(d);
                }
            }
        }
        if devices.is_empty() {
            return Err("no hardware decoder named".to_owned());
        }
        Ok(Policy::Devices(devices))
    }

    /// The devices tried, in order (none for `Off`).
    fn devices(&self) -> &[Device] {
        match self {
            Policy::Devices(d) => d,
            Policy::Off => &[],
            #[cfg(test)]
            Policy::Pseudo(_) => &[],
        }
    }
}

/// `nvidia`, `amd`, `intel` or `vendor=0xNNNN` (never an adapter index: S3 finding H7).
fn parse_vendor(v: &str) -> Result<u16, String> {
    match v {
        "nvidia" => Ok(NVIDIA),
        "amd" => Ok(AMD),
        "intel" => Ok(INTEL),
        _ => v
            .strip_prefix("vendor=0x")
            .and_then(|h| u16::from_str_radix(h, 16).ok())
            .ok_or_else(|| {
                format!("`{v}` is not a GPU vendor (nvidia, amd, intel or vendor=0xNNNN)")
            }),
    }
}

/// What a decoder needs to know about its stream to decide on hardware.
#[derive(Clone, Debug, Default)]
pub(super) struct StreamClass {
    /// Codec-id name (`h264`).
    pub codec: Option<String>,
    /// Pixel format name (`yuv420p10le`).
    pub pixel_format: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Every frame is a keyframe (intra codecs, proxies).
    pub intra_only: bool,
}

/// Whether a stream of this class may decode in hardware; the reason when not (logged at
/// debug).
pub(super) fn eligible(class: &StreamClass) -> Result<&'static str, String> {
    let codec = class.codec.as_deref().unwrap_or("?");
    let Some(codec) = CODECS.iter().copied().find(|c| *c == codec) else {
        return Err(format!("{codec} decodes in software"));
    };
    let layout = class.pixel_format.as_deref().unwrap_or("?");
    let ok_layout = match codec {
        "h264" => matches!(layout, "yuv420p" | "yuvj420p"),
        _ => matches!(layout, "yuv420p" | "yuvj420p" | "yuv420p10le"),
    };
    if !ok_layout {
        return Err(format!("{codec} in {layout} decodes in software"));
    }
    if class.intra_only {
        return Err(format!("intra-only {codec} decodes in software"));
    }
    if class.width.min(class.height) < MIN_SIZE {
        return Err(format!(
            "{}x{} is below the hardware minimum of {MIN_SIZE} pixels",
            class.width, class.height
        ));
    }
    Ok(codec)
}

/// How one decoder decodes in hardware.
#[derive(Clone)]
pub(super) struct Choice {
    /// The device label (`cuda`, `d3d11va:amd`).
    pub device: String,
    /// The implementation name (`h264_nvdec`).
    pub implementation: String,
    /// H.264 on a device not known to be exact from non-IDR keyframes (S3 H10): decoding may
    /// start only at IDR pictures, and a seek landing elsewhere goes over to software.
    pub idr_starts_only: bool,
    pub backing: Backing,
}

/// What does the decoding.
#[derive(Clone)]
pub(super) enum Backing {
    /// A real device and the surface format the codec decodes to on it.
    Device {
        device: Arc<HwDevice>,
        surface_format: c_int,
    },
    /// Tests: software decoding that claims to be hardware and fails as told.
    #[cfg(test)]
    Pseudo(Faults),
}

/// The hardware decoder a stream of `class` uses under `policy`, if any: the first device of the
/// policy whose probe passed for the codec.
pub(super) fn choose(policy: &Policy, class: &StreamClass) -> Result<Choice, String> {
    let codec = eligible(class)?;
    #[cfg(test)]
    if let Policy::Pseudo(faults) = policy {
        return Ok(Choice {
            device: "pseudo".to_owned(),
            implementation: format!("{codec}_pseudo"),
            idr_starts_only: faults.idr_starts_only && codec == "h264",
            backing: Backing::Pseudo(*faults),
        });
    }
    for device in policy.devices() {
        let Ok(found) = verified(device, codec) else {
            continue;
        };
        return Ok(Choice {
            device: device.label(),
            implementation: device.implementation(codec),
            idr_starts_only: codec == "h264" && !device.exact_from_non_idr_keyframes(),
            backing: Backing::Device {
                device: found.device,
                surface_format: found.surface_format,
            },
        });
    }
    match policy {
        Policy::Off => Err("hardware decode is off".to_owned()),
        Policy::Devices(_) => Err(format!("no verified hardware {codec} decoder")),
        #[cfg(test)]
        Policy::Pseudo(_) => Err("pseudo hardware".to_owned()),
    }
}

/// [`Capabilities::hw_decode`](crate::Capabilities::hw_decode) for `policy`: every (device,
/// codec) that passed the probe, devices in the policy's order. Probes what was not probed yet.
pub(super) fn capabilities(policy: &Policy) -> Vec<HwCodec> {
    let mut out = Vec::new();
    for device in policy.devices() {
        for codec in CODECS {
            if verified(device, codec).is_ok() {
                out.push(HwCodec {
                    device: device.label(),
                    codec: codec.to_owned(),
                    implementation: device.implementation(codec),
                });
            }
        }
    }
    out
}

/// Whether H.264 access unit `data` is an IDR picture: `Some(true)` for an IDR slice (NAL type
/// 5), `Some(false)` for another slice (types 1-4), `None` without a slice (or unparsable data).
/// `nal_length` is the NAL length-prefix size of MP4/Matroska (`avcC`) streams; `None` means
/// Annex B start codes (MPEG-TS, raw streams).
pub(super) fn h264_idr(data: &[u8], nal_length: Option<usize>) -> Option<bool> {
    let slice_type = |header: u8| match header & 0x1f {
        5 => Some(true),
        1..=4 => Some(false),
        _ => None,
    };
    match nal_length {
        Some(size @ (1 | 2 | 4)) => {
            let mut rest = data;
            while rest.len() > size {
                let len = rest[..size]
                    .iter()
                    .fold(0usize, |n, &b| (n << 8) | usize::from(b));
                let nal = rest.get(size..size + len)?;
                if let Some(idr) = nal.first().copied().and_then(slice_type) {
                    return Some(idr);
                }
                rest = &rest[size + len..];
            }
            None
        }
        Some(_) => None,
        None => {
            // Each NAL header follows a 00 00 01 start code (4-byte codes end the same way).
            let mut i = 0;
            while i + 3 < data.len() {
                if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                    if let Some(idr) = slice_type(data[i + 3]) {
                        return Some(idr);
                    }
                    i += 3;
                } else {
                    i += 1;
                }
            }
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The probe
// ---------------------------------------------------------------------------------------------

/// A device that passed the probe for a codec.
#[derive(Clone)]
struct Verified {
    device: Arc<HwDevice>,
    surface_format: c_int,
}

type Verdict = Result<Verified, String>;

/// A process-wide map whose values are computed once per key, outside the map's lock (so slow
/// probes of different keys run in parallel and callers of the same key wait for one).
type Cache<K, V> = OnceLock<Mutex<HashMap<K, Arc<OnceLock<V>>>>>;

/// The value of `key` in `cache`, computed with `init` on first use.
fn cached<K: Eq + Hash, V: Clone>(cache: &Cache<K, V>, key: K, init: impl FnOnce() -> V) -> V {
    let slot = {
        let mut map = cache
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Arc::clone(map.entry(key).or_default())
    };
    slot.get_or_init(init).clone()
}

/// The open devices of the process (opened once, shared by every decoder), and their failures.
fn shared_device(device: &Device) -> Result<Arc<HwDevice>, String> {
    static DEVICES: Cache<Device, Result<Arc<HwDevice>, String>> = OnceLock::new();
    cached(&DEVICES, device.clone(), || device.open().map(Arc::new))
}

/// The probe verdict for (`device`, `codec`), probed on first use and cached for the process.
fn verified(device: &Device, codec: &'static str) -> Verdict {
    static PROBES: Cache<(Device, &'static str), Verdict> = OnceLock::new();
    cached(&PROBES, (device.clone(), codec), || {
        let verdict = probe(device, codec);
        let name = device.implementation(codec);
        match &verdict {
            Ok(_) => tracing::info!(
                target: "cutweave_media",
                "hardware decoder {name} on {}: verified bit-exact against software",
                device.label()
            ),
            Err(why) => tracing::info!(
                target: "cutweave_media",
                "hardware decoder {name} on {}: unavailable ({why})",
                device.label()
            ),
        }
        verdict
    })
}

/// Opens the device, finds the codec's hwaccel for it and decodes the test stream in hardware,
/// expecting every frame bit-exactly equal to the software decode.
fn probe(device: &Device, codec: &'static str) -> Verdict {
    let hw_device = shared_device(device)?;
    let decoder_codec = decoder::find_by_name(codec)
        .ok_or_else(|| format!("no {codec} decoder in this FFmpeg build"))?;
    let surface_format = hw_device.surface_format(&decoder_codec).ok_or_else(|| {
        format!(
            "the {codec} decoder has no {} hwaccel",
            device.api.hwaccel()
        )
    })?;
    let stream = test_stream(codec)?;
    let software = software_frames(codec)?;
    let hardware = decode_test_stream(decoder_codec, &stream, Some((&hw_device, surface_format)))?;
    if hardware.len() != software.len() {
        return Err(format!(
            "{} frames in hardware, {} in software",
            hardware.len(),
            software.len()
        ));
    }
    if let Some(n) = (0..software.len()).find(|&n| hardware[n] != software[n]) {
        return Err(format!(
            "frame {n} differs from the software decode (not bit-exact)"
        ));
    }
    Ok(Verified {
        device: hw_device,
        surface_format,
    })
}

/// Size of the test stream (above every decoder's minimum).
const TEST_SIZE: u32 = 256;
/// Frames in the test stream: one GOP with B-frames.
const TEST_FRAMES: i64 = 12;

/// One encoded packet of the test stream.
struct TestPacket {
    data: Vec<u8>,
    pts: Option<i64>,
    dts: Option<i64>,
    key: bool,
}

/// The test stream for `codec`, encoded once per process.
fn test_stream(codec: &'static str) -> Result<Arc<Vec<TestPacket>>, String> {
    static STREAMS: Cache<&'static str, Result<Arc<Vec<TestPacket>>, String>> = OnceLock::new();
    cached(&STREAMS, codec, || encode_test_stream(codec).map(Arc::new))
}

/// Decoded test-stream frames: the Y, Cb and Cr bytes of each.
type Pictures = Arc<Vec<Vec<u8>>>;

/// The software decode of the test stream, once per process.
fn software_frames(codec: &'static str) -> Result<Pictures, String> {
    static FRAMES: Cache<&'static str, Result<Pictures, String>> = OnceLock::new();
    cached(&FRAMES, codec, || {
        let decoder_codec = decoder::find_by_name(codec)
            .ok_or_else(|| format!("no {codec} decoder in this FFmpeg build"))?;
        decode_test_stream(decoder_codec, &test_stream(codec)?, None).map(Arc::new)
    })
}

/// Frame `n` of the test stream: a moving diagonal texture with varying chroma, so motion
/// compensation and both chroma planes are exercised.
fn test_picture(index: i64) -> frame::Video {
    let luma_side = TEST_SIZE as usize;
    let mut picture = frame::Video::new(Pixel::YUV420P, TEST_SIZE, TEST_SIZE);
    let shift = usize::try_from(index).unwrap_or(0) * 3;
    for plane in 0..3 {
        let side = if plane == 0 { luma_side } else { luma_side / 2 };
        let stride = picture.stride(plane);
        let data = picture.data_mut(plane);
        for row in 0..side {
            for col in 0..side {
                let value = match plane {
                    0 => 16 + ((col + 2 * row + shift) ^ (col * row / 64)) % 220,
                    1 => 64 + (col + shift) % 128,
                    _ => 64 + (row * 2 + shift) % 128,
                };
                if let Some(px) = data.get_mut(row * stride + col) {
                    *px = u8::try_from(value).unwrap_or(u8::MAX);
                }
            }
        }
    }
    picture.set_pts(Some(index));
    picture
}

/// Encodes the test stream with libx264 / libx265 (Annex B, headers in band, two B-frames).
fn encode_test_stream(codec: &str) -> Result<Vec<TestPacket>, String> {
    let (name, params_key, params) = match codec {
        "h264" => ("libx264", "x264-params", "keyint=12:bframes=2:scenecut=0"),
        _ => (
            "libx265",
            "x265-params",
            "keyint=12:bframes=2:scenecut=0:log-level=error",
        ),
    };
    let fail = |what: &str, e: Error| format!("cannot make the {codec} test stream: {what}: {e}");
    let enc_codec = encoder::find_by_name(name)
        .ok_or_else(|| format!("cannot make the {codec} test stream: no {name} encoder"))?;
    let mut enc = codec::context::Context::new_with_codec(enc_codec)
        .encoder()
        .video()
        .map_err(|e| fail("encoder", e))?;
    enc.set_width(TEST_SIZE);
    enc.set_height(TEST_SIZE);
    enc.set_format(Pixel::YUV420P);
    enc.set_time_base(Rational::new(1, 30));
    enc.set_frame_rate(Some(Rational::new(30, 1)));
    let mut options = Dictionary::new();
    options.set("preset", "veryfast");
    options.set(params_key, params);
    let mut enc = enc.open_with(options).map_err(|e| fail("open", e))?;
    let mut packets = Vec::new();
    let mut packet = Packet::empty();
    let mut drain = |enc: &mut encoder::video::Encoder, packets: &mut Vec<TestPacket>| loop {
        match enc.receive_packet(&mut packet) {
            Ok(()) => packets.push(TestPacket {
                data: packet.data().map_or_default(<[u8]>::to_vec),
                pts: packet.pts(),
                dts: packet.dts(),
                key: packet.is_key(),
            }),
            Err(Error::Other { errno }) if errno == ffmpeg_next::error::EAGAIN => {
                return Ok(());
            }
            Err(Error::Eof) => return Ok(()),
            Err(e) => return Err(fail("encode", e)),
        }
    };
    for n in 0..TEST_FRAMES {
        enc.send_frame(&test_picture(n))
            .map_err(|e| fail("send", e))?;
        drain(&mut enc, &mut packets)?;
    }
    enc.send_eof().map_err(|e| fail("flush", e))?;
    drain(&mut enc, &mut packets)?;
    if packets.first().is_none_or(|p| !p.key) {
        return Err(format!(
            "cannot make the {codec} test stream: it does not start with a keyframe"
        ));
    }
    Ok(packets)
}

/// Decodes the test stream (in hardware with `hw`, which must then produce surfaces only) into
/// the bytes of the Y, Cb and Cr planes of every frame, in output order.
fn decode_test_stream(
    decoder_codec: ffmpeg_next::Codec,
    stream: &[TestPacket],
    hw: Option<(&HwDevice, c_int)>,
) -> Result<Vec<Vec<u8>>, String> {
    let mut ctx = codec::Context::new_with_codec(decoder_codec);
    ctx.set_threading(threading::Config {
        kind: threading::Type::Frame,
        count: 1,
    });
    let attached = match hw {
        Some((device, format)) => Some(
            HwDecode::attach(&mut ctx, device, format)
                .map_err(|e| format!("cannot attach the device: {e}"))?,
        ),
        None => None,
    };
    let mut dec = ctx.decoder();
    dec.set_packet_time_base(Rational::new(1, 30));
    let mut dec = dec
        .video()
        .map_err(|e| format!("the decoder did not open: {e}"))?;
    let mut frames = Vec::new();
    let mut decoded = frame::Video::empty();
    let mut copied = frame::Video::empty();
    let mut collect = |dec: &mut decoder::Video, frames: &mut Vec<Vec<u8>>| -> Result<(), String> {
        loop {
            match dec.receive_frame(&mut decoded) {
                Ok(()) => {
                    let picture = match &attached {
                        Some(hw) => {
                            if !hw.is_surface(&decoded) {
                                return Err(
                                    "the hwaccel did not initialise (frames came out in software)"
                                        .to_owned(),
                                );
                            }
                            sys::transfer_surface(&decoded, &mut copied)
                                .map_err(|e| format!("copying a surface to memory failed: {e}"))?;
                            &copied
                        }
                        None => &decoded,
                    };
                    frames.push(planar_bytes(picture)?);
                }
                Err(Error::Other { errno }) if errno == ffmpeg_next::error::EAGAIN => {
                    return Ok(());
                }
                Err(Error::Eof) => return Ok(()),
                Err(e) => return Err(format!("decoding failed: {e}")),
            }
        }
    };
    for p in stream {
        let mut packet = Packet::copy(&p.data);
        packet.set_pts(p.pts);
        packet.set_dts(p.dts);
        if p.key {
            packet.set_flags(ffmpeg_next::packet::Flags::KEY);
        }
        dec.send_packet(&packet)
            .map_err(|e| format!("decoding failed: {e}"))?;
        collect(&mut dec, &mut frames)?;
    }
    dec.send_eof()
        .map_err(|e| format!("decoding failed: {e}"))?;
    collect(&mut dec, &mut frames)?;
    drop(dec);
    drop(attached);
    Ok(frames)
}

/// The Y, Cb and Cr samples of an 8-bit 4:2:0 frame (YUV420P or NV12), tight, one byte each.
fn planar_bytes(f: &frame::Video) -> Result<Vec<u8>, String> {
    let (w, h) = (f.width() as usize, f.height() as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let missing = || "a decoded frame is smaller than its layout".to_owned();
    let mut out = sys::copy_plane(f, 0, w, h).ok_or_else(missing)?;
    match f.format() {
        Pixel::YUV420P | Pixel::YUVJ420P => {
            out.extend(sys::copy_plane(f, 1, cw, ch).ok_or_else(missing)?);
            out.extend(sys::copy_plane(f, 2, cw, ch).ok_or_else(missing)?);
        }
        Pixel::NV12 => {
            let uv = sys::copy_plane(f, 1, cw * 2, ch).ok_or_else(missing)?;
            out.extend(uv.iter().step_by(2));
            out.extend(uv.iter().skip(1).step_by(2));
        }
        other => return Err(format!("unexpected layout {other:?}")),
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
