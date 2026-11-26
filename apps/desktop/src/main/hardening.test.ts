// SPDX-License-Identifier: GPL-3.0-or-later
import { EventEmitter } from "node:events";
import type { Session, WebContents } from "electron";
import { describe, expect, it, vi } from "vitest";
import {
  ALLOWED_PERMISSIONS,
  type HardeningContext,
  hardenSession,
  hardenWebContents,
  isPermissionAllowed,
} from "./hardening.ts";
import { shouldInstallTestHooks } from "./test-hooks.ts";

const ctx = (over: Partial<HardeningContext> = {}): HardeningContext => ({
  isDev: false,
  enginePort: () => 43_117,
  log: () => {},
  ...over,
});

/** A cancellable Electron-style event. */
const evt = (fields: Record<string, unknown> = {}) => ({
  defaultPrevented: false,
  preventDefault() {
    this.defaultPrevented = true;
  },
  ...fields,
});

function fakeContents() {
  const emitter = new EventEmitter();
  let openHandler:
    | ((d: { url: string; frameName: string }) => {
        action: string;
        overrideBrowserWindowOptions?: Record<string, unknown>;
      })
    | undefined;
  const contents = Object.assign(emitter, {
    setWindowOpenHandler: (h: typeof openHandler) => {
      openHandler = h;
    },
    closeDevTools: vi.fn(),
  });
  return {
    contents,
    open: (url: string, frameName = "") => openHandler?.({ url, frameName }),
  };
}

describe("popout windows for detached panels (ADR-0031)", () => {
  const popoutOptions = { useContentSize: true, webPreferences: { sandbox: false } };
  const hooks = (kinds: Map<unknown, "editor" | "popout">) => ({
    isEditor: (c: unknown) => kinds.get(c) === "editor",
    isPopout: (c: unknown) => kinds.get(c) === "popout",
    options: () => popoutOptions,
    created: vi.fn(),
  });

  it("allows only an editor window's popout.html with dockview's window name", () => {
    const kinds = new Map<unknown, "editor" | "popout">();
    const popouts = hooks(kinds);
    const editor = fakeContents();
    const aux = fakeContents();
    kinds.set(editor.contents, "editor");
    for (const f of [editor, aux]) {
      hardenWebContents(f.contents as unknown as WebContents, ctx({ popouts }));
    }
    expect(editor.open("app://cutweave/popout.html", "1-2")).toEqual({
      action: "allow",
      overrideBrowserWindowOptions: popoutOptions,
    });
    // Not from another window, not another page, not another window name.
    expect(aux.open("app://cutweave/popout.html", "1-2")).toEqual({ action: "deny" });
    expect(editor.open("app://cutweave/", "1-2")).toEqual({ action: "deny" });
    expect(editor.open("app://cutweave/popout.html?x", "1-2")).toEqual({ action: "deny" });
    expect(editor.open("https://example.com/popout.html", "1-2")).toEqual({ action: "deny" });
    expect(editor.open("app://cutweave/popout.html", "")).toEqual({ action: "deny" });
    expect(editor.open("app://cutweave/popout.html", "_blank")).toEqual({ action: "deny" });

    // Electron made the allowed one: main checks and registers it.
    const child = { webContents: {} };
    const details = { url: "app://cutweave/popout.html", frameName: "1-2" };
    editor.contents.emit("did-create-window", child, details);
    expect(popouts.created).toHaveBeenCalledWith(child, details, editor.contents);
  });

  it("denies a popout's own window.open and every navigation away from its page", () => {
    const kinds = new Map<unknown, "editor" | "popout">();
    const popout = fakeContents();
    kinds.set(popout.contents, "popout");
    const log = vi.fn();
    hardenWebContents(
      popout.contents as unknown as WebContents,
      ctx({ popouts: hooks(kinds), log }),
    );
    expect(popout.open("app://cutweave/popout.html", "1-3")).toEqual({ action: "deny" });

    const navigate = (url: string, event = "will-frame-navigate") => {
      const e = evt({ url, isMainFrame: true });
      popout.contents.emit(event, e);
      return e.defaultPrevented ? "blocked" : "allowed";
    };
    expect(navigate("app://cutweave/popout.html")).toBe("allowed");
    // The editor's own page is still somewhere else: a popout never becomes a second editor.
    expect(navigate("app://cutweave/")).toBe("blocked");
    expect(navigate("https://example.com/")).toBe("blocked");
    expect(navigate("app://cutweave/", "will-navigate")).toBe("blocked");
    expect(navigate("app://cutweave/", "will-redirect")).toBe("blocked");
    expect(log).toHaveBeenCalledWith("navigation blocked: app://cutweave/");
  });

  it("denies every window.open without the popout hooks", () => {
    const { contents, open } = fakeContents();
    hardenWebContents(contents as unknown as WebContents, ctx());
    expect(open("app://cutweave/popout.html", "1-2")).toEqual({ action: "deny" });
    expect(contents.listenerCount("did-create-window")).toBe(0);
  });
});

describe("hardenWebContents", () => {
  it("denies window.open and blocks external, file and data navigation", () => {
    const { contents, open } = fakeContents();
    hardenWebContents(contents as unknown as WebContents, ctx());
    expect(open("https://example.com/")).toEqual({ action: "deny" });
    expect(open("app://cutweave/other")).toEqual({ action: "deny" });

    const navigate = (url: string, isMainFrame = true) => {
      const e = evt({ url, isMainFrame });
      contents.emit("will-frame-navigate", e);
      return e.defaultPrevented ? "blocked" : "allowed";
    };
    expect(navigate("app://cutweave/settings")).toBe("allowed");
    expect(navigate("https://example.com/")).toBe("blocked");
    expect(navigate("file:///C:/Windows/win.ini")).toBe("blocked");
    expect(navigate("data:text/html,x")).toBe("blocked");
    expect(navigate("about:blank", false)).toBe("allowed");
    expect(navigate("https://example.com/", false)).toBe("blocked");

    const legacy = evt({ url: "https://example.com/" });
    contents.emit("will-navigate", legacy);
    expect(legacy.defaultPrevented).toBe(true);
    const redirect = evt({ url: "https://example.com/", isMainFrame: true });
    contents.emit("will-redirect", redirect);
    expect(redirect.defaultPrevented).toBe(true);
  });

  it("refuses webviews and device pickers, and closes DevTools when packaged", () => {
    const { contents } = fakeContents();
    hardenWebContents(contents as unknown as WebContents, ctx());
    const attach = evt();
    contents.emit("will-attach-webview", attach);
    expect(attach.defaultPrevented).toBe(true);
    const pick = vi.fn();
    const bt = evt();
    contents.emit("select-bluetooth-device", bt, [{ deviceId: "a" }], pick);
    expect(bt.defaultPrevented).toBe(true);
    expect(pick).toHaveBeenCalledWith("");
    contents.emit("devtools-opened");
    expect(contents.closeDevTools).toHaveBeenCalled();
  });

  it("leaves DevTools alone in dev", () => {
    const { contents } = fakeContents();
    hardenWebContents(contents as unknown as WebContents, ctx({ isDev: true }));
    contents.emit("devtools-opened");
    expect(contents.closeDevTools).not.toHaveBeenCalled();
  });
});

function fakeSession() {
  const emitter = new EventEmitter();
  const handlers: Record<string, (...a: never[]) => unknown> = {};
  const capture = (name: string) => (h: never) => {
    handlers[name] = h;
  };
  const ses = Object.assign(emitter, {
    setPermissionRequestHandler: capture("request"),
    setPermissionCheckHandler: capture("check"),
    setDevicePermissionHandler: capture("device"),
    setBluetoothPairingHandler: capture("pairing"),
    webRequest: {
      onBeforeRequest: capture("beforeRequest"),
      onHeadersReceived: capture("headers"),
    },
  });
  return { ses, handlers };
}

describe("hardenSession", () => {
  it("denies every permission request and check", () => {
    const { ses, handlers } = fakeSession();
    hardenSession(ses as unknown as Session, ctx());
    for (const permission of [
      "media",
      "geolocation",
      "notifications",
      "clipboard-read",
      "openExternal",
      "fullscreen",
    ]) {
      const cb = vi.fn();
      (handlers.request as (...a: unknown[]) => void)(null, permission, cb, {
        requestingUrl: "app://cutweave/",
      });
      expect(cb, permission).toHaveBeenCalledWith(false);
      expect(
        (handlers.check as (...a: unknown[]) => boolean)(null, permission, "app://cutweave"),
      ).toBe(false);
    }
    expect((handlers.device as () => boolean)()).toBe(false);
  });

  it("cancels device pickers and downloads", () => {
    const { ses } = fakeSession();
    hardenSession(ses as unknown as Session, ctx());
    const hid = vi.fn();
    const e1 = evt();
    ses.emit("select-hid-device", e1, {}, hid);
    expect(e1.defaultPrevented).toBe(true);
    expect(hid).toHaveBeenCalledWith(null);
    const serial = vi.fn();
    const e2 = evt();
    ses.emit("select-serial-port", e2, [], {}, serial);
    expect(serial).toHaveBeenCalledWith("");
    const usb = vi.fn();
    const e3 = evt();
    ses.emit("select-usb-device", e3, {}, usb);
    expect(usb).toHaveBeenCalledWith();
    const dl = evt();
    ses.emit("will-download", dl);
    expect(dl.defaultPrevented).toBe(true);
  });

  it("filters requests against the current engine port", () => {
    let port: number | null = 43_117;
    const { ses, handlers } = fakeSession();
    hardenSession(ses as unknown as Session, ctx({ enginePort: () => port }));
    const decide = (url: string, fromMain = false) => {
      const webContentsId = fromMain ? undefined : 1;
      const cb = vi.fn();
      (handlers.beforeRequest as (...a: unknown[]) => void)(
        { url, webContentsId, resourceType: "xhr" },
        cb,
      );
      return cb.mock.calls[0]?.[0];
    };
    expect(decide("http://127.0.0.1:43117/rpc")).toEqual({ cancel: false });
    expect(decide("https://example.com/")).toEqual({ cancel: true });
    expect(decide("file:///etc/passwd")).toEqual({ cancel: true });
    expect(decide("file:///bundle/index.html", true)).toEqual({ cancel: false });
    port = 50_001;
    expect(decide("http://127.0.0.1:43117/rpc")).toEqual({ cancel: true });
    expect(decide("http://127.0.0.1:50001/rpc")).toEqual({ cancel: false });
    expect(handlers.headers).toBeUndefined();
  });

  it("is idempotent and adds the dev CSP only for the dev server", () => {
    const { ses, handlers } = fakeSession();
    const devCtx = ctx({ isDev: true, devServerOrigin: "http://localhost:5173" });
    hardenSession(ses as unknown as Session, devCtx);
    const first = handlers.beforeRequest;
    hardenSession(ses as unknown as Session, devCtx);
    expect(handlers.beforeRequest).toBe(first);

    const onHeaders = handlers.headers as (...a: unknown[]) => void;
    const devDoc = vi.fn();
    onHeaders(
      { url: "http://localhost:5173/", responseHeaders: { "content-security-policy": ["x"] } },
      devDoc,
    );
    const csp = devDoc.mock.calls[0]?.[0].responseHeaders["Content-Security-Policy"][0];
    expect(csp).toContain("connect-src http://127.0.0.1:43117 ws://127.0.0.1:43117");
    expect(devDoc.mock.calls[0]?.[0].responseHeaders["content-security-policy"]).toBeUndefined();
    const engine = vi.fn();
    onHeaders({ url: "http://127.0.0.1:43117/rpc", responseHeaders: {} }, engine);
    expect(engine).toHaveBeenCalledWith({});
  });
});

describe("permission policy", () => {
  it("grants nothing in Phase 1", () => {
    expect(ALLOWED_PERMISSIONS).toEqual([]);
    expect(isPermissionAllowed("media", "app://cutweave/", {})).toBe(false);
  });
});

describe("test hooks", () => {
  it("install only for unpackaged runs that ask for them", () => {
    expect(shouldInstallTestHooks(false, { CUTWEAVE_TEST_HOOKS: "1" })).toBe(true);
    expect(shouldInstallTestHooks(true, { CUTWEAVE_TEST_HOOKS: "1" })).toBe(false);
    expect(shouldInstallTestHooks(false, {})).toBe(false);
    expect(shouldInstallTestHooks(false, { CUTWEAVE_TEST_HOOKS: "true" })).toBe(false);
  });
});
