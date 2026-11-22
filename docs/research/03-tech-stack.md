# Cutweave - Tech Stack Research (03)

Date: 2026-11-22. Scope: stress-test of "Rust engine + web UI in Tauri 2". AI/ML excluded.

Legend: [V] verified today via gh / crates.io / npm / fetched page. [U] unverified, from prior knowledge or inference; confirm before committing. [I] my inference/opinion.

---

## 0. Executive summary

1. The Rust-engine half of the hypothesis holds up very well. wgpu 30.0.2 (2026-11-12) is active and has gained exactly the zero-copy video-interop hooks we need (see 3.1). FFmpeg bindings are alive again (ffmpeg-next 9.1.0, 2026-11-11).
2. The Tauri + webview half is the weak link, for the specific job of "show GPU-composited frames in the preview at 30/60 fps with low latency":
   - On Linux, WebKitGTK is the shipped webview. It has a poor reputation for performance (open Tauri issue #14963 describes it as "really bad" [V that the issue exists; the claim is a user opinion]), and I found no evidence of WebGPU in WebKitGTK [U - absence of evidence]. WebCodecs on WebKitGTK is unconfirmed.
   - The only robust way to get native-GPU frames next to a webview is a native child surface (wgpu) composited alongside the webview. wry has an official example for it, but it is X11-only on Linux (the example refuses Wayland) [V], and community prototypes list "Windows/macOS support", "mouse pass-through", and "put surface behind the webview" as TODO [V: tauri-wgpu-tag README].
3. Strongest contradicting evidence: **Cap** (CapSoftware/Cap, 25k stars, Rust, a Tauri + wgpu + FFmpeg product with a timeline editor) **abandoned Tauri**. Since 0.6.1 its only desktop app is `apps/desktop-gpui` - "No Tauri, no webview - the whole UI is drawn by gpui and every pixel is Rust" [V, repo README]. Its own playback notes claim the GPUI editor sustains 60 fps with zero dropped frames and uses 67% less CPU and 57% less memory than an "optimized Electron" build [V as a claim in their doc; not independently reproduced]. This is the closest real-world precedent to Cutweave and it went the opposite direction of our hypothesis.
4. Recommendation: keep Rust core + wgpu, but make the **UI shell decision the first spike** (Spike 1 and 2 below). Default plan: Tauri 2 + React with a native wgpu preview surface (child window), with GPUI as the explicit plan B, decided by a hard pass/fail within ~3 weeks. Isolate engine behind a UI-agnostic crate + command/event protocol so the shell is swappable.
5. Browser build: feasible for a *subset* (WebCodecs + WebGPU + wasm engine core), but FFmpeg-in-Rust won't compile to wasm32 usefully; plan a separate decode/encode backend trait (WebCodecs/mediabunny in browser). Do not promise code parity beyond data model, timeline logic, render graph, and WGSL shaders.

---

## 1. App shell & UI

### 1.1 Versions and health [V]
| Item | Version | Date | Note |
|---|---|---|---|
| tauri (crates.io) | 2.12.7 | 2026-11-18 | very active, ~47M downloads |
| wry | 0.57.3 | 2026-11-18 | |
| @tauri-apps/api | 2.12.5 | npm | |
| tauri-action | v1.0.0 | 2026-06-29 | GH Action for builds/releases |
| gpui (crates.io) | 0.2.2 | 2026-10-30 | "pre-1.0, breaking changes between versions" per README |
| zed | v1.29.3 | 2026-11-20 | GPUI's host app, 95k stars |
| egui | 0.37.0 | 2026-10-15 | |
| slint | 1.19.0 | 2026-11-16 | GPL / royalty-free / commercial licensing [U on exact terms] |
| iced | 0.14.0 | 2025-12-07 | |
| dioxus | 0.7.10 | 2026-10-16 (crate) | |
| xilem | 0.4.0 | 2025-10-29 | still experimental |
| tauri-apps/cef-rs | active (pushed 2026-11-22), 495 stars | | CEF bindings by Tauri org; Tauri-on-CEF runtime appears to be experimental/community (several `tauri-runtime-cef` forks with 0 stars) [V repos exist; U maturity] |

### 1.2 Webview per OS
- Windows: WebView2 (Chromium, evergreen). WebGPU + WebCodecs available [U - consistent with Chromium baseline; verify in Spike 2]. Child HWND in-process.
- macOS: WKWebView; WebCodecs full from Safari 26, WebGPU shipped in Safari 26 [V caniuse for WebCodecs: Safari full from 26.0; global ~95.9%]. Tied to OS version; macOS < 26 gets partial WebCodecs.
- Linux: WebKitGTK. Release 2.50 notes (Nov 2025) discuss Skia threaded rendering/damage propagation; **no mention of WebGPU or WebCodecs** [V]. A WebKit Bugzilla entry (GTK WebGPU, opened 2023) reported `navigator.gpu` null [V it exists; U current state]. A webkit-dev post (2025-07) said GTK/WPE only use the GPU process for WebGL [V via search summary]. Net: assume **no WebGPU and unreliable WebCodecs** on Linux webview [I]. Also: distro-dependent WebKitGTK versions, Wayland/NVIDIA rendering glitches are commonly reported [U].
- Implication: any design that depends on WebGPU/WebCodecs *inside the webview* is not portable to Linux under Tauri. Design the UI to be a thin shell: timeline/inspector/media bin in DOM/Canvas2D, preview/composite in native.

### 1.3 Ways to get frames to the screen (ranked for Cutweave)
A. **Native wgpu child surface beside/under a transparent webview** (the Cap-on-Tauri/Spacedrive style approach).
   - wry has `examples/wgpu.rs`: wgpu surface from the window + transparent webview via `build_as_child` [V]. Linux path is GTK + X11 only; example explicitly rejects Wayland [V]. Child webviews are "macOS, Windows, Linux (X11 only)" [V via search summary].
   - Problems seen in prototypes (oscartbeaumont/tauri-wgpu-tag, pushed 2026-11-17): works best with fixed-size elements; resize coordination between DOM layout and native surface needs custom protocol; surface cannot yet go behind the body; click pass-through, multi-surface, Windows and macOS all unchecked TODOs [V].
   - Z-order: DOM overlays (playhead, safe-area guides, transform handles, text boxes) over the video are the hard part; you either draw overlays in the native layer, or put the webview on top with transparency and accept per-OS compositing quirks (white flash on transparent windows reported in wgpu-native issue [V via search]).
   - Wayland is the blocker: Tauri+wry child surfaces on Wayland are not supported (X11/XWayland fallback) [V for the example; U for general state].
B. **Frames as bytes over IPC to a `<canvas>`** (JPEG/raw RGBA via Tauri custom protocol `cutweave://` or channel; WebCodecs `VideoFrame` -> canvas/WebGPU on Chromium).
   - Tauri IPC docs: commands are JSON-serialised; events are one-way JSON [V docs]. Channels and custom URI schemes are the byte-friendly paths [U - docs page I fetched did not cover them]. Budget math [I]: 1080p RGBA = 8.3 MB/frame; 60 fps = ~500 MB/s through IPC + upload + texImage2D: not viable as the main path. 1080p JPEG ~150-300 KB * 60 = ~15 MB/s is fine but costs encode+decode latency and loses color accuracy. Fine for *scrub-preview fallback* and thumbnails, not primary playback.
C. **Shared memory / shared texture into the webview**: WebView2 has `ICoreWebView2SharedBuffer` / composition hosting [U]; WKWebView and WebKitGTK have no equivalent. Not portable.
D. **Webview plays a proxy `<video>`** and the native engine only renders on export. This is what many web editors do; loses effect-accurate preview. Possible as a phase-1 fallback.
E. **Drop the webview for the preview window**: separate native window (wgpu) docked visually; poor UX docking on Windows/macOS/Wayland.
F. **No webview at all** (GPUI/egui/Slint/Iced): preview is just a wgpu texture in the UI tree. Zero IPC. This is what Cap did.

### 1.4 Real projects [V unless noted]
- **Cap** (Tauri+Rust+wgpu+FFmpeg, SolidJS UI) -> rewrote desktop in GPUI, shipped as 0.6.1+; Tauri app last shipped 0.6.0, source removed after 0.6.2. Crate layout includes `editor`, `rendering`, `rendering-skia`, `gpu-converters`, `video-decode`, `enc-ffmpeg`, `enc-avfoundation`, `enc-mediafoundation`, `scap-*` (platform capture) - a very good reference for how a Rust media workspace is organised. Open issues show real-world wgpu pain: crash on AMD Radeon 780M iGPU (#1954), Linux SIGSEGV after wgpu device lost on NVIDIA (#3946). They still use Tauri's *bundler/updater* (`tauri bundle`) under GPUI, i.e. Tauri tooling is reusable even without the Tauri runtime. They also note debug builds of pixel loops are 23x too slow (42.7 ms vs 1.0 ms/frame) -> need opt-level on hot crates in dev profile.
- **kenichi-preview** (simplearyan): "Tauri v2 + Rust + WGPU + FFmpeg native-layer video player bypassing the `<video>` tag"; hobby project, last push 2026-02 [V exists; U quality].
- **tauri-wgpu-cam**, **tauri-wgpu-tag**: prototypes, no production use [V].
- Electrobun ships an `<electrobun-wgpu>` tag (embedding a native wgpu surface in a page) [V it exists via search]; tauri-wgpu-tag is an attempt to copy it - a sign the pattern is wanted but not solved in Tauri.
- Other OSS editors: Kdenlive (GPL-3, C++/MLT/Qt, 6.2k stars, active), Shotcut (GPL-3, 15.8k), OpenCut (MIT, browser/TypeScript, 99k stars, last push 2026-09-24) [V]. Many small "Tauri + React + FFmpeg" editors exist on GitHub, all <10 stars and mostly shell out to the ffmpeg CLI [V].

### 1.5 Option comparison
| Option | Preview path | Linux story | Pros | Cons |
|---|---|---|---|---|
| Tauri 2 + React + native wgpu child | native surface | X11 only; WebKitGTK webview | Best web UI ecosystem, hiring, shadcn; small binary | Surface/DOM sync, z-order, Wayland gap, per-OS hacks; UI perf on WebKitGTK |
| Tauri 2 + React + canvas via IPC | canvas | WebKitGTK | Simple | Throughput/latency; color accuracy; not scalable to 4K |
| Electron + React + WebGPU/WebCodecs | in-webview WebGPU + native addon | Chromium everywhere | One consistent engine; WebCodecs+WebGPU in renderer; shared texture APIs (offscreen rendering) [U] | 150 MB+ install; Node/native addon layer; not "Rust UI" but engine can be Rust via napi-rs |
| Tauri + CEF runtime | like Electron | Chromium | Consistent webview | Experimental [U], 100MB+ installers, API churn |
| GPUI (Zed) | in-tree wgpu texture | Wayland+X11 | Proven by Cap & Zed; preview = texture; fast | Pre-1.0 breaking changes; tiny ecosystem; no ready timeline/inspector widgets; accessibility/IME immaturity [U]; own wgpu/font fork pins (Cap: first build 11 min, 6.4 GB target) [V] ; contributor barrier (Rust-only UI) |
| Slint | wgpu texture import supported [U] | good | Declarative, mature, embedded heritage | License (GPLv3/royalty-free/commercial) conflicts with some OSS goals [U]; custom widgets for timeline are work |
| egui | wgpu native via eframe/egui-wgpu | good | Fast to prototype, immediate mode | Styling limits, accessibility, text quality; heavy UI redraw costs |
| Iced | wgpu native | ok | Elm style | Smaller widget set; 0.14 released Dec 2025 |
| Dioxus | webview (wry) or blitz | same as Tauri | React-like in Rust | Same wry limitations; Blitz native renderer is young [U] |
| Xilem/Masonry | vello | ok | Good text/GPU stack | 0.4.0 Oct 2025, pre-production |

[I] Recommendation: **Tauri 2 + React with a native wgpu preview child surface, but gated by Spike 1**. If Spike 1 fails on Linux-Wayland or overlay interaction, fall back to **GPUI** (not egui) since Cap proved a Rust-engine editor there; keep UI logic out of the engine so the swap costs the UI only. Electron is the pragmatic "boring" fallback if the team prefers TS UI over Rust UI and can accept 150 MB.

---

## 2. Media I/O

### 2.1 FFmpeg status [V]
- FFmpeg releases listed on ffmpeg.org today: 9.1, 9.0.2, 8.1.3, 8.0.3, 7.1.5 (also 6.1.6, 5.1.10, 4.4.8). 8.1 "Hoare" released 2026-03-16 with more Vulkan compute codecs (ProRes, DPX), D3D12 H.264/AV1 encode; 8.0 was Aug 2025.
- **ffmpeg-next** (zmwangx/rust-ffmpeg): crate 9.1.0 and ffmpeg-sys-next 9.1.1 both published 2026-11-11; release tags v8.1.0, v9.0.0, v9.1.0 -> the crate tracks FFmpeg majors in lockstep (crate version == FFmpeg version). Repo moved under zmwangx; pushed 2026-11-16, 2k stars. Maintenance is healthy again. 9.2M downloads.
- **ffmpeg-the-third** 6.0.0+ffmpeg-9.0 (2026-08) - an actively-maintained fork alternative [V].
- **rsmpeg** 0.18.0+ffmpeg.8.0 (2025-08-24), last push 2025-08-24 -> ~15 months stale, FFmpeg 8.0 only [V]. Thinner, closer to the C API; but stalled.
- **gstreamer-rs** 0.25.4 (2026-09-21), active [V]. GStreamer pro: pipeline graph, hw plugins (va, d3d11, nvcodec, vulkan), GES (editing services) exists. Con: distribution weight on Windows/macOS, plugin matrix, frame-accurate random access scrubbing is awkward vs. libav direct decode, LGPL dynamic-link obligations. [I] GStreamer is a poor fit for a scrub-heavy NLE; reconsider only for Linux hardware paths.
- **Pure Rust**: symphonia 0.6.1 (2026-08, audio demux/decode: MP3/AAC/FLAC/Vorbis/ALAC/WAV, no encode) [V]; re_mp4 0.5.1 (MP4 demux, from Rerun) [V]; mp4parse 0.17.0 (2023, stale) [V]; openh264 0.9.10 (Cisco OpenH264 wrapper, 2026-11-10) [V] - useful as a *license-clean* H.264 software codec because Cisco pays the patent royalties if you use their prebuilt binary [U - verify terms]. vk-video 0.3.1 (2026-04, Software Mansion, Vulkan Video H.264 decode/encode into wgpu) [V crate exists; U feature set from memory].
- Pure Rust cannot cover HEVC/ProRes/DNxHD/VP9/AV1 broadly or the container zoo. FFmpeg stays the baseline.

### 2.2 Frame-accurate seeking and VFR [I unless noted]
- Build an index at import: demux pass (packets only, fast) to record per-stream pts/dts, keyframe positions, duration; store in SQLite. Convert everything to rational timebase + integer "frame ticks" on the project timebase; never use float seconds.
- Seek = jump to previous keyframe, decode forward to target pts, discard. Cache decoded frames in a GOP-aware LRU; keep N decoder instances (Cap uses a "multi-position decoder pool for smooth scrubbing" [V]).
- VFR: keep source pts; timeline conforms by nearest-frame lookup via index; offer "conform to CFR" proxy. Detect via pts deltas; phone footage is commonly VFR.
- Long-GOP H.264/HEVC scrub is the performance killer -> generate intra-frame proxies (see 5.4) automatically.

### 2.3 Hardware decode/encode matrix
| Platform | Decode API | Encode API | Zero-copy to wgpu today |
|---|---|---|---|
| Windows | D3D11VA (FFmpeg `d3d11va`, `dxva2`), D3D12VA decode (FFmpeg 7+/8), NVDEC, QSV | NVENC, AMF, QSV, MediaFoundation, D3D12 H.264/AV1 enc (FFmpeg 8.1) [V] | wgpu DX12 backend: share NV12 texture via shared handle; `dx12::Texture::with_plane_slice` added in wgpu 30 for wrapping one plane of NV12 DXGI resource [V changelog]. Requires wgpu on DX12 backend and decoder on the same adapter. |
| macOS | VideoToolbox (FFmpeg `videotoolbox`; AVAssetReader as Cap uses [V]) | VideoToolbox | IOSurface -> Metal texture -> wgpu via `create_texture_from_hal` (Metal HAL). Per-platform unsafe interop code [U details]. |
| Linux | VA-API (Intel/AMD), NVDEC (CUDA), Vulkan Video (FFmpeg 8.x; vk-video crate) | VA-API, NVENC, Vulkan encode (experimental) | wgpu 30 adds `vulkan::Device::texture_from_dmabuf_fd()` with VULKAN_EXTERNAL_MEMORY_FD/DMA_BUF features [V changelog]; GLES DMA-BUF import also added [V]. VA-API -> DRM prime dmabuf -> Vulkan import is the path. Modifier/format negotiation is the hard part (changelog mentions compressed modifiers [V]). |

Notes:
- wgpu 30 changelog also lists `Device::import_external_texture` for WebCodecs `VideoFrame`/`HTMLVideoElement` (WebGPU/wasm target) [V] - relevant for browser build.
- `create_texture_from_hal` now takes an explicit `initial_state` and the changelog states it affected "zero-copy hardware-decoded video imports" [V] - confirms others are doing this in practice (AdrianEddy of Gyroflow is credited on the DX12 and external-texture PRs [V]; Gyroflow is a good prior-art project [U]).
- Achievable today: **zero-copy is achievable per platform with 150-400 lines of unsafe interop each [I]**, but each has driver/modifier edge cases. Strategy: ship phase 1 with hw decode -> CPU copy -> `queue.write_texture` (NV12 planes uploaded as R8/RG8 and converted to RGB in WGSL). 1080p NV12 upload is 3 MB/frame - fine. Add zero-copy per platform as optimization behind a trait. Software fallback must always exist.
- NVDEC in FFmpeg needs CUDA runtime/driver only (no CUDA toolkit at runtime besides driver; nv-codec-headers at build) [U]. Hardware AV1 decode availability depends on GPU gen.
- Export: prefer encode via FFmpeg (libx264 requires GPL build; see 8.3) or hw encoders (NVENC/AMF/QSV/VideoToolbox, all LGPL-compatible via FFmpeg's wrappers).

### 2.4 Recommendation
ffmpeg-next 9.1 (+ ffmpeg-sys-next) pinned to FFmpeg 8.1.x or 9.x shared build [see 8.3], behind a `MediaBackend` trait (decode/encode/probe) so GStreamer, WebCodecs (browser), or AVFoundation (macOS, as Cap does) can be substituted. Use symphonia for fast audio-only waveform decode paths (no FFmpeg process needed), optional.

Open question [U]: FFmpeg 9.x is brand new (9.0.2 and 9.1 exist); check API breaks and distribution availability (BtbN builds `latest` exist daily [V]). Pin to 8.1.x LTS-like if 9.x binaries are not yet in distros.

---

## 3. Rendering / compositing

### 3.1 wgpu [V]
- wgpu 30.0.2 (2026-11-12); v30.0.0 on 2026-07-01; v30.0.1 2026-08-21. ~53M downloads; 18.6k stars; very active. Majors arrive roughly quarterly -> expect migration chores (changelog 30 shows breaking changes in vertex buffers, naga interpolation, `create_texture_from_hal`). Pin the version and upgrade deliberately; don't float.
- Backends: Vulkan, Metal, DX12, GL(ES), WebGPU. Video-interop additions in 30 listed above. EXTERNAL_TEXTURE feature originally DX12-only [V changelog line] - check current state per backend [U].
- Compute: yes (needed for scopes, histograms, blur, optical flow style effects).
- Known real-world issues: device-lost crashes on some Linux NVIDIA / Windows AMD iGPU (Cap issues) [V]. Implement device-lost recovery and a software (llvmpipe/WARP) fallback test path.

### 3.2 Render graph and effects
- [I] Design: retained timeline -> per-frame "evaluate at time t" -> render graph (DAG of passes: source decode/upload -> color convert to working space -> per-clip effect chain -> transform/mask -> blend into track -> transitions (two-input passes) -> adjustment layers -> output transform -> scopes/display). Passes are WGSL render or compute; allocate from a texture pool with lifetime analysis; cache clip-level results by (clip id, params hash, time) for static content.
- Effects/transitions in WGSL with a declarative manifest (params, UI hints) -> same shader source runs desktop and browser WebGPU. Blend modes: implement in premultiplied linear space; port from W3C compositing spec (normal, multiply, screen, overlay, soft-light, etc.).
- Reference graphs: Cap's `rendering` crate, Gyroflow (Rust+wgpu OpenFX/ shaders) [U], Vello (compute-based 2D renderer) as an inspiration.
- Alternative: use an existing 2D/3D engine (Bevy render, Vello). Bevy is too heavy/unstable; build thin custom graph over wgpu.

### 3.3 Color management [I; no web verification]
- Internal working space: **linear-light, scene-referred float16 (Rgba16Float) RGB with Rec.709 (BT.709 primaries) default; allow Rec.2020 and ACEScg-style wide-gamut later**. Source frames: decode YUV (matrix/range/transfer from stream metadata - must read colorspace tags correctly, a common bug), upload, convert to linear in shader.
- Transfer functions in shader: sRGB/BT.709 OETF inverse, BT.1886 display, PQ (ST 2084), HLG (ARIB B67). HDR->SDR tone map (e.g., BT.2390 / Reinhard / ACES-ish) for preview and export.
- OCIO: OpenColorIO is C++ (BSD); no maintained pure-Rust port; wrapping it adds a heavy C++ dependency [U]. Recommendation: skip OCIO in v1; implement a small built-in set (709/2020/P3-D65/sRGB, PQ/HLG/sRGB/gamma 2.4) as WGSL functions + 3D LUT (.cube) support; keep a "ColorSpace" enum ready to map to OCIO later.
- Compositing in linear float vs 8-bit gamma: linear is correct for blur/blend/scale; costs 2x memory bandwidth vs 8-bit; use Rgba16Float. Offer "gamma-space blending (CapCut-like)" toggle for look-parity with consumer tools [I: consumer editors typically blend in gamma space; unverified].
- HDR output to display: platform-specific (DXGI HDR10 swapchain, macOS EDR/CAMetalLayer). wgpu surface HDR support is limited [U]; defer. HDR *export* (HEVC 10-bit PQ/HLG) is feasible via FFmpeg.

### 3.4 Text rendering for animated captions [V versions]
| Lib | Version | Role | Fit |
|---|---|---|---|
| cosmic-text | 0.19.0 (2026-04) | shaping (rustybuzz/harfrust), layout, fallback, bidi, editing | Mature, used by iced/Bevy/COSMIC; good for layout + per-glyph positions |
| parley | 0.13.0 (2026-11-12) | Linebender rich text layout | Active, pairs with Vello/Xilem |
| swash | 0.2.10 (2026-07) | glyph scaling/outlines/color glyphs | rasteriser used by cosmic-text |
| glyphon | 0.12.0 (2026-07) | cosmic-text -> wgpu glyph atlas renderer | simple but atlas bitmaps => poor with large scale animation/outline |
| vello | 0.12.0 (2026-11-11) | GPU compute 2D vector renderer | Resolution-independent glyph paths, strokes (outlines), gradients; alpha-quality; 1.25M downloads |
| skia-safe | 0.157.0 (2026-11-03) | Skia bindings (Ganesh/Graphite) | Full-featured (strokes, shadows, blur, paragraph); heavy build/binary; Cap has a `rendering-skia` crate [V] |

[I] Recommendation: **cosmic-text (or parley) for shaping/layout -> glyph outlines via swash/skrifa -> own GPU path rendering: SDF/MSDF atlas for fill+outline+shadow at arbitrary scale, or Vello for vector glyph paths** (best quality for stroke outlines, per-glyph transform/colour/opacity animation since each glyph is an independent instance). Fast path v1: MSDF atlas glyph quads in wgpu with instance data (position, scale, rotation, colour, per-glyph time offset) - gives per-glyph animation cheaply. Evaluate Vello in a spike for stroke quality; skia-safe is the safe quality fallback but costs build complexity and binary size (not in browser-friendly). Caption styles (karaoke highlight, word pop) rely on per-word/per-glyph timing from the caption model, independent of renderer choice.
Emoji/colour fonts (COLR/CBDT) need handling in swash; complex scripts (Arabic/Indic) require proper shaping -> rule out naive atlas-by-codepoint approaches.

---

## 4. Audio engine [V versions]
| Lib | Version | Notes |
|---|---|---|
| cpal | 0.18.2 (2026-08), very active | device I/O (WASAPI, CoreAudio, ALSA, JACK, WebAudio/wasm AudioWorklet) - baseline |
| rodio | 0.23.0 (2026-10) | convenient playback; too simple for a NLE mixer |
| kira | 0.12.6 (2026-10) | game audio, clocks/tweens; good timing model but not tailored to arbitrary-clip NLE |
| fundsp | 0.23.0 (2026-01) | DSP graph; useful for effects (EQ, compressor) |
| dasp | 0.11.0 (**2020-05**) | stale; avoid |
| rubato | 5.1.0 (2026-11-12) | async/fft resamplers; active |
| ebur128 | 0.1.11 (2026-10) | EBU R128 loudness (LUFS, true peak) |
| signalsmith-stretch | 0.1.3 crate (2025-09); C++ upstream MIT [V license] | time-stretch + pitch shift, high quality, permissive |
| rubberband crate | 0.1.5 (2026-09); Rubber Band library is **GPL-2.0** upstream [V] (commercial licence available) | high quality but GPL: acceptable if Cutweave itself is GPL; **incompatible with permissive/MIT licensing of the app and with the browser/wasm distribution under a non-GPL licence**. Avoid unless Cutweave chooses GPL. |
| symphonia | 0.6.1 | decode |

Design [I]:
- Own a small mixer graph: per-track sources (decoded, resampled to device rate via rubato) -> gain/pan/EQ/compressor -> bus -> master -> limiter -> cpal callback. Real-time thread: no allocation/locks; lock-free SPSC ring buffers (e.g., `rtrb`) feeding from decoder threads; parameter updates via atomics/command queue. Same graph renders offline for export (faster than real time), guaranteeing preview == export.
- **A/V sync: audio is the master clock.** Preview player derives video presentation time from the audio device's played-sample counter (compensate for output latency reported by cpal); video frames are dropped/held to match. When no audio track, use monotonic system clock. Scrubbing: short audio grain ("scrub snippets") from decoder.
- Time-stretch for speed ramps: signalsmith-stretch (MIT). Pitch-preserve optional.
- Loudness: ebur128 for meters and "normalize to -14 LUFS" export presets.
- Waveform peaks: computed at import by the decoder into multi-resolution min/max pyramid (like audiowaveform's .dat), stored beside the project cache; UI reads LOD level by zoom.
- Browser: cpal supports wasm via WebAudio [U]; for multi-threaded audio use AudioWorklet + SharedArrayBuffer (needs cross-origin isolation).

---

## 5. Data model & persistence

### 5.1 Timeline model
- Own model in a `cutweave-model` crate (pure Rust, no I/O, wasm-friendly): Project -> Sequences -> Tracks -> Clips/Gaps/Transitions -> Effects/Keyframes; media references with rational time (`RationalTime` as in OTIO). Stable IDs (UUID v7 / ULID) on every entity.
- **OpenTimelineIO**: Academy Software Foundation project, 2.1k stars, pushed 2026-11-20 [V]; core is C++ with Python bindings; the crates.io `opentimelineio` crate is 0.1.0 from 2020 (unmaintained) [V]. -> Treat as an *interchange* format: implement OTIO JSON (de)serialization ourselves (it is documented JSON schema: Timeline/Stack/Track/Clip/Gap/Transition, ExternalReference, RationalTime/TimeRange) [U on exact schema versions]. Round-trip lossy for effects; store Cutweave-specific data in OTIO `metadata`. Also plan FCPXML / EDL / SRT/VTT import-export later.

### 5.2 Project file
- Recommend **a single-file SQLite project (`.cutweave`) OR a directory/zip with JSON + assets**. [I] Opinion: human-diffable JSON (or RON/MessagePack) for the document + sidecar cache dir; use a zip container (`.cutweave` = zip with `project.json`, `media-refs`, thumbnails) for portability with an "include media" option. SQLite gives atomic saves, crash recovery, and incremental autosave but is not git-diffable. Pick: **SQLite for media library/cache/analysis (rebuildable)**, **JSON-based document for the project** (diffable, mergeable, versioned with `schemaVersion` + migrations).
- rusqlite 0.43.0 (2026-11-15) [V] with `bundled` feature; WAL mode. Alternative `sqlx`/`diesel` are overkill. In browser: sqlite-wasm/OPFS [U].

### 5.3 Undo/redo and collaboration
- **Command pattern** with inverse ops/patches (or immutable snapshots with structural sharing, e.g. `im`/`rpds`) is the right default for a single-user local-first editor: simple, predictable, supports grouping (drag = one undo step), and works with a JSON model. Persist command log for crash recovery.
- CRDTs: Loro 1.18.0 (2026-11-16, active, has movable tree/list and built-in undo manager) [V]; Yrs 0.28.2 (2026-11-19) [V]; Automerge crate 0.12.0 (2026-09) / JS 3.5.0 [V]. They matter only if real-time collaboration or multi-device sync is a goal. Costs: document-size/memory overhead, model constrained to CRDT types, hard to express "time-ordered clips on track with ripple semantics" (ripple edits are *global* operations that CRDTs merge poorly [I]). Recommendation: **commands now; design IDs and ops so a Loro-backed model can be introduced later** (Loro movable list/tree match tracks/clips well [U]). Don't adopt CRDT in v1.

### 5.4 Media library cache, proxies
- SQLite tables: assets (path, hash [xxh3 of head+tail+size, or full blake3 on demand], probe JSON, streams, duration, timebase), keyframe index, thumbnails (sprite sheets), waveform peaks, proxies, analysis results. Key caches by content hash so moved files re-link.
- Proxy strategy: auto-generate on import for long-GOP / >1080p / HEVC / VFR sources: 540p-720p intra-ish (ProRes Proxy/ DNxHR LB if available, or H.264 all-intra / MJPEG for patent-free simplicity). [I] Intra-only proxy in `.mov`/`.mkv` makes scrubbing O(1) decode per frame. Render cache: store pre-rendered segments keyed by hash of (graph subtree, time range) for heavy effects; LRU with size cap; user-configurable cache directory.
- Relink and "media offline" handling; store both absolute and project-relative path.

---

## 6. Frontend

### 6.1 Framework [V versions]
| | Version | Note |
|---|---|---|
| React | 19.3.1 | largest ecosystem; hiring; shadcn/Radix; React Compiler available [U] |
| Svelte | 5.59.1 | runes; small bundles; fine-grained reactivity; smaller component ecosystem |
| SolidJS | solid-js 2.0.3 | fine-grained signals; Cap's Tauri UI was SolidJS [V: `packages/ui-solid`]; Solid 2.0 is a recent major [U on stability] |
| Zustand | 5.1.0 | simple store, selector subscriptions |
| Jotai | 3.0.2 | atomic state |
| PixiJS | 8.23.0 | WebGL/WebGPU 2D scene graph |
| Tailwind | 4.3.5 | v4 |
| Vite | 8.3.8 | |
| Vitest | 5.1.1 | |
| Biome | 2.6.8 | |
| Playwright | 1.65.1 | |
| mediabunny | 1.66.2 | |

[I] Recommendation: **React 19 + Zustand (for UI state) with the engine as source of truth for the document**. The heavy part (timeline canvas, preview) is not framework-bound; React's ecosystem (shadcn/ui, Radix primitives, dnd-kit, react-aria, hotkey libs, virtualised lists) and contributor pool dominate for an open-source project. Solid would win on raw update granularity but the timeline is going to be canvas anyway. Keep React render out of the hot path: playhead position, drag state, and meters live in refs/imperative canvas, not React state (`useSyncExternalStore` for rare updates).

### 6.2 State architecture
- Single source of truth: Rust model. UI holds a **read replica** (immutable snapshots or JSON patches pushed over a Tauri channel) and sends **commands** (`MoveClip{...}`) to Rust; Rust validates, applies, pushes patch + undo state. Optimistic local updates for drag, committed on drop. Keeps browser build parity (same command protocol over postMessage to a wasm worker).
- Type sharing: `specta`/`tauri-specta` or `ts-rs` to generate TS types from Rust [U - check maintenance].

### 6.3 Timeline rendering
- DOM breaks down past a few hundred clips with keyframes, thumbnails and waveforms (layout cost, hit testing). **Canvas2D with virtualisation (draw only the visible time window and visible tracks), retained scene model, spatial index (interval tree) for hit testing** handles thousands of clips at 60 fps. Move to PixiJS 8 (WebGL/WebGPU) if you need hundreds of thumbnails/waveforms composited and zoom animation; Canvas2D + OffscreenCanvas in a worker is another path. Use DOM only for chrome (headers, inspector, menus), plus a thin DOM overlay for the clip being edited (text input etc.). [I - consistent with how Figma/web NLEs do it; not benchmarked here.]
- Thumbnails: Rust generates per-asset sprite sheets/filmstrips at multiple LODs at import (e.g., one tile per N seconds depending on zoom), stored as JPEG/WebP files served via a **custom URI protocol** (`cutweave-thumb://asset/lod/index`) -> browser caches natively; draw with `drawImage` from an `ImageBitmap` cache. Waveforms: ship min/max peak pyramid as binary (Float32/Int8 typed array) via Tauri Channel/ custom protocol (`fetch` -> ArrayBuffer), draw as path/bars at current zoom. Avoid per-sample JSON arrays.
- Keyframe editor: separate canvas with curve editing; use a library or custom (bezier handles).

### 6.4 UI kit
- shadcn/ui + Radix (React) + Tailwind v4: fine for panels/inspector/dialogs/menus. Dense pro-tool look requires custom components (timeline, scrubby number inputs, curve editor, color wheels, dockable panels: use `dockview` or `rc-dock` [U], or own splitter layout). Radix is maintained under WorkOS; check status of radix-ui consolidated package [U].
- Accessibility: keep keyboard-first (JKL, I/O, etc.), focus management.

---

## 7. Browser build feasibility

### 7.1 Status [V partly]
- WebCodecs: global support ~95.9% (92.3% full + 3.7% partial); Chrome/Edge 94+, Firefox 130+ desktop, Safari full 26.0 (partial 16.4-18.7) [V caniuse]. Firefox Android/limited codec sets, and encoder codec support varies (AAC/HEVC encode patchy) [U].
- WebGPU: reported Baseline across all major browsers as of Jan 2026 (Chrome/Edge, Firefox 141+ on Windows then others, Safari 26) with Linux still catching up for Firefox [V via search summary of web.dev/third-party; U specifics].
- **mediabunny 1.66.2** (2026-11-19, 7.5k stars, very active): pure TS library for reading/writing/converting media files on WebCodecs (MP4, WebM, MOV, MKV, MP3, etc.), successor to the author's WebM/MP4 muxers [V repo; U feature detail]. A strong building block for browser demux/mux/export.
- File System Access API: Chromium only (showOpenFilePicker, OPFS also in Safari/Firefox). Firefox/Safari lack the picker -> fall back to `<input type=file>` and OPFS copy; limits "local-first reference-in-place" workflows [U].
- WASM threads/SIMD: threads need SharedArrayBuffer -> **cross-origin isolation (COOP/COEP)** headers; SIMD is universal; wasm64/memory > 4 GB not generally available [U]. Multi-GB video in memory is the real constraint: stream via File/Blob slices and WebCodecs, never load whole files.
- Prior art: OpenCut (MIT, browser/TS, 99k stars [V]), omniclip [U], Clipchamp (proprietary).

### 7.2 Can the Rust engine compile to wasm32?
- Yes for: `cutweave-model`, timeline evaluation, render graph, color/shader code, caption layout (cosmic-text builds on wasm [U]), audio DSP, wgpu itself (WebGPU backend; WebGL2 fallback) [V wgpu supports WebGPU backend; U wasm details].
- No/poor for: ffmpeg-next (native C library; ffmpeg.wasm exists but is slow/large and LGPL/GPL build size issues - single-thread builds ~25-30 MB [U]), cpal native backends (WebAudio backend exists), rusqlite (use sqlite-wasm), file I/O, hardware decode paths.
- Design implication [I]: define traits `Decoder`, `Encoder`, `Storage`, `AudioOut`, `Clock`; desktop = FFmpeg/hw/native; web = WebCodecs (via wasm-bindgen + web-sys or mediabunny on the JS side), OPFS, WebAudio. wgpu 30's `import_external_texture` for `VideoFrame` gives zero-copy WebCodecs -> wgpu on web [V changelog]. Realistic parity target: 70-80% of core crates shared; UI shared if React (not if GPUI).
- Choice of GPUI/egui as desktop UI would reduce web-sharing (egui does run on wasm via eframe; GPUI web support is unproven [U]). This is a real tension with plan B.

---

## 8. Tooling & distribution

### 8.1 Repo layout [I]
```
cutweave/
  Cargo.toml (workspace, resolver 3, [workspace.dependencies] to pin wgpu/ffmpeg)
  crates/
    cutweave-model/      (timeline, commands, undo; wasm-safe)
    cutweave-timecode/   (rational time)
    cutweave-render/     (wgpu render graph, WGSL, color, text)
    cutweave-audio/      (mixer, dsp, clock)
    cutweave-media/      (traits + ffmpeg backend + hw interop modules per OS)
    cutweave-cache/      (sqlite, proxies, thumbs, peaks)
    cutweave-io/         (OTIO/EDL/FCPXML import-export)
    cutweave-engine/     (orchestration, playback, export; UI-agnostic command/event API)
    cutweave-cli/        (headless export/test harness)
  apps/
    desktop-tauri/    (src-tauri + web UI)  [or desktop-gpui]
    web/              (browser build)
  packages/ (pnpm workspace) ui, timeline-canvas, protocol (generated types)
  shaders/ (wgsl)
```
Set `opt-level = 2/3` for hot crates in dev profile (Cap measured 23x slowdown in unoptimised pixel loops [V]); use `cargo-nextest` [U version], `cargo-deny` (licence gate for GPL/LGPL), `cargo-about`, `cargo-hack`, `sccache`.

### 8.2 JS tooling [V versions in 6.1]
pnpm workspaces + Vite 8 + Biome 2 + Vitest 5 + Playwright 1.65 (Playwright against Tauri: use `tauri-driver` [crate in tauri repo V] for WebDriver on Windows/Linux; no macOS WKWebView driver support [U] -> E2E on macOS via Playwright against the Vite dev server only; native preview verified by golden-image tests in Rust headless).
- Golden-frame tests: render graph headless on llvmpipe/WARP/Metal in CI, compare to reference PNGs with tolerance (insta + image-compare).

### 8.3 CI matrix
GitHub Actions: windows-latest, macos-latest (arm64) + macos-13/15-intel [U], ubuntu-24.04 (+ ubuntu-24.04-arm [U]). Use `tauri-apps/tauri-action` v1 [V] to build installers/draft releases. Linux needs libwebkit2gtk-4.1-dev, libayatana-appindicator, etc. [U]. GPU tests: no real GPUs on hosted runners -> use software Vulkan (lavapipe)/WARP; self-hosted GPU runners for hw-decode tests later.

### 8.4 Packaging, signing, updates (Tauri) [U unless noted]
- Bundles: MSI/NSIS (Windows), .dmg/.app (macOS), .deb/.rpm/AppImage (Linux); Flatpak/Snap via community manifests. Cap reuses the Tauri bundler (`tauri bundle`) even without Tauri runtime [V] - tauri-bundler is separable.
- Signing: Windows Authenticode (OV/EV cert or Azure Trusted Signing), macOS Developer ID + notarization (Apple developer account $99/yr), Linux optional. `tauri-macos-sign` crate exists in the repo [V].
- Updater: `tauri-plugin-updater` with its own Ed25519 update signatures + static JSON/GitHub releases endpoint.
- **Shipping FFmpeg**:
  - Dynamic linking of an **LGPL build** is the legally cleanest (LGPLv2.1+ allows closed/any-licence app linking if users can replace the library; ship as shared DLL/dylib/so, include notices + source offer). Static linking LGPL is possible but then you must provide object files / relinking ability; avoid.
  - **GPL build** (libx264, libx265, etc.) forces the *combined work* to GPL; fine if Cutweave is GPL-licensed (Kdenlive/Shotcut/OpenShot are GPL-3 [V for Kdenlive/Shotcut]). The user said "fully open-source" so GPL-3 is available - decide licence early; it widens options (x264, x265, rubberband, fdk is "nonfree" and un-redistributable).
  - BtbN/FFmpeg-Builds publishes win64/linux64 (+arm64) builds in variants `gpl`, `lgpl`, `nonfree`, each `-shared` or static [V README]; daily autobuilds, latest tag 2026-11-22 [V]. macOS: build own (Homebrew bottles not redistributable-friendly) or use evermeet/ martin-riedl builds [U]; best: own reproducible build script via CI producing a pinned set of artifacts we host.
  - Patents: H.264/HEVC/AAC are patent-encumbered in some jurisdictions; open-source projects typically distribute software and leave patent licensing to the user/distributor (Shotcut, Kdenlive, OpenShot ship FFmpeg with x264 etc. [U]). Hardware encoders (NVENC/VideoToolbox/etc.) use the OS/vendor-licensed path; OpenH264 binary from Cisco carries Cisco's licence coverage only if downloaded from Cisco's binaries unmodified [U]. HEVC has multiple pools (MPEG LA/Access Advance/Velos) and is the riskiest [U]. Mitigation: default export presets = AV1 (SVT-AV1/libaom) / VP9 / H.264 via hw; clearly document; legal review before public release; never claim patent grants.
  - Prefer AV1/VP9/Opus/ProRes-decode in LGPL build; keep a separate optional "codec pack" download for GPL/x264/x265 to keep the base app LGPL-clean [I].

---

## 9. Recommended stack

| Component | Choice | Version (2026-11-22) | Why |
|---|---|---|---|
| Engine language | Rust (stable) | latest stable | perf, wasm-able model, ecosystem |
| GPU | wgpu | 30.0.2 (pin) | zero-copy interop hooks (dmabuf, DX12 plane slice), WebGPU/native |
| Shaders | WGSL (naga) | with wgpu | shared desktop/web |
| App shell | **Tauri 2 (gated by Spike 1/2); plan B GPUI** | tauri 2.12.7 / wry 0.57.3 | small, mature, bundler/updater |
| Preview surface | native wgpu child surface (+ canvas/JPEG fallback) | | avoids IPC bottleneck |
| UI | React 19 | 19.3.1 | ecosystem, contributors |
| State (UI) | Zustand + Rust as source of truth | 5.1.0 | simple |
| Timeline | Canvas2D virtualised (PixiJS 8.23 if needed) | | thousands of clips |
| UI kit | shadcn/ui + Radix + Tailwind | Tailwind 4.3.5 | |
| Build/test (JS) | pnpm, Vite 8.3, Biome 2.6, Vitest 5.1, Playwright 1.65 | | |
| Media I/O | ffmpeg-next / ffmpeg-sys-next behind `MediaBackend` trait | 9.1.0 / 9.1.1 (target FFmpeg 8.1.x or 9.x shared LGPL) | alive again, broadest codec/hw coverage |
| HW interop | per-OS modules (D3D11/12 shared handle; IOSurface/Metal; VA-API dmabuf->Vulkan); initial CPU-upload fallback | | |
| Audio I/O | cpal | 0.18.2 | |
| Audio mix | custom graph (+ fundsp for FX) | fundsp 0.23 | NLE needs |
| Resample | rubato | 5.1.0 | |
| Loudness | ebur128 | 0.1.11 | |
| Time-stretch | signalsmith-stretch (MIT) | 0.1.3 | avoid GPL rubberband unless GPL app |
| Text | cosmic-text 0.19 (or parley 0.13) + swash 0.2.10 + MSDF/Vello renderer | | per-glyph GPU animation |
| Persistence | JSON document + rusqlite cache | rusqlite 0.43.0 | diffable + robust |
| Undo | command pattern; design for later Loro | loro 1.18.0 (future) | |
| Interchange | OTIO JSON own impl, FCPXML/EDL later | | no maintained Rust OTIO crate |
| Browser | WebCodecs + WebGPU + mediabunny, wasm core | mediabunny 1.66.2 | |
| CI | GitHub Actions + tauri-action v1, nextest, cargo-deny | | |
| FFmpeg distribution | BtbN `lgpl-shared` (Win/Linux), own build (macOS); optional GPL codec pack | | licence cleanliness |

## 10. Alternatives considered (summary)
- Shell: Electron (consistency, size cost), Tauri+CEF (experimental), GPUI (proven by Cap, Rust-only UI, pre-1.0), Slint (licence), egui/Iced (UI quality), Dioxus/Xilem (immaturity).
- Media: GStreamer (pipeline/dist weight, scrub), rsmpeg (stale), ffmpeg-the-third (alive fork), libmpv (playback only), MLT (C, GPL; used by Kdenlive/Shotcut - a viable engine if Cutweave wanted to avoid writing the compositor; sacrifices Rust/wgpu design [I]).
- Text: skia-safe, Vello, glyphon.
- State/undo: CRDT (Loro/Yrs/Automerge).
- Frontend: Svelte 5, Solid 2.

## 11. Risk register
| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Preview pipeline in Tauri (native surface + webview overlay) unworkable on Linux/Wayland or flaky z-order/resizing across OSes | High | High | Spike 1 early; keep engine UI-agnostic; plan B GPUI or Electron; ship X11/XWayland on Linux initially |
| R2 | WebKitGTK webview perf/feature gaps (no WebGPU, WebCodecs uncertain, slow timeline canvas) | High | Med-High | No webview dependency for preview; canvas2D timeline; benchmark Spike 2; optionally CEF later |
| R3 | Zero-copy HW decode->wgpu per-platform interop complexity & driver bugs | Med-High | Med | Ship CPU-upload path first; interop behind trait + feature flags; automated per-vendor tests; software fallback always |
| R4 | wgpu major-version churn and GPU device-lost/driver crashes (AMD iGPU, NVIDIA Linux per Cap issues) | Med | Med-High | Pin and upgrade quarterly; device-lost recovery; lavapipe CI; crash reporting |
| R5 | FFmpeg licensing/patent exposure (GPL, HEVC/AAC) when shipping | Med | High | Decide project licence early; LGPL shared build; optional codec pack; cargo-deny; legal review; HW encoders |
| R6 | FFmpeg major changes (9.x new) break ffmpeg-next / distribution lag; single maintainer bus-factor | Med | Med | Pin FFmpeg version; `MediaBackend` trait; vendor fork option (ffmpeg-the-third as fallback) |
| R7 | Frame-accurate seek/VFR/long-GOP scrub performance | High | High | Index at import; auto proxies; decoder pool & GOP cache; test corpus (VFR phone footage, HEVC 10-bit, ProRes) |
| R8 | Color-management mistakes (matrix/range/transfer, HDR) produce visibly wrong output | Med | High | Explicit colourspace tags in model; reference test charts; golden images; defer HDR display |
| R9 | Browser build diverges from desktop (codecs, memory, COOP/COEP, FSA API gaps) | High | Med | Backend traits; scope browser to lite editor; share only model/render/shaders; don't block desktop on it |
| R10 | Text rendering quality/perf for animated captions (shaping, emoji, outlines) | Med | Med | MSDF + cosmic-text first; Vello/Skia spike; shaping test suite incl. RTL/CJK/emoji |
| R11 | Audio sync drift / glitches (device latency, underruns, Bluetooth) | Med | Med-High | Audio master clock; latency compensation; lock-free RT design; soak tests |
| R12 | Project reach: UI shell switch mid-project (Cap precedent) | Med | High | Command/event protocol boundary; no UI types in engine; shell decision by Spike result |
| R13 | Unverified claims in this report (see [U]) | Med | Low-Med | Treat as hypotheses; spikes validate |
| R14 | Dependency health: dasp stale, rsmpeg stale, xilem/iced slower cadence | Low | Low | avoid those |

## 12. Spikes (throwaway prototypes)

**Spike 1 - Native wgpu preview beside a Tauri/React UI (the crux).**
Build: Tauri 2 app, transparent webview over/beside a wgpu child surface playing a decoded 1080p60 clip with a DOM overlay (draggable box, playhead scrub bar) and live window resize.
Pass: on Windows 11, macOS (Apple Silicon), Ubuntu 24.04 (X11 *and* Wayland/XWayland) : sustained 60 fps, < 2 dropped frames/min, scrub-to-pixel latency < 80 ms, no flicker/tearing on resize, overlay interaction works, CPU < 25%. Fail: any OS requiring a hack with no workaround, or Wayland unsupported with no XWayland option the team accepts. Failure triggers Spike 1b: same demo in GPUI (and optionally Electron+native addon) and compare.

**Spike 2 - Webview capability and IPC throughput matrix.**
Run on WebView2, WKWebView, WebKitGTK (Ubuntu 24.04/Fedora latest): feature-detect WebGPU, WebCodecs (H.264/AV1 decode/encode), SharedArrayBuffer, OffscreenCanvas; measure Tauri channel/custom-protocol throughput for 1 MB / 8 MB payloads, and Canvas2D timeline stress (5,000 clips + 50k keyframes + 200 thumbnails).
Pass: custom protocol >= 300 MB/s sustained (round-trip to ArrayBuffer), timeline pans/zooms >= 55 fps on all three webviews including WebKitGTK, waveform/thumb requests < 5 ms from cache. Record exact feature matrix (resolves the [U] items above). Fail: WebKitGTK timeline < 30 fps -> evaluate CEF runtime or GPUI.

**Spike 3 - Decode path and zero-copy interop.**
ffmpeg-next 9.1: open H.264 4K30, HEVC 10-bit, ProRes, VFR phone clip; build keyframe index; frame-exact seek test (decode frame N by pts, compare against ffprobe truth for 500 random seeks); hardware decode (D3D11VA/NVDEC on Windows, VideoToolbox on mac, VA-API on Linux) into wgpu via (a) CPU upload NV12 and (b) zero-copy import (DX12 shared handle with `with_plane_slice`; IOSurface/Metal; dmabuf->Vulkan `texture_from_dmabuf_fd`).
Pass: 100% frame-exact seeks; scrub across 4K H.264 long-GOP: p95 < 100 ms per seek (proxy path p95 < 33 ms); (a) 4K30 sustained with < 15% CPU delta over (b); at least 2 of 3 platforms achieve (b) with no copies; fallback works everywhere. Fail on (b): ship (a) and defer interop.

**Spike 4 - GPU compositor + color + captions.**
Render graph with 3 video layers, 2 blend modes, a cross-dissolve transition, LUT, linear-light Rgba16Float, BT.709 and PQ/HLG source tone-mapped to SDR; animated caption (outline + shadow + per-word pop + karaoke highlight) using cosmic-text + MSDF and a Vello variant; headless export to H.264 through FFmpeg, compared with reference frames.
Pass: 1080p60 real-time preview at < 8 ms GPU frame time on an integrated GPU (Intel Xe/Apple M1); output matches reference (PSNR > 45 dB vs ffmpeg-filter-based reference for simple cases; ΔE < 2 on colour chart); caption quality acceptable at 4K without shimmering and 1000 animated glyphs < 1 ms; same WGSL runs in a browser WebGPU page. Fail: Vello/MSDF both unacceptable -> evaluate skia-safe.

**Spike 5 - Audio master clock and A/V sync + browser feasibility.**
(a) cpal mixer (8 tracks, rubato resample, signalsmith-stretch at 0.5x-2x, ebur128 meter) driving video presentation clock; 30-minute soak.
(b) Browser proof: compile `cutweave-model` + render graph to wasm32; decode with WebCodecs/mediabunny; composite with wgpu WebGPU backend using `import_external_texture`; export a 10 s MP4 via mediabunny.
Pass (a): drift < 1 frame over 30 min, zero underruns at 128-sample buffer on Win/mac/Linux, A/V offset < 40 ms measured with a click/flash test. Pass (b): shared crates build for wasm32 with no `cfg` forks in `cutweave-model`/`cutweave-render`, 1080p30 preview >= 24 fps in Chrome and Safari 26, export plays correctly. Fail (b): re-scope browser build as a separate lite product.

---

## Sources
Verified today (gh CLI / crates.io API / npm registry / fetched pages):
- https://github.com/tauri-apps/tauri , https://crates.io/crates/tauri , https://github.com/tauri-apps/wry , https://github.com/tauri-apps/tauri/issues/14963 (WebKitGTK performance complaint / bundle Chromium request)
- https://github.com/tauri-apps/cef-rs
- https://github.com/tauri-apps/tauri-action
- https://v2.tauri.app/concept/inter-process-communication/
- https://v2.tauri.app/reference/webview-versions/
- https://github.com/tauri-apps/wry/blob/dev/examples/wgpu.rs (wgpu + child webview example; X11-only on Linux)
- https://github.com/oscartbeaumont/tauri-wgpu-tag (prototype and TODO list)
- https://github.com/michele-xilo/tauri-wgpu-cam
- https://github.com/simplearyan/kenichi-preview
- https://github.com/CapSoftware/Cap ; https://github.com/CapSoftware/Cap/tree/main/apps/desktop-gpui (README: Tauri replaced by GPUI) ; https://github.com/CapSoftware/Cap/blob/main/crates/editor/PLAYBACK-FINDINGS.md ; issues #1954, #3946
- https://github.com/zed-industries/zed (gpui crate 0.2.2) ; https://crates.io/crates/gpui
- https://github.com/gfx-rs/wgpu (CHANGELOG v30.0.0-30.0.2) ; https://crates.io/crates/wgpu
- https://github.com/zmwangx/rust-ffmpeg ; https://crates.io/crates/ffmpeg-next ; https://crates.io/crates/ffmpeg-sys-next ; https://crates.io/crates/ffmpeg-the-third ; https://github.com/larksuite/rsmpeg ; https://crates.io/crates/rsmpeg
- https://ffmpeg.org/download.html (release list: 9.1, 9.0.2, 8.1.3, 8.0.3, 7.1.5)
- https://phoronix.com/news/ffmpeg-8.1-released (8.1 Vulkan/D3D12 features, 2026-03-16)
- https://github.com/BtbN/FFmpeg-Builds (variants gpl/lgpl/nonfree, shared/static)
- https://github.com/gstreamer/gstreamer-rs ; https://crates.io/crates/gstreamer
- https://github.com/pdeljanov/Symphonia ; https://crates.io/crates/re_mp4 ; https://crates.io/crates/openh264 ; https://crates.io/crates/vk-video
- https://github.com/RustAudio/cpal ; https://crates.io/crates/rodio ; https://crates.io/crates/kira ; https://crates.io/crates/fundsp ; https://crates.io/crates/dasp ; https://github.com/HEnquist/rubato ; https://crates.io/crates/ebur128 ; https://github.com/Signalsmith-Audio/signalsmith-stretch (MIT) ; https://github.com/breakfastquay/rubberband (GPL-2.0) ; https://crates.io/crates/rubberband
- https://github.com/pop-os/cosmic-text ; https://crates.io/crates/parley ; https://crates.io/crates/swash ; https://github.com/grovesNL/glyphon ; https://github.com/linebender/vello ; https://crates.io/crates/skia-safe
- https://github.com/AcademySoftwareFoundation/OpenTimelineIO ; https://crates.io/crates/opentimelineio (0.1.0, 2020)
- https://github.com/loro-dev/loro ; https://crates.io/crates/yrs ; https://github.com/automerge/automerge ; https://crates.io/crates/rusqlite
- https://github.com/emilk/egui ; https://github.com/slint-ui/slint ; https://github.com/iced-rs/iced ; https://github.com/DioxusLabs/dioxus ; https://github.com/linebender/xilem
- npm registry (react 19.3.1, svelte 5.59.1, solid-js 2.0.3, zustand 5.1.0, jotai 3.0.2, pixi.js 8.23.0, tailwindcss 4.3.5, vite 8.3.8, vitest 5.1.1, @biomejs/biome 2.6.8, @playwright/test 1.65.1, mediabunny 1.66.2)
- https://github.com/Vanilagy/mediabunny ; https://github.com/OpenCut-app/OpenCut ; https://github.com/KDE/kdenlive ; https://github.com/mltframework/shotcut
- https://caniuse.com/webcodecs
- https://webkitgtk.org/2025/11/26/webkitgtk-2.50.html (no WebGPU/WebCodecs mention)
- https://blogs.igalia.com/webkit/blog/2026/wip-58/ ; https://bugs.webkit.org/show_bug.cgi?id=257694 ; https://web.dev/blog/webgpu-supported-major-browsers (via search snippets only; treat as lightly verified)
- https://blog-ximei-me.pages.dev/blog/en/2025/1008-tauri-issue (blog, opinion on WebKitGTK/CEF)

Not verified (no source fetched): per-OS signing details, Tauri updater specifics, FFmpeg LGPL linking obligations (read the licence text/lawyer), HEVC patent pool status, WebView2 shared buffer API, Slint licence terms, OpenH264 licence terms, nextest/tauri-specta versions, macOS CI runner labels, WebGPU on Firefox Linux timing, WebKitGTK WebCodecs actual status.
