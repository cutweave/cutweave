# Spike S4 — render graph, linear-light compositor, LUT, tone-map, headless export (WP-0F.01)

> **Throwaway spike** (docs/wbs/P0-foundations.md). Own Cargo workspace, excluded from the root
> workspace, never imported by `crates/`. Only the findings below carry forward into
> `cutweave-render` (Phase 1). Architecture: §8 (render pipeline), §20 (golden tolerances).

## What it does

```text
timeline::demo_scene(frame) ──► scene::Scene ──► graph::build ──► RenderGraph (DAG)
   (pure fn of the frame)          (serde JSON)   cull · order ·     │
                                                  cache keys          ├──► gpu::Renderer (wgpu 30.0.2, compute)
                                                                      │       └─► PNG (8/16-bit, cICP-tagged)
                    cpu_ref::render (independent f64 reference) ◄─────┘
                                     └──► metrics: PSNR · SSIM · ΔE2000
```

The demo sequence (1080p60, 300 frames) composites **3 layers with 2 blend modes**, a
**cross-dissolve**, **blur-lite** and a **`.cube` LUT**, all in **linear-light `Rgba16Float`**:

| Layer | Source (synthetic, generated in code) | Input transform | Effects / blend |
|---|---|---|---|
| `bg` | `camera709`: 8-bit 4:2:0 Y'CbCr, BT.709 matrix, limited range, BT.709 TF; 75% bars, ramp, steps, a moving white bar | YUV→RGB from tags, inverse OETF | blur-lite, normal |
| `pip` | `hdr10`: 10-bit 4:2:0, BT.2020 NCL, limited, **PQ** (0–4000 cd/m² ramp, BT.2020 primaries at 1000 cd/m²) **dissolving (frames 120–180) into** `phone601`: 8-bit 4:2:0, **BT.601, full range**, sRGB TF | PQ EOTF → BT.2020→709 → **BT.2390 EETF → SDR** | rotating PiP, normal 95% |
| `chart` | `chart`: RGBA8 sRGB 24-patch colour checker, straight alpha, rounded card | sRGB EOTF, premultiply | **multiply** 85% |
| sequence | — | — | `warm_contrast` 9³ `.cube` LUT (BT.709-encoded domain) → BT.709 OETF output |

No FFmpeg is used (not available on the build machine): every source is generated as the exact
planes a decoder would hand over (4:2:0 with MPEG-2 chroma siting, 8/10-bit codes, probe tags).

## How to run

```sh
cd spikes/s4-compositor
cargo test                                   # 42 unit + 8 GPU tests (goldens on every adapter present)
cargo clippy --all-targets -- -D warnings

cargo run --release --bin s4 -- adapters                       # list adapters (+ golden keys)
cargo run --release --bin s4 -- render --frame 150 --out f150.png          # demo frame, 1080p, auto adapter
cargo run --release --bin s4 -- render --frame 150 --out f.png --adapter software --graph   # WARP + DAG dump
cargo run --release --bin s4 -- render --scene blend_modes --out b.png --bits 16 --transfer srgb
cargo run --release --bin s4 -- render --frame 0 --out ref.png --cpu       # independent CPU reference
cargo run --release --bin s4 -- reference --width 1920 --height 1080 --diff-dir diffs   # PSNR/SSIM/ΔE per scene
cargo run --release --bin s4 -- bench --all-adapters --passes             # timing table (timestamp queries)
cargo run --release --bin s4 -- export --out-dir frames                   # 300-frame PNG sequence
cargo run --release --bin s4 -- compare a.png b.png                       # §20 metrics for two PNGs
cargo run --release --bin s4 -- scene --frame 150 > scene.json            # scene JSON (WP-0G.01 format)
cargo run --release --bin s4 -- render --scene-json scene.json --out s.png
cargo run --release --bin s4 -- golden --update --adapter software        # rewrite one golden set
S4_UPDATE_GOLDENS=1 cargo test --test gpu golden                          # rewrite all present sets
```

Everything is headless (no window or surface); `--adapter` takes `auto`, `software`
(WARP/lavapipe), an index, `key:<golden-key>` or a name substring. `S4_ADAPTER` picks the adapter
for the single-adapter GPU tests (default: software, so CI and dev machines agree).

## Design

### Scene graph (`src/scene.rs`)

`Scene { width, height, background, layers, effects, output }`; each `Layer { content: Node,
transform, opacity, blend, effects }`. **The contract every `Node` fulfils: render to a
premultiplied, linear-light, Rec.709-primaries `Rgba16Float` texture of its intrinsic size.** Effects
run on that texture; the compositor places it with the layer transform. Node kinds:

* `Media { source, frame, tags?, tone_map? }` — resolved through a `SourceProvider` (decoder stand-in).
* `Shape { width, height, corner_radius, fill_srgb }` — analytic AA rounded rect (compositions).
* `Group { width, height, layers }` — nested stack in its own intermediate (compositions, §12).
* `Transition { kind, progress, from: Layer, to: Layer }` — both sides rendered in the enclosing
  canvas, blended in linear light.
* **`Text { text, font, size_px, fill_srgb, box_size }` — the WP-0F.02 plug-in point.** The graph
  already lowers it to a cacheable `Text` pass sized `box_size`; the executor returns
  `Error::Unsupported` until 0F.02 adds an MSDF/Vello pass that fills that texture. Nothing else
  changes: effects, transforms, blend modes and caching apply to text like any other node.

The scene is plain `serde` data, so the WP-0G.01 QuickJS runtime can emit it as JSON
(`s4 scene` / `s4 render --scene-json`). Time is flicks (`timeline::frame_to_flicks`).

### Render graph (`src/graph.rs`)

* **Passes** (`Fill, Decode, Shape, Text, Blur, Lut, Dissolve, Composite, Output`) read 0..n
  resources and write one. Built bottom-up from the scene, then **pruned** (unreachable passes)
  and **topologically ordered** (deterministic Kahn; cycle detection tested).
* **Cache keys** are Merkle hashes (stable FNV-1a over a canonical encoding): `key = H(op, params,
  size, input keys)`. Static sources keep the same decode key on every frame; the output key
  identifies the whole subgraph (the `(subgraph hash, t)` frame-cache key of §8).
* **Culling**: zero opacity, degenerate transform, fully off-canvas, transitions at progress 0/1,
  and **occlusion** (everything below an opaque, canvas-sized, identity-transformed normal layer;
  compositing that cover is an exact copy, so its content becomes the base — saves the background
  fill and one full-frame composite in the demo).
* **Layer-space transitions**: when both sides of a dissolve share transform and size (the usual
  clip-to-clip case), the dissolve runs at source resolution and the result is composited once with
  the combined transform. Bilinear sampling and edge coverage are linear, so this equals the
  canvas-space definition (used by the CPU reference and as the general fallback) up to rounding,
  and removes two canvas fills, two full-frame composites and a full-frame dissolve.

### Executor (`src/gpu.rs`, `src/shaders/*.wgsl`)

* wgpu **30.0.2**, all passes are **compute shaders** writing `Rgba16Float` storage textures
  (explicit bind-group layouts; no auto-layout).
* **Exact-weight filtering**: bilinear layer sampling, 4:2:0 chroma upsampling and LUT trilinear
  use `textureLoad` + explicit weights, never a hardware sampler (vendor-specific weight precision).
* **Explicit quantisation**: the output pass encodes and rounds half-up into a packed storage
  buffer (8- or 16-bit), instead of relying on float→unorm render-target conversion.
* Cacheable pass outputs (decodes, shapes, text, effects on those) persist across frames (LRU of one
  frame); transient textures are recycled from a pool after their last reader.
* GPU time via **timestamp queries** per pass; uncaptured errors and device loss are captured;
  `Renderer::recover()` re-enumerates the adapter, recreates the device and drops all caches.

### Colour pipeline

Input (`decode.wgsl`): codes → range (H.273 limited/full, 8/10-bit) → Y'CbCr→R'G'B' (Kr/Kb from the
matrix tag) → clamp [0,1] → inverse transfer (BT.709 inverse OETF / sRGB / BT.1886 / PQ EOTF) →
primaries matrix (BT.2020→709 derived from xy chromaticities) → **PQ only: BT.2390 EETF on
max(R,G,B) in the PQ domain** (hue-preserving; source peak 1000 → target 100 cd/m², 1.0 = 100 cd/m²)
→ clip negatives (gamut clip) → premultiply. Blending (W3C separable formulas on premultiplied
colour), dissolve, blur and the LUT run in linear light; the LUT encodes to its declared space,
samples trilinearly and decodes back. Output: clamp → BT.709 OETF (or sRGB/BT.1886/linear) →
quantise; PNGs carry a `cICP` chunk (H.273 code points).

### Reference and metrics (`src/cpu_ref.rs`, `src/metrics.rs`)

The CPU reference walks the **scene** (not the graph) in `f64` and implements the same documented
semantics independently of the WGSL. Metrics: PSNR (RGB), SSIM (Wang 2004, 11×11 Gaussian σ=1.5,
Rec.709 luma), CIEDE2000 (validated against Sharma et al. test pairs; colours decoded through the
display EOTF). Tolerances: §20 golden **SSIM ≥ 0.995 and PSNR ≥ 40 dB**; independent reference
**PSNR > 45 dB and mean ΔE2000 < 2**.

### Goldens (`tests/golden/<adapter-key>/*.png`)

384×216 frames of `demo_f0000/0150/0299` and `blend_modes`, one set per adapter class present on
the dev machine (5 sets, 324 KB, plain PNG — deliberately outside the LFS patterns). The test
compares each adapter with its own set under the §20 tolerance and reports bit-identity; an adapter
without a set (e.g. lavapipe in CI) is compared cross-adapter against the WARP set.

## Results (dev machine: Win11 26200, RTX 3080 Laptop + AMD Radeon iGPU, Rust 1.100)

### GPU vs independent CPU reference (640×360, every adapter)

`s4 reference --width 640 --height 360 --adapter <n>`; WARP shown, the other four adapters are
within ±0.04 dB and ±0.003 mean ΔE of these values. Max |Δ| is 1 code value everywhere.

| Scene | PSNR dB | SSIM | ΔE2000 mean | ΔE p95 | ΔE max | Pass (> 45 dB, ΔE < 2) |
|---|---|---|---|---|---|---|
| `demo_f0000` (HDR PiP, multiply chart, blur, LUT) | 60.28 | 0.99991 | 0.070 | 0.42 | 1.21 | ok |
| `demo_f0150` (mid-dissolve HDR → BT.601) | 59.89 | 0.99986 | 0.076 | 0.46 | 1.18 | ok |
| `demo_f0299` | 60.14 | 0.99988 | 0.066 | 0.35 | 1.28 | ok |
| `identity709` (BT.709 in → out round trip) | 73.63 | 0.99998 | 0.001 | 0.00 | 0.68 | ok |
| `hdr_tonemap` (PQ 10-bit → SDR) | 62.64 | 0.99991 | 0.010 | 0.00 | 0.62 | ok |
| `blend_modes` (8 modes + rotated group) | 68.19 | 0.99997 | 0.006 | 0.00 | 1.04 | ok |
| `lut_chart` (layer LUT on colour checker) | 65.54 | 1.00000 | 0.018 | 0.12 | 0.96 | ok |

### Goldens and determinism

* **Same adapter**: every adapter reproduces its stored set **bit-identically**; a frame rendered
  cold (fresh device) equals the same frame rendered mid-sequence with warm caches and recycled
  textures (`same_adapter_frames_are_bit_identical_cold_and_warm`), and a device recovered after
  loss renders bit-identically (`device_lost_is_reported_and_recovered`).
* **Across backends / vendors** (stored sets compared with `s4 compare`):

| Pair | Bit-identical scenes | Worst PSNR | Max \|Δ\| | Max ΔE2000 |
|---|---|---|---|---|
| AMD iGPU Vulkan vs AMD DX12 | 4/4 | ∞ | 0 | 0 |
| RTX 3080 Vulkan vs RTX 3080 DX12 | 3/4 | 99.1 dB | 1 | 0.24 |
| AMD DX12 vs WARP | 2/4 | 94.3 dB | 1 | 0.38 |
| RTX 3080 DX12 vs WARP | 0/4 | 82.4 dB | 1 | 1.26 |
| RTX 3080 DX12 vs AMD DX12 | 0/4 | 82.5 dB | 1 | 1.26 |

All pairs are > 40 dB above the §20 floor. 16-bit output vs the f64 reference: 68.9 dB (max 78 of
65535 codes ≈ 0.3 of an 8-bit LSB) — the `Rgba16Float` intermediates set that ceiling.

### Informal timings — 1080p demo scene (INFORMAL; formal measurement is WP-0F.03)

`s4 bench --all-adapters --passes` (release build): the demo at 1920×1080 over frames 120–240
(60 dissolve frames, then the BT.601 clip): `bg` decode + blur-lite (2 passes), layer-space
dissolve, PiP composite, multiply chart composite, sequence LUT, output. Static sources hit the pass
cache, so 7 of 9 passes run per frame. GPU = first pass start → last pass end (timestamp queries).
Wall = encode + plane uploads + submit + wait + 8 MB readback. One run each, laptop on mains power,
no clock pinning.

| Adapter | Backend | GPU p50 | GPU p95 | Cold frame (no pass cache) | Wall p50 | Device + pipelines + 1st frame |
|---|---|---|---|---|---|---|
| AMD Radeon iGPU | DX12 | **6.46 ms** | 8.06 ms | 6.68 ms | 13.8 ms | 1.0 s |
| AMD Radeon iGPU | Vulkan | **6.55 ms** | 8.45 ms | 7.14 ms | 13.9 ms | 0.8 s |
| RTX 3080 Laptop | DX12 | **0.72 ms** | 3.13 ms | 0.81 ms | 8.8 ms | 1.0 s |
| RTX 3080 Laptop | Vulkan | **0.63 ms** | 2.97 ms | 0.69 ms | 7.3 ms | 0.9 s |
| WARP (software) | DX12 | 191 ms | 405 ms | 202 ms | 540 ms | 1.7 s |

Per-pass mean on the AMD iGPU (DX12): composites 1.99 ms (2 passes), blur 2.15 ms (2), LUT
0.91 ms, decode 0.81 ms, output 0.70 ms, dissolve 0.17 ms.

How the graph changed the iGPU number (DX12 p50, same scene): naive canvas-space graph **12.2 ms**
→ + occlusion culling **10.7 ms** → + layer-space dissolve **6.5 ms**.

Export (`s4 export`, RTX 3080 DX12, 300 frames of 1080p PNG, cICP-tagged, 53 MB): 41 s = 7.3 fps,
limited by PNG encoding; render + readback 23.9 ms/frame. The median GPU time there was 7.1 ms
rather than 0.7 ms: the dGPU probably clocks down while the CPU encodes PNGs. WP-0F.03 must warm up
and measure under sustained load, or pin clocks.

## Acceptance criteria (WP-0F.01)

| Criterion (WBS + brief) | Status | Evidence |
|---|---|---|
| Render graph: DAG of passes, simple cache keys | **Met** | `graph.rs`; `graph::tests::*` (topology, cycles, pruning, Merkle keys, cacheability, culling) |
| 3 layers, 2 blend modes (normal, multiply), dissolve, `.cube` 3D LUT, linear `Rgba16Float` | **Met** (+ all 8 core modes) | demo scene, `blend_modes` scene, goldens |
| YUV→RGB with matrix/range/transfer from tags | **Met** | BT.709 limited 8-bit, BT.601 full 8-bit, BT.2020 PQ 10-bit sources; `identity709` 73 dB |
| PQ→SDR tone-map (BT.2390-style) for HDR10 input | **Met** | `hdr_tonemap` scene; `color::tests::eetf_*` |
| Headless on software adapters and real GPUs, readback to PNG/EXR | **Met** for WARP + RTX 3080 + AMD iGPU (DX12 and Vulkan), PNG 8/16-bit. lavapipe not on this OS; **EXR not done** (16-bit PNG instead) | `s4 adapters`, GPU tests on all 5 adapters |
| Independent CPU float reference; PSNR and ΔE printed per test scene | **Met** | `cpu_ref.rs`, `s4 reference` (7 scenes) |
| Comparison tool unit tests (identical → ∞ dB, ΔE = 0) | **Met** | `metrics::tests` (+ Sharma CIEDE2000 pairs, PSNR closed form) |
| GPU timing via timestamp queries; timing table per adapter (1080p, 3 layers + blur-lite + LUT) | **Met** | `s4 bench --all-adapters --passes` (table above) |
| Headless export of 300 frames → MP4 via `ffmpeg` (ffprobe ok, 300 frames) | **Not met — pending FFmpeg** (not installed; bundle is WP-0A.06). The 300-frame **PNG sequence** export works | `s4 export`, `png_sequence_export_round_trips` |
| Golden test vs stored per-adapter reference PNGs with the §20 metrics | **Met** | `golden_frames_match_per_adapter_references` |
| CLI `s4 render --frame N --out x.png` | **Met** | `src/bin/s4.rs` |
| Text node plugs into the scene graph (rendering is WP-0F.02) | **Met** (design) | `Node::Text` → cacheable `Text` pass; `text_*` tests |
| `cargo test`, `cargo clippy -- -D warnings` in the spike; root `just check` unaffected | **Met** | 42 unit + 8 GPU tests; clippy also with `--all-targets` |
| Epic pass bar "1080p60 < 8 ms GPU on an iGPU" (judged in WP-0F.03 on M1 / Intel Xe) | **Borderline** on this AMD iGPU: p50 6.5 ms, p95 8.1–8.5 ms (informal) | `s4 bench` |

Deviations from the WBS text, for the integrator: the crate lives in `spikes/s4-compositor`
(the brief) rather than `spikes/s4-render` (WBS); the colour notes the WBS routes to
`docs/spikes/S4-color-notes.md` are in this README (the brief restricted changes to this
directory); sources are synthetic (no FFmpeg, no corpus).

## Findings for Phase 1 (`cutweave-render`)

**Determinism and goldens**

1. **Exact-weight filtering + explicit quantisation make adapters agree to ≤ 1 LSB.** Manual
   bilinear/trilinear via `textureLoad` and round-half-up into a storage buffer gave ≤ 1 LSB vs the
   f64 reference on all 5 adapters (PSNR 59.7–73.6 dB, mean ΔE2000 ≤ 0.08, max 1.28) and ≥ 82 dB
   between vendors; same-GPU DX12 vs Vulkan is mostly bit-identical. Keep this in the export /
   golden path; reserve hardware samplers for preview downscales. Never rely on float→unorm
   conversion of render targets for goldens.
2. **Tolerance recommendation for WP-1O.01:** same adapter → bit-identical is achievable (and
   held across cold/warm caches, pooled textures and device recovery). Cross-adapter goldens could
   use PSNR ≥ 50 dB / max |Δ| ≤ 2; the §20 floor (40 dB) is very loose for this pipeline. Key golden
   sets by adapter *and* backend; driver updates are the remaining risk.
3. **`Rgba16Float` is enough for SDR** (≤ 1 LSB at 8-bit) but caps 16-bit output at ≈ 69 dB
   (≈ 11–12 effective bits). Re-evaluate (`Rgba32Float` for grading chains) before 12-bit/HDR export.

**Performance (see timings; informal)**

4. **Full-canvas ping-pong compute passes are bandwidth-bound on the iGPU**: each 1080p
   `Rgba16Float` read+write pass costs ≈ 0.7–1.1 ms on the AMD iGPU vs ≈ 0.1–0.2 ms on the RTX
   3080. Removing work in the graph mattered more than tuning shaders: occlusion culling plus
   layer-space transitions took the iGPU from 12.2 ms to 6.5 ms (DX12 p50) without touching a shader.
5. To meet "1080p60 < 8 ms on an iGPU" reliably in Phase 1: (a) composite **in place with render
   passes + fixed-function premultiplied blending** for normal/multiply/screen/add/darken/lighten,
   drawing only the layer's quad, so cost scales with layer area, not canvas area; (b) for
   overlay/soft-light, copy only the layer's bbox of the backdrop and blend in the shader; (c)
   **fuse passes** (decode → per-layer effects; sequence LUT → output transform, which saves the
   ≈ 0.7 ms output pass on the iGPU); (d) a cheaper blur (shared-memory tiles or downsampled), since
   blur-lite is ≈ 1/3 of the iGPU frame; (e) keep the graph-level culls implemented here. The p95
   (8.1–8.5 ms) shows there is no headroom yet for captions (WP-0F.02) on top.
6. **Pass cache works**: Merkle keys turn static sources into cache hits (2/frame in the demo:
   their decode and upload disappear). The one-frame LRU is only a demo; Phase 1 needs the
   VRAM-budgeted GPU texture cache of §7. Per-frame allocations remain (bind groups, views, upload
   textures); uniform buffers are recycled per pass slot. The "no per-frame allocations" rule needs
   an upload staging ring and bind-group caching.

**wgpu 30 / platform**

7. **Concurrent pipeline creation on several WARP devices crashes the process**
   (`STATUS_ACCESS_VIOLATION`, ≈ 50% of parallel test runs; hardware DX12/Vulkan never crashed in
   16 runs). A process-wide lock around pipeline creation fixed it (0/20). CI running GPU tests on
   WARP must serialise pipeline creation or run single-threaded. Phase 1 should build pipelines once
   per device anyway (pipeline cache).
8. **Device loss**: `request_device` cannot be called twice on one `Adapter`, so recovery must
   re-enumerate from a new `Instance`, recreate the device and drop every cache.
   `Device::destroy()` triggers the lost callback and is a usable test hook. The default uncaptured
   error handler panics; install `on_uncaptured_error` and use error scopes around pipeline creation.
9. **API notes for 30.x**: `InstanceDescriptor::new_without_display_handle()`,
   `PipelineLayoutDescriptor { bind_group_layouts: &[Option<&_>], immediate_size }`,
   `get_mapped_range` returns `Result`, `PollType::wait_indefinitely()`, the DX12 compiler defaults
   to `Auto` (static DXC → `dxcompiler.dll` → FXC). Use explicit bind-group layouts: an
   `Rgba32Float` 3D LUT needs `filterable: false`. Every adapter here exposed `TIMESTAMP_QUERY`,
   including WARP.

**Colour**

10. **Tone-map defaults need a product decision.** BT.2390 EETF on max(RGB) with 1000 → 100 cd/m²
    and 1.0 = 100 cd/m² puts the knee at ≈ 28 cd/m²: 100 → 0.69, 203 (HDR reference white) → 0.88,
    400 → 0.98, ≥ 1000 → 1.0. BT.2408-style pre-scaling (203 → SDR white) should be a candidate, and
    source peak should come from mastering metadata/MaxCLL (§7 probe).
11. **Gamut clip is visibly lossy**: BT.2020 primaries at 1000 cd/m² clip to pure BT.709 primaries.
    Phase 1 needs gamut compression (BT.2407-like or a smooth chroma compression) before the clip.
12. **Range edges are clipped at decode** (R'G'B' clamped to [0,1]), so limited-range super-whites
    are lost; the same clamp feeds screen/overlay/soft-light, which are defined on [0,1] while
    scene-linear values can exceed 1. Both need an explicit policy.
13. **`.cube` files carry no colour space**, so "LUT applied in the specified space" (§8) has to be a
    user/package parameter (`Effect::Lut3d { space }` here; this LUT expects BT.709-OETF input).
14. **Linear-light dissolve and blur look different from gamma-space NLE defaults** (brighter
    mid-dissolve, different halos). Correct per §8, but worth a UI note or an optional
    "perceptual" mode.
15. **Chroma siting** (MPEG-2 "left" here, bilinear upsampling) must come from the probe
    (`chroma_sample_location`); 4:2:0 HDR sources commonly use top-left siting.

**Scene graph / interfaces**

16. The "every node renders a premultiplied linear texture of its intrinsic size" contract made
    groups, shapes, transitions and the text placeholder uniform. WP-0F.02 only adds a `Text` pass
    executor; WP-0G.01 can emit `Scene` JSON (`Group`/`Shape`/`Text` nodes) and render it with
    `s4 render --scene-json`.
17. PNG exports carry `cICP` (H.273 primaries/transfer/range) so frames are self-describing; the
    MP4 path should map the same tags to `-color_primaries/-color_trc/-colorspace/-color_range`.

**Pending**

* H.264/MP4 export (pipe raw frames to `ffmpeg`, ffprobe check of 300 frames) once the FFmpeg
  bundle exists (WP-0A.06). `s4 export` prints the intended command.
* EXR readback; lavapipe/Metal runs (WP-0F.03 on reference machines); HLG input; text (WP-0F.02).
