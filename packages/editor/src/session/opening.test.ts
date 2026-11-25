// SPDX-License-Identifier: GPL-3.0-or-later
// Opening a project (ADR-0029, issue #161): `SessionSync` holds `opening` from `project.opening`
// until `project.opened` came and the new document is in the mirror, or until
// `project.open_failed`; an agent's open, refusal or failure becomes a notice for a toast.
import type { ProjectSnapshot, ProjectStatus, RpcClient } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { i18n } from "../i18n/index.ts";
import { createDocStore } from "../state/docStore.ts";
import { openNoticeToast } from "./openNotices.ts";
import { createSessionStore, projectNameOfPath } from "./sessionStore.ts";
import { type OpenNotice, SessionSync } from "./sessionSync.ts";

function fakeRpc() {
  const handlers = new Map<string, Set<(payload: unknown) => void>>();
  const rpc = {
    on: (name: string, fn: (payload: unknown) => void) => {
      const set = handlers.get(name) ?? new Set();
      set.add(fn);
      handlers.set(name, set);
      return () => set.delete(fn);
    },
    call: async () => ({ latest: undefined }),
  } as unknown as RpcClient;
  const emit = (name: string, payload: unknown): void => {
    for (const fn of handlers.get(name) ?? []) fn(payload);
  };
  return { rpc, emit };
}

function status(name: string, path: string | null): ProjectStatus {
  return {
    state: path === null ? "no_project" : "open",
    path,
    name,
    rev: 3,
    saved_rev: path === null ? null : 3,
    dirty: false,
    read_only: null,
    recovery: null,
  } as ProjectStatus;
}

function snapshot(name: string): ProjectSnapshot {
  return {
    rev: 3,
    committed_rev: 3,
    project: {
      version: 2,
      meta: { name },
      settings: {},
      media: {},
      folders: {},
      sequences: {},
    },
  } as unknown as ProjectSnapshot;
}

function setup() {
  const { rpc, emit } = fakeRpc();
  const session = createSessionStore();
  const doc = createDocStore();
  doc.getState().resync(snapshot("Mine"));
  const notices: OpenNotice[] = [];
  const sync = new SessionSync({
    store: session,
    rpc,
    doc,
    onOpenNotice: (n) => notices.push(n),
  });
  sync.start();
  return { emit, session, doc, notices, sync };
}

const opening = (actor: string) => ({ path: "/work/Second", actor });

describe("SessionSync: a project opening (ADR-0029)", () => {
  it("shows the open until project.opened came and the new document is in the mirror", () => {
    const { emit, session, doc, notices } = setup();
    emit("project.opening", opening("agent:cli"));
    expect(session.getState().opening).toEqual(opening("agent:cli"));
    emit("doc.reset", { rev: 3, prev_rev: 9, committed_rev: 3 });
    emit("project.opened", status("Second", "/work/Second/Second.cutweave"));
    // The engine is done, but the editor still shows the old document: keep waiting.
    expect(session.getState().opening).not.toBeNull();
    expect(notices).toEqual([]);
    doc.getState().resync(snapshot("Second"));
    expect(session.getState().opening).toBeNull();
    expect(notices).toEqual([
      { kind: "opened", actor: "agent:cli", path: "/work/Second", name: "Second" },
    ]);
  });

  it("also ends when the new document arrives before project.opened", () => {
    const { emit, session, doc } = setup();
    emit("project.opening", opening("agent:cli"));
    // A mirror change before the reset is the old document's, not the new one.
    doc.getState().resync(snapshot("Mine edited"));
    emit("doc.reset", { rev: 3, prev_rev: 9, committed_rev: 3 });
    doc.getState().resync(snapshot("Second"));
    expect(session.getState().opening).not.toBeNull();
    emit("project.opened", status("Second", "/work/Second/Second.cutweave"));
    expect(session.getState().opening).toBeNull();
  });

  it("says nothing about the app's own open (the controller does)", () => {
    const { emit, session, doc, notices } = setup();
    emit("project.opening", opening("user"));
    expect(session.getState().opening?.actor).toBe("user");
    emit("doc.reset", { rev: 3, prev_rev: 9, committed_rev: 3 });
    emit("project.opened", status("Second", "/work/Second/Second.cutweave"));
    doc.getState().resync(snapshot("Second"));
    emit("project.open_failed", {
      path: "/x",
      actor: "user",
      code: "conflict",
      reason: "unsaved_changes",
      message: "no",
    });
    expect(session.getState().opening).toBeNull();
    expect(notices).toEqual([]);
  });

  it("tells the person an agent's open was refused for their unsaved changes", () => {
    const { emit, session, notices } = setup();
    emit("project.open_failed", {
      path: "/work/Second",
      actor: "agent:cli",
      code: "conflict",
      reason: "unsaved_changes",
      message: '"Mine" has unsaved changes',
    });
    expect(session.getState().opening).toBeNull();
    expect(notices).toEqual([
      {
        kind: "refused",
        actor: "agent:cli",
        path: "/work/Second",
        current: "Mine",
        message: '"Mine" has unsaved changes',
      },
    ]);
  });

  it("ends a failed open and says so", () => {
    const { emit, session, notices } = setup();
    emit("project.opening", opening("agent:claude"));
    emit("project.open_failed", {
      path: "/work/Second",
      actor: "agent:claude",
      code: "invalid",
      reason: null,
      message: "no project there",
    });
    expect(session.getState().opening).toBeNull();
    expect(notices[0]?.kind).toBe("failed");
  });

  it("a new handshake ends an open that was under way", () => {
    const { emit, session, sync } = setup();
    emit("project.opening", opening("agent:cli"));
    sync.applyHello({ project: status("Mine", null) });
    expect(session.getState().opening).toBeNull();
  });
});

describe("open notices", () => {
  const t = (key: string, vars?: Record<string, unknown>) =>
    i18n.t(key as never, vars as never) as unknown as string;

  it("names the agent and the project, in agent colour", () => {
    const opened = openNoticeToast(
      { kind: "opened", actor: "agent:cli", path: "/work/Second", name: "Second" },
      t,
    );
    expect(opened).toMatchObject({ tone: "agent", title: "Project opened by an agent" });
    expect(opened.message).toBe("Command line (agent:cli) opened Second.");
    expect(opened.duration).toBeUndefined();
  });

  it("a refusal says whose changes and what to do, and waits for the person", () => {
    const refused = openNoticeToast(
      {
        kind: "refused",
        actor: "agent:cli",
        path: "/work/Second/Second.cutweave",
        current: "",
        message: "",
      },
      t,
    );
    expect(refused.tone).toBe("agent");
    expect(refused.title).toBe("Project not opened");
    expect(refused.message).toBe(
      "Command line (agent:cli) tried to open Second, but Untitled project has unsaved changes. Save them, then ask the agent again.",
    );
    expect(refused.duration).toBe(Number.POSITIVE_INFINITY);
  });

  it("names a project by its folder or its document", () => {
    expect(projectNameOfPath("/work/Film")).toBe("Film");
    expect(projectNameOfPath("C:\\work\\Film\\Film.cutweave")).toBe("Film");
    expect(projectNameOfPath("/work/Film/")).toBe("Film");
  });
});
