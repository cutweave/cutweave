// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// File > Open and unsaved changes (ADR-0029, issue #161): the app's own open drops unsaved
// changes only after the person chose Discard, in the same call (`discard_unsaved: true`, no
// close first); when the engine refuses an open for changes the editor did not count, it asks
// then. Through the real ProjectController over the fake engine.
import { RpcError } from "@cutweave/protocol";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { docStore } from "../state/docStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { createFakeEngine, type FakeEngine } from "../testing/fakeEngine.ts";
import { EMPTY_STACKS, initialSessionState, sessionStore } from "./sessionStore.ts";
import { sessionUiStore, type UnsavedChoice } from "./sessionUiStore.ts";
import { projectController, resetProjectController } from "./useProject.ts";

let engine: FakeEngine;

const opened = {
  project: {
    state: "open",
    path: "C:\\p\\Other\\Other.cutweave",
    name: "Other",
    rev: 1,
    saved_rev: 1,
    dirty: false,
    read_only: null,
    recovery: null,
  },
  missing_media: [],
  migrated_from: null,
};

const unsaved = () =>
  new RpcError("conflict", '"Film" has unsaved changes', undefined, {
    reason: "unsaved_changes",
  });

beforeEach(() => {
  engine = createFakeEngine();
  engine.install();
  sessionStore.setState({
    ...initialSessionState,
    known: true,
    path: "C:\\p\\Film\\Film.cutweave",
    name: "Film",
    stacks: EMPTY_STACKS,
  });
  sessionUiStore.setState({ unsaved: null, naming: null, recent: [], busy: false });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  toastStore.getState().clear();
  resetProjectController();
  engine.answer("project.open", () => opened);
});

afterEach(() => {
  engine.uninstall();
  resetProjectController();
});

/** Waits for the "Save changes?" question and answers it. */
async function answer(choice: UnsavedChoice): Promise<void> {
  for (let i = 0; i < 100 && sessionUiStore.getState().unsaved === null; i++) {
    await new Promise((r) => setTimeout(r, 10));
  }
  const prompt = sessionUiStore.getState().unsaved;
  if (prompt === null) throw new Error("the editor did not ask");
  prompt.resolve(choice);
}

const opens = () => engine.calls.filter((c) => c.name === "project.open").map((c) => c.input);

describe("File > Open with unsaved changes", () => {
  it("with nothing unsaved, opens without dropping anything", async () => {
    expect(await projectController().open("C:\\p\\Other")).toBe(true);
    expect(engine.names()).toEqual(["project.open"]);
    expect(opens()).toEqual([{ path: "C:\\p\\Other" }]);
    expect(sessionStore.getState().name).toBe("Other");
  });

  it("Discard drops them in the open itself, not with a close first", async () => {
    sessionStore.setState({ dirty: true });
    const flow = projectController().open("C:\\p\\Other");
    await answer("discard");
    expect(await flow).toBe(true);
    expect(engine.names()).toEqual(["project.open"]);
    expect(opens()).toEqual([{ path: "C:\\p\\Other", discard_unsaved: true }]);
  });

  it("Cancel stays and asks the engine nothing", async () => {
    sessionStore.setState({ dirty: true });
    const flow = projectController().open("C:\\p\\Other");
    await answer("cancel");
    expect(await flow).toBe(false);
    expect(engine.names()).toEqual([]);
  });

  it("asks when the engine knows of changes the editor did not count, then drops them", async () => {
    engine.reject("project.open", unsaved());
    const flow = projectController().open("C:\\p\\Other");
    await answer("discard");
    expect(await flow).toBe(true);
    expect(opens()).toEqual([
      { path: "C:\\p\\Other" },
      { path: "C:\\p\\Other", discard_unsaved: true },
    ]);
    expect(sessionUiStore.getState().unsaved).toBeNull();
  });

  it("Cancel after such a refusal stays on the project, with no error toast", async () => {
    engine.reject("project.open", unsaved());
    const flow = projectController().open("C:\\p\\Other");
    await answer("cancel");
    expect(await flow).toBe(false);
    expect(opens()).toEqual([{ path: "C:\\p\\Other" }]);
    expect(sessionStore.getState().name).toBe("Film");
    expect(toastStore.getState().notices).toEqual([]);
  });
});
