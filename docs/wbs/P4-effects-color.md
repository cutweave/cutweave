# Cutweave — Phase 4 Work Breakdown: Effects, Transitions & Color → M4 Creative Parity

> Status: baseline 2026-11-22. Contract: [01-architecture.md](../01-architecture.md) (§8 render/color/masks, §15 plugins L1, §19 budgets, §20 testing). Roadmap: [04-roadmap.md](../04-roadmap.md#phase-4--effects-transitions--color-9-weeks-parallel-with-p3p5--m4). Epic codes: [05-work-breakdown.md](../05-work-breakdown.md). Plugin research: [research/07-plugin-architecture.md](../research/07-plugin-architecture.md) §2.
> Earlier-phase dependencies are named by epic (E0F … E2J). P4 runs in parallel with P3/P5; the only P3 dependency is the `cutweave-plugins` v0 skeleton (WP-3B.01) — see Notes.

## Phase goal

Creative parity with CapCut's visual toolbox, built entirely on the L1 plugin interface (built-ins are first-party plugins using the same interface third parties will use).

## Exit criteria (copied from roadmap)

**Exit (M4):** a "CapCut trend recreation" test suite (10 reference edits recreated using only built-ins) passes design review. All effects render identically in preview and export (golden frames). Each effect stays within its GPU budget on the iGPU reference machine.

## Epics (one-line goals)

| Epic | Goal |
|---|---|
| **4A WGSL effect framework** | `effect.toml` spec, loader/validator (naga), render-graph integration (multi-pass, keyframeable params, temporal taps), transitions engine, GPU budgets, commands, hot-reload, pack QA harness, browser UI. |
| **4B Transitions pack** | 45 transitions in 5 themed batches + design gate. |
| **4C Effects pack** | 54 video effects in 6 batches, the 10-edit "trend recreation" suite and design gate. |
| **4D Filters & LUTs** | Filter pack format + procedural LUT generator, 45 filters in 5 batches, design gate. |
| **4E Clip animations** | Preset-driven in/out/combo clip animations (layered over user keyframes), 28 presets. |
| **4F Color tools & scopes** | Color pipeline spec; basic adjustments, HSL, curves, wheels, `.cube` LUTs; waveform/vectorscope/histogram; Color + Scopes UI. |
| **4G Masks, keying, adjustment layers** | Shape masks, chroma/luma key, effect masks & track mattes, adjustment layers. |
| **4H Speed curves & frame blending** | Speed-graph retiming with presets, optical-flow slow-mo, audio handling, curve editor. |
| **4I Stabilization & tracking** | vidstab analysis + our own stabilizer, point and planar-lite tracking driving keyframes. |

## Critical path

`3B.01 (plugins skeleton, from P3)` → `4A.01 (effect.toml spec)` → `4A.02 (loader/validator)` → `4A.03 (render-graph integration)` → `4A.08 (pack QA harness)` → all content batches (`4B.*`, `4C.01–06`, `4D.*`) → `4C.07 (trend suite)` → `4C.08 (design gate)`.
Parallel chains: `4F.01 → 4F.02 → 4F.03/4F.04 → 4D.01`; `4G.01 → 4G.04/4G.05`; `4I.01/4I.03 → 4I.02/4I.04 → 4I.05`; `4H.01 → 4H.02/4H.03`. The trend suite (4C.07) additionally needs 4E, 4F, 4G, 4H, 4I merged. Longest chain ≈ 9 weeks with content batches parallelised over multiple agents.

## Definition of done — applies to EVERY WP (not repeated per WP)

- **DoD-CMD:** every new command/query uses `#[command]`/`#[query]` with `mcp = core|catalog`, appears in the generated schema/TS/docs, parity tests green; mutating commands have an undo/redo property test; **every new command is demonstrated by an MCP/`cutweave_execute` script test** (no screenshots; `render_frame` used to verify).
- **DoD-CI:** clippy `-D warnings`, nextest, cargo-deny, Biome/Vitest; wasm-safe crates still build for wasm32 (`cutweave-render` core must not gain native-only deps in its pure parts); licence headers, DCO.
- **DoD-GF:** golden frames on software adapters (WARP/lavapipe), PSNR ≥ 45 dB (ΔE00 ≤ 2 for colour-science tests) vs blessed reference; **preview == export** (full-quality preview frame bit-identical to export frame for the same `(rev,t)`); bit-identical across 2 runs (determinism).

## GPU budget policy (referenced by every content WP)

Reference iGPU = the §19 machine (Apple M1 / Intel Xe). Measured at **1080p, full quality**, per effect instance, p95 over 300 frames, GPU time from timestamp queries. Tiers (declared in `effect.toml` as `budget.tier`; numbers are proposals to be re-calibrated after Spike S4 — see Notes):

| Tier | Budget @1080p iGPU | Typical content |
|---|---|---|
| **S** | ≤ 0.5 ms | single pass, ≤ 8 texture taps |
| **M** | ≤ 1.2 ms | 2–3 passes or ≤ 32 taps (separable blur, bloom) |
| **L** | ≤ 2.5 ms | ≥ 4 passes / history taps; allowed only with justification |
| Transition | ≤ 1.5 ms | two inputs + progress (decode excluded) |
| Filter/LUT | ≤ 0.4 ms | 3D LUT, tetrahedral |

Auto-disable (4A.05) triggers at 2× budget for 30 consecutive frames. At ½/¼ preview resolution costs must scale ≥ 3×/10× down (pixel-proportional passes).

## Shared quality checklist — QC-FX (all effects, transitions, filters, animations)

1. `cutweave effect check` (or the kind-specific check) passes: schema valid, naga valid, static budget lint within tier.
2. Defaults look good out of the box; every meaningful param is `keyframeable`, has min/max/default/unit/description; params are **resolution-independent** (normalised to frame height/width) so ½-res preview ≈ full res (ΔE00 mean ≤ 1.5).
3. **Golden frames:** 3 frames per item (param default; param min; param max, or progress 0.25/0.5/0.75 for transitions) × 4 reference stills (colour chart, portrait, high-detail landscape, text chart) on software adapters; PSNR ≥ 45 dB vs blessed; `cutweave pack test` (4A.08) green.
4. **Preview == export** and **determinism** checks (4A.08) green; any randomness seeded from `(item id, frame)` only.
5. Correct under alpha (premultiplied), linear-light working space, 16:9 and 9:16, 1:1.
6. GPU budget measured on the iGPU reference ≤ declared tier (nightly job, 4A.05); static lint on hosted CI.
7. Metadata: name ≤ 24 chars, description, category, ≥ 3 tags, thumbnail (static + 12-frame animated webp via `cutweave pack thumbs`), licence header. **No trademarked names; no code copied from third-party shaders** (Shadertoy etc. are not GPL-safe); each `.wgsl` header states "original" or provenance + licence.
8. MCP: appears in `effect.list` with schema; a `cutweave_execute` script adds it by id with default params and `render_frame` differs from the baseline by PSNR ≤ 40 dB.
9. Transitions only: **identity at the ends** (progress 0 == A, progress 1 == B, bit-exact), `reverse`/`direction` param where meaningful, duration range 0.1–3 s (default 0.5 s), correct with alpha clips, auto audio crossfade.

## WPs

---

# Epic 4A — WGSL effect framework

### WP-4A.01 — `effect.toml` specification, schema & parser
- **Area:** crates/cutweave-plugins (`effect`), docs/specs/effect-toml.md, schema/effect.schema.json
- **Depends on:** WP-3B.01, E0F
- **Size:** M
- **Executor:** opus
- **Scope:** normative spec + JSON Schema + parser. `kind = effect|transition|generator|blend`; params `float|int|bool|color|vec2|enum|curve|gradient|image|angle|point` with `min/max/default/step/unit/group/ui/keyframeable/preview_only`; inputs `source`, `secondary` (transitions: `from`,`to` + auto `progress`), `mask`, **`history = N`** (≤ 8 previous frames of the same clip via the frame cache — the deterministic replacement for persistent feedback buffers); passes `{shader, reads, writes, scale, format, iterations}`; built-ins `time, resolution, frame, fps, progress, seed`; `color_space = linear|display`, `alpha = premult|straight`; `budget{tier}`; `data_input` hook (used by stabilisation/tracking); metadata (category, tags, thumbnail, i18n name keys). Includes three reference effects in the spec (separable blur, RGB split, cross-dissolve) that must compile in CI.
- **Interfaces:** `cutweave_plugins::effect::{EffectManifest, ParamDef, PassDef}`; `contributes.effects` entries from 3B.01.
- **Acceptance criteria:** schema snapshot; parser rejects ≥ 25 malformed manifests with path-accurate diagnostics (file:line); round-trip parse→serialise→parse identical; spec examples compiled in CI; spec reviewed by a human maintainer (recorded in PR).
- **Tests:** unit, fixtures, insta schema snapshot.
- **Out of scope:** shader compilation (4A.02), WASM effect metadata worlds.

### WP-4A.02 — WGSL loader, validator & host prelude
- **Area:** crates/cutweave-plugins (`effect::compile`), crates/cutweave-render
- **Depends on:** WP-4A.01
- **Size:** M
- **Executor:** opus
- **Scope:** naga parse + validate at install and load; reject `enable` extensions, push constants, undeclared bindings, unbounded loops (only const-bounded `for`, ≤ 64 iterations), instruction/size caps, texture format/size whitelist, passes ≤ 8; **host-generated prelude** (uniform struct from params, bind groups: 0 uniforms, 1 textures+samplers, 2 LUT/aux) so authors write only `fn fx_main(uv, ctx) -> vec4<f32>`; diagnostics mapped back to author line/col (prelude offset removed); pipeline cache keyed by hash(source, layout).
- **Interfaces:** `compile(manifest, sources) -> CompiledEffect | Vec<Diagnostic>`; CLI `cutweave effect check <dir>`.
- **Acceptance criteria:** corpus of 30 hostile/invalid shaders (infinite loop, OOB indexing, forbidden extensions, huge arrays, recursion, undeclared bindings) all rejected within 50 ms each; 10 valid shaders pass; diagnostics point to the right line in 100% of corpus cases; second load hits the pipeline cache (< 5 ms).
- **Tests:** corpus tests, fuzz-lite on WGSL mutations (no panics), unit.
- **Out of scope:** runtime hang prevention (4A.05), UI.

### WP-4A.03 — Render-graph integration: effect instances, multi-pass, temporal taps
- **Area:** crates/cutweave-render (`effects`), crates/cutweave-model
- **Depends on:** WP-4A.02, E1F, E2C
- **Size:** M
- **Executor:** opus
- **Scope:** `EffectInstance` evaluation at `t` (params `Animatable` → uniform ring buffer); pass allocation from a texture pool (scale/format); fusion of compatible single-pass pointwise effects into one pass; `history` taps served from the frame cache (deterministic; effect disabled gracefully if frames unavailable); effect order/enable; per-effect param-hash in render-graph cache keys; resolution-independent unit scaling for the quality ladder; effects on clips (adjustment layers in 4G.05, transitions in 4A.04).
- **Interfaces:** `cutweave_render::effects::{EffectRuntime, EffectNode}`; `EffectRef` scene-graph nodes from P3's `SceneGraph` become real (removes the `effect.unavailable` stub).
- **Acceptance criteria:** ½-res preview vs full export of resolution-independent effects ΔE00 mean ≤ 1.5; 10 stacked trivial effects fuse into ≤ 3 passes (test on graph); two runs bit-identical; seeking to a frame with `history` taps gives the same pixels as playing into it (determinism test); GF for the 3 reference effects.
- **Tests:** golden, graph-shape unit tests, determinism, bench.
- **Out of scope:** transitions, GPU timing.

### WP-4A.04 — Transitions engine
- **Area:** crates/cutweave-render, crates/cutweave-model, crates/cutweave-ops
- **Depends on:** WP-4A.03, E2A
- **Size:** M
- **Executor:** opus
- **Scope:** `Transition{a,b,kind,duration,alignment(center|start|end),params}` evaluation: render `from`/`to` to textures, run the transition pass with eased `progress`; **handles policy** when media beyond the cut is unavailable: `freeze|clamp` (shorten to available handle); single-clip in/out transitions (against transparent/black); max duration ≤ min(a,b) adjusted automatically; alpha-aware; speed-change aware; automatic audio crossfade (equal-power) with `audio: crossfade|none`.
- **Interfaces:** `cutweave_render::transition::{TransitionNode}`; audio hook into `cutweave-audio` crossfades.
- **Acceptance criteria:** identity at ends for every shipped transition (harness check, bit-exact); transition across a 2× speed clip is visually continuous (A/B frame difference at cut boundary ≤ 1 frame step); audio crossfade energy constant (±1 dB) in test; GF for 3 reference transitions; no frame hitch when entering/leaving the transition region (render-graph rebuild < 2 ms).
- **Tests:** golden, identity-at-ends, audio crossfade test, proptest on duration clamping.
- **Out of scope:** content, UI.

### WP-4A.05 — GPU budget enforcement, profiler & benchmark CLI
- **Area:** crates/cutweave-render (`profiler`), crates/cutweave-cli, .github/workflows
- **Depends on:** WP-4A.03
- **Size:** M
- **Executor:** sonnet
- **Scope:** timestamp-query profiler per effect pass (fallback: fence timing), rolling p95; budget tiers from policy above; **auto-disable** after 30 consecutive frames > 2× budget with event `effect.disabled{id,reason}` and manual re-enable; `cutweave effect bench <id|--all> [--res 1080p|4k] [--json]` → `{p50,p95,tier,pass_count,taps}`; **static budget lint** (pass count × texture-tap count from naga analysis must fit tier) runs on hosted CI; nightly workflow on self-hosted iGPU runners (Intel Xe + Apple M1) compares measured p95 against declared tier and fails on regression.
- **Interfaces:** events `effect.disabled`; CLI `cutweave effect bench`; `render.stats` query gains per-effect timings.
- **Acceptance criteria:** a deliberately slow test effect (100 taps × 6 passes) is auto-disabled within 1.5 s of playback and the UI receives the event; static lint flags it in CI; bench output stable (p95 CV < 10% across 3 runs); nightly job produces a JSON artifact per runner.
- **Tests:** unit, integration with synthetic slow effect, CI workflow dry-run.
- **Out of scope:** per-effect optimisation work (owned by content WPs).

### WP-4A.06 — Effect & transition commands and queries
- **Area:** crates/cutweave-ops, crates/cutweave-api
- **Depends on:** WP-4A.03, WP-4A.04
- **Size:** M
- **Executor:** sonnet
- **Scope:** `effect.add{item|track,effect,params?,index?}`, `effect.remove`, `effect.reorder`, `effect.set_param{instance,param,value|keyframes}`, `effect.toggle`, `effect.reset`, `effect.copy_to{instance,targets}`; `transition.add{a,b|item+edge,kind,duration,alignment,params}`, `transition.set`, `transition.remove`, `transition.apply_to_all{track,kind,duration}`, `transition.set_default`; queries `effect.list{kind,category,query,tags}`, `effect.describe{id}` (param schema + budget + thumbnail ref).
- **Interfaces:** MCP core `effect_add`, `transition_add`; the rest catalog/`cutweave_execute`; `effect.describe` supplies param schemas to the schema-driven inspector and MCP.
- **Acceptance criteria:** MCP script: add blur + transition then `render_frame` differs from baseline and matches golden; undo/redo property tests including keyframed params; `apply_to_all` over 50 cuts is one undo step and < 100 ms; invalid param → `Invalid{path,msg}`.
- **Tests:** proptest, MCP e2e, schema snapshot.
- **Out of scope:** filters (4D), masks (4G).

### WP-4A.07 — Hot-reload & author tooling
- **Area:** crates/cutweave-plugins, crates/cutweave-cli
- **Depends on:** WP-4A.02, WP-4A.06
- **Size:** M
- **Executor:** sonnet
- **Scope:** dev-plugin directory watcher (`CUTWEAVE_DEV_PLUGINS` / `--dev-plugin`), recompile on save, errors as structured `plugin.diagnostics` events, last-good shader kept running with an on-canvas error badge; `cutweave effect preview <path> --param k=v --t 0.5 --out png`; `cutweave effect sweep <id> --param radius --steps 5` (contact sheet PNG an agent can inspect); `cutweave effect new <name> [--kind transition]` scaffold with a working template.
- **Interfaces:** event `plugin.diagnostics`; CLI subcommands above (superseded by `cutweave plugin …` in Phase 6C, aliases kept).
- **Acceptance criteria:** save-to-pixels < 300 ms for a single-pass effect on the reference machine; syntax error leaves previous output on screen and emits a diagnostic with correct line/col; scaffolded effect passes `effect check` and renders; `sweep` contact sheet has the requested number of cells.
- **Tests:** integration (file watcher), CLI golden outputs.
- **Out of scope:** publishing, signing.

### WP-4A.08 — Pack QA harness (`cutweave pack test`)
- **Area:** crates/cutweave-cli, tools/pack-qa, .github/workflows
- **Depends on:** WP-4A.03, WP-4A.04
- **Size:** M
- **Executor:** sonnet
- **Scope:** for each effect/transition/filter/clip-animation in a plugin dir: render defined frames on 4 reference stills (colour chart, portrait, detailed landscape, text chart; CC0 or synthetic, committed small) via headless software adapter; compare to blessed goldens (`--bless` requires a PR label); **preview-vs-export parity** (same frame through both pipelines at full quality must be bit-identical); determinism (2 runs, equal hash); **transition identity-at-ends**; static budget lint; metadata lint (name, description, ≥ 3 tags, category, licence header, banned-trademark word list); thumbnail generation (`thumb.webp` 256 px + 12-frame animated webp); markdown report; CI job per changed plugin.
- **Interfaces:** `cutweave pack test <dir> [--bless] [--report out.md]`, `cutweave pack thumbs <dir>`.
- **Acceptance criteria:** catches 5 seeded faults (non-deterministic effect, preview≠export divergence, transition not identity at ends, missing metadata, over-budget static cost); a 54-effect pack tests in ≤ 10 min on CI; report links failing frames.
- **Tests:** harness self-tests with seeded-fault plugins.
- **Out of scope:** iGPU timing (4A.05 nightly).

### WP-4A.09 — Effects/Transitions/Filters browser & inspector UI
- **Area:** packages/editor, packages/timeline
- **Depends on:** WP-4A.06, WP-4A.08, E2H
- **Size:** M
- **Executor:** sonnet
- **Scope:** shared browser component for Effects, Transitions and Filters tabs: categories, search, tags, favourites, animated hover thumbnails (pre-rendered from `pack thumbs`, on-demand `render.frame` fallback at 160 px cached); drag onto clip / edit point / gap or double-click to apply to selection; inspector "Effects" section (reorder drag, toggle, delete, reset, schema-driven params with keyframe diamonds from E2C, "disabled: over budget" badge); timeline transition handles (drag to extend, context menu: change type, apply to all, set default).
  - Settings > Editing: duration of new transitions (`editing.transition_duration`, 0.5 s); applying a transition uses it (catalog `docs/reference/settings.md`).
- **Interfaces:** consumes `effect.*`, `transition.*`, `color.apply_filter`; registers tabs in the panel registry.
- **Acceptance criteria:** Playwright e2e: drag "Cross Dissolve" to a cut → handle appears → drag duration → export frame matches golden; applying 5 effects and undoing 5 times restores; browser with 150 items scrolls ≥ 55 fps and shows thumbnails < 300 ms warm.
- **Tests:** Playwright-Electron, Vitest, visual regression.
- **Out of scope:** Color panel (4F.06), animations tab (4E.05).

---

# Epic 4B — Transitions pack (45)

*All items follow QC-FX incl. item 9; tier: Transition (≤ 1.5 ms). Plugin dir `plugins/transitions-core`, ids `cutweave.tr.<slug>`.*

### WP-4B.01 — Transitions batch A: dissolves & fades ×9
- **Area:** plugins/transitions-core
- **Depends on:** WP-4A.04, WP-4A.08
- **Size:** S
- **Executor:** sonnet
- **Scope:** **Cross Dissolve**, **Dip to Black**, **Dip to White**, **Dip to Colour** (colour param), **Additive Dissolve**, **Film Dissolve** (luma-weighted, gamma-aware), **Blur Dissolve**, **Noise Dissolve** (seeded threshold), **Flash Cut** (white flash peak at midpoint).
- **Interfaces:** `cutweave.tr.*`.
- **Acceptance criteria:** QC-FX for all 9; cross dissolve is energy-correct in linear light (mid-grey A=0.2/B=0.8 → at progress 0.5 equals 0.5 ± 1e-3 linear).
- **Tests:** `cutweave pack test`, MCP add-by-id script.
- **Out of scope:** other themes.

### WP-4B.02 — Transitions batch B: wipes & slides ×9
- **Area:** plugins/transitions-core
- **Depends on:** WP-4A.04, WP-4A.08
- **Size:** S
- **Executor:** sonnet
- **Scope:** **Linear Wipe** (angle, feather), **Barn Doors** (h/v), **Clock Wipe**, **Iris Round**, **Iris Diamond**, **Push** (4 directions), **Slide Over**, **Split Push**, **Checker Wipe** (grid size).
- **Interfaces:** `cutweave.tr.*`.
- **Acceptance criteria:** QC-FX for all 9; `feather` yields monotonic coverage (property test); direction param covers 4/8 directions.
- **Tests:** `cutweave pack test`, MCP script.
- **Out of scope:** glitch/3D.

### WP-4B.03 — Transitions batch C: zooms, spins & pseudo-3D ×9
- **Area:** plugins/transitions-core
- **Depends on:** WP-4A.04, WP-4A.08
- **Size:** M
- **Executor:** sonnet
- **Scope:** **Zoom In**, **Zoom Out**, **Zoom Blur** (radial blur ramp), **Spin**, **Spin Zoom**, **Whip Pan** (directional motion blur slide), **Cube Turn** (pseudo-3D), **Flip Card**, **Page Peel**.
- **Interfaces:** `cutweave.tr.*`.
- **Acceptance criteria:** QC-FX for all 9; blur-based ones ≤ tier M-equivalent (≤ 1.5 ms); edge handling clamps/mirrors with no visible border artefacts on test chart.
- **Tests:** `cutweave pack test`, MCP script.
- **Out of scope:** true 3D scenes.

### WP-4B.04 — Transitions batch D: glitch & distortion ×9
- **Area:** plugins/transitions-core
- **Depends on:** WP-4A.04, WP-4A.08
- **Size:** M
- **Executor:** sonnet
- **Scope:** **RGB Split Glitch**, **Block Glitch**, **Scanline Slice**, **Pixelate**, **Mosaic Tiles**, **Ripple Warp**, **Swirl**, **Wave Warp**, **Shake Cut** (whip shake with chroma split).
- **Interfaces:** `cutweave.tr.*`.
- **Acceptance criteria:** QC-FX for all 9; randomness seeded from `(transition id, frame)` and identical across runs/OS; intensity param 0–2.
- **Tests:** `cutweave pack test`, determinism, MCP script.
- **Out of scope:** audio glitches.

### WP-4B.05 — Transitions batch E: light & organic ×9
- **Area:** plugins/transitions-core
- **Depends on:** WP-4A.04, WP-4A.08
- **Size:** M
- **Executor:** sonnet
- **Scope:** **Warm Light Leak**, **Cool Light Leak**, **Anamorphic Streak**, **Bokeh Burst**, **Film Burn**, **Lens Flare Sweep**, **Ink Bleed**, **Smoke Reveal**, **Luma Fade** (gradient matte or image param). Light/organic looks are **procedural** (noise + gradients in WGSL, no bundled third-party textures).
- **Interfaces:** `cutweave.tr.*`.
- **Acceptance criteria:** QC-FX for all 9 (**total transitions ≥ 45**, count test in CI); additive light stays below 100% clip at default intensity on the test chart (≤ 1% pixels clipped outside the leak peak).
- **Tests:** `cutweave pack test`, count test.
- **Out of scope:** video-based leak overlays (L0 asset packs, post-alpha).

### WP-4B.06 — Transitions pack design review (human gate)
- **Area:** plugins/transitions-core/REVIEW.md
- **Depends on:** WP-4B.01, WP-4B.02, WP-4B.03, WP-4B.04, WP-4B.05, WP-4A.09
- **Size:** S
- **Executor:** human
- **Scope:** reviewer plays each transition on 3 reference cut pairs (talking head ↔ b-roll, 9:16 and 16:9; high-motion ↔ static) via `cutweave pack sheet`; scores look, taste, usefulness, default duration; culls or fixes.
- **Interfaces:** sign-off block.
- **Acceptance criteria:** ≥ 40 transitions signed "keep"; none with visible artefacts at default params; fixes filed as issues `phase:P4 epic:4B`.
- **Tests:** n/a.
- **Out of scope:** new transitions.

---

# Epic 4C — Effects pack (54) & trend suite

*All items follow QC-FX; plugin dir `plugins/effects-core`, ids `cutweave.fx.<slug>`; tier noted by category (S unless stated).*

### WP-4C.01 — Effects batch A: blur & glow ×9
- **Area:** plugins/effects-core
- **Depends on:** WP-4A.03, WP-4A.08
- **Size:** M
- **Executor:** sonnet
- **Scope:** **Gaussian Blur** (separable, tier M), **Box Blur**, **Directional Blur**, **Radial Blur**, **Tilt Shift**, **Lens Blur** (bokeh disc, tier M), **Soft Focus**, **Glow / Bloom** (threshold + blur, tier M), **Vignette Blur** (blur toward edges). Blur radius normalised to frame height.
- **Interfaces:** `cutweave.fx.gaussian-blur@1` is a dependency of 4E (Blur In/Out) — keep the id stable.
- **Acceptance criteria:** QC-FX for all 9; blur radius 0 is bit-exact identity; ½-res vs full ΔE00 ≤ 1.0; separable blur of 40 px radius ≤ 1.2 ms.
- **Tests:** `cutweave pack test`, identity test, bench.
- **Out of scope:** AI background blur (Phase 5).

### WP-4C.02 — Effects batch B: stylize ×9
- **Area:** plugins/effects-core
- **Depends on:** WP-4A.03, WP-4A.08
- **Size:** S
- **Executor:** sonnet
- **Scope:** **Pixelate**, **Halftone Dots**, **Posterize**, **Edge Outline**, **Pencil Sketch**, **Toon**, **Emboss**, **Duotone** (two colour params), **Threshold**.
- **Interfaces:** `cutweave.fx.*`.
- **Acceptance criteria:** QC-FX for all 9; pixelate block size in output pixels is integer-aligned at export and scales consistently in preview.
- **Tests:** `cutweave pack test`.
- **Out of scope:** AI stylisation.

### WP-4C.03 — Effects batch C: retro & film ×9
- **Area:** plugins/effects-core
- **Depends on:** WP-4A.03, WP-4A.08
- **Size:** M
- **Executor:** sonnet
- **Scope:** **VHS Tape** (tracking lines, chroma bleed, noise), **CRT Scanlines**, **Film Grain** (luma-weighted, seeded), **Vignette**, **Film Weave** (gate jitter), **Dust & Scratches** (procedural), **Old Film Flicker**, **Tape Wobble**, **Retro Camcorder** (soft glow, chroma bleed, rounded corners).
- **Interfaces:** `cutweave.fx.*`.
- **Acceptance criteria:** QC-FX for all 9; film grain is temporally varying but seed-deterministic and has no fixed-pattern repetition within 10 s (autocorrelation test); grain amount 0 is bit-exact identity.
- **Tests:** `cutweave pack test`, determinism, autocorrelation test.
- **Out of scope:** date/time-stamp text.

### WP-4C.04 — Effects batch D: glitch & distortion ×9
- **Area:** plugins/effects-core
- **Depends on:** WP-4A.03, WP-4A.08
- **Size:** M
- **Executor:** sonnet
- **Scope:** **RGB Split** (angle, amount; chromatic aberration variant param), **Block Glitch**, **Digital Noise**, **Displacement Glitch**, **Ripple**, **Twirl**, **Bulge / Pinch**, **Lens Distortion** (barrel/pincushion), **Earthquake Shake**.
- **Interfaces:** `cutweave.fx.*`.
- **Acceptance criteria:** QC-FX for all 9; distortion effects resample with ≥ bilinear (bicubic for lens distortion) and show no aliasing shimmer on test chart (temporal difference of static scene = 0); amount 0 is identity.
- **Tests:** `cutweave pack test`.
- **Out of scope:** face-aware distortions.

### WP-4C.05 — Effects batch E: motion & rhythm ×9
- **Area:** plugins/effects-core
- **Depends on:** WP-4A.03, WP-4A.08
- **Size:** M
- **Executor:** sonnet
- **Scope:** **Zoom Pulse** (bpm, offset, shape), **Camera Shake**, **Strobe Flash**, **Echo Trail** (history taps, tier L), **Frame Stutter** (hold every N frames, history taps), **Wobble**, **Jitter**, **Sway**, **Heartbeat**. Beat sync is manual (`bpm`+`offset` params; beat-marker binding arrives with Phase 5 analysis).
- **Interfaces:** `cutweave.fx.*`.
- **Acceptance criteria:** QC-FX for all 9; history-tap effects deterministic under seeking (seek-vs-play pixel identity test); camera shake seeded and amplitude bounded (max displacement ≤ param × frame height).
- **Tests:** `cutweave pack test`, seek-vs-play test.
- **Out of scope:** audio-reactive effects.

### WP-4C.06 — Effects batch F: geometry & light ×9
- **Area:** plugins/effects-core
- **Depends on:** WP-4A.03, WP-4A.08
- **Size:** M
- **Executor:** sonnet
- **Scope:** **Mirror** (h/v/quad), **Kaleidoscope** (segments, rotation), **Tile Repeat**, **Split Screen Grid** (2×1/2×2/3×3), **Perspective Tilt**, **Fisheye**, **Lens Flare**, **Light Rays**, **Neon Edge Glow**.
- **Interfaces:** `cutweave.fx.*`.
- **Acceptance criteria:** QC-FX for all 9 (**total effects ≥ 54**, count test in CI); mirror/kaleidoscope seams have no 1-px gaps (test on chart); lens flare additive clip ≤ 1% pixels at default.
- **Tests:** `cutweave pack test`, count test.
- **Out of scope:** particles, generators.

### WP-4C.07 — "Trend recreation" suite: 10 reference edits using only built-ins
- **Area:** testdata/trend-suite, tests/e2e-trends
- **Depends on:** WP-4B.05, WP-4C.06, WP-4D.06, WP-4E.04, WP-4F.04, WP-4G.05, WP-4H.03, WP-4I.05, WP-3A.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** ten projects built by scripted `cutweave exec`/MCP from CC0 media (generated or committed small clips), each with a README stating the intended look in generic terms (no trademarked templates): 1 **Glitch-cut montage intro** (RGB split, block-glitch transition, shake); 2 **Zoom-punch vlog transitions** (zoom blur, spin, whip pan); 3 **Cinematic teal-amber look** (filter + letterbox + grain + light-leak transition); 4 **VHS memory** (VHS tape, camcorder, tape wobble); 5 **Dreamy pastel portrait** (soft focus, glow, pastel filter, vignette); 6 **Speed-ramp beat edit** (hero/bullet speed curve, zoom pulse, flash cuts); 7 **Green-screen reveal** (chroma key + background plate + mask + shadow); 8 **Mirror/kaleidoscope loop**; 9 **Neon cyberpunk** (duotone/neon edge, circle-mask reveal, adjustment layer); 10 **Tracked callout** (stabilised footage, motion-tracked text + sticker + animated callout).
- **Interfaces:** `cutweave test-trends` runner (CI); contact sheets under `testdata/trend-suite/out/`.
- **Acceptance criteria:** each project builds via script with zero errors and uses only built-in plugins (lint); golden frames (3 per edit) pass with preview == export; every effect instance stays within its tier on the nightly iGPU run; projects total ≥ 40 distinct built-in items across all kinds (coverage report); contact sheets attached to the design-review issue.
- **Tests:** the runner + goldens; nightly GPU job.
- **Out of scope:** the human review (4C.08).

### WP-4C.08 — Effects pack & trend suite design review (human gate)
- **Area:** docs/evals/p4-design-review.md
- **Depends on:** WP-4C.07, WP-4C.01, WP-4C.02, WP-4C.03, WP-4C.04, WP-4C.05, WP-4C.06, WP-4D.07, WP-4E.05
- **Size:** S
- **Executor:** human
- **Scope:** reviewers score each of the 10 trend recreations (fidelity to the described look, taste, defaults, perf) and cull/fix the effect and animation packs via contact sheets; confirm exit criterion wording ("passes design review").
- **Interfaces:** review record with per-edit verdict.
- **Acceptance criteria:** ≥ 8/10 edits "pass", the remaining two have filed fixes closed before M4; ≥ 50 effects signed "keep"; M4 sign-off recorded.
- **Tests:** n/a.
- **Out of scope:** new effects.

---

# Epic 4D — Filters & LUTs (45)

*Plugin dir `plugins/filters-core`, ids `cutweave.flt.<slug>`. All LUTs are **generated by our own recipes** (`tools/lutgen`), CC0; no third-party LUTs are copied.*

### WP-4D.01 — Filter pack format, procedural LUT generator & `color.apply_filter`
- **Area:** crates/cutweave-plugins, crates/cutweave-ops, tools/lutgen, plugins/filters-core
- **Depends on:** WP-4F.04, WP-4A.06, WP-4A.08
- **Size:** M
- **Executor:** sonnet
- **Scope:** L0 `filter.toml` (id, name, category, tags, `lut = "x.cube"` or `recipe`, default intensity 100, range 0–150, thumbnail); `tools/lutgen` turns TOML recipes (exposure/contrast/tone curves per channel, split-toning, HSL shifts, saturation, grain-free) into 33³ `.cube` files deterministically (same bytes every run); `color.apply_filter{item|track,filter,intensity}` (adds/updates a `cutweave.color.lut` instance, one per clip, intensity keyframeable), `color.remove_filter`; queries `filter.list` folded into `effect.list{kind:filter}`.
- **Interfaces:** MCP catalog `color_apply_filter`; consumes the LUT sampler from 4F.04.
- **Acceptance criteria:** lutgen output byte-identical across OS; identity recipe yields identity LUT (max error 1e-4); applying at intensity 0 is bit-exact identity; filter pass ≤ 0.4 ms at 1080p iGPU; applying a filter + undo is one step.
- **Tests:** unit, determinism, golden, bench.
- **Out of scope:** the filter content, `.cube` import UI (4F.06).

*Shared filter checks (QC-FLT, in addition to QC-FX 1–7):* on a 24-patch colour chart plus gradients — (a) no clipping > 2% of pixels at intensity 100 on the test chart; (b) 16-bit-pipeline gradient ramp shows no banding step > 1.5/255; (c) portrait filters: skin patches ("dark skin", "light skin") shift ≤ 15 ΔE00 (≤ 8 for "Fresh/Clean" categories); (d) neutral axis tint ≤ declared in recipe (documented); (e) thumbnail uses the same portrait + landscape stills across the pack for comparability.

### WP-4D.02 — Filters batch A: cinematic ×9
- **Area:** plugins/filters-core
- **Depends on:** WP-4D.01
- **Size:** S
- **Executor:** sonnet
- **Scope:** **Teal & Amber**, **Bleach Bypass**, **Golden Hour**, **Moonlit Blue**, **Noir Contrast**, **Desaturated Drama**, **Warm Epic**, **Steel Cool**, **Forest Green**.
- **Interfaces:** `cutweave.flt.*`.
- **Acceptance criteria:** QC-FX + QC-FLT for all 9.
- **Tests:** `cutweave pack test`, chart checks.
- **Out of scope:** other categories.

### WP-4D.03 — Filters batch B: retro & film-look ×9
- **Area:** plugins/filters-core
- **Depends on:** WP-4D.01
- **Size:** S
- **Executor:** sonnet
- **Scope:** **Faded Matte**, **Seventies Warm**, **Warm Negative**, **Cross Process**, **Cyan Shadows**, **Vintage Fade**, **Toy Camera**, **Tintype Sepia**, **Soft Print**. (Generic names — no film-stock or camera brand names.)
- **Interfaces:** `cutweave.flt.*`.
- **Acceptance criteria:** QC-FX + QC-FLT for all 9.
- **Tests:** `cutweave pack test`.
- **Out of scope:** grain (use 4C Film Grain).

### WP-4D.04 — Filters batch C: fresh & portrait ×9
- **Area:** plugins/filters-core
- **Depends on:** WP-4D.01
- **Size:** S
- **Executor:** sonnet
- **Scope:** **Skin Warm**, **Soft Peach**, **Fresh Mint**, **Clean Bright**, **Pastel Dream**, **Vivid Pop**, **Rose Gold**, **Matcha**, **Cool Pastel**.
- **Interfaces:** `cutweave.flt.*`.
- **Acceptance criteria:** QC-FX + QC-FLT for all 9 with the stricter skin limit (≤ 8 ΔE00 for Skin Warm/Clean Bright/Fresh Mint).
- **Tests:** `cutweave pack test`.
- **Out of scope:** AI beautify.

### WP-4D.05 — Filters batch D: mood & genre ×9
- **Area:** plugins/filters-core
- **Depends on:** WP-4D.01
- **Size:** S
- **Executor:** sonnet
- **Scope:** **Travel Vibrant**, **Food Appetite**, **Moody Green**, **Sunset Glow**, **Cyberpunk Neon**, **Vaporwave**, **Horror Drain**, **Dreamy Haze**, **Winter Frost**.
- **Interfaces:** `cutweave.flt.*`.
- **Acceptance criteria:** QC-FX + QC-FLT for all 9 (stylised filters may exceed the skin limit; flagged `stylized=true` in metadata).
- **Tests:** `cutweave pack test`.
- **Out of scope:** —

### WP-4D.06 — Filters batch E: mono & technical ×9
- **Area:** plugins/filters-core
- **Depends on:** WP-4D.01
- **Size:** S
- **Executor:** sonnet
- **Scope:** **B&W Punchy**, **B&W Soft**, **B&W Faded**, **Sepia Classic**, **Duotone Blue-Orange**, **Duotone Purple-Pink**, **Log to Rec.709 Neutral** (documented generic log-curve assumption), **Day for Night**, **Neutral Punch**. B&W uses Rec.709 luma in linear light (documented).
- **Interfaces:** `cutweave.flt.*`.
- **Acceptance criteria:** QC-FX + QC-FLT for all 9 (**total filters ≥ 40**, count test); B&W outputs have R=G=B exactly (max channel difference 0 after quantisation).
- **Tests:** `cutweave pack test`, count test.
- **Out of scope:** camera-specific log profiles.

### WP-4D.07 — Filters pack design review (human gate)
- **Area:** plugins/filters-core/REVIEW.md
- **Depends on:** WP-4D.02, WP-4D.03, WP-4D.04, WP-4D.05, WP-4D.06, WP-4A.09
- **Size:** S
- **Executor:** human
- **Scope:** reviewer applies every filter to 3 reference clips (portrait, landscape, product/food) and scores look, skin safety, variety (≥ 6 distinct categories), naming; cull/fix.
- **Interfaces:** sign-off block.
- **Acceptance criteria:** ≥ 40 filters "keep"; no filter damaging skin tones in portrait category; fixes filed as issues.
- **Tests:** n/a.
- **Out of scope:** —

---

# Epic 4E — Clip animations (in / out / combo)

### WP-4E.01 — Clip animation framework
- **Area:** crates/cutweave-model, crates/cutweave-render, crates/cutweave-plugins, crates/cutweave-ops
- **Depends on:** WP-4A.03, WP-3B.01, E2C
- **Size:** M
- **Executor:** opus
- **Scope:** L0 `clip-anim.toml` (`slot = in|out|combo`, tracks for position/scale/rotation/opacity/anchor in **normalised units** (% of frame / unit scale), keyframes normalised to [0,1] with easing, optional `effect_params` referencing an effect id + param tracks (e.g. blur), `duration` default/min/max, `intensity`, `direction`); `Clip.animation{in,out,combo}: Option<AnimInstance>`; evaluation layered **over** base props and user keyframes (position/rotation additive; scale/opacity multiplicative) so user keyframes are never rewritten; works on any visual item (clip, text, sticker, composition); commands `clip.set_animation{item,slot,anim,params,duration}`, `clip.clear_animation`; query `clip.animations.list{slot,query}`.
- **Interfaces:** MCP catalog; ids `cutweave.anim.<slot>.<slug>`.
- **Acceptance criteria:** in+out never overlap (auto-clamp when item shorter than sum; test); combo excludes in/out (enforced with typed error); removing the animation restores exact base render (bit-exact); resolution-independent; GF with user keyframes + animation stacked.
- **Tests:** proptest (layering math, undo), golden.
- **Out of scope:** the preset content, UI.

### WP-4E.02 — Clip animations pack: IN ×10
- **Area:** plugins/clip-animations
- **Depends on:** WP-4E.01, WP-4C.01
- **Size:** S
- **Executor:** sonnet
- **Scope:** **Fade In**, **Slide In Left**, **Slide In Right**, **Slide In Up**, **Zoom In**, **Pop In** (overshoot), **Spin In**, **Flip In**, **Blur In** (uses `cutweave.fx.gaussian-blur`), **Drop Bounce**.
- **Interfaces:** `cutweave.anim.in.*`.
- **Acceptance criteria:** QC-FX 2,3,5,7,8 for all 10; default duration 0.5 s; start state (t=0) matches spec (opacity 0 / offscreen) within 1e-3; end state identical to base (bit-exact at t_end).
- **Tests:** `cutweave pack test`.
- **Out of scope:** out/combo.

### WP-4E.03 — Clip animations pack: OUT ×10
- **Area:** plugins/clip-animations
- **Depends on:** WP-4E.01, WP-4C.01
- **Size:** S
- **Executor:** sonnet
- **Scope:** **Fade Out**, **Slide Out Left**, **Slide Out Right**, **Slide Out Down**, **Zoom Out**, **Shrink Out**, **Spin Out**, **Flip Out**, **Blur Out**, **Fall Away**.
- **Interfaces:** `cutweave.anim.out.*`.
- **Acceptance criteria:** QC-FX 2,3,5,7,8 for all 10; at t_end the item is fully transparent/offscreen; start state identical to base (bit-exact).
- **Tests:** `cutweave pack test`.
- **Out of scope:** —

### WP-4E.04 — Clip animations pack: COMBO ×8
- **Area:** plugins/clip-animations
- **Depends on:** WP-4E.01
- **Size:** S
- **Executor:** sonnet
- **Scope:** (span the whole clip duration) **Rock**, **Pulse**, **Swing**, **Float**, **Shake Loop**, **Heartbeat**, **Slow Push-In** (Ken-Burns zoom), **Slow Pan** (Ken-Burns pan). Loop-type combos are seamless (value(t=0) == value(t=period)).
- **Interfaces:** `cutweave.anim.combo.*`.
- **Acceptance criteria:** QC-FX 2,3,5,7,8; **28 animations total** (count test); Ken-Burns ones never reveal frame edges (scale ≥ 1 enforced at all times; test).
- **Tests:** `cutweave pack test`, count test, seamlessness test.
- **Out of scope:** —

### WP-4E.05 — Animations tab UI
- **Area:** packages/editor
- **Depends on:** WP-4E.01, WP-4A.09
- **Size:** S
- **Executor:** sonnet
- **Scope:** Animations tab (In / Out / Combo sections) with animated thumbnails, duration slider (clamped), inspector section showing current animation + params, remove button; reuse the 4A.09 browser component.
- **Interfaces:** consumes `clip.set_animation`, `clip.animations.list`.
- **Acceptance criteria:** Playwright e2e: apply In + Out to a clip → timeline shows animation ranges → undo restores; duration slider clamps to clip length.
- **Tests:** Playwright-Electron.
- **Out of scope:** —

---

# Epic 4F — Color tools & scopes

### WP-4F.01 — Color pipeline specification & reference implementation
- **Area:** docs/specs/color-pipeline.md, crates/cutweave-render (`color::reference`)
- **Depends on:** E0F, E1F
- **Size:** M
- **Executor:** opus
- **Scope:** normative spec of the **canonical operation order and colour space per operation**: Exposure & white balance (linear; WB via chromatic adaptation, slider → Kelvin/tint mapping), Contrast (S-curve in log2 about 0.18 mid-grey), Highlights/Shadows/Whites/Blacks (luminance-weighted tone regions), Saturation (Rec.709 luma-preserving), Vibrance (chroma-inverse weighting with skin protection), HSL (8 hue bands in OKLCH, smooth overlapping weights), Wheels (lift/gamma/gain/offset), Curves (display-referred, PCHIP), LUT/Filter (encode → LUT → decode, with declared input space); slider ranges (−100…100) and default-neutral guarantees; interplay with HDR→SDR tone-map and export transforms; f64 CPU reference implementation used by tests (`color::reference`).
- **Interfaces:** `docs/specs/color-pipeline.md`; `cutweave_render::color::reference::*`.
- **Acceptance criteria:** reviewed by a human maintainer; reference implementation passes self-consistency (neutral params = identity within 1e-9; monotonicity for exposure/contrast); each GPU op in 4F.02–4F.04 states its ΔE00 tolerance vs reference (≤ 1.0 mean, ≤ 2.5 max on 10k sampled colours).
- **Tests:** unit on reference; property tests (identity at defaults).
- **Out of scope:** GPU implementation.

### WP-4F.02 — Basic adjustments (built-in L1 effect `cutweave.color.basic`)
- **Area:** plugins/color-core, crates/cutweave-ops
- **Depends on:** WP-4F.01, WP-4A.03
- **Size:** M
- **Executor:** opus
- **Scope:** one fused WGSL pass: exposure, contrast, highlights, shadows, whites, blacks, temperature, tint, saturation, vibrance; all keyframeable; commands `color.set_basic{item,params}` (convenience over `effect.set_param`), `color.reset{item,section}`, `color.copy_grade{item}` / `color.paste_grade{targets}` (copies the colour-effect stack), query `color.get{item}`; defaults bit-exact identity.
- **Interfaces:** effect id `cutweave.color.basic@1`; MCP core tool `color_set` (basic section), others catalog.
- **Acceptance criteria:** GPU vs reference ΔE00 mean ≤ 1.0, max ≤ 2.5 on 10k colours per control at ±50/±100; neutral defaults bit-exact identity; ≤ 0.5 ms at 1080p iGPU; temperature slider monotonic in B/R ratio (property test); GF.
- **Tests:** ΔE harness, property tests, golden, bench.
- **Out of scope:** auto-adjust, AI colour match.

### WP-4F.03 — HSL per hue and curves
- **Area:** plugins/color-core
- **Depends on:** WP-4F.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** `cutweave.color.hsl@1` (8 bands: red, orange, yellow, green, cyan, blue, purple, magenta; hue/sat/lum shifts with smooth band weights per spec); `cutweave.color.curves@1` (master RGB + R, G, B channels; ≤ 16 points each; monotone cubic (PCHIP) baked to a 1024-entry 1D LUT texture; point-set keyframes interpolate only when point counts match, else hold — documented); commands `color.set_hsl`, `color.set_curves`.
- **Interfaces:** `curve` param type from the 4A.01 spec; MCP catalog.
- **Acceptance criteria:** ΔE vs reference (HSL ≤ 1.5 mean; curves ≤ 0.5 mean); identity curve is bit-exact identity; curve monotonic (no overshoot) for any control-point set (property test); each ≤ 0.5 ms (HSL) / ≤ 0.3 ms (curves) at 1080p iGPU.
- **Tests:** ΔE harness, proptest, golden, bench.
- **Out of scope:** UI.

### WP-4F.04 — Colour wheels & `.cube` LUT import/sampler
- **Area:** plugins/color-core, crates/cutweave-render, crates/cutweave-ops
- **Depends on:** WP-4F.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** `cutweave.color.wheels@1` (lift/gamma/gain/offset with hue+amount+luma per wheel, plus shadows/mids/highlights variant per spec); `cutweave.color.lut@1`: `.cube` parser (`LUT_1D_SIZE`, `LUT_3D_SIZE` 17–65, `DOMAIN_MIN/MAX`, `TITLE`, comments, shaper 1D), 3D texture upload (rgba16f), **tetrahedral** interpolation in shader, `input_space` option (Rec.709 gamma | sRGB | generic log), intensity; project asset store `assets.luts` (copy, content-hash dedupe); commands `color.import_lut{path}` (validates, size limit 65³), `color.apply_lut{item,lut,intensity,input_space}`, `color.set_wheels`.
- **Interfaces:** this WP owns the shared LUT sampler used by 4D.
- **Acceptance criteria:** identity .cube round-trips (ΔE00 ≤ 0.2); non-trivial LUT vs CPU reference ΔE00 mean ≤ 0.5; malformed `.cube` (cargo-fuzz 30 min) never panics; LUT apply ≤ 0.4 ms at 1080p; imported LUT survives save/load and collect/archive.
- **Tests:** fuzz, ΔE harness, golden, bench.
- **Out of scope:** LUT generation tooling (4D.01), 1D-only shaper UI.

### WP-4F.05 — Scopes: waveform, vectorscope, histogram (compute)
- **Area:** crates/cutweave-render (`scopes`), crates/cutweave-ops, crates/cutweave-api
- **Depends on:** WP-4F.01, E1H
- **Size:** M
- **Executor:** opus
- **Scope:** wgpu compute passes on the final (or per-clip) frame sampled to ≤ 960 px wide: luma waveform, RGB parade, vectorscope (CbCr density, graticule, skin-tone line), RGB+luma histogram; scale modes (0–100% / 0–255); query `scopes.get{sequence,t,kind,size}` returning an image (MCP image content) **plus numeric summary** `{min,max,mean per channel, clipping %, black/white point}` so agents can judge exposure without vision; throttled UI streaming (≤ 15 Hz, ≤ 5 ms GPU per update on iGPU; paused during full-rate playback unless enabled).
- **Interfaces:** MCP catalog tool `scopes_get`; UI polling API.
- **Acceptance criteria:** horizontal ramp test chart → diagonal waveform within 1 bin; pure R/G/B/C/M/Y patches land within 2 px of vectorscope targets; histogram bin counts sum exactly to pixel count; summary stats equal CPU reference within 1e-3; scopes at 1080p source ≤ 1.5 ms GPU.
- **Tests:** known-chart tests, GF of scope images, bench.
- **Out of scope:** UI (4F.06), HDR scopes.

### WP-4F.06 — Color panel & Scopes panel UI
- **Area:** packages/editor, packages/ui
- **Depends on:** WP-4F.02, WP-4F.03, WP-4F.04, WP-4F.05, E2H
- **Size:** M
- **Executor:** sonnet
- **Scope:** Color inspector tab: Basic (sliders, double-click reset, keyframe diamonds), HSL (band picker with swatches), Curves (canvas editor: add/drag/delete points, per-channel, numeric entry, snapping), Wheels, LUT section (import `.cube`, choose, intensity, input space), Copy/Paste grade, before/after split compare (`\` key); dockable Scopes panel (tabs: waveform/parade/vectorscope/histogram) updating on scrub/playhead.
- **Interfaces:** consumes `color.*`, `scopes.get`.
- **Acceptance criteria:** Playwright e2e: drag exposure → preview changes and scope reacts within 100 ms; add 3 curve points → export frame matches golden; keyboard-operable sliders with aria labels; one gesture = one undo step.
- **Tests:** Playwright-Electron, Vitest, visual regression.
- **Out of scope:** auto colour match.

---

# Epic 4G — Masks, keying, adjustment layers

### WP-4G.01 — Shape masks: model, commands & renderer
- **Area:** crates/cutweave-model, crates/cutweave-render (`mask`), crates/cutweave-ops
- **Depends on:** WP-4A.03, E2C
- **Size:** M
- **Executor:** opus
- **Scope:** `Mask{id,shape: Rect{radius}|Ellipse|Polygon{points}|Bezier{nodes+handles},feather,expansion,invert,opacity,mode: add|subtract|intersect|difference}` on clips (list, ordered); transform (position/scale/rotation) and path points all `Animatable` (same node count for point animation); coverage via bounded-loop analytic signed-distance in WGSL (paths flattened to ≤ 256 segments), feather = smoothstep on distance; static masks cached as textures; commands `mask.add`, `mask.set`, `mask.set_points`, `mask.remove`, `mask.reorder`; mask applies to clip alpha before clip effects and transitions.
- **Interfaces:** MCP catalog `mask_add`; `Clip.masks`.
- **Acceptance criteria:** GF for 5 shapes × feather/invert/expansion; edge AA quality: SSIM ≥ 0.995 vs 8× supersampled reference; feather coverage monotonic (property test); two masks ≤ 0.3 ms at 1080p iGPU; keyframed point animation preserves topology (no flipping, test); undo/redo.
- **Tests:** golden, proptest, bench.
- **Out of scope:** UI, AI/tracked masks (Phase 5 mattes plug into `matte` in 4G.04).

### WP-4G.02 — Mask & key UI overlays
- **Area:** packages/editor
- **Depends on:** WP-4G.01, WP-4G.03
- **Size:** M
- **Executor:** sonnet
- **Scope:** on-canvas mask tools (rect, ellipse, pen for bezier/polygon), node add/delete/smooth↔corner, feather and expansion handles, invert toggle, mask list with modes in inspector; keying UI: colour eyedropper from preview (`render.sample_pixel{t,x,y}` query returns linear + display values), similarity/smoothness/spill/shadow sliders, view modes Result | Matte | Spill | Original (preview-only param, never exported).
- **Interfaces:** consumes `mask.*`, `render.sample_pixel`, key effect params.
- **Acceptance criteria:** Playwright e2e: draw ellipse mask → feather → keyframe → export frame matches golden; eyedropper on green screen sets key colour within ΔE ≤ 3 of the clicked pixel; view mode never affects export (golden test).
- **Tests:** Playwright-Electron, visual regression.
- **Out of scope:** tracking integration (4I.05).

### WP-4G.03 — Chroma key with spill suppression
- **Area:** plugins/key-core
- **Depends on:** WP-4A.03, WP-4F.01
- **Size:** M
- **Executor:** opus
- **Scope:** `cutweave.key.chroma@1`: key colour (green/blue/any), CbCr-distance key with luma weighting, `similarity`, `smoothness`, matte `clip_black/clip_white`, edge `feather` + `erode/dilate` (2-pass), **despill** (average/maximum modes, strength, luminance-preserving replace), `shadow` retention, `output: composite|matte|despilled`; budget tier M.
- **Interfaces:** effect id `cutweave.key.chroma@1`; MCP via `effect_add`.
- **Acceptance criteria:** on synthetic composites with known ground-truth alpha (5 clips incl. hair-like fine detail, motion blur, green shadows): recovered alpha PSNR ≥ 35 dB and mean absolute alpha error ≤ 0.03; spill metric `max(G − max(R,B))` on foreground edge pixels ≤ 0.04 after despill; temporal flicker (frame-to-frame alpha RMS on static background) ≤ 0.01; ≤ 1.2 ms at 1080p iGPU.
- **Tests:** ground-truth alpha harness, golden, bench.
- **Out of scope:** AI keying (Phase 5 matting).

### WP-4G.04 — Luma key, effect masks & track mattes
- **Area:** plugins/key-core, crates/cutweave-render, crates/cutweave-ops
- **Depends on:** WP-4G.01, WP-4A.06
- **Size:** M
- **Executor:** sonnet
- **Scope:** `cutweave.key.luma@1` (low/high/softness/invert); **effect-level masks**: `EffectInstance.mask_ref` + `mask_mode: inside|outside` (effect result blended by mask coverage; masks may be tracked later); **track matte**: `Clip.matte{source_item,mode: alpha|luma|inverted-luma}` using another item (video/image/composition/alpha-video) as matte — also the hook where Phase 5 AI mattes (alpha video) plug in; commands `effect.set_mask`, `clip.set_matte`, `clip.clear_matte`.
- **Interfaces:** MCP catalog; `Clip.matte`.
- **Acceptance criteria:** blur restricted to an ellipse leaves outside pixels bit-identical to the source (test); matte from an alpha-video asset equals the asset's alpha within 1/255; luma key on a ramp keys the expected threshold ± 1 level; GF; undo/redo.
- **Tests:** golden, property (outside-identity), unit.
- **Out of scope:** AI mattes themselves.

### WP-4G.05 — Adjustment layers
- **Area:** crates/cutweave-render, crates/cutweave-model, crates/cutweave-ops
- **Depends on:** WP-4A.03, WP-4G.04
- **Size:** M
- **Executor:** opus
- **Scope:** `Adjustment{effects,opacity,blend,masks}` items on `adjustment` tracks; render-graph: composite everything below (within item time range, optionally limited to `target_tracks`) to a texture, run the effect stack, blend over by opacity/mask; effects/keyframes via existing `effect.*` commands addressing the adjustment item; command `timeline.add_adjustment{sequence,track?,start,duration}` (creates the track if absent); works with transitions underneath and the preview quality ladder.
- **Interfaces:** MCP catalog; item kind `Adjustment`.
- **Acceptance criteria:** adjustment with colour + blur equals applying the same stack to the flattened composite (PSNR ≥ 50 dB); extra cost of an adjustment layer (empty stack) ≤ 0.8 ms at 1080p iGPU; effects on tracks above the adjustment are unaffected (test); opacity 0 = identity (bit-exact); undo/redo.
- **Tests:** golden, property, bench.
- **Out of scope:** nested adjustment groups.

---

# Epic 4H — Speed curves & frame blending

### WP-4H.01 — Speed curves: model, time-map evaluation & presets
- **Area:** crates/cutweave-model (`Speed::Curve`), crates/cutweave-ops, crates/cutweave-render
- **Depends on:** E2D, E2C
- **Size:** M
- **Executor:** sonnet
- **Scope:** `Speed::Curve{points[{t,speed,ease/handles}],keep: duration|source_range}`; speed range 0.1×–10× (curves) ; source-time function `t_src(t) = ∫speed` computed by monotone numeric integration (cached frame-accurate table, one entry per output frame); `keep: source_range` makes clip duration = ∫; commands `clip.set_speed_curve{item,points|preset,keep}`, `clip.clear_speed_curve`; query `clip.speed_presets`; presets (normalised time): **Montage** (1×→0.4×→2.5×→0.4×→1×), **Hero** (1× until 30%, ramp to 0.25× across 40–60%, back to 1×), **Bullet** (3× entry, deep 0.08× hold 35–65%, 3× exit), **Flash In** (4× → 1× by 30%), **Flash Out** (1× → 4× from 70%), plus **Ramp Up**, **Ramp Down**.
- **Interfaces:** MCP catalog `clip_set_speed_curve`; time-map consumed by render, audio (4H.03), captions (`timemap` from 3E.01 must use it — coordinate in Notes).
- **Acceptance criteria:** a constant-speed curve is bit-identical to constant mode; source frames selected match the analytic integral (frame-exact test on 5 presets); no negative/zero speed ever (validation); duration equals ∫ within 1 flick-rounding frame; presets GF; undo/redo.
- **Tests:** proptest (monotonic time-map), analytic fixtures, golden.
- **Out of scope:** UI, frame blending, audio.

### WP-4H.02 — Frame blending & basic optical-flow slow-motion
- **Area:** crates/cutweave-render (`retime`), crates/cutweave-cache
- **Depends on:** WP-4H.01, E1F
- **Size:** M
- **Executor:** opus
- **Scope:** per-clip `retime_quality = nearest|blend|optical_flow|auto`; frame blend in linear light; **optical flow**: GPU pyramidal (3–4 levels) Horn–Schunck/LK dense flow at ½ res, forward+backward, forward-backward consistency/occlusion mask, flow-compensated warp of both neighbours and blend, automatic fallback to blend where confidence low; flow fields cached on disk keyed `(media hash, frame pair, scale)`; preview `auto` uses flow only when paused or ≤ 0.5× speed and within frame budget; `retime_provider = builtin|ai` hook (AI interpolation arrives in Phase 5).
- **Interfaces:** `clip.set_retime{item,quality}`; MCP catalog.
- **Acceptance criteria:** on a synthetic translating-texture clip at 0.25× slow-mo: PSNR ≥ blend + 3 dB vs ground truth; no NaN/Inf, deterministic (bit-identical same adapter); export 1080p at 0.5× with optical flow ≥ 8 fps on the iGPU reference; flow memory ≤ 400 MB; cache hit makes second export ≥ 2× faster.
- **Tests:** ground-truth synthetic, golden, determinism, bench.
- **Out of scope:** AI interpolation (RIFE etc.), sub-frame motion blur synthesis.

### WP-4H.03 — Audio and export behaviour for speed curves
- **Area:** crates/cutweave-audio, crates/cutweave-engine
- **Depends on:** WP-4H.01, E2E
- **Size:** M
- **Executor:** sonnet
- **Scope:** variable-ratio time-stretch (signalsmith) following the speed curve with pitch preservation, smooth ratio changes; modes `audio: stretch|pitch|mute`; export parity (audio length = video length ± 1 sample-frame, A/V sync after ramps ≤ 1 frame); reverse + curve combination; clip-level crossfade at curve ends.
- **Interfaces:** `clip.set_speed_curve{audio_mode}`.
- **Acceptance criteria:** click-track test: click positions after retime match analytic map ± 5 ms; no discontinuity artefacts (max sample jump ≤ −30 dBFS relative on tone fixture); audio/video durations equal for all 7 presets; underrun-free playback at 256-sample buffer with a curved clip.
- **Tests:** click-track harness, tone-continuity test, soak.
- **Out of scope:** AI voice effects.

### WP-4H.04 — Speed curve editor UI
- **Area:** packages/editor, packages/timeline
- **Depends on:** WP-4H.01, WP-4H.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** inspector graph editor (speed vs time, bezier handles, add/delete/drag points with snapping to markers/beat markers), preset dropdown with mini-curve icons, `keep` toggle with resulting duration readout, retime quality selector, timeline clip speed ramp visualisation (stripes) and badge.
- **Interfaces:** consumes `clip.set_speed_curve`, `clip.speed_presets`, `clip.set_retime`.
- **Acceptance criteria:** Playwright e2e: apply Hero → drag a point → preview updates ≤ 100 ms → export frame matches golden; duration readout equals engine duration; one gesture = one undo step.
- **Tests:** Playwright-Electron, Vitest.
- **Out of scope:** keyframe graph editor for other props (owned by E2C).

---

# Epic 4I — Stabilization & tracking

### WP-4I.01 — Stabilization analysis job (FFmpeg vidstab)
- **Area:** crates/cutweave-media (`stabilize`), crates/cutweave-jobs, crates/cutweave-ops
- **Depends on:** E1D, E2G
- **Size:** M
- **Executor:** sonnet
- **Scope:** run libvidstab `vidstabdetect` through the FFmpeg filter graph (**requires `--enable-libvidstab` in the pinned FFmpeg build — update the build policy**) on ≤ 1080p downscaled frames; parse transforms into a compact binary per-frame {dx,dy,angle,zoom} cached by `(media hash, range, params)`; job `analysis.stabilize{item,shakiness,accuracy}` with progress/cancel; if the filter is missing, fall back to the built-in tracker (4I.03) with a warning.
- **Interfaces:** MCP catalog `analysis_stabilize`; cache table in `cache.sqlite`.
- **Acceptance criteria:** 30 s 1080p analysed in ≤ 1× realtime on the reference machine; cache hit < 50 ms; cancel < 1 s; deterministic output across runs; clear error when FFmpeg lacks vidstab and fallback engages.
- **Tests:** integration on shaky fixture, cache test.
- **Out of scope:** applying the transforms.

### WP-4I.02 — Stabilization apply: smoothing & warp effect
- **Area:** plugins/stabilize-core, crates/cutweave-render
- **Depends on:** WP-4I.01, WP-4A.03
- **Size:** M
- **Executor:** opus
- **Scope:** effect `cutweave.stabilize@1` using the `data_input` hook: camera-path smoothing (Gaussian window up to 3 s; `smoothness` 0–100), modes `smooth | lock (tripod)`, `crop_mode: auto-scale | mirror-edges | none`, strength keyframeable, 2D affine warp (translate, rotate, zoom) with bicubic sampling; command `clip.stabilize{item,smoothness,mode,crop}` (triggers analysis if missing); rolling-shutter correction explicitly excluded.
- **Interfaces:** MCP catalog `clip_stabilize`.
- **Acceptance criteria:** synthetic shaky clip with known injected path: residual high-pass path RMS reduced ≥ 80%; real handheld fixture ≥ 60% (vs analysis output); `auto-scale` never shows borders (test); strength 0 is bit-exact identity; ≤ 0.8 ms at 1080p iGPU; GF.
- **Tests:** synthetic ground truth, golden, bench.
- **Out of scope:** AI stabilisation, rolling shutter.

### WP-4I.03 — Point tracker (pyramidal Lucas-Kanade)
- **Area:** crates/cutweave-media (`track`) or crates/cutweave-render (`analysis`), crates/cutweave-model
- **Depends on:** E1D, E2G
- **Size:** M
- **Executor:** opus
- **Scope:** CPU pyramidal LK in Rust (no OpenCV), luma ≤ 960 px, patch 21–61 px selectable, forward-backward error check, drift control by template re-anchoring, loss/occlusion detection (stops with `lost_at`); job `tracking.run{item,region{point|rect},range,direction}`; stores `MotionTrack{id,item,points[{t,x,y,confidence}]}` in `project.motion_tracks` (model addition + migration; named to avoid collision with sequence tracks).
- **Interfaces:** MCP catalog `tracking_run`; namespace `tracking` (new — see Notes).
- **Acceptance criteria:** synthetic moving-texture clips with known paths (incl. rotation ±20°, scale 0.8–1.25×, blur): mean error ≤ 1.0 px, 95th percentile ≤ 2 px at 1080p over 300 frames; ≥ 20 fps analysis on reference CPU; loss detected within 5 frames of full occlusion; deterministic.
- **Tests:** ground-truth harness, unit, bench.
- **Out of scope:** planar tracking, UI.

### WP-4I.04 — Planar-lite tracker
- **Area:** crates/cutweave-media (`track`)
- **Depends on:** WP-4I.03
- **Size:** M
- **Executor:** opus
- **Scope:** quad region; 5×5 feature grid tracked by LK; per-frame transform estimated with RANSAC in modes `translate | similarity | affine | perspective`; outputs 4 corner positions and decomposed position/scale/rotation per frame; confidence and loss handling; `tracking.run{mode:"planar"}`.
- **Interfaces:** extends `MotionTrack` with `corners[]`.
- **Acceptance criteria:** synthetic planar warp ground truth: mean corner error ≤ 1.5 px (≤ 3 px for perspective at 30° tilt); survives 30% of points occluded; falls back to a lower-order model when degenerate and reports it; deterministic (seeded RANSAC).
- **Tests:** ground-truth harness, occlusion test.
- **Out of scope:** 3D camera solve, corner-pin rendering.

### WP-4I.05 — Track → keyframes application & tracking/stabilize UI
- **Area:** crates/cutweave-ops, packages/editor
- **Depends on:** WP-4I.03, WP-4I.04, WP-4I.02, E2C
- **Size:** M
- **Executor:** sonnet
- **Scope:** `tracking.apply{track,target,mode: position|position_scale_rotation,offset,smoothing,keyframe_density}` generates keyframes on the target's transform (text, sticker, composition, mask centre or effect point-param) with RDP decimation (tolerance 0.5 px) as one transaction; `tracking.list`, `tracking.delete`; UI: tracking tool overlay on the preview (draw region, Track fwd/back with progress, path overlay, apply to selected item), stabilisation section (analyse, smoothness slider, crop mode).
- **Interfaces:** MCP catalog `tracking_apply`; UI consumes `tracking.*`, `clip.stabilize`.
- **Acceptance criteria:** Playwright e2e: track a moving object, attach text → text centre stays within ±3 px of the object over 150 frames; keyframe count ≤ 15% of frames at tolerance 0.5 px; apply is one undo step; works on stabilised clips.
- **Tests:** Playwright-Electron, ground-truth e2e, proptest on RDP (max deviation ≤ tolerance).
- **Out of scope:** AI object tracking (Phase 5 `SegmentTrack`).

---

## Traceability

### Roadmap scope bullets → WPs

| Roadmap bullet | WPs |
|---|---|
| WGSL effect framework: `effect.toml`, param types, multi-pass, keyframeable params, validation, GPU budgets, hot-reload | 4A.01, 4A.02, 4A.03, 4A.04, 4A.05, 4A.06, 4A.07, 4A.08, 4A.09 |
| ≥ 40 transitions (dissolves, wipes, zooms, spins, glitch, light leaks, slide/push, blur) | 4B.01–4B.05 (45), 4B.06 |
| ≥ 50 video effects (blur, glow, VHS, RGB split, shake, zoom pulse, grain, vignette, pixelate, mirror, kaleidoscope, lens distortion) | 4C.01–4C.06 (54), 4C.08 |
| ≥ 40 filters/LUTs | 4D.01–4D.06 (45), 4D.07 |
| Clip animations in/out/combo | 4E.01–4E.05 (28 presets) |
| Color: basic adjustments, HSL, curves, LUT import, wheels, scopes | 4F.01, 4F.02, 4F.03, 4F.04, 4F.05, 4F.06 |
| Masks & keying: shape masks (feather, invert, keyframeable), chroma key (spill), luma key, adjustment layers | 4G.01, 4G.02, 4G.03, 4G.04, 4G.05 |
| Speed: curve presets (montage, hero, bullet, flash in/out), optical-flow frame blending (basic) | 4H.01, 4H.02, 4H.03, 4H.04 |
| Motion: stabilization (vidstab analysis job), motion tracking (point/planar-lite) driving keyframes | 4I.01, 4I.02, 4I.03, 4I.04, 4I.05 |

### Exit criteria → WPs

| Exit criterion | WPs that deliver / verify it |
|---|---|
| "CapCut trend recreation" suite (10 edits, built-ins only) passes design review | 4C.07 (suite), **4C.08 (gate)**; supporting 4B.*, 4C.01–06, 4D.*, 4E.*, 4F.*, 4G.*, 4H.*, 4I.* |
| All effects render identically in preview and export (golden frames) | 4A.03, 4A.04, 4A.08 (parity + determinism check enforced for every item), DoD-GF, every content WP via QC-FX 3–4 |
| Each effect within its GPU budget on the iGPU reference | 4A.05 (profiler, static lint, nightly runner), QC-FX 6 in every content WP, 4C.07 (nightly run of the trend suite) |

## Notes for integrator

1. **WP count is 55 (guideline 35–50).** Content batches (5 transition, 6 effect, 5 filter, 3 animation) plus 3 human gates account for 22 of them. To cut, merge 4B.01+4B.02 and 4D.02+4D.03 (both S).
2. **Content-count targets exceeded deliberately:** 45 transitions, 54 effects, 45 filters, 28 clip animations (roadmap minima 40/50/40; headroom absorbs design-review culls).
3. **Dependency on P3's `cutweave-plugins` skeleton (WP-3B.01).** P4 cannot merge loader code before it; 4A.01 spec work can start earlier. If P3 slips, move 3B.01 to the critical path or give P4 its own copy and reconcile (not recommended). P6A must extend this crate.
4. **Trend suite epic.** The roadmap exit needs a recreation suite but 05-work-breakdown.md has no epic for it, and the instruction forbids new epics; it lives in 4C (4C.07/4C.08) and depends on nearly every other P4 epic plus P3's text (WP-3A.01). A dedicated "4J Integration & review" epic would be cleaner.
5. **GPU budgets are proposals.** The 0.5/1.2/2.5 ms tiers and 1.5 ms transition budget assume Spike S4's "1080p60 < 8 ms total GPU on an iGPU" holds; re-calibrate after S4 and after the first nightly numbers. CI has **no iGPU**: hosted CI only runs the static lint; real measurement needs self-hosted Intel Xe + Apple M1 runners (4A.05) — infra owner needed.
6. **Determinism vs feedback effects.** research/07 §2 mentions persistent ping-pong buffers, which break "pure function of (snapshot, t)" and parallel chunked export. This plan replaces them with `history = N` temporal taps from the frame cache (≤ 8 frames). Echo Trail and Frame Stutter use it; true recursive feedback is out of scope. Needs ADR sign-off.
7. **Golden frames per adapter.** WARP, lavapipe and Metal differ slightly; store blessed goldens per adapter family with PSNR ≥ 45 dB tolerance (colour-science tests use ΔE00). Cross-adapter exactness is not asserted; same-adapter bit-identity is.
8. **Command namespaces.** §5.1 lacks `analysis` and `tracking`; this phase adds `tracking.*` (explicitly not `track.*`, which collides with timeline tracks) and `analysis.stabilize`; filters use `color.apply_filter`; clip animations use `clip.set_animation`; adjustment layers use `timeline.add_adjustment`; the project field is `motion_tracks` (not `tracks`). Suggest amending §5.1.
9. **FFmpeg build must include libvidstab** (GPL; allowed). Update 02-tech-stack "FFmpeg build policy" and the CI FFmpeg recipe; 4I.01 has a built-in tracker fallback.
10. **Timemap coupling.** P3's `cutweave_ops::timemap` (WP-3E.01) maps source↔timeline time for captions and transcript editing; it must consume 4H.01's speed-curve integral once curves exist (captions on a ramped clip). Add a regression test in whichever phase merges second.
11. **Licensing of first-party content.** No shader code from Shadertoy/third parties; LUTs are generated by our recipes; names are generic (no brand film stocks, no platform trademarks). Proposed licence: plugin sources GPL-3.0-or-later with the plugin exception, data (LUTs, thumbnails) CC0 — confirm with ADR-011.
12. **Panel registry and UI conflicts.** 4A.09/4E.05/4F.06/4G.02/4H.04/4I.05 add tabs and inspector sections while P3 and P5 add their own; E2H should provide a registry (panels + inspector sections) to avoid merge conflicts in `packages/editor`.
13. **Optical flow cost.** 4H.02's export-speed target (≥ 8 fps at 0.5× on iGPU) is a guess; if missed, keep `auto` conservative and defer quality work to the Phase 5 AI interpolation (`retime_provider = ai`).
14. **Beat-sync effects.** Zoom Pulse/Strobe use manual `bpm`+`offset` until Phase 5 beat detection writes markers; add a binding WP then (not planned here).
15. **Preview-only params.** Key "view mode" and debug views use `preview_only` params excluded from export; the spec (4A.01) and the export path must both honour it (golden test in 4G.02).
16. **Generators and blend modes** appear in the `effect.toml` `kind` enum but have no dedicated WP here (roadmap does not list them beyond P2's basic blend modes); light-leak and overlay looks are procedural transitions/effects only.
