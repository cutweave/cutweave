// SPDX-License-Identifier: GPL-3.0-or-later
// The bin's predicate and sort against the shared fixture that a Rust test generates from the
// engine's `media.list` (`crates/cutweave-ops/tests/media_find_fixture.rs`), plus direct cases.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { FolderId, MediaFilter, MediaSort, Project } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { selectMediaCards, selectMediaUsage } from "./cards.ts";
import {
  compareCodePoints,
  filterCount,
  findIds,
  findMedia,
  foldText,
  isEmptyFilter,
  termsOf,
} from "./find.ts";

interface FixtureAsset {
  id: string;
  kind: string;
  path_abs: string | null;
  path_rel: string | null;
  duration: number | null;
  streams: number;
  name: string | null;
  folder: string | null;
  tags: string[];
  rating: number | null;
  note: string | null;
}

interface FixtureDocument {
  name: string;
  media: FixtureAsset[];
  folders: { id: string; name: string; parent: string | null }[];
  collections: { id: string; name: string; kind: unknown }[];
  sequences: string[][];
}

interface FixtureCase {
  document: string;
  filter: MediaFilter | null;
  sort: MediaSort | null;
  ids: string[];
  matched: number;
  limit?: number;
  pages?: string[][];
}

const fixture = JSON.parse(
  readFileSync(
    fileURLToPath(
      new URL("../../../../crates/cutweave-ops/tests/fixtures/media-find.json", import.meta.url),
    ),
    "utf8",
  ),
) as { documents: FixtureDocument[]; cases: FixtureCase[] };

/** A project holding just what the bin reads. */
function projectOf(doc: FixtureDocument): Project {
  const media = Object.fromEntries(
    doc.media.map((a) => [
      a.id,
      {
        id: a.id,
        kind: a.kind,
        path_abs: a.path_abs,
        path_rel: a.path_rel,
        name: a.name,
        folder: a.folder,
        // The file leaves these out while empty; the reader must cope with both.
        ...(a.tags.length > 0 ? { tags: a.tags } : {}),
        ...(a.rating !== null ? { rating: a.rating } : {}),
        ...(a.note !== null ? { note: a.note } : {}),
        probe: {
          streams: Array.from({ length: a.streams }, (_, index) => ({
            index,
            kind: a.kind === "audio" ? "audio" : "video",
            codec: null,
          })),
          duration: a.duration,
          fps: null,
        },
      },
    ]),
  );
  const sequences = Object.fromEntries(
    doc.sequences.map((used, i) => [
      `seq_${i}`,
      {
        tracks: {
          [`trk_${i}`]: {
            items: Object.fromEntries(used.map((m, k) => [`itm_${i}_${k}`, { media: m }])),
          },
        },
      },
    ]),
  );
  const folders = Object.fromEntries(doc.folders.map((f) => [f.id, f]));
  const collections = Object.fromEntries(doc.collections.map((c) => [c.id, c]));
  return {
    media,
    folders,
    sequences,
    ...(doc.collections.length > 0 ? { collections } : {}),
  } as unknown as Project;
}

const projects = new Map(fixture.documents.map((d) => [d.name, projectOf(d)]));

describe("the media predicate against the Rust fixture", () => {
  it("has at least 80 cases, with paging under every sort and every part of the filter", () => {
    expect(fixture.cases.length).toBeGreaterThanOrEqual(80);
    for (const part of ["tags", "min_rating", "collection"] as const) {
      const used = fixture.cases.filter((c) => (c.filter?.[part] ?? null) !== null);
      expect(used.length).toBeGreaterThanOrEqual(5);
    }
    const paged = fixture.cases.filter((c) => c.pages !== undefined);
    for (const by of ["imported", "name", "duration", "kind"]) {
      expect(paged.some((c) => c.sort?.by === by)).toBe(true);
    }
  });

  it.each(
    fixture.cases.map(
      (c, i) => [`#${i} ${JSON.stringify(c.filter)} ${JSON.stringify(c.sort)}`, c] as const,
    ),
  )("%s", (_name, c) => {
    const project = projects.get(c.document) as Project;
    const cards = selectMediaCards(project);
    const usage = selectMediaUsage(project);
    const found = findMedia(cards, usage, c.filter, c.sort, project.folders, project.collections);
    expect(found.cards.map((card) => card.id)).toEqual(c.ids);
    expect(found.cards.length).toBe(c.matched);
    expect(found.total).toBe(cards.length);
    if (c.pages !== undefined && c.limit !== undefined) {
      // The engine pages the same list; chunking ours by the limit gives its pages.
      const chunks: string[][] = [];
      for (let i = 0; i < c.ids.length; i += c.limit) chunks.push(c.ids.slice(i, i + c.limit));
      expect(c.pages).toEqual(chunks.length === 0 ? [[]] : chunks);
    }
  });
});

describe("display names and folders", () => {
  const project = projects.get("folders") as Project;
  const cards = selectMediaCards(project);
  const usage = selectMediaUsage(project);
  const names = (filter: MediaFilter | null, sort?: MediaSort | null) =>
    findMedia(cards, usage, filter, sort, project.folders).cards.map((c) => c.name);

  it("shows the display name, else the file name", () => {
    expect(cards.map((c) => c.name)).toEqual([
      "Opening",
      "b.mp4",
      "Thème",
      "interview wide",
      "e.png",
      "A-roll",
      "Opening night.mp4",
    ]);
  });

  it("matches the display name and no longer the file name it replaces", () => {
    expect(names({ text: "opening" })).toEqual(["Opening", "Opening night.mp4"]);
    expect(names({ text: "a.mp4" })).toEqual([]);
  });

  it("reads one level of a folder, its subtree with recursive, and the top level with recursive false", () => {
    const interviews = cards[0]?.folder as FolderId;
    expect(names({ folder: interviews })).toEqual(["Opening"]);
    expect(names({ folder: interviews, recursive: true })).toEqual([
      "Opening",
      "b.mp4",
      "interview wide",
    ]);
    expect(names({ recursive: false })).toEqual(["Thème", "Opening night.mp4"]);
    expect(isEmptyFilter({ recursive: false })).toBe(false);
    expect(isEmptyFilter({ folder: interviews })).toBe(false);
  });
});

describe("the media predicate", () => {
  const project = projects.get("library") as Project;
  const cards = selectMediaCards(project);
  const usage = selectMediaUsage(project);
  const names = (filter: MediaFilter | null, sort?: MediaSort | null) =>
    findMedia(cards, usage, filter, sort).cards.map((c) => c.name);

  it("ignores case and accents and wants every term", () => {
    expect(names({ text: "CAFE" })).toEqual(["Café Interview.mp4", "cafe_broll.mov"]);
    expect(names({ text: "interview café" })).toEqual(["Café Interview.mp4"]);
    expect(names({ text: "   " })).toHaveLength(cards.length);
  });

  it("keeps stills out when a duration bound is set", () => {
    expect(names({ kinds: ["image"], min_duration: 0 })).toEqual([]);
    expect(names({ kinds: ["image"] })).toEqual(["title card.png", "logo.png"]);
  });

  it("tells what is on a timeline of any sequence", () => {
    expect(names({ used: true }).length + names({ used: false }).length).toBe(cards.length);
  });

  it("sorts desc as the exact reverse of asc", () => {
    for (const by of ["imported", "name", "duration", "kind"] as const) {
      const asc = names(null, { by, order: "asc" });
      expect(names(null, { by, order: "desc" })).toEqual([...asc].reverse());
    }
  });

  it("orders by code point, as the engine's string compare does", () => {
    expect(compareCodePoints("a", "b")).toBeLessThan(0);
    expect(compareCodePoints("ab", "a")).toBeGreaterThan(0);
    expect(compareCodePoints("x", "x")).toBe(0);
    // U+FF21 (BMP, above the surrogates) sorts below U+1F600 (astral) by code point.
    expect(compareCodePoints("Ａ", "\u{1f600}")).toBeLessThan(0);
    expect(compareCodePoints("\u{1f600}", "Ａ")).toBeGreaterThan(0);
  });

  it("counts filters, not the search text", () => {
    expect(filterCount({ text: "x" })).toBe(0);
    expect(filterCount({ kinds: ["audio", "video"], used: false, max_duration: 5 })).toBe(3);
    expect(filterCount({ kinds: [], problems: false })).toBe(0);
    expect(isEmptyFilter({ text: " " })).toBe(true);
    expect(isEmptyFilter({ problems: true })).toBe(false);
  });

  it("folds and splits like the engine", () => {
    expect(foldText("ÉCLAIR Ñandú")).toBe("eclair nandu");
    expect(termsOf("  a\tB\nc  ")).toEqual(["a", "b", "c"]);
    expect(findIds(cards, usage, { text: "zzz" })).toEqual([]);
  });
});

describe("tags, ratings, notes and collections (ADR-0036)", () => {
  const project = projects.get("tagged") as Project;
  const cards = selectMediaCards(project);
  const usage = selectMediaUsage(project);
  const names = (filter: MediaFilter | null) =>
    findMedia(cards, usage, filter, null, project.folders, project.collections).cards.map(
      (c) => c.name,
    );
  const collection = (name: string) =>
    Object.values(project.collections ?? {}).find((c) => c?.name === name)?.id ?? null;

  it("reads tags, rating and note onto the card, with defaults for assets without them", () => {
    expect(cards[0]).toMatchObject({ tags: ["Café", "interview"], rating: 5 });
    expect(cards[0]?.note).toContain("Naïve");
    expect(cards[7]).toMatchObject({ tags: [], rating: null, note: null });
  });

  it("wants every tag, whole, ignoring case and accents", () => {
    expect(names({ tags: ["CAFE"] })).toEqual(["A001.mp4"]);
    expect(names({ tags: ["interview", "Wide"] })).toEqual(["A003.mp4"]);
    expect(names({ tags: ["inter"] })).toEqual([]);
  });

  it("finds a term in the name, a tag or the note", () => {
    expect(names({ text: "eclair" })).toEqual(["A004.mp4"]);
    expect(names({ text: "cafe" })).toEqual(["A001.mp4", "B001 cafe.mp4"]);
  });

  it("lists a manual collection without the member that is gone, and a smart one live", () => {
    expect(names({ collection: collection("Selects") })).toEqual([
      "A001.mp4",
      "B001 cafe.mp4",
      "A005.mp4",
    ]);
    expect(names({ collection: collection("Unused long interviews") })).toEqual([
      "A001.mp4",
      "A004.mp4",
      "A005.mp4",
    ]);
    expect(names({ collection: collection("Gone folder") })).toEqual([]);
    // A collection the project does not have: nothing (the engine answers not_found).
    expect(names({ collection: "col_00000000000000000000000000000000" as never })).toEqual([]);
  });

  it("counts tags and a rating as filters, and a collection as a place", () => {
    expect(filterCount({ tags: ["a", "b"], min_rating: 3 })).toBe(2);
    expect(filterCount({ tags: [] })).toBe(0);
    expect(isEmptyFilter({ collection: collection("Selects") })).toBe(false);
    expect(isEmptyFilter({ tags: [] })).toBe(true);
  });
});
