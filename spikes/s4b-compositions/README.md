# Spike S4b — compositions: oxc → rquickjs → scene graph → S4 compositor (WP-0G.01)

> **Throwaway spike** (docs/wbs/P0-foundations.md §0G). Own Cargo workspace, excluded from the root
> workspace, never imported by `crates/`. Only the findings carry forward into `cutweave-compose`
> and `packages/compose-runtime` (Phase 3). Architecture: §12 (programmable compositions), §8
> (render pipeline), §15 (L1.5 plugins). Decision it feeds: ADR-0012 (via WP-0G.02).

## What it does

```text
lower-third.tsx ──► transpile (oxc 0.159.0, in-process)
                      parse → semantic (early errors) → sandbox policy (imports, eval/Function,
                      I/O globals) → TS strip + automatic JSX runtime (importSource
                      "@cutweave/compose") → codegen (ASCII, source map)
                      └─► structured diagnostics {code, file, line, column, help}
                │
                ▼
rquickjs 0.14.0 (QuickJS-ng 0.16.2) sandbox, one runtime per composition
  modules: "@cutweave/compose" (prelude) · ".../jsx-runtime" · "cutweave:host" (hardening + bind)
  defineComposition({...}).render({frame, fps, props, size}) ─► plain node objects
  limits: 64 MiB heap · 512 KiB JS stack · 50 ms/frame interrupt deadline · 10k nodes
                │  (host reads the node objects straight from the QuickJS heap; JSON path kept
                ▼   for canonical hashing / debugging)
lower (nodes → s4::scene::Scene: groups sized to their content and clipped to the canvas,
       ellipse → rounded shape, text → s4 Text or greeked placeholders, image → s4 Media)
                │
                ▼
s4::gpu::Renderer (wgpu 30, linear light, Rgba16Float) ─► PNG (cICP-tagged)
```

The §12 example is in [`examples/lower-third.tsx`](examples/lower-third.tsx) verbatim; the
200-node stress scene is [`examples/stress-200.tsx`](examples/stress-200.tsx).

## How to run

```sh
cd spikes/s4b-compositions
cargo test                                    # 17 unit + 10 compose + 43 sandbox + 3 GPU render tests
cargo clippy --all-targets -- -D warnings

cargo run --release --bin s4b -- transpile examples/lower-third.tsx          # JS out (or --json)
cargo run --release --bin s4b -- describe examples/lower-third.tsx           # id, fps, props JSON Schema
cargo run --release --bin s4b -- scene examples/lower-third.tsx --frame 60   # nodes (or --s4 for the S4 scene)
cargo run --release --bin s4b -- render examples/lower-third.tsx --frames 0,60,119 --out-dir out \
    [--adapter software|auto] [--transfer srgb] [--under camera709] [--text auto|native|placeholder]
cargo run --release --bin s4b -- bench examples/stress-200.tsx --frames 10000 [--render 100 --adapter auto]
cargo run --release --bin s4b -- determinism examples/lower-third.tsx --runs 3   # 3 processes + reverse order
cargo run --release --bin s4b -- sandbox                                       # adversarial suite table
cargo run --release --bin s4b -- globals                                       # what a composition can see
```

Props come from `--props '{...}'`, `--props @file.json` or `<stem>.props.json` next to the source.
`S4_ADAPTER` picks the adapter for the GPU tests (default: software/WARP, like S4).

## Design

### Authoring API (scratch `@cutweave/compose`, [`runtime/compose.js`](runtime/compose.js))

Original implementation; no Remotion code or API surface is copied. Everything is a pure function
of its arguments.

| Export | What |
|---|---|
| `defineComposition({ id, title?, props, durationInFrames, fps?, render })` | Validates and brands the definition; `render({ frame, fps, props, size: { w, h }, durationInFrames })` returns nodes. |
| `p.string/number/integer/boolean/color/enum/array/object` + `.default() .optional() .label() .describe() .min() .max() .step() .multiline() .animatable()` | Immutable props builder: validates props (applying defaults, JSON-Pointer error paths) and emits JSON Schema 2020-12 with `x-cutweave-*` UI hints. |
| `interpolate(x, inRange, outRange, { clamp: true\|"start"\|"end", easing })` | Piecewise-linear, multi-segment, optional per-segment easing. |
| `spring({ frame, fps, from, to, stiffness, damping, mass, velocity, delay, clamp })` | **Closed-form** damped oscillator (under/critically/over-damped): O(1) for any frame, so random access (scrubbing, parallel export) costs the same as sequential playback. |
| `easing.*` (`linear`, `in/out/inOut` × `Quad/Cubic/Sine/Expo`, `outBack`, `bezier(x1,y1,x2,y2)`) | Easing curves. |
| `random(seed)`, `hashSeed`, `mixColor(a, b, t)` | Stateless seeded noise (FNV-1a + mulberry32); sRGB hex mixing. |
| `Group`, `Rect`, `Ellipse`, `Text`, `Image`, `Fragment`, `jsx`/`jsxs`/`createElement` | Plain JSON-able nodes `{ type, ...props }`; function components are called immediately (no hooks, no state). |

Node props: all nodes take `x, y` (where the anchor lands in the parent), `anchor` (normalised,
default top-left), `scale`, `rotation` (clockwise degrees), `opacity`, `blend` (S4's 8 modes),
`blur` (S4 blur-lite), `id`/`key`. `Rect { w, h, radius, fill }`, `Ellipse { w, h, fill }`,
`Text { font, size, fill }` with string children, `Image { src, w?, h?, frame? }` where `src` is
an **asset id** resolved by the host (no file access), `Group` pivots on its local origin.

### Sandbox ([`src/runtime.rs`](src/runtime.rs), [`runtime/host.js`](runtime/host.js))

| Threat | Mechanism |
|---|---|
| I/O (`fetch`, `require`, `fs`, `process`, timers, workers) | Absent: QuickJS core has none of them (quickjs-libc is not linked). The transpiler rejects foreign `import`, `require()` and dynamic `import()` (errors) and flags I/O globals (warnings). |
| Module loading | No module loader is installed: only the pre-declared `@cutweave/compose`, its jsx-runtime and `cutweave:host` resolve; `import()` of anything else fails at runtime too (tested with the transpiler bypassed). |
| Wall clock | `Date` / `Performance` intrinsics not added; `Date.now()` and `performance.now()` return the **composition time** of the frame (`frame / fps` in ms); `new Date()` throws. |
| Unseeded RNG | `Math.random` = mulberry32 reseeded per **(item seed, frame)**: values are independent of evaluation order. |
| Code generation | `eval` deleted; `Function` and the `.constructor` of plain/async/generator/async-generator function prototypes replaced by a throwing stub (non-writable, non-configurable). The transpiler rejects `eval`, `new Function` statically. |
| Blocking / GC-observable | `Atomics`, `SharedArrayBuffer`, `WeakRef`, `FinalizationRegistry`, `queueMicrotask` deleted (QuickJS-ng adds `queueMicrotask` to its base objects); `Atomics.wait` would also be refused (`can_block` is false). |
| CPU | Interrupt handler with a wall-clock deadline: 50 ms/frame, 500 ms for module evaluation and `bind`; the interrupt is uncatchable in QuickJS-ng; QuickJS's regex engine polls it too; a call that returns after the deadline (long native builtin) is still reported as a timeout. Optional deterministic fuel budget (`Limits::fuel_polls`). Microtasks queued by `render()` are drained inside the same deadline. |
| Memory | `JS_SetMemoryLimit` 64 MiB (catchable `InternalError: out of memory`); `run_gc` after any failed call; 10,000-node and 16 MiB scene caps. |
| Stack | `JS_SetMaxStackSize` 512 KiB; the CLI runs compositions on a 16 MiB worker thread. |
| Purity | `FrameEvaluator::check_pure(frames)` re-evaluates frames in reverse and requires identical scene JSON (catches module-level state). |
| Console | `console.*` captured per frame (50 lines, 1,000 chars each) and returned with the scene. |

### Diagnostics ([`src/diagnostics.rs`](src/diagnostics.rs))

Every failure is a `ComposeError { kind, diagnostics: [{ severity, code, phase, message, file,
line, column, endLine, endColumn, path?, help?, stack? }] }` (serde JSON = the MCP payload).
Columns are 1-based UTF-16 units. Runtime positions are mapped back to the TS source through the
oxc source map (QuickJS-ng reports `file:line:col`; codegen is ASCII-only so columns are bytes);
errors thrown inside `@cutweave/compose` are located at the composition's call site.

| Code | Kind | Meaning |
|---|---|---|
| CW0001 / 0002 / 0003 | compile | oxc parse / semantic / transform error |
| CW0101 / 0102 / 0103 | compile | foreign import or `require` / dynamic `import()` / `eval` or `Function` |
| CW0104 | warning | reference to an I/O global that does not exist in the sandbox |
| CW0105 | compile | no `export default` |
| CW0201 | props | props do not match the `p.*` schema (`path` = JSON Pointer) |
| CW0301 | runtime | exception (`RangeError: too late` at `throw.tsx:8:…`) |
| CW0302 / 0303 / 0304 | timeout / out_of_memory / stack_overflow | limits hit |
| CW0305 / 0306 / 0307 | runtime | async `render()` / bad definition or default export / impure `render()` |
| CW0401 / 0402 | scene | invalid node (with node path, e.g. `nodes/0/children/2 (<rect>)`) / too many nodes |

Example (`s4b scene throw.tsx --frame 3`, a bad `interpolate(frame, [0], [1])` call on line 9 of
a TSX file whose line 2 is a TS type alias — the position survives TS stripping):

```json
{ "kind": "runtime", "diagnostics": [{ "severity": "error", "code": "CW0301", "phase": "runtime",
  "message": "CompositionError: interpolate(): inputRange and outputRange must be arrays of equal length >= 2",
  "file": "throw.tsx", "line": 9, "column": 39,
  "stack": ["CompositionError (@cutweave/compose:16:11)", "interpolate (@cutweave/compose:255:15)",
            "render (throw.tsx:9:39)", "renderFrame (cutweave:host:177:31)"] }] }
```

### Scene → S4 ([`src/lower.rs`](src/lower.rs))

* Each node becomes an S4 `Layer` (`Transform { position: (x, y), anchor, scale, rotation }`).
* `Group` → S4 `Group` whose intermediate is the **bounding box of its children** (rotations
  included, +1 px AA pad, + blur margin), **clipped to what can reach the canvas** (the canvas
  mapped back through the accumulated transform), and culled when empty. The layer anchor is
  set so the group pivots on its local origin. Group opacity is true group opacity.
* `Ellipse` → `Shape` with `corner_radius = min(w, h) / 2` (exact for circles; S4 has no ellipse).
* `Text` → S4 `Text` built through serde (so fields WP-0F.02 adds with defaults still parse),
  box estimated from `size` (0.54–0.58 em advance, 1.25 line height). S4 (as merged) reports
  `Unsupported` for text, so `--text auto` falls back once to **greeked placeholders** (one
  rounded bar per word in the text colour). When WP-0F.02's text pass lands, `auto` renders glyphs
  with no change here.
* `Image` → S4 `Media` (asset id → S4 synthetic sources or the generated `logo`), scaled to `w×h`.

## Results (INFORMAL — dev laptop, Win11 26200, Ryzen 9 5900HX, Rust 1.100, release build)

**Caveat:** other agents were compiling on the same machine during every run (60–100% total CPU
load), so p95/p99 and maxima are inflated and differ between rounds. Each row is two rounds of
`s4b bench --frames 10000 --iters 100 --frame-ms 1000`; formal numbers are WP-0G.02's job on the
reference machines. Versions: **oxc `=0.159.0`**, **rquickjs `=0.14.0`** (QuickJS-ng 0.16.2,
feature `disable-assertions`).

### Pass criteria (WBS 0G / roadmap S4b)

| Criterion | Target | Result | Verdict |
|---|---|---|---|
| Transpile (stress, 3.3 KB TSX) | < 50 ms | cold **0.63–0.94 ms**, warm p50 **0.14–0.16 ms** (p95 ≤ 0.22) | **PASS** (≈ 300×) |
| Transpile (lower-third) | < 50 ms | cold 0.47–0.59 ms, warm p50 0.035–0.049 ms | **PASS** |
| Scene eval, 200 nodes, every node animated (stress) | < 1 ms/frame | p50 **1.9–2.8 ms**, p95 2.6–4.8 ms (by toolchain, table below) | **FAIL** (2–3×) |
| Scene eval, 200 nodes, plain objects + trivial maths | < 1 ms/frame | p50 0.64 ms (clang-cl), 1.0 ms (MSVC, loaded) | borderline |
| Scene eval, §12 lower-third (4 nodes) | — | p50 **0.023–0.029 ms**, p95 ≤ 0.053 ms | — |
| Deterministic across runs | identical | scene JSON of frames 0..239 and PNGs byte-identical in 3 processes + reverse order | **PASS** |
| Sandbox (no I/O) enforced | all contained | **35/35** adversarial scripts contained, runtime survives | **PASS** |

### Per-frame eval of the 200-node stress scene (ms; render + host read + lowering)

| QuickJS C toolchain | p50 (r1 / r2) | p95 | p99 | Notes |
|---|---|---|---|---|
| MSVC (default `cc` on Windows) | 2.63 / 2.82 | 3.64 / 4.84 | 5.28 / 6.84 | switch dispatch |
| clang-cl (`CC_x86_64_pc_windows_msvc=clang-cl`) | 2.02 / 2.67 | 2.56 / 3.70 | 3.24 / 4.49 | switch dispatch |
| clang-cl + computed goto (one-line patch, scratch `[patch]`) | 1.94 / 2.24 | 3.35 / 3.59 | 3.91 / 4.08 | what GCC/clang get on Linux/macOS |
| *JSON path instead of direct read* (MSVC) | 3.21 + 0.13 serde | 5.4–5.7 | 6.2–7.4 | `JSON.stringify` ≈ 1 ms of it |

Breakdown (p50, MSVC): QuickJS `render()` + host read 2.6 ms · lowering to the S4 scene 0.06 ms ·
37 KB of scene JSON when serialised · 2.8 interrupt polls per frame · 185 KiB heap after 10⁴
frames. Calibration (1M iterations): `s += i * 0.5` loop 50 ms MSVC / 26 ms clang-cl; JS→JS call
loop 70 / 49 ms; 100k object allocations 27 / 21 ms. QuickJS C assertions (rquickjs default) cost
another 2.2× on the loop; they are off here.

### Cold start (per composition instance)

| Stage | ms (cold) |
|---|---|
| QuickJS runtime + context (custom intrinsics) | 0.25–0.38 |
| `@cutweave/compose` + `cutweave:host` evaluation and hardening | 1.2–1.5 |
| Composition module compile + eval + `describe()` | 0.17 (lower-third) – 0.5 (stress) |
| `bind` (props validation + defaults) | 0.06–0.11 |
| **Total `Composition::load`** | **1.7–2.4** (warm p50 1.2–1.8) |

### GPU (S4 executor, for context)

The lower-third renders at frames **0, 60, 119** (`tests/render.rs`, `s4b render`; text as greeked
placeholders because S4 has no text pass yet). Stress scene, 1080p, RTX 3080 Laptop (Vulkan):
GPU p50 **5.8 ms** (p95 7.6) — every shape is its own compute pass plus a full composite pass, so
200 nodes ≈ 400+ passes; on WARP ≈ 440 ms. Phase 1 must batch shapes (one instanced SDF pass per
group) — see Findings.

### Determinism

`s4b determinism <file> --runs 3` runs `hash` in three separate processes plus once evaluating
frames in reverse order (fresh QuickJS runtime and fresh WARP device each):

| Composition | scene JSON, frames 0..239 (fnv64 of per-frame hashes) | PNG fnv64 (1920×1080, BT.709, WARP) |
|---|---|---|
| lower-third | `e1547a9e67be6f04` ×4 | f0 `5bcfcd4edb89be5b`, f60 `2fb31a470eef1d76`, f119 `11c266c5ba1c20df` ×4 |
| stress-200 | `ee9fdd85979d03a1` ×4 | f0 `2ccbef3dcdb416a9`, f120 `fc3592f6dec04dc7`, f239 `eb4ad53b6861a5bb` ×4 |

`tests/render.rs` repeats the PNG check in-process (3 runs × 3 frames, fresh runtime + device),
`tests/sandbox.rs` checks that `Math.random` depends only on (item seed, frame) and not on
evaluation history, and `tests/compose.rs` runs `check_pure` on the stress scene.

### Sandbox suite (`s4b sandbox`, limits: 50 ms/frame, 64 MiB, 512 KiB stack)

| case | attack | result | ms |
|---|---|---|---|
| `fetch` | network via `fetch()` | Runtime CW0301 (ReferenceError) | 2.4 |
| `require-static` / `import-static` | `require('fs')` / `import fs from 'fs'` | Compile CW0101 | 0.0 |
| `require-obfuscated` | `globalThis['req'+'uire']('fs')` | Runtime CW0301 | 1.4 |
| `import-dynamic` | `import('fs')` | Compile CW0102 | 0.0 |
| `import-dynamic-runtime` | `await import('fs')`, transpiler bypassed (no module loader) | Runtime CW0301 at load | 1.4 |
| `process` / `timers` | `process.env`, `setTimeout` | Runtime CW0301 | 1.4 |
| `eval` / `new-function` | `eval('1+1')`, `new Function(...)` | Compile CW0103 | 0.0 |
| `eval-obfuscated` | `(0, globalThis['ev'+'al'])('1')` | Runtime CW0301 | 1.4 |
| `function-ctor-via-prototype` | `(function(){}).constructor('return globalThis')()` | Runtime CW0301 (EvalError) | 1.3 |
| `async-` / `generator-function-ctor` | `.constructor` of async / generator functions | Runtime CW0301 | 1.3 |
| `date-now` / `performance-now` | `Date.now()`, `performance.now()` | ok: composition time (frame 3 @ 30 fps = 100 ms) | 1.4 |
| `new-date` | `new Date()` | Runtime CW0301 (TypeError) | 1.4 |
| `math-random` | `Math.random()` | ok: seeded (determinism in tests) | 1.3 |
| `infinite-loop` | `while (true) {}` | Timeout CW0302 | 51.4 |
| `infinite-loop-catch` | catch the interrupt and loop again | Timeout CW0302 (uncatchable) | 51.8 |
| `regex-backtracking` | `/(a+)+$/` on 40 `a`s + `b` | Timeout CW0302 | 51.9 |
| `native-builtin-overrun` | default-comparator sort of 600k elements | Timeout CW0302, **finished late** | 129.7 |
| `microtask-flood` | endless `await` chain queued by `render()` | Timeout CW0302 | 52.5 |
| `async-render` | `render()` returns a Promise | Runtime CW0305 | 1.4 |
| `huge-alloc` | `new ArrayBuffer(1 GiB)` | OutOfMemory CW0303 | 1.4 |
| `huge-alloc-catchable` | the same inside `try/catch` | ok: caught in JS (`InternalError: out of memory`) | 1.4 |
| `alloc-bomb` | unbounded 1 MiB string growth | OutOfMemory CW0303 | 153.3 |
| `stack-overflow` | unbounded recursion | StackOverflow CW0304 | 1.8 |
| `scene-bomb` | 20,000 nodes (limit 10,000) | Scene CW0402 | 20.3 |
| `atomics-wait` | `Atomics.wait` | Runtime CW0301 (removed) | 1.5 |
| `serializer-tamper` | `Object.prototype.toJSON` | ok: no effect (host reads objects) | 1.3 |
| `host-read-getter-trap` / `-throw` | endless / throwing getter on `Object.prototype` hit by the host read | Timeout CW0302 / Runtime CW0301 | 51.6 / 1.7 |
| `lock-tamper` | redefine `Function.prototype.constructor` | Runtime CW0301 (TypeError) | 1.4 |
| `console-flood` | 10,000 `console.log` lines | ok: 50 kept, 9,950 dropped | 3.2 |

Every case also checks that the same runtime still evaluates afterwards; `tests/sandbox.rs`
additionally proves that a loop at frame 5 is interrupted in 50–500 ms while frames 4 and 6 keep
rendering, that heap is reclaimed after an OOM, that the deterministic fuel budget interrupts on
exactly the 51st poll, and that recursion is caught on a 1 MiB native stack. Globals visible to a
composition (`s4b globals`): the ES2023+ built-ins, `console`, the `Date` and `performance`
shims, and nothing else (no `eval`, `fetch`, `require`, `process`, timers, `Atomics`,
`SharedArrayBuffer`, `WeakRef`, `queueMicrotask`).

### Props JSON Schema produced from `p.*`

The §12 lower-third (`p.object({ name: p.string(), title: p.string(), accent:
p.color().default("#7c5cff") })`), as returned by `s4b describe` and asserted in
`tests/compose.rs`:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "name": { "type": "string" },
    "title": { "type": "string" },
    "accent": {
      "type": "string",
      "format": "color",
      "pattern": "^#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$",
      "x-cutweave-widget": "color",
      "default": "#7c5cff"
    }
  },
  "required": ["name", "title"],
  "additionalProperties": false,
  "x-cutweave-order": ["name", "title", "accent"]
}
```

The stress scene's `speed: p.number().min(0.1).max(4).step(0.1).default(1).animatable()` becomes
`{ "type": "number", "minimum": 0.1, "maximum": 4, "x-cutweave-step": 0.1, "x-cutweave-widget":
"slider", "x-cutweave-animatable": true, "default": 1 }`, and `.label("Start colour")` becomes
`"title"`. Props errors come back as `CW0201` with a JSON Pointer: `{}` → `required` at `/name`;
`"accent": "red"` → `expected a hex colour like "#7c5cff"` at `/accent`; an extra key →
`unknown prop "colour"` at `/colour`.

## Acceptance criteria (WP-0G.01)

| Criterion | Status | Evidence |
|---|---|---|
| Transpile the §12 TSX lower-third with oxc (TS + JSX → JS, custom JSX factory emitting plain node objects), in-process | **Met** | `transpile.rs`; automatic runtime → `@cutweave/compose/jsx-runtime`; `transpile::tests` |
| Scratch `@cutweave/compose` shim: `defineComposition`, `interpolate`, `spring`, `Group/Rect/Text` (+ `Ellipse`, `Image`, `easing`, `random`, `mixColor`, `p.*`) | **Met** | `runtime/compose.js` |
| rquickjs with no `fetch`/`fs`/`require`/`import()`; `Date.now`/`Math.random`/`performance.now` replaced (seeded from item id); memory limit; interrupt time limit | **Met** | `runtime.rs`, `runtime/host.js`, sandbox table |
| `render({frame,fps,props,size})` → serialisable scene → drawn by S4 (rects, rounded rects, text) | **Met**; text as placeholders until WP-0F.02 (`--text auto` switches to S4 text when its pass exists) | `lower.rs`, `render.rs`, `tests/render.rs` |
| Props schema → JSON Schema (`p` builder) proving the inspector/MCP path | **Met** | `s4b describe`, `lower_third_props_schema_is_json_schema` |
| 200-node test scene; adversarial scripts (infinite loop, huge alloc, `process`, `require('fs')`, `fetch`, `eval`, `new Function`) | **Met** (35 scripts) | `examples/stress-200.tsx`, `src/adversarial.rs`, `tests/sandbox.rs` |
| Loop interrupted within the time limit (50 ms default); alloc bomb fails with a catchable error; forbidden globals undefined/throw; engine survives (asserted) | **Met** | `loop_is_interrupted_within_the_limit_and_runtime_keeps_rendering`, `huge_alloc_catchable`, `forbidden_globals_are_absent`, `survived` on every case |
| Lower-third renders on S4 to PNG at frames 0, 60, 119 | **Met** | `lower_third_renders_frames_0_60_119`; `s4b render --frames 0,60,119` |
| Rust tests for each adversarial script and for determinism | **Met** | one test per case (`every_case_has_a_test` enforces it); `png_bytes_are_identical_across_three_independent_runs`; `s4b determinism` |
| `cargo test` + `cargo clippy --all-targets -D warnings` in the spike; root `just check` unaffected | **Met** | 17 unit + 10 compose + 43 sandbox + 3 GPU tests |

Deviations for the integrator: the crate lives in `spikes/s4b-compositions` (the brief) rather than
`spikes/s4b-compose` (WBS Area). The JSON Schema extraction uses a `p` builder (arch §12 naming),
not a `z`-like name. Text renders as placeholders (S4 text is WP-0F.02, in flight on another
branch); when it merges, `TextNode` is built through serde, so added fields with defaults need no
change here, but a renamed/required field would.

## Findings for Phase 1

**Performance**

1. **Per-frame cost is the composition's own JavaScript, not the boundary.** In the stress scene,
   removing helpers one at a time (spring, interpolate+easing, mixColor, 2× seeded random, JSX
   factory) each saved 0.2–0.5 ms; the same 200 nodes built as plain objects with trivial maths
   cost ≈ 0.64 ms, of which ≈ 0.23 ms is the host read. QuickJS-ng here runs a `for` loop
   iteration (`s += i * 0.5`) in ≈ 26–50 ns, a JS→JS call in ≈ 50–70 ns, an object allocation in
   ≈ 0.2–0.3 µs. A "fully animated" 200-node scene (5 helper calls per node) therefore lands at
   ≈ 2–2.8 ms; the §12 lower-third at ≈ 0.025 ms.
2. **Build flags matter 2–4×.** (a) rquickjs builds QuickJS-ng **with C assertions on** unless the
   `disable-assertions` feature is set (2.2× on the loop calibration; set here). (b) MSVC cannot
   compile QuickJS-ng's computed-goto dispatch (`DIRECT_DISPATCH 0` under `_MSC_VER`): building
   the C code with **clang-cl** (`CC_x86_64_pc_windows_msvc=clang-cl`) was ≈ 1.5–2× faster than
   MSVC; re-enabling computed goto for clang-cl (one-line patch, tested via a scratch
   `[patch]`) gave little extra here. Linux/macOS builds (GCC/clang) get computed goto for free.
   Phase 1: ship QuickJS built with clang everywhere, assertions off; measure 0G.02 on the
   reference machines rather than this contended laptop.
3. **Don't serialise scenes through JSON on the hot path.** `JSON.stringify` of the 37 KB stress
   scene costs ≈ 1 ms inside QuickJS (double formatting) — the whole budget — plus ≈ 0.15 ms of
   serde. Reading the returned node objects **directly from the QuickJS heap** with pre-created
   property atoms costs ≈ 0.23 ms for 200 nodes (≈ 5× cheaper). Keep JSON as the canonical form
   for hashing, debugging and MCP (`s4b scene`), not for playback. A typed-array encoding would be
   the next step if the read shows up in profiles.
4. **Make the budget achievable by architecture, not only by speed:** frames are pure, so (a)
   evaluate ahead of the playhead on a worker pool (one runtime per worker; QuickJS runtimes are
   `!Send`), (b) cache scenes by `(source hash, props hash, frame)` — scrubbing back is free,
   (c) offer helpers natively (spring/interpolate/colour are pure and tiny; a Rust host function
   call costs less than the JS body), (d) let authors hoist frame-independent subtrees
   (`memo`-style API or automatic dependency tracking), (e) apply the 1 ms budget to preview
   playback only; export can afford more.
5. **Wall-clock deadlines are not deterministic.** Under load (other builds on the machine), a
   2–3 ms frame was descheduled past the 50 ms deadline once in ~10⁴ frames (bench aborted with
   CW0302). A **fuel budget** — counting interrupt-handler polls, which QuickJS makes every
   10,000 calls/back-edges — is deterministic (`Limits::fuel_polls`, tested: interrupted on
   exactly the 51st poll). Use fuel as the primary limit and a generous wall clock (≥ 250 ms) as
   the backstop; the stress scene needs ≈ 3–5 polls/frame.
6. **Cold start ≈ 2 ms per composition** (runtime + context 0.35 ms, prelude + hardening
   1.4 ms, module + describe 0.4 ms). Precompiling the prelude to bytecode (`Module::write` /
   `Module::load`, which needs a reviewed `unsafe`) or one runtime with many contexts per worker
   would cut it; it is already far below anything a user notices.

**Transpile (oxc)**

7. **oxc is ≈ 400× under the 50 ms budget**: parse + semantic + policy + transform + codegen with
   a source map takes ≈ 0.13 ms warm, ≈ 1 ms cold, for a 3 KB TSX file. Semantic analysis (needed
   to tell a global `require`/`eval`/`fetch` from a local binding) is ≈ 20% of it.
8. **oxc API churn is real** (0.159.0 today, weekly releases): during this spike `ParserReturn`
   lost `panicked`, `SemanticBuilder::new()` stopped building `AstNodes` by default (an index panic
   if you use them), and `oxc_sourcemap` is not re-exported. Pin exactly (`=0.159.0`) and keep all
   oxc use behind one module (`transpile.rs` here) with its own tests; budget an upgrade task per
   quarter.
9. The automatic JSX runtime with `importSource: "@cutweave/compose"` works as is; `<X {...p}
   key={k}/>` falls back to `createElement` from the import source, so the runtime must export
   it. oxc does **no type checking**: agents get syntax, semantic, policy, props and runtime
   diagnostics, but TS type errors need `compose.d.ts` + `tsc`/tsgo outside the engine (an
   optional MCP tool), not in-engine.

**Sandbox**

10. **"Sandbox by absence" holds**: QuickJS core has no I/O, no timers and no module loader; all
    35 adversarial scripts are contained with structured errors and the runtime keeps rendering
    afterwards. Defence in depth (transpiler policy + runtime hardening) is cheap: the static
    checks give agents line-accurate errors before anything runs.
11. **Native-stack hazard (found the hard way):** QuickJS checks its JS stack limit against the
    *native* stack of the calling thread. With a 1 MiB limit on the Windows main thread (1 MiB
    stack) unbounded recursion **crashed the process** before QuickJS's check fired. Run
    compositions on dedicated threads with an explicit stack size and keep the JS limit well
    below it (512 KiB here, tested on a 1 MiB thread).
12. **QuickJS-ng adds `queueMicrotask` to its base objects** and `Atomics` comes with typed arrays;
    both removed. `Atomics.wait` is refused anyway (`can_block` false). Audit `s4b globals` on
    every QuickJS upgrade.
13. `eval_internal` must stay enabled for the host to compile modules, so `eval`/`Function` are
    removed by patching globals and prototypes. Phase 1 can compile in a privileged context and
    load **bytecode** into an Eval-less context, which removes code generation entirely.
14. **Prototype traps reach the host**: getters on `Object.prototype` run during the host's
    property reads (tested: an endless getter is interrupted, a throwing one is a CW0301). Host
    reads must stay inside the deadline (they do). Freezing intrinsics (SES-style lockdown)
    would close this and stop cross-frame state in prototypes, at the cost of the "override
    mistake" for code that assigns inherited properties.
15. **Interrupts do not cover native builtins**: the deadline is polled in bytecode and the regex
    engine, not inside long native calls. A default-comparator sort of 600k elements ran
    ≈ 130 ms against the 50 ms deadline and *returned normally*; the host now also checks the
    elapsed time after each call and reports such frames as CW0302 ("finished late"). The memory
    limit bounds the input size (a 1.5M-element sort hits OOM first), so the overrun is bounded,
    but Phase 1 needs a supervisor that can abandon a wedged worker (process or thread pool
    with a watchdog) as the real backstop.

**Determinism**

16. Same process, separate processes and reverse frame order give **byte-identical scene JSON
    (240 frames) and PNGs**. Two design choices make it hold: `Math.random` reseeded per
    (item, frame) and a closed-form `spring`. Module-level state is the remaining hole;
    `check_pure` (forward vs reverse evaluation) catches it and should run in
    `composition.validate`.
17. **Cross-platform caveat:** QuickJS uses the C runtime's `libm` (`sin`, `cos`, `exp`, `pow`), so
    last-ulp differences between Windows/macOS/Linux are possible; scene hashes should be
    compared per platform, or numbers quantised at the boundary (positions to 1/256 px; S4
    consumes `f32` anyway), or a deterministic libm compiled into QuickJS.

**Scene graph / API**

18. Text needs **host metrics**: the sandbox has no font data, so wrapping, alignment and
    "background that fits the text" need a pure host function (`measureText(font, size, text)`)
    backed by `cutweave-text`. Boxes here are estimates.
19. S4 gaps surfaced by compositions: no ellipse primitive (stadium approximation), effects clip
    to the node's own texture (a blurred rect has hard edges; effects need bounds expansion),
    and group intermediates should be bbox-sized and canvas-clipped by the render graph itself
    (done here in lowering; it keeps 200-node scenes cheap on the GPU).
20. Props schema: JSON Schema 2020-12 + `x-cutweave-widget/-step/-animatable/-order` covers the
    inspector and MCP needs here. `serde_json` without `preserve_order` sorts keys, so the
    explicit `x-cutweave-order` is needed (or enable `preserve_order` in the engine).
21. The flat node format (`{ type, x, y, ...props, children }`) doubles as the **declarative
    fallback's** format: plain JSON scenes go through the same reader, lowering and renderer, so
    the fallback is a subset rather than a second pipeline.
22. **The GPU, not QuickJS, is the next wall for 200-node scenes**: S4 runs one compute pass per
    shape plus one composite per layer, so the stress scene costs 5.8 ms GPU (p50) on an RTX 3080
    and would blow the iGPU budget. Phase 1's render graph must batch composition shapes (one
    instanced SDF/rounded-rect pass per group, composites only at group boundaries).

## Recommendation for ADR-012

**Accept "TS/JSX → oxc → QuickJS → scene graph" as the compositions runtime, with conditions;
do not fall back to declarative-only.** Three of the four S4b criteria pass with wide margins
(transpile ≈ 300× under budget, byte-identical determinism, 35/35 sandbox cases contained), and
the failing one — < 1 ms/frame for 200 *fully animated* nodes (measured 1.9–2.8 ms p50 on a
loaded Windows laptop) — is a throughput problem with known, cheap mitigations, while the
declarative fallback would lose functions, components and loops (everything that makes the
stress scene expressible in 100 lines) without making the GPU side any cheaper. Conditions for
Phase 1 (`cutweave-compose`):

1. Ship QuickJS-ng built with **clang** (all platforms) and `disable-assertions`; pin `rquickjs`
   and `oxc` exactly and wrap each behind one module with its own tests (oxc churns weekly).
2. **Read scenes from the QuickJS heap** (no JSON on the playback path); keep JSON for hashing,
   MCP and debugging.
3. Restate the budget as **"≤ 1 ms p50 for a typical 200-node template on the reference
   machines, measured in 0G.02"**, and design playback so heavier compositions don't need it:
   per-item runtime pool evaluating ahead of the playhead, a scene cache keyed by (source, props,
   frame), native (Rust) implementations of the pure helpers, and pre-rendering for export.
4. Keep the sandbox as built here and add: a **fuel budget** as the primary CPU limit (wall clock
   as backstop), runtimes on dedicated threads with explicit native stacks, a watchdog that can
   abandon a wedged worker, a `measureText` host function, and `check_pure` in
   `composition.validate`.
5. Accept the declarative JSON scene format (the same node schema) as a supported input for
   templates that need no code — a subset, not a fallback pipeline.

WP-0G.02 should re-measure on the reference machines (Linux GCC build, idle machine) before the
ADR is marked Accepted; if the typical-template budget still fails there, the verdict becomes
"Accepted with fallback: declarative scenes for heavy templates" rather than a rejection.
