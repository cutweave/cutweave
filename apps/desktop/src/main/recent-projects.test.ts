// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { CloseGuard } from "./close-guard.ts";
import { MAX_RECENT, type RecentFiles, RecentProjects } from "./recent-projects.ts";

function memory(initial?: string): RecentFiles & { text: string | null } {
  const files = {
    text: initial ?? null,
    read: () => files.text,
    write: (_file: string, text: string) => {
      files.text = text;
    },
  };
  return files;
}

const root = process.platform === "win32" ? "C:\\projects" : "/projects";
const at = (name: string) => `${root}${process.platform === "win32" ? "\\" : "/"}${name}`;

describe("RecentProjects", () => {
  it("keeps the newest first, without repeats, and persists", () => {
    const files = memory();
    const recent = new RecentProjects("recent.json", files);
    recent.add(at("a"));
    recent.add(at("b"));
    recent.add(at("a"));
    expect(recent.list()).toEqual([at("a"), at("b")]);
    expect(new RecentProjects("recent.json", files).list()).toEqual([at("a"), at("b")]);
  });

  it("caps the list and removes entries", () => {
    const recent = new RecentProjects("recent.json", memory());
    for (let i = 0; i < MAX_RECENT + 5; i++) recent.add(at(`p${i}`));
    expect(recent.list()).toHaveLength(MAX_RECENT);
    expect(recent.list()[0]).toBe(at(`p${MAX_RECENT + 4}`));
    recent.remove(at(`p${MAX_RECENT + 4}`));
    expect(recent.list()).not.toContain(at(`p${MAX_RECENT + 4}`));
  });

  it("ignores relative paths and a damaged file", () => {
    const recent = new RecentProjects("recent.json", memory("not json"));
    expect(recent.list()).toEqual([]);
    recent.add("relative/path");
    recent.add("");
    expect(recent.list()).toEqual([]);
    const mixed = new RecentProjects("r.json", memory(JSON.stringify([at("ok"), "rel", 7, null])));
    expect(mixed.list()).toEqual([at("ok")]);
  });
});

describe("CloseGuard", () => {
  it("holds the first close while there are unsaved changes, then lets the confirmed one go", () => {
    const guard = new CloseGuard();
    expect(guard.holds(true)).toBe(false);
    guard.setDirty(true);
    expect(guard.holds(true)).toBe(true);
    expect(guard.holds(false)).toBe(false); // a dead renderer cannot answer
    guard.confirm();
    expect(guard.holds(true)).toBe(false);
    guard.setDirty(true); // more unsaved work asks again
    expect(guard.holds(true)).toBe(true);
    guard.reset();
    expect(guard.holds(true)).toBe(false);
  });
});
