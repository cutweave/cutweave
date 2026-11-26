// SPDX-License-Identifier: GPL-3.0-or-later
//! `export.list_encoders` (WP-2G.02; arch §8): which video codecs this machine can export, with
//! the encoder behind each (software, and the hardware encoders that passed a trial encode),
//! the containers and bit depths they take and their profiles.
//!
//! The engine reports the encoders it found ([`EncoderEntry`], through
//! [`ExportStarter::encoders`](super::ExportStarter::encoders)); this module adds what this
//! build writes for each codec (the table in [`codec_rows`]). A hardware encoder is listed only
//! after it opened and encoded eight frames of a 256x256 picture on this machine (not merely
//! because its name is in the FFmpeg build), so what the list shows is what `hw: "auto"` and
//! `hw: {"force": "<implementation>"}` can use. The query is in the MCP catalog.

use cutweave_commands::{
    Annotations, CmdError, CommandSpec, Described, EngineCommand, EntryKind, McpExposure,
    Registrable, Registry, RegistryError, Scope, wire,
};

use super::ExportCtx;
use super::presets::{Container, VideoCodec};

/// One encoder the engine has, as it reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderEntry {
    /// Codec-id name: `h264`, `hevc`, `av1` or `prores`.
    pub codec: String,
    /// The implementation: `libx265`, `hevc_nvenc`.
    pub implementation: String,
    /// Whether it runs on a GPU or media engine (it passed the trial encode).
    pub hardware: bool,
    /// The hardware device type (`cuda`, `amf`, `qsv`, `videotoolbox`, `vaapi`) of a hardware
    /// encoder.
    pub device: Option<String>,
}

/// Lists the video codecs this machine exports (H.264, HEVC, AV1, ProRes) with their software
/// encoder, the hardware encoders that passed an 8-frame trial encode (what `hw: "auto"` uses
/// and `hw: {"force": ...}` names), and the containers, bit depths and profiles each takes.
#[wire]
pub struct ListEncoders {}

impl Described for ListEncoders {
    const SPEC: CommandSpec = CommandSpec {
        name: "export.list_encoders",
        summary: "List the video encoders this machine exports with: H.264, HEVC, AV1, ProRes, hardware ones verified",
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
        rust_type: "ListEncoders",
    };
}

impl Registrable for ListEncoders {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<ListEncoders>().map(drop)
    }
}

/// A hardware encoder that passed the trial encode.
#[wire]
pub struct HardwareEncoderInfo {
    /// The name for `hw: {"force": ...}` (`hevc_nvenc`).
    pub implementation: String,
    /// The device type it runs on (`cuda`, `amf`, `qsv`, `videotoolbox`, `vaapi`).
    pub device: String,
}

/// One codec and how it can be exported here.
#[wire]
pub struct CodecEncoders {
    /// The codec.
    pub codec: VideoCodec,
    /// Whether any encoder (software or hardware) is available.
    pub available: bool,
    /// The software encoder (`libx265`), when this FFmpeg build has it.
    #[serde(default)]
    pub software: Option<String>,
    /// Hardware encoders verified on this machine, in the order `auto` tries them.
    pub hardware: Vec<HardwareEncoderInfo>,
    /// The containers the codec goes into.
    pub containers: Vec<Container>,
    /// The bit depths of the coded video.
    pub bit_depths: Vec<u8>,
    /// The profile names a preset's `video.profile` takes (empty when the codec has none).
    pub profiles: Vec<String>,
}

/// Result of `export.list_encoders`.
#[wire]
pub struct ListEncodersOutput {
    /// H.264, HEVC, AV1 and ProRes, in that order.
    pub codecs: Vec<CodecEncoders>,
}

/// What this build writes for a codec: `(codec, codec-id, software encoder, containers, bit
/// depths, profiles)`.
type Row = (
    VideoCodec,
    &'static str,
    &'static str,
    &'static [Container],
    &'static [u8],
    &'static [&'static str],
);

/// The codecs of the export encoder (the media crate's `ffmpeg::encode`).
fn codec_rows() -> [Row; 4] {
    [
        (
            VideoCodec::H264,
            "h264",
            "libx264",
            &[Container::Mp4, Container::Mov],
            &[8],
            &[],
        ),
        (
            VideoCodec::Hevc,
            "hevc",
            "libx265",
            &[Container::Mp4, Container::Mov],
            &[8, 10],
            &["main", "main10"],
        ),
        (
            VideoCodec::Av1,
            "av1",
            "libsvtav1",
            &[Container::Mp4],
            &[8, 10],
            &[],
        ),
        (
            VideoCodec::Prores,
            "prores",
            "prores_ks",
            &[Container::Mov],
            &[10],
            &["422proxy", "422lt", "422", "422hq", "4444", "4444xq"],
        ),
    ]
}

/// The listing for the encoders the engine found.
#[must_use]
pub fn describe(entries: &[EncoderEntry]) -> ListEncodersOutput {
    let codecs = codec_rows()
        .into_iter()
        .map(|(codec, id, soft, containers, depths, profiles)| {
            let software = entries
                .iter()
                .find(|e| !e.hardware && e.codec == id && e.implementation == soft)
                .map(|e| e.implementation.clone());
            let hardware: Vec<HardwareEncoderInfo> = entries
                .iter()
                .filter(|e| e.hardware && e.codec == id)
                .map(|e| HardwareEncoderInfo {
                    implementation: e.implementation.clone(),
                    device: e.device.clone().unwrap_or_default(),
                })
                .collect();
            CodecEncoders {
                codec,
                available: software.is_some() || !hardware.is_empty(),
                software,
                hardware,
                containers: containers.to_vec(),
                bit_depths: depths.to_vec(),
                profiles: profiles.iter().map(|p| (*p).to_owned()).collect(),
            }
        })
        .collect();
    ListEncodersOutput { codecs }
}

impl EngineCommand for ListEncoders {
    type Context = ExportCtx;
    type Output = ListEncodersOutput;

    fn run(&self, ctx: &mut ExportCtx) -> Result<ListEncodersOutput, CmdError> {
        Ok(describe(&ctx.starter.encoders()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(codec: &str, implementation: &str, device: Option<&str>) -> EncoderEntry {
        EncoderEntry {
            codec: codec.to_owned(),
            implementation: implementation.to_owned(),
            hardware: device.is_some(),
            device: device.map(str::to_owned),
        }
    }

    #[test]
    fn the_listing_pairs_each_codec_with_what_the_engine_found() {
        let out = describe(&[
            entry("h264", "libx264", None),
            entry("hevc", "libx265", None),
            entry("hevc", "hevc_nvenc", Some("cuda")),
            entry("hevc", "hevc_amf", Some("amf")),
            entry("prores", "prores_ks", None),
        ]);
        let names: Vec<VideoCodec> = out.codecs.iter().map(|c| c.codec).collect();
        assert_eq!(
            names,
            [
                VideoCodec::H264,
                VideoCodec::Hevc,
                VideoCodec::Av1,
                VideoCodec::Prores
            ]
        );
        let hevc = &out.codecs[1];
        assert_eq!(hevc.software.as_deref(), Some("libx265"));
        let hw: Vec<&str> = hevc
            .hardware
            .iter()
            .map(|h| h.implementation.as_str())
            .collect();
        assert_eq!(hw, ["hevc_nvenc", "hevc_amf"], "auto order is kept");
        assert_eq!(hevc.bit_depths, [8, 10]);
        // No AV1 encoder was found: listed, not available.
        let av1 = &out.codecs[2];
        assert!(!av1.available && av1.software.is_none() && av1.hardware.is_empty());
        assert_eq!(av1.containers, [Container::Mp4]);
        assert_eq!(out.codecs[3].containers, [Container::Mov]);
        assert_eq!(out.codecs[3].profiles.len(), 6);
    }
}
