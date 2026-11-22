# 06 - Cloud AI Providers & Integration Architecture for Cutweave

Research date: 2026-11-22. Scope: cloud-first generative/AI integrations (bring-your-own-key and aggregators). Local models are out of scope (covered separately; they are "just another provider").

## 0. How to read this report

- Evidence labels: **[V]** verified on an official/primary page fetched during this research; **[S]** secondary source (vendor blog, reseller, news) - directional only; **[U]** unverified / conflicting / from prior knowledge - re-check before committing.
- "As of" dates are the date the source was read (2026-11-22) unless stated. Pricing in this market changes monthly: **never hardcode prices**; treat them as catalog data (see section 5).
- Big finding: **the landscape moved fast in 2026.** OpenAI shut down Sora 2 and the Videos API (2026-09-24), Google's Imagen 4 is shut down, Veo is no longer on the Gemini model list page (replaced by Gemini Omni for video), and Higgsfield launched a public API. Any model list older than ~3 months is stale. This argues strongly for the remote-catalog design in section 5.

## 1. Executive summary

1. **Aggregators first, direct adapters second.** fal.ai, Replicate, OpenRouter, Runware, WaveSpeed and Higgsfield's own API each expose dozens-to-hundreds of media models behind one key and one async pattern. One "fal" adapter + one "Replicate" adapter + one "OpenRouter" adapter covers most video/image/audio models.
2. **Direct adapters only where the aggregator is weak**: LLM/VLM (Anthropic, OpenAI, Gemini - native video understanding, prompt caching, tool use), STT with diarization (ElevenLabs Scribe, AssemblyAI, Deepgram), TTS/voice clone (ElevenLabs), Google (Veo/Omni, Nano Banana, Lyria, TTS) and Runway (Aleph video editing).
3. **Everything is async job-based** (submit -> poll or webhook -> download CDN URL that expires). A desktop app has no public webhook endpoint, so poll by default; optionally support webhooks via a tunnel/relay later.
4. **Model manifest is the product.** A JSON-schema-described model catalog (params, limits, pricing formula, capabilities) lets UI forms, MCP tool schemas and cost estimates be generated, and lets the catalog update weekly without app releases.
5. **Licensing/provenance matters in 2026**: EU AI Act Art. 50 transparency obligations applied from 2026-08-02 [S]; record provenance and preserve/emit C2PA where possible.

## 2. Higgsfield (what to copy)

### 2.1 What it is today (as of 2026-11-22)
- Browser-based, multi-model "creative studio" aggregator for video, image, editing, storyboarding, ads. Surfaces: creation workspaces, one-click **Apps** (94 public apps in 22 collections per its llms-full.txt: face swap, ad/product scenes, VFX, viral formats), community feed, Academy [V - higgsfield.ai/llms-full.txt].
- **Integrated video models listed**: Sora 2 (OpenAI API is shut down, so how Higgsfield serves it now is [U]), Veo 3.1, Kling 3.0, Wan (incl. 2.6), Seedance 2.0, Grok Imagine. **Image**: Nano Banana Pro, Nano Banana 2, FLUX.2, GPT Image 1.5, Seedream 5.0, plus own **Soul 2.0 / Soul Cinema / Soul Cast** [V].
- Own features: **Cinema Studio** (camera language, lenses, shot design), **Camera Controls** + Professional app collection (camera/angle/transition effects), **Effects/VFX** apps, **Presets** (e.g. Sora 2 Presets), **Soul ID** (persistent character/identity consistency, trained reference), AI Influencer Studio [V].
- Plans: free trial, creator plans (credits), team, enterprise; no prices on the page [V].

### 2.2 Public API
- Yes. Separate product from the web app: pay-per-generation in USD with an API key (not plan credits); video priced per output second, images per image [S]. Launch reported 2026-09-17 [S, single Polish review - treat date as U].
- Docs at docs.higgsfield.ai; model catalog/keys in console.higgsfield.ai [V]. Docs index lists: Quickstart, Authentication, Requests/lifecycle, **Polling**, **Webhooks**, File uploads, Errors/retries, Rate limits, Billing/retention, Enterprise (org roles, training opt-out, data retention), Python+TypeScript SDKs, OpenAPI spec at /docs/openapi.json [V]. The docs index does not state the request base URL or auth scheme (read the authentication page before building) [V - gap].
- Changelog facts [V]: 2026-08-11 polling + webhook docs, OpenAPI for status/cancel; 2026-10-23 over-concurrency requests are queued not rejected; Soul ID training cap returns 429+Retry-After; 2026-10-24 queue-full -> 429 `account_queue_full`; **webhooks signed with Standard Webhooks** (`webhook-id`, `webhook-timestamp`, `webhook-signature`); per-request retention header `Hf-Expires-After` (>=3600 s), `expires_at` in status; `DELETE /requests/{id}` purges files+prompt. Model availability/schema changes will arrive with a "redesigned model catalog".
- Model count claims conflict: "50+ models" (Higgsfield help) vs "100+" (aggregator) [S]. Soul / Cinema Studio exposure through the API is [U].
- Kling 3.0 promo price seen at $0.042/s (50% promo, 2026-09-23) [S].

### 2.3 UX patterns worth copying
1. **One prompt box, model picker, per-model parameter panel** driven by capability (video/image/edit/audio).
2. **Presets / "Apps"**: curated prompt+model+param bundles with a thumbnail, one-click ("face swap", "product ad", "explosion"). In Cutweave: shareable JSON "recipes" (model + params + prompt template + optional input slots).
3. **Camera-motion picker** (dolly, orbit, crane, handheld...) that injects prompt text or native camera params depending on the model (Kling/Runway/Luma have native controls; others take prompt text).
4. **Character/identity persistence** (Soul ID): a reusable "reference asset" (set of images or trained id) attachable to any generation. Model as a first-class `Reference` entity in the media library.
5. **Cinema Studio**: shot-list/storyboard -> per-shot generations -> timeline. This maps naturally onto a video editor timeline (generate-to-timeline).
6. **Credits/cost shown before generate**, queue state, history feed of generations (we add provenance).
7. Community feed/templates: out of scope, but a recipe import/export format gives 80% of it.

Cutweave should support **Higgsfield as an aggregator provider** (user pastes their API key) rather than copying its closed catalog.

## 3. Models by capability (as of 2026-11-22)

Prices are indicative snapshots [S unless marked] and must come from the live catalog.

### 3a. Text-to-video / image-to-video / video editing

| Model | Provider / access | Key facts | Pricing signal |
|---|---|---|---|
| **Veo 3.1** (+Fast, Lite) | Google Gemini API / Vertex; also via fal, OpenRouter (`google/veo-3.1`), Higgsfield | 8 s clips, 720p/1080p/4K, native audio, portrait, video extension, first+last frame, up to 3 reference images [V - Gemini Veo doc page, undated; docs still show `veo-3.1-generate-preview` ID]. The Gemini model-list page no longer lists Veo (shows Omni) [V] - status of Veo going forward [U]. | $0.40/s (720/1080p), $0.60/s 4K; Fast $0.10-0.30/s; Lite $0.05-0.08/s [S]. Preview endpoints reportedly sunset 2026-10-22 [S/U]. |
| **Gemini Omni Flash 1.1 / 1.2** (`gemini-omni-1.2-flash`) | Google Gemini API / Google Cloud | Conversational generate + **edit** video, up to 15 s; GA in API reported 2026-11-05 [S, single source]; listed as "New" on model page [V] | Pricing [U] |
| **Sora 2 / Sora 2 Pro** | OpenAI | **Shut down: Videos API and all sora-2 models ended 2026-09-24; consumer app closed 2026-04-26; no replacement** [V - OpenAI model page via search; S for dates]. Do **not** build an OpenAI video adapter. | n/a |
| **Kling 3.0 / Omni / O3** | Kuaishou official API (prepaid resource packages); fal, Replicate, WaveSpeed, Krea, Higgsfield | Multi-shot, 720p/1080p/4K tier, native audio, Omni = reference/edit [S] | ~$0.084-0.14/s depending on res/audio (official, derived) [S/U] |
| **Runway Gen-4.5 / Aleph 2.0** | Runway API (api.dev.runwayml.com), OpenRouter (`runway/gen-4.5`), Krea | Gen-4.5 T2V/I2V; **Aleph = video-to-video editing** (restyle, add/remove objects); HDR/professional output surcharges [S] | Credits at $0.01: Gen-4.5 12 credits/s ($0.12/s); Aleph 2.0 28 credits/s, min 56 [S]; Runway says credits unified web+API as of 2026-09-18 [S] |
| **Luma Ray 3 / 3.2 + Modify** | Luma API | I2V/T2V, **video_edit (Modify)**, start/end frames, character reference; source clip cap 18 s / 200 MB; presigned CDN URLs expire ~1 h [S] | see lumalabs.ai/api/pricing [U] |
| **Seedance 2.0 (+mini/fast), 2.5** | ByteDance BytePlus ModelArk (create/query/list/cancel/delete task API); fal, WaveSpeed, Replicate | Multimodal refs, native audio. 2.0 GA on BytePlus [S, 2026-07]; 2.5 announced, API access unconfirmed [U] | Token-based; ~$0.15-0.19/s at 720p via resellers [S] |
| **MiniMax Hailuo (2.3 / newer)** | MiniMax API; fal, Replicate, WaveSpeed | Strong motion; first/last frame, subject ref [U] | conflicting [U] |
| **Alibaba Wan 2.x (2.5/2.6/2.7), HappyHorse-1.0** | Alibaba Model Studio / Bailian; fal; WaveSpeed; open weights | Wan open-source variants (local-capable) + commercial API [S] | [U] |
| **Grok Imagine** | xAI API [U]; listed on Higgsfield [V] | | [U] |
| **LTX (Lightricks)** | open weights + LTX API/fal [U] | fast, local-friendly | [U] |
| **Pika** | No confirmed 2026 public API [U] | consumer-focused | n/a |
| **Higgsfield own (Soul, DoP, etc.)** | Higgsfield API | see 2.2 | per second [S] |

Cross-cutting limits to model in the manifest: duration (typ. 4-15 s per clip), resolution (720p/1080p/4K), aspect ratios (16:9, 9:16, 1:1, sometimes 4:3/3:4/21:9), reference-image count (0-9 depending), first/last frame, audio on/off, seed, negative prompt, camera control (native enum vs prompt text), extend/continue, upload formats and size/duration caps for V2V.

### 3b. Text-to-image and image editing

| Model | Access | Notes | Pricing signal |
|---|---|---|---|
| **Nano Banana Pro** (`gemini-3-pro-image`, stable), **Nano Banana 2** (`gemini-3.1-flash-image`, stable), **NB 2 Lite**, **Nano Banana 2.1** (`gemini-nano-banana-2.1`, "New") | Gemini API/Vertex [V - model page]; fal, OpenRouter | Edit with multi-image reference, text rendering, up to 4K. **Imagen 4 shut down** [V] | Pro ~$0.134-0.15/img 1-2K, ~$0.30 4K; NB2 ~$0.067/1K [S] |
| **GPT Image** (`gpt-image-2.6-sunburst/flare`, 2.5 variants) | OpenAI [V - model page] | Edits with masks, token-priced, quality tiers | ~$0.006 low 1024 [S] |
| **FLUX.2 [pro/flex/dev], FLUX Kontext** | Black Forest Labs API; fal, Replicate, Runware, OpenRouter | Strong edit/Kontext, per-megapixel billing | FLUX.2 pro ~$0.03/MP [S] |
| **Seedream 4.5 / 5.0** | BytePlus; fal, WaveSpeed | 4K, multi-ref | ~$0.04/img [S] |
| **Ideogram (4.0 Turbo etc.), Recraft** | Own APIs; fal/Replicate | Typography/vector (Recraft SVG) | Ideogram ~ $0.03 [S] |
| **Midjourney** | **No official API** as of Aug-Nov 2026 [S]; third-party wrappers violate ToS - do **not** support | | n/a |

### 3c. Lip-sync, avatars, dubbing

| Product | Access | Notes |
|---|---|---|
| **HeyGen** | Official API v3 (v1/v2 discontinued 2026-11-01 [S]); Avatar III-V, video translate, **Overdub/lip-sync** (speed/precision) | Per-second: Speed $0.0333, Precision $0.0667 [S]; avatars $0.0167-0.0667/s [S] |
| **Sync Labs (lipsync-2 / pro)** | API; also on fal/Replicate | Best generic video lip-sync [U - not verified this session] |
| **Hedra (Character-3)** | API | Character animation incl. non-human faces [S, competitor blog] |
| **Synthesia** | Enterprise API | avatars; plan-gated [U] |
| **Captions/Mirage** | API availability [U] | |
| **ElevenLabs Dubbing** | API (dub with voice cloning, speaker detection) [U pricing] | Changelog entries exist in 2026 [S] |
| Kling/Runway/Veo "lipsync" modes; Wan-S2V, InfiniteTalk (open) | via aggregators | cheap alternatives |

### 3d. TTS, voice cloning, voice changer

| Provider | Model | Notes |
|---|---|---|
| **ElevenLabs** | Eleven v3 (expressive), Flash/Turbo; instant+pro voice clone; voice changer; SFX | Top quality; ~$100/M chars effective [S] |
| **MiniMax** | Speech 2.8 HD/Turbo | $100/M HD, $60/M Turbo [S]; cloning |
| **Fish Audio** | S2 Pro (open weights) | ~$15/M chars [S]; cloning |
| **Cartesia** | Sonic 3.x | low-latency; cloning [S/U pricing] |
| **Google** | Gemini 3.8 Flash TTS / Lite TTS (new), 3.1 Flash TTS preview (legacy) [V] | multi-speaker, style prompts |
| **OpenAI** | gpt-4o-mini-tts, tts-1: **listed for shutdown 2027-01-06** [S] - avoid as primary |
| Inworld, AssemblyAI-TTS-1 | newer entrants [S] |

### 3e. Speech-to-text (word timestamps, diarization)

| Provider | Model | Notes |
|---|---|---|
| **ElevenLabs** | Scribe v2 (batch), Scribe v2 Realtime | word timestamps, diarization (batch certain; realtime diarization claim conflicting [S]), ~$0.22/h batch [S]; low WER on Artificial Analysis (2.2%) [S] |
| **AssemblyAI** | Universal-3 Pro / "AssemblyAI-STT-3.7" | diarization, word timestamps; ~$0.21-0.26/h [S]; claims better diarization than Scribe v2/Nova-3 (vendor benchmark) |
| **Deepgram** | Nova-3 | fast/streaming; $0.0077/min (+diarization add-on) [S] |
| **OpenAI** | `gpt-transcribe`, `gpt-live-transcribe`, `gpt-realtime-whisper` [V model page]; diarization not mentioned [V] | word timestamps support [U] |
| **Google** | Gemini 3.5 Transcribe / Chirp [U] | |
| **Gladia** | Solaria [U] | |
| Local Whisper/Parakeet = separate local provider | | |

Cutweave needs: word-level timestamps, speaker labels, language detect, custom vocabulary, filler-word retention (for text-based editing). Define a **normalized transcript schema** (words[{text,start,end,speaker,confidence}]) and normalize every adapter into it.

### 3f. Music and SFX

| Product | API status | Licensing |
|---|---|---|
| **ElevenLabs Music + Sound Effects** | Official, shipping API; $0.15-0.30/min conflicting [S] | Trained on licensed data, commercial use cleared (vendor claim) [S] |
| **Google Lyria** (`lyria-3.5`, `lyria-3-clip-preview` 30 s clips, `lyria-3-pro-preview`, `lyria-realtime-exp`) | In Gemini API model list [V] | SynthID watermark; terms [U] |
| **Suno** | **No self-serve official API**; curated partner program announced 2026-07-01 [S]; wrappers violate ToS | Label settlements/licensing evolving [U] |
| **Udio** | No official public API [S] | licensing in flux [S] |
| **Stable Audio** (Stability) | API/open weights [U] | |
| Aggregators (fal/Replicate/WaveSpeed) host MiniMax Music, ACE-Step, Stable Audio etc. [U] | | |

Recommendation: ship ElevenLabs Music/SFX + Lyria; record license tag per model in manifest (`license: {commercial: true|false|unknown, source}`).

### 3g. LLM / VLM (agentic editing, highlight detection, scripts, video understanding)

| Provider | Current models [V unless noted] | Relevance |
|---|---|---|
| **Anthropic** | `claude-fable-5-2` ($8/$40 per MTok), `claude-opus-5-5` ($4/$20), `claude-sonnet-5-5` ($2/$10), `claude-haiku-5-5` (from $0.10/$0.50); all 1M context, 128K output, **text+image input, tool use**; retirements >= Sep-Nov 2027 | Best fit for agentic editing / tool use / MCP; no native video input - send sampled frames + transcript |
| **OpenAI** | `gpt-6.2-astra`, `gpt-6.2-sol`, `gpt-6.1-terra`, `gpt-6-luna` | Tool use, images; video via frames |
| **Google Gemini** | `gemini-4-*` family (carbon/neon/hydrogen/argon), `gemini-3.8-flash`, `3.6-flash`, `3.5-flash-lite`, `3.1-flash-lite`; 2.5 routed to 3.x on 2026-12-01 | **Native video understanding**: YouTube URL input (free 8 h/day; paid unlimited; public videos only; <=10 videos/request for 2.5+), Files API up to 20 GB paid / 2 GB free [V - video-understanding doc]; timestamped Q&A, highlight detection, scene segmentation - strongest for "find the best moments" |
| **OpenRouter** | one key, hundreds of LLMs incl. the above, OpenAI-compatible `/chat/completions` | Cheap way to cover Llama/Qwen/DeepSeek/Grok etc. |

Design implication: video understanding is a distinct capability `VideoUnderstand` (input: video file/URL + prompt -> timestamped JSON). Gemini implements it natively; others implement via a **frame-sampling + transcript shim** inside Cutweave.

### 3h. Upscale / enhance / background removal / relight

| Product | Notes |
|---|---|
| **Topaz Labs API** | Official developer API (developer.topazlabs.com); credit-billed by **output** resolution; video credits per 10 s@1080p30: Proteus 4, Starlight Fast 6/Quality 12, Astra 40, Denoise 2-4 [V - developer pricing page via search]; also resold via Picsart ~$0.004-0.005/credit [S] |
| **Magnific (Freepik)** | upscaler/relight APIs via Freepik API [U] |
| **Clipdrop (Stability)** | bg removal, relight, uncrop - API [U; status of service unverified] |
| **remove.bg** | image bg removal API [U] |
| Aggregator-hosted: BiRefNet/BRIA RMBG, SeedVR2, Real-ESRGAN, Clarity upscaler, IC-Light | cheap, widely on fal/Replicate/Runware [U] |

## 4. Aggregators / gateways

| Aggregator | Coverage | API shape | Notes |
|---|---|---|---|
| **fal.ai** | 1,000+ models (image, video, audio, 3D) [S/V docs] | `Authorization: Key $FAL_KEY`; sync `fal.run/{model}`; **queue** `queue.fal.run/{model}` submit -> status -> result -> cancel; **webhooks**; streaming; realtime WebSocket; media returned as CDN URLs; Python/JS SDKs [V - fal docs] | Per-model pricing shown on model page; per-output units (sec/megapixel/image); prepaid credits [S]. **Best first target**: fastest to get new models (day-0 Kling/Veo/Seedance/Wan/FLUX) |
| **Replicate** | 50,000+ community+official models; acquired by Cloudflare (announced 2025-11-17; close status [U]) [V press] | `POST /predictions`, `Prefer: wait` (1-60 s sync), polling, `webhook` + `webhook_events_filter` [V docs]; model `version` pinning; OpenAPI schema per model | Good for long tail and open models; per-model input schemas are machine-readable - ideal for auto-forms |
| **OpenRouter** | LLMs (hundreds) + image models + **video generation API** (launched 2026-04-15): `POST /api/v1/videos` -> 202 + job id + polling URL; `GET /api/v1/videos/models`; `/content` download; **webhooks** for completed/failed/cancelled/expired with idempotency key; Veo, Seedance, Wan, Runway Gen-4.5 etc. [V/S] | OpenAI-compatible chat; models filterable by `output_modalities=video`; incompatible with ZDR | Best single gateway for LLM + some media; markup ~5.5% credit fee [U - recall, verify] |
| **Higgsfield API** | 50+ (claimed) media models incl. own Soul | polling + signed webhooks (Standard Webhooks), file upload, cancel/delete, retention header [V] | Directly serves the "like Higgsfield" request; per-output pricing |
| **Runware** | very large model count claim (400k+ incl. community) [S]; image/video/audio | WebSocket/HTTP unified API, cheapest per-unit claims ("images from $0.0006, videos from $0.14") [S vendor] | Good cost-saver; add later |
| **WaveSpeed** | curated + exclusive (Seedream, Kling, Wan outside China) [S] | per-successful-output billing; no charge on failures [S] | Add later; useful for Chinese models |
| **Together AI** | LLMs + FLUX/image + some video [U] | OpenAI-compatible | optional |
| **Eden AI, AIML API, CometAPI, Atlas Cloud, Novita, Krea API, apiframe (Midjourney wrappers)** | meta-aggregators | various | Lower priority; apiframe/Midjourney wrappers ToS risk |
| **Krea API** | hosts Kling 3.0, Runway Gen-4.5 etc. [S] | | optional |

Markup: no clean cross-vendor comparison exists [S]. Aggregators typically price at or near provider list price, sometimes cheaper (volume deals) or with promos. Show the user "price via X vs direct" in the model picker when both exist.

**Recommended aggregator order**: (1) fal.ai, (2) Replicate, (3) OpenRouter (LLM + video), (4) Higgsfield, (5) Runware, (6) WaveSpeed, then Together/Eden/AIML on demand. Each adapter implements a *generic* "aggregator" with per-model manifest entries, so adding a model is a JSON entry, not code.

## 5. Integration architecture recommendations

### 5.1 Capability-typed provider interface

Rust crate `cutweave-ai` with capability traits; a provider adapter implements any subset:

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;                       // "fal", "google", "elevenlabs"
    fn auth_scheme(&self) -> AuthScheme;         // ApiKey{header}, Bearer, OAuth, Custom
    async fn list_models(&self) -> Result<Vec<ModelDescriptor>>; // live discovery where available
    async fn submit(&self, req: JobRequest) -> Result<RemoteJob>;   // returns remote id + poll hints
    async fn poll(&self, job: &RemoteJob) -> Result<JobStatus>;     // Queued|Running{progress}|Succeeded{outputs}|Failed{err,retryable}
    async fn cancel(&self, job: &RemoteJob) -> Result<()>;
    fn estimate_cost(&self, model: &ModelDescriptor, params: &Params) -> CostEstimate;
}
// Capabilities are a closed enum used in manifests + a trait-less dispatch on JobRequest.kind:
enum Capability { TextToVideo, ImageToVideo, VideoToVideo, FirstLastFrame, TextToImage, ImageEdit,
  LipSync, Avatar, Dub, Tts, VoiceClone, VoiceChange, Stt, Music, Sfx, Llm, Vlm, VideoUnderstand,
  Upscale, BgRemove, Relight, Embedding }
```
- Prefer **one `submit/poll` core + capability-tagged payload** over a trait per capability: aggregators serve many capabilities through one endpoint, and the registry (not the Rust type system) decides what a model can do. Add typed convenience wrappers for strongly-typed paths (`Stt` -> normalized transcript; `Llm` -> streaming chat, tool use; `VideoUnderstand`).
- **Streaming** (LLM tokens, realtime STT/TTS) is a separate `Stream` interface; not every provider supports it.
- Normalize outputs: `Asset{kind, url|bytes, mime, duration, w/h, fps, sample_rate}` + `Provenance`.
- Normalize errors: `Auth, RateLimited{retry_after}, QuotaExceeded, ContentPolicy, InvalidParams, ProviderDown, Timeout, Unknown` with `retryable` flag. (Higgsfield returns 429+Retry-After and `account_queue_full`; OpenRouter, fal, Replicate each have their own shapes.)

### 5.2 Model registry / manifest

One JSON document per model (JSON Schema 2020-12 for `params`), example:

```json
{
  "id": "fal/kling-3.0/image-to-video",
  "provider": "fal", "endpoint": "fal-ai/kling-video/v3/pro/image-to-video",
  "capabilities": ["ImageToVideo"], "family": "kling", "version": "3.0",
  "status": "ga|preview|deprecated|shutdown", "deprecation_date": null, "replaced_by": null,
  "inputs": { "image": {"type":"asset","kind":"image","max_mb":20,"role":"first_frame"},
              "prompt": {"type":"string","maxLength":2500} },
  "params": { "$schema":"https://json-schema.org/draft/2020-12/schema", "type":"object",
              "properties": { "duration": {"enum":[5,10]}, "aspect_ratio": {"enum":["16:9","9:16","1:1"]},
                              "generate_audio": {"type":"boolean","default":true}, "seed": {"type":"integer"} } },
  "outputs": [{"kind":"video","mime":"video/mp4","max_duration_s":10,"resolutions":["720p","1080p"]}],
  "pricing": { "unit":"second", "formula":"rate[resolution][audio] * duration", "table":{...}, "currency":"USD", "as_of":"2026-11-22", "source":"url" },
  "license": { "commercial_use": "unknown", "watermark": "synthid|none|visible", "c2pa": true },
  "ui": { "group":"Video", "presets":[...], "param_order":[...], "labels":{...} },
  "request_map": {...}, "response_map": {...}
}
```
- The same `params` schema drives (a) the generated UI form, (b) **MCP tool input schemas** (`generate_video` tools auto-built per capability, with `model` enum), (c) validation before spending money, (d) cost estimate.
- For aggregators, **import schemas automatically**: Replicate and fal publish per-model OpenAPI/input schemas; OpenRouter exposes `/videos/models` with capabilities; Higgsfield publishes OpenAPI. A "catalog builder" CI job pulls these, normalizes into manifests, and hand-curates only `ui`, `license`, `pricing` overrides.
- Keep `request_map`/`response_map` declarative (JSONPath/JMESPath templates) so most models need **no code**.

### 5.3 Async job manager (desktop)
- Persistent SQLite job table: `id, provider, model, params_hash, state, remote_id, cost_est, cost_actual, created/started/finished, attempts, error`. Survives app restart; on launch, resume polling.
- **Polling by default** (desktop has no inbound webhook); exponential backoff with jitter, per-provider hints (Retry-After, poll-interval from manifest). Optional webhook mode later through a tiny relay (e.g. user-run tunnel or Cutweave-hosted signed-webhook inbox) - note Higgsfield/OpenRouter/fal/Replicate all support webhooks; Higgsfield uses Standard Webhooks signing.
- **Retries**: only on retryable errors; idempotency keys where supported (OpenRouter has `X-OpenRouter-Idempotency-Key` on deliveries); never auto-retry a submit that may have been accepted without checking (risk of double billing) - use client-generated job ids stored with remote id.
- **Cancel** support (Higgsfield, Replicate, BytePlus, fal queue all have cancel).
- **Download promptly**: result URLs expire (Luma ~1 h [S]; Higgsfield `expires_at`; fal/Replicate CDN limited retention) -> on success immediately copy to the local project media store.
- **Cost estimate before run** from manifest formula; show range; require confirm above threshold. **Budget limits**: per-job, per-project, daily, per-provider caps; hard stop + soft warning; track actual via provider-returned usage where available, else estimate.
- **Result cache keyed by hash** of (provider, model, version, canonicalized params, input asset content hashes, seed). Only reuse when seed fixed or user opts in ("generate again" bypasses). Also dedupes accidental double-clicks.
- Concurrency limits per provider; queue UI with ETA; handle 429/`queue_full` by local queueing.
- Moderation failures: surface provider's policy message; don't retry.

### 5.4 Secure key storage
- OS keychain via Rust `keyring` crate (Windows Credential Manager, macOS Keychain, Secret Service/libsecret), accessed only from the Rust core - **never exposed to the webview**; the UI calls Tauri commands. Per-provider key + optional label/organization. Optionally Tauri Stronghold as fallback for headless Linux. Redact keys in logs, never write to project files (so projects can be shared). Key validation call on entry. Support env-var override for CLI/MCP use.
- Send media only to the provider the user selected; show data-flow disclosure per provider (training opt-out info e.g. Higgsfield Enterprise has training opt-out).

### 5.5 Importing results + provenance
- On completion: download -> content-hash -> media library asset with `provenance` sidecar (JSON in project DB and embedded XMP where the container allows): `{generator: {provider, aggregator, model, version}, prompt, negative_prompt, params, seed, input_asset_hashes, job_id, remote_request_id, cost, currency, created_at, license_tags, watermark_info}`.
- Keep the generation recipe so "re-run / vary / extend" is one click; link derived assets (parent -> child graph).

### 5.6 C2PA / content credentials
- Regulatory driver: EU AI Act Art. 50 machine-readable marking/deepfake disclosure applicable from 2026-08-02 [S]; sources disagree whether C2PA is mandated vs one accepted technique [S] - treat C2PA as best-practice, not legal advice.
- Providers embed marks: Google SynthID (invisible, by default in Veo/Gemini outputs) [S], some include C2PA manifests. Platforms (YouTube) label based on C2PA/SynthID [S].
- Cutweave: (1) **preserve** incoming C2PA manifests where lossless; note that re-encoding strips them - warn the user; (2) **on export, optionally write a C2PA manifest** (via `c2pa-rs`, MIT/Apache, Rust-native) listing "AI generated" assertions (`c2pa.actions` with digitalSourceType trainedAlgorithmicMedia) built from provenance records, signed with a user-supplied cert (or unsigned/self-signed with clear label); (3) never strip SynthID/visible watermarks; (4) expose a "contains AI-generated content" project flag and per-clip badge.

### 5.7 Third-party provider plugins
Two tiers:
1. **Declarative HTTP manifest (primary, safe)**: manifest = auth scheme, base URL, request/response templates, job pattern (`sync | poll{status_url, done_when, result_path} | webhook`), error mapping, pricing formula, model list. Executed by the Rust core; no third-party code runs. Covers ~90% (fal, Replicate, OpenRouter, Runware, WaveSpeed, Higgsfield, Runway, Luma, Kling, BytePlus, HeyGen, ElevenLabs are all REST+JSON+polling). Because Cutweave is GPL-3.0, plugin manifests can be community-contributed via PR and user-installable by URL.
2. **WASM adapter (escape hatch)**: WASI component (wasmtime) implementing a `provider` WIT interface (`describe`, `build-request`, `parse-response`, optional `sign-request` for AWS SigV4/OAuth/HMAC), **no direct network or key access** - the host performs HTTP and injects secrets, with a per-plugin allowed-hosts list. Avoids arbitrary native code and keeps keys safe. Needed for odd auth (Kling JWT signing, SigV4, multi-step uploads, websocket protocols).
- Plugins declare required permissions (hosts, secrets), are signed/hashed; UI shows permission prompt on install.
- Skip native dynamic libraries and JS plugins in the webview (key exposure + supply-chain risk).

### 5.8 Keeping the registry current (weekly model churn)
Evidence of churn: Sora API removed with 6 months' notice; Imagen 4 shut down; Gemini 2.5 routed on 2026-12-01; OpenAI TTS shutdown 2027-01-06; HeyGen v1/v2 discontinued 2026-11-01; Seedance 2.0 -> 2.5 within months.

Recommended hybrid:
1. **Remote signed catalog** (primary): static JSON (+ JSON Schemas) on a CDN/GitHub release, **signed with minisign/ed25519** (public key pinned in app; also supports key rotation), versioned with monotonic `catalog_version` (anti-rollback) and `min_app_version`. App checks daily/on-launch (opt-out; works offline from cached copy). Contains models, schemas, pricing, deprecation dates, UI hints, license tags. Built by a CI job that pulls aggregator schemas (fal, Replicate, OpenRouter `/videos/models`, Higgsfield OpenAPI) and diffs provider docs; maintainers review the diff. Community PRs welcome (GPL project).
2. **Live discovery at runtime** as a supplement: where a provider offers a models endpoint (OpenRouter, Replicate, Anthropic Models API with `capabilities`, Gemini models list, Higgsfield console catalog), fetch and merge as "unverified/new" entries (usable via generic form from the provider's schema, flagged "not curated").
3. **Plugin updates** (new adapter code / WASM) ship through the plugin channel or app releases only when API *shape* changes - rare compared with model changes.
4. **Custom model entry**: user can add an aggregator model by slug (e.g., a new fal endpoint) - the manifest auto-builds from the fetched schema. This guarantees "support day-0 models" without waiting for us.
5. Deprecation handling: `status`, `deprecation_date`, `replaced_by` -> UI banner, auto-suggest replacement, block submit after shutdown date.
6. Prices: catalog `as_of` timestamp shown in UI ("estimate, verified 2026-11-20"); where a provider has a pricing API (fal model pages, OpenRouter models endpoint) refresh live.

## 6. Recommended first wave

**Aggregators (cover most media models):** fal.ai -> Replicate -> OpenRouter (LLMs + video) -> Higgsfield API. Second wave: Runware, WaveSpeed.

**Direct adapters (first wave):**
1. Anthropic (Claude; agentic editing/tool use) - plus OpenAI-compatible adapter (OpenAI, OpenRouter, Together, Groq, local servers)
2. Google Gemini API (video understanding, Nano Banana, Veo/Omni, Lyria, TTS)
3. ElevenLabs (TTS, voice clone/changer, Scribe STT, music, SFX, dubbing)
4. AssemblyAI or Deepgram (STT/diarization alternate)
5. Runway (Aleph V2V), then Kling official, BytePlus Seedance, Luma, HeyGen, Topaz, BFL FLUX, MiniMax as second wave (most are reachable via aggregators already).

**Do not build**: OpenAI Sora (shut down), Midjourney/Suno/Udio unofficial wrappers (ToS), OpenAI tts-1 as primary (EOL 2027-01-06).

## 7. Open questions / to verify before implementation
- Higgsfield API base URL, auth scheme, model IDs and whether Soul/Cinema Studio are API-exposed (read docs.higgsfield.ai authentication + OpenAPI).
- Veo's long-term status vs Gemini Omni; Omni pricing and GA date (single source 2026-11-05).
- Kling official API current price table and JWT auth details; Seedance 2.5 API access.
- Sync Labs, Hedra, Synthesia, Captions, ElevenLabs Dubbing/Music exact pricing and limits (not verified this session).
- Magnific/Clipdrop/remove.bg current API status and pricing.
- OpenRouter fee structure; Replicate post-Cloudflare API/pricing changes.
- Legal: Art. 50 specifics and C2PA obligations; per-model commercial-use terms.

## 8. Sources
- Higgsfield: https://higgsfield.ai/llms-full.txt ; https://docs.higgsfield.ai/docs/changelog.md ; https://docs.higgsfield.ai/docs/llms.txt ; https://higgsfield.ai/blog/higgsfield-api ; https://apiframe.ai/guides/higgsfield-api-guide ; https://modelslab.com/higgsfield-alternative ; https://higgsfield.ai/blog/sora-api-migration-guide
- fal: https://fal.ai/docs/documentation/model-apis/overview.md ; https://fal.ai/docs/documentation/model-apis/pricing
- Replicate: https://replicate.com/docs/topics/predictions/create-a-prediction.md ; https://replicate.com/docs/reference/http ; https://www.cloudflare.com/en-gb/press/press-releases/2025/cloudflare-to-acquire-replicate-to-build-the-most-seamless-ai-cloud-for-developers/
- OpenRouter video: https://openrouter.ai/docs/guides/overview/multimodal/video-generation ; https://openrouter.ai/docs/api_reference/videos ; https://openrouter.ai/blog/announcements/video-generation/
- Aggregator comparisons (vendor-authored): https://wavespeed.ai/blog/posts/best-ai-inference-platform-2026/ ; https://runware.ai/alternative ; https://apidog.com/blog/best-ai-inference-platform-guide-2026
- OpenAI: https://developers.openai.com/api/docs/models ; https://developers.openai.com/api/docs/models/sora-2.md ; https://dev.to/mr_manushukla/sora-2-and-the-videos-api-shut-down-on-24-september-2026-the-migration-and-the-real-cost-per-second-bek ; https://wavespeed.ai/blog/ai-guides/sora-2-api-shutdown-migration-guide/
- Google: https://aistudio.google.com/docs/models (redirect of ai.google.dev/gemini-api/docs/models) ; https://ai.google.dev/gemini-api/docs/veo ; https://ai.google.dev/gemini-api/docs/video-understanding ; https://thenextweb.com/news/google-gemini-omni-flash-video-model-io-2026 ; https://letsdatascience.com/news/google-launches-gemini-omni-12-flash-with-15-second-video-ch-678ef595 ; https://www.cloudzero.com/blog/veo-3-pricing/ ; https://invideo.io/blog/ai-video-model-pricing
- Anthropic: https://platform.claude.com/docs/en/models/overview
- Runway: https://docs.dev.runwayml.com/guides/pricing/ ; https://magichour.ai/blog/runway-gen-45-pricing ; https://runway.com/news/company-news/working-smarter-with-runway-unified-pricing
- Kling / Seedance: https://kling.ai/document-api/pricing/base/video.md ; https://www.krea.ai/blog/kling-3-0-api-access-guide-pricing-code-examples-for-multi-shot-ai-video ; https://cntechpost.com/2026/03/04/bytedance-announces-api-pricing-seedance-2-0 ; https://anikuku.com/blog/seedance-2-api-pricing-guide-2026
- Luma: https://lumalabs.ai/api ; https://lumalabs.ai/api/pricing
- Images: https://openrouter.ai/collections/image-models ; https://tech-insider.org/how-to-use-nano-banana-pro-api-2026/ ; https://www.cometapi.com/ai-image-api-pricing/ ; Midjourney: https://unifically.com/blogs/midjourney-api ; https://apiframe.ai/blog/best-midjourney-apis
- Avatars/lipsync: https://developers.heygen.com ; https://developers.heygen.com/changelog ; https://www.g2.com/articles/heygen-api-pricing
- TTS: https://invideo.io/blog/ai-voice-model-pricing/ ; https://www.digitalapplied.com/blog/best-text-to-speech-models-september-2026-ranked-priced ; https://artificialanalysis.ai/text-to-speech/leaderboard/controlled-voice
- STT: https://www.assemblyai.com/compare/scribe-v2-vs-assemblyai ; https://artificialanalysis.ai/speech-to-text/models/elevenlabs-scribe-v2 ; https://deepgram.com/blog/elevenlabs-transcription-vs-deepgram ; https://www.gladia.io/blog/best-elevenlabs-alternatives-for-speech-to-text-gladia-deepgram-assemblyai-compared
- Music: https://www.digitalmusicnews.com/2026/07/03/suno-is-opening-an-api-partner-program/ ; https://elevenlabs.io/blog/suno-alternatives ; https://apiframe.ai/blog/best-ai-music-generation-apis
- Upscale: https://developer.topazlabs.com/getting-started/pricing ; https://picsart.com/api-platform/models/topaz-upscale-video/pricing
- Provenance/regulation: https://blog.google/company-news/outreach-and-initiatives/public-policy/eu-ai-act-transparency-code-of-practice/ ; https://www.ssl.com/article/eu-ai-act-article-50-a-complete-guide-to-ai-transparency-compliance/amp/ ; https://c2paviewer.com/articles/eu-ai-act-content-credentials ; https://tianpan.co/blog/2026-04-26-content-provenance-c2pa-synthid-audit-trail
