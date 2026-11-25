// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2H.12 acceptance (Vitest part): a command with menu and `when` hints appears in its surface
// with no UI code change; the walk test over every surface table; the target rules; the
// contribution registry's limits; the one-mutation-per-choice guard.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { MenuEntry, MenuItem, MenuSubmenu } from "@cutweave/ui";
import { describe, expect, it, vi } from "vitest";
import { registerHistoryCommands } from "../history/commands.ts";
import { i18n, type TKey } from "../i18n/index.ts";
import en from "../i18n/locales/en.json";
import { registerPaletteCommands } from "../palette/commands.ts";
import { registerShortcutRowMenu } from "../settings/shortcutRowMenu.ts";
import { listBindings } from "../shortcuts/bindings.ts";
import { buildMenu } from "./build.ts";
import { registerBuiltinMenus } from "./builtins.ts";
import { guardedCall, MultipleMutationsError } from "./commands.ts";
import { hintBindingIds, hintedCommands } from "./keys.ts";
import { MAX_PLUGIN_ITEMS, plainLabel } from "./registry.ts";
import { createMenuServices, type MenuServices } from "./services.ts";
import { registerSurfaceMenus } from "./surfaces/index.ts";
import { resolveTarget } from "./target.ts";
import type { MenuRow, MenuSurfaceDef, SelectionAdapter } from "./types.ts";

// ---- helpers ------------------------------------------------------------------------------------

const catalog = JSON.parse(
  readFileSync(join(import.meta.dirname, "../../../../schema/commands.json"), "utf8"),
) as {
  commands: {
    name: string;
    undoable: boolean;
    annotations: { destructiveHint: boolean; readOnlyHint: boolean };
  }[];
};
const byName = new Map(catalog.commands.map((c) => [c.name, c]));

/** Whether the `en` catalog has `key`, or the plural forms of it. */
function hasKey(key: string): boolean {
  let node: unknown = en;
  for (const part of key.split(".")) {
    if (typeof node !== "object" || node === null) return false;
    node = (node as Record<string, unknown>)[part];
  }
  return typeof node === "string";
}

const keyOf = (value: unknown): string[] => (typeof value === "string" ? [value] : []);

function fixtureServices(overrides: Partial<MenuServices> = {}): MenuServices {
  const services = createMenuServices({
    call: vi.fn(async () => ({})),
    keysFor: () => undefined,
    baseKeys: () => ({ "engine.live": true, "project.readOnly": false }),
    playhead: () => 0,
    t: (spec) => i18n.t(spec.key, spec.vars ?? {}) as string,
    now: () => 0,
    report: vi.fn(),
    ...overrides,
  });
  registerBuiltinMenus(services);
  registerSurfaceMenus(services); // WP-2H.13: the walk below covers today's surfaces too
  registerPaletteCommands(services);
  registerHistoryCommands(services);
  registerShortcutRowMenu(services, { reset: () => undefined, remove: () => undefined });
  return services;
}

const items = (entries: readonly MenuEntry[]): MenuItem[] =>
  entries.filter((e): e is MenuItem => e.kind === undefined || e.kind === "item");

const FIXTURE_SURFACE: MenuSurfaceDef = {
  id: "fixture.surface",
  name: (target) => ({ key: "contextMenu.field.nameFor", vars: { name: target.name ?? "" } }),
  groups: ["edit"],
  rows: [],
};

// ---- a command with hints appears in its surface -----------------------------------------------

describe("a fixture command with menu and when hints", () => {
  it("appears in its surface, disabled with the reason when the context says so, with no UI code change", () => {
    const services = fixtureServices();
    services.surfaces.register(FIXTURE_SURFACE);
    // The "registry entry": how it reads, and where it goes. Nothing else in the editor changes.
    services.commands.register({ name: "fixture.do", label: "menu.paste" as TKey });
    services.contributions.register({
      surface: "fixture.surface",
      command: "fixture.do",
      group: "edit",
      order: 1,
      when: "selection.count == 1 && !track.locked",
      reason: "contextMenu.reason.noSelection",
    });

    const open = buildMenu(
      services,
      {
        surface: "fixture.surface",
        ids: ["itm_a"],
        name: "a.mov",
        keys: { "track.locked": false },
      },
      () => {},
    );
    expect(open?.name).toBe("Text actions for a.mov");
    const [row] = items(open?.entries ?? []);
    expect(row?.label).toBe("Paste");
    expect(row?.disabled).toBeUndefined();

    const locked = buildMenu(
      services,
      { surface: "fixture.surface", ids: ["itm_a"], keys: { "track.locked": true } },
      () => {},
    );
    const [disabled] = items(locked?.entries ?? []);
    expect(disabled?.disabled).toBe(true);
    expect(disabled?.disabledReason).toBe("Nothing is selected");

    const several = buildMenu(services, { surface: "fixture.surface", ids: ["a", "b"] }, () => {});
    expect(items(several?.entries ?? [])[0]?.disabled).toBe(true);
  });

  it("leaves out a row whose command nothing registered, and a row that does not apply", () => {
    const services = fixtureServices();
    services.surfaces.register({
      ...FIXTURE_SURFACE,
      rows: [
        { command: "not.registered", group: "edit", order: 0 },
        { command: "edit.undo", group: "edit", order: 1, visible: "selection.kind == audio" },
        { command: "edit.redo", group: "edit", order: 2, visible: "selection.kind == audio" },
      ],
    });
    const audio = buildMenu(
      services,
      { surface: "fixture.surface", ids: ["a"], keys: { "selection.kind": "audio" } },
      () => {},
    );
    expect(items(audio?.entries ?? []).map((i) => i.id)).toEqual(["edit.undo", "edit.redo"]);
    const video = buildMenu(
      services,
      { surface: "fixture.surface", ids: ["a"], keys: { "selection.kind": "video" } },
      () => {},
    );
    expect(video).toBeNull();
  });

  it("disables a document-changing command in a read-only project or without an engine", () => {
    const readOnly = fixtureServices({
      baseKeys: () => ({ "engine.live": true, "project.readOnly": true }),
    });
    const surface: MenuSurfaceDef = {
      ...FIXTURE_SURFACE,
      rows: [
        { command: "clip.split", group: "edit", order: 0 },
        { command: "view.set_playhead", group: "edit", order: 1 },
      ],
    };
    readOnly.surfaces.register(surface);
    const a = items(
      buildMenu(readOnly, { surface: "fixture.surface", ids: [] }, () => {})?.entries ?? [],
    );
    expect(a[0]?.disabled).toBe(true);
    expect(a[0]?.disabledReason).toBe("The project is read-only");
    expect(a[1]?.disabled).toBeUndefined(); // view state is not an edit

    const noEngine = fixtureServices({
      baseKeys: () => ({ "engine.live": false, "project.readOnly": false }),
    });
    noEngine.surfaces.register(surface);
    const b = items(
      buildMenu(noEngine, { surface: "fixture.surface", ids: [] }, () => {})?.entries ?? [],
    );
    expect(b[0]?.disabledReason).toBe("The engine is not running");
  });

  it("names the object: 'Delete clip' and 'Delete 3 clips'", () => {
    const services = fixtureServices();
    services.surfaces.register({
      ...FIXTURE_SURFACE,
      rows: [{ command: "item.delete", group: "edit", order: 0 }],
    });
    const one = buildMenu(services, { surface: "fixture.surface", ids: ["a"] }, () => {});
    expect(items(one?.entries ?? [])[0]?.label).toBe("Delete clip");
    expect(items(one?.entries ?? [])[0]?.variant).toBe("danger");
    const three = buildMenu(
      services,
      { surface: "fixture.surface", ids: ["a", "b", "c"] },
      () => {},
    );
    expect(items(three?.entries ?? [])[0]?.label).toBe("Delete 3 clips");
  });

  it("runs the chosen row's command with its arguments from the context, once", async () => {
    const call = vi.fn(async () => ({}));
    const services = fixtureServices({ call });
    services.surfaces.register({
      ...FIXTURE_SURFACE,
      rows: [
        {
          command: "clip.split",
          group: "edit",
          order: 0,
          args: (ctx) => ({ item: ctx.ids[0], at: ctx.playhead }),
        },
      ],
    });
    let chosen: (() => void | Promise<void>) | null = null;
    const open = buildMenu(services, { surface: "fixture.surface", ids: ["itm_a"] }, (run) => {
      chosen = run;
    });
    items(open?.entries ?? [])[0]?.onSelect?.();
    expect(call).not.toHaveBeenCalled(); // the host runs it once the menu has closed
    await (chosen as (() => void | Promise<void>) | null)?.();
    expect(call).toHaveBeenCalledWith("clip.split", { item: "itm_a", at: 0 });
  });
});

// ---- the walk test --------------------------------------------------------------------------

describe("surface-table walk", () => {
  /** Every row of every table, including contributions and submenus. */
  function rowsOf(services: MenuServices): { surface: string; row: MenuRow }[] {
    const out: { surface: string; row: MenuRow }[] = [];
    const walk = (surface: string, rows: readonly MenuRow[]): void => {
      for (const row of rows) {
        out.push({ surface, row });
        if (row.submenu !== undefined) walk(surface, row.submenu.rows);
      }
    };
    for (const def of services.surfaces.list()) walk(def.id, def.rows);
    return out;
  }

  const services = fixtureServices();

  it("finds every row's command in the catalog's command info or the UI command table", () => {
    const rows = rowsOf(services).filter(({ row }) => row.command !== undefined);
    expect(rows.length).toBeGreaterThan(0);
    for (const { surface, row } of rows) {
      const command = row.command as string;
      const known =
        services.commands.get(command) !== undefined || services.ui.get(command) !== undefined;
      expect(known, `${surface}: ${command}`).toBe(true);
    }
  });

  it("holds the command info to the generated catalog", () => {
    for (const info of services.commands.list()) {
      const entry = byName.get(info.name);
      expect(entry, `${info.name} is in schema/commands.json`).toBeDefined();
      expect(info.undoable ?? false, `${info.name} undoable`).toBe(entry?.undoable);
      expect(info.destructive ?? false, `${info.name} destructive`).toBe(
        entry?.annotations.destructiveHint,
      );
    }
  });

  it("finds every label key in the en catalog", () => {
    const keys: { where: string; key: string }[] = [];
    for (const info of services.commands.list()) {
      keys.push({ where: info.name, key: info.label });
      if (info.labelMany !== undefined) keys.push({ where: info.name, key: info.labelMany });
    }
    for (const ui of services.ui.list()) keys.push({ where: ui.id, key: ui.label });
    for (const { surface, row } of rowsOf(services)) {
      if (typeof row.label === "object") keys.push({ where: surface, key: row.label.key });
      if (typeof row.reason === "string") keys.push({ where: surface, key: row.reason });
      if (row.submenu !== undefined) keys.push({ where: surface, key: row.submenu.label.key });
    }
    for (const def of services.surfaces.list()) {
      keys.push({ where: def.id, key: def.name({ surface: def.id, ids: [], name: "x" }).key });
    }
    for (const { where, key } of keys) expect(hasKey(key), `${where}: ${key}`).toBe(true);
  });

  it("has no literal shortcut text in a row, and every hint names a real binding", () => {
    for (const { row, surface } of rowsOf(services)) {
      expect("shortcut" in row, `${surface} row carries a shortcut literal`).toBe(false);
      expect(keyOf((row as { shortcut?: unknown }).shortcut)).toEqual([]);
    }
    const bindings = new Set(listBindings("windows", { dev: true }).map((b) => b.id));
    for (const id of hintBindingIds()) expect(bindings.has(id), id).toBe(true);
    // Every hinted command is one a menu can name.
    for (const command of hintedCommands()) {
      const known =
        services.commands.get(command) !== undefined || services.ui.get(command) !== undefined;
      expect(known, command).toBe(true);
    }
  });

  it("requires every host UI command to be on the exemption list, with a reason", () => {
    const listed = JSON.parse(
      readFileSync(join(import.meta.dirname, "host-actions-exempt.json"), "utf8"),
    ) as { exempt: { id: string; reason: string }[] };
    const hostIds = services.ui
      .list()
      .filter((c) => c.host === true)
      .map((c) => c.id)
      .sort();
    expect(listed.exempt.map((e) => e.id).sort()).toEqual(hostIds);
    for (const e of listed.exempt) expect(e.reason.length, e.id).toBeGreaterThan(20);
  });

  it("rejects a closure that would apply two mutating commands (one composite is required)", async () => {
    const real = vi.fn(async (_name: string, _args?: unknown) => ({}));
    const call = guardedCall(real);
    await call("view.select", { selection: [] }); // view state is free
    await call("timeline.get"); // reads are free
    await call("clip.split", {});
    await expect(call("item.delete", {})).rejects.toBeInstanceOf(MultipleMutationsError);
    expect(real.mock.calls.map((c) => c[0])).toEqual(["view.select", "timeline.get", "clip.split"]);
  });

  it("applies the guard to a UI command's run (a choice is one undo step)", async () => {
    const real = vi.fn(async (_name: string, _args?: unknown) => ({}));
    const svc = fixtureServices({ call: real });
    svc.ui.register({
      id: "fixture.two_edits",
      label: "menu.paste" as TKey,
      async run(_args, ctx) {
        await ctx.call("clip.split", {});
        await ctx.call("item.delete", {});
      },
    });
    svc.surfaces.register({
      ...FIXTURE_SURFACE,
      rows: [{ command: "fixture.two_edits", group: "edit", order: 0 }],
    });
    let chosen: (() => void | Promise<void>) | null = null;
    const open = buildMenu(svc, { surface: "fixture.surface", ids: [] }, (run) => {
      chosen = run;
    });
    items(open?.entries ?? [])[0]?.onSelect?.();
    await expect((chosen as unknown as () => Promise<void>)()).rejects.toBeInstanceOf(
      MultipleMutationsError,
    );
    expect(real).toHaveBeenCalledTimes(1);
  });
});

// ---- target rules -----------------------------------------------------------------------------

describe("target rules", () => {
  function adapter(initial: string[], empty: "keep" | "clear") {
    let selected = [...initial];
    const set = vi.fn((ids: readonly string[]) => {
      selected = [...ids];
    });
    const value: SelectionAdapter = { current: () => selected, set, empty };
    return { value, set, selected: () => selected };
  }
  const hit = (...ids: string[]) => ({ surface: "timeline.clip", ids });

  it("selects an unselected item first, and the menu acts on it", () => {
    const a = adapter(["itm_x"], "keep");
    const r = resolveTarget(hit("itm_a"), a.value);
    expect(r.select).toEqual(["itm_a"]);
    expect(r.target.ids).toEqual(["itm_a"]);
  });

  it("keeps the selection on a selected item, and acts on all of it", () => {
    const a = adapter(["itm_a", "itm_b", "itm_c"], "keep");
    const r = resolveTarget(hit("itm_b"), a.value);
    expect(r.select).toBeNull();
    expect(r.target.ids).toEqual(["itm_a", "itm_b", "itm_c"]);
  });

  it("acts on a single selected item as itself", () => {
    const a = adapter(["itm_a"], "keep");
    const r = resolveTarget(hit("itm_a"), a.value);
    expect(r.select).toBeNull();
    expect(r.target.ids).toEqual(["itm_a"]);
  });

  it("keeps the selection on an empty timeline lane", () => {
    const a = adapter(["itm_a", "itm_b"], "keep");
    const r = resolveTarget({ surface: "timeline.lane", ids: [] }, a.value);
    expect(r.select).toBeNull();
    expect(r.target.ids).toEqual([]);
  });

  it("clears the selection on an empty bin, and does nothing when it is already empty", () => {
    const some = adapter(["med_a"], "clear");
    expect(resolveTarget({ surface: "bin.background", ids: [] }, some.value).select).toEqual([]);
    const none = adapter([], "clear");
    expect(resolveTarget({ surface: "bin.background", ids: [] }, none.value).select).toBeNull();
  });

  it("passes a surface with no selection through", () => {
    expect(resolveTarget(hit("itm_a"), undefined)).toEqual({ target: hit("itm_a"), select: null });
  });

  it("disables editing rows on a locked track with the reason, from the surface's keys", () => {
    const services = fixtureServices();
    services.surfaces.register({
      ...FIXTURE_SURFACE,
      keys: (target) => ({ "track.locked": target.place?.trackId === "trk_locked" }),
      rows: [
        {
          command: "clip.split",
          group: "edit",
          order: 0,
          when: "!track.locked",
          reason: "contextMenu.reason.noSelection",
        },
      ],
    });
    const open = (trackId: string) =>
      items(
        buildMenu(
          services,
          { surface: "fixture.surface", ids: ["a"], place: { trackId } },
          () => {},
        )?.entries ?? [],
      )[0];
    expect(open("trk_free")?.disabled).toBeUndefined();
    expect(open("trk_locked")?.disabled).toBe(true);
    expect(open("trk_locked")?.disabledReason).toBe("Nothing is selected");
  });
});

// ---- the contribution registry -----------------------------------------------------------------

describe("contribution registry", () => {
  const surface: MenuSurfaceDef = {
    ...FIXTURE_SURFACE,
    groups: ["edit"],
    rows: [{ command: "edit.undo", group: "edit", order: 0 }],
  };

  function withCommands(count: number) {
    const services = fixtureServices();
    services.surfaces.register(surface);
    for (let i = 0; i < count; i++) {
      services.commands.register({ name: `plug.cmd${i}`, label: "menu.paste" as TKey });
    }
    return services;
  }
  const plugin = { id: "acme", name: "Acme Tools" };
  const open = (services: MenuServices) =>
    buildMenu(services, { surface: "fixture.surface", ids: [] }, () => {});

  it("puts a plugin's items in a trailing group, three at most, then a submenu named for it", () => {
    const services = withCommands(5);
    for (let i = 4; i >= 0; i--) {
      services.contributions.register({
        surface: "fixture.surface",
        command: `plug.cmd${i}`,
        order: i,
        plugin,
        label: `Acme ${i}`,
      });
    }
    const entries = open(services)?.entries ?? [];
    const labels = entries.map((e) => (e.kind === "separator" ? "---" : (e as MenuItem).label));
    expect(labels).toEqual(["Undo", "---", "Acme 0", "Acme 1", "Acme 2", "Acme Tools"]);
    expect(MAX_PLUGIN_ITEMS).toBe(3);
    const more = entries.at(-1) as MenuSubmenu;
    expect(more.kind).toBe("submenu");
    expect(more.items.map((i) => (i as MenuItem).label)).toEqual(["Acme 3", "Acme 4"]);
  });

  it("clamps order, and keeps first-party contributions in the groups they name", () => {
    const services = withCommands(2);
    services.contributions.register({
      surface: "fixture.surface",
      command: "plug.cmd0",
      group: "edit",
      order: 9999,
    });
    services.contributions.register({
      surface: "fixture.surface",
      command: "plug.cmd1",
      group: "edit",
      order: -5,
    });
    expect(services.contributions.forSurface("fixture.surface").map((c) => c.order)).toEqual([
      0, 999,
    ]);
    const labels = items(open(services)?.entries ?? []).map((i) => i.id);
    expect(labels).toEqual(["edit.undo", "plug.cmd1", "plug.cmd0"]); // the table's row at order 0 first (stable)
  });

  it("makes a plugin's label plain text", () => {
    expect(plainLabel("<b>Make</b> <script>x</script>it\n\tnow")).toBe("Make xit now");
    expect(plainLabel("x".repeat(100))).toHaveLength(48);
    const services = withCommands(1);
    services.contributions.register({
      surface: "fixture.surface",
      command: "plug.cmd0",
      order: 0,
      plugin,
      label: "<img src=x onerror=alert(1)>Do it",
    });
    const labels = items(open(services)?.entries ?? []).map((i) => i.label);
    expect(labels).toContain("Do it");
  });

  it("shows only commands the plugin may run", () => {
    const services = withCommands(2);
    services.contributions.setPermit((id, command) => id === "acme" && command === "plug.cmd0");
    for (const command of ["plug.cmd0", "plug.cmd1"]) {
      services.contributions.register({ surface: "fixture.surface", command, order: 0, plugin });
    }
    const ids = items(open(services)?.entries ?? []).map((i) => i.id);
    expect(ids).toEqual(["edit.undo", "plug.cmd0"]);
  });

  it("unregisters", () => {
    const services = withCommands(1);
    const off = services.contributions.register({
      surface: "fixture.surface",
      command: "plug.cmd0",
      order: 0,
    });
    expect(services.contributions.forSurface("fixture.surface")).toHaveLength(1);
    off();
    expect(services.contributions.forSurface("fixture.surface")).toHaveLength(0);
  });
});

// ---- a row can be a submenu or a check --------------------------------------------------------

describe("row kinds", () => {
  it("builds a submenu from its rows and drops it when none apply", () => {
    const services = fixtureServices();
    const move = (visible: string): MenuRow => ({
      id: "move",
      group: "edit",
      order: 0,
      submenu: {
        label: { key: "contextMenu.field.paste" },
        rows: [
          { command: "edit.undo", group: "x", order: 1, visible },
          { command: "edit.redo", group: "x", order: 0, visible },
        ],
      },
    });
    services.surfaces.register({ ...FIXTURE_SURFACE, rows: [move("true")] });
    const sub = buildMenu(services, { surface: "fixture.surface", ids: [] }, () => {})
      ?.entries[0] as MenuSubmenu;
    expect(sub.kind).toBe("submenu");
    expect(sub.items.map((i) => (i as MenuItem).id)).toEqual(["edit.redo", "edit.undo"]);
    services.surfaces.register({ ...FIXTURE_SURFACE, rows: [move("false")] });
    expect(buildMenu(services, { surface: "fixture.surface", ids: [] }, () => {})).toBeNull();
  });

  it("shows a check row's state and separates groups", () => {
    const services = fixtureServices();
    services.surfaces.register({
      ...FIXTURE_SURFACE,
      groups: ["a", "b"],
      rows: [
        { id: "mute", command: "track.set", group: "a", order: 0, checked: () => true },
        { command: "edit.undo", group: "b", order: 0 },
      ],
    });
    const entries =
      buildMenu(services, { surface: "fixture.surface", ids: [] }, () => {})?.entries ?? [];
    expect(entries.map((e) => e.kind ?? "item")).toEqual(["item", "separator", "item"]);
    expect((entries[0] as MenuItem).checked).toBe(true);
  });
});
