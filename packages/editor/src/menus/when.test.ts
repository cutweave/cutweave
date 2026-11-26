// SPDX-License-Identifier: GPL-3.0-or-later
import { afterEach, describe, expect, it } from "vitest";
import { CONTEXT_KEYS, evaluateWhen, setWhenLogger } from "./when.ts";

const logged: string[] = [];
setWhenLogger((message) => logged.push(message));
afterEach(() => {
  logged.length = 0;
  setWhenLogger((message) => logged.push(message));
});

describe("evaluateWhen", () => {
  it("holds when there is no clause", () => {
    expect(evaluateWhen(undefined, {})).toBe(true);
    expect(evaluateWhen("  ", {})).toBe(true);
  });

  it("reads booleans, numbers and text against the context", () => {
    const ctx = { "item.linked": true, "selection.count": 3, "selection.kind": "video" } as const;
    expect(evaluateWhen("item.linked", ctx)).toBe(true);
    expect(evaluateWhen("!item.linked", ctx)).toBe(false);
    expect(evaluateWhen("selection.count == 3", ctx)).toBe(true);
    expect(evaluateWhen("selection.count != 3", ctx)).toBe(false);
    expect(evaluateWhen("selection.count > 1 && selection.count <= 3", ctx)).toBe(true);
    expect(evaluateWhen("selection.kind == video", ctx)).toBe(true);
    expect(evaluateWhen("selection.kind == 'audio'", ctx)).toBe(false);
    expect(evaluateWhen("item.linked == true", ctx)).toBe(true);
  });

  it("binds ! tighter than comparison, comparison tighter than &&, && tighter than ||", () => {
    const ctx = { "item.linked": false, "track.locked": true, "selection.count": 2 } as const;
    // a || b && c  ==  a || (b && c)
    expect(evaluateWhen("item.linked || track.locked && selection.count == 9", ctx)).toBe(false);
    expect(evaluateWhen("item.linked || track.locked && selection.count == 2", ctx)).toBe(true);
    // (a || b) && c
    expect(evaluateWhen("(item.linked || track.locked) && selection.count == 9", ctx)).toBe(false);
    expect(evaluateWhen("!item.linked && !track.locked", ctx)).toBe(false);
    expect(evaluateWhen("!(item.linked || selection.count == 1)", ctx)).toBe(true);
  });

  it("treats a context key that is not set as false", () => {
    expect(evaluateWhen("item.linked", {})).toBe(false);
    expect(evaluateWhen("item.linked == true", {})).toBe(false);
    expect(evaluateWhen("selection.count > 0", {})).toBe(false);
    expect(logged).toEqual([]);
  });

  it("treats an unknown key as false and logs it once", () => {
    expect(evaluateWhen("clip.selected", { "item.linked": true })).toBe(false);
    expect(evaluateWhen("clip.selected != true", {})).toBe(false);
    expect(evaluateWhen("clip.selected", {})).toBe(false);
    expect(logged).toHaveLength(2); // "clip.selected" in two different clauses; the repeat is silent
    expect(logged[0]).toContain('unknown context key "clip.selected"');
  });

  it("treats a clause that does not parse as false and logs it", () => {
    expect(evaluateWhen("selection.count ==", { "selection.count": 1 })).toBe(false);
    expect(evaluateWhen("(item.linked", { "item.linked": true })).toBe(false);
    expect(evaluateWhen("item.linked @", { "item.linked": true })).toBe(false);
    expect(logged.length).toBe(3);
  });

  it("names the published keys", () => {
    expect(CONTEXT_KEYS).toContain("selection.count");
    expect(CONTEXT_KEYS).toContain("project.readOnly");
    // WP-2D.01 added `item.movingPicture` (keys are added freely, renamed only by ADR).
    expect(CONTEXT_KEYS).toHaveLength(14);
  });
});
