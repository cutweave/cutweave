# JSON-RPC over WebSocket

The engine's command and event channel for the desktop UI and scripts: JSON-RPC 2.0 over a WebSocket at `ws://127.0.0.1:<port>/rpc`. Implemented in `cutweave_api::rpc` (WP-1I.02); the engine plugs in through `cutweave_engine::rpc_mounts`. Architecture: [§5.1](../01-architecture.md#51-commands-queries-events), [§5.2](../01-architecture.md#52-transactions-undo-concurrency), [§6](../01-architecture.md#6-api-surfaces-cutweave-api), [§19](../01-architecture.md#19-performance-budgets-ci-benchmarked-on-reference-machines); decisions: [ADR-0015](../adr/0015-wire-protocol.md) (wire format, `request_id`), [ADR-0016](../adr/0016-credential-classes.md) (credentials), [ADR-0017](../adr/0017-transaction-kinds.md) (transactions). The TypeScript side is `@cutweave/protocol` ([protocol.md](protocol.md)).

## Connecting

| | |
|---|---|
| URL | `ws://127.0.0.1:<port>/rpc` (the port from the [launch protocol](launch-protocol.md)'s ready line, or the run-file) |
| Credential | `Authorization: Bearer <token>` on the upgrade request, or, where headers cannot be set (browsers), the subprotocol `Sec-WebSocket-Protocol: cutweave.token.<token>`; the server echoes that subprotocol back. The UI credential maps to actor `user`, an agent token to `agent:<label>`. |
| Host / Origin | `Host` must be `127.0.0.1:<port>` or `localhost:<port>`; `Origin` must be absent, a launcher-configured origin (`app://cutweave`) or the engine's own. Otherwise 403 before the upgrade. |
| Frames | Text frames only, one JSON-RPC message (or batch) each, at most 16 MiB. A binary frame is answered with an `invalid request` error. |

Each WebSocket gets **one engine connection** with a connection id of its own (never reused). Transactions opened on it (`edit.begin_tx`) belong to it: **closing the socket rolls back its open transaction** and releases its soft locks. Requests still queued behind a running one are dropped when the socket closes; resend them (with the same `request_id`) after reconnecting.

The server closes a socket with:

| Close code | Reason | Meaning | Client action |
|---|---|---|---|
| 4001 | `resync_required` | The client fell behind its events (its queue of 1024 events overflowed) or the server missed events | Reconnect, `rpc.hello`, `rpc.subscribe`, `project.snapshot` |
| 1001 | `engine_stopped` | The engine's event stream ended | Wait for the supervisor's restart |

When the engine process stops, connections end without a close frame.

## Messages

Standard [JSON-RPC 2.0](https://www.jsonrpc.org/specification): requests with an `id` (string, number or `null`) get one response; notifications (no `id`) get none; a batch (an array) gets one array with the responses of its requests, in request order (nothing if it held only notifications). `params` must be an object (named params) or absent.

```json
{"jsonrpc":"2.0","id":7,"method":"clip.split","params":{"item":"itm_…","at":"4s","_meta":{"request_id":"0190f5c2-a1b2-7c3d-8e4f-5a6b7c8d9e0f"}}}
{"jsonrpc":"2.0","id":7,"result":{"left":"itm_…","right":"itm_…","at":282240000,"linked":[],"_meta":{"rev":12,"committed_rev":12,"request_id":"0190f5c2-a1b2-7c3d-8e4f-5a6b7c8d9e0f","tx_id":"tx_9"}}}
```

## Methods

Every registered command, query and engine command is a method under its registry name (`clip.split`, `timeline.get`, `edit.undo`, `job.wait`, `project.open`, …), with `params` = its input. [commands.md](commands.md) and `schema/commands.json` list them with their schemas. The result is the command's output with `_meta` added:

| `result._meta` field | Meaning |
|---|---|
| `rev` | The live document revision after the call (of the snapshot read, for queries) |
| `committed_rev` | The committed revision after the call |
| `request_id` | The idempotency key the document actor recorded (calls that ran on the document actor) |
| `tx_id` | The transaction the call ran in (calls that ran on the document actor) |

Reserved methods (not registry entries):

| Method | Params | Result |
|---|---|---|
| `rpc.hello` | `{client, version}` (`RpcClientHello`) | `{engine_version, schema_hash, rev, committed_rev, actor, project, shm?}` (`RpcServerHello`): compare `schema_hash` with the client's generated `SCHEMA_HASH`; `project` is the engine's project status (path, name, dirty, read-only, a pending `recovery`); `actor` is derived from the credential; `shm` (WP-1I.05) announces the preview ring of a Gui engine as JSON text `{name, epoch: "0x…", layout_version, mapping_size}` (clients also accept the object), absent without a ring |
| `rpc.subscribe` | `{events: [name, …]}`: event names or `*` (all) | `{events}`: the subscriptions now held |
| `rpc.unsubscribe` | `{events?: [name, …]}`; no `events` = all | `{events}` |
| `project.snapshot` | none | `{rev, committed_rev, project}`: the live document (resync) |

`rpc.hello` is optional and may be sent at any time.

## Envelope

`params._meta` carries the envelope (arch §5.1); it is removed before the params reach the command.

| Field | Meaning |
|---|---|
| `request_id` | A client-generated UUIDv7. **Required on every mutating call** (an entry not annotated read-only, `view.*` aside): missing, it is `invalid` at `/_meta/request_id`. A repeat within 10 minutes by the same actor with the same method and params returns the original result (also over another connection, also after a reconnect); with other params it is a `conflict` (`request_id_reused`). |
| `expected_rev` | Fail with `conflict` (`stale_rev`) unless the document is at this revision. Nothing changes. `data.current_rev` is the current revision, `data.expected_rev` the one sent, and `data.changed_since` says what the revisions in between touched: `{"items": ["itm_…", ...]}` (item ids in order; empty when no item was touched) or `"unknown"` (the revision is older than the last 200 changes of the document the engine has open, is not a past revision, or more than 100 items changed). Read the document again and retry with `current_rev` (WP-2I.04). |
| `tx` | Run inside this open transaction of the connection (`tx_<n>` from `edit.begin_tx`). Transaction ids are never issued twice, also not after the engine restarts. |
| `replace` | `true` on an update of an open `gesture` (with `tx`): the engine rolls the gesture's live changes back to the committed document and applies this command there, atomically, so the gesture commits the effect of its last update (timeline spec INV-301/302, ADR-0023, WP-2A.12). A refused update leaves the previous one in place. The items every update touched stay soft-locked until the gesture closes. Without `tx`, or in a batch or proposal: `invalid` at `/_meta/replace`. |

There is **no actor field**: the actor is always the connection credential's. An `actor` key (or any other unknown key) in `_meta` is ignored.

**Closing a transaction and sending the close again** (ADR-0043). `edit.commit_tx` and `edit.cancel_tx` take an optional `tx` in their params; without it they close the connection's open transaction, whichever that is when the request runs. A client that may send a close again (after a reconnect, or after the engine restarted) **names the transaction**: a repeat that names a transaction that is gone is answered from the deduplication window or refused with `not_found`, and can never close another one. A repeat without `tx`, sent after the client opened a new transaction, closes the new one. After an engine restart a repeated `edit.cancel_tx`, and a repeated `edit.commit_tx` of a transaction that changed nothing, get their original results like any other acknowledged request (the journal keeps them); a close the engine had not acknowledged before it stopped is refused (`not_found` with `tx`, `invalid` without), because the transaction's changes went with the engine.

Deduplication is the document actor's (WP-1B.03). It covers the document commands, the `edit.*` commands and queries inside a transaction. **Engine commands that run outside the document actor are not deduplicated by `request_id`**: `job.*`, `media.import`, `view.*`, the session commands (`project.open`, `project.close`, `project.save`, `project.save_as`, `project.recover`, `project.discard_recovery`, `project.set_autosave`) and subsystem commands. `request_id` is still required on the mutating ones, so clients need no special cases and the engine can deduplicate them later without a protocol change.

`expected_rev` is checked on **every call that is not read-only** (WP-2I.04). The document actor checks it together with the change, so nothing can slip in between. For an engine command outside the document actor it is checked against the live revision right before the command runs; the document can still change while such a command works. A read outside a transaction ignores it. A deduplicated repeat of a request that was already applied (same `request_id`) returns the first result whatever the revision is now.

## Ordering and concurrency

- A connection's **mutating calls run one at a time, in the order they were received**, each through the single document actor (so a pipelined drag gesture applies in order).
- A **read-only call** (queries, `job.wait`, `job.status`, `edit.history`, `rpc.hello`, `project.snapshot`) waits for the mutating calls received before it on the same connection, then runs concurrently: a long `job.wait` never holds up later edits. Responses can therefore arrive out of request order; match them by `id`.
- `rpc.subscribe` and `rpc.unsubscribe` take effect before the next message is read.
- A connection may have 64 requests (or batches) in flight; beyond that the server stops reading the socket until one completes.
- Different connections run concurrently; their commands interleave on the document actor.

## Errors

A failed call answers `{"jsonrpc":"2.0","id":…,"error":{"code":…,"message":…,"data":{…}}}`. `data` is the [`CmdError`](../01-architecture.md#51-commands-queries-events) wire form: it always has `code` (the machine code, `CmdErrorCode` in TypeScript) and `message`, plus the fields of that code. The TypeScript transport rejects with `RpcError(data.code, error.message, data.path)`. WP-1N will generate this table from the registry.

| `error.code` | `data.code` | When | Fields of `data` |
|---|---|---|---|
| -32700 | `invalid` | The frame is not JSON (the response `id` is `null`) | `path`, `msg` |
| -32600 | `invalid` | Not a JSON-RPC 2.0 request object, an empty batch, a binary frame | `path`, `msg` |
| -32601 | `not_found` | No such method (`kind: "command"`) | `kind`, `id` |
| -32602 | `invalid` | A param or `_meta` field failed validation; `path` is a JSON Pointer into the params (`/_meta/request_id` for a missing key) | `path`, `msg` |
| -32603 | `internal` | A bug or panic in the engine | `msg` |
| -32001 | `not_found` | An ID does not exist | `kind`, `id` |
| -32002 | `locked` | A locked track or item, or another connection's transaction | `kind`, `id`, `by?`, `tx?` |
| -32003 | `conflict` | `stale_rev` (with `current_rev`), `request_id_reused`, `history_target_modified`, `item_in_gesture` (with `holder` and `retry_after_ms`; see below) | `reason`, `current_rev?`, `retry_after_ms?`, `holder?`, `detail` |
| -32004 | `forbidden` | The credential lacks the scope (UI-only commands from an agent token) | `scope`, `msg` |
| -32005 | `unsupported` | A valid request this engine cannot do (a missing subsystem, the engine shutting down) | `msg` |

### Soft locks: `conflict` / `item_in_gesture`

While a `gesture` or `proposal` is open, the items its commands touched (also the items a ripple shifted, the linked members and attached children that moved with them, and the items of earlier `replace` updates) are soft-locked for every other actor (arch §5.2, WP-2A.12). A request by another actor that would change one of them fails before anything changes:

```json
{"code":"conflict","reason":"item_in_gesture","holder":"user","retry_after_ms":27500,"current_rev":41,
 "detail":"item `itm_…` is being edited in open gesture tx_12 by user (\"Move clip\"); retry in 27500 ms or once it closes; `edit.locks` lists who holds what",
 "message":"conflict (item_in_gesture): item `itm_…` is being edited … (current rev 41)"}
```

- `holder` is the actor holding the item (`user`, `agent:<label>`, `plugin:<id>`).
- `retry_after_ms` is the time left until the gesture's 30 s idle deadline (at least 250 ms); it is absent for a proposal, which waits for a person to accept or reject it.
- Retry once after the hint, or work on other items; never in a tight loop. The editor never retries on its own: it marks the held clips "In use by …" and shows the refusal in an agent-blue toast with the code. Commands on other items proceed, and the holder's own actor is never blocked.
- The same error reaches MCP clients (with a `hint` that names the holder and the retry time) and `cutweave exec` (stderr, exit 1).
- `edit.locks` lists the open transactions and their items; `tx.opened`, `tx.locked` and `tx.closed` announce them, and `gesture.cancelled` (`reason: "idle_timeout"`) says the engine rolled back a gesture nobody updated for 30 s.

## Events

Events are notifications named after the event, with the event's payload as `params`; a connection receives only the names it subscribed to (`rpc.subscribe`, `*` for all):

```json
{"jsonrpc":"2.0","method":"doc.patched","params":{"rev":13,"prev_rev":12,"committed_rev":13,"actor":"user","tx_id":"tx_10","cause":"command","request_id":"…","ops":[…]}}
```

| Events | Source |
|---|---|
| `doc.patched`, `doc.reset`, `doc.command_panicked`, `edit.history_changed`, `tx.opened`, `tx.locked`, `tx.closed`, `gesture.cancelled` | The document actor |
| `project.dirty`, `project.saved`, `project.recovery_available`, `project.autosave_failed`, `project.opening`, `project.opened`, `project.open_failed`, `project.closed` | The project file and the session (`project.opening` starts every `project.open`; `project.opened` or `project.open_failed` ends it; ADR-0029) |
| `job.progress`, `media.indexed`, `media.analysis_failed`, `media.import_progress` | Jobs and media analysis |
| `view.changed` | View state; not sent back to the connection that made the change |
| `playback.state`, `engine.degraded` | Subsystems (WP-1I.05) |
| `playback.meters` | Audio (WP-2E.02): 30 times a second while playing, `{t, master{peak_db[2], rms_db[2], true_peak_db, lufs_momentary, lufs_short_term, lufs_integrated, gr_db}, buses[{track, peak_db[2], rms_db[2]}]}`. `t` is the timeline time (flicks) of the end of the window; levels are dBFS (`[left, right]`), `null` when silent; RMS is on the AES17 scale (a sine reads its peak level); bus levels are post-fader (a muted track reads `null`); LUFS are `null` until measurable, the integrated loudness covers what played since the last seek; `gr_db` is the master limiter's deepest gain reduction (0: none). Nothing is sent while paused |
| `media.reverse_slow` | Proxies (WP-2D.01b): a clip plays a long-GOP file backwards and the file has no proxy, so reverse playback is slower until the proxy queued for it is ready; `{media_id, reason, job_id}` (`job_id` is null when none was queued: the proxy was evicted or failed before, and `media.generate_proxy` makes it again). Sent once per file per engine session |

`schema/events.json` has the payload schemas. Events of one source arrive in order (document events in revision order); there is no order across sources, nor between a response and the events its call caused.

**Backpressure.** Each connection has a bounded event queue (1024). A client that does not read fast enough is closed with **4001 `resync_required`**, as are all subscribed clients when the server itself misses events. Nothing is dropped silently.

## Resync: the revision-gap protocol

A UI document store mirrors the document from a snapshot plus `doc.patched` events:

1. `rpc.subscribe` with `doc.patched` and `doc.reset` (and whatever else it shows), **then** `project.snapshot`. Keep `{rev, project}`.
2. For each `doc.patched`: if `rev <= mirror.rev`, drop it (already in the snapshot); if `prev_rev == mirror.rev`, apply `ops` (`applyPatch` in `@cutweave/protocol`) and set `mirror.rev = rev`; otherwise revisions were missed: fetch `project.snapshot` again.
3. On `doc.reset` (the engine swapped the document: `project.open`, `project.close`, recovery), fetch `project.snapshot` again.
4. On close 4001, reconnect and start over at 1. After any reconnect, resend unacknowledged mutating calls with their original `request_id`s.

## Limits

`cutweave_api::rpc::RpcConfig`: events queued per connection (1024), requests in flight per connection (64), message size (16 MiB).

The command round trip (request, response and its `doc.patched`) for `clip.trim` on a 400-item project is budgeted at p95 < 16 ms (arch §19); the `rpc_engine` test gates it at 25 ms.
