# S1 timeline stress (WP-0C.03)

Throwaway spike: a virtualized Canvas2D timeline loaded with 5,000 clips on 20 tracks, 50,000 keyframes and 200 thumbnail bitmaps, plus a scripted pan/zoom driver. Answers the S1 roadmap question "does Canvas2D hold >= 55 fps at this scale, or do we need PixiJS/WebGL?" ([architecture §16](../../docs/01-architecture.md), [Phase 0 S1 row](../../docs/04-roadmap.md)).

It is a standalone Vite app, **not** in the pnpm workspace (own `pnpm-lock.yaml`) and not covered by Biome or the root Vitest run, so root `just check` is unaffected.

## Run

```sh
cd spikes/s1-timeline-stress
pnpm install --ignore-workspace      # once
pnpm dev                             # interactive, with the perf HUD (bottom right)
pnpm bench                           # headless Chromium, 30 s scripted sweep, prints JSON
pnpm bench --seconds 10 --uncapped   # no vsync cap: shows headroom
pnpm bench --out results/x.json      # also write the JSON
pnpm typecheck
```

`pnpm bench` needs Playwright's Chromium (`pnpm exec playwright install chromium`). If the download is blocked it exits with code 3 and says so; use `pnpm dev` and press **A** (auto sweep) to read the same numbers off the HUD.

Controls: wheel pans, **Ctrl+wheel** zooms at the cursor, **Shift+wheel** scrolls vertically, drag pans, click selects a clip or keyframe (Ctrl adds), **Shift+drag** is a marquee, click/drag on the ruler moves the playhead, **Space** plays, **A** runs the sweep, **Esc** clears the selection.

## Design

| File | Role |
|---|---|
| `src/model.ts` | Seeded synthetic data in typed arrays (structure of arrays); `IntervalIndex` per track |
| `src/view.ts` | Pan/zoom state, track layout, hit-testing, selection, marquee |
| `src/renderer.ts` | Canvas2D renderer, counters for the HUD |
| `src/thumbs.ts` | 200 bitmaps: `OffscreenCanvas` -> PNG blob -> `createImageBitmap` |
| `src/theme.ts` | Reads all colors, sizes and fonts from the `tokens.css` CSS variables (`getComputedStyle`) |
| `src/driver.ts` | Sweep function, metrics collector, hit-test benchmark |
| `scripts/bench.mjs` | Build, `vite preview`, Playwright, JSON |

- **Virtualization.** Per track, an interval index (clip ids sorted by start plus a prefix-max of end times) answers "which clips overlap [t0, t1]" in O(log n + visible). Tracks outside the vertical viewport are skipped. Clips are never iterated globally.
- **Batching.** Per visible track: one lane fill, one path for all clip bodies, one for all edge strips, one for all keyframe diamonds, one for selection outlines. Draw calls stay in the low hundreds (HUD "draw calls", about 200 typical, 421 peak).
- **Level of detail.** Clips are at least 1 px wide; labels at >= 70 px; thumbnail tiles at >= 48 px (only tiles intersecting the canvas, last tile cropped with the 9-argument `drawImage`); keyframes at >= 100 px. The hit-test uses the same keyframe rule, so what is hittable is what is drawn.
- **Hit-testing.** Same index: track from Y, `query(t ± 0.5px)`, then a binary search over the clip's sorted keyframe times. Marquee queries each intersecting track's index.
- **Tokens.** Clip colors, lanes, playhead (marigold), selection outline, fonts, track heights and spacing come from CSS variables. Documented exceptions: the 11 px label size mirrors the `caption` type token (canvas cannot read a composite font token), and the thumbnail index text is 10 px.
- **Determinism.** Data uses a seeded PRNG.

The scripted sweep is a pure function of elapsed time: log-sinusoidal zoom between 0.5 px/s (the whole timeline, about 3,400 clips visible) and 300 px/s (period 12 s), a pan that crosses the whole timeline (17 s) and a vertical scroll over all 20 tracks (9 s). It calls the same `View` methods the wheel and drag handlers use, so input event overhead is not included. Frame time is the `requestAnimationFrame` delta; draw time is the wall time of `renderer.draw()`.

## Informal numbers

**Informal.** One run each on one dev machine (Windows 11, headless Chromium 159 via Playwright, 1600x900, DPR 1, whatever GPU/raster path headless chose). Not the formal measurement; WP-0C.04 does that on the reference machines. Raw JSON in [`results/`](results/).

| Run | fps p50 | fps p5 | frame p95 | frame max | draw p95 (CPU) | hit-test p95 |
|---|---|---|---|---|---|---|
| 30 s, vsync (`bench-vsync.json`) | 60.0 | 60.0 | 16.7 ms | 16.7 ms | 1.3 ms | < 0.01 ms |
| 30 s, uncapped (`bench-uncapped.json`) | 222 | 111 | 9.0 ms | 31 ms | 1.0 ms | < 0.01 ms |

Worst frames of the sweep: 3,407 clips and up to ~1,200 keyframes and ~185 thumbnail tiles visible (not simultaneously). JS heap about 10 MB. `measureUserAgentSpecificMemory` returned null here.

Against the WP criteria (fps p5 >= 55, hit-test p95 < 2 ms): **both met with wide margin** on this machine. Caveats: vsync-capped runs saturate at 60, so the uncapped run is the better headroom indicator (p5 about 111 fps); hit-test timing is at the timer's resolution limit, so "< 0.01 ms" is what can be said; the draw-time column is CPU submit time and excludes deferred rasterization.

## Findings for Phase 1 (`packages/timeline`, WP-1M.05)

1. **Canvas2D is enough at this scale; PixiJS/WebGL is not needed.** Per-frame cost with virtualization and batching is about 1 ms of JS plus a few ms of raster, 5x under the 16.7 ms budget even when the whole 5k-clip timeline is on screen. Per architecture §16, revisit WebGL only if the real-media case (waveforms, filmstrip decode, effects badges) or 4K/HiDPI measurements in WP-0C.04 say otherwise.
2. **What made it fast:** (a) per-track interval index, never a global scan; (b) batching all clips of a track into one path per fill style; (c) LOD thresholds so subpixel clips and unreadable keyframes cost nothing; (d) no per-clip allocations in the hot loop; (e) thumbnails as pre-decoded `ImageBitmap`s, cropped rather than clipped.
3. **Things to watch.** `fillText` is the most expensive primitive, so cap labels (we cap at 400 per frame). Per-clip `save/clip` would not scale; avoid it. HiDPI (DPR 2) quadruples fill cost and was not measured. Redraw on demand (dirty flag) already works and should be kept for idle.
4. **Not covered here (by scope):** interaction state machine, snapping, real thumbnail/waveform fetch and LRU eviction (200 bitmaps of 96x54 are about 4 MB, a real filmstrip cache needs a byte budget), drag-editing, and React integration. Interval index rebuild cost on edits was not measured; an incremental structure (or rebuilding one track on change) is the likely Phase 1 shape.
5. **Time units.** The spike uses `f64` seconds. Product code must use flicks (`i64`) via the conversion helpers; the index and renderer should take pixel-per-flick math from the wasm-shared helpers.
6. **Spec discrepancy.** The WBS lists the area as `spikes/s1-preview/timeline-stress` and results under `spikes/s1-preview/results/`; this spike lives in `spikes/s1-timeline-stress/` as requested, so the integrator should reconcile paths when 0C.04 collects results.
7. **Not done relative to the WBS text:** `measureUserAgentSpecificMemory` is wired but returned null in headless Chromium; the JS heap size is reported instead. Fps p50/p5 and longest frame are in the same JSON shape fields (`fps`, `frameMs`).
