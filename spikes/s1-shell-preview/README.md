# Spike S1: Electron shell + Rust sidecar + SHM frame ring + napi reader (WP-0C.01)

> **Throwaway spike code.** Not part of the root Cargo or pnpm workspaces; product code never
> depends on it. It answers one question for ADR-001: can preview frames rendered in a
> separate Rust process reach the Electron UI at 1080p60 with low latency (transport (a)).
>
> Location note: the WBS names the area `spikes/s1-preview/{engine,bridge,desktop}`; this
> work package was assigned `spikes/s1-shell-preview/` and keeps the same sub-structure.
> WP-0C.02/0C.04/0C.06 should either reuse this path or rename it in one go.

## What is here

```text
spikes/s1-shell-preview/
  Cargo.toml         own Cargo workspace: ring, engine, bridge
  ring/              s1-ring: named shared memory + seqlock frame ring (layout: RING.md)
  engine/            s1-engine: wgpu synthetic 1080p60 frames -> async readback -> ring;
                     WebSocket control channel; stdout handshake
  bridge/            s1-bridge: napi-rs v3 addon (RingReader, TestRingWriter) + Node tests
  desktop/src/       Electron main, preload (transport (a)), renderer (WebGPU present + HUD)
  scripts/           build-native / build-js (esbuild) / selftest / summarize
  results/           informal self-test JSON from the dev machine (see "Informal numbers")
  RING.md            ring header/slot layout and protocol (feeds WP-1H.02)
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
| `--present=gpu\|2d` | WebGPU `queue.writeTexture` (default) or Canvas2D `putImageData` (RGBA8 only) |
| `--verify=0` | disable per-frame checksum + row-stamp verification |
| `--gpu=high\|low\|software` | wgpu adapter preference for the engine |
| `--size=WxH` | engine frame size (default 1920x1080) |
| `--chromium-dgpu` | adds Chromium's `force_high_performance_gpu` (WebGPU on the dGPU) |

Environment: `WGPU_BACKEND=dx12|vulkan` (engine backend), `S1_LOG=debug` (engine log
filter), `S1_ENGINE=<path>` (engine binary). Window buttons: play/pause, seek -1 s / +1 s,
toggle RGBA8/NV12, verification on/off, reset stats.

The engine also runs standalone: `target/release/s1-engine --ring cw-s1-test --no-stdin-watch`.

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
(0C.02); transport (b) (0C.06); formal measurement and other OSes (0C.04); POSIX shared memory
(stub); a Playwright smoke test (the self-test drives the same checks from Electron main).

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
