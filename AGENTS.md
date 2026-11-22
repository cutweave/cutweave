# AGENTS.md — rules for AI coding agents working on Cutweave

Cutweave is an open-source desktop video editor and agent-native video environment: a Rust headless engine, an Electron + React UI, full MCP control, a cloud-first AI gateway and plugins at every layer. Canonical file for Codex, Gemini CLI, Cursor, Claude Code and others (`CLAUDE.md` imports it).

**Current stage: Phase 0.** The scaffold, CI, governance files and the design-token pipeline exist. Spikes (`spikes/`) and ADRs (`docs/adr/`) are in progress; product crates are mostly stubs until Phase 1.

## Read before working

1. [docs/00-product-brief.md](docs/00-product-brief.md): settled decisions. Don't relitigate them.
2. [docs/01-architecture.md](docs/01-architecture.md): **the contract.** Cite § numbers in PRs. §3 is the repo map and dependency rule.
3. Your work package in [docs/wbs/](docs/wbs/) (stay within its `Area`), the process in [docs/06-dev-process.md](docs/06-dev-process.md), and the integrator resolutions R1-R26 in [docs/05-work-breakdown.md](docs/05-work-breakdown.md), which override phase-file notes.
4. Decisions: [docs/adr/](docs/adr/README.md) (index with statuses). A **Proposed** ADR is not decided; don't build on it past its spike.

## Run things

| Task | Command |
|---|---|
| Everything CI runs (do this before a PR) | `just check` |
| One Rust crate | `cargo nextest run -p cutweave-<crate>` (doc-tests: `cargo test -p <crate> --doc`) |
| One TS package | `pnpm --filter @cutweave/<pkg> test` |
| Design tokens (generate / verify) | `cargo xtask tokens` / `cargo xtask tokens --check` |
| Dependency rule, wasm32 check | `cargo xtask check-deps`, `cargo xtask check-wasm` |
| License headers | `cargo xtask license-headers --check` (`--fix` adds them) |
| Markdown links | `python scripts/check-md-links.py` |
| Snapshots | `cargo insta review`; never hand-edit `.snap` files |
| All recipes | `just --list` |

`cargo xtask gen` (schemas and TS types) arrives in WP-1N. Never hand-edit generated files (`schema/`, `packages/protocol/src/generated/`, token outputs); regenerate them.

## Non-negotiable rules

- **Every mutation is a registered command** (`#[command]` in `cutweave-ops`) applied through `Tx`. Never mutate the project document any other way. If the UI can do it, MCP and the CLI must be able to do it too; the parity tests enforce this (ADR-0002, ADR-0005).
- **Time is flicks (`i64`)**, using only the conversion helpers. Entities are addressed **by ID, never by index** (ADR-0004).
- **Crate boundaries** (§3): `model ← commands ← ops`. `render`, `audio` and `media` never depend on `api` or UI. `cutweave-model`, `-commands`, `-render` (core) and `-text` must stay `wasm32`-clean.
- **Real-time and render threads:** no blocking, no locks on hot paths, no per-frame allocations.
- **Determinism:** no wall clock or unseeded RNG in render or command code; seed from item IDs. Tests may use `--deterministic-ids` (ADR-0015).
- **Secrets never leave the engine:** no API keys in the renderer, plugins, logs or MCP results.
- **Contracts change only via ADR:** command schemas, WIT, project format, plugin manifest, MCP surface.
- **No `unsafe`** outside the crates that need it for FFI, SHM or GPU interop. Justify each block in a `// SAFETY:` comment.
- **No new dependency** without a justification in the PR. It must pass `cargo-deny` and the JS license check (GPL-3.0-compatible; policy in §21).
- **No network in tests. No committing media**: the corpus is fetched (`just corpus`), goldens go in LFS.
- **UI uses the design system only:** `packages/ui` components and `var(--token)` values from `design/tokens.json`. No literal colors, sizes or font stacks. Follow `design/README.md` (voice, states, marigold = "here", agent blue = AI/agent). Missing a value? Add a token with a usage note in the same PR.
- **Tests first** from the acceptance criteria. Run `just check` before opening a PR.
- **Don't guess:** if the WP is ambiguous or larger than 4 agent-days, stop and report a split or a question.

## Code conventions

- Rust: edition 2024; `thiserror` in libraries, `anyhow` only in binaries; `tracing` for logs; no `unwrap()` outside tests; `cargo fmt` and `clippy -D warnings`.
- TS: strict mode; Biome (`pnpm lint`, `pnpm format:check`, `pnpm typecheck`).
- **License headers:** every source file starts with `SPDX-License-Identifier: GPL-3.0-or-later`, except `wit/`, `packages/plugin-sdk` and `packages/compose-runtime`, which use `MIT OR Apache-2.0` (ADR-0011). `cargo xtask license-headers --fix` inserts them.
- Docs: relative links only (the link checker is part of `just check`). Don't add docs files nobody asked for.

## Git, worktrees and PRs

- One WP per branch: `wp/<WP-id>-<slug>` (e.g. `wp/1B.04-undo-property-tests`), cut from `main`, ideally in its own git worktree (`git worktree add ../cutweave-<WP> -b wp/<WP-id>-<slug> main`). Squash-merge; short-lived branches.
- Conventional commits (`feat(ops): …`, `docs(adr): …`); **`git commit -s`** (DCO sign-off is required).
- The PR includes the WP id, an acceptance-criteria checklist with evidence, the touched architecture sections and "Notes for integrator". Link the ADR when a contract or decision changes.
- Don't touch files outside your WP's `Area`; surface cross-cutting needs as notes for the integrator.
- Never skip hooks or force-push shared branches.

## Where things are

`crates/` engine (arch §3) · `apps/desktop` Electron · `packages/` TS (`protocol` and `schema/` generated) · `wit/` plugin interfaces · `design/` tokens and design system · `spikes/` throwaway Phase 0 prototypes (never depend on them) · `xtask/` automation · `docs/adr/` decisions.
- **CI minutes:** never trigger, dispatch or re-run GitHub Actions workflows. Verify locally with `just ci-local` (see docs/06-dev-process.md §8).
