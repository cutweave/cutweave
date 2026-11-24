// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import { applyEarlyTheme } from "./early-theme.ts";

describe("applyEarlyTheme", () => {
  it("sets the theme at once when the root element exists", () => {
    const dataset: Record<string, string | undefined> = {};
    const whenReady = vi.fn();
    applyEarlyTheme("light", { documentElement: { dataset } }, whenReady);
    expect(dataset.theme).toBe("light");
    expect(whenReady).not.toHaveBeenCalled();
  });

  it("waits for the root element when the document has none yet", () => {
    const dataset: Record<string, string | undefined> = {};
    const target: { documentElement: { dataset: typeof dataset } | null } = {
      documentElement: null,
    };
    let ready: (() => void) | undefined;
    applyEarlyTheme("dark", target, (cb) => {
      ready = cb;
    });
    expect(dataset.theme).toBeUndefined();
    target.documentElement = { dataset };
    ready?.();
    expect(dataset.theme).toBe("dark");
  });

  it("ignores anything but dark or light", () => {
    const dataset: Record<string, string | undefined> = {};
    for (const bad of ["system", "", 3, null, undefined, "<script>"]) {
      applyEarlyTheme(bad, { documentElement: { dataset } }, vi.fn());
    }
    expect(dataset.theme).toBeUndefined();
  });
});
