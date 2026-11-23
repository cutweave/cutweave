// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import { isMenuCommand } from "../shared/ipc-contract.ts";
import {
  createSenderCheck,
  INVOKE_CHANNELS,
  type InvokeHandlers,
  type IpcEventLike,
  IpcSenderError,
  IpcValidationError,
  invokeSchemas,
  parseInvokeArgs,
  registerIpc,
  type SenderCheck,
} from "./ipc.ts";

const handlers = (): InvokeHandlers => ({
  "app:getInfo": () => ({ name: "Cutweave", version: "0", platform: "linux" }),
  "engine:getConnection": () => null,
  "engine:getStatus": () => ({ state: "unavailable" }),
  "dialog:openProjectFolder": () => null,
  "recent:list": () => [],
  "recent:add": vi.fn(),
  "recent:remove": vi.fn(),
  "window:setDirty": vi.fn(),
  "window:confirmClose": vi.fn(),
  "shell:showItemInFolder": vi.fn(),
  "shell:openExternal": vi.fn(async () => undefined),
});

type Listener = (e: IpcEventLike, ...a: unknown[]) => unknown;

const editor = { id: 1 };
const appFrame = { url: "app://cutweave/", parent: null };
const okEvent: IpcEventLike = { sender: editor, senderFrame: appFrame };
const allowAll: SenderCheck = () => null;

function register(h: InvokeHandlers, check: SenderCheck = allowAll) {
  const registered = new Map<string, Listener>();
  registerIpc({ handle: (c, l) => void registered.set(c, l), removeHandler: () => {} }, h, check);
  return (channel: string, event: IpcEventLike, ...args: unknown[]) => {
    const listener = registered.get(channel);
    if (listener === undefined) throw new Error(`No handler registered for '${channel}'`);
    return listener(event, ...args);
  };
}

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
      allowAll,
    );
    expect([...registered.keys()].sort()).toEqual([...INVOKE_CHANNELS].sort());
    expect(INVOKE_CHANNELS.sort()).toEqual(Object.keys(invokeSchemas).sort());
    dispose();
    expect(removed.sort()).toEqual([...INVOKE_CHANNELS].sort());
  });

  it("has no handler for unlisted channels", () => {
    const call = register(handlers());
    expect(() => call("fs:readFile", okEvent, "/etc/passwd")).toThrow(/No handler registered/);
    expect(INVOKE_CHANNELS).not.toContain("invoke");
  });

  it("validates arguments with zod before the handler runs", () => {
    const h = handlers();
    const call = register(h);
    expect(() => call("shell:showItemInFolder", okEvent, 42)).toThrow(IpcValidationError);
    expect(() => call("shell:showItemInFolder", okEvent, 42)).toThrow(/expected string/);
    expect(() => call("shell:showItemInFolder", okEvent, "")).toThrow(IpcValidationError);
    expect(() => call("shell:showItemInFolder", okEvent, "a", "b")).toThrow(IpcValidationError);
    expect(() => call("shell:showItemInFolder", okEvent, "a\0b")).toThrow(/NUL/);
    expect(() => call("shell:showItemInFolder", okEvent, "x".repeat(4097))).toThrow(
      IpcValidationError,
    );
    call("shell:showItemInFolder", okEvent, "C:/exports/out.mp4");
    expect(h["shell:showItemInFolder"]).toHaveBeenCalledWith("C:/exports/out.mp4");
    expect(h["shell:showItemInFolder"]).toHaveBeenCalledTimes(1);
    expect(() => call("engine:getConnection", okEvent, "extra")).toThrow(IpcValidationError);
  });

  it("accepts only https links for openExternal", async () => {
    const h = handlers();
    const call = register(h);
    for (const bad of [
      "http://example.com",
      "file:///etc/passwd",
      "javascript:alert(1)",
      "https://user:pw@example.com/",
      "app://cutweave/",
      "not a url",
      `https://example.com/${"a".repeat(2100)}`,
      42,
    ]) {
      expect(() => call("shell:openExternal", okEvent, bad), String(bad)).toThrow(
        IpcValidationError,
      );
    }
    await call("shell:openExternal", okEvent, "https://cutweave.org/docs");
    expect(h["shell:openExternal"]).toHaveBeenCalledWith("https://cutweave.org/docs");
  });

  it("runs the sender check before validation and the handler", () => {
    const h = handlers();
    const check = vi.fn<SenderCheck>(() => "untrusted sender origin");
    const call = register(h, check);
    expect(() => call("shell:showItemInFolder", okEvent, 42)).toThrow(IpcSenderError);
    expect(() => call("app:getInfo", okEvent)).toThrow(/untrusted sender origin/);
    expect(h["shell:showItemInFolder"]).not.toHaveBeenCalled();
    expect(check).toHaveBeenCalledTimes(2);
  });

  it("parseInvokeArgs returns the typed tuple", () => {
    expect(parseInvokeArgs("shell:showItemInFolder", ["/a"])).toEqual(["/a"]);
    expect(parseInvokeArgs("app:getInfo", [])).toEqual([]);
  });

  it("only accepts known menu commands", () => {
    expect(isMenuCommand("undo")).toBe(true);
    expect(isMenuCommand("rm -rf")).toBe(false);
    expect(isMenuCommand(undefined)).toBe(false);
  });
});

describe("createSenderCheck", () => {
  const check = createSenderCheck({
    isEditorContents: (sender) => sender === editor,
  });
  const devCheck = createSenderCheck({
    devServerOrigin: "http://localhost:5173",
    isEditorContents: (sender) => sender === editor,
  });

  it("accepts the editor window's top frame on the app origin", () => {
    expect(check(okEvent)).toBeNull();
    expect(
      check({ sender: editor, senderFrame: { url: "app://cutweave/x?y", parent: null } }),
    ).toBeNull();
  });

  it("rejects foreign origins, subframes, gone frames and non-editor windows", () => {
    const at = (url: string) => ({ sender: editor, senderFrame: { url, parent: null } });
    expect(check(at("https://example.com/"))).toBe("untrusted sender origin");
    expect(check(at("data:text/html,hi"))).toBe("untrusted sender origin");
    expect(check(at("file:///C:/app/index.html"))).toBe("untrusted sender origin");
    expect(check(at("app://evil/"))).toBe("untrusted sender origin");
    expect(check(at("app://cutweave@evil/"))).toBe("untrusted sender origin");
    expect(check(at("http://localhost:5173/"))).toBe("untrusted sender origin");
    expect(check({ sender: editor, senderFrame: null })).toBe("the sender frame is gone");
    expect(
      check({ sender: editor, senderFrame: { url: "app://cutweave/", parent: appFrame } }),
    ).toBe("IPC from a subframe");
    expect(check({ sender: { id: 2 }, senderFrame: appFrame })).toBe(
      "the sender is not an editor window",
    );
  });

  it("trusts the dev server origin only when one is configured", () => {
    const dev = { sender: editor, senderFrame: { url: "http://localhost:5173/", parent: null } };
    expect(devCheck(dev)).toBeNull();
    expect(
      devCheck({ sender: editor, senderFrame: { url: "http://localhost:5174/", parent: null } }),
    ).toBe("untrusted sender origin");
  });
});
