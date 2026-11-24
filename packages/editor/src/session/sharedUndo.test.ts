// SPDX-License-Identifier: GPL-3.0-or-later
// The shared Undo/Redo view (ADR-0019): the labels it produces and how `SessionSync` reads it
// from `edit.history` and `edit.history_changed`.

import type { LatestStacks, RpcClient, TxId } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { i18n } from "../i18n/index.ts";
import { translator } from "../timeline/errors.ts";
import { actorDisplayName, redoText, undoText } from "./historyLabels.ts";
import { createSessionStore, EMPTY_STACKS } from "./sessionStore.ts";
import { SessionSync, stacksOf } from "./sessionSync.ts";

const t = translator(i18n.t.bind(i18n));
const tx = (id: string) => id as TxId;

const latest = (over: Partial<LatestStacks> = {}): LatestStacks => ({
  undo_depth: 0,
  redo_depth: 0,
  next_undo: null,
  next_redo: null,
  ...over,
});

describe("actor display names", () => {
  it("names who made a change, the person not at all", () => {
    expect(actorDisplayName("user", t)).toBeNull();
    expect(actorDisplayName("agent:claude", t)).toBe("Claude");
    expect(actorDisplayName("agent:Codex", t)).toBe("Codex");
    expect(actorDisplayName("agent:my-bot", t)).toBe("my-bot");
    expect(actorDisplayName("plugin:acme.tools", t)).toBe("acme.tools");
    expect(actorDisplayName("system", t)).toBe("Cutweave");
  });
});

describe("Undo and Redo text", () => {
  it("is plain Undo / Redo with nothing to do", () => {
    expect(undoText(EMPTY_STACKS, t)).toBe("Undo");
    expect(redoText(EMPTY_STACKS, t)).toBe("Redo");
  });

  it("leaves the person's own step unnamed and names another actor", () => {
    const own = stacksOf(
      latest({
        undo_depth: 1,
        next_undo: { tx_id: tx("tx_1"), label: "Move clip", actor: "user" },
      }),
    );
    expect(undoText(own, t)).toBe("Undo Move clip");
    const theirs = stacksOf(
      latest({
        undo_depth: 1,
        redo_depth: 1,
        next_undo: { tx_id: tx("tx_2"), label: "timeline.insert", actor: "agent:claude" },
        next_redo: { tx_id: tx("tx_3"), label: "Trim clip", actor: "plugin:acme.tools" },
      }),
    );
    expect(undoText(theirs, t)).toBe("Undo Insert clip (Claude)");
    expect(redoText(theirs, t)).toBe("Redo Trim clip (acme.tools)");
  });
});

function fakeRpc(page: LatestStacks) {
  const handlers = new Map<string, (payload: unknown) => void>();
  const rpc = {
    on: (name: string, fn: (payload: unknown) => void) => {
      handlers.set(name, fn);
      return () => handlers.delete(name);
    },
    call: async () => ({ latest: page }),
  } as unknown as RpcClient;
  return { rpc, emit: (name: string, payload: unknown) => handlers.get(name)?.(payload) };
}

describe("SessionSync and the shared view", () => {
  it("follows another actor's change without a refresh", () => {
    const store = createSessionStore();
    const { rpc, emit } = fakeRpc(latest());
    new SessionSync({ store, rpc }).start();
    emit("edit.history_changed", {
      change: "recorded",
      tx_id: "tx_9",
      committed_rev: 9,
      // The agent's own stacks; the shared view is what the editor reads.
      stacks: {
        actor: "agent:claude",
        undo_depth: 1,
        redo_depth: 0,
        next_undo: null,
        next_redo: null,
      },
      latest: latest({
        undo_depth: 3,
        next_undo: { tx_id: tx("tx_9"), label: "Insert clip", actor: "agent:claude" },
      }),
    });
    expect(store.getState().stacks).toMatchObject({
      undoDepth: 3,
      nextUndo: "Insert clip",
      nextUndoActor: "agent:claude",
    });
  });

  it("reads the shared view from edit.history", async () => {
    const store = createSessionStore();
    const { rpc } = fakeRpc(
      latest({
        redo_depth: 1,
        next_redo: { tx_id: tx("tx_4"), label: "Move clip", actor: "user" },
      }),
    );
    await new SessionSync({ store, rpc }).refreshStacks();
    expect(store.getState().stacks).toMatchObject({
      redoDepth: 1,
      nextRedo: "Move clip",
      nextRedoActor: "user",
    });
  });
});
