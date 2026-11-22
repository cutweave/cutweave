# Cutweave — Tech Stack (pinned choices)

> Status: baseline 2026-11-22. Versions marked ✓ were verified by research on that date. "Pin at scaffold" means we take the latest stable release when Phase 0 starts and pin it in lockfiles.
> Upgrade cadence: Rust deps monthly (Renovate); **wgpu and Electron quarterly**, behind a golden-frame test run.
> Rationale lives in [01-architecture.md](01-architecture.md); evidence lives in [research/03-tech-stack.md](research/03-tech-stack.md).

## Engine (Rust)

| Concern | Choice | Version | Notes / alternatives rejected |
|---|---|---|---|
| Language | Rust stable, edition 2024 | pin at scaffold | `opt-level=3` for hot crates in the dev profile (pixel loops are about 20× slower in debug) |
| Async runtime | tokio | pin at scaffold | Real-time audio and render threads stay **off** tokio |
| GPU | **wgpu** + WGSL (naga) | 30.0.2 ✓ | Upgrades quarterly. Software adapters in CI: WARP, lavapipe |
| Media I/O | **ffmpeg-next** / ffmpeg-sys-next behind `MediaBackend` | 9.1.x ✓ (FFmpeg 8.1.x pinned) | rsmpeg is stale. GStreamer is heavier for scrubbing. Fallback fork: ffmpeg-the-third |
| HW decode/encode | FFmpeg hwaccels (D3D11VA/NVDEC/QSV/AMF, VideoToolbox, VA-API) | — | CPU-upload first. Zero-copy interop later, per OS **[Spike 3]** |
| Audio I/O | cpal | 0.18.x ✓ | |
| Audio DSP | custom mixer graph + fundsp (FX) | fundsp 0.23 ✓ | dasp is dead (2020) |
| Resample | rubato | 5.1 ✓ | |
| Time-stretch | signalsmith-stretch (MIT) | 0.1.x ✓ | Rubber Band (GPL) is also allowed under GPL-3.0; evaluate quality in Spike 2 |
| Loudness | ebur128 | 0.1.11 ✓ | |
| Text shaping | cosmic-text + swash | 0.19 / 0.2.x ✓ | parley is the alternative |
| Text rendering | MSDF atlas renderer (own) + Vello for complex glyphs | Vello 0.12 ✓ | skia-safe is the fallback if both fail **[Spike 4]** |
| Persistent state | imbl (persistent collections) | pin at scaffold | O(1) snapshots for render/query threads |
| Serialization & schema | serde, serde_json, **schemars** (JSON Schema), **specta** (TS types) | pin at scaffold | One source of truth for RPC, MCP and CLI types |
| DB (caches, jobs) | rusqlite (bundled SQLite) | 0.43 ✓ | |
| Local API | axum + tokio-tungstenite (WS) | pin at scaffold | Localhost only, token auth |
| MCP | **rmcp** (official Rust SDK) | 3.11.x ✓ | Spec 2026-07-28 (stateless) per research; verify in **[Spike 5]** |
| WASM plugins | **wasmtime** + WIT component model (WASI 0.2 / 0.3) | ≥ 46 ✓ | Extism is rejected (no component model) |
| JS compositions | **rquickjs** (QuickJS) + **oxc** (TS/JSX transpile) | pin at scaffold | deno_core is too heavy; Boa is slower **[Spike 4b]** |
| Secrets | keyring (OS keychain) | pin at scaffold | |
| HTTP client (AI) | reqwest (rustls) | pin at scaffold | |
| Local AI (optional plugin) | ort (ONNX Runtime) 2.x, whisper-rs (whisper.cpp), sherpa-onnx | ort 2.0.0-rc.x ✓ | Downloaded on demand. Not in the base installer except the CPU/DirectML/CoreML ONNX Runtime |
| C2PA | c2pa-rs | pin at scaffold | Phase 5 |
| Errors / logging | thiserror, anyhow (bins only), tracing + tracing-subscriber | — | Structured logs; opt-in crash reports |
| Testing | cargo-nextest, proptest, insta, criterion, cargo-fuzz | — | |
| Supply chain | cargo-deny (licenses + advisories), cargo-about (notices) | — | GPL-compatible allowlist |

## Desktop shell & UI (TypeScript)

| Concern | Choice | Version | Notes |
|---|---|---|---|
| Shell | **Electron** | latest stable, pin at scaffold | ADR-001: consistent Chromium (WebGPU, WebCodecs) on all OSes. Plan B: GPUI **[Spike 1]** |
| Native bridge | **napi-rs** v3 addon (`cutweave-bridge`) | pin at scaffold | SHM ring reader in preload |
| Packaging / updates | electron-builder + electron-updater | pin at scaffold | NSIS/MSI, DMG (notarized), AppImage/deb/rpm, Flatpak |
| UI framework | React | 19.3 ✓ | Large contributor pool; AI coding agents are fluent in it |
| Language | TypeScript (strict) | pin at scaffold | |
| Build | Vite | 8.3 ✓ | |
| State | Zustand | 5.1 ✓ | Engine is the source of truth; stores are projections |
| Components | `packages/ui` on Radix primitives + Tailwind CSS (theme generated from `design/tokens.json`) | Tailwind 4.3 ✓ | Cutweave design system, dark-first ([07](07-design-system.md)) |
| Design tokens | `design/tokens.json` → `cargo xtask tokens` (CSS vars, Tailwind `@theme`, TS, Rust) | — | Contrast check in CI |
| Fonts / icons | Instrument Sans, Bricolage Grotesque, JetBrains Mono (SIL OFL, bundled) · Lucide (ISC) + custom editor glyphs | pin at scaffold | Offline-safe |
| Component workshop | Storybook + Playwright screenshot tests + axe | pin at scaffold | Both themes per story |
| Docking | dockview | pin at scaffold | |
| Command palette | cmdk | pin at scaffold | Lists every registry command |
| Timeline | own Canvas2D virtualized renderer | — | PixiJS 8 only if benchmarks demand it |
| Preview | WebGPU canvas fed from the SHM ring | — | `VideoFrame`/`importExternalTexture` path tested in Spike 1 |
| i18n | i18next (ICU) | pin at scaffold | |
| Lint / format | Biome | 2.6 ✓ | |
| Unit tests | Vitest | 5.1 ✓ | |
| E2E | Playwright (Electron) | 1.65 ✓ | |
| Package manager | pnpm workspaces | pin at scaffold | |

## Plugin & agent-facing SDKs (MIT/Apache-2.0)

| Package | Contents |
|---|---|
| `wit/` | WIT worlds: `cutweave:effect` (metadata only; effects are WGSL), `cutweave:provider`, `cutweave:importer`, `cutweave:exporter`, `cutweave:command`, `cutweave:publisher` |
| `@cutweave/sdk` | TS SDK for UI panels (iframe RPC), typed commands client (generated) |
| `@cutweave/compose` | Scene API for compositions: `defineComposition`, `interpolate`, `spring`, easing, scene nodes, text animators |
| `cutweave-plugin` CLI subcommand | `cutweave plugin new | build | validate | pack | publish` |

## Tooling & infrastructure

| Concern | Choice |
|---|---|
| Repo | Monorepo: cargo workspace + pnpm workspace |
| CI | GitHub Actions matrix: `windows-latest`, `macos-14` (arm64), `ubuntu-24.04` (X11 and Wayland jobs). Software GPU for golden frames |
| Caching | sccache + Swatinem/rust-cache; pnpm store cache |
| Release | Signed builds; release notes generated from conventional commits; SBOM (CycloneDX) |
| Deps | Renovate (grouped, weekly) |
| Docs site | Starlight (Astro) built from `docs/` and the generated command reference |
| Issue tracking | GitHub Issues + Projects; one issue per work package (generated from [05-work-breakdown.md](05-work-breakdown.md)) |

## FFmpeg build policy

- Pinned FFmpeg 8.1.x, built in CI per OS as **shared libraries**. A GPL build (x264, x265, SVT-AV1, libvpx, libopus, libass, …) is allowed because Cutweave is GPL-3.0.
- Every release publishes the FFmpeg source tarball and configure line.
- Hardware encoders are preferred by default; software encoders are the fallback.
- Linux Flatpak uses `org.freedesktop.Platform.ffmpeg-full` where appropriate.
