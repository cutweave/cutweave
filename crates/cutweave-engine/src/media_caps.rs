// SPDX-License-Identifier: GPL-3.0-or-later
//! `media.capabilities` (WP-1M.08 interface, wired by WP-1I.05): what the engine's media
//! backend can decode and encode, for the export dialog (the hardware H.264 encoders `auto`
//! tries, in order) and for agents.
//!
//! It is a read-only engine command (like `job.status`): the answer is the backend's
//! `MediaBackend::capabilities`, which belongs to the engine's subsystems, not to the
//! document. Hardware encoders are probed once per process on first use (WP-1E.02); hardware
//! decode paths are listed once WP-1D.04 probes them. An engine without a media backend answers
//! `available: false` with empty lists.

use cutweave_commands::{
    Annotations, CmdError, CommandSpec, Described, EngineCommand, EntryKind, McpExposure,
    Registrable, Registry, RegistryError, Scope, wire,
};
use cutweave_media::{Capabilities, HwCodec};
use cutweave_model::StreamKind;

/// What `media.capabilities` runs against: the backend's capabilities, if there is a backend.
#[derive(Debug)]
pub struct CapabilitiesCall {
    pub(crate) caps: Option<Capabilities>,
}

/// Returns what this engine's media backend can decode and encode: software codecs with their
/// implementations, the hardware encoders in the order `export.start{hw: "auto"}` tries them,
/// and the hardware decode paths. `available` is false when the engine has no media backend.
#[wire]
pub struct MediaCapabilities {}

/// A codec a hardware API handles on this machine.
#[wire]
pub struct HwCodecInfo {
    /// Hardware device type (`d3d11va`, `cuda`, `amf`, `videotoolbox`, `vaapi`, `qsv`).
    pub device: String,
    /// Codec name (`h264`).
    pub codec: String,
    /// The implementation, as `export.start{hw: {force}}` takes it (`h264_nvenc`).
    pub implementation: String,
}

/// A software codec and its implementations.
#[wire]
pub struct CodecInfo {
    /// Codec name (`h264`, `hevc`, `aac`), as the probe reports it.
    pub name: String,
    /// What it carries.
    pub kind: StreamKind,
    /// Decoder implementations (empty: cannot decode).
    pub decoders: Vec<String>,
    /// Encoder implementations (empty: cannot encode).
    pub encoders: Vec<String>,
}

/// Result of `media.capabilities`.
#[wire]
pub struct MediaCapabilitiesOutput {
    /// Whether the engine has a media backend at all.
    pub available: bool,
    /// Software codecs, sorted by name.
    pub codecs: Vec<CodecInfo>,
    /// Hardware encoders that passed a test encode, in the order `auto` tries them.
    pub hw_encode: Vec<HwCodecInfo>,
    /// Hardware decode paths verified on this machine.
    pub hw_decode: Vec<HwCodecInfo>,
}

impl Described for MediaCapabilities {
    const SPEC: CommandSpec = CommandSpec {
        name: "media.capabilities",
        summary: "List the codecs and hardware encoders/decoders the media backend supports",
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
        rust_type: "MediaCapabilities",
    };
}

impl Registrable for MediaCapabilities {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<MediaCapabilities>().map(drop)
    }
}

fn hw(list: &[HwCodec]) -> Vec<HwCodecInfo> {
    list.iter()
        .map(|c| HwCodecInfo {
            device: c.device.clone(),
            codec: c.codec.clone(),
            implementation: c.implementation.clone(),
        })
        .collect()
}

impl EngineCommand for MediaCapabilities {
    type Context = CapabilitiesCall;
    type Output = MediaCapabilitiesOutput;

    fn run(&self, call: &mut CapabilitiesCall) -> Result<MediaCapabilitiesOutput, CmdError> {
        let Some(caps) = &call.caps else {
            return Ok(MediaCapabilitiesOutput {
                available: false,
                codecs: Vec::new(),
                hw_encode: Vec::new(),
                hw_decode: Vec::new(),
            });
        };
        Ok(MediaCapabilitiesOutput {
            available: true,
            codecs: caps
                .codecs
                .iter()
                .map(|c| CodecInfo {
                    name: c.name.clone(),
                    kind: c.kind,
                    decoders: c.decoders.clone(),
                    encoders: c.encoders.clone(),
                })
                .collect(),
            hw_encode: hw(&caps.hw_encode),
            hw_decode: hw(&caps.hw_decode),
        })
    }
}

/// Registers `media.capabilities`.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register::<MediaCapabilities>()?;
    Ok(())
}
