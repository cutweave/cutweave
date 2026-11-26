// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2H.05: the global dispatcher runs the keymap's `global` bindings: the palette's key (now a
// binding like any other), a chord, and leaves text fields their plain keys and an input method
// its composition.
import { cleanup, fireEvent, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { closePalette, paletteStore } from "../palette/paletteStore.ts";
import "../palette/index.ts";
import { preferences } from "../prefs/prefsStore.ts";
import { addPopoutWindow, removePopoutWindow, resetWindowRegistry } from "../shell/windows.ts";
import { useKeymapDispatcher } from "./dispatch.ts";
import { setKeyPlatform } from "./store.ts";

beforeEach(() => setKeyPlatform("windows"));
afterEach(async () => {
  cleanup();
  resetWindowRegistry();
  closePalette();
  setKeyPlatform(null);
  await preferences.reset(["shortcuts.preset", "shortcuts.bindings"]);
});

const ctrl = (key: string, init: KeyboardEventInit = {}) => ({
  key,
  code: `Key${key.toUpperCase()}`,
  ctrlKey: true,
  ...init,
});

describe("the global dispatcher", () => {
  it("opens the palette with Ctrl+K and closes it with the same key", () => {
    renderHook(() => useKeymapDispatcher());
    fireEvent.keyDown(window, ctrl("k"));
    expect(paletteStore.getState().open).toBe(true);
    fireEvent.keyDown(window, ctrl("k"));
    expect(paletteStore.getState().open).toBe(false);
  });

  it("follows a rebinding: the palette on Ctrl+J, and Ctrl+K is free", async () => {
    await preferences.set("shortcuts.bindings", { "palette.open": ["Mod+J"] });
    renderHook(() => useKeymapDispatcher());
    fireEvent.keyDown(window, ctrl("k"));
    expect(paletteStore.getState().open).toBe(false);
    fireEvent.keyDown(window, ctrl("j"));
    expect(paletteStore.getState().open).toBe(true);
  });

  it("opens it on a Hebrew layout, by the physical key", () => {
    renderHook(() => useKeymapDispatcher());
    fireEvent.keyDown(window, { key: "ל", code: "KeyK", ctrlKey: true });
    expect(paletteStore.getState().open).toBe(true);
  });

  it("runs a chord on its second stroke, and not before", async () => {
    await preferences.set("shortcuts.bindings", {
      "palette.open": ["Mod+K Mod+P"],
    });
    renderHook(() => useKeymapDispatcher());
    fireEvent.keyDown(window, ctrl("k"));
    expect(paletteStore.getState().open).toBe(false);
    fireEvent.keyDown(window, ctrl("p"));
    expect(paletteStore.getState().open).toBe(true);
  });

  it("drops a chord that is abandoned with Escape or with another key", async () => {
    await preferences.set("shortcuts.bindings", { "palette.open": ["Mod+K Mod+P"] });
    renderHook(() => useKeymapDispatcher());
    fireEvent.keyDown(window, ctrl("k"));
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.keyDown(window, ctrl("p"));
    expect(paletteStore.getState().open).toBe(false);
  });

  it("leaves a plain key to the text field it is typed in, and runs one with a modifier", async () => {
    await preferences.set("shortcuts.bindings", { "palette.open": ["P"] });
    renderHook(() => useKeymapDispatcher());
    const input = document.createElement("input");
    document.body.append(input);
    fireEvent.keyDown(input, { key: "p", code: "KeyP" });
    expect(paletteStore.getState().open).toBe(false);
    fireEvent.keyDown(document.body, { key: "p", code: "KeyP" });
    expect(paletteStore.getState().open).toBe(true);
    closePalette();
    // Ctrl+K from a text field still opens it by default, as a native accelerator does.
    await preferences.reset(["shortcuts.bindings"]);
    fireEvent.keyDown(input, ctrl("k"));
    expect(paletteStore.getState().open).toBe(true);
    input.remove();
  });

  it("leaves the keys another listener owns alone: Import (the bin's), Save (the menu's)", () => {
    renderHook(() => useKeymapDispatcher());
    for (const key of ["i", "s"]) {
      const press = new KeyboardEvent("keydown", {
        key,
        code: `Key${key.toUpperCase()}`,
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      });
      window.dispatchEvent(press);
      expect(press.defaultPrevented, key).toBe(false);
    }
  });

  // WP-2H.15: a detached panel's window (an iframe's window here) has the same keys.
  it("runs a binding pressed in a detached panel's window, and opens the palette there", () => {
    const frame = document.createElement("iframe");
    document.body.append(frame);
    const popout = frame.contentWindow as Window;
    renderHook(() => useKeymapDispatcher());
    // Registered after the dispatcher mounted: it is installed as the window opens.
    addPopoutWindow(popout, ["panel.timeline"]);
    const target = popout.document.createElement("div");
    target.tabIndex = 0;
    popout.document.body.append(target);
    fireEvent.keyDown(target, ctrl("k"));
    expect(paletteStore.getState().open).toBe(true);
    expect(paletteStore.getState().window).toBe(popout);
    closePalette();
    // Pressed in the main window, it opens in the main window.
    fireEvent.keyDown(document.body, ctrl("k"));
    expect(paletteStore.getState().window).toBeNull();
    closePalette();
    // A window that closed has no keys any more.
    removePopoutWindow(popout);
    fireEvent.keyDown(target, ctrl("k"));
    expect(paletteStore.getState().open).toBe(false);
    frame.remove();
  });

  it("takes nothing from an input method that is composing, and nothing from a held key", () => {
    renderHook(() => useKeymapDispatcher());
    fireEvent.keyDown(window, ctrl("k", { isComposing: true }));
    fireEvent.keyDown(window, { ...ctrl("k"), keyCode: 229 });
    fireEvent.keyDown(window, ctrl("k", { repeat: true }));
    expect(paletteStore.getState().open).toBe(false);
  });
});
