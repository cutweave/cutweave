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
  request(method: string, params: unknown, meta?: RequestMeta): Promise<unknown>;
  subscribe(event: string, handler: (payload: unknown) => void): () => void;
  settled?(): Promise<void>; // every subscription so far is in effect on the engine
}
```

`MockTransport` is the in-memory test double: `handle(method, fn)` answers requests, `emit(event, payload)` pushes events, and `requests` records what was sent. `RpcClient` also has `snapshot()` (`project.snapshot`), `subscribed()` and `onUntyped(name, handler)` for events the generated `EventMap` does not know yet (`playback.state`, `view.changed`).

### RpcConnection (WebSocket)

`RpcConnection` is the real `Transport` ([rpc.md](rpc.md)): JSON-RPC 2.0 over `ws://127.0.0.1:<port>/rpc`.

```ts
const connection = new RpcConnection({
  resolve: async () => {            // called on every (re)connect: port and token may be new
    const c = await window.cutweave.engine.getConnection();
    return c && { wsUrl: c.wsUrl, token: c.token };
  },
  hooks: { onDisconnected, onReconnected }, // see below
});
const rpc = new RpcClient(connection);
connection.connect();
await connection.whenReady();
```

- The token travels only as the WebSocket subprotocol `cutweave.token.<token>`, never in the URL.
- Mutating calls get a UUIDv7 `_meta.request_id` (`uuidv7()`); reads and `view.*` get none, so a repeat is never answered from the dedup window. `needsRequestId(method)` decides, backed by `READ_ONLY_METHODS` (a test checks it against `schema/commands.json` and `schema/queries.json`). Pass `{ request_id, expected_rev, tx }` as the third argument of `call` to set the envelope yourself.
- Responses are matched by JSON-RPC id (a read may overtake a write). `result._meta` is stripped from typed outputs; `connection.requestWithMeta()` returns it (`rev`, `committed_rev`, `tx_id`).
- After the handshake (`rpc.hello`; a `schema_hash` that differs from `SCHEMA_HASH` calls `hooks.onSchemaMismatch`) the engine subscriptions of every registered handler are sent again, then the state is `ready`. Subscribe first, then read `project.snapshot`: `await rpc.subscribed()` between the two.

**State machine** (`connection-state.ts`): `idle` to `connecting` to `handshaking` to `ready`; a lost socket (or a failed attempt) goes to `reconnecting`, where a `ReconnectPolicy` (default `backoffPolicy()`: 250 ms doubling to 4 s, 20 tries, immediate first retry after close code 4001) schedules `retry` or `give_up` (`failed`); `close()` is final. Hook points for the engine supervisor work (WP-1L.03):

| Hook point | Purpose |
|---|---|
| `resolve()` | reads the current port and token on every attempt; `connection.reconnectNow()` skips the backoff |
| `hooks.onStateChange`, `connection.onStateChange` | drive banners and disabled controls |
| `hooks.onDisconnected({code, reason, willRetry, unacked})` | the socket was lost while `ready`; `unacked` lists the calls still waiting, with their `request_id` |
| `hooks.onReconnected(hello, {outbox, connection})` | the handshake after a loss finished; `hooks.onConnected` is the first one |
| `connection.outbox` | every unanswered call keyed by `request_id`; `retainUnacked: true` keeps calls that carry one (they are sent again, unchanged, after the next handshake) instead of rejecting them; reads always reject |
| `autoFlush: false` + `connection.flushOutbox()` | take over the order of re-sending (for example snapshot first) |

Without `retainUnacked` a call that was waiting when the socket dropped rejects with `RpcError("unsupported")`; calls made before the first handshake wait for it. A call bound to the lost socket's transaction (`edit.begin_tx`, `edit.commit_tx`, `edit.cancel_tx`, or `_meta.tx`) is never kept: the engine rolled the transaction back, so it rejects with `RpcError("conflict")` whose `data.reason` is `transaction_lost` (`isTransactionLost(error)`), and its owner starts the gesture again.

**Under a supervisor** (WP-1L.03): `supervisedPolicy(hint)` wraps a policy with what the supervisor says. `restarting` waits (polls every second, never gives up) until `reconnectNow()`; `down` gives up at once; `up` and `unknown` use the base policy.

**In the editor** (`packages/editor/src/engine/runtime.ts`) the connection runs with `retainUnacked: true`, `autoFlush: false` and `supervisedPolicy` fed by Electron main's `engine:status`:

- After every handshake: `project.snapshot` first, then `flushOutbox()` (the waiting commands go out under their original `request_id`; the engine answers a repeat from its deduplication window, which journal recovery restores), then `view.get` and `job.list`.
- `getConnection()` is read again on every attempt (a new port and token after a restart). A status with a higher `generation` than the connected one makes the runtime drop the old socket and reconnect; after the resync the "Engine restarted — recovered" notice shows what main restored (`status.recovery`) for 3 s, or warns that an untitled project was not kept.
- Main `failed`, or a `schema_hash` that differs from `SCHEMA_HASH`: editing is blocked with a message, waiting commands are rejected and nothing is re-sent to that engine. `ready` again (the dialog's Restart, a matching engine) lifts the block.
- A port change on restart reloads the editor (its CSP names the port), so the outbox of the old page is lost; the new page resyncs from the snapshot.

## Keeping a document mirror

`DocSync` keeps a `DocSink` current by the revision-gap protocol of [rpc.md](rpc.md): `start()` subscribes to `doc.patched` and `doc.reset`, then reads the snapshot; each event is applied when `prev_rev` continues the mirror, dropped when it is not newer, and a gap (or `doc.reset`, or a patch that does not apply) triggers one `project.snapshot` (concurrent triggers share it; events that arrive meanwhile are replayed on top). Call `resync()` after every reconnect. `DocMirror` is a framework-free sink; the editor's `docStore` is another. `applyDocEvent(state, event)` is the pure step both use.

## Applying patches

`doc.patched` carries `ops`, the RFC 6902 subset the engine uses: `add`, `remove`, `replace` and `move`, where `remove` and `replace` carry the expected old value (`old`). `applyPatch(doc, ops)` returns the new document, shares unchanged subtrees and never mutates its input. It throws `PatchError` (with the failing `opIndex`) when an op does not apply, for example when `old` no longer matches; the caller then resyncs from a snapshot. It is tested against `schema/patch-fixtures.json` in both directions (forward and inverse).

## Regenerating

```sh
cargo xtask gen          # rewrite schema/ and the generated TS
cargo xtask gen --check  # fail with a diff if anything is stale
pnpm --filter @cutweave/protocol test
```
