// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1M.13: the shortcut list is generated from the tables the handlers read, and a test walks
// each table, so a handler added without a binding (or a binding without a handler) fails here.
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { TIMELINE_KEYS, timelineKeyAction } from "@cutweave/timeline";
import { describe, expect, it } from "vitest";
import { i18n } from "../i18n/index.ts";
import { BIN_KEYS, binKeyAction } from "../panels/binKeys.ts";
import { SOURCE_KEYS, sourceKeyAction } from "../preview/sourceKeys.ts";
import {
  BROWSER_KEYS,
  buildAppMenu,
  type Chord,
  chordsFor,
  commandForKey,
  KEYED,
  type MenuPlatform,
} from "../shell/appMenu.ts";
import { keymap } from "../transport/keymap.ts";
import {
  BINDING_GROUPS,
  type Binding,
  bindingKeysText,
  comboLabel,
  comboParts,
  distinctCombos,
  type KeyCombo,
  listBindings,
  parseKeymapKey,
} from "./bindings.ts";

const PLATFORMS: readonly MenuPlatform[] = ["windows", "linux", "mac"];

const event = (
  key: string,
  mods: Partial<Record<"ctrlKey" | "metaKey" | "altKey" | "shiftKey", boolean>> = {},
) => ({
  key,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});

/** The key event a combo is pressed with on `platform`. */
const press = (combo: KeyCombo | Chord, platform: MenuPlatform) =>
  event(combo.key, {
    ...(combo.mod === true && (platform === "mac" ? { metaKey: true } : { ctrlKey: true })),
    ...(combo.shift === true && { shiftKey: true }),
  });

const ids = (bindings: readonly Binding[]) => bindings.map((b) => b.id);

describe("listBindings", () => {
  it("lists every binding once, with a label and a description in the catalog", () => {
    for (const platform of PLATFORMS) {
      const all = listBindings(platform, { dev: true });
      expect(new Set(ids(all)).size, platform).toBe(all.length);
      for (const binding of all) {
        expect(i18n.exists(binding.label), binding.id).toBe(true);
        expect(i18n.exists(binding.description), binding.id).toBe(true);
        expect(binding.combos.length, binding.id).toBeGreaterThan(0);
      }
    }
  });

  it("belongs to the known groups, in their order", () => {
    const order = BINDING_GROUPS.map((g) => g.id);
    const used = listBindings("windows").map((b) => order.indexOf(b.group));
    expect(used.every((i) => i >= 0)).toBe(true);
    expect(used).toEqual([...used].sort((a, b) => a - b));
    for (const g of BINDING_GROUPS) expect(i18n.exists(g.title), g.id).toBe(true);
  });

  it("leaves the development-only keys out of a packaged build", () => {
    expect(ids(listBindings("windows"))).not.toContain("menu.reload");
    expect(ids(listBindings("windows", { dev: true }))).toContain("menu.reload");
  });
});

describe("the menu bar's keys", () => {
  it("lists every command the key handler runs, with every chord it matches", () => {
    for (const platform of PLATFORMS) {
      const bindings = listBindings(platform, { dev: true });
      for (const command of [...KEYED, ...BROWSER_KEYS]) {
        const chords = chordsFor(command, platform);
        const binding = bindings.find((b) => b.id === `menu.${command}`);
        if (chords.length === 0) {
          expect(binding, `${platform} ${command}`).toBeUndefined();
          continue;
        }
        expect(binding?.combos.length, `${platform} ${command}`).toBe(chords.length);
      }
    }
  });

  it("is what the handler matches: pressing a listed chord runs its command", () => {
    for (const platform of PLATFORMS) {
      for (const command of KEYED) {
        for (const chord of chordsFor(command, platform)) {
          const ran = commandForKey(press(chord, platform), platform, { typing: false, dev: true });
          expect(ran, `${platform} ${command} ${chord.key}`).toBe(command);
        }
      }
    }
  });

  it("covers every shortcut the menus show", () => {
    for (const platform of PLATFORMS) {
      const menus = buildAppMenu(
        {
          platform,
          desktop: true,
          dev: true,
          live: true,
          readOnly: false,
          recovering: false,
          canUndo: true,
          canRedo: true,
          undoLabel: "Undo",
          redoLabel: "Redo",
          recent: [],
          panels: [],
          exportAvailable: true,
        },
        (key) => key,
      );
      const listed = new Set(ids(listBindings(platform, { dev: true })));
      for (const menu of menus) {
        for (const entry of menu.items) {
          if (entry.kind === "command" && entry.shortcut !== undefined) {
            expect(listed.has(`menu.${entry.command}`), `${platform} ${entry.command}`).toBe(true);
          }
        }
      }
    }
  });
});

describe("the playback keys", () => {
  it("lists every key of the keymap, and nothing else", () => {
    const listed = listBindings("windows")
      .filter((b) => b.group === "playback" && b.id.startsWith("playback."))
      .flatMap((b) => b.combos.map((c) => c.key))
      .sort();
    const keymapKeys = keymap
      .entries()
      .map(([spelled]) => parseKeymapKey(spelled).key)
      .sort();
    expect(listed).toEqual(keymapKeys);
  });

  it("lists the Source view's keys (WP-1M.15) with the playback keys", () => {
    const listed = listBindings("windows")
      .filter((b) => b.id.startsWith("source."))
      .flatMap((b) => b.combos.map((c) => c.key))
      .sort();
    expect(listed).toEqual(SOURCE_KEYS.map((entry) => entry.key).sort());
    expect(sourceKeyAction(event("Escape"))).toBe("back");
    expect(sourceKeyAction(event("Enter"))).toBe("insert");
    expect(sourceKeyAction(event("Enter", { ctrlKey: true }))).toBeNull();
    expect(sourceKeyAction(event("a"))).toBeNull();
  });

  it("lists the bin's play key (Space selects the card and plays it)", () => {
    const play = BIN_KEYS.find((entry) => entry.action === "play");
    expect(play?.key).toBe(" ");
    const listed = listBindings("windows").find((b) => b.id === "bin.play");
    expect(listed?.combos).toEqual([{ key: " " }]);
    expect(binKeyAction(event(" "))).toBe("play");
  });

  it("is what the keymap matches: every listed key finds its action", () => {
    for (const [spelled] of keymap.entries()) {
      const combo = parseKeymapKey(spelled);
      const key = combo.key === "Space" ? " " : combo.key;
      expect(
        keymap.actionFor({ ...event(key), repeat: false, defaultPrevented: false, target: null }),
        spelled,
      ).not.toBeNull();
    }
  });
});

describe("the timeline's keys", () => {
  it("lists every key of the interaction table", () => {
    const listed = listBindings("windows").filter((b) => b.group === "timeline");
    expect(listed.map((b) => b.id).sort()).toEqual(
      TIMELINE_KEYS.map((e) => `timeline.${e.action}`).sort(),
    );
    for (const entry of TIMELINE_KEYS) {
      const binding = listed.find((b) => b.id === `timeline.${entry.action}`);
      expect(binding?.combos.map((c) => c.key)).toEqual(entry.keys);
      for (const key of entry.keys) {
        expect(timelineKeyAction(key), key).toBe(entry.action);
        // A letter works in either case.
        if (key.length === 1) {
          expect(timelineKeyAction(key.toUpperCase()), key).toBe(entry.action);
        }
      }
    }
  });
});

describe("the media bin's keys", () => {
  it("lists every key of the bin's table", () => {
    const listed = listBindings("windows")
      .filter((b) => b.group === "bin")
      .flatMap((b) => b.combos.map((c) => `${c.mod === true ? "mod+" : ""}${c.key}`))
      .sort();
    const table = BIN_KEYS.map((e) => `${e.mod === true ? "mod+" : ""}${e.key}`).sort();
    expect(listed).toEqual(table);
  });

  it("is what the panel matches: pressing a listed key does its action", () => {
    for (const entry of BIN_KEYS) {
      const e = event(entry.key, entry.mod === true ? { ctrlKey: true } : {});
      expect(binKeyAction(e), entry.key).toBe(entry.action);
    }
    expect(binKeyAction(event("i"))).toBeNull();
    expect(binKeyAction(event("i", { ctrlKey: true, shiftKey: true }))).toBeNull();
    expect(binKeyAction(event("ArrowLeft", { ctrlKey: true }))).toBeNull();
  });
});

describe("every key handler in the editor is accounted for", () => {
  // A file with a `keydown` listener or an `onKeyDown` prop is a key handler. Each one is either
  // generated into the list above or a text field's own keys, named here with the reason. A new
  // handler fails this test until it is listed in bindings.ts or named here.
  const KNOWN: Readonly<Record<string, string>> = {
    "inspector/MediaPreview.tsx":
      "a slider's own keys while it has the focus: arrows step a frame, Home, End, Page Up and Down",
    "menus/host.tsx":
      "Shift+F10 and the Menu key open the context menu of the focused surface: the platform's own keys",
    "palette/CommandPalette.tsx":
      "the palette's search field: Up, Down, Page Up, Page Down and Enter move through and run its list",
    "palette/shortcut.ts": "Ctrl+K opens the command palette: PALETTE_COMBO (menu group)",
    "panels/InspectorPanel.tsx":
      "name field: Enter commits, Escape puts the old text back (dialogs group)",
    "panels/MediaPanel.tsx": "the bin's keys: binKeys.ts",
    "panels/PreviewPanel.tsx": "the Source view's keys: SOURCE_KEYS (preview/sourceKeys.ts)",
    "shell/appCommands.ts": "the menu shortcuts: appMenu.ts",
    "timeline/TrackRenameInput.tsx":
      "track name field: Enter commits, Escape puts the old name back (dialogs group)",
    "timeline/useTimelineIntents.ts": "the timeline's keys: TIMELINE_KEYS",
    "transport/keymap.ts": "the playback keys: the keymap",
  };

  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
  const sources = (dir: string): string[] =>
    readdirSync(dir).flatMap((name) => {
      const full = path.join(dir, name);
      if (statSync(full).isDirectory()) return name === "testing" ? [] : sources(full);
      return /\.tsx?$/.test(name) && !/\.(test|stories)\.tsx?$/.test(name) ? [full] : [];
    });

  it("finds no handler outside the known files", () => {
    const found = sources(root)
      .filter((file) => /addEventListener\("keydown"|onKeyDown=/.test(readFileSync(file, "utf8")))
      .map((file) => path.relative(root, file).split(path.sep).join("/"))
      .sort();
    expect(found).toEqual(Object.keys(KNOWN).sort());
  });
});

describe("how keys read", () => {
  it("uses the platform's modifier names", () => {
    const save: KeyCombo = { key: "S", mod: true, shift: true };
    expect(comboLabel(save, "windows")).toBe("Ctrl+Shift+S");
    expect(comboLabel(save, "linux")).toBe("Ctrl+Shift+S");
    expect(comboParts(save, "mac")).toEqual(["⌘", "⇧", "S"]);
  });

  it("names the special keys", () => {
    expect(comboLabel({ key: "ArrowLeft" }, "windows")).toBe("←");
    expect(comboLabel({ key: " " }, "windows")).toBe("Space");
    expect(comboLabel({ key: "Space" }, "windows")).toBe("Space");
    expect(comboLabel({ key: "Escape" }, "windows")).toBe("Esc");
    expect(comboLabel({ key: "s" }, "windows")).toBe("S");
  });

  it("shows a letter once, whatever case its table spells", () => {
    const split = listBindings("windows").find((b) => b.id === "timeline.split");
    expect(split === undefined ? [] : distinctCombos(split, "windows")).toHaveLength(1);
    expect(split === undefined ? "" : bindingKeysText(split, "windows")).toBe("S");
  });

  it("reads the keymap's spelling back", () => {
    expect(parseKeymapKey("Ctrl+Shift+K")).toEqual({ key: "K", mod: true, shift: true });
    expect(parseKeymapKey("Space")).toEqual({ key: "Space" });
  });
});
