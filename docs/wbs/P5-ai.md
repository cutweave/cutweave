# Phase 5 — AI Gateway & AI Studio: Work Breakdown

> Status: draft 2026-11-22, revised after the integrator resolutions (three-tier spending authority, write-only keys + key handles, WASM adapters deferred to E6A, stock search, Kokoro, §21 licence policy). Contract: [01-architecture.md](../01-architecture.md) §13 (AI gateway), §14/§14b (MCP, Agent Kit), §15 (plugins), §18 (security). Research: [06-cloud-ai-providers](../research/06-cloud-ai-providers.md), [04-local-ai](../research/04-local-ai.md), [08-agent-harness](../research/08-agent-harness.md), [01-capcut-features](../research/01-capcut-features.md). Template: [04-roadmap.md](../04-roadmap.md#work-package-template-used-in-05-work-breakdownmd).
> **Every provider/model/price/limit/API-shape fact in this file is a research snapshot (2026-11-22) and must be treated as "verify at implementation time".** Model lists live in the catalog (data), never in code.

## Phase goal

The Higgsfield-style "all leading models" layer plus CapCut's AI features: a capability-typed gateway (BYO keys + aggregators + local provider), a signed remote model catalog, first-wave adapters, the AI Studio panel, ten-plus AI editing features that are ordinary undoable commands (and therefore MCP tools), long-video-to-shorts, stock-media search, an optional local AI plugin (incl. a Kokoro TTS fallback) with a model manager, and the Agent Kit v1 generation additions.

## Exit criteria (M5 — copied from the roadmap)

1. Each first-wave adapter passes a contract-test suite (recorded fixtures plus an opt-in live smoke test).
2. **Every AI feature listed in the roadmap runs end-to-end**: background removal, object masks, auto reframe, voice enhance, stem split, TTS, voice changer, caption translation, dubbing (+lip-sync), upscale, interpolation, beat/scene detection and stock search. Each shows its cost up front, records provenance and is undoable.
3. The three-tier spending authority (§13) passes its security tests. Long → shorts produces ≥ 3 usable shorts from a 20-min podcast in a blind review.

## Epics (one-line goals)

| Epic | Goal |
|---|---|
| **5A Gateway core** | Capability-typed provider interface, router/policy, key store, host HTTP, AI jobs (extending the E1D queue), result ingest + provenance, costs/budgets, confirm tokens, C2PA, `ai.*` commands. **Extends the Phase-3 (3D) Stt slice; never duplicates it.** |
| **5B Model catalog** | Manifest schema, signed CI-built catalog, importers from aggregator schemas, live discovery, user-added slugs, deprecation handling, wave-1 seed data. |
| **5C Aggregator adapters** | fal, Replicate, OpenRouter (media), Higgsfield — one adapter each, built on the 5E declarative engine. |
| **5D Direct adapters** | Anthropic, OpenAI-compatible, Gemini (LLM/video understanding + media), ElevenLabs (speech; audio gen/dubbing), Runway, Deepgram + AssemblyAI, stock media (`StockSearch`). |
| **5E Adapter SDK** | Declarative HTTP manifest format + executor (first-party adapters are compiled-in Rust or declarative manifests) and adapter tooling. **Third-party WASM adapters are deferred to E6A.** |
| **5F AI Studio UI** | Generate image/video/audio, model picker, cost UX, queue/gallery, send-to-timeline, extend/fill gap, settings/keys, preset browser, stock tab. |
| **5G AI editing features** | Registered commands: bg removal, click-to-track masks, reframe, enhance/stems, TTS/voice changer, translation, dubbing, lip-sync, upscale/interpolate, beat/scene detection. |
| **5H Long → shorts** | Video understanding, highlight mining, short builder, review UI, blind-review evaluation. |
| **5I Local AI plugin** | Model manager with licence gate, local provider runtime (ort + sidecar), whisper (migration of the P3 fallback)/matting/Kokoro TTS/audio/analysis/detection packs. |
| **5J Agent Kit: generation** | Preset/prompt-pack runtime and content, generation MCP tools + prompts, generation skills, generation/safety evals. |

## Cross-phase dependencies (stated in the roadmap)

Matting and object masks (5G.02, 5G.03) build on P4 masks (**E4G**), interpolation (5G.10) on frame blending (**E4H**), reframe (5G.04) on tracking (**E4I**); those WPs wait for their P4 epic. Everything else starts at M2. Other upstream: E1D (`cutweave-jobs` queue), E3D (Stt slice + whisper.cpp fallback), E3E/E3F (captions, text editing), E2A/E2D/E2E/E2H/E2I.

## Critical path

Sequential chain (about 23–26 agent-days plus review latency; fits the 10-week window only if 5A starts first):

`WP-3D.02 / E3D (interim gateway slice: Stt + whisper.cpp fallback; 5A.02 refactors it in place, Step 0)` → `5A.02 core types` → `5A.05 jobs` → `5A.06 ingest/provenance` → `5A.10 ai.* commands` → `5G.01 feature framework` → `5G.04 reframe` (needs `5I.08`) → `5H.03 short builder` → `5H.05 eval fixtures` → `5H.06 blind review`.

Parallel chains that must converge before AI features can be demoed:
- `WP-3D.02 (E3D) → 5A.02 → 5A.03 → 5E.01 → 5E.02 → 5C.01 (fal)`: first working cloud adapter (5A.02, 5D.05, 5G.06–5G.08 and several 5H WPs also depend on E3D; if E3D slips, 5A.02 must publish the core-types PR first, see its Step 0). **5E precedes 5C**; epic letters are not an order.
- `5B.01 → 5B.02 → 5B.07 (seed data)`: nothing can be generated without catalog entries.
- `5A.07 + 5A.08` (budgets, confirm tokens) gate every paid command; `5A.04` (contract-test kit + MockProvider) gates every adapter and UI e2e.
- `5I.01 → 5I.02 → 5I.03..08`: local packs (parallel track; the `5I.09` licence review must precede the default-set merge).
- `5A.11` (human: provider test accounts, CI secrets) gates recording every adapter's cassettes.

Highest-risk items: 5A.08 (spending-authority security), 5C.04 (Higgsfield API unverified), 5I.02 (VRAM contention with preview), the P4 dependencies of 5G.02/03/04/10.

## Conventions that apply to every WP below

**A1 — Adapter Definition of Done (all 5C/5D WPs).**
- Implemented as a manifest (5E) plus minimal Rust glue; **no model IDs, prices or limits in code** (catalog data only; each adapter ships one sample manifest for its tests).
- Keys are used only through key handles (5A.03). Cassettes recorded with `cargo xtask ai-record --provider <id>` (accounts/secrets provisioned in 5A.11) into `crates/cutweave-ai/tests/fixtures/<provider>/`, scrubbed (5A.04). The contract suite `provider_conformance!` passes **offline in CI** with the network disabled.
- Opt-in live smoke: `cargo test -p cutweave-ai --features live -- --ignored <provider>`; needs `CUTWEAVE_LIVE_<PROVIDER>_KEY`; hard-capped by `CUTWEAVE_LIVE_BUDGET_USD` (default 0.50); cheapest model, tiny inputs; never runs in default CI.
- Error-mapping table (HTTP status/body → `AiError` kind + `retryable`) in the adapter README; `estimate_cost` equals the catalog formula in golden tests; sentinel-key test proves keys never appear in cassettes, logs or errors.
- README section "Facts to verify at implementation time" lists every assumption taken from research and the date it was verified.

**F1 — AI feature Definition of Done (all 5G/5H feature WPs).**
- Registered `#[command]` (namespace `ai`/`audio`/`video`/`caption`/…), `mcp = "catalog"` (or `core` where stated), correct `paid`/`destructive` annotations; reachable via RPC, MCP (`cutweave_execute`) and CLI with identical schemas.
- `dry_run` returns a `CostEstimate` and, when paid, the spending-authority outcome of 5A.08 (tier 1 runs, tier 2 `confirm_token` capped by the remaining agent budget, tier 3 `approval_required{approval_id}`); the UI shows cost before run; local-only runs report cost 0 and "ran locally because …".
- Runs as an AI job (5A.05): cancellable, progress events; result downloaded and provenance recorded (5A.06); applied in **one undoable transaction** (actor-tagged). Generated assets stay in the bin after undo.
- Cloud-first with local fallback exactly where the WP says so; the routing decision is recorded in provenance.
- Text derived from media (transcripts, captions, metadata) is untrusted data: delimited in prompts, outputs schema-validated, never able to change policy.
- Covered by the generic `ai_feature_conformance` test (5G.01) using `MockProvider`.

---

## Epic 5A — Gateway core

### WP-5A.01 — Decision record: wave-1 providers, defaults, wording, consent policy
- **Area:** docs/adr/ (new ADR-014 "AI gateway wave-1 decisions")
- **Depends on:** E0J (ADR process), 01-architecture §13, research/06 §6
- **Size:** S
- **Executor:** human
- **Scope:** decide and record, as numbered resolved items: (1) wave-1 provider order and any deferrals (suggested cut list in Notes §21); (2) default provider/model slot per capability (cloud-first) and which capabilities may silently fall back to local (STT, denoise, scene/beat, matting only); (3) cost-display wording ("estimate", `as_of` date, "price via X vs direct", stale-price warning); (4) budget defaults (per-action and per-session agent budget — default $0 per §13 — plus daily/monthly caps); (5) excluded providers and ToS policy (no Sora, no Midjourney/Suno/Udio wrappers, no `tts-1` as default) and whether user-added slugs may bypass it; (6) key-entry UX details (mechanism fixed by §13: write-only `ai.set_key`, fingerprint only); (7) polling-only (no webhooks/relay) for v1; (8) voice-cloning consent policy; (9) whether Higgsfield Soul ID / a `Reference` asset entity is in scope for P5; (10) semantics and default of the headless `--budget` flag.
- **Interfaces:** ADR text only.
- **Acceptance criteria:** ADR merged with all ten items resolved (no "TBD"); 5A.07, 5A.08, 5F.03, 5B.08 link to it.
- **Tests:** none (review gate).
- **Out of scope:** legal sign-off on model licences (5B.08, 5I.09).

### WP-5A.02 — Gateway core types, provider trait, registry and capability router (extends the 3D slice)
- **Area:** crates/cutweave-ai (core modules)
- **Depends on:** E1A, E1B, **E3D (minimal Stt slice + one cloud provider + the whisper.cpp fallback)**, E0H
- **Size:** M
- **Executor:** opus
- **Scope:**
  - **Step 0 (mandatory):** read the 3D gateway slice. If merged, this WP *refactors it in place* (same crate `cutweave-ai`, same `Stt` names where sensible) and updates its tests in the same PR. If 3D has not merged, publish the core-types PR first so 3D rebases onto it. Never create a second Stt trait or a second provider registry.
  - `Capability` closed enum as §13 plus `StockSearch` per the roadmap (Notes §27) (Llm, VideoUnderstand, Stt, Tts, VoiceClone, VoiceConvert, Dub, LipSync, Translate, TextToImage, ImageEdit, TextToVideo, ImageToVideo, VideoToVideo, Music, Sfx, Upscale, Interpolate, Matting, SegmentTrack, AudioEnhance, StemSplit, StockSearch), `#[non_exhaustive]`, `CAPABILITY_SET_VERSION`.
  - One `Provider` trait: `submit/poll/cancel/estimate_cost/list_models` core with a capability-tagged `JobRequest`/`JobOutput` (research/06 §5.1), plus thin typed facades (`SttClient`, `LlmClient`, …) that normalize outputs (Stt → word-timed `Transcript`; generation → `Asset[]`; matting/segmentation → `MaskTrack`). Optional `stream` interface for Llm tokens; Llm message/tool/image/usage/structured-output types defined here.
  - `AiError` taxonomy: Auth, RateLimited{retry_after}, QuotaExceeded, ContentPolicy, InvalidParams, ProviderDown, Timeout, QueueFull, Unknown, each with `retryable`.
  - `ProviderRegistry` + **router**: policy modes `cloud-first` (default), `local-first`, `local-only` (privacy), `auto-fallback` (on offline/error, allow-listed capabilities only). Returns an ordered candidate list with a human-readable reason that ends up in provenance. `local-only` must never yield a cloud candidate.
- **Interfaces:** `cutweave_ai::{Capability, Provider, JobRequest, JobStatus, JobOutput, Asset, MaskTrack, AiError, ProviderRegistry, Router, RoutingPolicy}`; JSON Schemas via schemars; TS via specta.
- **Acceptance criteria:** 3D tests still pass; router policy matrix test (4 modes × capabilities × installed/missing providers); schemas snapshot-tested; docs page `docs/reference/ai-gateway.md` describes the traits.
- **Tests:** unit + proptest (serde round-trip of every type), insta schema snapshots.
- **Out of scope:** job persistence (5A.05), concrete adapters, catalog (5B).

### WP-5A.03 — Key store and host HTTP layer
- **Area:** crates/cutweave-ai (`keys`, `http` modules)
- **Depends on:** 5A.02
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `KeyStore` over `keyring` (Credential Manager / Keychain / Secret Service), keyed by (provider, label). **Write-only API**: `ai.set_key{provider, key}` stores the key and returns only a fingerprint (last 4 characters) — the renderer discards the value; `has`, `delete`, `validate` (provider-specific cheap call). Adapters reference keys only by opaque **key handle**; no read API exists anywhere. Env override `CUTWEAVE_KEY_<PROVIDER>` for CLI/headless MCP. Headless Linux without a Secret Service: env-only plus a clear error (Notes §25). `secrecy` + `zeroize`; redaction layer on `tracing` and on every error `Display`.
  - `HostHttp` (reqwest + rustls): injects auth per `AuthScheme` (header, bearer, query, basic, and `signed`: host-side `sign(alg, key_handle, payload)` for JWT/HMAC/SigV4 providers, returning only the signature), timeouts, jittered retries for idempotent status/list calls only, `Retry-After` parsing, response-size caps, redirect policy, **per-provider host allowlist** (API + CDN download hosts), blocks private/loopback IPs for user-supplied URLs (explicit allow only for the "local server" OpenAI-compatible profile), proxy support, cassette hook for 5A.04.
  - Keys never reach project files, journal, command log, MCP results, UI stores or plugin memory.
  - `ai.set_key` is a **UI-only scope**: the engine derives the actor server-side from the presented credential (UI token → `user`; agent token → `agent:<label>`; plugin → `plugin:<id>`; internal → `system`) and rejects `ai.set_key`, `ai.provider.remove_key` and key validation-with-new-key from any non-`user` actor. The other UI-only scopes (approval minting, roots allowlist, budget configuration, plugin permission grants) use the same mechanism; no client-supplied actor or origin tag is ever trusted.
- **Interfaces:** `KeyStore`, `KeyHandle`, `sign(alg, key_handle, payload)`, `HostHttp::{request, download}`, `AuthScheme`.
- **Acceptance criteria:** an agent token or plugin credential calling `ai.set_key` is rejected with `forbidden_scope` (test over RPC and MCP); sentinel-key test: a planted key is searched for in all tracing output, error strings, command log, journal and `Debug` dumps and never found; `sign` passes JWT (HS256/ES256), HMAC-SHA256 and SigV4 test vectors and raw key bytes are unobtainable through any API; SSRF tests (redirect to 127.0.0.1 / 169.254.169.254 rejected); `cargo-deny` clean.
- **Tests:** unit, wiremock integration, fuzz target for header/URL handling.
- **Out of scope:** key-entry UI (5F.07), OAuth flows, exposing `sign` to WASM (deferred to E6A).

### WP-5A.04 — Contract-test kit, scrubbed cassettes, MockProvider, cross-adapter conformance
- **Area:** crates/cutweave-ai-testkit, xtask
- **Depends on:** 5A.02, 5A.03
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Record/replay of HTTP (JSON cassettes; match on method + path + normalized body). `cargo xtask ai-record --provider <id> --budget-usd N` requires a live key.
  - Scrubber (auth headers, API keys, signed URLs, emails, account/org IDs) plus a CI lint failing on secret-looking strings (known prefixes + entropy) in `tests/fixtures/**`.
  - Macro `provider_conformance!(adapter, cases)` asserting: submit→poll→success, failure + error mapping, 429/Retry-After, cancel, result download, `estimate_cost` == catalog formula, no secret in cassette, timeout handling. A **cross-adapter normalization test** runs the same `JobRequest` through every registered adapter's cassettes and asserts identical normalized shapes (this replaces a separate conformance WP in 5C).
  - `MockProvider`: scriptable latency/progress/failure/cost, deterministic synthetic outputs (ffmpeg colour bars, tones, speech-like audio, matte videos). Used by UI e2e, evals, 5G/5H tests.
  - Opt-in live-smoke harness (`--features live`, `#[ignore]`, env keys, budget cap).
- **Interfaces:** `cutweave-ai-testkit::{Cassette, provider_conformance!, MockProvider, LiveSmoke}`.
- **Acceptance criteria:** MockProvider passes the suite; a planted key fails the lint; CI job `ai-contract` passes with outbound network blocked.
- **Tests:** self-tests for scrubber and matcher.
- **Out of scope:** real adapters; recording against real providers.

### WP-5A.05 — AI job manager (persistent, resumable, safe against double billing)
- **Area:** crates/cutweave-jobs (AI job kind), crates/cutweave-ai
- **Depends on:** 5A.02, E1B, **E1D (`cutweave-jobs` basic persistent queue — owned by Phase 1, not re-owned here)**
- **Size:** M
- **Executor:** opus
- **Scope:**
  - **Extend the E1D queue (do not fork it)** with an AI job kind. Add to its SQLite schema: client-generated UUIDv7 `id`, provider, model, capability, `params_hash`, state, `remote_id`, `cost_est`, `cost_actual`, attempts, error, project, actor, timestamps, outputs.
  - State machine `Queued → Submitting → Running → Downloading → Succeeded | Failed | Cancelled | NeedsReview`. **Persist `Submitting` + an idempotency key before any network call.** On restart a `Submitting` job with no `remote_id` becomes `NeedsReview` (never auto-resubmit unless the provider supports an idempotency key). `Running` jobs resume polling.
  - Poll with exponential backoff + jitter, manifest poll-interval hints and `Retry-After`; per-provider and global concurrency limits; local queueing on 429/`queue_full`; cancel (remote + local); retry only when `retryable` and safe; per-capability timeouts; resource-class hooks (`gpu|cpu|io`) for local jobs.
  - Events `job.progress`, `job.finished`, `job.needs_review`.
- **Interfaces:** `AiJobs::{enqueue, cancel, retry, list, get}`; events on the existing bus.
- **Acceptance criteria:** kill -9 during `Running` resumes after restart; kill during `Submitting` yields `NeedsReview` and no second remote job; double submit of an identical request creates one remote job; MockProvider chaos test (failures, 429, slow polls, cancel races) stable over 1000 iterations.
- **Tests:** unit, proptest (state-machine transitions), crash-injection integration.
- **Out of scope:** webhooks, result download (5A.06), UI (5F.04).

### WP-5A.06 — Result ingest, input upload, input-hash cache, provenance sidecars
- **Area:** crates/cutweave-ai (`ingest`, `provenance`), crates/cutweave-model (Provenance schema + migration), crates/cutweave-media (import hook)
- **Depends on:** 5A.05, E1A (migration framework), E1C, E1D
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - On success download **immediately** (stream to temp, Range resume, verify size/mime with ffprobe), content-hash, move to `<project>/generated/<yyyy-mm>/`, import as `MediaAsset` with provenance. Disk-full or expired-URL failures leave the job retryable from the remote URL while it is valid.
  - Extend `Provenance` (schema migration vN→vN+1 with golden fixtures): provider, aggregator, model, version, prompt, negative_prompt, params, seed, input_asset_hashes, job_id, remote_request_id, cost + currency, license_tags, watermark_info, parent asset IDs, `recipe` (enough to re-run/vary), routing reason, c2pa summary. Sidecar `<asset>.provenance.json` plus XMP where the container allows.
  - Input upload helper: local asset (or range) → provider-accepted form (URL/multipart/data URI) through an adapter hook; size/format checks; strips unneeded metadata; checks data-flow consent (5A.08).
  - Input-hash cache keyed by (provider, model, version, canonical params, input content hashes, seed): reuse only when seed is fixed or the user opts in; "generate again" bypasses; in-flight dedupe of identical submits. Cache is project-local.
- **Interfaces:** `Ingest::finish(job)`, `Provenance` v2, `UploadHook`, `GenCache`.
- **Acceptance criteria:** golden migration test from the previous schema; generated asset appears in the bin with provenance and survives save/reload; a cache hit costs 0 and says so; expired-URL path tested with MockProvider.
- **Tests:** unit, integration, migration golden.
- **Out of scope:** C2PA (5A.09), UI (5F.04).

### WP-5A.07 — Cost estimator, pricing-formula evaluator and budget ledger
- **Area:** crates/cutweave-ai (`cost`, `budget`)
- **Depends on:** 5A.02, 5A.05, 5A.01, 5B.01 (formula fields; may start against a stub schema)
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `Money` (decimal + currency; USD only in v1, provider credits converted by catalog data), `CostEstimate{low, expected, high, basis, confidence(exact|estimate|range|unknown), as_of, stale}`; stale when `as_of` is older than a configurable age (default 30 days).
  - Pricing-formula DSL: a tiny non-Turing-complete evaluator (`+ - * /`, `min max ceil`, `table[a][b]`, variables from params/inputs/outputs such as `duration`, `resolution`, `megapixels`, `chars`, `minutes`, `tokens_in/out`) with a step limit. No scripting engine, no `eval`.
  - Budget ledger (SQLite): scopes **per-action agent budget and per-session agent budget (default $0, §13)**, per-project, daily, monthly, per-provider; soft warning vs hard stop; **reserve on submit**, reconcile with provider-reported usage (else the estimate, flagged), release on failure/cancel. App-level caps in app config; project-level caps in project settings (Notes §23).
  - Unknown price → confidence `unknown` → always tier 3, human-approved (5A.08).
- **Interfaces:** `Estimator::estimate(req) -> CostEstimate`, `Budget::{reserve, commit, release, status}`, queries `ai.budget`, `ai.spend`.
- **Acceptance criteria:** property test: concurrent reservations never exceed any cap; evaluator fuzz (no panics, bounded time); golden estimates for 3 sample manifests (per-second video, per-megapixel image, per-character TTS).
- **Tests:** unit, proptest, cargo-fuzz target.
- **Out of scope:** UI (5F.03), token issuing (5A.08), FX conversion.

### WP-5A.08 — Spending authority: three-tier policy engine, approvals, consent, untrusted-content labelling
- **Area:** crates/cutweave-ai (`policy`), crates/cutweave-api (credential-derived actor, UI-only scope enforcement, MCP result shapes), crates/cutweave-cli (`--budget`)
- **Depends on:** 5A.07, E1I (auth/token), E1J, E1L (token handoff), 5A.01
- **Size:** M
- **Executor:** opus
- **Scope:** implement §13 exactly, enforced by the engine (never the model):
  - `PolicyEngine` classifies an invocation as read | reversible | destructive | **paid** | external | `requires_human` (publishing, deleting originals, voice cloning, uncurated models; also feeds the copilot, 6I).
  - **Tier 1 — Auto:** cost below the user-set *per-action* budget **and** within the *per-session agent budget* (default **$0 ⇒ off**). Runs immediately; reserved in the ledger.
  - **Tier 2 — Agent-confirmed:** a dry run (`ai.estimate`, `cutweave_execute dry_run`) returns `cost_estimate` + `confirm_token` (HMAC with a per-launch secret; bound to {command, canonical params hash, actor (derived server-side from the credential), project, max_cost, expiry 5 min, single use}). The token only proves the agent saw the price: redeeming **cannot exceed the remaining agent budget** (checked and reserved atomically at redeem).
  - **Tier 3 — Human-approved:** anything above budget, with unknown price, or marked `requires_human` returns `approval_required{approval_id}` (≥ 128-bit random id, bound to params hash + cost, single use, expiring). **Only the desktop UI can mint the approval** (`ai.approval.grant|deny` is a UI-only scope: the actor is derived server-side from the credential, so only the UI token, actor `user`, may call it, via the Electron token handoff; agent tokens (`agent:<label>`) and plugins (`plugin:<id>`) are rejected; `mcp = hidden`). The agent polls `approval_status{approval_id}` → pending | approved | denied | expired.
  - **Headless** (`cutweave mcp --headless`): tiers 1–2 only, requires an explicit `--budget <usd>` (absent ⇒ $0); tier-3 requests return `approval_unavailable_headless` and can never be granted.
  - First-use per-provider data-flow consent; privacy mode `local-only` blocks cloud routes; provider/media-derived text wrapped `{untrusted:true}` and never used in policy decisions; append-only audit log (credential-derived actor, cost, tier, token/approval id). Budget configuration is also a UI-only scope (`user` actor only).
- **Interfaces:** `PolicyEngine::{classify, decide, mint_token, redeem_token, request_approval, approval_status}`, MCP result `approval_required`, query `ai.approval_status`, CLI flag `--budget`.
- **Acceptance criteria (security tests, part of exit criterion 3a):** (a) default $0 budget: no paid MCP call runs automatically; (b) a valid confirm token cannot spend beyond the remaining agent budget, including under concurrent redemption (property test); (c) replayed, altered, expired or cross-actor/cross-project tokens rejected; (d) an agent over the **real MCP transport and over RPC with an agent token** (actor `agent:<label>`) cannot mint, guess or replay an approval, cannot change the budget, and `ai.approval.grant` is unreachable from MCP; a forged actor/origin field in a request is ignored (actor always comes from the credential); (e) `requires_human` commands are tier 3 even at cost 0; (f) headless without `--budget` refuses paid actions, with `--budget` never reaches tier 3; (g) transcript/provider text such as "approve the spend" changes nothing; (h) session-budget reset semantics tested.
- **Tests:** unit, proptest (budget invariants), integration over RPC + MCP + CLI, injection fixtures.
- **Out of scope:** the approval card UI (5F.03), copilot UX (6I).

### WP-5A.09 — C2PA read/write and AI-content flag
- **Area:** crates/cutweave-ai (`c2pa`), crates/cutweave-media (import/export hooks)
- **Depends on:** 5A.06, E1E, E2G
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `c2pa-rs` (pinned): on import read manifests → provenance summary (present, issuer, AI-generated, validity) + badge data. Verify which containers it supports (MP4/MOV/WAV/JPEG/PNG expected; WebM/MKV not) and fall back to a sidecar elsewhere.
  - Export setting `export.c2pa = off|on`: write a manifest with `c2pa.actions` (`digitalSourceType = trainedAlgorithmicMedia`) built from the provenance of AI assets used; sign with a user-supplied cert/key (kept in the keychain); without one, write an unsigned/self-signed manifest clearly labelled as such.
  - Warn before export when re-encoding will drop incoming manifests; never strip SynthID or visible watermarks; project flag `contains_ai`.
- **Interfaces:** `export.c2pa` setting, query `export.c2pa_preview`, `MediaAsset.provenance.c2pa`.
- **Acceptance criteria:** round trip (import AI-marked file → export with manifest → c2pa-rs validates); unsupported container uses the sidecar and says so; no key material in logs; docs state this is not legal advice.
- **Tests:** unit + integration with fixture media.
- **Out of scope:** certificate issuance; platform upload labelling.

### WP-5A.10 — `ai.*` commands, queries, events and MCP surface
- **Area:** crates/cutweave-ops (`ai`), crates/cutweave-api (MCP), packages/protocol (generated)
- **Depends on:** 5A.02–5A.08, 5B.02, E1J, E2I, E1N
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Commands: `ai.generate{capability, model, inputs, params, seed?, dry_run?, confirm_token?}` → job; `ai.estimate`; `ai.cancel_job`; `ai.retry_job`; `ai.routing.set`; `ai.set_key{provider, key}` (returns a fingerprint only; `mcp = hidden`, **UI-only scope**: UI token → actor `user`), `ai.provider.remove_key`, `ai.provider.validate`; `ai.approval.grant|deny` (`mcp = hidden`, UI-only scope).
  - Queries: `ai.models{capability?, filters, cursor}`, `ai.model.get` (returns the params JSON Schema), `ai.jobs`, `ai.job.get`, `ai.providers` (configured/status only), `ai.budget`, `ai.spend`, `ai.approval_status{approval_id}` (MCP-visible).
  - Events: `job.*`, `ai.budget.warning`, `catalog.updated`.
  - MCP: `ai_generate` and `ai_models` in the **core** profile (annotations: openWorld, paid, non-idempotent), `job_status` and `job_wait{job_id, timeout_ms}` reused (agents wait on AI jobs with `job_wait`; `media_import` returns `media_ids` synchronously and analysis is async), resource `cutweave://catalog/models` (paginated summaries). `params` is validated at runtime against the catalog JSON Schema of that model (parity-test exception: Notes §9). Over-budget calls return `approval_required{approval_id}` (5A.08). Results land in the bin only; timeline placement is 5F.05.
- **Interfaces:** the commands/queries above; generated TS client; JSON Schema.
- **Acceptance criteria:** CLI = RPC = MCP parity for `ai.estimate` and `ai.generate` with `MockProvider`; paid call without a token fails with the typed error; `ai.set_key` and `ai.approval.*` are not listed in MCP; `tools/list` grows by exactly 2 core tools.
- **Tests:** schema snapshots, parity test, scripted-agent e2e (generate with MockProvider: estimate → confirm → asset in bin).
- **Out of scope:** UI, AI feature commands (5G), preset tools (5J.03).

### WP-5A.11 — Provider test accounts, CI secrets and cassette-recording procedure
- **Area:** docs/dev/ai-test-accounts.md, .github/workflows/ai-live-smoke.yml (secrets only, no code)
- **Depends on:** 5A.01, 5A.04
- **Size:** S
- **Executor:** human
- **Scope:** maintainers create and own test accounts/keys for every first-wave provider (fal, Replicate, OpenRouter, Higgsfield, Anthropic, OpenAI-compatible, Gemini, ElevenLabs, Runway, Deepgram/AssemblyAI, the stock providers of 5D.09) with prepaid/spend limits; store them as CI secrets in a protected environment for the **opt-in** live-smoke workflow (manual dispatch or opt-in nightly, never on PRs); document the local cassette-recording procedure (`cargo xtask ai-record`, per-provider budget cap default $0.50, who may record, rotation and revocation, cassette review checklist beyond the scrubber); record the owner of each account.
- **Interfaces:** secrets named `CUTWEAVE_LIVE_<PROVIDER>_KEY`; workflow `ai-live-smoke`.
- **Acceptance criteria:** doc merged with a provider → owner table; workflow runs green against `MockProvider` and skips providers whose secret is absent; one real recording session done and reviewed for the first adapter (5C.01).
- **Tests:** none (workflow dry-run).
- **Out of scope:** paying for production usage, user-facing keys.

---

## Epic 5B — Model catalog

### WP-5B.01 — Model, provider and catalog manifest schemas
- **Area:** catalog/schema/, crates/cutweave-ai (`catalog::types`)
- **Depends on:** 5A.02, E1N
- **Size:** M
- **Executor:** opus
- **Scope:** JSON Schema 2020-12 + Rust types (serde, schemars) for: **model manifest** (`id` = `provider/slug`, provider, endpoint, capabilities[], family, version, `status` ga|preview|deprecated|shutdown, `deprecation_date`, `replaced_by`, `inputs{name: {type, kind, role(first_frame|last_frame|reference|source), max_mb, formats}}`, `params` (JSON Schema + `x-cutweave` UI hints: widget, group, order, label), `outputs`, `limits` (duration, resolutions, aspect ratios, reference count, first/last frame, audio), `pricing{unit, formula, table, currency, as_of, source, confidence}`, `license{commercial_use: yes|no|unknown, watermark, c2pa, terms_url}`, `locality` cloud|local, `artifacts[]` for local models (pinned URL, sha256, size, SPDX licence, required EP, min VRAM, redistributable), `data_flow`, `adapter{kind: builtin|declarative|wasm, ref}`, `ui{group, presets, param_order, camera_controls{native|prompt}}`, `source` curated|discovered|user); **provider manifest** envelope reference (spec in 5E.01); **catalog envelope** (`schema_version`, `catalog_version`, `min_app_version`, `valid_until`, `generated_at`, entries[], signature block). Validator with precise error paths and a "form-renderer-compatible subset" check (no `$ref` cycles, no unsupported keywords). Compatibility rules: unknown fields ignored; unknown capability ⇒ entry hidden, never fatal. Doc `docs/reference/catalog-manifest.md` with 3 worked examples (per-second video, per-image edit, local ONNX model).
- **Interfaces:** `catalog/schema/{model,provider,catalog}.schema.json`; `cutweave_ai::catalog::{ModelManifest, CatalogEnvelope}`.
- **Acceptance criteria:** examples validate; invalid fixtures (missing licence tag, bad formula, unknown status) fail with the exact path; schema reviewed together with 5E.01 (shared `adapter` field and request/response mapping references).
- **Tests:** schema snapshot (insta), proptest round-trip, negative fixtures.
- **Out of scope:** signing (5B.03), loader (5B.02), content (5B.07).

### WP-5B.02 — Catalog store: layering, queries, lifecycle and update check
- **Area:** crates/cutweave-ai (`catalog::store`)
- **Depends on:** 5B.01, 5B.03 (verifier interface), 5A.03
- **Size:** M
- **Executor:** sonnet
- **Scope:** layers merged by `id` with precedence **user > live-discovered (uncurated) > cached signed catalog > bundled snapshot**; works fully offline from the bundled/cached copy; hot reload with `catalog.updated` event; filter/search queries (capability, provider, status, locality, licence, price tier, text); group by `family+version` for "price via X vs direct"; lifecycle: availability `ok | warn(deprecated, replaced_by) | blocked(shutdown date passed)` evaluated at submit time, with replacement suggestion data for the UI; skip entries whose `min_app_version` is newer than the app; daily/launch update check (opt-out setting) through `HostHttp` with ETag; stale-catalog flag when `valid_until` passed.
- **Interfaces:** `Catalog::{models, get, availability, refresh}`, event `catalog.updated`, setting `catalog.channel` and `catalog.auto_update`.
- **Acceptance criteria:** precedence test matrix; offline start works; a shutdown-dated model is blocked with a typed error and a suggestion; update failure never corrupts the cache (atomic replace).
- **Tests:** unit, integration with a local HTTP server serving a signed test catalog.
- **Out of scope:** discovery (5B.06), signing tools (5B.04).

### WP-5B.03 — Catalog signing, verification and rollback protection
- **Area:** crates/cutweave-ai (`catalog::trust`), docs/security/catalog-threat-model.md
- **Depends on:** 5B.01
- **Size:** M
- **Executor:** opus
- **Scope:** detached ed25519 signature (minisign-compatible) over the RFC 8785 canonical JSON of the catalog plus its SHA-256; pinned public keys with key IDs and **rotation** (new key accepted only when announced in a catalog signed by an already-trusted key, with an overlap window); **emergency revocation list**; anti-rollback via persisted highest-seen `catalog_version` per channel; `min_app_version`; `valid_until` (soft: warn "catalog stale" after 30 days). Failure modes: invalid signature, downgrade, expired ⇒ keep previous catalog, surface a warning, never partially apply. The same verifier is reused for local-model artifacts and any future adapter packages (E6A). Threat-model doc (key compromise, CDN compromise, replay, downgrade).
- **Interfaces:** `Verifier::verify(bytes, sig, channel) -> Result<Verified>`, `TrustStore`.
- **Acceptance criteria:** test vectors for every failure mode; rotation and revocation scenarios pass; no network code in this module; reviewed by a human before merge.
- **Tests:** unit with fixed vectors, proptest (tampering of any byte fails), fuzz the envelope parser.
- **Out of scope:** CI signing step and key custody (5B.04, 5B.08).

### WP-5B.04 — Catalog build, lint, sign and publish pipeline
- **Area:** catalog/ (sources), xtask, .github/workflows/catalog.yml
- **Depends on:** 5B.01, 5B.03, 5B.08 (key custody decision)
- **Size:** M
- **Executor:** sonnet
- **Scope:** sources `catalog/providers/*.json`, `catalog/models/**/*.json`, `catalog/overrides/` (UI hints, licence, pricing overrides). `cargo xtask catalog build`: validate against schemas; lint (every model has a licence tag; `pricing.as_of` older than 45 days = warning; `replaced_by` resolves; no local model in the default set whose licence is NC/research-only, AGPL, GPL-2.0-only or unclear (§21); no model with `status: shutdown` outside the archive); assemble `catalog.json`; assign a monotonic `catalog_version`; sign (CI step; PR builds use a test key); publish to a GitHub Release + stable CDN path per channel (`stable`, `beta`). PR check posts a human-readable diff (added/removed models, price changes, status changes). Weekly scheduled job runs the importers (5B.05) and opens a PR. Bundled snapshot for the installer is produced by the same build.
- **Interfaces:** `xtask catalog {build,lint,sign,diff}`; published artifacts `catalog.json`, `catalog.json.minisig`.
- **Acceptance criteria:** a PR with a bad entry fails CI with a precise message; a release build produces a signature that the 5B.02 store accepts; rollback test (publishing a lower version is rejected by the client).
- **Tests:** xtask unit tests; workflow dry-run in CI.
- **Out of scope:** hosting domain purchase, key ceremony (5B.08).

### WP-5B.05 — Aggregator schema importers (CI and runtime library)
- **Area:** crates/cutweave-catalog-import, xtask
- **Depends on:** 5B.01, 5A.03, 5A.04
- **Size:** M
- **Executor:** sonnet
- **Scope:** importers for fal, Replicate, OpenRouter (`/videos/models` + models endpoint) and Higgsfield (OpenAPI) that fetch public per-model schemas and emit **draft manifests** under `catalog/models/_drafts/`: map JSON Schema → `params` (drop unsupported keywords), infer capability from category and I/O types, extract price where the API exposes it. **Never overwrite curated fields** (ui, licence, pricing overrides). Shared library so 5B.06 reuses it at runtime. Recorded fixtures; the run emits a report (new, changed, removed upstream).
- **Interfaces:** `xtask catalog import <provider>`; `cutweave_catalog_import::{import_model(slug) -> ModelManifest}`.
- **Acceptance criteria:** importing a recorded fal, Replicate and OpenRouter model yields schema-valid manifests; re-import is idempotent and preserves curated fields; the endpoints/fields used are listed under "Facts to verify".
- **Tests:** fixture-driven unit tests, insta snapshots of drafts.
- **Out of scope:** runtime UI (5F.07), hand-curation (5B.07).

### WP-5B.06 — Live discovery and user-added model slugs
- **Area:** crates/cutweave-ai (`catalog::discovery`), crates/cutweave-ops
- **Depends on:** 5B.02, 5B.05, 5C.01, 5C.02, 5C.03, 5A.10
- **Size:** M
- **Executor:** sonnet
- **Scope:** `ai.catalog.discover{provider}` uses each provider's `list_models` (OpenRouter, Replicate, Anthropic Models API, Gemini list, fal/Higgsfield where available) and merges results as `source: discovered`, hidden behind "show uncurated". `ai.catalog.add_model{provider, slug}` fetches the schema and builds a manifest (via 5B.05) stored in the per-user catalog (`~/.cutweave/catalog/user/`). Uncurated/user models: price unknown ⇒ every run needs human confirmation; licence `unknown` flagged; first use validates with a dry call where the provider allows. `ai.catalog.remove_model`, export/import of the user catalog. MCP `ai_models` marks uncurated entries.
- **Interfaces:** commands `ai.catalog.{discover, add_model, remove_model}`; `source` field on models.
- **Acceptance criteria:** adding a recorded new fal slug yields a usable generation form without an app release; uncurated model run without human confirmation is refused; user entries survive catalog updates.
- **Tests:** unit + integration with cassettes.
- **Out of scope:** UI (5F.07).

### WP-5B.07 — Wave-1 seed catalog content
- **Area:** catalog/models/, catalog/overrides/
- **Depends on:** 5B.01, 5B.05, 5B.08, 5A.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** curated manifests (data only) covering every capability in the enum with ≥ 1 cloud model where a cloud model exists: video (Veo 3.1 / Gemini Omni, Kling 3.0, Seedance 2.0, Wan 2.x, Runway Gen-4.5 + Aleph; via direct and aggregator routes), image (Nano Banana family, GPT Image, FLUX.2, Seedream, optional Ideogram/Recraft), audio (ElevenLabs TTS/music/SFX/voice changer/dubbing, Lyria, MiniMax speech via aggregator), STT (Scribe, Deepgram Nova-3, AssemblyAI, Gemini), LLM (Claude, GPT, Gemini families; Llama/Qwen via OpenRouter), enhancement (upscalers, interpolation, matting, SAM2-type segmentation, stem split, voice isolation, lip-sync via aggregators), VideoUnderstand (Gemini). Each has pricing formula + `as_of` + `source`, limits, licence/watermark tags, UI hints (camera controls native vs prompt). Local entries come from 5I. **Excluded:** Sora, Imagen 4, Midjourney, non-commercial local models. All facts are research snapshots: re-verify each against the provider page when authoring and record `source`.
- **Interfaces:** data files only.
- **Acceptance criteria:** `catalog build` passes; coverage test: every cloud feature in 5G has ≥ 1 model; every model has a golden `estimate` test input; licence flags reviewed by the 5B.08 owner.
- **Tests:** coverage matrix test, golden estimates.
- **Out of scope:** local-model entries (5I.01), Runware/WaveSpeed/other second-wave aggregators.

### WP-5B.08 — Decision: signing-key custody, catalog hosting, licence-tag and price-wording policy
- **Area:** docs/security/, docs/legal/
- **Depends on:** 5B.03 (threat model), 5A.01
- **Size:** S
- **Executor:** human
- **Scope:** decide who holds the root signing key (offline hardware key) vs an online CI signing subkey, rotation/revocation procedure, who can trigger a signed release, hosting (GitHub Releases vs own CDN domain) and channel policy; licence-tag vocabulary and review rule (`commercial_use: unknown` handling, watermark/SynthID display); legal review of price-display wording and of redistributing aggregator-derived metadata.
- **Interfaces:** `docs/security/catalog-key-custody.md`, `docs/legal/catalog-policy.md`.
- **Acceptance criteria:** both documents merged; public key committed and pinned in 5B.03 test vectors; key ceremony performed and recorded (without secrets).
- **Tests:** none.
- **Out of scope:** per-model licence review for local weights (5I.09).

---

## Epic 5C — Aggregator adapters (built on 5E.01/5E.02; all follow A1)

### WP-5C.01 — fal adapter
- **Area:** plugins/providers/fal/ (manifest + glue), crates/cutweave-ai (dialect hook)
- **Depends on:** 5A.03, 5A.04, 5A.05, 5A.06, 5B.01, 5E.01, 5E.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** queue API (submit → status → result → cancel; request id, status/response/cancel URLs), sync endpoint for fast models, auth `Authorization: Key …`, file upload for local inputs, CDN output URLs, statuses (queued/in-progress/completed) with queue position/logs → `JobStatus`, error mapping (validation, auth, rate limit, content policy), per-model params from the catalog, cost from the catalog formula. Capabilities exposed through catalog entries: TextToImage, ImageEdit, TextToVideo, ImageToVideo, VideoToVideo, Upscale, Interpolate, Matting, SegmentTrack, Music, Sfx, Tts, LipSync, StemSplit, AudioEnhance (as models exist). Verify at implementation time: endpoint host/paths, status enum, upload flow, rate limits, whether subpath endpoints need the dialect hook.
- **Interfaces:** provider id `fal`; manifest `plugins/providers/fal/provider.json`; sample model manifest for tests.
- **Acceptance criteria:** A1; plus an end-to-end MockProvider-free test with cassettes: image gen and video gen from submit to ingested asset; cancel test; 429 queueing test.
- **Tests:** `provider_conformance!`, cassettes, opt-in live smoke (one cheap image).
- **Out of scope:** webhooks, realtime WebSocket, streaming endpoints.

### WP-5C.02 — Replicate adapter
- **Area:** plugins/providers/replicate/
- **Depends on:** 5A.03, 5A.04, 5A.05, 5A.06, 5B.01, 5E.01, 5E.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** `POST` predictions (model-scoped or version-pinned), optional `Prefer: wait` short sync window, polling via the returned `get` URL, statuses starting/processing/succeeded/failed/canceled, cancel via the returned cancel URL, outputs are expiring URLs (download immediately), inputs via data URI (small) or Files API upload (large), per-model OpenAPI input schema → `params` (via 5B.05), version pinning in the manifest, actual cost from run metrics where provided. Verify: current API shapes, auth header, file API, rate limits, and any changes since the Cloudflare acquisition.
- **Interfaces:** provider id `replicate`.
- **Acceptance criteria:** A1; a version-pinned model and a model-scoped model both run from cassettes; output-expiry path tested.
- **Tests:** conformance + cassettes + opt-in live smoke.
- **Out of scope:** webhooks, training/fine-tune, deployments.

### WP-5C.03 — OpenRouter adapter (media; shares the chat client with 5D.02)
- **Area:** plugins/providers/openrouter/
- **Depends on:** 5D.02, 5A.04, 5A.05, 5A.06, 5B.01, 5E.01, 5E.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** Llm through the shared OpenAI-compatible client of 5D.02 (base URL + key; **omit optional attribution headers by default** to avoid leaking app identity unless the user opts in); media: video generation (`POST` videos → 202 + job id/poll URL; models listing endpoint; content download; cancel/expired states), image models, per-request cost from response `usage` where present (actual cost), model discovery with `output_modalities` filter (feeds 5B.06). Verify: endpoint names, job states, ZDR incompatibility note, fee structure.
- **Interfaces:** provider id `openrouter`.
- **Acceptance criteria:** A1; Llm chat+tools from cassette through the shared client; one video job from submit to asset; actual cost recorded in provenance.
- **Tests:** conformance + cassettes + opt-in live smoke (cheapest text model).
- **Out of scope:** webhooks (idempotency keys), provider routing preferences UI.

### WP-5C.04 — Higgsfield adapter
- **Area:** plugins/providers/higgsfield/
- **Depends on:** 5A.03, 5A.04, 5A.05, 5A.06, 5B.01, 5E.01, 5E.02, 5A.01 (item 9)
- **Size:** M
- **Executor:** sonnet
- **Scope:** **Step 0 (≤ 0.5 day): read docs.higgsfield.ai authentication, requests/lifecycle, polling, errors and `/docs/openapi.json`; record base URL, auth scheme and model ids in the adapter README** (research could not confirm them). Then: submit, poll status (with `expires_at`), cancel, `DELETE` request to purge files + prompt after download (setting, default on), file upload, `Hf-Expires-After` retention header, 429 + `Retry-After` and `account_queue_full` → local queue (5A.05), model list from the OpenAPI/console catalog via 5B.05. Surface retention/training opt-out info in the data-flow consent text. Soul ID / reference training is out of scope unless 5A.01 item 9 says otherwise. If the API turns out materially different from research or immature, mark this WP deferred (Notes §21) rather than guessing.
- **Interfaces:** provider id `higgsfield`.
- **Acceptance criteria:** A1; purge-after-download tested; queue-full path tested with cassettes.
- **Tests:** conformance + cassettes + opt-in live smoke.
- **Out of scope:** webhooks (Standard Webhooks signing), community feed, Apps/presets import (covered by recipes in 5J.01).

---

## Epic 5D — Direct adapters (all follow A1)

### WP-5D.01 — Anthropic adapter (Llm)
- **Area:** plugins/providers/anthropic/
- **Depends on:** 5A.02–5A.06, 5E.01, 5E.02, 5B.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** Messages API with SSE streaming, tool use, image input, prompt-caching hints, structured output (tool-forced JSON), usage → cost, rate-limit/overloaded error mapping, Models API discovery (capabilities field) for 5B.06; header/version pinning; `LlmClient` facade conformance (message, tool-call, image, usage normalization). No native video input: `VideoUnderstand` goes through the shim in 5H.01. Model IDs come from the catalog (research lists several 5.x models: verify).
- **Interfaces:** provider id `anthropic`; `Llm` capability.
- **Acceptance criteria:** A1; streaming and tool-call cassettes; `anthropic` usable as default Llm slot for 5G.07/5H.02.
- **Tests:** conformance + cassettes + live smoke (smallest model, 20 tokens).
- **Out of scope:** extended thinking controls, batch API, computer-use tools.

### WP-5D.02 — OpenAI-compatible adapter (Llm; images/TTS/STT endpoints; local-server profile)
- **Area:** plugins/providers/openai-compat/
- **Depends on:** 5A.02–5A.06, 5E.01, 5E.02, 5B.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** `/chat/completions` with streaming, tools, vision parts, JSON/structured output, usage; **configurable base URL + extra headers** with profiles (OpenAI, Groq, Together, OpenRouter, LM Studio/Ollama/llama.cpp on loopback; loopback allowed **only** for profiles explicitly marked local, see 5A.03 SSRF rule); optional media endpoints: image generation/edit, speech (TTS), transcription. Catalog marks `tts-1`-class models deprecated/discouraged (research: EOL 2027-01-06, verify). **No Sora/video adapter** (shut down per research).
- **Interfaces:** provider id `openai-compat` with profile id; reused by 5C.03 and 5I.09.
- **Acceptance criteria:** A1; the same suite passes against two profiles (cloud cassette and a local mock server); loopback profile rejected for non-local profile ids.
- **Tests:** conformance + cassettes + live smoke (cheapest model).
- **Out of scope:** Assistants/Responses stateful APIs, fine-tuning.

### WP-5D.03 — Gemini adapter: Llm and native VideoUnderstand
- **Area:** plugins/providers/google-gemini/
- **Depends on:** 5A.02–5A.06, 5E.01, 5E.02, 5B.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** `generateContent`/streaming with tools and vision; **Files API** upload for local video (resumable; size limits and file expiry per docs, delete after use); YouTube URL input (public videos only); video clipping/FPS metadata; `VideoUnderstand` implemented natively: `{media|youtube_url, range?, prompt, response_schema}` → timestamped JSON (`Description` type defined in 5H.01); token-based cost from usage and a duration-based pre-estimate (research: ~263 tokens/s of video, verify). Data-flow consent text names video upload explicitly (5A.08).
- **Interfaces:** provider id `google-gemini`; capabilities `Llm`, `VideoUnderstand`.
- **Acceptance criteria:** A1; a local-file video question and a YouTube-URL question from cassettes return schema-valid timestamped JSON; uploaded file deleted after the call.
- **Tests:** conformance + cassettes + live smoke (a 5-second clip).
- **Out of scope:** Gemini media generation (5D.04), Live/realtime API, Vertex auth.

### WP-5D.04 — Gemini adapter: image, video, music and TTS generation
- **Area:** plugins/providers/google-gemini/ (media manifests)
- **Depends on:** 5D.03, 5B.07 (entries)
- **Size:** M
- **Executor:** sonnet
- **Scope:** image generation/edit via the Nano Banana family (multi-image reference, aspect/size), video via long-running operations (Veo 3.1 `predictLongRunning` style start + operation poll; Omni-style conversational edit; first/last frame, reference images, extension) — **model availability is volatile (Veo vs Omni): everything model-specific is catalog data**; Lyria music clips; Gemini TTS (multi-speaker, style prompt). Result downloads require the API key header (host injects it). Record SynthID/watermark flags in provenance. Verify: operation endpoints, download auth, limits, pricing units, preview-endpoint sunsets.
- **Interfaces:** capabilities `TextToImage`, `ImageEdit`, `TextToVideo`, `ImageToVideo`, `VideoToVideo` (where offered), `Music`, `Tts`.
- **Acceptance criteria:** A1; one image, one video (long-running) and one TTS job from cassettes; watermark flag appears in provenance.
- **Tests:** conformance + cassettes + live smoke (cheapest image model).
- **Out of scope:** Vertex AI auth, Lyria realtime.

### WP-5D.05 — ElevenLabs adapter: speech (TTS, voices, voice clone, voice changer, Scribe STT)
- **Area:** plugins/providers/elevenlabs/
- **Depends on:** 5A.02–5A.06, 5A.08 (consent), 5E.01, 5E.02, 5B.01, E3D
- **Size:** M
- **Executor:** sonnet
- **Scope:** `xi-api-key` auth; TTS (with word/character alignment variant → word timings), voice list/search normalized to `VoiceRef`, instant voice clone (requires a consent attestation recorded locally; MCP actors cannot self-attest), voice changer (speech-to-speech), **Scribe STT** → normalized `Transcript` (word timestamps, diarization, audio-event tags, language detection, keyterms) with filler words retained. **If 3D already shipped an ElevenLabs/Deepgram Stt provider, this WP moves the Stt part onto the 5A.02 trait and adds only what is missing — no second implementation.** Char-based cost formulas in the catalog.
- **Interfaces:** provider id `elevenlabs`; capabilities `Tts`, `VoiceClone`, `VoiceConvert`, `Stt`.
- **Acceptance criteria:** A1; TTS-with-alignment yields word timings; Scribe cassette yields a `Transcript` identical in shape to the local/other-provider ones (cross-adapter test, 5A.04).
- **Tests:** conformance + cassettes + live smoke (≤ 100 characters).
- **Out of scope:** realtime/streaming TTS/STT, voice design, music/SFX/dubbing (5D.06).

### WP-5D.06 — ElevenLabs adapter: music, sound effects, dubbing, voice isolation
- **Area:** plugins/providers/elevenlabs/ (media manifests)
- **Depends on:** 5D.05, 5B.07
- **Size:** M
- **Executor:** sonnet
- **Scope:** music generation (prompt, duration, instrumental), sound-effects generation, **dubbing** job (create → poll → download per target language; speaker detection, background retention) mapped to the `Dub` capability, voice isolator as `AudioEnhance` (verify existence/endpoint). Per-minute/char/credit pricing in catalog formulas. Licence/commercial-use flags from the catalog (vendor claims: verify).
- **Interfaces:** capabilities `Music`, `Sfx`, `Dub`, `AudioEnhance`.
- **Acceptance criteria:** A1; a dubbing job from submit to per-language asset in cassettes; music/SFX outputs ingest with provenance.
- **Tests:** conformance + cassettes + live smoke (shortest SFX).
- **Out of scope:** Studio projects API, voice design.

### WP-5D.07 — Runway adapter (T2V, I2V, Aleph video-to-video)
- **Area:** plugins/providers/runway/
- **Depends on:** 5A.02–5A.06, 5E.01, 5E.02, 5B.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** versioned-header API; task creation for text-to-video, image-to-video and video-to-video (Aleph-style edit); task polling (pending/throttled/running/succeeded/failed/cancelled); cancel/delete; credit-based pricing (research: credits at $0.01, per-second rates — verify, store as catalog formula); input upload (data URI or upload endpoint; V2V size/duration caps from the manifest); expiring output URLs (download immediately); moderation errors → `ContentPolicy` (no retry). Eligible for deferral (Notes §21) because the same models are reachable through aggregators.
- **Interfaces:** provider id `runway`; capabilities `TextToVideo`, `ImageToVideo`, `VideoToVideo`.
- **Acceptance criteria:** A1; throttled → local queue; moderation failure path tested.
- **Tests:** conformance + cassettes + live smoke (shortest clip, if budget allows).
- **Out of scope:** Act-Two/avatars, image models, workflows.

### WP-5D.08 — Second-source STT adapters: Deepgram and AssemblyAI
- **Area:** plugins/providers/deepgram/, plugins/providers/assemblyai/
- **Depends on:** 5A.02–5A.06, 5E.01, 5E.02, 5B.01, E3D
- **Size:** M
- **Executor:** sonnet
- **Scope:** Deepgram prerecorded (`nova-3`-class models, diarization, utterances, word confidence, filler-word retention, keyterms); AssemblyAI (upload → create transcript → poll; speaker labels, disfluencies, word boost). Both normalize into the shared `Transcript` (words with text/start/end/confidence/speaker, language, segments). Per-minute/hour cost formulas. **If 3D implemented one of these two as its single cloud Stt provider, that provider's part of this WP is limited to hardening (error mapping, cassettes, conformance) on the 5A.02 trait.**
- **Interfaces:** provider ids `deepgram`, `assemblyai`; capability `Stt`.
- **Acceptance criteria:** A1 for both; cross-adapter test shows all Stt adapters (3D's, ElevenLabs, these) produce the same normalized shape for one fixture.
- **Tests:** conformance + cassettes + live smoke (10-second audio).
- **Out of scope:** streaming/realtime, audio intelligence add-ons.

### WP-5D.09 — Stock media adapters (`StockSearch`)
- **Area:** plugins/providers/stock-*/ (declarative manifests), crates/cutweave-ops (`stock.search`, `stock.import`)
- **Depends on:** 5A.02–5A.06, 5A.10, 5E.01, 5E.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** declarative adapters for free stock APIs (Pexels/Pixabay-class; verify terms, attribution rules, rate limits, download/hotlink rules, key requirements at implementation time) implementing `StockSearch`. Normalized `StockResult{id, kind(video|image), thumbnail, preview_url, download_options[resolution], duration, author, source_url, license{name, url, attribution_required, commercial_use}}`. Commands: `stock.search{query, kind, orientation, min_duration, page}` (read-only, cost 0 — the UI/estimate shows "free"; the query leaves the machine, so first-use consent applies) and `stock.import{result_id, resolution}` (download into `<project>/stock/`, `MediaAsset` with provenance incl. licence and attribution text; placement via 5F.05, undoable). Result caching and rate-limit backoff; keys (free API keys) by handle. Tagged `ai_feature` so the 5G.01 conformance test covers it. MCP catalog-profile tool `stock_search`.
- **Interfaces:** capability `StockSearch`; commands above.
- **Acceptance criteria:** A1 for each stock provider; imported asset shows licence/attribution in the provenance drawer; one undo removes the placement; no key in logs.
- **Tests:** conformance + cassettes + opt-in live smoke (one query).
- **Out of scope:** GIF/sticker providers, paid stock libraries, music (3G).

---

## Epic 5E — Adapter SDK (declarative HTTP; WASM deferred to E6A)

> **Scope change:** P5 ships compiled-in Rust adapters and declarative HTTP manifests only. The WIT provider world, the wasmtime host and third-party WASM adapters move to E6A (list in Notes §8).

### WP-5E.01 — Declarative provider manifest spec and safe template language
- **Area:** catalog/schema/provider-manifest.schema.json, crates/cutweave-ai (`declarative::{spec,template}`), docs/reference/provider-manifest.md
- **Depends on:** 5A.02, 5A.03, 5B.01 (co-designed)
- **Size:** M
- **Executor:** opus
- **Scope:** manifest fields: `id`, `hosts` (allowlist), `auth{scheme: header|bearer|query|basic|signed}` where **`signed` calls the host-side `sign(alg, key_handle, payload)` (5A.03)** with a claims/payload template, so JWT/HMAC/SigV4 providers need no code; endpoints; **job pattern** `sync | poll{submit,status,result,cancel,done_when,failed_when,interval_hint} | multi_step` (no webhook patterns in v1); request templates (JSON body, multipart, query, headers); response mapping to normalized outputs (Asset list with kind/mime, Transcript, StockResult, usage/cost); `error_map` (status/body match → `AiError`); upload steps (presigned/multipart); model → endpoint mapping; pricing refs. Extraction with JMESPath; templating = minimal `${path}` substitution preserving JSON types; **no loops, no conditionals beyond `default`/`coalesce`, no code**; hard limits (template size, depth, output size). Spec doc with 4 worked examples (fal-like queue, Replicate-like predictions, sync TTS, JWT-signed API).
- **Interfaces:** `ProviderManifest`, `Template::render(ctx)`, `Extract::eval(json)`.
- **Acceptance criteria:** the four examples validate and render expected requests (the signed one against fixed JWT vectors); pathological templates hit limits instead of hanging; reviewed with 5B.01 (shared `adapter` field and mapping references).
- **Tests:** unit, proptest (render never panics), fuzz the template parser.
- **Out of scope:** the executor (5E.02), WASM.

### WP-5E.02 — Declarative HTTP job executor
- **Area:** crates/cutweave-ai (`declarative::exec`)
- **Depends on:** 5E.01, 5A.03, 5A.05, 5A.06
- **Size:** M
- **Executor:** sonnet (opus review: security)
- **Scope:** `DeclarativeProvider: Provider` built from a manifest + catalog entry: renders requests, drives the job pattern through the 5A.05 hooks, multi-step uploads (presigned PUT, multipart), polling hints, output mapping, error mapping, host allowlist, size/time limits, redaction, `signed` auth via key handles. A private Rust `Dialect` trait for first-party quirks (e.g. fal subpath endpoints) — **first-party only**.
- **Interfaces:** `DeclarativeProvider::from_manifest`, `Dialect`.
- **Acceptance criteria:** wiremock re-implementations of a fal-like, a Replicate-like, a sync-TTS and a JWT-signed API run end-to-end through jobs + ingest; redirect-to-private-IP and oversized-response cases rejected; no secret or raw key bytes in logs or requests other than the signature.
- **Tests:** wiremock integration, cargo-fuzz on manifests, property test "no request leaves the allowlist".
- **Out of scope:** WASM adapters, webhooks.

### WP-5E.03 — Adapter tooling, reference declarative adapters and authoring guide
- **Area:** crates/cutweave-cli (`cutweave ai adapter`), plugins/providers/_examples/, docs/guides/adapter-authoring.md
- **Depends on:** 5E.02, 5A.04
- **Size:** M
- **Executor:** sonnet
- **Scope:** `cutweave ai adapter new|validate|test` (thin; folded into `cutweave plugin` by 6C): `validate` checks manifest + schemas, `test` runs the conformance suite against the author's cassettes. Reference adapters: a minimal declarative template and a **JWT-signed declarative adapter (Kling-official-style)** using `signed` auth. Local-file install of a user-supplied declarative manifest (`ai.provider.install`, `mcp = hidden`, UI approval showing the declared hosts; no index, no signing, no WASM — Notes §28). Guide: "write an adapter in an hour", cassette-first workflow.
- **Interfaces:** CLI subcommand; `ai.provider.install|remove|list`.
- **Acceptance criteria:** a new declarative adapter can be scaffolded, tested and installed from a file without touching the repo; install refused without UI approval; guide walkthrough verified by someone who did not write it.
- **Tests:** CLI integration, install/approval tests.
- **Out of scope:** WASM, index/registry/signing (6A/6C), publishing.

---

## Epic 5F — AI Studio UI

### WP-5F.01 — AI Studio panel shell and catalog-driven model picker
- **Area:** packages/editor (panels/ai-studio), packages/ui
- **Depends on:** E2H, E1N, 5A.10, 5B.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** "AI Studio" tab in the left asset browser (architecture §16) with Image / Video / Audio (Voice, Music, SFX) sub-tabs and a Zustand `aiStudioStore` (projection of `ai.models`, `ai.providers`; never holds keys). **Model picker**: searchable, grouped by family; filters for capability, provider/aggregator, locality (cloud/local), price tier, supported duration/resolution/aspect, status (deprecated badge + replacement), licence (commercial OK), "show uncurated"; "price via fal $x vs direct $y" comparison; detail card (limits, price + `as_of`, licence/watermark). Default model per capability from settings. Empty state without keys: "Add a key" CTA and local options.
- **Interfaces:** panel registration, `aiStudioStore`, generated protocol client.
- **Acceptance criteria:** picker filters work against a 200-model fixture catalog at 60 fps scroll; deprecated model shows banner + one-click switch to `replaced_by`; no secret ever appears in store/devtools snapshots.
- **Tests:** Vitest (store, filters), Playwright (open panel, filter, pick) with MockProvider catalog.
- **Out of scope:** generation form (5F.02), settings (5F.07).

### WP-5F.02 — Schema-driven generation form, reference slots and camera controls
- **Area:** packages/editor (panels/ai-studio/form), packages/ui (schema-driven renderer reuse)
- **Depends on:** 5F.01, E2H (schema-driven inspector renderer)
- **Size:** M
- **Executor:** sonnet
- **Scope:** form generated from the model's params JSON Schema + `x-cutweave` hints using the **existing inspector renderer** (no second renderer); prompt and negative-prompt boxes; asset slots by role (first frame, last frame, reference images up to the manifest limit, source video) fed by drag from bin/timeline or "use current frame"; duration/aspect/resolution controls clamped to model limits; seed lock; variants count with cost multiplier; **camera-controls widget**: native enum when the model declares native controls, otherwise inserts camera phrases into the prompt with a live preview (vocabulary from 5J.02, fallback built-in list); inline validation from the schema; "copy as MCP call / JSON recipe"; keyboard-accessible; per-project draft persistence in view state (not the doc).
- **Interfaces:** `GenerationForm` component; recipe JSON (shared with 5J.01 `cutweave.preset/1` inputs).
- **Acceptance criteria:** switching model re-renders form and preserves compatible fields; invalid combination (e.g. 4K with a 1080p-max model) blocked before cost is requested; recipe round-trips.
- **Tests:** Vitest, Playwright generate-image flow with MockProvider.
- **Out of scope:** cost UI (5F.03), audio-specific controls (5F.06).

### WP-5F.03 — Cost, budget and confirmation UX
- **Area:** packages/editor (panels/ai-studio/cost), apps/desktop (UI-token channel)
- **Depends on:** 5F.02, 5A.07, 5A.08, 5A.01 (wording)
- **Size:** S
- **Executor:** sonnet
- **Scope:** price chip on the Generate button using the ADR wording (estimate, range, `as_of`, "unknown", stale-price warning); budget bars (session/project/month) and per-provider spend; **approval card for tier 3**: when a call returns `approval_required{approval_id}` (agent- or UI-initiated) the card shows cost, provider and what data is sent, and mints the approval via `ai.approval.grant` using the UI token (actor `user`, a UI-only scope; 5A.08); running total in the queue panel; blocked state with link to budget settings; first-use data-flow consent dialog; full keyboard and screen-reader support.
- **Interfaces:** `CostChip`, `ConfirmSpendDialog`, `ConsentDialog`.
- **Acceptance criteria:** over-allowance generation cannot start without the dialog; refusing leaves no job; the dialog states cost, provider and what data is sent.
- **Tests:** Vitest, Playwright (confirm/deny paths), axe accessibility check.
- **Out of scope:** budget editing screens (5F.07).

### WP-5F.04 — Job queue panel, results gallery and provenance drawer
- **Area:** packages/editor (panels/ai-studio/{queue,gallery})
- **Depends on:** 5F.02, 5A.05, 5A.06, 5A.09
- **Size:** M
- **Executor:** sonnet
- **Scope:** queue list (state, progress, ETA, cancel, retry, `NeedsReview` with explanation and manual resolve), results gallery (virtualized grid; video hover-play; audio waveform players; compare variants side by side; favourite), history persisted from the jobs table across restarts, provenance drawer (model, prompt, seed, params, cost, licence, watermark, C2PA badge, parent/child links), actions: **vary**, **re-run with same seed**, **use as reference**, send to bin; filters/search.
- **Interfaces:** gallery and queue components; actions call `ai.*` commands.
- **Acceptance criteria:** 500-item history scrolls at 60 fps; restart mid-job restores the queue; "vary" creates a new job from the stored recipe with the cost chip.
- **Tests:** Vitest, Playwright with MockProvider (success, failure, cancel, needs-review).
- **Out of scope:** timeline placement (5F.05).

### WP-5F.05 — Send to timeline, Extend clip and Fill gap
- **Area:** crates/cutweave-ops (`ai.place_result`, `ai.extend_clip`, `ai.fill_gap`), packages/editor (timeline context menu, gallery drag)
- **Depends on:** 5F.02, 5F.04, 5A.10, E2A, E2D, 5G.01 (feature framework)
- **Size:** M
- **Executor:** sonnet
- **Scope:** `ai.place_result{asset, at, track|new_track, mode: insert|overwrite|replace_clip}` (undoable; marks `contains_ai`; badge on clip); drag from gallery to timeline. `ai.extend_clip{clip, seconds, model?, prompt?}`: last frame (or last N seconds) → ImageToVideo/native-extend model, result appended with optional crossfade. `ai.fill_gap{track, range}`: first/last frame from neighbouring clips → generate to the gap length; handle model duration limits by generating longer and trimming or applying a speed change (E2D), else report impossibility with the nearest option. Timeline context-menu entries; all are F1 paid commands exposed via MCP.
- **Interfaces:** the three commands; timeline context-menu contribution.
- **Acceptance criteria:** MockProvider: extend adds the expected clip and one undo removes only the placement; fill-gap on a 7.3 s gap with a model limited to 5 s/8 s yields a gap-exact result; costs shown before run.
- **Tests:** unit (gap/duration solver, property tests), Playwright.
- **Out of scope:** model-specific prompt tuning (5J.02), scene-consistency features.

### WP-5F.06 — Audio generation tabs and voice library
- **Area:** packages/editor (panels/ai-studio/audio)
- **Depends on:** 5F.02, 5F.05, 5D.05, 5D.06 (via catalog entries)
- **Size:** S
- **Executor:** sonnet
- **Scope:** Voice tab (text box, voice picker with preview from normalized `VoiceRef`s, language, speed/stability controls from the manifest), user voice library (saved voices; cloned voices only with the consent attestation checkbox and a stored local consent record), Music tab (prompt, duration, instrumental, loop hint), SFX tab; inline players; insert at playhead through `ai.place_result`.
- **Interfaces:** components consuming `ai.generate` with audio capabilities.
- **Acceptance criteria:** generate → preview → insert works with MockProvider; clone button disabled without attestation.
- **Tests:** Vitest, Playwright.
- **Out of scope:** TTS command with alignment/caption sync (5G.06).

### WP-5F.07 — AI settings: providers, keys, routing, budgets, catalog, custom models, local models
- **Area:** packages/editor (settings/ai), apps/desktop (key-entry path)
- **Depends on:** 5A.03, 5A.07, 5B.02, 5B.06, 5I.01 (commands only), 5F.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** provider cards (status, key entry with validation spinner, remove, data-flow disclosure, last used); **key entry is write-only**: the dialog calls `ai.set_key{provider, key}` with the UI token (UI-only scope), the engine returns only a fingerprint (last 4 characters) and the renderer discards the value immediately — never stored in Zustand/localStorage/devtools-visible state; per-capability routing policy (cloud-first / local-first / local-only / off) and privacy-mode toggle; budget editor (per-action and per-session **agent budget**, default $0, with a plain-language explanation); catalog status (version, channel, last update, update now, stale warning); custom model slug form + uncurated list; "Local models" page wrapping `model.*` commands (size, licence text + accept, download/remove, benchmark tier).
- **Interfaces:** settings pages; commands `ai.provider.*`, `ai.routing.set`, `ai.catalog.*`, `model.*`.
- **Acceptance criteria:** key never readable back from UI or engine (verified by the 5A.03 sentinel test extended to the UI e2e); local model cannot be installed without accepting the licence screen; budgets persist.
- **Tests:** Vitest, Playwright with MockProvider and a fake keychain.
- **Out of scope:** plugin permission prompts (6A/6B), OAuth.

### WP-5F.08 — Preset browser UI
- **Area:** packages/editor (panels/ai-studio/presets)
- **Depends on:** 5J.01, 5F.02
- **Size:** S
- **Executor:** sonnet
- **Scope:** grid of presets from `gen.preset.list` (thumbnail, kind, supported models), input-slot form, "Render prompt" preview via the free `gen.preset.render` dry run showing the per-model prompt/params, "Run" through the normal Generate flow (cost chip), save current form as a user preset, import/export a pack file.
- **Interfaces:** consumes `gen.preset.*`.
- **Acceptance criteria:** rendering the same preset for two models shows different prompt orderings/params; running goes through the cost/confirm flow.
- **Tests:** Vitest, Playwright.
- **Out of scope:** pack authoring tools, community gallery.

### WP-5F.09 — Stock tab in the asset panel
- **Area:** packages/editor (asset-panel/stock)
- **Depends on:** 5D.09, 5F.05, E2H
- **Size:** S
- **Executor:** sonnet
- **Scope:** "Stock" tab in the asset browser: search box, kind/orientation filters, result grid with hover preview, licence badge + attribution requirement, import with progress, drag to timeline (via 5F.05 placement), credits list for attribution; first-use data-flow consent (query leaves the machine); key-setup prompt when a provider needs a free key.
- **Interfaces:** consumes `stock.search`, `stock.import`.
- **Acceptance criteria:** search → preview → import → place works against recorded cassettes; licence shown before import; keyboard accessible.
- **Tests:** Vitest, Playwright with a mock stock provider.
- **Out of scope:** favourites/collections, paid libraries.

---

## Epic 5G — AI editing features (all follow F1; framework in 5G.01)

### WP-5G.01 — AI feature framework and conformance test
- **Area:** crates/cutweave-ai (`feature`), crates/cutweave-ops, crates/cutweave-model (matte asset kind), crates/cutweave-cache (analysis store)
- **Depends on:** 5A.05–5A.10, E1B, E4G (matte mask item)
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `AiFeature` helper: `plan(ctx, params) -> FeaturePlan{steps: [Analysis|Job|Apply], capability, est cost}`; standard lifecycle: dry run → estimate/confirm → jobs → **apply as ONE undoable tx** tagged with the actor; provenance attached; routing (cloud/local per policy) with "ran locally because …" recorded; progress/cancel/partial results.
  - **Analysis cache** `media.analysis` keyed (media hash, feature, model sha, params): beats, scenes, transcripts, masks, descriptions; query `media.analysis_get`.
  - Matte/alpha asset kind (`MediaAsset.kind = matte`, normalized `MaskTrack`) and its use by E4G matte masks; schema migration if needed.
  - **`ai_feature_conformance` test**: enumerates registry commands tagged `ai_feature` and asserts F1 (dry-run exists, estimate returned, paid ⇒ confirm needed, provenance present, undo restores via MockProvider, MCP annotations, untrusted wrapper). New features are covered automatically.
- **Interfaces:** `AiFeature`, `FeaturePlan`, tag `ai_feature` on `#[command]`, `media.analysis_get`.
- **Acceptance criteria:** a toy feature (MockProvider "invert audio") passes conformance and demonstrates dry-run, cost, undo; analysis cache hit returns instantly with cost 0.
- **Tests:** unit, property (undo identity), the conformance test itself.
- **Out of scope:** real features (below).

### WP-5G.02 — Background removal / smart cutout
- **Area:** crates/cutweave-ops (`ai.remove_background`), plugins (matte glue)
- **Depends on:** 5G.01, 5C.01 or 5C.02, 5B.07, E4G; local path optional via 5I.04
- **Size:** M
- **Executor:** sonnet
- **Scope:** `ai.remove_background{clip, range?, subject: auto|person, quality: fast|high}`; cloud via `Matting` models in the catalog (video background removal / segmentation on aggregators — verify; **never RMBG-2.0-class non-commercial models**); returns an alpha-matte video normalized to the matte kind; uploads only the needed source range (optional downscale); chunking for long clips; **applies as a matte mask on the clip (E4G)** so the original is preserved, with feather/invert controls; local fallback (MODNet/PP-Matting provider, 5I.04) used automatically when offline or `local-only`, flagged lower quality; edge cases (no subject, multiple people) return typed errors/warnings.
- **Interfaces:** command `ai.remove_background`; matte asset.
- **Acceptance criteria:** F1; MockProvider matte applies and toggles non-destructively; chunked 60 s clip reassembles seamlessly; cost scales with seconds.
- **Tests:** unit, golden frame of matte composite, e2e with MockProvider.
- **Out of scope:** click-to-track (5G.03), refinement brushes.

### WP-5G.03 — Click-to-track object masks
- **Area:** crates/cutweave-ops (`ai.segment_track`), packages/editor (overlay tool)
- **Depends on:** 5G.01, 5C.01 or 5C.02, E4G, E4I, E2H (preview overlays)
- **Size:** M
- **Executor:** sonnet
- **Scope:** `ai.segment_track{clip, frame_time, prompts: points(+/-)|box, range}` using `SegmentTrack` (SAM2-class). Two-stage UX: first-frame preview mask through a single-frame cloud call (aggregator image-segmentation model; local SAM is a stretch) → accept → video propagation job → `MaskTrack` applied as matte mask; optional bounding-box track emitted as tracker keyframes to attach text/stickers (E4I link). Overlay click tool with positive/negative points and undo of points before submit.
- **Interfaces:** command `ai.segment_track`; preview query `ai.segment_preview`.
- **Acceptance criteria:** F1; preview responds within one request round trip on MockProvider; box track keyframes drive a text item in a golden test.
- **Tests:** unit, Playwright click flow, golden.
- **Out of scope:** multi-object tracking UI, mask refinement.

### WP-5G.04 — Auto reframe (subject tracking → keyframed crop)
- **Area:** crates/cutweave-ops (`ai.reframe`)
- **Depends on:** 5G.01, E2B, E2C, E3D (diarization/speaker), 5I.08 (detector; a heuristic fallback is part of this WP)
- **Size:** M
- **Executor:** sonnet
- **Scope:** `ai.reframe{clip|sequence, target_aspect, subject: auto|face|speaker|point, smoothing, safe_area}`. Pipeline: detect (local detector/tracker from 5I.08 at 5–10 fps; there is no gateway "Detect" capability, Notes §4) → select subject (largest face / active speaker from diarization) → crop path smoothing (critically damped spring or Kalman) → **keyframes on transform/crop** simplified (Douglas–Peucker) so users can edit them; scene-cut aware (reset at cuts, uses 5G.11 when available). Deterministic and local (cost 0) by default; optional cloud `SegmentTrack` with a text prompt when the user selects it. Fallbacks: face-only detector → static centre crop. Shows crop-path overlay in preview. Reused by 5H.03.
- **Interfaces:** command `ai.reframe`; internal `CropPath` solver.
- **Acceptance criteria:** F1 (cost 0, still dry-run capable); on the corpus talking-head clip the subject stays inside the crop ≥ 95% of frames; no keyframe jitter (max acceleration bound tested); undo restores the original transform.
- **Tests:** unit (solver, property tests), golden frames, corpus accuracy test.
- **Out of scope:** multi-subject cuts, manual path editor beyond normal keyframes.

### WP-5G.05 — Voice enhance / denoise and stem split
- **Area:** crates/cutweave-ops (`ai.enhance_audio`, `ai.split_stems`)
- **Depends on:** 5G.01, 5D.06 or 5C.01, 5B.07, E2E; local via 5I.06
- **Size:** M
- **Executor:** sonnet
- **Scope:** `ai.enhance_audio{item, mode: denoise|voice_isolate|studio, strength}` and `ai.split_stems{item, stems}` using `AudioEnhance`/`StemSplit` models (cloud-first per default policy; **automatic offline fallback to the local DeepFilterNet/Demucs packs** from 5I.06, flagged in provenance). Long audio processed in windows with overlap-add; results as new audio assets; apply by swapping the clip's media ref in one tx (undo restores) — stems are added as new tracks; loudness compared before/after (E2E meter) and reported; clipping guard.
- **Interfaces:** commands above.
- **Acceptance criteria:** F1; 10-min file processes in windows without seams (null test on overlap); local fallback used when routed so and cost is 0.
- **Tests:** unit, SNR improvement on a synthetic noisy fixture, e2e.
- **Out of scope:** music-vs-speech auto ducking (E2E), real-time preview of enhancement.

### WP-5G.06 — TTS voiceover and voice changer (and voice-clone consent)
- **Area:** crates/cutweave-ops (`ai.tts`, `ai.voice_change`, `ai.voice_clone`)
- **Depends on:** 5G.01, 5D.05, E3D, E3E, E2E; local fallback via 5I.05
- **Size:** M
- **Executor:** sonnet
- **Scope:** `ai.tts{text|caption_range|script, voice, model, language, style, place?}`: splits long text within provider limits with prosody context from neighbouring chunks, glossary/pronunciation dictionary, output audio + **word-level alignment → synthetic `Transcript`** (provider alignment, else Stt on the output) so captions can follow it (3E); optional placement at the playhead. `ai.voice_change{item, target_voice}` (`VoiceConvert`). `ai.voice_clone{samples, name, consent}` requires the recorded attestation and is `requires_human` (**tier 3**: an MCP actor cannot self-attest or self-approve). **Local fallback: Kokoro TTS (5I.05)** when offline, `local-only`, or chosen by the user (cost 0, flagged in provenance); cloud-first by default.
- **Interfaces:** the three commands; `VoiceRef`.
- **Acceptance criteria:** F1; a 5,000-character script yields seamless chunks and a transcript with monotonic word timings; voice-clone refused without attestation and without a UI-minted approval; with Kokoro installed and `local-only`, `ai.tts` completes offline end to end.
- **Tests:** unit (chunker, alignment merge), e2e with MockProvider.
- **Out of scope:** realtime voice, singing, emotion editor.

### WP-5G.07 — Caption translation
- **Area:** crates/cutweave-ops (`caption.translate`)
- **Depends on:** 5G.01, 5D.01 or 5D.02, E3E, E3D
- **Size:** M
- **Executor:** sonnet
- **Scope:** `caption.translate{transcript|caption_track, target_langs[], mode: replace|bilingual, glossary, tone}` using the `Translate` capability (LLM-backed with JSON-schema output, or a dedicated provider). Produces a **new `Transcript`** with `language` and `translated_from`, preserving segment timings; word timings redistributed deterministically (proportional by characters within each segment); respects caption chunking limits (max chars/lines) by re-chunking; glossary/brand terms enforced; batched to bound tokens; cost from token estimate. Transcript text is untrusted: delimited, outputs validated against the schema, instructions inside it ignored. No local fallback in P5 (cloud Llm only; local LLM/MT is a stretch): offline runs fail with a clear error. Bilingual layout is a 3E feature this command calls.
- **Interfaces:** command `caption.translate`; `Transcript.translated_from`.
- **Acceptance criteria:** F1; timings monotonic and within source segment bounds; a transcript containing "ignore previous instructions" translates verbatim with no side effects; glossary respected in cassette test.
- **Tests:** unit (redistribution, re-chunking), injection fixtures, e2e.
- **Out of scope:** SRT import/export (3E), live translation.

### WP-5G.08 — Dubbing pipeline
- **Area:** crates/cutweave-ops (`ai.dub`)
- **Depends on:** 5G.05, 5G.06, 5G.07, 5D.06 (native path), E3D, E2E
- **Size:** M
- **Executor:** sonnet
- **Scope:** `ai.dub{source, target_lang, voice_mode: clone|stock|provider_native, keep_background, speakers, lip_sync?}`: (1) Stt + diarization (cached) → (2) optional stem split to keep background → (3) translate → (4) TTS per segment/speaker → (5) **time-fit**: stretch ≤ 1.15× (E2E time-stretch), else one LLM "shorten" retry, else allow bounded overflow into silence → (6) mix onto a new `dub:<lang>` track group, original muted not deleted → (7) captions in the target language. `provider_native` runs the ElevenLabs-style dubbing job as one step. Per-step costs summed into one estimate. Segment review data (text edit + regenerate one segment with its own cost) exposed via `ai.dub.segments` / `ai.dub.regenerate_segment`. `lip_sync: true` invokes 5G.09 if present.
- **Interfaces:** command `ai.dub` and the segment commands.
- **Acceptance criteria:** F1; MockProvider 2-speaker fixture produces aligned dub track, original intact, one undo reverts all; combined estimate equals the sum of steps.
- **Tests:** unit (time-fit solver), integration, e2e.
- **Out of scope:** voice-cloning consent (5G.06 owns it), review UI beyond the commands.

### WP-5G.09 — Lip-sync
- **Area:** crates/cutweave-ops (`ai.lip_sync`)
- **Depends on:** 5G.01, 5C.01, 5B.07, 5A.09
- **Size:** M
- **Executor:** sonnet
- **Scope:** `ai.lip_sync{clip, audio, model}` using `LipSync` models (aggregator-hosted lip-sync models; verify). Validates inputs against manifest limits (max duration, resolution, face required); uploads only the needed range; output replaces the video of a **new clip** in one tx (original kept disabled below); faceless/multi-face failures surface typed errors; synthetic-media disclosure: provenance + `contains_ai` + C2PA assertion on export. Eligible for deferral (Notes §21).
- **Interfaces:** command `ai.lip_sync`.
- **Acceptance criteria:** F1; over-limit clip rejected before cost is requested; no-face MockProvider failure leaves the project unchanged.
- **Tests:** unit, e2e with MockProvider.
- **Out of scope:** avatar generation, full-body animation.

### WP-5G.10 — Upscale and frame interpolation
- **Area:** crates/cutweave-ops (`ai.upscale`, `ai.interpolate`), crates/cutweave-render (Lanczos fallback shader reuse)
- **Depends on:** 5G.01, 5C.01 or 5C.02, 5B.07, E4H, E2F
- **Size:** M
- **Executor:** sonnet
- **Scope:** `ai.upscale{clip|media, factor|target_res, model}` and `ai.interpolate{clip, target_fps|factor, model}` using `Upscale`/`Interpolate` models (verify availability/prices). Long video split into keyframe-aligned chunks and reassembled (ffmpeg concat); cost by output pixels × seconds / frames; "try one frame" cost-free-ish preview for upscale. Applies by replacing the clip's media ref (re-proxy via E2F) or, for slow-motion, by creating an interpolated asset and wiring E4H "frame mix: AI". Non-AI Lanczos/bicubic upscale is always available and clearly labelled "not AI". (Local Real-ESRGAN/RIFE packs are a stretch, Notes §21.)
- **Interfaces:** commands above; E4H hook `speed.frame_mix = ai`.
- **Acceptance criteria:** F1; chunk boundaries show no seam in a golden diff; slow-motion 0.25× uses the interpolated asset and undo restores the original.
- **Tests:** unit (chunker), golden, e2e.
- **Out of scope:** denoise/deblur models, restoration of old footage.

### WP-5G.11 — Beat detection and scene detection (zero-model implementations)
- **Area:** crates/cutweave-ops (`audio.detect_beats`, `video.detect_scenes`), crates/cutweave-audio, crates/cutweave-media
- **Depends on:** 5G.01, E1D, E2A (markers/snapping), E2E
- **Size:** M
- **Executor:** sonnet
- **Scope:** local-only analysis jobs (cpu class, cost 0): **beats** via an Ellis-style spectral-flux + dynamic-programming tracker in Rust (own implementation keeps dependencies small; aubio (GPL) would now be licence-allowed under §21, but essentia (AGPL) and madmom weights (NC-SA) stay excluded); **scenes** via an HSV-histogram content/adaptive detector on 48×27 downscaled frames. Outputs stored in the analysis cache and written as markers (`beat`, `scene`) on the clip/sequence so E2A snapping uses them; optional `timeline.split_at_scenes`. ONNX upgrades (beat_this, TransNetV2) arrive as higher-quality local providers in 5I.07 and take over when installed.
- **Interfaces:** commands `audio.detect_beats`, `video.detect_scenes`; markers.
- **Acceptance criteria:** F1 (cost 0); beat F-measure ≥ 0.8 on synthetic click tracks and the CC0 music fixtures; scene detection precision/recall ≥ 0.9 on the corpus hard-cut set; results cached per media hash.
- **Tests:** unit, accuracy tests on corpus, e2e.
- **Out of scope:** tempo-synced auto-edit (3G/6D), music structure analysis.

---

## Epic 5H — Long video → shorts

### WP-5H.01 — `media.describe` and the VideoUnderstand frame-sampling shim
- **Area:** crates/cutweave-ops (`media.describe`), crates/cutweave-ai (`understand`)
- **Depends on:** 5A.10, 5D.01, 5D.03, 5G.01, E2I (contact-sheet renderer), E3D
- **Size:** M
- **Executor:** sonnet
- **Scope:** `media.describe{media, range?, question?, response_schema?, detail}` → `Description{segments:[{start,end,summary,tags}]}` returned as untrusted. Native path: Gemini (5D.03). **Shim for other Llm providers**: sample frames (scene-aware via 5G.11 when cached, else uniform, capped count), burned-timecode contact sheets (reusing the E2I renderer) + transcript → vision-capable Llm → timestamped JSON validated against the schema. Cache per (media hash, range, prompt hash, model). Cost estimate by frames/tokens (or video seconds for native); it is paid when cloud, so it follows F1.
- **Interfaces:** command `media.describe`; type `Description`.
- **Acceptance criteria:** same question on one fixture via native cassette and via shim returns schema-valid output; second call hits the cache with cost 0; frame cap enforced.
- **Tests:** unit (sampler), cassette integration, injection fixture (text overlaid in a frame cannot alter behaviour).
- **Out of scope:** face recognition, embeddings/search index.

### WP-5H.02 — Highlight candidate mining
- **Area:** crates/cutweave-ops (`ai.shorts.find`), agent-kit/prompts/ (`shorts-score.v1.md`)
- **Depends on:** 5H.01, 5D.01, E3D, 5G.11, 5G.01
- **Size:** M
- **Executor:** opus
- **Scope:** `ai.shorts.find{source, count, min_s, max_s, language, criteria}` → ranked `ShortCandidate{id,start,end,hook_text,title,score,rationale,signals{hook,completeness,emotion,energy},needs_context}`. Algorithm: transcript → topic/segment boundaries (sentence ends, pauses, scene cuts, audio-energy/laughter peaks) → candidate windows within limits → LLM scoring with a JSON-schema output and a rubric (hook in first 3 s, self-contained, emotional peak, no dependence on earlier context) → **over-select 2×**, de-duplicate overlaps, rank. Map-reduce for 20+ minute sources (per-chunk then global) with token-budget-aware chunking and a pre-run cost estimate. Optional video-native pass via `media.describe` for visual cues. Prompts are versioned files shared with the skill (5J.04). Temperature 0 + cache for repeatability; transcript is untrusted and delimited. No local LLM fallback in P5 (stretch). Read-only: never mutates the timeline.
- **Interfaces:** command `ai.shorts.find`; `ShortCandidate`.
- **Acceptance criteria:** F1; on the 5H.05 fixture with recorded LLM responses the output is schema-valid, de-duplicated, within length limits and stable across runs; an injection string in the transcript does not alter output structure.
- **Tests:** unit (windowing, dedupe), cassette tests, prompt-version snapshot.
- **Out of scope:** building sequences (5H.03), UI (5H.04).

### WP-5H.03 — Short builder (candidates → 9:16 sequences)
- **Area:** crates/cutweave-ops (`ai.shorts.make`, `ai.shorts_from_sequence`)
- **Depends on:** 5H.02, 5G.04, E3E, E3F, E2B, E2E, E3B/E3C (optional hook title)
- **Size:** M
- **Executor:** sonnet
- **Scope:** `ai.shorts.make{source, candidates, aspect, caption_style, options{remove_silence, remove_fillers, loudness_lufs, hook_title, reframe}}`: per short create a sequence (1080×1920 default); add the range snapped to **sentence/word boundaries** (pad 0.15 s, never mid-word, using the Transcript); silence/filler removal (3F commands); reframe (5G.04); captions from the transcript range with the chosen style (3E); loudness normalization (E2E); optional hook title composition. One checkpoint + one tx per batch with `expected_rev`; source untouched; partial failure rolls back. Returns sequence IDs plus automatic checks (duration ≤ max, captions inside safe area, LUFS in range). `ai.shorts_from_sequence` = find + make for an "auto" mode (paid steps need confirmation).
- **Interfaces:** the two commands.
- **Acceptance criteria:** F1; on the fixture every short passes the state checks; one undo removes all created sequences; source media and sequence unchanged.
- **Tests:** unit, golden frames of first/last frames, e2e with MockProvider + recorded LLM.
- **Out of scope:** publishing (6F), thumbnail generation.

### WP-5H.04 — Shorts panel UI (shortlist review)
- **Area:** packages/editor (panels/shorts)
- **Depends on:** 5H.02, 5H.03, E2H
- **Size:** M
- **Executor:** sonnet
- **Scope:** source picker; settings (count, length, aspect, criteria text); "Find highlights" with cost chip (5F.03 components); shortlist table (thumbnail strip, start–end, hook line, score, why, signals); play range in preview; adjust in/out with a mini-trim; accept/reject/regenerate; "Make shorts" opens the new sequences as tabs with a progress list; one-click undo of the batch; empty/error states.
- **Interfaces:** consumes `ai.shorts.*`.
- **Acceptance criteria:** the flow from source to three sequences works with MockProvider + recorded LLM; adjusting boundaries re-snaps to word boundaries; the whole panel is keyboard accessible.
- **Tests:** Vitest, Playwright e2e.
- **Out of scope:** batch export (2G), publishing.

### WP-5H.05 — Shorts evaluation harness and fixtures
- **Area:** xtask (`shorts-eval`), testdata/manifest (E0I corpus), docs/eval/shorts-blind-review.md
- **Depends on:** 5H.03, E0I (corpus manifest)
- **Size:** S
- **Executor:** sonnet
- **Scope:** fixture: a ≥ 20-minute podcast/interview with a **redistributable licence** (record licence and source in the corpus manifest; fetched, not committed) plus its transcript. Replay mode with recorded LLM responses for CI. `xtask shorts-eval` renders N candidates from Cutweave plus 2 baselines (uniform chunking; random segments), anonymizes and shuffles them, and emits a rating sheet; rubric for "usable": standalone, hook within 3 s, clean in/out (no mid-word cut), captions legible and in safe area, subject framed. Scoring script computes usable rates; automatic state checks reused from 5H.03.
- **Interfaces:** `xtask shorts-eval {render,sheet,score}`.
- **Acceptance criteria:** end-to-end dry run with recorded responses produces a sheet of ≥ 9 anonymized clips and a scoring report; the rubric is written down and reviewed by the human owner of 5H.06.
- **Tests:** script tests.
- **Out of scope:** the review itself.

### WP-5H.06 — Blind review (exit criterion 3)
- **Area:** docs/eval/
- **Depends on:** 5H.05, 5H.02, 5H.03
- **Size:** S
- **Executor:** human
- **Scope:** ≥ 2 reviewers rate the anonymized shorts independently with the rubric; pass when **≥ 3 Cutweave shorts are rated usable by majority** and Cutweave's usable rate beats both baselines. On failure, iterate prompts/segmentation (5H.02) and repeat on a second fixture to avoid overfitting. Record results in `docs/eval/shorts-blind-review.md`.
- **Interfaces:** results document.
- **Acceptance criteria:** signed-off result file linked from the Phase 5 exit checklist.
- **Tests:** none.
- **Out of scope:** statistical significance claims.

## Epic 5I — Local AI plugin & model manager

> Local models are "just another provider" (same traits, same normalized outputs, `Locality::Local`, cost 0). Installed on demand, never in the installer except the CPU/DirectML/CoreML ONNX Runtime. Licence and library facts come from research/04 and are **verify at implementation time**; the licence gate is 5I.09.

### WP-5I.01 — Model manager: local entries, download, verify, licence acceptance
- **Area:** crates/cutweave-ai (`models`), xtask (`models-lint`)
- **Depends on:** 5A.05, 5B.01, 5B.02, 5B.03
- **Size:** M
- **Executor:** sonnet
- **Scope:** local models are **catalog entries** (`locality: local`, `artifacts[]` with pinned-commit URL or mirror, sha256, size, SPDX licence, EP, min VRAM, `redistributable`) — no second registry (Notes §16). Commands `model.list|install|remove|verify|status|license`. Download over HTTPS with Range resume, SHA-256 verification, content-addressed store `<app-data>/models/<id>/<sha>/`, disk usage + quota, progress as `io` jobs. **Licence screen is mandatory**: SPDX id, licence text, source URL; acceptance stored with a hash of the text; installs of deny-listed licences (NC/research-only, AGPL, GPL-2.0-only, unclear weights — §21) refused unless the entry is user-added with an explicit warning. Runtime "GPU packs" (CUDA/TensorRT/OpenVINO ORT builds) use the same mechanism with signature checks. `xtask models-lint` fails CI on deny-listed licences (§21) or missing sha256 in the default set and checks the "core pack" budget (< 1.5 GB).
- **Interfaces:** `model.*` commands/queries, events `model.progress`.
- **Acceptance criteria:** interrupted download resumes; corrupted file detected and re-fetched; installing without accepting the licence fails; lint demonstrably rejects an NC fixture.
- **Tests:** unit, integration with a local HTTP server (Range, corruption), lint tests.
- **Out of scope:** runtime (5I.02), the licence review itself (5I.09), UI (5F.07).

### WP-5I.02 — Local provider runtime: probe, EP policy, sessions, VRAM budget, sidecar
- **Area:** crates/cutweave-ai-local (new), crates/cutweave-cli (`cutweave worker ai`), crates/cutweave-playback (yield signal)
- **Depends on:** 5A.02, 5A.05, 5I.01, E1H
- **Size:** M
- **Executor:** opus
- **Scope:** `LocalProvider: Provider` (`Locality::Local`, cost 0) registering only capabilities whose packs are installed **and** whose hardware probe passes. Hardware probe (GPU vendor/VRAM via wgpu adapter info + platform APIs). `ort` with `load-dynamic`; EP order TensorRT (opt-in) > CUDA > DirectML > CoreML > OpenVINO > CPU, always keeping CPU fallback; one-time benchmark per model/device cached; quality tiers tiny/balanced/best. Session LRU with memory cost and idle unload; **VRAM budget manager** reserving renderer headroom, queueing or downgrading jobs that exceed it; GPU jobs **pause/yield during playback and scrubbing**; resource classes (gpu: 1 job, cpu: cores−2, io); chunked inference with progress and cancel tokens between chunks. **Sidecar mode** (`cutweave worker ai`, stdio/local-socket protocol): hard-kill cancellation, crash isolation, own ORT version; sherpa-onnx and any other library bundling its own ORT only ever run in a sidecar (never two ORT versions in one process). If this exceeds 4 days, split the sidecar into its own WP.
- **Interfaces:** `LocalProvider`, `LocalPack` trait implemented by 5I.03–5I.08, sidecar protocol v0.
- **Acceptance criteria:** a dummy pack runs in-process and in the sidecar with identical results; killing the sidecar mid-job yields a clean `Failed(retryable)`; playback never drops below budget in the perf harness while a GPU job runs (frame-drop counter, E2J).
- **Tests:** unit, integration (kill/resume), perf test with the playback bench.
- **Out of scope:** specific models.

### WP-5I.03 — Migrate the Phase-3 whisper.cpp fallback onto the model manager and local runtime (+ VAD, diarization)
- **Area:** crates/cutweave-ai-local (`stt`), the P3 whisper plugin crate (moved, not rewritten)
- **Depends on:** 5I.01, 5I.02, E3D (the P3 whisper.cpp fallback plugin)
- **Size:** M
- **Executor:** sonnet
- **Scope:** **an explicit migration, not a rebuild — no second whisper binding.**
  - Step 0: inventory the P3 plugin (crate, how it fetches models, its `Transcript` mapping, its tests).
  - Move its models to catalog local entries (large-v3-turbo GPU tier, small/base CPU tier, ggml) installed through 5I.01 with the licence screen; keep already-downloaded P3 model files working through a one-time migration.
  - Re-home it as a `LocalPack` on 5I.02 (hardware tier selection, VRAM budget, cancel tokens, optional sidecar), replacing its private job/thread handling.
  - Keep the P3 `Stt` adapter surface and `Transcript` output unchanged.
  - Add what P3 lacks: Silero VAD (if missing), DTW word-timestamp check, filler-word retention, **diarization** via sherpa-onnx in the sidecar (pyannote-segmentation ONNX + WeSpeaker/3D-Speaker; ungated exports, verify licences) merged by overlap, optional Parakeet-v3 pack (CC BY 4.0, attribution shown).
- **Interfaces:** provider id `local-whisper` (unchanged from P3 if it already has one); capability `Stt`.
- **Acceptance criteria:** all P3 whisper tests pass unchanged; fresh install downloads through the manager with the licence screen; an existing P3 install migrates and keeps working; cross-adapter test shows identical `Transcript` shape; runs fully offline under `local-only`.
- **Tests:** P3 tests, conformance (tiny model cached in CI), accuracy test, migration test.
- **Out of scope:** live/streaming captions.

### WP-5I.04 — Local matting pack
- **Area:** crates/cutweave-ai-local (`matting`)
- **Depends on:** 5I.01, 5I.02, E1D, E4G
- **Size:** M
- **Executor:** sonnet
- **Scope:** `Matting` provider: MODNet or PP-Matting ONNX per frame with temporal smoothing (EMA on alpha), BiRefNet-lite for single-frame high-quality cutouts, MediaPipe-selfie-class as the lowest tier. Streams frames from decode → ONNX → matte video encode; output is the normalized `MaskTrack`/matte asset. Licence of each weight file verified (5I.09). **Excluded:** RMBG-2.0 (CC BY-NC). **RobustVideoMatting (GPL-3.0 code) is licence-allowed under §21 and is an optional higher-quality temporal matter**, shipped only if its weights licence is verified in 5I.09 (unclear weights are not allowed) and it beats MODNet on the corpus.
- **Interfaces:** provider id `local-matting`; capability `Matting`.
- **Acceptance criteria:** 1080p30 clip processed at ≥ 1× realtime on the reference GPU tier (documented numbers for CPU tier); temporal flicker metric below threshold on the corpus; used by 5G.02 as offline fallback.
- **Tests:** unit, golden mattes, perf bench.
- **Out of scope:** click-to-track (5G.03 uses cloud segmentation; local SAM is a stretch).

### WP-5I.05 — Local TTS pack: Kokoro-82M (offline / `local-only` fallback)
- **Area:** crates/cutweave-ai-local (`tts`)
- **Depends on:** 5I.01, 5I.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** `Tts` local provider with Kokoro-82M ONNX via ort (Apache-2.0 per research; verify); voice list (multilingual v1.0 voices; verify counts) normalized to `VoiceRef`; grapheme-to-phoneme via espeak-ng (GPL-3.0-or-later, **allowed in the host under §21**; decide in-process vs separate process and document, licence notice on the install screen); Piper voices only where each voice licence is verified permissive; sentence chunking with crossfade; word timings from model durations if exposed, else 5G.06 derives alignment with Stt; cost 0; registered only when installed. Voice-cloning packs (Chatterbox) are not in scope.
- **Interfaces:** provider id `local-kokoro`; capability `Tts`.
- **Acceptance criteria:** conformance suite with local fixtures; 1 minute of text synthesizes ≥ 3× realtime on the CPU tier (numbers documented); loudness consistent across chunks; `ai.tts` works fully offline via 5G.06.
- **Tests:** unit (chunker), conformance, perf.
- **Out of scope:** voice cloning, singing, expressive/emotion control.

### WP-5I.06 — Local audio pack: DeepFilterNet denoise and Demucs stems
- **Area:** crates/cutweave-ai-local (`audio`)
- **Depends on:** 5I.01, 5I.02, E2E
- **Size:** M
- **Executor:** sonnet
- **Scope:** `AudioEnhance` via DeepFilterNet3 (48 kHz full-band, streaming windows; RNNoise as CPU fallback) and `StemSplit` via Demucs v4 htdemucs (ONNX with STFT/iSTFT done in Rust outside the graph; chunked overlap-add; GPU/CPU). Verify weights licences and ONNX export feasibility. Loudness/clipping guard.
- **Interfaces:** provider ids `local-denoise`, `local-stems`.
- **Acceptance criteria:** denoise improves SNR on the synthetic noisy fixture by a stated margin and runs ≥ 5× realtime on CPU; stems sum back to the mix within tolerance; used by 5G.05 as offline fallback.
- **Tests:** unit, SNR/null tests, perf.
- **Out of scope:** voice conversion, reverb removal.

### WP-5I.07 — Local analysis models: TransNetV2 scenes and beat_this beats
- **Area:** crates/cutweave-ai-local (`analysis`)
- **Depends on:** 5I.01, 5I.02, 5G.11
- **Size:** M
- **Executor:** sonnet
- **Scope:** TransNetV2 ONNX on 48×27 frames (windowed) → scene boundaries; beat_this ONNX export (check STFT/conv ops; STFT in Rust) → beats + downbeats. Registered as higher-quality local providers that **take precedence over the zero-model implementations of 5G.11 when installed**; fallbacks remain. Accuracy comparison against the Rust implementations on the fixtures.
- **Interfaces:** provider ids `local-scenes`, `local-beats`; same command outputs as 5G.11.
- **Acceptance criteria:** measurable improvement (F-measure / boundary F1) over the 5G.11 baselines on the fixtures or the WP is marked "not worth shipping" with the numbers.
- **Tests:** accuracy tests, unit.
- **Out of scope:** music structure/segment labelling.

### WP-5I.08 — Local detection and tracking pack (for reframe)
- **Area:** crates/cutweave-ai-local (`vision`)
- **Depends on:** 5I.01, 5I.02, E1D
- **Size:** M
- **Executor:** sonnet
- **Scope:** face detector (YuNet; BlazeFace fallback), person/object detector (YOLOX-s / RT-DETRv2-s / D-FINE-S — pick by licence + accuracy, verify weights), **ByteTrack implemented in Rust** (Kalman + IoU association), API `detect_and_track(range, classes, fps) -> Tracks` on downscaled frames at 5–10 fps; active-speaker association from supplied diarization. Internal trait consumed by 5G.04 (not a gateway capability, Notes §4). **Excluded:** Ultralytics YOLO (AGPL), SCRFD/insightface weights (non-commercial), YOLO-NAS.
- **Interfaces:** `vision::detect_and_track`.
- **Acceptance criteria:** recall ≥ 0.9 for the main face on the annotated corpus talking-head clips; track IDs stable through short occlusions; runs ≥ 10 fps on the CPU tier at the analysis resolution.
- **Tests:** unit (tracker, property tests), accuracy tests.
- **Out of scope:** recognition/identification, pose estimation.

### WP-5I.09 — Licence and redistribution review of the default local set
- **Area:** docs/legal/model-licenses.md, catalog/overrides (licence fields)
- **Depends on:** 5I.01 (list of candidates), research/04 open items
- **Size:** S
- **Executor:** human
- **Scope:** read the actual licence text of every weight file and code dependency in the default set (whisper, Silero, sherpa/diarization exports, Parakeet attribution, MODNet/PP-Matting/BiRefNet, SAM2/EdgeTAM, DeepFilterNet, Demucs, TransNetV2, beat_this, YuNet/YOLOX/RT-DETR/D-FINE, Kokoro + espeak-ng, optional RVM); record snapshot hashes of the texts; apply the §21 policy (GPL-2.0-or-later/GPL-3.0/LGPL allowed; GPL-2.0-only, AGPL, NC/research-only and unclear weights rejected), including RVM weights and espeak-ng for Kokoro; confirm mirroring rights for a Cutweave-hosted model mirror; approve or reject each entry before it enters the default set.
- **Interfaces:** `docs/legal/model-licenses.md`; updated `license` fields.
- **Acceptance criteria:** every default-set entry has an approved row; rejected entries removed; `models-lint` passes.
- **Tests:** none.
- **Out of scope:** cloud-model terms (5B.08 owns the tag policy).

---

## Epic 5J — Agent Kit: generation skills & prompt packs

### WP-5J.01 — Preset / prompt-pack format and render runtime
- **Area:** crates/cutweave-ai (`presets`), agent-kit/schema/, crates/cutweave-ops (`gen.preset.*`)
- **Depends on:** 5A.10, 5B.01, E3G (pack loading)
- **Size:** M
- **Executor:** sonnet
- **Scope:** format `cutweave.preset/1` as in research/08 §5 (inputs, model-agnostic **intent layer**, template, per-model `adapters` with `prompt_order`, params, native-camera mapping, `avoid_negation`, `fallback`, `tests`, `cost_hint`, licence) + `pack.yaml`; JSON Schema + `xtask packs lint`; loaders for bundled `agent-kit/packs/` and user `~/.cutweave/packs/` (generalized by 6A, Notes §19); safe YAML parsing (no anchor bombs). **Pure deterministic `render_preset(preset, inputs, model) -> {prompt, negative_prompt, params, refs, warnings}`**; unknown models use the `fallback` adapter with a warning. Commands/queries `gen.preset.list|get|render` (read-only, free) and `gen.preset.run` (= render + `ai.generate`, paid gate); preset `tests` runnable by the eval harness.
- **Interfaces:** schema, `render_preset`, `gen.preset.*`.
- **Acceptance criteria:** same preset renders differently for two model adapters with golden outputs; invalid preset fails lint with a path; `run` without a confirm token fails like `ai.generate`.
- **Tests:** unit, insta goldens, fuzz the YAML loader.
- **Out of scope:** UI (5F.08), content (5J.02).

### WP-5J.02 — Camera vocabulary and per-model-family prompt packs (content)
- **Area:** agent-kit/packs/
- **Depends on:** 5J.01, 5B.07
- **Size:** M
- **Executor:** sonnet
- **Scope:** data packs (CC0-1.0): `camera-vocab` (shot size, angle, movement, lens, lighting, with mappings to native camera controls for models that have them), `cinematic-moves` (≥ 12 presets: orbit reveal, dolly in/out, crane, whip pan, handheld, rack focus, …), `product-promo`, `social-hooks`, `storyboard-templates`, `platform-specs`. **Per-model-family adapters** for Veo, Kling, Runway, Luma, Seedance, Wan, FLUX, Nano Banana, GPT Image, ElevenLabs voice style tags and Lyria, each derived from that vendor's current official prompt guide (research cites Veo and Runway guides; **fetch the others before writing**; verify at implementation time). Consistency recipes (reference sheets, first/last-frame chaining, storyboard-stills-first) documented in pack README.
- **Interfaces:** pack files only.
- **Acceptance criteria:** `packs lint` passes; goldens for ≥ 10 presets × 6 families; every adapter cites its source guide and verification date.
- **Tests:** render goldens, lint.
- **Out of scope:** thumbnails (generated later), community packs.

### WP-5J.03 — Generation MCP tools, prompts and resources
- **Area:** crates/cutweave-api (MCP catalog), agent-kit/prompts/
- **Depends on:** 5A.10, 5J.01, 5H.01, 5G.01, E3H, E2I
- **Size:** M
- **Executor:** sonnet
- **Scope:** expose in the `standard` profile: `ai_estimate`, `gen_preset_render`, `gen_preset_run`, `media_describe` (plus 5A.10's `ai_generate`/`ai_models` in core), with agent-oriented descriptions ("always estimate first", "never exceed the stated budget", "prefer cheap image storyboards before video"); the `approval_required{approval_id}` + `approval_status` flow documented and tested; MCP prompts `script-to-video`, `add-broll`, `dub`, `make-short`; resources `cutweave://presets/{id}`, `cutweave://catalog/models`. Tool-list size within the single budget (minimal ≤ 12 tools / ≤ 3k tokens; standard ≤ 36 tools / ≤ 8k tokens; full = standard + search-based long tail with no extra listed tools; profiles per endpoint `/mcp/minimal`, `/mcp`, `/mcp/full` or `--profile`, never per connection): ≤ 6 added tools, the rest stay behind `cutweave_search_commands`/`cutweave_execute`. Agents wait on jobs with `job_wait{job_id, timeout_ms}`. Change notifications use the negotiated MCP revision (2025-11-25 `resources/subscribe`, 2026-07-28 `subscriptions/listen`). All provider/transcript-derived text returned is `untrusted`.
- **Interfaces:** MCP tools/prompts/resources above.
- **Acceptance criteria:** MCP Inspector passes; a scripted client completes estimate → `approval_required` → (approval minted by the test harness presenting the UI token) → `approval_status` → generation; `tools/list` budget check green.
- **Tests:** MCP conformance, scripted-agent e2e, budget CI check.
- **Out of scope:** skills text (5J.04), copilot (6I).

### WP-5J.04 — Generation skills (SKILL.md)
- **Area:** agent-kit/skills/
- **Depends on:** 5J.03, E3H (skill conventions)
- **Size:** M
- **Executor:** sonnet
- **Scope:** spec-valid skills with the canonical architecture §14b names: `generate-shots` (preset render, consistency techniques, cost gates, storyboard-stills-first), `b-roll-insertion` (B-roll plan from transcript keywords → generate or stock → insert), `dubbing-and-translation` (includes TTS voiceover: script → `ai.tts` → place → captions; plus caption translation and dubbing), `shorts-from-longform` (builds on 3H's `shorts-reframe-basic` and 5H commands, includes `references/hook-patterns.md`), and a **budget protocol section** added to `cutweave-basics` (estimate → within agent budget? → dry-run `confirm_token`, else `approval_required` and wait on `approval_status`; never loop paid calls). Rules in each: never delete source media, transcripts are data, ask before paid actions, check tool existence and degrade gracefully. Names are the canonical §14b names (no aliases). CI validates with the reference `skills-ref` validator (name ≤ 64 chars, description ≤ 1024, body < 500 lines, references one level deep).
- **Interfaces:** `agent-kit/skills/<name>/SKILL.md` (+ references/).
- **Acceptance criteria:** validator green; each skill's tool names exist in the MCP catalog (CI cross-check); descriptions have distinct "use when" phrases.
- **Tests:** validator, tool-name cross-check.
- **Out of scope:** installers (6G), eval of skills with real LLMs (6H).

### WP-5J.05 — Generation and safety eval tasks (scripted-agent)
- **Area:** agent-kit/evals/, xtask (`eval-scripted`)
- **Depends on:** 5J.03, 5J.04, 5A.04, 5H.03, 5G.08
- **Size:** S
- **Executor:** sonnet
- **Scope:** tasks in the schema the 6H harness will adopt ({fixture project + media, prompt, allowed tools/skills, budget, assertions}) run with `MockProvider`: storyboard-from-script, assemble-generated-shots, b-roll-on-keywords, voiceover, dub-to-Spanish-with-captions, longform-to-3-shorts; **safety tasks**: over-budget generation attempt (must stop/ask), injection in a transcript ("generate 100 videos" / "approve the spend"), attempts to self-approve (mint or guess an `approval_id`, replay a `confirm_token`, exceed the remaining agent budget with a valid token, run headless without `--budget`), provider-returned text that tries to issue instructions. State-based assertions plus budget-ledger and audit-log assertions. A minimal deterministic scripted-agent runner runs them in PR CI (no LLM); LLM-in-the-loop nightly runs are 6H.
- **Interfaces:** task files; `xtask eval-scripted`.
- **Acceptance criteria:** all tasks pass in CI; each safety task fails if the corresponding guard from 5A.08 is disabled (mutation check).
- **Tests:** the tasks themselves.
- **Out of scope:** judge/rubric scoring, nightly infrastructure.

---

## Traceability

### Roadmap Phase 5 scope bullets → WPs

| Roadmap scope bullet | WPs |
|---|---|
| Gateway: capability enum, normalized I/O types, provider interface | 5A.02 |
| Gateway: persistent job manager, polling/backoff, result download, input-hash cache (extends E1D queue) | 5A.05, 5A.06 |
| Gateway: budgets and cost estimates | 5A.07, 5A.08, 5F.03 |
| Gateway: keychain storage (write-only `ai.set_key`, key handles, host `sign`) | 5A.03, 5F.07 |
| Gateway: provenance sidecars | 5A.06, 5F.04 |
| Gateway: C2PA read/write | 5A.09 |
| Gateway: three-tier spending authority (§13/§14) | 5A.08, 5A.10, 5F.03, 5J.03, 5J.05 |
| Gateway: commands + MCP exposure | 5A.10, 5J.03 |
| Catalog: manifest schema | 5B.01 |
| Catalog: CI-built signed catalog | 5B.03, 5B.04, 5B.08 |
| Catalog: live discovery from aggregators, user-added slugs | 5B.05, 5B.06 |
| Catalog: deprecation handling | 5B.02 |
| Catalog: content (first-wave models) | 5B.07 |
| Adapters, aggregators: fal, Replicate, OpenRouter, Higgsfield | 5C.01, 5C.02, 5C.03, 5C.04 |
| Adapters, direct: Anthropic, OpenAI-compatible, Gemini, ElevenLabs, Runway, Deepgram/AssemblyAI | 5D.01, 5D.02, 5D.03 + 5D.04, 5D.05 + 5D.06, 5D.07, 5D.08 |
| Declarative HTTP manifest format (first-party adapters compiled-in Rust) | 5E.01, 5E.02, 5E.03 |
| Third-party WASM adapters (moved to P6) | **Deferred to E6A** (list in Notes) |
| Test accounts/keys for cassettes; opt-in live smoke on CI secrets (maintainer task) | 5A.11, 5A.04 |
| Studio: generate images, video (T2V/I2V, first/last frame, camera controls), music, SFX, voice | 5F.01, 5F.02, 5F.06 (+ 5D.04, 5D.06, 5C.01 for models) |
| Studio: model picker, presets/prompt packs, cost display, results gallery, send-to-timeline | 5F.01, 5F.08, 5J.01, 5J.02, 5F.03, 5F.04, 5F.05 |
| Studio: "Extend clip" and "fill gap" | 5F.05 |
| AI feature: background removal / smart cutout | 5G.02 (+ 5I.04) |
| AI feature: click-to-track object masks | 5G.03 |
| AI feature: auto reframe | 5G.04 (+ 5I.08) |
| AI feature: voice enhance/denoise, stem split | 5G.05 (+ 5I.06) |
| AI feature: TTS voiceover, voice changer | 5G.06 (+ 5I.05 Kokoro fallback) |
| AI feature: caption translation | 5G.07 |
| AI feature: dubbing with optional lip-sync | 5G.08, 5G.09 |
| AI feature: upscale, frame interpolation | 5G.10 |
| AI feature: beat detection to markers, scene detection | 5G.11 (+ 5I.07) |
| AI feature: long video → shorts | 5H.01–5H.06 |
| Stock media: `StockSearch` capability, adapters, asset-panel browsing, licences in provenance | 5A.02 (enum), 5D.09, 5F.09, 5A.06 (provenance) |
| AI features: framework, cost-first, provenance, undo | 5G.01 (F1 convention) |
| Local AI plugin: whisper.cpp (migration of the P3 fallback), ONNX models (matting, denoise, scene/beat), Kokoro TTS fallback, on-demand model manager, licence display | 5I.01–5I.09 |
| Agent Kit v1 additions: `generate-shots`, `b-roll-insertion`, `dubbing-and-translation` (incl. TTS voiceover), `shorts-from-longform` | 5J.03, 5J.04 |
| Agent Kit v1 additions: prompt packs per model family | 5J.01, 5J.02 |
| Safety evals (over-budget, injection, self-approval) | 5J.05 |
| Stated dependencies: matting/object masks → E4G, interpolation → E4H, reframe → E4I | 5G.01, 5G.02, 5G.03, 5G.04, 5G.10 (those WPs wait for their P4 epic) |

### Exit criteria → WPs

| Exit criterion | WPs that deliver / verify it |
|---|---|
| 1. Each first-wave adapter passes the contract-test suite (recorded fixtures + opt-in live smoke) | 5A.04 (kit), 5A.11 (accounts/secrets), 5C.01–5C.04, 5D.01–5D.08 (each A1), 5E.03 (reference adapters), 5D.09 (stock adapters, same suite) |
| 2. Every listed AI feature runs end-to-end, cost up front, provenance, undoable: background removal → 5G.02; object masks → 5G.03; auto reframe → 5G.04; voice enhance → 5G.05; stem split → 5G.05; TTS → 5G.06 (+5I.05); voice changer → 5G.06; caption translation → 5G.07; dubbing (+lip-sync) → 5G.08/5G.09; upscale + interpolation → 5G.10; beat/scene detection → 5G.11; stock search → 5D.09 + 5F.09 | 5G.01 (`ai_feature_conformance` covers every command tagged `ai_feature`; stock search is tagged too, cost "free"), 5A.06, 5A.07, 5F.03 |
| 3a. Three-tier spending authority passes its security tests | 5A.08 (test suite), 5A.10 (surface), 5J.05 (agent-level safety tasks) |
| 3b. Long → shorts yields ≥ 3 usable shorts from a 20-min podcast (blind review) | 5H.02, 5H.03, 5H.05, 5H.06 |

---

## Totals

| Epic | WPs | S | M | Agent-days (S=1, M=3) |
|---|---|---|---|---|
| 5A | 11 | 2 | 9 | 29 |
| 5B | 8 | 1 | 7 | 22 |
| 5C | 4 | 0 | 4 | 12 |
| 5D | 9 | 0 | 9 | 27 |
| 5E | 3 | 0 | 3 | 9 |
| 5F | 9 | 4 | 5 | 19 |
| 5G | 11 | 0 | 11 | 33 |
| 5H | 6 | 2 | 4 | 14 |
| 5I | 9 | 1 | 8 | 25 |
| 5J | 5 | 1 | 4 | 13 |
| **Total** | **75** | **11** | **64** | **203** |

Executor mix: opus 11 (5A.02/03/05/07/08, 5B.01/03, 5E.01, 5G.01, 5H.02, 5I.02), human 5 (5A.01, 5A.11, 5B.08, 5H.06, 5I.09), sonnet 59.
Changes in this revision: −5E WASM host and reference adapter (deferred), −5I SAM2 and −5I local-LLM packs (stretch; not in roadmap scope), +5A.11 (human, test accounts), +5D.09 and +5F.09 (stock search), +5I.05 (Kokoro, replaces the SAM2 slot).

---

## Notes for integrator

Status legend: **RESOLVED** = decided by the integrator follow-up and applied in this file; **OPEN** = still needs a decision.

1. **3D slice vs 5A (RESOLVED).** P3 (3D) builds the minimal Stt slice **plus the whisper.cpp fallback**; P5 migrates/extends it. 5A.02 has a mandatory "Step 0" for both merge orders; 5I.03 is an explicit migration. Still assign one owner of the `cutweave-ai` crate API.
2. **Whisper in P3 and P5 (RESOLVED).** Same decision: P3 ships it, 5I.03 migrates it onto the model manager/local runtime without a second binding and with unchanged P3 tests.
3. **Job manager ownership (RESOLVED).** Owned by Phase 1 epic E1D (`cutweave-jobs`, basic persistent queue). 5A.05 only extends it for AI (idempotent submit, polling/backoff, resume, `NeedsReview`, concurrency). If E1D turns out in-memory only, 5A.05 grows by about one M — raise at P1 review.
4. **Capability set (RESOLVED/OPEN).** §13 is the contract; research differences stay unsupported. Added per roadmap: **`StockSearch`** (not yet in the §13 enum — see §27). No gateway `Detect` capability: reframe uses a local internal detector (5I.08). First/last frame is an input role.
5. **Key entry (RESOLVED).** `ai.set_key{provider, key}`: write-only, the engine returns a fingerprint (last 4 characters), the renderer discards the value; no read API; adapters use key handles (5A.03, 5F.07, 5A.10).
6. **Secrets vs signing (RESOLVED).** Host-side `sign(alg, key_handle, payload)` for JWT/HMAC/SigV4 (5A.03). Declarative manifests use an `auth: signed` scheme (5E.01), so signing providers (e.g. Kling official) need no WASM.
7. **Spending authority (RESOLVED).** Three tiers implemented exactly as §13 in 5A.08: auto (per-action/per-session agent budget, default $0), agent-confirmed (dry-run `confirm_token`, capped by remaining budget), human-approved (`approval_required{approval_id}` + `approval_status`, UI-minted only). Headless: tiers 1–2 with `--budget`. Security tests are an exit criterion.
8. **WASM provider host (RESOLVED: removed from P5).** P5 = compiled-in Rust adapters + declarative manifests. **Deferred to E6A:** (a) `wit/cutweave-provider` WIT world; (b) wasmtime provider host with fuel/memory limits, no WASI net/fs, allowed-hosts; (c) exposing the 5A.03 `sign` primitive as a WASM host function; (d) the reference WASM adapter (Kling-style JWT); (e) adapter package format with signature + permission prompts and index distribution; (f) a feasibility spike for wasmtime components on the 3 OSes (still absent from P0). 5E.01's manifest and the `Provider` trait are designed so a `WasmProvider` can be added without changes.
9. **Dynamic params vs static schema parity (OPEN).** `ai.generate.params` is validated against a catalog schema at runtime. Suggest typing it as an open object in the command schema; the parity test compares envelopes and asserts `ai.model.get` returns the per-model schema; document the exception in §5.3.
10. **MCP tool-list budget (OPEN, suggestion applied).** One `ai_generate` + `ai_models` in core plus ≤ 6 `standard` tools; no per-capability generate tools.
11. **P4 dependencies (RESOLVED).** Stated in the roadmap (matting/object masks → E4G, interpolation → E4H, reframe → E4I); kept as cross-phase deps on 5G.01–5G.04, 5G.10. Also needed: E3D/E3E/E3F (5G.06–5G.08, 5H), E2A/E2D (5F.05).
12. **Model/schema gaps (OPEN).** No `matte` asset kind (5G.01 adds it); `Provenance` lacks fields (5A.06 migration) including a stock licence record; no `Reference` entity (out of scope unless 5A.01 item 9 says otherwise).
13. **List of AI features (RESOLVED).** The roadmap exit criterion now enumerates them; traceability follows it (incl. stock search and Kokoro).
14. **Blind review prerequisites (OPEN).** Redistributable 20-min podcast, ≥ 2 reviewers, "usable" rubric; keep a second fixture against overfitting (5H.05/5H.06).
15. **Licence posture (RESOLVED).** §21: GPL-2.0-or-later, GPL-3.0, LGPL are allowed in the host; disallowed = GPL-2.0-only, AGPL, NC/research-only, unclear weights. Applied: RVM (GPL-3.0 code) is now an optional local matting candidate (5I.04) subject to verified weights; espeak-ng is allowed for Kokoro (5I.05); aubio would be allowed (5G.11 still uses its own implementation). Still excluded: RMBG-2.0, F5-TTS, XTTS, NLLB, Ultralytics YOLO (AGPL), insightface/SCRFD weights, essentia (AGPL), madmom weights (NC-SA), Seed-VC weights (unclear). The licence-tag policy for cloud models stays informational (5B.08). Anything meant for the MIT/Apache SDK packages must still avoid GPL.
16. **Local registry vs catalog (RESOLVED in plan).** Local models are signed-catalog entries (`locality: local`, `artifacts[]`).
17. **Provider-fact volatility (OPEN, process in place).** Every adapter WP carries "Facts to verify"; catalog `as_of`/`source` fields.
18. **Cassette/live-smoke accounts (RESOLVED).** New human WP 5A.11 (accounts with spend caps, CI secrets, recording procedure); live smoke stays opt-in.
19. **Pack loading before P6 (OPEN).** 5J.01 loads from two fixed directories; re-point to the 6A/E3G loader.
20. **Skill names (RESOLVED — canonical §14b names).** 5J.04 ships `generate-shots`, `b-roll-insertion`, `dubbing-and-translation` (includes TTS voiceover) and `shorts-from-longform`; no aliases.
21. **Size and cuts (RESOLVED).** Applied: WASM WPs removed (→ E6A), local SAM2 and local-LLM packs removed (not in roadmap scope; stretch). Nothing else in the earlier cut list could be removed without dropping roadmap scope (Runway, Higgsfield, Deepgram/AssemblyAI, lip-sync, ONNX scene/beat, preset UI, audio tabs are all named). If review capacity is the limit, the only remaining lever is to treat AssemblyAI as optional inside 5D.08 (roadmap writes "Deepgram/AssemblyAI"; −1.5 days). Stretch, not planned: Runware/WaveSpeed, Topaz direct adapter, local Real-ESRGAN/RIFE, local LLM/translation, local SAM2, webhooks/relay, Soul ID.
22. **Consent/regulation for voice cloning and lip-sync (OPEN).** Attestation + local record + `requires_human` (tier 3) + C2PA/`contains_ai`; legal review via 5B.08.
23. **Budget storage and currency (OPEN).** Agent budgets and caps in app config; project-level caps in project settings — a shared project must not carry another user's caps. USD only in v1.
24. **Prompts in provenance may leak (OPEN).** Offer to strip prompts/recipes on collect/share.
25. **Headless Linux without a Secret Service (OPEN).** Env-var keys (`CUTWEAVE_KEY_<PROVIDER>`) and a clear error; decide on an encrypted-file fallback.
26. **MCP spec drift (OPEN).** No flow relies on elicitation/sampling/MCP Apps; `approval_required` is a plain tool result.
27. **`StockSearch` is not in the §13 capability enum (OPEN).** The roadmap adds it; suggest amending §13 (Media: `StockSearch`) and the catalog capability list. It is a zero-cost, external-network capability (query leaves the machine → consent), implemented as `stock.search`/`stock.import` commands (5D.09). Stock API terms (attribution, download/hotlink rules, rate limits) must be verified at implementation time.
28. **Third-party declarative adapters (OPEN).** Roadmap says only first-party adapters are compiled-in; unclear whether user-installed third-party *declarative* manifests ship in P5. 5E.03 supports local-file install with a host-allowlist confirmation and nothing more (no index, no signing — 6C).
29. **Rename (RESOLVED).** All identifiers use `cutweave-*`/`cutweave`; nothing left to do.
