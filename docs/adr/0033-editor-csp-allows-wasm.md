# ADR-0033: The editor's CSP allows `'wasm-unsafe-eval'` for the bundled math module

- **Status:** Accepted (product owner, 2026-11-26)
- **Date:** 2026-11-26
- **Deciders:** product owner
- **Supersedes:** none (amends the CSP rule in `apps/desktop/src/main/csp.ts` and arch §18)
- **Architecture refs:** `docs/01-architecture.md` §10 ("Shared math via wasm"), §18 (security); WP-2C.06

## Context

Arch §10 decides that the timeline and Inspector evaluate time, keyframe, easing and snapping math locally with the engine's own Rust code, compiled to a small WebAssembly module shipped in `@cutweave/protocol` (WP-2C.06, built: 98 KB gzipped, bit-identical with the engine). The renderer's Content Security Policy forbids `'wasm-unsafe-eval'`, and Chromium refuses to compile any WebAssembly without it. The two rules contradict each other; until one changes, no UI code can call `initMath()`.

`'wasm-unsafe-eval'` allows compiling WebAssembly only. It does not allow JavaScript `eval`, `new Function` or inline scripts, and the module still has to come from the app origin (`script-src 'self'`).

## Considered options

1. **Allow `'wasm-unsafe-eval'` in the editor window and its popouts only** (recommended). Plugin frames, sandboxed windows and every other window keep the strict policy.
2. Keep the ban and ask the engine over RPC for every curve point and snap: a round trip per frame while dragging, which §10 rejected.
3. Port the math to TypeScript: two implementations to keep bit-identical, which is what the wasm module avoids.

## Decision

Option 1: the editor window and its popouts allow `'wasm-unsafe-eval'` in `script-src`; every other window and plugin frame keeps the strict policy. (Accepted by the product owner on 2026-11-26.)

## Consequences

- With option 1: `csp.ts` adds `'wasm-unsafe-eval'` to `script-src` for the editor and popout policy only; its test asserts the directive appears there and nowhere else; the editor awaits `initMath()` at startup and refuses to start on a version mismatch.
- A script-injection bug could compile WebAssembly, which it could already approximate with plain script; the CSP's origin rules, the navigation guard and the absence of Node in the renderer are unchanged.
