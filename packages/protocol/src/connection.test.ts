// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import { type ConnectionHooks, RpcConnection, type RpcConnectionOptions } from "./connection.ts";
import type { ConnectionState } from "./connection-state.ts";
import { RpcClient, RpcError } from "./rpc.ts";
import { FakeEngine, flushPromises, HELLO, ManualTimers } from "./test-support/fake-socket.ts";

const INFO = { wsUrl: "ws://127.0.0.1:4321/rpc", token: "secret-token" };

function setup(options: Partial<RpcConnectionOptions> = {}) {
  const engine = new FakeEngine();
  const timers = new ManualTimers();
  let counter = 0;
  const states: ConnectionState[] = [];
  const connection = new RpcConnection({
    resolve: () => INFO,
    createSocket: engine.factory,
    timers,
    newRequestId: () => `req-${++counter}`,
    ...options,
  });
  connection.onStateChange((s) => states.push(s));
  return { engine, timers, connection, states, client: new RpcClient(connection) };
}

async function connected(options: Partial<RpcConnectionOptions> = {}) {
  const s = setup(options);
  s.connection.connect();
  await s.connection.whenReady();
  return s;
}

const item = "itm_00000000000000000000000000000001" as never;

describe("RpcConnection handshake", () => {
  it("sends the token as a subprotocol only, then rpc.hello", async () => {
    const { engine, connection, states } = await connected();
    expect(engine.last.url).toBe(INFO.wsUrl);
    expect(engine.last.url).not.toContain(INFO.token);
    expect(engine.last.protocols).toEqual(["cutweave.token.secret-token"]);
    expect(engine.last.sent[0]).toMatchObject({ method: "rpc.hello" });
    expect(states).toEqual(["connecting", "handshaking", "ready"]);
    expect(connection.hello?.actor).toBe("user");
  });

  it("reports a schema hash mismatch and stays connected", async () => {
    const onSchemaMismatch = vi.fn();
    const engine = new FakeEngine();
    engine.hello = { ...HELLO, schema_hash: "other" };
    const connection = new RpcConnection({
      resolve: () => INFO,
      createSocket: engine.factory,
      hooks: { onSchemaMismatch },
    });
    connection.connect();
    await connection.whenReady();
    expect(onSchemaMismatch).toHaveBeenCalledTimes(1);
    expect(connection.state).toBe("ready");
  });

  it("queues calls made before the handshake and sends them after it", async () => {
    const { engine, connection, client } = setup();
    engine.handler = (_s, m) => (m.method === "timeline.get" ? { id: "seq" } : undefined);
    const pending = client.call("timeline.get", { sequence: "seq" as never });
    connection.connect();
    await expect(pending).resolves.toEqual({ id: "seq" });
    expect(engine.last.methods()).toEqual(["rpc.hello", "timeline.get"]);
  });
});

describe("RpcConnection calls", () => {
  it("adds a request_id to mutating calls only", async () => {
    const { engine, client } = await connected();
    engine.handler = () => ({});
    await client.call("clip.split", { item, at: 4 as never });
    await client.call("timeline.get", { sequence: "s" as never });
    await client.call("view.set", { patch: {} });
    const [split, get, view] = engine.last.sent.slice(-3);
    expect(split?.params._meta).toEqual({ request_id: "req-1" });
    expect(get?.params._meta).toBeUndefined();
    expect(view?.params._meta).toBeUndefined();
  });

  it("keeps a caller's request_id and expected_rev", async () => {
    const { engine, client } = await connected();
    engine.handler = () => ({});
    await client.call(
      "clip.split",
      { item, at: 1 as never },
      { request_id: "mine", expected_rev: 3 },
    );
    expect(engine.last.sent.at(-1)?.params._meta).toEqual({ request_id: "mine", expected_rev: 3 });
  });

  it("strips result._meta and exposes it through requestWithMeta", async () => {
    const { engine, connection } = await connected();
    engine.handler = () => ({ left: "a", _meta: { rev: 7, committed_rev: 7 } });
    const out = await connection.requestWithMeta("clip.split", { item });
    expect(out.result).toEqual({ left: "a" });
    expect(out.meta).toMatchObject({ rev: 7 });
  });

  it("matches responses by id even when a read overtakes a write", async () => {
    const { engine, client } = await connected();
    const held: { id: number; method: string }[] = [];
    engine.handler = (_s, m) => {
      held.push({ id: m.id as number, method: m.method });
      return undefined;
    };
    const write = client.call("clip.split", { item, at: 1 as never });
    const read = client.call("timeline.get", { sequence: "s" as never });
    const [w, r] = held;
    engine.last.reply(r?.id as number, { which: "read" });
    engine.last.reply(w?.id as number, { which: "write" });
    await expect(read).resolves.toEqual({ which: "read" });
    await expect(write).resolves.toEqual({ which: "write" });
  });

  it("rejects with RpcError carrying code, path and data", async () => {
    const { engine, client } = await connected();
    engine.handler = (socket, m) => {
      socket.replyError(
        m.id as number,
        -32602,
        { code: "invalid", path: "/at", msg: "bad" },
        "bad at",
      );
      return undefined;
    };
    const error = await client
      .call("clip.split", { item, at: 0 as never })
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(RpcError);
    expect(error).toMatchObject({ code: "invalid", path: "/at", message: "bad at" });
    expect((error as RpcError).data).toMatchObject({ msg: "bad" });
  });

  it("routes events to subscribers and survives a throwing handler", async () => {
    const onError = vi.fn();
    const { engine, client } = await connected({ hooks: { onError } });
    const seen: number[] = [];
    client.on("doc.patched", () => {
      throw new Error("boom");
    });
    client.on("doc.patched", (e) => seen.push(e.rev));
    await client.subscribed();
    engine.last.emit("doc.patched", { rev: 5 });
    expect(seen).toEqual([5]);
    expect(onError).toHaveBeenCalledTimes(1);
  });

  it("subscribes remotely once per event name and unsubscribes with the last handler", async () => {
    const { engine, client } = await connected();
    const off1 = client.on("doc.patched", () => {});
    const off2 = client.on("doc.patched", () => {});
    await client.subscribed();
    const subscribes = () => engine.last.sent.filter((m) => m.method === "rpc.subscribe");
    expect(subscribes()).toHaveLength(1);
    expect(subscribes()[0]?.params).toEqual({ events: ["doc.patched"] });
    off1();
    await client.subscribed();
    expect(engine.last.sent.some((m) => m.method === "rpc.unsubscribe")).toBe(false);
    off2();
    await client.subscribed();
    expect(engine.last.sent.at(-1)).toMatchObject({
      method: "rpc.unsubscribe",
      params: { events: ["doc.patched"] },
    });
  });
});

describe("RpcConnection reconnect", () => {
  function lose(engine: FakeEngine, code = 1006) {
    engine.last.serverClose(code, "");
  }

  it("goes reconnecting, retries after the backoff, and restores subscriptions", async () => {
    const hooks: ConnectionHooks = { onDisconnected: vi.fn(), onReconnected: vi.fn() };
    const { engine, timers, connection, client } = await connected({ hooks });
    client.on("doc.patched", () => {});
    await client.subscribed();
    lose(engine);
    expect(connection.state).toBe("reconnecting");
    expect(hooks.onDisconnected).toHaveBeenCalledWith(
      expect.objectContaining({ code: 1006, willRetry: true, unacked: [] }),
    );
    expect(engine.sockets).toHaveLength(1);
    timers.advance(250);
    await connection.whenReady();
    expect(engine.sockets).toHaveLength(2);
    // The new socket subscribed again during the handshake, before `ready`.
    expect(engine.last.methods()).toEqual(["rpc.hello", "rpc.subscribe"]);
    expect(engine.last.sent[1]?.params).toEqual({ events: ["doc.patched"] });
    await flushPromises();
    expect(hooks.onReconnected).toHaveBeenCalledTimes(1);
  });

  it("retries at once after 4001 resync_required", async () => {
    const { engine, timers, connection } = await connected();
    lose(engine, 4001);
    expect(timers.nextDelay).toBe(0);
    timers.advance(0);
    await connection.whenReady();
    expect(engine.sockets).toHaveLength(2);
  });

  it("gives up after the policy says so and rejects what is queued", async () => {
    const { engine, timers, connection, client } = setup({
      reconnect: { nextDelayMs: (attempt) => (attempt < 1 ? 10 : null) },
    });
    engine.autoOpen = false;
    const queued = client.call("timeline.get", { sequence: "s" as never });
    connection.connect();
    await flushPromises();
    engine.last.serverClose(1006);
    timers.advance(10);
    await flushPromises();
    engine.last.serverClose(1006);
    expect(connection.state).toBe("failed");
    await expect(queued).rejects.toBeInstanceOf(RpcError);
    await expect(client.call("timeline.get", { sequence: "s" as never })).rejects.toMatchObject({
      code: "unsupported",
    });
  });

  it("treats an unavailable engine address as a failed attempt", async () => {
    let available = false;
    const { engine, timers, connection } = setup({
      resolve: () => (available ? INFO : null),
    });
    connection.connect();
    await flushPromises();
    expect(connection.state).toBe("reconnecting");
    expect(engine.sockets).toHaveLength(0);
    available = true;
    timers.advance(250);
    await connection.whenReady();
    expect(engine.sockets).toHaveLength(1);
  });

  it("resolves the address again on every attempt (a restarted engine has a new port)", async () => {
    let port = 1000;
    const { engine, timers, connection } = setup({
      resolve: () => ({ wsUrl: `ws://127.0.0.1:${port++}/rpc`, token: "t" }),
    });
    connection.connect();
    await connection.whenReady();
    lose(engine);
    timers.advance(250);
    await connection.whenReady();
    expect(engine.sockets.map((s) => s.url)).toEqual([
      "ws://127.0.0.1:1000/rpc",
      "ws://127.0.0.1:1001/rpc",
    ]);
  });

  it("reconnectNow skips the backoff and drops a live socket", async () => {
    const { engine, timers, connection } = await connected();
    connection.reconnectNow();
    expect(connection.state).toBe("reconnecting");
    expect(engine.sockets[0]?.closedByClient).not.toBeNull();
    timers.advance(0);
    await connection.whenReady();
    expect(engine.sockets).toHaveLength(2);
  });

  it("by default rejects calls that were waiting for an answer, reads and writes alike", async () => {
    const { engine, connection, client } = await connected();
    engine.handler = () => undefined;
    const write = client.call("clip.split", { item, at: 1 as never });
    const read = client.call("timeline.get", { sequence: "s" as never });
    lose(engine);
    await expect(write).rejects.toMatchObject({ code: "unsupported" });
    await expect(read).rejects.toMatchObject({ code: "unsupported" });
    expect(connection.outbox.size).toBe(0);
    // Calls while reconnecting fail fast too.
    await expect(client.call("clip.split", { item, at: 1 as never })).rejects.toMatchObject({
      code: "unsupported",
    });
  });

  describe("with retainUnacked (the hook point of WP-1L.03)", () => {
    it("keeps un-acked commands by request_id and sends them again, unchanged, after the handshake", async () => {
      const onDisconnected = vi.fn();
      const { engine, timers, connection, client } = await connected({
        retainUnacked: true,
        hooks: { onDisconnected },
      });
      engine.handler = () => undefined;
      const write = client.call("clip.split", { item, at: 1 as never });
      const read = client.call("timeline.get", { sequence: "s" as never });
      const firstSent = engine.last.sent.find((m) => m.method === "clip.split");
      lose(engine);
      // Reads are not retried; the command is offered to the hook with its request_id.
      await expect(read).rejects.toMatchObject({ code: "unsupported" });
      expect(onDisconnected.mock.calls[0]?.[0].unacked).toEqual([
        expect.objectContaining({ requestId: "req-1", method: "clip.split", sent: false }),
      ]);
      // A command fired during the outage waits in the outbox as well.
      const during = client.call("clip.split", { item, at: 2 as never });
      expect(connection.outbox.size).toBe(2);

      engine.handler = (_s, m) => (m.method === "clip.split" ? { ok: true } : undefined);
      timers.advance(250);
      await connection.whenReady();
      const resent = engine.last.sent.filter((m) => m.method === "clip.split");
      expect(resent).toHaveLength(2);
      expect(resent[0]?.params).toEqual(firstSent?.params);
      expect(resent.map((m) => (m.params._meta as { request_id: string }).request_id)).toEqual([
        "req-1",
        "req-2",
      ]);
      await expect(write).resolves.toEqual({ ok: true });
      await expect(during).resolves.toEqual({ ok: true });
      expect(connection.outbox.size).toBe(0);
    });

    it("autoFlush: false leaves the sending to the onReconnected hook", async () => {
      let flushed = false;
      const { engine, timers, connection, client } = await connected({
        retainUnacked: true,
        autoFlush: false,
        hooks: {
          onReconnected: (_hello, { connection: c }) => {
            c.flushOutbox();
            flushed = true;
          },
        },
      });
      engine.handler = () => undefined;
      void client.call("clip.split", { item, at: 1 as never }).catch(() => {});
      lose(engine);
      engine.handler = () => ({});
      timers.advance(250);
      await connection.whenReady();
      await flushPromises();
      expect(flushed).toBe(true);
      expect(engine.last.sent.filter((m) => m.method === "clip.split")).toHaveLength(1);
    });
  });

  it("close() rejects pending calls and stops retrying", async () => {
    const { engine, timers, connection, client } = await connected();
    engine.handler = () => undefined;
    const pending = client.call("clip.split", { item, at: 1 as never });
    connection.close();
    await expect(pending).rejects.toBeInstanceOf(RpcError);
    expect(connection.state).toBe("closed");
    expect(engine.last.closedByClient).toMatchObject({ code: 1000 });
    timers.advance(60_000);
    expect(engine.sockets).toHaveLength(1);
  });

  it("a hook that throws does not cost the connection", async () => {
    const onError = vi.fn();
    const { connection } = await connected({
      hooks: {
        onError,
        onConnected: () => {
          throw new Error("hook");
        },
      },
    });
    await flushPromises();
    expect(connection.state).toBe("ready");
    expect(onError).toHaveBeenCalledTimes(1);
  });
});
