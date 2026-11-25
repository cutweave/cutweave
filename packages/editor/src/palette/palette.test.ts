// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2H.04 acceptance (Vitest part): a registered command appears in the palette, and in a menu
// surface, with no UI code change; `when` filters the list; the list opens in under 100 ms with
// 400 commands and a keystroke ranks in under 16 ms; keyboard-only operation; the argument form;
// recents first; the row's menu; axe.
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n, type TKey } from "../i18n/index.ts";
import { keymapModelFor } from "../keymap/store.ts";
import { strokeOfEvent } from "../keymap/stroke.ts";
import { buildMenu } from "../menus/build.ts";
import { registerBuiltinMenus } from "../menus/builtins.ts";
import { ContextMenuHost, contextMenuStore, openContextMenu } from "../menus/host.tsx";
import { createMenuServices, type MenuServices } from "../menus/services.ts";
import { setWhenLogger } from "../menus/when.ts";
import { settingsPages } from "../settings/registry.ts";
import "../settings/core.ts";
import { listBindings } from "../shortcuts/bindings.ts";
import { listWorkspaceCommands } from "../workspace/commands.ts";
import type { PanelDef } from "../workspace/panelRegistry.ts";
import { registerPaletteAction } from "./actions.ts";
import {
  fieldsOf,
  fullyPrefilled,
  initialValues,
  needsArguments,
  prefillFor,
  toArguments,
} from "./argForm.ts";
import { CommandPalette, MAX_ROWS, visibleEntries } from "./CommandPalette.tsx";
import { type CatalogCommand, commandCatalog, listedCatalog } from "./catalog.ts";
import { registerPaletteCommands } from "./commands.ts";
import { buildEntries, type PaletteEnv, planFor } from "./entries.ts";
import { openPalette, paletteStore } from "./paletteStore.ts";
import { MAX_RECENTS, recentsStore } from "./recents.ts";
import { rank, searchable } from "./search.ts";

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
  globalThis.requestAnimationFrame ??= (cb: FrameRequestCallback) =>
    setTimeout(() => cb(0), 0) as unknown as number;
});

// ---- fixtures -----------------------------------------------------------------------------------

const FIXTURE: CatalogCommand = {
  name: "fixture.do_thing",
  namespace: "fixture",
  summary: "Do the fixture thing",
  undoable: true,
  destructive: false,
  readOnly: false,
  hidden: false,
  view: false,
  inputSchema: { type: "object" },
};

const PANELS: PanelDef[] = [
  {
    id: "inspector",
    title: "panel.inspector" as TKey,
    component: () => null,
    defaultDock: { ref: null, direction: "right" },
  },
];

let call: ReturnType<typeof vi.fn<(name: string, args?: unknown) => Promise<unknown>>>;
let report: ReturnType<typeof vi.fn<(error: unknown) => void>>;
let keys: PaletteEnv["keys"];
let selection: string[];

function makeServices(extra: Partial<MenuServices> = {}): MenuServices {
  call = vi.fn(async () => ({}));
  report = vi.fn();
  const services = createMenuServices({
    call,
    keysFor: (c) => (c === "clip.split" ? "S" : undefined),
    baseKeys: () => ({ "engine.live": true, "project.readOnly": false }),
    playhead: () => 1234,
    t: (spec) => i18n.t(spec.key, spec.vars ?? {}) as string,
    now: () => 0,
    report,
    ...extra,
  });
  registerBuiltinMenus(services);
  registerPaletteCommands(services);
  return services;
}

function makeEnv(services: MenuServices, over: Partial<PaletteEnv> = {}): PaletteEnv {
  return {
    services,
    catalog: () => commandCatalog.getState().commands,
    panels: () => PANELS,
    workspaceCommands: () => [
      ...listWorkspaceCommands(),
      ...PANELS.map((p) => ({
        id: `workspace.toggle_panel.${p.id}`,
        title: "workspace.commands.togglePanelNamed" as TKey,
        panel: p.id,
      })),
    ],
    settingsPages,
    keys: () => keys(),
    prefill: () => ({ items: selection, playhead: services.playhead() }),
    ...over,
  };
}

beforeEach(() => {
  selection = [];
  keys = () => ({
    "engine.live": true,
    "project.readOnly": false,
    "selection.count": selection.length,
  });
  recentsStore.getState().clear();
  paletteStore.setState({ open: false, pendingRun: null });
  setWhenLogger(() => {});
});

afterEach(() => {
  cleanup();
  setWhenLogger(null);
});

const titles = (entries: readonly { title: string }[]): string[] => entries.map((e) => e.title);

// ---- the list --------------------------------------------------------------------------------------

describe("what the palette lists", () => {
  it("lists every non-hidden catalog command, and none of the engine's plumbing or view calls", () => {
    const services = makeServices();
    selection = ["itm_a"]; // item.delete needs a selection
    const entries = buildEntries(makeEnv(services));
    const listed = listedCatalog(commandCatalog.getState().commands);
    expect(listed.length).toBeGreaterThan(40);
    for (const command of listed) {
      expect(
        entries.some((e) => e.command === command.name),
        command.name,
      ).toBe(true);
    }
    const all = commandCatalog.getState().commands;
    expect(all.some((c) => c.hidden)).toBe(true);
    for (const command of all.filter((c) => c.hidden || c.view)) {
      expect(
        entries.some((e) => e.command === command.name),
        command.name,
      ).toBe(false);
    }
  });

  it("shows a catalog command by the editor's label when it has one, else the engine's summary", () => {
    const entries = buildEntries(makeEnv(makeServices()));
    const split = entries.find((e) => e.command === "clip.split");
    expect(split?.title).toBe("Split at playhead");
    expect(split?.chips).toEqual(["clip"]);
    expect(split?.shortcut).toBe("S");
    // No editor label for `clip.slip` (the edge menu labels `clip.trim`): the engine's summary.
    const slip = entries.find((e) => e.command === "clip.slip");
    expect(slip?.title).toBe(slip?.catalog?.summary);
    expect(slip?.title).toBeTruthy();
  });

  it("leaves out the menu-only UI commands (a text field's Cut) and lists the editor's own", () => {
    const entries = buildEntries(makeEnv(makeServices()));
    expect(entries.some((e) => e.command === "edit.text.cut")).toBe(false);
    expect(entries.some((e) => e.command === "palette.copy_id")).toBe(false);
    expect(entries.find((e) => e.command === "app.open_settings" && e.kind === "ui")).toBeDefined();
  });

  it("lists panels, workspaces and settings pages and rows, each by a stable command and arguments", () => {
    const entries = buildEntries(makeEnv(makeServices()));
    const toggle = entries.find((e) => e.command === "workspace.toggle_panel.inspector");
    expect(toggle?.kind).toBe("panel");
    expect(toggle?.chips).toEqual(["Panel"]);
    const longForm = entries.find(
      (e) => e.command === "workspace.switch_workspace" && e.args?.workspace === "long-form",
    );
    expect(longForm?.title).toBe("Switch to Long-form");
    expect(entries.some((e) => e.command === "workspace.reset_layout")).toBe(true);
    // The three argument-taking forms are for scripts: the per-panel toggles stand for them.
    expect(entries.some((e) => e.command === "workspace.open_panel")).toBe(false);

    const rows = entries.filter((e) => e.kind === "settings");
    expect(rows.length).toBeGreaterThan(5);
    const row = rows.find((e) => e.args?.row !== undefined);
    expect(row?.command).toBe("app.open_settings");
    expect(row?.chips[0]).toBe("Settings");
    const keysOf = entries.map((e) => e.key);
    expect(new Set(keysOf).size).toBe(keysOf.length);
  });
});

describe("a newly registered command", () => {
  it("appears in the palette with no UI code change", () => {
    const services = makeServices();
    expect(buildEntries(makeEnv(services)).some((e) => e.command === FIXTURE.name)).toBe(false);
    commandCatalog.getState().register(FIXTURE);
    const found = buildEntries(makeEnv(services)).find((e) => e.command === FIXTURE.name);
    expect(found?.title).toBe("Do the fixture thing");
    expect(found?.chips).toEqual(["fixture"]);
    expect(found?.schema).toBeUndefined();
  });

  it("appears in a context-menu surface and in the palette from its hints alone", () => {
    const services = makeServices();
    commandCatalog.getState().register(FIXTURE);
    // The registry entry's hints: how it reads, and when it applies. Nothing else changes.
    services.commands.register({
      name: FIXTURE.name,
      label: "menu.paste" as TKey,
      when: "selection.count > 0",
    });
    services.surfaces.register({
      id: "fixture.surface",
      name: () => ({ key: "palette.title" }),
      groups: ["g"],
      rows: [],
    });
    services.contributions.register({
      surface: "fixture.surface",
      command: FIXTURE.name,
      order: 0,
    });

    // No selection: the palette leaves it out and the menu shows it disabled.
    selection = [];
    expect(buildEntries(makeEnv(services)).some((e) => e.command === FIXTURE.name)).toBe(false);
    const target = { surface: "fixture.surface", ids: [] as string[] };
    const closed = buildMenu(services, target, () => {});
    const row = closed?.entries.find((e) => "id" in e && e.id === FIXTURE.name);
    expect(row).toMatchObject({ label: "Paste", disabled: true });

    // With a selection: both have it.
    selection = ["itm_a"];
    const entry = buildEntries(makeEnv(services)).find((e) => e.command === FIXTURE.name);
    expect(entry?.title).toBe("Paste");
    const open = buildMenu(services, { ...target, ids: ["itm_a"] }, () => {});
    const enabled = open?.entries.find((e) => "id" in e && e.id === FIXTURE.name);
    expect(enabled).toBeDefined();
    expect(enabled).not.toHaveProperty("disabled", true);
  });
});

describe("when clauses", () => {
  it("filter the list by the context", () => {
    const services = makeServices();
    keys = () => ({ "engine.live": true, "project.readOnly": false, "selection.count": 0 });
    expect(buildEntries(makeEnv(services)).some((e) => e.command === "item.delete")).toBe(false);
    keys = () => ({ "engine.live": true, "project.readOnly": false, "selection.count": 2 });
    expect(buildEntries(makeEnv(services)).some((e) => e.command === "item.delete")).toBe(true);
  });

  it("show a mutating command disabled, with the reason, while the project is read-only or the engine is down", () => {
    const services = makeServices();
    keys = () => ({ "engine.live": true, "project.readOnly": true, "selection.count": 1 });
    const readOnly = buildEntries(makeEnv(services)).find((e) => e.command === "track.add");
    expect(readOnly?.disabled).toBeDefined();
    const query = buildEntries(makeEnv(services)).find((e) => e.command === "track.list");
    expect(query).toBeUndefined(); // not in this catalog; queries have no row
    keys = () => ({ "engine.live": false, "project.readOnly": false, "selection.count": 1 });
    const down = buildEntries(makeEnv(services)).find((e) => e.command === "track.add");
    expect(down?.disabled).toBeDefined();
    const jobs = buildEntries(makeEnv(services)).find((e) => e.command === "job.list");
    expect(jobs?.disabled).toBeUndefined(); // a read has no document to protect
  });
});

// ---- search --------------------------------------------------------------------------------------

describe("search", () => {
  it("finds a command by a subsequence of its words, best match first", () => {
    const entries = buildEntries(makeEnv(makeServices()));
    const rows = entries.map((e) => e.search);
    expect(titles(rank(rows, "split").map((i) => entries[i] as { title: string }))[0]).toBe(
      "Split at playhead",
    );
    const fuzzy = rank(rows, "spl hd").map((i) => entries[i]?.title);
    expect(fuzzy[0]).toBe("Split at playhead");
    expect(rank(rows, "").length).toBe(entries.length);
    expect(rank(rows, "zzzzqq")).toEqual([]);
  });

  it("ignores case and accents and matches the namespace chip and the id", () => {
    const rows = [searchable("Café", ["media"]), searchable("Other", ["clip", "clip.split"])];
    expect(rank(rows, "CAFE")).toEqual([0]);
    expect(rank(rows, "clip.sp")).toEqual([1]);
  });

  it("puts recent commands first with no query", () => {
    const entries = buildEntries(makeEnv(makeServices()));
    const [a, b] = [entries[10], entries[3]];
    const { rows, recentCount } = visibleEntries(entries, [b?.key ?? "", a?.key ?? ""], "");
    expect(recentCount).toBe(2);
    expect(rows[0]).toBe(b);
    expect(rows[1]).toBe(a);
    expect(visibleEntries(entries, [b?.key ?? ""], "split").recentCount).toBe(0);
  });

  it("keeps at most eight recents, newest first", () => {
    for (let i = 0; i < 12; i++) recentsStore.getState().push(`cmd.${i}`);
    const kept = recentsStore.getState().keys;
    expect(kept).toHaveLength(MAX_RECENTS);
    expect(kept[0]).toBe("cmd.11");
  });
});

describe("speed with 400 commands", () => {
  it("builds the list in under 100 ms and ranks a keystroke in under 16 ms", { retry: 2 }, () => {
    const services = makeServices();
    const many: CatalogCommand[] = Array.from({ length: 400 }, (_, i) => ({
      ...FIXTURE,
      name: `bulk.command_${i}`,
      namespace: `bulk${i % 7}`,
      summary: `Bulk command number ${i} that does a thing to the timeline`,
    }));
    const env = makeEnv(services, {
      catalog: () => [...commandCatalog.getState().commands, ...many],
    });
    buildEntries(env); // warm up
    const t0 = performance.now();
    const entries = buildEntries(env);
    const built = performance.now() - t0;
    expect(entries.length).toBeGreaterThan(400);
    expect(built).toBeLessThan(100);

    const rows = entries.map((e) => e.search);
    for (const query of ["b", "bulk", "bulk comm 37", "tmln"]) {
      const t1 = performance.now();
      rank(rows, query);
      expect(performance.now() - t1, query).toBeLessThan(16);
    }
  });
});

// ---- the argument form ----------------------------------------------------------------------------

describe("the argument form", () => {
  const splitSchema = commandCatalog
    .getState()
    .commands.find((c) => c.name === "clip.split")?.inputSchema;

  it("makes one field per property from the input schema, required ones first", () => {
    const fields = fieldsOf(splitSchema ?? {});
    // `item` or `items`: the form asks for `item`, and `items` given instead also does.
    expect(fields.map((f) => [f.name, f.required])).toEqual([
      ["at", true],
      ["item", true],
      ["fine", false],
      ["items", false],
    ]);
    expect(fields[0]?.numeric).toBe(true); // a time: a number of flicks or a timecode text
  });

  it("fills item and at from the selection and the playhead, and runs at once when that is all", () => {
    const prefilled = prefillFor(splitSchema ?? {}, { items: ["itm_x"], playhead: 77 });
    expect(prefilled).toEqual({ item: "itm_x", at: 77 });
    expect(fullyPrefilled(splitSchema ?? {}, prefilled)).toBe(true);
    expect(
      fullyPrefilled(splitSchema ?? {}, prefillFor(splitSchema ?? {}, { items: [], playhead: 5 })),
    ).toBe(false);
  });

  it("turns the typed values into arguments, or says what is wrong with each field", () => {
    const fields = fieldsOf(splitSchema ?? {});
    expect(toArguments(fields, { item: "itm_x", at: "12" })).toEqual({
      ok: true,
      args: { item: "itm_x", at: 12 },
    });
    expect(toArguments(fields, { item: "itm_x", at: "1.5s" })).toEqual({
      ok: true,
      args: { item: "itm_x", at: "1.5s" },
    });
    const bad = toArguments(fields, { item: " ", at: "" });
    expect(bad.ok).toBe(false);
    expect(bad.ok ? {} : bad.errors).toEqual({
      item: "palette.form.required",
      at: "palette.form.required",
    });
    expect(initialValues(fields, { item: "itm_x" })).toEqual({
      item: "itm_x",
      at: "",
      fine: false,
      items: "",
    });
  });

  it("takes one of a command's alternatives: item or items, start or delta", () => {
    const schema = splitSchema ?? {};
    const several = prefillFor(schema, { items: ["itm_x", "itm_y"], playhead: 77 });
    expect(several).toEqual({ items: ["itm_x", "itm_y"], at: 77 });
    expect(fullyPrefilled(schema, several)).toBe(true);
    const fields = fieldsOf(schema);
    expect(toArguments(fields, { item: "", items: '["itm_x"]', at: "5" })).toEqual({
      ok: true,
      args: { items: ["itm_x"], at: 5 },
    });

    const move = commandCatalog.getState().commands.find((c) => c.name === "item.move")
      ?.inputSchema ?? { properties: {} };
    expect(needsArguments(move)).toBe(true);
    const prefilled = prefillFor(move, { items: ["itm_x"], playhead: 77 });
    expect(prefilled).toEqual({ item: "itm_x" });
    expect(fullyPrefilled(move, prefilled)).toBe(false);
    expect(
      fieldsOf(move)
        .filter((f) => f.required)
        .map((f) => [f.name, f.alternative]),
    ).toEqual([
      ["item", "items"],
      ["start", "delta"],
    ]);
    const moved = toArguments(fieldsOf(move), { item: "itm_x", delta: "10" });
    expect(moved).toEqual({ ok: true, args: { item: "itm_x", delta: 10 } });
  });

  it("reads enums, booleans, whole numbers and JSON", () => {
    const fields = fieldsOf({
      type: "object",
      required: ["mode"],
      properties: {
        mode: { enum: ["a", "b"] },
        on: { type: "boolean" },
        count: { type: "integer" },
        extra: { type: "object" },
      },
    });
    expect(fields.map((f) => [f.name, f.kind])).toEqual([
      ["mode", "select"],
      ["on", "boolean"],
      ["count", "number"],
      ["extra", "json"],
    ]);
    const ok = toArguments(fields, { mode: "a", on: true, count: "3", extra: '{"x":1}' });
    expect(ok).toEqual({ ok: true, args: { mode: "a", on: true, count: 3, extra: { x: 1 } } });
    const bad = toArguments(fields, { mode: "a", on: false, count: "1.5", extra: "{" });
    expect(bad.ok ? {} : bad.errors).toEqual({
      count: "palette.form.notWhole",
      extra: "palette.form.notJson",
    });
  });
});

// ---- the key --------------------------------------------------------------------------------------

describe("the palette's key", () => {
  const press = (over: Partial<KeyboardEvent>) =>
    ({
      key: "k",
      code: "KeyK",
      ctrlKey: true,
      metaKey: false,
      altKey: false,
      shiftKey: false,
      ...over,
    }) as KeyboardEvent;
  /** Whether the keymap (the NLE preset) runs `palette.open` for this press on `platform`. */
  const opens = (e: KeyboardEvent, platform: "windows" | "linux" | "mac"): boolean => {
    const stroke = strokeOfEvent(e, platform);
    return (
      stroke !== null &&
      keymapModelFor(platform)
        .lookup(stroke, ["global"])
        .some((b) => b.command === "palette.open")
    );
  };

  it("is Ctrl+K on Windows and Linux and Cmd+K on macOS, and no other chord", () => {
    expect(opens(press({}), "windows")).toBe(true);
    expect(opens(press({ key: "K" }), "linux")).toBe(true);
    expect(opens(press({ ctrlKey: false, metaKey: true }), "mac")).toBe(true);
    expect(opens(press({}), "mac")).toBe(false);
    expect(opens(press({ shiftKey: true }), "windows")).toBe(false);
    expect(opens(press({ altKey: true }), "windows")).toBe(false);
    expect(opens(press({ key: "j", code: "KeyJ" }), "windows")).toBe(false);
  });

  it("still opens it with a non-Latin layout (the physical key)", () => {
    expect(opens(press({ key: "ל" }), "windows")).toBe(true);
  });

  it("is in the shortcut list, and a rebinding moves it", () => {
    const binding = listBindings("windows").find((b) => b.id === "palette.open");
    expect(binding?.combos).toEqual([{ key: "K", mod: true }]);
  });
});

// ---- the component ---------------------------------------------------------------------------------

function page(env: PaletteEnv) {
  return h(
    I18nextProvider,
    { i18n },
    h(
      TooltipProvider,
      null,
      h("div", null, h(CommandPalette, { env }), h(ContextMenuHost, { services: env.services })),
    ),
  );
}

async function opened(env: PaletteEnv) {
  render(page(env));
  act(() => openPalette());
  return screen.findByRole("combobox", { name: "Search commands" });
}

describe("the palette", () => {
  it("opens focused on its search field and lists the commands", async () => {
    const field = await opened(makeEnv(makeServices()));
    expect(screen.getByRole("dialog", { name: "Command palette" })).toBeTruthy();
    await waitFor(() => expect(document.activeElement).toBe(field));
    expect(screen.getAllByRole("option").length).toBeGreaterThan(20);
    expect(screen.getAllByRole("option").length).toBeLessThanOrEqual(MAX_ROWS);
  });

  it("draws at most MAX_ROWS rows and says how many more there are", async () => {
    const many: CatalogCommand[] = Array.from({ length: 150 }, (_, i) => ({
      ...FIXTURE,
      name: `bulk.command_${i}`,
      summary: `Bulk command ${i}`,
    }));
    await opened(
      makeEnv(makeServices(), {
        catalog: () => [...commandCatalog.getState().commands, ...many],
      }),
    );
    expect(screen.getAllByRole("option")).toHaveLength(MAX_ROWS);
    expect(screen.getByText(/\d+ more not shown/)).toBeTruthy();
  });

  it("runs a command with the keyboard alone: type, Enter, and the command is called", async () => {
    selection = ["itm_a"];
    const services = makeServices();
    const field = await opened(makeEnv(services));
    const user = userEvent.setup();
    await user.type(field, "delete clip");
    const options = screen.getAllByRole("option");
    expect(options[0]?.textContent).toContain("Delete clip");
    expect(field.getAttribute("aria-activedescendant")).toBe(options[0]?.id);
    await user.keyboard("{Enter}");
    await waitFor(() => expect(call).toHaveBeenCalledWith("item.delete", { items: ["itm_a"] }));
    expect(paletteStore.getState().open).toBe(false);
    expect(recentsStore.getState().keys[0]).toBe("item.delete");
  });

  it("moves with the arrow keys, wrapping, and shows the key bound to a command", async () => {
    const field = await opened(makeEnv(makeServices()));
    const user = userEvent.setup();
    await user.type(field, "split");
    const first = screen.getAllByRole("option")[0];
    expect(first?.textContent).toContain("S");
    await user.keyboard("{ArrowDown}");
    expect(field.getAttribute("aria-activedescendant")).toBe(screen.getAllByRole("option")[1]?.id);
    await user.keyboard("{ArrowUp}{ArrowUp}");
    const options = screen.getAllByRole("option");
    expect(field.getAttribute("aria-activedescendant")).toBe(options[options.length - 1]?.id);
  });

  it("runs an editor-side action registered for a registry command (Split at playhead)", async () => {
    const split = vi.fn();
    const off = registerPaletteAction("clip.split", split);
    const field = await opened(makeEnv(makeServices()));
    const user = userEvent.setup();
    await user.type(field, "split at");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(split).toHaveBeenCalledTimes(1));
    expect(call).not.toHaveBeenCalled();
    off();
  });

  it("shows an empty state, and Esc clears the text and then closes", async () => {
    const field = await opened(makeEnv(makeServices()));
    const user = userEvent.setup();
    await user.type(field, "qqqzzz");
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText(/Nothing matches/)).toBeTruthy();
    await user.keyboard("{Escape}");
    expect((field as HTMLInputElement).value).toBe("");
    expect(paletteStore.getState().open).toBe(true);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(paletteStore.getState().open).toBe(false));
  });

  it("lists recent commands first, under their own heading", async () => {
    const services = makeServices();
    const env = makeEnv(services);
    const picked = buildEntries(env).find((e) => e.command === "edit.undo");
    recentsStore.getState().push(picked?.key ?? "");
    await opened(env);
    expect(screen.getByRole("group", { name: "Recent" })).toBeTruthy();
    expect(screen.getAllByRole("option")[0]?.textContent).toContain("Undo");
  });

  it("asks for what the context cannot answer in a generated form, then runs with the typed values", async () => {
    selection = [];
    const services = makeServices();
    const field = await opened(makeEnv(services));
    const user = userEvent.setup();
    await user.type(field, "split a clip");
    // Without a registered action, `clip.split` opens its form: `at` comes from the playhead, `item` is asked.
    const split = buildEntries(makeEnv(services)).find((e) => e.command === "clip.split");
    expect(split?.schema).toBeDefined();
    await user.type(field, "{Enter}");
    const item = await screen.findByLabelText("Item");
    const at = screen.getByLabelText("At") as HTMLInputElement;
    expect(at.value).toBe("1234");
    await user.click(screen.getByRole("button", { name: "Run" }));
    expect((await screen.findAllByRole("alert"))[0]?.textContent).toBe("This is required.");
    expect(call).not.toHaveBeenCalled();
    await user.type(item, "itm_0123");
    await user.click(screen.getByRole("button", { name: "Run" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("clip.split", { item: "itm_0123", at: 1234 }),
    );
  });

  it("goes back from the form with Esc, keeping the palette open", async () => {
    const field = await opened(makeEnv(makeServices()));
    const user = userEvent.setup();
    await user.type(field, "split a clip");
    await user.keyboard("{Enter}");
    await screen.findByLabelText("Item");
    await user.keyboard("{Escape}");
    await screen.findByRole("combobox", { name: "Search commands" });
    expect(paletteStore.getState().open).toBe(true);
  });

  it("does not run a disabled row and says why", async () => {
    const services = makeServices({
      baseKeys: () => ({ "engine.live": true, "project.readOnly": true }),
    });
    keys = () => ({ "engine.live": true, "project.readOnly": true });
    const field = await opened(makeEnv(services));
    const user = userEvent.setup();
    await user.type(field, "add track");
    const row = screen.getAllByRole("option")[0] as HTMLElement;
    expect(row.getAttribute("aria-disabled")).toBe("true");
    expect(row.textContent).toContain("read-only");
    await user.keyboard("{Enter}");
    expect(paletteStore.getState().open).toBe(true);
    expect(call).not.toHaveBeenCalled();
  });

  it("opens a settings row through the app's own command", async () => {
    const services = makeServices();
    const env = makeEnv(services);
    const field = await opened(env);
    const user = userEvent.setup();
    const row = buildEntries(env).find((e) => e.kind === "settings" && e.args?.row !== undefined);
    await user.type(field, row?.title ?? "");
    await user.keyboard("{Enter}");
    const { settingsStore } = await import("../settings/settingsStore.ts");
    await waitFor(() => expect(settingsStore.getState().open).toBe(true));
    expect(settingsStore.getState().target).toEqual({ page: row?.args?.page, row: row?.args?.row });
    settingsStore.getState().setOpen(false);
  });

  it("reports a failing command with the app's toast", async () => {
    selection = ["itm_a"];
    const services = makeServices();
    call.mockRejectedValueOnce(new Error("locked"));
    const field = await opened(makeEnv(services));
    const user = userEvent.setup();
    await user.type(field, "delete clip");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(report).toHaveBeenCalledTimes(1));
  });
});

describe("a palette row's menu", () => {
  it("has Run, Copy command id and Assign shortcut…", async () => {
    const services = makeServices();
    const env = makeEnv(services);
    const field = await opened(env);
    await userEvent.setup().type(field, "undo");
    fireEvent.contextMenu(screen.getAllByRole("option")[0] as HTMLElement);
    const menu = await screen.findByRole("menu", { name: "Actions for Undo" });
    expect(
      within(menu)
        .getAllByRole("menuitem")
        .map((i) => i.textContent),
    ).toEqual(["Run", "Copy command id", "Assign shortcut…"]);
  });

  it("Run runs the row", async () => {
    const services = makeServices();
    const field = await opened(makeEnv(services));
    await userEvent.setup().type(field, "undo");
    fireEvent.contextMenu(screen.getAllByRole("option")[0] as HTMLElement);
    const menu = await screen.findByRole("menu");
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Run" }));
    await waitFor(() => expect(call).toHaveBeenCalledWith("edit.undo", {}));
  });

  it("Copy command id writes the id to the clipboard", async () => {
    const writeText = vi.fn(async () => {});
    const services = makeServices();
    const field = await opened(makeEnv(services));
    const user = userEvent.setup(); // installs its own clipboard stub: replace it afterwards
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await user.type(field, "undo");
    fireEvent.contextMenu(screen.getAllByRole("option")[0] as HTMLElement);
    const menu = await screen.findByRole("menu");
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy command id" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("edit.undo"));
  });

  it("is a surface with a table: opening it by hand builds the same rows", () => {
    const services = makeServices();
    const ok = openContextMenu(
      {
        surface: "palette.row",
        point: { x: 1, y: 1 },
        target: { name: "X", data: { key: "k", command: "c" } },
      },
      services,
    );
    expect(ok).toBe(true);
    expect(contextMenuStore.getState().open?.menu.entries).toHaveLength(4); // 3 rows and a separator
  });
});

// ---- accessibility ----------------------------------------------------------------------------------

describe("accessibility", () => {
  async function violations(): Promise<string[]> {
    const results = await axe.run(document.body, {
      rules: { "color-contrast": { enabled: false }, region: { enabled: false } },
    });
    return results.violations.map((v) => `${v.id}: ${v.nodes[0]?.html}`);
  }

  it("has no axe violations in the list", async () => {
    selection = ["itm_a"];
    await opened(makeEnv(makeServices()));
    expect(await violations()).toEqual([]);
  });

  it("has no axe violations with a query, an empty result and a disabled row", async () => {
    const services = makeServices({
      baseKeys: () => ({ "engine.live": true, "project.readOnly": true }),
    });
    keys = () => ({ "engine.live": true, "project.readOnly": true });
    const field = await opened(makeEnv(services));
    const user = userEvent.setup();
    await user.type(field, "track");
    expect(await violations()).toEqual([]);
    await user.clear(field);
    await user.type(field, "qqqzzz");
    expect(await violations()).toEqual([]);
  });

  it("has no axe violations in the argument form, with its errors showing", async () => {
    const field = await opened(makeEnv(makeServices()));
    const user = userEvent.setup();
    await user.type(field, "split a clip");
    await user.keyboard("{Enter}");
    await screen.findByLabelText("Item");
    await user.click(screen.getByRole("button", { name: "Run" }));
    await screen.findAllByRole("alert");
    expect(await violations()).toEqual([]);
  });

  it("has no axe violations with a row's menu open", async () => {
    const field = await opened(makeEnv(makeServices()));
    await userEvent.setup().type(field, "undo");
    fireEvent.contextMenu(screen.getAllByRole("option")[0] as HTMLElement);
    await screen.findByRole("menu");
    expect(await violations()).toEqual([]);
  });
});

describe("the planner", () => {
  it("runs a command with no required arguments at once, and one the editor carries out itself", () => {
    const services = makeServices();
    const env = makeEnv(services);
    const entries = buildEntries(env);
    const undo = entries.find((e) => e.command === "edit.undo");
    expect(undo && planFor(undo, env).kind).toBe("run");
    const add = entries.find((e) => e.command === "track.add");
    expect(add && planFor(add, env).kind).toBe("form");
    const split = entries.find((e) => e.command === "clip.split");
    expect(split && planFor(split, env).kind).toBe("form");
    const off = registerPaletteAction("clip.split", () => {});
    expect(split && planFor(split, env).kind).toBe("run");
    off();
  });
});
