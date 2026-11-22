# Spike S4 — render graph, linear-light compositor, LUT, tone-map, headless export (WP-0F.01) and animated text (WP-0F.02)

> **Throwaway spike** (docs/wbs/P0-foundations.md). Own Cargo workspace, excluded from the root
> workspace, never imported by `crates/`. Only the findings below carry forward into
> `cutweave-render` (Phase 1). Architecture: §8 (render pipeline), §11 (captions), §20 (golden
> tolerances). **WP-0F.02 (text: MSDF atlas vs Vello) is [its own section](#wp-0f02--text-animated-captions-via-an-msdf-atlas-and-vello)**,
> with the comparison table and the ADR-007 recommendation.

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
cargo test                                   # 60 unit + 7 GPU + 7 text tests (goldens on every adapter present)
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
S4_UPDATE_GOLDENS=1 cargo test --test text text_golden                    # rewrite the text sets
```

Text (WP-0F.02) commands are listed [in its section](#how-to-run-text).

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
* **`Text { text, font, size_px, fill_srgb, box_size, align, line_height, outline, shadow, spans,
  renderer }`** (WP-0F.02; the last six are optional serde fields). Unlike the other nodes, text is
  rasterised *through* the layer transform at canvas resolution (see the WP-0F.02 section);
  effects, blend modes and caching still apply.

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
   (8.1–8.5 ms) shows there is no headroom yet for captions on top (WP-0F.02: an MSDF caption
   drawn in place adds ≈ 0.1 ms on this iGPU, a Vello one ≈ 3–6 ms; see the WP-0F.02 section).
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
    groups, shapes, transitions and the text placeholder uniform. WP-0F.02 kept it for text but
    rasterises text through the layer transform (and in place for plain MSDF layers): see its
    section. WP-0F.01 note, kept for history: 0F.02 was to add only a `Text` pass
    executor; WP-0G.01 can emit `Scene` JSON (`Group`/`Shape`/`Text` nodes) and render it with
    `s4 render --scene-json`.
17. PNG exports carry `cICP` (H.273 primaries/transfer/range) so frames are self-describing; the
    MP4 path should map the same tags to `-color_primaries/-color_trc/-colorspace/-color_range`.

**Pending**

* H.264/MP4 export (pipe raw frames to `ffmpeg`, ffprobe check of 300 frames) once the FFmpeg
  bundle exists (WP-0A.06). `s4 export` prints the intended command.
* EXR readback; lavapipe/Metal runs (WP-0F.03 on reference machines); HLG input.

---

## WP-0F.02 — text: animated captions via an MSDF atlas and Vello

```text
TextNode (frame's spans) ──layout (cosmic-text, cached)──► TextLayout ──resolve(spans)──► ResolvedText
                                                                                            │
     ┌──────────────────────────────────────────────────────────────────────────────────────┤
     ▼                                       ▼                                              ▼
 MTSDF atlas (own generator)          vello::Scene (glyph runs,                  text::raster (CPU reference:
 + instanced quads, 1 draw:           stroke outline, gradient karaoke)          exact outlines, 8 sub-scanlines,
 shadow → outline → fill layers       → Vello 0.12 → RGBA8 → resolve             exact horizontal coverage)
 (TextOver: straight onto backdrop)     → composite
```

The caption demo (`timeline::caption_scene`, on top of the WP-0F.01 demo) shows
*"Captions that pop, word by word, right on the beat"* in Bricolage Grotesque 800, white with a
6 px dark outline and a soft drop shadow: each word **pops in** at its start frame (scale
0.55 → ease-out-back overshoot → 1, fade, small rise) and the **active word gets a karaoke sweep**
to marigold (`text::caption::pop_karaoke`, a pure function of the frame). Other scenes:
`multiscript_scene` (Latin + Arabic RTL with a right-to-left sweep + CJK pop + emoji),
`glyph_storm_scene` (N glyphs with per-glyph rotation/scale/offset/colour/opacity),
`crisp_scene` (fill-only, for the CPU reference) and `shimmer_scene` (sub-pixel motion).

### How to run (text)

```sh
cargo run --release --bin s4 -- text --scene caption --frame 64 --out c.png            # MSDF caption on the demo
cargo run --release --bin s4 -- text --scene caption --renderer vello --out v.png      # same, Vello
cargo run --release --bin s4 -- text --scene multiscript --out m.png --crop 40,380,900,320,2   # + zoom crop
cargo run --release --bin s4 -- text --scene crisp --width 3840 --height 2160 --size 200 --cpu --out ref.png
cargo run --release --bin s4 -- text --scene crisp --out a.png --atlas-out atlas.png   # dump the MTSDF atlas
cargo run --release --bin s4 -- text-bench --all-adapters [--plain] [--glyphs 1000]    # 1000-glyph CPU/GPU/memory
cargo run --release --bin s4 -- text-crisp --width 3840 --height 2160 --size 200 --all-adapters --diff-dir d [--em 96]
cargo run --release --bin s4 -- text-shimmer --all-adapters                            # sub-pixel motion
cargo run --release --bin s4 -- atlas-bench [--em 64] [--weight 400]                   # MTSDF generation rate
cargo run --release --bin s4 -- bench --all-adapters --captions msdf --passes          # §19 budget scene (or vello)
```

`S4_VELLO_CPU=1` enables Vello's CPU-stage mode (the only way it runs on WARP, ≈ 10 s/frame).

### Design

* **Fonts (`text/fonts.rs`)**: only bundled SIL OFL 1.1 faces, never system fonts (determinism).
  Bricolage Grotesque and Instrument Sans are the UI's own woff2 files (`packages/ui/src/fonts`,
  decoded at load with `wuff`); Noto Sans Arabic, Noto Sans SC and Noto Emoji (monochrome) are
  7–43 KB fontTools subsets of the google/fonts TTFs to the sample text, keeping GSUB/GPOS and the
  variation tables, with their OFL texts in `assets/fonts/`. Unknown families fall back to
  Instrument Sans.
* **Shaping (`text/layout.rs`)**: cosmic-text 0.19 (harfrust inside; std only, no swash, no
  fontconfig): bidi, Arabic joining, line breaking, alignment, fallback across the bundled faces,
  variable `wght` carried into the glyph key. Layouts are cached by text/font/size/box: an
  animated caption re-resolves its spans every frame but is shaped once.
* **MTSDF generator (`text/msdf.rs`, ≈ 600 lines + tests)**: written for the spike after
  msdfgen: simple edge colouring (seeded, deterministic), pseudo-distance channels, the
  overlapping-contour combiner (variable fonts keep overlapping contours), a sign correction
  against the true nonzero fill, legacy clash correction plus a sub-texel interpolation check
  against the true SDF. RGB = MSDF (sharp fill corners), **A = true SDF** (round outlines, soft
  shadows). 48 px/em, range 12 px (±0.125 em, which is also the outline-width limit). *`fdsm` 0.8
  (MIT) was evaluated instead*: it generates MSDF and has a scanline sign correction, but no error
  correction, no MTSDF alpha and no overlap combiner, writes into `image::RgbImage` and pulls
  `nalgebra`. It would have needed the same additions, so the generator is our own.
* **Atlas (`text/atlas.rs`)**: on-demand, missing glyphs generated in parallel and packed in
  sorted key order into a shelf-packed RGBA8 1024 × (256…8192) texture. The shader clamps every
  bilinear tap to the glyph's cell, so atlas placement never changes pixels.
* **Animation model**: animation is not in the node. Animators emit per-frame `TextSpan`s (byte
  range, per-span or per-glyph pivot, offset/scale/rotation, opacity, fill override, karaoke
  progress), so the node stays a pure description of one frame, hashed into the pass cache key.
* **MSDF pass (`text_gpu.rs`, `shaders/text.wgsl`)**: one instanced draw (storage-buffer
  instances, 112 B each) ordered shadow layer → outline layer → fill layer, so a neighbour's
  outline never covers a fill; exact-weight bilinear `textureLoad` taps; coverage from the field
  scaled by the instance's screen scale; karaoke sweep tested in layout space with an `fwidth`
  ramp; premultiplied linear output blended with fixed-function "over".
* **Canvas-space text**: text layers are rasterised *through* the layer transform at canvas
  resolution, never rendered at box size and resampled. Plain (normal blend, no effects) MSDF
  layers lower to **`TextOver`**: the glyphs are drawn straight onto the backdrop texture (moved
  from its producer when this pass is its last reader, else onto a copy), so there is no
  intermediate, no clear and no full-frame composite. Other text layers render into a texture of
  the transformed box's pixel bounds, composited with an integer translation (an exact copy).
* **Vello variant (`text/vello_scene.rs`)**: the same `ResolvedText` as glyph runs (one run per
  word or glyph sharing transform and brush), outline = round-join stroke under the fill, karaoke
  = hard-stop linear gradient in layout space, colours handed over as linear values. Vello
  renders into an RGBA8 storage texture with its own queue submission (bracketed by timestamp
  marker passes); a compute pass re-premultiplies it into `Rgba16Float`; then the normal composite.
* **CPU reference (`text/raster.rs`)**: exact outlines (same ttf-parser geometry), flattened to
  1/100 px, nonzero rule on 8 sub-scanlines per row with exact horizontal coverage (no 1/8 px
  staircase under horizontal motion, unlike an 8 × 8 grid). Fill, spans and karaoke; not outline
  or shadow. `cpu_ref` uses it for text layers.

### Results (informal; dev machine as above, one run each, no clock pinning)

**MSDF vs Vello at a glance** (details below):

| | MSDF atlas (own) | Vello 0.12 |
|---|---|---|
| 1000 animated glyphs + outline + soft shadow, text GPU p50, AMD iGPU (Vulkan / DX12) | **0.71 / 0.70 ms** | 8.5 / 18.6 ms |
| same, RTX 3080 (Vulkan / DX12) | **0.063 / 0.090 ms** | 5.2 / 6.0 ms |
| same, WARP | 33 ms | **crashes WARP** (unsupported) |
| 1000 glyphs fill only, AMD iGPU (Vulkan / DX12) | **0.26 / 0.30 ms** | 4.8 / 6.5 ms |
| CPU per frame for 1000 glyphs (resolve + instances / scene encode), p50 | **0.18–0.28 ms** (+ animator 0.1, graph 0.07) | 1.4–2.4 ms |
| Caption on the 1080p demo, AMD iGPU: text pass / whole frame p50 | **0.07–0.09 ms / 6.4–6.7 ms** (no caption: 6.5–7.1) | 1.1–1.7 ms + a composite / 9.2–12.8 ms |
| GPU memory (allocator delta vs the same frame without text) | **1.6 MB** (atlas 1 MB) | ≈ 210 MB |
| Crispness, 4K 200 px: SSIM of the text box (PSNR) vs CPU reference | 0.9976 (36.4 dB) | **0.9994 (42.8 dB)** |
| Crispness, 1080p 72 / 48 / 24 px | 0.9988 / 0.9992 / 0.9994 | 0.9993 / 0.9994 / 0.9995 |
| Sub-pixel motion: per-phase PSNR spread over 16 phases | 0.47 dB | 0.33–0.35 dB |
| Same adapter: cold vs warm vs repeat | bit-identical | bit-identical |
| Across vendors (NVIDIA / AMD / WARP), text goldens | ≤ 1 LSB (79–84 dB) | up to 28 codes (64 dB, ΔE max 9.7) |
| Soft shadow | yes (true SDF) | hard only (no path blur) |
| First frame (device + pipelines) | + 0.02–0.06 s (atlas generation) | + 0.7 s (Vulkan) … 7.6–9 s (DX12 shader compile) |

**1000 animated glyphs** (`s4 text-bench --all-adapters`, 1920 × 1080, per-glyph rotation ±18°,
scale ±20 %, offset, hue and opacity every frame, 120 frames; MSDF draws 3000 quads styled, 1000
fill-only; "text GPU" = the text pass, or Vello's bracketed submission + resolve; ms):

| Adapter | Backend | MSDF styled p50 / p95 | Vello styled p50 / p95 | MSDF fill p50 / p95 | Vello fill p50 / p95 | MSDF frame | Vello frame |
|---|---|---|---|---|---|---|---|
| AMD Radeon iGPU | Vulkan | **0.710 / 0.933** | 8.45 / 10.4 | **0.258 / 0.522** | 4.79 / 7.65 | 2.13 | 11.3 |
| AMD Radeon iGPU | DX12 | **0.697 / 1.011** | 18.6 / 20.4 | **0.301 / 0.534** | 6.52 / 9.47 | 2.05 | 21.6 |
| RTX 3080 Laptop | Vulkan | **0.063 / 0.065** | 5.19 / 5.89 | **0.026 / 0.027** | 1.74 / 2.88 | 0.16 | 5.38 |
| RTX 3080 Laptop | DX12 | **0.090 / 0.091** | 6.03 / 11.2 | **0.035 / 0.035** | 2.47 / 5.73 | 0.18 | 6.54 |
| WARP | DX12 | 33.3 / 38.8 | (crash) | 11.3 / 13.4 | (crash) | 51.0 | — |

Before `TextOver` (text into a canvas-sized intermediate, then a full-frame composite) the AMD
iGPU needed 1.3 ms of text GPU and a 4.2–4.5 ms frame for the same styled storm; ≈ 0.6 ms of that
was the clear/store of the 16 MB intermediate.

**Crispness** (`s4 text-crisp`; fill-only `crisp_scene` "Ag@% Cutweave / 字幕 مرحبا 🎬" vs the
CPU reference; SSIM over the text box / PSNR / max |Δ|). Identical to 5 digits on every adapter:

| Scene | MSDF 48 px/em | MSDF 64 px/em | MSDF 96 px/em | Vello |
|---|---|---|---|---|
| 3840 × 2160, 200 px glyphs (WBS scene 2) | 0.99760 / 36.35 dB / 240 | 0.99845 / 39.4 dB | 0.99906 / 42.4 dB | 0.99941 / 42.77 dB / 111 |
| 1920 × 1080, 72 px | 0.99883 / 40.12 dB / 240 | | | 0.99933 / 41.7 dB / 78 |
| 1920 × 1080, 48 px / 24 px | 0.99923 / 0.99942 | | | 0.99941 / 0.99954 |

Zoom crops (`--diff-dir`): Latin edges match the reference within a few codes. The MSDF max |Δ|
of 240 comes from features thinner than one atlas texel (the slit inside a heavy `@`, the stripes
of the 🎬 emoji), which bulge or break at 4× magnification (fixed by 96 px/em); tiny counters of
weight-800 glyphs (`a`) also fill in at caption sizes. Vello has no such artefacts.

**Sub-pixel motion** (`s4 text-shimmer`: a 40 px line moved through 16 phases of one pixel, 1080p):

| Path | ink range | centroid wobble | PSNR vs reference: mean / spread |
|---|---|---|---|
| CPU reference | 0.018 % | 0.029 px | — |
| MSDF (every adapter) | 0.05 % | 0.13–0.14 px | 52.7 dB / 0.47 dB |
| Vello (NVIDIA / AMD) | 0.07 / 0.13 % | 0.13 / 0.24 px | 49.7 dB / 0.33–0.35 dB |
| MSDF rendered at box size, then resampled by the layer transform | 0.015 % | 0.03 px | 42.6 dB / **13.3 dB** |

Both canvas-space paths are stable from frame to frame (their small centroid wobble is the sum of
tiny, phase-dependent edge errors: field interpolation for MSDF, curve flattening for Vello). The
resampled variant tracks position but its sharpness pumps by 13 dB between integer and half-pixel
phases: that is the visible shimmer, and why text is rasterised through the layer transform.

**MTSDF generation** (`s4 atlas-bench`): all 745 glyphs of the 5 bundled faces at wght 800 in
0.71 s on 16 threads = **1,050 glyphs/s** at 48 px/em (8 MB atlas), 610 glyphs/s at 64 px/em;
identical atlas bytes across runs. The storm's ≈ 25 glyphs take 17–30 ms cold.

### Acceptance criteria (WP-0F.02)

| Criterion (WBS + brief) | Status | Evidence |
|---|---|---|
| cosmic-text shaping: Latin + one RTL + one CJK sample + emoji → glyph runs | **Met** | `layout::tests::shapes_latin_rtl_cjk_and_emoji_with_fallback`, `multiscript_scene` |
| MSDF atlas renderer (own, msdfgen-compatible; fdsm evaluated) with fill, outline, shadow, per-word pop, karaoke sweep, per-glyph transform/opacity | **Met** | `text/msdf.rs`, `text_gpu.rs`, `text.wgsl`; caption goldens |
| The same caption via Vello 0.12, composited by the graph | **Met** on hardware adapters; **unsupported on WARP** (crash) | `text/vello_scene.rs`; `text_*_vello` goldens |
| Scenes: animated caption at 1080p; 4K with 200 px glyphs; 1000 animated glyphs | **Met** | `caption_scene`, `crisp_scene`, `glyph_storm_scene` |
| CPU (layout + instance build) and GPU time per frame, both paths, per adapter | **Met** | `s4 text-bench`; tables above |
| 1000 animated glyphs: CPU instance build < 1 ms | **Met** for MSDF (≤ 0.46 ms p95 with outline + shadow); Vello 1.4–2.4 ms p50 styled (0.4–0.6 fill-only) | `text-bench` |
| 1000 animated glyphs < 1 ms GPU (epic bar, judged in 0F.03 on M1 / Intel Xe) | **Met** for MSDF on the AMD iGPU (0.70 ms p50 styled, 0.26–0.30 fill); Vello **fails** (4.8–18.6 ms) | `text-bench` |
| Crispness: 4K vs supersampled CPU rasterisation, SSIM ≥ 0.98 for the chosen path; diff images | **Met** (MSDF 0.9976, Vello 0.9994) | `s4 text-crisp --diff-dir`; `text_is_crisp_against_the_supersampled_outline_reference` |
| Quality at 1080p and 4K (zoom crops); shimmer under sub-pixel motion | **Met** | tables above; `canvas_space_text_tracks_sub_pixel_motion` |
| Memory | **Met** | allocator-report deltas above |
| Determinism: same adapter bit-identical | **Met** (both paths) | `text_is_bit_identical_cold_warm_and_repeated_on_one_adapter`; goldens |
| Golden tests for both paths | **Met** (5 adapter sets; WARP: MSDF only) | `text_golden_frames_match_per_adapter_references` |
| Snapshot test for a caption frame | **Met differently**: per-adapter golden PNGs (bit-identical on the same adapter, §20 across adapters) instead of `insta` hashes, as in WP-0F.01 | `tests/golden/*/text_*.png` |
| `cargo test`, `cargo clippy --all-targets -D warnings` in the spike; root `just check` | **Met** | 60 unit (+ 1 ignored diagnostic) + 7 GPU + 7 text tests; `just check` passes |

Deviations: built inside `spikes/s4-compositor` (the brief) rather than `spikes/s4-text` (WBS);
the reference uses 8 sub-scanlines with exact horizontal coverage instead of an 8 × 8 grid (at
least as accurate, and needed for the motion test); emoji are the monochrome Noto Emoji outlines
(colour emoji are WP-3A.06).

### Findings for ADR-007 and Phase 3 (`cutweave-text`, the text pass)

1. **MSDF is ≈ 10–100× cheaper on the GPU than Vello for caption workloads** and fits the iGPU
   budget: a 10-word caption costs 0.07–0.09 ms on the AMD iGPU, 1000 animated styled glyphs
   0.7 ms. Vello's per-frame cost is a full compute pipeline plus an RGBA8 intermediate, a resolve
   and a full-frame composite: 1–1.7 ms for one caption on the iGPU, which alone breaks the §19
   "1080p60 with captions" budget on top of the 6.5 ms demo.
2. **Draw text in place.** Rendering text into its own canvas-sized `Rgba16Float` target cost
   ≈ 0.6 ms of clear/store plus a ≈ 1 ms full-frame composite on the iGPU. `TextOver` (glyph quads
   blended straight onto the backdrop) halved the storm's frame time and cut text memory from
   32 MB to 1.6 MB. This confirms WP-0F.01 finding 5a for Phase 1: composite with render passes
   and fixed-function blending, so cost follows the drawn area.
3. **Rasterise text through the final transform; never resample glyphs.** Rendering at box size
   and resampling makes sharpness pump with the sub-pixel phase (13 dB PSNR spread); canvas-space
   rasterisation is stable (≤ 0.5 dB). Text effects then run in canvas pixels.
4. **MSDF quality limits are resolution limits.** 48 px/em is crisp for Latin up to ≈ 4×
   magnification (SSIM 0.9976 at 4K / 200 px, a pass) but loses features thinner than one texel
   (heavy `@`, emoji detail, tiny counters at wght 800). 96 px/em reaches Vello's 4K quality
   (0.9991). Use a per-glyph em tier (48 px normally; 96–128 px when a glyph is drawn more than
   ≈ 3× its atlas size, or for CJK, emoji and complex outlines) rather than a second renderer.
5. **Variable fonts need an overlap-aware generator.** The UI's variable fonts keep overlapping
   contours, and a naive MSDF shows seams and holes. The overlapping-contour combiner, a sign
   correction against the true fill and an interpolation check removed every sign error at
   interpolated sample points (16.7 M samples: the first 120 glyphs of every face, wght 400 and
   800) except sub-texel features. 3A.04 should run this test on every glyph of every bundled font.
6. **Use MTSDF (alpha = true SDF).** Outlines and shadows from the true SDF get round joins and
   no miter spikes, and the true SDF bounds far-field MSDF artefacts in the fill. Outline width is
   limited by the distance range (±0.125 em at range 12 and 48 px/em); thick "MrBeast" outlines
   need a larger range (less 8-bit precision) or an RGBA16F atlas.
7. **Generation speed needs work in 3A.04**: 1,050 glyphs/s on 16 threads, against a P3 target of
   ≥ 2,000/s on the reference CPU. Per-texel cost is O(edges) plus a winding test; spatial culling
   of edges and a disk cache keyed by (font hash, glyph, wght, parameters) are the next steps.
   Generation is deterministic (identical atlas bytes) and placement-independent.
8. **Vello 0.12 in practice**: it crashes WARP (an access violation inside WARP while it executes
   Vello's pre-fine stages, on every standalone run, with FXC or DXC; `use_cpu` survives at ≈ 10 s
   per frame), so a Vello path has no Windows CI goldens. It also allocates ≈ 210 MB of GPU
   buffers, takes 7–9 s to compile its shaders on DX12 at first use, only targets straight-alpha
   RGBA8 (an 8-bit **linear** intermediate when fed linear colours, so dark tones can band), has
   no path blur (soft shadows would need a second render and a blur pass), submits its own
   command buffers (it cannot join the frame's encoder; timestamps need marker passes), and its
   output differs by up to 28 codes between NVIDIA and AMD.
9. **Determinism**: both paths are bit-identical on one adapter (cold vs warm caches, repeated
   frames, recycled textures). Exact-weight `textureLoad` taps keep MSDF within 1 LSB across
   NVIDIA / AMD / WARP and DX12 / Vulkan, so cross-adapter text goldens can use the compositor's
   tight tolerance (PSNR ≥ 50 dB, max |Δ| ≤ 2); Vello would need ≥ 30 codes.
10. **The CPU side is cheap with a layout cache.** Per frame for 1000 animated glyphs: animator
    0.1 ms, span resolve + instance build 0.2–0.3 ms, graph build (hashing 1000 spans) 0.07 ms.
    Shaping 1000 glyphs cold takes 0.6–1.8 ms and never happens per frame for captions. Vello's
    scene encoding takes 1.4–2.4 ms because per-glyph transforms become per-glyph runs.
11. **Animation as resolved spans** (per word or glyph: transform about a pivot, opacity, fill,
    karaoke progress) keeps the node a pure per-frame description, makes the pass cache key exact,
    and is what a QuickJS composition (WP-0G.01) or a caption-style package would emit.
12. **Platform notes**: a WGSL struct field cannot be named `target` (reserved keyword); several
    WARP devices used concurrently from test threads crash even with pipeline creation
    serialised, so the text GPU tests run serially; the `woff2` 0.3 crate no longer builds
    (bytes ≥ 1.10), so `wuff` decodes the UI's woff2; cosmic-text 0.19 pulls skrifa 0.40 while
    Vello pulls 0.49 (two copies).
13. **Open**: colour emoji (COLRv1 / CBDT through an RGBA atlas; Vello renders COLR natively, the
    one place it might still earn its keep), glow and gradient fills (same MTSDF, not built here),
    caption background boxes, M1 / Intel Xe numbers (WP-0F.03), and lavapipe (Vello on lavapipe
    is untested).

### Recommendation for ADR-007

**MSDF only, as the primary and only v1 text renderer**: cosmic-text + an MTSDF atlas, drawn as
instanced quads directly into the composite (no intermediate), with an em-size tier (48 → 96–128
px per em) for glyphs magnified beyond ≈ 3× or with sub-texel detail, plus an RGBA colour-glyph
atlas for emoji (3A.06). It meets every glyph criterion measured here on the AMD iGPU (1000
animated glyphs in 0.70 ms GPU and 0.3 ms CPU; 4K SSIM 0.9976, or 0.9991 with the 96 px tier), is
deterministic to ≤ 1 LSB across vendors, and runs on WARP for CI.

**Do not adopt Vello as the in-graph "large or complex glyph" path for v1**: it is 10–100× slower
per glyph, costs ≈ 210 MB, has no Windows CI story (the WARP crash), diverges across vendors and
cannot do soft shadows; its one quality advantage (exact coverage of fine detail) is matched by
the higher MSDF tier. Revisit Vello (or `vello_hybrid` / `vello_cpu`) in P3/P4 for COLRv1 emoji or
1000+ px display type if the MSDF tiers fall short. **skia-safe is not needed**: the glyph
criteria pass with MSDF. Once WP-0F.03 confirms the iGPU numbers on M1 and Intel Xe, §8 ("Vello
path for very large or complex glyphs") and WP-3A.06 should be amended accordingly.

### Notes for the integrator

* `TextNode` gained optional fields (`align`, `line_height`, `outline`, `shadow`, `spans`,
  `renderer`, all serde-defaulted, so WP-0F.01 scene JSON still parses). Struct literals with only
  the five original fields no longer compile: use `TextNode::new(text, font, size_px, fill_srgb,
  box_size)`. `PassOp::Text` gained `transform`, `PassOp::TextOver` is new, and `FrameStats` has
  `text` and `text_gpu_ns`. The rest of the public scene-graph API is unchanged.
* Unknown font families (e.g. `"Inter 700"` in `text_scene`) fall back to Instrument Sans.
* New spike dependencies (spike workspace only): `cosmic-text =0.19.0`, `vello =0.12.0`,
  `ttf-parser 0.25`, `wuff 0.2`. The Noto subsets in `assets/fonts/` are OFL-1.1 (license texts
  beside them) and may need a REUSE annotation if REUSE linting ever covers `spikes/`.
