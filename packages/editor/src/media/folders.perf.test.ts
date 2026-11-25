// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2F.08 budget: a folder of 5,000 assets opens in < 50 ms (p95). The measured call is what the
// bin does when a folder opens: its subfolders and its assets, with the project's cards already
// read. (Under load, rerun this file alone before reading a failure.)
import type { FolderId, MediaId, Project } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { NO_FILTERS } from "./binStore.ts";
import { computeBinView } from "./binView.ts";
import { selectMediaCards } from "./cards.ts";

const COUNT = 5_000;
const BUDGET_MS = 50;

function bigProject(): Project {
  const media: Record<string, unknown> = {};
  const folders: Record<string, unknown> = {};
  const big = "fld_big";
  folders[big] = { id: big, name: "Big", parent: null, custom: {} };
  for (let k = 0; k < 20; k++) {
    const id = `fld_${k}`;
    folders[id] = { id, name: `Day ${k}`, parent: big, custom: {} };
  }
  for (let n = 0; n < COUNT + 500; n++) {
    const id = `med_${n.toString(16).padStart(32, "0")}`;
    media[id] = {
      id,
      kind: "video",
      path_abs: `/media/clip_${String(n).padStart(5, "0")}.mp4`,
      path_rel: null,
      name: null,
      folder: n < COUNT ? big : null,
      probe: {
        streams: [{ index: 0, kind: "video", codec: "h264" }],
        duration: 705_600_000,
        fps: null,
      },
    };
  }
  return { media, folders, sequences: {} } as unknown as Project;
}

describe("opening a folder of 5,000 assets", () => {
  it(`takes under ${BUDGET_MS} ms at p95`, () => {
    const project = bigProject();
    const cards = selectMediaCards(project);
    expect(cards).toHaveLength(COUNT + 500);
    const input = {
      text: "",
      filters: NO_FILTERS,
      sort: { by: "name", order: "asc" },
      scope: "all",
    } as const;
    const open = "fld_big" as FolderId;
    // Warm the sort index like a first open does once.
    expect(computeBinView(project, input, open).cards).toHaveLength(COUNT);
    const samples: number[] = [];
    for (let i = 0; i < 30; i++) {
      const start = performance.now();
      const view = computeBinView(project, { ...input }, open);
      samples.push(performance.now() - start);
      expect(view.cards).toHaveLength(COUNT);
      expect(view.folders).toHaveLength(20);
    }
    samples.sort((a, b) => a - b);
    const p95 = samples[Math.floor(samples.length * 0.95)] as number;
    expect(p95).toBeLessThan(BUDGET_MS);
    expect(cards[0]?.id as MediaId).toBeDefined();
  });
});
