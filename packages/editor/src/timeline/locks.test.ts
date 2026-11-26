// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2A.12: the "in use by <actor>" state (locks.ts) and the refusal toast's wording (errors.ts).
import { MockTransport, RpcClient, RpcError } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { i18n } from "../i18n/index.ts";
import { describeRefusal, heldByOther, lockHolder, translator } from "./errors.ts";
import { createLocksStore, LocksFeed } from "./locks.ts";

const t = translator(i18n.t.bind(i18n));

const activity = (tx: string, actor: string, items: string[]) => ({
  tx_id: tx,
  kind: actor === "user" ? "gesture" : "proposal",
  label: "Edit",
  actor,
  items,
  rev: 1,
});

const ended = (tx: string, actor: string, items: string[]) => ({
  tx_id: tx,
  kind: "proposal",
  actor,
  outcome: "committed",
  items,
  rev: 2,
  committed_rev: 2,
});

describe("locksStore", () => {
  it("lists the items other actors hold, not the person's own", () => {
    const store = createLocksStore();
    store.getState().held(activity("tx_1", "agent:claude", ["itm_a"]) as never);
    store.getState().held(activity("tx_2", "user", ["itm_b"]) as never);
    store.getState().held(activity("tx_1", "agent:claude", ["itm_a", "itm_c"]) as never);
    expect([...store.getState().byItem]).toEqual([
      ["itm_a", "agent:claude"],
      ["itm_c", "agent:claude"],
    ]);
    store.getState().released(ended("tx_1", "agent:claude", ["itm_a", "itm_c"]) as never);
    expect(store.getState().byItem.size).toBe(0);
    expect(store.getState().txs.size).toBe(1);
  });

  it("follows the engine: a baseline from edit.locks, then the tx events", async () => {
    const transport = new MockTransport().handle("edit.locks", () => ({
      transactions: [
        {
          tx: "tx_4",
          kind: "proposal",
          label: "Tidy",
          actor: "plugin:acme.tools",
          mine: false,
          items: ["itm_x"],
          opened_at: 0,
          last_activity: 0,
          expires_at: null,
          conflicted: false,
        },
      ],
      rev: 3,
      committed_rev: 3,
    }));
    const store = createLocksStore();
    const feed = new LocksFeed({ rpc: new RpcClient(transport), store });
    feed.start();
    await feed.refresh();
    expect(store.getState().byItem.get("itm_x")).toBe("plugin:acme.tools");
    transport.emit("tx.opened", activity("tx_5", "agent:cli", []));
    transport.emit("tx.locked", activity("tx_5", "agent:cli", ["itm_y"]));
    expect(store.getState().byItem.get("itm_y")).toBe("agent:cli");
    transport.emit("tx.closed", ended("tx_4", "plugin:acme.tools", ["itm_x"]));
    expect(store.getState().byItem.has("itm_x")).toBe(false);
    // A reset document is read again.
    transport.emit("doc.reset", { rev: 9, prev_rev: 8, committed_rev: 9 });
    await feed.refresh();
    expect(transport.requests.filter((r) => r.method === "edit.locks").length).toBeGreaterThan(1);
    feed.stop();
    expect(store.getState().byItem.size).toBe(0);
    transport.emit("tx.locked", activity("tx_6", "agent:cli", ["itm_z"]));
    expect(store.getState().byItem.size).toBe(0);
  });
});

describe("refusal wording", () => {
  const inUse = (holder?: string) =>
    new RpcError("conflict", "conflict (item_in_gesture): …", undefined, {
      reason: "item_in_gesture",
      retry_after_ms: 28_000,
      ...(holder !== undefined && { holder }),
    });

  it("names who holds the clip, with the machine code", () => {
    expect(describeRefusal(inUse("agent:claude"), t)).toBe(
      "In use by Claude. Try again when they finish. (item_in_gesture)",
    );
    expect(describeRefusal(inUse("plugin:acme.tools"), t)).toBe(
      "In use by acme.tools. Try again when they finish. (item_in_gesture)",
    );
    expect(lockHolder(inUse("agent:cli"))).toBe("agent:cli");
    expect(heldByOther(inUse("agent:cli"))).toBe(true);
  });

  it("falls back when the holder is the person or unknown", () => {
    for (const error of [inUse("user"), inUse()]) {
      expect(describeRefusal(error, t)).toBe(
        "Someone else is editing this clip right now. Try again in a moment. (item_in_gesture)",
      );
      expect(heldByOther(error)).toBe(false);
    }
    const stale = new RpcError("conflict", "stale", undefined, { reason: "stale_rev" });
    expect(lockHolder(stale)).toBeNull();
    expect(describeRefusal(stale, t)).toBe(
      "The project changed while you were editing. Try again.",
    );
  });
});
