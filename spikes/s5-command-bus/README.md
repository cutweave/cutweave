# Spike S5 (build): command bus, `#[command]` macro, registry, schema + TS codegen

WP-0H.01. **Throwaway.** This is its own Cargo workspace. It is excluded from the root workspace (`exclude = ["spikes"]`), is not part of `just check`, and nothing in `crates/` imports it. Only the findings below carry forward, to `cutweave-commands-derive` (1B.02), `cutweave-commands`/`-ops` (1B) and 1N.

It proves the arch §5 chain end to end on a toy document:

`#[command]` struct → `inventory` registry entry (meta + JSON Schemas + specta types + type-erased JSON runner) → engine (`Tx` with invertible patches, per-actor undo/redo, `request_id` dedup, `expected_rev`, batches, events) → `schema/*.json` + `ts/src/generated/commands.ts` (snapshot-tested, `tsc --strict`-checked).

> Deviations from the WBS text, both at the integrator's request: the folder is `spikes/s5-command-bus/` (the WBS says `spikes/s5-bus`), and `Tx`/undo/dedup are **in** scope (the WBS lists "transactions/undo" as out of scope).

## Layout

| Path | What |
|---|---|
| `macros/` (`s5-bus-macros`) | `#[command]`, `#[query]`, `#[wire]` attribute macros |
| `bus/` (`s5-bus`) | Runtime, standing in for `cutweave-model` + `cutweave-commands`: `time` (flicks `Time`, ADR-015 wire format, timecodes), `ids` (prefixed UUIDv7-shaped IDs, deterministic mode), `doc` (toy project), `tx` (`Tx`, `Patch`), `registry`, `engine`, `codegen`, `edit.undo`/`edit.redo` |
| `ops/` (`s5-ops`) | The toy commands, standing in for `cutweave-ops`: `project.open`, `media.import`, `timeline.insert`, `clip.split`, `clip.trim`, `item.delete`; queries `timeline.get`, `render.frame` (PNG); the `s5-codegen` binary; all integration tests |
| `schema/commands.json`, `schema/queries.json` | Generated JSON Schema catalogs (draft 2020-12) |
| `ts/src/generated/commands.ts` | Generated TS types, the `Commands` map and the typed `CutweaveClient` |
| `ts/src/sample.ts`, `ts/negative/wrong_field.ts` | Typecheck fixtures (positive with `@ts-expect-error` negatives; one file that must fail) |
| `mcp/` (`s5-mcp`) | WP-0H.02: rmcp MCP server (stdio + Streamable HTTP) over the registry; the `s5-mcp` binary; MCP tests. See [WP-0H.02: MCP server](#wp-0h02-mcp-server-rmcp-3110) |
| `agent-run/`, `MCP-AGENT-RUN.md` | WP-0H.03 runbook, checker (`check_run.py`) and evidence (scripted stdio dry-run transcripts) |

## How to run

Run these from `spikes/s5-command-bus/` (toolchain and rustfmt config are inherited from the repo root):

```sh
cargo test                                         # everything (trybuild takes ~30 s)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check

cargo run -p s5-ops --bin s5-codegen               # regenerate schema/ and ts/src/generated/
cargo run -p s5-ops --bin s5-codegen -- --check    # CI form: fail if stale

cargo insta review                                 # after an intended schema/TS change
TRYBUILD=overwrite cargo test -p s5-ops --test compile_fail   # after changing macro diagnostics

cargo check -p s5-bus -p s5-ops --lib --target wasm32-unknown-unknown
```

The TypeScript tests (`ops/tests/typescript.rs`) use the repo's pinned TypeScript (7.0.2), found by walking up to `node_modules/typescript`. Run `pnpm install` at the repo root first. Without it the tests print `SKIPPED`. Set `S5_REQUIRE_TSC=1` to turn a missing compiler into a failure (CI), or `S5_TSC=<path to tsc>` to point at a specific one.

## Acceptance criteria (WP-0H.01) and evidence

| Criterion | Status | Evidence |
|---|---|---|
| `cargo test` snapshot tests pass; changing a command field changes the snapshot and fails until reviewed | Met | `ops/tests/codegen.rs` (insta: `commands_json`, `queries_json`, `commands_ts`) and `committed_artifacts_are_up_to_date`. Verified by adding `snap: Option<bool>` to `SplitClip`: 3 tests failed with the diff (`+ snap?: boolean \| null`), then the field was reverted. |
| Generated TS compiles under `tsc --strict`; sample calls `client.call("clip.split", {...})` with compile-time-checked args; a wrong field fails `tsc` | Met | `ops/tests/typescript.rs` runs `ts/tsconfig.json`, which extends the repo's strict `tsconfig.base.json` (`exactOptionalPropertyTypes`, `noUncheckedIndexedAccess`, ...). `sample.ts` also asserts 5 negatives via `@ts-expect-error` (unknown field, missing field, bigint `Time`, unknown command, unknown output field). `negative/wrong_field.ts` must fail with exactly one TS2353 on `position`. |
| `Time` exports as TS `number` (no `bigint`); a value above 2^53-1 is `Invalid`; a timecode string parses | Met | `time_exports_as_number_never_bigint`, `time_above_max_safe_integer_is_invalid` (`{"code":"invalid","path":"/at",...}`), `timecode_inputs_parse_against_sequence_rate`, `agent_task_import_cut_and_render_frame` (`"at": "00:00:02:00"`, `"3s"`), and unit tests in `bus/src/time.rs`. |
| Tests: snapshot + trybuild compile-fail for a malformed `#[command]` | Met | `ops/tests/compile_fail.rs` runs 14 compile-fail cases with reviewed `.stderr` files, plus 1 pass case. |
| Extra (requested): `Tx` with forward+inverse patches; undo/redo; proptest `undo(apply(doc)) == doc` for each command; `request_id` dedup | Met | `ops/tests/undo_props.rs` has one property per undoable command plus undo-all/redo-all. A coverage test fails if a new undoable command lacks a generator, and a guard checks that the generators reach success paths. Mutation-checked: breaking `Patch::inverse` for items fails 3 properties. Dedup and the rest are in `ops/tests/engine.rs` (19 tests). |

Results on Windows 11, Rust 1.100: **49 tests pass** (8 unit, 7 codegen, 1 trybuild, 19 engine, 4 registry, 2 TS, 8 proptest). Clippy is clean with the root lint set (pedantic, `-D warnings`), and wasm32 `cargo check` is clean. Not run on macOS or Linux.

## Registry API (what WP-0H.02 builds on)

```rust
let registry: &'static Registry = s5_ops::registry();   // links s5-ops; or Registry::global()
for entry in registry.iter() {                          // sorted by name
    entry.meta.name;                // "clip.split"
    entry.meta.mcp_tool_name();     // "clip_split"
    entry.meta.summary;             // MCP tool description (<= 100 chars, enforced)
    entry.meta.kind;                // Command | Query
    entry.meta.mcp;                 // Core | Catalog | Hidden
    entry.meta.annotations;         // serializes as {readOnlyHint, destructiveHint, idempotentHint, openWorldHint}
    entry.params_schema();          // inputSchema: object root, additionalProperties:false, no $ref
    entry.output_schema();          // outputSchema: object root (checked at registry build)
}
let mut engine = Engine::new(EngineConfig::deterministic(seed));
let caller = Caller::agent("claude");                  // the transport derives this from the credential
engine.invoke_json(&caller, json!({"name": "clip.split", "params": {...}, "request_id": "..."}));
// -> {"ok":true,"rev":3,"result":{...},"dry_run":false,"deduplicated":false}
// -> {"ok":false,"error":{"code":"invalid","path":"/at","msg":"...","message":"..."}}
engine.execute_json(&caller, json!({"steps": [...], "dry_run": true}));  // one Tx, one undo entry
registry.get_by_tool_name("render_frame");             // render.frame -> {mime, data(base64), width, height, visible}
engine.subscribe();                                    // mpsc::Receiver<Event::DocPatched{rev, actor, label, ops}>
```

## Findings for Phase 1

### What worked

- **Attribute macro on a plain struct, plus a hand-written `impl Command`.** The macro emits a `const` `CommandMeta` and `inventory::submit!(CommandEntry::command::<T>())`. That is a `const fn` holding fn pointers to monomorphized generic runners and schema/TS hooks, so there is no runtime registration code, no `dyn` and no boxing. Registration works across crates on Windows/MSVC: `edit.*` lives in `s5-bus`, the rest in `s5-ops`.
- **The macro owns the wire contract.** It adds `Serialize, Deserialize, JsonSchema, specta::Type` (plus `Debug, Clone, PartialEq`) through `::s5_bus::__private` re-exports, using `#[serde(crate)]`, `#[schemars(crate)]` and `#[specta(crate)]`. Command crates therefore need no serde, schemars or specta dependency. It also forces `deny_unknown_fields`, so a mistyped agent argument becomes `Invalid{path:"/bogus"}` instead of being silently ignored. It adds `#[serde(default)]` to `Option<T>` fields, which makes them optional in serde, JSON Schema (not `required`) and TS (`field?: T | null`). `#[wire]` applies the same derives to output and nested types.
- **Policy is enforced at compile time.** Checks: the name grammar and the arch §5.1 namespace allowlist ("new namespaces require an ADR"), `summary` of 100 chars or fewer (tool-list budget), `undoable` stated explicitly, `annotations(destructive=…)` stated explicitly (MCP's `destructiveHint` defaults to true), named-field structs only (MCP needs an object `inputSchema`), no generics, and queries cannot be undoable or destructive. A forgotten `impl Command` reports a custom `#[diagnostic::on_unimplemented]` message on the struct.
- **Three entry kinds proved necessary.** `Command` takes `&mut Tx`. `Query` takes `&Doc`, so it is read-only by construction. `EngineOp` (`#[command(engine)]`) handles history and lifecycle ops such as `edit.undo`, which need the engine rather than a `Tx`. All three share one registry, schema pipeline and dedup path. A retried `edit.undo` with the same `request_id` undoes once.
- **Entity-level `{before, after}` patches.** Undo and redo are generic, with no per-command inverse code, and are coalesced per entity at commit. Undo checks that each entity still equals its `after` and returns `Conflict(history_target_modified)` otherwise. That check is what makes per-actor undo (arch §5.2) safe when another actor touched the same item. It is cheap, and the proptests run through the JSON path including timecode-string arguments.
- **schemars 1.x `Contract`** gives `Time` a schema that depends on direction. Params accept `integer | timecode-string`, and outputs are a plain `integer` with ±(2^53−1) bounds. `inline_subschemas = true` yields self-contained draft 2020-12 schemas with no `$ref`/`$defs`, the most portable form for MCP clients.
- **specta rc.28 + specta-serde `Format`.** `#[specta(type = specta_typescript::Number)]` on the newtype produces `export type Time = number`. Any other `i64`/`u64` is a hard export error, which is useful: `Rev` needed the same override. `framework_runtime` + `ctx.reference(&datatype)` generates the `Commands` map and the client in the same file as the types, so names can never drift.
- **`serde_path_to_error` gives `Invalid.path` as a JSON Pointer**, e.g. `/at`, `/items/1`, `/bogus`. Batches prefix it with `/steps/<i>/params`, which agents can act on directly.
- **Dedup key and replay.** The key is `(actor, request_id)` plus a canonical payload fingerprint (serde_json without `preserve_order` sorts keys). An identical retry replays the original result, a different payload returns `Conflict(request_id_reused)`, failures are not journaled, and entries expire after 10 minutes on an injectable clock.

### Gotchas

- **specta is still a release candidate.** `2.0.0-rc.28` and `specta-typescript 0.0.15` changed a lot from earlier RCs: `TypeCollection` became `Types`, `export` now needs a `Format`, and serde handling moved to `specta-serde`. Pin them with `=` and budget time for upgrades. The exporter's output is not formatter-clean (tabs, trailing spaces, `/**  ` doubled spaces), so `packages/protocol/src/generated` must stay excluded from Biome or be piped through `biome format`.
- **Directional TS types are expensive.** Putting `number | string` timecode inputs into TS would need specta-serde `PhasesFormat`, which renames every containing type to `*_Serialize`/`*_Deserialize`. This spike keeps TS `Time = number` (ADR-015) and documents timecode strings only in the JSON Schema, as a JSON/MCP/CLI convenience.
- **Timecodes need a frame rate at deserialize time.** `"HH:MM:SS:FF"` and `"f:N"` cannot be parsed without one. The spike uses a scoped thread-local that the engine sets to the sequence rate. That will not hold with several sequences at different rates. Phase 1 likely needs a `TimeInput` wire type resolved during semantic validation against the target's sequence. Drop-frame timecode (`;`) is rejected as unsupported.
- **Hand-written `JsonSchema` impls can drift from their custom `Deserialize`** (`Time`, IDs). Phase 1 should add a differential test: generated JSON instances, schema-valid versus serde-accepted.
- **The macro cannot see output types.** The object-root requirement for outputs is checked when the registry is built (`RegistryError::NotAnObject`). A `WireObject` marker trait could move this to compile time.
- **`Option` detection in the macro is syntactic.** It matches the last path segment `Option`, so type aliases are missed.
- **`inventory` linking.** A crate that only registers commands is not linked unless the binary references it, hence `s5_ops::registry()`. On wasm32 the crates compile, but runtime registration on `wasm32-unknown-unknown` is unverified. `uuid` v7 needs OS entropy, so it is gated off wasm. `Instant` panics on wasm, so the clock is injected.
- **Clippy in integration tests.** `allow-unwrap-in-tests` exempts `#[test]` bodies only, not helper fns in `tests/*.rs`, so each test crate needs an `#![allow(clippy::unwrap_used, …)]`. The root workspace will hit the same issue.
- **Binary output.** `render.frame` returns base64 inside JSON. Phase 1 needs a dedicated `ImageData` (or resource-link) wire type that MCP maps to image content blocks.
- **`project.open` as a `Tx`** (remove everything, insert a new sequence) is clunky. A doc swap as an `EngineOp` is cleaner. Non-undoable doc commands currently clear all history, which is conservative.

### Numbers

- 10 entries: 9 MCP-core, 1 catalog. (WP-0H.02 adds an 11th, `ai.set_key`: catalog, UI-only.)
- Rough `tools/list` estimate for core params only: about 1.8k tokens (bytes/4). WP-0H.02 should measure it properly, including `outputSchema`.
- `commands.json` is 26 KB, `queries.json` 16 KB, `commands.ts` 12 KB.
- Each proptest property runs 96 cases. The suite runs in under 1 s, excluding about 30 s of trybuild.

### Open questions

1. Which frame rate applies to frame-based timecode inputs in multi-sequence projects? Should `Time` params be snapped to the frame grid at the boundary or inside commands? The spike does not snap.
2. Should `idempotent` also be mandatory? Should agents (MCP) be *required* to send `request_id` for commands? The spike treats it as optional.
3. The dedup journal is in memory. Arch §5.1 says clients resend after an engine restart, so the journal must persist with the project autosave.
4. Are entity-level patches fine-grained enough for `doc.patched` events and a later Loro/CRDT mapping (ADR-005), or does Phase 1 need field-level ops?
5. What should per-actor redo do after another actor's edit? The spike re-validates `before` and returns a conflict.
6. specta 2.0 final versus a fallback (`ts-rs`) if the RC churn continues. The specta-specific code is small (`#[specta(...)]` overrides and the `framework_runtime` client template).

## WP-0H.02: MCP server (rmcp 3.11.0)

`s5-mcp` (in `mcp/`) serves the registry over MCP. It stands in for the MCP module of `cutweave-api` (arch §6 and §14, ADR-009, WP 1J). rmcp is pinned to `=3.11.0`, the latest release on crates.io on 2026-11-22 (published 2026-11-17).

```sh
cargo run -p s5-mcp -- stdio [--profile minimal|standard|full] [--agent-label L] [--state-file P] [--log-file P]
cargo run -p s5-mcp -- http  [--port N] [--run-file P] [--allow-origin O] [--state-file P]
cargo run -p s5-mcp -- budget                      # tools/list size per profile
cargo test -p s5-mcp                               # 22 tests (rmcp clients, raw JSON-RPC, raw HTTP)
S5_RECORD_DRY_RUN=1 cargo test -p s5-mcp --test stdio_dry_run   # re-record agent-run/stdio-dry-run-*
```

The WP-0H.03 runbook (Claude Code over stdio, the checker and the Inspector steps) is [MCP-AGENT-RUN.md](MCP-AGENT-RUN.md).

### As built

- **Core tools.** Every registry entry with `mcp = "core"` that is not UI-only becomes a tool. It keeps its name (`clip_split`), its `summary` as the description and its params schema *verbatim* as `inputSchema`, so the schema is identical to RPC. `annotations` come from the macro metadata, with `openWorldHint: false`.
  - Results carry `structuredContent`, the RPC reply shape `{rev, result, dry_run?, deduplicated?}`, plus the same JSON as text.
  - The matching `outputSchema` is listed only on `/mcp/full`.
- **Generic tools** reach every non-hidden entry, including `catalog` ones (`edit.redo`, `ai.set_key`):
  - `cutweave_search_commands` does keyword matching over name and summary.
  - `cutweave_describe_command` returns both schemas and how to call the command.
  - `cutweave_execute` takes `steps`, `dry_run`, `expected_rev` and `request_id`. It runs all steps in one `Tx` and one undo entry, with `{"$ref":"#/steps/N/<pointer>"}` between steps. A single engine-op or non-undoable step runs as a plain call; otherwise `edit.redo` and `project.open` would be unreachable.
  - `cutweave_query` runs a query by name.
  - Wire names (`clip.split`) and tool names (`clip_split`) are both accepted.
- **Envelope for core tools.** `request_id`, `expected_rev` and `dry_run` travel in request `_meta` (`io.cutweave/*`), so `inputSchema` stays identical to RPC. LLMs cannot set `_meta`, so for a model these controls exist only on `cutweave_execute`.
- **Errors.** A command error becomes a tool result with `isError: true` and text `{"error":{code,path?,msg,message}}`, so the model can correct itself. Only an unknown tool is a JSON-RPC error (-32602).
- **Images.** An output schema with string `mime` and `data` is an image. The bytes go out as an MCP `image` content block (first), and `data` is removed from `structuredContent` and from `outputSchema`. This also works for `render.frame` inside a batch. Frames are PNG ≤ 768 px (the WBS says JPEG ≤ 768 px; a synthetic frame needs no JPEG encoder dependency).
- **Profiles per endpoint.** HTTP serves `/mcp` (standard), `/mcp/minimal` and `/mcp/full` over one engine; stdio uses `--profile`. Minimal lists `timeline_get`, `render_frame`, `edit_undo` and the 4 generic tools.
- **Change notifications.** There is one resource, `cutweave://project` (summary + rev), and one internal `Notifier` fed by the engine's `doc.patched` events. It writes either to the session peer (2025-11-25 `resources/subscribe`) or to the `subscriptions/listen` sink (2026-07-28). Nothing uses `ping`.
- **Credentials (arch §6).**
  - On stdio, the actor is `agent:<--agent-label>`: the process boundary is the credential.
  - Over HTTP, an axum layer runs before rmcp and checks `Host` (`127.0.0.1:<port>` / `localhost:<port>`), then `Origin` (allowlist, empty by default), answering 403, then `Authorization: Bearer <agent token>`, answering 401 with `WWW-Authenticate`. The token's label becomes the actor; the request body never can.
  - rmcp's own Host/Origin checks are configured as a second layer.
  - The run-file `{pid, port, url, endpoints, agent_label, agent_token}` is written with mode 0600 on Unix and removed on exit.
- **UI-only scope.** It is registry metadata (`#[command(scope = "ui")]`), and the macro rejects `scope = "ui"` together with `mcp = "core"`. The **engine** refuses it to any caller without the UI credential with the new error `CmdError::Forbidden{scope:"ui"}`, for single calls and for every batch step. `ai.set_key` (engine op, catalog) is the example.

### Acceptance (WP-0H.02 brief) and evidence

| Criterion | Status | Evidence |
|---|---|---|
| In-process rmcp client drives import, insert, split, render frame, undo and asserts doc state | Met | `mcp/tests/agent_flow.rs`: both revisions via MCP and directly on the engine, plus a minimal-profile variant through `cutweave_execute` with `$ref` |
| `tools/list` size test against the budgets (documented heuristic) | Met | `mcp/tests/tool_budget.rs` and `mcp/src/budget.rs`; see the table below |
| Conformance against 2025-11-25 and 2026-07-28 as far as rmcp supports | Met: everything below passes on both | `mcp/tests/conformance.rs` (in-process, child-process stdio and HTTP, each × 2 revisions; sessions on the wire; negotiation edges) |
| Security: wrong/missing token → 401; bad Origin/Host → 403; UI-only scope from an agent token → forbidden | Met | `mcp/tests/security.rs`, which also covers actor-from-token (two tokens, separate undo) and the run-file |
| `MCP-AGENT-RUN.md` + one scripted dry run through stdio | Met | [MCP-AGENT-RUN.md](MCP-AGENT-RUN.md); `mcp/tests/stdio_dry_run.rs` drives the real binary with raw JSON-RPC (no rmcp client) for the exact WP-0H.03 prompt on both revisions; transcripts and server state files in `agent-run/`, `check_run.py` passes |
| WBS: Inspector over stdio and HTTP | Open (human, WP-0H.03) | MCP-AGENT-RUN.md §6; not run here because it downloads and executes an npm package |
| WBS: record results in `docs/spikes/S5-report.md` | Deferred to WP-0H.03 | outside this WP's area; numbers are here |

Results on Windows 11 with Rust 1.100: the workspace has **73 tests passing**. That is 51 in the WP-0H.01 crates (2 new engine tests: `ui_only_scope_needs_the_ui_credential` and `batch_steps_reference_earlier_results_with_ref`; 1 new trybuild case: `ui_only_core`) and 22 in `s5-mcp`. Clippy (pedantic, `-D warnings`) and `cargo fmt --check` are clean, and wasm32 `cargo check` of `s5-bus`/`s5-ops` is still clean. Not run on macOS or Linux.

### `tools/list` size (`s5-mcp budget`)

| profile | tools / max | bytes | tokens ~bytes/4 | tokens upper (bytes/3) / max | model-visible ~tokens | outputSchema ~tokens |
|---|---|---|---|---|---|---|
| minimal | 7 / 12 | 3873 | 969 | 1291 / 3000 | 786 | 0 |
| standard | 13 / 36 | 10824 | 2706 | 3608 / 8000 | 2365 | 0 |
| full | 13 / 36 | 19903 | 4976 | 6635 / 8000 | 2365 | 2234 |

- **Heuristic** (`mcp/src/budget.rs`). There is no offline Claude tokenizer. Budgets are asserted on the pessimistic `bytes/3` and also reported as `bytes/4`, over the compact JSON of the `tools` array.
- **Model-visible** counts name + description + `inputSchema` only.
- **P1 gate.** Standard is also checked against ≤ 19 tools / ≤ 5k tokens.
- **Calibration** against Claude Code's `/context` is step 3 of MCP-AGENT-RUN.md.

### MCP spec revisions with rmcp 3.11.0 (measured)

| | 2025-11-25 | 2026-07-28 |
|---|---|---|
| Lifecycle | `initialize`/`initialized`. The server echoes the requested version if supported (2025-06-18 tested), otherwise the newest version with a handshake | `server/discover` (`supportedVersions`: all 5 revisions rmcp knows) + per-request `_meta` `protocolVersion` + `clientCapabilities` |
| HTTP | `Mcp-Session-Id` session (`legacy_session_mode = true`); notifications on the GET stream | Stateless (no session id, whatever the flag). `MCP-Protocol-Version` header **must** mirror `_meta`, else HTTP 400 (-32020) |
| tools/list, tools/call, structured output, image content, resources/read | ✓ identical tools and order | ✓ identical; rmcp adds `resultType`, `ttlMs` (default 0) and `cacheScope: private` |
| Change notification | `resources/subscribe` → `notifications/resources/updated` ✓ (stdio, HTTP) | `subscriptions/listen {resourceSubscriptions}` → ack + `resources/updated` on the listen stream ✓ (stdio, HTTP) |
| Other revision's mechanism | `subscriptions/listen` → -32601 | `resources/subscribe` → -32601 |
| `ping` | ✓ | -32601 (removed); not relied on |
| Clients exercised | rmcp 3.11.0 (in-process, child-process, HTTP), raw JSON-RPC (stdio dry run), raw HTTP | same |

Not exercised: the tasks extension, MRTR/elicitation, and non-rmcp SDK clients (Claude Code and others: WP-0H.03).

### Findings for ADR-009 and 1J

1. **One rmcp server serves both revisions with the same handler code.** rmcp owns negotiation, sessions versus statelessness, `resultType` and cache hints, and -32002 versus -32602 for missing resources. No fallback or shim was needed. ADR-009 can say "both revisions, rmcp `=3.11.0`".
2. **Notification mechanism per revision works.** rmcp gap: it acknowledges `subscriptions/listen` *before* calling `listen()` and has no pre-ack hook, so a change in between is lost. Workaround: join the feed in `accepted_subscription_filter`. Report it upstream or keep the workaround.
3. **Over HTTP, 2026-07-28 requires the SEP-2243 headers** (`MCP-Protocol-Version`, `Mcp-Method`). Hand-rolled or proxy clients (the planned `cutweave mcp --attach`) must send them.
4. **rmcp's Origin validation is off by default**, and its Host allowlist matches loopback names on any port. Phase 1 must configure both explicitly, and put the 403 check before auth.
5. **outputSchema costs about 45% of `tools/list`** (`timeline_get` alone is 3.3 KB). At about 200 tokens per core tool, around 30 core tools would land near 6–7k (bytes/4), roughly 9k on the bytes/3 bound, so over 8k. Recommendation: list output schemas only in `full` (as built), keep `structuredContent` everywhere, and keep schemas lean.
6. **`#[command]` doc comments become agent-facing text**, since root `description` is about 45% of `inputSchema` bytes. Two consequences:
   - Write doc comments for agents, without internal references; `edit.undo` currently ships "(arch §5.2)".
   - Minimal-profile membership (`MINIMAL_CORE`) belongs in macro metadata, not in the transport.
7. **Envelope placement is a decision for ADR-009/015.** `_meta` keeps schemas identical to RPC, but models cannot use it. The alternative is optional `request_id`/`expected_rev` properties on every mutating tool, at about 40 tokens each.
8. **UI-only scope must be enforced in the engine, not the transport.** Otherwise `cutweave_execute` would bypass it. This adds `forbidden` to the error set (contract change, ADR-015).
9. **The dedup fingerprint keeps params in plaintext** for 10 minutes, so `ai.set_key`'s key sits in the journal. Phase 1 should hash fingerprints and never journal secret-bearing params.
10. **Image outputs want a dedicated wire type** (confirms the WP-0H.01 finding). The `mime` + `data` detection rule works but is implicit.
11. **The spike `Tx` does not roll back on panic.** `EngineHandle` keeps serving after a poisoned lock, which Phase 1 must not copy.
