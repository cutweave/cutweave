// SPDX-License-Identifier: GPL-3.0-or-later
//! The cache and decoding commands of the settings pages "Cache and storage" and "Performance
//! and hardware" (WP-1D.08; arch §7, §16; ADR-0016).
//!
//! - `media.cache_status` (a read-only engine command, in the MCP catalog): the cache folder,
//!   each disk cache's usage and limit, the hardware decoding policy and the memory the decoded
//!   frame cache may use. Agents may read it.
//! - `media.set_cache_config`, `media.clear_cache`, `media.set_decode_policy` and
//!   `media.set_proxy_config` (WP-2F.03b: the proxy size and the proxy jobs at once) (**UI-only**
//!   scope): the desktop app applies its machine preferences to its engine after each
//!   handshake and when they change. None is undoable or journaled, none touches the project
//!   document or marks it dirty, and an agent token gets `forbidden`. A lower limit shrinks
//!   its cache at once (least recently used first); clearing deletes only cache files (never a
//!   project or `generated/` media), and everything is rebuilt on demand.
//!
//! The commands act through [`CacheControl`], which the engine's subsystems provide
//! ([`Subsystems::cache_control`](crate::Subsystems::cache_control)); an engine without media
//! caches reports empty status and refuses the others as unsupported.

use std::sync::Arc;

use cutweave_commands::{
    CmdError, CommandSpec, Described, EngineCommand, EntryKind, McpExposure, Registrable, Registry,
    RegistryError, Scope, command, wire,
};

/// The smallest decoded-frame memory `media.set_cache_config` accepts: 64 MiB.
pub const MIN_FRAME_CACHE_BYTES: u64 = 64 << 20;
/// The largest decoded-frame memory `media.set_cache_config` accepts: 64 GiB.
pub const MAX_FRAME_CACHE_BYTES: u64 = 64 << 30;
/// The largest disk cache limit `media.set_cache_config` accepts: 16 TiB.
pub const MAX_CACHE_LIMIT_BYTES: u64 = 16 << 40;

/// The proxy sizes `media.set_proxy_config` accepts: the longest side in pixels ("540p",
/// "720p" and "960p" in the app's settings; a 16:9 picture at 960 is 960 × 540).
pub const PROXY_SIZES: [u32; 3] = [540, 720, 960];
/// The proxy size of an engine nobody configured.
pub const DEFAULT_PROXY_SIZE: u32 = 960;
/// The most proxy jobs at once `media.set_proxy_config` accepts.
pub const MAX_PROXY_JOBS: u32 = 8;
/// The proxy jobs at once of an engine nobody configured.
pub const DEFAULT_PROXY_JOBS: u32 = 2;

/// A kind of disk cache.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CacheKind {
    /// Thumbnails and filmstrips (JPEG files, accounted in `cache.sqlite`).
    Thumbnails,
    /// Waveform pyramids (one file per audio stream).
    Waveforms,
    /// Proxies: small intra-frame copies of video files the preview scrubs (WP-2F.01). Clearing
    /// them makes the preview use the originals; `media.generate_proxy` makes them again.
    Proxies,
}

impl CacheKind {
    /// Every kind, in the order the settings page lists them.
    pub const ALL: [CacheKind; 3] = [
        CacheKind::Thumbnails,
        CacheKind::Waveforms,
        CacheKind::Proxies,
    ];
}

/// Whether decoders may use the hardware.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DecodeHw {
    /// A verified hardware decoder when the machine has one for the stream (the default).
    Auto,
    /// Software decoding only.
    Off,
}

/// The limits `media.set_cache_config` changes; a kind that is absent keeps its limit.
#[wire]
#[derive(Default)]
pub struct CacheLimits {
    /// Cap on thumbnails and filmstrips, in bytes.
    #[specta(type = Option<specta_typescript::Number>)]
    pub thumbnails: Option<u64>,
    /// Cap on waveforms, in bytes.
    #[specta(type = Option<specta_typescript::Number>)]
    pub waveforms: Option<u64>,
    /// Cap on proxies, in bytes (50 GiB by default).
    #[serde(default)]
    #[specta(type = Option<specta_typescript::Number>)]
    pub proxies: Option<u64>,
}

/// One disk cache's usage and limit.
#[wire]
pub struct CacheKindStatus {
    /// Which cache.
    pub kind: CacheKind,
    /// Bytes it holds.
    #[specta(type = specta_typescript::Number)]
    pub bytes: u64,
    /// Bytes it may hold before the least recently used entries are deleted.
    #[specta(type = specta_typescript::Number)]
    pub limit_bytes: u64,
    /// Entries it holds.
    #[specta(type = specta_typescript::Number)]
    pub entries: u64,
}

/// What `media.cache_status` and `media.set_cache_config` report.
#[wire]
pub struct MediaCacheStatusOutput {
    /// The folder the caches live in (`--cache-dir`, else the OS cache folder); `null` when
    /// the engine has none.
    pub folder: Option<String>,
    /// Each disk cache, in a fixed order (thumbnails, waveforms, proxies).
    pub kinds: Vec<CacheKindStatus>,
    /// The hardware decoding policy in force.
    pub decode_hw: DecodeHw,
    /// Bytes of decoded frames the RAM frame cache may hold.
    #[specta(type = specta_typescript::Number)]
    pub frame_cache_bytes: u64,
    /// The longest side, in pixels, of the proxies made from now on (540, 720 or 960).
    pub proxy_size: u32,
    /// How many proxy jobs run at once (one while an export runs).
    pub proxy_jobs: u32,
}

/// What clearing one kind removed.
#[wire]
pub struct ClearedKind {
    /// Which cache.
    pub kind: CacheKind,
    /// Entries deleted.
    #[specta(type = specta_typescript::Number)]
    pub removed: u64,
    /// Bytes freed.
    #[specta(type = specta_typescript::Number)]
    pub freed_bytes: u64,
}

/// Result of `media.clear_cache`.
#[wire]
pub struct MediaClearCacheOutput {
    /// What each requested kind lost.
    pub cleared: Vec<ClearedKind>,
}

/// Result of `media.set_decode_policy`.
#[wire]
pub struct MediaSetDecodePolicyOutput {
    /// The policy now in force.
    pub hw: DecodeHw,
}

/// The engine's media caches and decoding policy, as the cache commands see them. Implemented
/// by the engine's subsystems; every method may block briefly (SQLite, file deletes).
pub trait CacheControl: Send + Sync {
    /// The folder, usage and limits.
    fn status(&self) -> MediaCacheStatusOutput;

    /// Applies new limits (shrinking a cache at once) and decoded-frame memory (`0` is
    /// "Auto": the smaller of 1 GiB and a quarter of RAM); returns the status in force.
    fn set_config(
        &self,
        limits: &CacheLimits,
        frame_cache_bytes: Option<u64>,
    ) -> Result<MediaCacheStatusOutput, CmdError>;

    /// Deletes every entry of each of `kinds`.
    fn clear(&self, kinds: &[CacheKind]) -> Result<Vec<ClearedKind>, CmdError>;

    /// Sets the hardware decoding policy for decoders opened from now on; decoders already
    /// open are closed when idle and reopened by the next seek.
    fn set_decode_hw(&self, hw: DecodeHw) -> Result<(), CmdError>;

    /// Sets the size of the proxies made from now on and how many proxy jobs run at once (a
    /// value left out stays); returns the status in force.
    fn set_proxy_config(
        &self,
        size: Option<u32>,
        jobs: Option<u32>,
    ) -> Result<MediaCacheStatusOutput, CmdError>;
}

/// What the cache commands run against: the engine's [`CacheControl`], if it has one.
pub struct CacheCall {
    pub(crate) control: Option<Arc<dyn CacheControl>>,
}

impl std::fmt::Debug for CacheCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheCall")
            .field("control", &self.control.is_some())
            .finish()
    }
}

impl CacheCall {
    fn control(&self) -> Result<&Arc<dyn CacheControl>, CmdError> {
        self.control
            .as_ref()
            .ok_or_else(|| CmdError::unsupported("this engine has no media caches"))
    }
}

// ----- media.cache_status ---------------------------------------------------------------------

/// Returns the media caches: the folder they live in, each disk cache's usage and limit, the
/// hardware decoding policy and the memory the decoded-frame cache may use. Agents may read it;
/// changing any of it is the desktop app's.
#[wire]
pub struct MediaCacheStatus {}

impl Described for MediaCacheStatus {
    const SPEC: CommandSpec = CommandSpec {
        name: "media.cache_status",
        summary: "Show the media caches: folder, usage and limits, hardware decoding, frame memory",
        kind: EntryKind::Engine,
        undoable: false,
        view: false,
        scope: Scope::Any,
        mcp: McpExposure::Catalog,
        annotations: cutweave_commands::Annotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        secret_fields: &[],
        rust_type: "MediaCacheStatus",
    };
}

impl Registrable for MediaCacheStatus {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<MediaCacheStatus>().map(drop)
    }
}

impl EngineCommand for MediaCacheStatus {
    type Context = CacheCall;
    type Output = MediaCacheStatusOutput;

    fn run(&self, call: &mut CacheCall) -> Result<MediaCacheStatusOutput, CmdError> {
        Ok(match &call.control {
            Some(control) => control.status(),
            None => MediaCacheStatusOutput {
                folder: None,
                kinds: Vec::new(),
                decode_hw: DecodeHw::Auto,
                frame_cache_bytes: 0,
                proxy_size: DEFAULT_PROXY_SIZE,
                proxy_jobs: DEFAULT_PROXY_JOBS,
            },
        })
    }
}

// ----- media.set_cache_config -----------------------------------------------------------------

/// Sets the disk cache limits and the memory for decoded frames. A lower limit deletes the
/// least recently used entries at once. Not undoable, not journaled, never marks the project
/// dirty; UI credential only (the desktop app applies the person's machine preferences).
#[command(
    name = "media.set_cache_config",
    summary = "Set the cache limits and the memory for decoded frames",
    engine,
    scope = "ui",
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = true, idempotent = true)
)]
pub struct MediaSetCacheConfig {
    /// New limits in bytes (`0` to `16 TiB`); a kind left out keeps its limit.
    pub limits: Option<CacheLimits>,
    /// Memory for decoded frames in bytes (64 MiB to 64 GiB), or `0` for Auto (the smaller of
    /// 1 GiB and a quarter of the machine's RAM). Left out: unchanged.
    #[specta(type = Option<specta_typescript::Number>)]
    pub frame_cache_bytes: Option<u64>,
}

impl EngineCommand for MediaSetCacheConfig {
    type Context = CacheCall;
    type Output = MediaCacheStatusOutput;

    fn run(&self, call: &mut CacheCall) -> Result<MediaCacheStatusOutput, CmdError> {
        let control = call.control()?;
        if let Some(limits) = &self.limits {
            for (path, bytes) in [
                ("/limits/thumbnails", limits.thumbnails),
                ("/limits/waveforms", limits.waveforms),
                ("/limits/proxies", limits.proxies),
            ] {
                if bytes.is_some_and(|b| b > MAX_CACHE_LIMIT_BYTES) {
                    return Err(CmdError::invalid(path, "must be at most 16 TiB"));
                }
            }
        }
        if let Some(bytes) = self.frame_cache_bytes
            && bytes != 0
            && !(MIN_FRAME_CACHE_BYTES..=MAX_FRAME_CACHE_BYTES).contains(&bytes)
        {
            return Err(CmdError::invalid(
                "/frame_cache_bytes",
                "must be 0 (Auto) or between 64 MiB and 64 GiB",
            ));
        }
        control.set_config(
            &self.limits.clone().unwrap_or_default(),
            self.frame_cache_bytes,
        )
    }
}

// ----- media.clear_cache ----------------------------------------------------------------------

/// Deletes every entry of the named caches. Projects and `generated/` media are never touched:
/// only files the engine can rebuild from the media. Not undoable, not journaled, never marks
/// the project dirty; UI credential only.
#[command(
    name = "media.clear_cache",
    summary = "Delete everything in the named media caches (they rebuild on demand)",
    engine,
    scope = "ui",
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = true, idempotent = true)
)]
pub struct MediaClearCache {
    /// The caches to empty (`thumbnails`, `waveforms`, `proxies`).
    pub kinds: Vec<CacheKind>,
}

impl EngineCommand for MediaClearCache {
    type Context = CacheCall;
    type Output = MediaClearCacheOutput;

    fn run(&self, call: &mut CacheCall) -> Result<MediaClearCacheOutput, CmdError> {
        let control = call.control()?;
        let mut kinds = Vec::new();
        for kind in &self.kinds {
            if !kinds.contains(kind) {
                kinds.push(*kind);
            }
        }
        Ok(MediaClearCacheOutput {
            cleared: control.clear(&kinds)?,
        })
    }
}

// ----- media.set_decode_policy ----------------------------------------------------------------

/// Sets whether decoders may use the hardware. It applies to decoders opened from now on; idle
/// decoders are closed and the open pools reopen on their next seek. Exports and `render.frame`
/// always decode in software. Not undoable, not journaled, never marks the project dirty; UI
/// credential only.
#[command(
    name = "media.set_decode_policy",
    summary = "Choose hardware or software decoding for the preview",
    engine,
    scope = "ui",
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaSetDecodePolicy {
    /// `auto` (a verified hardware decoder when there is one) or `off` (software only).
    pub hw: DecodeHw,
}

impl EngineCommand for MediaSetDecodePolicy {
    type Context = CacheCall;
    type Output = MediaSetDecodePolicyOutput;

    fn run(&self, call: &mut CacheCall) -> Result<MediaSetDecodePolicyOutput, CmdError> {
        call.control()?.set_decode_hw(self.hw)?;
        Ok(MediaSetDecodePolicyOutput { hw: self.hw })
    }
}

// ----- media.set_proxy_config -----------------------------------------------------------------

/// Sets how proxies are made on this computer: their size (the longest side: 540, 720 or 960
/// pixels) and how many proxy jobs run at once (1 to 8; one while an export runs). A new size
/// applies to proxies made from now on: generations of the old size stop, the open project's
/// wanted proxies are queued at the new one, and ready proxies of other sizes stay in the cache
/// unused until its limit removes them. Not undoable, not journaled, never marks the project
/// dirty; UI credential only (the desktop app applies the person's machine preferences).
#[command(
    name = "media.set_proxy_config",
    summary = "Set the proxy size and how many proxy jobs run at once",
    engine,
    scope = "ui",
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaSetProxyConfig {
    /// The longest side of new proxies in pixels: 540, 720 or 960. Left out: unchanged.
    pub size: Option<u32>,
    /// Proxy jobs at once, 1 to 8. Left out: unchanged.
    pub jobs: Option<u32>,
}

impl EngineCommand for MediaSetProxyConfig {
    type Context = CacheCall;
    type Output = MediaCacheStatusOutput;

    fn run(&self, call: &mut CacheCall) -> Result<MediaCacheStatusOutput, CmdError> {
        let control = call.control()?;
        if self.size.is_some_and(|s| !PROXY_SIZES.contains(&s)) {
            return Err(CmdError::invalid("/size", "must be 540, 720 or 960"));
        }
        if self
            .jobs
            .is_some_and(|j| !(1..=MAX_PROXY_JOBS).contains(&j))
        {
            return Err(CmdError::invalid("/jobs", "must be between 1 and 8"));
        }
        control.set_proxy_config(self.size, self.jobs)
    }
}

/// Registers the cache commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<MediaCacheStatus>()?
        .register::<MediaSetCacheConfig>()?
        .register::<MediaClearCache>()?
        .register::<MediaSetDecodePolicy>()?
        .register::<MediaSetProxyConfig>()?;
    Ok(())
}
