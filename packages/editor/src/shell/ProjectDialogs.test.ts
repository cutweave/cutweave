// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-1M.07 dialogs: crash recovery (Restore / Discard), "Save changes?", the name and place of a
// new project, the read-only banner, the top bar's undo labels and the keyboard shortcuts. The
// engine is a recording fake; `session/session.integration.test.ts` runs the same flows on a real
// engine.
import { RpcError } from "@cutweave/protocol";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Fragment, createElement as h, type ReactElement } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../i18n/index.ts";
import { prefsStore } from "../prefs/prefsStore.ts";
import { EMPTY_STACKS, initialSessionState, sessionStore } from "../session/sessionStore.ts";
import { sessionUiStore } from "../session/sessionUiStore.ts";
import { resetProjectController, useProjectWiring } from "../session/useProject.ts";
import { docStore } from "../state/docStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { createFakeEngine, type FakeEngine } from "../testing/fakeEngine.ts";
import { AppMenuBar } from "./AppMenuBar.tsx";
import { ProjectDialogs, ReadOnlyBanner } from "./ProjectDialogs.tsx";
import { TopBar } from "./TopBar.tsx";

beforeAll(() => {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
  globalThis.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
    onchange: null,
  })) as unknown as typeof matchMedia;
  Element.prototype.scrollIntoView ??= () => {};
});

let engine: FakeEngine;

const status = (extra: object = {}) => ({
  state: "open",
  path: "C:\\p\\Film\\Film.cutweave",
  name: "Film",
  rev: 4,
  saved_rev: 1,
  dirty: false,
  read_only: null,
  recovery: null,
  ...extra,
});

const wrap = (child: ReactElement) => h(I18nextProvider, { i18n }, h(TooltipProvider, null, child));

beforeEach(() => {
  engine = createFakeEngine();
  engine.install();
  sessionStore.setState({ ...initialSessionState, known: true, name: "Film" });
  sessionUiStore.setState({ unsaved: null, naming: null, recent: [], busy: false });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  toastStore.getState().clear();
  resetProjectController();
});

afterEach(() => {
  cleanup();
  engine.uninstall();
  resetProjectController();
  delete (globalThis as { cutweave?: unknown }).cutweave;
});

describe("recovery dialog", () => {
  const lastAt = new Date(2026, 10, 23, 14, 5).getTime();
  const recovery = { records: 3, base_rev: 1, last_rev: 4, last_ts: lastAt, damaged: false };

  it("offers Restore with the edit count and time, and Discard; it cannot be dismissed", async () => {
    sessionStore.setState({ recovery });
    render(wrap(h(ProjectDialogs)));
    const dialog = await screen.findByRole("dialog", { name: "Restore unsaved changes?" });
    expect(within(dialog).getByText(/3 edits from 14:05 can be restored/)).toBeTruthy();
    expect(
      within(dialog).getByRole("button", {
        name: "Restore unsaved changes (3 edits, last at 14:05)",
      }),
    ).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "Discard" })).toBeTruthy();
    await userEvent.setup().keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeNull();
  });

  it("Restore sends project.recover, closes the dialog and says how many edits came back", async () => {
    sessionStore.setState({ recovery });
    engine.answer("project.recover", () => ({ project: status(), applied: 3, damaged: false }));
    render(wrap(h(ProjectDialogs)));
    await userEvent
      .setup()
      .click(await screen.findByRole("button", { name: /Restore unsaved changes \(3 edits/ }));
    await waitFor(() => expect(sessionStore.getState().recovery).toBeNull());
    expect(engine.names()).toEqual(["project.recover"]);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(toastStore.getState().notices[0]).toMatchObject({
      tone: "success",
      title: "Changes restored",
    });
  });

  it("Discard sends project.discard_recovery", async () => {
    sessionStore.setState({ recovery });
    engine.answer("project.discard_recovery", () => ({
      project: status(),
      applied: 0,
      damaged: false,
    }));
    render(wrap(h(ProjectDialogs)));
    await userEvent.setup().click(await screen.findByRole("button", { name: "Discard" }));
    await waitFor(() => expect(sessionStore.getState().recovery).toBeNull());
    expect(engine.names()).toEqual(["project.discard_recovery"]);
  });

  it("says when the journal ended in a damaged entry, and keeps the dialog if Restore fails", async () => {
    sessionStore.setState({ recovery: { ...recovery, records: 1, damaged: true } });
    engine.reject("project.recover", new Error("journal unreadable"));
    render(wrap(h(ProjectDialogs)));
    expect(await screen.findByText(/damaged entry/)).toBeTruthy();
    expect(screen.getByText(/1 edit from 14:05 can be restored/)).toBeTruthy();
    await userEvent.setup().click(screen.getByRole("button", { name: /Restore unsaved changes/ }));
    await waitFor(() => expect(toastStore.getState().notices).toHaveLength(1));
    expect(toastStore.getState().notices[0]?.title).toBe("Changes not restored");
    expect(screen.queryByRole("dialog")).not.toBeNull();
  });
});

describe("unsaved changes dialog", () => {
  function ask() {
    const resolve = vi.fn();
    sessionUiStore.getState().setUnsaved({ project: "Film", resolve });
    return resolve;
  }

  it("names the project and answers Save, Discard changes or Cancel", async () => {
    const resolve = ask();
    render(wrap(h(ProjectDialogs)));
    const dialog = await screen.findByRole("dialog", { name: "Save changes to Film?" });
    const user = userEvent.setup();
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await user.click(within(dialog).getByRole("button", { name: "Discard changes" }));
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(resolve.mock.calls.map((c) => c[0])).toEqual(["save", "discard", "cancel"]);
  });

  it("Escape cancels", async () => {
    const resolve = ask();
    render(wrap(h(ProjectDialogs)));
    await screen.findByRole("dialog");
    await userEvent.setup().keyboard("{Escape}");
    expect(resolve).toHaveBeenCalledWith("cancel");
  });
});

describe("new project dialog", () => {
  it("asks for a name and a folder, validates, and resolves with the project folder", async () => {
    const resolve = vi.fn();
    sessionUiStore.getState().setNaming({ mode: "new", initialName: "", resolve });
    render(wrap(h(ProjectDialogs)));
    const dialog = await screen.findByRole("dialog", { name: "New project" });
    const user = userEvent.setup();
    await user.click(within(dialog).getByRole("button", { name: "Create project" }));
    expect((await within(dialog).findByRole("alert")).textContent).toBe("Give the project a name.");
    expect(resolve).not.toHaveBeenCalled();

    await user.type(within(dialog).getByRole("textbox", { name: "Name" }), "a/b");
    await user.click(within(dialog).getByRole("button", { name: "Create project" }));
    expect((await within(dialog).findByRole("alert")).textContent).toMatch(/cannot contain/);

    const name = within(dialog).getByRole("textbox", { name: "Name" });
    await user.clear(name);
    await user.type(name, "My film");
    await user.click(within(dialog).getByRole("button", { name: "Create project" }));
    expect((await within(dialog).findByRole("alert")).textContent).toBe(
      "Choose the folder to create it in.",
    );

    await user.type(within(dialog).getByRole("textbox", { name: "Location" }), "C:\\Videos");
    await user.click(within(dialog).getByRole("button", { name: "Create project" }));
    expect(resolve).toHaveBeenCalledWith({
      name: "My film",
      parent: "C:\\Videos",
      path: "C:\\Videos\\My film",
    });
  });

  it("Save as uses its own title and Cancel resolves null", async () => {
    const resolve = vi.fn();
    sessionUiStore.getState().setNaming({ mode: "saveAs", initialName: "Film", resolve });
    render(wrap(h(ProjectDialogs)));
    const dialog = await screen.findByRole("dialog", { name: "Save project as" });
    expect((within(dialog).getByRole("textbox", { name: "Name" }) as HTMLInputElement).value).toBe(
      "Film",
    );
    await userEvent.setup().click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(resolve).toHaveBeenCalledWith(null);
  });
});

describe("read-only banner", () => {
  it("appears for a project from a newer version and offers Save as", async () => {
    render(wrap(h(ReadOnlyBanner)));
    expect(screen.queryByRole("status")).toBeNull();
    sessionStore.setState({ readOnly: "schema_version 9 is newer than this build" });
    const banner = await screen.findByRole("status");
    expect(banner.textContent).toMatch(/Read-only project/);
    await userEvent.setup().click(within(banner).getByRole("button", { name: "Save as…" }));
    expect(sessionUiStore.getState().naming?.mode).toBe("saveAs");
  });
});

describe("top bar", () => {
  it("names what Undo and Redo would do and disables them when there is nothing to do", () => {
    sessionStore.setState({ stacks: EMPTY_STACKS });
    render(wrap(h(TopBar)));
    expect((screen.getByRole("button", { name: "Undo" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Redo" }) as HTMLButtonElement).disabled).toBe(true);
    cleanup();

    sessionStore.setState({
      stacks: {
        undoDepth: 2,
        redoDepth: 1,
        nextUndo: "clip.split",
        nextRedo: "Move clip",
        nextUndoActor: "user",
        nextRedoActor: "user",
      },
    });
    render(wrap(h(TopBar)));
    expect(
      (screen.getByRole("button", { name: "Undo Split clip" }) as HTMLButtonElement).disabled,
    ).toBe(false);
    expect(
      (screen.getByRole("button", { name: "Redo Move clip" }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });

  it("has no Save button: File > Save sends project.save for a titled project", async () => {
    sessionStore.setState({ path: "C:\\p\\Film\\Film.cutweave", dirty: true });
    render(wrap(h(TopBar)));
    expect(screen.queryByRole("button", { name: /^Save/ })).toBeNull();
    const user = userEvent.setup();
    await user.click(screen.getByRole("menuitem", { name: "File" }));
    await user.click(await screen.findByRole("menuitem", { name: /^Save(?! as)/ }));
    await waitFor(() => expect(engine.names()).toEqual(["project.save"]));
  });

  it("File > Save on an untitled project asks where to save it", async () => {
    sessionStore.setState({ path: null });
    render(wrap(h(TopBar)));
    const user = userEvent.setup();
    await user.click(screen.getByRole("menuitem", { name: "File" }));
    await user.click(await screen.findByRole("menuitem", { name: /^Save(?! as)/ }));
    await waitFor(() => expect(sessionUiStore.getState().naming?.mode).toBe("saveAs"));
    expect(engine.calls).toEqual([]);
  });

  it("lists the recent projects and opens the one picked", async () => {
    sessionUiStore.setState({ recent: ["C:\\p\\Film\\Film.cutweave", "C:\\p\\Short"] });
    engine.answer("project.open", () => ({
      project: status({ path: "C:\\p\\Short\\Short.cutweave", name: "Short" }),
      missing_media: [],
      migrated_from: null,
    }));
    render(wrap(h(TopBar)));
    const user = userEvent.setup();
    // File > Open recent (the top bar keeps its own Recent list on macOS only).
    await user.click(screen.getByRole("menuitem", { name: "File" }));
    const sub = await screen.findByRole("menuitem", { name: /Open recent/ });
    sub.focus();
    await user.keyboard("{ArrowRight}");
    const menu = await screen.findByRole("menu", { name: "Open recent" });
    expect(
      within(menu)
        .getAllByRole("menuitem")
        .map((i) => i.textContent),
    ).toEqual(["Film.cutweave", "Short"]);
    await user.click(within(menu).getByRole("menuitem", { name: "Short" }));
    await waitFor(() => expect(engine.names()).toEqual(["project.open"]));
    expect(engine.calls[0]?.input).toEqual({ path: "C:\\p\\Short" });
    await waitFor(() => expect(sessionStore.getState().name).toBe("Short"));
  });

  it("disables the project actions until the engine is connected", async () => {
    engine.uninstall();
    render(wrap(h(TopBar)));
    const user = userEvent.setup();
    await user.click(screen.getByRole("menuitem", { name: "File" }));
    const save = await screen.findByRole("menuitem", { name: /^Save(?! as)/ });
    expect(save.getAttribute("aria-disabled")).toBe("true");
    expect(screen.queryByRole("button", { name: /^(New|Open)$/ })).toBeNull();
  });
});

describe("the title bar's save status (WP-1M.10)", () => {
  const statusText = () => screen.getByTestId("save-status").textContent;

  beforeEach(() => {
    sessionStore.setState({ path: "C:\\p\\Film\\Film.cutweave", dirty: false });
    prefsStore.setState({ values: { ...prefsStore.getState().values, "saving.autosave": true } });
  });

  it("says everything is saved, with no unsaved dot", () => {
    render(wrap(h(TopBar)));
    expect(statusText()).toBe("All changes saved");
    expect(screen.queryByTestId("dirty-dot")).toBeNull();
  });

  it("says unsaved changes, and whether autosave will save them", () => {
    render(wrap(h(TopBar)));
    act(() => sessionStore.setState({ dirty: true }));
    expect(statusText()).toBe("Unsaved changes, saving automatically");
    expect(screen.getByTestId("dirty-dot")).toBeTruthy();
    act(() =>
      prefsStore.setState({
        values: { ...prefsStore.getState().values, "saving.autosave": false },
      }),
    );
    expect(statusText()).toBe("Unsaved changes, autosave is off");
    expect(screen.getByTestId("dirty-dot")).toBeTruthy();
  });

  it("says Saving… while a save runs, and Autosave failed after project.autosave_failed", () => {
    render(wrap(h(TopBar)));
    act(() => sessionStore.setState({ dirty: true }));
    act(() => sessionUiStore.setState({ saving: true }));
    expect(statusText()).toBe("Saving…");
    act(() => sessionUiStore.setState({ saving: false }));
    act(() => sessionStore.setState({ autosaveFailed: true }));
    expect(statusText()).toBe("Autosave failed");
    expect(screen.getByTestId("dirty-dot")).toBeTruthy();
    // A save clears it.
    act(() => sessionStore.setState({ dirty: false, autosaveFailed: false }));
    expect(statusText()).toBe("All changes saved");
  });
});

describe("keyboard shortcuts of the menu bar (WP-1M.09)", () => {
  function Wired() {
    useProjectWiring();
    return h(Fragment, null, h(AppMenuBar), h("input", { "aria-label": "Field" }));
  }

  const realPlatform = Object.getOwnPropertyDescriptor(Navigator.prototype, "platform");
  const setPlatform = (value: string) =>
    Object.defineProperty(navigator, "platform", { value, configurable: true });
  afterEach(() => {
    delete (navigator as { platform?: string }).platform;
    if (realPlatform !== undefined) {
      Object.defineProperty(Navigator.prototype, "platform", realPlatform);
    }
  });

  it("Ctrl+Z undoes, Ctrl+Shift+Z and Ctrl+Y redo, Ctrl+S saves", async () => {
    setPlatform("Win32");
    sessionStore.setState({
      path: "C:\\p\\Film\\Film.cutweave",
      stacks: {
        undoDepth: 1,
        redoDepth: 1,
        nextUndo: "Move clip",
        nextRedo: "Trim clip",
        nextUndoActor: "user",
        nextRedoActor: "user",
      },
    });
    render(wrap(h(Wired)));
    fireEvent.keyDown(window, { key: "z", ctrlKey: true });
    fireEvent.keyDown(window, { key: "Z", ctrlKey: true, shiftKey: true });
    fireEvent.keyDown(window, { key: "y", ctrlKey: true });
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    await waitFor(() => expect(engine.names()).toHaveLength(4));
    expect(engine.names()).toEqual(["edit.undo", "edit.redo", "edit.redo", "project.save"]);
  });

  it("Undo and Redo ask for anyone's latest change (ADR-0019)", async () => {
    setPlatform("Win32");
    sessionStore.setState({
      path: "C:pFilmFilm.cutweave",
      stacks: {
        undoDepth: 1,
        redoDepth: 1,
        nextUndo: "Insert clip",
        nextRedo: "Trim clip",
        nextUndoActor: "agent:claude",
        nextRedoActor: "user",
      },
    });
    render(wrap(h(Wired)));
    fireEvent.keyDown(window, { key: "z", ctrlKey: true });
    fireEvent.keyDown(window, { key: "y", ctrlKey: true });
    await waitFor(() => expect(engine.names()).toHaveLength(2));
    expect(engine.calls.map((c) => c.input)).toEqual([{ actor: "*" }, { actor: "*" }]);
  });

  it("names the agent on the Undo button when it made the latest change", () => {
    cleanup();
    sessionStore.setState({
      stacks: {
        undoDepth: 1,
        redoDepth: 0,
        nextUndo: "Insert clip",
        nextRedo: null,
        nextUndoActor: "agent:claude",
        nextRedoActor: null,
      },
    });
    render(wrap(h(TopBar)));
    expect(screen.getByRole("button", { name: "Undo Insert clip (Claude)" })).toBeTruthy();
  });

  it("explains a refused Undo in plain words", async () => {
    setPlatform("Win32");
    sessionStore.setState({
      path: "C:pFilmFilm.cutweave",
      stacks: {
        undoDepth: 1,
        redoDepth: 0,
        nextUndo: "Import media",
        nextRedo: null,
        nextUndoActor: "user",
        nextRedoActor: null,
      },
    });
    engine.reject(
      "edit.undo",
      new RpcError("conflict", "conflict (history_target_modified): later edits", undefined, {
        reason: "history_target_modified",
      }),
    );
    render(wrap(h(Wired)));
    fireEvent.keyDown(window, { key: "z", ctrlKey: true });
    await waitFor(() => expect(toastStore.getState().notices).toHaveLength(1));
    const notice = toastStore.getState().notices[0];
    expect(notice?.title).toBe("Nothing was undone");
    expect(notice?.message).toBe(
      "Import media could not be undone because later edits changed the same clips or media. Undo those first.",
    );
    expect(notice?.message).not.toMatch(/history_target_modified/);
  });

  it("leaves Ctrl+Z to a text field that has the focus", async () => {
    sessionStore.setState({
      stacks: {
        undoDepth: 1,
        redoDepth: 0,
        nextUndo: "Move clip",
        nextRedo: null,
        nextUndoActor: "user",
        nextRedoActor: null,
      },
    });
    render(wrap(h(Wired)));
    const field = screen.getByRole("textbox", { name: "Field" });
    field.focus();
    fireEvent.keyDown(field, { key: "z", ctrlKey: true });
    await new Promise((r) => setTimeout(r, 20));
    expect(engine.calls).toEqual([]);
  });

  it("runs the other shortcuts from a text field, as native accelerators do", async () => {
    sessionStore.setState({ path: null });
    render(wrap(h(Wired)));
    const field = screen.getByRole("textbox", { name: "Field" });
    field.focus();
    fireEvent.keyDown(field, { key: "n", ctrlKey: true });
    await waitFor(() => expect(sessionUiStore.getState().naming?.mode).toBe("new"));
  });

  it("a project shortcut pressed while a save is running runs once it has finished", async () => {
    sessionStore.setState({ path: "C:\\p\\Film\\Film.cutweave" });
    sessionUiStore.setState({ busy: true });
    render(wrap(h(Wired)));
    fireEvent.keyDown(window, { key: "n", ctrlKey: true });
    await new Promise((r) => setTimeout(r, 60));
    expect(sessionUiStore.getState().naming).toBeNull();
    act(() => sessionUiStore.setState({ busy: false }));
    await waitFor(() => expect(sessionUiStore.getState().naming?.mode).toBe("new"));
  });

  it("does nothing for Undo when there is nothing to undo", async () => {
    render(wrap(h(Wired)));
    fireEvent.keyDown(window, { key: "z", ctrlKey: true });
    await new Promise((r) => setTimeout(r, 20));
    expect(engine.calls).toEqual([]);
  });

  it("runs nothing while the engine is not connected (the menu items are disabled)", async () => {
    engine.uninstall();
    sessionStore.setState({ path: "C:\\p\\Film\\Film.cutweave" });
    render(wrap(h(Wired)));
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    fireEvent.keyDown(window, { key: "n", ctrlKey: true });
    await new Promise((r) => setTimeout(r, 20));
    expect(engine.calls).toEqual([]);
    expect(sessionUiStore.getState().naming).toBeNull();
  });

  it("in the Windows and Linux app the page has the keys even though a menu bridge exists", async () => {
    setPlatform("Win32");
    (globalThis as { cutweave?: unknown }).cutweave = {
      dialog: { openProjectFolder: () => Promise.resolve(null) },
      menu: { onCommand: () => () => undefined },
    };
    resetProjectController();
    sessionStore.setState({
      path: "C:\\p\\Film\\Film.cutweave",
      stacks: {
        undoDepth: 1,
        redoDepth: 0,
        nextUndo: "Move clip",
        nextRedo: null,
        nextUndoActor: "user",
        nextRedoActor: null,
      },
    });
    render(wrap(h(Wired)));
    fireEvent.keyDown(window, { key: "z", ctrlKey: true });
    await waitFor(() => expect(engine.names()).toEqual(["edit.undo"]));
  });

  it("with the native menu (macOS) the page ignores the keys and the menu commands run", async () => {
    setPlatform("MacIntel");
    const listeners: ((command: string) => void)[] = [];
    (globalThis as { cutweave?: unknown }).cutweave = {
      dialog: { openProjectFolder: () => Promise.resolve(null) },
      menu: {
        onCommand: (l: (c: string) => void) => {
          listeners.push(l);
          return () => undefined;
        },
      },
    };
    resetProjectController();
    sessionStore.setState({
      path: "C:\\p\\Film\\Film.cutweave",
      stacks: {
        undoDepth: 1,
        redoDepth: 0,
        nextUndo: "Move clip",
        nextRedo: null,
        nextUndoActor: "user",
        nextRedoActor: null,
      },
    });
    render(wrap(h(Wired)));
    fireEvent.keyDown(window, { key: "z", metaKey: true });
    await new Promise((r) => setTimeout(r, 20));
    expect(engine.calls).toEqual([]);
    listeners[0]?.("undo");
    await waitFor(() => expect(engine.names()).toEqual(["edit.undo"]));
    listeners[0]?.("save");
    await waitFor(() => expect(engine.names()).toEqual(["edit.undo", "project.save"]));
  });
});
