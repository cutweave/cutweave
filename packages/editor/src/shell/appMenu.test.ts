// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { i18n } from "../i18n/index.ts";
import { translator } from "../timeline/errors.ts";
import {
  type AppMenu,
  type AppMenuState,
  buildAppMenu,
  chordLabel,
  commandForKey,
  isEnabled,
  menuPlatform,
  type ShortcutEvent,
  shortcutLabel,
} from "./appMenu.ts";

const t = translator(i18n.t.bind(i18n));

const state = (over: Partial<AppMenuState> = {}): AppMenuState => ({
  platform: "windows",
  desktop: true,
  dev: false,
  live: true,
  readOnly: false,
  recovering: false,
  canUndo: true,
  canRedo: false,
  undoLabel: "Undo Split clip",
  redoLabel: "Redo",
  recent: [{ path: "C:/p/Film/Film.cutweave", label: "Film.cutweave" }],
  exportAvailable: false,
  panels: [
    { id: "assets", label: "Assets", checked: true },
    { id: "inspector", label: "Inspector", checked: false },
  ],
  ...over,
});

/** Each menu as `label: item [shortcut] (off)` lines, the way a person reads it. */
function outline(menus: readonly AppMenu[]): Record<string, string[]> {
  return Object.fromEntries(
    menus.map((menu) => [
      menu.label,
      menu.items.map((entry) => {
        if (entry.kind === "separator") return "---";
        if (entry.kind === "recent") {
          return `${entry.label} > ${entry.items.map((i) => i.label).join(", ") || entry.emptyLabel}${entry.enabled ? "" : " (off)"}`;
        }
        if (entry.kind === "panels") {
          return `${entry.label} > ${entry.items.map((i) => `${i.label}${i.checked ? " (checked)" : ""}`).join(", ")}`;
        }
        if (entry.kind === "workspace") return entry.label;
        return `${entry.label}${entry.shortcut ? ` [${entry.shortcut}]` : ""}${entry.enabled ? "" : " (off)"}`;
      }),
    ]),
  );
}

const key = (k: string, mods: Partial<ShortcutEvent> = {}): ShortcutEvent => ({
  key: k,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});

describe("the application menu (WP-1M.09)", () => {
  it("holds what the native menu had, plus Open recent, on Windows", () => {
    expect(outline(buildAppMenu(state(), t))).toEqual({
      File: [
        "New project… [Ctrl+N]",
        "Open project… [Ctrl+O]",
        "Open recent > Film.cutweave",
        "---",
        "Save [Ctrl+S]",
        "Save as… [Ctrl+Shift+S]",
        "---",
        "Export… [Ctrl+M] (off)",
        "Collect files…",
        "Archive project…",
        "---",
        "New sequence [Ctrl+Alt+N]",
        "Sequence settings…",
        "Project settings…",
        "Settings… [Ctrl+,]",
        "---",
        "Exit",
      ],
      Edit: [
        "Undo Split clip [Ctrl+Z]",
        "Redo [Ctrl+Y] (off)",
        "History",
        "---",
        "Cut [Ctrl+X]",
        "Copy [Ctrl+C]",
        "Paste [Ctrl+V]",
        "Select all [Ctrl+A]",
      ],
      View: [
        "Panels > Assets (checked), Inspector",
        "Reset layout",
        "Reset all workspaces",
        "---",
        "Next sequence [Ctrl+PageDown]",
        "Previous sequence [Ctrl+PageUp]",
        "---",
        "Toggle full screen [F11]",
      ],
      Help: ["About Cutweave"],
    });
  });

  it("adds Reload and developer tools in development builds only", () => {
    expect(outline(buildAppMenu(state({ dev: true }), t)).View).toEqual([
      "Panels > Assets (checked), Inspector",
      "Reset layout",
      "Reset all workspaces",
      "---",
      "Next sequence [Ctrl+PageDown]",
      "Previous sequence [Ctrl+PageUp]",
      "---",
      "Toggle full screen [F11]",
      "---",
      "Reload [Ctrl+R]",
      "Toggle developer tools [Ctrl+Shift+I]",
    ]);
    // Without the desktop app there is nothing to reload or inspect through main.
    expect(outline(buildAppMenu(state({ dev: true, desktop: false }), t)).View).toEqual([
      "Panels > Assets (checked), Inspector",
      "Reset layout",
      "Reset all workspaces",
      "---",
      "Next sequence [Ctrl+PageDown]",
      "Previous sequence [Ctrl+PageUp]",
      "---",
      "Toggle full screen [F11] (off)",
    ]);
  });

  it("is Quit with Ctrl+Q on Linux, like the native role", () => {
    const file = outline(buildAppMenu(state({ platform: "linux" }), t)).File;
    expect(file?.at(-1)).toBe("Quit [Ctrl+Q]");
  });

  it("disables project commands until the engine is live, and Save while read-only", () => {
    const off = outline(buildAppMenu(state({ live: false, canRedo: true, recent: [] }), t));
    expect(off.File).toEqual([
      "New project… [Ctrl+N] (off)",
      "Open project… [Ctrl+O] (off)",
      "Open recent > No recent projects yet (off)",
      "---",
      "Save [Ctrl+S] (off)",
      "Save as… [Ctrl+Shift+S] (off)",
      "---",
      "Export… [Ctrl+M] (off)",
      "Collect files… (off)",
      "Archive project… (off)",
      "---",
      "New sequence [Ctrl+Alt+N] (off)",
      "Sequence settings… (off)",
      "Project settings… (off)",
      "Settings… [Ctrl+,]",
      "---",
      "Exit",
    ]);
    expect(off.Edit?.slice(0, 2)).toEqual([
      "Undo Split clip [Ctrl+Z] (off)",
      "Redo [Ctrl+Y] (off)",
    ]);
    const readOnly = buildAppMenu(state({ readOnly: true }), t);
    expect(isEnabled(readOnly, "save")).toBe(false);
    expect(isEnabled(readOnly, "saveAs")).toBe(true);
    expect(isEnabled(readOnly, "undo")).toBe(false);
    const recovering = buildAppMenu(state({ recovering: true }), t);
    expect(isEnabled(recovering, "save")).toBe(false);
    expect(isEnabled(recovering, "saveAs")).toBe(false);
    expect(isEnabled(buildAppMenu(state({ desktop: false }), t), "quit")).toBe(false);
  });

  it("labels chords with Ctrl, and Cmd on macOS", () => {
    expect(chordLabel({ key: "S", mod: true, shift: true }, "windows")).toBe("Ctrl+Shift+S");
    expect(chordLabel({ key: "S", mod: true }, "mac")).toBe("⌘+S");
    expect(shortcutLabel("redo", "mac")).toBe("⌘+Shift+Z");
    expect(shortcutLabel("about", "windows")).toBeUndefined();
    expect(menuPlatform("Win32")).toBe("windows");
    expect(menuPlatform("MacIntel")).toBe("mac");
    expect(menuPlatform("Linux x86_64")).toBe("linux");
  });
});

describe("menu shortcuts", () => {
  const free = { typing: false, dev: false };
  const typing = { typing: true, dev: false };

  it("maps the File, Edit and View chords to their commands", () => {
    const on = (e: ShortcutEvent) => commandForKey(e, "windows", free);
    expect(on(key("n", { ctrlKey: true }))).toBe("new");
    expect(on(key("o", { ctrlKey: true }))).toBe("open");
    expect(on(key("s", { ctrlKey: true }))).toBe("save");
    expect(on(key("S", { ctrlKey: true, shiftKey: true }))).toBe("saveAs");
    expect(on(key("m", { ctrlKey: true }))).toBe("export");
    expect(on(key(",", { ctrlKey: true }))).toBe("settings");
    expect(on(key("z", { ctrlKey: true }))).toBe("undo");
    expect(on(key("y", { ctrlKey: true }))).toBe("redo");
    expect(on(key("Z", { ctrlKey: true, shiftKey: true }))).toBe("redo");
    expect(on(key("F11"))).toBe("toggleFullScreen");
  });

  it("leaves Ctrl+C, X, V and A to the browser and unrelated chords alone", () => {
    const on = (e: ShortcutEvent) => commandForKey(e, "windows", free);
    for (const k of ["c", "x", "v", "a"]) expect(on(key(k, { ctrlKey: true }))).toBeNull();
    expect(on(key("s"))).toBeNull();
    expect(on(key("s", { ctrlKey: true, altKey: true }))).toBeNull();
    expect(on(key("s", { metaKey: true }))).toBeNull();
    expect(on(key("F11", { ctrlKey: true }))).toBeNull();
    expect(on(key("q", { ctrlKey: true }))).toBeNull();
    expect(commandForKey(key("q", { ctrlKey: true }), "linux", free)).toBe("quit");
  });

  it("gives undo and redo to a focused text field; the rest still run from it", () => {
    expect(commandForKey(key("z", { ctrlKey: true }), "windows", typing)).toBeNull();
    expect(commandForKey(key("y", { ctrlKey: true }), "windows", typing)).toBeNull();
    expect(commandForKey(key("s", { ctrlKey: true }), "windows", typing)).toBe("save");
    expect(commandForKey(key("F11"), "windows", typing)).toBe("toggleFullScreen");
  });

  it("takes Reload and developer tools only in development builds", () => {
    const reload = key("r", { ctrlKey: true });
    const tools = key("I", { ctrlKey: true, shiftKey: true });
    expect(commandForKey(reload, "windows", free)).toBeNull();
    expect(commandForKey(tools, "windows", free)).toBeNull();
    expect(commandForKey(reload, "windows", { typing: false, dev: true })).toBe("reload");
    expect(commandForKey(tools, "windows", { typing: true, dev: true })).toBe("toggleDevTools");
  });

  it("acts once per press except undo and redo, which repeat while held", () => {
    expect(commandForKey(key("s", { ctrlKey: true, repeat: true }), "windows", free)).toBeNull();
    expect(commandForKey(key("F11", { repeat: true }), "windows", free)).toBeNull();
    expect(commandForKey(key("z", { ctrlKey: true, repeat: true }), "windows", free)).toBe("undo");
  });

  it("uses Cmd on macOS", () => {
    expect(commandForKey(key("s", { metaKey: true }), "mac", free)).toBe("save");
    expect(commandForKey(key("s", { ctrlKey: true }), "mac", free)).toBeNull();
    expect(commandForKey(key("Z", { metaKey: true, shiftKey: true }), "mac", free)).toBe("redo");
  });
});
