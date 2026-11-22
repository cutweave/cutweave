# 02 - OSS Video Editor Landscape and Reusable Building Blocks

Date of research: 2026-11-22. Stats pulled with `gh api` on that date unless marked. Items tagged **[U]** are unverified (from memory or secondary sources). Star counts for 2026-created repos look unusually high; treat them as indicative only.

## Headline correction

OpenCut is **not simply abandoned**. The original web codebase was archived (moved to `opencut-app/opencut-classic`, 2026-05-16, "archived and no longer maintained"). The main repo `OpenCut-app/OpenCut` was **reset for a from-scratch Rust-core rewrite** (README: "rewritten from the ground up"; plugin-first architecture, Editor API, MCP server, headless mode, desktop via GPUI, web via WASM). Last push 2026-09-24 (FFmpeg build workflow). The rewrite takes **no outside contributions** and has no usable release. So: classic = dead; rewrite = alive but pre-product, ~2 months since last commit as of today. The user's "no longer maintained" is true for what anyone can use today.

## Part A - Editors

### A1. Established / classic OSS editors

| Project | Stack | License | Stars | Last push / release | Status | Platforms |
|---|---|---|---|---|---|---|
| Kdenlive | C++/Qt/QML, MLT | GPL-3.0 | 6.2k | push 2026-11-22; tag v26.11.80 (beta) | Very active (KDE) | Win/Mac/Linux |
| Shotcut | C++/Qt/QML, MLT, FFmpeg | GPL-3.0 | 15.8k | push 2026-11-21; v26.11.15 (2026-11-16) | Active, effectively one lead (Dan Dennedy) **[U on bus factor]** | Win/Mac/Linux |
| OpenShot / libopenshot | Python+Qt UI; C++ libopenshot (FFmpeg, JUCE, Qt) | GPL-3.0 / LGPL-3.0 (libopenshot) | 6.8k UI / 1.6k lib | push 2026-11-22; v4.1.1 (2026-11-02) | Active | Win/Mac/Linux |
| Flowblade | Python/GTK, MLT | GPL-3.0 | 3.1k | push 2026-11-17; v2.24.2 (2026-05-29) | Active, single developer (Janne Liljeblad) | Linux (Win/Mac unofficial) |
| Pitivi | Python/GTK, GStreamer GES | LGPL-2.1 **[U]** (GitHub shows NOASSERTION) | 240 (GitHub mirror) | push 2026-10-08; no GitHub releases | Low-activity maintenance (small commits) | Linux |
| Blender (VSE) | C/C++/Python | GPL-3.0+ | 22k | push 2026-11-21 | Very active, but VSE is a side feature | Win/Mac/Linux |
| Cinelerra-GG | C/C++, custom engine | GPL-2.0 **[U]** | 18 (stale GitHub mirror; real dev at cinelerra-gg.org) | GitHub mirror last push 2021-12-23 | Alive off-GitHub **[U]** | Linux, Win/Mac builds |
| LosslessCut | Electron/TS, FFmpeg CLI | GPL-2.0 | 46k | push 2026-11-14; v3.70.2 (2026-10-28) | Active, single maintainer (mifi) | Win/Mac/Linux |
| Natron | C++/Qt, OpenFX (compositor) | GPL-2.0 | 5.8k | push 2026-11-22; last release v2.5.0 (2022-11-26) | Slow community maintenance; not an NLE | Win/Mac/Linux |
| Olive | C++/Qt, OpenColorIO, OIIO, OTIO | GPL-3.0 | 9.2k | last push 2024-12-05; 0.2.0-nightly 2024-12-05; 0.1.2 was 2019 | **Stalled** | Win/Mac/Linux |

### A2. Web / JS-ecosystem projects

| Project | Stack | License | Stars | Last push / release | Status |
|---|---|---|---|---|---|
| OpenCut (rewrite) | Rust core, GPUI desktop, WASM web (moon/bun) | MIT | 99k | 2026-09-24; FFmpeg-build release same day | Pre-product rewrite, closed to contributors |
| opencut-classic | Next.js/TS, early Rust/WASM compositor | MIT | 301 | 2026-05-17 | Archived |
| Omniclip | TS, WebCodecs **[U on UI libs]** | MIT | 1.5k | push 2026-11-15; no GitHub releases | Active-ish, one maintainer **[U]** |
| Diffusion Studio core | TS, WebCodecs **[U on renderer]** | MPL-2.0 | 1.2k | push 2025-11-18; v1.1.1 (2024-10-29) | Dormant ~12 months (library, not an editor) |
| Remotion | React/TS, Chromium+FFmpeg render | **Source-available "Remotion License", NOT OSI**; company license needed above a small-team threshold **[U on thresholds]** | 74.9k | push 2026-11-20 | Very active (commercial) |
| Motion Canvas | TS, Canvas, generator-based animation | MIT | 20.1k | push 2026-07-02 | Slowing (~4.5 months quiet) |
| Revideo | TS fork of Motion Canvas, headless/API render | MIT | 4.2k | push 2026-07-15 | Slowing |
| Twick | TS/React SDK + timeline, Remotion-like | NOASSERTION (check LICENSE) | 550 | push 2026-06-04; v0.15.31 (2026-03-18) | Slow |

### A3. New 2025-2026 projects (found via GitHub search)

| Project | Stack | License | Stars | Pushed | Notes |
|---|---|---|---|---|---|
| FilmCraft (storytold/ArtCraft) | "Pure Rust" Cargo workspace: own h264/hevc/vp9/prores/aac and container crates, egui UI, wgpu, MCP automation, WASM build | MIT OR Apache-2.0 | 18.8k (created 2026-09-30; 2.4k open issues) | 2026-11-22; v0.20.1 | Alpha; self-reported "~56% ready for real work"; speed on big footage is top gap. Most ambitious Rust NLE. Own codecs = huge surface |
| OpenReel Video | React/TS, WebCodecs, WebGPU, browser-only | MIT | 6.0k | 2026-11-20; v1.0.0 | Feature-rich CapCut-like (karaoke captions, keyframes, LUTs, ducking, noise reduction). Chrome/Edge only |
| OpenMotion (nexu-io) | TS, Node 22; AI "reference video to editable motion graphics" | Apache-2.0 | 15k | 2026-11-22 | AI-first motion/template tool, not a full NLE **[U on depth]** |
| Clypra | Rust + Tauri v2 + React 19, FFmpeg, HW decode (VideoToolbox/D3D11VA/VAAPI), mobile via Capacitor | MIT | 3.4k | 2026-11-22 | Closest to the architecture Cutweave would pick; ships production telemetry (red flag for local-first) |
| Nuzky | Native Rust engine | GPL-3.0 | 27 | 2026-11-20; v0.1.13-alpha | Best product thinking: CapCut-style magnetic track, text-based cutting via local Whisper, 9:16 first, platform export presets with LUFS, MCP agent, imports CapCut drafts/OTIO. Tiny, alpha |
| FreeCut | TS, WebGPU/WebCodecs/OPFS, plain-file workspace folder | MIT | 2.3k | 2026-10-29 | Browser NLE, local files, semantic scene search |
| WeftCut | TS desktop app, MCP server exposing all editing tools | MIT | 385 | 2026-11-22 | Agent-drivable NLE |
| Others seen | Gausian (Rust, 930), OpenTake (Rust GPL fork), BananaV/Freya-NLE/klipp (Rust, <20 stars), VeloCut, flycut (WebCodecs), capcraft-android (Rust+Kotlin, AGPL) | various | small | 2026 | Mostly hobby/alpha; confirms Rust + wgpu + FFmpeg is the convergent stack |

### A4. Per-project analysis

| Project | Architecture notes | Strengths | Weaknesses | Why not CapCut-class |
|---|---|---|---|---|
| **OpenCut (classic)** | Next.js web app, canvas + FFmpeg.wasm export **[U]**; later started Rust/WASM GPU compositor | Brand + 99k stars, MIT, simple UX, big contributor interest (~1000 commits by lead, 392 open issues, ~97 open PRs) | Browser export slow/memory-bound **[U]**; logic in TS then migrating to Rust | Went viral before depth; lead decided the architecture was wrong and restarted. Little captions/templates/effects depth before archive |
| **OpenCut (rewrite)** | Rust core; GPUI desktop; WASM web; plugin-first; headless; MCP | Right instincts (one core, agents, plugins) | Rewrite risk (Olive story); closed to contributors; GPUI is young; ~2-month silence | Pre-product |
| **Kdenlive** | MLT multitrack, Qt/QML UI, proxy clips, bin | Most complete OSS NLE; proxies, keyframes, titler, subtitles, speech-to-text **[U on exact engines]** | Stability reputation, dense pro UI, Windows/Mac historically second-class, MLT CPU-centric | Pro-editor UX, not template/effect/caption/sticker-driven flow |
| **Shotcut** | MLT, Qt Quick, FFmpeg, Movit GPU filters | Stable, wide format support, regular releases | Non-standard UX (filters-per-clip), no template/asset ecosystem, thin text animation | Utility-style editor; no creator workflow |
| **OpenShot** | libopenshot (C++), own timeline, Python UI | Easiest classic OSS UX | Crashes and slow with many tracks/long timelines **[U]**, limited effects, mixed licensing | Simple but shallow |
| **Olive** | 0.2 rewrite: node-based, OpenColorIO, OTIO, GPU render graph | Great architecture goals | One lead dev; two full rewrites (0.1 to 0.2); alpha since 2019; last commit 2024-12-05 | Never shipped; scope = Nuke + Premiere |
| **Flowblade** | MLT, Python/GTK | Fast insert/overwrite workflow | Linux focus, one dev, dated UI | Niche |
| **Pitivi** | GStreamer GES, GTK | Clean design | Tiny team; Linux only | Feature-stalled |
| **Blender VSE** | Blender sequencer | Scriptable, huge community | Blender-ism UX, weak text/captions | Not an editor-first product |
| **Cinelerra-GG** | Legacy C/C++ engine | Heavy-duty features | Very dated UI, build-from-source culture | Legacy design |
| **LosslessCut** | Electron + FFmpeg CLI, stream copy | Instant lossless trims | Not an editor (no compositing) | Different category |
| **Natron** | Node compositor, OpenFX | Compositing | Not an NLE; release gap 2022-2026 | Different category |
| **Omniclip** | Browser WebCodecs editor | Early WebCodecs pioneer | One maintainer; no release cadence | Early stage |
| **Diffusion Studio core** | WebCodecs library, MPL | Clean programmatic API | Dormant ~12 months; company direction unclear **[U]** | Library only |
| **Remotion** | React components rendered via headless Chromium | Best programmatic video; huge community | **Not open source**; code-first; heavy render; no timeline editor | Video-as-code framework |
| **Motion Canvas / Revideo** | Generator-based TS scenes, Canvas | Great for animated explainers; Revideo adds headless API | Code-first, no GUI NLE | Not for end-user editing |
| **Twick** | React timeline SDK + Remotion-like render | Embeddable timeline UI | License unclear, young | SDK |

## Part B - Reusable building blocks

| Component | License | Maturity | Fit for CapCut-style editor | Risks |
|---|---|---|---|---|
| **MLT** (mltframework/mlt) | LGPL-2.1 core, some GPL modules; v7.42.0 (2026-10-03) | Very mature, powers Kdenlive/Shotcut/Flowblade | Quick CPU multitrack engine and XML projects | CPU-centric frame-server model; GPL via some modules/Movit; weak for GPU/WebCodecs; C API; poor Rust bindings **[U]** |
| **GStreamer + GES** | LGPL-2.1 **[U]** | Mature; GES slow-moving (Pitivi) | Pipeline plumbing, HW decode plugins | Complex/async, hard frame-accurate scrub; Windows/macOS plugin packaging |
| **OpenTimelineIO** | Apache-2.0; repo active 2026-11-20 | Industry standard (ASWF) | Use for **interchange** only (Nuzky does) | Does not model effects/text/keyframes richly |
| **FFmpeg / libav*** | LGPL-2.1+ (GPL if x264/x265 enabled) | Gold standard | Mandatory desktop decode/encode; HW accel (NVDEC, VideoToolbox, D3D11VA, VAAPI, QSV) | Codec patents; GPL flags; LGPL dynamic-link compliance; per-platform builds (OpenCut is building its own) |
| **Mediabunny** | MPL-2.0; v1.66.2 (2026-11-19) | Very active, 7.5k stars, pure TS | Best browser demux/mux/transcode on WebCodecs (successor to mp4-muxer/webm-muxer) | MPL file-level copyleft (fine); browser codec gaps (e.g. HEVC) |
| **web-demuxer** (bilibili) | MIT; push 2025-12-20; 203 stars | Small, WASM FFmpeg demux | Formats WebCodecs/Mediabunny lack | Bus factor |
| **ffmpeg-next / rust-ffmpeg** | WTFPL (wraps LGPL/GPL FFmpeg); push 2026-11-16 | Mature, may lag new FFmpeg APIs **[U]** | Standard Rust FFmpeg binding | Unsafe edges, ergonomics |
| **rsmpeg** | MIT; last push 2025-08-24 | Thin safe bindings | More modern style | Maintenance slowdown |
| **gstreamer-rs** | MIT/Apache **[U]**; push 2026-11-16 | Excellent | Rust path to GStreamer | Inherits GStreamer complexity |
| **Symphonia** | MPL-2.0; v0.6.1 (2026-08-13) | Pure-Rust audio demux/decode | Waveforms/audio scrub without FFmpeg | Audio only |
| **re_video (rerun)** | Apache-2.0/MIT (rerun); crate path not verified **[U]** | Production Rust video decode with HW paths | Study/borrow for frame-accurate seek and GOP handling | Inside a monorepo; not standalone |
| **wgpu** | MIT/Apache-2.0 **[U]** | Mature | GPU compositor/effects on all desktops + WebGPU | Per-platform zero-copy video texture import |
| **whisper.cpp / whisper-rs** | MIT **[U]** | Mature | Local captions (Nuzky, OpenReel use local STT) | Model distribution/size |
| **Rust NLE engines** | FilmCraft MIT/Apache; Clypra MIT | Alpha | Reference only | Unstable; own-codec path is a trap |

## Part C - Lessons learned

| # | Failure mode | Evidence | What Cutweave should do |
|---|---|---|---|
| 1 | **Rewrite-itis** resets progress | Olive 0.1 to 0.2 (alpha since 2019, last commit 2024-12); OpenCut web to Rust rewrite, closed to contributors | Spike and choose architecture once; ship a usable vertical slice monthly; never freeze for a rewrite |
| 2 | **Scope creep to Premiere/Nuke class** | Olive node compositing; Natron; Blender | Target CapCut's 80%: trim/split, text, captions, stickers, templates, speed, keyframes, export presets. No node graph in v1 |
| 3 | **Single maintainer / bus factor** | Flowblade, LosslessCut, Omniclip, Olive | Governance, CI, plugin API, good-first-issues from day one |
| 4 | **Engine not built for GPU and long timelines** | MLT CPU path; FFmpeg.wasm export | wgpu compositor, proxies/cache by default, background render, long-timeline benchmarks in CI |
| 5 | **No hardware decode/encode** | Browser FFmpeg.wasm; older classic editors | WebCodecs (web) and native HW decode (VideoToolbox/D3D11VA/VAAPI/NVDEC) behind one abstraction |
| 6 | **Crashes and data loss** | Olive "highly unstable"; classic editors' reputations | Journaled autosave, crash-safe undo log, non-destructive (Nuzky never touches originals), fuzz demuxers |
| 7 | **Weak text, captions, templates** | CapCut's moat is captions + templates + effects | Local Whisper word-level captions with karaoke styles, text-based cutting, animated text presets, shareable open template format |
| 8 | **Pro-editor UX, not creator UX** | Kdenlive, Cinelerra, Blender | 9:16 default, magnetic main track, one-click presets, simple inspector |
| 9 | **Viral stars without product** | OpenCut classic ~99k stars, no usable editor | Ship usable alpha with real export early; test with real creators |
| 10 | **Licensing traps** | Remotion (not OSI), GPL/LGPL FFmpeg, MLT GPL modules | Choose Cutweave's license up front (Apache-2.0 or MPL-2.0 core), LGPL dynamic FFmpeg, track third-party licenses, no non-OSI deps |
| 11 | **Telemetry / cloud creep vs "local-first"** | Clypra telemetry; Timeline Studio cloud tie-in | No telemetry by default; fully offline; models downloaded with pinned checksums |
| 12 | **Closed ecosystem / no interchange** | Proprietary project files | Plain-file project folder (FreeCut style), OTIO + FCPXML export, CapCut draft import, plugin/scripting + MCP/headless |
| 13 | **Own-codec ambition** | FilmCraft re-implements h264/hevc/vp9 | Wrap FFmpeg and platform codecs; do not write decoders |

## The gap

Nobody has shipped a **stable, local-first, creator-grade (CapCut-flow) editor** combining: (a) short-form UX with magnetic timeline and templates, (b) local Whisper captions with karaoke styles plus text-based editing, (c) GPU-accelerated, crash-safe, long-timeline performance on all three desktop OSes with hardware decode, (d) an open template/effect/preset ecosystem with plain-file projects, and (e) agent/scripting hooks. Classic editors partly have (c) but not (a, b, d); web editors have (a, b) but not (c); Rust projects (Nuzky, FilmCraft, Clypra, OpenCut rewrite) are alpha or pre-product. Opening: shared-core + wgpu + FFmpeg desktop-first app with a polished vertical slice, with a browser build via Mediabunny/WebCodecs later.

## Recommended components (top 5)

1. FFmpeg/libav* (LGPL-2.1+ build, dynamic link) via rust-ffmpeg (WTFPL) or rsmpeg (MIT)
2. wgpu (MIT/Apache-2.0) for compositor/effects
3. Mediabunny (MPL-2.0) for web/WebCodecs build
4. OpenTimelineIO (Apache-2.0) for interchange only
5. whisper.cpp (MIT) plus Symphonia (MPL-2.0) for local captions and audio waveforms

## Sources

- https://github.com/OpenCut-app/OpenCut and https://github.com/opencut-app/opencut-classic (README, commits, contributors via gh api)
- https://github.com/KDE/kdenlive
- https://github.com/mltframework/shotcut
- https://github.com/OpenShot/libopenshot
- https://github.com/OpenShot/openshot-qt
- https://github.com/olive-editor/olive
- https://en.wikipedia.org/wiki/Olive_(software)
- https://github.com/olive-editor/olive/issues/1175
- https://github.com/jliljebl/flowblade
- https://github.com/pitivi/pitivi
- https://github.com/blender/blender
- https://github.com/Cinelerra-GG/cinelerra-gg
- https://github.com/mifi/lossless-cut
- https://github.com/NatronGitHub/Natron
- https://github.com/omni-media/omniclip
- https://github.com/diffusionstudio/core
- https://github.com/remotion-dev/remotion
- https://github.com/motion-canvas/motion-canvas
- https://github.com/midrender/revideo
- https://github.com/ncounterspecialist/twick
- https://github.com/storytold/filmcraft
- https://github.com/Augani/openreel-video
- https://github.com/nexu-io/openmotion
- https://github.com/AIEraDev/Clypra
- https://github.com/nuzkyapp/nuzky
- https://github.com/walterlow/freecut
- https://github.com/WeftCut/WeftCut
- https://github.com/MartinDelophy/ai-video-editor
- https://github.com/mltframework/mlt
- https://github.com/GStreamer/gst-editing-services
- https://github.com/AcademySoftwareFoundation/OpenTimelineIO
- https://github.com/Vanilagy/mediabunny
- https://github.com/bilibili/web-demuxer
- https://github.com/zmwangx/rust-ffmpeg
- https://github.com/larksuite/rsmpeg
- https://github.com/GStreamer/gstreamer-rs
- https://github.com/pdeljanov/Symphonia
- https://github.com/rerun-io/rerun
- GitHub repo searches (`gh search repos`) for "capcut alternative", "video-editor rust/tauri/webgpu/webcodecs", run 2026-11-22

Not done: HN/Reddit threads not searched directly; Cinelerra-GG's real repo (cinelerra-gg.org) not checked; Remotion license thresholds, MLT/GES license details and OpenShot performance claims are from memory **[U]**.
