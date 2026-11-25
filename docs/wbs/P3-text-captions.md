# Cutweave — Phase 3 Work Breakdown: Text, Captions & Transcript Editing → M3 Public Alpha

> Status: baseline 2026-11-22. Contract: [01-architecture.md](../01-architecture.md) (§8 render/text, §11 captions, §12 compositions, §14 MCP, §15 plugins). Roadmap: [04-roadmap.md](../04-roadmap.md#phase-3--text-captions--transcript-editing-8-weeks--m3-public-alpha). Epic codes: [05-work-breakdown.md](../05-work-breakdown.md).
> Earlier-phase dependencies are named by epic (E1A … E2J) as defined in 05-work-breakdown.md. Phase 4 and Phase 5 run in parallel; cross-phase WP references are listed in "Notes for integrator".

## Phase goal

CapCut's moat: fast, beautiful captions and text-driven editing.

## Exit criteria (copied from roadmap)

**Exit (M3, Public Alpha):** a new user turns a 3-min talking-head clip into a styled, captioned, silence-cut 9:16 short with music in **under 5 minutes**. An agent does the same via MCP from a one-line instruction. Installers exist for all 3 OSes (unsigned OK for alpha).

## Epics (one-line goals)

| Epic | Goal |
|---|---|
| **3A Text engine** | Rich text model, fonts, shaping/layout, MSDF atlas + render pass with fill/outline/shadow/glow/gradient/box, text UI. |
| **3B Compositions runtime & `@cutweave/compose`** | `cutweave-plugins` v0 skeleton, `@cutweave/compose` v1, oxc + QuickJS runtime, scene-graph rendering, composition items/commands/inspector, first-party lower thirds/titles/callouts/progress/CTA. |
| **3C Text animations & presets** | Per-glyph/word/line animators, in/out/loop text animations built as compositions, text preset library. |
| **3D Transcripts & Stt providers** | Transcript model, minimal AI-gateway slice (Stt only), ElevenLabs Scribe + local whisper.cpp providers, `transcript.generate`, transcript panel. |
| **3E Captions & style gallery** | Caption track over transcripts, chunking, style package format + renderer, **28 caption styles**, SRT/VTT/ASS, burn-in/sidecar, bilingual. |
| **3F Text-based editing** | Delete/reorder transcript words to edit the timeline; silence and filler-word removal with a review UI. |
| **3G Asset packs** | L0 pack loader, stickers/overlays/emoji, CC0 music + SFX starter library, library UI. |
| **3H Agent Kit v0** | MCP tools/resources for captions & transcripts, 3 skills, 3 MCP prompts, scripted-agent e2e. |
| **3I Alpha packaging** | Unsigned installers on 3 OSes, first-run, opt-in self-hosted crash reporting (moved from P7 7B.01), smoke tests, the < 5 min usability gate. |

## Critical path

`3B.01 (plugins skeleton)` → `3B.02/3B.03 (compose API + runtime)` → `3B.04 (scene render)` → `3C.01 (animators)` → `3E.03 (caption style pkg)` → `3E.04 (caption renderer)` → `3E.06–3E.09 (28 styles)` → `3E.10 (design gate)` → `3I.04 (< 5 min usability gate)`.
Parallel chains that must also land: `3A.02 → 3A.03 → 3A.04 → 3A.05` (text pixels, needed by 3B.04 and 3E.04); `3D.02 → 3D.03/3D.04 → 3D.05 → 3E.01 → 3F.02 → 3F.04` (words → captions → cuts); `3H.01–3H.04 → 3H.05` (agent exit criterion); `3I.01 → 3I.03` (installers).
Longest single chain ≈ 8 weeks if 3A, 3B and 3D start in week 1.

## Definition of done — applies to EVERY WP below (not repeated per WP)

- **DoD-CMD:** every new command/query is declared with `#[command]`/`#[query]` (`mcp = core|catalog`, annotations set), appears in `schema/commands.json`, generated TS and `docs/reference/commands.md`; the CLI = MCP = RPC parity test and the "every non-`view` command reachable via MCP" test stay green. Mutating commands have an undo/redo property test (`undo(apply(doc)) == doc`).
- **DoD-CI:** `cargo clippy -D warnings`, `cargo nextest`, `cargo deny`, Biome + Vitest green; wasm-safe crates (`cutweave-model`, `cutweave-commands`, `cutweave-render` core, `cutweave-text`) still build for `wasm32-unknown-unknown`; license headers + DCO.
- **DoD-GF (anything that draws pixels):** golden frames rendered headless with `cutweave render` on the software adapters (WARP / lavapipe) at ≥ 3 named times; regression vs the stored per-adapter reference = SSIM ≥ 0.995 AND PSNR ≥ 40 dB; **preview path == export path** (full-quality preview frame is bit-identical to the pre-encode export frame for the same `(rev, t)` on the same adapter); two runs are bit-identical (determinism).
- **DoD-DOC:** public APIs documented; behaviour changes noted in `docs/reference/` (auto-generated where possible).

## Shared quality checklists (referenced by content WPs)

**QC-COMP (first-party compositions & text animations):** (1) `cutweave compose check` passes, props JSON Schema has defaults, min/max, `x-cutweave-ui` hints, and every prop has a description; (2) runs at 16:9, 9:16, 1:1 without clipping (safe-area aware, positions derived from `size`); (3) scene eval ≤ 0.3 ms/frame CPU, ≤ 150 nodes, GPU ≤ 0.5 ms at 1080p iGPU; (4) deterministic (hash of 3 frames equal across 2 runs); (5) GF at t = in-end, mid, out-start; (6) animated 12-frame webp thumbnail auto-generated (`cutweave pack thumbs`); (7) license header, original design (no trademarked names/logos/third-party artwork), fonts from the bundled OFL set only; (8) appears in `composition.list` with description + tags ≥ 3; (9) an MCP script adds it with default props via `cutweave_execute` and `render_frame` is non-empty.

**QC-CAP (caption styles):** (1) `cutweave caption-style check` passes; (2) legible on 3 reference backgrounds (dark, bright, busy): measured contrast ≥ 4.5:1 against the median background luma OR outline/box present; (3) works for 1-line and 2-line blocks, 16:9 and 9:16, inside every platform safe-zone preset; (4) ≥ 3 colour variants exposed as palette param; (5) active-word state renders correctly for first/middle/last word (GF ×3 × 2 aspect ratios); (6) caption pass cost ≤ 0.5 ms GPU + 0.3 ms CPU per frame at 1080p on the iGPU reference with 2 lines visible; (7) fonts bundled & OFL; (8) CJK/RTL fall back gracefully (renders without highlight animation, never tofu for bundled-subset scripts); (9) ids `cutweave.cap.<slug>`, name ≤ 24 chars, description, tags (vibe, motion, platform); (10) design-gate sign-off by 3E.10.

## WPs

---

# Epic 3A — Text engine

### WP-3A.01 — Text model, commands & queries
- **Area:** crates/cutweave-model, crates/cutweave-ops, crates/cutweave-commands
- **Depends on:** E1A, E1B, E2C
- **Size:** M
- **Executor:** sonnet
- **Scope:** `RichText { runs: Vec<Run{text, style_override?}> }`; `TextStyle` { font{family,weight,italic,axes}, size, fill: Solid|Linear|Radial gradient, strokes[] (width, join, colour, order), shadows[] (offset, blur, colour), glow{radius,colour,intensity}, background box {colour, padding, radius, mode: block|per-line}, letter_spacing, line_height, align, v_align, case_transform, auto_fit: none|shrink|grow_box, direction: auto|ltr|rtl }; `Item::Text` gets `box`, `style: TextStyleRef|inline`, `animation{in,out,loop}: Option<AnimRef>` (null until 3C); `text` track kind; schema_version bump + migration + golden fixture; commands `text.add`, `text.set_content`, `text.set_style` (merge-patch), `text.set_run_style{range,…}`, `text.set_box`, `text.add_emoji`; queries `text.get`, `text.styles.list`.
- **Interfaces:** `cutweave_model::text::{RichText,TextStyle,TextBox}`; MCP core tool `text_add`; all visual props animatable via existing `Animatable<T>` (E2C).
- **Acceptance criteria:** project round-trips; migration from schema vN fixture passes; `cutweave_execute` adds a titled text with defaults in ≤ 1 call and `timeline_get` shows it; invalid font size/colour returns `Invalid{path,msg}`; locked track returns `Locked`.
- **Tests:** proptest undo/redo for every command; insta schema snapshots; serialize round-trip; wasm32 build.
- **Out of scope:** rendering, fonts, animation, captions.

### WP-3A.02 — Font manager & bundled font set
- **Area:** crates/cutweave-text (`fonts`), assets/fonts, crates/cutweave-ops
- **Depends on:** WP-3A.01
- **Size:** S
- **Executor:** sonnet
- **Scope:** `FontDb` on `cosmic-text`/`fontdb`: async system-font scan (cached index in `cache.sqlite`), bundled OFL fonts under `assets/fonts/` with `LICENSES.json`: Inter, Roboto, Montserrat, Poppins, Oswald, Bebas Neue, Anton, Playfair Display, Lora, Merriweather, Caveat, Pacifico, Lobster, Permanent Marker, Bangers, Space Grotesk, JetBrains Mono, Noto Sans (Latin/Cyrillic/Greek), Noto Color Emoji; CJK/Arabic/Devanagari/Thai as **downloadable font packs** (L0 `pack.toml`, on demand) with a bundled Noto Sans fallback chain; project font assets (`assets.fonts`, content hash, collect/archive aware; missing font → fallback + warning); `font.list{query,category,source}`, `font.import{path}` (parses, shows licence/embedding flags), `font.get`.
- **Interfaces:** `FontDb::{resolve(family,weight,italic) -> FontId, fallback_chain(script)}`; namespace `font` (new — see Notes).
- **Acceptance criteria:** system font scan of 500 fonts < 1.5 s cold, < 100 ms warm; every bundled font has SPDX OFL-1.1 entry (CI check fails otherwise); unknown family resolves deterministically to the documented fallback; installer font payload ≤ 25 MB.
- **Tests:** unit for resolution/fallback; CI licence check script; snapshot of `font.list`.
- **Out of scope:** shaping, rendering, CJK font content itself (pack only).

### WP-3A.03 — Shaping & rich-text layout
- **Area:** crates/cutweave-text (`layout`)
- **Depends on:** WP-3A.01, WP-3A.02, E0F
- **Size:** M
- **Executor:** opus
- **Scope:** `layout(rich_text, style, box, scale) -> TextLayout { lines, glyphs[{glyph_id,font,cluster,x,y,advance,line,word,run}], words[], bounds }` using cosmic-text/swash: bidi, CJK line breaking (ICU segmenter, kinsoku), emoji ZWJ clusters, variable-font axes, letter/line spacing, alignment, auto-fit (deterministic binary search), case transforms, per-run style; pure and deterministic (no system fonts unless the project asks for them); glyph/word/line metrics exposed for animators (3C.01) and caption highlighting (3E.04); query `text.layout{item}` returns glyph rects for UI caret/selection.
- **Interfaces:** `cutweave_text::layout::{layout, TextLayout, GlyphPos}`; query `text.layout`.
- **Acceptance criteria:** layout JSON snapshots stable across Win/macOS/Linux for fixtures (Latin, Arabic RTL, Hebrew, Japanese, Thai, Devanagari, ZWJ emoji, variable font) using bundled fonts only; 1,000 glyphs laid out < 2 ms; auto-fit converges ≤ 12 iterations and never overflows the box (property test); `cutweave-text` builds for wasm32.
- **Tests:** insta layout snapshots; proptest bounds/containment; criterion bench.
- **Out of scope:** drawing; vertical text layout.

### WP-3A.04 — MSDF glyph atlas
- **Area:** crates/cutweave-text (`msdf`), crates/cutweave-cache
- **Depends on:** WP-3A.03, E0F
- **Size:** M
- **Executor:** opus
- **Scope:** MSDF generation from swash outlines (own implementation or vetted crate; GPL-compatible), em-size 48 px with range 4, corner-preserving; skyline atlas packing; LRU eviction with VRAM budget; background generation thread; disk cache keyed `(font hash, glyph, params)`; flags glyphs needing fallback (colour/bitmap, > size threshold) for 3A.06; atlas texture upload API for `cutweave-render`.
- **Interfaces:** `cutweave_text::msdf::{Atlas, GlyphEntry{uv,metrics,kind}}`; `Atlas::ensure(&[GlyphKey]) -> Ready|Pending`.
- **Acceptance criteria:** ≥ 2,000 glyphs/s generation on reference CPU; render of "Ag@%" at 4K via atlas has SSIM ≥ 0.99 vs vector reference; sharp corners preserved (corner-error ≤ 0.5 px at 256 px); eviction never drops a glyph needed by the current frame; no CPU stall on the render thread (missing glyph → placeholder next frame in preview, **blocking generation in export**).
- **Tests:** unit + SSIM tests; criterion bench; determinism (same atlas bytes twice).
- **Out of scope:** drawing the text; colour emoji.

### WP-3A.05 — Text render pass (fill, outline, shadow, glow, gradient, box)
- **Area:** crates/cutweave-render (`text_pass`), WGSL
- **Depends on:** WP-3A.04, E1F
- **Size:** M
- **Executor:** opus
- **Scope:** per-glyph instance buffer {transform, colour, opacity, offset, scale, rotation, skew, extra} (the animator hook); WGSL MSDF fragment: multi-stroke outline (ordered, mitre/round joins), drop shadow (offset + blur via widened SDF + optional blur pass), glow (SDF dilation + blur), linear/radial gradient in text-box space, background box (rounded, padding, per-line "pill" mode); premultiplied alpha, **composited in linear light**; Text item evaluation in timeline eval (keyframed visual props honoured); deterministic.
- **Interfaces:** `cutweave_render::text::{TextPassInput, GlyphInstance}`; `render.frame` shows Text items.
- **Acceptance criteria:** S4 gate re-verified on production code: 1,000 animated glyphs < 1 ms GPU on iGPU reference; 4K text crisp (SSIM ≥ 0.99 vs vector ref); 1080p single-title cost ≤ 0.4 ms GPU; GF for ≥ 12 style combinations (outline+shadow+glow, gradient, box) preview == export.
- **Tests:** golden frames; GPU timing bench (nightly on reference iGPU); determinism.
- **Out of scope:** animation logic; large-glyph vector path (3A.06).

### WP-3A.06 — Vector/colour glyph fallback & text golden + perf suite
- **Area:** crates/cutweave-text, crates/cutweave-render
- **Depends on:** WP-3A.05
- **Size:** M
- **Executor:** opus
- **Scope:** path for glyphs beyond MSDF quality (> 400 px effective or heavy warp): Vello (or lyon fallback if ADR-007 selects it); colour emoji (COLRv1/CBDT/sbix via swash → RGBA atlas, premultiplied); ZWJ emoji sequences; text golden suite of ≥ 30 frames (CJK, RTL, emoji, variable font, 4K, stroke joins, gradient-on-emoji exclusion rules) and perf bench wired into the E2J benchmark harness.
- **Interfaces:** `GlyphKind::{Msdf, Path, ColourBitmap}` selected per glyph by `Atlas`.
- **Acceptance criteria:** all suite frames GF-pass; 200-char two-line caption renders ≤ 0.5 ms GPU; switching MSDF→path at threshold shows no visible pop (max ΔE ≤ 1.5 across the boundary); preview == export.
- **Tests:** golden, bench (criterion + GPU nightly).
- **Out of scope:** emoji picker UI (3G.05).

### WP-3A.07 — Text UI: add, inspector, on-canvas editing
- **Area:** packages/editor, packages/ui
- **Depends on:** WP-3A.01, WP-3A.03, WP-3A.05, E1M, E2H
- **Size:** M
- **Executor:** sonnet
- **Scope:** Text tab "Add text" (title/subtitle/body defaults); schema-driven inspector extended with custom controls: font picker (search, preview, categories, recents), colour/gradient picker, stroke/shadow/glow list editors, background box, spacing, alignment, auto-fit; on-canvas editing overlay (DOM caret/selection positioned from `text.layout`), transform box; keyboard (Enter = newline, Esc = commit); all edits through commands with ephemeral transactions while dragging.
  - Settings > Editing: duration of new text (`editing.text_duration`, 5 s); "Add text" uses it (catalog `docs/reference/settings.md`).
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** a menu on a text clip (in the Preview and on the timeline): Edit text, Copy style, Paste style, Save as preset…, plus the Text tab's preset cards (Apply, Remove).
- **Interfaces:** consumes `text.*`, `font.list`, `text.layout`; registers a panel in the left-tab registry (see Notes).
- **Acceptance criteria:** Playwright e2e: add text → type → change font/colour/stroke → undo ×3 restores; caret position matches rendered glyph within 1 px at 100% zoom; IME composition (CJK) input works; every control has an aria-label.
- **Tests:** Vitest stores; Playwright-Electron e2e; visual regression for the inspector.
- **Out of scope:** presets and animations (3C).

---

# Epic 3B — Compositions runtime & `@cutweave/compose`

### WP-3B.01 — `cutweave-plugins` v0 skeleton (manifest, discovery, contribution registry)
- **Area:** crates/cutweave-plugins, schema/
- **Depends on:** E1B, E1I
- **Size:** M
- **Executor:** opus
- **Scope:** `cutweave-plugin.toml` manifest v0 (research/07 §7 subset: id, version, license, `host_api`, `[[contributes.*]]` for effects, compositions, caption_styles, text_presets, packs, clip_animations, filters); discovery of bundled `plugins/` + user dir; typed `ContributionRegistry`; L0 data-pack validators (JSON Schema per kind); `PluginRef` resolution (`"cutweave.fx.gaussian-blur@1"`); project pin record + placeholder for missing plugins; events `plugin.changed`; queries `plugin.list`, `plugin.describe`. **No wasmtime, permissions UI or distribution** (Phase 6 extends this crate, not replaces it).
- **Interfaces:** `cutweave_plugins::{Manifest, Registry, Contribution, PluginRef}`; consumed by 3B.05, 3C, 3E, 3G and Phase 4 (4A.01).
- **Acceptance criteria:** fixture plugin dir loads in < 50 ms; malformed manifest yields diagnostics with file+line; missing referenced plugin opens project with placeholder and a `plugin.missing` event; hot-reload hook (`Registry::reload(path)`) works; `plugin_list` reachable via MCP.
- **Tests:** unit, fixtures, insta schema snapshots.
- **Out of scope:** WASM host, permissions, signing, index, UI panels.

### WP-3B.02 — `@cutweave/compose` scene API v1
- **Area:** packages/compose-runtime (MIT), docs/reference/compose.md, schema/scene-graph.json
- **Depends on:** E0G
- **Size:** M
- **Executor:** opus
- **Scope:** original API (do **not** mirror Remotion names): `defineComposition({id, props, duration, render(ctx)})`, `ctx = {frame, time, fps, size, props, progress, random(seed)}`; props schema builder `p` (`p.string()`, `p.number()`, `p.bool()`, `p.enum()`, `p.color()`, `p.font()`, `p.asset()`, `p.angle()`, `p.object()`, `p.array()`) emitting JSON Schema 2020-12 with `x-cutweave-ui` hints (named `p`, not zod's `z`; no `z` alias); `interpolate`, `spring` (closed-form damped), `Easing` (linear, ease*, cubic-bezier, back, elastic, bounce); `seq(from,dur,fn)`/`series`; nodes `Group, Rect, Ellipse, Path, Text, Image, Video, EffectRef, Mask`; JSX runtime producing plain JSON nodes; scene-graph JSON Schema = contract with 3B.04; TS types + docs.
- **Interfaces:** `@cutweave/compose` exports; `schema/scene-graph.json`.
- **Acceptance criteria:** bundle ≤ 30 KB minified; math helpers match fixture tables to 1e-9; types compile under `tsc --strict`; the architecture §12 example uses `p` (`import { defineComposition, p } from '@cutweave/compose'`) and compiles as written; no `Date`/`Math.random` references (lint).
- **Tests:** Vitest numeric fixtures, type tests, bundle-size gate.
- **Out of scope:** text animators (3C.01), caption context (3E.03).

### WP-3B.03 — `cutweave-compose`: oxc transpile + QuickJS sandbox runtime
- **Area:** crates/cutweave-compose
- **Depends on:** WP-3B.02, E0G
- **Size:** M
- **Executor:** opus
- **Scope:** oxc TS/JSX → JS with local-module bundling; structured diagnostics `{file,line,col,code,message}`; `SceneRuntime` trait + rquickjs impl: no I/O globals, `Date`/`Math.random`/timers/`eval`/`Function` removed, seeded RNG, 64 MB memory, stack limit, interrupt 10 ms soft / 50 ms hard per frame; `eval(composition, props, frame, fps, size) -> SceneGraph` (cap 5,000 nodes); static-subtree hash cache; failure → placeholder scene + error overlay, never a crash.
- **Interfaces:** `cutweave_compose::{compile, SceneRuntime, SceneGraph, Diagnostic}`.
- **Acceptance criteria:** transpile < 50 ms for a 200-line TSX; 200-node scene eval < 1 ms/frame; scene-graph hash identical across Win/macOS/Linux and 3 runs; `while(true){}` terminated < 60 ms; attempts to reach `fetch`, `require`, `process`, `Date.now` throw documented errors; memory bomb stopped at limit.
- **Tests:** unit, sandbox-escape suite (≥ 20 cases), cargo-fuzz target for the transpiler input, criterion benches.
- **Out of scope:** drawing (3B.04), wasm-component runtime backend.

### WP-3B.04 — Scene-graph rendering in `cutweave-render`
- **Area:** crates/cutweave-render (`scene`), WGSL
- **Depends on:** WP-3B.03, WP-3A.05, E1F
- **Size:** M
- **Executor:** opus
- **Scope:** draw `SceneGraph`: Group (transform, opacity, blend, clip), Rect/RoundRect/Ellipse (SDF), Path (lyon/Vello, fill/stroke/dash/gradients), Text (via `cutweave-text` incl. glyph instances), Image/Video (host-resolved asset refs, project/pack assets only), Mask, EffectRef (logs `effect.unavailable` until Phase 4 lands); static-subtree layer cache; linear-light premultiplied compositing; deterministic.
- **Interfaces:** `cutweave_render::scene::draw(scene, ctx)`; `CompositionInstance` items render through it.
- **Acceptance criteria:** GF on 10 reference scenes (shapes, paths, nested groups, text, images); 200-node scene ≤ 1.0 ms GPU on iGPU; preview == export; unknown node kinds rejected with diagnostics, not panics.
- **Tests:** golden, bench, fuzz-lite on random scene graphs (no panics, bounded time).
- **Out of scope:** effect nodes' real implementation (Phase 4).

### WP-3B.05 — Composition packaging, instances, commands & props inspector
- **Area:** crates/cutweave-plugins, crates/cutweave-ops, crates/cutweave-cli, packages/editor
- **Depends on:** WP-3B.01, WP-3B.03, WP-3B.04, E1M
- **Size:** M
- **Executor:** sonnet
- **Scope:** `composition.toml` loader (L1.5 contribution; fields per research/07 §6.3); `CompositionDef` in project + embedded source for user-authored ones; `CompositionInstance` item (props `Animatable`, duration, visual props); commands `composition.create{source,…}`, `composition.update_source`, `composition.validate{source}` (diagnostics, no mutation), `composition.add_instance{composition,track,start,duration,props}`, `composition.set_props` (keyframeable per prop), queries `composition.list`, `composition.describe`; `cutweave compose check <dir>` CLI (compile + contact-sheet PNG); props JSON Schema → schema-driven inspector with keyframe diamonds.
- **Interfaces:** MCP core `composition_create`; others reachable via `cutweave_execute`.
- **Acceptance criteria:** scripted agent test: create a lower third from TSX source → add instance → `render_frame` returns a non-empty image — with **no screenshots**; syntax error returns diagnostic pointing at the correct line/col; props keyframes animate; project saved/loaded with embedded source; undo/redo.
- **Tests:** unit, MCP e2e, Playwright inspector test, golden.
- **Out of scope:** third-party plugin distribution, wasm compositions.

### WP-3B.06 — Compositions batch A: lower thirds ×8 + titles ×6
- **Area:** plugins/compositions-core
- **Depends on:** WP-3B.05, WP-3A.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** *Lower thirds:* Clean Bar, Slide Tab, Name Pill, Accent Underline, Split Reveal, Minimal Line, Gradient Ribbon, Social Handle (generic icon, no brand logos). *Titles:* Big Center Title, Kinetic Words, Typewriter Title, Split Title, Boxed Title, Outline Draw Title. Props: text lines, accent/secondary colour, font, duration, in/out style. Shared helpers in `plugins/compositions-core/lib`.
- **Interfaces:** ids `cutweave.comp.lower-third.<slug>`, `cutweave.comp.title.<slug>`.
- **Acceptance criteria:** QC-COMP for all 14; each has a design thumbnail; names ≤ 24 chars.
- **Tests:** golden ×3 times per item, MCP add-by-id script, `cutweave compose check` in CI.
- **Out of scope:** text-animation presets (3C), templates (Phase 6).

### WP-3B.07 — Compositions batch B: callouts ×6, progress bars ×4, CTA ×4
- **Area:** plugins/compositions-core
- **Depends on:** WP-3B.05, WP-3A.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** *Callouts:* Arrow Callout, Circle Highlight, Scribble Underline, Speech Bubble, Number Badge, Box Highlight. *Progress:* Linear Bar, Thin Top Bar, Ring Progress, Step Dots (progress bound to item duration or a `value` prop, keyframeable). *Subscribe/CTA:* Subscribe Button (click animation, generic bell — no platform logo/trademark), Bell Ring, Follow Button, Like-Share-Follow Trio.
- **Interfaces:** ids `cutweave.comp.callout.<slug>`, `.progress.<slug>`, `.cta.<slug>`.
- **Acceptance criteria:** QC-COMP for all 14; progress bars reach exactly 100% at last frame (test).
- **Tests:** golden, MCP script, determinism.
- **Out of scope:** AI-generated variants.

---

# Epic 3C — Text animations & presets

### WP-3C.01 — Text animator framework (per-glyph / word / line)
- **Area:** packages/compose-runtime, crates/cutweave-compose, crates/cutweave-text, crates/cutweave-render
- **Depends on:** WP-3B.03, WP-3B.04, WP-3A.03, WP-3A.05
- **Size:** M
- **Executor:** opus
- **Scope:** `animate.text({unit:'glyph'|'word'|'line', stagger, order(forward|reverse|center|random(seed)), range, ease, props:{x,y,scale,rotation,opacity,colour,skew,blur}})`; host binding `ctx.text.metrics` (glyph/word/line index, count, bounds from `cutweave-text::layout`); runtime output = per-glyph instance overrides consumed by the 3A.05 instance buffer (no per-glyph JS calls: animators are declarative curves evaluated natively).
- **Interfaces:** `@cutweave/compose` `animate`, `Text` node `animators[]`; `cutweave_text::animator::{Animator, eval}`.
- **Acceptance criteria:** 1,000-glyph stagger animation < 1 ms CPU + < 1 ms GPU per frame (native evaluation); deterministic random order (same seed → same result across OS); GF for 6 reference animators; CJK/RTL/emoji clusters never split mid-cluster.
- **Tests:** golden, property (every glyph reaches final state at t_end), criterion bench.
- **Out of scope:** the shipped animation library.

### WP-3C.02 — Text animation binding (in / out / loop), commands & picker UI
- **Area:** crates/cutweave-model, crates/cutweave-ops, crates/cutweave-plugins, packages/editor
- **Depends on:** WP-3C.01, WP-3B.05, WP-3A.07
- **Size:** M
- **Executor:** sonnet
- **Scope:** `kind="text-animation"` compositions (params: duration, intensity, direction); `AnimRef` on `Text` and `Caption` items; timing rules (in at item start, out ends at item end, loop fills the middle with period); commands `text.set_animation{item,slot,anim,params,duration}`, `text.clear_animation`; queries `text.animations.list{slot,query}`; fallback to no animation + warning if plugin missing; UI: Text tab "Animations" section and inspector picker with animated hover thumbnails (cached webp).
- **Interfaces:** MCP catalog tools reachable through `cutweave_execute`; `text_add` accepts `animation_in/out/loop` convenience args.
- **Acceptance criteria:** applying in+out+loop to one title yields 3 non-overlapping time ranges (test); animations shorter than the item clamp to its duration; GF at in-end/mid/out-start; undo/redo; picker thumbnails load < 200 ms from cache.
- **Tests:** unit, golden, Playwright.
- **Out of scope:** caption-specific animation (3E).

### WP-3C.03 — Text animations pack: IN ×10
- **Area:** plugins/text-animations
- **Depends on:** WP-3C.02
- **Size:** S
- **Executor:** sonnet
- **Scope:** Fade Up, Typewriter, Pop Scale, Slide Left, Slide Right, Blur In, Word Rise (staggered by word), Letter Drop, Spin In, Wipe Reveal.
- **Interfaces:** ids `cutweave.txa.in.<slug>`.
- **Acceptance criteria:** QC-COMP; default duration 0.5 s (0.8 s for staggered), intensity param 0.2–2.0; reduced-motion flag honoured (instant fade).
- **Tests:** golden (start, mid, end), MCP script.
- **Out of scope:** out/loop.

### WP-3C.04 — Text animations pack: OUT ×8 + LOOP ×6
- **Area:** plugins/text-animations
- **Depends on:** WP-3C.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** *Out:* Fade Out, Slide Down Out, Shrink Out, Blur Out, Word Fall, Letter Scatter, Wipe Out, Glitch Out. *Loop:* Pulse, Wiggle, Float, Shimmer Sweep, Neon Flicker, Wave (all seamless: value(t=0) == value(t=period) within 1e-6).
- **Interfaces:** ids `cutweave.txa.out.<slug>`, `cutweave.txa.loop.<slug>`.
- **Acceptance criteria:** QC-COMP; loops seamless (numeric test); out animations end fully transparent or off-canvas.
- **Tests:** golden, seamlessness test.
- **Out of scope:** combos.

### WP-3C.05 — Text presets: L0 format, commands & pack of 30
- **Area:** plugins/text-presets, crates/cutweave-plugins, crates/cutweave-ops
- **Depends on:** WP-3A.01, WP-3C.02, WP-3A.05
- **Size:** M
- **Executor:** sonnet
- **Scope:** `text-preset.json` (style + default in/out/loop animation + sample text + category + tags); commands `text.apply_preset{item|new,preset}`, `text.save_preset{item,name}` (user presets in app data); query `text.presets.list{category,query}`. Pack of **30**: *Titles:* Clean Bold, Outline Pop, Neon Glow, Retro Eighties, Comic Burst, Gradient Sunset, Chrome Metal, Handwritten Note, Minimal Light, Cinematic Spaced, Boxed Label, Sticker Border, Heavy Shadow, Paper Cut, Double Stroke, Outline Only, Graffiti Tag, Serif Elegant. *Subtitle-like:* Classic Subtitle, Yellow Subtitle, Boxed Subtitle, Soft Shadow Subtitle. *Social:* Quote Card, Hashtag Tag, Price Tag, Breaking Banner, Question Prompt, Step Number, Review Stars, Location Pin.
- **Interfaces:** ids `cutweave.txp.<slug>`; MCP `text_add{preset}`.
- **Acceptance criteria:** each preset passes legibility check (contrast ≥ 4.5:1 or outline/box on the 3 reference backgrounds); GF ×1 each; thumbnails auto-generated; apply preset is one undo step; `save_preset` round-trips.
- **Tests:** golden, unit, MCP script.
- **Out of scope:** marketplace/sharing.

---

# Epic 3D — Transcripts & Stt providers

### WP-3D.01 — Transcript model, queries & commands
- **Area:** crates/cutweave-model, crates/cutweave-ops
- **Depends on:** E1A, E1B
- **Size:** M
- **Executor:** sonnet
- **Scope:** `Transcript{id,source{media,stream,range}|synthetic,language,words,segments,speakers,provenance}`, `Word{id,text,start,end,conf,speaker?,kind(word|punct|audio_event),flags}` (times in source-media flicks), `Speaker{id,label,colour}`; commands `transcript.import{words|srt}`, `transcript.edit_word{word,text}`, `transcript.set_speaker{words,speaker}`, `transcript.rename_speaker`, `transcript.delete`; queries `transcript.get{id,range,fields,cursor}`, `transcript.search{q,regex?,speaker?,context}`, `transcript.list`; schema migration; MCP results mark transcript text **untrusted**.
- **Interfaces:** `cutweave_model::transcript`; MCP core `transcript_get`, `transcript_search`.
- **Acceptance criteria:** invariants enforced (monotonic non-overlapping words, ids stable); search 10k-word transcript < 10 ms; paging works with cursor; edit_word keeps timings; untrusted-label present in MCP output (test).
- **Tests:** proptest invariants/undo, insta snapshots, migration fixture.
- **Out of scope:** generation (3D.05), timeline mapping (3F.02).

### WP-3D.02 — Minimal AI-gateway slice in `cutweave-ai` (Stt only) — **E5A foundation**
- **Area:** crates/cutweave-ai, crates/cutweave-jobs, crates/cutweave-ops
- **Depends on:** E1B, E1I, E2G (job queue)
- **Size:** M
- **Executor:** opus
- **Scope:** the **full closed `Capability` enum from architecture §13** (`#[non_exhaustive]`) but only `Stt` wired; normalised `SttRequest{audio:AudioRef,language?,diarize,word_timestamps,vocabulary?,keep_fillers}` → `Transcript`; `Provider` trait `{id,capabilities,estimate_cost,submit→JobHandle,poll,cancel}`; provider registry; `AiJobManager` on `cutweave-jobs` (SQLite `ai_jobs`, backoff polling, cancel, resume after restart, input-hash result cache); keys via `keyring` (`ai.key_set`, `ai.key_clear`, query `ai.key_status` — values never returned); cost estimate + `confirm_above_usd` setting with dry-run `confirm_token`; reqwest(rustls) client wrapper with host-injected auth and header/URL redaction in logs; `job.progress` events.
- **Interfaces:** `cutweave_ai::{Capability, Provider, JobHandle, SttRequest, CostEstimate}`; commands `ai.key_set`, `ai.key_clear`, `ai.providers_list`; MCP catalog.
- **Acceptance criteria:** mock-provider contract test; a job survives engine kill/restart and completes; key material never appears in logs, RPC payloads, project files or crash dumps (grep + redaction test); estimate above threshold without token → `Invalid{confirm_required}`; `cargo deny` clean.
- **Tests:** unit, wiremock integration, kill-restart e2e, secrets-leak test.
- **Out of scope:** catalog, other capabilities, budgets UI, WASM adapters (all Phase 5). Phase 5 **extends** this module — see Notes.

### WP-3D.03 — Cloud Stt provider: ElevenLabs Scribe adapter
- **Area:** crates/cutweave-ai (`providers/elevenlabs_stt`)
- **Depends on:** WP-3D.02, WP-3D.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** upload (multipart) of prepared audio, Scribe batch model, word-level timestamps, diarization, audio-event tags, language auto-detect, custom vocabulary/keyterms where supported, retries/backoff/rate-limit handling; normalise into `Transcript` (filler words retained when the API returns them); cost estimate from duration; recorded fixtures (≥ 5: clean speech, 2 speakers, multilingual, noisy, error cases). **Verify field names and pricing against current API docs at implementation time** (research marked them unverified).
- **Interfaces:** provider id `elevenlabs.scribe`.
- **Acceptance criteria:** fixtures normalise to expected transcripts (word count, monotonic times); invalid key → typed error without retry; 429 → backoff; opt-in live smoke test (`CUTWEAVE_LIVE_ELEVENLABS_KEY`) passes on a 30 s sample with word-time error ≤ 150 ms vs reference.
- **Tests:** wiremock fixtures, live smoke (opt-in).
- **Out of scope:** realtime/streaming STT, other cloud providers (Deepgram/AssemblyAI are Phase 5).

### WP-3D.04 — Local Stt provider: whisper.cpp + model manager (minimal)
- **Area:** crates/cutweave-stt-whisper (new), crates/cutweave-ai, crates/cutweave-ops
- **Depends on:** WP-3D.02, WP-3D.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** `whisper-rs` provider behind cargo feature `local-stt` (CPU default; Vulkan/Metal/CUDA features per OS build); models tiny/base/small (q5) and large-v3-turbo (q5_0) downloadable with SHA-256 verify, resume, licence text shown (MIT), disk path in app data; whisper.cpp built-in VAD; word timestamps (DTW/token timestamps); progress callbacks and cancel < 2 s; optional filler-friendly prompt; commands `ai.model_download{id}`, `ai.model_remove{id}`, query `ai.models_local`.
- **Interfaces:** provider id `local.whisper`; registered as a built-in native provider (becomes a plugin in Phase 5/6 — see Notes).
- **Acceptance criteria:** 3-min fixture transcribes with WER ≤ 12% (base, English) and mean absolute word-start error ≤ 120 ms vs reference alignment; CPU run completes in ≤ 3× realtime on CI Linux runner using `base`; resumed download passes hash; works fully offline once the model is present.
- **Tests:** unit, WER harness on the corpus, download-resume test with local server.
- **Out of scope:** ONNX models, Parakeet, streaming, the full model manager UI (Phase 5I).

### WP-3D.05 — `transcript.generate`: audio prep, chunking, normalisation, job command
- **Area:** crates/cutweave-ops, crates/cutweave-media, crates/cutweave-ai
- **Depends on:** WP-3D.01, WP-3D.02, WP-3D.03, WP-3D.04, E1D
- **Size:** M
- **Executor:** sonnet
- **Scope:** command `transcript.generate{media|item|range, provider?, model?, language?|auto, diarize?, keep_fillers?, vocabulary?, dry_run?}` → job; audio prep (mono 16 kHz FLAC/Opus extraction; chunk > 10 min into windows with 2 s overlap and de-duplicated stitching; time-map restore); post-normaliser (monotonic, no overlaps, punctuation merge, CJK split, speaker id map, language tag); provider auto-pick (cloud if key set and online, else local, else error with guidance); result applied by one `transcript.attach` transaction; MCP `transcript_generate` returns `job_id` and agents wait with `job_wait{job_id, timeout_ms}` (`media_import` already returned `media_ids` synchronously; analysis is async); cache keyed `(audio hash, provider, model, opts)`; progress events; paid-provider dry-run → `confirm_token`.
- **Interfaces:** MCP core `transcript_generate`; events `job.progress`.
- **Acceptance criteria:** e2e with mock provider produces a transcript in the project; stitching test: no duplicate/missing words at chunk boundaries on a 25-min synthetic; killing the engine mid-job resumes; second identical call is a cache hit costing 0; cancelling stops provider polling.
- **Tests:** unit, e2e, stitching property test.
- **Out of scope:** translation (Phase 5), live captions.

### WP-3D.06 — Transcript panel UI & AI-provider settings
- **Area:** packages/editor
- **Depends on:** WP-3D.01, WP-3D.05, E2H
- **Size:** M
- **Executor:** sonnet
- **Scope:** Transcript panel: virtualised word list grouped by speaker, click word → seek, playhead-follow highlight, search with next/prev, low-confidence underline, inline word edit, speaker rename/colour; "Generate transcript" dialog (provider, language, diarize, cost estimate, progress, cancel); Settings > AI providers: masked key entry via `ai.set_key` (write-only, arch §13), status badge, local model list with download/remove/progress/licence.
  - This WP creates the AI providers page in WP-1M.11's settings frame (keys and local models; WP-5F.07 extends it) and adds Settings > Media and import: "Transcribe speech in new media" (`media.transcribe_on_import`, off) (catalog `docs/reference/settings.md`).
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** a transcript menu on words and selections (`transcript.range`): Seek here, Copy, Add marker, Delete words; the timeline editing items are WP-3F.03's.
- **Interfaces:** consumes `transcript.*`, `ai.*`; dockview panel registration.
- **Acceptance criteria:** Playwright e2e with mock STT: generate → words appear → click seeks within one frame → edit word → undo; 10k-word transcript scrolls ≥ 55 fps; key never shown after save; keyboard accessible.
- **Tests:** Vitest, Playwright-Electron, visual regression.
- **Out of scope:** text-based editing operations (3F.03).

---

# Epic 3E — Captions & style gallery

### WP-3E.01 — Caption model, time mapping & commands
- **Area:** crates/cutweave-model, crates/cutweave-ops
- **Depends on:** WP-3D.01, WP-3E.02, E2A, E2D
- **Size:** M
- **Executor:** sonnet
- **Scope:** `caption` track kind; `Item::Caption{transcript, word_range|text, style: CaptionStyleRef, layout{position,max_width,align,safe_zone}, emphasis[], locked}`; track-level defaults; `cutweave_ops::timemap::{source_to_timeline, timeline_to_source}` (handles trim, speed, reverse; reused by 3F.02); commands `caption.generate{sequence|track,transcript,style?,chunking?}` (one tx), `caption.regenerate{track}` (keeps `locked` blocks), `caption.edit_text`, `caption.split`, `caption.merge`, `caption.set_style{caption|track,style,overrides}`, `caption.set_layout`, `caption.delete`; `caption.resync{track}` re-anchors blocks after clip edits (see Notes).
- **Interfaces:** MCP core `caption_generate`, `caption_style`, `caption_edit`.
- **Acceptance criteria:** generating on a 3-min clip with a 0.5× speed segment and a trim places every block within 1 frame of the spoken word (verified with synthetic-TTS fixture of known timings); edit_text keeps timings and, on 1:1 word match, writes back to the transcript; undo/redo; blocks never overlap on one track.
- **Tests:** proptest (no overlap, order, undo), timemap property tests, fixtures.
- **Out of scope:** rendering (3E.04), styles.

### WP-3E.02 — Caption chunking engine
- **Area:** crates/cutweave-text (`chunk`)
- **Depends on:** WP-3D.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** pure `chunk(words, rules) -> Vec<Block>`: max chars/line (defaults 32 for 16:9, 20 for 9:16), max lines (1–2), min/max duration (0.8 s / 4.0 s), max words per block, break on sentence punctuation, break on pause ≥ 350 ms, avoid orphan single-word lines, balanced line breaks, speaker-change break, CJK per-character rules + kinsoku, RTL; presets `standard`, `short-form` (1–3 words/block), `karaoke-line`, `two-line`.
- **Interfaces:** `cutweave_text::chunk::{ChunkRules, chunk}`; exposed as `caption.generate{chunking}` args.
- **Acceptance criteria:** no word lost/duplicated/reordered (proptest); all hard constraints honoured on fixtures; deterministic; 10k words < 50 ms; wasm32 build.
- **Tests:** proptest, insta snapshots per preset × language fixture (en, es, de, ja, ar).
- **Out of scope:** translation, LLM-based segmentation.

### WP-3E.03 — Caption style package format & per-word context
- **Area:** crates/cutweave-text (`caption`), crates/cutweave-plugins, crates/cutweave-compose, packages/compose-runtime, schema/
- **Depends on:** WP-3B.01, WP-3B.05, WP-3C.01, WP-3E.01
- **Size:** M
- **Executor:** opus
- **Scope:** L0 `caption-style.toml` + `style.json` (base text style, position, background, **native highlight modes**: `color`, `scale`, `pill`, `underline`, `marker`, `glow`, `sweep(karaoke)`, `reveal`, plus palette variants, safe-zone defaults) with optional L1.5 composition entry (`kind="caption-style"`); host-computed per-word context injected into compositions: `ctx.caption = {words[{text,start,end,index,progress,state:'past'|'active'|'future'}], block{start,end,lines}, activeIndex, wordProgress, emphasis[]}`; `@cutweave/compose` `defineCaptionStyle`; override params → JSON Schema → inspector; `cutweave caption-style check`.
- **Interfaces:** `schema/caption-style.json`; `cutweave_text::caption::{CaptionStyle, WordState}`; query `caption.styles.list{query,tags}`.
- **Acceptance criteria:** a data-only style and a composition style both render; ≥ 60% of the 28 shipped styles must be expressible data-only (fast path, no JS per frame — see Notes); validator rejects missing fonts, bad schema, oversize; context deterministic and identical in preview/export.
- **Tests:** schema snapshot, unit for state computation, golden.
- **Out of scope:** the styles themselves (3E.06–3E.09), renderer optimisations (3E.04).

### WP-3E.04 — Caption renderer (word states, karaoke, pop/bounce, backgrounds)
- **Area:** crates/cutweave-render (`caption_pass`), crates/cutweave-text
- **Depends on:** WP-3E.03, WP-3A.05, WP-3A.06
- **Size:** M
- **Executor:** opus
- **Scope:** layout cache per block; per-glyph instance attributes for word state (colour lerp, karaoke horizontal sweep mask by word progress, spring pop/overshoot, bounce, shake); active-word pill/underline/marker backgrounds; only active blocks evaluated; 200+ blocks on a track cost nothing when off-screen; emphasis emoji sprite rendered above the word with pop-in; `render.frame` metadata lists `captions_visible[{text,style,active_word}]`.
- **Interfaces:** `cutweave_render::caption::draw`; `Caption` items render natively.
- **Acceptance criteria:** caption pass ≤ 0.5 ms GPU + 0.3 ms CPU per frame at 1080p on the iGPU reference (2 lines, animated); §19 scenario (3 video layers + 2 effects + animated captions, 1080p60 iGPU) 0 dropped frames over 60 s; GF for each highlight mode; preview == export.
- **Tests:** golden, nightly GPU bench, playback soak.
- **Out of scope:** style content.

### WP-3E.05 — Emoji emphasis (keyword → emoji)
- **Area:** crates/cutweave-text, plugins/caption-data, crates/cutweave-ops
- **Depends on:** WP-3E.01, WP-3E.04
- **Size:** S
- **Executor:** sonnet
- **Scope:** bundled keyword→emoji dictionary (en ≥ 600 entries, es/fr/de/pt ≥ 200 each; derived from Unicode CLDR annotations, licence recorded); query `caption.suggest_emphasis{track,density}` (pure function; deterministic; max density param); command `caption.set_emphasis{caption,word_idx,emoji|null}`; user may edit; hook for an LLM suggester later.
- **Interfaces:** `Caption.emphasis[]`; MCP catalog.
- **Acceptance criteria:** suggestions never exceed `density` (default 1 per 8 words) nor repeat the same emoji within 3 blocks; every dictionary entry is a valid emoji (unicode-data test); GF with emphasis.
- **Tests:** unit, golden.
- **Out of scope:** AI suggestions, sticker-style emphasis.

### WP-3E.06 — Caption styles batch A: active-word highlight ×7
- **Area:** plugins/caption-styles
- **Depends on:** WP-3E.04, WP-3A.02
- **Size:** S
- **Executor:** sonnet
- **Scope:** (data-only styles) **Pop Yellow** (white bold + black outline, active word yellow with 1.15× pop), **Clean Highlight** (active word on accent pill), **Underline Track** (animated underline follows active word), **Marker Highlight** (highlighter swipe behind active word), **Contrast Flip** (active word inverted black-on-white box), **Gradient Active** (active word magenta→orange gradient), **Soft Glow Active** (active word glows).
- **Interfaces:** ids `cutweave.cap.<slug>`.
- **Acceptance criteria:** QC-CAP for all 7.
- **Tests:** GF 3 words × 2 aspect ratios per style; `caption-style check` in CI.
- **Out of scope:** karaoke/pop motion.

### WP-3E.07 — Caption styles batch B: karaoke & sweep ×7
- **Area:** plugins/caption-styles
- **Depends on:** WP-3E.04, WP-3A.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** **Karaoke Sweep** (colour fill sweeps across active word by progress), **Karaoke Line** (sweep across whole line), **Karaoke Bounce** (sweep + word bounce), **Lyric Fade** (past words dim, future words faint), **Lyric Stack** (centred 3-line stack, active line enlarged), **Neon Sweep** (glow sweep), **Typewriter Reveal** (words appear progressively, no pre-display of future words).
- **Interfaces:** ids `cutweave.cap.<slug>`.
- **Acceptance criteria:** QC-CAP for all 7; sweep progress is exactly 0 at word start and 1 at word end (numeric test).
- **Tests:** GF, numeric sweep test.
- **Out of scope:** pop-style motion.

### WP-3E.08 — Caption styles batch C: pop, bounce & kinetic ×7
- **Area:** plugins/caption-styles
- **Depends on:** WP-3E.04, WP-3C.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** **Word Pop** (one word at a time scales in), **Bounce Words** (spring bounce per word), **Slam Impact** (big words slam with micro-shake), **Zoom Punch** (quick zoom-in per block), **Rotate Pop** (seeded ±4° tilt), **Stack Build** (words stack into lines), **Shake Emphasis** (shake on emphasis-flagged words).
- **Interfaces:** ids `cutweave.cap.<slug>`.
- **Acceptance criteria:** QC-CAP for all 7; seeded randomness identical across runs/OS; motion settles (final-frame offset ≤ 0.5 px) before block ends when block ≥ 0.6 s.
- **Tests:** GF, determinism, settle test.
- **Out of scope:** emoji sprites.

### WP-3E.09 — Caption styles batch D: boxes, cinematic, creator ×7
- **Area:** plugins/caption-styles
- **Depends on:** WP-3E.04, WP-3E.05, WP-3A.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** **Subtitle Classic** (white, soft shadow, bottom), **Boxed Subtitle** (semi-transparent black box), **Cinematic Serif** (letter-spaced serif, lower third), **Minimal Lowercase**, **Comic Burst** (comic font, thick outline, colour-per-speaker), **Retro Neon**, **Emoji Hype** (emphasis emoji sprites + pop).
- **Interfaces:** ids `cutweave.cap.<slug>`.
- **Acceptance criteria:** QC-CAP for all 7; speaker colours work with diarized transcripts; 28 styles total across A–D (count test in CI).
- **Tests:** GF, count test.
- **Out of scope:** bilingual variants (3E.13).

### WP-3E.10 — Caption style gallery design review (human gate)
- **Area:** docs/, plugins/caption-styles/REVIEW.md
- **Depends on:** WP-3E.06, WP-3E.07, WP-3E.08, WP-3E.09
- **Size:** S
- **Executor:** human
- **Scope:** reviewer renders the contact sheet of all 28 styles on 3 reference clips (9:16 talking head, 16:9 wide, busy background) via `cutweave caption-style sheet`; scores legibility, motion taste, variety (≥ 5 distinct "vibes"), naming; produces keep/fix/cut list as issues.
- **Interfaces:** `REVIEW.md` sign-off block (date, reviewer, per-style verdict).
- **Acceptance criteria:** ≥ 25 styles signed off "keep"; zero style with legibility fail; fixes filed as issues labelled `phase:P3 epic:3E`.
- **Tests:** n/a (review record).
- **Out of scope:** new styles beyond the 28 (backlog).

### WP-3E.11 — Captions panel UI
- **Area:** packages/editor
- **Depends on:** WP-3E.01, WP-3E.03, WP-3E.04, WP-3D.06, E2H
- **Size:** M
- **Executor:** sonnet
- **Scope:** Captions tab: generate dialog (source clip/range, language, provider, chunking preset incl. "short-form", cost estimate); style gallery grid with animated thumbnails (pre-rendered webp, on-demand fallback), search/tags/vibe filter, apply-to-all / apply-to-selected; caption list with inline text editing; style-override inspector (font, size, colours, highlight colour, position drag on preview with safe-zone guides); "Re-chunk" and "Resync" buttons; hooks for bilingual toggle.
  - Settings > Editing: characters per line and lines per caption for new captions (32, 2) and the caption style for new captions (`editing.caption_lines`, `editing.caption_style`); the generate dialog starts from them (catalog `docs/reference/settings.md`).
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** a caption row menu (`caption.row`): Split here, Merge with next, Style…, Copy text, Delete; style cards: Apply, Set as default.
- **Interfaces:** consumes `caption.*`, `transcript.generate`.
- **Acceptance criteria:** Playwright e2e (mock STT): generate → pick style → change highlight colour → drag position → export a frame matching GF; gallery of 28 loads thumbnails < 1 s warm; every action undoable in one step.
- **Tests:** Playwright-Electron, Vitest, visual regression.
- **Out of scope:** subtitle file import/export UI beyond a menu entry (3E.12).

### WP-3E.12 — SRT / VTT / ASS import & export
- **Area:** crates/cutweave-interchange (`subs`), crates/cutweave-ops
- **Depends on:** WP-3E.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** parsers for SRT, WebVTT (cue settings, basic tags), ASS/SSA (styles, override tags subset `\b \i \c \pos \an \fs`, `\k` karaoke → word timings); writers for SRT/VTT/ASS (ASS style mapped from caption style approximations: font, size, primary/outline colour, outline, shadow, alignment; `\k` emitted from word timings); encoding detection (UTF-8/16, BOM, Latin-1); commands `caption.import{path,track?,language}` (creates a synthetic transcript if none), `caption.export{track,format,path,mode:lines|words}`; MCP catalog.
- **Interfaces:** `cutweave_interchange::subs::{parse_srt, parse_vtt, parse_ass, write_*}`.
- **Acceptance criteria:** round-trip SRT→caption→SRT byte-identical for the 20-file corpus (normalised newlines); ASS karaoke import yields word timings within 10 ms; cargo-fuzz 1 h clean on all 3 parsers; export of 1 h captions < 200 ms.
- **Tests:** golden files, fuzz targets, round-trip property tests.
- **Out of scope:** LRC, full ASS drawing/animation tags.

### WP-3E.13 — Export integration: burn-in vs sidecar, bilingual layout, safe zones
- **Area:** crates/cutweave-ops, crates/cutweave-render, crates/cutweave-engine, data/platform-specs.json
- **Depends on:** WP-3E.04, WP-3E.12, E2G
- **Size:** M
- **Executor:** sonnet
- **Scope:** export setting `captions:{mode: burn|sidecar|both|none, format, per_track}` (sidecar named `name.<lang>.srt`); burn-in uses the same render path as preview; **bilingual**: `caption.set_bilingual{primary_track,secondary_track,layout:stacked|secondary_smaller|secondary_italic}` (secondary comes from an imported SRT/transcript — translation capability arrives in Phase 5); platform safe-zone presets (YouTube, Shorts, Reels, TikTok) in `data/platform-specs.json`, `caption.set_layout{safe_zone}` auto-clamps; preview overlay data for UI guides.
- **Interfaces:** export options schema; MCP `export_start{captions}`; `caption.set_bilingual`.
- **Acceptance criteria:** burned-in export frame equals preview frame (DoD-GF); sidecar file passes round-trip; bilingual blocks never overlap and share the safe zone; clamp keeps all glyphs inside the zone (property test).
- **Tests:** golden, e2e export test, proptest clamp.
- **Out of scope:** automatic translation, soft-sub muxing into MP4 (sidecar only).

---

# Epic 3F — Text-based editing

### WP-3F.01 — `timeline.ripple_delete_ranges` and `timeline.reorder`
- **Area:** crates/cutweave-ops (timeline)
- **Depends on:** E2A, E2E
- **Size:** M
- **Executor:** opus
- **Scope:** `timeline.ripple_delete_ranges{sequence,ranges,tracks:all|[ids],linked:true,crossfade_ms:8,dry_run}`: splits and removes ranges across all unlocked tracks, ripples later content, shifts markers, shrinks/splits/removes captions inside ranges, preserves/clips keyframes, respects linked groups and magnetic track, returns `Locked{items}` when blocked; `timeline.reorder{segments,to}` (split at segment bounds + ripple move); micro-crossfade at cut points (audio click-free); one transaction; `dry_run` returns a summary (duration delta, items touched). *(If E2A already ships equivalents, this WP shrinks to the missing parts — see Notes.)*
- **Interfaces:** commands above (MCP catalog); used by 3F.02/3F.04.
- **Acceptance criteria:** `new_duration == old_duration − Σ(ranges ∩ track extents)` (property test); A/V sync unchanged for linked clips (≤ 1 sample); no overlaps; undo identity; cut points free of clicks (max sample jump at splice ≤ −40 dBFS with 8 ms crossfade on tone fixture); 500 ranges applied in < 150 ms.
- **Tests:** proptest, fixtures with captions/keyframes/markers, audio click test.
- **Out of scope:** UI, transcript mapping.

### WP-3F.02 — Transcript ↔ timeline projection & word-edit commands
- **Area:** crates/cutweave-ops
- **Depends on:** WP-3E.01, WP-3F.01, WP-3D.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** query `transcript.sequence_view{sequence,track?}` → words mapped via `timemap` to `{item,timeline_start,timeline_end,present}` (handles speed, trims, splits, repeated source use); reverse `timeline_to_words`; cached by rev; commands `transcript.delete_words{sequence,words}` (merge adjacent, cut at word edges padded by min(gap/2, 60 ms), → `ripple_delete_ranges`) and `transcript.move_words{words,before_word}` (→ `reorder`); captions auto-resync (via 3E.01).
- **Interfaces:** MCP core `transcript_delete_words`, catalog `transcript_move_words`, query `transcript_sequence_view`.
- **Acceptance criteria:** on a synthetic-TTS fixture with known word timings, deleting 5 words reduces duration by their span (± 1 frame) and re-transcribing the output (mock oracle) lacks exactly those words; moving a sentence preserves A/V sync; `sequence_view` for a 1 h sequence < 50 ms.
- **Tests:** unit, e2e on fixtures, proptest (mapping inverse consistency).
- **Out of scope:** UI.

### WP-3F.03 — Transcript editing UI
- **Area:** packages/editor
- **Depends on:** WP-3F.02, WP-3D.06
- **Size:** M
- **Executor:** sonnet
- **Scope:** in the Transcript panel: select words (click/drag/shift/sentence), Delete/Backspace deletes, drag-reorder words/sentences, cut/paste, deleted words shown as struck-through "ghosts" with restore (via undo), present/removed styling from `sequence_view`, speaker-block operations, "Delete sentence" shortcut, keyboard navigation; multi-clip sequences.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** transcript editing items on `transcript.range`: Cut from timeline, Delete and close gap, Keep only this range.
- **Interfaces:** consumes `transcript.sequence_view/delete_words/move_words`.
- **Acceptance criteria:** Playwright e2e: delete 3 sentences → timeline duration drops by the matching amount and playback skips the gap seamlessly; undo restores; panel updates < 100 ms after edit.
- **Tests:** Playwright-Electron, Vitest.
- **Out of scope:** silence/filler UI (3F.05).

### WP-3F.04 — Silence & filler detection and removal commands
- **Area:** crates/cutweave-ops (`analysis`), crates/cutweave-audio, data/fillers/
- **Depends on:** WP-3F.01, WP-3D.01, E1D
- **Size:** M
- **Executor:** sonnet
- **Scope:** `analysis.detect_silence{target,threshold_db(default adaptive ≈ −35),min_duration_ms:400,pad_ms:100,use_transcript_gaps}` (10 ms RMS windows + hysteresis, refined by word timings when a transcript exists); `analysis.detect_fillers{transcript,lexicon,languages}` with lexicons en/es/fr/de/pt (`hard`: um/uh/er/euh/äh…, default on; `soft`: like/you know/so…, default off) and stutter/repeat detection; `timeline.remove_silences{sequence,ranges|params,dry_run}` and `timeline.remove_fillers{…}` (dry_run returns candidates + time saved; apply via `ripple_delete_ranges`, one undo step).
- **Interfaces:** MCP core `silence_remove`, `filler_remove` (dry_run → apply).
- **Acceptance criteria:** on the 6-clip labelled fixture set: silence recall ≥ 95% and precision ≥ 95% at default params; hard-filler recall ≥ 80% when the transcript retains fillers; never cuts inside a word (property test against word times); applying removes ≥ the reported time ± 1 frame; analysis of 1 h audio < 5 s.
- **Tests:** labelled-fixture benchmark in CI, proptest, unit.
- **Out of scope:** ML VAD (Phase 5), retake detection.

### WP-3F.05 — Clean-up review UI (silences & fillers)
- **Area:** packages/editor, packages/timeline
- **Depends on:** WP-3F.04, WP-3F.03
- **Size:** M
- **Executor:** sonnet
- **Scope:** "Clean up" panel with Silences/Fillers tabs: threshold/min-duration/padding sliders (debounced live recompute via dry-run), candidate list with checkboxes, timestamp, duration, transcript context, audition (play ±1 s with the cut applied), total time saved, "Apply selected" as a single undo step, "Remove all"; timeline overlay shades candidate ranges on the waveform.
- **Interfaces:** consumes `analysis.*`, `timeline.remove_*`.
- **Acceptance criteria:** Playwright e2e on the fixture: adjust threshold → list updates < 300 ms; uncheck 2 candidates → they remain; apply → duration delta equals the displayed saving ± 1 frame; one undo restores.
- **Tests:** Playwright-Electron, Vitest.
- **Out of scope:** auto-apply on import.

---

# Epic 3G — Asset packs

### WP-3G.01 — L0 asset pack loader, library backend & licence CI
- **Area:** crates/cutweave-plugins (`packs`), crates/cutweave-ops, tools/check-pack-licenses
- **Depends on:** WP-3B.01, E1D
- **Size:** M
- **Executor:** sonnet
- **Scope:** `pack.toml` kinds `sticker|overlay|music|sfx|emoji|font|lut`; per-item metadata (id, name, tags, SPDX licence, author, source URL, attribution_required, duration, bpm, mood, sha256); bundled starter content ≤ 40 MB + on-demand packs from a `pack-index.json` (GitHub release assets) with SHA-256 verify and resume; queries `asset.search{kind,q,tags,licence}`, `asset.get`; commands `asset.import_pack{path|id}`, `asset.add_to_timeline{asset,track,at}` (imports into media bin with provenance `origin: import`, pack id); CI tool rejects missing/NC/ND/unknown licences (allowlist CC0-1.0, CC-BY-4.0, MIT, OFL-1.1, public-domain) and generates the attribution file for CC-BY items.
- **Interfaces:** namespace `asset` (new — see Notes); MCP catalog `asset_search` (core), others via `cutweave_execute`.
- **Acceptance criteria:** licence CI fails on a seeded bad pack; interrupted download resumes with correct hash; adding a music asset creates a media item at the playhead; attribution file lists every CC-BY item used in the project.
- **Tests:** unit, CI-tool tests, download-resume with local server.
- **Out of scope:** stock-photo/video plugins, marketplace.

### WP-3G.02 — Sticker, overlay & emoji support in the engine
- **Area:** crates/cutweave-media, crates/cutweave-render, crates/cutweave-ops
- **Depends on:** WP-3G.01, WP-3A.06, E2C
- **Size:** S
- **Executor:** sonnet
- **Scope:** decode animated WebP, APNG, GIF, PNG sequences and alpha video (VP9-alpha WebM, ProRes 4444, HEVC-alpha) with premultiplied alpha; looping stickers (loop count/ping-pong); overlay defaults (`screen`/`add` blend for light overlays); `text.add_emoji{emoji,at}` (colour emoji text item); emoji search data from Unicode CLDR names/keywords; pack-level default duration.
- **Interfaces:** `asset.add_to_timeline` handles stickers; query `emoji.search` (namespace flagged in Notes → `asset.search{kind:emoji}`).
- **Acceptance criteria:** animated-WebP frame N matches the stored per-adapter reference (SSIM ≥ 0.995 AND PSNR ≥ 40 dB); alpha edges show no dark fringe (test on white/black backgrounds, max ΔE ≤ 2); loop wraps seamlessly; GF.
- **Tests:** decode golden, render golden.
- **Out of scope:** Lottie, GIPHY integration.

### WP-3G.03 — Sticker & overlay content (original artwork), ≥ 60 items
- **Area:** plugins/assets-stickers, tools/rasterize
- **Depends on:** WP-3G.01, WP-3G.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** original SVG sources committed under `src/`, rasterised by `tools/rasterize` (resvg) to PNG/animated APNG, licence CC0-1.0. *Shapes & frames (22):* 6 arrows, 3 circles/ellipses, 3 scribble underlines, star burst, 4 speech bubbles, 3 frames/borders, check, cross. *Social/reaction (26):* like, heart, fire, sparkle, bell, generic subscribe button, notification dot, comment bubble, share arrow, 100 badge, NEW badge, trending tag, LIVE badge, thumbs-down, clap, lightning, crown, star rating (5), laugh, wow, sad, mind-blown, eye-roll, question, exclamation, pointing hand. *Overlays (12):* film border, soft vignette frame, dust, viewfinder REC frame, focus brackets, rounded phone frame, polaroid-style frame (generic), torn-paper edge, halftone corner, snow, rain, confetti.
- **Interfaces:** pack ids `cutweave.pack.stickers-*`.
- **Acceptance criteria:** all items original (no third-party artwork; reviewer checklist), ≤ 512 px longest side (overlays 1080p), tags ≥ 3, licence CC0 recorded; licence CI green; pack ≤ 12 MB.
- **Tests:** CI licence/lint, thumbnail render check.
- **Out of scope:** AI-generated stickers (Phase 5).

### WP-3G.04 — Music & SFX starter library (curation, human-reviewed)
- **Area:** plugins/assets-audio, tools/curate-audio
- **Depends on:** WP-3G.01
- **Size:** M
- **Executor:** human
- **Scope:** agent-built tooling (`tools/curate-audio`: fetch lists, trim silence, normalise **music to −16 LUFS ± 1, SFX to −3 dBFS peak**, encode Opus 128 kbps, emit metadata incl. bpm/mood) run by a human who selects and licence-checks each item from CC0 sources only (Free Music Archive CC0, Freesound CC0, OpenGameArt CC0, Internet Archive public-domain); targets: **30 music tracks** (5 each: upbeat, chill, cinematic, corporate, energetic, emotional; 60–180 s, loopable flagged) and **120 SFX** (whoosh ×10, impact ×10, riser ×6, UI click ×10, pop ×8, glitch ×8, shutter/camera ×4, applause ×4, notification ×8, typing ×4, ambience ×8, transitions ×10, comedy-neutral stingers ×10, misc ×20). Excluded: Pixabay/Mixkit licences (not redistributable in an OSS pack).
- **Interfaces:** packs `cutweave.pack.music-starter`, `cutweave.pack.sfx-starter`.
- **Acceptance criteria:** 100% items have SPDX licence + source URL + human sign-off record in `plugins/assets-audio/CURATION.md`; licence CI green; bundled starter (subset) ≤ 25 MB, rest downloadable; loudness within ±1 LU (script check).
- **Tests:** CI loudness + licence checks.
- **Out of scope:** AI music generation (Phase 5), copyright-fingerprint checks.

### WP-3G.05 — Library UI: Audio, Stickers, Emoji tabs
- **Area:** packages/editor
- **Depends on:** WP-3G.01, WP-3G.02, E2H
- **Size:** M
- **Executor:** sonnet
- **Scope:** Audio tab (Music/SFX: search, mood/genre filters, BPM, waveform preview, hover/click audition on a preview bus without moving the playhead, favourites, drag or "+" to timeline at the playhead with beat-snap when BPM known); Stickers tab (grid, animated hover, drag to preview); Emoji tab (search, recents); download-on-demand states; Attribution panel ("copy credits" for CC-BY items).
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** library card menus (`library.card`): Preview, Add to timeline, Favourite, Reveal in Explorer (user packs).
- **Interfaces:** consumes `asset.*`, `text.add_emoji`.
- **Acceptance criteria:** Playwright e2e: search "whoosh" → audition plays within 150 ms → add to timeline → clip exists at playhead; offline mode shows installed packs only; credits text lists exactly the CC-BY items on the timeline.
- **Tests:** Playwright-Electron, Vitest.
- **Out of scope:** stock video/photo browsing.

---

# Epic 3H — Agent Kit v0

### WP-3H.01 — MCP tools for captions & transcripts
- **Area:** crates/cutweave-api (MCP), crates/cutweave-ops
- **Depends on:** WP-3D.05, WP-3E.01, WP-3F.02, WP-3F.04, WP-3G.01, E2I
- **Size:** M
- **Executor:** sonnet
- **Scope:** core tools (profile `standard`; `minimal` stays at its ≤ 12-tool budget and gets none of these beyond `job_wait`; profiles are per endpoint `/mcp`, `/mcp/minimal`, `/mcp/full`, never per connection): `transcript_generate`, `transcript_get`, `transcript_search`, `transcript_delete_words`, `caption_generate`, `caption_style` (list/apply/override; returns compact id/name/vibe + up to 3 preview image contents), `caption_edit`, `silence_remove`, `filler_remove`, `asset_search`; ≤ 10 tools added to `tools/list`; correct annotations (readOnly/destructive/idempotent); `untrusted()` content-labelling helper in `cutweave-api` for transcript/caption text; paid calls require `confirm_token` from dry-run; tool descriptions tuned with example args.
- **Interfaces:** the tools above; `tools/list` token-budget test.
- **Acceptance criteria:** the `standard` profile stays within the single budget (≤ 36 tools / ≤ 8k tokens) with these tools added, `minimal` ≤ 12 tools / ≤ 3k tokens (CI budget check); change notifications use the negotiated MCP revision's mechanism (2025-11-25 `resources/subscribe`, 2026-07-28 `subscriptions/listen`); MCP Inspector passes on stdio + HTTP; every new command reachable (parity test); transcript text in results carries the untrusted marker (test).
- **Tests:** MCP conformance, parity, scripted client tests.
- **Out of scope:** generation tools (Phase 5), copilot.

### WP-3H.02 — MCP resources & vision aids for captions
- **Area:** crates/cutweave-api
- **Depends on:** WP-3H.01, WP-3E.04
- **Size:** S
- **Executor:** sonnet
- **Scope:** resource `cutweave://transcript/{id}` (paged, summarised first, untrusted-labelled), change subscriptions via the negotiated revision's mechanism (`resources/subscribe` or `subscriptions/listen`); `render_frame`/`render_contact_sheet` structured metadata includes `captions_visible[{text,style,active_word}]` and `text_items_visible`; `render_contact_sheet{with_captions}`; resource templates for caption styles (`cutweave://caption-styles/{id}` with preview).
- **Interfaces:** MCP resources; extended `render_*` metadata schema.
- **Acceptance criteria:** an agent can verify "captions are on screen at t" from metadata alone (e2e assertion without image reading); resource pages ≤ 4k tokens each.
- **Tests:** MCP e2e, schema snapshots.
- **Out of scope:** MCP Apps.

### WP-3H.03 — Skills: `captions-and-styling` and `transcript-editing`
- **Area:** agent-kit/skills, agent-kit/evals
- **Depends on:** WP-3H.01
- **Size:** S
- **Executor:** sonnet
- **Scope:** spec-valid `SKILL.md` (frontmatter name/description ≤ 1024 chars, body < 500 lines) + `references/caption-style-guide.md` (style → vibe → platform mapping, chunking guidance); `transcript-editing` (query before edit, dry-run first, thresholds guidance, verify duration delta, never delete source media); each skill has an eval task spec (state assertions) under `agent-kit/evals/`.
- **Interfaces:** skills reference tools by MCP name and degrade gracefully when a tool is absent.
- **Acceptance criteria:** `skills-ref` validator passes in CI; eval tasks run against the mock STT fixture and pass state assertions; descriptions are distinct (no trigger overlap test: each skill's "use when" phrases don't appear in the other).
- **Tests:** skills validator, eval dry-run.
- **Out of scope:** other 12 skills (Phase 6G).

### WP-3H.04 — Skill `reframe-and-aspect` + 3 MCP prompts
- **Area:** agent-kit/skills, agent-kit/prompts, crates/cutweave-api
- **Depends on:** WP-3H.03, E2B
- **Size:** S
- **Executor:** sonnet
- **Scope:** skill for 9:16 conversion using `sequence.set_format`, manual reframe with transform scale/position keyframes (agent inspects frames via `render_frame`), blurred-background fill, caption safe zone; explicit non-goal: no AI tracking; MCP prompts `make-short`, `caption-pass`, `tighten` (templates in `agent-kit/prompts/*.md` served via MCP prompts API) that load the matching skill; skills also exposed as `cutweave://skills/{name}` resources.
- **Interfaces:** MCP prompts + resources.
- **Acceptance criteria:** prompts listed by Inspector with argument schemas; skill validator passes; eval task "16:9 → 9:16 with subject centred" passes state assertions (crop/transform keyframes present, sequence 1080×1920).
- **Tests:** MCP prompt tests, validator, eval.
- **Out of scope:** `cutweave agent-kit install` CLI (Phase 6G).

### WP-3H.05 — Scripted-agent e2e & LLM-in-the-loop run (exit criterion #2)
- **Area:** agent-kit/evals, tests/e2e-agent, docs/evals/p3.md
- **Depends on:** WP-3H.01, WP-3H.02, WP-3H.03, WP-3H.04, WP-3F.04, WP-3E.13, WP-3G.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** deterministic MCP client script performing "make this a captioned 9:16 short, remove silences, add upbeat music, export" through the tool sequence an LLM is expected to use (mock STT, fixture clip); assertions on final doc state + exported file (ffprobe: 1080×1920, duration, audio present); plus a documented manual run with Claude Code via `cutweave mcp --attach` and one other client from a one-line instruction, recorded in `docs/evals/p3.md` (tool calls, wall time, issues).
- **Interfaces:** `agent-kit/evals/p3-short-from-talking-head.yaml`.
- **Acceptance criteria:** scripted run passes in CI on all 3 OSes; manual run achieves the result with ≤ 40 tool calls, 0 screenshots, ≤ 10 min wall; failures produce actionable issues.
- **Tests:** the e2e itself; MCP Inspector.
- **Out of scope:** nightly eval infrastructure (Phase 6H).

---

# Epic 3I — Alpha packaging

### WP-3I.01 — Alpha installers for Windows, macOS, Linux
- **Area:** apps/desktop, .github/workflows/release-alpha.yml
- **Depends on:** E1L, WP-3A.02, WP-3G.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** electron-builder: Windows NSIS (x64, unsigned), macOS DMG (arm64, ad-hoc signed; documented Gatekeeper override), Linux AppImage + deb (x64); bundle engine binary, FFmpeg shared libs + source-offer file, bundled fonts and starter packs (whisper models **not** bundled); tag-driven workflow (`v0.N.0-alpha.K`) building all three, SBOM (CycloneDX), third-party notices stub (`cargo-about`).
- **Interfaces:** release artifacts; `docs/alpha/INSTALL.md`.
- **Acceptance criteria:** CI produces 3 artifacts from a tag; installer size ≤ 400 MB each; clean-VM install → launch → open sample project works on Win 11, macOS 14 arm64, Ubuntu 24.04; uninstall removes app files (keeps user data).
- **Tests:** workflow dry-run, install smoke on VMs (manual checklist recorded + automated in 3I.03).
- **Out of scope:** signing/notarisation, auto-update (Phase 7A).

### WP-3I.02 — First-run experience, sample content & alpha docs
- **Area:** packages/editor, apps/desktop, docs/alpha
- **Depends on:** WP-3D.06, WP-3E.11, WP-3I.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** first-run dialog (GPU adapter diagnostics with fallback message; choose STT: paste cloud key or download local model; telemetry-off statement; opt-in crash-reporting prompt, default off, see 3I.05); "Make a short" checklist overlay (import → transcribe → captions → clean up → music → export); "Alpha" banner + "Report issue" (opens prefilled GitHub issue; `cutweave doctor` zips logs without media; nothing sent automatically); sample project with a CC0 talking-head clip (1 min bundled, 3 min downloadable); 9:16 default workspace; docs: `docs/alpha/README.md`, `KNOWN_ISSUES.md`, per-OS install notes.
  - The first-run choices are written to the preferences store (WP-1M.11) and stay changeable in Settings; Settings > General's "On startup" gains "Show the welcome screen".
- **Interfaces:** `cutweave doctor` CLI subcommand.
- **Acceptance criteria:** fresh profile reaches a captioned sample in ≤ 8 clicks (Playwright count); `cutweave doctor` bundle contains no media paths beyond file names (test); docs lint (links valid).
- **Tests:** Playwright-Electron, CLI test.
- **Out of scope:** onboarding video, usage analytics (crash reporting is separate, 3I.05).

### WP-3I.03 — Installer smoke tests & exit-criterion automation
- **Area:** .github/workflows, tests/e2e-alpha
- **Depends on:** WP-3I.01, WP-3H.05
- **Size:** M
- **Executor:** sonnet
- **Scope:** CI job on 3 OSes: silent install, launch, Playwright-Electron launch test, `cutweave render` of the bundled sample with captions via mock STT verified with ffprobe + golden frame; scripted engine-only pipeline benchmark (mock transcribe → caption → silence cut → music → export 3-min 9:16 1080p) logging per-step time on the reference machine.
- **Interfaces:** CI workflow `alpha-smoke.yml`; benchmark JSON artifact.
- **Acceptance criteria:** green on all 3 OSes; engine-only pipeline ≤ 90 s on reference hardware excluding STT network time (leaves ≥ 3.5 min for human steps in the 5-minute budget); regression > 20% fails the job.
- **Tests:** the job itself.
- **Out of scope:** UI timing (3I.04).

### WP-3I.04 — Usability gate: < 5-minute short from a 3-min talking-head clip (human)
- **Area:** docs/evals/p3-usability.md
- **Depends on:** WP-3I.02, WP-3I.03, WP-3E.10, WP-3G.05, WP-3F.05
- **Size:** S
- **Executor:** human
- **Scope:** 5 first-time users (non-developers) on the reference machine with a provided cloud STT key; script: "turn this 3-min clip into a styled, captioned, silence-cut 9:16 short with music and export"; stopwatch from app launch (after install) to exported file; record friction points; file issues.
- **Interfaces:** results table in `docs/evals/p3-usability.md`.
- **Acceptance criteria:** median < 5:00 and ≥ 4 of 5 users under 5:00; exported file plays with captions burned in; blockers filed as `phase:P3` issues; M3 exit sign-off recorded.
- **Tests:** n/a.
- **Out of scope:** local-STT timing (reported separately, not gating).

### WP-3I.05 — Opt-in, self-hosted, privacy-preserving crash reporting (ships with Public Alpha)
- **Area:** crates/cutweave-engine (crash handler), apps/desktop, packages/editor (consent UI), infra/crash-collector/, docs/privacy/crash-reporting.md
- **Depends on:** E1L, WP-3I.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Moved earlier from P7 7B.01 so the Public Alpha builds produce the crash data that the 1.0 crash-free gate (≥ 99.5%) is measured on. P7 7B keeps analysis, triage and hardening.
  - Engine and shell crash handlers (minidump via `crash-handler`/`minidumper` for Rust, Electron `crashReporter` for the shell) writing dumps locally; symbolication on the collector, never on the client. Reports contain: app/engine version, OS/arch, GPU adapter name, stack, anonymous random install id (rotatable), session length and a clean-exit flag (for crash-free-session stats). No media paths, project contents, file names, prompts, transcripts, API keys or IPs stored (collector drops source IP).
  - Strictly opt-in: first-run prompt (3I.02) and Settings toggle, default off; per-report preview of what would be sent with "send / don't send / always"; one-click delete of the install id's reports.
  - Self-hosted collector: a minimal open-source service (container + docs; Sentry-compatible-minidump or Crashpad-compatible endpoint) the project runs on its own infrastructure; no third-party SaaS; endpoint configurable so forks/enterprises can point elsewhere or disable at build time.
  - Redaction test harness shared with 5A.03 (secrets never appear in dumps or logs).
  - The switch lives on the Privacy page of Settings (WP-1M.11's frame), with a plain statement of what leaves this computer: crash reports when on, AI requests to the providers the person set up, update checks (catalog `docs/reference/settings.md`).
- **Interfaces:** setting `privacy.crash_reports: off|ask|always`; `cutweave doctor --crash-preview`; collector HTTP API (documented).
- **Acceptance criteria:** with the setting off, a forced crash sends zero network requests (test with a blocking proxy); with it on, a forced crash on each of Windows/macOS/Ubuntu yields a symbolicated report on the collector within 5 min; grep/redaction test finds no paths, usernames or key material in 20 sampled reports; clean-exit sessions are counted for crash-free-session stats; privacy doc reviewed by the maintainers.
- **Tests:** forced-crash integration on 3 OSes, network-blocking test, redaction test, collector container smoke.
- **Out of scope:** crash triage dashboards and hardening work (P7 7B), usage analytics, automatic bug filing.



---

## Traceability

### Roadmap scope bullets → WPs

| Roadmap bullet | WPs |
|---|---|
| Rich text: fonts (system + bundled open), fill, outline, shadow, glow, background box, gradient, letter/line spacing, alignment, auto-fit | 3A.01, 3A.02, 3A.03, 3A.04, 3A.05, 3A.06, 3A.07 |
| Text presets library | 3C.05 |
| Text animations (in/out/loop) built as compositions; per-glyph/word/line animators | 3C.01, 3C.02, 3C.03, 3C.04 |
| `@cutweave/compose` v1, QuickJS host, inspector from props schema | 3B.01, 3B.02, 3B.03, 3B.04, 3B.05 |
| First-party compositions: lower thirds, titles, callouts, progress bars, subscribe buttons | 3B.06, 3B.07 |
| Stt capability; first providers (one cloud + local whisper.cpp) | 3D.02, 3D.03, 3D.04, 3D.05 |
| Word-level transcript model, speaker labels, transcript panel with search | 3D.01, 3D.06 |
| Auto captions to a caption track with chunking rules | 3E.01, 3E.02 |
| Caption style gallery (≥ 25 styles): highlight, karaoke, pop/bounce, emoji emphasis | 3E.03, 3E.04, 3E.05, 3E.06, 3E.07, 3E.08, 3E.09, 3E.10, 3E.11 |
| SRT/VTT/ASS import/export, burn-in or sidecar, bilingual layout | 3E.12, 3E.13 |
| Text-based editing: delete/reorder transcript words | 3F.01, 3F.02, 3F.03 |
| Silence removal and filler-word removal with review UI | 3F.04, 3F.05 |
| Sticker/overlay packs (L0), emoji, music + SFX library (CC0) | 3G.01, 3G.02, 3G.03, 3G.04, 3G.05 |
| Agent Kit v0: MCP tools for captions/transcripts | 3H.01, 3H.02 |
| First 3 skills (`captions-and-styling`, `transcript-editing`, `reframe-and-aspect`) and MCP prompts | 3H.03, 3H.04 |

### Exit criteria → WPs

| Exit criterion | WPs that deliver / verify it |
|---|---|
| New user: 3-min talking-head → styled, captioned, silence-cut 9:16 short with music in < 5 min | 3D.01–3D.06, 3E.01–3E.13, 3F.01–3F.05, 3G.01, 3G.04, 3G.05, 3I.02, 3I.03, **3I.04 (gate)** |
| An agent does the same via MCP from a one-line instruction | 3H.01–3H.04, **3H.05 (gate)** |
| Installers for all 3 OSes (unsigned OK) | 3I.01, 3I.03 |
| (roadmap 1.0 gate input) Opt-in crash reporting ships with the Public Alpha | 3I.05 |
| Caption gallery ≥ 25 styles reviewed | 3E.06–3E.09, 3E.10 |

## Notes for integrator

1. **WP count is 58 (guideline 35–50; 3I.05 crash reporting added, moved earlier from P7 7B.01).** The overage is content batches (28 caption styles, 24 text animations, 30 presets, 28 compositions) and the human gates; engine/UI WPs alone are ≈ 40. If a cut is needed, merge 3C.03+3C.04 and 3B.06+3B.07.
2. **`cutweave-plugins` skeleton ownership.** P3 (3C, 3E, 3G) and P4 (4A.01 onward) both need a manifest + contribution registry before Phase 6's plugin platform. Resolution used here: **WP-3B.01 builds `cutweave-plugins` v0** (manifest v0, discovery, registry, L0 validators; no wasmtime/permissions). Phase 4's 4A.01 depends on it; Phase 6A must extend rather than replace it. Phase-4 author may start spec work in parallel but cannot merge loader code before 3B.01.
3. **AI gateway slice (WP-3D.02).** It declares the **full `Capability` enum** and provider trait now but only wires `Stt`; Phase 5's E5A must build on `cutweave-ai` + `cutweave-jobs` rather than create a second job manager. Built-in Rust adapters (ElevenLabs, whisper) are interim: Phase 5E should re-express ElevenLabs as a declarative HTTP manifest and 5I should repackage local whisper as the "Local AI plugin". The minimal model download manager in 3D.04 is also a subset of 5I's manager (same SQLite table and licence display).
3b. **Local whisper "fallback plugin" wording.** The roadmap calls it a plugin, but the WASM plugin host does not exist until Phase 6. Resolution: ship as a native, cargo-feature-gated built-in provider in P3.
4. **Ripple-delete/reorder ownership.** Architecture §11 says the engine "emits `timeline.ripple_delete_ranges` / `timeline.reorder`", but neither is listed in P2 scope (ripple delete of a selection is). 3F.01 assumes E2A does **not** provide multi-range, cross-track, caption-aware versions. If E2A does, shrink 3F.01.
5. **Command namespaces missing from §5.1.** New namespaces/verbs used: `font`, `asset`, `analysis` (detect_silence/detect_fillers), and `timeline.remove_silences/remove_fillers` (avoid `edit`, which is undo/redo). Suggest adding `font`, `asset`, `analysis` to the §5.1 list. Emoji search was folded into `asset.search{kind:emoji}`.
6. **Caption anchoring semantics (undecided).** Captions are timeline items with absolute times plus word-range links. When the user later trims/moves the source clip, blocks do not follow automatically. Chosen: explicit `caption.resync` + automatic resync when the edit goes through `ripple_delete_ranges`. Needs product sign-off (CapCut captions follow their clip).
7. **`@cutweave/compose` API vs architecture §12 example.** RESOLVED — the props builder is named `p` (`p.string()`, `p.color()`, `p.number()`…), not zod `z`; we ship our own tiny schema builder instead of bundling zod into QuickJS, and §12's example uses `p`. Also avoid Remotion-identical names (licence hygiene, ADR-012); human API review needed before 3B.02 merges.
8. **Caption performance vs JS.** 28 styles × per-frame QuickJS would risk the §19 budget. 3E.03 mandates a native data-driven fast path (≥ 60% of styles data-only); JS only for complex motion. Revisit if Spike 4b numbers disappoint.
9. **Text stack contingency.** ADR-007 (MSDF vs Vello vs skia-safe) is decided by Spike S4. 3A.04–3A.06 assume MSDF + Vello fallback; if S4 selects skia-safe, replace 3A.04/3A.06 and re-baseline goldens.
10. **Golden frames across adapters.** WARP, lavapipe and Metal produce slightly different pixels; goldens must be stored per adapter family with the single tolerance SSIM ≥ 0.995 AND PSNR ≥ 40 dB (preview vs pre-encode export on the same adapter is bit-identical). DoD-GF assumes 1O established that scheme.
11. **Cloud STT verification.** Research marks Scribe's API fields/pricing as unverified `[S]`; 3D.03 must verify at implementation time. Deepgram/AssemblyAI deliberately deferred to Phase 5.
12. **Music/SFX sourcing risk.** Truly CC0 high-quality music is scarce; plan allows CC-BY with an attribution UI. Pixabay/Mixkit licences excluded (not OSS-redistributable). Needs a named human curator; no Content-ID guarantees (disclaimer in docs).
13. **Exit-criterion definition.** "< 5 minutes" must be fixed as: cloud STT, HW encode, reference machine, install excluded, 3-min clip. 3I.03 enforces the engine share (≤ 90 s); local-STT timing reported but non-gating.
14. **Alpha installers unsigned.** macOS arm64 refuses unsigned binaries unless ad-hoc signed and quarantine-cleared; Windows SmartScreen warns. Docs explain overrides; real signing in 7A.
15. **Left-panel tab registry.** P3 (Text, Captions, Audio, Stickers), P4 (Effects, Transitions, Filters) and P5 (AI Studio) all add tabs. E2H should expose a panel-registration API so these don't conflict in `packages/editor`.
16. **Bundled font payload and CJK.** Bundled OFL fonts ≈ 25 MB; CJK/Arabic/Indic via downloadable packs. Caption styles must degrade (no animation tofu) when a pack is missing.
17. **First-party content licence.** Propose plugins' code GPL-3.0-or-later (with plugin exception) and data assets (stickers, presets, LUTs, SFX) CC0 or CC-BY-4.0; confirm against ADR-011.
