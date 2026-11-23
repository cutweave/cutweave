// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { MockTransport, RpcClient, RpcError } from "@cutweave/protocol";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { docStore } from "../state/docStore.ts";
import { engineStore } from "./engineStore.ts";
import { runCommand, useCommand, useQuery } from "./hooks.ts";

const item = "itm_00000000000000000000000000000001" as never;

function attach(transport: MockTransport) {
  engineStore.setState({ rpc: new RpcClient(transport), connection: "ready" });
}

beforeEach(() => {
  engineStore.setState({ rpc: null, connection: "none", blocked: null });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
});
afterEach(cleanup);

describe("useCommand", () => {
  it("is typed from the schema: a missing input field fails tsc", () => {
    const check = () => {
      const split = useCommand("clip.split");
      void split.run({ item, at: 1 as never });
      // @ts-expect-error `at` is required
      void split.run({ item });
      // @ts-expect-error not a command
      useCommand("nope");
      // @ts-expect-error a query is not a command
      useCommand("timeline.get");
    };
    expect(typeof check).toBe("function");
  });

  it("runs the command, then reports success and the result", async () => {
    const transport = new MockTransport().handle("clip.split", () => ({ left: "l", right: "r" }));
    attach(transport);
    const { result } = renderHook(() => useCommand("clip.split"));
    expect(result.current.status).toBe("idle");
    let out: unknown;
    await act(async () => {
      out = await result.current.run({ item, at: 5 as never });
    });
    expect(out).toEqual({ left: "l", right: "r" });
    expect(result.current.status).toBe("success");
    expect(result.current.result).toEqual({ left: "l", right: "r" });
    expect(transport.requests[0]).toEqual({
      method: "clip.split",
      params: { item, at: 5 },
    });
  });

  it("rejects with the engine's RpcError and exposes it", async () => {
    attach(
      new MockTransport().handle("clip.split", () => {
        throw new RpcError("locked", "the track is locked");
      }),
    );
    const { result } = renderHook(() => useCommand("clip.split"));
    await act(async () => {
      await expect(result.current.run({ item, at: 1 as never })).rejects.toMatchObject({
        code: "locked",
      });
    });
    expect(result.current.status).toBe("error");
    expect(result.current.error?.code).toBe("locked");
    act(() => result.current.reset());
    expect(result.current.status).toBe("idle");
  });

  it("rejects clearly when no engine is connected", async () => {
    await expect(runCommand("edit.undo", {})).rejects.toMatchObject({
      code: "unsupported",
      message: "no engine is connected",
    });
  });

  it("refuses edits while the engine failed or speaks another schema (WP-1L.03)", async () => {
    const transport = new MockTransport().handle("edit.undo", () => ({}));
    attach(transport);
    engineStore.setState({ blocked: { kind: "engine-failed", message: null } });
    await expect(runCommand("edit.undo", {})).rejects.toMatchObject({
      code: "unsupported",
      message: "the engine stopped; editing resumes when it restarts",
    });
    engineStore.setState({ blocked: { kind: "schema-mismatch", engineVersion: "9" } });
    await expect(runCommand("edit.undo", {})).rejects.toMatchObject({ code: "unsupported" });
    expect(transport.requests).toEqual([]);
    engineStore.setState({ blocked: null });
    await runCommand("edit.undo", {});
    expect(transport.requests).toHaveLength(1);
  });

  it("passes an explicit request_id or expected_rev along", async () => {
    const transport = new MockTransport().handle("edit.undo", () => ({}));
    attach(transport);
    await runCommand("edit.undo", {}, { request_id: "abc", expected_rev: 4 });
    expect(transport.requests[0]?.meta).toEqual({ request_id: "abc", expected_rev: 4 });
  });
});

describe("useQuery", () => {
  const seqId = "seq_00000000000000000000000000000001" as never;

  it("fetches once the engine is connected and again after each document revision", async () => {
    let calls = 0;
    const transport = new MockTransport().handle("timeline.get", () => ({
      id: seqId,
      track_count: 1,
      item_count: ++calls,
    }));
    const { result } = renderHook(() => useQuery("timeline.get", { sequence: seqId }));
    expect(result.current.status).toBe("idle");
    await act(async () => {
      attach(transport);
    });
    await waitFor(() => expect(result.current.data?.item_count).toBe(1));
    expect(result.current.status).toBe("success");

    await act(async () => {
      docStore.setState({ rev: 5 });
    });
    await waitFor(() => expect(result.current.data?.item_count).toBe(2));

    // An equal input in a new object does not refetch; refetch() does.
    act(() => result.current.refetch());
    await waitFor(() => expect(result.current.data?.item_count).toBe(3));
    expect(calls).toBe(3);
  });

  it("reports an error and can be paused", async () => {
    attach(
      new MockTransport().handle("timeline.get", () => {
        throw new RpcError("not_found", "no such sequence");
      }),
    );
    const { result, rerender } = renderHook(
      ({ enabled }) => useQuery("timeline.get", { sequence: seqId }, { enabled }),
      { initialProps: { enabled: false } },
    );
    expect(result.current.status).toBe("idle");
    rerender({ enabled: true });
    await waitFor(() => expect(result.current.status).toBe("error"));
    expect(result.current.error?.code).toBe("not_found");
  });
});
