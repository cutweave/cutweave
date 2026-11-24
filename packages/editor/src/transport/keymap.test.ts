// SPDX-License-Identifier: GPL-3.0-or-later
// The keymap module (WP-1M.04): a simple key → action map with registered handlers. Keys typed
// into fields, or that the focused control uses itself, are left alone.
import { describe, expect, it, vi } from "vitest";
import { DEFAULT_KEYMAP, type KeyEventLike, Keymap, keyOf } from "./keymap.ts";

const ev = (key: string, over: Partial<KeyEventLike> = {}): KeyEventLike => ({
  key,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  repeat: false,
  defaultPrevented: false,
  target: null,
  ...over,
});

const el = (tagName: string, attrs: Record<string, string> = {}, editable = false) =>
  ({
    tagName,
    isContentEditable: editable,
    getAttribute: (name: string) => attrs[name] ?? null,
  }) as unknown as EventTarget;

describe("keyOf", () => {
  it("names keys with their modifiers, letters case-insensitively", () => {
    expect(keyOf(ev(" "))).toBe("Space");
    expect(keyOf(ev("j"))).toBe("J");
    expect(keyOf(ev("J", { shiftKey: true }))).toBe("Shift+J");
    expect(keyOf(ev("ArrowLeft"))).toBe("ArrowLeft");
    expect(keyOf(ev("k", { ctrlKey: true }))).toBe("Ctrl+K");
    expect(keyOf(ev("l", { metaKey: true, altKey: true }))).toBe("Alt+Meta+L");
  });
});

describe("Keymap", () => {
  it("binds the NLE transport keys by default", () => {
    expect(DEFAULT_KEYMAP).toMatchObject({
      Space: "transport:togglePlay",
      J: "transport:reverse",
      K: "transport:pause",
      L: "transport:forward",
      ArrowLeft: "transport:stepBack",
      ArrowRight: "transport:stepForward",
    });
  });

  it("runs the registered handler and reports the key handled", () => {
    const keymap = new Keymap();
    const play = vi.fn();
    const off = keymap.register("transport:togglePlay", play);
    expect(keymap.handle(ev(" "))).toBe(true);
    expect(play).toHaveBeenCalledTimes(1);
    off();
    expect(keymap.handle(ev(" "))).toBe(false);
  });

  it("ignores unbound keys, modified keys and keys nobody handles", () => {
    const keymap = new Keymap();
    const fwd = vi.fn();
    keymap.register("transport:forward", fwd);
    expect(keymap.handle(ev("x"))).toBe(false);
    expect(keymap.handle(ev("l", { ctrlKey: true }))).toBe(false);
    expect(keymap.handle(ev("k"))).toBe(false);
    expect(fwd).not.toHaveBeenCalled();
  });

  it("leaves keys to text fields and to the control that has the focus", () => {
    const keymap = new Keymap();
    const play = vi.fn();
    const step = vi.fn();
    keymap.register("transport:togglePlay", play);
    keymap.register("transport:stepForward", step);
    expect(keymap.handle(ev(" ", { target: el("INPUT") }))).toBe(false);
    expect(keymap.handle(ev(" ", { target: el("TEXTAREA") }))).toBe(false);
    expect(keymap.handle(ev(" ", { target: el("DIV", {}, true) }))).toBe(false);
    // Space presses a focused button; arrows move a slider or a radio group.
    expect(keymap.handle(ev(" ", { target: el("BUTTON") }))).toBe(false);
    expect(keymap.handle(ev(" ", { target: el("DIV", { role: "radio" }) }))).toBe(false);
    expect(keymap.handle(ev("ArrowRight", { target: el("SPAN", { role: "slider" }) }))).toBe(false);
    expect(keymap.handle(ev("ArrowRight", { target: el("BUTTON") }))).toBe(true);
    expect(keymap.handle(ev(" ", { target: el("CANVAS") }))).toBe(true);
    expect(keymap.handle(ev(" ", { defaultPrevented: true }))).toBe(false);
    expect(play).toHaveBeenCalledTimes(1);
    expect(step).toHaveBeenCalledTimes(1);
  });

  it("auto-repeat steps but does not toggle play", () => {
    const keymap = new Keymap();
    const play = vi.fn();
    const step = vi.fn();
    keymap.register("transport:togglePlay", play);
    keymap.register("transport:stepBack", step);
    keymap.handle(ev(" ", { repeat: true }));
    keymap.handle(ev("ArrowLeft", { repeat: true }));
    expect(play).not.toHaveBeenCalled();
    expect(step).toHaveBeenCalledTimes(1);
  });

  it("lists the keys of an action for tooltips", () => {
    const keymap = new Keymap();
    expect(keymap.keysFor("transport:togglePlay")).toEqual(["Space"]);
    expect(keymap.keysFor("transport:stepBack")).toEqual(["←"]);
    expect(keymap.keysFor("transport:forward")).toEqual(["L"]);
  });

  it("the latest registration for an action wins until it is removed", () => {
    const keymap = new Keymap();
    const a = vi.fn();
    const b = vi.fn();
    keymap.register("transport:pause", a);
    const offB = keymap.register("transport:pause", b);
    keymap.handle(ev("k"));
    offB();
    keymap.handle(ev("k"));
    expect(b).toHaveBeenCalledTimes(1);
    expect(a).toHaveBeenCalledTimes(1);
  });
});
