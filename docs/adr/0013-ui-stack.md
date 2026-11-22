# ADR-0013: UI stack: React 19, Zustand, Canvas2D timeline, dockview and a schema-driven inspector

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (product owner approved)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §16, §5.3, §20; `docs/07-design-system.md`

## Context

The UI is a client of the engine (ADR-0002) and must feel as fast and familiar as CapCut: dockable panels, a timeline with thousands of items, a live preview with overlays, and inspectors for core props, effects, compositions, AI models and plugins. The project is open source and relies on AI coding agents, so contributor and agent familiarity matters. Research 03 §6 evaluates frameworks and timeline rendering.

## Decision drivers

- Contributor pool and agent fluency.
- Timeline performance: >= 55 fps pan/zoom with 5,000 items (arch §19).
- The engine is the single source of truth; the UI holds a read-only projection.
- One inspector for every parameterized thing (JSON Schema driven).
- The design system and tokens are the only source of styling (R26).

## Considered options

1. **React 19 + TypeScript + Vite, Zustand, Radix + Tailwind v4 on design tokens, dockview, cmdk, Canvas2D virtualized timeline** (chosen).
2. **Svelte 5 or SolidJS.** Rejected: smaller component and contributor ecosystems. Solid's fine-grained updates matter little because the timeline and preview are canvas, not framework-rendered (research 03 §6.1).
3. **DOM-rendered timeline.** Rejected: layout and hit-testing cost collapse past a few hundred clips with thumbnails and waveforms.
4. **PixiJS/WebGL timeline from the start.** Deferred: adopt only if benchmarks show Canvas2D cannot meet the budget.
5. **Rust-native UI (GPUI, egui, Slint).** Not chosen with ADR-0001 (Rust-only UI, no iframe plugin UI, contributor barrier); GPUI stays plan B.

## Decision

The editor is React 19 + TypeScript (strict) + Vite. State is split: `docStore` (normalized read-only projection updated by `doc.patched` events), `viewStore` (selection, zoom, layout, tools) and `playbackStore`. The UI never mutates `docStore`; it sends commands, and gestures use ephemeral transactions (ADR-0017). High-frequency state (playhead, drag, meters) lives in refs and imperative canvas code, not React state. Shared math (time conversion, keyframe and easing evaluation, snapping) is compiled to a wasm module shipped in `packages/protocol`. The timeline is a virtualized **Canvas2D** renderer with an explicit interaction state machine and a spatial index. Panels use `dockview`, commands use `cmdk`, components use Radix primitives and Tailwind v4 driven by `design/tokens.json` with no literal colors or sizes. Inspectors are produced by **one schema-driven renderer** over JSON Schema plus UI hints, shared by core props, effects, compositions, AI forms and plugins. i18n (ICU) and keyboard-first operation are required from day one.

## Consequences

**Positive**

- Large contributor and agent familiarity; mature Radix, Tailwind and dockview ecosystems.
- Local, exact timeline math without per-frame RPC; one inspector for every extension point.
- The design system is enforceable through lint (no literal values).

**Negative / costs**

- A Canvas2D timeline is custom code: hit-testing, accessibility and IME for in-timeline editing need dedicated work.
- React must be kept off hot paths by discipline and benchmark gates.
- Electron's size and memory overhead (ADR-0001) are accepted.
- The wasm math module is another build artifact to keep in sync with the engine's `cutweave-model` (generated, snapshot-gated).

**Follow-ups**

- Epic 0K (tokens, `packages/ui`), 1M (editor UI v0), 2H (workspace UI), WebGL upgrade only on benchmark evidence.

## Links

- Architecture: [§16](../01-architecture.md), [§5.3](../01-architecture.md), [§19](../01-architecture.md)
- Research: [03-tech-stack.md](../research/03-tech-stack.md) §6.1 (framework), §6.2 (state), §6.3 (timeline), §6.4 (UI kit)
- Design: [07-design-system.md](../07-design-system.md), [design/README.md](../../design/README.md); R26 in [05-work-breakdown.md](../05-work-breakdown.md)
- Related: ADR-0001 (shell), ADR-0002, ADR-0015
