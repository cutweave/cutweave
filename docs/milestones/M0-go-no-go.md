# M0 — Phase 0 go/no-go (provisional)

> **Status: GO with changes, signed off 2026-11-23.** Drafted by the integrator (WP-0J.03, 2026-11-22); the product owner delegated the sign-off decision to the integrator on 2026-11-23.
> All evidence comes from one Windows 11 laptop (`ref-win-dgpu` plus interim `ref-win-igpu`; see [reference-machines.md](../reference-machines.md)), often measured while other agents loaded the machine. Formal cross-platform measurements (WP-0C.04, 0D.02, 0E.03, 0F.03, 0G.02) wait for the hardware in [#45](https://github.com/cutweave/cutweave/issues/45).

## Spike results against pass criteria

| Spike | What it proved | Key numbers (informal, Windows) | Verdict |
|---|---|---|---|
| **S1** Shell & preview ([spike](../../spikes/s1-shell-preview/README.md)) | A Rust sidecar delivers wgpu frames into Electron via shared memory. WebGPU presentation, overlays, scrub and live resize all work | 60.00 fps / 0 skips (preload transport); scrub-to-pixel p95 **49.5 ms** (budget 80); 0 blank frames over 119 resizes; Canvas2D timeline 60 fps with 5k clips (p95 draw 1.3 ms) | **Pass on Windows.** macOS/Linux pending |
| S1 transport (b) | A fully sandboxed renderer over MessagePort | ~540 skips/min, ~2 cores (Electron forces 4 copies) | Rejected for pixels. **GPU shared textures** (~0.5 ms, no CPU copy) are the target |
| **S2** Audio clock ([spike](../../spikes/s2-audio-clock/README.md)) | cpal mixer as master clock, time-stretch, loudness; video follows audio | 10-min soak: 0 underruns, 0 RT allocations, callback max 0.29 ms, +11 ppm drift, 0 skipped video frames, LUFS ±0.01 | **Pass on Windows.** 30-min soak + wired A/V offset pending |
| **S3** Decode & seek | Frame-exact seek, VFR, HW decode | — | **Partial pass 2026-11-24** (WP-0E.01: 500/500 frame-exact on 16 files; 4K CPU scrub p95 282-804 ms, over 150 ms; proxies 0.2-1 ms). HW decode (0E.02) and macOS/Linux (0E.03) pending |
| **S4** Compositor ([spike](../../spikes/s4-compositor/README.md)) | Deterministic linear-light render graph, blend modes, LUT, PQ→SDR | 1080p GPU p50/p95: RTX 3080 0.7/3.1 ms; AMD iGPU 6.5/8.1 ms (budget 8, borderline); ≤1 code value across vendors | **Pass**, iGPU borderline. Re-measure under sustained load |
| S4 text ([spike](../../spikes/s4-compositor/README.md)) | Animated captions | MSDF: 1000 glyphs 0.70 ms iGPU / 0.06 ms RTX; 4K SSIM 0.9976; no shimmer | **Pass**. MSDF chosen, Vello rejected |
| **S4b** Compositions ([spike](../../spikes/s4b-compositions/README.md)) | TS/JSX → oxc → QuickJS → scene graph | Transpile 0.14 ms; deterministic; 35/35 sandbox escapes contained; 200 fully animated nodes 2.6 ms (budget 1) | **Conditional pass** (ADR-012 conditions) |
| **S5** Command bus → MCP ([spike](../../spikes/s5-command-bus/README.md)) | Macro + registry + codegen; rmcp server; a real agent drives it | 73 tests; tools/list standard ≈2.7k tokens; **Claude Code completed the task 3/3 over MCP with no screenshots** ([report](../../spikes/s5-command-bus/AGENT-RUN-REPORT.md)) | **Pass** |

## ADR status

| ADR | Decision | Status |
|---|---|---|
| 001 | Electron + React; preload transport in Phase 1, GPU shared textures as target | Accepted (provisional) |
| 003 | FFmpeg via ffmpeg-next behind `MediaBackend` | **Accepted with fallback** 2026-11-24 (proxies cover the 4K CPU scrub budget) |
| 007 | MSDF text (Vello rejected) | Accepted (provisional) |
| 009 | rmcp + hybrid MCP catalog | Accepted (provisional); second client pending |
| 012 | oxc + QuickJS compositions | Accepted (provisional, with conditions) |
| 014 | Minimum OS | Proposed: needs min-OS VMs |

## Recommendation

**GO for Phase 1, except where FFmpeg is required.**

- **Proceed now:** model, command bus, persistence, render core, API server, MCP, CLI, Electron shell, editor UI, timeline, codegen.
- **Hold until S3 passes:** media probe/decode/thumbnails/waveforms (1D), export (1E), audio clip decoding (1G.01), and anything that renders real footage.
- **Risks carried forward:**
  - iGPU compositor headroom (6.5–8.1 ms).
  - The compositions evaluation budget.
  - macOS/Linux behaviour is unmeasured for every spike.
  - The non-sandboxed preload until shared textures land.
- **Re-estimate:** Phase 0 build work finished well ahead of plan with parallel agents. The critical path is now **FFmpeg availability and the hardware purchase**, not engineering capacity.

**Product-owner sign-off:** ☐ GO as recommended ☒ **GO with changes** ☐ NO-GO (2026-11-23; decision delegated by the product owner to the integrator)

Changes to the recommendation above:

- **FFmpeg-dependent work was sequenced, not held:** resolutions R27–R31 (docs/05-work-breakdown.md) let the job manager, CLI, stdio MCP, audio, transport and UI proceed against seams and stand-ins (`--dev-stub-media`, synthetic frames, generated tones). Real decode, thumbnails, waveforms, preview frames and export follow S3.
- **S3 reported a partial pass on 2026-11-24** (WP-0E.01, Windows, generated test media, FFmpeg 8.1.3 BtbN GPL shared build for development; the CI-built bundle replaces it for releases): frame-exact seek passed, the CPU 4K long-GOP scrub budget did not, and proxies cover it. ADR-003 is Accepted with fallback (R32), so the 1D/1E epics start; hardware decode (0E.02) and macOS/Linux (0E.03) fill the remaining rows.
- **Cross-platform measurements** (0C.04, 0D.02, 0E.03, 0F.03, 0G.02) run as the hardware arrives: an Apple Silicon Mac is available; Linux runs in a VM for functional checks (GPU and audio performance numbers still need bare metal); audio loopback fixtures are being sourced.
- **Risks carried forward** are unchanged: iGPU compositor headroom, the compositions evaluation budget, unmeasured macOS/Linux behaviour, and the non-sandboxed preload until GPU shared textures land.
