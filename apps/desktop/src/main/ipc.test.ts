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
  "recent:clear": vi.fn(),
  "window:setDirty": vi.fn(),
  "window:confirmClose": vi.fn(),
  "dialog:pickMediaFiles": vi.fn(() => null),
  "dialog:chooseExportFile": vi.fn(() => null),
  "shell:showItemInFolder": vi.fn(),
  "shell:openExternal": vi.fn(async () => undefined),
  "app:about": () => ({
    name: "Cutweave",
    version: "0",
    electron: "0",
    chrome: "0",
    node: "0",
    v8: "0",
    platform: "linux x64",
  }),
  "app:quit": vi.fn(),
  "window:toggleFullScreen": vi.fn(),
  "window:setTheme": vi.fn(),
  "window:reload": vi.fn(),
  "window:toggleDevTools": vi.fn(),
  "window:inspectElement": vi.fn(),
  "edit:run": vi.fn(),
  "prefs:get": vi.fn(() => ({ values: {}, defaults: {} }) as never),
  "prefs:set": vi.fn(),
  "prefs:reset": vi.fn(),
  "prefs:export": vi.fn(() => ({ status: "cancelled" as const })),
  "prefs:import": vi.fn(() => ({ status: "cancelled" as const })),
  "shell:openAppFolder": vi.fn(),
  "agents:addFolder": vi.fn(() => ({ status: "cancelled" as const })),
  "agents:removeFolder": vi.fn(),
  "agents:connectInfo": vi.fn(() => ({ cliPath: null, setups: [] })),
  "agents:copySetup": vi.fn(() => ""),
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

  it("accepts a preference write only for a known key and a value of its type (WP-1M.11)", () => {
    const h = handlers();
    const call = register(h);
    call("prefs:set", okEvent, "appearance.theme", "light");
    call("prefs:set", okEvent, "saving.interval", 120);
    expect(h["prefs:set"]).toHaveBeenCalledWith("appearance.theme", "light");
    for (const [key, value] of [
      ["appearance.theme", "purple"],
      ["appearance.theme", 3],
      ["saving.autosave", "yes"],
      ["saving.interval", 45],
      ["general.recent_count", 100],
      ["no.such.key", true],
      ["__proto__", {}],
      [42, "light"],
    ] as const) {
      expect(
        () => call("prefs:set", okEvent, key, value),
        `${String(key)}=${String(value)}`,
      ).toThrow(IpcValidationError);
    }
    expect(() => call("prefs:set", okEvent, "appearance.theme")).toThrow(IpcValidationError);
    expect(h["prefs:set"]).toHaveBeenCalledTimes(2);
  });

  it("never lets the renderer write the folders agents may use (WP-1J.05)", () => {
    const h = handlers();
    const call = register(h);
    for (const value of [["C:\\Users"], ["/home/me"], []]) {
      expect(() => call("prefs:set", okEvent, "agents.allowed_folders", value)).toThrow(
        IpcValidationError,
      );
    }
    expect(h["prefs:set"]).not.toHaveBeenCalled();
    // The rows a person may set, and resetting the folders (which only takes access away).
    call("prefs:set", okEvent, "agents.client_profile", "minimal");
    call("prefs:set", okEvent, "notify.agent_connected", false);
    call("prefs:reset", okEvent, ["agents.allowed_folders"]);
    expect(h["prefs:reset"]).toHaveBeenCalledWith(["agents.allowed_folders"]);
  });

  it("takes no folder to add, one bounded folder to remove and a known client to copy", () => {
    const h = handlers();
    const call = register(h);
    expect(() => call("agents:addFolder", okEvent, "C:\\Users")).toThrow(IpcValidationError);
    call("agents:addFolder", okEvent);
    expect(h["agents:addFolder"]).toHaveBeenCalledWith();
    call("agents:removeFolder", okEvent, "/footage");
    expect(h["agents:removeFolder"]).toHaveBeenCalledWith("/footage");
    for (const bad of ["", "a\0b", "x".repeat(5000), 3]) {
      expect(() => call("agents:removeFolder", okEvent, bad)).toThrow(IpcValidationError);
    }
    expect(() => call("agents:connectInfo", okEvent, "x")).toThrow(IpcValidationError);
    call("agents:copySetup", okEvent, "claude-code");
    expect(h["agents:copySetup"]).toHaveBeenCalledWith("claude-code");
    for (const bad of ["bash", "claude code --pid 1", 1]) {
      expect(() => call("agents:copySetup", okEvent, bad)).toThrow(IpcValidationError);
    }
  });

  it("resets only known keys, and takes no arguments to read, export or import", () => {
    const h = handlers();
    const call = register(h);
    call("prefs:reset", okEvent, ["saving.autosave", "appearance.theme"]);
    expect(h["prefs:reset"]).toHaveBeenCalledWith(["saving.autosave", "appearance.theme"]);
    expect(() => call("prefs:reset", okEvent, ["nope"])).toThrow(IpcValidationError);
    expect(() => call("prefs:reset", okEvent, "saving.autosave")).toThrow(IpcValidationError);
    for (const channel of ["prefs:get", "prefs:export", "prefs:import"]) {
      expect(() => call(channel, okEvent, "/etc/passwd"), channel).toThrow(IpcValidationError);
    }
    expect(() => call("shell:openAppFolder", okEvent, "/etc")).toThrow(IpcValidationError);
    call("shell:openAppFolder", okEvent, "logs");
    expect(h["shell:openAppFolder"]).toHaveBeenCalledWith("logs");
  });

  it("rejects preference calls from another frame or window", () => {
    const check = createSenderCheck({
      devServerOrigin: undefined,
      isEditorContents: (sender) => sender === editor,
    });
    const call = register(handlers(), check);
    expect(() =>
      call(
        "prefs:set",
        { sender: editor, senderFrame: { url: "https://evil.example/", parent: null } },
        "appearance.theme",
        "light",
      ),
    ).toThrow(IpcSenderError);
    expect(() => call("prefs:get", { sender: { id: 2 }, senderFrame: appFrame })).toThrow(
      IpcSenderError,
    );
    expect(() =>
      call("prefs:export", { sender: editor, senderFrame: { url: appFrame.url, parent: {} } }),
    ).toThrow(IpcSenderError);
  });

  it("takes no arguments on the media picker channel", () => {
    const h = handlers();
    const call = register(h);
    expect(() => call("dialog:pickMediaFiles", okEvent, "/etc")).toThrow(IpcValidationError);
    expect(() => call("dialog:pickMediaFiles", okEvent, { properties: ["openDirectory"] })).toThrow(
      IpcValidationError,
    );
    call("dialog:pickMediaFiles", okEvent);
    expect(h["dialog:pickMediaFiles"]).toHaveBeenCalledTimes(1);
  });

  it("takes one bounded path on the export file picker channel (WP-1M.08)", () => {
    const h = handlers();
    const call = register(h);
    expect(() => call("dialog:chooseExportFile", okEvent)).toThrow(IpcValidationError);
    expect(() => call("dialog:chooseExportFile", okEvent, 7)).toThrow(IpcValidationError);
    expect(() => call("dialog:chooseExportFile", okEvent, "")).toThrow(IpcValidationError);
    expect(() => call("dialog:chooseExportFile", okEvent, "a\0b.mp4")).toThrow(/NUL/);
    expect(() => call("dialog:chooseExportFile", okEvent, "a.mp4", "b")).toThrow(
      IpcValidationError,
    );
    call("dialog:chooseExportFile", okEvent, "C:/p/film.mp4");
    expect(h["dialog:chooseExportFile"]).toHaveBeenCalledWith("C:/p/film.mp4");
  });

  it("takes no arguments on the title bar's app and window actions (WP-1M.09)", () => {
    const h = handlers();
    const call = register(h);
    for (const channel of [
      "app:about",
      "app:quit",
      "window:toggleFullScreen",
      "window:reload",
      "window:toggleDevTools",
    ] as const) {
      expect(() => call(channel, okEvent, "x"), channel).toThrow(IpcValidationError);
      expect(() => call(channel, okEvent, { force: true }), channel).toThrow(IpcValidationError);
      call(channel, okEvent);
    }
    expect(h["app:quit"]).toHaveBeenCalledTimes(1);
    expect(h["window:toggleFullScreen"]).toHaveBeenCalledTimes(1);
    expect(h["window:reload"]).toHaveBeenCalledTimes(1);
    expect(h["window:toggleDevTools"]).toHaveBeenCalledTimes(1);
  });

  it("takes two whole page coordinates for Inspect element (WP-2H.12)", () => {
    const h = handlers();
    const call = register(h);
    call("window:inspectElement", okEvent, 10, 20);
    expect(h["window:inspectElement"]).toHaveBeenCalledWith(10, 20);
    for (const bad of [[], [1], [1.5, 2], [-1, 2], ["1", 2], [1, 2, 3], [1e9, 2]]) {
      expect(() => call("window:inspectElement", okEvent, ...bad)).toThrow(IpcValidationError);
    }
  });

  it("accepts only a known theme and a known edit action", () => {
    const h = handlers();
    const call = register(h);
    for (const bad of ["Dark", "high-contrast", "", 1, null, ["dark"], { theme: "dark" }]) {
      expect(() => call("window:setTheme", okEvent, bad), String(bad)).toThrow(IpcValidationError);
    }
    expect(() => call("window:setTheme", okEvent, "dark", "light")).toThrow(IpcValidationError);
    call("window:setTheme", okEvent, "light");
    expect(h["window:setTheme"]).toHaveBeenCalledWith("light");
    for (const bad of ["undo", "delete", "openDevTools", "paste\0", 3, undefined]) {
      expect(() => call("edit:run", okEvent, bad), String(bad)).toThrow(IpcValidationError);
    }
    for (const action of ["cut", "copy", "paste", "selectAll"]) call("edit:run", okEvent, action);
    expect(vi.mocked(h["edit:run"]).mock.calls).toEqual([
      ["cut"],
      ["copy"],
      ["paste"],
      ["selectAll"],
    ]);
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
