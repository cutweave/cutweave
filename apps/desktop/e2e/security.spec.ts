// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1L.02 security suite (arch §18). Every test runs against the built app with the fake engine.
// No test touches the network: each blocked request is stopped before it leaves the app.

import { createServer } from "node:net";
import path from "node:path";
import type { ElectronApplication, Page } from "@playwright/test";
import { desktopDir, expect, launchApp, test } from "./fixtures.ts";

const waitForEngine = (page: Page) =>
  expect
    .poll(() =>
      page
        .evaluate(async () => (await window.cutweave.engine.getStatus()).state)
        .catch(() => "navigating"),
    )
    .toBe("ready");

/** Fetch a URL or load it in an iframe; report the first CSP violation it caused (or null). */
async function violationOf(
  page: Page,
  action: { fetch: string } | { iframe: string },
): Promise<string | null> {
  return page.evaluate(async (a) => {
    const violation = new Promise<string | null>((resolve) => {
      document.addEventListener(
        "securitypolicyviolation",
        (e) => resolve(`${e.effectiveDirective} ${e.blockedURI}`),
        { once: true },
      );
      setTimeout(() => resolve(null), 2000);
    });
    if ("fetch" in a) {
      await fetch(a.fetch).catch(() => {});
    } else {
      const frame = document.createElement("iframe");
      frame.src = a.iframe;
      document.body.append(frame);
    }
    return violation;
  }, action);
}

test.describe("content security policy", () => {
  test("app:// documents carry the strict CSP for the engine port", async ({ app, page }) => {
    await waitForEngine(page);
    const port = await app.evaluate(() => globalThis.__cutweaveTest?.enginePort());
    const response = await page.reload();
    const csp = response?.headers()["content-security-policy"] ?? "";
    expect(csp).toContain("default-src 'none'");
    expect(csp).toContain("script-src 'self';");
    expect(csp).toContain(`connect-src http://127.0.0.1:${port} ws://127.0.0.1:${port};`);
    expect(csp).toContain("frame-src cutweave-plugin:;");
    expect(csp).not.toContain("unsafe");
  });

  test("the editor loads without a single CSP violation", async ({ page, mainLog }) => {
    await page.addInitScript(() => {
      const seen: string[] = [];
      (window as unknown as { __violations: string[] }).__violations = seen;
      document.addEventListener("securitypolicyviolation", (e) =>
        seen.push(`${e.effectiveDirective} ${e.blockedURI}`),
      );
    });
    await page.reload();
    await expect(page.locator(".cw-topbar")).toBeVisible();
    await waitForEngine(page);
    await page.waitForTimeout(500);
    expect(
      await page.evaluate(() => (window as unknown as { __violations: string[] }).__violations),
    ).toEqual([]);
    // Nor did the session filter, guards or permission handler have to block anything.
    expect(mainLog.filter((line) => line.includes("[security]"))).toEqual([]);
  });

  test("fetch to a foreign origin is blocked by connect-src", async ({ page }) => {
    const violation = await page.evaluate(async () => {
      const seen = new Promise<string>((resolve) =>
        document.addEventListener(
          "securitypolicyviolation",
          (e) => resolve(`${e.effectiveDirective} ${e.blockedURI}`),
          { once: true },
        ),
      );
      const outcome = await fetch("http://example.com/").then(
        () => "fetched",
        () => "rejected",
      );
      return `${outcome}: ${await seen}`;
    });
    expect(violation).toBe("rejected: connect-src http://example.com/");
  });

  test("only the engine's own loopback port is reachable; /rpc needs the token", async ({
    page,
  }) => {
    await waitForEngine(page);
    const status = await page.evaluate(async () => {
      const connection = await window.cutweave.engine.getConnection();
      return (await fetch(`${connection?.httpUrl}/rpc`)).status;
    });
    expect(status).toBe(401);
    const port = await page.evaluate(
      async () => (await window.cutweave.engine.getConnection())?.port,
    );
    expect(await violationOf(page, { fetch: `http://127.0.0.1:${(port ?? 0) + 1}/rpc` })).toBe(
      `connect-src http://127.0.0.1:${(port ?? 0) + 1}/rpc`,
    );
    expect(await violationOf(page, { fetch: `http://localhost:${port}/rpc` })).toBe(
      `connect-src http://localhost:${port}/rpc`,
    );
  });

  test("eval and new Function are refused", async ({ page }) => {
    // DevTools-evaluated code may bypass the eval check, so attempt it from a page task.
    const results = await page.evaluate(
      () =>
        new Promise((resolve) =>
          setTimeout(() => {
            const attempt = (fn: () => unknown) => {
              try {
                fn();
                return "ran";
              } catch (err) {
                return (err as Error).name;
              }
            };
            resolve({
              // biome-ignore lint/security/noGlobalEval: asserting the CSP blocks it
              eval: attempt(() => eval("1 + 1")),
              func: attempt(() => new Function("return 1")()),
            });
          }, 0),
        ),
    );
    expect(results).toEqual({ eval: "EvalError", func: "EvalError" });
  });
});

/** Assign `location.href` from a page task (a real renderer-initiated navigation). */
const navigateFromPage = (page: Page, url: string) =>
  page.evaluate((target) => {
    setTimeout(() => {
      window.location.href = target;
    }, 0);
  }, url);

/** The editor document is still the one that was loaded (no navigation happened). */
const stillTheEditor = (page: Page) =>
  page.evaluate(() => ({
    href: location.href,
    marker: (window as unknown as { __marker?: string }).__marker,
  }));

test.describe("navigation and windows", () => {
  test("navigation to https://example.com is blocked", async ({ page, mainLog }) => {
    await page.evaluate(() => {
      (window as unknown as { __marker?: string }).__marker = "same-document";
    });
    await navigateFromPage(page, "https://example.com/");
    await expect
      .poll(() => mainLog.some((l) => l.includes("navigation blocked: https://example.com/")))
      .toBe(true);
    expect(await stillTheEditor(page)).toEqual({
      href: "app://cutweave/",
      marker: "same-document",
    });
  });

  test("file:// navigation outside the bundle is blocked", async ({ page, mainLog }) => {
    const target =
      process.platform === "win32" ? "file:///C:/Windows/win.ini" : "file:///etc/hosts";
    const consoleErrors: string[] = [];
    page.on("console", (m) => consoleErrors.push(m.text()));
    await page.evaluate(() => {
      (window as unknown as { __marker?: string }).__marker = "same-document";
    });
    await navigateFromPage(page, target);
    // Chromium refuses file:// from app:// itself; the will-frame-navigate guard is the backstop.
    await expect
      .poll(
        () =>
          mainLog.some((l) => l.includes(`navigation blocked: ${target}`)) ||
          consoleErrors.some((m) => m.includes(`Not allowed to load local resource: ${target}`)),
      )
      .toBe(true);
    expect(await stillTheEditor(page)).toEqual({
      href: "app://cutweave/",
      marker: "same-document",
    });
  });

  test("window.open returns null", async ({ app, page }) => {
    const opened = await page.evaluate(() => ({
      external: window.open("https://example.com/") === null,
      app: window.open("app://cutweave/") === null,
      blank: window.open("about:blank", "_blank") === null,
    }));
    expect(opened).toEqual({ external: true, app: true, blank: true });
    expect(app.windows()).toHaveLength(1);
  });

  test("external https links open only through the allowlisted IPC", async ({ app, page }) => {
    await app.evaluate(() => globalThis.__cutweaveTest?.captureOpenExternal());
    await page.evaluate(() => window.cutweave.shell.openExternal("https://cutweave.org/docs"));
    const refused = await page.evaluate(async () =>
      Promise.all(
        ["http://example.com/", "file:///etc/passwd", "javascript:alert(1)"].map((url) =>
          window.cutweave.shell.openExternal(url).then(
            () => "opened",
            (err: Error) => err.message,
          ),
        ),
      ),
    );
    for (const message of refused)
      expect(message).toMatch(/invalid IPC call on shell:openExternal/);
    expect(await app.evaluate(() => globalThis.__cutweaveTest?.openedExternal())).toEqual([
      "https://cutweave.org/docs",
    ]);
  });

  test("a second window from the factory is sandboxed and has no bridge", async ({ app }) => {
    const [aux, id] = await Promise.all([
      app.waitForEvent("window"),
      app.evaluate(() => globalThis.__cutweaveTest?.createWindow("auxiliary")),
    ]);
    await aux.waitForLoadState("domcontentloaded");
    expect(
      await aux.evaluate(() => ({
        cutweave: typeof (window as { cutweave?: unknown }).cutweave,
        preview: typeof (window as { cutweave?: { preview?: unknown } }).cutweave?.preview,
        require: typeof (window as { require?: unknown }).require,
        process: typeof (window as { process?: unknown }).process,
      })),
    ).toEqual({
      cutweave: "undefined",
      preview: "undefined",
      require: "undefined",
      process: "undefined",
    });

    const info = await app.evaluate(({ app: electronApp, BrowserWindow }, windowId) => {
      const describe = (win: Electron.BrowserWindow | null | undefined) => {
        if (!win) return null;
        const pid = win.webContents.getOSProcessId();
        const metric = electronApp.getAppMetrics().find((m) => m.pid === pid);
        return { pid, osSandboxed: metric?.sandboxed ?? null };
      };
      const main = BrowserWindow.getAllWindows().find((w) => w.id !== windowId);
      return { aux: describe(BrowserWindow.fromId(windowId ?? -1)), main: describe(main) };
    }, id);
    expect(info.aux?.pid).not.toBe(info.main?.pid);
    // getAppMetrics reports the OS sandbox on Windows and macOS only.
    if (process.platform !== "linux") {
      expect(info.aux?.osSandboxed).toBe(true);
      expect(info.main?.osSandboxed).toBe(false);
    }
  });

  test("the factory refuses a non-sandboxed or preloaded non-main window", async ({ app }) => {
    // A popout window exists only as an editor window's `window.open` (ADR-0031).
    expect(
      await app.evaluate(({ BrowserWindow }) => {
        const before = BrowserWindow.getAllWindows().length;
        try {
          globalThis.__cutweaveTest?.createWindow("popout");
          return "created";
        } catch (err) {
          return `${(err as Error).name}: ${(err as Error).message} (${BrowserWindow.getAllWindows().length - before} new)`;
        }
      }),
    ).toBe(
      "WindowPolicyError: popout window: only an editor window opens one (window.open) (0 new)",
    );
    const attempt = (prefs: Record<string, unknown>, kind = "auxiliary") =>
      app.evaluate(
        ({ BrowserWindow }, [k, p]) => {
          const before = BrowserWindow.getAllWindows().length;
          try {
            globalThis.__cutweaveTest?.createWindow(k as string, p as Record<string, unknown>);
            return "created";
          } catch (err) {
            return `${(err as Error).name}: ${(err as Error).message} (${BrowserWindow.getAllWindows().length - before} new)`;
          }
        },
        [kind, prefs] as const,
      );
    expect(await attempt({ sandbox: false })).toBe(
      "WindowPolicyError: auxiliary window: must be sandboxed (sandbox: true) (0 new)",
    );
    expect(
      await attempt({ preload: path.join(desktopDir, "dist", "preload", "index.cjs") }),
    ).toMatch(/^WindowPolicyError: .*preload.*\(0 new\)$/);
    expect(await attempt({ contextIsolation: false }, "editor")).toMatch(
      /^WindowPolicyError: editor window: webPreferences.contextIsolation must be true/,
    );
    expect(await attempt({ nodeIntegration: true }, "editor")).toMatch(/^WindowPolicyError/);
  });

  test("an iframe gets no bridge, and frame-src admits only the plugin scheme", async ({
    page,
  }) => {
    const result = await page.evaluate(() => {
      const frame = document.createElement("iframe");
      document.body.append(frame);
      const child = frame.contentWindow as
        | (Window & { cutweave?: unknown; require?: unknown })
        | null;
      const out = { cutweave: typeof child?.cutweave, require: typeof child?.require };
      frame.remove();
      return out;
    });
    expect(result).toEqual({ cutweave: "undefined", require: "undefined" });
    // Chromium reports a non-http(s) blocked URI by its scheme only.
    expect(await violationOf(page, { iframe: "app://cutweave/" })).toBe("frame-src app");
    expect(await violationOf(page, { iframe: "https://example.com/" })).toBe(
      "frame-src https://example.com",
    );
    // The plugin scheme passes the CSP; main's stub handler answers 404 until P6.
    expect(await violationOf(page, { iframe: "cutweave-plugin://captions/index.html" })).toBeNull();
  });
});

/**
 * Opens a detached panel's window the way the dock does (dockview's `window.open` of
 * `popout.html` under its `<dock>-<group>` name) and resolves with its page.
 */
async function openPopout(app: ElectronApplication, page: Page, name = "1-99"): Promise<Page> {
  const [child] = await Promise.all([
    app.waitForEvent("window"),
    page.evaluate((target) => {
      const opened = window.open(
        "/popout.html",
        target,
        "popup,left=40,top=40,width=320,height=240",
      );
      (window as unknown as { __popout?: Window | null }).__popout = opened;
    }, name),
  ]);
  await child.waitForLoadState("domcontentloaded");
  await expect.poll(() => child.url()).toBe("app://cutweave/popout.html");
  return child;
}

/** What main knows of each window: its URL, renderer process, preload and sandbox. */
const windowsInMain = (app: ElectronApplication) =>
  app.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows().map((w) => {
      const prefs = (
        w.webContents as unknown as {
          getLastWebPreferences?: () => { preload?: string; sandbox?: boolean } | null;
        }
      ).getLastWebPreferences?.();
      return {
        url: w.webContents.getURL(),
        pid: w.webContents.getOSProcessId(),
        preload: prefs?.preload ?? "",
        sandbox: prefs?.sandbox ?? null,
        onTop: w.isAlwaysOnTop(),
      };
    }),
  );

test.describe("detached panels' windows (ADR-0031)", () => {
  test("a popout shares the editor's process with no preload, no bridge and no Node", async ({
    app,
    page,
  }) => {
    const child = await openPopout(app, page);
    expect(
      await child.evaluate(() => ({
        cutweave: typeof (window as { cutweave?: unknown }).cutweave,
        require: typeof (window as { require?: unknown }).require,
        process: typeof (window as { process?: unknown }).process,
      })),
    ).toEqual({ cutweave: "undefined", require: "undefined", process: "undefined" });
    // The editor reaches its document: it is the same agent, which is what the dock needs.
    expect(
      await page.evaluate(
        () => (window as unknown as { __popout?: Window | null }).__popout?.document.URL,
      ),
    ).toBe("app://cutweave/popout.html");

    const windows = await windowsInMain(app);
    const editor = windows.find((w) => w.url === "app://cutweave/");
    const popout = windows.find((w) => w.url === "app://cutweave/popout.html");
    expect(popout).toMatchObject({ preload: "", sandbox: false, pid: editor?.pid });
    // So the addon is loaded once, by the editor's preload: the only window with one.
    expect(windows.filter((w) => w.preload !== "").map((w) => w.url)).toEqual(["app://cutweave/"]);
    expect(await page.evaluate(() => window.cutweave.preview.isAvailable())).toBe(true);
  });

  test("a popout's document has the app CSP", async ({ app, page }) => {
    const child = await openPopout(app, page);
    expect(await violationOf(child, { fetch: "http://example.com/" })).toBe(
      "connect-src http://example.com/",
    );
    const refused = await child.evaluate(
      () =>
        new Promise((resolve) =>
          setTimeout(() => {
            try {
              // biome-ignore lint/security/noGlobalEval: asserting the CSP blocks it
              eval("1 + 1");
              resolve("ran");
            } catch (err) {
              resolve((err as Error).name);
            }
          }, 0),
        ),
    );
    expect(refused).toBe("EvalError");
  });

  test("a popout cannot navigate or open windows of its own", async ({ app, page, mainLog }) => {
    const child = await openPopout(app, page);
    expect(
      await child.evaluate(() => ({
        popout: window.open("/popout.html", "1-98", "popup") === null,
        app: window.open("app://cutweave/") === null,
      })),
    ).toEqual({ popout: true, app: true });
    // Not to the web, and not to the editor's own page either.
    for (const target of ["https://example.com/", "app://cutweave/"]) {
      await navigateFromPage(child, target);
      await expect
        .poll(() => mainLog.some((l) => l.includes(`navigation blocked: ${target}`)))
        .toBe(true);
      expect(child.url()).toBe("app://cutweave/popout.html");
    }
    expect((await windowsInMain(app)).length).toBe(2);
  });

  test("only an editor window may open popout.html, and only under the dock's names", async ({
    app,
    page,
  }) => {
    const refused = await page.evaluate(() => ({
      name: window.open("/popout.html", "popout", "popup") === null,
      blank: window.open("/popout.html", "_blank", "popup") === null,
      query: window.open("/popout.html?x=1", "1-5", "popup") === null,
      page: window.open("/index.html", "1-5", "popup") === null,
      foreign: window.open("https://example.com/popout.html", "1-5", "popup") === null,
    }));
    expect(refused).toEqual({ name: true, blank: true, query: true, page: true, foreign: true });

    // A sandboxed window from the factory loads the app origin too, and is refused.
    const [aux] = await Promise.all([
      app.waitForEvent("window"),
      app.evaluate(() => globalThis.__cutweaveTest?.createWindow("auxiliary")),
    ]);
    await aux.waitForLoadState("domcontentloaded");
    expect(await aux.evaluate(() => window.open("/popout.html", "1-6", "popup") === null)).toBe(
      true,
    );
    expect(app.windows()).toHaveLength(2);
  });

  test("Keep on top reaches only a popout this editor opened, by its name", async ({
    app,
    page,
  }) => {
    const setOnTop = (frame: string, on: boolean) =>
      page.evaluate(
        ([f, o]) =>
          window.cutweave.window.setPopoutOnTop(f as string, o as boolean).then(
            (found) => found,
            (err: Error) => err.message,
          ),
        [frame, on] as const,
      );
    expect(await setOnTop("1-99", true)).toBe(false);
    expect(await setOnTop("main", true)).toMatch(
      /IpcValidationError: invalid IPC call on window:setPopoutOnTop/,
    );
    await openPopout(app, page, "1-99");
    expect(await setOnTop("1-99", true)).toBe(true);
    const popout = (await windowsInMain(app)).find((w) => w.url.endsWith("/popout.html"));
    expect(popout?.onTop).toBe(true);
    expect(await setOnTop("1-99", false)).toBe(true);
  });
});

test.describe("IPC", () => {
  test("an invalid-args call is rejected with a zod error", async ({ page }) => {
    const message = await page.evaluate(() =>
      window.cutweave.shell.showItemInFolder(42 as unknown as string).then(
        () => "accepted",
        (err: Error) => err.message,
      ),
    );
    expect(message).toMatch(/IpcValidationError: invalid IPC call on shell:showItemInFolder/);
    expect(message).toMatch(/expected string, received number/);
  });

  test("the title bar's channels take only a known theme and edit action (WP-1M.09)", async ({
    page,
  }) => {
    const refusals = await page.evaluate(async () => {
      const outcome = (call: Promise<unknown>) =>
        call.then(
          () => "accepted",
          (err: Error) => err.message,
        );
      return {
        theme: await outcome(window.cutweave.window.setTheme("neon" as never)),
        edit: await outcome(window.cutweave.edit.run("openDevTools" as never)),
        ok: await outcome(window.cutweave.window.setTheme("dark")),
      };
    });
    expect(refusals.theme).toMatch(/IpcValidationError: invalid IPC call on window:setTheme/);
    expect(refusals.edit).toMatch(/IpcValidationError: invalid IPC call on edit:run/);
    expect(refusals.ok).toBe("accepted");
  });

  test("the bridge exposes named functions only (no generic invoke)", async ({ page }) => {
    const surface = await page.evaluate(() => {
      const walk = (value: unknown, prefix: string): string[] =>
        typeof value === "object" && value !== null
          ? Object.keys(value).flatMap((k) =>
              walk((value as Record<string, unknown>)[k], `${prefix}.${k}`),
            )
          : [prefix];
      return walk(window.cutweave, "cutweave").sort();
    });
    expect(surface).toEqual([
      "cutweave.agents.addFolder",
      "cutweave.agents.connectInfo",
      "cutweave.agents.copySetup",
      "cutweave.agents.removeFolder",
      "cutweave.app.about",
      "cutweave.app.getInfo",
      "cutweave.app.quit",
      "cutweave.dialog.chooseExportFile",
      "cutweave.dialog.openProjectFolder",
      "cutweave.dialog.pickMediaFiles",
      "cutweave.dialog.pickMediaFolders",
      "cutweave.edit.run",
      "cutweave.engine.getConnection",
      "cutweave.engine.getStatus",
      "cutweave.engine.onStatus",
      "cutweave.files.pathForFile",
      "cutweave.menu.onCommand",
      "cutweave.prefs.exportToFile",
      "cutweave.prefs.get",
      "cutweave.prefs.importFromFile",
      "cutweave.prefs.onChanged",
      "cutweave.prefs.reset",
      "cutweave.prefs.set",
      "cutweave.preview.attachCanvas",
      "cutweave.preview.close",
      "cutweave.preview.detachCanvas",
      "cutweave.preview.getClock",
      "cutweave.preview.getLatestFrame",
      "cutweave.preview.getPresentStats",
      "cutweave.preview.getStatus",
      "cutweave.preview.isAvailable",
      "cutweave.preview.open",
      "cutweave.preview.redraw",
      "cutweave.preview.startPresenting",
      "cutweave.preview.stopPresenting",
      "cutweave.preview.waitForFrame",
      "cutweave.recent.add",
      "cutweave.recent.clear",
      "cutweave.recent.list",
      "cutweave.recent.remove",
      "cutweave.shell.openAppFolder",
      "cutweave.shell.openExternal",
      "cutweave.shell.showItemInFolder",
      "cutweave.window.confirmClose",
      "cutweave.window.inspectElement",
      "cutweave.window.onCloseRequested",
      "cutweave.window.reload",
      "cutweave.window.setDirty",
      "cutweave.window.setPopoutOnTop",
      "cutweave.window.setTheme",
      "cutweave.window.toggleDevTools",
      "cutweave.window.toggleFullScreen",
    ]);
  });

  test("forged IPC from a non-app origin is rejected", async ({ app }) => {
    const [foreign] = await Promise.all([
      app.waitForEvent("window"),
      app.evaluate(() =>
        globalThis.__cutweaveTest?.openEditorAt("data:text/html,<title>foreign</title>"),
      ),
    ]);
    await foreign.waitForURL(/^data:/);
    const message = await foreign.evaluate(() =>
      window.cutweave.app.getInfo().then(
        () => "accepted",
        (err: Error) => err.message,
      ),
    );
    expect(message).toMatch(/IpcSenderError: IPC on app:getInfo rejected: untrusted sender origin/);
  });

  test("a compromised renderer cannot reach unregistered channels or pose as the editor", async ({
    app,
  }) => {
    const preload = path.join(desktopDir, "test-support", "forge-preload.cjs");
    const [forge] = await Promise.all([
      app.waitForEvent("window"),
      app.evaluate(({ BrowserWindow }, preloadPath) => {
        // Deliberately outside the factory: an attacker-controlled sender.
        const win = new BrowserWindow({
          show: false,
          webPreferences: { preload: preloadPath, sandbox: true, contextIsolation: true },
        });
        void win.loadURL("app://cutweave/");
        return win.id;
      }, preload),
    ]);
    await forge.waitForLoadState("domcontentloaded");
    const outcomes = await forge.evaluate(async () => {
      const forgeApi = (
        window as unknown as {
          forge: { invoke(channel: string, ...args: unknown[]): Promise<unknown> };
        }
      ).forge;
      const tryInvoke = (channel: string, ...args: unknown[]) =>
        forgeApi.invoke(channel, ...args).then(
          () => "accepted",
          (err: Error) => err.message,
        );
      return {
        unregistered: await tryInvoke("fs:readFile", "/etc/passwd"),
        notEditor: await tryInvoke("engine:getConnection"),
      };
    });
    expect(outcomes.unregistered).toMatch(/No handler registered for 'fs:readFile'/);
    expect(outcomes.notEditor).toMatch(/rejected: the sender is not an editor window/);
  });
});

test.describe("permissions", () => {
  test("permission requests and checks are denied", async ({ page }) => {
    const states = await page.evaluate(async () => ({
      notifications: await Notification.requestPermission(),
      query: (await navigator.permissions.query({ name: "geolocation" })).state,
    }));
    expect(states).toEqual({ notifications: "denied", query: "denied" });
  });
});

test("an occupied engine port is re-picked and the renderer reloads with the new CSP", async () => {
  const blocker = createServer();
  await new Promise<void>((resolve) => blocker.listen(0, "127.0.0.1", resolve));
  const address = blocker.address();
  const taken = typeof address === "object" && address !== null ? address.port : 0;
  const { app, page, documents, mainLog, close } = await launchApp({
    CUTWEAVE_DEV_ENGINE_PORT: String(taken),
  });
  try {
    await waitForEngine(page);
    const port = await app.evaluate(() => globalThis.__cutweaveTest?.enginePort());
    expect(port).not.toBe(taken);
    expect(mainLog.some((l) => l.includes("engine failed to start"))).toBe(false);
    // Main reloaded the renderer after the re-pick...
    await expect
      .poll(() =>
        page
          .evaluate(
            () =>
              (performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming).type,
          )
          .catch(() => "navigating"),
      )
      .toBe("reload");
    // ...and the reloaded document's CSP response header names the new port, not the old one.
    await expect
      .poll(() => documents.at(-1)?.csp ?? "")
      .toContain(`connect-src http://127.0.0.1:${port} ws://127.0.0.1:${port};`);
    expect(documents.at(-1)?.csp).not.toContain(`:${taken}`);
    if (documents.length > 1) expect(documents[0]?.csp).toContain(`127.0.0.1:${taken};`);
    // The enforced policy follows: the engine answers, the old port is refused.
    expect(
      await page.evaluate(async (p) => (await fetch(`http://127.0.0.1:${p}/rpc`)).status, port),
    ).toBe(401);
    expect(await violationOf(page, { fetch: `http://127.0.0.1:${taken}/rpc` })).toBe(
      `connect-src http://127.0.0.1:${taken}/rpc`,
    );
  } finally {
    await close();
    blocker.close();
  }
});
