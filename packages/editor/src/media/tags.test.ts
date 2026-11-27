// SPDX-License-Identifier: GPL-3.0-or-later
// The tag, rating and note rules of the bin (WP-2F.06b), held to the engine's
// (`cutweave-model/src/collection.rs`).
import type { MediaId } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import type { MediaCard } from "./cards.ts";
import {
  charCount,
  MAX_NOTE_CHARS,
  MAX_TAG_CHARS,
  normalizeTag,
  notePlan,
  noteTooLong,
  parseTagEntry,
  projectTags,
  selectionTags,
  sharedRating,
  suggestTags,
  tagKey,
  tagProblem,
} from "./tags.ts";

const card = (n: number, tags: string[], rating: number | null = null): MediaCard => ({
  id: `med_${String(n).padStart(4, "0")}` as MediaId,
  kind: "video",
  name: `clip-${n}.mp4`,
  folder: null,
  duration: 1,
  width: null,
  height: null,
  fps: null,
  rate: null,
  codec: null,
  broken: false,
  tags,
  rating,
  note: null,
});

describe("a tag as the engine stores it", () => {
  it("is trimmed", () => {
    expect(normalizeTag("  b-roll \t")).toBe("b-roll");
  });

  it("is the same tag ignoring case and accents", () => {
    expect(tagKey("Café")).toBe(tagKey("cafe"));
    expect(tagKey("INTERVIEW")).toBe(tagKey("interview"));
    expect(tagKey("Ångström")).toBe("angstrom");
    expect(tagKey("day 1")).not.toBe(tagKey("day 2"));
  });

  it("has 1 to 64 characters, counted as code points", () => {
    expect(tagProblem("")).toBe("empty");
    expect(tagProblem("   ")).toBe("empty");
    expect(tagProblem("a")).toBeNull();
    expect(tagProblem("x".repeat(MAX_TAG_CHARS))).toBeNull();
    expect(tagProblem("x".repeat(MAX_TAG_CHARS + 1))).toBe("tooLong");
    // 64 astral characters are 128 UTF-16 units and still one tag.
    expect(charCount("𝄞".repeat(MAX_TAG_CHARS))).toBe(MAX_TAG_CHARS);
    expect(tagProblem("𝄞".repeat(MAX_TAG_CHARS))).toBeNull();
    expect(tagProblem("𝄞".repeat(MAX_TAG_CHARS + 1))).toBe("tooLong");
  });

  it("has no comma and no control characters", () => {
    expect(tagProblem("a,b")).toBe("comma");
    expect(tagProblem("a\tb")).toBe("control");
    expect(tagProblem("a\nb")).toBe("control");
    expect(tagProblem("a\u0007b")).toBe("control");
    expect(tagProblem("a\u0085b")).toBe("control");
    expect(tagProblem("two words")).toBeNull();
    expect(tagProblem("日本語")).toBeNull();
  });
});

describe("typed or pasted tags", () => {
  it("split on commas and line breaks, trimmed, without blanks", () => {
    expect(parseTagEntry(" interview , b-roll,, \n day 1 ")).toEqual([
      "interview",
      "b-roll",
      "day 1",
    ]);
    expect(parseTagEntry(" , ")).toEqual([]);
  });

  it("keep the first spelling of a tag typed twice", () => {
    expect(parseTagEntry("Café, cafe, CAFÉ, tea")).toEqual(["Café", "tea"]);
  });
});

describe("the project's tags (media.list_tags on the mirror)", () => {
  const cards = [
    card(1, ["b-roll", "Interview"]),
    card(2, ["interview", "Zoo"]),
    card(3, ["Äpfel", "b-roll"]),
    card(4, []),
  ];

  it("counts assets per tag, one entry per key, in the first asset's spelling", () => {
    expect(projectTags(cards)).toEqual([
      { tag: "Äpfel", count: 1 },
      { tag: "b-roll", count: 2 },
      { tag: "Interview", count: 2 },
      { tag: "Zoo", count: 1 },
    ]);
  });

  it("is empty without tags", () => {
    expect(projectTags([card(1, []), card(2, [])])).toEqual([]);
  });
});

describe("the tags of a selection", () => {
  it("of one asset are all its own", () => {
    expect(selectionTags([card(1, ["a", "b"])])).toEqual([
      { tag: "a", count: 1, all: true },
      { tag: "b", count: 1, all: true },
    ]);
  });

  it("says which tags every asset has and which only some have", () => {
    const tags = selectionTags([
      card(1, ["b-roll", "Interview", "keep"]),
      card(2, ["B-Roll", "interview"]),
      card(3, ["b-roll", "night"]),
    ]);
    expect(tags).toEqual([
      { tag: "b-roll", count: 3, all: true },
      { tag: "Interview", count: 2, all: false },
      { tag: "keep", count: 1, all: false },
      { tag: "night", count: 1, all: false },
    ]);
    // The intersection and what is left of the union.
    expect(tags.filter((t) => t.all).map((t) => t.tag)).toEqual(["b-roll"]);
    expect(tags.filter((t) => !t.all).map((t) => t.tag)).toEqual(["Interview", "keep", "night"]);
  });

  it("is empty for no assets and for assets without tags", () => {
    expect(selectionTags([])).toEqual([]);
    expect(selectionTags([card(1, []), card(2, [])])).toEqual([]);
  });
});

describe("the rating a selection shares", () => {
  it("is the rating when all agree, null when none is rated, mixed otherwise", () => {
    expect(sharedRating([])).toBeNull();
    expect(sharedRating([card(1, [], 4)])).toBe(4);
    expect(sharedRating([card(1, [], 4), card(2, [], 4)])).toBe(4);
    expect(sharedRating([card(1, []), card(2, [])])).toBeNull();
    expect(sharedRating([card(1, [], 4), card(2, [])])).toBe("mixed");
    expect(sharedRating([card(1, [], 4), card(2, [], 2)])).toBe("mixed");
  });
});

describe("tag suggestions", () => {
  const all = [
    { tag: "b-roll", count: 5 },
    { tag: "Broll night", count: 1 },
    { tag: "Café", count: 2 },
    { tag: "interview", count: 9 },
    { tag: "problem", count: 1 },
  ];

  it("lists the most used tags when nothing is typed", () => {
    expect(suggestTags(all, "", [], 3)).toEqual(["interview", "b-roll", "Café"]);
  });

  it("puts tags that start with the text before those that contain it", () => {
    expect(suggestTags(all, "ro")).toEqual(["b-roll", "Broll night", "problem"]);
    expect(suggestTags(all, "B")).toEqual(["b-roll", "Broll night", "problem"]);
  });

  it("matches ignoring case and accents and leaves out tags the selection has", () => {
    expect(suggestTags(all, "CAFE")).toEqual(["Café"]);
    expect(suggestTags(all, "cafe", ["cafe"])).toEqual([]);
    expect(suggestTags(all, "", ["INTERVIEW"], 2)).toEqual(["b-roll", "Café"]);
  });
});

describe("committing the note field", () => {
  it("does nothing when the text is the stored note", () => {
    expect(notePlan("", null)).toEqual({ change: false });
    expect(notePlan("   \n", null)).toEqual({ change: false });
    expect(notePlan("good take", "good take")).toEqual({ change: false });
    expect(notePlan(" good take\n", "good take")).toEqual({ change: false });
  });

  it("sends the trimmed text, or null to remove the note", () => {
    expect(notePlan(" line 1\nline 2 ", null)).toEqual({ change: true, note: "line 1\nline 2" });
    expect(notePlan("  ", "old")).toEqual({ change: true, note: null });
  });

  it("is too long past 2,000 characters", () => {
    expect(noteTooLong("x".repeat(MAX_NOTE_CHARS))).toBe(false);
    expect(noteTooLong(` ${"x".repeat(MAX_NOTE_CHARS)} `)).toBe(false);
    expect(noteTooLong("x".repeat(MAX_NOTE_CHARS + 1))).toBe(true);
  });
});
