// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import {
  INVOKE_CHANNELS,
  type InvokeHandlers,
  IpcValidationError,
  invokeValidators,
  isMenuCommand,
  registerIpc,
} from "./ipc.ts";

const handlers = (): InvokeHandlers => ({
  "app:getInfo": () => ({ name: "Cutweave", version: "0", platform: "linux" }),
  "engine:getConnection": () => null,
  "engine:getStatus": () => ({ state: "unavailable" }),
  "dialog:openProjectFolder": () => null,
  "shell:showItemInFolder": vi.fn(),
});

type Listener = (e: unknown, ...a: unknown[]) => unknown;

describe("ipc registry", () => {
  it("registers exactly the allowlisted channels and disposes them", () => {
    const registered = new Map<string, Listener>();
    const removed: string[] = [];
    const dispose = registerIpc(
      {
        handle: (c, l) => void registered.set(c, l),
        removeHandler: (c) => void removed.push(c),
      },
      handlers(),
    );
    expect([...registered.keys()].sort()).toEqual([...INVOKE_CHANNELS].sort());
    dispose();
    expect(removed.sort()).toEqual([...INVOKE_CHANNELS].sort());
  });

  it("validates arguments before the handler runs", () => {
    const h = handlers();
    const registered = new Map<string, Listener>();
    registerIpc({ handle: (c, l) => void registered.set(c, l), removeHandler: () => {} }, h);
    const show = registered.get("shell:showItemInFolder");
    expect(() => show?.({}, 42)).toThrow(IpcValidationError);
    expect(() => show?.({}, "")).toThrow(IpcValidationError);
    expect(() => show?.({}, "a", "b")).toThrow(IpcValidationError);
    show?.({}, "C:/exports/out.mp4");
    expect(h["shell:showItemInFolder"]).toHaveBeenCalledWith("C:/exports/out.mp4");
    expect(() => registered.get("engine:getConnection")?.({}, "extra")).toThrow(IpcValidationError);
  });

  it("only accepts known menu commands", () => {
    expect(isMenuCommand("undo")).toBe(true);
    expect(isMenuCommand("rm -rf")).toBe(false);
    expect(isMenuCommand(undefined)).toBe(false);
    expect(Object.keys(invokeValidators)).toContain("app:getInfo");
  });
});
