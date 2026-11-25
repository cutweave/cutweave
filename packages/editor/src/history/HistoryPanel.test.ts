// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The History panel (WP-2H.11): rows from a mixed history, the marker following
// `edit.history_changed`, jumps as `*` steps, a refused step's message, the filter, the keys, the
// empty state and axe.
import type { HistoryEntry } from "@cutweave/protocol";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { entry, mixedHistory } from "../testing/historyFixtures.ts";
import { HistoryPanel } from "./HistoryPanel.tsx";
import { historyStore, initialHistoryState } from "./historyStore.ts";

let entries: HistoryEntry[];
let handlers: Map<string, () => void>;
let call: ReturnType<typeof vi.fn>;

function connect(): void {
  handlers = new Map();
  call = vi.fn(async (name: string, input: Record<string, unknown>) => {
    if (name === "edit.history") return { entries, next_cursor: null };
    if (name === "edit.undo" || name === "edit.redo") return { done: input.steps ?? 1 };
    return {};
  });
  const rpc = {
    call,
    on: (event: string, fn: () => void) => {
      handlers.set(event, fn);
      return () => handlers.delete(event);
    },
  };
  engineStore.setState({ rpc: rpc as never, connection: "ready" });
}

/** The engine changed its history: say so, as `edit.history_changed` does. */
async function changeHistory(next: HistoryEntry[]): Promise<void> {
  entries = next;
  await act(async () => {
    handlers.get("edit.history_changed")?.();
  });
}

/** The calls of one command, as [name, input]. */
const calls = (command: string) =>
  call.mock.calls.filter(([name]) => name === command).map(([name, input]) => [name, input]);

const view = () => render(h(I18nextProvider, { i18n }, h(HistoryPanel)));
const rowOf = (name: RegExp) => screen.getByRole("button", { name });

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  historyStore.setState({ ...initialHistoryState });
  entries = mixedHistory();
  connect();
});

afterEach(() => {
  cleanup();
  engineStore.setState({ rpc: null, connection: "none" });
});

describe("History panel", () => {
  it("lists the changes oldest first, who made each, with the marker under the current state", async () => {
    view();
    await screen.findByRole("list", { name: "Project history" });
    const items = within(screen.getByRole("list", { name: "Project history" })).getAllByRole(
      "listitem",
    );
    // Seven changes and the marker; the marker comes after the fifth.
    expect(items).toHaveLength(8);
    expect(items[5]?.getAttribute("aria-current")).toBe("step");
    expect(items[5]?.textContent).toContain("Current state");
    const names = screen
      .getAllByRole("button")
      .map((b) => b.getAttribute("aria-label") ?? "")
      .filter((l) => l !== "")
      .map((l) => l.split(", ").slice(0, 2).join(", "));
    expect(names).toEqual([
      "Import media, You",
      "Insert clip, You",
      "Move clip, You",
      "Trim clip, Claude",
      "Edit properties, acme.luts",
      "Move clip, You",
      "Trim clip, Claude",
    ]);
    // Agent rows are agent blue, the undone tail sits below the marker, dimmed.
    const rows = screen.getAllByRole("button").filter((b) => b.dataset.tx !== undefined);
    expect(rows.map((r) => r.dataset.actor)).toEqual([
      "user",
      "user",
      "user",
      "agent",
      "plugin",
      "user",
      "agent",
    ]);
    expect(rows.map((r) => r.dataset.state)).toEqual([
      "applied",
      "applied",
      "applied",
      "applied",
      "applied",
      "undone",
      "undone",
    ]);
    expect(rowOf(/^Move clip, You, .*, undone$/)).toBeTruthy();
  });

  it("follows edit.history_changed: a new edit moves the marker, unredoable rows are gone", async () => {
    view();
    await screen.findByRole("list", { name: "Project history" });
    expect(screen.getAllByRole("button").filter((b) => b.dataset.state === "undone")).toHaveLength(
      2,
    );
    // A new edit clears the redo tail.
    await changeHistory([
      entry(10, "clip.split", "user"),
      entry(8, "Trim clip", "agent:claude", { status: "undone", can_redo: false }),
      entry(7, "Move clip", "user", { status: "undone", can_redo: false }),
      ...mixedHistory().slice(3),
    ]);
    await waitFor(() => expect(screen.getByText("Split clip")).toBeTruthy());
    const rows = screen.getAllByRole("button").filter((b) => b.dataset.tx !== undefined);
    expect(rows.some((r) => r.dataset.state === "undone")).toBe(false);
    const items = within(screen.getByRole("list", { name: "Project history" })).getAllByRole(
      "listitem",
    );
    expect(items[items.length - 1]?.getAttribute("aria-current")).toBe("step");
  });

  it("goes back to a row with one edit.undo of the later steps, anyone's, newest first", async () => {
    const user = userEvent.setup();
    view();
    await screen.findByRole("list", { name: "Project history" });
    await user.click(rowOf(/^Move clip, You, [^,]*$/));
    const undo = calls("edit.undo");
    expect(undo).toEqual([["edit.undo", { actor: "*", steps: 2 }]]);
  });

  it("goes forward from a dimmed row with edit.redo up to it", async () => {
    const user = userEvent.setup();
    view();
    await screen.findByRole("list", { name: "Project history" });
    await user.click(rowOf(/^Trim clip, Claude, .*, undone$/));
    expect(calls("edit.redo")).toEqual([["edit.redo", { actor: "*", steps: 2 }]]);
  });

  it("does nothing on the row it is already at", async () => {
    const user = userEvent.setup();
    view();
    await screen.findByRole("list", { name: "Project history" });
    await user.click(rowOf(/^Edit properties, acme.luts/));
    expect(call.mock.calls.some(([name]) => name === "edit.undo" || name === "edit.redo")).toBe(
      false,
    );
  });

  it("shows the engine's message when a step is refused and stops there", async () => {
    const user = userEvent.setup();
    view();
    await screen.findByRole("list", { name: "Project history" });
    const message = "item.rename changed /name since: user in tx_6";
    call.mockImplementation(async (name: string) => {
      if (name === "edit.history") return { entries, next_cursor: null };
      return { done: 1, stopped: message };
    });
    await user.click(rowOf(/^Import media, You/));
    expect((await screen.findByRole("status")).textContent).toContain(message);
    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("shows a refusal of the first step as the same message", async () => {
    const user = userEvent.setup();
    view();
    await screen.findByRole("list", { name: "Project history" });
    call.mockImplementation(async (name: string) => {
      if (name === "edit.history") return { entries, next_cursor: null };
      throw new Error("a later change by the agent touched it");
    });
    await user.click(rowOf(/^Import media, You/));
    expect((await screen.findByRole("status")).textContent).toContain(
      "a later change by the agent",
    );
  });

  it("dims the rows a filter leaves out, and still goes by the shared order", async () => {
    const user = userEvent.setup();
    view();
    await screen.findByRole("list", { name: "Project history" });
    await user.click(screen.getByRole("radio", { name: "Agents" }));
    const rows = screen.getAllByRole("button").filter((b) => b.dataset.tx !== undefined);
    expect(rows.map((r) => r.dataset.dim === "true")).toEqual([
      true,
      true,
      true,
      false,
      true,
      true,
      false,
    ]);
    await user.click(rowOf(/^Insert clip, You/));
    expect(calls("edit.undo")).toEqual([["edit.undo", { actor: "*", steps: 3 }]]);
  });

  it("moves between rows with the arrow keys and goes with Enter", async () => {
    const user = userEvent.setup();
    view();
    await screen.findByRole("list", { name: "Project history" });
    // The roving stop starts on the row at the marker.
    const start = screen.getAllByRole("button").find((b) => b.getAttribute("tabindex") === "0");
    expect(start?.getAttribute("aria-label")).toMatch(/^Edit properties/);
    start?.focus();
    await user.keyboard("{ArrowUp}{ArrowUp}{ArrowUp}");
    expect(document.activeElement?.getAttribute("aria-label")).toMatch(/^Insert clip/);
    await user.keyboard("{Enter}");
    expect(calls("edit.undo")).toEqual([["edit.undo", { actor: "*", steps: 3 }]]);
  });

  it("says there are no changes yet", async () => {
    entries = [];
    view();
    expect(await screen.findByText("No changes yet")).toBeTruthy();
    expect(screen.queryByRole("list", { name: "Project history" })).toBeNull();
  });

  it("has no axe violations, with a history or none", async () => {
    const { container } = view();
    await screen.findByRole("list", { name: "Project history" });
    expect((await axe.run(container)).violations).toEqual([]);
    cleanup();
    entries = [];
    historyStore.setState({ ...initialHistoryState });
    const empty = view();
    await screen.findByText("No changes yet");
    expect((await axe.run(empty.container)).violations).toEqual([]);
  });
});
