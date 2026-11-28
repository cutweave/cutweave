# Phase 0 — Foundations & Spikes: Work Breakdown

> Source: [04-roadmap.md](../04-roadmap.md) Phase 0, contract: [01-architecture.md](../01-architecture.md). WP template: roadmap bottom. IDs `WP-0<epic>.<nn>`.
> Sizing: S = 1 agent-day, M = 3 (2–4). Executors: `sonnet` default, `opus` for cross-cutting/GPU/media/concurrency, `human` for decisions, legal and hardware-bound measurement.
> **Spike code is throwaway**: lives in `spikes/<spike>/` (excluded from the cargo workspace, own `Cargo.toml`/`package.json`), is never imported by `crates/`. Only reports, ADRs, numbers and learnings carry forward.

## Day-0 prerequisites (human, before any WP starts)

| # | Prerequisite | Role (owner named in the kickoff issue) | Needed by |
|---|---|---|---|
| D0.1 | Create repo `cutweave/cutweave` in the existing org github.com/cutweave (branch protection placeholder, Actions enabled, Discussions on) | `org-owner` | 0A.* |
| D0.2 | Git LFS data-pack quota sized for golden frames (>= 5 GB storage / 10 GB bandwidth per month) | `org-owner` (billing) | 1O.01 (reserve now) |
| D0.3 | Create `cutweave/cutweave-testdata` repo (Releases host the corpus assets; LFS not required) | `infra-maintainer` | 0I.01 |
| D0.4 | CI secrets/apps: Renovate app, DCO app, (later) signing secrets stubs; scoped tokens for `ffmpeg.yml` release publishing and `wbs-to-issues` | `infra-maintainer` | 0A.03-0A.07 |
| D0.5 | **Hardware owner and purchase order** for the reference machines (Win11 dGPU, Win11 iGPU, macOS M-series, Ubuntu 24.04 X11 + Wayland, plus min-OS VMs/hosts: Win10 22H2, macOS 13, Ubuntu 22.04). Long-lead item: the order must be placed on day 0 or the spike measure WPs (0C.04, 0D.02, 0E.03, 0F.03) slip | `hardware-owner` | 0I.04 |
| D0.6 | Named **legal reviewer** for the plugin exception text and codec/patent posture (counsel or an experienced OSS-licensing maintainer) | `legal-reviewer` | 0B.01 |
| D0.7 | Project domain / contact aliases (`security@`, `conduct@`) | `org-owner` | 0B.03 |

## Tolerance policy (single source for this phase)

| Comparison | Threshold |
|---|---|
| Preview (full quality) vs pre-encode export frames, same adapter | **bit-identical** |
| Regression vs stored per-adapter golden reference | **SSIM >= 0.995 AND PSNR >= 40 dB** |
| Spike/implementation quality vs an independent (ffmpeg/CPU float) reference | **PSNR > 45 dB** (and dE2000 < 2 for color) |
| Decoded lossy MP4 vs rendered frame | PSNR >= 38 dB (encoder loss allowance) |

## Phase goal

Remove the existential risks before writing product code: prove (or fall back from) the Electron preview path, audio-clock sync, decode/seek/HW accel, the wgpu compositor + color + text, the compositions runtime and the command-bus → codegen → MCP chain. Stand up the repo, CI, licensing, governance, corpus and benchmark harness so Phase 1 starts on rails.

## Exit criteria (M0, copied from roadmap)

All spikes pass or have an accepted fallback; ADRs 001/003/007/009/012 are marked Accepted; durations are re-estimated.
Spike pass criteria apply on **Windows 11, macOS arm64, Ubuntu 24.04 X11 + Wayland** (reproduced per spike below).

## Epics (one-line goals)

| Epic | Goal |
|---|---|
| **0A** Repo & CI scaffold | Cargo + pnpm monorepo per arch §3, CI matrix, lint/format, supply-chain gates, DCO/PR/CODEOWNERS, FFmpeg bundle, WBS→issues generator |
| **0B** Governance & licensing docs | GPL-3.0 + plugin exception (legal gate), MIT/Apache for SDK/WIT, community docs, agent instructions |
| **0C** Spike S1 shell/preview | Electron + Rust sidecar + SHM ring + WebGPU canvas + DOM overlay + Canvas2D timeline stress |
| **0D** Spike S2 audio clock | cpal mixer + rubato + signalsmith-stretch + ebur128; video follows audio clock |
| **0E** Spike S3 decode/seek/HW | ffmpeg-next decode, index, frame-exact seek, HW decode with CPU upload and zero-copy |
| **0F** Spike S4 compositor/color/text | wgpu render graph, linear Rgba16Float, tone-map, MSDF + Vello text, headless export |
| **0G** Spike S4b compositions | oxc → rquickjs → scene graph rendered by the S4 compositor |
| **0H** Spike S5 command bus → MCP | `#[command]` macro, schemars/specta codegen, rmcp stdio+HTTP, agent-driven edit |
| **0I** Corpus & benchmarks | Test-media manifest + fetch, reference machines, benchmark harness skeleton |
| **0J** ADRs, go/no-go, re-estimate | ADR 001–015 write-ups (014 min-OS, 015 wire protocol), M0 review, Phase 1+ re-estimate |

## Critical path

Two parallel long poles:

1. **Hardware long-lead:** D0.5 (purchase order, day 0) → `0I.02` (definitions, week 1) → `0I.04` (provisioned, runners registered) → measure WPs `0C.04 / 0D.02 / 0E.03 / 0F.03 / 0G.02` → `0J.02`. If hardware is late, measure WPs may run on developer machines that meet the 0I.02 definition (results flagged "dev-machine" and re-run on reference hardware before M0 sign-off).
2. **Build chain:** `0A.01 → 0A.02 → 0A.03 → 0A.06` (CI + FFmpeg bundle) ‖ spike builds `0C.01→0C.02/0C.06→0C.04`, `0E.01→0E.02→0E.03`, `0F.01→0F.02→0F.03→0G.01→0G.02`, `0H.01→0H.02→0H.03`, `0D.01→0D.02` → `0J.02 (spike ADRs incl. ADR-001 transport choice)` → `0J.03 (go/no-go, ADR-014)` → `0J.04 (re-estimate)`.

Longest chain: **0A.01 → 0F.01 → 0F.02 → 0F.03 (needs 0I.04) → 0G.01 → 0G.02 → 0J.02 → 0J.03 → 0J.04** (S4 feeds S4b). S1 (`0C`) is the riskiest and starts on day 1; a failed S1 triggers 0C.05 (GPUI), about +1 week. `0A.07 (wbs-to-issues)` and `0B.*` are off the critical path but must land before Phase 1 issues are generated.
Dependency-cycle rule: no WP may depend on a WP that depends on it, directly or via epics; `0A.07` lints this in CI.
Human gates: D0.* (day 0), 0B.01 (legal text), 0C.04/0D.02/0E.03/0F.03 (hardware runs), 0I.04, 0J.03, 0J.04.

---

## 0A — Repo & CI scaffold

### WP-0A.01 — Cargo workspace, toolchain, crate stubs
- **Area:** repo root, `crates/*`, `xtask/`
- **Depends on:** —
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Root `Cargo.toml` workspace (resolver 3, edition 2024) with members: every crate in arch §3: `cutweave-model, cutweave-commands, cutweave-commands-derive, cutweave-ops, cutweave-media, cutweave-cache, cutweave-render, cutweave-text, cutweave-audio, cutweave-playback, cutweave-jobs, cutweave-ai, cutweave-compose, cutweave-plugins, cutweave-interchange, cutweave-api, cutweave-engine, cutweave-cli, cutweave-bridge` + `xtask`. Each crate = `lib.rs` with `//!` doc and one smoke test (CLI crate = `main.rs`).
  - `rust-toolchain.toml` pinning stable (exact version recorded), components `rustfmt, clippy`, target `wasm32-unknown-unknown`.
  - `[workspace.lints]` (clippy pedantic subset, `unsafe_code = "deny"` except `cutweave-bridge`, `cutweave-playback`, `cutweave-audio` which `allow` with justification), `[workspace.dependencies]` skeleton, `[profile.dev.package.<hot crates>] opt-level = 3` (render, text, media, audio, cache) per tech-stack.
  - `.cargo/config.toml`, `rustfmt.toml`, `clippy.toml`, `.editorconfig`, `.gitignore`, `.gitattributes`.
  - `spikes/` directory with README ("throwaway; not in workspace") and `exclude = ["spikes"]`.
  - Top-level directories per arch §3: `schema/`, `bench/`, `spikes/`, `xtask/`, `scripts/`, `testdata/`, `docs/` (each with a README stub).
  - `justfile` with the recipes of 06-dev-process §2: `setup, dev, check, golden, bench, e2e, mcp-inspect, corpus, evals, ffmpeg-fetch`. Recipes depending on later WPs exist and print `not implemented yet (WP-...)` with exit 0; `check` is real (fmt, clippy, Biome, nextest, vitest, wasm32 check, cargo-deny, schema diff placeholder).
  - `xtask` crate (`cargo xtask <ci|check-wasm|fetch-corpus|gen>`; justfile recipes delegate to it) with `check-wasm` implemented: `cargo check --target wasm32-unknown-unknown -p cutweave-model -p cutweave-commands -p cutweave-render -p cutweave-text`.
- **Interfaces:** workspace layout (arch §3); dependency rule enforced by `cargo xtask check-deps` (fails if `cutweave-render|audio|media` depend on `cutweave-api`, or `cutweave-model` depends on anything non-wasm-safe).
- **Acceptance criteria:**
  - `just --list` shows all recipes; every top-level directory above exists (xtask test). (`just setup && just check` full-green on 3 OSes is verified in WP-0A.06, which owns FFmpeg and runs after Biome/cargo-deny exist; this avoids a dependency cycle.)
  - `cargo build --workspace`, `cargo nextest run --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` all exit 0 on the 3 OSes.
  - `cargo xtask check-wasm` exits 0.
  - `cargo xtask check-deps` exits 0 and a deliberate violation (in a test fixture) exits non-zero.
- **Tests:** per-crate smoke test; xtask unit test for dep-rule checker.
- **Out of scope:** real crate content; CI YAML (0A.03).

### WP-0A.02 — pnpm workspace, TS config, Biome
- **Area:** repo root, `apps/desktop`, `packages/*`
- **Depends on:** —
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `package.json` (private, `packageManager: pnpm@<pinned>`, engines node LTS pinned in `.nvmrc`), `pnpm-workspace.yaml` listing `apps/*`, `packages/*`.
  - Stub packages: `packages/{protocol,editor,timeline,ui,plugin-sdk,compose-runtime}` (names `@cutweave/<x>`), `apps/desktop`; each with `package.json`, `tsconfig.json` extending `tsconfig.base.json` (strict, `noUncheckedIndexedAccess`), `src/index.ts`, one Vitest test.
  - `biome.json` (Biome 2.x: format + lint, import sorting; no ESLint/Prettier). Root scripts: `pnpm lint`, `pnpm format:check`, `pnpm typecheck`, `pnpm test` (Vitest workspace), `pnpm build`.
  - `.npmrc` with `engine-strict=true`, `save-exact=true`; committed `pnpm-lock.yaml`.
- **Interfaces:** package names/paths per arch §3.
- **Acceptance criteria:**
  - `pnpm install --frozen-lockfile && pnpm lint && pnpm typecheck && pnpm test && pnpm build` exit 0 on 3 OSes.
  - Introducing a Biome violation or a TS error makes the matching script fail (verified by a CI negative test or documented manual check).
- **Tests:** one Vitest smoke per package.
- **Out of scope:** Electron, React, Vite config (Phase 1/Spike).

### WP-0A.03 — CI workflows (GitHub Actions matrix)
- **Area:** `.github/workflows/`
- **Depends on:** WP-0A.01, WP-0A.02
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `ci.yml` jobs, matrix `windows-latest`, `macos-14`, `ubuntu-24.04`: `rust` (fmt, clippy, `cargo nextest run --workspace`, `cargo xtask check-wasm`, `cargo xtask check-deps`), `ts` (`pnpm lint/typecheck/test/build`), `deny` (0A.04), `schema-snapshot` (placeholder step that runs `cargo xtask gen --check`, no-op until 1N), `docs-lint` (markdown link check).
  - Linux display matrix for GUI tests: `ubuntu-24.04` job variants `x11` (xvfb) and `wayland` (weston headless) — jobs exist and run a trivial `xdpyinfo`/`wayland-info` probe now; Electron tests wired in Phase 1.
  - Software GPU job scaffolding: install lavapipe (`mesa-vulkan-drivers`) on Linux, WARP is default on Windows runner; job prints `wgpu` adapter list via `cargo xtask gpu-info`.
  - Caching: `Swatinem/rust-cache`, `sccache`, pnpm store cache. `concurrency` cancel-in-progress per PR. Required-status list documented in `docs/ci.md`.
  - `nightly.yml` stub (cron) for soak/benchmarks (filled by 0I.03, P2).
  - **macOS Metal check:** a step on `macos-14` runs `cargo xtask gpu-info` and records whether the hosted runner exposes a usable, non-software Metal adapter (and whether a wgpu headless render succeeds). Result written to `docs/ci.md`. **Fallback:** if unusable, macOS golden-frame/GPU jobs are routed only to the self-hosted `ref-mac-m1` runner (0I.04) and the hosted macOS job runs CPU-only tests.
  - **Linux release-engine job** `engine-linux (ubuntu-22.04 container)`: builds the engine binary inside an Ubuntu 22.04 container (glibc 2.35) to honour ADR-014's Ubuntu 22.04 floor; the standard `rust (ubuntu-24.04)` test job is unchanged.
- **Interfaces:** job names are stable (used as branch-protection required checks): `rust (<os>)`, `ts (<os>)`, `deny`, `wasm-check`, `schema-snapshot`.
- **Acceptance criteria:**
  - A PR with a trivial change shows all required jobs green on a fork run; median PR CI wall time < 15 min with warm cache (recorded in the PR).
  - Intentionally failing clippy / Biome / wasm-check makes the respective job red (screenshots or run links in PR).
  - `cargo xtask gpu-info` lists a software adapter on Linux and Windows runners, and `docs/ci.md` states the macOS Metal verdict with the run link.
  - `ldd` on the container-built engine binary shows max required glibc symbol version <= 2.35 (script check).
- **Tests:** workflow validated with `actionlint` in CI.
- **Out of scope:** release/signing workflows (P7), real GPU runners.

### WP-0A.04 — Supply chain: cargo-deny, Renovate, license headers
- **Area:** `deny.toml`, `renovate.json`, `REUSE.toml`, `.github/`
- **Depends on:** WP-0A.01
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `deny.toml`: `[licenses]` allowlist GPL-3.0-compatible (MIT, Apache-2.0, BSD-2/3, ISC, MPL-2.0, Zlib, Unicode-3.0, GPL-3.0-or-later, LGPL-*, CC0-1.0); deny AGPL, SSPL, proprietary, non-commercial; `[advisories]` deny vulnerabilities, warn unmaintained; `[bans]` multiple-versions = warn, explicit ban list note for license traps in arch §13 (crate-level only: none yet); `[sources]` crates.io only.
  - `renovate.json`: weekly grouped PRs; groups `rust-minor`, `ts-minor`; **wgpu and Electron** in a separate quarterly group (`schedule: ["every 3 months on the first day of the month"]`) with label `needs-golden-run`; automerge disabled; `rangeStrategy: pin`.
  - SPDX header policy (REUSE): `REUSE.toml` + `cargo xtask check-headers` verifying `SPDX-License-Identifier` in every `.rs/.ts/.tsx/.wgsl/.wit` (GPL-3.0-or-later default; MIT OR Apache-2.0 under `wit/`, `packages/plugin-sdk`, `packages/compose-runtime`).
  - `pnpm audit --audit-level high` step in CI `deny` job.
- **Interfaces:** CI job `deny`.
- **Acceptance criteria:**
  - `cargo deny check` exits 0 on the clean tree; adding a dev-dependency with AGPL license in a scratch branch makes it fail (documented).
  - `cargo xtask check-headers` exits 0; removing a header from one file fails it.
  - `renovate-config-validator renovate.json` exits 0.
- **Tests:** header-checker unit tests.
- **Out of scope:** `cargo-about` notices (P7), SBOM.

### WP-0A.05 — DCO, PR template, CODEOWNERS, issue templates, labels
- **Area:** `.github/`
- **Depends on:** —
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - DCO check via the DCO GitHub App config (`.github/dco.yml`, `require.members: false`) **plus** a fallback workflow step that greps `Signed-off-by:` on every PR commit.
  - `pull_request_template.md`: WP id, linked issue, acceptance-criteria checklist (copied from WP), "schema snapshots updated?", "ADR needed?", DCO reminder, AI-agent disclosure line.
  - `CODEOWNERS`: `/crates/cutweave-commands*`, `/crates/cutweave-model` and `/docs/adr` → maintainers; `/LICENSE*`, `/wit`, `/packages/plugin-sdk` → maintainers + legal-review group; `/.github` → maintainers.
  - Issue templates: `work-package.yml` (fields mirror WP template: area, depends, size, executor, scope, acceptance), `bug.yml`, `feature.yml`. `labels.yml` (+ sync workflow) with `phase:P0..P7`, `epic:<code>` for all epics in 05-work-breakdown.md, `size:S|M`, `exec:sonnet|opus|human`, `spike`, `needs-golden-run`.
  - Branch protection settings documented in `docs/ci.md` (required checks from 0A.03, linear history, signed-off commits).
- **Interfaces:** label vocabulary (consumed by the issue-generation script in the integrator's process).
- **Acceptance criteria:**
  - A test PR with an unsigned-off commit is blocked by the DCO check; with `git commit -s` it passes.
  - `labels.yml` contains exactly the epic codes listed in 05-work-breakdown.md (script check).
- **Tests:** `cargo xtask check-labels` compares labels.yml against the epic table.
- **Out of scope:** automating issue creation from WBS markdown.

### WP-0A.06 — FFmpeg 8.1.x shared-bundle CI build + `just ffmpeg-fetch`
- **Area:** `.github/workflows/ffmpeg.yml`, `ci/ffmpeg/`, `justfile`, `xtask`
- **Depends on:** WP-0A.02, WP-0A.03, WP-0A.04
- **Size:** M
- **Executor:** opus (build-system subtleties)
- **Scope:**
  - Workflow + scripts (`ci/ffmpeg/build-{windows,macos,linux}.sh`) building the **pinned FFmpeg 8.1.x as shared libraries per OS** (win-x64, macOS arm64, Linux x64). **The Linux bundle is built in an Ubuntu 22.04 container (glibc 2.35)** to honour ADR-014. GPL build (`--enable-gpl --enable-version3`) with **required** libraries: `libx264, libx265, libsvtav1, libvpx, libopus, libmp3lame, libvidstab, libzimg, libsoxr, libass`; native encoders `prores_ks`, `dnxhd`, `gif` available; per-OS hwaccels: Windows `d3d11va, dxva2, nvdec/nvenc (ffnvcodec), amf, qsv(libvpl)`, macOS `videotoolbox`, Linux `vaapi, nvdec/nvenc (ffnvcodec), qsv(libvpl)`. Version, source tarball sha256 and the full `configure` line are recorded in `ci/ffmpeg/VERSION.toml`.
  - Publishes each bundle (headers, import libs, shared libs, `configure.txt`, source tarball, license) as a GitHub Release asset with sha256 manifest `ci/ffmpeg/bundles.toml`; the source tarball and configure line are published alongside (GPL compliance, arch §21).
  - `just ffmpeg-fetch` (delegates to `cargo xtask fetch-ffmpeg`): downloads the pinned bundle for the host OS into `.ffmpeg/`, verifies sha256, emits an env file setting `FFMPEG_DIR`; idempotent. Developers may instead use brew/distro `libav*-dev` for quick hacking (06-dev-process §2): documented, not CI-supported.
  - CI jobs needing FFmpeg restore the bundle from a cache keyed on `bundles.toml`.
- **Interfaces:** `bundles.toml` schema; `FFMPEG_DIR` contract consumed by 1D.01 and the S3 spike.
- **Acceptance criteria:**
  - The workflow produces bundles for 3 OSes; `ffmpeg -version` from each prints the pinned 8.1.x and `--enable-gpl`; after `just ffmpeg-fetch` on a clean machine of each OS a 10-line Rust build script links `avformat` successfully.
  - **Configure/encoder check (script, all 3 OS):** `configure.txt` contains every required `--enable-lib*` flag above, and `ffmpeg -hide_banner -encoders` lists `libx264 libx265 libsvtav1 libvpx-vp9 libopus libmp3lame prores_ks dnxhd gif`; `-filters` lists `vidstabdetect vidstabtransform zscale`; `-buildconf` shows `--enable-libsoxr --enable-libass`; hwaccel flags per OS are present (`-hwaccels`).
  - Release assets include the source tarball and `configure.txt` (script check).
  - `just setup && just check` is green on the 3 OSes using the fetched bundle.
  - Linux bundle `ldd`/`objdump` shows glibc requirement <= 2.35.
- **Tests:** smoke link test in CI.
- **Out of scope:** release signing, the P7 FFmpeg source-offer page, Flatpak ffmpeg-full.

### WP-0A.07 — `scripts/wbs-to-issues`: generate GitHub issues from the WBS
- **Area:** `scripts/wbs-to-issues/`, `.github/workflows/wbs-lint.yml`
- **Depends on:** WP-0A.02, WP-0A.05
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - TypeScript CLI (`pnpm wbs-issues <cmd>`): parse every `docs/wbs/*.md` `### WP-<id> — <title>` block into a record `{id, phase, epic, title, area, depends[], size, executor, scope, interfaces, acceptance, tests, out_of_scope}` (fails with file:line on any missing template field or malformed size/executor).
  - **Dependency rule:** a dependency written as an epic code (e.g. `E1B`, or "epic 1B") means *all WPs of that epic merged* unless refined to WP ids; the generator **expands epic deps into explicit WP lists** and renders them as task-list references in the issue body.
  - Commands: `lint` (duplicate ids, unknown deps, **dependency cycles**, ids not matching `WP-<phase><epic>.<nn>`, phase/epic labels exist in `labels.yml`), `plan` / `--dry-run` (prints creates/updates/no-ops), `sync` (idempotent create/update via `gh`/Octokit: issue keyed by hidden marker `<!-- wp-id: WP-1B.03 -->`; sets labels `phase:*`, `epic:*`, `size:*`, `exec:*`; sets GitHub Project v2 fields Phase/Epic/Size/Executor/Status; links dependencies as "blocked by" task-list; never closes or deletes issues; updates body only when the rendered hash changed).
  - `wbs-lint` CI workflow runs `lint` on every PR touching `docs/wbs/**`.
- **Interfaces:** CLI above; rendered issue body format documented in `scripts/wbs-to-issues/README.md`.
- **Acceptance criteria:**
  - `pnpm wbs-issues lint` over the current `docs/wbs/*.md` exits 0 (no unknown ids, no cycles); an injected cycle or unknown dep fixture exits non-zero.
  - `plan --dry-run` over P0 and P1 reports exactly as many creates as there are `### WP-` headings (script cross-check with `grep -c`).
  - Running `sync` twice against a mock GitHub API server yields 0 creates and 0 updates on the second run; changing one WP's acceptance text yields exactly 1 update.
  - Epic-dependency expansion test: `E1B` expands to the full list of 1B WPs.
- **Tests:** Vitest (parser, expander, cycle detector) + mock-API integration.
- **Out of scope:** GitHub Project board automation beyond field setting; two-way sync.

---

## 0B — Governance & licensing docs

### WP-0B.01 — LICENSE (GPL-3.0) + plugin exception text  *(legal gate)*
- **Area:** `LICENSE`, `LICENSE-PLUGIN-EXCEPTION.md`, `docs/licensing.md`
- **Depends on:** —
- **Size:** S
- **Executor:** human (the `legal-reviewer` from D0.6 reviews; agent drafts, human signs off)
- **Scope:**
  - `LICENSE` = verbatim GPL-3.0 text (SPDX `GPL-3.0-or-later` header policy).
  - Draft the **plugin exception** per ADR-011: a GPLv3 §7 additional permission allowing plugins of any license that interact with Cutweave **only** through published interfaces (`wit/`, `@cutweave/sdk`, `@cutweave/compose`, the plugin manifest, the documented command/MCP APIs); defines "published interface", states that linking Cutweave internals (crates) is not covered, preserves the exception for downstream forks at their option (removable per §7).
  - `docs/licensing.md`: matrix of directory → license (core GPL-3.0-or-later; `wit/`, `packages/plugin-sdk`, `packages/compose-runtime` MIT OR Apache-2.0), model-weights policy, FFmpeg GPL build note (arch §21), DCO rationale.
  - Open a tracking issue "Legal review: plugin exception + codec patent posture" with the exact text.
- **Interfaces:** none (docs).
- **Acceptance criteria:**
  - `LICENSE` sha256 equals the canonical GPL-3.0 text (hash recorded in `docs/licensing.md`).
  - Exception text reviewed and approved in the PR by a human maintainer; if outside counsel is not available this is recorded as "maintainer-approved, counsel review pending before 1.0" (arch §21).
  - The licensing matrix covers every top-level directory in arch §3.
- **Tests:** `cargo xtask check-headers` (0A.04) passes with the matrix.
- **Out of scope:** third-party notices, trademark policy.

### WP-0B.02 — MIT/Apache-2.0 licensing for SDK, WIT, compose-runtime
- **Area:** `wit/`, `packages/plugin-sdk`, `packages/compose-runtime`
- **Depends on:** WP-0A.02, WP-0B.01
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Add `LICENSE-MIT` and `LICENSE-APACHE` (canonical texts) in each of the three directories; `package.json` `"license": "MIT OR Apache-2.0"`; `wit/README.md` stating the license; placeholder `wit/cutweave-provider/world.wit` etc. are NOT created (Phase 6) — only the directory + README + license.
  - `NOTICE` for the Apache part. CONTRIBUTING pointer that contributions to those dirs are under both licenses.
  - `cargo xtask check-headers` extended: files under these dirs must carry `MIT OR Apache-2.0`.
- **Interfaces:** none.
- **Acceptance criteria:**
  - `pnpm -r exec node -e "…license…"` (script in xtask) asserts the three package licenses; `cargo xtask check-headers` exits 0.
  - Root `LICENSE` is unchanged vs 0B.01 hash.
- **Tests:** header check.
- **Out of scope:** actual SDK code.

### WP-0B.03 — CONTRIBUTING, CODE_OF_CONDUCT, SECURITY, governance stub
- **Area:** repo root, `docs/`
- **Depends on:** WP-0A.05
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `CONTRIBUTING.md`: dev setup per OS (rust toolchain, node, pnpm, ffmpeg dev libs placeholder → filled in 1D.01), WP workflow (issue per WP, worktree + branch `wp/<id>`), DCO (`git commit -s`), AI-assisted contribution policy (disclosure, human review required, no secrets/licensed code), commit style (conventional commits), how to run all checks (`cargo xtask ci`).
  - `CODE_OF_CONDUCT.md`: Contributor Covenant 2.1 verbatim with contact placeholder `conduct@<project-domain>` flagged TODO for maintainer.
  - `SECURITY.md`: private vulnerability reporting via GitHub advisories, 90-day disclosure, supported versions table (pre-1.0: latest only), scope (local API, plugin sandbox, key handling).
  - `GOVERNANCE.md` stub: maintainers list placeholder, decision process (ADR for architecture-level changes per brief), RFC process placeholder (full in 7E), release owner role.
- **Interfaces:** none.
- **Acceptance criteria:** markdown link check (CI `docs-lint`) passes; CONTRIBUTING commands run verbatim on a clean clone (reviewer checklist item); no TODO tokens other than the single flagged contact placeholder.
- **Tests:** `docs-lint` job.
- **Out of scope:** full governance, trademark, funding.

### WP-0B.04 — CLAUDE.md / AGENTS.md and ADR template
- **Area:** `CLAUDE.md`, `AGENTS.md`, `docs/adr/`
- **Depends on:** WP-0A.01, WP-0A.02
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `AGENTS.md` (canonical) and `CLAUDE.md` (imports `@AGENTS.md` + Claude-specific notes) for coding agents: repo map (arch §3), dependency rule, how to build/test per crate (`cargo nextest run -p <crate>`, `pnpm --filter @cutweave/<pkg> test`), the WP workflow (read WP in `docs/wbs/`, stay within `Area`, never edit generated files — regenerate with `cargo xtask gen`), snapshot update procedure (`cargo insta review`), forbidden actions (no `unsafe` outside allowed crates, no new deps without noting in PR, no network in tests, no committing media), determinism rules (no wall clock/unseeded RNG in render/commands), error-handling conventions (`thiserror` in libs, `anyhow` only in bins), logging via `tracing`.
  - `docs/adr/0000-template.md` (MADR style: Context, Decision, Status, Consequences, Alternatives, Evidence/links to spike report, Supersedes) and `docs/adr/README.md` index table (ADR 001–015 rows, status column).
- **Interfaces:** ADR template used by 0J.
- **Acceptance criteria:** a fresh sonnet agent given only `AGENTS.md` + WP-1A.01 text can state the correct crate, test command and forbidden actions (reviewer spot check recorded in PR); template contains all 6 section headings; README index lists 15 ADRs with current statuses from arch §23.
- **Tests:** `docs-lint`.
- **Out of scope:** per-crate AGENTS files, Agent Kit skills (P3/P6).

---

## 0C — Spike S1: shell & preview path  (pass criteria from roadmap)

> **Pass:** 60 fps sustained, < 2 dropped frames/min, scrub-to-pixel p95 < 80 ms, transport CPU overhead < 15%, Canvas2D timeline ≥ 55 fps, no overlay/z-order issues — on Win11, macOS arm64, Ubuntu 24.04 X11 + Wayland. **On fail:** build same demo in GPUI, compare, decide ADR-001.

### WP-0C.01 — S1 build: Electron shell + Rust sidecar + SHM ring + napi reader
- **Area:** `spikes/s1-preview/{engine,bridge,desktop}`
- **Depends on:** WP-0A.01, WP-0A.02
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Rust sidecar generating synthetic 1080p60 frames (moving pattern with frame counter + embedded pts, NV12 and RGBA8 variants) via wgpu readback (software fine) into a **named shared-memory ring** per arch §10: 4–6 slots, header = atomic write index, per-slot `pts,w,h,format,rev,seq`, audio-clock word written continuously (fake clock).
  - napi-rs v3 addon `bridge` mapping the ring and exposing slots as `ArrayBuffer`s (zero-copy) + `readHeader()`.
  - **Transport (a):** the addon is loaded in a **non-sandboxed preload of the main editor window only** (`sandbox:false` for that preload; `contextIsolation:true`, `nodeIntegration:false`; narrow `contextBridge` API; content served from a local `app://` protocol only; strict CSP; plugin iframes get no preload). Rationale: Electron sandboxed preloads cannot load native modules. All other windows stay `sandbox:true`.
  - Electron main spawns the sidecar and passes the token/ring name via the preload bridge only (never URL/DOM).
  - (Transport (b) is WP-0C.06.)
  - WS channel for transport commands (play/pause/seek) from renderer to sidecar.
- **Interfaces:** ring header layout documented in `spikes/s1-preview/RING.md` (feeds 1H.02).
- **Acceptance criteria:**
  - Transport (a) works with `contextIsolation:true`, `nodeIntegration:false`: a test asserts `typeof require === 'undefined'` and `typeof process === 'undefined'` in the renderer main world while `window.bridge.getLatestFrame()` returns frames, and that a second (sandboxed) `BrowserWindow` has no `window.bridge`.
  - On the dev machine, 60 s run produces frames with monotonically increasing `seq` and zero torn frames (checksum embedded in frame verified in renderer).
  - Sidecar crash (kill -9) is detected by main in < 1 s; ring is cleaned up (no leaked shm object on Linux/macOS; handle closed on Windows).
- **Tests:** Rust unit test for ring writer/reader wrap-around; Node test for addon reader.
- **Out of scope:** WebGPU drawing (0C.02), transport (b) (0C.06), measurement (0C.04).

### WP-0C.02 — S1 build: WebGPU canvas, DOM overlay, scrub bar, live resize
- **Area:** `spikes/s1-preview/renderer`
- **Depends on:** WP-0C.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - React/Vite renderer: WebGPU canvas presenting the newest slot with `pts ≤ audio clock` in `requestAnimationFrame`, via **both** upload paths behind a toggle: `queue.writeTexture` and `VideoFrame`→`importExternalTexture`.
  - DOM overlay above canvas: draggable/resizable transform box (pointer events), safe-area guides, scrub bar sending seek requests (latest-wins).
  - Live window resize: preview canvas pixel size changes are sent to the sidecar which re-renders at the new size (arch §10 step 1).
  - Instrumentation: per-frame present timestamps, dropped-frame counter, scrub-to-pixel latency (seek sent → first frame with matching pts presented, using a pts-stamped glyph), CPU% of sidecar + main + renderer processes sampled every 1 s to a JSON log.
  - Chromium flags required for WebGPU on Linux (Vulkan, Wayland ozone) documented in `spikes/s1-preview/FLAGS.md`.
- **Interfaces:** metrics JSON schema `{ts, fps, dropped, scrub_ms[], cpu{engine,main,renderer}}`.
- **Acceptance criteria:**
  - Overlay box stays pixel-aligned over the canvas during resize and while playing (visual check + automated DOM-rect vs canvas-rect assertion every second).
  - Both upload paths run for 60 s on the dev machine and emit metrics JSON.
- **Tests:** Playwright-Electron smoke launching the spike and asserting metrics file exists.
- **Out of scope:** timeline stress (0C.03).

### WP-0C.03 — S1 build: Canvas2D timeline stress test
- **Area:** `spikes/s1-preview/timeline-stress`
- **Depends on:** WP-0A.02
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Standalone page (runs in Electron renderer and in plain Chromium) with a virtualized Canvas2D timeline: **5,000 clips, 50,000 keyframes, 200 thumbnails** (bitmap tiles from generated PNGs); pan/zoom with wheel + drag; visible-range culling; spatial index (e.g. interval tree / uniform grid) for hit-testing; marquee select.
  - Scripted interaction driver (pan/zoom sweep for 30 s) recording frame times via `requestAnimationFrame` deltas and `performance.measureUserAgentSpecificMemory` where available.
  - Emits the same metrics JSON (fps p50/p5, longest frame).
- **Interfaces:** none carried forward except learnings → feed `packages/timeline` (1M.05).
- **Acceptance criteria:** on the dev machine the sweep reports ≥ 55 fps (p5 of frame rate), hit-test p95 < 2 ms; results JSON committed under `spikes/s1-preview/results/`.
- **Tests:** none beyond the driver.
- **Out of scope:** interaction state machine, thumbnails from real media.

### WP-0C.04 — S1 measure on all platforms + report
- **Area:** `spikes/s1-preview/results`, `docs/spikes/S1-report.md`
- **Depends on:** WP-0C.02, WP-0C.03, WP-0C.06, WP-0I.04 (or developer machines meeting the 0I.02 definition)
- **Size:** M
- **Executor:** human (operates hardware; agent assists with analysis script)
- **Scope:**
  - Run 0C.02 and 0C.03 for 10 min each on Windows 11, macOS arm64, Ubuntu 24.04 X11, Ubuntu 24.04 Wayland on the reference machines from 0I.02 (provisioned in 0I.04); `cargo xtask bench-report` aggregates JSON into a table.
  - Evaluate each criterion: 60 fps sustained; dropped frames/min (< 2); scrub-to-pixel p95 (< 80 ms); transport CPU (< 15% of one core-equivalent, define in report); timeline ≥ 55 fps; overlay/z-order issues (none). Compare `writeTexture` vs `importExternalTexture`, **and transport (a) non-sandboxed-preload addon vs transport (b) utilityProcess + MessagePort** (frame-delivery latency, copy cost, CPU, jitter; security posture noted).
  - Write `docs/spikes/S1-report.md`: raw tables per OS, verdict PASS/FAIL per criterion, recommended upload path, **recommended transport (a) or (b)**, required Chromium flags, known OS quirks, recommendation for ADR-001.
- **Interfaces:** report is the evidence for ADR-001 (0J.02), which **records the transport choice** (a vs b), and `RING.md` for 1H.02/1H.03.
- **Acceptance criteria:**
  - Report contains a verdict line `S1: PASS | PASS-WITH-FALLBACK | FAIL` and a numeric cell for every criterion × every OS (16 cells + timeline).
  - If any criterion fails on any OS, a ticket/WP-0C.05 is activated (below) and the report says so explicitly.
- **Tests:** n/a.
- **Out of scope:** fixing failures (0C.05 or Phase 1 backlog).

### WP-0C.06 — S1 build: transport (b) utilityProcess/main + MessagePort
- **Area:** `spikes/s1-preview/desktop`
- **Depends on:** WP-0C.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Implement the alternative to transport (a): the napi addon runs in an Electron `utilityProcess` (or the main process); frames reach the sandboxed renderer through a transferred `MessagePort` (transferable `ArrayBuffer`s from a pre-allocated pool, or `SharedArrayBuffer` if cross-origin isolation can be enabled for the `app://` origin — measure both). All windows are `sandbox:true` with a **no-native-module** preload exposing only the port handoff.
  - Same metrics output and same renderer present loop as 0C.02 so results are comparable; add an A/B switch `?transport=a|b`.
  - Record copy count per frame, per-frame delivery latency (ring write → renderer receipt), and CPU of the utility process.
- **Interfaces:** metrics JSON as 0C.02 plus `transport` field.
- **Acceptance criteria:**
  - Transport (b) runs 60 s at 1080p60 on the dev machine producing metrics JSON; a test proves the renderer is sandboxed (`process.sandboxed === true`) and has no native-module access.
  - A/B comparison table (a vs b) is generated by `cargo xtask bench-report`.
- **Tests:** Playwright-Electron smoke for both transports.
- **Out of scope:** measurement on all OSes (0C.04), product code.

---

### WP-0C.05 — S1 fallback: GPUI comparison demo (conditional)
- **Area:** `spikes/s1-gpui`
- **Depends on:** WP-0C.04 (only runs if S1 verdict ≠ PASS)
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Build the equivalent demo in GPUI: 1080p60 frames in a native view, a draggable transform box, scrub bar, live resize; measure the same metrics on the failing OS(es).
  - Compare against Electron on: criteria numbers, dev effort for plugin-iframe UI, contributor/agent friendliness, Linux Wayland behaviour; write `docs/spikes/S1-gpui-comparison.md` with a recommendation.
- **Interfaces:** none.
- **Acceptance criteria:** comparison doc contains numbers for the same criteria as 0C.04 and an explicit recommendation (stay Electron with mitigations / switch to GPUI / hybrid); decision recorded in ADR-001 by 0J.02.
- **Tests:** n/a.
- **Out of scope:** any product code. Skipped (closed as "not needed") if S1 passes.

---

## 0D — Spike S2: audio clock & A/V sync

> **Pass:** drift < 1 frame / 30 min; 0 underruns at 256-sample buffer; A/V offset < 40 ms (flash/click test). **On fail:** larger buffers + latency compensation; evaluate Rubber Band.

### WP-0D.01 — S2 build: cpal mixer, stretch, loudness, clock-following video
- **Area:** `spikes/s2-audio`
- **Depends on:** WP-0A.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - cpal 0.18 output stream (256-sample buffer requested; record actual), 8 simultaneous decoded tracks (pre-decoded WAV/FLAC fixtures from 0I.01 or synthesized), per-track gain/pan, `rubato` resample 44.1k/96k → device rate, `signalsmith-stretch` at 0.5×, 1.0×, 2.0×, `ebur128` momentary/integrated loudness on master.
  - Real-thread discipline: lock-free SPSC ring buffers from worker threads, no allocation/locks in the callback (verified with `assert_no_alloc` in debug).
  - Master clock = samples played (+ device latency reported by cpal); minimal video window (winit or the S1 renderer) generating frames whose presentation follows the clock; test signal mode: video flashes white on frame N while audio plays a click at the same timeline time.
  - Logging: callback underrun count, clock drift vs `Instant`, reported latency.
- **Interfaces:** learnings feed `cutweave-audio` (1G) and `cutweave-playback` (1H.01).
- **Acceptance criteria:**
  - 10-min run at 256-sample buffer on the dev machine: 0 underruns with 8 tracks + stretch 0.5×–2×.
  - `ebur128` integrated loudness of a −23 LUFS reference tone within ±0.1 LU.
  - Flash/click harness emits a capture file (for 0D.02).
- **Tests:** unit tests for ring buffer; stretch ratio sanity (output length within 0.5% of expected).
- **Out of scope:** effects, ducking, real file decoding.

### WP-0D.02 — S2 measure (30-min drift, A/V offset) + report
- **Area:** `docs/spikes/S2-report.md`
- **Depends on:** WP-0D.01, WP-0I.04 (or developer machines meeting the 0I.02 definition)
- **Size:** M
- **Executor:** human (hardware; loopback/camera measurement) with sonnet-written analysis script
- **Scope:**
  - Run 30-min soak on each reference machine/OS (Win11 WASAPI, macOS CoreAudio, Linux PulseAudio/PipeWire): drift = (audio clock − wall clock) at end; underruns; reported latency.
  - A/V offset: measure flash→click offset with a phone high-speed camera + audio, or loopback (photodiode/mic) — procedure written in the report; accept ±1 frame@60 measurement error.
  - If any criterion fails: retry with 512/1024 buffers + latency compensation (apply `cpal` reported latency) and with Rubber Band (GPL, via FFI) for stretch quality/CPU comparison; document.
  - Report: verdict per OS, recommended default buffer size, latency-compensation rule, stretch library choice (signalsmith vs Rubber Band: CPU %, subjective artifacts at 0.5×/2×).
- **Interfaces:** evidence for `cutweave-audio` design (1G.01/1G.02); no dedicated ADR (arch §23 has none for audio), verdict is recorded in the 0J.03 go/no-go review.
- **Acceptance criteria:** report has numeric drift (frames/30 min), underrun count, A/V offset ms on each OS; verdict `PASS` or `PASS-WITH-FALLBACK (<setting>)`; fallback settings are reproducible via a documented flag.
- **Tests:** n/a.
- **Out of scope:** fixing platform-specific audio bugs.

---

## 0E — Spike S3: decode, seek & HW accel

> **Pass:** 100% frame-exact on 500 random seeks; 4K long-GOP scrub p95 < 150 ms (proxy < 33 ms); path (a) CPU upload works everywhere. **On fail:** ship (a) only; defer zero-copy.

### WP-0E.01 — S3 build: ffmpeg-next decode, keyframe index, frame-exact seek
- **Area:** `spikes/s3-decode`
- **Depends on:** WP-0A.01, WP-0I.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Use the pinned bundle from WP-0A.06 (`just ffmpeg-fetch`) when available, otherwise brew/distro libs for quick hacking (06-dev-process §2); document how `ffmpeg-sys-next` links them; record the working recipe per OS in `spikes/s3-decode/SETUP.md` (feeds 1D.01). Verify ffmpeg-next version/FFmpeg ABI compatibility; if broken try `ffmpeg-the-third` and report.
  - Decode corpus items from 0I.01: H.264 4K30 long-GOP, HEVC 10-bit, ProRes, VFR phone clip (rotated).
  - Build frame index (pts → keyframe/GOP map) by packet scan; frame-exact seek = keyframe seek + decode-forward to exact pts using the index; VFR handled through index (never constant fps).
  - Ground truth via `ffprobe -show_frames` pts list; harness does 500 random seeks per file and compares returned frame pts and a CRC of the Y plane against `ffmpeg -vf select` reference output.
  - Intra-codec proxy (ProRes Proxy / MJPEG) generated by script for the scrub comparison.
- **Interfaces:** learnings → `MediaBackend` trait shape (1D.01).
- **Acceptance criteria:**
  - Harness prints `frame-exact: 500/500` for every corpus file (CPU decode) on the dev machine.
  - Scrub benchmark prints p50/p95 for 4K long-GOP and proxy.
- **Tests:** the harness itself (cargo test with `--ignored` flag for corpus-dependent tests).
- **Out of scope:** HW decode (0E.02), encode.

### WP-0E.02 — S3 build: HW decode (CPU upload) and zero-copy interop
- **Area:** `spikes/s3-decode`
- **Depends on:** WP-0E.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - (a) HW decode via FFmpeg hwaccel (D3D11VA on Windows, VideoToolbox on macOS, VA-API on Linux; NVDEC where present), `av_hwframe_transfer_data` to NV12/P010 in CPU memory, upload as wgpu textures and convert in a shader (reuse S4 shader if available, otherwise a minimal one). Fallback to SW decode when hwaccel init fails; frame-exactness re-verified on HW path (HW decoders can differ at bit level: compare pts, and PSNR > 50 dB vs SW frame).
  - (b) zero-copy experiment behind a cargo feature: DX12 shared handle (Windows), IOSurface→Metal (macOS), dmabuf→Vulkan (Linux). Time-boxed to 4 agent-days; record "works / works-with-caveats / not viable" per OS with evidence.
  - Metrics: per-frame decode→texture-ready latency, CPU%, GPU%.
- **Interfaces:** learnings → 1D.04 (HW decode).
- **Acceptance criteria:**
  - Path (a) decodes + uploads + renders H.264 4K30 and HEVC 10-bit on every OS available to the dev machine; failures produce clean SW fallback (test with hwaccel forced off).
  - Table of (b) status per OS is present with at least one measured number or a concrete blocker per OS.
- **Tests:** harness extended; PSNR comparison HW vs SW.
- **Out of scope:** production error handling.

### WP-0E.03 — S3 measure on all platforms + report
- **Area:** `docs/spikes/S3-report.md`
- **Depends on:** WP-0E.02, WP-0I.04 (or developer machines meeting the 0I.02 definition)
- **Size:** S
- **Executor:** human (hardware runs; agent writes aggregation)
- **Scope:**
  - Run the harness on Win11, macOS arm64, Ubuntu 24.04 (X11 + Wayland where VA-API differs) reference machines; include an NVIDIA-equipped Windows machine if available.
  - Verdict per criterion: 500/500 frame-exact (all files, all OS, SW and HW), 4K long-GOP scrub p95 < 150 ms, proxy p95 < 33 ms, path (a) works everywhere; zero-copy status per OS.
  - Recommend ADR-003 text: FFmpeg via `ffmpeg-next` behind `MediaBackend`, CPU upload first; zero-copy deferred or scheduled, with FFmpeg acquisition recipe and ffmpeg-next vs ffmpeg-the-third verdict.
- **Interfaces:** evidence for ADR-003.
- **Acceptance criteria:** numeric cells for each criterion × OS; verdict `PASS | PASS-WITH-FALLBACK (a-only)`; FFmpeg dev setup recipe linked for each OS.
- **Tests:** n/a.
- **Out of scope:** production decoder design.

---

## 0F — Spike S4: compositor, color, text

> **Pass:** 1080p60 at < 8 ms GPU on an iGPU; PSNR > 45 dB vs an independent reference (the phase's tolerance policy); ΔE < 2; crisp 4K text; 1000 animated glyphs < 1 ms. **On fail:** skia-safe evaluation.

### WP-0F.01 — S4 build: render graph, linear compositor, LUT, tone-map, headless export
- **Area:** `spikes/s4-render`
- **Depends on:** WP-0A.01, WP-0I.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - wgpu (pin 30.0.x) render graph (DAG of passes, simple cache keys) compositing **3 layers**, **2 blend modes** (normal, multiply), a **dissolve** transition, a `.cube` 3D **LUT** pass, in **linear light `Rgba16Float`**; YUV→RGB with matrix/range/transfer from tags; **PQ→SDR tone-map** (BT.2390-style) for HDR10 input frames.
  - Headless mode (`--headless`) on software adapters (WARP, lavapipe) and real GPUs; reads back to PNG/EXR for comparison.
  - Reference generation: independent CPU float implementation (Python/numpy script or Rust) of the same scene → reference images; PSNR and ΔE2000 comparison tool.
  - GPU timing via timestamp queries; scene for timing = 1080p60, 3 layers + 2 effects (blur-lite + LUT) on iGPU.
  - Headless export: render 300 frames, pipe raw frames to `ffmpeg` CLI to an MP4 (smoke only).
- **Interfaces:** learnings → `cutweave-render` (1F), color pipeline notes `docs/spikes/S4-color-notes.md`.
- **Acceptance criteria:**
  - Headless run produces reference-comparison output printing PSNR and ΔE for each test scene.
  - Timing table printed for each available adapter.
  - MP4 produced plays (ffprobe ok, 300 frames).
- **Tests:** comparison tool unit tests (identical images → ∞ dB, ΔE = 0).
- **Out of scope:** text (0F.02), production architecture.

### WP-0F.02 — S4 build: text — animated caption via MSDF atlas and Vello
- **Area:** `spikes/s4-text`
- **Depends on:** WP-0F.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `cosmic-text` shaping (Latin + one RTL + one CJK sample + emoji) → glyph runs → **MSDF atlas** renderer (own, msdfgen-compatible generation using `fdsm`/`msdfgen` crate or offline atlas) with fill, outline, shadow, per-word **pop** scale animation and **karaoke** highlight sweep; and a second path rendering glyphs via **Vello 0.12**.
  - Scenes: (1) animated caption on 1080p; (2) same at 4K with 200 px glyphs for crispness; (3) 1000 animated glyphs (per-glyph transform/color) cost.
  - Measure CPU time for layout+instance buffer build per frame, and GPU time per frame, for MSDF vs Vello.
  - Crispness metric: render at 4K, compare against a supersampled (8×8) CPU rasterization reference; report SSIM + visual diff images.
- **Interfaces:** learnings → `cutweave-text` (P3) and ADR-007.
- **Acceptance criteria:** prints for both paths: 1000-animated-glyph per-frame CPU instance-build time (target < 1 ms) and GPU time; crispness SSIM ≥ 0.98 on the 4K scene for the chosen path; diff images saved.
- **Tests:** snapshot test (`insta` image hash with tolerance) for caption frame.
- **Out of scope:** rich-text editing, caption chunking.

### WP-0F.03 — S4 measure on all platforms + report (+ skia fallback if failing)
- **Area:** `docs/spikes/S4-report.md`
- **Depends on:** WP-0F.02, WP-0I.04 (or developer machines meeting the 0I.02 definition)
- **Size:** M
- **Executor:** human (hardware runs) with opus-assisted analysis; skia-safe evaluation only if a criterion fails
- **Scope:**
  - Run 0F.01/0F.02 on all reference machines including the iGPU reference (M1 and Intel Xe). Evaluate: GPU time < 8 ms (1080p60 scene) on iGPU; PSNR > 45 dB vs reference; ΔE < 2; 4K text crispness; 1000 glyphs < 1 ms.
  - Decide text path for ADR-007: MSDF only / MSDF + Vello for large-complex glyphs / Vello only. If MSDF AND Vello both fail the glyph criteria, build a minimal skia-safe variant (time-box 3 days) and compare.
  - Record wgpu behaviour deltas between WARP/lavapipe/real GPUs relevant to golden-frame tolerances (feeds 1O.01).
- **Interfaces:** evidence for ADR-007, input to 0G.
- **Acceptance criteria:** table with numeric result per criterion × adapter; verdict `PASS | PASS-WITH-FALLBACK (<which>)`; documented tolerance recommendation (PSNR/FLIP thresholds) for software-vs-hardware golden frames.
- **Tests:** n/a.
- **Out of scope:** HDR output.

---

## 0G — Spike S4b: compositions runtime

> **Pass:** transpile < 50 ms; scene eval < 1 ms/frame for 200 nodes; deterministic across runs; sandbox (no I/O) enforced. **On fail:** simplify to declarative JSON scenes + expressions.

### WP-0G.01 — S4b build: oxc → rquickjs → scene graph → S4 compositor
- **Area:** `spikes/s4b-compose`
- **Depends on:** WP-0F.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Transpile the TSX lower-third from arch §12 (using a minimal scratch `@cutweave/compose` shim: `defineComposition`, `interpolate`, `spring`, `Group/Rect/Text`) with **oxc** (TS + JSX → JS; custom JSX factory emitting plain node objects) in-process.
  - Run in **rquickjs** with: no `fetch`/`fs`/`require`/`import()` of external modules, `Date.now`/`Math.random`/`performance.now` removed or replaced (seeded PRNG from item id), memory limit and interrupt-handler time limit.
  - `render({frame,fps,props,size})` → serializable scene description (JSON or compact binary) → drawn by the S4 compositor (rects, rounded rects, text via MSDF path).
  - Props schema → JSON Schema extraction (`z`-like shim) just enough to prove the inspector/MCP path.
  - Test scene with 200 nodes; adversarial scripts (infinite loop, huge alloc, attempt to touch `process`, `require('fs')`, `fetch`, `eval`, `new Function`).
- **Interfaces:** learnings → `cutweave-compose` (P3), `packages/compose-runtime`.
- **Acceptance criteria:**
  - All adversarial scripts are contained: loop is interrupted within the time limit (default 50 ms/frame), alloc bomb fails with a catchable error, forbidden globals are `undefined`/throw; engine process survives (test asserts).
  - The lower-third renders on the S4 compositor to a PNG at frames 0, 60, 119.
- **Tests:** Rust tests for each adversarial script and for determinism (below).
- **Out of scope:** the full scene API, npm module resolution, hot reload.

### WP-0G.02 — S4b measure + report (fallback decision)
- **Area:** `docs/spikes/S4b-report.md`
- **Depends on:** WP-0G.01, WP-0I.04 (or developer machines meeting the 0I.02 definition)
- **Size:** S
- **Executor:** sonnet (numbers) + human review of verdict
- **Scope:**
  - Measure on every reference machine: transpile time (cold + warm, median of 100) < 50 ms; scene eval time per frame for the 200-node scene (median/p99 of 10,000 frames) < 1 ms; determinism: render frames 0..239 three times in three separate processes → byte-identical scene JSON (hash compare); sandbox suite passes.
  - Compare rquickjs vs Boa quickly on eval time (informational) and note oxc crate version/API stability.
  - If failing: sketch the declarative JSON scene + expression fallback (expression grammar, node set) in the report, with an estimate of the lost capability.
- **Interfaces:** evidence for ADR-012.
- **Acceptance criteria:** numeric cells for 3 thresholds × OS, determinism hash equality shown, verdict `PASS | PASS-WITH-FALLBACK (declarative)`.
- **Tests:** n/a.
- **Out of scope:** implementing the fallback.

---

## 0H — Spike S5: command bus → codegen → MCP

> **Pass:** schemas generated and snapshot-tested; MCP Inspector passes; an agent (Claude Code) completes "import, cut, render frame" with no screenshots. **On fail:** adjust macro design; pin rmcp version.

### WP-0H.01 — S5 build: `#[command]` macro, registry, schema + TS codegen
- **Area:** `spikes/s5-bus`
- **Depends on:** WP-0A.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Proc-macro `#[command(name, summary, undoable, mcp="core|catalog|hidden", annotations(...))]` over a struct, per arch §5.1; generates `inventory`-style registration (linkme/inventory crate), a `CommandMeta` (name, summary, flags, input `schemars` schema, output schema), and a type-erased `invoke(json) -> Result<json, CmdError>`.
  - 5 toy commands over a toy doc (`project.open`, `media.import`, `timeline.insert`, `clip.split`, `clip.trim`) + query `timeline.get` + query `render.frame` (returns PNG bytes); typed errors `NotFound/Invalid{path,msg}/Conflict`.
  - Codegen binary emits `schema/commands.json` and TS types + typed client via **specta** (`specta-typescript`); implements the **decided** `Time` wire format (arch §4.1, ADR-015): JSON integer flicks, TS `number`, safe-integer validation at the boundary, timecode-string inputs accepted, specta type override so `i64` exports as `number`.
  - `insta` snapshot tests of the JSON Schema and TS output.
- **Interfaces:** learnings → `cutweave-commands-derive` (1B.02) and 1N.
- **Acceptance criteria:**
  - `cargo test` snapshot tests pass; changing a command field changes the snapshot and fails the test until reviewed.
  - Generated TS compiles under `tsc --strict` and a sample script calls `client.call("clip.split", {...})` with compile-time checked args (negative test with a wrong field fails `tsc`).
  - `Time` fields export as TS `number` (no `bigint`), a value above 2^53-1 is rejected with `Invalid` (test), and a timecode string input parses (test), per ADR-015.
- **Tests:** snapshot + trybuild compile-fail test for a malformed `#[command]`.
- **Out of scope:** transactions/undo, real ops.

### WP-0H.02 — S5 build: rmcp server (stdio + HTTP) over the registry
- **Area:** `spikes/s5-bus`
- **Depends on:** WP-0H.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Verify the current rmcp version; pin exact version. **Test BOTH MCP revisions, 2025-11-25 and 2026-07-28, via rmcp** (client and server negotiation, session handling, tool listing/calling, image content, structured output) and record in `docs/spikes/S5-report.md` which revision(s) rmcp supports, how negotiation resolves, and any gaps.
  - Change notifications: implement whichever the negotiated revision supports (`resources/subscribe` + `notifications/resources/updated` on the older revision; `subscriptions/listen` on the newer) behind one internal `Notifier` abstraction. **Do not depend on `ping`** for liveness on the new revision.
  - Tool profiles are selected **per endpoint** (`/mcp`, `/mcp/minimal`, `/mcp/full`) or by a `--profile` flag on stdio servers, never per connection; prototype `/mcp` and `/mcp/minimal`.
  - rmcp server exposing the 5 commands as tools (annotations `readOnlyHint/destructiveHint/idempotentHint` from macro metadata) + generic `cutweave_search_commands`, `cutweave_describe_command`, `cutweave_execute(steps, dry_run?, expected_rev?)` and `render_frame` returning **image content** (JPEG ≤ 768 px, base64) + structured metadata.
  - Transports: stdio and Streamable HTTP on `127.0.0.1` random port with bearer token + Host/Origin checks.
  - Tool-list budget measurement (tool count and estimated tokens of `tools/list`) against the single-source budgets: **minimal <= 12 tools / <= 3k tokens; standard <= 36 tools / <= 8k tokens; full = standard + search-based long tail; P1 gate <= 19 tools / <= 5k tokens**.
- **Interfaces:** learnings → `cutweave-api` MCP module (1J).
- **Acceptance criteria:**
  - `npx @modelcontextprotocol/inspector` connects over stdio and HTTP and lists tools, calls each, receives an image content block for `render_frame` (screenshots or CLI-mode transcript saved).
  - HTTP without token → 401; wrong Host header → 403.
  - The same test suite passes against both MCP revisions (or the report documents which fails and the fallback: pin to the older revision, adapter shim for the newer).
  - A change notification reaches a subscribed client under each revision's mechanism.
  - `tools/list` size recorded (target informational: < 6k tokens for 8 tools).
- **Tests:** Rust integration test using rmcp client against the server (both transports).
- **Out of scope:** real engine, resources/subscriptions.

### WP-0H.03 — S5 agent run + report
- **Area:** `docs/spikes/S5-report.md`, `spikes/s5-bus/agent-run`
- **Depends on:** WP-0H.02
- **Size:** S
- **Executor:** human (runs Claude Code) with sonnet-written report skeleton
- **Scope:**
  - Configure Claude Code with the spike MCP server (stdio) and give the prompt "Import `<fixture>`, cut it at 2 s, and show me the frame at 3 s"; record transcript; confirm **no screenshots/computer-use** tools used and that the final doc state matches expectations (query `timeline.get`).
  - Repeat 3 times and once with an alternative client (e.g. Codex CLI or Gemini CLI, if available) to check client-independence.
  - Verdict on each pass criterion; list macro/design adjustments discovered; final pinned rmcp version.
- **Interfaces:** evidence for ADR-009 (hybrid catalog) and for 1J.
- **Acceptance criteria:** 3/3 successful agent runs (transcripts committed to `spikes/s5-bus/agent-run/`); report states rmcp version + the result for each of the two spec revisions; verdict `PASS | PASS-WITH-FALLBACK (macro/rmcp pin)`.
- **Tests:** n/a.
- **Out of scope:** LLM eval harness (P6).

---

## 0I — Test-media corpus & benchmark harness

### WP-0I.01 — Corpus manifest + fetch script + license audit
- **Area:** `testdata/`, `xtask`
- **Depends on:** WP-0A.01
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `testdata/corpus.toml` manifest: for each item `id, description, sha256, size, license (CC0/CC-BY/self-generated), source_url or generator, tags`. Required items: H.264 4K30 long-GOP, H.264 1080p60, HEVC 10-bit SDR, HEVC 10-bit HDR10 (PQ), ProRes 422 HQ 1080p, **VFR phone clip** (rotated 90°), odd sample-rate audio (22.05k, 44.1k, 48k, 96k), 5.1 audio, image (PNG/JPEG), a truncated/broken MP4, a zero-audio-stream video, a variable-frame-size case. Generated items use `ffmpeg lavfi` recipes committed in `testdata/generate/*.sh` (so licensing is trivially clean); real-world items only CC0/CC-BY.
  - `cargo xtask fetch-corpus [--tier small|full]` downloads to `testdata/.cache/` (gitignored) verifying sha256, resumable, offline-friendly (skips if present); `--tier small` < 50 MB for PR CI, `full` for nightly. Includes `xtask corpus-verify`.
  - CI step caches `testdata/.cache` keyed on manifest hash.
  - `testdata/README.md` with attribution table (CC-BY credits).
- **Interfaces:** corpus ids referenced by tests (`corpus::path("h264_4k30_longgop")` helper in a tiny `cutweave-testutil` module under `xtask`, to be moved into a dev-crate in 1A).
- **Acceptance criteria:**
  - `cargo xtask fetch-corpus --tier small && cargo xtask corpus-verify` exits 0 on 3 OSes; corrupting a cached file makes verify fail.
  - Every manifest item has a license field; `xtask` rejects a manifest entry without one.
  - Generated items reproduce byte-identical sha256 across runs on the same ffmpeg version (record ffmpeg version in manifest) — or declare tolerance (`probe_hash` of stream properties) if not.
- **Tests:** manifest parser unit tests; checksum mismatch test.
- **Out of scope:** hosting the large files (document the chosen host in README: GitHub Releases assets of a `cutweave-testdata` repo).

### WP-0I.02 — Reference machine definitions
- **Area:** `docs/reference-machines.md`
- **Depends on:** —
- **Size:** S
- **Executor:** human (decides hardware) with sonnet drafting the doc
- **Scope:**
  - Define named reference machines (min 5): `ref-win-dgpu` (Win11 + NVIDIA mid-range), `ref-win-igpu` (Win11 + Intel Xe/Arc iGPU, 16 GB RAM), `ref-mac-m1` (macOS arm64 M1, 8–16 GB), `ref-linux-x11` and `ref-linux-wayland` (Ubuntu 24.04, Mesa; can be dual-boot or VM with GPU passthrough — specify), each with CPU, GPU, RAM, storage, OS build, driver versions, display refresh, audio device (and loopback fixture), and who owns it (person/CI self-hosted).
  - Rules: how results are tagged (`machine`, `commit`, `ffmpeg`, `wgpu`, `electron`), power-plan/thermal conditions (AC power, performance mode, 5-min warm-up), reproducibility checklist.
  - Map each arch §19 budget and each spike to the machine(s) it must pass on (e.g. iGPU playback budget → `ref-win-igpu` and `ref-mac-m1`). State which budgets can only be human-run until self-hosted runners exist.
  - Fixed answer on Intel macOS: not a reference platform (decision recorded; arm64 only per roadmap).
  - **Minimum-OS machines** (for ADR-014): Windows 10 22H2, macOS 13, Ubuntu 22.04 (VMs acceptable) used by the nightly min-OS smoke test (0I.04).
  - Hardware order is a **day-0 long-lead item** (D0.5): the doc lists vendor, lead time and the date the order was placed.
- **Interfaces:** machine ids used in the benchmark harness JSON (`machine` field).
- **Acceptance criteria:** doc exists with ≥ 5 machines each having all listed fields filled with concrete values or an explicit "TBD owner=<name>, due=<date>" for ≤ 1 machine; spike→machine mapping table complete for S1–S5.
- **Tests:** `docs-lint`.
- **Out of scope:** the purchase itself (D0.5).

### WP-0I.03 — Benchmark harness skeleton
- **Area:** `xtask/` (`bench` subcommand), `bench/`, per-crate `benches/`
- **Depends on:** WP-0A.01, WP-0I.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Rust: `criterion` benches scaffold per crate (empty `benches/` with one placeholder each for `cutweave-model`, `cutweave-commands`, `cutweave-render`, `cutweave-media`) and a **budget file** `bench/budgets.toml` encoding every arch §19 budget as `{id, metric, op (<,≥), value, unit, machine_scope}` (10 rows).
  - A result schema `bench/result.schema.json` (`{machine, commit, date, toolchain{rustc,ffmpeg,wgpu,electron}, metrics:[{id,value,unit,samples}]}`), `cargo xtask bench --suite <name> --out results.json`, and `cargo xtask bench-check results.json` which compares against `budgets.toml` and exits non-zero on regression (> budget, or > 10% vs stored baseline for criterion).
  - TS side: `packages/bench-utils` not created; instead `scripts/bench/ui-bench.ts` emitting same schema (frame-time collector reused from S1 learnings) — skeleton only.
  - `nightly.yml` calls the suite on CI runners (informational, non-blocking) and uploads JSON artifacts; `bench/README.md` describes how to add a metric.
- **Interfaces:** result JSON schema + `budgets.toml` consumed by 2J (P2) and 7D.
- **Acceptance criteria:**
  - `cargo xtask bench --suite smoke --out r.json && cargo xtask bench-check r.json` passes with a synthetic metric; editing the synthetic metric above its budget makes `bench-check` exit non-zero.
  - `budgets.toml` has exactly the 10 §19 rows (test asserts count and ids).
  - Schema validates sample output (`ajv` or `jsonschema` in a test).
- **Tests:** unit tests for comparator logic and schema.
- **Out of scope:** real benchmarks (P2), dashboards.

### WP-0I.04 — Provision reference machines and self-hosted runners
- **Area:** `docs/reference-machines.md`, `.github/workflows/`, `ci/runners/`
- **Depends on:** WP-0I.02, WP-0A.03
- **Size:** M
- **Executor:** human (hardware/ops) with sonnet-written runner setup scripts
- **Scope:**
  - Provision the machines defined in 0I.02: Win11 dGPU, Win11 iGPU (Intel Xe/Arc), macOS M-series arm64, Ubuntu 24.04 with **X11 and Wayland** sessions. Install drivers, audio loopback fixture, pinned toolchains (`just setup`).
  - Register them as **self-hosted GitHub Actions runners** with labels `ref-win-dgpu`, `ref-win-igpu`, `ref-mac-m1`, `ref-linux-x11`, `ref-linux-wayland`; ephemeral working dirs, non-privileged user, repo-scoped, only `workflow_dispatch`/nightly/`main` triggers (no fork PRs; security note in `docs/ci.md`).
  - `ci/runners/setup-<os>.{ps1,sh}` idempotent provisioning scripts; `nightly.yml` gains a `bench-ref` matrix over these labels running `just bench` and uploading JSON results (informational).
  - `runner-health.yml` (weekly): GPU adapter list, audio device, FFmpeg bundle, disk.
  - **Nightly minimum-OS smoke test** (`nightly.yml` job `min-os`): runners/VMs labelled `min-win10` (Windows 10 22H2), `min-mac13` (macOS 13), `min-ubuntu2204` (Ubuntu 22.04) run the fetched FFmpeg bundle probe, the engine binary `--version`, a headless render of one synthetic frame (software adapter) and an Electron launch smoke. Linux binaries come from the Ubuntu 22.04 container build (0A.03/0A.06).
- **Interfaces:** runner labels (used by measure WPs 0C/0D/0E/0F, HW tests in P1, 2J in P2).
- **Acceptance criteria:**
  - Each of the 5 labels runs `runner-health.yml` green and reports its real GPU adapter (not software), audio device present, and expected display server (X11 vs Wayland).
  - `nightly.yml` `bench-ref` completes on all 5 and uploads schema-valid result JSON (0I.03 schema).
  - `min-os` nightly passes on the 3 minimum-OS targets or each failure is filed against ADR-014 as a floor adjustment.
  - A PR from a fork cannot schedule on these runners (workflow `if:` guard, verified by a documented test run).
- **Tests:** health workflow.
- **Out of scope:** purchasing decisions (owner sign-off in 0I.02), cloud GPU runners.

---

## 0J — ADRs, go/no-go, re-estimate

### WP-0J.01 — ADR write-ups for already-accepted decisions
- **Area:** `docs/adr/`
- **Depends on:** WP-0B.04
- **Size:** M
- **Executor:** sonnet (drafts), human review
- **Scope:**
  - Write ADRs using the template: **002** (single binary, headless-first, one bus), **004** (flicks), **005** (doc actor + imbl + invertible patches; CRDT later), **006** (project format), **008** (AI gateway), **010** (plugin layers), **011** (licensing — link 0B.01 outputs), **013** (UI stack) and **015** (wire protocol: `request_id` idempotency, flicks as JSON integers with TS `number`, `--deterministic-ids` test mode; arch §4.1/§5.1). Status Accepted, date 2026-11-22 (011 cites the user's decision). Each: context, decision, 2+ alternatives with why rejected, consequences, source references to arch § and research files; content is *derived from* the architecture doc, no new decisions.
  - Update `docs/adr/README.md` index.
- **Interfaces:** ADR numbers/titles per arch §23.
- **Acceptance criteria:** 9 ADR files exist with all template sections non-empty; each cites at least one architecture § number; markdown link check passes; ADR 004 includes the flicks arithmetic table (fps/sample rates dividing 705,600,000 — verified by a script in the PR for the 9 frame rates and 3 audio rates listed in arch §4.1).
- **Tests:** `docs-lint`.
- **Out of scope:** the spike-dependent ADRs (0J.02).

### WP-0J.02 — Spike-dependent ADRs (001, 003, 007, 009, 012) → Accepted
- **Area:** `docs/adr/`
- **Depends on:** WP-0C.04, WP-0C.06, WP-0E.03, WP-0F.03, WP-0G.02, WP-0H.03 (and WP-0C.05 if activated), WP-0D.02 (recorded as input to go/no-go)
- **Size:** M
- **Executor:** human decides; opus drafts from reports
- **Scope:**
  - For each of ADR-001 (Electron vs GPUI), 003 (FFmpeg/`MediaBackend`, CPU upload first), 007 (text: MSDF/Vello/skia), 009 (rmcp + hybrid tool catalog), 012 (TS→oxc→QuickJS compositions): fill Context/Decision/Consequences from the corresponding spike report, quoting measured numbers and any fallback adopted; set Status = Accepted (or "Accepted with fallback: …" or "Rejected → superseded by ADR-0xx"). **ADR-001 must record the preview transport choice** (a: non-sandboxed preload addon in the main editor window only / b: utilityProcess + MessagePort) from the S1 report; **ADR-009 records which MCP spec revision(s) are supported** and the notification mechanism per revision. **ADRs become Accepted here, in 0J.02; Phase 1 WPs depend on 0J.02, not on the individual spike reports.**
  - If a spike failed without an accepted fallback, do NOT mark Accepted; create the open-issue list for 0J.03.
  - Update arch §23 status column? **No** (integrator edits docs 01); instead list needed edits in `docs/adr/ARCH-DELTAS.md` for the integrator.
- **Interfaces:** ADR-001/003/007/009/012 statuses are the M0 exit gate.
- **Acceptance criteria:** the five ADR files have Status ∈ {Accepted, Accepted with fallback}; every ADR links its spike report and quotes ≥ 3 measured numbers; `ARCH-DELTAS.md` lists every divergence from architecture text (e.g. fallback adopted).
- **Tests:** `docs-lint`.
- **Out of scope:** revising the architecture doc.

### WP-0J.03 — Min OS decision + M0 go/no-go review
- **Area:** `docs/adr/0014-min-os.md` (ADR-014, Proposed in arch §23, Accepted here), `docs/m0-go-no-go.md`
- **Depends on:** WP-0J.02, WP-0D.02, WP-0A.03
- **Size:** S
- **Executor:** human
- **Scope:**
  - Decide arch §24 open question 4: minimum OS (proposal: Windows 10 22H2+ if the pinned Electron/wgpu support it else Windows 11 only; macOS 13+ (Metal 3 / arm64 only); Ubuntu 22.04+ (glibc 2.35 build container) with Mesa ≥ 23) — constrained by the pinned Electron's supported OS list and wgpu backends; record the evidence.
  - Go/no-go review meeting doc: S1–S5/S4b verdict table (from reports), exit checklist (all spikes pass or have accepted fallback; ADR-001/003/007/009/012 Accepted), risks carried into P1, decision: GO / GO-with-changes / NO-GO with owner sign-offs.
  - If GO-with-changes, list concrete modifications to Phase 1 WPs (by id).
- **Interfaces:** gates Phase 1 start.
- **Acceptance criteria:** `m0-go-no-go.md` has a signed-off decision line and a checklist where each of the 3 exit criteria has a link to evidence; ADR-014 (min OS) status Accepted.
- **Tests:** n/a.
- **Out of scope:** schedule (0J.04).
- **Decided (2026-11-28):** [ADR-0014](../adr/0014-min-os.md) accepted by the product owner as proposed; the nightly min-OS smoke test waits for the reference machines (WP-0I.04).

### WP-0J.04 — Re-estimate roadmap and Phase 1 backlog
- **Area:** `docs/m0-reestimate.md`
- **Depends on:** WP-0J.03
- **Size:** S
- **Executor:** human (with opus analysis)
- **Scope:**
  - Compare actual spike effort vs estimates (agent-days logged per WP); compute velocity and review-latency; adjust durations for P1–P7 in a table (old vs new, rationale); identify Phase 1 WPs to split/merge given spike learnings; update parallelism plan (how many agents concurrently, reviewer capacity).
  - Output: proposed edits to 04-roadmap.md durations (for the integrator), Phase 1 risk register top 10.
- **Interfaces:** feeds roadmap Gantt.
- **Acceptance criteria:** doc contains a table of P1–P7 old/new durations and the total-to-1.0 and total-to-Public-Alpha estimates; each changed number has a one-line rationale.
- **Tests:** n/a.
- **Out of scope:** editing 04-roadmap.md.

---

## 0K — Design system foundations

*Goal:* every pixel of UI built from one source. `design/tokens.json` feeds code through a checked pipeline, and `packages/ui` provides the primitives every later UI WP must use. Spec: [docs/07-design-system.md](../07-design-system.md), [design/README.md](../../design/README.md), and the design-system artifact's component previews and guidelines.

### WP-0K.01 — Token pipeline and contrast gate
- **Area:** `xtask/` (`tokens` subcommand), `design/`, `packages/ui/src/tokens/`, `crates/cutweave-tokens/`
- **Depends on:** WP-0A.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `cargo xtask tokens` reads `design/tokens.json` (list-shaped families, themes `dark` first, then `light`) and generates:
    - (a) `packages/ui/src/tokens/tokens.css`: CSS custom properties under `:root,[data-theme="dark"]` and `[data-theme="light"]`, plus `--font-*` and the type-style classes.
    - (b) a Tailwind v4 `@theme` file mapping colors, spacing, radius and shadow.
    - (c) `@cutweave/tokens` TS constants with types.
    - (d) the `cutweave-tokens` Rust crate (`const` colors as linear and sRGB values, spacing, radii) for engine-drawn overlays and default caption styles.
  - It replaces `design/scripts/tokens_to_css.py`.
  - **Contrast gate:** port of the integrator's contrast checker. Every text token is checked against every surface named in its usage note, per theme (4.5:1). Every `border-control`, `focus-ring` and `clip-*` mark is checked at 3:1 on its grounds.
- **Interfaces:** generated files are committed and snapshot-gated; the token names are the public theming API for plugins (6B).
- **Acceptance criteria:**
  - `cargo xtask tokens --check` exits 0 on the current tokens, and non-zero if any generated file is stale or any declared pair falls below its minimum.
  - Changing one token value regenerates all four outputs deterministically (byte-identical on re-run).
  - The Rust crate builds for `wasm32-unknown-unknown`.
- **Tests:** snapshot tests of the outputs; unit tests for alias resolution (`{marigold}`), 8-digit hex and contrast math against known WCAG pairs.
- **Out of scope:** high-contrast theme values (P7).

### WP-0K.02 — Bundled fonts and icon set
- **Area:** `packages/ui/src/fonts/`, `packages/ui/src/icons/`
- **Depends on:** WP-0K.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Vendor woff2 subsets (Latin, Latin-ext; full files available on demand) of Instrument Sans (variable wdth/wght), Bricolage Grotesque (variable opsz/wght) and JetBrains Mono (variable wght), with their SIL OFL texts and `@font-face` rules. The app must render identically offline.
  - `<Icon name size>` component: wraps `lucide-react` (ISC) with stroke 1.75, `icon-sm/md/lg` sizes and `currentColor`.
  - Custom editor glyph set on the same 24-unit grid: razor, ripple, roll, slip, slide, magnet, keyframe-diamond (empty/filled/half), mark-in and mark-out, proposal (sparkles reserved for AI).
- **Acceptance criteria:**
  - With the network disabled, Storybook (0K.05) renders all three families. A font-loading test asserts `document.fonts.check()` is true for each family.
  - The icon gallery story lists every Lucide icon used, plus all custom glyphs, at three sizes.
  - Third-party notices include the OFL and ISC texts (check in `scripts/notices`).
- **Out of scope:** CJK and Arabic UI fonts (fallback to system fonts until the i18n work in 7C).

### WP-0K.03 — `packages/ui` primitives I: actions and inputs
- **Area:** `packages/ui/src/components/`
- **Depends on:** WP-0K.01, WP-0K.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** React components implementing the design-system specs (preview, `bundle.css` reference and README guidelines): Button, IconButton, SegmentedControl, NumberScrubber (pointer-lock drag-to-scrub, shift for ×10 and alt for ×0.1, Enter to type, mixed value "—"), Slider with KeyframeToggle, Switch. Built on Radix primitives where one exists. Values only via tokens.
- **Acceptance criteria:**
  - Each component's story matches its design-system preview in both themes (Playwright screenshot within the R24 SSIM tolerance against an approved baseline).
  - axe: 0 violations. Fully keyboard operable, with `focus-ring` visible.
  - NumberScrubber emits `onChange` with exact numbers (no float drift for integer steps) and supports units.
- **Tests:** Vitest interaction tests; screenshot stories in both themes.

### WP-0K.04 — `packages/ui` primitives II: structure, feedback, overlays
- **Area:** `packages/ui/src/components/`
- **Depends on:** WP-0K.03
- **Size:** M
- **Executor:** sonnet
- **Scope:** Panel, Tabs, InspectorSection, Menu (context menu with shortcuts), Tooltip (with kbd), Dialog, Toast (with job variant), StatusChip (icon + word always), JobProgress, CostChip, ActorBadge. Specs come from the design-system artifact.
- **Acceptance criteria:** the same screenshot, axe and keyboard gates as 0K.03. Menu and Dialog trap and restore focus. Toasts respect `prefers-reduced-motion`.
- **Out of scope:** editor-specific components (TimelineClip, TrackHeader, Playhead, Timecode, AssetTile, ModelCard, ProposalBar, CommandPalette), which are built in 1M, 2H, 5F and 6I from the same specs.

### WP-0K.05 — Storybook, visual regression and the no-literals lint
- **Area:** `packages/ui/.storybook/`, CI workflow, lint config
- **Depends on:** WP-0K.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Storybook with a theme toolbar (dark/light) and a density preview.
  - Playwright screenshot job per story and theme on Linux (software rendering), with baselines in Git LFS.
  - An axe addon run in CI.
  - A lint rule (Biome plugin or a stylelint/ESLint-equivalent check script) that rejects literal colors (`#hex`, `rgb(`, `hsl(`, `oklch(`), raw px font sizes and font-family stacks in `packages/*` outside generated token files.
- **Acceptance criteria:**
  - CI fails when a story's screenshot changes without an approved baseline update (label-gated, as for golden frames).
  - The lint catches a planted `color: #fff` in a test fixture.
  - `pnpm storybook:build` succeeds.

### WP-0K.06 — Design-system sync and ownership
- **Area:** `docs/07-design-system.md`, `scripts/design-sync/`, CODEOWNERS
- **Depends on:** WP-0K.01
- **Size:** S
- **Executor:** human (owner decision) + sonnet (script)
- **Scope:**
  - Name the design owner and add CODEOWNERS for `design/`.
  - Document and script the republish of the design-system artifact from `design/`: tokens, README, logo assets and component specs.
  - Once `packages/ui` exists, switch to syncing from code (`/design-sync`).
  - Edits made on the artifact page are copied back to the repo by PR.
- **Acceptance criteria:**
  - After a token change merges, running the sync updates the artifact, and its tokens match `design/tokens.json` (a diff script reports zero differences).
  - The owner is recorded in CODEOWNERS.

**Traceability (0K):**

| Item | WPs |
|---|---|
| Token pipeline + contrast gate | 0K.01 |
| Fonts + icons | 0K.02 |
| `packages/ui` primitives | 0K.03, 0K.04 |
| Storybook + visual regression + lint | 0K.05 |
| Design-system ownership and sync | 0K.06 |


## Traceability

### Roadmap Phase 0 scope → WPs

| Roadmap item | WPs |
|---|---|
| S1 shell & preview: Electron + Rust sidecar, SHM ring, WebGPU canvas, DOM overlay, scrub bar, live resize (both preview transports) | 0C.01, 0C.02, 0C.06, 0C.04 |
| S1 Canvas2D timeline stress (5k clips, 50k keyframes, 200 thumbnails) | 0C.03, 0C.04 |
| S1 fail path: GPUI comparison → ADR-001 | 0C.05, 0J.02 |
| S2 audio clock: cpal 8 tracks, rubato, signalsmith-stretch 0.5–2×, ebur128, video follows audio | 0D.01, 0D.02 |
| S2 fail path: bigger buffers + latency comp, Rubber Band | 0D.02 |
| S3 decode/seek/HW: 4 codecs, index, frame-exact seek, HW (a)+(b) | 0E.01, 0E.02, 0E.03 |
| S4 compositor, color, text (render graph, LUT, tone-map, MSDF+Vello, headless export) | 0F.01, 0F.02, 0F.03 |
| S4 fail path: skia-safe | 0F.03 |
| S4b compositions runtime | 0G.01, 0G.02 |
| S5 command bus → codegen → MCP | 0H.01, 0H.02, 0H.03 |
| Repo scaffold per arch §3, CI matrix, lint/format, cargo-deny, license headers, DCO bot | 0A.01–0A.05 |
| (integrator) FFmpeg bundle CI, WBS→issues generator, Day-0 prerequisites | 0A.06, 0A.07, Day-0 section |
| LICENSE (GPL-3.0) + plugin exception | 0B.01, 0B.02 |
| CONTRIBUTING, CODE_OF_CONDUCT, ADR template, ADRs 001–013 | 0B.03, 0B.04, 0J.01, 0J.02 |
| CLAUDE.md / AGENTS.md | 0B.04 |
| Test-media corpus manifest + fetch script | 0I.01 |
| Reference machines + benchmark harness skeleton | 0I.02, 0I.03, 0I.04 |
| Decide minimum supported OS versions | 0J.03 |

### Spike pass criteria → measuring WP

| Criterion | WP |
|---|---|
| S1: 60 fps, < 2 drops/min, scrub p95 < 80 ms, CPU < 15%, timeline ≥ 55 fps, no z-order issue | 0C.04 |
| S2: drift < 1 frame/30 min, 0 underruns @256, A/V < 40 ms | 0D.02 |
| S3: 500/500 frame-exact, 4K scrub p95 < 150 ms (proxy < 33 ms), (a) everywhere | 0E.03 |
| S4: < 8 ms GPU iGPU, PSNR > 45 dB, ΔE < 2, crisp 4K text, 1000 glyphs < 1 ms | 0F.03 |
| S4b: transpile < 50 ms, eval < 1 ms/200 nodes, deterministic, sandboxed | 0G.02 (build proof 0G.01) |
| S5: schemas snapshot-tested, Inspector passes, agent completes task without screenshots | 0H.01 (snapshots), 0H.02 (Inspector), 0H.03 (agent) |

### Exit criteria → WPs

| Exit criterion | WPs |
|---|---|
| All spikes pass or have an accepted fallback | 0C.04/05, 0D.02, 0E.03, 0F.03, 0G.02, 0H.03, 0J.03 |
| ADRs 001/003/007/009/012 Accepted | 0J.02 |
| Durations re-estimated | 0J.04 |

---

## Notes for integrator

Status per item after the two integrator rounds.

1. **RESOLVED: missing docs.** `03-feature-parity.md`, `06-dev-process.md` exist.
2. **RESOLVED: layout.** `spikes/`, `xtask/`, `bench/`, `schema/`, `scripts/`, `justfile` in arch section 3; created by 0A.01.
3. **RESOLVED: ADR numbering.** ADR-014 (min OS) accepted in 0J.03; ADR-015 (wire protocol) written in 0J.01.
4. **RESOLVED: hardware.** Measure WPs stay `human` and depend on 0I.04 (or developer machines meeting the 0I.02 definition); hardware purchase is Day-0 item D0.5 and heads the P0 critical path.
5. **RESOLVED: FFmpeg.** 0A.06 builds the pinned bundle in CI (Linux in an Ubuntu 22.04 container; full required configure list and `-encoders` check) with `just ffmpeg-fetch`.
6. **RESOLVED: `Time` wire format.** ADR-015; implemented in 0H.01.
7. **RESOLVED: Electron sandbox vs native addon.** S1 measures both transports (0C.01 transport a: non-sandboxed preload of the main editor window only; 0C.06 transport b: utilityProcess + MessagePort); ADR-001 (0J.02) records the choice.
8. **RESOLVED: MCP spec revisions.** 0H.02 tests both 2025-11-25 and 2026-07-28; notifications per revision; no reliance on `ping`; profiles per endpoint; single-source budgets.
9. **RESOLVED: macOS Metal on hosted runners.** Verified in 0A.03 with a fallback to the self-hosted `ref-mac-m1` runner.
10. **RESOLVED: minimum OS.** Linux engine/FFmpeg built in an Ubuntu 22.04 container (glibc 2.35); nightly min-OS smoke test in 0I.04.
11. **RESOLVED: WBS to issues / cycle prevention.** 0A.07 (`wbs-to-issues`, epic-dep expansion, cycle lint); the 0A.01/0A.06 dependency cycle was removed by moving `just setup` acceptance to 0A.06.
12. **RESOLVED: Day-0 prerequisites** listed with named roles at the top of the file.
13. **OPEN (minor): S5 agent run is non-deterministic.** Objective form "3/3 runs".
14. **OPEN (minor): min-OS floor for Windows 10** depends on the pinned Electron/wgpu; decided in 0J.03.
15. **OPEN (minor): S1 "transport CPU < 15%"** defined in 0C.04 as a share of one core-equivalent across engine + main + renderer; confirm.
16. **OPEN (minor): S2 A/V offset** needs external measurement (camera/loopback); assumed manual.
17. **Phase size.** 38 WPs (above the 20-30 guidance) because each spike has build + measure WPs.
