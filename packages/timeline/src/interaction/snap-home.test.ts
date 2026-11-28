// SPDX-License-Identifier: GPL-3.0-or-later
// Issue #263: a drag with no net movement must leave the clip where it was. A clip 8 px from
// another clip's edge, pressed, moved away, moved back and released at the press point, was
// pulled onto that edge by the snap. The rule: a snap never pulls further than the pointer has
// travelled from the press, so a candidate counts only once it is nearer than where the drag
// started; and a gesture that ends on its starting position is cancelled, not committed.
import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { initMath } from "@cutweave/protocol/wasm";
import { beforeAll, describe, expect, it } from "vitest";
import type { TimelineModel } from "../model.ts";
import { clip, fmt, fmtCommand } from "./testing/transcript.ts";
import { f, MODEL, World } from "./testing/world.ts";

beforeAll(async () => {
  await initMath(mathModuleBytes());
}, 900_000);

/**
 * The standard sequence with the music (A2) ending at frame 98: an edge 2 frames, 8 px, before
 * Z's start (100) and, for Z's end (120), one at 122 (the music of a second world).
 */
function modelWithEdgeAt(frame: number): TimelineModel {
  return {
    ...MODEL,
    items: MODEL.items.map((it) => (it.id === "M" ? { ...it, duration: f(frame) } : it)),
  };
}

const shown = (w: World): string[] =>
  w.intents
    .filter((i) => i.type === "update_tx" || i.type === "commit_tx" || i.type === "cancel_tx")
    .map(fmt);

interface Hand {
  down(): void;
  /** Moves to `dx` px from the press and lets a frame pass. */
  to(dx: number, dy?: number): void;
  up(dx: number): void;
}

function hand(w: World, at: { x: number; y: number }): Hand {
  return {
    down: () => w.send({ type: "pointer.down", ...at, button: 0 }),
    to: (dx, dy = 0) => {
      w.send({ type: "pointer.move", x: at.x + dx, y: at.y + dy });
      w.send({ type: "frame" });
    },
    up: (dx) => w.send({ type: "pointer.up", x: at.x + dx, y: at.y, button: 0 }),
  };
}

describe("a drag with no net movement leaves the clip where it was (#263)", () => {
  it("press, away, back, release at the press point: nothing is committed", () => {
    const w = new World({ model: modelWithEdgeAt(98) });
    const h = hand(w, clip("Z")(w));
    h.down();
    h.to(4);
    h.to(60);
    h.to(20);
    h.to(0);
    // Back at the press point the clip is at its own start, not on the edge 8 px away.
    expect(w.state.guides).toEqual([]);
    expect(w.state.preview).toMatchObject({ trackId: "V2", start: f(100), end: f(120) });
    h.up(0);
    expect(shown(w).at(-1)).toBe("cancel #1 unchanged");
    expect(w.engine.history).toEqual([]);
    expect(w.state.mode.name).toBe("hover");
  });

  it("a short drag that barely leaves the press point and comes back sends nothing at all", () => {
    const w = new World({ model: modelWithEdgeAt(98) });
    const at = clip("Z")(w);
    const h = hand(w, at);
    h.down();
    h.to(3);
    w.send({ type: "pointer.move", ...at });
    h.up(0);
    expect(shown(w)).toEqual([
      "update #1 item.move items=[Z] start=101f mode=new_track",
      "cancel #1 unchanged",
    ]);
    expect(w.engine.history).toEqual([]);
  });

  it("a pixel off the press point is still home, and a candidate wins only once it is nearer", () => {
    const w = new World({ model: modelWithEdgeAt(98) });
    const h = hand(w, clip("Z")(w));
    const start = (): unknown => {
      const m = w.state.mode;
      const c = m.name === "moving" ? m.tx.pending : null;
      return c !== null && c.name === "item.move" ? c.args.start : undefined;
    };
    h.down();
    h.to(40);
    h.to(-1);
    expect(start()).toBe(f(100));
    // 3 px toward the edge (5 px from it): the clip follows the pointer on the frame grid.
    h.to(-3);
    expect(start()).toBe(f(99));
    expect(w.state.guides).toEqual([]);
    // Halfway (4 px): the edge is as near as home, and it snaps.
    h.to(-4);
    expect(start()).toBe(f(98));
    expect(w.state.guides).toEqual([{ kind: "edge", t: f(98) }]);
    h.up(-4);
    expect(w.engine.history.map(fmtCommand)).toEqual([
      "item.move items=[Z] start=98f mode=new_track",
    ]);
  });

  it("a clip on the main track pressed, dragged past a neighbour and brought back stays put", () => {
    const w = new World();
    const h = hand(w, clip("B")(w));
    h.down();
    h.to(4);
    h.to(140);
    h.to(0);
    h.up(0);
    expect(shown(w).at(-1)).toBe("cancel #1 unchanged");
    expect(w.engine.history).toEqual([]);
  });

  it("a trim handle brought back to where it was pressed trims nothing", () => {
    const w = new World({ model: modelWithEdgeAt(98) });
    const h = hand(w, clip("Z", "start")(w));
    h.down();
    h.to(4);
    h.to(40);
    h.to(0);
    expect(w.state.guides).toEqual([]);
    h.up(0);
    expect(shown(w).at(-1)).toBe("cancel #1 unchanged");
    expect(w.engine.history).toEqual([]);
  });

  it("an end edge too: back at the press point it does not jump to the edge 8 px after it", () => {
    const w = new World({ model: modelWithEdgeAt(122) });
    const h = hand(w, clip("Z", "end")(w));
    h.down();
    h.to(-40);
    h.to(0);
    h.up(0);
    expect(shown(w).at(-1)).toBe("cancel #1 unchanged");
    expect(w.engine.history).toEqual([]);
  });

  it("a roll, a slide and a keyframe brought back commit nothing", () => {
    // Roll the cut between A and B (frame 30) with the playhead 2 frames after it.
    const roll = new World({ env: { tool: "roll", playhead: f(32) } });
    let h = hand(roll, clip("A", "end")(roll));
    h.down();
    h.to(-40);
    h.to(0);
    h.up(0);
    expect(roll.engine.history).toEqual([]);
    expect(shown(roll).at(-1)).toBe("cancel #1 unchanged");

    const slide = new World({ model: modelWithEdgeAt(98), env: { tool: "slide" } });
    h = hand(slide, clip("Z")(slide));
    h.down();
    h.to(40);
    h.to(0);
    h.up(0);
    expect(slide.engine.history).toEqual([]);
    expect(shown(slide).at(-1)).toBe("cancel #1 unchanged");

    // Y's first key is at frame 50; the playhead 2 frames before it.
    const keys = new World({ keyframes: true, env: { playhead: f(48) } });
    h = hand(keys, clip("Y", 0)(keys));
    h.down();
    h.to(40);
    h.to(0);
    h.up(0);
    expect(keys.engine.history).toEqual([]);
    expect(shown(keys).at(-1)).toBe("cancel #1 unchanged");
  });

  it("a drag that really moves still snaps and commits as before", () => {
    const w = new World({ model: modelWithEdgeAt(98) });
    const h = hand(w, clip("Z")(w));
    h.down();
    h.to(-6);
    h.up(-6);
    expect(shown(w)).toEqual([
      "update #1 item.move items=[Z] start=98f mode=new_track",
      "commit #1",
    ]);
  });
});
