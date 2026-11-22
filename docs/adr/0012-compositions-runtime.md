# ADR-0012: Compositions: TS/JSX transpiled by oxc and run in QuickJS to a scene graph (no headless browser)

- **Status:** Proposed
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (decision pending Spike S4b)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §12, §15 (L1.5), §8

## Context

Text animations, caption styles, lower-thirds, title cards and templates should be code-defined, deterministic per frame and authorable by agents, in the spirit of Remotion but reimplemented originally (brief D8; Remotion is source-available, not OSI, so no code or API is copied). Compositions must not have I/O or a wall clock, must be fast enough for per-frame evaluation, and must render through the same wgpu pipeline as everything else.

## Decision drivers

- Deterministic, sandboxed evaluation (no I/O, no clock, seeded RNG, CPU and memory limits).
- Per-frame cost small enough for playback: scene evaluation < 1 ms for 200 nodes.
- Cold-start cost: transpile < 50 ms.
- Props schema converts to JSON Schema, giving inspector UI, MCP parameters and keyframeable props.
- No browser dependency; compatible with agents that author compositions through MCP.

## Considered options

1. **oxc transpile of TS/JSX; run in `rquickjs` (QuickJS); `render(frame)` returns a scene-graph description drawn by `cutweave-render`** (leading hypothesis).
2. **Headless browser (Remotion-style DOM rendering).** Rejected: heavy, non-deterministic, no sandbox guarantees, and unavailable in headless engine mode.
3. **`deno_core` (V8).** Rejected as too heavy (stack doc); **Boa** rejected as slower.
4. **Declarative JSON scenes with expressions only.** Not chosen first: less expressive, but it is the documented fallback if the runtime misses its criteria.
5. **WASM components for compositions** (via ADR-0010 L2). Rejected: authoring cost and weak fit for fast scene-function iteration.

## Decision

**Decided by: Spike S4b** (WP-0G.01 build proof, WP-0G.02 measurement) with these pass criteria: **transpile < 50 ms**; **scene eval < 1 ms/frame for 200 nodes**; **deterministic across runs**; **sandbox (no I/O) enforced**. The S4b scene graph is rendered by the S4 compositor (ADR-0007). **On fail, simplify to declarative JSON scenes plus expressions.** Not decided here.

## Consequences

**Positive** (if confirmed)

- Deterministic code-defined motion graphics usable by humans and agents (`composition.create{source}` with structured compile diagnostics).
- Built-in text animations and templates dogfood the same runtime (ADR-0010 L1.5).
- Props schemas feed the inspector and MCP automatically (ADR-0013, ADR-0009).

**Negative / costs**

- An embedded JS engine plus transpiler to secure, fuzz and keep updated; QuickJS has performance limits for heavy scenes.
- `@cutweave/compose` becomes a public, MIT/Apache API (ADR-0011) with versioning obligations, and we must avoid copying Remotion's API surface.
- A fallback to declarative scenes would reduce authoring power and require a template migration.

**Follow-ups**

- WP-0G.01-0G.02 (spike); WP-0J.02 (accept); Epic 3B (runtime and `@cutweave/compose`).

## Links

- Architecture: [§12](../01-architecture.md), [§15](../01-architecture.md), [§8](../01-architecture.md)
- Research: [07-plugin-architecture.md](../research/07-plugin-architecture.md) §6 (programmable compositions, scene functions)
- Stack: [02-tech-stack.md](../02-tech-stack.md); roadmap S4b in [04-roadmap.md](../04-roadmap.md); WBS [0G](../wbs/P0-foundations.md)
- Related: ADR-0007, ADR-0010, ADR-0011
