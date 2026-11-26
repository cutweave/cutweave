// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import {
  fitPopout,
  isAllowedPopoutNavigation,
  isFilledPopoutTitle,
  isPopoutUrl,
  livePopoutProblem,
  POPOUT_MIN,
  popoutRefusal,
  popoutWindowOptions,
} from "./popout.ts";
import { webPreferencesFor } from "./window-policy.ts";

const prod = {};
const dev = { devServerOrigin: "http://localhost:5173" };
const ctx = { preloadPath: "/bundle/preload/index.cjs", isDev: false };

const ask = (url: string, frameName = "1-2", openerIsEditor = true, policy = prod) =>
  popoutRefusal({ url, frameName, openerIsEditor }, policy);

describe("the popout rule (ADR-0031)", () => {
  it("allows an editor window's popout.html on the app origin with dockview's window name", () => {
    expect(ask("app://cutweave/popout.html")).toBeNull();
    expect(ask("app://cutweave/popout.html", "12-345")).toBeNull();
    expect(ask("http://localhost:5173/popout.html", "1-2", true, dev)).toBeNull();
  });

  it("denies the popout page to any window that is not an editor window", () => {
    expect(ask("app://cutweave/popout.html", "1-2", false)).toBe("not opened by an editor window");
  });

  it("denies every other page, origin, query or fragment", () => {
    for (const url of [
      "app://cutweave/",
      "app://cutweave/index.html",
      "app://cutweave/popout.html?x=1",
      "app://cutweave/popout.html#x",
      "app://cutweave/assets/popout.html",
      "app://cutweave@evil/popout.html",
      "app://other/popout.html",
      "https://example.com/popout.html",
      "http://localhost:5173/popout.html",
      "file:///C:/bundle/renderer/popout.html",
      "data:text/html,popout",
      "about:blank",
      "javascript:alert(1)",
      "",
    ]) {
      expect(ask(url), url).toBe("not the popout page");
    }
    expect(ask("http://localhost:5174/popout.html", "1-2", true, dev)).toBe("not the popout page");
  });

  it("denies a window name that is not dockview's `<dock>-<group>`", () => {
    for (const name of ["", "_blank", "popout", "1", "1-", "-2", "a-b", "1-2-3", "1-2 ", " 1-2"]) {
      expect(ask("app://cutweave/popout.html", name), JSON.stringify(name)).toBe(
        "not a dock's popout window name",
      );
    }
  });

  it("lets a popout's top frame show the popout page and go nowhere else", () => {
    expect(isAllowedPopoutNavigation("app://cutweave/popout.html", prod)).toBe(true);
    for (const url of ["app://cutweave/", "https://example.com/", "file:///etc/hosts"]) {
      expect(isAllowedPopoutNavigation(url, prod), url).toBe(false);
    }
    expect(isPopoutUrl("http://localhost:5173/popout.html", dev)).toBe(true);
    expect(isPopoutUrl("http://localhost:5173/popout.html", prod)).toBe(false);
  });
});

describe("the created popout, checked again", () => {
  it("passes only with no preload, no Node and the editor's process", () => {
    const ok = { sandbox: false, contextIsolation: true, nodeIntegration: false };
    expect(livePopoutProblem(ok)).toBeNull();
    expect(livePopoutProblem({ ...ok, preload: "" })).toBeNull();
    expect(livePopoutProblem({ ...ok, preload: ctx.preloadPath })).toBe("it has a preload");
    expect(livePopoutProblem({ ...ok, sandbox: true })).toMatch(/editor's process/);
    expect(livePopoutProblem({ contextIsolation: true })).toMatch(/editor's process/);
    expect(livePopoutProblem({ ...ok, nodeIntegration: true })).toBe("it has Node integration");
    expect(livePopoutProblem({ ...ok, contextIsolation: false })).toMatch(/isolation/);
    expect(livePopoutProblem(null)).not.toBeNull();
  });
});

describe("the popout window", () => {
  it("gets the popout policy (no preload), no menu bar and the theme's frame color", () => {
    const options = popoutWindowOptions(ctx, "#1b1b1b");
    expect(options.webPreferences).toEqual(webPreferencesFor("popout", ctx));
    expect(options.webPreferences?.sandbox).toBe(false);
    expect(options.webPreferences?.preload).toBeUndefined();
    expect(options).toMatchObject({
      autoHideMenuBar: true,
      useContentSize: true,
      backgroundColor: "#1b1b1b",
    });
    expect(options.parent).toBeUndefined();
    // WP-2H.15: hidden until the editor has moved its panels in.
    expect(options.show).toBe(false);
  });

  it("is shown only once the editor titled it after its panels (it has content)", () => {
    expect(isFilledPopoutTitle("Cutweave")).toBe(false);
    expect(isFilledPopoutTitle("")).toBe(false);
    expect(isFilledPopoutTitle("Inspector — Cutweave")).toBe(true);
    expect(isFilledPopoutTitle("Timeline — My video — Cutweave")).toBe(true);
  });

  it("opens on a connected display at a usable size", () => {
    const display = { x: 0, y: 0, width: 1920, height: 1040 };
    expect(fitPopout({ x: 200, y: 100, width: 400, height: 600 }, [display])).toEqual({
      x: 200,
      y: 100,
      width: 400,
      height: 600,
    });
    // Too small grows to the minimum; too big shrinks to the largest display.
    expect(fitPopout({ x: 10, y: 10, width: 20, height: 20 }, [display])).toEqual({
      x: 10,
      y: 10,
      ...POPOUT_MIN,
    });
    expect(fitPopout({ x: 0, y: 0, width: 5000, height: 3000 }, [display])).toEqual({
      x: 0,
      y: 0,
      width: 1920,
      height: 1040,
    });
    // Its monitor is gone: the position is dropped (main centers it), the size kept.
    expect(fitPopout({ x: 4000, y: 100, width: 400, height: 600 }, [display])).toEqual({
      width: 400,
      height: 600,
    });
    // A second monitor left of the first keeps a window on it.
    const left = { x: -1280, y: 0, width: 1280, height: 1000 };
    expect(fitPopout({ x: -900, y: 50, width: 400, height: 300 }, [display, left])).toMatchObject({
      x: -900,
      y: 50,
    });
  });
});
