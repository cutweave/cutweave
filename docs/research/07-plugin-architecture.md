# 07 - Plugin / Extension Architecture for Cutweave

Date: 2026-11-22. Status: research + recommendation. Not legal advice.
Legend: [V] verified from a source listed in Sources. [U] unverified / prior knowledge - check before relying.

## 0. Executive summary

Layer extension mechanisms by trust and cost. Most plugin authors should never write native code:

| Layer | Mechanism | Trust | Examples |
|---|---|---|---|
| L0 | Data packs (zip + manifest) | none needed | LUTs, fonts, stickers, SFX, templates, caption styles, export presets, macros |
| L1 | Declarative GPU effects (WGSL + TOML manifest) | validated, GPU-budgeted | filters, transitions, blend modes, generators |
| L1.5 | Programmable compositions: sandboxed JS/TS scene functions (deterministic, per-frame) rendered by the Rust/wgpu engine | sandboxed, CPU/time budgeted | motion graphics, text animations, code-defined templates |
| L2 | WASM components (WIT, wasmtime), capability-gated | sandboxed | AI provider adapters, importers, publishers, automations, MCP tools |
| L3 | UI extensions: sandboxed iframes + contribution points | sandboxed | panels, inspectors |
| L4 | Out-of-process native (CLAP/VST3 audio host, OFX bridge, heavy codecs) | user-trusted, crash-isolated | pro audio plugins, OFX |

Everything registers through the command bus: a plugin command declared once (typed JSON Schema) appears automatically in GUI (palette/auto-form), CLI and MCP.

## 1. WASM Component Model + WIT (L2)

Status [V]: WASI 0.3.0 released 2026-06-11; 0.3.1 on 2026-08-11 (adds `map<K,V>`). Wasmtime 46+ implements final 0.3.0 with component-model-async enabled by default; Wasmtime 43-45 implement RC snapshots; 41-42 earlier RCs; WASI 0.2 components still run. 0.3 drops `wasi:io` in favour of native `stream<T>`, `future<T>`, `async func`. `wit-bindgen` emits Rust `async fn`; jco for JS. Mismatched tool versions give confusing instantiation errors. Latest wasmtime on docs.rs seen: 43.x; WASI site says 46 is current [V]; exact latest version/date [U] - check GitHub releases.

Recommendation: wasmtime (Bytecode Alliance, Rust-native, component model first-class). Host supports p2 and p3; our own WIT worlds are versioned independently (section 7). Starting v1 directly on 0.3 is reasonable (streaming HTTP responses, progress, cancellation need async); guest toolchain maturity outside Rust/JS is uneven [U].

Extism: simple multi-language PDKs and host-function ABI, but not component-model based; maintainers stated they wanted more CM maturity and don't guarantee wasmtime [V, issue #666, date unknown; may be stale]. Wasmer: viable but component-model support lags wasmtime [U]. Choose raw wasmtime + WIT: typed, resource handles, and the WIT files double as the documented SDK contract (Zed model).

Fit by workload:
- Per-frame CPU pixel processing: NOT recommended. 4K RGBA is ~33 MB/frame; copying across the boundary per frame plus no GPU access misses real time. OK for tiny/low-res, analysis at downscale, or non-realtime export filters. Use L1 for pixels. Wasm perf is commonly quoted 1.2-2x native for compute [U - measure].
- Control logic, parameter computation, automations, importers (plan only; FFmpeg decodes), exporters/publishers: ideal.
- AI-provider adapters (HTTP): ideal via `wasi:http` with host-enforced allowlist; host injects secrets/auth headers so plugin never reads raw API keys.
- Resource handles: WIT `resource` for opaque host objects (project, clip, asset, job, frame); handles not pointers; per-call fuel/epoch deadlines; StoreLimits memory caps.
- Audio block DSP in WASM (512-sample blocks) is feasible.

Capabilities (declared in manifest, granted at install, enforced by omitting imports from the linker; deny by default; permission prompt in UI):
- `network`: host allowlist, https only, no raw sockets, size/rate limits.
- `fs`: preopen only plugin data dir; user files via host file-picker tokens; no ambient FS.
- `secrets`: named keys, host-injected.
- `project.read/write:<scope>`: command-bus scopes. `media.read`: frame access. `clock`, `random`.

## 2. GPU effects (L1)

Prior art:
- DaVinci DCTL / Fusion Fuses: single-file kernels with source-declared params; closed host. Lesson: low barrier.
- ISF (VIDVOX): GLSL with a JSON header declaring INPUTS (float/color/image/bool/point2D/long), PASSES (named buffers, persistent, size expressions), CATEGORIES. Closest to our goal; reuse the schema idea [U details].
- Shadertoy: iTime/iResolution/iChannel conventions - import converters only.
- OBS shader filter: uniforms with annotations -> auto UI.
- AE/Premiere C++ SDK (PF suites) and UXP panels: native ABI per host/OS; avoid. Motion/FxPlug: Apple-only, XPC out-of-process isolation - precedent for isolating plugins. Blender nodes: node-graph composition; possible later effect-graph.
- OpenFX [V]: ASWF project, BSD-3-Clause, v1.5 (2024; GPU support through OpenCL/CUDA/Metal; first major update since 1.4 in 2015). Hosts: Resolve, Natron, Nuke, Baselight, Houdini Copernicus (announced). Is hosting OFX worth it? Big ecosystem (largely proprietary binaries), but they are C ABI CPU/CUDA/OpenCL pipelines that don't zero-copy with wgpu, and proprietary binaries in a GPL host raise licensing questions. Verdict: v3 or never; if done, an out-of-process L4 bridge.

Effect package (`effect.toml` + `*.wgsl`):
- parameters: `float|int|bool|color|vec2|enum|curve|gradient|image|angle|point` with `min/max/default/step/unit`, `keyframeable`, `group`, UI hint.
- inputs: `source`; optional `secondary` (transitions: `from`,`to` + auto `progress`), `mask`.
- passes: `{shader, reads, writes, scale, format, iterations}`; persistent ping-pong buffers for feedback; built-ins `time, resolution, frame, progress, fps`.
- Host owns bind-group layout and generates the uniform struct from params; authors write only the entry point.
- Declares color space (linear/display) and alpha mode.

Validating untrusted WGSL:
1. naga parse+validate at install and load; reject unsupported `enable` extensions, push constants, undeclared bindings.
2. Static limits: pass count, texture size/format, loop-bound analysis (reject unbounded loops), instruction cap.
3. Only host-defined bindings; WebGPU robust access semantics keep OOB defined.
4. Runtime: GPU hangs can't be reliably killed; use reduced-res preview with frame budget via timestamp queries, auto-disable after N slow frames, install-time benchmark render, GPU work in a separate render process so device-lost/TDR doesn't kill the editor [U: wgpu device-lost handling exists, hang prevention not guaranteed].
5. Pipeline cache keyed by hash.
Residual risk: shader DoS cannot be fully prevented; document honestly.

## 3. Audio plugins (L4, mostly v2)

- CLAP: open standard, MIT [U prior knowledge], clean C ABI. Rust host: `clack-host` (low-level safe CLAP host) + `clack-extensions` [V]. nih-plug is authoring-only.
- VST3: relicensed to MIT from SDK 3.8 (Oct 2025); GPLv3/proprietary dual license no longer offered [V]. Removes the old GPL-compat issue. Host effort is much higher than CLAP (COM-like C++, GUI embedding, buses). Rust options: vst3 bindings, `rack` (hosts VST3/AU/CLAP; maturity unknown [V exists/U quality]).
- LV2: Linux-centric; skip v1.
- Plan: v1 = built-in effects + WASM audio effect world. v2 = CLAP host (clack) in a separate audio-plugin-host process (shared-memory ring buffer; native floating GUI windows; crash isolation). Then VST3. GUI embedding inside a Tauri window is per-OS painful - use floating windows.

## 4. UI plugins (L3)

Prior art:
- VS Code: extension host process, `package.json` contribution points, activation events, webviews (iframes, CSP, message passing) [U, well known].
- Figma [V]: plugin logic in QuickJS-in-WASM (formerly Realms), UI in sandboxed iframe, postMessage bridge, network allowlist via manifest enforced by CSP; messages must be treated as untrusted.
- Obsidian: plugins run with full renderer access (trust-based) - cautionary. Grafana/Backstage: in-process React plugins / module federation, vetted trust [U]. Tauri plugins are compile-time Rust crates; Tauri capabilities/permissions can scope IPC per window [U].

Options: module federation (fast, no isolation; first-party only); web components (no isolation); sandboxed iframe with opaque origin, strict CSP (`connect-src none` unless granted), served from custom protocol (`cutweave-ext://<id>/`), postMessage RPC onto the typed command bus (recommended). Provide `@cutweave/ext-sdk` (typed RPC generated from command schemas) and `@cutweave/ui` theme tokens. Contribution points: `panels`, `inspectors`, `menus`, `commands`, `keybindings`, `settings`, `timeline.decorations`. Inspectors for L1/L1.5 are auto-generated from parameter schemas (no UI code). Iframes cannot share GPU textures: panels for controls, not heavy preview. Never run plugin JS in the main window context.

## 5. Scripting / macros

Options: Rhai (pure Rust, easy sandbox, small community), Lua/mlua (fast, familiar, needs sandbox discipline), QuickJS via rquickjs / Boa (pure Rust, slower) / deno_core (V8, heavy), Python via PyO3 (popular with AI/video users but ships CPython and has effectively no sandbox).
Recommendation: no bespoke embedded language in v1. Command bus + CLI + MCP already allow automation in any language and by AI agents. In-app macros = recorded lists of typed commands (L0 data). Logic = WASM components (Rust, or JS via jco/ComponentizeJS, Python via componentize-py [U]). An embedded console later: QuickJS over command bus (see section 6, same runtime as compositions). Python out-of-process via CLI/MCP, not PyO3.

## 6. Programmable compositions / code-defined templates (L1.5)

### 6.1 Remotion's model (ideas only) [U unless noted - not fetched this session]
- Video = React components. A composition declares width/height/fps/durationInFrames and a component.
- Frame-driven: `useCurrentFrame()`, `useVideoConfig()`, `interpolate()`, `spring()`, `Easing`; `<Sequence from durationInFrames>` shifts time; `<Series>`, `<AbsoluteFill>`, `<Audio>/<Video>` primitives.
- Typed props: zod schema attached to the composition -> Studio auto-generates a props editor; props passed from CLI/API (`--props`), enabling data-driven templates.
- `<Player>` embeds the same component in a web app for live preview.
- Rendering: headless Chromium screenshots each frame (deterministic given pure function of frame), frames stitched with FFmpeg; because each frame is independent, rendering parallelizes across tabs/machines (Remotion Lambda distributes chunks). Cost: Chrome dependency, slow, memory heavy, no GPU zero-copy.
- License: Remotion uses a custom source-available license (free for individuals and small companies, paid company license above a threshold) - NOT OSI/GPL-compatible. We must not copy or derive code or API text; only adopt generic ideas (pure function of frame, schema-typed props, sequences, deterministic parallel render), which are not protectable. Naming should differ (avoid `useCurrentFrame`-identical API surface clones where practical). Verify current terms at remotion.dev/license.

### 6.2 Motion Canvas / Revideo [U]
- Motion Canvas (MIT): TypeScript, scenes are generator functions (`yield*` animation flows) building a retained scene graph of nodes (Rect, Txt, Circle, Layout), signals for reactive values, timeline via `waitFor`, `all()`, `chain()`; editor with live preview; renders via canvas in browser.
- Revideo (MIT, Re.video fork of Motion Canvas): headless rendering API (`renderVideo({projectFile, variables})`), variables for templates, still browser/Puppeteer-based rendering with ffmpeg.
- Takeaways: generator/timeline style is great for authoring sequences by LLMs; frame-pure style is better for scrubbing/random access/parallelism. MIT licensing means ideas and even code could be reused with attribution (GPL-compatible), but Puppeteer rendering is what we want to avoid.

### 6.3 Recommendation: "Scene functions" evaluated in a sandboxed runtime, rendered by Rust/wgpu
Core idea: the author's code does NOT draw pixels. It is a pure function `(props, time/frame, ctx) -> SceneGraph` (JSON-serializable tree of primitives) evaluated per frame (or per keyframe-batch) in a sandbox; the Rust engine diffs/consumes the scene graph and renders with wgpu (vector paths via lyon/vello, text via cosmic-text/swash with the same engine used for captions, images/video layers as textures, shader effects from L1 as node `effect` references, masks, blend modes, groups with transforms). Result: no headless Chrome, GPU-native, same renderer as timeline, deterministic, parallel-friendly.

Package (`composition.toml` + `scene.ts` bundled to `scene.js`):
```toml
[composition]
id = "com.example.lower-third"
kind = "template"            # template | text-animation | motion-graphic | generator
runtime = "js"               # js (QuickJS) | wasm-component
entry = "dist/scene.js"
size = "inherit"             # or fixed [1920,1080]
fps = "inherit"
duration = { default = 5.0, min = 1.0, max = 30.0 }   # seconds, or "from-content"
params_schema = "schema.json"      # JSON Schema (draft 2020-12) incl. defaults, min/max, x-cutweave-ui hints: color, font, asset, keyframeable
[permissions]
assets = ["project"]         # which asset refs it may reference; no network by default
```
Authoring API (original design, idea-level): `export default defineScene({ schema, render(ctx){ ... } })` with `ctx.frame, ctx.time, ctx.props, ctx.progress`, helpers `lerp/interpolate/ease/spring` (pure math), `seq(from,dur,fn)` for sequencing, node constructors `rect/text/path/image/video/group/effect`. Two authoring styles on the same output: (1) pure per-frame `render(ctx)` (random access, scrubbing, parallel); (2) optional generator-style timeline (`function*` with `yield* wait()/tween()`) that the SDK *compiles to a pure function* by pre-running to build a keyframed timeline (Motion Canvas feel, still deterministic). Text animations: `render` receives per-glyph/word/line metrics from the engine (text layout is done in Rust, passed in as `ctx.text.glyphs`) so JS returns per-glyph transforms/opacity - avoids JS layout engines.
Determinism rules: no `Date`, no `Math.random` (seeded `ctx.random(seed)` instead), no network, no timers, no DOM; floating math identical across machines is not guaranteed but is acceptable (QuickJS soft-uses doubles; same binary => same result). Enables render-time parallelism across frame ranges, caching (scene-graph hash per (props, frame)), and distributed rendering later.
Params: declared as JSON Schema -> (a) auto inspector (GUI), (b) MCP tool input schema for "apply template X with props" / "generate scene" commands, (c) CLI flags/`--props file.json`, (d) keyframeable props animate via core keyframe system before being passed in. LLMs write `scene.ts` + `schema.json`; `cutweave plugin check` validates and renders a contact sheet that an agent can inspect (feedback loop). Also expose "composition from prompt" as an AI-provider-agnostic command that returns a composition package for user review before install (code review dialog; runs sandboxed regardless).

Performance model: per-frame JS call costs ~tens-hundreds of microseconds for modest scenes; scene-graph serialization to Rust via structured buffers (or QuickJS native bindings building nodes directly to avoid JSON). Evaluate only when frame/props change; cache static subtrees (scene returns `static: true` hints). Target: 60 fps preview for scenes <~2000 nodes [U - to benchmark].

Runtime comparison:
| Option | Pros | Cons |
|---|---|---|
| QuickJS (rquickjs / quickjs-ng) | small (~1 MB), fast startup, easy memory/interrupt limits (interrupt handler, memory limit, stack limit), ES2023, sync, Figma precedent [V Figma QuickJS-in-WASM] | slow vs JIT (10-50x slower than V8 for hot loops [U]), no TS (pre-bundle with esbuild/SWC at build/install) |
| QuickJS-in-WASM (run inside wasmtime) | double sandbox, same wasm capability model, trivially deterministic, portable, fuel metering | ~2x slower again, boundary marshalling cost, build complexity (javy / quickjs-wasm) |
| Boa | pure Rust, no C, embeddable, improving conformance | slower than QuickJS currently [U], less mature limits |
| deno_core (V8) | fastest JIT, TS ecosystem | huge binary (tens of MB), build complexity, JIT in sandbox = bigger attack surface, V8 isolates still need external process hardening, nondeterminism risk minimal but heavy |
| Pure WASM component (author compiles Rust/AssemblyScript/JS->wasm) | strongest typing via WIT, wasmtime fuel | higher barrier; LLMs/authors prefer TS |
Recommendation: v1.5: QuickJS (quickjs-ng via rquickjs) in-process with memory/interrupt/stack limits behind a `SceneRuntime` trait; keep an alternative backend = QuickJS compiled to a wasm component (via wasmtime) for stronger isolation/untrusted marketplace content, and the pure-WASM-component route for power users. Avoid deno_core in v1 (size, attack surface). Revisit Boa when it matches QuickJS speed. TS is compiled at pack time by `cutweave plugin build` (esbuild) - no TS runtime on user machine.

What this gives vs Remotion: pure-function frame determinism and parallel/distributed export (frame-range chunks across worker processes) without Chrome; props schema auto-UI; MCP-friendly. What it lacks: arbitrary React/DOM/CSS/web libs (three.js, d3 DOM). Escape hatch: optional "web renderer" L4 sidecar (hidden webview/Chromium) for DOM/CSS compositions - off by default, v3, clearly labeled slower and less deterministic. Lottie import and Rive/Lottie-as-asset rendering (rlottie/dotlottie-rs/vello) can cover most animated-sticker needs at L0.

Security: JS runs with no host objects beyond `ctx` and math; per-frame instruction/time budget via interrupt handler (e.g. 10 ms default, 50 ms hard), memory cap (e.g. 64 MB), output scene-graph node cap, asset refs only to project/pack assets (host resolves), no eval of remote code. Failing scenes render a placeholder + error overlay and are surfaced in the UI, never crash.

## 7. Packaging, distribution, versioning

Manifest (`cutweave-plugin.toml`, inspired by Zed `extension.toml`, VS Code, Figma):

```toml
schema = 1
id = "com.example.glitch-pack"          # reverse-DNS unique
name = "Glitch Pack"
version = "1.2.0"
license = "MIT"                          # SPDX, required
authors = ["Jane <jane@example.com>"]
repository = "https://github.com/example/glitch-pack"

[engine]
host_api = "^0.3"                        # WIT package version range
min_app = "1.0.0"

[[contributes.effects]]
id = "glitch.rgb-split"
manifest = "effects/rgb-split/effect.toml"      # L1

[[contributes.compositions]]
id = "glitch.title"
manifest = "compositions/title/composition.toml"  # L1.5

[[contributes.commands]]
id = "glitch.randomize"
title = "Randomize glitch seed"
schema = "commands/randomize.json"       # JSON Schema in/out
handler = "component"                    # or "macro"
expose = ["gui", "cli", "mcp"]
risk = "write"                           # read | write | destructive | network

[[contributes.ai_providers]]
id = "acme-video-gen"
world = "cutweave:ai-provider/provider-plugin@0.3.0"

[[contributes.importers]]
extensions = ["xyz"]
mime = ["video/x-xyz"]

[[contributes.panels]]
id = "glitch.panel"
entry = "ui/index.html"
location = "right"

[[contributes.packs]]
kind = "lut"
path = "luts/"

[component]
wasm = "plugin.wasm"
world = "cutweave:plugin/plugin@0.3.0"

[permissions]
network = ["api.acme.ai"]
fs = ["plugin-data:rw"]
secrets = ["acme_api_key"]
project = ["read", "write:clips"]
```

Host API versioning: WIT packages semver (`cutweave:effect@0.3.0`); additive = new optional interface; breaking = new major package with host linking old+new for N-1. Zed pattern [V]: per-release WIT dirs (`since_vX`), manifest `wasm_api_version`, host keeps supported range, newer-API extensions refuse on older hosts; Zed forked the API often pre-1.0 (jumped 0.6 -> 0.8). Expect churn.

Distribution: git-based index repo (Zed/Homebrew-style; Zed's `zed-industries/extensions` is a repo of references and metadata comes from `extension.toml` [V]) - PR-reviewed entries pointing to repo+commit+artifact hash; CI validates (naga, WIT conformance, permission lint, license, test render) and publishes to GitHub Releases/mirror; zero hosting cost. Hosted search/ratings later over the same index. Package `.cutweaveplugin` = zip + `SHA256SUMS`.
Signing: v1 = hash pinned in reviewed index + optional minisign/Sigstore author signatures; verified-publisher later. Sideload = permission dialog + "unverified" banner. Auto-update: index poll (opt-out), user approves, re-prompt when permissions widen, keep previous version for rollback. Telemetry-free error reporting: local crash files, "copy report / open prefilled GitHub issue" button, nothing auto-sent.
Crash isolation: wasm traps caught in-process (plugin disabled, app continues); epoch interruption for runaway CPU; memory caps. In-process wasmtime for v1 control-plane calls; sidecar plugin-host process (IPC-ready async interfaces) for L4 from day one, optionally for all L2 later. QuickJS scene runtime in-process with interrupt limits, or in a render-worker process.
Prior art: Zed = Rust+wasmtime+WIT [V]. Bevy: compile-time Rust crate plugins, no runtime sandbox - not applicable to untrusted code. Lapce: WASI plugins with `volt.toml` + registry [U].

## 8. Licensing (GPL-3.0 host) - not legal advice

Facts: FSF's position is that dynamically linking GPL code creates a combined work, making proprietary linked plugins problematic absent an exception; arm's-length communication (pipes, sockets, separate processes) is generally argued more permissively but untested in court [V partly]. Copyright holders can grant additional permissions (GCC Runtime Library Exception, Classpath exception precedent) [V]. Key subtlety: a plugin that links a GPL-licensed SDK/binding crate is a derivative of the SDK; so the SDK license matters most.

Options:
A. Pure GPL-3.0 everywhere. Pros: simplest, maximum copyleft; Cons: chills commercial/proprietary plugin packs, ambiguity for OFX/binary plugins (VST3 now MIT is compatible).
B. GPL-3.0 app/engine + permissive SDK (MIT OR Apache-2.0, or MPL-2.0 if SDK changes should flow back) containing WIT files, SDK crates/TS packages, schemas, examples; plus a written "Plugin Interface Exception": plugins interacting only via documented sandboxed interfaces (WIT worlds, command bus/CLI/MCP, effect/composition manifests, iframe RPC) are independent works under any license. Pros: clear, cheap, GCC-exception-like; Cons: needs contributor consent mechanism (DCO alone cannot add exceptions later - use CLA-lite or include exception from day one), lawyer review.
C. LGPL/MPL engine crates + GPL GUI. Pros: adoption; Cons: allows closed forks of the engine, contrary to "fully open source" goal.
D. No explicit statement (rely on interpretation; Blender's Python add-on situation is argued as GPL-requiring but unclear in practice [U not verified]). Cons: ambiguity deters authors.
Recommendation: B. Do not support in-process dynamic native loading (L4 are separate processes) which strengthens the separate-works argument. Registry: SPDX required; proprietary allowed but labeled.
Remotion note: its license is not OSI; never copy code; use Motion Canvas/Revideo (MIT) code only with attribution if ever needed.

## 9. Extension points -> mechanism -> permissions -> priority

| Extension point | Mechanism | Permissions | Pri |
|---|---|---|---|
| Video effects/filters | L1 WGSL + TOML | none (GPU budget) | v1 |
| Transitions | L1 (two inputs + progress) | none | v1 |
| Blend modes | L1 (WGSL snippet in blend stage) | none | v1 |
| Generators (shapes, backgrounds) | L1 (no source input) | none | v1 |
| Particles / stateful generators | L1 persistent passes (simple); L1.5 scene fn | none | v2 |
| Text/caption styles | L0 style JSON | none | v1 |
| Text animations | L1.5 scene fn with engine-provided glyph metrics (+ L1 shader) | none | v1.5 |
| Motion graphics / code-defined templates | L1.5 scene fn (QuickJS) + JSON Schema params | none (asset refs) | v1.5 |
| Template packs/projects | L0 (+ L1.5 for parametric) | none | v1 |
| Audio effects | built-in; L2 WASM block DSP; L4 CLAP | sandbox | v1 WASM / v2 CLAP / v3 VST3, LV2 |
| AI model providers | L2 `ai-provider` world (wasi:http) | network allowlist, secrets | v1 |
| Importers | L2 `importer` world (+FFmpeg handoff) | fs (picked files), media | v1 |
| Exporters/codecs | L2 containers/metadata; L4 codecs | fs, spawn (L4) | v2 |
| Export presets | L0 | none | v1 |
| Export destinations (YouTube/TikTok) | L2 `publisher` world, host-managed OAuth | network, oauth, fs read | v1.5 |
| Asset packs (stickers, LUTs, fonts, SFX, Lottie) | L0 | none | v1 |
| UI panels/inspectors | L3 iframe + contribution points; auto-inspectors from schemas | command scopes | auto-inspectors v1; panels v1.5 |
| Commands/automations/macros | L0 macro JSON / L2 handlers | command scopes | v1 |
| MCP tools | automatic from commands; extra descriptors in manifest | per-command `risk` | v1 |
| DOM/CSS web compositions | L4 hidden-webview renderer (opt-in) | user trust | v3 / maybe never |
| OFX plugins | L4 out-of-process bridge | user trust | v3 / maybe never |
| Scripting console | QuickJS over command bus | scopes | v3 |
Also worth adding: color management (LUT/ACES, L0), scopes/analysis (L2), STT/caption engines (AI provider variant), project-format importers (FCPXML/EDL/OTIO), stock-asset sources (L2 + network), themes/keymaps/workspaces/localization (L0), render-farm backends.

## 10. Sample WIT

```wit
package cutweave:common@0.3.0;
interface types {
  record rational { num: s64, den: u64 }
  variant param-value { f(f64), i(s64), b(bool), color(tuple<f32,f32,f32,f32>), text(string) }
  record error { code: string, message: string, retryable: bool }
  resource frame {                 // opaque host-owned frame
    width: func() -> u32;
    height: func() -> u32;
    read-rgba8: func() -> list<u8>; // requires media.read; slow path
  }
}

// (a) Video effect (CPU escape hatch; GPU effects use the L1 manifest)
package cutweave:effect@0.3.0;
interface effect {
  use cutweave:common/types@0.3.0.{param-value, frame, error};
  record params { values: list<tuple<string, param-value>>, time: f64, fps: f64 }
  describe: func() -> string;  // JSON equivalent of effect.toml
  process: func(input: borrow<frame>, p: params) -> result<frame, error>; // small frames/analysis
  compute-uniforms: func(p: params) -> result<list<tuple<string, list<f32>>>, error>;
}
world effect-plugin { import cutweave:log/log; export effect; }

// (b) AI provider adapter (async, WASI 0.3)
package cutweave:ai-provider@0.3.0;
interface provider {
  use cutweave:common/types@0.3.0.{error};
  record capability { kind: string, models: list<string> } // t2i, t2v, i2v, tts, stt, llm, upscale
  record job-request { capability: string, model: string, input-json: string, assets: list<string> }
  variant job-event { progress(f32), partial(list<u8>), done(string), failed(error) }
  capabilities: func() -> list<capability>;
  estimate-cost: func(r: job-request) -> result<string, error>;
  submit: async func(r: job-request) -> result<stream<job-event>, error>;
  cancel: func(job-id: string);
}
world ai-provider-plugin {
  import wasi:http/client@0.3.0;   // host-filtered by manifest allowlist
  import cutweave:secrets/secrets;    // host injects auth, never returns the key
  export provider;
}

// (c) Importer
package cutweave:importer@0.3.0;
interface importer {
  use cutweave:common/types@0.3.0.{rational, error};
  record probe-result { confidence: f32, format-name: string }
  record track { kind: string, codec: string, start: rational, duration: rational, source-uri: string }
  record import-plan { tracks: list<track>, metadata-json: string }
  probe: func(head: list<u8>, filename: string) -> option<probe-result>;
  plan: async func(source: borrow<file-handle>) -> result<import-plan, error>;
}
world importer-plugin { import cutweave:files/files; export importer; }
```
Note: exact async WIT syntax is [U]; p3 uses `async func`/`stream<T>` per WASI site [V]. Importers return a plan; decoding stays in host FFmpeg so plugins never sit in the hot decode path.

## 11. Commands -> GUI + CLI + MCP automatically

1. Manifest declares command: JSON Schema in/out, title, category, `risk`, `expose`.
2. Host registers it on the command bus as `plugin-id.command-id` (namespaced; core ids reserved).
3. Generators from one schema: GUI = palette entry + auto-form (+ optional menu/keybinding); CLI = `cutweave <plugin> <command> --flags` and `cutweave call <id> '<json>'`; MCP = tool with name, description, inputSchema, annotations (readOnly/destructive from `risk`).
4. Execution: bus authenticates caller (gui/cli/mcp/plugin), checks permission scopes, writes to the undo journal (commands are undoable via inverse patch or transaction wrapper), and an audit log.
5. Plugins calling commands use scoped capability tokens; recursion and rate limits.
6. MCP: destructive/network commands require human confirmation for agent callers; plugin-supplied descriptions are untrusted text (prompt-injection risk) - sanitize, length-cap, show at install review.
7. Compositions and effects register automatically as commands such as `composition.apply {id, props}` with the composition's `params_schema` inlined as the tool schema.

## 12. Test and compatibility strategy

- WIT conformance suite per world: reference host harness + golden plugins (Rust, JS) in CI on all OSes; matrix of current and N-1 API versions.
- `cutweave plugin check`: manifest schema, WIT match, naga validate, permission lint, size, SPDX license, test render on software wgpu (lavapipe/WARP) vs golden image with tolerance, parameter fuzzing (NaN/inf/extremes), scene-fn determinism test (same frame rendered twice and in different order must match byte-for-byte).
- Visual regression for built-ins; per-backend tolerance (Vulkan/DX12/Metal).
- Sandbox abuse corpus: infinite loops, memory bombs, denied net/fs, malformed payloads; cargo-fuzz of host imports and scene-graph decoder.
- Perf budgets in CI: ms/frame for effects, per-frame scene eval cost, wasm call overhead.
- Stability policy: semver on WIT/schemas, deprecation window >= 2 minors, `cutweave plugin migrate`; index CI re-validates all registry plugins against each host RC.
- Template repos + SDK crates/npm packages (`cutweave-plugin-sdk`, `@cutweave/scene`) with scaffolding (cargo-component, wit-bindgen, esbuild).

## 13. Top risks
1. WASI 0.3/component-model async still young; non-Rust guest toolchains uneven - own versioned WIT, thin SDK, keep p2 fallback.
2. GPU DoS/hangs from WGSL cannot be fully prevented - budgets, auto-disable, separate render process.
3. Scene-fn runtime: QuickJS speed ceiling, determinism, and scope creep towards re-implementing a web renderer; authors will ask for DOM/CSS.
4. Authors tempted by per-frame CPU WASM pixel code - docs steer to L1/L1.5.
5. Licensing ambiguity must be settled (exception + CLA/DCO) before outside contributors; Remotion-derived code/API must be avoided.
6. UI plugin isolation vs usefulness; webview differences (WebView2/WKWebView/WebKitGTK).
7. Marketplace supply-chain and prompt-injection via MCP tool descriptions/AI-generated compositions; API-key handling in provider plugins.
8. Native audio plugin hosting (GUI embedding, crashes) scope creep.

## Sources
- WASI 0.3: https://wasi.dev/releases/wasi-p3 ; https://bytecodealliance.org/articles/WASI-0.3
- Wasmtime releases: https://docs.rs/crate/wasmtime-cli/43.0.0/source/RELEASES.md ; https://github.com/bytecodealliance/wasmtime/releases
- Rust wasm components goal: https://rust-lang.github.io/goals/2026/wasm-components.html
- Extism: https://github.com/extism/extism/issues/666
- Zed extensions: https://zed.dev/blog/zed-decoded-extensions ; https://zed.dev/docs/extensions/developing-extensions ; https://docs.rs/crate/zed_extension_api/latest
- VST3 MIT: https://steinbergmedia.github.io/vst3_dev_portal/pages/VST+3+Licensing/VST3+License ; https://forums.steinberg.net/t/vst-3-8-0-sdk-released/1011988 ; https://heise.de/-10963451
- clack / rack: https://docs.rs/clack-host ; https://github.com/prokopyl/clack ; https://docs.rs/crate/rack/latest
- OpenFX: https://www.cgchannel.com/2024/09/openfx-1-5-has-been-released/ ; https://openeffects.org/ ; https://openfx.readthedocs.io/en/main/ReleaseNotes/relnotes-1.5.html
- Figma sandbox: https://www.figma.com/blog/how-we-built-the-figma-plugin-system/ ; https://developers.figma.com/docs/plugins/how-plugins-run
- GPL plugins/exceptions: https://en.wikipedia.org/wiki/GPL_linking_exception ; https://www.gnu.org/licenses/gcc-exception-faq ; https://newsroom.eclipse.org/news/community-news/fsf-and-eclipse-offer-guidance-licences
- Not fetched this session ([U], prior knowledge; verify): Remotion docs/license (remotion.dev, remotion.dev/license), Motion Canvas (motioncanvas.io), Revideo (github.com/redotvideo/revideo), ISF (isf.video), DCTL, FxPlug, VS Code extension docs, Obsidian, Grafana/Backstage, Lapce, Bevy, CLAP license (github.com/free-audio/clap), Tauri capabilities docs, nih-plug, quickjs-ng/rquickjs/Boa/deno_core performance claims.
