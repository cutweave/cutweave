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

- 10 entries: 9 MCP-core, 1 catalog.
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
