# ADR-0008: Cloud-first, capability-typed AI gateway with a signed model catalog

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (product owner approved)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §13, §14b, §18, §21

## Context

Decision D2 in the [product brief](../00-product-brief.md) makes AI **cloud-first and model-agnostic** (bring-your-own keys plus aggregators), with local models as an optional provider and offline fallback. The model landscape churns weekly, providers are asynchronous (submit, poll, download from expiring URLs), and agents can spend real money. Research 06 surveys Higgsfield, aggregators and direct providers.

## Decision drivers

- New models appear without an app release.
- Secrets never reach the renderer, plugins, logs or MCP results.
- Spending is bounded by policy the model cannot override.
- One normalized I/O per capability so features and agents are provider-independent.
- Jobs survive restarts; results are downloaded before URLs expire; provenance is kept.

## Considered options

1. **Capability-typed gateway + signed, CI-built model catalog + declarative HTTP and WASM adapters, aggregators first** (chosen).
2. **Hard-coded per-provider integrations.** Rejected: every new model needs a release, parameter forms and cost estimates are hand-maintained, and the combinatorics grow without bound.
3. **A project-hosted proxy service that fronts all providers.** Rejected: contradicts the no-account, no-hosting non-goals (brief) and creates a trust and cost liability.
4. **Local-first AI with cloud as an add-on.** Rejected by D2: quality and breadth of leading models are cloud-only; local models remain a provider plugin ([research/04-local-ai.md](../research/04-local-ai.md)).

## Decision

`cutweave-ai` exposes a **closed, host-versioned capability enum** (Llm, VideoUnderstand, Stt, Tts, VoiceClone, VoiceConvert, Dub, LipSync, Translate, TextToImage, ImageEdit, TextToVideo, ImageToVideo, VideoToVideo, Music, Sfx, Upscale, Interpolate, Matting, SegmentTrack, AudioEnhance, StemSplit, StockSearch (R18)). Each capability has a normalized I/O type. Provider adapters are **declarative HTTP manifests** (most aggregator models) or **WASM component adapters** (`wit/cutweave-provider`); **the host performs the HTTP and injects keys**. A **CI-built, ed25519-signed, remotely updatable model catalog** (rollback-protected) drives AI Studio forms, MCP parameters and cost estimates, merged with live aggregator discovery and user-added slugs. Jobs are persisted in SQLite, polled with backoff, cancellable, with results downloaded immediately into `generated/` with a provenance sidecar and an input-hash cache. Keys live in the OS keychain (write-only entry via `ai.set_key`; adapters use key handles and host-side signing); headless uses `CUTWEAVE_KEY_<PROVIDER>` env vars or an opt-in `age`-encrypted file (R18). **Spending authority** has three tiers (auto within budget, agent-confirmed dry run, human-approved), enforced by the engine policy engine, never the model (R9, ADR-0016). `ai.generate` has a static envelope schema validated at runtime against the selected model's catalog schema (R18). Third-party WASM provider adapters wait for P6; local declarative manifests load in P5 behind a trust prompt.

## Consequences

**Positive**

- New models and prices ship through the catalog, with no release.
- Keys are never exposed; agents cannot approve their own spending (the UI credential is the only issuer of approvals).
- AI features are ordinary commands, so MCP, CLI and the copilot get them for free.
- Provenance and C2PA support are built in.

**Negative / costs**

- We run a signing and catalog pipeline (keys, rollback protection, CI) and must keep it healthy.
- The dynamic per-model schema means parity tests check the envelope only; a separate CI test validates every catalog manifest.
- Cost estimates are formulas and may differ from billing; budgets are enforced against estimates until actuals are known.
- Aggregator and provider ToS and license traps (RMBG-2.0, F5-TTS, XTTS, NLLB, Ultralytics, insightface) must be policed through catalog metadata and `cargo-deny`.

**Follow-ups**

- Epic 5A-5E; minimal Stt slice in WP-3D.02 (R2); Epic 5I local AI plugin.

## Links

- Architecture: [§13](../01-architecture.md), [§14b](../01-architecture.md), [§18](../01-architecture.md)
- Research: [06-cloud-ai-providers.md](../research/06-cloud-ai-providers.md) (§1, §5.1-5.7), [04-local-ai.md](../research/04-local-ai.md), [07-plugin-architecture.md](../research/07-plugin-architecture.md) §1 (WIT adapters)
- Integrator resolutions: R2, R9, R18, R20
- Related: ADR-0010 (plugin layers, WASM adapters), ADR-0016 (credentials and UI-only scopes)
