# Cutweave

**The open-source video editor for creators and their AI agents.**
CapCut-class speed and ease for short-form *and* long-form video. Fully drivable over MCP, open to plugins everywhere, and connected to the leading AI models with your own keys.

**Website:** [cutweave.github.io](https://cutweave.github.io)

> **Status: pre-alpha. Phase 1 (walking skeleton) is in progress.** Phase 0 passed with a provisional go ([M0](docs/milestones/M0-go-no-go.md)); the FFmpeg decode spike is still blocked. The document model, the command bus with undo, atomic project saves, the editor shell and the timeline renderer are in. Media import, playback, export, the MCP server and the CLI come next. Nothing is ready to use yet.

## Why

- No maintained open-source editor offers CapCut's workflow: fast captions, animated text, templates, effects and AI tools in one place.
- CapCut keeps moving features behind its paywall and claims broad rights over user content. Creators deserve a tool they own.
- AI agents increasingly *do* the editing. Cutweave is built for them from day one: **every feature is an MCP tool**, and agents see results as rendered frames, not screenshots.

## Pillars

| | |
|---|---|
| **Headless engine, many clients** | Rust engine; the Electron/React UI, MCP server, CLI and plugins all use one command bus |
| **Creator-grade editing** | Magnetic timeline, keyframes, styled auto-captions, text-based editing, effects, transitions, color, templates |
| **Any model, your keys** | Cloud-first AI gateway (fal, Replicate, OpenRouter, Higgsfield, Anthropic, Google, ElevenLabs, Runway, …) plus optional local models |
| **Agent Kit** | MCP server, Agent Skills, MCP prompts, model-aware prompt/preset packs, an eval suite and a built-in copilot |
| **Plugins everywhere** | Data packs, WGSL effects, TS/JSX code compositions, WASM components, UI panels |
| **Yours** | GPL-3.0, no account, no telemetry by default, open project format |

## Documentation

| Doc | Contents |
|---|---|
| [Product brief](docs/00-product-brief.md) | Vision, settled decisions, principles, non-goals |
| [Architecture](docs/01-architecture.md) | System design, contracts, ADR log |
| [Tech stack](docs/02-tech-stack.md) | Pinned technology choices |
| [Feature parity](docs/03-feature-parity.md) | CapCut → Cutweave mapping, gaps, differentiators |
| [Roadmap](docs/04-roadmap.md) | Phases, milestones, exit criteria |
| [Work breakdown](docs/05-work-breakdown.md) | Executable work packages per phase |
| [Dev process](docs/06-dev-process.md) | How humans and AI agents build it |
| [Design system](docs/07-design-system.md) | Brand, tokens, components ([live reference](https://claude.ai/artifact/573tiShNRj7J6eiGyApFiH), source in [`design/`](design/)) |
| [Research](docs/research/) | Evidence behind every decision (2026-11) |

## License

Cutweave is licensed under GPL-3.0-or-later. The plugin SDK, WIT interfaces and composition runtime are MIT OR Apache-2.0. A plugin exception that would let plugins using only the published interfaces use any license is being drafted; it is pending legal review and not yet in effect (see [LICENSE-PLUGIN-EXCEPTION.md](LICENSE-PLUGIN-EXCEPTION.md)). Contributions require a DCO sign-off.
