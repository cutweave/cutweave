// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1M.12 budget: filtering 10,000 assets takes < 8 ms (p95) per keystroke. The measured call is
// what the bin does on a key press: the next search text over cards that are already indexed, in
// a sort that is already built. (Under load, rerun this file alone before reading a failure.)
import type { MediaId, Project } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { selectMediaCards, selectMediaUsage } from "./cards.ts";
import { findMedia } from "./find.ts";

const COUNT = 10_000;
const BUDGET_MS = 8;

function library(): Project {
  const media: Record<string, unknown> = {};
  const words = ["beach", "café", "city", "interview", "drone", "música"];
  for (let n = 0; n < COUNT; n++) {
    const id = `med_${n.toString(16).padStart(32, "0")}`;
    const kind = n % 5 === 0 ? "audio" : n % 7 === 0 ? "image" : "video";
    media[id] = {
      id,
      kind,
      path_abs: `/media/Shoot ${n % 40}/clip_${String(n).padStart(5, "0")}_${words[n % words.length]}.mp4`,
      path_rel: null,
      probe: {
        streams: [{ index: 0, kind: kind === "audio" ? "audio" : "video", codec: "h264" }],
        duration: kind === "image" ? null : (1 + (n % 600)) * 705_600_000,
        fps: null,
      },
    };
  }
  return { media, sequences: {} } as unknown as Project;
}

describe("finding media in 10,000 assets", () => {
  it(`filters in under ${BUDGET_MS} ms at p95 per keystroke`, { retry: 2 }, () => {
    const project = library();
    const cards = selectMediaCards(project);
    const usage = selectMediaUsage(project);
    expect(cards).toHaveLength(COUNT);
    const sort = { by: "name", order: "asc" } as const;
    findMedia(cards, usage, { text: "" }, sort); // builds the index and the sort once
    const typed = "clip beach";
    const times: number[] = [];
    for (let round = 0; round < 8; round++) {
      for (let len = 1; len <= typed.length; len++) {
        const filter = { text: typed.slice(0, len), kinds: ["video" as const], used: false };
        const t = performance.now();
        const found = findMedia(cards, usage, filter, sort);
        times.push(performance.now() - t);
        expect(found.total).toBe(COUNT);
      }
    }
    times.sort((a, b) => a - b);
    const p95 = times[Math.floor((times.length - 1) * 0.95)] as number;
    console.info(
      `findMedia over ${COUNT}: p50 ${times[times.length >> 1]?.toFixed(2)} ms, p95 ${p95.toFixed(2)} ms`,
    );
    expect(p95).toBeLessThan(BUDGET_MS);
  });

  it("builds the index and a new sort in a blink too", () => {
    const cards = selectMediaCards(library());
    const t = performance.now();
    findMedia(cards, new Map(), { text: "a" }, { by: "kind", order: "desc" });
    expect(performance.now() - t).toBeLessThan(250);
    const id = cards[0]?.id as MediaId;
    expect(id).toMatch(/^med_/);
  });
});
