// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1M.13, WP-2H.05: the shortcut list is generated from the keymap the handlers read, and a test
// walks each handler's table, so a handler added without a binding (or a binding without a
// handler) fails here.
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { MACHINE_KEYS, machineKeyAction } from "@cutweave/timeline";
import { describe, expect, it } from "vitest";
import { i18n } from "../i18n/index.ts";
import { KEY_COMMANDS } from "../keymap/commands.ts";
import { PRESETS } from "../keymap/model.ts";
import { EDITOR_COMMANDS, TIMELINE_COMMANDS, timelineActionOf } from "../keymap/timeline.ts";
import { BIN_COMMANDS, binKeyAction } from "../panels/binKeys.ts";
import { SOURCE_COMMANDS, sourceKeyAction } from "../preview/sourceKeys.ts";
import {
  BROWSER_KEYS,
  buildAppMenu,
  type Chord,
  COMMAND_IDS,
  chordsFor,
  commandForKey,
  KEYED,
  type MenuPlatform,
} from "../shell/appMenu.ts";
import { ACTION_COMMANDS, keymap } from "../transport/keymap.ts";
import {
  BINDING_GROUPS,
  type Binding,
  bindingKeysText,
  comboLabel,
  comboParts,
  distinctCombos,
  type KeyCombo,
  listBindings,
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
  event(combo.key === "Space" ? " " : combo.key, {
    ...(combo.mod === true && (platform === "mac" ? { metaKey: true } : { ctrlKey: true })),
    ...(combo.alt === true && { altKey: true }),
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
    expect(ids(listBindings("windows"))).not.toContain("window.reload");
    expect(ids(listBindings("windows", { dev: true }))).toContain("window.reload");
  });

  it("lists a command of the table once, and every bound command of the NLE preset", () => {
    expect(new Set(KEY_COMMANDS.map((c) => c.id)).size).toBe(KEY_COMMANDS.length);
    const listed = new Set(ids(listBindings("linux", { dev: true })));
    for (const entry of PRESETS.nle.bindings) {
      const known = KEY_COMMANDS.some((c) => c.id === entry.command);
      if (known) expect(listed.has(entry.command), entry.command).toBe(true);
    }
  });
});

describe("the menu bar's keys", () => {
  it("lists every command the key handler runs, with every chord it matches", () => {
    for (const platform of PLATFORMS) {
      const bindings = listBindings(platform, { dev: true });
      for (const command of [...KEYED, ...BROWSER_KEYS]) {
        const chords = chordsFor(command, platform);
        const binding = bindings.find((b) => b.id === COMMAND_IDS[command]);
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
      const listed = listBindings(platform, { dev: true });
      for (const menu of menus) {
        for (const entry of menu.items) {
          if (entry.kind === "command" && entry.shortcut !== undefined) {
            const found = listed.some((b) => b.id === COMMAND_IDS[entry.command]);
            expect(found, `${platform} ${entry.command}`).toBe(true);
          }
        }
      }
    }
  });
});

describe("the playback keys", () => {
  it("lists every key of the transport, and nothing else", () => {
    const listed = listBindings("windows")
      .filter((b) => b.id.startsWith("transport."))
      .flatMap((b) => b.combos.map((c) => c.key))
      .sort();
    const keymapKeys = keymap
      .entries()
      .map(([spelled]) => spelled.split("+").pop() ?? spelled)
      .sort();
    expect(listed).toEqual(keymapKeys);
    expect(Object.values(ACTION_COMMANDS).sort()).toEqual(
      listBindings("windows")
        .filter((b) => b.id.startsWith("transport."))
        .map((b) => b.id)
        .sort(),
    );
  });

  it("lists the Source view's keys (WP-1M.15, WP-2A.14) with the playback keys", () => {
    const listed = listBindings("windows")
      .filter((b) => b.id.startsWith("source."))
      .flatMap((b) => b.combos.map((c) => c.key))
      .sort();
    expect(listed).toEqual([",", ".", "Enter", "Escape", "I", "O", "X"]);
    expect(Object.values(SOURCE_COMMANDS).sort()).toEqual(
      listBindings("windows")
        .filter((b) => b.id.startsWith("source."))
        .map((b) => b.id)
        .sort(),
    );
    expect(sourceKeyAction(event("Escape"))).toBe("back");
    expect(sourceKeyAction(event("Enter"))).toBe("insert");
    // Marks and three-point edits (WP-2A.14): the timeline's own I, O, comma and period, by focus.
    expect(sourceKeyAction(event(","))).toBe("insert");
    expect(sourceKeyAction(event("."))).toBe("overwrite");
    expect(sourceKeyAction(event("i"))).toBe("markIn");
    expect(sourceKeyAction(event("o"))).toBe("markOut");
    expect(sourceKeyAction(event("x", { altKey: true }))).toBe("clearMarks");
    expect(sourceKeyAction(event("Enter", { ctrlKey: true }))).toBeNull();
    expect(sourceKeyAction(event("a"))).toBeNull();
  });

  it("lists the bin's play key (Space selects the card and plays it)", () => {
    const listed = listBindings("windows").find((b) => b.id === "bin.play");
    expect(listed?.combos).toEqual([{ key: "Space" }]);
    expect(binKeyAction(event(" "))).toBe("play");
  });

  it("is what the keymap matches: every listed key finds its action", () => {
    for (const [spelled] of keymap.entries()) {
      const name = spelled.split("+").pop() ?? spelled;
      const key = name === "Space" ? " " : name;
      expect(
        keymap.actionFor({ ...event(key), repeat: false, defaultPrevented: false, target: null }),
        spelled,
      ).not.toBeNull();
    }
  });
});

describe("the timeline's keys", () => {
  /** The key event a machine default is pressed with (Shift+, prints `<` on a US layout). */
  const machineEvent = (key: string, shift: boolean) =>
    ({
      ...event(key, { shiftKey: shift }),
      ...(key === "<" && { code: "Comma" }),
      ...(key === ">" && { code: "Period" }),
    }) as ReturnType<typeof event>;

  it("lists every default key of the interaction machine, and the NLE preset agrees with them", () => {
    const listed = listBindings("windows").filter((b) => b.context === "timeline");
    const byCommand = new Map(listed.map((b) => [b.id, b]));
    for (const entry of MACHINE_KEYS) {
      const shift = entry.shift === true;
      const id = TIMELINE_COMMANDS[entry.action];
      const combos = byCommand.get(id)?.combos ?? [];
      const spelled = entry.key === "<" ? "," : entry.key === ">" ? "." : entry.key;
      expect(
        combos.some(
          (c) => c.key.toLowerCase() === spelled.toLowerCase() && (c.shift === true) === shift,
        ),
        `${id} ${entry.key}`,
      ).toBe(true);
      expect(machineKeyAction(entry.key, { shift }), entry.key).toBe(entry.action);
      // The keymap decides in the app, and it agrees.
      expect(timelineActionOf(machineEvent(entry.key, shift)), entry.key).toBe(entry.action);
      // A letter works in either case.
      if (/^[a-z]$/.test(entry.key)) {
        expect(timelineActionOf(event(entry.key.toUpperCase())), entry.key).toBe(entry.action);
      }
    }
    // Every timeline binding is a machine action or one the editor runs itself (Duplicate, the
    // markers, in and out, Rename).
    const known = new Set<string>([...Object.values(TIMELINE_COMMANDS), ...EDITOR_COMMANDS]);
    for (const binding of listed) expect(known.has(binding.id), binding.id).toBe(true);
    expect(byCommand.get("item.duplicate")?.combos.map((c) => c.key.toLowerCase())).toEqual(["d"]);
    expect(byCommand.get("timeline.add_marker")?.combos.map((c) => c.key)).toEqual(["M"]);
    expect(byCommand.get("timeline.mark_in")?.combos.map((c) => c.key)).toEqual(["I"]);
    expect(byCommand.get("timeline.mark_out")?.combos.map((c) => c.key)).toEqual(["O"]);
  });
});

describe("the media bin's keys", () => {
  it("lists every bin command, each with the keys the panel matches", () => {
    const listed = listBindings("windows").filter((b) => b.group === "bin");
    expect(listed.map((b) => b.id).sort()).toEqual(Object.values(BIN_COMMANDS).sort());
  });

  it("is what the panel matches: pressing a listed key does its action", () => {
    for (const [action, command] of Object.entries(BIN_COMMANDS)) {
      const binding = listBindings("linux").find((b) => b.id === command);
      for (const combo of binding?.combos ?? []) {
        expect(
          binKeyAction(press(combo, "linux")),
          `${command} ${comboLabel(combo, "linux")}`,
        ).toBe(action);
      }
    }
    expect(binKeyAction(event("i"))).toBeNull();
    expect(binKeyAction(event("i", { ctrlKey: true, shiftKey: true }))).toBeNull();
    expect(binKeyAction(event("ArrowLeft", { ctrlKey: true }))).toBeNull();
    // A plain key ignores Shift, as before: Shift+Arrow still moves.
    expect(binKeyAction(event("ArrowRight", { shiftKey: true }))).toBe("moveNext");
  });
});

describe("every key handler in the editor is accounted for", () => {
  // A file with a `keydown` listener or an `onKeyDown` prop is a key handler. Each one is either
  // generated into the list above or a text field's own keys, named here with the reason. A new
  // handler fails this test until it is listed in the keymap or named here.
  const KNOWN: Readonly<Record<string, string>> = {
    "export/ExportForm.tsx":
      "the preset name field: Enter saves the preset instead of starting the export (dialogs group)",
    "history/HistoryPanel.tsx":
      "the history list's keys: Up, Down, Home and End move between rows, Enter goes to one",
    "inspector/MediaOrganize.tsx":
      "note field: Ctrl+Enter commits (Enter is a new line), Escape puts the stored note back (dialogs group)",
    "inspector/MediaPreview.tsx":
      "a slider's own keys while it has the focus: arrows step a frame, Home, End, Page Up and Down",
    "keymap/dispatch.ts": "the global dispatcher: every `global` binding of the keymap",
    "media/BinSidebar.tsx":
      "the bin sidebar's trees: Up, Down, Home and End move between rows, Right and Left open and close a folder, Enter opens a row; rename and delete of a collection are the keymap's `bin` bindings (binKeys.ts)",
    "menus/host.tsx":
      "Shift+F10 and the Menu key open the context menu of the focused surface: the platform's own keys (reserved)",
    "inspector/SpeedSection.tsx":
      "speed field: Enter commits, Escape puts the old text back (dialogs group)",
    "inspector/TrackSection.tsx":
      "track name field: Enter commits, Escape puts the old name back (dialogs group)",
    "palette/CommandPalette.tsx":
      "the palette's search field: Up, Down, Page Up, Page Down and Enter move through and run its list",
    "panels/InspectorPanel.tsx":
      "name field: Enter commits, Escape puts the old text back (dialogs group)",
    "panels/MediaPanel.tsx": "the bin's keys: the keymap's `bin` context (binKeys.ts)",
    "panels/PreviewPanel.tsx": "the Source view's keys: the keymap's `source` context",
    "panels/SourceScrubBar.tsx":
      "a slider's own keys while it has the focus: arrows step a frame, Home, End, Page Up and Down",
    "preview/overlays/TransformOverlay.tsx":
      "the picture's handles: the keymap's `preview` context (nudges, crop); Escape ends a drag or crop mode",
    "settings/shortcuts.tsx": "record-a-shortcut: the next key press is the shortcut",
    "shell/appCommands.ts": "the menu shortcuts: appMenu.ts, from the keymap",
    "timeline/TrackRenameInput.tsx":
      "track name field: Enter commits, Escape puts the old name back (dialogs group)",
    "timeline/useTimelineHost.ts": "the timeline's keys: the keymap's `timeline` context",
    "transport/keymap.ts": "the playback keys: the keymap's `playback` context",
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
    expect(comboLabel({ key: "Space" }, "windows")).toBe("Space");
    expect(comboLabel({ key: "Escape" }, "windows")).toBe("Esc");
    expect(comboLabel({ key: "s" }, "windows")).toBe("S");
  });

  it("reads a chord as two strokes", () => {
    const chord: KeyCombo = { key: "K", mod: true, next: { key: "S", mod: true } };
    expect(comboLabel(chord, "windows")).toBe("Ctrl+K Ctrl+S");
  });

  it("shows a letter once, whatever case its table spells", () => {
    const split = listBindings("windows").find((b) => b.id === "clip.split");
    expect(split === undefined ? [] : distinctCombos(split, "windows")).toHaveLength(1);
    expect(split === undefined ? "" : bindingKeysText(split, "windows")).toBe("S");
  });
});
