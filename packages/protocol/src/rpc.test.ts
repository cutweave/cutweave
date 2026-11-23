// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, expectTypeOf, it } from "vitest";
import type { CmdErrorCode, CommandMap, EventMap, ItemId, QueryMap, Time } from "./index.ts";
import { MockTransport, RpcClient, RpcError, SCHEMA_HASH } from "./index.ts";

const item = "itm_00000000000000000000000000000001" as ItemId;
const at = 4_233_600_000 as Time;

describe("RpcClient types", () => {
  const client = new RpcClient(new MockTransport());

  it("types call() from the generated maps", () => {
    // Type-level checks: `tsc` (typecheck / build) enforces the @ts-expect-error lines.
    const typed = () => {
      void client.call("clip.split", { item, at });
      // @ts-expect-error `at` is required
      void client.call("clip.split", { item });
      // @ts-expect-error unknown command
      void client.call("nope", {});
      // @ts-expect-error a plain string is not an ItemId
      void client.call("clip.split", { item: "itm_1", at });
    };
    expect(typeof typed).toBe("function");
    expectTypeOf<CommandMap["clip.split"]["input"]["item"]>().toEqualTypeOf<ItemId>();
  });

  it("accepts queries and commands alike", () => {
    expectTypeOf<keyof QueryMap>().not.toBeNever();
    expectTypeOf<keyof CommandMap>().not.toBeNever();
  });

  it("types events and error codes", () => {
    expectTypeOf<EventMap["doc.patched"]["rev"]>().toEqualTypeOf<number>();
    expectTypeOf<"not_found">().toExtend<CmdErrorCode>();
    const typed = () => {
      // @ts-expect-error not an event
      client.on("nope", () => {});
    };
    expect(typeof typed).toBe("function");
  });
});

describe("RpcClient over MockTransport", () => {
  it("forwards name and input and resolves the output", async () => {
    const transport = new MockTransport().handle("project.rename", () => ({}));
    const client = new RpcClient(transport);
    await client.call("project.rename", { name: "x" });
    expect(transport.requests).toEqual([{ method: "project.rename", params: { name: "x" } }]);
  });

  it("rejects with RpcError", async () => {
    const transport = new MockTransport().handle("clip.split", () => {
      throw new RpcError("locked", "track is locked");
    });
    const client = new RpcClient(transport);
    await expect(client.call("clip.split", { item, at })).rejects.toMatchObject({
      code: "locked",
    });
  });

  it("delivers events until unsubscribed", () => {
    const transport = new MockTransport();
    const client = new RpcClient(transport);
    const seen: number[] = [];
    const off = client.on("doc.reset", (p) => seen.push(p.rev));
    transport.emit("doc.reset", { rev: 2, prev_rev: 1, committed_rev: 2 });
    off();
    transport.emit("doc.reset", { rev: 3, prev_rev: 2, committed_rev: 3 });
    expect(seen).toEqual([2]);
  });

  it("exposes the schema hash", () => {
    expect(SCHEMA_HASH).toMatch(/^[0-9a-f]{64}$/);
  });
});
