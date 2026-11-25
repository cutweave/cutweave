// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2H.05: the keymap engine. Presets load with no conflicts; letters match by the physical key
// when the layout prints another script; reserved keys are refused with their reason; the stored
// preferences hold only what differs from the preset; a rebinding reaches every handler and every
// hint without a restart.
import { validatePreference } from "@cutweave/preferences";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { keysFor } from "../menus/keys.ts";
import { preferences } from "../prefs/prefsStore.ts";
import { chordsFor, commandForKey, shortcutLabel } from "../shell/appMenu.ts";
import { CHECKED_PLATFORMS, checkPreset, MIN_PRESET_COMMANDS } from "./check.ts";
import { KEY_COMMANDS } from "./commands.ts";
import {
  KeymapModel,
  overridesFor,
  PRESETS,
  withKey,
  withoutKey,
  withPresetKeys,
} from "./model.ts";
import type { PresetFile } from "./preset.ts";
import { reservedReason } from "./reserved.ts";
import {
  chordsStartingWith,
  completeChord,
  hintOf,
  keymapModel,
  keymapModelFor,
  matchEvent,
  setKeyPlatform,
} from "./store.ts";
import { type KeyEventLike, parseKeys, spell, strokeOfEvent } from "./stroke.ts";
import { timelineActionOf } from "./timeline.ts";

const ev = (key: string, over: Partial<KeyEventLike> = {}): KeyEventLike => ({
  key,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...over,
});

beforeEach(() => setKeyPlatform("windows"));
afterEach(async () => {
  setKeyPlatform(null);
  await preferences.reset(["shortcuts.preset", "shortcuts.bindings"]);
});

describe("the presets", () => {
  for (const preset of Object.values(PRESETS)) {
    for (const platform of CHECKED_PLATFORMS) {
      it(`${preset.id} loads with no conflicts on ${platform}, and covers at least ${MIN_PRESET_COMMANDS} commands`, () => {
        expect(checkPreset(preset, platform)).toEqual([]);
      });
    }
  }

  it("finds a conflict, an unknown command and a misspelled key in a broken preset", () => {
    const broken: PresetFile = {
      id: "broken",
      name: "Broken",
      bindings: [
        { command: "clip.split", keys: ["S"] },
        { command: "timeline.toggle_snapping", keys: ["S"] },
        { command: "no.such_command", keys: ["Mod+Shift+x", "Mod+"] },
        // A playback key and a timeline key can both be live, so they clash.
        { command: "transport.pause", keys: ["S"] },
      ],
    };
    const messages = checkPreset(broken, "windows").map((p) => p.message);
    expect(messages.some((m) => m.includes("clip.split and timeline.toggle_snapping"))).toBe(true);
    expect(messages.some((m) => m.includes("clip.split and transport.pause"))).toBe(true);
    expect(messages.some((m) => m.includes("no.such_command is not a command"))).toBe(true);
    expect(messages.some((m) => m.includes("should be spelled"))).toBe(true);
    expect(messages.some((m) => m.includes("is not a key"))).toBe(true);
    expect(messages.some((m) => m.includes("needs at least"))).toBe(true);
  });

  it("marks the CapCut bindings it cannot verify, and not the verified split", () => {
    const capcut = PRESETS.capcut.bindings;
    expect(capcut.find((b) => b.command === "clip.split")?.keys).toEqual(["Mod+B"]);
    expect(capcut.find((b) => b.command === "clip.split")?.unverified).toBeUndefined();
    expect(capcut.filter((b) => b.unverified === true).length).toBeGreaterThan(0);
    // The Redo CapCut shows first is Ctrl+Shift+Z; macOS stays Shift+Cmd+Z.
    const redo = capcut.filter((b) => b.command === "edit.redo");
    expect(redo.find((b) => b.platforms?.includes("windows"))?.keys[0]).toBe("Mod+Shift+Z");
    expect(redo.find((b) => b.platforms?.includes("mac"))?.keys).toEqual(["Mod+Shift+Z"]);
  });

  it("keeps the NLE preset where the handlers always had their keys", () => {
    expect(chordsFor("redo", "windows").map(spell)).toEqual(["Mod+Y", "Mod+Shift+Z"]);
    expect(chordsFor("redo", "mac").map(spell)).toEqual(["Mod+Shift+Z"]);
    expect(chordsFor("quit", "linux").map(spell)).toEqual(["Mod+Q"]);
    expect(chordsFor("quit", "windows")).toEqual([]);
    expect(shortcutLabel("save", "windows")).toBe("Ctrl+S");
    expect(shortcutLabel("saveAs", "mac")).toBe("⌘+Shift+S");
  });
});

describe("any keyboard layout", () => {
  it("undoes with Ctrl+Z on a Hebrew layout (key is non-Latin, code is KeyZ)", () => {
    const hebrew = ev("ז", { code: "KeyZ", ctrlKey: true });
    expect(commandForKey(hebrew, "windows", { typing: false, dev: false })).toBe("undo");
    expect(matchEvent(hebrew, ["global"])?.command).toBe("edit.undo");
  });

  it("undoes with Ctrl+Z on a Russian layout", () => {
    const russian = ev("я", { code: "KeyZ", ctrlKey: true });
    expect(commandForKey(russian, "windows", { typing: false, dev: false })).toBe("undo");
    expect(strokeOfEvent(russian, "windows")).toEqual({ key: "Z", mod: true });
  });

  it("matches the other letters, digits and punctuation of a non-Latin layout by the physical key", () => {
    expect(
      strokeOfEvent(ev("ы", { code: "KeyS", ctrlKey: true, shiftKey: true }), "windows"),
    ).toEqual({
      key: "S",
      mod: true,
      shift: true,
    });
    expect(strokeOfEvent(ev("б", { code: "Comma", ctrlKey: true }), "windows")).toEqual({
      key: ",",
      mod: true,
    });
    // The settings shortcut works too.
    expect(
      commandForKey(ev("б", { code: "Comma", ctrlKey: true }), "windows", {
        typing: false,
        dev: false,
      }),
    ).toBe("settings");
  });

  it("a Latin layout wins: Dvorak's Z is Z, whatever physical key it sits on", () => {
    expect(strokeOfEvent(ev("z", { code: "Semicolon", ctrlKey: true }), "windows")?.key).toBe("Z");
  });

  it("uses Cmd as Mod on macOS and keeps the physical Control key apart", () => {
    expect(strokeOfEvent(ev("z", { metaKey: true }), "mac")).toEqual({ key: "Z", mod: true });
    expect(strokeOfEvent(ev("z", { ctrlKey: true }), "mac")).toEqual({ key: "Z", ctrl: true });
    // The Windows key belongs to the OS.
    expect(strokeOfEvent(ev("z", { metaKey: true }), "windows")).toBeNull();
  });

  it("takes no key from an input method that is composing, or a bare modifier", () => {
    expect(matchEvent(ev("z", { ctrlKey: true, isComposing: true }), ["global"])).toBeNull();
    expect(matchEvent(ev("Process", { ctrlKey: true, keyCode: 229 }), ["global"])).toBeNull();
    expect(strokeOfEvent(ev("Control", { ctrlKey: true }), "windows")).toBeNull();
    expect(strokeOfEvent(ev("Dead"), "windows")).toBeNull();
  });
});

describe("reserved keys", () => {
  const reasonOf = (text: string, platform: "windows" | "mac" = "windows") => {
    const combo = parseKeys(text, platform);
    return combo === null ? null : reservedReason(combo, platform);
  };

  it("refuses the menu bar's keys, the context-menu keys, Alt+F4, the clipboard and the dialog keys, each with its reason", () => {
    expect(reasonOf("F10")).toBe("keymap.reserved.menuBar");
    expect(reasonOf("Alt+F")).toBe("keymap.reserved.menuBar");
    expect(reasonOf("Shift+F10")).toBe("keymap.reserved.contextMenu");
    expect(reasonOf("ContextMenu")).toBe("keymap.reserved.contextMenu");
    expect(reasonOf("Alt+F4")).toBe("keymap.reserved.closeWindow");
    expect(reasonOf("Mod+C")).toBe("keymap.reserved.clipboard");
    expect(reasonOf("Mod+V")).toBe("keymap.reserved.clipboard");
    expect(reasonOf("Escape")).toBe("keymap.reserved.dialogs");
    expect(reasonOf("Enter")).toBe("keymap.reserved.dialogs");
    // A chord is checked stroke by stroke.
    expect(reasonOf("Mod+K Escape")).toBe("keymap.reserved.dialogs");
  });

  it("leaves Ctrl+K and the ordinary keys to be bound", () => {
    expect(reasonOf("Mod+K")).toBeNull();
    expect(reasonOf("S")).toBeNull();
    expect(reasonOf("Mod+Shift+Z")).toBeNull();
    expect(reasonOf("Mod+K Mod+S")).toBeNull();
    // Alt+F is the menu bar's on Windows and Linux only.
    expect(reasonOf("Alt+F", "mac")).toBeNull();
  });
});

describe("only the differences from the preset are stored", () => {
  it("rebinds Split to Ctrl+K, taking it from the palette, and stores two entries", async () => {
    const model = keymapModel();
    expect(model.conflicts("clip.split", "Mod+K").map((c) => c.binding.id)).toEqual([
      "palette.open",
    ]);
    const wanted = withKey(
      model,
      model.effective(),
      "clip.split",
      "Mod+K",
      model.conflicts("clip.split", "Mod+K"),
    );
    const stored = overridesFor(model, wanted);
    expect(stored).toEqual({ "clip.split": ["Mod+K", "S"], "palette.open": [] });
    await preferences.set("shortcuts.bindings", stored);
    expect(preferences.get("shortcuts.bindings")).toEqual(stored);
    expect(validatePreference("shortcuts.bindings", stored).ok).toBe(true);
    // Ctrl+K now splits on the timeline, S still does, and the palette key is gone.
    expect(timelineActionOf(ev("k", { ctrlKey: true }))).toBe("split");
    expect(timelineActionOf(ev("s"))).toBe("split");
    expect(matchEvent(ev("k", { ctrlKey: true }), ["global"])).toBeNull();
    // The hint a menu shows follows, at once.
    expect(keysFor("clip.split", "windows")).toBe("Ctrl+K");
    expect(hintOf("clip.split")).toBe("Ctrl+K");
  });

  it("stores nothing when a binding is put back, and moving a key first stores one entry", async () => {
    const model = keymapModel();
    // Redo shows Ctrl+Y first; choosing Ctrl+Shift+Z puts it first.
    const wanted = withKey(model, model.effective(), "edit.redo", "Mod+Shift+Z");
    const stored = overridesFor(model, wanted);
    expect(stored).toEqual({ "edit.redo": ["Mod+Shift+Z", "Mod+Y"] });
    await preferences.set("shortcuts.bindings", stored);
    expect(shortcutLabel("redo", "windows")).toBe("Ctrl+Shift+Z");
    // macOS is untouched by the Windows-only override? The override is per id, so it applies there too.
    const after = keymapModel();
    expect(overridesFor(after, withPresetKeys(after, after.effective(), "edit.redo"))).toEqual({});
  });

  it("removes a key and a binding, and a removal is stored as an empty list", async () => {
    const model = keymapModel();
    const wanted = withoutKey(
      withoutKey(model.effective(), "item.delete", "Backspace"),
      "track.rename",
      "F2",
    );
    const stored = overridesFor(model, wanted);
    expect(stored).toEqual({ "item.delete": ["Delete"], "track.rename": [] });
    await preferences.set("shortcuts.bindings", stored);
    expect(timelineActionOf(ev("Backspace"))).toBeNull();
    expect(timelineActionOf(ev("Delete"))).toBe("delete");
    expect(timelineActionOf(ev("F2"))).toBeNull();
  });

  it("keeps the person's keys when the preset changes under them", async () => {
    await preferences.set("shortcuts.bindings", { "clip.split": ["Mod+Shift+X"] });
    await preferences.set("shortcuts.preset", "capcut");
    // CapCut's Delete (not Backspace) applies, and the person's Split key is still theirs.
    expect(timelineActionOf(ev("x", { ctrlKey: true, shiftKey: true }))).toBe("split");
    expect(timelineActionOf(ev("b", { ctrlKey: true }))).toBeNull();
    expect(timelineActionOf(ev("Backspace"))).toBeNull();
  });

  it("switches the preset: Ctrl+B splits with CapCut and not with the NLE preset", async () => {
    expect(timelineActionOf(ev("b", { ctrlKey: true }))).toBeNull();
    await preferences.set("shortcuts.preset", "capcut");
    expect(timelineActionOf(ev("b", { ctrlKey: true }))).toBe("split");
    expect(timelineActionOf(ev("s"))).toBeNull();
    // CapCut shows Ctrl+Shift+Z as the Redo.
    expect(shortcutLabel("redo", "windows")).toBe("Ctrl+Shift+Z");
    expect(shortcutLabel("redo", "mac")).toBe("⌘+Shift+Z");
  });

  it("drops overrides for a command that no longer exists without failing", async () => {
    await preferences.set("shortcuts.bindings", { "gone.command": ["Mod+9"] });
    expect(matchEvent(ev("9", { ctrlKey: true }), ["global"])?.command).toBe("gone.command");
    expect(
      commandForKey(ev("9", { ctrlKey: true }), "windows", { typing: false, dev: false }),
    ).toBeNull();
  });

  it("refuses a shortcuts value that is not keys", () => {
    expect(validatePreference("shortcuts.bindings", { "a b": ["Mod+S"] }).ok).toBe(false);
    expect(validatePreference("shortcuts.bindings", { "clip.split": "S" }).ok).toBe(false);
    expect(
      validatePreference("shortcuts.bindings", { "clip.split": ["Mod+S Mod+T Mod+U"] }).ok,
    ).toBe(false);
    expect(validatePreference("shortcuts.preset", "emacs").ok).toBe(false);
  });
});

describe("conflicts", () => {
  it("names who has the key already, across contexts that can both be live", () => {
    const model = keymapModelFor("windows");
    expect(model.conflicts("clip.split", "Mod+Z").map((c) => c.binding.id)).toEqual(["edit.undo"]);
    // A playback key and a timeline key can both be live.
    expect(model.conflicts("clip.split", "K").map((c) => c.binding.id)).toEqual([
      "transport.pause",
    ]);
    // The bin's own keys take precedence where it has the focus, so Space is free for the bin.
    expect(model.conflicts("bin.play", "Space").map((c) => c.binding.id)).toEqual([]);
    // Not a conflict with itself, and the text fields' own keys are the reserved keys' business.
    expect(model.conflicts("clip.split", "S")).toEqual([]);
  });

  it("treats a key and a chord that starts with it as ambiguous", () => {
    const model = new KeymapModel("nle", { "project.save": ["Mod+K Mod+S"] }, "windows");
    expect(
      model
        .conflicts("clip.split", "Mod+K")
        .map((c) => c.binding.id)
        .sort(),
    ).toEqual(["palette.open", "project.save"]);
  });
});

describe("chords", () => {
  it("starts on the first stroke and completes on the second", async () => {
    await preferences.set("shortcuts.bindings", {
      "palette.open": [],
      "workspace.toggle_panel.inspector": ["Mod+K Mod+I"],
    });
    const first = strokeOfEvent(ev("k", { ctrlKey: true }), "windows");
    const second = strokeOfEvent(ev("i", { ctrlKey: true }), "windows");
    expect(first !== null && chordsStartingWith(first, ["global"]).map((b) => b.id)).toEqual([
      "workspace.toggle_panel.inspector",
    ]);
    expect(first !== null && second !== null && completeChord(first, second, ["global"])?.id).toBe(
      "workspace.toggle_panel.inspector",
    );
    // A chord is not a menu accelerator and is not a single press.
    expect(matchEvent(ev("k", { ctrlKey: true }), ["global"])).toBeNull();
  });
});

describe("lookup speed", () => {
  it("answers a key press in well under 0.2 ms", () => {
    const press = ev("z", { ctrlKey: true });
    const n = 5000;
    matchEvent(press, ["global"]);
    const start = performance.now();
    for (let i = 0; i < n; i++) matchEvent(press, ["global"]);
    const each = (performance.now() - start) / n;
    expect(each).toBeLessThan(0.2);
  });
});

describe("the table", () => {
  it("has a key command for every id a preset binds that is not a catalog command", () => {
    const ids = new Set(KEY_COMMANDS.map((c) => c.id));
    for (const preset of Object.values(PRESETS)) {
      for (const b of preset.bindings) {
        expect(ids.has(b.command) || /^workspace\./.test(b.command), b.command).toBe(true);
      }
    }
  });
});
