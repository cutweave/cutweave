# Cutweave — Product Brief & Settled Decisions

> Status: decided 2026-11-22. Changing any item here requires a new ADR in `docs/adr/`.

## Vision

A **fully open-source, desktop-first video editor** with CapCut-class ease and speed for short-form social *and* long-form YouTube creators.
It is **AI-native** (all leading models, Higgsfield-style), **agent-controllable** (full MCP), and **extensible** (third-party plugins everywhere).

## Why now

- No maintained OSS editor delivers CapCut's workflow: fast captioning, styled animated text, templates, effects, and AI tools in one place. OpenCut has stalled, and the classic NLEs (Kdenlive, Shotcut, OpenShot) are built around a pro or legacy UX.
- CapCut keeps moving features behind its paywall and has a history of ToS and privacy controversy. Creators want a tool they own.
- AI agents increasingly *do* the editing. No editor today is fully drivable over MCP without screenshots.

## Settled decisions

| # | Decision | Choice |
|---|----------|--------|
| D1 | Platforms (v1) | Desktop: Windows, macOS, Linux. Web/mobile deferred, but the architecture must not block them. |
| D2 | AI strategy | **Cloud-first, all leading models** through a provider gateway (BYO keys + aggregators). Local models are an optional provider and an offline fallback. |
| D3 | MVP audience | Short-form social **and** long-form YouTube equally. |
| D4 | License | **GPL-3.0** core. **MIT/Apache-2.0** for the plugin SDK, WIT interfaces and composition runtime, plus a written **plugin exception** and a **DCO** (ADR-011). |
| D5 | Agent control | **100% of capabilities via MCP**: structured state and rendered frames as images, never screenshots. |
| D6 | Extensibility | Plugin system across effects, transitions, text styles, AI providers, importers/exporters, UI panels, commands/MCP tools, templates/asset packs. |
| D7 | Agent harness | Cutweave ships an **Agent Kit** (skills, MCP prompts, prompt/preset packs, evals) and a built-in copilot. It is an environment for video-creation agents, not just an editor. |
| D8 | Inspiration | Remotion's ideas (code-defined compositions, deterministic per-frame rendering) are reimplemented originally, with no Remotion code or API cloning. |
| D9 | Name | **Cutweave**, GitHub org [github.com/cutweave](https://github.com/cutweave). CLI `cutweave`, crates `cutweave-*`, npm scope `@cutweave/`. |

## Architectural principles (non-negotiable)

1. **Headless core, API-first.** Every user-visible action is a typed, serializable **Command** on one command bus. The GUI, MCP server, CLI and plugins are all *clients*. If the UI can do it, an agent can do it.
2. **One source of truth.** The project document lives in the Rust core. The UI holds a read-only projection plus ephemeral view state.
3. **Everything undoable, everything replayable.** Commands produce invertible patches. Command logs make bug reports and agent sessions reproducible.
4. **Preview and export share one pipeline.** One render graph serves real-time preview (adaptive quality and proxies) and final export (full quality).
5. **Extensibility by construction.** Built-in effects, transitions and providers use the *same* plugin interfaces third parties use (dogfooding).
6. **Never lose user work.** Autosave, crash-safe project writes and versioned project schema with migrations.
7. **Performance budgets are features.** 60 fps UI, under 100 ms scrub latency on 1080p, and hour-long timelines without degradation.
8. **Privacy & ownership.** No account required, no telemetry by default, keys in the OS keychain, project files in an open documented format.

## Non-goals (v1)

- Mobile apps and the browser editor (D1)
- Real-time multi-user collaboration (the data model should allow it later)
- Hosting our own paid cloud inference (users bring keys or aggregators)
- Pro color-grading / VFX compositing depth (Resolve/Nuke territory)
