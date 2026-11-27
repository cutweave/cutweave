// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import { type ExportNoticeDeps, notifyExportFinished, shouldNotify } from "./export-notice.ts";

const deps = (patch: Partial<Record<"enabled" | "supported" | "hidden" | "focused", boolean>>) => {
  const state = { enabled: true, supported: true, hidden: false, focused: false, ...patch };
  return {
    enabled: () => state.enabled,
    supported: () => state.supported,
    hidden: () => state.hidden,
    focused: () => state.focused,
    show: vi.fn(),
    bringBack: vi.fn(),
  } satisfies ExportNoticeDeps;
};

const notice = { title: "Export complete", body: "Film.mp4" };

describe("the OS notification of a finished export", () => {
  it("shows while Cutweave is in the background: without the focus, or minimized", () => {
    expect(shouldNotify(deps({ focused: false }))).toBe(true);
    expect(shouldNotify(deps({ focused: true, hidden: true }))).toBe(true);
  });

  it("stays quiet while the person is in the app", () => {
    const d = deps({ focused: true });
    expect(notifyExportFinished(d, notice)).toBe(false);
    expect(d.show).not.toHaveBeenCalled();
  });

  it("stays quiet when the setting is off or the OS has no notifications", () => {
    expect(shouldNotify(deps({ enabled: false }))).toBe(false);
    expect(shouldNotify(deps({ supported: false }))).toBe(false);
  });

  it("shows the editor's text, and a click brings the window back", () => {
    const d = deps({ hidden: true });
    expect(notifyExportFinished(d, notice)).toBe(true);
    expect(d.show).toHaveBeenCalledWith(notice, expect.any(Function));
    expect(d.bringBack).not.toHaveBeenCalled();
    const onClick = d.show.mock.calls[0]?.[1] as (() => void) | undefined;
    onClick?.();
    expect(d.bringBack).toHaveBeenCalledTimes(1);
  });
});
