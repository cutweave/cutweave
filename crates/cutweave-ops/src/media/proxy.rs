// SPDX-License-Identifier: GPL-3.0-or-later
//! Proxies (WP-2F.01; arch §7): which media get one automatically ([`ProxyPolicy`],
//! [`auto_reason`]) and the engine commands `media.generate_proxy`, `media.delete_proxy` and
//! `media.proxy_status`, all in the MCP catalog.
//!
//! A proxy is a smaller intra-frame copy of a video file in the global cache. The preview
//! scrubs it; exports, `render.frame` and thumbnails always use the original. Proxies are cache
//! state, not document state: these commands never touch the project (not undoable, not
//! journaled), and the engine answers them through a [`ProxyService`].
//!
//! **Policy.** Under [`ProxyPolicy::Auto`] a video gets a proxy when it is long-GOP H.264 or
//! HEVC and wider or taller than 1920 pixels, faster than 60 fps, or HEVC with 10-bit samples
//! ([`auto_reason`]). The project's own policy and a per-asset mode (`project.settings
//! .proxy_policy`, `MediaAsset.proxy_mode`, `media.set_proxy_policy`, `media.set_proxy_mode`)
//! change the project format: they wait for ADR-0025 (Proposed), and every project follows
//! `auto` until then ([`DEFAULT_POLICY`]).
//!
//! **Import.** `media.import` reports per file whether a proxy was queued
//! ([`ImportProxy`]), after the asset's `index` analysis job.

use std::sync::Arc;

use cutweave_commands::{
    Annotations, CmdError, CommandSpec, Described, EngineCommand, EntryKind, McpExposure,
    Registrable, Registry, RegistryError, Scope, command, wire,
};
use cutweave_jobs::JobId;
use cutweave_model::{EntityKind, MediaAsset, MediaId, MediaKind, ProbeInfo, Project, StreamKind};

/// Most assets one proxy command names.
pub const MAX_PROXY_MEDIA: usize = 256;

/// When proxies are made automatically.
#[wire]
#[derive(Copy, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProxyPolicy {
    /// Never automatically (`media.generate_proxy` still makes one).
    Off,
    /// For video that is slow to scrub ([`auto_reason`]); the default.
    #[default]
    Auto,
    /// For every video file.
    All,
}

/// The policy every project follows until ADR-0025 stores one in the project.
pub const DEFAULT_POLICY: ProxyPolicy = ProxyPolicy::Auto;

/// Long-GOP codecs `auto` considers (FFmpeg codec names).
const LONG_GOP: [&str; 2] = ["h264", "hevc"];

/// Why `auto` gives a video with `probe` a proxy, if it does: long-GOP H.264 or HEVC wider or
/// taller than 1920 pixels, faster than 60 fps, or HEVC with 10-bit samples.
#[must_use]
pub fn auto_reason(probe: &ProbeInfo) -> Option<String> {
    let video = probe.streams.iter().find(|s| s.kind == StreamKind::Video)?;
    let codec = video.codec.as_deref()?;
    if !LONG_GOP.contains(&codec) {
        return None;
    }
    let long = video.width.unwrap_or(0).max(video.height.unwrap_or(0));
    if long > 1920 {
        return Some(format!(
            "{codec} at {}x{} (larger than 1920 pixels)",
            video.width.unwrap_or(0),
            video.height.unwrap_or(0)
        ));
    }
    if let Some(fps) = probe.fps
        && u64::from(fps.num()) > 60 * u64::from(fps.den())
    {
        return Some(format!("{codec} at {fps} fps (faster than 60 fps)"));
    }
    let ten_bit = video
        .pixel_format
        .as_deref()
        .is_some_and(|p| p.contains("10"))
        || video.profile.as_deref().is_some_and(|p| p.contains("10"));
    (codec == "hevc" && ten_bit).then(|| "10-bit HEVC".to_owned())
}

/// Whether `policy` gives `asset` a proxy automatically, and why.
#[must_use]
pub fn wants_proxy(policy: ProxyPolicy, asset: &MediaAsset) -> Option<String> {
    if !proxyable(asset) {
        return None;
    }
    match policy {
        ProxyPolicy::Off => None,
        ProxyPolicy::Auto => auto_reason(&asset.probe),
        ProxyPolicy::All => Some("the policy is `all`".to_owned()),
    }
}

/// Whether `asset` can have a proxy at all: a video file with a picture stream.
#[must_use]
pub fn proxyable(asset: &MediaAsset) -> bool {
    asset.kind == MediaKind::Video
        && asset.path_abs.is_some()
        && asset
            .probe
            .streams
            .iter()
            .any(|s| s.kind == StreamKind::Video)
}

// ----- status ------------------------------------------------------------------------------

/// Where an asset's proxy is.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProxyStatus {
    /// There is none and none is being made.
    None,
    /// Waiting for its job to start.
    Queued,
    /// Being made (see `progress`).
    Generating,
    /// Ready: the preview scrubs it.
    Ready,
    /// A generation stopped before the end (cancelled, or the engine stopped); the next one
    /// continues from where it got.
    Incomplete,
    /// It was removed from the cache (its limit, a clear, `media.delete_proxy`): the original
    /// is used; `media.generate_proxy` makes it again.
    Evicted,
    /// The last generation failed (see `error`).
    Failed,
}

/// One asset's proxy (`ProxyRef` of WP-2F.01).
#[wire]
pub struct ProxyInfo {
    /// The asset.
    pub media_id: MediaId,
    /// Where its proxy is.
    pub status: ProxyStatus,
    /// Whether the project's policy wants one (it is made automatically).
    pub wanted: bool,
    /// Why the policy wants one, or why the asset cannot have one.
    pub reason: Option<String>,
    /// How it is made (`prores_proxy-960`), once there is one.
    pub profile: Option<String>,
    /// The proxy file while it is ready.
    pub path: Option<String>,
    /// Its size in bytes while it is ready.
    #[specta(type = Option<specta_typescript::Number>)]
    pub bytes: Option<u64>,
    /// Its picture size `[width, height]`.
    pub size: Option<[u32; 2]>,
    /// The job making it (queued or generating).
    pub job_id: Option<JobId>,
    /// Done fraction while generating.
    pub progress: Option<f64>,
    /// Why the last generation failed.
    pub error: Option<String>,
}

impl ProxyInfo {
    /// An asset without a proxy.
    #[must_use]
    pub fn none(media_id: MediaId) -> ProxyInfo {
        ProxyInfo {
            media_id,
            status: ProxyStatus::None,
            wanted: false,
            reason: None,
            profile: None,
            path: None,
            bytes: None,
            size: None,
            job_id: None,
            progress: None,
            error: None,
        }
    }
}

/// What `media.import` says about each file's proxy.
#[wire]
#[derive(Copy, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImportProxy {
    /// None is made (the policy does not want one, or the file cannot have one).
    #[default]
    None,
    /// Its generation is queued (after the asset's analysis); `media.proxy_status` follows it.
    Queued,
    /// One is already in the cache (the same content was imported before).
    Ready,
}

/// The engine's proxies, as the proxy commands and `media.import` see them. Implemented by
/// the engine's subsystems; methods may block briefly (SQLite, file deletes) but never wait
/// for a generation.
pub trait ProxyService: Send + Sync {
    /// The proxies of `media` (assets of `doc`).
    fn status(&self, doc: &Project, media: &[MediaId]) -> Vec<ProxyInfo>;

    /// Queues the generation of each proxy of `media` that is not ready or being made;
    /// returns their status afterwards.
    fn generate(&self, doc: &Project, media: &[MediaId]) -> Result<Vec<ProxyInfo>, CmdError>;

    /// Cancels the generation of each of `media` and removes its proxy; returns their status
    /// afterwards.
    fn delete(&self, doc: &Project, media: &[MediaId]) -> Result<Vec<ProxyInfo>, CmdError>;

    /// `media` was just imported into the current document: queues its proxy (after `after`,
    /// its `index` job) if the policy wants one.
    fn imported(&self, media: MediaId, after: &[JobId]) -> ImportProxy;
}

/// What the proxy commands run against (their engine-command context).
pub struct ProxyCall {
    /// The engine's proxies; `None` when it has no media caches.
    pub service: Option<Arc<dyn ProxyService>>,
    /// The current document.
    pub doc: Project,
}

impl std::fmt::Debug for ProxyCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyCall")
            .field("service", &self.service.is_some())
            .finish_non_exhaustive()
    }
}

impl ProxyCall {
    fn service(&self) -> Result<&Arc<dyn ProxyService>, CmdError> {
        self.service
            .as_ref()
            .ok_or_else(|| CmdError::unsupported("this engine makes no proxies"))
    }
}

/// Checks a list of assets for the proxy commands: 1..=256 distinct assets of `doc`, each a
/// video file.
fn check_media(doc: &Project, media: &[MediaId]) -> Result<(), CmdError> {
    if media.is_empty() || media.len() > MAX_PROXY_MEDIA {
        return Err(CmdError::invalid(
            "/media",
            format!("name 1..={MAX_PROXY_MEDIA} assets"),
        ));
    }
    for (i, id) in media.iter().enumerate() {
        let pointer = format!("/media/{i}");
        if media[..i].contains(id) {
            return Err(CmdError::invalid(
                pointer,
                format!("`{id}` is listed twice"),
            ));
        }
        let asset = doc
            .media
            .get(id)
            .ok_or_else(|| CmdError::not_found(EntityKind::Media, *id))?;
        if !proxyable(asset) {
            return Err(CmdError::invalid(
                pointer,
                format!(
                    "media `{id}` is {} {}: only video files have proxies",
                    if asset.kind == MediaKind::Image {
                        "an"
                    } else {
                        "a"
                    },
                    asset.kind
                ),
            ));
        }
    }
    Ok(())
}

/// Result of the proxy commands.
#[wire]
pub struct ProxiesOutput {
    /// One entry per asset, in request order (`media.proxy_status` without `media`: every video
    /// asset, by id).
    pub proxies: Vec<ProxyInfo>,
    /// The policy that decides which assets get one automatically.
    pub policy: ProxyPolicy,
}

// ----- media.generate_proxy ----------------------------------------------------------------

/// Makes proxies of video assets now, whatever the policy: one background job each (two run
/// at a time, one while an export runs), resumed from its finished parts if an earlier one was
/// interrupted. Assets whose proxy is ready or being made are left alone. Returns their status;
/// follow it with `media.proxy_status` or `job.wait`.
#[command(
    name = "media.generate_proxy",
    summary = "Make proxies (small intra-frame copies the preview scrubs) of video assets now",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaGenerateProxy {
    /// The video assets (1..=256, no repeats).
    pub media: Vec<MediaId>,
}

impl EngineCommand for MediaGenerateProxy {
    type Context = ProxyCall;
    type Output = ProxiesOutput;

    fn run(&self, call: &mut ProxyCall) -> Result<ProxiesOutput, CmdError> {
        check_media(&call.doc, &self.media)?;
        Ok(ProxiesOutput {
            proxies: call.service()?.generate(&call.doc, &self.media)?,
            policy: DEFAULT_POLICY,
        })
    }
}

// ----- media.delete_proxy ------------------------------------------------------------------

/// Removes the proxies of video assets from the cache (cancelling a generation in progress):
/// the preview uses the originals again. The assets and their files are untouched; a proxy is
/// not made again automatically until `media.generate_proxy` asks for it.
#[command(
    name = "media.delete_proxy",
    summary = "Remove the proxies of video assets (the preview uses the originals again)",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = true, idempotent = true)
)]
pub struct MediaDeleteProxy {
    /// The video assets (1..=256, no repeats).
    pub media: Vec<MediaId>,
}

impl EngineCommand for MediaDeleteProxy {
    type Context = ProxyCall;
    type Output = ProxiesOutput;

    fn run(&self, call: &mut ProxyCall) -> Result<ProxiesOutput, CmdError> {
        check_media(&call.doc, &self.media)?;
        Ok(ProxiesOutput {
            proxies: call.service()?.delete(&call.doc, &self.media)?,
            policy: DEFAULT_POLICY,
        })
    }
}

// ----- media.proxy_status ------------------------------------------------------------------

/// Shows where the proxies of video assets are: none, queued, generating (with progress),
/// ready (path, size), incomplete, evicted or failed, and whether the policy wants one.
#[wire]
pub struct MediaProxyStatus {
    /// The assets; absent: every video asset of the project.
    #[serde(default)]
    pub media: Option<Vec<MediaId>>,
}

impl Described for MediaProxyStatus {
    const SPEC: CommandSpec = CommandSpec {
        name: "media.proxy_status",
        summary: "Show the proxies of video assets: status, progress, file, size, policy",
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
        rust_type: "MediaProxyStatus",
    };
}

impl Registrable for MediaProxyStatus {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<MediaProxyStatus>().map(drop)
    }
}

impl EngineCommand for MediaProxyStatus {
    type Context = ProxyCall;
    type Output = ProxiesOutput;

    fn run(&self, call: &mut ProxyCall) -> Result<ProxiesOutput, CmdError> {
        let media: Vec<MediaId> = match &self.media {
            Some(media) => {
                check_media(&call.doc, media)?;
                media.clone()
            }
            None => call
                .doc
                .media
                .values()
                .filter(|m| proxyable(m))
                .map(|m| m.id)
                .collect(),
        };
        let proxies = match &call.service {
            Some(service) => service.status(&call.doc, &media),
            None => media.iter().map(|id| ProxyInfo::none(*id)).collect(),
        };
        Ok(ProxiesOutput {
            proxies,
            policy: DEFAULT_POLICY,
        })
    }
}

/// Registers the proxy commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<MediaGenerateProxy>()?
        .register::<MediaDeleteProxy>()?
        .register::<MediaProxyStatus>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::{Extra, FrameRate, StreamInfo};

    use super::*;

    fn video(codec: &str, (w, h): (u32, u32), fps: FrameRate, pix: &str) -> MediaAsset {
        let mut probe = ProbeInfo {
            fps: Some(fps),
            ..ProbeInfo::default()
        };
        probe.streams.push(StreamInfo {
            index: 0,
            kind: StreamKind::Video,
            codec: Some(codec.to_owned()),
            profile: None,
            width: Some(w),
            height: Some(h),
            pixel_format: Some(pix.to_owned()),
            sample_rate: None,
            channels: None,
            language: None,
            extra: Extra::new(),
        });
        MediaAsset::file(MediaId::from_u128(1), MediaKind::Video, "/m/a.mp4", probe)
    }

    #[test]
    fn auto_picks_large_fast_or_10_bit_long_gop_video() {
        let fps120 = FrameRate::new(120, 1).unwrap();
        let cases = [
            (
                video("h264", (3840, 2160), FrameRate::FPS_30, "yuv420p"),
                true,
            ),
            (
                video("h264", (1080, 1920), FrameRate::FPS_30, "yuv420p"),
                false,
            ),
            (
                video("h264", (1080, 2400), FrameRate::FPS_30, "yuv420p"),
                true,
            ),
            (video("h264", (1920, 1080), fps120, "yuv420p"), true),
            (
                video("h264", (1920, 1080), FrameRate::FPS_60, "yuv420p"),
                false,
            ),
            (
                video("hevc", (1920, 1080), FrameRate::FPS_30, "yuv420p10le"),
                true,
            ),
            (
                video("hevc", (1920, 1080), FrameRate::FPS_30, "yuv420p"),
                false,
            ),
            (
                video("h264", (1920, 1080), FrameRate::FPS_30, "yuv420p10le"),
                false,
            ),
            (
                video("prores", (3840, 2160), FrameRate::FPS_30, "yuv422p10le"),
                false,
            ),
        ];
        for (asset, wanted) in &cases {
            assert_eq!(
                wants_proxy(ProxyPolicy::Auto, asset).is_some(),
                *wanted,
                "{:?}",
                asset.probe.streams[0]
            );
            assert!(wants_proxy(ProxyPolicy::Off, asset).is_none());
            assert!(wants_proxy(ProxyPolicy::All, asset).is_some());
        }
        let mut audio = video("aac", (0, 0), FrameRate::FPS_30, "");
        audio.kind = MediaKind::Audio;
        assert!(wants_proxy(ProxyPolicy::All, &audio).is_none());
        assert_eq!(
            auto_reason(&cases[0].0.probe).unwrap(),
            "h264 at 3840x2160 (larger than 1920 pixels)"
        );
    }

    #[test]
    fn the_wire_names_are_snake_case() {
        let v = serde_json::to_value(ProxyStatus::Incomplete).unwrap();
        assert_eq!(v, serde_json::json!("incomplete"));
        assert_eq!(
            serde_json::to_value(ImportProxy::Queued).unwrap(),
            serde_json::json!("queued")
        );
        assert_eq!(
            serde_json::to_value(ProxyPolicy::default()).unwrap(),
            serde_json::json!("auto")
        );
    }
}
