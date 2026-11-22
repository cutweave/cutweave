// SPDX-License-Identifier: GPL-3.0-or-later
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  assertWindowPreferences,
  resolveWebPreferences,
  WINDOW_KINDS,
  WindowPolicyError,
  webPreferencesFor,
} from "./window-policy.ts";

const ctx = { preloadPath: "/bundle/preload/index.cjs", isDev: false };
const devCtx = { ...ctx, isDev: true };

const LOCKED = {
  contextIsolation: true,
  nodeIntegration: false,
  nodeIntegrationInWorker: false,
  nodeIntegrationInSubFrames: false,
  webSecurity: true,
  allowRunningInsecureContent: false,
  webviewTag: false,
  experimentalFeatures: false,
  plugins: false,
  navigateOnDragDrop: false,
  safeDialogs: true,
};

describe("webPreferencesFor", () => {
  it("editor: isolated, no Node, non-sandboxed only for the audited preload", () => {
    expect(webPreferencesFor("editor", ctx)).toEqual({
      ...LOCKED,
      devTools: false,
      sandbox: false,
      preload: "/bundle/preload/index.cjs",
    });
  });

  it("every other kind: sandboxed with no preload", () => {
    for (const kind of WINDOW_KINDS.filter((k) => k !== "editor")) {
      expect(webPreferencesFor(kind, ctx)).toEqual({ ...LOCKED, devTools: false, sandbox: true });
    }
  });

  it("DevTools only in dev", () => {
    expect(webPreferencesFor("editor", devCtx).devTools).toBe(true);
    expect(webPreferencesFor("auxiliary", devCtx).devTools).toBe(true);
    expect(webPreferencesFor("editor", ctx).devTools).toBe(false);
  });
});

describe("resolveWebPreferences", () => {
  it("returns the policy and keeps tunable keys", () => {
    expect(resolveWebPreferences("auxiliary", ctx, { zoomFactor: 1.25 })).toEqual({
      ...webPreferencesFor("auxiliary", ctx),
      zoomFactor: 1.25,
    });
    expect(
      resolveWebPreferences("editor", ctx, { sandbox: false, contextIsolation: true }),
    ).toEqual(webPreferencesFor("editor", ctx));
  });

  it("throws on sandbox: false for a non-editor window", () => {
    expect(() => resolveWebPreferences("auxiliary", ctx, { sandbox: false })).toThrow(
      WindowPolicyError,
    );
    expect(() => resolveWebPreferences("auxiliary", ctx, { sandbox: false })).toThrow(
      /must be sandboxed/,
    );
  });

  it("throws on any preload for a non-editor window, or a different editor preload", () => {
    expect(() => resolveWebPreferences("auxiliary", ctx, { preload: ctx.preloadPath })).toThrow(
      /preload/,
    );
    expect(() => resolveWebPreferences("editor", ctx, { preload: "/tmp/evil.js" })).toThrow(
      /preload/,
    );
  });

  it("throws on a sandboxed editor (the addon preload needs Node)", () => {
    expect(() => resolveWebPreferences("editor", ctx, { sandbox: true })).toThrow(/sandbox/);
  });

  it("throws on every weakening of a locked setting, for every kind", () => {
    const weakened: Record<string, unknown> = {
      contextIsolation: false,
      nodeIntegration: true,
      nodeIntegrationInWorker: true,
      nodeIntegrationInSubFrames: true,
      webSecurity: false,
      allowRunningInsecureContent: true,
      webviewTag: true,
      experimentalFeatures: true,
      plugins: true,
      navigateOnDragDrop: true,
      safeDialogs: false,
      devTools: true,
    };
    for (const kind of WINDOW_KINDS) {
      for (const [key, value] of Object.entries(weakened)) {
        expect(() => resolveWebPreferences(kind, ctx, { [key]: value }), `${kind}.${key}`).toThrow(
          WindowPolicyError,
        );
      }
    }
  });

  it("throws on keys outside the policy and the tunable list", () => {
    for (const extra of [
      { partition: "persist:other" },
      { additionalArguments: ["--inspect"] },
      { enableBlinkFeatures: "Foo" },
      { offscreen: true },
    ]) {
      expect(() => resolveWebPreferences("auxiliary", ctx, extra), JSON.stringify(extra)).toThrow(
        /not allowed/,
      );
    }
  });

  it("throws on unknown kinds", () => {
    expect(() => resolveWebPreferences("plugin" as "editor", ctx)).toThrow(/unknown window kind/);
  });
});

describe("assertWindowPreferences", () => {
  it("accepts the policy and rejects hand-built variants", () => {
    expect(() =>
      assertWindowPreferences("editor", webPreferencesFor("editor", ctx), ctx),
    ).not.toThrow();
    expect(() =>
      assertWindowPreferences(
        "auxiliary",
        { ...webPreferencesFor("auxiliary", ctx), sandbox: false },
        ctx,
      ),
    ).toThrow(/sandboxed/);
    expect(() =>
      assertWindowPreferences(
        "auxiliary",
        { ...webPreferencesFor("auxiliary", ctx), session: {} as never },
        ctx,
      ),
    ).toThrow(/session/);
  });
});

describe("factory is the only window constructor", () => {
  it("no `new BrowserWindow` outside src/main/windows.ts", () => {
    const srcDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
    const offenders: string[] = [];
    const walk = (dir: string) => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) walk(full);
        else if (/\.tsx?$/.test(entry.name) && !entry.name.endsWith(".test.ts")) {
          const rel = path.relative(srcDir, full).replaceAll("\\", "/");
          if (
            /new\s+BrowserWindow\s*\(/.test(readFileSync(full, "utf8")) &&
            rel !== "main/windows.ts"
          ) {
            offenders.push(rel);
          }
        }
      }
    };
    walk(srcDir);
    expect(offenders).toEqual([]);
  });
});
