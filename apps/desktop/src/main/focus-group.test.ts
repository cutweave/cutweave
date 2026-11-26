// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { FOCUS_SETTLE_MS, FocusGroup } from "./focus-group.ts";

/** A group over fake windows and a manual clock. */
function setup() {
  const reports: boolean[] = [];
  let focusedWindow: "editor" | "popout" | null = null;
  const timers: { run: () => void; ms: number }[] = [];
  const group = new FocusGroup({
    report: (focused) => reports.push(focused),
    anyFocused: () => focusedWindow !== null,
    schedule: (run, ms) => {
      const timer = { run, ms };
      timers.push(timer);
      return timer;
    },
    cancel: (handle) => {
      const i = timers.indexOf(handle as (typeof timers)[number]);
      if (i >= 0) timers.splice(i, 1);
    },
  });
  /** The OS moves the focus: the old window blurs first, then the new one focuses. */
  const moveTo = (next: "editor" | "popout" | null) => {
    if (focusedWindow !== null) {
      focusedWindow = null;
      group.blur();
    }
    focusedWindow = next;
    if (next !== null) group.focus();
  };
  const settle = () => {
    for (const timer of timers.splice(0)) timer.run();
  };
  return { group, reports, timers, moveTo, settle };
}

describe("the editor and its popouts share one focus record (ADR-0021, ADR-0031)", () => {
  it("reports gaining the focus once, and not again when it moves to a detached window", () => {
    const { reports, moveTo, settle } = setup();
    moveTo("editor");
    expect(reports).toEqual([true]);
    moveTo("popout");
    settle();
    moveTo("editor");
    settle();
    expect(reports).toEqual([true]);
  });

  it("reports losing it only once no window of the group has it", () => {
    const { group, reports, timers, moveTo, settle } = setup();
    moveTo("popout");
    moveTo(null);
    expect(reports).toEqual([true]);
    expect(timers[0]?.ms).toBe(FOCUS_SETTLE_MS);
    settle();
    expect(reports).toEqual([true, false]);
    expect(group.focused).toBe(false);
    moveTo("editor");
    expect(reports).toEqual([true, false, true]);
  });

  it("does not report a blur whose check runs after the focus landed in the group", () => {
    const { reports, moveTo, settle } = setup();
    moveTo("editor");
    moveTo("popout");
    // The focus arrived before the check: nothing to report.
    settle();
    expect(reports).toEqual([true]);
  });
});
