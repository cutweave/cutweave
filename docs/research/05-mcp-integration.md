# 05 - MCP Integration for Cutweave (research + recommended architecture)

Date: 2026-11-22. Legend: [V] verified from a primary source this session; [U] unverified / from memory / vendor blog; [D] design recommendation (our opinion).

## 1. MCP spec status (late 2026)

- Latest revision: **2026-07-28** (supersedes 2025-11-25) [V]. Earlier: 2025-03-26 (Streamable HTTP, annotations, OAuth 2.1), 2025-06-18 (structured output/outputSchema, elicitation, resource links, OAuth resource-server split), 2025-11-25 (tasks experimental, URL elicitation) [U for older-version feature attribution; sources disagree on which revision introduced tool annotations].
- Headline changes in 2026-07-28 [V, changelog]:
  - **Stateless protocol**: no `initialize` handshake, no `Mcp-Session-Id` (SEP-2567, SEP-2575). Each request carries protocolVersion + clientCapabilities in `_meta`. New mandatory `server/discover` RPC. List endpoints no longer vary per connection.
  - `subscriptions/listen` replaces the GET stream and `resources/subscribe`; opt in to toolsListChanged / promptsListChanged / resourcesListChanged / resourceSubscriptions. Progress/message notifications stay on the originating request's response stream.
  - SSE resumability (Last-Event-ID) removed: a dropped stream loses the request; client must re-issue. Long renders therefore MUST use tasks or a job-id pattern, never one long-held call.
  - **Tasks** moved to official extension `io.modelcontextprotocol/tasks` (SEP-2663): poll `tasks/get`, `tasks/update`; no `tasks/list`; servers may return task handles unsolicited.
  - **Multi Round-Trip Requests** (SEP-2322): servers return `resultType:"input_required"` with `inputRequests` (elicitation/sampling/roots) instead of server-initiated requests; client retries with `inputResponses`. All results carry `resultType`.
  - `ping`, `logging/setLevel`, `roots/list_changed` removed. Log level per request via `_meta`.
  - `ttlMs`/`cacheScope` required on list/read results (SEP-2549); deterministic tool ordering SHOULD (prompt-cache friendly); `Mcp-Method`/`Mcp-Name` headers on HTTP POST (SEP-2243); `inputSchema`/`outputSchema` accept any JSON Schema 2020-12 and `structuredContent` any JSON value (SEP-2106); OTel trace context in `_meta`; resource-not-found now -32602; error codes -32020..-32099 reserved for spec; `extensions` field in capabilities.
  - **Deprecated** (SEP-2577): **Roots, Sampling, Logging** (use tool params / resource URIs / config for paths; stderr or OTel for logs). HTTP+SSE transport deprecated. Dynamic Client Registration deprecated in favour of **Client ID Metadata Documents**.
- Auth [V]: optional. HTTP transports SHOULD follow the OAuth 2.1 profile (server = resource server; RFC 9728 Protected Resource Metadata MUST; RFC 8707 resource indicators; PKCE; `iss` validation RFC 9207; step-up scopes via 403 `insufficient_scope`). **stdio SHOULD NOT use it; take credentials from the environment.**
- Stable features still relevant: tools with annotations (readOnlyHint, destructiveHint, idempotentHint, openWorldHint; defaults conservative: not read-only, destructive=true, not idempotent, open-world=true; hints are untrusted) [V via MCP blog]; structured output + outputSchema; content types text/image/audio/resource_link/embedded resource [U exact list]; resources, templates, subscriptions; prompts; completions; progress; cancellation; elicitation (form + URL).
- Implication: do NOT build on roots/sampling/logging. Use explicit allowed-roots config + tool params; use tasks (or own job handles) for rendering; design stateless (handles in args, not connection state).

## 2. Rust SDKs and embedding

- **rmcp** (modelcontextprotocol/rust-sdk): crates.io latest **3.11.0** (updated 2026-11-17), ~53M total downloads, ~4.1k stars [V]. Targets spec 2026-07-28, back-compatible with 2025-11-25 [V]. Features [V from README]: `#[tool_router]/#[tool]/#[tool_handler]` macros (+ prompt equivalents); schemars-derived inputSchema/outputSchema (2020-12); stdio, child-process client, Streamable HTTP server as a Tower service (axum-mountable), in-process worker transport; stateless mode automatic for 2026-07-28, `with_legacy_session_mode(false)`, `with_json_response(true)`; Tasks extension with `TaskManager`; elicitation form/URL; resources/prompts/completions/subscriptions; caching; OAuth support. Legacy HTTP+SSE deliberately absent. Caveats: tokio stdin read cannot be cancelled (shutdown may wait for EOF); stale cache served on error by default; 3.x migration guide exists (API churn between majors - pin versions).
- Alternatives [U, search only]: `tower-mcp`, `rmcp-server-kit` (auth/RBAC/OAuth JWT atop rmcp), `cloudllm_mcp`. Recommendation: **rmcp**; optionally borrow rmcp-server-kit for auth.
- Existing Tauri MCP projects (hypothesi/mcp-server-tauri, tauri-mcp) are debugging bridges (Node/WebSocket), not a model for embedding. I found no ready recipe for a stdio-to-HTTP proxy [V: not found]; it is small with rmcp client + server in one process.
- **Embedding plan [D]**:
  - `cutweave-core` (no Tauri dependency) owns project state + command bus. `cutweave-api` defines command schemas (serde + schemars). `cutweave-mcp` = rmcp adapter over a `CommandBus` trait. `cutweave-cli` binary.
  - In Tauri `setup()`: spawn a tokio task running axum with rmcp `StreamableHttpService` at `127.0.0.1:<port>/mcp`. Loopback only; validate `Host` and `Origin` (DNS-rebinding defence); require `Authorization: Bearer <token>`; token generated per launch (or persistent and user-rotatable) and written to a 0600 file in the app data dir `mcp.json` {url, token, pid}. Settings UI shows "Copy MCP config" snippets for common clients.
  - `cutweave mcp` (stdio) modes: (a) `--attach` (default if `mcp.json` exists and pid alive): thin stdio<->HTTP proxy so agent and GUI share one live project; (b) `--headless`: loads core in-process, opens a `.cutweave` project, no GUI, same tool set; (c) autodetect with fallback. With sessions gone, proxying is near-stateless forwarding.
  - Optional named pipe / unix socket for CLI instead of TCP (avoids port clashes).
  - GUI and MCP subscribe to the same event stream so the GUI refreshes when an agent edits.

## 3. Prior art and lessons

- DaVinci Resolve MCP: samuelgursky server wraps the official scripting API; `resolve-mcp` (PyPI) claims 215+ tools mirroring the whole API; Tooflex variant has 32 tools plus read-only state resources [V from listing snippets]. Pattern: tool-per-API-method gives huge lists; compact variants appeared. Scripting API limitations (Studio-only external scripting, limited low-level edit) [U].
- OpenEdit: Premiere via a panel that must be open, or ffmpeg-only fallback [V snippet]; another Premiere MCP is roadmap-only. Lesson: GUI-coupled bridges are brittle.
- Blender MCP: few tools (scene info, look/screenshot, asset import) plus one `execute_blender_code` escape hatch [V from tool descriptions in this environment]. Works because models know bpy; downsides: unconstrained, no schema, no undo grouping. Vision is via screenshots (what Cutweave should do better with render APIs).
- Figma Dev Mode MCP: `get_metadata` (sparse outline) -> `get_design_context` (one node) -> `get_screenshot`. Complaints [V]: one response of 351k tokens vs a 25k client cap, hangs/aborts on deep nesting, tool rename (get_code -> get_design_context) breaking stale clients, plan-gated call limits. Lessons: always offer an outline tool, depth/limit/pagination everywhere, server-side response caps, never rename tools casually.
- Ableton / OBS / Kdenlive / Shotcut / Remotion / Motion Canvas MCP: NOT verified this session [U]. (obs-websocket v5 is a good typed request/event API precedent [U].)
- Context-bloat literature: tool definitions are charged every request; Anthropic's code-execution-with-MCP post claims 150k -> 2k tokens (98.7%) in an example [V]; Cloudflare Code Mode exposes just `search()` + `execute()` (~1k tokens) for a 2,500-endpoint API [V]; Claude Code auto-enables MCP tool search when definitions exceed ~10% of context, API has `defer_loading` [U, blogs]; a study found richer tool descriptions gave +5.85 pts accuracy but +67% steps [U]; direct tools fine below ~20-30 tools [U].
- Takeaways: mirror-the-API one-tool-per-method bloats context; escape-hatch code tools are popular but unsafe; typed bounded outputs matter; don't depend on GUI state.

## 4. Design guidance for a huge surface

### 4.1 Granularity decision [D]
Hybrid, three tiers:
1. **Core tools (~30, always listed)**: coarse, intent-level, domain-grouped (`clip_add`, `clip_trim`, `timeline_batch`, `render_frame`...). Tight schemas; cover ~80% of use.
2. **Generic command tools (always listed, 4)**: `cutweave_search_commands(query|namespace)` returns command names + summaries + JSON Schemas (paged, token-capped); `cutweave_describe_command(name)`; `cutweave_execute(commands[], {transaction, dry_run, undo_label, idempotency_key})` taking a discriminated union validated server-side against the full registry; `cutweave_query(...)` for state. The full command union is NOT embedded in `tools/list`; detailed schemas come from describe/resources. This covers the long tail (effect params, keyframes, color) without hundreds of tools.
3. **Optional script tool** `cutweave_script` (Rhai/JS sandbox, only the bus API, no fs/net, step/time limits): Code-Mode analog for loops over many clips. Phase 2.
Rationale: hundreds of tools cost tens of thousands of tokens and hurt selection; a lone `execute_command` loses discoverability, per-tool annotations (approval UX) and good schemas. Hybrid keeps annotations on core tools and a safe generic path. Split read vs write (`cutweave_query` readOnlyHint=true vs `cutweave_execute`); irreversible ops in distinct tools with destructiveHint=true.
- Profiles: `--profile minimal|standard|full` controls listed tools; `list_changed` (via `subscriptions/listen`) when the profile or open project changes (project tools appear when a project is open). Deterministic ordering (spec SHOULD). Never rename tools silently (Figma lesson): `schemaVersion`, deprecated aliases.
- Naming: snake_case `namespace_noun_verb`-style, <=64 chars; clients usually prefix the server name already.

### 4.2 Core ideas
- **Single command bus**: `Command {id, name, args}` -> `Result<Outcome, Error>` + `Event`s. Commands are serde enums with schemars; the registry (name, summary, args/result schema, risk class, namespace, since-version, examples) is generated once and feeds MCP, CLI, HTTP API, docs, GUI. Registry snapshots live in git.
- **Transactions/undo**: `transaction:true` = all-or-nothing, one undo group labelled "Agent: <label>"; on failure at command N, roll back 1..N-1 (apply to a persistent/immutable project snapshot so rollback = drop). `edit_undo(n)`, `edit_redo`, `edit_history` (actor = agent|user), `edit_checkpoint`.
- **Optimistic concurrency**: project `revision`; mutating calls accept `expected_revision`; conflict returns current revision. Essential because the GUI user and agent edit simultaneously.
- **Dry-run/preview**: `dry_run:true` returns the diff (created/modified/deleted ids, validation errors, resulting duration) without committing; optional `preview:true` returns a frame at the affected time.
- **Idempotency**: client `idempotency_key` for mutating calls (store recent keys per project); prefer "set" semantics (absolute values) over increments.
- **IDs**: stable opaque prefixed ULIDs (`clip_...`, `trk_...`, `asset_...`), never indices; names/labels resolvable via `timeline_find`. Return ids in every result. Index only for explicit insert positions.
- **Time**: rational `{num, den}` (or `"n/d"`); accepted conveniences: timecode `"00:01:23:12@29.97"`, frames `{frame, rate}`, float seconds (flagged lossy). Outputs include exact rational + display timecode. Never floats internally (OTIO RationalTime precedent [U]). Ranges half-open `{start, duration}`.
- **State as data (resources, cacheable via ttlMs)**: `cutweave://project/current`, `cutweave://timeline/{id}?depth=`, `cutweave://clip/{id}`, `cutweave://asset/{id}` (+probe), `cutweave://asset/{id}/thumbnail`, `cutweave://waveform/{asset}?start&end&w`, `cutweave://frame/{timeline}/{time}?w=`, `cutweave://schema/commands`, `cutweave://history`, `cutweave://render/{job}`. Many clients are tool-centric (resource support varies [U]) so mirror every resource as a read-only tool (`project_get`, `timeline_get`...). Pagination + hard byte cap with `truncated`/`next_cursor`.
- **Images (seeing without screenshots)**: `render_frame`, `render_contact_sheet` (grid of N frames in one image, timecode burned in - far cheaper than N images), `media_thumbnails`, `media_waveform` image, optional `timeline_overview_image`. Return MCP `image` content (JPEG/WebP, default max edge 512-768, `max_size` param) + text metadata + `structuredContent`; large artifacts as `resource_link` to a `cutweave://` URI so the client fetches only if needed. Rough image token cost ~ (w*h)/750 [U]: 512px frame ~350 tokens. Audio: return structured `audio_loudness` data (LUFS, peaks) rather than blobs.
- **Long-running ops** (export, proxies, transcription, scene detection): Tasks extension when the client advertises it [V exists]; fallback `export_start` -> `job_id` + `job_get/job_cancel` + `notifications/progress` when a progressToken is supplied. Jobs must survive client disconnect (no SSE resume [V]).
- **Safety**: server-side filesystem allowlist (project dir, media library dirs, export dir), canonicalised paths, symlink checks; roots are deprecated [V] so don't rely on them. Irreversible actions (overwrite export file, delete files on disk) require a dry-run-issued `confirm` token or MRTR elicitation. Timeline edits are undoable so not "destructive". Scopes: `read`, `edit`, `render`, `fs_write`, `admin`. Rate/size limits. Treat media-derived text (filenames, metadata, transcripts, subtitles) as untrusted (prompt injection): return in clearly typed data fields.
- **Errors**: tool results with `isError:true` + structured `{code, message, hint, details, retryable}` including valid alternatives; protocol errors only for malformed requests.

## 5. Recommended architecture

```
 Web UI (Tauri IPC)   `cutweave mcp` stdio   MCP Streamable HTTP (in-app)   `cutweave` CLI (JSON)
        \                    |  proxy/headless       |                          /
         +-------------------+-----------------------+-------------------------+
                                       |
                       cutweave-api: CommandBus (submit / query / subscribe)
                                       |
          cutweave-core: command registry + handlers, project state (persistent snapshots
                      + revision), undo/transactions, job manager, event bus
                                       |
                    cutweave-engine (Rust): decode/composite/encode, caches, analysis
```
- Transports: Streamable HTTP in-app (primary), stdio attach-proxy and stdio headless, optional WebSocket/event stream for GUI/CLI. The UI uses Tauri IPC to the same bus (not via MCP).
- One `CommandBus` per open project; MCP adapter is thin, with no business logic.
- Auth: loopback + per-launch bearer token + Host/Origin validation; stdio via env/arg (spec: no OAuth for stdio [V]). Future remote: OAuth 2.1 with protected resource metadata (rmcp OAuth support [V]); Client ID Metadata Documents preferred over DCR [V].
- Headless: `cutweave mcp --project foo.cutweave [--profile full] [--allow-root DIR]` runs core without GUI; wgpu offscreen or software render fallback.
- Discoverability: `server/discover` + server `instructions` summarising the workflow ("call project_get then timeline_get; ids are opaque; time is rational"); MCP prompts for common workflows.

### 5.1 Representative tool catalog (~40) by namespace
Annotations: R=readOnly, W=mutating non-destructive, D=destructive, I=idempotent, J=job/task.
- **project**: `project_open`(W), `project_new`(W), `project_get`(R, summary+revision), `project_save`(W,I), `project_close`(W), `project_settings_set`(W,I: rate, resolution, color space).
- **media**: `media_import`(W; allowlisted paths; returns asset ids + probe), `media_list`(R, paged), `media_get`(R, probe), `media_thumbnails`(R, image), `media_waveform`(R, image/peaks), `media_analyze`(J: scene detect, silence, transcribe, beats).
- **timeline**: `timeline_get`(R, depth/limit/range), `timeline_find`(R), `timeline_batch`(W, transactional, dry_run), `timeline_marker_add`(W).
- **clip**: `clip_add`(W), `clip_move`(W,I), `clip_trim`(W,I), `clip_split`(W), `clip_delete`(W, ripple option), `clip_set`(W,I: transform/speed/volume/opacity), `clip_link`(W).
- **track**: `track_add`(W), `track_set`(W,I), `track_remove`(W).
- **effect**: `effect_list`(R, catalogue + param schemas), `effect_apply`(W), `effect_set`(W,I: params/keyframes), `transition_add`(W).
- **caption/text**: `caption_generate`(J), `text_add`(W), `caption_edit`(W).
- **audio**: `audio_set`(W,I), `audio_loudness`(R/J), `audio_normalize`(W).
- **render**: `render_frame`(R, image), `render_contact_sheet`(R, image), `render_preview_clip`(J, resource_link), `export_start`(J, destination checked, D if overwrite), `job_get`(R), `job_cancel`(W).
- **edit**: `edit_undo`, `edit_redo`, `edit_history`(R), `edit_checkpoint`.
- **meta**: `cutweave_search_commands`(R), `cutweave_describe_command`(R), `cutweave_execute`(W), `cutweave_query`(R), later `cutweave_script`.
- **interchange**: `otio_export`, `otio_import`; later FCPXML/EDL.
Prompts: `rough_cut_from_transcript`, `add_captions`, `make_vertical_cutdown`, `beat_sync_montage`, `audit_timeline` (gaps, clipping, black frames).

### 5.2 Mapping to the bus
- Each tool handler = `CommandBus::submit(Command::ClipTrim{...})`; tool inputSchema == command args schema (schemars), outputSchema == `Outcome` (changed ids, new revision, diff, warnings). `timeline_batch`/`cutweave_execute` = `Command::Batch(Vec<Command>)` with `$ref` back-references between steps (e.g. `{"$ref":"#/steps/0/created/0"}`) so an agent can create then configure in one round trip.
- Registry is generated: a `#[command]` derive registers metadata; a build test fails if a command lacks summary, risk class or example.

## 6. Other channels - what is worth it [D]
- **CLI (`cutweave`, JSON in/out)**: YES, essential. Same registry; `cutweave exec cmd.json`, `cutweave schema`, `cutweave query`, NDJSON event stream. Shell-capable agents use far less context than MCP; great for CI.
- **Local HTTP/WebSocket API**: YES, minimal: reuse the in-app axum server for `/api/v1/commands` (OpenAPI generated from the registry) and a WS event stream.
- **Scripting layer**: Phase 2. Rhai (pure Rust, easy sandbox) or embedded JS (QuickJS) for macros/plugins, also exposed as `cutweave_script`. Python as an out-of-process SDK generated from schemas rather than embedded. [U on engine maturity]
- **OpenTimelineIO**: YES for interchange (Resolve/Premiere/Avid; rational time, JSON); tracks/clips/transitions/markers only, effects not carried [U, not fetched]. Not the internal model. EDL/FCPXML later.
- Not early: gRPC, GraphQL, hand-written multi-language SDKs.

## 7. Pitfalls
1. Tool/context bloat: >40 always-on tools or verbose schemas. Tiers, profiles, search/describe, byte-capped outputs; CI budget check on `tools/list` token size.
2. Unbounded state dumps (Figma 351k-token case [V]): enforce depth/limit/pagination; summaries first.
3. Image token cost/latency: small default frames, contact sheets, JPEG; opt-in `preview:true`.
4. Concurrency with the live GUI: revisions, actor tagging, undo interleaving; GUI selection must not leak into command semantics.
5. Long ops over a stateless protocol without SSE resume [V]: jobs outlive requests; support both tasks extension and job-id polling since client support varies [U].
6. Spec churn: 2026-07-28 removed handshake/sessions and deprecated roots/sampling/logging; many clients still speak 2025-xx. rmcp 3.x supports legacy mode [V]; test old and new clients; pin rmcp.
7. Security: DNS rebinding / web page hitting localhost MCP (Host/Origin + token), path traversal, prompt injection via media metadata/transcripts, token-file permissions, agent-run scripts. Annotations are only hints [V].
8. Float time drift / off-by-one frames: rational time, half-open ranges, test 29.97/23.976 and VFR.
9. Schema instability: renames break clients (Figma [V]); versioned registry, aliases, snapshot tests.
10. Index-based addressing breaks inside batches; use ids and `$ref`.
11. GUI/bus logic drift: enforce by lint/CI that UI code mutates only via the bus.
12. stdio hygiene: only protocol on stdout; logs to stderr [V per logging deprecation guidance]; tokio stdin shutdown quirk [V].

## 8. Test strategy
- **Schema snapshot tests**: dump registry + `tools/list` + `server/discover` to JSON golden files (insta); CI fails on unreviewed change; breaking-change check (removed/renamed fields); token-size budget on `tools/list`.
- **Bus tests (no MCP)**: proptest random command sequences preserve invariants (no overlap on a track, consistent durations); undo/redo round-trip; transaction rollback leaves state identical; `$ref` resolution.
- **MCP conformance**: MCP Inspector (CLI mode in CI, UI manually) against stdio + HTTP [U exact invocation]; official conformance suite if available [U]; test stateless 2026-07-28 and legacy 2025-11-25 clients; auth failures (401/403, bad Origin/Host).
- **In-process client e2e**: rmcp client over the worker transport; scripted scenarios (import, rough cut, captions, export) asserting state + golden frames (SSIM/perceptual hash tolerance).
- **Agent e2e (nightly, non-blocking)**: scripted agent harness (e.g. Claude Agent SDK) with natural-language tasks graded by state assertions; track tool-call counts and tokens; deterministic replay of recorded transcripts for PR CI.
- **Fuzzing**: cargo-fuzz on command JSON; path-allowlist tests (symlinks, `..`, UNC, 8.3 names on Windows); script sandbox limits.
- **Parity**: same scenario via GUI IPC, CLI, MCP yields identical project state hash.
- **Windows specifics**: path normalisation, loopback firewall prompt, CRLF on stdio.

## 9. Phasing
P0: command registry + bus + CLI + MCP stdio headless with ~30 core tools + render_frame/contact sheet. P1: in-app Streamable HTTP + token + attach proxy, resources, jobs/tasks, transactions/dry-run. P2: scripting tool, OTIO, prompts, OAuth for remote, profiles/list_changed tuning.

## Sources
- MCP 2026-07-28 changelog: https://modelcontextprotocol.io/specification/2026-07-28/changelog
- MCP authorization (2026-07-28): https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization
- Other revisions: https://modelcontextprotocol.io/specification/2025-11-25/changelog , https://modelcontextprotocol.io/specification/2025-06-18/changelog
- Tool annotations blog: https://blog.modelcontextprotocol.io/posts/2026-03-16-tool-annotations/
- rmcp: https://github.com/modelcontextprotocol/rust-sdk , https://crates.io/api/v1/crates/rmcp , https://docs.rs/rmcp
- rmcp-server-kit: https://docs.rs/crate/rmcp-server-kit/2.1.1/source/AGENTS.md ; tower-mcp: https://docs.rs/crate/tower-mcp/0.18.1
- Anthropic, code execution with MCP: https://www.anthropic.com/engineering/code-execution-with-mcp
- Cloudflare Code Mode: https://blog.cloudflare.com/code-mode-mcp/
- Tool-bloat commentary (unverified blogs): https://thenewstack.io/how-to-reduce-mcp-token-bloat/ , https://getunblocked.com/blog/mcp-tool-overload/ , https://engineering.leanix.net/blog/code-mode
- DaVinci Resolve MCP: https://github.com/samuelgursky/davinci-resolve-mcp , https://pypi.org/project/resolve-mcp/ , https://mcpservers.org/servers/Tooflex/davinci-resolve-mcp
- Figma MCP issues: https://developers.figma.com/docs/figma-mcp-server/mcp-clients-issues
- Tauri MCP debug server: https://github.com/hypothesi/mcp-server-tauri
- OpenEdit: https://glama.ai/mcp/servers/Davdadev/Openedit

Not researched in depth (unverified): Ableton/OBS/Kdenlive/Shotcut/Remotion MCP servers, OTIO format details, MCP Apps extension, exact image-token costs, conformance-suite availability.
