# Spike S1: Electron shell + Rust sidecar + SHM frame ring + WebGPU preview (WP-0C.01, WP-0C.02)

> **Throwaway spike code.** Not part of the root Cargo or pnpm workspaces; product code never
> depends on it. It answers one question for ADR-001: can preview frames rendered in a
> separate Rust process reach the Electron UI at 1080p60 with low latency (transport (a)).
> WP-0C.02 adds the WebGPU canvas (three upload paths), the DOM overlay, the scrub bar and live
> resize, plus a benchmark mode; see [WP-0C.02](#wp-0c02-webgpu-canvas-dom-overlay-scrub-bar-live-resize).
>
> Location note: the WBS names the area `spikes/s1-preview/{engine,bridge,desktop}`; this
> work package was assigned `spikes/s1-shell-preview/` and keeps the same sub-structure.
> WP-0C.02/0C.04/0C.06 should either reuse this path or rename it in one go.

## What is here

```text
spikes/s1-shell-preview/
  Cargo.toml         own Cargo workspace: ring, engine, bridge
  ring/              s1-ring: named shared memory + seqlock frame ring (layout: RING.md)
  engine/            s1-engine: wgpu synthetic 60 fps frames at the preview size -> async
                     readback -> ring; WebSocket control channel (play/pause/seek/format/
                     resize/scrub-config); scrub.rs = low-res-first scrub scheduling
  bridge/            s1-bridge: napi-rs v3 addon (RingReader, TestRingWriter) + Node tests
  desktop/src/       Electron main, preload (transport (a)), bench.ts (WP-0C.02 benchmark)
    shared/          present.ts (WebGPU upload paths), loop.ts (rAF loop + scrub-to-pixel),
                     stage.ts (canvas size + resize renegotiation), preview.ts (wiring)
    renderer/        page: stage, overlay.ts (transform box, safe areas), scrub.ts, HUD
  scripts/           build-native / build-js (esbuild) / selftest / bench / summarize(-bench)
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
pnpm start                          # window with live preview + HUD
pnpm selftest 60                    # WP-0C.01 acceptance checks -> results/selftest-*.json
pnpm bench [quick|<s>]              # WP-0C.02 benchmark (20 s phases by default, ~4 min)
                                    #   -> results/bench-*.json (+ a git-ignored screenshot)
node scripts/summarize-bench.mjs    # Markdown tables of results/bench-*.json
pnpm test                           # Node tests for the addon (needs `pnpm build`)
pnpm test:rust                      # ring unit tests (wrap-around, seqlock, shm lifetime)
pnpm typecheck
node scripts/summarize.mjs          # Markdown table of results/*.json
```

`pnpm start` / `pnpm selftest` accept these flags (`pnpm start -- --format=nv12`):

| Flag | Meaning |
|---|---|
| `--format=rgba8\|nv12` | engine output format (also toggled live in the window) |
| `--draw=main\|preload` | where the rAF present loop runs: renderer main world (pixels cross `contextBridge`) or the preload's isolated world (draws into the page's canvas, no clone) |
| `--policy=latest\|next\|clock` | frame choice: latest-wins; naive FIFO; or newest slot with `pts <= audio clock - latency` |
| `--latency=<ms>` | latency target of `policy=clock` (default 20) |
| `--present=gpu\|2d` | WebGPU (default) or Canvas2D `putImageData` (RGBA8 only) |
| `--upload=writeTexture\|videoFrame` | WebGPU upload path (WP-0C.02; also toggled live in the window) |
| `--engine-size=canvas\|WxH` | engine renders at the canvas pixel size (default) or a fixed size the presenter scales |
| `--bench[=quick\|<s>]` | run the WP-0C.02 benchmark instead of the interactive window (`pnpm bench` adds `--draw=preload --policy=clock`) |
| `--verify=0` | disable per-frame checksum + row-stamp verification |
| `--gpu=high\|low\|software` | wgpu adapter preference for the engine |
| `--size=WxH` | engine frame size (default 1920x1080) |
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

## Architecture sketch

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
(done in WP-0C.02, below); transport (b) (0C.06); formal measurement and other OSes (0C.04);
POSIX shared memory (stub); a Playwright smoke test (the self-test drives the same checks from
Electron main).

## Informal numbers (dev machine, NOT the WP-0C.04 measurement)

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

## Findings and risks (input for ADR-001)

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
`draw=preload` + `policy=clock` numbers above, not against `draw=main`.

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
- **Benchmark** (`desktop/src/bench.ts`, `pnpm bench`). It drives real input through
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

