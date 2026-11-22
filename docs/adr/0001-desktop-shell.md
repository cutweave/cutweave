# ADR-0001: Desktop shell: Electron + React with a separate Rust engine process (GPUI as plan B)

- **Status:** Accepted (provisional, 2026-11-22), on Spike S1 evidence from Windows (spikes/s1-shell-preview/README.md). Electron + React. **Phase 1 uses transport (a):** a napi-rs addon in the main window's non-sandboxed preload, with the present loop in the preload and NV12 `writeTexture`. Measured: 60.00 fps, 0 skips, scrub p95 49.5 ms. The sandbox exception is documented, mitigated by contextIsolation, no nodeIntegration, local-only content, strict CSP, a narrow bridge and sandboxed plugin iframes. **Transport (b)** (utilityProcess + MessagePort) is rejected for pixels: Electron forces 4 copies per frame, so ~540 skips/min and about 2 cores. **The target is Electron GPU shared textures** (probe: 58–60 fps, ~0.5 ms, no CPU copy), which restores a fully sandboxed renderer. It is gated on engine-side texture export, matching Chromium's GPU on hybrid laptops, and WP-0C.04 numbers. It becomes final after macOS/Linux measurements (WP-0C.04).
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (decision pending Spike S1)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §2, §10, §18

## Context

The shell hosts the React editor UI (ADR-0013) and the preview. The riskiest path in the product is getting 1080p60 frames from the Rust engine to the screen with an interactive DOM overlay (transform handles, crop box, text caret) and no z-order problems, on Windows 11, macOS arm64 and Ubuntu 24.04 (X11 and Wayland). Research 03 §1 initially leaned to Tauri 2 with a native wgpu child surface, then identified Electron as the pragmatic alternative and GPUI as the fallback. The architecture currently hypothesizes Electron; this is a hypothesis until Spike S1 passes.

## Decision drivers

- One consistent Chromium (WebGPU, WebCodecs, SharedArrayBuffer) on all three OSes, including Linux/Wayland.
- Overlay and preview in the same page (no native-surface z-order issues).
- Preview budget: 60 fps sustained, < 2 dropped frames/min, scrub-to-pixel p95 < 80 ms, transport CPU overhead < 15%.
- Contributor and AI-agent familiarity (TypeScript UI, iframe plugin UI).
- Crash isolation from the engine (ADR-0002).

## Considered options

1. **Electron + React; engine as a separate Rust process; preview via a shared-memory ring read by a napi-rs addon into a WebGPU canvas** (leading hypothesis). Sub-choice on the addon's home:
   - **(a)** non-sandboxed preload in the main editor window only (all other windows and plugin iframes sandboxed), or
   - **(b)** addon in a `utilityProcess` that forwards frames over `MessagePort`.
2. **Tauri 2 + React + native wgpu child surface.** Concerns: WebKitGTK on Linux lacks WebGPU and performs poorly, the native-surface-under-webview pattern is unsolved on Wayland, and z-order and resize issues exist on every OS.
3. **Tauri 2 + canvas via IPC.** Concerns: throughput, latency and color accuracy do not scale to 4K.
4. **GPUI (Rust-only UI), plan B.** Concerns: pre-1.0 with breaking changes, no timeline or inspector widgets, IME and accessibility immaturity, Rust-only contributor barrier, and no iframe plugin-UI story. Proven by Zed and Cap.
5. **Other Rust UI toolkits (egui, Iced, Slint, Dioxus, Xilem).** Not pursued: weaker fit than GPUI for this workload (research 03 §1.5).

## Decision

**Decided by: Spike S1** (WP-0C.04 measures, WP-0J.02 records) with the following pass criteria, all on Win11, macOS arm64, Ubuntu 24.04 X11 and Wayland:

- 60 fps sustained and < 2 dropped frames/min on the SHM-to-WebGPU preview;
- scrub-to-pixel p95 < 80 ms;
- transport CPU overhead < 15%;
- Canvas2D timeline >= 55 fps with 5k clips, 50k keyframes and 200 thumbnails;
- no overlay or z-order issues; live resize works.

On pass, ADR-0001 is Accepted with Electron and **records the preview transport choice (a or b)** from the S1 report. On fail, build the same demo in GPUI, compare, and decide (Accepted with fallback, or Rejected and superseded). The engine is UI-agnostic by contract (ADR-0002), so a shell swap costs only the UI. Nothing in this ADR is decided until S1 reports.

## Consequences

**Positive** (if Electron is confirmed)

- Identical Chromium and WebGPU everywhere; a single page for overlay and preview.
- The largest contributor pool and iframe plugin UI (ADR-0010 L3).

**Negative / costs**

- About 120 MB or more of install size and memory.
- A non-sandboxed preload (option a) widens the attack surface of the main window; it needs `contextIsolation`, no `nodeIntegration`, a narrow `contextBridge`, local `app://` content only and a strict CSP. Option (b) costs an extra hop and copy.
- A native addon layer (napi-rs) per platform and a quarterly Electron upgrade cadence behind golden-frame tests.
- If S1 fails, a UI rewrite to GPUI loses the React, plugin-iframe and contributor advantages.

**Follow-ups**

- WP-0C.01-0C.06 (spike), WP-0J.02 (accept and record transport), Epic 1H and 1L.

## Links

- Architecture: [§2](../01-architecture.md), [§10](../01-architecture.md), [§18](../01-architecture.md)
- Research: [03-tech-stack.md](../research/03-tech-stack.md) §1 (shell options, preview paths, comparison)
- Roadmap: [Phase 0, S1](../04-roadmap.md); WBS: [0C](../wbs/P0-foundations.md)
- Integrator resolutions: R19 (non-sandboxed preload in the main editor window only)
- Related: ADR-0002, ADR-0013
