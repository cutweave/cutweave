# ADR-0010: Layered plugin system (L0-L4): data, WGSL, QuickJS compositions, WASM components, iframes, native out-of-process

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (product owner approved)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §15, §12, §14, §18

## Context

Decisions D6 and principle 5 require extensibility across effects, transitions, text styles, AI providers, importers/exporters, UI panels, commands and packs, with first-party built-ins using the same interfaces as third parties. Plugins run in a GPL host and must not be able to read keys, corrupt the document or break determinism. Research 07 maps extension points to mechanisms and risk.

## Decision drivers

- Most authors should never write native code.
- Sandboxing proportional to trust and cost; capability-based permissions.
- A plugin command is registered once and appears in the UI palette, CLI and MCP.
- Deterministic rendering (no hidden state, no clock, seeded RNG).
- Language-neutral, versionable interfaces.

## Considered options

1. **Layers L0-L4, each with the weakest mechanism that suffices** (chosen).
2. **One mechanism for everything: WASM components.** Rejected: GPU effects must be WGSL for the render graph, UI needs a DOM, and compositions need a cheap per-frame scene function.
3. **Native dynamic libraries loaded in-process.** Rejected: no isolation, ABI churn, and it weakens the separate-works licensing argument (ADR-0011). Native code is allowed only out of process (L4).
4. **Extism instead of wasmtime and WIT.** Rejected (research 07 §1; stack doc): no component model.
5. **JS-only plugins (VS Code style).** Rejected: no GPU or deterministic-render story and weak sandboxing for providers and importers.

## Decision

| Layer | Mechanism | Sandbox |
|---|---|---|
| L0 data packs | files plus manifest (LUTs, fonts, stickers, SFX, caption styles, templates, macros, export presets) | validated data only |
| L1 GPU effects | WGSL plus `effect.toml` (effects, transitions, blend modes, generators) | naga validation, GPU time budget, auto-disable |
| L1.5 compositions | TS/JSX via oxc into QuickJS scene functions (ADR-0012) | no I/O, deterministic, CPU and memory limits |
| L2 WASM components | wasmtime plus WIT (WASI 0.2/0.3): providers, importers/exporters, publishers, command handlers, audio FX | capability permissions; no keys |
| L3 UI extensions | sandboxed iframes, contribution points, postMessage SDK | CSP, no Node, permissioned RPC |
| L4 native | separate process plus IPC (CLAP/VST3, OFX, codecs) | process isolation, v2+ |

A plugin manifest (`cutweave-plugin.toml`) declares id, version, `host_api` range, contributions, permissions and signing. Plugin commands go into the same registry as built-ins. Distribution uses a signed git-based index; projects pin versions and open with placeholders when a plugin is missing. Plugin data lives only in namespaced `custom` maps. Built-in effects, transitions, providers and compositions ship as first-party plugins. `cutweave-plugins` v0 lands in WP-3B.01 for first-party use and P6 extends it to third parties (R1). The manifest and WIT worlds are contracts: changes need an ADR.

## Consequences

**Positive**

- One registry means automatic palette, CLI and MCP surfaces for every plugin command.
- Dogfooding keeps the interfaces honest.
- Each layer has a clear cost and trust model, and most contributions are data or WGSL.
- Out-of-process native code keeps crashes and licensing contained.

**Negative / costs**

- Several runtimes to maintain and secure (naga, QuickJS, wasmtime, iframe host); a large test and fuzz surface.
- WIT, `@cutweave/sdk` and the compose runtime become versioned public APIs with a deprecation cycle (arch §11).
- Temporal effects must use `history` taps (<= 8 frames), not feedback buffers (R4), which limits some effect styles.
- Permission prompts and signing infrastructure are required before third-party distribution (P6).

**Follow-ups**

- Epic 3B (compositions and v0 plugins), 4A (effects), 6A-6C (platform and index).

## Links

- Architecture: [§15](../01-architecture.md), [§12](../01-architecture.md), [§8](../01-architecture.md)
- Research: [07-plugin-architecture.md](../research/07-plugin-architecture.md) (§0, §1 WIT, §2 GPU effects, §4 UI, §9 extension points)
- Integrator resolutions: R1, R4, R19
- Related: ADR-0008, ADR-0011, ADR-0012 (compositions), ADR-0016
