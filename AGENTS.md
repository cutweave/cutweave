# AGENTS.md — rules for AI coding agents working on Cutweave

Cutweave is an open-source (GPL-3.0) desktop video editor and agent-native video environment. It has a Rust headless engine, an Electron + React UI, full MCP control, a cloud-first AI gateway and plugins at every layer.

**Current stage: planning.** Only `docs/` exists. Phase 0 (scaffold + spikes) starts from [docs/wbs/P0-foundations.md](docs/wbs/P0-foundations.md).

## Read before working

1. [docs/00-product-brief.md](docs/00-product-brief.md): settled decisions. Don't relitigate them.
2. [docs/01-architecture.md](docs/01-architecture.md): **the contract.** Cite § numbers in PRs.
3. Your work package in [docs/wbs/](docs/wbs/) and the process in [docs/06-dev-process.md](docs/06-dev-process.md).

## Non-negotiable rules

- **Every mutation is a registered command** (`#[command]` in `cutweave-ops`) applied through `Tx`. Never mutate the project document any other way. If the UI can do it, MCP and the CLI must be able to do it too; the parity tests enforce this.
- **Time is flicks (`i64`)**, using only the conversion helpers. Entities are addressed **by ID, never by index**.
- **Crate boundaries** (§3): `model ← commands ← ops`. `render`, `audio` and `media` never depend on `api` or UI. `cutweave-model`, `-commands`, `-render` (core) and `-text` must stay `wasm32`-clean.
- **Real-time and render threads:** no blocking, no locks on hot paths, no per-frame allocations.
- **Secrets never leave the engine:** no API keys in the renderer, plugins, logs or MCP results.
- **Contracts change only via ADR:** command schemas, WIT, project format, plugin manifest, MCP surface.
- **No new dependency** without a justification in the PR. It must pass `cargo-deny` / the license check (GPL-3.0-compatible).
- **UI uses the design system only:** `packages/ui` components and `var(--token)` values from `design/tokens.json`. No literal colors, sizes or font stacks. Follow `design/README.md` (voice, states, marigold = "here", agent blue = AI/agent). Missing a value? Add a token with a usage note in the same PR.
- **Tests first** from the acceptance criteria. Run `just check` before opening a PR.
- **Don't guess:** if the WP is ambiguous or larger than 4 agent-days, stop and report a split or a question.

## Conventions

- Branch `wp/<WP-id>-<slug>`; conventional commits; `git commit -s` (DCO).
- The PR includes the WP id, an acceptance-criteria checklist with evidence, the touched architecture sections and "Notes for integrator".
- Rust: edition 2024; `thiserror` in libraries; `tracing` for logs; no `unwrap()` outside tests.
- TS: strict mode; Biome; never edit generated files in `packages/protocol/src/generated/`.
