# Command scripts (CLI = MCP = RPC parity)

Each `*.json` file here is a command script that the parity test (`crates/cutweave-api/tests/parity`, WP-1O.02) runs on every surface, each time against a fresh engine with `--deterministic-ids 42` (seeded ids, the clock frozen at the Unix epoch) and the dev stub media backend:

| Surface | How a step runs | Actor |
|---|---|---|
| CLI | a `cutweave exec` (or `cutweave query`) child process attached to the engine through its run-file | `agent:cli` |
| MCP | an rmcp client (MCP 2025-11-25) over the stdio transport of `cutweave mcp --headless`, served in the test process so its engine's events can be observed: the core tool when one is listed, otherwise `cutweave_query` or `cutweave_execute` | `agent:mcp` |
| MCP (process) | the same client (MCP 2026-07-28) on the stdin and stdout of a `cutweave mcp --headless --deterministic-ids 42 --dev-stub-media --allow-root <root>` child process; its engine is in the child, so its `doc.patched` events are not compared | `agent:mcp` |
| RPC | a raw JSON-RPC WebSocket client of `/rpc` | `agent:rpc` |

After the last step the test saves the document through the same surface (`project.save_as`) and requires the `.cutweave` files to be **byte-identical** (equal sha256, arch §20), the `doc.patched` sequences to carry the same ops and revisions (the actor, the client's `request_id` and the transaction route, `cause` and `tx_id`, may differ), the step outputs to be equal (in `edit.*` history results, apart from the actor and the client's own transaction label), and failing steps to fail with the same error `code` (and `failed_step`). Run it with `cargo nextest run -p cutweave-api --test parity`.

The saved document does not record actors (the model has no actor field; history lives in the engine), so three different credentials still produce the same bytes. (The sha256 changes from one test run to the next, because the document stores the absolute path of each media file, which lives in a fresh temporary folder; within a run it is the same for every surface.) The test asserts that every `doc.patched` carries its surface's actor.

## Format

```json
{
  "description": "What the script covers",
  "steps": [
    { "cmd": "project.new", "args": { "name": "Parity" } },
    { "cmd": "media.import", "args": { "paths": [{ "$media": "a.mp4" }] } },
    { "cmd": "timeline.insert", "args": { "sequence": { "$ref": "steps[0].output.sequence" }, "track": { "$ref": "steps[0].output.video_track" }, "media": { "$ref": "steps[1].output.media_ids[0]" }, "at": 0 } },
    {
      "batch": [
        { "cmd": "clip.split", "args": { "item": { "$script": "steps[2].output.clips[0]" }, "at": "2s" } },
        { "cmd": "item.delete", "args": { "items": [{ "$ref": "steps[0].output.right" }] } }
      ]
    },
    { "cmd": "clip.split", "args": { "item": "itm_0000000000000000000000000000beef", "at": 0 },
      "expect": { "error": "not_found" } }
  ]
}
```

- **Steps** run in order. A step is one call (`cmd` and `args`, default `{}`) or a `batch`: calls that run as **one transaction and one undo step** (`cutweave exec '[...]'`, `cutweave_execute`, or `edit.begin_tx` / `_meta.tx` / `edit.commit_tx` over RPC). A batch that fails is rolled back.
- **`expect`**: `{"error": "<code>", "failed_step"?: N}`. Without it the step must succeed. `failed_step` is the 0-based call of a batch that failed.
- **`"expected_rev": {"$rev": N}`** on a step that changes something sends the call with the document revision that script step `N`'s result reported on the same surface: `cutweave exec --expected-rev`, the tool's `expected_rev` argument (`cutweave_execute`'s for a batch or a command without a tool), JSON-RPC `_meta.expected_rev`. When the document has moved on since step `N` the step fails with `conflict`. The test also requires every successful step to report a revision, the revisions to be equal on every surface, and a `stale_rev` conflict to carry the same `current_rev`, `expected_rev` and `changed_since` everywhere (WP-2I.04; `expected-rev.json`).
- **`{"$ref": "steps[N].output.<path>"}`** (the `cutweave_execute` grammar: `.field` and `[index]` parts) is a value from an earlier step's output:
  - in a top-level step, script step `N` (the test resolves it before the call);
  - inside a `batch`, call `N` of that batch. It is sent as is and resolved by the surface (`cutweave exec`, `cutweave_execute`; the RPC client resolves it from the results it already has).
- **`{"$script": "steps[N].output.<path>"}`** is always script step `N`, also inside a batch. The output of a batch step is the array of its calls' outputs (`steps[3].output[0].left`).
- **`{"$media": "<name>"}`** is the absolute path of a stub media file the test creates under `<root>/media/` (4 KiB; the stub backend probes by extension: `.mp4`/`.mov` are 10 s videos with sound, `.wav` audio, `.png` stills, anything else fails to import).
- The stub backend runs no analysis jobs (`media.import` returns no `job_ids`). With a backend that does, wait for them (`job.wait`) before any step whose result could depend on them.

## Adding a script

Drop a new `*.json` file here; the test picks up every file. Keep each script deterministic: no wall-clock values, no steps whose outcome depends on timing, and no `view.*` commands (UI state, not part of the document). A step that legitimately behaves differently on one surface (a `hidden` command, for example) does not belong in a parity script.
