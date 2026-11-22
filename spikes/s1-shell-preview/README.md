# Spike S1: Electron shell + Rust sidecar + SHM frame ring + WebGPU preview (WP-0C.01, WP-0C.02, WP-0C.06)

> **Throwaway spike code.** Not part of the root Cargo or pnpm workspaces; product code never
> depends on it. It answers one question for ADR-001: can preview frames rendered in a
> separate Rust process reach the Electron UI at 1080p60 with low latency?
>
> - WP-0C.01 built transport (a): the napi addon in the main window's preload.
> - WP-0C.02 added the WebGPU canvas (three upload paths), the DOM overlay, the scrub bar, live
>   resize and the benchmark suite; see
>   [WP-0C.02](#wp-0c02-webgpu-canvas-dom-overlay-scrub-bar-live-resize).
> - WP-0C.06 added transport (b) (utility process + `MessagePort` to a fully sandboxed
>   renderer), an A/B benchmark mode and a GPU `sharedTexture` probe; see
>   [WP-0C.06](#wp-0c06-transport-b-ab-against-a-and-a-gpu-texture-probe).
>
> The combined verdict is in [ADR-001 input](#adr-001-input-summary) below.
>
> Location note: the WBS names the area `spikes/s1-preview/{engine,bridge,desktop}`; this
> spike lives in `spikes/s1-shell-preview/` with the same sub-structure. WP-0C.04 should either
> reuse this path or rename it in one go.

## ADR-001 input (summary)

This summarises the three work packages for [ADR-001](../../docs/adr/0001-desktop-shell.md),
which is now Accepted (provisional) on this evidence. All numbers are informal: one Windows
laptop, often shared with other agents' builds. WP-0C.04 owns the formal multi-OS measurement.

**Recommendation, as recorded in ADR-001:**

1. **Electron + React, with the engine as a separate Rust process.** Phase 1 previews through
   **transport (a)**: the napi-rs addon in the main editor window's preload
   (`sandbox:false`), with the present loop in the preload's isolated world, clock-scheduled
   frame choice (`policy=clock`, 20 ms) and NV12 + `queue.writeTexture`. Measured: 60.00 fps
   with 0 skipped frames on quiet runs (WP-0C.01 60 s run, WP-0C.02 run B, WP-0C.06 A/B rows),
   and scrub-to-pixel p95 49.5 ms.
2. **The cost of (a) is the sandbox exception.** The editor renderer runs without the OS sandbox
   (measured `sandboxed: false`, integrity `medium`). The required mitigations are
   `contextIsolation`, no `nodeIntegration`, local `app://` content only, a strict CSP, a narrow
   `contextBridge`, the UI token kept out of the main world, every other window and plugin UI
   sandboxed with no preload, and validated IPC.
3. **Transport (b) is rejected for pixels.** It works, and it passes every functional and
   sandbox check. But `MessagePortMain` cannot transfer buffers, so Electron copies each frame
   about 4 times. At 1080p RGBA8 that cost 1.2-1.7 cores in the renderer and utility alone
   (about 2 cores with the GPU process and engine), and a median of about 540 skipped
   frames/min. Keep `utilityProcess` + `MessagePort` for control and metadata only.
4. **The target is Electron GPU shared textures.** In the probe they reached a fully sandboxed
   renderer at 58-60 fps, about 0.5 ms main -> page, with no CPU pixel copy, so the sandbox
   exception can go. The switch is gated on engine-side texture export (D3D12 NT handle,
   IOSurface, DMA-BUF), the engine rendering on Chromium's adapter on hybrid-GPU laptops, the
   still-experimental `sharedTexture` API holding up, and WP-0C.04 numbers.
5. **Keep the renderer-facing frame interface transport-agnostic.** That means frames with
   `seq/pts/size/format` plus the engine audio clock, feeding the scheduler and present loop.
   Swapping (a) for GPU textures then does not touch the React or overlay code. This spike
   runs one `PresentLoop` in the preload world, the page's main thread and a worker.

**ADR-001 pass criteria on this machine (Windows 11 only):**

| Criterion | Result | Where |
|---|---|---|
| 60 fps sustained, < 2 dropped frames/min | **Met on quiet runs** with (a): 60.0 fps and 0 drops in the WP-0C.01 60 s run, WP-0C.02 run B and four of five WP-0C.06 (a) rows (the fifth had 5.8/min under about 9.5 cores of foreign load). The 60 s phases of WP-0C.02 run A had 10-36 drops/min from producer-side stalls on a shared machine. (b) misses it. | [0C.01](#wp-0c01-informal-numbers-dev-machine-not-the-wp-0c04-measurement), [0C.02](#wp-0c02-informal-numbers-dev-machine-not-the-wp-0c04-measurement), [A/B](#ab-table-informal-this-machine) |
| Scrub-to-pixel p95 < 80 ms | **Met**: 49.5 ms (low-res first; 79 ms with an emulated 50 ms decode) | [0C.02](#wp-0c02-findings-informal-one-machine) |
| Transport CPU overhead < 15 % | **(a) met**: 1 copy, about 0.8 ms per 1080p RGBA8 frame (about 5 % of one core at 60 fps). **(b) fails**: 4 copies, 8-10 ms of `postMessage` per frame plus deserialization and GC. | [copies per frame](#copies-per-frame-1080p-rgba8--83-mb) |
| Canvas2D timeline >= 55 fps (5k clips, 50k keyframes, 200 thumbnails) | Met in WP-0C.03 (separate spike) | [s1-timeline-stress](../s1-timeline-stress/README.md) |
| No overlay or z-order issues; live resize works | **Met**: 22/22 hit tests, 6/6 real-input interactions, 0 blank frames in 119 resizes, engine renegotiates in 2-3 frames | [0C.02](#wp-0c02-findings-informal-one-machine) |
| macOS arm64, Ubuntu 24.04 X11 and Wayland | **Not measured**: POSIX shared memory is a stub; [FLAGS.md](FLAGS.md) rows are unverified | WP-0C.04 |

Risks to carry into Phase 1: hybrid GPUs put the engine and Chromium on different adapters
(WP-0C.01 finding 5); crash detection needs a ring-heartbeat watchdog for hangs (WP-0C.01
finding 6, WP-0C.06 finding 9); the 20 ms clock margin leaves little headroom for producer
stalls (WP-0C.02 finding 2); and an intermittent "no frames after restart" self-test failure
is still open (WP-0C.02 finding 8).

## What is here

```text
spikes/s1-shell-preview/
  Cargo.toml         own Cargo workspace: ring, engine, bridge
  ring/              s1-ring: named shared memory + seqlock frame ring (layout: RING.md)
  engine/            s1-engine: wgpu synthetic 60 fps frames at the preview size -> async
                     readback -> ring; WebSocket control channel (play/pause/seek/format/
                     resize/scrub-config); scrub.rs = low-res-first scrub scheduling
  bridge/            s1-bridge: napi-rs v3 addon (RingReader, TestRingWriter) + Node tests
  desktop/src/       Electron main, preload (transport (a)), bench.ts (suite + A/B modes)
    shared/          present.ts (WebGPU upload paths), loop.ts (rAF loop + scrub-to-pixel),
                     stage.ts (canvas size + resize renegotiation), preview.ts (wiring)
    renderer/        (a) page: stage, overlay.ts (transform box, safe areas), scrub.ts, HUD
    transport-b/     transport (b) + sharedTexture probe (WP-0C.06): main-side wiring,
                     utility frame pump, sandboxed preload/page/worker, probe
  scripts/           build-native / build-js(-b) (esbuild) / selftest / bench / bench-ab /
                     summarize(-bench)
  results/           informal self-test and bench JSON from the dev machine
  RING.md            ring header/slot layout and protocol (feeds WP-1H.02)
  FLAGS.md           Chromium flags for WebGPU per OS (only Windows verified)
```

## Run it

Prerequisites: the repo toolchain (Rust 1.100 via `rust-toolchain.toml`, Node 24, pnpm 12).
Shared memory is implemented for **Windows only**; Linux/macOS compile to a stub that
returns `Unsupported` (see `ring/src/shm.rs` for the intended POSIX implementation).

```sh
cd spikes/s1-shell-preview
pnpm install                        # own lockfile; Electron 45 fetches its binary lazily
node node_modules/electron/install.js   # optional: fetch the Electron binary up front
pnpm build                          # cargo build --release (engine + addon) + esbuild bundles
pnpm start                          # transport (a) window with live preview + HUD
pnpm selftest 60                    # WP-0C.01 acceptance checks -> results/selftest-*.json
pnpm bench [quick|<s>]              # WP-0C.02 benchmark suite (20 s phases, ~4 min)
                                    #   -> results/bench-*-suite-*.json (+ git-ignored screenshot)
pnpm bench ab [<s>] [--transport=utility|texture]   # one WP-0C.06 A/B run (default 60 s)
node scripts/bench-ab.mjs [<s>]     # the whole A/B matrix: (a), (b) main thread, (b) worker
node scripts/summarize-bench.mjs    # Markdown tables of the suite results
node scripts/bench-ab.mjs --table   # Markdown tables of the A/B results
pnpm test                           # Node tests for the addon (needs `pnpm build`)
pnpm test:rust                      # ring unit tests (wrap-around, seqlock, shm lifetime)
pnpm typecheck
node scripts/summarize.mjs          # Markdown table of results/selftest-*.json
```

Transport (b) and the probe: `pnpm start -- --transport=utility [--draw=main|worker]`,
`pnpm selftest 60 --transport=utility --policy=clock`, `pnpm start -- --transport=texture`
(details in [WP-0C.06](#wp-0c06-transport-b-ab-against-a-and-a-gpu-texture-probe)).

`pnpm start` / `pnpm selftest` / `pnpm bench` accept these flags (`pnpm start -- --format=nv12`):

| Flag | Meaning |
|---|---|
| `--format=rgba8\|nv12` | engine output format (also toggled live in the window) |
| `--transport=preload\|utility\|texture` | WP-0C.06: (a) addon in the preload (default), (b) addon in a utility process + `MessagePort` to a sandboxed renderer, or the sharedTexture GPU probe (no engine) |
| `--draw=main\|preload` (a), `--draw=main\|worker` (b) | where the rAF present loop runs. (a): renderer main world (pixels cross `contextBridge`) or the preload's isolated world (draws into the page's canvas, no clone). (b): the sandboxed page's main thread or a dedicated worker with an `OffscreenCanvas` |
| `--policy=latest\|next\|clock` | frame choice: latest-wins; naive FIFO; or newest slot with `pts <= audio clock - latency` |
| `--latency=<ms>` | latency target of `policy=clock` (default 20) |
| `--present=gpu\|2d` | WebGPU (default) or Canvas2D `putImageData` (RGBA8 only) |
| `--upload=writeTexture\|videoFrame` | WebGPU upload path (WP-0C.02; also toggled live in the window) |
| `--engine-size=canvas\|WxH` | (a): engine renders at the canvas pixel size (default) or a fixed size the presenter scales; an A/B bench run defaults to `--size` |
| `--bench[=quick\|<s>]` | run a benchmark instead of the interactive window, then exit (see [Bench CLI](#bench-cli-and-result-schema)) |
| `--bench-mode=suite\|ab` | `suite` = the WP-0C.02 phases (default for (a), (a) only); `ab` = one WP-0C.06 A/B run (default for `utility` and `texture`) |
| `--warmup=<s>` | A/B mode: warm-up before counters reset (default 5) |
| `--bench-priority=above` | both modes: above-normal priority for every process that touches frames |
| `--verify=0` | disable per-frame checksum + row-stamp verification |
| `--gpu=high\|low\|software` | wgpu adapter preference for the engine |
| `--size=WxH` | engine frame size at start (default 1920x1080) |
| `--chromium-dgpu` | adds Chromium's `force_high_performance_gpu` (WebGPU on the dGPU) |

Environment: `WGPU_BACKEND=dx12|vulkan` (engine backend), `S1_LOG=debug` (engine log
filter), `S1_ENGINE=<path>` (engine binary). Window: play/pause, scrub bar with timecode
(drag, click, arrow keys), transform box (drag, 8 handles, arrow keys), back/forward 1 s,
format RGBA8/NV12, upload path, engine size (canvas / 1920 × 1080), safe areas, stats HUD,
verification on/off, reset stats.

The engine also runs standalone: `target/release/s1-engine --ring cw-s1-test --no-stdin-watch`
(WP-0C.02 flags: `--max-width/--max-height` ring capacity, default 2560x1440;
`--lowres-div` scrub preview divisor, default 2, 0 = off; `--full-cost-ms` emulated
full-resolution decode time per scrub frame, default 0).

## Bench CLI and result schema

One flag, `--bench[=quick|<s>]`, runs a benchmark in either of two modes (`desktop/src/bench.ts`).
`pnpm bench [suite|ab] [quick|<s>] [flags]` wraps it. For transport (a) it adds
`--draw=preload --policy=clock`, and for the other transports `--policy=clock`.

| Mode | Transports | Length | What it measures |
|---|---|---|---|
| `suite` (WP-0C.02; default for (a)) | (a) only: it drives that page's overlay, scrub bar and stage | `<s>` per playback phase (default 20, `quick` = 5) | playback matrix ({RGBA8, NV12} x {writeTexture, VideoFrame}) at 1920x1080, canvas-size playback, 4 scrub variants with real pointer input, a live-resize sweep with pixel captures, overlay hit tests and drags; per-second metrics plus checks; exits non-zero if a check fails |
| `ab` (WP-0C.06; default for `utility`, `texture`) | all three | `<s>` measured after `--warmup` (default 60, `quick` = 10) | one configuration: present fps, skipped/min, engine submit -> present p50/p95, per-process CPU incl. the utility, whole-machine and foreign load. For (a) the engine stays at `--size`, so (a) and (b) move the same bytes |

`scripts/bench-ab.mjs` launches `ab` runs as a matrix. Per size, it alternates (a), (b) on the
main thread and (b) in a worker, waits for a quiet machine before each run and repeats
contaminated runs. `--bench-mode=suite` with `--transport=utility|texture` exits with status 2.

Both modes write `results/bench-*.json` with a common envelope, then a mode-specific body:

| Part | Fields |
|---|---|
| envelope (both) | `kind: "s1-bench"`, `mode: "suite" \| "ab"`, `wp`, `transport`, `informal`, `startedAt`, `platform`, `electron`, `chrome`, `options` (all CLI options), `engine` (handshake info), `priorityRaised` |
| `suite` body | `display`, `config`, `fastest`, `matrix[]`, `scrub {variants[], allVariantsScrubPixelP50/P95}`, `control`, `resize {stage, steps[], captures}`, `overlay {hitTests[], interactions[], alignAfter}`, `phases[]`, `metrics[]`, `checks`, `notes`, `pass` |
| `suite` per-second row (`metrics[]`) | `{ts, phase, fps, dropped, engineDropped, scrub_ms[], renegotiate_ms[], cpu{engine, main, renderer, gpu}, frame, align}` |
| `ab` body | `summary` (flat: one row of the A/B table: `transport`, `drawIn`, `policy`, `size`, `format`, `presentFps`, `skippedPerMin`, `engineDropped`, `renderToPresentMs`, `cpu{renderer, utility, producer?, gpu, main, engine, system, foreign}`, `os`, ...), `snapshot` (the page's full loop snapshot), `extra` (utility or probe stats) |

New files are named `bench-<platform>-suite-<stamp>.json` and
`bench-<platform>-<transport>-<loop>-<size>-<format>-<stamp>.json`. The committed files predate
the `mode` field. In those, `summary` marks an A/B file and `matrix` a suite file; both table
scripts apply that rule and skip the other mode's files.

## Architecture sketch

Transport (a); transport (b) is sketched in [WP-0C.06](#what-transport-b-is).

```text
 Electron main (Node)                             s1-engine (Rust, child process)
 ─────────────────────                            ───────────────────────────────
 pick free port P ── spawn --ring R --port P ───▶ CreateFileMapping("Local\R")  (exclusive)
                                                  wgpu device (dGPU), bind 127.0.0.1:P
 read ONE stdout line ◀── {"s1":"ready",port,     256-bit token from OS RNG
   token kept in memory only     token,ring,...}  render loop, clock-driven (frame N when the
 child 'exit' / stdout close ─▶ engine down         audio clock enters N): render pass
   ─▶ notify preload, wait for ring-closed ack      [-> NV12 compute] -> copy -> map_async
   ─▶ restart (same P, new R, new token)            -> memcpy into slot (seqlock) -> publish
 ipcMain.handle("s1:handshake")                   audio-clock thread (1 kHz) -> header
   only for the main editor window's main frame   WS control: auth msg, play/pause/seek/format
   on app://s1; token handed out once per launch
        │ IPC                                            ▲ ws://127.0.0.1:P (Origin app://s1)
        ▼                                                │
 Preload (main editor window ONLY; sandbox:false, contextIsolation:true, nodeIntegration:false)
   require(s1_bridge.node) ── RingReader.open(R) ── OpenFileMapping(FILE_MAP_READ) ──┐
   WebSocket (token never leaves this closure)                                        │ shm
   contextBridge.exposeInMainWorld("bridge", {getLatestFrame, getFrame, transport,   │ ring
     presentInPreload, readHeader, nowUs, info, stats, onEngineEvent, ...})           │
        │ contextBridge (pixels structured-cloned)                                     │
        ▼                                                                              │
 Renderer main world (app://s1, strict CSP): rAF -> bridge.getFrame() -> verify ->     │
   WebGPU writeTexture -> draw; HUD.  Alternative: --draw=preload runs the same loop   │
   in the isolated world and only stats cross the bridge.                              │
 Any other window: sandbox:true, no preload -> no window.bridge                        │
```

Security posture as built: renderer content only from the `app://s1` custom protocol
(path-traversal checked) with `default-src 'none'; script-src 'self'; connect-src
ws://127.0.0.1:P; ...`; navigation, `window.open` and `<webview>` denied; permission
requests denied; the IPC handshake checks the sender frame; the engine binds 127.0.0.1 only,
checks `Host` and `Origin`, and requires the token as the first WebSocket message; the token
is never written to disk, a URL, the DOM, the main world or a log line; the engine exits when
its stdin closes (Electron gone), so it cannot outlive the UI.

## WP-0C.01 acceptance criteria

| Criterion | Status | Evidence |
|---|---|---|
| Transport (a) with `contextIsolation:true`, `nodeIntegration:false`: main world has `typeof require === 'undefined'` and `typeof process === 'undefined'` while `window.bridge.getLatestFrame()` returns frames; a second sandboxed `BrowserWindow` has no `window.bridge` | PASS | self-test checks `mainWorldHasNoRequire`, `mainWorldHasNoProcess`, `bridgeReturnsFrames`, `sandboxedWindowHasNoBridge` (`desktop/src/main.ts` `runSelftest`) |
| 60 s run: monotonically increasing `seq`, zero torn frames (checksum embedded in the frame, verified in the renderer) | PASS | three 60 s runs, `results/selftest-win32-rgba8-{main-latest,preload-latest,preload-clock}-gpu-*.json`: `seqViolations 0`, `checksumFailures 0`, `rowStampFailures 0`, addon torn-retries 0 |
| Sidecar crash (`kill -9`) detected by main in < 1 s; ring cleaned up (handle closed on Windows) | PASS | self-test `crashDetectedUnder1s`, `ringReleased` (`OpenFileMappingW` fails after the preload closed its view); engine restarts and frames flow again |
| Rust unit test for ring writer/reader wrap-around | PASS | `ring/src/ring.rs` tests (4..=6 slots, several laps, slow reader, torn slot, concurrent writer/reader), `ring/src/shm.rs` lifetime test |
| Node test for the addon reader | PASS | `bridge/test/reader.test.mjs` (7 tests incl. an end-to-end run against the real engine and the WebSocket control channel) |
| NV12 and RGBA8 variants, wgpu readback, 4-6 slots, per-slot `pts,w,h,format,rev,seq`, continuous audio-clock word | Done | `engine/src/gpu.rs`, `engine/src/frame.wgsl`, `RING.md` |
| WS channel for play/pause/seek from renderer to sidecar | Done | `engine/src/control.rs` (+ `format`, `ping`) |
| Ring layout documented | Done | `RING.md` |

Not done here (by design): WebGPU canvas polish, `importExternalTexture`, overlays, resize
(done in WP-0C.02, below); transport (b) (done in WP-0C.06, below); formal measurement and
other OSes (0C.04);
POSIX shared memory (stub); a Playwright smoke test (the self-test drives the same checks from
Electron main).

## WP-0C.01 informal numbers (dev machine, NOT the WP-0C.04 measurement)

> **Informal, first-look numbers.** One run per row, on one developer laptop, with other
> apps open. There is no warm-up control, the window was visible on screen, and run-to-run
> variance is large: the same `rgba8/main/latest` configuration gave 6 gaps/min in the run
> below and 290-320/min in exploratory runs. Formal, repeated, multi-OS measurement is
> WP-0C.04. Raw JSON is in `results/`, and the table comes from `node scripts/summarize.mjs`.

Machine: AMD Ryzen 9 5900HX, 32 GB RAM, Windows 11 Pro (build 26200), hybrid graphics.
The engine ran on the NVIDIA GeForce RTX 3080 Laptop GPU (wgpu 30.0.2, Vulkan; driver
32.0.16.1692). Chromium's WebGPU ran on the AMD Radeon iGPU ("amd / gcn-5"), which drives
the 60 Hz internal panel, except in the `/dgpu` row. Software: Electron 45.4.1 (Chromium
156.0.8078.100), 1920x1080 at 60 fps, 6 slots, verification on.

| format / loop in / policy / present | run s | present fps | seq gaps /min | torn (cksum/stamp) | engine render -> present submit p50/p95 ms | addon copy p50/p95 ms | contextBridge clone p50/p95 ms | upload+draw p50/p95 ms | CPU % of one core: engine / renderer / GPU proc | kill -> exit event ms |
|---|---|---|---|---|---|---|---|---|---|---|
| rgba8 / main / latest / webgpu | 60 | 59.85 | 6.0 | 0 / 0 | 18.9 / 23.6 | 0.7 / 1.2 | 4.8 / 8.5 | 0.8 / 1.4 | 14.7 / 62.3 / 25.3 | 173 |
| rgba8 / preload / latest / webgpu | 60 | 59.53 | 25.8 | 0 / 0 | 20.5 / 22.4 | 0.8 / 1.7 | 0.1 / 0.2 | 0.8 / 1.5 | 15.9 / 20.4 / 27.0 | 97 |
| **rgba8 / preload / clock 20 ms / webgpu** | 60 | **60.00** | **0** | 0 / 0 | **24.6 / 25.3** | 0.8 / 1.1 | 0.1 / 0.2 | 0.7 / 1.1 | 10.2 / **17.6** / 22.2 | 229 |
| rgba8 / main / clock 20 ms / webgpu | 30 | 58.42 | 31.6 | 0 / 0 | 39.0 / 43.7 | 0.7 / 1.5 | 5.8 / 10.4 | 0.7 / 1.5 | 14.8 / 67.5 / 26.7 | 225 |
| nv12 / main / latest / webgpu | 30 | 59.02 | 47.5 | 0 / 0 | 12.0 / 13.8 | 0.2 / 0.4 | 2.5 / 3.7 | 1.2 / 2.0 | 6.0 / 36.4 / 16.2 | 85 |
| nv12 / preload / clock 20 ms / webgpu | 30 | 59.15 | 0 | 0 / 0 | 35.9 / 36.8 | 0.3 / 0.4 | 0.1 / 0.2 | 1.3 / 2.0 | 9.2 / 18.1 / 17.2 | 92 |
| rgba8 / preload / clock 20 ms / webgpu, `/dgpu` | 30 | 58.23 | 23.6 | 0 / 0 | 30.8 / 32.1 | 0.8 / 1.3 | 0.1 / 0.2 | 0.8 / 1.5 | 10.3 / 20.0 / 29.1 | 155 |

How to read the columns:

- **seq gaps** are frames the engine published that were never presented.
- **engine render -> present submit** runs from the engine's GPU submit to the renderer's
  `queue.submit`. Compositor and scan-out latency (typically one or two more vsyncs) are not
  included.
- **CPU** is per process over the run, as a percentage of one core: the engine from
  `TotalProcessorTime`, Electron processes from `app.getAppMetrics()`.

Other observations from the same runs:

- The engine sustained 60 fps. GPU submit to publish took 2.5-8 ms, including a 0.9-2 ms
  memcpy into the ring.
- Producer-side drops were 0-46 per run, clustered in bursts at startup or alongside other
  GPU load.
- The stdout pipe closed 10-25 ms before the `exit` event.
- After `kill -9`, the restarted engine had frames on screen again after 1.3-2.2 s (about
  1 s of that is GPU init).
- The WebSocket control round trip was about 0.6 ms.
- `externalArrayBuffers` was `false` in every Electron run.
- The 60 s acceptance runs (rows 1-3) all passed every WP-0C.01 check.

## WP-0C.01 findings and risks (input for ADR-001)

1. **"Zero-copy ArrayBuffer over the ring" is impossible in Electron.** Electron's V8 memory
   cage rejects external buffers (`napi_create_external_arraybuffer` returns
   `napi_no_external_buffers_allowed`) and napi-rs then silently copies. The self-test
   records `externalArrayBuffers: false` in the Electron preload; the same addon call is
   zero-copy in plain Node (Node test). So transport (a) costs at least one copy from shared
   memory into the V8 heap: about 0.6-1.0 ms for 8.3 MB RGBA8 and 0.3 ms for 3.1 MB NV12.
   Arch §10 step 2 ("zero-copy") should be reworded.
2. **`contextBridge` structured-clones every ArrayBuffer, on the renderer main thread.**
   Handing the pixels to the page's main world adds a second full copy: about 4-9 ms p50 and
   up to 14 ms p95 for 1080p RGBA8 (about 2.5 ms for NV12). The renderer process then uses
   45-85 % of a core, compared with 17-21 % when the same loop runs in the preload. At 1080p60
   RGBA8, `draw=main` is marginal and drops frames whenever the main thread is busy.
   **To make transport (a) viable, run the preview present loop (ring read, WebGPU upload,
   draw) in the preload's isolated world.** The loop draws into a canvas element, which
   crosses `contextBridge` by reference; the main world only receives stats and controls.
   This changes the 0C.02 plan ("React renderer owns the WebGPU canvas"): React owns the
   element and its layout, while privileged preload code owns the GPU upload. The other
   option is NV12 plus the main world, which still costs about 2.4 ms per frame of
   main-thread copy.
3. **`sandbox:false` turns off the OS sandbox for the whole renderer process, not just the
   preload.** With contextIsolation on, page JS still cannot reach Node. But a renderer RCE
   in the main editor window is not contained by Chromium's OS sandbox. This is the real
   security cost of transport (a), and 0C.06 / ADR-001 should weigh it against the extra
   copies and IPC that transport (b) needs. Mitigations already in place: local `app://`
   only, strict CSP, no remote content, a narrow bridge, the token kept out of the main
   world, and every other window sandboxed with no preload.
4. **Pacing, not transport, sets the drop rate.** Latest-wins pulling in rAF against a
   free-running 60 Hz producer drops 6-320 frames/min, depending on phase, even with zero
   producer drops and zero torn frames. Producer and display are two independent ~60 Hz
   clocks, and publish jitter of a few ms around the vsync phase is enough to skip a frame.
   Choosing "the newest slot with `pts <= extrapolated audio clock - 20 ms`" (the ring's
   audio-clock word plus the slot table, i.e. arch §10 step 3) presented 60.00 fps with
   0 gaps over 60 s with the loop in the preload (also 0 for NV12 over 30 s).
   Engine-submit to present-submit latency was about 25-36 ms. Runs that did show gaps (up
   to about 24/min) matched producer stalls: GPU readback hiccups of 60-100 ms at startup or
   under other GPU load, visible as engine-side drops. Implications:
   - 0C.02 needs this scheduler.
   - 0C.04 must define "dropped frame" precisely. A 60 fps timeline on a 59.94 Hz panel
     repeats or skips frames at the beat frequency whatever the transport does.
   - Bug found and fixed here: the first engine loop (a tick accumulator) drifted about
     35 ms ahead of its own audio clock under load. Production is now clock-driven: frame N
     is rendered when the clock enters N.
5. **Hybrid GPUs put the engine and Chromium on different adapters.** wgpu
   `HighPerformance` picks the RTX 3080, but Chromium's WebGPU used the AMD iGPU that drives
   the panel, even with `powerPreference: 'high-performance'`. `--force_high_performance_gpu`
   (the `--chromium-dgpu` flag) moves Chromium to the NVIDIA dGPU; the result there was
   similar. The future zero-copy shared-texture path (arch §10 upgrade) needs both on the
   same adapter: the engine must pick Chromium's adapter (match the LUID), or Chromium must
   be forced onto the dGPU. The ring path does not care.
6. **Crash detection passes but has a long tail.** From `kill` to the child `exit` event took
   64-230 ms, with one 530 ms outlier on Vulkan in exploratory runs. Most of that is GPU
   driver teardown inside the dying process; the stdout pipe closes only about 5-25 ms
   earlier. This is under 1 s, but a ring-heartbeat watchdog (`heartbeat_us`) should be added
   to detect hangs, which never produce an exit.
7. **Ring cleanup on Windows depends on the reader.** A file-mapping object dies with its
   last handle, and Electron main cannot close a handle held by the renderer process. The
   preload therefore closes its view on `engine-down` and acknowledges; the self-test then
   verifies the object is gone. On POSIX, main can `shm_unlink` by name. A hung renderer
   keeps the object (and up to about 50 MB of pagefile-backed memory) alive.
8. **Named-object access.** The mapping uses the default DACL, so any process of the same
   user in the session can open it to read or spoof frames. Exclusive create and a random
   name prevent squatting. Stricter options: a DACL for the current logon SID only, an
   unnamed section duplicated into the renderer process, or `memfd` plus fd passing on Linux.
9. **Toolchain surprises.**
   - Electron 45 no longer downloads its binary in `postinstall`; it fetches it lazily on
     first launch (or via `install-electron`). CI and packaging must pre-fetch it.
   - pnpm 12 needs `allowBuilds` for esbuild.
   - TypeScript 7's `lib.dom` lacks the `"webgpu"` overload of `getContext` and the
     `GPUTextureUsage` / `GPUShaderStage` constants. Product packages need `@webgpu/types` or
     local constants.
10. **Engine cost is fine.** The engine runs a steady 60 fps. GPU submit to publish is
    2.5-8 ms, including 0.9-2 ms to memcpy 8.3 MB into the ring. That is about 10-15 % of
    one core, mostly the memcpy. wgpu `map_async` with 3 staging buffers is enough at
    1080p60 on both the Vulkan and DX12 backends.

**Bottom line for ADR-001 (informal, one machine):** the Electron + sidecar + shared-memory
ring path delivers 1080p60 with zero torn frames and ~25-35 ms engine-to-present latency,
**provided** the present loop lives in the preload (or frames are NV12) and presentation is
clock-scheduled. Transport (a) as literally specified (ArrayBuffer views handed to the main
world) does not hold in Electron. Its remaining costs are one 8 MB copy per frame and a
renderer process without the OS sandbox. 0C.06 should measure transport (b) against the
`draw=preload` + `policy=clock` numbers above, not against `draw=main`. (It did; see the
[A/B table](#ab-table-informal-this-machine).)

---

## WP-0C.02: WebGPU canvas, DOM overlay, scrub bar, live resize

### How it works

- **Who owns what.** The page (main world) owns the layout, the overlay, the scrub bar and the
  controls. The preview loop owns the canvas pixels; it runs in the preload's isolated world
  (`draw=preload`, the WP-0C.01 recommendation), so only small calls cross `contextBridge`:
  `scrub(pts, inputTime)`, `setUpload`, `setEngineSize`, and `presented()` once per frame for
  the playhead. `draw=main` still works for comparison.
- **Stage.** The page sizes the stage to the largest 16:9 box that fits, in whole device
  pixels with width % 4 == 0 and an even height (the engine's NV12 constraint). Canvas backing
  store, screen pixels and engine frame are then the same size (the HUD says `1:1`).
- **Presenter** (`shared/present.ts`). WebGPU, upload path switchable at run time:
  `queue.writeTexture` of RGBA8, `queue.writeTexture` of the NV12 Y and UV planes plus a BT.709
  YUV->RGB shader, or `new VideoFrame(pixels, {format: "RGBA" | "NV12"})` +
  `device.importExternalTexture`. Frames of any size are scaled to the canvas; `redraw()`
  repaints the last frame.
- **Live resize** (`shared/stage.ts`). A `ResizeObserver` on the canvas
  (`device-pixel-content-box`) sets the backing store, redraws the last frame in the same
  rendering step (a resized canvas is cleared; this keeps the cleared one from ever being
  composited) and sends `resize {width, height}` to the engine. The engine re-creates its
  render target (readback buffers are sized once for the ring's maximum, 2560x1440) and the
  next frame has the new size; frames in flight at the old size are scaled meanwhile.
- **Scrub** (`renderer/scrub.ts`, `engine/src/scrub.rs`). Each dispatched `pointermove`
  (Chromium aligns them to animation frames) sends one latest-wins seek, skipped when the frame
  did not change, stamped with the earliest coalesced input event time. The engine blocks on
  its command channel between frames, so a seek wakes it at once. While paused it renders a
  half-size preview first (slot `flags` bit 0, `LOWRES`) and the full-resolution frame from an
  emulated decoder: one decode at a time, `full_cost_ms` each (0 by default; the synthetic
  frame takes about 1 ms), and a newer request replaces a waiting one. **Scrub-to-pixel** runs
  from the input event to the start of the animation frame after the matching frame's
  `queue.submit`, when the compositor takes the canvas (display scan-out is not included). A
  frame also answers older requests still waiting, each with its own, longer, latency.
- **Overlay** (`renderer/overlay.ts`). A transform box in normalised sequence coordinates, laid
  out in percentages of the overlay (which has exactly the canvas's box), so it needs no JS to
  stay aligned through resizes. It has 8 `hit-min` handles and arrow-key nudging. Safe areas
  are 93 % (action) and 90 % (title) plus a centre cross, in `overlay-guide` with a 1 px
  `overlay-guide-shadow` outline. The selection is marigold and focus uses `focus-ring`. Guides
  and the HUD are `pointer-events: none`. All colours and sizes are tokens (`tokens.css` is
  copied from `packages/ui` at build time).
- **Benchmark** (`desktop/src/bench.ts` suite mode, `pnpm bench`). It drives real input through
  `webContents.sendInputEvent` and runs these phases: a playback matrix at a fixed 1920x1080
  engine size ({RGBA8, NV12} x {writeTexture, VideoFrame}), playback at the canvas size with
  the fastest path, scripted scrubs (drags at 2/6/15/40 px per move plus click jumps) in four
  configurations, a live resize sweep (11 sizes plus a drag-like sweep with a size change every
  ~16 ms) with a pixel check on captured frames, and overlay hit tests and drags. It writes
  per-second metrics `{ts, phase, fps, dropped, engineDropped, scrub_ms[], cpu{engine, main,
  renderer, gpu}, align}` plus summaries and checks to `results/bench-*.json`.

### WP-0C.02 acceptance criteria

| Criterion | Status | Evidence |
|---|---|---|
| WebGPU canvas presents the newest slot with `pts <= audio clock` in rAF, `writeTexture` and `VideoFrame` -> `importExternalTexture` behind a toggle | Done | `shared/present.ts`; `policy=clock`; Upload button / `--upload` |
| DOM overlay above the canvas: draggable/resizable transform box (pointer events), safe-area guides, scrub bar sending latest-wins seeks | Done | `renderer/overlay.ts`, `renderer/scrub.ts`; bench interactions and hit tests |
| Live resize: canvas pixel size sent to the sidecar, which re-renders at the new size | Done | `shared/stage.ts`, engine `resize`; bench resize steps; addon end-to-end test |
| Instrumentation: present timestamps, dropped frames, scrub-to-pixel, CPU of engine, main and renderer every 1 s to JSON | Done | `results/bench-*.json` `metrics[]` |
| Overlay stays pixel-aligned during resize and playback (visual check + DOM-rect vs canvas-rect assertion every second) | PASS | `checks.overlayAlignedEverySecond`, `resize.steps[].align`; screenshots |
| Both upload paths run 60 s on the dev machine and emit metrics JSON | PASS | `pnpm bench 60` result below |
| `FLAGS.md` with the Linux flags | Partial | [FLAGS.md](FLAGS.md): only Windows was run; the Linux and macOS rows are unverified starting points for WP-0C.04 |
| Test: Playwright-Electron smoke asserting the metrics file | Deviation | as in WP-0C.01, the checks run from Electron main (`pnpm bench quick` always writes the JSON and exits non-zero on failure); the addon end-to-end test covers `resize`, low-res-first seeks and `scrub-config` |

Other deviation: the WBS names a React/Vite renderer. The spike keeps WP-0C.01's plain
TypeScript + esbuild page, because React adds nothing to what is measured. WP-0C.01 finding 2
still applies: React can own the canvas element and its layout, but the privileged preload
owns the GPU upload.

### WP-0C.02 informal numbers (dev machine, NOT the WP-0C.04 measurement)

> Same machine as above (Ryzen 9 5900HX, Windows 11, engine on the RTX 3080 via Vulkan,
> Chromium's WebGPU on the AMD iGPU, display scale 100 %), Electron 45.4.1, 1 run per
> configuration, verification on, `draw=preload`, `policy=clock` (20 ms). **The machine was
> shared:** other agents' spike engines, Electron windows and compiles ran during parts of
> these runs. Rows marked `(contended*)` had another `s1-engine` running. Raw JSON:
> `results/bench-win32-*.json`; tables from `node scripts/summarize-bench.mjs`.

**Run A: `pnpm bench 60`** (60 s per playback phase, 12 s per scrub variant;
`results/bench-win32-1795368821621.json`). It fulfils the "both upload paths run 60 s" criterion.

| format + upload (engine 1920x1080, canvas 1252x706) | fps | drops/min (seq gaps + engine) | upload+draw CPU p50/p95 ms | submit -> GPU done p50/p95 ms | engine render -> present submit p50/p95 ms | CPU % of a core: engine / renderer / GPU proc |
|---|---|---|---|---|---|---|
| rgba8 + writeTexture | 59.7 | 14.9 (9.0 + 6.0) | 0.8 / 1.3 | 6.3 / 7.8 | 21.9 / 23.2 | 10.8 / 22.4 / 27.6 |
| **nv12 + writeTexture** (fastest) | 59.4 | 35.8 (22.9 + 12.9) | 1.4 / 2.2 | 4.2 / 5.2 | 21.1 / 22.4 | 7.0 / 24.6 / 22.0 |
| rgba8 + videoFrame | 45.3 | 882.5 (395.7 + 486.9) | 4.6 / 6.7 | 11.1 / 19.7 | 26.1 / 48.8 | 16.9 / 66.5 / 37.6 |
| nv12 + videoFrame (contended*) | 50.2 | 587.3 (261.0 + 326.4) | 1.9 / 4.8 | 12.6 / 21.6 | 36.6 / 41.2 | 12.7 / 35.8 / 43.4 |

Canvas-size playback (contended*), engine at the canvas size 1252x706, nv12 + writeTexture:
59.8 fps, 9.9 drops/min, renderer CPU 21.1 %.

| scrub (paused, real pointer drags + clicks) | requests | scrub-to-pixel p50/p95 ms | full-res on screen p50/p95 ms | low-res frames shown | rAF wait p50/p95 ms | lost |
|---|---|---|---|---|---|---|
| **low-res first, full-res cost 0 ms (default)** | 311 | **41.3 / 49.5** | 41.3 / 49.5 | 3 | - | 0 |
| low-res first, full-res cost 50 ms (emulated decode) | 322 | 44.8 / 79.4 | 107.2 / 138.7 | 224 | - | 0 |
| full-res only, cost 50 ms | 330 | 109.3 / 133.3 | 109.3 / 133.3 | 0 | - | 0 |
| low-res first, cost 0 ms, rAF waits <= 8 ms (opt-in) | 322 | 29.8 / 42.2 | 41.2 / 49.0 | 208 | 1.6 / 2.0 | 0 |

- **Resize (contended\*):** 119 canvas resizes, 0 blank (instrumented), 83 captured frames
  checked with 0 blank (56 more were discarded because the window moved during the capture).
  Engine renegotiation, from the size request to the first full-resolution frame at the new
  size, took 32.3 / 33.4 ms p50 / p95. The engine renegotiated at every step (12 of 12), and
  playback ran at 58.0 fps during the sweep.
- **Overlay:** 22 of 22 hit tests and 6 of 6 real-input interactions passed. All 364
  per-second alignment checks passed (overlay vs canvas 0 device px, box vs model at most
  0.013 px, backing store vs box 0 px), and none needed the re-measure after a frame.
- **Checks:** every check passed except `playback60fps` and `dropsUnder2PerMin` (finding 2).

**Run B: `pnpm bench 20`** (20 s per playback phase; `results/bench-win32-1795369074193.json`).
The first two phases ran with nothing else on the machine; then another engine started.

| format + upload (engine 1920x1080) | fps | drops/min (gaps + engine) | upload+draw CPU p50/p95 ms | submit -> GPU done p50/p95 ms | engine render -> present submit p50/p95 ms | CPU % engine / renderer / GPU proc |
|---|---|---|---|---|---|---|
| rgba8 + writeTexture | **60.0** | **0.0** | 0.9 / 1.1 | 5.4 / 6.6 | 25.7 / 26.6 | 9.1 / 21.6 / 26.2 |
| **nv12 + writeTexture** (fastest) | **60.0** | **0.0** | 1.3 / 2.0 | 4.0 / 5.2 | 25.1 / 26.2 | 5.9 / 21.8 / 20.3 |
| rgba8 + videoFrame | 57.8 | 134.9 (30.0 + 104.9) | 3.0 / 4.1 | 6.6 / 8.7 | 27.3 / 29.0 | 12.0 / 63.4 / 38.9 |
| nv12 + videoFrame (contended*) | 59.1 | 50.9 (9.0 + 42.0) | 0.9 / 2.2 | 6.6 / 9.8 | 24.4 / 26.5 | 7.6 / 26.3 / 36.7 |

In run B every scrub variant was contended. The default scrub configuration had a p95 of
179.5 ms (p50 43.8), which fails its check; the low-res/50 ms variant gave 44.1 / 76.6 ms and
the rAF-wait variant 29.7 / 44.2 ms. Resize again gave 0 blank frames (121 resizes, 96
captures), renegotiation at 49.1 / 50.5 ms, and 12 of 12 steps. The overlay checks all passed.

### WP-0C.02 findings (informal, one machine)

1. **Upload path: keep NV12 + `queue.writeTexture` + the YUV shader.** It had the lowest total
   cost in every run: about 1.4 ms of CPU for upload and draw, 4.2 ms from submit to GPU done,
   and 3.1 MB per frame against 8.3 MB for RGBA8 through every copy (engine memcpy, addon copy,
   upload). RGBA8 `writeTexture` uses the least CPU (0.8 ms) but more GPU time (6.3 ms).
   `VideoFrame` + `importExternalTexture` is the slowest path for frames that start in CPU
   memory, because `new VideoFrame(ArrayBuffer)` adds a copy and Chromium uploads in the GPU
   process. RGBA8 `VideoFrame` used about 66 % of a core in the renderer (writeTexture: 22 %),
   took 11 ms to GPU done and could not hold 60 fps (33-60 fps across runs). NV12 `VideoFrame`
   is cheap on CPU but takes 2-3x as long on the GPU. `VideoFrame` only pays off when the source
   already is one (WebCodecs decode, GPU-backed frames). The renderer defaults to
   `writeTexture` and `pnpm bench` picks the cheapest path itself. Use `--format=nv12`
   interactively; main's default stays RGBA8, so WP-0C.01 and WP-0C.06 numbers stay comparable.
2. **Playback is at 60 fps, with sporadic drops on this shared machine.** With nothing else
   running, both `writeTexture` paths presented 60.0 fps with 0 drops (run B, 20 s each), as
   WP-0C.01's 60 s run did. In run A, the 60 s phases had 10-36 drops/min at 59.4-59.8 fps,
   which fails the < 2 drops/min criterion. Those drops come in single seconds (1-7 frames) and
   coincide with producer-side stalls (engine drops) while other agents' work ran on the
   machine. The per-second fps values swing between 47 and 75 because the main-process sampler
   itself was delayed. WP-0C.04 decides on a quiet reference machine. Risk to carry: the 20 ms
   clock-scheduling margin leaves little headroom for producer hiccups. WP-0C.04 should also
   try `--latency=33` (one more frame of latency) and a higher-priority engine render thread.
3. **Scrub-to-pixel p95 is about 50 ms against the 80 ms criterion.** It was 49.5 ms in run A
   and 49-49.5 ms in every uncontended run. A run with another engine on the GPU (run B) had a
   179.5 ms tail, so WP-0C.04 must measure on a quiet machine. Anatomy: the input event
   waits up to one frame for dispatch (Chromium aligns pointer moves to frames), the engine
   renders, reads back and publishes in about 3-5 ms, the frame misses the current rAF and is
   drawn in the next one (one frame), then the compositor takes it (one frame).
   - **Low-res first is what keeps 80 ms with real decode costs.** With an emulated 50 ms
     full-resolution decode, the half-size preview gives 45 ms p50 / 79 ms p95, against
     109 / 133 ms without it. That p95 sits right at the budget, so arch §10 step 5
     (proxies / low-res first) is required, not optional.
   - **Waiting in rAF saves a frame.** With `scrubWaitMs` (opt-in), the rAF callback waits up
     to 8 ms for the frame of a seek sent in the same frame. That gives about 30 / 42 ms and
     cost about 1.6 ms of main-thread waiting per scrub frame. It is worth adopting with a
     budget in WP-1H.
   - Latest-wins worked: no request was lost in about 1,300 per run. Fast drags answer many
     requests with one frame, which the conservative per-request latency includes.
4. **Live resize is clean in the page.** 119 canvas resizes produced 0 blank frames: every
   resize had a frame to redraw in the same rendering step, and none of 83 captured frames was
   blank. The engine renegotiated the size at every step (12 of 12). The first
   full-resolution frame at the new size arrived after 32-50 ms (p50 / p95 32 / 33 ms in run A,
   49 / 51 ms in run B; two to three frames), and the presenter scaled the old frame until
   then. The stage snapping gave a 1:1 canvas-to-frame
   mapping after every step. Not measured:
   - window-level artifacts outside the page (DWM gutters while an edge is dragged by hand);
   - fractional DPR (this machine runs at 100 %; 125 % and 150 % are for WP-0C.04).

   The ring must be sized for the largest canvas (88 MB for 6 slots at 2560x1440); larger
   canvases are clamped and scaled.
5. **No overlay or z-order issues on Windows.** 22 of 22 hit tests passed: the canvas never
   receives pointer events, guides and HUD are click-through, and every handle is hit. 6 of 6
   real-input interactions worked (move, two resizes, keyboard nudge, deselect/select, scrub
   click). The overlay stayed within 0.5 device pixels of the canvas, and the transform box
   within 1 pixel of its model, at every 1 s check, including during the resize sweep. The
   overlay is ordinary DOM above a WebGPU canvas in the same page (arch §10 step 4).
6. **WP-0C.01 bugs found and fixed here.**
   - Pings were keyed by fractional timestamps that do not always survive serde_json's default
     float parsing, so pongs went unmatched.
   - The control socket never reconnected after the engine dropped a connection whose auth
     arrived after 2 s, which can happen while WebGPU starts up on a busy main thread. One
     early bench run lost every seek and resize this way. It reconnects now.
   - The preload's per-size frame buffers grew without bound under live resize.
   - The addon end-to-end test hung the test runner when an assertion failed.
7. **Measurement hygiene.** The bench marks phases during which another `s1-engine` ran
   (`(contended*)` rows, `notes`). Trust phase totals over per-second rows. Chromium's WebGPU
   ran on the AMD iGPU and the engine on the RTX 3080, as in WP-0C.01.
8. **Open issue: intermittent "no frames" in the WP-0C.01 self-test.** 3 of about 20 runs
   failed in one 6-minute window while other agents' spike runs were active; the next 10
   consecutive runs passed.
   - Two failures stopped at the restart check: after `kill -9` and the restart, the new engine
     was up and the control socket re-authenticated, but the preload read 0 frames from the
     new ring in 20 s. The new ring's header showed an implausible `engine_dropped` of about
     4.2e10, roughly 59.6 s worth of flicks.
   - One failure had no rAF callbacks at all for 8 s before the crash step. A plain Electron
     page ran rAF at 61/s at the same time.

   The root cause was not found. WP-0C.04 and WP-0C.06 should watch for it: on a failure, dump
   `bridge.readHeader()` and `bridge.stats()`.

**Bottom line for WP-0C.04 / ADR-001 (informal):** the in-page parts of S1 hold on Windows.
The WebGPU canvas presents 1080p60 with NV12 + `writeTexture`. Scrub-to-pixel p95 is about
50 ms, and low-res-first keeps it under 80 ms when full frames cost 50 ms. Live resize
renegotiates the engine size in two to three frames with no blank frame. The DOM overlay has no
alignment, hit-testing or z-order issues. Open for WP-0C.04: drops/min on a quiet reference
machine (sporadic producer stalls here), fractional DPR, window-level resize artifacts,
Linux/macOS flags ([FLAGS.md](FLAGS.md)), and whether to adopt the rAF wait for scrubs.

---

## WP-0C.06: transport (b), A/B against (a), and a GPU-texture probe

**Question.** Can a **fully sandboxed** editor renderer (`sandbox:true`, `contextIsolation`, no
Node, no native code in the process) still get 1080p60 with acceptable latency and CPU?

**Short answer (informal, this machine).** Not with pixels over a `MessagePort`. Transport (b)
works and passes every functional and sandbox check. But Electron structured-clones every frame
(transfer is impossible), so (b) costs about 4 CPU copies per frame instead of 1. At 1080p
RGBA8 that is about 1.2-1.7 cores: renderer 64-98 % plus utility 57-72 % of one core,
against 14-32 % for (a)'s renderer.

Over four 60 s runs, (b) presented 1080p RGBA8 at 42-60 fps with 2-1055 skipped frames/min
(median about 540). Its best run, 59.97 fps with 2 skipped/min, was the (b) run with the least
foreign load. In the same conditions, (a) held 59.9-60.0 fps with 0-6 skipped/min in every run.

So a sandboxed renderer *can* hit 60 fps over a MessagePort, but not reliably, and not
within the "< 15 % transport CPU" budget. It *can* get 1080p60 cheaply over **GPU shared
textures**, via Electron 45's experimental `sharedTexture` module. The probe below delivered
every frame its source produced (58-60 fps) into a sandboxed renderer. It used about 13 %
renderer CPU, no CPU pixel copies, and about 0.5 ms from main to the page.

### What transport (b) is

```text
 Electron main                                     frame-pump utilityProcess (Node + addon)
 ─────────────                                     ───────────────────────────────────────
 engine supervisor (unchanged) ── engine-up ─────▶ RingReader.open(R); WS control (token here)
   {info, token} / engine-down ─────────────────▶ waitForFrame() poll thread -> readSlotInto
   (token never leaves main + utility)             -> port.postMessage({h, clock, pixels})
 MessageChannelMain per page request:                 ArrayBuffer is structured-cloned
   port1 ───────────────────────────────────────▶    (MessagePortMain transfers ports only)
   port2 ──▶ preload-b (SANDBOXED) ──┐                         │ Mojo, utility -> renderer
             window.postMessage(port) ▼                        ▼  directly (not via main)
 Main editor window, sandbox:true. The page's main thread (--draw=main) or a dedicated worker
 with an OffscreenCanvas (--draw=worker) runs FrameReceiver (ack-based credit window of 3,
 clock offset to the host clock estimated over the port) -> the same PresentLoop, presenters
 and checksum/row-stamp checks as (a), with policy=clock against the engine audio clock.
```

Files: `desktop/src/transport-b/` (`main-b.ts` main-side wiring and self-test, `utility.ts`
frame pump, `preload.ts` sandboxed port-handoff preload, `receiver.ts`, `renderer.ts`,
`worker.ts`, `protocol.ts`), `desktop/src/bench.ts` (`ab` mode, shared with
the WP-0C.02 suite), `bridge/src/wait.rs` (`RingReader.waitForFrame`, with Node tests in
`bridge/test/wait.test.mjs`), `scripts/build-js-b.mjs`, `scripts/bench-ab.mjs`. Shared files
only gained additive hooks: the `--transport` option and its dispatch in `main.ts`, one method
in the addon, and the `build:js` chain. Transport (a) and its renderer UI files are unchanged.

```sh
pnpm build
pnpm start -- --transport=utility [--draw=main|worker]    # (b) with HUD and controls
pnpm selftest 60 --transport=utility --policy=clock        # (b) checks -> results/selftest-*-utility-*
pnpm bench ab 60 --transport=utility [--draw=worker]       # one A/B run -> results/bench-*.json
node scripts/bench-ab.mjs 60                               # A/B matrix -> results/bench-*.json + tables
node scripts/bench-ab.mjs --table                          # tables from existing results
pnpm start -- --transport=texture [--policy=latest]        # sharedTexture GPU probe
```

### Self-test for (b) (`--transport=utility --selftest=60`)

The same checks as (a), plus the WP-0C.06 sandbox checks. Evidence:
`results/selftest-win32-utility-rgba8-main-clock-gpu-1795370816032.json`, all 15 checks PASS.
It ran at 51 fps under about 7 cores of foreign load: the run checks correctness, not speed.

| Check | Result |
|---|---|
| Main world: `typeof require`, `typeof process`, `typeof window.bridge` are all `undefined`; frames still arrive | PASS |
| Preload: `process.sandboxed === true`; `require("fs")`, `require("child_process")` and the `.node` addon all throw `module not found`; no `process.dlopen` | PASS |
| OS view (`app.getAppMetrics()`): editor renderer `sandboxed: true`, integrity `untrusted`; frame-pump utility `sandboxed: false`, integrity `medium` | PASS (recorded) |
| A second window (sandboxed, no preload) has no bridge or port | PASS |
| 60 s run: monotonic `seq`, zero torn frames (checksum + row stamps verified in the renderer) | PASS |
| `kill -9` of the engine detected in < 1 s; the utility closes its view and acknowledges; `OpenFileMappingW` then fails | PASS: exit after 152 ms, ring released after 184 ms. One earlier run under heavy load took 2.47 s (finding 9). |
| Engine restarts and frames flow again | PASS |
| Probes: `postMessage(msg, [arrayBuffer])` on MessagePortMain | throws `Port at index 0 is not a valid port` |
| Probes: posting a `SharedArrayBuffer` utility -> renderer (page is cross-origin isolated: `crossOriginIsolated === true`) | throws `An object could not be cloned.` |
| Probes: `latestView().zeroCopy` in the utility (external ArrayBuffer over the mapping) | `false` (Electron's V8 cage, as in the preload) |

### Copies per frame (1080p RGBA8 = 8.3 MB)

| Path | CPU copies of the pixels | Where | Measured cost (p50, this machine) |
|---|---|---|---|
| (a) addon in preload, loop in preload | 1 | shm -> preload heap (napi); then `writeTexture` | 0.8 ms |
| (a) loop in the main world | 2 | + `contextBridge` structured clone on the renderer main thread | + 4.8 ms |
| (b) utility + MessagePort | 4 | shm -> utility heap (napi); V8 serialization + Mojo `BigBuffer` shared memory in `postMessage`; Blink deserialization into a fresh ArrayBuffer in the renderer; then `writeTexture` | 0.6-1.1 + 7.7-10.4 + 1.1-2.1 ms (copy + `postMessage` + `ev.data`), plus GC of about 500 MB/s |
| sharedTexture probe | 0 | GPU texture handle only; one GPU-to-GPU copy into the page's texture ring | GPU copy call 0.2 ms CPU |

The utility's `postMessage` cost does not shrink with chunking. A probe that posted 8.3 MB as
1, 4, 16, 64, 128 or 256 messages measured 11.3, 11.5, 9.1, 8.6, 7.0 and 6.4 ms per frame on
the utility side. The renderer's receive time grew from 1.8 to 4.6 ms and its CPU from 23 to
58 % at 30 fps. Allocating fresh multi-MB buffers (and page-faulting them in) every frame, on
both sides, is a large part of it.

### A/B table (informal, this machine)

> **Informal numbers on a shared, busy machine.** Same laptop as above: Ryzen 9 5900HX
> (16 threads); the engine on the RTX 3080 Laptop (Vulkan); Chromium WebGPU on the AMD iGPU;
> Electron 45.4.1. Other agents compiled Rust and ran their own Electron/engine spikes
> throughout, so every row records its **foreign load**, and two rows of each core
> configuration are shown to give an idea of the variance.
>
> Mitigations, applied to every transport alike:
> - `bench-ab.mjs` waits (up to 90 s) for a quieter machine before each run;
> - all measured processes run at above-normal priority (`--bench-priority=above`);
> - per size, runs alternate (a), (b) on the main thread and (b) in a worker.
>
> Each row is one fresh launch: 5 s warm-up, then 60 s measured, RGBA8 unless stated, with
> clock-based frame choice. Raw JSON is in `results/bench-*.json`; the table comes from
> `node scripts/bench-ab.mjs --table`. Earlier runs under heavier load (up to 14 foreign
> cores) showed the same pattern, only worse; they were discarded and not committed.
> WP-0C.04 should redo this on the reference machines.

| size / format | transport / loop in | frame choice | s | present fps | skipped /min | engine drops | submit -> draw p50 / p95 ms | CPU % renderer | CPU % utility | CPU % GPU / main / engine | foreign load | torn |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1280x720 rgba8 | (a) preload | clock 20 ms | 60.3 | 60.00 | 0.0 | 0 | 34.6 / 35.4 | 19.4 | n/a | 16.9 / 0.3 / 7.6 | 437.8 | 0 |
| 1280x720 rgba8 | (a) preload | clock 20 ms | 60.7 | 60.00 | 0.0 | 0 | 25.9 / 26.8 | 16.8 | n/a | 15.3 / 0.2 / 8.9 | 302.6 | 0 |
| 1280x720 rgba8 | sharedTexture probe main-thread | next | 60.0 | 59.98 | 1.0 | - | 30.8 / 32.2 | 13.2 | n/a (OSR source 5.6) | 36.8 / 7.9 / - | 525.5 | 0 |
| 1280x720 rgba8 | (b) main-thread | clock 20 ms | 60.3 | 55.32 | 281.5 | 0 | 34.5 / 36.1 | 52.8 | 28.7 | 18.5 / 0.8 / 9.3 | 694.1 | 0 |
| 1280x720 rgba8 | (b) main-thread | clock 20 ms | 60.3 | 54.88 | 306.3 | 0 | 21.9 / 37.6 | 42.8 | 23.7 | 15.8 / 0.7 / 9.0 | 275.9 | 0 |
| 1280x720 rgba8 | (b) worker | clock 20 ms | 60.4 | 54.04 | 357.4 | 0 | 34.6 / 36.2 | 56.0 | 32.4 | 22.0 / 0.8 / 9.3 | 656.7 | 0 |
| 1280x720 rgba8 | (b) worker | clock 20 ms | 60.3 | 59.42 | 34.8 | 0 | 22.3 / 26.5 | 46.5 | 26.0 | 18.5 / 0.7 / 8.6 | 384.1 | 0 |
| 1920x1080 nv12 | (a) preload | clock 20 ms | 60.4 | 60.01 | 0.0 | 0 | 31.6 / 32.7 | 19.8 | n/a | 18.7 / 0.2 / 9.4 | 858.4 | 0 |
| 1920x1080 nv12 | (b) worker | clock 20 ms | 60.3 | 57.90 | 126.3 | 0 | 30.1 / 33.2 | 40.3 | 20.1 | 14.6 / 0.6 / 7.3 | 660.1 | 0 |
| 1920x1080 rgba8 | (a) preload | clock 20 ms | 61.6 | 59.90 | 5.8 | 0 | 34.5 / 39.0 | 31.9 | n/a | 36.5 / 0.2 / 19.4 | 948.6 | 0 |
| 1920x1080 rgba8 | (a) preload | clock 20 ms | 60.3 | 60.00 | 0.0 | 0 | 29.8 / 30.5 | 13.9 | n/a | 19.0 / 0.1 / 7.3 | 136.7 | 0 |
| 1920x1080 rgba8 | sharedTexture probe main-thread | next | 60.0 | 58.18 | 4.0 | - | 25.0 / 27.8 | 13.4 | n/a (OSR source 7.1) | 37.5 / 6.4 / - | 614.3 | 0 |
| 1920x1080 rgba8 | (b) main-thread | clock 20 ms | 60.4 | 46.01 | 839.1 | 0 | 29.7 / 41.6 | 67.1 | 56.5 | 22.1 / 0.8 / 12.6 | 441.4 | 0 |
| 1920x1080 rgba8 | (b) main-thread | clock 20 ms | 60.4 | 59.97 | 2.0 | 0 | 28.1 / 33.9 | 63.9 | 58.7 | 23.2 / 0.8 / 10.6 | 239.2 | 0 |
| 1920x1080 rgba8 | (b) main-thread | clock 35 ms | 60.6 | 58.08 | 114.8 | 0 | 39.0 / 50.1 | 77.8 | 63.6 | 31.1 / 1.0 / 14.4 | 471.2 | 0 |
| 1920x1080 rgba8 | (b) worker | clock 20 ms | 60.7 | 42.35 | 1055.3 | 1 | 33.3 / 51.7 | 97.7 | 71.5 | 28.4 / 1.0 / 15.9 | 876.9 | 0 |
| 1920x1080 rgba8 | (b) worker | clock 20 ms | 60.4 | 55.93 | 244.3 | 0 | 29.4 / 41.1 | 73.7 | 68.3 | 26.5 / 1.1 / 14.1 | 396.4 | 0 |
| 1920x1080 rgba8 | (b) worker | clock 35 ms | 60.6 | 56.53 | 209.1 | 0 | 42.0 / 49.6 | 71.0 | 58.1 | 26.4 / 1.0 / 12.6 | 417.4 | 0 |

Transport (b) stages, p50 / p95 ms (from the same runs):

| (b) size / format / loop | publish -> utility wake | shm -> heap copy | postMessage (serialize + BigBuffer) | renderer `ev.data` deserialize | publish -> renderer receipt | credit drops | rx queue drops |
|---|---|---|---|---|---|---|---|
| 1280x720 rgba8 main-thread clock 20 ms | 0.33 / 0.74 | 0.27 / 0.47 | 3.07 / 3.92 | 0.53 / 0.79 | 6.60 / 10.63 | 0 | 0 |
| 1280x720 rgba8 main-thread clock 20 ms | 0.43 / 0.93 | 0.31 / 0.68 | 3.65 / 5.82 | 0.71 / 1.43 | 7.95 / 14.97 | 4 | 12 |
| 1280x720 rgba8 worker clock 20 ms | 0.38 / 0.82 | 0.30 / 0.54 | 3.33 / 4.79 | 0.54 / 0.94 | 7.16 / 14.83 | 0 | 0 |
| 1280x720 rgba8 worker clock 20 ms | 0.45 / 0.94 | 0.45 / 0.94 | 4.10 / 6.03 | 0.73 / 1.31 | 8.75 / 21.36 | 5 | 14 |
| 1920x1080 nv12 worker clock 20 ms | 0.33 / 0.66 | 0.22 / 0.32 | 2.33 / 3.46 | 0.46 / 0.73 | 5.16 / 11.09 | 0 | 3 |
| 1920x1080 rgba8 main-thread clock 20 ms | 0.34 / 0.63 | 0.58 / 0.96 | 8.58 / 10.81 | 1.13 / 1.68 | 15.10 / 21.39 | 1 | 0 |
| 1920x1080 rgba8 main-thread clock 20 ms | 0.40 / 0.83 | 0.75 / 1.48 | 7.69 / 12.05 | 1.22 / 2.02 | 14.54 / 27.75 | 3 | 0 |
| 1920x1080 rgba8 main-thread clock 35 ms | 0.43 / 1.49 | 0.90 / 1.88 | 8.59 / 13.89 | 1.46 / 2.77 | 17.07 / 39.62 | 68 | 0 |
| 1920x1080 rgba8 worker clock 20 ms | 0.40 / 1.06 | 0.69 / 1.59 | 9.68 / 13.76 | 1.36 / 2.31 | 18.54 / 36.71 | 36 | 34 |
| 1920x1080 rgba8 worker clock 20 ms | 0.48 / 3.17 | 1.06 / 2.17 | 10.35 / 15.14 | 2.13 / 3.76 | 27.43 / 54.45 | 141 | 320 |
| 1920x1080 rgba8 worker clock 35 ms | 0.38 / 0.89 | 0.74 / 1.56 | 7.92 / 11.62 | 1.29 / 2.52 | 16.22 / 31.51 | 8 | 10 |

How to read: **skipped/min** counts frames the engine published that were never presented
(seq gaps). **engine drops** are frames the engine itself skipped during the run: producer
stalls, not transport. **submit -> draw** runs from the engine's GPU submit to the renderer's
`queue.submit` (compositor and scan-out not included). **CPU** is % of one core per process over
the run. **foreign load** is whole-machine CPU minus our processes, in % of one core: other
agents' builds and spikes shared this 16-thread laptop during the runs. Rows with high foreign
load are noisy for every transport.

### sharedTexture probe (`--transport=texture`)

Electron 45.4.1 ships `sharedTexture` (typed `@experimental`).
`importSharedTexture` and `sendSharedTexture` run in main. `setSharedTextureReceiver` **is
available in a sandboxed preload**; the self-test lists `sharedTexture` among the sandboxed
preload's modules.

The probe uses an offscreen window with `useSharedTexture` (60 fps canvas animation) as a
stand-in GPU frame source. Main imports each `paint` texture and sends it to a fully sandboxed
window. The preload turns it into a `VideoFrame` and transfers it to the page with
`window.postMessage(..., [videoFrame])`. The page copies it GPU-to-GPU into a 4-texture WebGPU
ring (`copyExternalImageToTexture`) and closes it at once, so the source texture is released
immediately; rAF presents from the ring with a 2-frame FIFO.

Results: 60 s each, the same bench and machine as the table above, with 5-6 cores of foreign
load. Sizes are the OSR source's.

| size | present fps | skipped /min | source paints/s | main `sendSharedTexture` -> `VideoFrame` in page p50 / p95 ms | paint (main) -> present submit p50 / p95 ms | CPU % renderer / main / GPU / source | import + send in main (mean ms) |
|---|---|---|---|---|---|---|---|
| 1920x1080 | 58.18 | 4.0 | 58.4 | 0.49 / 0.83 | 25.0 / 27.8 | 13.4 / 6.4 / 37.5 / 7.1 | 0.09 + 0.63 |
| 1280x720 | 59.98 | 1.0 | 60.0 | 0.55 / 0.83 | 30.8 / 32.2 | 13.2 / 7.9 / 36.8 / 5.6 | 0.11 + 0.68 |

`paint -> present` is mostly the 2-frame FIFO (latest-wins: about 2-10 ms, with more skips
against the free-running source). Renderer CPU is about 13 % with **no pixel copy on the CPU**:
the per-frame work is a GPU `copyExternalImageToTexture` (0.2 ms of CPU to issue) and a draw.
Main pays about 0.7 ms per frame for import and send, against (b)'s 8-10 ms `postMessage` in the
utility.

What the probe does **not** cover:
- The engine-side export: wgpu -> D3D11/D3D12 NT handle, IOSurface or DMA-BUF.
- Same-adapter selection on hybrid GPUs (WP-0C.01 finding 5). The OSR source lives in Chromium's
  own GPU process, so it is on Chromium's adapter by construction.
- Linux and macOS.
- Pts scheduling. The probe has no audio clock; production would schedule on pts exactly like
  (a) and (b).

Releasing the imported texture before the page was done with the `VideoFrame` produced
`GPU state invalid` errors and stalled the source after a few frames. The lifetime rule matters:
release only after the frame is closed or copied.

### Security comparison

| | (a) addon in the editor's preload | (b) addon in a utility process + MessagePort | GPU textures (`sharedTexture`) |
|---|---|---|---|
| Editor renderer OS sandbox | **off** for the whole renderer process (`sandbox:false`): measured `sandboxed: false`, integrity `medium` | **on**: `sandboxed: true`, integrity `untrusted` | on (same as (b)) |
| Node / native code in the editor renderer process | Node in the preload's isolated world; the addon and a read-only view of the ring are mapped into the renderer | none (sandboxed preload: no `fs`, `child_process`, `.node`, `dlopen`) | none |
| What a renderer compromise (e.g. a V8 bug reached from page JS) gets | the user's privileges directly; no sandbox escape needed | a sandboxed, untrusted-integrity process; needs a second (sandbox-escape) bug. It can also send messages to the utility over its port (small protocol; the utility validates every field before anything reaches the engine) | same as (b) |
| Where native code and shared memory live | in the web-content process | in a utility process that never runs web content (not sandboxed, integrity `medium`) | main imports texture handles; the engine exports them |
| UI token | preload closure (isolated world) | main and the utility only, never in the renderer | n/a (frames need no WebSocket) |
| Arch §18 rule "every window sandboxed except the main editor window's preload" | needs that exception | no exception | no exception |
| Extra moving parts | none | one more process, port handoff, clock sync, credit and ack protocol | experimental Electron API; engine-side GPU export; adapter matching |

### Findings (WP-0C.06)

1. **The utility process can host the napi addon** (Electron 45, Windows). The addon loads
   unchanged, and `RingReader` works as in the preload. Its V8 is caged too, so
   `latestView().zeroCopy` is `false` and reading costs one copy, same as (a). Electron's main
   process could host it as well. It would pay the same `MessagePortMain` serialization, but on
   the browser process's main thread, so it was not pursued.
2. **`MessagePortMain` cannot transfer `ArrayBuffer`s, and a `SharedArrayBuffer` cannot cross
   processes.** `postMessage(msg, [arrayBuffer])` throws "Port at index 0 is not a valid port",
   because only `MessagePortMain` objects are transferable. Pixels are therefore always
   structured-cloned. Serving `app://s1` with COOP/COEP makes the page cross-origin isolated
   (`crossOriginIsolated === true`, `SharedArrayBuffer` available). But posting a SAB from the
   utility throws "An object could not be cloned.": a SAB never leaves its agent cluster. Both
   WBS options ("transferable ArrayBuffers from a pre-allocated pool", "SharedArrayBuffer")
   are unavailable across the utility -> renderer hop.
3. **Serialization dominates.** `postMessage` of an 8.3 MB frame takes 7.7-10.4 ms (p50) of
   utility CPU, independent of chunking: 3.1-4.1 ms at 720p, 2.3 ms for NV12. The renderer
   then deserializes it into a fresh ArrayBuffer (`MessageEvent.data`, 1.1-2.1 ms p50) and
   garbage-collects about 500 MB/s of ArrayBuffers. Renderer CPU rises accordingly (see the
   table). Since the utility's event loop is blocked while it posts, a single pump thread caps
   out at about 100 frames/s of 8 MB.
4. **Windows timers.** Node timers in the utility tick at about 15.6 ms, because Windows timer
   resolution is per process. A JS poll loop would therefore add up to a frame of latency.
   `RingReader.waitForFrame()` polls on a libuv thread with a high-resolution sleep and
   resolves a promise: publish -> utility wake p50 0.3-0.5 ms, p95 < 1 ms in most runs. The
   product bridge should get a real wake-up: an OS event or futex signalled by the engine
   (WP-1H.02).
5. **The renderer main thread vs a worker.** Neither variant was consistently better under
   this load. Main thread: 46-60 fps at 1080p. Worker: 42-56 fps. Renderer-process CPU is the
   same, because the bytes still arrive in that process. The worker variant (OffscreenCanvas
   plus worker rAF, in a sandboxed, cross-origin-isolated renderer) is still the right shape
   for (b): it keeps deserialization, verification and upload off the UI thread that React and
   the overlays (WP-0C.02) use.
6. **Why (b) skips.** Three effects add up:
   - At 1080p RGBA8, delivery (ring publish -> pixels in the renderer heap) is 15-27 ms p50 and
     21-54 ms p95. The clock scheduler's 20 ms target (tuned for (a)) is then too tight, and
     late frames become skips. Raising it to 35 ms gave 58.1 / 56.5 fps with 115 / 209
     skipped/min (main thread / worker): better, but nowhere near < 2/min, and +15 ms latency.
   - At 720p or NV12, delivery is fast (5-9 ms p50) but still jittery (p95 11-21 ms), and the
     renderer does 2-3 times more per-frame work than (a): deserialization and GC of 3-8 MB
     buffers. Worker runs had 29-314 long rAFs (> 25 ms) against 0-2 for (a).
   - Main-thread runs had few long rAFs but still skipped about 300/min at 720p. The clock
     assigns each frame a vsync slot, and a frame that arrives a few ms late misses it. This
     happens most when the engine-clock/vsync phase puts the slot boundary near the rAF: the
     306-skips/min run presented only 2 ms after its target, p50.

   (a) reads the ring at rAF time, so it has no delivery jitter and showed 0-6 skips/min. A
   product scheduler (WP-0C.02) should still adapt the latency target to keep that phase
   mid-frame, whatever the transport.
7. **Clock in a sandboxed renderer.** The page has no access to the engine's host clock.
   NTP-style pings over the same port give an offset good to about 0.02 ms, with a minimum
   RTT of 0.1-0.2 ms, which is enough for clock-based frame choice. One exception: when the
   utility was saturated, the best RTT was 9 ms and the offset off by about 2 ms; that run is
   the 1055-skips/min row. The product should sample the clock from shared memory or a
   dedicated channel, not behind multi-MB frames.
8. **napi-rs borrow tracking.** An `AsyncTask` returned from a `&self` method holds a shared
   borrow of the object until it resolves, so `&mut` methods (`readSlotInto`, `close`) throw
   "cannot be borrowed mutably" meanwhile. The utility awaits the pending wait before closing.
   The product bridge should make read methods `&self`.
9. **Crash detection is unchanged.** It is the same supervisor in main. Under heavy foreign
   load, one run took 2.47 s from `TerminateProcess` to the exit event; the stdout pipe also
   closed only then, during GPU-driver teardown. The utility now also reports a stale ring
   heartbeat (> 250 ms), measured as `crash.heartbeatDetectMs`. In the passing run the exit
   came first (152 ms), so it stayed `null`. Finding 6 of WP-0C.01 stands: use the heartbeat
   for hang and crash detection.
10. **Process cost.** (b) adds a utility process. Its CPU (57-72 % of one core at 1080p RGBA8,
    24-32 % at 720p, 20 % for NV12) is mostly serialization, which is exactly the work a GPU
    texture handoff removes.

### WP-0C.06 recommendation for ADR-001

ADR-001 adopted all four points (Accepted, provisional); the combined view is in
[ADR-001 input](#adr-001-input-summary).

1. **Do not ship transport (b) for full-resolution preview pixels.** It is functionally sound,
   and a fully sandboxed renderer is a real security win. But `MessagePortMain` cannot
   transfer buffers, so every frame is serialized and copied about 4 times. At 1080p RGBA8
   that costs more than a core across the utility and renderer processes, far outside the S1
   criterion "transport CPU overhead < 15 %". It also misses "< 2 dropped frames/min" on this
   machine. It narrows the gap at 1280x720 and with NV12 (fewer bytes), but never reaches (a)'s
   1 copy. Keep MessagePort and the utility process for control and metadata, not pixels.
2. **ADR-001 should record transport (a) as the Phase-1 path**: the napi addon in the main
   editor window's preload, with the present loop in the preload's isolated world and
   clock-scheduled frame choice (the WP-0C.01 configuration). It is the only CPU path here
   that holds 60 fps with 1 copy and 14-32 % renderer CPU. The ADR must state the
   cost explicitly: the editor renderer runs **without the OS sandbox**. It must also list the
   mitigations as requirements: local `app://` content only, strict CSP, no remote content,
   narrow bridge, token kept out of the main world, plugin UIs in sandboxed frames with no
   preload, and validated IPC.
3. **ADR-001 should name GPU shared textures as the target that removes the sandbox
   exception**, and fund a follow-up spike before preview work hardens in Phase 1. Electron
   45's `sharedTexture` reached a fully sandboxed renderer at 58-60 fps with zero CPU pixel
   copies, about 13 % renderer CPU and about 0.5 ms main -> page in the probe above. This is
   the same path as arch
   §10's "zero-copy upgrade", so one investment buys both performance and the sandbox. Gates
   before switching the default:
   - the engine exports its preview texture: wgpu/D3D12 -> shared NT handle, IOSurface on
     macOS, DMA-BUF on Linux;
   - the engine renders on Chromium's adapter on hybrid-GPU laptops (match the LUID; WP-0C.01
     finding 5);
   - `sharedTexture` (still `@experimental`) holds up across the Electron versions we pin;
   - WP-0C.04 numbers on all four OS targets.
   Until then, (a) stays the fallback.
4. **Keep the renderer-facing frame interface transport-agnostic**: frames with
   `seq/pts/size/format` plus the engine audio clock, consumed by the scheduler and present
   loop. Then swapping (a) for GPU textures (or for (b) at low resolution, if a sandboxed
   renderer is ever mandated before textures land) does not touch React or overlay code
   (WP-0C.02). The (b) implementation shows that this interface works across the preload
   world, the page's main thread and a worker.

### Notes for the integrator

- WBS asks for the A/B table from `cargo xtask bench-report`. That belongs to WP-0C.04 (xtask
  is outside this area). For now, `scripts/bench-ab.mjs --table` builds it from
  `results/bench-*.json`. Each A/B file has a `transport` field and a flat `summary`
  ([schema](#bench-cli-and-result-schema)).
- The WBS's Playwright-Electron smoke test for both transports is replaced by the in-app
  `--selftest`, the same approach as WP-0C.01.
- The A/B switch is the CLI flag `--transport=preload|utility`, not the WBS's
  `?transport=a|b` query: the window's `sandbox` preference is fixed when it is created.
- `--transport=texture` is a probe, not a transport: it has no engine.
- Arch §2, §10 and §18 wording should follow the ADR-001 decision. In particular, §10's
  "upgrade path" (shared textures) is also the way to drop the §18 sandbox exception.
