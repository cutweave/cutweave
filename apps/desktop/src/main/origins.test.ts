// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import {
  isAllowedFrameNavigation,
  isAllowedNavigation,
  isAllowedRequest,
  isAppUrl,
  isTrustedOrigin,
  parseExternalUrl,
  type RequestPolicy,
} from "./origins.ts";

const prod = {};
const dev = { devServerOrigin: "http://localhost:5173" };

describe("isAppUrl / isTrustedOrigin", () => {
  it("accepts only app://cutweave without credentials", () => {
    expect(isAppUrl("app://cutweave/")).toBe(true);
    expect(isAppUrl("app://cutweave/assets/a.js?x#y")).toBe(true);
    expect(isAppUrl("app://cutweave@evil/")).toBe(false);
    expect(isAppUrl("app://user:pw@cutweave/")).toBe(false);
    expect(isAppUrl("app://cutweave.evil/")).toBe(false);
    expect(isAppUrl("app://evil/")).toBe(false);
    expect(isAppUrl("https://cutweave/")).toBe(false);
    expect(isAppUrl("garbage")).toBe(false);
  });

  it("trusts the dev server only in dev", () => {
    expect(isTrustedOrigin("http://localhost:5173/src/main.tsx", dev)).toBe(true);
    expect(isTrustedOrigin("http://localhost:5173/", prod)).toBe(false);
    expect(isTrustedOrigin("http://localhost:5174/", dev)).toBe(false);
    expect(isTrustedOrigin("http://user@localhost:5173/", dev)).toBe(false);
  });
});

describe("navigation", () => {
  it("allows top-level navigation to the app origin only", () => {
    expect(isAllowedNavigation("app://cutweave/settings", prod)).toBe(true);
    for (const url of [
      "https://example.com/",
      "http://127.0.0.1:1234/",
      "file:///C:/Windows/win.ini",
      "file:///etc/passwd",
      "data:text/html,hi",
      "javascript:alert(1)",
      "about:blank",
      "cutweave-plugin://p/index.html",
      "chrome://gpu",
    ]) {
      expect(isAllowedNavigation(url, prod), url).toBe(false);
    }
  });

  it("allows subframes on the app origin, inert documents and the plugin scheme", () => {
    expect(isAllowedFrameNavigation("about:blank", prod)).toBe(true);
    expect(isAllowedFrameNavigation("about:srcdoc", prod)).toBe(true);
    expect(isAllowedFrameNavigation("app://cutweave/panel", prod)).toBe(true);
    expect(isAllowedFrameNavigation("cutweave-plugin://captions/index.html", prod)).toBe(true);
    expect(isAllowedFrameNavigation("https://example.com/", prod)).toBe(false);
    expect(isAllowedFrameNavigation("file:///etc/passwd", prod)).toBe(false);
    expect(isAllowedFrameNavigation("data:text/html,x", prod)).toBe(false);
  });
});

describe("parseExternalUrl", () => {
  it("accepts https links with a host", () => {
    expect(parseExternalUrl("https://cutweave.org/docs?q=1#a")).toBe(
      "https://cutweave.org/docs?q=1#a",
    );
  });

  it("rejects every other scheme, credentials and oversize input", () => {
    for (const url of [
      "",
      "http://example.com",
      "file:///etc/passwd",
      "javascript:alert(1)",
      "mailto:a@b.c",
      "app://cutweave/",
      "smb://host/share",
      "https://user:pw@example.com",
      "https://user@example.com",
      `https://example.com/${"a".repeat(2048)}`,
    ]) {
      expect(parseExternalUrl(url), url).toBeNull();
    }
  });
});

describe("isAllowedRequest", () => {
  const policy: RequestPolicy = { enginePort: 43_117, isDev: false };

  it("allows the app origin, inline resources and the engine origin", () => {
    expect(isAllowedRequest("app://cutweave/assets/a.js", policy, false)).toBe(true);
    expect(isAllowedRequest("data:image/png;base64,AA==", policy, false)).toBe(true);
    expect(isAllowedRequest("blob:app://cutweave/1234", policy, false)).toBe(true);
    expect(isAllowedRequest("http://127.0.0.1:43117/media/x/thumb", policy, false)).toBe(true);
    expect(isAllowedRequest("ws://127.0.0.1:43117/rpc", policy, false)).toBe(true);
  });

  it("cancels remote content, other loopback ports and page file:// loads", () => {
    for (const url of [
      "https://example.com/",
      "http://example.com/",
      "wss://example.com/",
      "http://127.0.0.1:43118/",
      "http://localhost:43117/",
      "http://u@127.0.0.1:43117/",
      "file:///etc/passwd",
      "app://evil/",
      "chrome-extension://abc/x.js",
      "devtools://devtools/bundled/inspector.html",
      "cutweave-plugin://p/",
    ]) {
      expect(isAllowedRequest(url, policy, false), url).toBe(false);
    }
    expect(
      isAllowedRequest("http://127.0.0.1:43117/", { ...policy, enginePort: null }, false),
    ).toBe(false);
  });

  it("lets main's protocol handler read files, and DevTools load in dev", () => {
    expect(isAllowedRequest("file:///bundle/renderer/index.html", policy, true)).toBe(true);
    expect(isAllowedRequest("https://example.com/", policy, true)).toBe(false);
    const devPolicy = { ...policy, ...dev, isDev: true };
    expect(isAllowedRequest("devtools://devtools/bundled/inspector.html", devPolicy, false)).toBe(
      true,
    );
    expect(isAllowedRequest("http://localhost:5173/@vite/client", devPolicy, false)).toBe(true);
    expect(isAllowedRequest("ws://localhost:5173/", devPolicy, false)).toBe(true);
    expect(isAllowedRequest("http://localhost:5174/", devPolicy, false)).toBe(false);
  });
});
