// SPDX-License-Identifier: GPL-3.0-or-later
// Integration against a REAL headless engine process (`cutweave-engine-serve`, see
// `test-support/engine.ts`). Skips with a message when the binary cannot be built here; set
// `CUTWEAVE_ENGINE_BIN` to use a prebuilt one.
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { RpcConnection } from "./connection.ts";
import { DocMirror } from "./doc-mirror.ts";
import { DocSync } from "./doc-sync.ts";
import type { DocPatched } from "./generated/types.ts";
import { type Json, jsonEqual } from "./patch.ts";
import { RpcClient, RpcError } from "./rpc.ts";
import { type EngineHandle, resolveEngineBinary, startEngine } from "./test-support/engine.ts";
import { mulberry32, randomEdits, seedProject } from "./test-support/scenario.ts";

let binary: string | undefined;
let skipReason: string | undefined;
let engine: EngineHandle | undefined;
let connection: RpcConnection;
let rpc: RpcClient;
const patches: DocPatched[] = [];

beforeAll(async () => {
  const resolved = resolveEngineBinary();
  if (resolved.unavailable !== undefined) {
    skipReason = resolved.unavailable;
    console.warn(`engine integration tests skipped: ${skipReason}`);
    return;
  }
  binary = resolved.path;
  engine = await startEngine(binary);
  connection = new RpcConnection({ resolve: () => engine?.info ?? null });
  rpc = new RpcClient(connection);
  rpc.on("doc.patched", (event) => patches.push(event));
  connection.connect();
  await connection.whenReady();
}, 20 * 60_000);

afterAll(async () => {
  connection?.close();
  await engine?.stop();
});

const live = (name: string, body: () => Promise<void>) =>
  it(name, async (ctx) => {
    if (skipReason !== undefined) {
      ctx.skip();
      return;
    }
    await body();
  }, 120_000);

/** A private engine for a test that needs an empty document (`project.new` only works once). */
async function withEngine(
  body: (own: { engine: EngineHandle; rpc: RpcClient }) => Promise<void>,
): Promise<void> {
  if (binary === undefined) {
    throw new Error("no engine binary");
  }
  const handle = await startEngine(binary);
  const own = new RpcConnection({ resolve: () => handle.info });
  try {
    own.connect();
    await own.whenReady();
    await body({ engine: handle, rpc: new RpcClient(own) });
  } finally {
    own.close();
    await handle.stop();
  }
}

/** Events trail responses: waits until the mirror reaches the engine's live revision, then compares. */
async function converge(mirror: DocMirror, client: RpcClient): Promise<void> {
  const deadline = Date.now() + 10_000;
  let snapshot = await client.snapshot();
  while (mirror.state.rev < snapshot.rev && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 20));
    snapshot = await client.snapshot();
  }
  expect(mirror.state.rev).toBe(snapshot.rev);
  expect(mirror.state.committedRev).toBe(snapshot.committed_rev);
  const same = jsonEqual(
    mirror.state.project as unknown as Json,
    snapshot.project as unknown as Json,
  );
  expect(same).toBe(true);
}

describe("RpcConnection against the real engine", () => {
  live("completes the handshake as the UI actor", async () => {
    expect(connection.hello?.actor).toBe("user");
    expect(connection.hello?.schema_hash).toBeTruthy();
  });

  live("maps engine errors to RpcError", async () => {
    const error = await rpc
      .call("clip.split", {
        item: "itm_00000000000000000000000000000001" as never,
        at: 1 as never,
      })
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(RpcError);
    expect((error as RpcError).code).toBe("not_found");
  });

  live("the mirror after 200 random events equals the engine snapshot", async () => {
    const mirror = new DocMirror();
    const sync = new DocSync(rpc, mirror, {
      onError: (error) => {
        throw error;
      },
    });
    const seed = await seedProject(rpc);
    await sync.start();
    const before = patches.length;

    await randomEdits(rpc, seed, mulberry32(42), () => patches.length - before >= 200);
    expect(patches.length - before).toBeGreaterThanOrEqual(200);

    await converge(mirror, rpc);
    sync.stop();
  });

  live("a repeated request_id returns the first result and changes nothing", async () => {
    await withEngine(async ({ rpc: client }) => {
      const seed = await seedProject(client, "Dedup");
      const input = {
        sequence: seed.sequence,
        track: seed.videoTrack,
        media: seed.media,
        at: "0s",
        duration: "2s",
      };
      const id = { request_id: "0190f5c2-a1b2-7c3d-8e4f-5a6b7c8d9e0f" };
      const first = await client.call("timeline.insert", input, id);
      const again = await client.call("timeline.insert", input, id);
      expect(again).toEqual(first);
      const { project } = await client.snapshot();
      const track = project.sequences[seed.sequence]?.tracks[seed.videoTrack];
      expect(Object.keys(track?.items ?? {})).toHaveLength(1);
    });
  });

  live("a second connection follows another client's edits by patches alone", async () => {
    await withEngine(async ({ engine: handle, rpc: client }) => {
      const second = new RpcConnection({ resolve: () => handle.info });
      const other = new RpcClient(second);
      const mirror = new DocMirror();
      const sync = new DocSync(other, mirror);
      second.connect();
      await second.whenReady();
      await sync.start(); // an empty document
      const seed = await seedProject(client, "Second");
      await client.call("timeline.insert", {
        sequence: seed.sequence,
        track: seed.videoTrack,
        media: seed.media,
        at: "0s",
        duration: "1s",
      });
      await converge(mirror, other);
      // `project.new` on an empty document is an ordinary patch: no resync was needed.
      expect(sync.snapshotCount).toBe(1);
      sync.stop();
      second.close();
    });
  });
});
