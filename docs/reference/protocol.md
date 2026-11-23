# `@cutweave/protocol`

The TypeScript side of the wire contract ([architecture §5](../01-architecture.md), ADR-0015): types generated from the Rust command registry, a typed RPC client over an injected transport, and the patch applier the UI document store uses.

## Generated files

`cargo xtask gen` writes `packages/protocol/src/generated/` next to the JSON catalogs in `schema/`. Never edit these files; `cargo xtask gen --check` (part of `just check`) fails when they are stale.

| File | Content |
|---|---|
| `types.ts` | every wire type: `Project`, `Item`, command inputs and outputs, `CmdError`, `PatchOp`, ... |
| `commands.ts` | `CommandMap` (commands and engine commands), `QueryMap`, `RpcMap` (both), `CmdErrorCode` |
| `events.ts` | `EventMap`: event name to payload type |
| `schema-hash.ts` | `SCHEMA_HASH`, the sha256 of `schema/commands.json`, to compare with the engine's `rpc.hello` |

Conventions (ADR-0015): time values (`Time`, `Duration`) are `number` (flicks as a JSON integer, no `bigint`); IDs are branded strings (`ItemId = string & { readonly __brand: "ItemId" }`), so an `ItemId` cannot be passed where a `TrackId` is expected. Engine events whose payload types are not exported yet are `unknown` (marked `TODO` in `events.ts`).

## Calling the engine

```ts
import { RpcClient, type ItemId, type Time } from "@cutweave/protocol";

const rpc = new RpcClient(transport); // a Transport: WebSocket JSON-RPC in the app, MockTransport in tests

const out = await rpc.call("clip.split", { item, at }); // typed input and output
const off = rpc.on("doc.patched", (e) => store.apply(e)); // typed payload; call off() to unsubscribe

// Compile errors:
rpc.call("clip.split", { item }); // `at` is missing
rpc.call("nope", {}); // not a command
```

`call` accepts every entry of `RpcMap`, commands and queries alike. A failed call rejects with `RpcError` (`code: CmdErrorCode`, `message`, optional JSON Pointer `path`).

### Transport

```ts
interface Transport {
  request(method: string, params: unknown): Promise<unknown>;
  subscribe(event: string, handler: (payload: unknown) => void): () => void;
}
```

The real transport (WebSocket JSON-RPC with `rpc.hello`) lives with the UI and API server work packages. `MockTransport` is the in-memory test double: `handle(method, fn)` answers requests, `emit(event, payload)` pushes events, and `requests` records what was sent.

## Applying patches

`doc.patched` carries `ops`, the RFC 6902 subset the engine uses: `add`, `remove`, `replace` and `move`, where `remove` and `replace` carry the expected old value (`old`). `applyPatch(doc, ops)` returns the new document, shares unchanged subtrees and never mutates its input. It throws `PatchError` (with the failing `opIndex`) when an op does not apply, for example when `old` no longer matches; the caller then resyncs from a snapshot. It is tested against `schema/patch-fixtures.json` in both directions (forward and inverse).

## Regenerating

```sh
cargo xtask gen          # rewrite schema/ and the generated TS
cargo xtask gen --check  # fail with a diff if anything is stale
pnpm --filter @cutweave/protocol test
```
